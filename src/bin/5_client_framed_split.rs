use std::{io, time::Duration};

use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    sync::mpsc,
    time::sleep,
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

pub fn huge_ascii(n: usize) -> String {
    String::from_utf8(vec![b'a'; n]).expect("always valid UTF-8")
}

async fn handle<IO: AsyncRead + AsyncWrite + Unpin>(
    stream: IO,
    rx: mpsc::Receiver<Bytes>,
) -> io::Result<()> {
    let stream = Framed::new(stream, LengthDelimitedCodec::new());

    // https://docs.rs/futures/latest/futures/lock/struct.BiLock.html
    let (writer, reader) = stream.split();

    let reader = reader.try_for_each(|msg| async move {
        print!("{}", String::from_utf8_lossy(&msg));
        Ok(())
    });

    let writer = ReceiverStream::new(rx).map(Ok).forward(writer);

    tokio::try_join!(reader, writer)?;
    Ok(())
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    let (tx, rx) = mpsc::channel::<Bytes>(1024);

    tokio::spawn({
        let tx = tx.clone();
        async move {
            loop {
                tx.send(huge_ascii(100 * 1024).into()).await.unwrap(); // 100 KB message
                sleep(Duration::from_millis(10)).await;
            }
        }
    });

    let stream = TcpStream::connect(&addr).await?;
    stream.set_nodelay(true).unwrap();

    handle(stream, rx).await
}
