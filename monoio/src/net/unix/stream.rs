use std::{
    future::Future,
    io::{self},
    os::unix::prelude::{AsRawFd, FromRawFd, IntoRawFd, RawFd},
    path::Path,
};

use super::{
    socket_addr::{local_addr, pair, peer_addr, socket_addr, SocketAddr},
    ucred::UCred,
};
use crate::{
    buf::{IoBuf, IoBufMut, IoVecBuf, IoVecBufMut},
    driver::{op::Op, shared_fd::SharedFd},
    io::{
        as_fd::{AsReadFd, AsWriteFd, SharedFdWrapper},
        operation_canceled, AsyncReadRent, AsyncWriteRent, CancelHandle, CancelableAsyncReadRent,
        CancelableAsyncWriteRent, Split,
    },
    net::new_socket,
    BufResult,
};

/// UnixStream
pub struct UnixStream {
    pub(super) fd: SharedFd,
}

/// UnixStream is safe to split to two parts
unsafe impl Split for UnixStream {}

impl UnixStream {
    pub(crate) fn from_shared_fd(fd: SharedFd) -> Self {
        Self { fd }
    }

    /// Connect UnixStream to a path.
    pub async fn connect<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let (addr, addr_len) = socket_addr(path.as_ref())?;
        Self::inner_connect(addr, addr_len).await
    }

    /// Connects the socket to an address.
    pub async fn connect_addr(addr: SocketAddr) -> io::Result<Self> {
        let (addr, addr_len) = addr.into_parts();
        Self::inner_connect(addr, addr_len).await
    }

    #[inline(always)]
    async fn inner_connect(
        sockaddr: libc::sockaddr_un,
        socklen: libc::socklen_t,
    ) -> io::Result<Self> {
        let socket = new_socket(socket2::Domain::UNIX, socket2::Type::STREAM).await?;
        let op = Op::connect_unix(SharedFd::new::<false>(socket)?, sockaddr, socklen)?;
        let completion = op.await;
        completion.meta.result?;

        let stream = Self::from_shared_fd(completion.data.fd);
        if crate::driver::op::is_legacy() {
            stream.writable(true).await?;
        }
        // getsockopt
        let sys_socket = unsafe { std::os::unix::net::UnixStream::from_raw_fd(stream.fd.raw_fd()) };
        let err = sys_socket.take_error();
        let _ = sys_socket.into_raw_fd();
        if let Some(e) = err? {
            return Err(e);
        }
        Ok(stream)
    }

    /// Creates an unnamed pair of connected sockets.
    ///
    /// Returns two `UnixStream`s which are connected to each other.
    pub fn pair() -> io::Result<(Self, Self)> {
        let (a, b) = pair(libc::SOCK_STREAM)?;
        Ok((Self::from_std(a)?, Self::from_std(b)?))
    }

    /// Returns effective credentials of the process which called `connect` or
    /// `pair`.
    pub fn peer_cred(&self) -> io::Result<UCred> {
        super::ucred::get_peer_cred(self)
    }

    /// Creates new `UnixStream` from a `std::os::unix::net::UnixStream`.
    pub fn from_std(stream: std::os::unix::net::UnixStream) -> io::Result<Self> {
        match SharedFd::new::<false>(stream.as_raw_fd()) {
            Ok(shared) => {
                let _ = stream.into_raw_fd();
                Ok(Self::from_shared_fd(shared))
            }
            Err(e) => Err(e),
        }
    }

    /// Returns the socket address of the local half of this connection.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        local_addr(self.as_raw_fd())
    }

    /// Returns the socket address of the remote half of this connection.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        peer_addr(self.as_raw_fd())
    }

    /// Wait for read readiness.
    /// Note: Do not use it before every io. It is different from other runtimes!
    ///
    /// Everytime call to this method may pay a syscall cost.
    /// In uring impl, it will push a PollAdd op; in epoll impl, it will use use
    /// inner readiness state; if !relaxed, it will call syscall poll after that.
    ///
    /// If relaxed, on legacy driver it may return false positive result.
    /// If you want to do io by your own, you must maintain io readiness and wait
    /// for io ready with relaxed=false.
    pub async fn readable(&self, relaxed: bool) -> io::Result<()> {
        let op = Op::poll_read(&self.fd, relaxed).unwrap();
        op.wait().await
    }

    /// Wait for write readiness.
    /// Note: Do not use it before every io. It is different from other runtimes!
    ///
    /// Everytime call to this method may pay a syscall cost.
    /// In uring impl, it will push a PollAdd op; in epoll impl, it will use use
    /// inner readiness state; if !relaxed, it will call syscall poll after that.
    ///
    /// If relaxed, on legacy driver it may return false positive result.
    /// If you want to do io by your own, you must maintain io readiness and wait
    /// for io ready with relaxed=false.
    pub async fn writable(&self, relaxed: bool) -> io::Result<()> {
        let op = Op::poll_write(&self.fd, relaxed).unwrap();
        op.wait().await
    }
}

impl AsReadFd for UnixStream {
    #[inline]
    fn as_reader_fd(&mut self) -> &SharedFdWrapper {
        SharedFdWrapper::new(&self.fd)
    }
}

impl AsWriteFd for UnixStream {
    #[inline]
    fn as_writer_fd(&mut self) -> &SharedFdWrapper {
        SharedFdWrapper::new(&self.fd)
    }
}

impl IntoRawFd for UnixStream {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        self.fd
            .try_unwrap()
            .expect("unexpected multiple reference to rawfd")
    }
}

impl AsRawFd for UnixStream {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.fd.raw_fd()
    }
}

