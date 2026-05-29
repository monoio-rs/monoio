use monoio::net::udp::UdpSocket;

#[monoio::test_all]
async fn connect() {
    const MSG: &str = "foo bar baz";

    let passive = UdpSocket::async_bind("127.0.0.1:0").await.unwrap();
    let passive_addr = passive.local_addr().unwrap();

    let active = UdpSocket::async_bind("127.0.0.1:0").await.unwrap();
    let active_addr = active.local_addr().unwrap();

    active.connect(passive_addr).await.unwrap();
    active.send(MSG).await.0.unwrap();

    let (res, buffer) = passive.recv(Vec::with_capacity(20)).await;
    res.unwrap();
    assert_eq!(MSG.as_bytes(), &buffer);
    assert_eq!(active.local_addr().unwrap(), active_addr);
    assert_eq!(active.peer_addr().unwrap(), passive_addr);
}

#[monoio::test_all]
async fn sync_bind_connect() {
    const MSG: &str = "sync bind udp";

    let passive = UdpSocket::bind("127.0.0.1:0").unwrap();
    let passive_addr = passive.local_addr().unwrap();

    let active = UdpSocket::bind("127.0.0.1:0").unwrap();
    active.connect(passive_addr).await.unwrap();
    active.send(MSG).await.0.unwrap();

    let (res, buffer) = passive.recv(Vec::with_capacity(32)).await;
    res.unwrap();
    assert_eq!(MSG.as_bytes(), &buffer);
}

#[monoio::test_all]
async fn send_to() {
    const MSG: &str = "foo bar baz";

    macro_rules! must_success {
        ($r: expr, $expect_addr: expr) => {
            let res = $r;
            assert_eq!(res.0.unwrap().1, $expect_addr);
            assert_eq!(res.1, MSG.as_bytes());
        };
    }

    let passive1 = UdpSocket::async_bind("127.0.0.1:0").await.unwrap();
    let passive1_addr = passive1.local_addr().unwrap();

    let passive01 = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let passive01_addr = passive01.local_addr().unwrap();

    let passive2 = UdpSocket::async_bind("127.0.0.1:0").await.unwrap();
    let passive2_addr = passive2.local_addr().unwrap();

    let passive3 = UdpSocket::async_bind("127.0.0.1:0").await.unwrap();
    let passive3_addr = passive3.local_addr().unwrap();

    let active = UdpSocket::async_bind("127.0.0.1:0").await.unwrap();
    let active_addr = active.local_addr().unwrap();

    active.send_to(MSG, passive01_addr).await.0.unwrap();
    active.send_to(MSG, passive1_addr).await.0.unwrap();
    active.send_to(MSG, passive2_addr).await.0.unwrap();
    active.send_to(MSG, passive3_addr).await.0.unwrap();

    must_success!(passive1.recv_from(vec![0; 20]).await, active_addr);
    must_success!(passive2.recv_from(vec![0; 20]).await, active_addr);
    must_success!(passive3.recv_from(vec![0; 20]).await, active_addr);
}

#[monoio::test_all(timer_enabled = true)]
async fn rw_able() {
    const MSG: &str = "foo bar baz";

    let passive = UdpSocket::async_bind("127.0.0.1:0").await.unwrap();
    let passive_addr = passive.local_addr().unwrap();

    let active = UdpSocket::async_bind("127.0.0.1:0").await.unwrap();

    assert!(active.writable(false).await.is_ok());
    monoio::select! {
        _ = monoio::time::sleep(std::time::Duration::from_millis(50)) => {},
        _ = passive.readable(false) => {
            panic!("unexpected readable");
        }
    }

    active.connect(passive_addr).await.unwrap();
    active.send(MSG).await.0.unwrap();
    assert!(passive.readable(false).await.is_ok());
}

#[monoio::test_all(timer_enabled = true)]
async fn cancel_recv_from() {
    let passive = UdpSocket::async_bind("127.0.0.1:0").await.unwrap();
    let canceller = monoio::io::Canceller::new();
    let recv = passive.cancelable_recv_from(vec![0; 20], canceller.handle());
    let mut recv = std::pin::pin!(recv);

    monoio::select! {
        _ = monoio::time::sleep(std::time::Duration::from_millis(50)) => {
            canceller.cancel();
            assert!(recv.await.0.is_err());
        },
        _ = &mut recv => {
            panic!("unexpected readable");
        }
    }
}
