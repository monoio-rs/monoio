use std::{
    cell::UnsafeCell,
    future::Future,
    io,
    net::{SocketAddr, ToSocketAddrs},
    time::Duration,
};

#[cfg(unix)]
use {
    libc::{shutdown, AF_INET, AF_INET6, SHUT_WR, SOCK_STREAM},
    std::os::unix::prelude::{AsRawFd, FromRawFd, IntoRawFd, RawFd},
};
#[cfg(windows)]
use {
    std::os::windows::prelude::{AsRawSocket, FromRawSocket, IntoRawSocket, RawSocket},
    windows_sys::Win32::Networking::WinSock::{
        shutdown, AF_INET, AF_INET6, SD_SEND as SHUT_WR, SOCK_STREAM,
    },
};

use crate::{
    buf::{IoBuf, IoBufMut, IoVecBuf, IoVecBufMut},
    driver::{op::Op, shared_fd::SharedFd},
    io::{
        as_fd::{AsReadFd, AsWriteFd, SharedFdWrapper},
        operation_canceled, AsyncReadRent, AsyncWriteRent, CancelHandle, CancelableAsyncReadRent,
        CancelableAsyncWriteRent, Split,
    },
    BufResult,
};

/// Custom tcp connect options
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct TcpConnectOpts {
    /// TCP fast open.
    pub tcp_fast_open: bool,
}

impl Default for TcpConnectOpts {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl TcpConnectOpts {
    /// Create a default TcpConnectOpts.
    #[inline]
    pub const fn new() -> Self {
        Self {
            tcp_fast_open: false,
        }
    }

    /// Specify FastOpen
    /// Note: This option only works for linux 4.1+
    /// and macos/ios 9.0+.
    /// If it is enabled, the connection will be
    /// established on the first call to write.
    #[must_use]
    #[inline]
    pub fn tcp_fast_open(mut self, fast_open: bool) -> Self {
        self.tcp_fast_open = fast_open;
        self
    }
}
/// TcpStream
pub struct TcpStream {
    pub(super) fd: SharedFd,
    meta: StreamMeta,
}

/// TcpStream is safe to split to two parts
unsafe impl Split for TcpStream {}

impl TcpStream {
    pub(crate) fn from_shared_fd(fd: SharedFd) -> Self {
        #[cfg(unix)]
        let meta = StreamMeta::new(fd.raw_fd());
        #[cfg(windows)]
        let meta = StreamMeta::new(fd.raw_socket());
        #[cfg(feature = "zero-copy")]
        // enable SOCK_ZEROCOPY
        meta.set_zero_copy();

        Self { fd, meta }
    }

    /// Open a TCP connection to a remote host.
    /// Note: This function may block the current thread while resolution is
    /// performed.
    // TODO(chihai): Fix it, maybe spawn_blocking like tokio.
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        // TODO(chihai): loop for all addrs
        let addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::other("empty address"))?;

