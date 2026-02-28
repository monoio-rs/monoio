use monoio::{
    io::{AsyncReadRent, AsyncWriteRent, BufReader, BufWriter, Splitable},
    net::{TcpListener, TcpStream},
};

#[monoio::test_all]
async fn ensure_buf_writter_write_properly() {
    let srv = TcpListener::async_bind("127.0.0.1:0").await.unwrap();
    let addr = srv.local_addr().unwrap();

    monoio::spawn(async move {
        let stream = TcpStream::connect(&addr).await.unwrap();
        let (_, stream_write) = stream.into_split();

        let mut buf_w = BufWriter::new(stream_write);
        assert!(buf_w.write(b"1").await.0.is_ok());
        assert!(buf_w.write(b"2").await.0.is_ok());
        assert!(buf_w.write(b"3").await.0.is_ok());
        assert!(buf_w.flush().await.is_ok());
    });

    let (stream, _) = srv.accept().await.unwrap();
    let (rd, _) = stream.into_split();

    let s: Vec<u8> = Vec::with_capacity(16);
    let mut buf = BufReader::new(rd);
    let (size, s) = buf.read(s).await;

    assert!(size.is_ok());
    assert_eq!(s, b"123");
}
