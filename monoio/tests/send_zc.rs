/// Tests for zero-copy send (`IORING_OP_SEND_ZC`) via `TcpStream::write_zc`.
///
/// `write_zc` is gated behind `#[cfg(all(target_os = "linux", feature = "iouring"))]`.
/// On the legacy/poll-io driver the operation falls back to a regular `send(2)`,
/// so `test_all` exercises both paths where available.
#[cfg(all(target_os = "linux", feature = "iouring"))]
mod send_zc_tests {
    use monoio::{
        buf::IoBufMut,
        io::{AsyncReadRentExt, AsyncWriteRentExt},
        net::{TcpListener, TcpStream},
    };

    /// Helper: bind a listener on localhost with a random port and return it
    /// together with the bound address.
    fn tcp_listener() -> (TcpListener, std::net::SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        (listener, addr)
    }

    // -- basic functionality ------------------------------------------------

    /// Small message round-trip: write_zc on the client, read on the server.
    #[monoio::test_all]
    async fn send_zc_small_message() {
        const MSG: &[u8] = b"hello zero-copy";

        let (srv, addr) = tcp_listener();
        let (tx, rx) = local_sync::oneshot::channel::<()>();

        monoio::spawn(async move {
            let stream = TcpStream::connect(&addr).await.unwrap();
            let (res, _buf) = stream.write_zc(MSG.to_vec()).await;
            assert_eq!(res.unwrap(), MSG.len());
            drop(stream); // close connection so server read returns
            let _ = tx.send(());
        });

        let (mut conn, _) = srv.accept().await.unwrap();
        let buf = Vec::<u8>::with_capacity(MSG.len()).slice_mut(0..MSG.len());
        let (res, buf) = conn.read_exact(buf).await;
        res.unwrap();
        assert_eq!(&buf.into_inner(), MSG);
        let _ = rx.await;
    }

    /// Larger payload (64 KiB) — the size where zero-copy is most useful.
    #[monoio::test_all]
    async fn send_zc_large_payload() {
        const SIZE: usize = 64 * 1024;
        let payload: Vec<u8> = (0..SIZE).map(|i| (i % 251) as u8).collect();

        let (srv, addr) = tcp_listener();
        let (tx, rx) = local_sync::oneshot::channel::<()>();

        let send_payload = payload.clone();
        monoio::spawn(async move {
            let stream = TcpStream::connect(&addr).await.unwrap();
            let (res, _buf) = stream.write_zc(send_payload).await;
            assert_eq!(res.unwrap(), SIZE);
            drop(stream);
            let _ = tx.send(());
        });

        let (mut conn, _) = srv.accept().await.unwrap();
        let buf = Vec::<u8>::with_capacity(SIZE).slice_mut(0..SIZE);
        let (res, buf) = conn.read_exact(buf).await;
        res.unwrap();
        assert_eq!(buf.into_inner(), payload);
        let _ = rx.await;
    }

    // -- buffer ownership ---------------------------------------------------

    /// The buffer must be returned to the caller after the write completes.
    #[monoio::test_all]
    async fn send_zc_returns_buffer() {
        const MSG: &[u8] = b"buffer ownership test";

        let (srv, addr) = tcp_listener();
        let (tx, rx) = local_sync::oneshot::channel::<()>();

        monoio::spawn(async move {
            let (mut conn, _) = srv.accept().await.unwrap();
            let buf = Vec::<u8>::with_capacity(MSG.len()).slice_mut(0..MSG.len());
            let (res, _buf) = conn.read_exact(buf).await;
            res.unwrap();
            let _ = tx.send(());
        });

        let stream = TcpStream::connect(&addr).await.unwrap();
        let original = MSG.to_vec();
        let (res, returned_buf) = stream.write_zc(original).await;
        res.unwrap();
        // The returned buffer must still contain the original data.
        assert_eq!(&returned_buf, MSG);
        drop(stream);
        let _ = rx.await;
    }

    // -- multiple sequential sends ------------------------------------------

    /// Multiple consecutive `write_zc` calls on the same stream.
    #[monoio::test_all]
    async fn send_zc_multiple_writes() {
        const ITER: usize = 16;
        const MSG: &[u8] = b"repeated message\n";

        let (srv, addr) = tcp_listener();
        let (tx, rx) = local_sync::oneshot::channel::<()>();

        monoio::spawn(async move {
            let stream = TcpStream::connect(&addr).await.unwrap();
            for _ in 0..ITER {
                let (res, _) = stream.write_zc(MSG.to_vec()).await;
                assert_eq!(res.unwrap(), MSG.len());
            }
            drop(stream);
            let _ = tx.send(());
        });

        let (mut conn, _) = srv.accept().await.unwrap();
        let total = ITER * MSG.len();
        let buf = Vec::<u8>::with_capacity(total).slice_mut(0..total);
        let (res, buf) = conn.read_exact(buf).await;
        res.unwrap();
        let received = buf.into_inner();
        for chunk in received.chunks(MSG.len()) {
            assert_eq!(chunk, MSG);
        }
        let _ = rx.await;
    }

