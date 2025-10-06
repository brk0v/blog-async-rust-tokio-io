use socket2::Socket;
use std::io;
use std::net::TcpStream as StdTcpStream;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::sleep;

#[tokio::main]
async fn main() -> io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    println!("Listening");

    loop {
        let (stream, peer) = listener.accept().await?;
        println!("New connection from {peer}");

        // Convert tokio stream -> std stream to tweak with socket2.
        let std_stream: StdTcpStream = stream.into_std()?;

        // Wrap with socket2 and set SO_RCVBUF = 1 to
        let s2 = Socket::from(std_stream);
        s2.set_recv_buffer_size(1)?;
        s2.set_nodelay(true)?;
        let effective = s2.recv_buffer_size()?;
        println!("SO_RCVBUF requested = 1, effective = {effective} bytes");

        // Convert back to std, then to tokio.
        let std_stream: StdTcpStream = s2.into();
        std_stream.set_nonblocking(true)?;
        let stream = TcpStream::from_std(std_stream)?;

        tokio::spawn(async move {
            if let Err(e) = handle_client(stream).await {
                eprintln!("Connection error: {e}");
            }
        });
    }
}

async fn handle_client(mut stream: TcpStream) -> std::io::Result<()> {
    println!("start serving");

    let mut i = 0;
    loop {
        // Don't read any data to emulate backpressure.
        // let mut buf = [0u8; 1024];
        // let n = stream.read(&mut buf).await?;
        // if n == 0 {
        //     break;
        // }
        // println!("Received {n} bytes");

        i += 1;
        sleep(Duration::from_millis(10)).await;

        stream
            .write_all(format!("message from server: {i}\n").as_bytes())
            .await?;
        stream.flush().await?;
        println!("written: {i}");
    }

    //Ok(())
}
