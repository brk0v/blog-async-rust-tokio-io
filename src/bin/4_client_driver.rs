use std::{error::Error, time::Duration};

use bytes::Bytes;
use driver::connection::Connection;
use tokio::{io::BufStream, net::TcpStream, time::sleep};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let stream = TcpStream::connect("127.0.0.1:8080").await?;
    let stream = BufStream::new(stream);

    let (conn, tx, mut rx) = Connection::new(stream)?;

    tokio::spawn(async move {
        if let Err(err) = conn.await {
            panic!("err: {err}");
        }
    });

    tokio::spawn(async move {
        for i in 1..100 {
            let s = format!("client message: {i}\n");
            tx.send((Bytes::from(s), None)).await.unwrap();

            // Send a multiple messages
            // for (j, sender) in tx.reserve_many(10).await.unwrap().enumerate() {
            //     let s = format!("client message: {i}:{j}\n");
            //     if j < 9 {
            //         sender.send((Bytes::from(s), None));
            //     } else {
            //         let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            //         sender.send((Bytes::from(s), Some(ack_tx)));
            //         _ = ack_rx.await.unwrap();
            //     }
            // }

            sleep(Duration::from_millis(100)).await;
        }

        tx.send((Bytes::from_static(b"\n"), None)).await.unwrap();
        drop(tx);
    });

    while let Some(buf) = rx.recv().await {
        println!("recv from server: {:?}", buf);
    }

    Ok(())
}