        Self::connect_addr(addr).await
    }

    /// Establish a connection to the specified `addr`.
    pub async fn connect_addr(addr: SocketAddr) -> io::Result<Self> {
        const DEFAULT_OPTS: TcpConnectOpts = TcpConnectOpts {
            tcp_fast_open: false,
        };
        Self::connect_addr_with_config(addr, &DEFAULT_OPTS).await
    }

    /// Establish a connection to the specified `addr` with given config.
    pub async fn connect_addr_with_config(
        addr: SocketAddr,
        opts: &TcpConnectOpts,
    ) -> io::Result<Self> {
        let domain = match addr {
            SocketAddr::V4(_) => AF_INET,
            SocketAddr::V6(_) => AF_INET6,
        };
        let socket = crate::net::new_socket(domain, SOCK_STREAM)?;
        #[allow(unused_mut)]
        let mut tfo = opts.tcp_fast_open;

        if tfo {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            super::tfo::try_set_tcp_fastopen_connect(&socket);
            #[cfg(any(target_os = "ios", target_os = "macos"))]
            // if we cannot set force tcp fastopen, we will not use it.
            if super::tfo::set_tcp_fastopen_force_enable(&socket).is_err() {
                tfo = false;
            }
        }
        let completion = Op::connect(SharedFd::new::<false>(socket)?, addr, tfo)?.await;
        completion.meta.result?;

        let stream = TcpStream::from_shared_fd(completion.data.fd);
        // wait write ready on epoll branch
        if crate::driver::op::is_legacy() {
            #[cfg(all(any(target_os = "ios", target_os = "macos"), feature = "legacy"))]
            if !tfo {
                stream.writable(true).await?;
            } else {
                // set writable as init state
                crate::driver::CURRENT.with(|inner| match inner {
                    crate::driver::Inner::Legacy(inner) => {
                        let idx = stream.fd.registered_index().unwrap();
                        if let Some(mut readiness) =
                            unsafe { &mut *inner.get() }.io_dispatch.get(idx)
                        {
                            readiness.set_writable();
                        }
                    }
                    #[allow(unreachable_patterns)]
                    _ => unreachable!("should never happens"),
                })
            }
            #[cfg(not(any(target_os = "ios", target_os = "macos")))]
            stream.writable(true).await?;

            // getsockopt libc::SO_ERROR
            #[cfg(unix)]
            let sys_socket = unsafe { std::net::TcpStream::from_raw_fd(stream.fd.raw_fd()) };
            #[cfg(windows)]
            let sys_socket =
                unsafe { std::net::TcpStream::from_raw_socket(stream.fd.raw_socket()) };
            let err = sys_socket.take_error();
            #[cfg(unix)]
            let _ = sys_socket.into_raw_fd();
            #[cfg(windows)]
            let _ = sys_socket.into_raw_socket();
            if let Some(e) = err? {
                return Err(e);
            }
        }
        Ok(stream)
    }

    /// Return the local address that this stream is bound to.
    #[inline]
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.meta.local_addr()
    }

    /// Return the remote address that this stream is connected to.
    #[inline]
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.meta.peer_addr()
    }

    /// Get the value of the `TCP_NODELAY` option on this socket.
    #[inline]
    pub fn nodelay(&self) -> io::Result<bool> {
        self.meta.no_delay()
    }

    /// Set the value of the `TCP_NODELAY` option on this socket.
    #[inline]
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.meta.set_no_delay(nodelay)
    }

    /// Set the value of the `SO_KEEPALIVE` option on this socket.
    #[inline]
    pub fn set_tcp_keepalive(
        &self,
        time: Option<Duration>,
        interval: Option<Duration>,
        retries: Option<u32>,
    ) -> io::Result<()> {
        self.meta.set_tcp_keepalive(time, interval, retries)
    }

    /// Creates new `TcpStream` from a `std::net::TcpStream`.
    pub fn from_std(stream: std::net::TcpStream) -> io::Result<Self> {
        #[cfg(unix)]
        let fd = stream.as_raw_fd();
        #[cfg(windows)]
        let fd = stream.as_raw_socket();
        match SharedFd::new::<false>(fd) {
            Ok(shared) => {
                #[cfg(unix)]
                let _ = stream.into_raw_fd();
                #[cfg(windows)]
                let _ = stream.into_raw_socket();
                Ok(Self::from_shared_fd(shared))
            }
            Err(e) => Err(e),
        }
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

    /// Attempts a zero-copy write using `IORING_OP_SEND_ZC` (Linux 6.0+).
    ///
    /// When supported by the kernel, NIC and socket configuration, the kernel
    /// may pin the userspace buffer and DMA directly from it, avoiding a copy
    /// into the socket buffer. The buffer is then held until the kernel
    /// signals it is safe to release via a notification CQE.
    ///
    /// In practice, `SEND_ZC` is best-effort: it may fall back to a regular
    /// send (with copying) or fail with errors such as `EOPNOTSUPP` or
    /// `EINVAL`. Callers should treat this like a normal write and must not
    /// assume that a successful call always performed true zero-copy.
    ///
    /// This is most effective for large writes (e.g. 10KB+).
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    pub async fn write_zc<T: IoBuf>(&self, buf: T) -> BufResult<usize, T> {
        let op = Op::send_zc(self.fd.clone(), buf).unwrap();
        op.result().await
    }
}

impl AsReadFd for TcpStream {
    #[inline]
    fn as_reader_fd(&mut self) -> &SharedFdWrapper {
        SharedFdWrapper::new(&self.fd)
    }
}

impl AsWriteFd for TcpStream {
    #[inline]
    fn as_writer_fd(&mut self) -> &SharedFdWrapper {
        SharedFdWrapper::new(&self.fd)
    }
}

#[cfg(unix)]
impl IntoRawFd for TcpStream {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        self.fd
            .try_unwrap()
            .expect("unexpected multiple reference to rawfd")
    }
}
#[cfg(unix)]
impl AsRawFd for TcpStream {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.fd.raw_fd()
    }
}

#[cfg(windows)]
impl IntoRawSocket for TcpStream {
    #[inline]
    fn into_raw_socket(self) -> RawSocket {
        self.fd
            .try_unwrap()
            .expect("unexpected multiple reference to rawfd")
    }
}

#[cfg(windows)]
impl AsRawSocket for TcpStream {
    #[inline]
    fn as_raw_socket(&self) -> RawSocket {
        self.fd.raw_socket()
    }
}

impl std::fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpStream").field("fd", &self.fd).finish()
    }
}

impl AsyncWriteRent for TcpStream {
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
    fn flush(&mut self) -> impl Future<Output = std::io::Result<()>> {
        // Tcp stream does not need flush.
        std::future::ready(Ok(()))
    }

