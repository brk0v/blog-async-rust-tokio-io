use std::{error::Error, io, net::SocketAddr, time::Duration};

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::{
    net::{TcpListener, TcpStream},
    time::{Interval, interval},
};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    println!("listening on 127.0.0.1:8080");

    loop {
        let (stream, addr) = listener.accept().await?;
        println!("accepted connection from {addr}");

        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, addr).await {
                eprintln!("connection {addr} closed with error: {err}");
            }
        });
    }
}

async fn handle_connection(mut stream: TcpStream, peer: SocketAddr) -> io::Result<()> {
    stream.set_nodelay(true)?;

    println!("start serving {peer}");

    let (r, w) = stream.split();

    let mut framed_read = FramedRead::new(r, LengthDelimitedCodec::new());
    let mut framed_write = FramedWrite::new(w, LengthDelimitedCodec::new());

    let mut ticker = heartbeat(Duration::from_millis(100));

    let reader = async move {
        while let Some(msg) = framed_read.next().await {
            let msg = msg?;
            println!("received {msg:?} bytes from {peer}");
        }

        Ok::<_, io::Error>(())
    };

    let writer = async move {
        let mut i: u64 = 0;

        loop {
            ticker.tick().await;
            i += 1;
            if i == 100 {
                break;
            }

            let payload = format!("message from server: {i}\n").into_bytes();
            framed_write.send(Bytes::from(payload)).await?;
            println!("written frame: {i}");
        }

        Ok::<_, io::Error>(())
    };

    tokio::try_join!(reader, writer)?;
    Ok(())
}

fn heartbeat(period: Duration) -> Interval {
    let mut ticker = interval(period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker
}
