#![cfg(unix)]
use futures::future::try_join;
use monoio::{
    io::{AsyncReadRent, AsyncReadRentExt, AsyncWriteRent, AsyncWriteRentExt},
    net::{UnixListener, UnixStream},
};

#[monoio::test_all]
async fn accept_read_write() -> std::io::Result<()> {
    let dir = tempfile::Builder::new()
        .prefix("monoio-uds-tests")
        .tempdir()
        .unwrap();
    let sock_path = dir.path().join("connect.sock");

    let listener = UnixListener::async_bind(&sock_path).await?;

    let accept = listener.accept();
    let connect = UnixStream::connect(&sock_path);
    let ((mut server, _), client) = try_join(accept, connect).await?;

    // testing into_raw_fd and from_raw_fd
    #[cfg(unix)]
    use std::os::fd::{FromRawFd, IntoRawFd};
    #[cfg(unix)]
    let fd = client.into_raw_fd();
    #[cfg(unix)]
    let client =
        UnixStream::from_std(unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) }).unwrap();

    let mut client = client;
    let write_len = client.write_all(b"hello").await.0?;
    assert_eq!(write_len, 5);
    drop(client);

    let buf = Box::new([0u8; 5]);
    let (res, buf) = server.read_exact(buf).await;
    assert_eq!(res.unwrap(), 5);
    assert_eq!(&buf[..], b"hello");
    let len = server.read(buf).await.0?;
    assert_eq!(len, 0);
    Ok(())
}

#[monoio::test_all]
async fn shutdown() -> std::io::Result<()> {
    let dir = tempfile::Builder::new()
        .prefix("monoio-uds-tests")
        .tempdir()
        .unwrap();
    let sock_path = dir.path().join("connect.sock");

    let listener = UnixListener::async_bind(&sock_path).await?;

    let accept = listener.accept();
    let connect = UnixStream::connect(&sock_path);
    let ((mut server, _), mut client) = try_join(accept, connect).await?;

    // Shut down the client
    client.shutdown().await?;
    // Read from the server should return 0 to indicate the channel has been closed.
    let n = server.read(Box::new([0u8; 1])).await.0?;
    assert_eq!(n, 0);
    Ok(())
}

#[monoio::test_all]
async fn sync_bind_accept_read_write() -> std::io::Result<()> {
    let dir = tempfile::Builder::new()
        .prefix("monoio-uds-tests")
        .tempdir()
        .unwrap();
    let sock_path = dir.path().join("connect-sync.sock");

    let listener = UnixListener::bind(&sock_path)?;

    let accept = listener.accept();
    let connect = UnixStream::connect(&sock_path);
    let ((mut server, _), mut client) = try_join(accept, connect).await?;

    let write_len = client.write_all(b"hello").await.0?;
    assert_eq!(write_len, 5);
    drop(client);

    let buf = Box::new([0u8; 5]);
    let (res, buf) = server.read_exact(buf).await;
    assert_eq!(res.unwrap(), 5);
    assert_eq!(&buf[..], b"hello");
    let len = server.read(buf).await.0?;
    assert_eq!(len, 0);

    Ok(())
}
