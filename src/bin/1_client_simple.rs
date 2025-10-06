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
                tx.send(huge_ascii(100 * 1024 /* 100 KB message */).into())
                    .await
                    .unwrap();
                sleep(Duration::from_millis(10)).await;
            }
        }
    });

    let mut stream = TcpStream::connect(&addr).await?;
    stream.set_nodelay(true).unwrap();

    let mut read_buf = vec![0u8; 8192];

    loop {
        tokio::select! {
            Some(msg) = rx.recv() => {
                stream.write_all(&msg).await?;
                stream.flush().await?;
                println!("client's written");
            }

            res = stream.read(&mut read_buf) => {
                let n = res?;
                if n == 0 {
                    // EOF - server closed connection
                    eprintln!("Server closed the connection.");
                    break; //exit
                }
                print!("{}", String::from_utf8_lossy(&read_buf[..n]));
            }

            else => break
        }
    }

    Ok(())
}