impl std::fmt::Debug for UnixStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnixStream").field("fd", &self.fd).finish()
    }
}

impl AsyncWriteRent for UnixStream {
    #[inline]
    fn write<T: IoBuf>(&mut self, buf: T) -> impl Future<Output = BufResult<usize, T>> {
        // Submit the write operation
        let op = Op::send(self.fd.clone(), buf).unwrap();
        op.result()
    }

    #[inline]
    fn writev<T: IoVecBuf>(&mut self, buf_vec: T) -> impl Future<Output = BufResult<usize, T>> {
        let op = Op::writev(self.fd.clone(), buf_vec).unwrap();
        op.result()
    }

    #[inline]
    async fn flush(&mut self) -> std::io::Result<()> {
        // Unix stream does not need flush.
        Ok(())
    }

    fn shutdown(&mut self) -> impl Future<Output = std::io::Result<()>> {
        // We could use shutdown op here, which requires kernel 5.11+.
        // However, for simplicity, we just close the socket using direct syscall.
        let fd = self.as_raw_fd();
        async move {
            match unsafe { libc::shutdown(fd, libc::SHUT_WR) } {
                -1 => Err(io::Error::last_os_error()),
                _ => Ok(()),
            }
        }
    }
}

impl CancelableAsyncWriteRent for UnixStream {
    #[inline]
    async fn cancelable_write<T: IoBuf>(
        &mut self,
        buf: T,
        c: CancelHandle,
    ) -> crate::BufResult<usize, T> {
        let fd = self.fd.clone();

        if c.canceled() {
            return (Err(operation_canceled()), buf);
        }

        let op = Op::send(fd, buf).unwrap();
        let _guard = c.associate_op(op.op_canceller());
        op.result().await
    }

    #[inline]
    async fn cancelable_writev<T: IoVecBuf>(
        &mut self,
        buf_vec: T,
        c: CancelHandle,
    ) -> crate::BufResult<usize, T> {
        let fd = self.fd.clone();

        if c.canceled() {
            return (Err(operation_canceled()), buf_vec);
        }

        let op = Op::writev(fd.clone(), buf_vec).unwrap();
        let _guard = c.associate_op(op.op_canceller());
        op.result().await
    }

    #[inline]
    async fn cancelable_flush(&mut self, _c: CancelHandle) -> io::Result<()> {
        // Unix stream does not need flush.
        Ok(())
    }

    async fn cancelable_shutdown(&mut self, _c: CancelHandle) -> io::Result<()> {
        // We could use shutdown op here, which requires kernel 5.11+.
        // However, for simplicity, we just close the socket using direct syscall.
        let fd = self.as_raw_fd();
        match unsafe { libc::shutdown(fd, libc::SHUT_WR) } {
            -1 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        }
    }
}

impl AsyncReadRent for UnixStream {
    #[inline]
    fn read<T: IoBufMut>(&mut self, buf: T) -> impl Future<Output = BufResult<usize, T>> {
        // Submit the read operation
        let op = Op::recv(self.fd.clone(), buf).unwrap();
        op.result()
    }

    #[inline]
    fn readv<T: IoVecBufMut>(&mut self, buf: T) -> impl Future<Output = BufResult<usize, T>> {
        // Submit the read operation
        let op = Op::readv(self.fd.clone(), buf).unwrap();
        op.result()
    }
}

impl CancelableAsyncReadRent for UnixStream {
    #[inline]
    async fn cancelable_read<T: IoBufMut>(
        &mut self,
        buf: T,
        c: CancelHandle,
    ) -> crate::BufResult<usize, T> {
        let fd = self.fd.clone();

        if c.canceled() {
            return (Err(operation_canceled()), buf);
        }

        let op = Op::recv(fd, buf).unwrap();
        let _guard = c.associate_op(op.op_canceller());
        op.result().await
    }

    #[inline]
    async fn cancelable_readv<T: IoVecBufMut>(
        &mut self,
        buf: T,
        c: CancelHandle,
    ) -> crate::BufResult<usize, T> {
        let fd = self.fd.clone();

        if c.canceled() {
            return (Err(operation_canceled()), buf);
        }

        let op = Op::readv(fd, buf).unwrap();
        let _guard = c.associate_op(op.op_canceller());
        op.result().await
    }
}

#[cfg(all(unix, feature = "legacy", feature = "tokio-compat"))]
impl tokio::io::AsyncRead for UnixStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        unsafe {
            let slice = buf.unfilled_mut();
            let raw_buf = crate::buf::RawBuf::new(slice.as_ptr() as *const u8, slice.len());
            let mut recv = Op::recv_raw(&self.fd, raw_buf);
            let ret = ready!(crate::driver::op::PollLegacy::poll_legacy(&mut recv, cx));

            std::task::Poll::Ready(ret.result.map(|n| {
                let n = n.into_inner();
                buf.assume_init(n as usize);
                buf.advance(n as usize);
            }))
        }
    }
}

#[cfg(all(unix, feature = "legacy", feature = "tokio-compat"))]
impl tokio::io::AsyncWrite for UnixStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, io::Error>> {
        unsafe {
            let raw_buf = crate::buf::RawBuf::new(buf.as_ptr(), buf.len());
            let mut send = Op::send_raw(&self.fd, raw_buf);
            let ret = ready!(crate::driver::op::PollLegacy::poll_legacy(&mut send, cx));

            std::task::Poll::Ready(ret.result.map(|n| n.into_inner() as usize))
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), io::Error>> {
        let fd = self.as_raw_fd();
        let res = match unsafe { libc::shutdown(fd, libc::SHUT_WR) } {
            -1 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        };
        std::task::Poll::Ready(res)
    }
}
