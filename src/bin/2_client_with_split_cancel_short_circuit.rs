use std::{io, time::Duration};

use bytes::Bytes;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    signal,
    sync::mpsc,
    time::sleep,
};
use tokio_util::sync::CancellationToken;

pub fn huge_ascii(n: usize) -> String {
    String::from_utf8(vec![b'a'; n]).expect("always valid UTF-8")
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let cancel_token = CancellationToken::new();

    tokio::spawn({
        let cancel_token = cancel_token.clone();
        async move {
            if signal::ctrl_c().await.is_ok() {
                cancel_token.cancel();
                println!("CTR_C");
            }
        }
    });

    let mut read_buf = vec![0u8; 8192];

    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    let (tx, mut rx) = mpsc::channel::<Bytes>(1024);
    tokio::spawn({
        let tx = tx.clone();
        async move {
            for _ in 1..100 {
                tx.send(huge_ascii(100 * 1024).into()).await.unwrap(); // 100 KB message
                tx.send(Bytes::from_static(b"\n")).await.unwrap(); // 100 KB message
                sleep(Duration::from_millis(10)).await;
            }
        }
    });
    drop(tx);

    let stream = TcpStream::connect(&addr).await?;
    stream.set_nodelay(true).unwrap();
    let (mut r, mut w) = stream.into_split();

    let (finish_tx, mut finish_rx) = mpsc::channel::<()>(1);

    let _guard = finish_tx.clone();
    let reader = async move {
        let _guard = _guard;

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

    let _guard = finish_tx.clone();
    let writer = async {
        let _guard = _guard;

        while let Some(msg) = rx.recv().await {
            w.write_all(&msg).await?;
            w.flush().await?;
        }

        Ok::<_, io::Error>(())
    };

    drop(finish_tx);
    let cancel_err = io::Error::other("canceled");

    let cancel = {
        async move {
            tokio::select! {
                _ = cancel_token.cancelled() => Err(cancel_err),
                _ = finish_rx.recv() => Ok(()),
            }
        }
    };

    tokio::try_join!(reader, writer, cancel)?;
    Ok(())
}
