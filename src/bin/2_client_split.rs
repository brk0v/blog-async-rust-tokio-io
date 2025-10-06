use std::{io, time::Duration};

use bytes::Bytes;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    time::sleep,
};

pub fn huge_ascii(n: usize) -> String {
    String::from_utf8(vec![b'a'; n]).expect("always valid UTF-8")
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    let (tx, mut rx) = mpsc::channel::<Bytes>(1024);

    tokio::spawn({
        let tx = tx.clone();
        async move {
            loop {
                tx.send(huge_ascii(100 * 1024).into()).await.unwrap(); // 100 KB message
                sleep(Duration::from_millis(10)).await;
            }
        }
    });

    let mut stream = TcpStream::connect(&addr).await?;
    stream.set_nodelay(true).unwrap();

    let mut read_buf = vec![0u8; 8192];

    let (mut r, mut w) = stream.split();

    let reader = async {
        loop {
            let n = r.read(&mut read_buf).await?;
            if n == 0 {
                // EOF - server closed connection
                eprintln!("Server closed the connection.");
                break;
            }
            print!("{}", String::from_utf8_lossy(&read_buf[..n]));
        }

        Ok::<_, io::Error>(())
    };

    let writer = async {
        while let Some(msg) = rx.recv().await {
            w.write_all(&msg).await?;
            w.flush().await?;
            println!("client's written");
        }

        Ok::<_, io::Error>(())
    };

    tokio::try_join!(reader, writer)?;
    Ok(())
}