    // -- zero-length send ---------------------------------------------------

    /// Sending an empty buffer should succeed with 0 bytes written.
    #[monoio::test_all]
    async fn send_zc_empty_buffer() {
        let (srv, addr) = tcp_listener();
        let (tx, rx) = local_sync::oneshot::channel::<()>();

        monoio::spawn(async move {
            let (conn, _) = srv.accept().await.unwrap();
            // Keep connection alive until client is done.
            let _ = rx.await;
            drop(conn);
        });

        let stream = TcpStream::connect(&addr).await.unwrap();
        let (res, buf) = stream.write_zc(Vec::<u8>::new()).await;
        // Empty send should succeed (or return 0).
        let n = res.unwrap();
        assert_eq!(n, 0);
        assert!(buf.is_empty());
        let _ = tx.send(());
    }

    // -- echo round-trip ----------------------------------------------------

    /// Full echo: client writes via write_zc, server echoes back via
    /// regular write, client verifies received data.
    #[monoio::test_all]
    async fn send_zc_echo() {
        const MSG: &[u8] = b"echo via zero-copy send";

        let (srv, addr) = tcp_listener();
        let (tx, rx) = local_sync::oneshot::channel::<()>();

        monoio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();

            // Send via write_zc
            let (res, _) = stream.write_zc(MSG.to_vec()).await;
            assert_eq!(res.unwrap(), MSG.len());

            // Read echo back
            let buf = Vec::<u8>::with_capacity(MSG.len()).slice_mut(0..MSG.len());
            let (res, buf) = stream.read_exact(buf).await;
            res.unwrap();
            assert_eq!(&buf.into_inner(), MSG);

            let _ = tx.send(());
        });

        let (mut conn, _) = srv.accept().await.unwrap();
        // Read what the client sent
        let buf = Vec::<u8>::with_capacity(MSG.len()).slice_mut(0..MSG.len());
        let (res, buf) = conn.read_exact(buf).await;
        res.unwrap();
        let received = buf.into_inner();
        assert_eq!(&received, MSG);

        // Echo it back with a normal write
        let (res, _) = conn.write_all(received).await;
        res.unwrap();

        let _ = rx.await;
    }

    // -- mixed write_zc and write_all --------------------------------------

    /// Interleave write_zc and regular write_all on the same stream.
    #[monoio::test_all]
    async fn send_zc_mixed_with_regular_write() {
        const ZC_MSG: &[u8] = b"zero-copy-part|";
        const REG_MSG: &[u8] = b"regular-part|";

        let (srv, addr) = tcp_listener();
        let (tx, rx) = local_sync::oneshot::channel::<()>();

        monoio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();

            // Alternate between write_zc and write_all
            let (res, _) = stream.write_zc(ZC_MSG.to_vec()).await;
            assert_eq!(res.unwrap(), ZC_MSG.len());

            let (res, _) = stream.write_all(REG_MSG).await;
            res.unwrap();

            let (res, _) = stream.write_zc(ZC_MSG.to_vec()).await;
            assert_eq!(res.unwrap(), ZC_MSG.len());

            let (res, _) = stream.write_all(REG_MSG).await;
            res.unwrap();

            drop(stream);
            let _ = tx.send(());
        });

        let (mut conn, _) = srv.accept().await.unwrap();
        let total = 2 * (ZC_MSG.len() + REG_MSG.len());
        let buf = Vec::<u8>::with_capacity(total).slice_mut(0..total);
        let (res, buf) = conn.read_exact(buf).await;
        res.unwrap();
        let received = buf.into_inner();

        let mut expected = Vec::with_capacity(total);
        for _ in 0..2 {
            expected.extend_from_slice(ZC_MSG);
            expected.extend_from_slice(REG_MSG);
        }
        assert_eq!(received, expected);

        let _ = rx.await;
    }

    // -- static buffer ------------------------------------------------------

    /// write_zc accepts `&'static [u8]` (which implements IoBuf).
    #[monoio::test_all]
    async fn send_zc_static_slice() {
        const MSG: &[u8] = b"static slice send";

        let (srv, addr) = tcp_listener();
        let (tx, rx) = local_sync::oneshot::channel::<()>();

        monoio::spawn(async move {
            let (mut conn, _) = srv.accept().await.unwrap();
            let buf = Vec::<u8>::with_capacity(MSG.len()).slice_mut(0..MSG.len());
            let (res, buf) = conn.read_exact(buf).await;
            res.unwrap();
            assert_eq!(&buf.into_inner(), MSG);
            let _ = tx.send(());
        });

        let stream = TcpStream::connect(&addr).await.unwrap();
        let (res, returned) = stream.write_zc(MSG).await;
        assert_eq!(res.unwrap(), MSG.len());
        // The returned reference should be identical.
        assert_eq!(returned, MSG);
        drop(stream);
        let _ = rx.await;
    }
}
