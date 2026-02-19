use monoio::net::udp::UdpSocket;

#[monoio::test_all]
async fn connect() {
    const MSG: &str = "foo bar baz";

    let passive = UdpSocket::bind("127.0.0.1:0").unwrap();
    let passive_addr = passive.local_addr().unwrap();

    let active = UdpSocket::bind("127.0.0.1:0").unwrap();
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
async fn send_to() {
    const MSG: &str = "foo bar baz";

    macro_rules! must_success {
        ($r: expr, $expect_addr: expr) => {
            let res = $r;
            assert_eq!(res.0.unwrap().1, $expect_addr);
            assert_eq!(res.1, MSG.as_bytes());
        };
    }

    let passive1 = UdpSocket::bind("127.0.0.1:0").unwrap();
    let passive1_addr = passive1.local_addr().unwrap();

    let passive01 = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let passive01_addr = passive01.local_addr().unwrap();

    let passive2 = UdpSocket::bind("127.0.0.1:0").unwrap();
    let passive2_addr = passive2.local_addr().unwrap();

    let passive3 = UdpSocket::bind("127.0.0.1:0").unwrap();
    let passive3_addr = passive3.local_addr().unwrap();

    let active = UdpSocket::bind("127.0.0.1:0").unwrap();
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

    let passive = UdpSocket::bind("127.0.0.1:0").unwrap();
    let passive_addr = passive.local_addr().unwrap();

    let active = UdpSocket::bind("127.0.0.1:0").unwrap();

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
    let passive = UdpSocket::bind("127.0.0.1:0").unwrap();
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

#[cfg(all(target_os = "linux", feature = "iouring"))]
mod multishot_tests {
    use std::net::SocketAddr;

    use monoio::{
        buf::{
            AnyRecvMsgParser, Ipv4RecvMsgParser, Ipv6RecvMsgParser, RecvMsgParser,
            UserRecvMsgRingBuf, UserRingBufBuilder,
        },
        io::Canceller,
        net::udp::{RecvMsgMultishot, UdpSocket},
    };

    const PAYLOAD_SIZE: usize = 1500;

    async fn test_recvmsg_multishot<P>(
        recv: &mut RecvMsgMultishot<UserRecvMsgRingBuf<P>>,
        sender: &UdpSocket,
        receiver_addr: SocketAddr,
        sender_addr: SocketAddr,
    ) where
        P: RecvMsgParser + Unpin + 'static,
        P::Address: Into<SocketAddr> + std::fmt::Debug,
    {
        let mut stream = recv.stream();

        sender.send_to(b"hello", receiver_addr).await.0.unwrap();
        {
            let (addr, buf) = stream.next().await.unwrap().unwrap();
            assert_eq!(addr.into(), sender_addr);
            assert_eq!(&*buf, b"hello");
        }

        sender.send_to(b"world", receiver_addr).await.0.unwrap();
        {
            let (addr, buf) = stream.next().await.unwrap().unwrap();
            assert_eq!(addr.into(), sender_addr);
            assert_eq!(&*buf, b"world");
        }

        let large_payload: Vec<u8> = (0..PAYLOAD_SIZE).map(|i| (i % 256) as u8).collect();
        let (result, _) = sender.send_to(large_payload.clone(), receiver_addr).await;
        result.unwrap();

        {
            let (addr, buf) = stream.next().await.unwrap().unwrap();
            assert_eq!(addr.into(), sender_addr);
            assert_eq!(buf.len(), PAYLOAD_SIZE);
            assert_eq!(&*buf, &large_payload[..]);
        }
    }

    #[monoio::test(driver = "uring")]
    async fn recvmsg_ipv4() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender_addr = sender.local_addr().unwrap();

        let ring = UserRecvMsgRingBuf::<Ipv4RecvMsgParser>::new(16, PAYLOAD_SIZE, 0).unwrap();
        let canceller = Canceller::new();
        let mut stream = receiver
            .cancelable_recvmsg_multishot(ring, canceller.handle())
            .unwrap();

        test_recvmsg_multishot(&mut stream, &sender, receiver_addr, sender_addr).await;

        canceller.cancel();
        while stream.stream().next().await.is_some() {}
        stream.try_into_ring().ok().unwrap().unregister().unwrap();
    }

    #[monoio::test(driver = "uring")]
    async fn recvmsg_ipv6() {
        let receiver = UdpSocket::bind("[::1]:0").unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        let sender = UdpSocket::bind("[::1]:0").unwrap();
        let sender_addr = sender.local_addr().unwrap();

        let ring = UserRecvMsgRingBuf::<Ipv6RecvMsgParser>::new(16, PAYLOAD_SIZE, 1).unwrap();
        let canceller = Canceller::new();
        let mut stream = receiver
            .cancelable_recvmsg_multishot(ring, canceller.handle())
            .unwrap();

        test_recvmsg_multishot(&mut stream, &sender, receiver_addr, sender_addr).await;

        canceller.cancel();
        while stream.stream().next().await.is_some() {}
        stream.try_into_ring().ok().unwrap().unregister().unwrap();
    }

    #[monoio::test(driver = "uring")]
    async fn recvmsg_any() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender_addr = sender.local_addr().unwrap();

        let ring = UserRecvMsgRingBuf::<AnyRecvMsgParser>::new(16, PAYLOAD_SIZE, 2).unwrap();
        let canceller = Canceller::new();
        let mut stream = receiver
            .cancelable_recvmsg_multishot(ring, canceller.handle())
            .unwrap();

        test_recvmsg_multishot(&mut stream, &sender, receiver_addr, sender_addr).await;

        canceller.cancel();
        while stream.stream().next().await.is_some() {}
        stream.try_into_ring().ok().unwrap().unregister().unwrap();
    }

    #[monoio::test(driver = "uring")]
    async fn recvmsg_multishot_buffer_reuse() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender_addr = sender.local_addr().unwrap();

        let ring = UserRecvMsgRingBuf::<Ipv4RecvMsgParser>::new(4, 64, 30).unwrap();
        let canceller = Canceller::new();
        let mut stream = receiver
            .cancelable_recvmsg_multishot(ring, canceller.handle())
            .unwrap();

        for i in 0..8 {
            let msg = format!("message {}", i);
            sender
                .send_to(msg.clone().into_bytes(), receiver_addr)
                .await
                .0
                .unwrap();

            let (addr, buf) = stream.stream().next().await.unwrap().unwrap();
            assert_eq!(SocketAddr::from(addr), sender_addr);
            assert_eq!(&*buf, msg.as_bytes());
        }

        canceller.cancel();
        while stream.stream().next().await.is_some() {}
        stream.try_into_ring().ok().unwrap().unregister().unwrap();
    }

    #[monoio::test(driver = "uring")]
    async fn recvmsg_multishot_buffer_exhaustion_and_recovery() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender_addr = sender.local_addr().unwrap();

        let ring = UserRecvMsgRingBuf::<Ipv4RecvMsgParser>::new(4, 64, 60).unwrap();
        let mut stream = receiver.recvmsg_multishot(ring).unwrap();

        // Send 4 messages, then receive them all
        for i in 0..4 {
            sender
                .send_to(format!("msg{}", i).into_bytes(), receiver_addr)
                .await
                .0
                .unwrap();
        }

        // Receive all 4 buffers - use a single poller to hold buffers across next() calls
        let mut poller = stream.stream();
        let mut held_buffers = Vec::new();
        for _ in 0..4 {
            let (_addr, buf) = poller.next().await.unwrap().unwrap();
            held_buffers.push(buf);
        }

        // send another message - kernel socket buffer receives it, but io_uring
        // can't deliver it because no buffers are available in the ring
        sender
            .send_to(b"overflow".to_vec(), receiver_addr)
            .await
            .0
            .unwrap();

        // this should fail with ENOBUFS - the buffer ring is exhausted
        let result = poller.next().await.unwrap();
        assert!(
            result.is_err(),
            "expected ENOBUFS error when ring exhausted"
        );
        let err = result.unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ENOBUFS));

        // stream is terminated after ENOBUFS, subsequent calls return None
        {
            let result = poller.next().await;
            assert!(
                result.is_none(),
                "stream should be terminated after ENOBUFS"
            );
        }
        drop(held_buffers);
        drop(poller);

        let ring = stream.try_into_ring().ok().expect("failed to get ring");
        let canceller = Canceller::new();
        let mut stream2 = receiver
            .cancelable_recvmsg_multishot(ring, canceller.handle())
            .unwrap();

        // The "overflow" packet is still in the kernel socket buffer
        {
            let (addr, buf) = stream2.stream().next().await.unwrap().unwrap();
            assert_eq!(SocketAddr::from(addr), sender_addr);
            assert_eq!(&*buf, b"overflow");
        }

        canceller.cancel();
        while stream2.stream().next().await.is_some() {}
        stream2.try_into_ring().ok().unwrap().unregister().unwrap();
    }

    #[monoio::test(driver = "uring")]
    async fn recvmsg_multishot_multiple_senders() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let receiver_addr = receiver.local_addr().unwrap();

        let sender1 = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender1_addr = sender1.local_addr().unwrap();
        let sender2 = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender2_addr = sender2.local_addr().unwrap();

        let ring = UserRecvMsgRingBuf::<Ipv4RecvMsgParser>::new(16, PAYLOAD_SIZE, 40).unwrap();
        let canceller = Canceller::new();
        let mut stream = receiver
            .cancelable_recvmsg_multishot(ring, canceller.handle())
            .unwrap();

        sender1
            .send_to(b"from sender1", receiver_addr)
            .await
            .0
            .unwrap();
        sender2
            .send_to(b"from sender2", receiver_addr)
            .await
            .0
            .unwrap();

        let mut received_from_1 = false;
        let mut received_from_2 = false;

        for _ in 0..2 {
            let (addr, buf) = stream.stream().next().await.unwrap().unwrap();
            let addr: SocketAddr = addr.into();
            if addr == sender1_addr {
                assert_eq!(&*buf, b"from sender1");
                received_from_1 = true;
            } else if addr == sender2_addr {
                assert_eq!(&*buf, b"from sender2");
                received_from_2 = true;
            }
        }

        assert!(received_from_1, "should have received from sender1");
        assert!(received_from_2, "should have received from sender2");

        canceller.cancel();
        while stream.stream().next().await.is_some() {}
        stream.try_into_ring().ok().unwrap().unregister().unwrap();
    }

    #[monoio::test(driver = "uring")]
    async fn recvmsg_multishot_sequential_messages() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender_addr = sender.local_addr().unwrap();

        let ring = UserRecvMsgRingBuf::<Ipv4RecvMsgParser>::new(32, 64, 50).unwrap();
        let canceller = Canceller::new();
        let mut stream = receiver
            .cancelable_recvmsg_multishot(ring, canceller.handle())
            .unwrap();

        const NUM_MESSAGES: usize = 20;

        for i in 0..NUM_MESSAGES {
            let msg = format!("seq{:04}", i);
            sender
                .send_to(msg.clone().into_bytes(), receiver_addr)
                .await
                .0
                .unwrap();

            let (addr, buf) = stream.stream().next().await.unwrap().unwrap();
            assert_eq!(SocketAddr::from(addr), sender_addr);
            assert_eq!(&*buf, msg.as_bytes());
        }

        canceller.cancel();
        while stream.stream().next().await.is_some() {}
        stream.try_into_ring().ok().unwrap().unregister().unwrap();
    }

    #[monoio::test(driver = "uring")]
    async fn recv_multishot_cancel() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();

        receiver
            .connect(sender.local_addr().unwrap())
            .await
            .unwrap();
        sender.connect(receiver_addr).await.unwrap();

        let ring = UserRingBufBuilder::new()
            .buffer_size(PAYLOAD_SIZE)
            .buffer_count(16)
            .group_id(60)
            .build()
            .unwrap();

        let canceller = Canceller::new();
        let handle = canceller.handle();
        let mut stream = receiver.cancelable_recv_multishot(ring, handle).unwrap();

        sender.send(b"hello before cancel").await.0.unwrap();

        {
            let buf = stream.stream().next().await.unwrap().unwrap();
            assert_eq!(&*buf, b"hello before cancel");
        }

        canceller.cancel();

        assert!(stream.stream().next().await.is_none());
        assert!(stream.stream().next().await.is_none());

        stream.try_into_ring().ok().unwrap().unregister().unwrap();
    }

    #[monoio::test(driver = "uring")]
    async fn recvmsg_multishot_cancel() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender_addr = sender.local_addr().unwrap();

        let ring = UserRecvMsgRingBuf::<Ipv4RecvMsgParser>::new(16, PAYLOAD_SIZE, 50).unwrap();

        let canceller = Canceller::new();
        let handle = canceller.handle();
        let mut stream = receiver.cancelable_recvmsg_multishot(ring, handle).unwrap();

        sender
            .send_to(b"hello before cancel", receiver_addr)
            .await
            .0
            .unwrap();

        {
            let (addr, buf) = stream.stream().next().await.unwrap().unwrap();
            assert_eq!(SocketAddr::from(addr), sender_addr);
            assert_eq!(&*buf, b"hello before cancel");
        }

        canceller.cancel();

        assert!(stream.stream().next().await.is_none());
        assert!(stream.stream().next().await.is_none());

        stream.try_into_ring().ok().unwrap().unregister().unwrap();
    }

    #[monoio::test(driver = "uring")]
    async fn buffer_ring_held_buffers_not_overwritten() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();

        let ring = UserRecvMsgRingBuf::<Ipv4RecvMsgParser>::new(4, 64, 70).unwrap();
        let canceller = Canceller::new();
        let mut stream = receiver
            .cancelable_recvmsg_multishot(ring, canceller.handle())
            .unwrap();

        let mut poller = stream.stream();

        sender
            .send_to(b"held_aaa".to_vec(), receiver_addr)
            .await
            .0
            .unwrap();
        sender
            .send_to(b"held_bbb".to_vec(), receiver_addr)
            .await
            .0
            .unwrap();

        let (_, held1) = poller.next().await.unwrap().unwrap();
        let (_, held2) = poller.next().await.unwrap().unwrap();
        let held_id1 = held1.id();
        let held_id2 = held2.id();
        assert_eq!(&*held1, b"held_aaa");
        assert_eq!(&*held2, b"held_bbb");

        for i in 0..10 {
            let msg = format!("iter_{}", i);
            sender
                .send_to(msg.clone().into_bytes(), receiver_addr)
                .await
                .0
                .unwrap();

            let (_, buf) = poller.next().await.unwrap().unwrap();

            assert_ne!(buf.id(), held_id1);
            assert_ne!(buf.id(), held_id2);
            assert_eq!(&*buf, msg.as_bytes());

            assert_eq!(&*held1, b"held_aaa");
            assert_eq!(&*held2, b"held_bbb");
        }

        drop(held1);
        drop(held2);
        drop(poller);
        canceller.cancel();
        while stream.stream().next().await.is_some() {}
        stream.try_into_ring().ok().unwrap().unregister().unwrap();
    }
}
