//! An example to show how to use TcpStream.

use futures::channel::oneshot;
use monoio::{
    io::{AsyncReadRent, AsyncWriteRentExt},
    net::{TcpListener, TcpStream},
    Buildable, Driver,
};

fn main() {
    println!("Will run with LegacyDriver(you must enable legacy feature)");
    run::<monoio::LegacyDriver>();
}

fn run<D>()
where
    D: Buildable + Driver,
{
    const ADDRESS: &str = "127.0.0.1:50000";

    let (mut tx, rx) = oneshot::channel::<()>();
    let client_thread = std::thread::spawn(|| {
        monoio::start::<D, _>(async move {
            println!("[Client] Waiting for server ready");
            tx.cancellation().await;

            println!("[Client] Server is ready, will connect and send data");
            let mut conn = TcpStream::connect(ADDRESS)
                .await
                .expect("[Client] Unable to connect to server");
            let buf: Vec<u8> = vec![97; 10];
            let (r, _) = conn.write_all(buf).await;
            println!("[Client] Written {} bytes data and leave", r.unwrap());
        });
    });

    let server_thread = std::thread::spawn(|| {
        monoio::start::<D, _>(async move {
            let listener = TcpListener::async_bind(ADDRESS)
                .await
                .unwrap_or_else(|_| panic!("[Server] Unable to bind to {ADDRESS}"));
            println!("[Server] Bind ready");
            drop(rx);

            let (mut conn, _addr) = listener
                .accept()
                .await
                .expect("[Server] Unable to accept connection");
            println!("[Server] Accepted a new connection, will read form it");

            let buf = vec![0; 64];
            let (r, buf) = conn.read(buf).await;

            let read_len = r.unwrap();
            println!(
                "[Server] Read {} bytes data: {:?}",
                read_len,
                &buf[..read_len]
            );
        });
    });

    server_thread.join().unwrap();
    client_thread.join().unwrap();
}
