use std::io::Cursor;

use monoio::{
    buf::VecBuf,
    io::{AsyncWriteRent, AsyncWriteRentExt},
};

const TEST_DATA: &[u8] = b"Hello, Monoio!";
const LARGE_TEST_DATA: &[u8] =
    b"This is a larger test string to ensure proper handling of multiple writes...";

#[monoio::test_all]
async fn test_cursor_vec() {
    let vec = Vec::new();
    let mut cursor = Cursor::new(vec);

    // Test single write
    let (res, _) = cursor.write(TEST_DATA).await;
    assert_eq!(res.unwrap(), TEST_DATA.len());
    assert_eq!(&cursor.get_ref()[..TEST_DATA.len()], TEST_DATA);

    // Test write_all
    let (res, _) = cursor.write_all(LARGE_TEST_DATA).await;
    assert_eq!(res.unwrap(), LARGE_TEST_DATA.len());
    assert_eq!(&cursor.get_ref()[TEST_DATA.len()..], LARGE_TEST_DATA);

    // Test flush and shutdown (should be no-ops for Cursor)
    assert!(cursor.flush().await.is_ok());
    assert!(cursor.shutdown().await.is_ok());
}

#[monoio::test_all]
async fn test_cursor_mut_vec() {
    let mut vec = Vec::new();
    let mut cursor = Cursor::new(&mut vec);

    // Test single write
    let (res, _) = cursor.write(TEST_DATA).await;
    assert_eq!(res.unwrap(), TEST_DATA.len());

    // Get a reference to vec through cursor to avoid borrow checker issues
    {
        let written_data = cursor.get_ref();
        assert_eq!(&written_data[..TEST_DATA.len()], TEST_DATA);
    }

    // Test write_all
    let (res, _) = cursor.write_all(LARGE_TEST_DATA).await;
    assert_eq!(res.unwrap(), LARGE_TEST_DATA.len());

    // Get a reference to vec through cursor to avoid borrow checker issues
    {
        let written_data = cursor.get_ref();
        assert_eq!(&written_data[TEST_DATA.len()..], LARGE_TEST_DATA);
    }
}

#[monoio::test_all]
async fn test_cursor_mut_slice() {
    let mut data = [0u8; 32];
    {
        let mut cursor = Cursor::new(&mut data[..]);

        // Test write (should only write up to slice capacity)
        let (res, _) = cursor.write(TEST_DATA).await;
        assert_eq!(res.unwrap(), TEST_DATA.len());

        // Get a reference through cursor
        {
            let written_data = cursor.get_ref();
            assert_eq!(&written_data[..TEST_DATA.len()], TEST_DATA);
        }

        // Test write beyond capacity (should return number of bytes that fit)
        let pos = cursor.position() as usize;
        let remaining = cursor.get_ref().len() - pos;
        let (res, _) = cursor.write(LARGE_TEST_DATA).await;
        let expected_write = std::cmp::min(LARGE_TEST_DATA.len(), remaining);
        assert_eq!(res.unwrap(), expected_write);
    }
}

#[monoio::test_all]
async fn test_cursor_box_slice() {
    let data = vec![0u8; 32].into_boxed_slice();
    let mut cursor = Cursor::new(data);

    // Test write
    let (res, _) = cursor.write(TEST_DATA).await;
    assert_eq!(res.unwrap(), TEST_DATA.len());
    assert_eq!(&cursor.get_ref()[..TEST_DATA.len()], TEST_DATA);

    // Test write beyond capacity
    let remaining = cursor.get_ref().len() - cursor.position() as usize;
    let expected_write = std::cmp::min(LARGE_TEST_DATA.len(), remaining);
    let (res, _) = cursor.write(LARGE_TEST_DATA).await;
    assert_eq!(res.unwrap(), expected_write);
}

