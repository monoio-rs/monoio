#[cfg(unix)]
use std::{
    io::{Read, Write},
    sync::mpsc::channel,
    time::Duration,
};

#[cfg(unix)]
use monoio::io::AsyncReadRentExt;

// This test is used to prove the runtime can close the cancelled(but failed to cancel) op's fd
// result.
// 1. accept(push accept op) and poll the future to Pending
// 2. spawn another thread to connect the listener(will start connecting after task drop)
// 3. cancel(drop) the accept task
// 4. spin for a while to delay the iouring enter which submit the cancel op
// 5. the other thread should be able to get a connection
// 6. if the other thread can read eof, then it can prove the runtime close the fd correctly
// 7. if the read blocked, then the runtime failed to close the fd
#[cfg(unix)]
#[cfg(feature = "async-cancel")]
#[monoio::test_all(timer_enabled = true)]
async fn test_fd_leak_cancel_fail() {
    // step 1 and 2
    let listener = monoio::net::TcpListener::async_bind("127.0.0.1:0")
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let mut incoming = listener.accept();
    let fut = unsafe { std::pin::Pin::new_unchecked(&mut incoming) };
    assert!(monoio::select! {
        result = fut => Ok(result),
        _ = monoio::time::sleep(Duration::from_millis(200)) => Err(()),
    }
    .is_err());

    // step 2
    let (tx1, rx1) = channel::<()>();
    let (tx2, rx2) = channel::<()>();
    std::thread::spawn(move || {
        rx1.recv().unwrap();
        // step 5
        let mut conn = std::net::TcpStream::connect(addr).unwrap();
        tx2.send(()).unwrap();
        let mut buf = [0u8; 1];
        conn.write_all(&buf).unwrap();
        // step 6
        let ret = conn.read(&mut buf[..]);
        assert!(
            matches!(ret, Ok(0))
                || matches!(ret, Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset)
        );
        tx2.send(()).unwrap();
    });

    // step 3: cancel the accept op but not submit the cancel op
    drop(incoming);
    tx1.send(()).unwrap();
    // step 4: block the thread with sync channel
    rx2.recv().unwrap();
    // step 7: wait for 1 second to make sure the runtime can close the fd
    monoio::time::sleep(Duration::from_secs(1)).await;

    if rx2.try_recv().is_ok() {
        // With iouring, the fd is accepted and closed by the runtime.
        // So here it will return.
        return;
    }
    // With legacy driver, the accept syscall is not executed.
    // So we can accept now and check if it is the connection established by the other thread.
    // We can read 1 byte to check if it is zero. Then we close the fd and wait for the other
    // thread.
    // So we can prove the connection at server side is either not accepted or closed by the
    // runtime.
    let (mut conn, _) = listener.accept().await.unwrap();
    let buf = vec![1; 1];
    let (r, buf) = conn.read_exact(buf).await;
    assert_eq!(r.unwrap(), 1);
    assert_eq!(buf[0], 0);
    drop(conn);
    rx2.recv_timeout(Duration::from_secs(1)).unwrap();
}

// This test is used to prove the runtime try best to cancel pending op when op is dropped.
#[cfg(unix)]
#[cfg(feature = "async-cancel")]
#[monoio::test_all(timer_enabled = true)]
async fn test_fd_leak_try_cancel() {
    let listener = monoio::net::TcpListener::async_bind("127.0.0.1:0")
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = listener.accept();
    assert!(monoio::select! {
        result = incoming => Ok(result),
        _ = monoio::time::sleep(Duration::from_millis(200)) => Err(()),
    }
    .is_err());
    // The future is dropped now and the cancel op is pushed.
    monoio::time::sleep(Duration::from_millis(200)).await;
    let (tx, rx) = channel::<()>();
    std::thread::spawn(move || {
        let mut conn = std::net::TcpStream::connect(addr).unwrap();
        let buf = [0u8; 1];
        conn.write_all(&buf).unwrap();
        tx.send(()).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut conn = listener.accept().await.unwrap().0;
    let buf = vec![1; 1];
    let (r, buf) = conn.read_exact(buf).await;
    assert_eq!(r.unwrap(), 1);
    assert_eq!(buf[0], 0);
}
