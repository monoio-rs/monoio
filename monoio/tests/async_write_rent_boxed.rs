use std::io::Cursor;

use monoio::{
    buf::VecBuf,
    io::{AsyncWriteRent, AsyncWriteRentExt, BufWriter},
};

const TEST_DATA: &[u8] = b"Hello, Boxed AsyncWriteRent!";
const LARGE_TEST_DATA: &[u8] = b"This is a larger test string to ensure proper handling of multiple writes in boxed AsyncWriteRent...";

#[monoio::test_all]
async fn test_boxed_cursor_vec() {
    let cursor = Cursor::new(Vec::new());
    let mut writer = Box::new(cursor);

    // Test single write
    let (res, _) = writer.write(TEST_DATA).await;
    assert_eq!(res.unwrap(), TEST_DATA.len());
    assert_eq!(&writer.get_ref()[..TEST_DATA.len()], TEST_DATA);

    // Test write_all
    let (res, _) = writer.write_all(LARGE_TEST_DATA).await;
    assert_eq!(res.unwrap(), LARGE_TEST_DATA.len());
    assert_eq!(&writer.get_ref()[TEST_DATA.len()..], LARGE_TEST_DATA);

    // Test flush and shutdown (should be no-ops for Cursor)
    assert!(writer.flush().await.is_ok());
    assert!(writer.shutdown().await.is_ok());
}

#[monoio::test_all]
async fn test_boxed_cursor_vec_writev() {
    let cursor = Cursor::new(Vec::new());
    let mut writer = Box::new(cursor);

    let buf_vec = VecBuf::from(vec![b"foo".to_vec(), b"bar".to_vec()]);
    let (res, _) = writer.writev(buf_vec).await;
    assert_eq!(res.unwrap(), 6);
    assert_eq!(&writer.get_ref()[..6], b"foobar");
}

#[monoio::test_all]
async fn test_boxed_bufwriter_cursor_vec() {
    let buf_writer = BufWriter::new(Cursor::new(Vec::new()));
    let mut writer = Box::new(buf_writer);

    let (res, _) = writer.write(TEST_DATA).await;
    assert_eq!(res.unwrap(), TEST_DATA.len());
    assert!(writer.flush().await.is_ok());
}

#[monoio::test_all]
async fn test_boxed_cursor_box_slice() {
    let data = vec![0u8; 16].into_boxed_slice();
    let mut writer = Box::new(Cursor::new(data));

    // Write less than capacity
    let (res, _) = writer.write(TEST_DATA).await;
    let expected = std::cmp::min(TEST_DATA.len(), 16);
    assert_eq!(res.unwrap(), expected);
    assert_eq!(&writer.get_ref()[..expected], &TEST_DATA[..expected]);

    // Write when full
    writer.set_position(16);
    let (res, _) = writer.write(TEST_DATA).await;
    assert_eq!(res.unwrap(), 0);
}

#[monoio::test_all]
async fn test_boxed_cursor_vec_zero_length_write() {
    let mut writer = Box::new(Cursor::new(Vec::new()));
    let (res, _) = writer.write(&[]).await;
    assert_eq!(res.unwrap(), 0);
}

// Error handling: mock a type that returns error on write
struct ErrorWriter;

impl AsyncWriteRent for ErrorWriter {
    fn write<T: monoio::buf::IoBuf>(
        &mut self,
        _buf: T,
    ) -> impl std::future::Future<Output = monoio::BufResult<usize, T>> {
        std::future::ready((Err(std::io::Error::other("fail")), _buf))
    }
    fn writev<T: monoio::buf::IoVecBuf>(
        &mut self,
        _buf_vec: T,
    ) -> impl std::future::Future<Output = monoio::BufResult<usize, T>> {
        std::future::ready((Err(std::io::Error::other("fail")), _buf_vec))
    }
    fn flush(&mut self) -> impl std::future::Future<Output = std::io::Result<()>> {
        std::future::ready(Ok(()))
    }
    fn shutdown(&mut self) -> impl std::future::Future<Output = std::io::Result<()>> {
        std::future::ready(Ok(()))
    }
}

#[monoio::test_all]
async fn test_boxed_error_writer() {
    let mut writer = Box::new(ErrorWriter);
    let (res, _) = writer.write(TEST_DATA).await;
    assert!(res.is_err());
    let buf_vec = VecBuf::from(vec![b"foo".to_vec()]);
    let (res, _) = writer.writev(buf_vec).await;
    assert!(res.is_err());
}
