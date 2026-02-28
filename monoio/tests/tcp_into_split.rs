use std::{
    io::{Error, Read, Result, Write},
    net, thread,
};

use monoio::{
    io::{AsyncReadRent, AsyncWriteRentExt, Splitable},
    net::{TcpListener, TcpStream},
    try_join,
};

#[monoio::test_all]
async fn split() -> Result<()> {
    const MSG: &[u8] = b"split";

    let listener = TcpListener::async_bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let (stream1, (mut stream2, _)) = try_join! {
        TcpStream::connect(&addr),
        listener.accept(),
    }?;
    let (mut read_half, mut write_half) = stream1.into_split();

    let ((), (), ()) = try_join! {
        async {
            let len = stream2.write_all(MSG).await.0?;
            assert_eq!(len, MSG.len());

            let read_buf = vec![0u8; 32];
            let (read_res, read_buf) = stream2.read(read_buf).await;
            assert_eq!(read_res.unwrap(), MSG.len());
            assert_eq!(&read_buf[..MSG.len()], MSG);
            Result::Ok(())
        },
        async {
            let len = write_half.write_all(MSG).await.0?;
            assert_eq!(len, MSG.len());
            Ok(())
        },
        async {
            let read_buf = vec![0u8; 32];
            let (read_res, read_buf) = read_half.read(read_buf).await;
            assert_eq!(read_res.unwrap(), MSG.len());
            assert_eq!(&read_buf[..MSG.len()], MSG);
            Ok(())
        },
    }?;

    Ok(())
}

#[monoio::test_all(enable_timer = true)]
async fn reunite() -> Result<()> {
    let listener = net::TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;

    let handle = thread::spawn(move || {
        drop(listener.accept().unwrap());
        drop(listener.accept().unwrap());
    });

    let stream1 = TcpStream::connect(&addr).await?;
    let (read1, write1) = stream1.into_split();

    let stream2 = TcpStream::connect(&addr).await?;
    let (_, write2) = stream2.into_split();

    let read1 = match read1.reunite(write2) {
        Ok(_) => panic!("Reunite should not succeed"),
        Err(err) => err.0,
    };

    read1.reunite(write1).expect("Reunite should succeed");

    handle.join().unwrap();
    Ok(())
}

/// Test that dropping the write half actually closes the stream.
#[monoio::test_all(enable_timer = true, entries = 1024)]
async fn drop_write() -> Result<()> {
    const MSG: &[u8] = b"split";

    let listener = net::TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;

    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(MSG).unwrap();

        let mut read_buf = [0u8; 32];
        let res = match stream.read(&mut read_buf) {
            Ok(0) => Ok(()),
            Ok(len) => Err(Error::other(format!("Unexpected read: {len} bytes."))),
            Err(err) => Err(err),
        };

        drop(stream);

        res
    });

    let stream = TcpStream::connect(&addr).await?;
    let (mut read_half, write_half) = stream.into_split();

    let read_buf = vec![0u8; 32];
    let (read_res, read_buf) = read_half.read(read_buf).await;
    assert_eq!(read_res.unwrap(), MSG.len());
    assert_eq!(&read_buf[..MSG.len()], MSG);
    // drop it while the read is in progress
    monoio::spawn(async move {
        monoio::time::sleep(std::time::Duration::from_millis(10)).await;
        drop(write_half);
    });
    match read_half.read(read_buf).await.0 {
        Ok(0) => {}
        Ok(len) => panic!("Unexpected read: {len} bytes."),
        Err(err) => panic!("Unexpected error: {err}."),
    }
    handle.join().unwrap().unwrap();
    Ok(())
}
