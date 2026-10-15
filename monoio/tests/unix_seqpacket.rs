#[cfg(target_os = "linux")]
#[monoio::test_all]
async fn test_seqpacket() -> std::io::Result<()> {
    use monoio::net::unix::{UnixSeqpacket, UnixSeqpacketListener};

    let dir = tempfile::Builder::new()
        .prefix("monoio-unix-seqpacket-tests")
        .tempdir()
        .unwrap();
    let sock_path = dir.path().join("seqpacket.sock");

    let listener = UnixSeqpacketListener::async_bind(&sock_path).await.unwrap();
    monoio::spawn(async move {
        let (conn, _addr) = listener.accept().await.unwrap();
        let (res, buf) = conn.recv(vec![0; 100]).await;
        assert_eq!(res.unwrap(), 5);
        assert_eq!(buf, b"hello");
    });
    let conn = UnixSeqpacket::connect(&sock_path).await.unwrap();
    conn.send(b"hello").await.0.unwrap();
    Ok(())
}

#[cfg(target_os = "linux")]
#[monoio::test_all]
async fn test_seqpacket_sync_bind() -> std::io::Result<()> {
    use monoio::net::unix::{UnixSeqpacket, UnixSeqpacketListener};

    let dir = tempfile::Builder::new()
        .prefix("monoio-unix-seqpacket-tests")
        .tempdir()
        .unwrap();
    let sock_path = dir.path().join("seqpacket-sync.sock");

    let listener = UnixSeqpacketListener::bind(&sock_path).unwrap();
    monoio::spawn(async move {
        let (conn, _addr) = listener.accept().await.unwrap();
        let (res, buf) = conn.recv(vec![0; 100]).await;
        assert_eq!(res.unwrap(), 5);
        assert_eq!(buf, b"hello");
    });
    let conn = UnixSeqpacket::connect(&sock_path).await.unwrap();
    conn.send(b"hello").await.0.unwrap();
    Ok(())
}
