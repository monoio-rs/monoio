//! An echo example.
//!
//! Run the example and `nc 127.0.0.1 50002` in another shell.
//! All your input will be echoed out.

use monoio::{
    io::{
        poll_io::{AsyncReadExt, AsyncWriteExt},
        IntoPollIo,
    },
    net::{TcpListener, TcpStream},
};

#[monoio::main(driver = "fusion")]
async fn main() {
    let listener = TcpListener::async_bind("127.0.0.1:50002").await.unwrap();
    println!("listening");
    loop {
        let incoming = listener.accept().await;
        match incoming {
            Ok((stream, addr)) => {
                println!("accepted a connection from {addr}");
                monoio::spawn(echo(stream));
            }
            Err(e) => {
                println!("accepted connection failed: {e}");
                return;
            }
        }
    }
}

async fn echo(stream: TcpStream) -> std::io::Result<()> {
    // Convert completion-based io to poll-based io(which impl tokio::io)
    let mut stream = stream.into_poll_io()?;
    let mut buf: Vec<u8> = vec![0; 1024];
    let mut res;
    loop {
        // read
        res = stream.read(&mut buf).await?;
        if res == 0 {
            return Ok(());
        }

        // write all
        stream.write_all(&buf[0..res]).await?;
    }
}
