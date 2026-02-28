//! A loop accept example.
//!
//! Run the example and `nc 127.0.0.1 50002` in another shell.

use std::time::Duration;

use monoio::net::TcpListener;

#[monoio::main(driver = "fusion", enable_timer = true)]
async fn main() {
    let listener = TcpListener::async_bind("127.0.0.1:50002").await.unwrap();
    monoio::spawn(async {
        loop {
            monoio::time::sleep(Duration::from_secs(1)).await;
            println!("tik tok");
        }
    });
    println!("listening");
    loop {
        println!("waiting...");
        let incoming = listener.accept().await;
        match incoming {
            Ok((_, addr)) => {
                println!("accepted a connection from {addr}");
            }
            Err(e) => {
                println!("accepted connection failed: {e}");
                return;
            }
        }
    }
}
