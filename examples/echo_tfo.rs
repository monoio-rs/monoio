use std::net::SocketAddr;

use monoio::{
    io::{AsyncReadRentExt, AsyncWriteRentExt},
    net::{TcpListener, TcpStream},
};

#[monoio::main]
async fn main() {
    let bind_addr = "127.0.0.1:11990".parse::<SocketAddr>().unwrap();
    let opts = monoio::net::ListenerOpts::default().tcp_fast_open(true);
    let listener = TcpListener::async_bind_with_config(bind_addr, &opts)
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = local_sync::oneshot::channel();
    monoio::spawn(async move {
        let (mut socket, active_addr) = listener.accept().await.unwrap();
        socket.read_exact(vec![0; 2]).await.0.unwrap();
        assert!(tx.send(active_addr).is_ok());
    });
    let opts = monoio::net::TcpConnectOpts::default().tcp_fast_open(true);
    let mut active = TcpStream::connect_addr_with_config(addr, &opts)
        .await
        .unwrap();
    active.write_all(b"hi").await.0.unwrap();
    let active_addr = rx.await.unwrap();
    assert_eq!(active.local_addr().unwrap(), active_addr);
}