    fn shutdown(&mut self) -> impl Future<Output = std::io::Result<()>> {
        // We could use shutdown op here, which requires kernel 5.11+.
        // However, for simplicity, we just close the socket using direct syscall.
        #[cfg(unix)]
        let fd = self.as_raw_fd();
        #[cfg(windows)]
        let fd = self.as_raw_socket() as _;
        let res = match unsafe { shutdown(fd, SHUT_WR) } {
            -1 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        };
        std::future::ready(res)
    }
}

impl CancelableAsyncWriteRent for TcpStream {
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
        // Tcp stream does not need flush.
        Ok(())
    }

    fn cancelable_shutdown(&mut self, _c: CancelHandle) -> impl Future<Output = io::Result<()>> {
        // We could use shutdown op here, which requires kernel 5.11+.
        // However, for simplicity, we just close the socket using direct syscall.
        #[cfg(unix)]
        let fd = self.as_raw_fd();
        #[cfg(windows)]
        let fd = self.as_raw_socket() as _;
        let res = match unsafe { shutdown(fd, SHUT_WR) } {
            -1 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        };
        std::future::ready(res)
    }
}

impl AsyncReadRent for TcpStream {
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

impl CancelableAsyncReadRent for TcpStream {
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
impl tokio::io::AsyncRead for TcpStream {
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
impl tokio::io::AsyncWrite for TcpStream {
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

    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<Result<usize, io::Error>> {
        unsafe {
            let raw_buf =
                crate::buf::RawBufVectored::new(bufs.as_ptr() as *const libc::iovec, bufs.len());
            let mut writev = Op::writev_raw(&self.fd, raw_buf);
            let ret = ready!(crate::driver::op::PollLegacy::poll_legacy(&mut writev, cx));

            std::task::Poll::Ready(ret.result.map(|n| n.into_inner() as usize))
        }
    }

    fn is_write_vectored(&self) -> bool {
        true
    }
}

struct StreamMeta {
    socket: Option<socket2::Socket>,
    meta: UnsafeCell<Meta>,
}

#[derive(Debug, Default, Clone)]
struct Meta {
    local_addr: Option<SocketAddr>,
    peer_addr: Option<SocketAddr>,
}

impl StreamMeta {
    #[cfg(unix)]
    fn new(fd: RawFd) -> Self {
        Self {
            socket: unsafe { Some(socket2::Socket::from_raw_fd(fd)) },
            meta: Default::default(),
        }
    }

    /// When operating files, we should use RawHandle;
    /// When operating sockets, we should use RawSocket;
    #[cfg(windows)]
    fn new(fd: RawSocket) -> Self {
        Self {
            socket: unsafe { Some(socket2::Socket::from_raw_socket(fd)) },
            meta: Default::default(),
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        let meta = unsafe { &mut *self.meta.get() };
        if let Some(addr) = meta.local_addr {
            return Ok(addr);
        }

        let ret = self
            .socket
            .as_ref()
            .unwrap()
            .local_addr()
            .map(|addr| addr.as_socket().expect("tcp socket is expected"));
        if let Ok(addr) = ret {
            meta.local_addr = Some(addr);
        }
        ret
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        let meta = unsafe { &mut *self.meta.get() };
        if let Some(addr) = meta.peer_addr {
            return Ok(addr);
        }

        let ret = self
            .socket
            .as_ref()
            .unwrap()
            .peer_addr()
            .map(|addr| addr.as_socket().expect("tcp socket is expected"));
        if let Ok(addr) = ret {
            meta.peer_addr = Some(addr);
        }
        ret
    }

    fn no_delay(&self) -> io::Result<bool> {
        self.socket.as_ref().unwrap().nodelay()
    }

    fn set_no_delay(&self, no_delay: bool) -> io::Result<()> {
        self.socket.as_ref().unwrap().set_nodelay(no_delay)
    }

    #[allow(unused_variables)]
    fn set_tcp_keepalive(
        &self,
        time: Option<Duration>,
        interval: Option<Duration>,
        retries: Option<u32>,
    ) -> io::Result<()> {
        let mut t = socket2::TcpKeepalive::new();
        if let Some(time) = time {
            t = t.with_time(time)
        }
        if let Some(interval) = interval {
            t = t.with_interval(interval)
        }
        #[cfg(unix)]
        if let Some(retries) = retries {
            t = t.with_retries(retries)
        }
        self.socket.as_ref().unwrap().set_tcp_keepalive(&t)
    }

    #[cfg(feature = "zero-copy")]
    fn set_zero_copy(&self) {
        #[cfg(target_os = "linux")]
        unsafe {
            let fd = self.socket.as_ref().unwrap().as_raw_fd();
            let v: libc::c_int = 1;
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ZEROCOPY,
                &v as *const _ as *const _,
                std::mem::size_of::<libc::c_int>() as _,
            );
        }
    }
}

impl Drop for StreamMeta {
    fn drop(&mut self) {
        let socket = self.socket.take().unwrap();
        #[cfg(unix)]
        let _ = socket.into_raw_fd();
        #[cfg(windows)]
        let _ = socket.into_raw_socket();
    }
}
