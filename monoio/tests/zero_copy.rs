#[cfg(all(target_os = "linux", feature = "splice"))]
#[monoio::test_all]
async fn zero_copy_for_tcp() {
    use monoio::{
        buf::IoBufMut,
        io::{zero_copy, AsyncReadRentExt, AsyncWriteRentExt, Splitable},
        net::TcpStream,
    };

    const MSG: &[u8] = b"copy for split";
    let srv = monoio::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let (mut c_tx, mut c_rx) = local_sync::oneshot::channel::<()>();
    let addr = srv.local_addr().unwrap();
    monoio::spawn(async move {
        let stream = TcpStream::connect(&addr).await.unwrap();
        let (mut rx, mut tx) = stream.into_split();
        tx.write_all(MSG).await.0.unwrap();
        let buf = Vec::<u8>::with_capacity(MSG.len()).slice_mut(0..MSG.len());
        let (res, buf) = rx.read_exact(buf).await;
        let buf = buf.into_inner();
        res.unwrap();
        assert_eq!(&buf, MSG);
        c_rx.close();
    });
    let (conn, _) = srv.accept().await.unwrap();
    let (mut rx, mut tx) = conn.into_split();
    assert_eq!(zero_copy(&mut rx, &mut tx).await.unwrap(), MSG.len() as u64);
    c_tx.closed().await;
}

#[cfg(all(target_os = "linux", feature = "splice"))]
#[monoio::test_all]
async fn splice_file_to_pipe() {
    use monoio::{fs::File, io::splice::SpliceSource, net::unix::new_pipe};

    let dir = tempfile::Builder::new()
        .prefix("monoio-splice-tests")
        .tempdir()
        .unwrap();
    let file_path = dir.path().join("splice_test.txt");
    let content = b"Hello, splice!";
    std::fs::write(&file_path, content).unwrap();

    let mut file = File::open(&file_path).await.unwrap();
    let (_r, mut w) = new_pipe().unwrap();

    let n = file
        .splice_to_pipe(&mut w, content.len() as u32)
        .await
        .unwrap();
    assert_eq!(n as usize, content.len());
}

#[cfg(all(target_os = "linux", feature = "splice"))]
#[monoio::test_all]
async fn zero_copy_for_uds() {
    use monoio::{
        buf::IoBufMut,
        io::{zero_copy, AsyncReadRentExt, AsyncWriteRentExt, Splitable},
        net::UnixStream,
    };

    const MSG: &[u8] = b"copy for split";
    let dir = tempfile::Builder::new()
        .prefix("monoio-uds-tests")
        .tempdir()
        .unwrap();
    let sock_path = dir.path().join("zero_copy.sock");
    let srv = monoio::net::UnixListener::bind(&sock_path).unwrap();
    let (mut c_tx, mut c_rx) = local_sync::oneshot::channel::<()>();
    monoio::spawn(async move {
        let stream = UnixStream::connect(&sock_path).await.unwrap();
        let (mut rx, mut tx) = stream.into_split();
        tx.write_all(MSG).await.0.unwrap();
        let buf = Vec::<u8>::with_capacity(MSG.len()).slice_mut(0..MSG.len());
        let (res, buf) = rx.read_exact(buf).await;
        let buf = buf.into_inner();
        res.unwrap();
        assert_eq!(&buf, MSG);
        c_rx.close();
    });
    let (conn, _) = srv.accept().await.unwrap();
    let (mut rx, mut tx) = conn.into_split();
    assert_eq!(zero_copy(&mut rx, &mut tx).await.unwrap(), MSG.len() as u64);
    c_tx.closed().await;
}
