use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::{fmt, io};

use futures::{Sink, Stream};
use pin_project::pin_project;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::{Decoder, Encoder, Framed};
use tokio_util::sync::PollSender;

type Ack = oneshot::Sender<()>;
type Outgoing<T> = (T, Option<Ack>);

#[derive(Debug, Default)]
enum ReadState {
    #[default]
    Reading,
    Done,
}

#[derive(Debug, Default)]
enum WriteState {
    #[default]
    Writing,
    Flushing((Option<Ack>, FromFlushingTo)),
    ShuttingDown,
    Done,
}

#[derive(Debug, Clone, Copy)]
enum FromFlushingTo {
    Writing,
    ShuttingDown,
}

impl From<FromFlushingTo> for WriteState {
    fn from(status: FromFlushingTo) -> Self {
        match status {
            FromFlushingTo::Writing => Self::Writing,
            FromFlushingTo::ShuttingDown => Self::ShuttingDown,
        }
    }
}

impl WriteState {
    fn set_flushing(&mut self, ack: Option<oneshot::Sender<()>>, next: FromFlushingTo) {
        if !matches!(self, WriteState::Writing) {
            unreachable!("bug set_flushing");
        }

        *self = WriteState::Flushing((ack, next));
    }
}

#[pin_project]
pub struct ConnectionFramed<T, C, In, Out>
where
    T: AsyncRead + AsyncWrite + Unpin,
    C: Decoder<Item = In, Error = io::Error> + Encoder<Out, Error = io::Error>,
    In: Send + fmt::Debug,
    Out: Send + fmt::Debug,
{
    #[pin]
    stream: Framed<T, C>,

    // read from IO
    inbound_buf: Option<In>,
    tx: PollSender<In>,

    // write to IO
    outbound_buf: Option<Outgoing<Out>>,
    rx: mpsc::Receiver<Outgoing<Out>>,

    read_state: ReadState,
    write_state: WriteState,
}

impl<T, C, In, Out> ConnectionFramed<T, C, In, Out>
where
    T: AsyncRead + AsyncWrite + Unpin,
    C: Decoder<Item = In, Error = io::Error> + Encoder<Out, Error = io::Error>,
    In: Send + fmt::Debug,
    Out: Send + fmt::Debug,
{
    pub fn new(
        stream: T,
        codec: C,
    ) -> io::Result<(Self, mpsc::Sender<Outgoing<Out>>, mpsc::Receiver<In>)> {
        let framed = Framed::new(stream, codec);
        let (downstream_tx, downstream_rx) = mpsc::channel(128);
        let (upstream_tx, upstream_rx) = mpsc::channel(128);
        let upstream_tx = PollSender::new(upstream_tx);

        Ok((
            Self {
                stream: framed,
                inbound_buf: None,
                tx: upstream_tx,
                outbound_buf: None,
                rx: downstream_rx,
                read_state: ReadState::Reading,
                write_state: WriteState::Writing,
            },
            downstream_tx,
            upstream_rx,
        ))
    }

    fn poll_read(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !matches!(self.read_state, ReadState::Reading) {
            unreachable!("poll_read called in invalid state");
        }

        loop {
            if self.inbound_buf.is_some() {
                ready!(
                    self.tx
                        .poll_reserve(cx)
                        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Channel closed"))?
                );
                self.tx
                    .send_item(
                        self.inbound_buf.take().expect("buffered message missing"), /* safe */
                    )
                    .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Channel closed"))?;
            }

            let mut stream = Pin::new(&mut self.stream);
            match ready!(Stream::poll_next(stream.as_mut(), cx)) {
                Some(Ok(item)) => {
                    self.inbound_buf = Some(item);
                }
                Some(Err(err)) => {
                    return Poll::Ready(Err(err));
                }
                None => {
                    self.read_state = ReadState::Done;
                    self.tx.close();
                    return Poll::Ready(Ok(()));
                }
            }
        }
    }

    fn poll_write(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !matches!(self.write_state, WriteState::Writing) {
            unreachable!("poll_write called in invalid state");
        }

        loop {
            if self.outbound_buf.is_none() {
                let msg = match self.rx.poll_recv(cx) {
                    Poll::Ready(Some(msg)) => msg,
                    Poll::Ready(None) => {
                        self.write_state
                            .set_flushing(None, FromFlushingTo::ShuttingDown);
                        return Poll::Ready(Ok(()));
                    }
                    Poll::Pending => {
                        self.write_state.set_flushing(None, FromFlushingTo::Writing);
                        return Poll::Pending;
                    }
                };
                self.outbound_buf = Some(msg);
            }

            if let Some((msg, ack)) = self.outbound_buf.take() {
                let mut stream = Pin::new(&mut self.stream);
                match Sink::poll_ready(stream.as_mut(), cx)? {
                    Poll::Ready(()) => {
                        Sink::start_send(stream.as_mut(), msg)?;

                        if let Some(ack) = ack {
                            self.write_state
                                .set_flushing(Some(ack), FromFlushingTo::Writing);
                            cx.waker().wake_by_ref();
                            return Poll::Pending;
                        }
                    }
                    Poll::Pending => {
                        self.outbound_buf = Some((msg, ack));
                        return Poll::Pending;
                    }
                }
            }
        }
    }

    fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let WriteState::Flushing(state) = &mut self.write_state else {
            unreachable!("poll_flush called in invalid state");
        };

        let mut stream = Pin::new(&mut self.stream);
        match ready!(Sink::poll_flush(stream.as_mut(), cx)) {
            Ok(()) => {
                let (ack, next) = state;

                if let Some(ack) = ack.take() {
                    _ = ack.send(()); // caller could drop the recv part
                }

                self.write_state = (*next).into();
                Poll::Ready(Ok(()))
            }
            Err(err) => Poll::Ready(Err(err)),
        }
    }

    fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !matches!(self.write_state, WriteState::ShuttingDown) {
            unreachable!("poll_shutdown called in invalid state");
        }

        let mut stream = Pin::new(&mut self.stream);
        match ready!(Sink::poll_close(stream.as_mut(), cx)) {
            Ok(()) => {
                self.write_state = WriteState::Done;
                Poll::Ready(Ok(()))
            }
            Err(err) => Poll::Ready(Err(err)),
        }
    }
}

impl<T, C, In, Out> std::future::Future for ConnectionFramed<T, C, In, Out>
where
    T: AsyncRead + AsyncWrite + Unpin,
    C: Decoder<Item = In, Error = io::Error> + Encoder<Out, Error = io::Error>,
    In: Send + fmt::Debug,
    Out: Send + fmt::Debug,
{
    type Output = Result<(), io::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if matches!(self.read_state, ReadState::Reading) {
            _ = self.poll_read(cx)?;
        }

        if matches!(self.write_state, WriteState::Writing) {
            _ = self.poll_write(cx)?;
        }

        if matches!(self.write_state, WriteState::Flushing(_)) {
            _ = self.poll_flush(cx)?;
        }

        if matches!(self.write_state, WriteState::ShuttingDown) {
            _ = self.poll_shutdown(cx)?;
        }

        if matches!(self.read_state, ReadState::Done)
            && matches!(self.write_state, WriteState::Done)
        {
            return Poll::Ready(Ok(()));
        }

        Poll::Pending
    }
}
