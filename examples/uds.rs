//! A example to show how to use UnixStream.
#[cfg(unix)]
use {
    local_sync::oneshot::channel,
    monoio::{
        io::{AsyncReadRent, AsyncWriteRentExt},
        net::{UnixListener, UnixStream},
    },
};

const ADDRESS: &str = "/tmp/monoio-unix-test.sock";

#[cfg(unix)]
#[monoio::main]
async fn main() {
    let (mut tx, rx) = channel::<()>();

    monoio::spawn(async move {
        tx.closed().await;
        let mut client = UnixStream::connect(ADDRESS).await.unwrap();
        let buf = "hello";
        let (ret, buf) = client.write_all(buf).await;
        ret.unwrap();
        println!("write {} bytes: {buf:?}", buf.len());
    });

    std::fs::remove_file(ADDRESS).ok();
    let listener = UnixListener::async_bind(ADDRESS).await.unwrap();
    println!("listening on {ADDRESS:?}");
    drop(rx);
    let (mut conn, addr) = listener.accept().await.unwrap();
    println!("accepted a new connection from {addr:?}");
    let buf = Vec::with_capacity(1024);
    let (ret, buf) = conn.read(buf).await;
    ret.unwrap();
    println!("read {} bytes: {buf:?}", buf.len());

    // clear the socket file
    drop(listener);
    std::fs::remove_file(ADDRESS).ok();
}

#[cfg(windows)]
#[monoio::main]
async fn main() {}
