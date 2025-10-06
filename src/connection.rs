use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::{Buf, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::PollSender;

type Ack = oneshot::Sender<()>;
type Outgoing = (Bytes, Option<Ack>);

const READ_BUF_CAPACITY: usize = 16 * 1024; // typical TLS record size

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

pub struct Connection<T> {
    stream: T,
    read_buf: BytesMut,

    // read from IO
    inbound_buf: Option<Bytes>,
    tx: PollSender<Bytes>,

    // write to IO
    outbound_buf: Option<Outgoing>,
    rx: mpsc::Receiver<Outgoing>,

    read_state: ReadState,
    write_state: WriteState,
}

impl<T> Connection<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    pub fn new(stream: T) -> io::Result<(Self, mpsc::Sender<Outgoing>, mpsc::Receiver<Bytes>)> {
        let (downstream_tx, downstream_rx) = mpsc::channel::<Outgoing>(128);
        let (upstream_tx, upstream_rx) = mpsc::channel::<Bytes>(128);
        let upstream_tx = PollSender::new(upstream_tx);
        Ok((
            Self {
                stream,
                read_buf: BytesMut::with_capacity(READ_BUF_CAPACITY),
                inbound_buf: None,
                outbound_buf: None,
                tx: upstream_tx,
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
            unreachable!("bug poll read");
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

            if self.read_buf.capacity() < READ_BUF_CAPACITY {
                self.read_buf.reserve(READ_BUF_CAPACITY);
            }

            let size = ready!(tokio_util::io::poll_read_buf(
                Pin::new(&mut self.stream),
                cx,
                &mut self.read_buf
            ))?;

            if size == 0 {
                self.read_state = ReadState::Done;
                self.tx.close();
                return Poll::Ready(Ok(()));
            }

            self.inbound_buf = Some(self.read_buf.split().freeze());
        }
    }

    fn poll_write(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !matches!(self.write_state, WriteState::Writing) {
            unreachable!("bug poll_write");
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

            if let Some((buf, ack)) = self.outbound_buf.as_mut() {
                while !buf.is_empty() {
                    match ready!(Pin::new(&mut self.stream).poll_write(cx, buf)?) {
                        0 => {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::WriteZero,
                                "write zero bytes",
                            )));
                        }
                        size => buf.advance(size),
                    };
                }

                if let Some(ack) = ack.take() {
                    self.write_state
                        .set_flushing(Some(ack), FromFlushingTo::Writing);
                    // returning pending without registering a waker, so wake up manually
                    // more work could be awaiting
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }

                self.outbound_buf = None;
            }
        }
    }

    fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let WriteState::Flushing(state) = &mut self.write_state else {
            unreachable!("bug poll_flush");
        };

        ready!(Pin::new(&mut self.stream).poll_flush(cx))?;
        let (ack, next) = state;

        if let Some(ack) = ack.take() {
            _ = ack.send(()); // caller could drop the recv part
        }

        self.write_state = (*next).into();

        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !matches!(self.write_state, WriteState::ShuttingDown) {
            unreachable!("bug poll_shutdown");
        }

        ready!(Pin::new(&mut self.stream).poll_shutdown(cx))?;
        self.write_state = WriteState::Done;

        Poll::Ready(Ok(()))
    }
}

impl<T> Future for Connection<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
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