// Test vectored writes using VecBuf
#[monoio::test_all]
async fn test_cursor_vectored_write() {
    let vec = Vec::new();
    let mut cursor = Cursor::new(vec);

    // Create VecBuf with multiple buffers
    let buf_vec = VecBuf::from(vec![TEST_DATA[..5].to_vec(), TEST_DATA[5..].to_vec()]);

    // Test vectored write
    let (res, buf_vec) = cursor.writev(buf_vec).await;
    assert_eq!(res.unwrap(), TEST_DATA.len());
    assert_eq!(cursor.get_ref(), TEST_DATA);

    // Convert back to Vec<Vec<u8>> and verify the buffers are unchanged
    let raw_vec: Vec<Vec<u8>> = buf_vec.into();
    assert_eq!(&raw_vec[0], &TEST_DATA[..5]);
    assert_eq!(&raw_vec[1], &TEST_DATA[5..]);

    // Test vectored write with multiple chunks
    let buf_vec = VecBuf::from(vec![
        LARGE_TEST_DATA[..10].to_vec(),
        LARGE_TEST_DATA[10..20].to_vec(),
        LARGE_TEST_DATA[20..].to_vec(),
    ]);

    let (res, buf_vec) = cursor.writev(buf_vec).await;
    assert_eq!(res.unwrap(), LARGE_TEST_DATA.len());
    assert_eq!(&cursor.get_ref()[TEST_DATA.len()..], LARGE_TEST_DATA);

    // Verify the original buffers are unchanged
    let raw_vec: Vec<Vec<u8>> = buf_vec.into();
    assert_eq!(&raw_vec[0], &LARGE_TEST_DATA[..10]);
    assert_eq!(&raw_vec[1], &LARGE_TEST_DATA[10..20]);
    assert_eq!(&raw_vec[2], &LARGE_TEST_DATA[20..]);
}

// Test vectored writes with fixed-size buffers
#[monoio::test_all]
async fn test_cursor_vectored_write_fixed_size() {
    let mut data = [0u8; 32];
    {
        let mut cursor = Cursor::new(&mut data[..]);

        // Create VecBuf that would exceed the buffer capacity
        let buf_vec = VecBuf::from(vec![
            vec![1; 16],
            vec![2; 16],
            vec![3; 16], // This should be partially written or not written at all
        ]);

        let total_size = 48; // Total size of all buffers
        let capacity = cursor.get_ref().len();
        let (res, buf_vec) = cursor.writev(buf_vec).await;
        let expected_write = std::cmp::min(total_size, capacity);
        assert_eq!(res.unwrap(), expected_write);

        // Get a reference through cursor to verify the data
        {
            let written_data = cursor.get_ref();
            assert_eq!(&written_data[..16], &[1; 16]);
            assert_eq!(&written_data[16..32], &[2; 16]);
        }

        // Verify the original buffers are unchanged
        let raw_vec: Vec<Vec<u8>> = buf_vec.into();
        assert_eq!(&raw_vec[0], &[1; 16]);
        assert_eq!(&raw_vec[1], &[2; 16]);
        assert_eq!(&raw_vec[2], &[3; 16]);
    }
}

// Test error conditions
#[monoio::test_all]
async fn test_cursor_error_conditions() {
    let mut data = [0u8; 8];
    {
        let mut cursor = Cursor::new(&mut data[..]);

        // First write should succeed
        let (res, _) = cursor.write(&[1, 2, 3, 4]).await;
        assert_eq!(res.unwrap(), 4);

        // Move cursor to end
        cursor.set_position(8);

        // Write at end should return 0 bytes written
        let (res, _) = cursor.write(&[5, 6, 7, 8]).await;
        assert_eq!(res.unwrap(), 0);

        // Test vectored write at end
        let buf_vec = VecBuf::from(vec![vec![5; 4], vec![6; 4]]);
        let (res, _) = cursor.writev(buf_vec).await;
        assert_eq!(res.unwrap(), 0);
    }
}
