//! For compat with tokio AsyncRead and AsyncWrite.

pub mod box_future;
mod buf;

mod safe_wrapper;
mod tcp_unsafe;

#[cfg(feature = "hyper")]
pub mod hyper;

#[cfg(feature = "futures-stream")]
mod futures_stream;
#[cfg(feature = "futures-stream")]
pub use futures_stream::LegacyStream;

pub use safe_wrapper::StreamWrapper;
pub use tcp_unsafe::TcpStreamCompat as TcpStreamCompatUnsafe;
pub use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub type TcpStreamCompat = StreamWrapper<monoio::net::TcpStream>;
#[cfg(unix)]
pub type UnixStreamCompat = StreamWrapper<monoio::net::UnixStream>;

#[cfg(test)]
mod tests {

    #[monoio::test_all]
    async fn test_rw() {
        let listener = monoio::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = async move {
            let (conn, _) = listener.accept().await.unwrap();
            let mut compat_conn = TcpStreamCompat::new(conn);

            let mut buf = [0u8; 10];
            compat_conn.read_exact(&mut buf).await.unwrap();
            buf[0] += 1;
            compat_conn.write_all(&buf).await.unwrap();
        };
        let client = async {
            let conn = monoio::net::TcpStream::connect(addr).await.unwrap();
            let mut compat_conn = TcpStreamCompat::new(conn);

            let mut buf = [65u8; 10];
            compat_conn.write_all(&buf).await.unwrap();
            compat_conn.read_exact(&mut buf).await.unwrap();
            assert_eq!(buf[0], 66);
        };
        monoio::spawn(server);
        client.await;
    }

    #[monoio::test_all]
    async fn test_rw_unsafe() {
        let listener = monoio::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = async move {
            let (conn, _) = listener.accept().await.unwrap();
            let mut compat_conn = unsafe { TcpStreamCompatUnsafe::new(conn) };

            let mut buf = [0u8; 10];
            compat_conn.read_exact(&mut buf).await.unwrap();
            buf[0] += 1;
            compat_conn.write_all(&buf).await.unwrap();
        };
        let client = async {
            let conn = monoio::net::TcpStream::connect(addr).await.unwrap();
            let mut compat_conn = unsafe { TcpStreamCompatUnsafe::new(conn) };

            let mut buf = [65u8; 10];
            compat_conn.write_all(&buf).await.unwrap();
            compat_conn.read_exact(&mut buf).await.unwrap();
            assert_eq!(buf[0], 66);
        };
        monoio::spawn(server);
        client.await;
    }
}
