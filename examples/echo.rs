//! An echo example.
//!
//! Run the example and `nc 127.0.0.1 50002` in another shell.
//! All your input will be echoed out.

use monoio::{
    io::{AsyncReadRent, AsyncWriteRentExt},
    net::{TcpListener, TcpStream},
};

#[monoio::main(driver = "fusion")]
async fn main() {
    // tracing_subscriber::fmt().with_max_level(tracing::Level::TRACE).init();
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

async fn echo(mut stream: TcpStream) -> std::io::Result<()> {
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut res;
    loop {
        // read
        (res, buf) = stream.read(buf).await;
        if res? == 0 {
            return Ok(());
        }

        // write all
        (res, buf) = stream.write_all(buf).await;
        res?;

        // clear
        buf.clear();
    }
}
