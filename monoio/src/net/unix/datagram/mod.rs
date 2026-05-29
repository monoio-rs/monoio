//! Unix datagram related.

use std::{
    io,
    os::unix::{
        net::UnixDatagram as StdUnixDatagram,
        prelude::{AsRawFd, IntoRawFd, RawFd},
    },
    path::Path,
};

use super::{
    socket_addr::{local_addr, pair, peer_addr, socket_addr},
    SocketAddr,
};
use crate::{
    buf::{IoBuf, IoBufMut},
    driver::{op::Op, shared_fd::SharedFd},
    net::new_socket,
};

/// UnixDatagram
pub struct UnixDatagram {
    fd: SharedFd,
}

impl UnixDatagram {
    pub(crate) fn from_shared_fd(fd: SharedFd) -> Self {
        Self { fd }
    }

    /// Creates a Unix datagram socket bound to the given path.
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        StdUnixDatagram::bind(path).and_then(Self::from_std)
    }

    /// Creates a new `UnixDatagram` which is not bound to any address.
    pub fn unbound() -> io::Result<Self> {
        StdUnixDatagram::unbound().and_then(Self::from_std)
    }

    /// Creates an unnamed pair of connected sockets.
    pub fn pair() -> io::Result<(Self, Self)> {
        let (a, b) = pair(libc::SOCK_DGRAM)?;
        Ok((Self::from_std(a)?, Self::from_std(b)?))
    }

    /// Connects the socket to the specified address.
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
        let socket = new_socket(socket2::Domain::UNIX, socket2::Type::DGRAM).await?;
        let op = Op::connect_unix(SharedFd::new::<false>(socket)?, sockaddr, socklen)?;
        let completion = op.await;
        completion.meta.result?;

        Ok(Self::from_shared_fd(completion.data.fd))
    }

    /// Creates new `UnixDatagram` from a `std::os::unix::net::UnixDatagram`.
    pub fn from_std(datagram: StdUnixDatagram) -> io::Result<Self> {
        match SharedFd::new::<false>(datagram.as_raw_fd()) {
            Ok(shared) => {
                let _ = datagram.into_raw_fd();
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

    /// Sends data on the socket to the given address. On success, returns the
    /// number of bytes written.
    pub async fn send_to<T: IoBuf, P: AsRef<Path>>(
        &self,
        buf: T,
        path: P,
    ) -> crate::BufResult<usize, T> {
        let addr = match crate::net::unix::socket_addr::socket_addr(path.as_ref()) {
            Ok(addr) => addr,
            Err(e) => return (Err(e), buf),
        };
        let op = Op::send_msg_unix(
            self.fd.clone(),
            buf,
            Some(SocketAddr::from_parts(addr.0, addr.1)),
        )
        .unwrap();
        op.wait().await
    }

    /// Receives a single datagram message on the socket. On success, returns the number
    /// of bytes read and the origin.
    pub async fn recv_from<T: IoBufMut>(&self, buf: T) -> crate::BufResult<(usize, SocketAddr), T> {
        let op = Op::recv_msg_unix(self.fd.clone(), buf).unwrap();
        op.wait().await
    }

    /// Sends data on the socket to the remote address to which it is connected.
    pub async fn send<T: IoBuf>(&self, buf: T) -> crate::BufResult<usize, T> {
        let op = Op::send_msg_unix(self.fd.clone(), buf, None).unwrap();
        op.wait().await
    }

    /// Receives a single datagram message on the socket from the remote address to
    /// which it is connected. On success, returns the number of bytes read.
    pub async fn recv<T: IoBufMut>(&self, buf: T) -> crate::BufResult<usize, T> {
        let op = Op::recv(self.fd.clone(), buf).unwrap();
        op.result().await
    }
}

impl AsRawFd for UnixDatagram {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.fd.raw_fd()
    }
}

impl std::fmt::Debug for UnixDatagram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnixDatagram")
            .field("fd", &self.fd)
            .finish()
    }
}
