use monoio::{
    io::{self, AsyncReadRentExt, AsyncWriteRentExt, Splitable},
    net::{TcpListener, TcpStream},
};

#[monoio::test_all]
async fn echo_server() {
    const ITER: usize = 1024;

    let (tx, rx) = local_sync::oneshot::channel();

    let srv = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = srv.local_addr().unwrap();

    let msg = "foo bar baz";
    let iov_msg = "iovec_is_so_good";
    monoio::spawn(async move {
        let mut stream = TcpStream::connect(&addr).await.unwrap();

        let mut buf_vec_to_write: Option<Vec<Vec<u8>>> = Some(vec![
            iov_msg.as_bytes()[..9].into(),
            iov_msg.as_bytes()[9..].into(),
        ]);
        for _ in 0..ITER {
            // write
            assert!(stream.write_all(msg).await.0.is_ok());

            // read
            let buf = Box::new([0; 11]);
            let (res, buf) = stream.read_exact(buf).await;
            assert!(res.is_ok());
            assert_eq!(res.unwrap(), 11);
            assert_eq!(&buf[..], msg.as_bytes());

            // writev
            let buf_vec: monoio::buf::VecBuf = buf_vec_to_write.take().unwrap().into();
            let (res, buf_vec) = stream.write_vectored_all(buf_vec).await;
            let raw_vec: Vec<Vec<u8>> = buf_vec.into();
            assert!(res.is_ok());
            assert_eq!(res.unwrap(), iov_msg.len());
            buf_vec_to_write = Some(raw_vec);

            // readv
            let buf_vec: monoio::buf::VecBuf = vec![vec![0; 3], vec![0; iov_msg.len() - 3]].into();
            let (res, buf_vec) = stream.read_vectored_exact(buf_vec).await;
            assert!(res.is_ok());
            assert_eq!(res.unwrap(), iov_msg.len());
            let raw_vec: Vec<Vec<u8>> = buf_vec.into();
            assert_eq!(&raw_vec[0], &iov_msg.as_bytes()[..3]);
            assert_eq!(&raw_vec[1], &iov_msg.as_bytes()[3..]);
        }

        assert!(tx.send(()).is_ok());
    });

    let (stream, _) = srv.accept().await.unwrap();
    let (mut rd, mut wr) = stream.into_split();

    // todo fix these CI in windows
    #[cfg(not(windows))]
    {
        let n = io::copy(&mut rd, &mut wr).await.unwrap();
        assert_eq!(n, (ITER * (msg.len() + iov_msg.len())) as u64);

        assert!(rx.await.is_ok());
    }
}

#[monoio::test_all(timer_enabled = true)]
async fn rw_able() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let listener_addr = listener.local_addr().unwrap();

    monoio::select! {
        _ = monoio::time::sleep(std::time::Duration::from_millis(50)) => {},
        _ = listener.readable(false) => {
            panic!("unexpected readable");
        }
    }
    let mut active = TcpStream::connect(listener_addr).await.unwrap();

    assert!(active.writable(false).await.is_ok());
    assert!(listener.readable(false).await.is_ok());
    let (conn, _) = listener.accept().await.unwrap();
    monoio::select! {
        _ = monoio::time::sleep(std::time::Duration::from_millis(50)) => {},
        _ = conn.readable(false) => {
            panic!("unexpected readable");
        }
        _ = active.readable(false) => {
            panic!("unexpected readable");
        }
        _ = listener.readable(false) => {
            // even listener's inner readiness state is ready, we will check it again
            panic!("unexpected readable");
        }
    }
    let (res, _) = active.write_all("MSG").await;
    assert!(res.is_ok());
    assert!(conn.readable(false).await.is_ok());
}

//essentially the same as rw_able, but test split stream instead of original stream
#[monoio::test_all(timer_enabled = true)]
async fn split_rw_able() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let listener_addr = listener.local_addr().unwrap();

    monoio::select! {
        _ = monoio::time::sleep(std::time::Duration::from_millis(50)) => {},
        _ = listener.readable(false) => {
            panic!("unexpected readable");
        }
    }
    let (_, mut writer) = TcpStream::connect(listener_addr).await.unwrap().into_split();

    assert!(writer.writable(false).await.is_ok());
    assert!(listener.readable(false).await.is_ok());
    let (conn, _) = listener.accept().await.unwrap();
    let (reader, _) = conn.into_split();
    monoio::select! {
        _ = monoio::time::sleep(std::time::Duration::from_millis(50)) => {},
        _ = reader.readable(false) => {
            panic!("unexpected readable");
        }
        _ = listener.readable(false) => {
            // even listener's inner readiness state is ready, we will check it again
            panic!("unexpected readable");
        }
    }
    assert!(writer.writable(false).await.is_ok());
    let (res, _) = writer.write_all("MSG").await;
    assert!(res.is_ok());
    assert!(reader.readable(false).await.is_ok());
}

#[monoio::test_all]
async fn echo_tfo() {
    use std::net::SocketAddr;

    let bind_addr = "127.0.0.1:0".parse::<SocketAddr>().unwrap();
    let opts = monoio::net::ListenerOpts::default().tcp_fast_open(true);
    let listener = TcpListener::bind_with_config(bind_addr, &opts).unwrap();
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
