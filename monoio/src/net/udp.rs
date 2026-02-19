//! UDP impl.

#[cfg(unix)]
use std::os::unix::prelude::{AsRawFd, FromRawFd, IntoRawFd};
#[cfg(windows)]
use std::os::windows::prelude::{AsRawSocket, FromRawSocket, IntoRawSocket, RawSocket};
use std::{
    io,
    net::{SocketAddr, ToSocketAddrs},
};

use crate::{
    buf::{IoBuf, IoBufMut},
    driver::{op::Op, shared_fd::SharedFd},
    io::{operation_canceled, CancelHandle, Split},
};

/// A UDP socket.
///
/// After creating a `UdpSocket` by [`bind`]ing it to a socket address, data can be
/// [sent to] and [received from] any other socket address.
///
/// Although UDP is a connectionless protocol, this implementation provides an interface
/// to set an address where data should be sent and received from. After setting a remote
/// address with [`connect`], data can be sent to and received from that address with
/// [`send`] and [`recv`].
#[derive(Debug)]
pub struct UdpSocket {
    fd: SharedFd,
}

/// UdpSocket is safe to split to two parts
unsafe impl Split for UdpSocket {}

impl UdpSocket {
    pub(crate) fn from_shared_fd(fd: SharedFd) -> Self {
        Self { fd }
    }

    #[cfg(feature = "legacy")]
    fn set_non_blocking(_socket: &socket2::Socket) -> io::Result<()> {
        crate::driver::CURRENT.with(|x| match x {
            #[cfg(all(target_os = "linux", feature = "iouring"))]
            crate::driver::Inner::Uring(_) => Ok(()),
            #[cfg(all(windows, feature = "iocp"))]
            crate::driver::Inner::Iocp(_) => Ok(()),
            crate::driver::Inner::Legacy(_) => _socket.set_nonblocking(true),
        })
    }

    /// Creates a UDP socket from the given address.
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        let addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::other("empty address"))?;
        let domain = if addr.is_ipv6() {
            socket2::Domain::IPV6
        } else {
            socket2::Domain::IPV4
        };
        let socket =
            socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
        #[cfg(feature = "legacy")]
        Self::set_non_blocking(&socket)?;

        let addr = socket2::SockAddr::from(addr);
        socket.bind(&addr)?;

        #[cfg(unix)]
        let fd = socket.into_raw_fd();
        #[cfg(windows)]
        let fd = socket.into_raw_socket();

        Ok(Self::from_shared_fd(SharedFd::new::<false>(fd)?))
    }

    /// Receives a single datagram message on the socket. On success, returns the number
    /// of bytes read and the origin.
    pub async fn recv_from<T: IoBufMut>(&self, buf: T) -> crate::BufResult<(usize, SocketAddr), T> {
        let op = Op::recv_msg(self.fd.clone(), buf).unwrap();
        op.wait().await
    }

    /// Sends data on the socket to the given address. On success, returns the
    /// number of bytes written.
    pub async fn send_to<T: IoBuf>(
        &self,
        buf: T,
        socket_addr: SocketAddr,
    ) -> crate::BufResult<usize, T> {
        let op = Op::send_msg(self.fd.clone(), buf, Some(socket_addr)).unwrap();
        op.wait().await
    }

    /// Returns the socket address of the remote peer this socket was connected to.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        #[cfg(unix)]
        let socket = unsafe { socket2::Socket::from_raw_fd(self.fd.as_raw_fd()) };
        #[cfg(windows)]
        let socket = unsafe { socket2::Socket::from_raw_socket(self.fd.as_raw_socket()) };
        let addr = socket.peer_addr();
        #[cfg(unix)]
        let _ = socket.into_raw_fd();
        #[cfg(windows)]
        let _ = socket.into_raw_socket();
        addr?
            .as_socket()
            .ok_or_else(|| io::ErrorKind::InvalidInput.into())
    }

    /// Returns the socket address that this socket was created from.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        #[cfg(unix)]
        let socket = unsafe { socket2::Socket::from_raw_fd(self.fd.as_raw_fd()) };
        #[cfg(windows)]
        let socket = unsafe { socket2::Socket::from_raw_socket(self.fd.as_raw_socket()) };
        let addr = socket.local_addr();
        #[cfg(unix)]
        let _ = socket.into_raw_fd();
        #[cfg(windows)]
        let _ = socket.into_raw_socket();
        addr?
            .as_socket()
            .ok_or_else(|| io::ErrorKind::InvalidInput.into())
    }

    /// Connects this UDP socket to a remote address, allowing the `send` and
    /// `recv` syscalls to be used to send data and also applies filters to only
    /// receive data from the specified address.
    pub async fn connect(&self, socket_addr: SocketAddr) -> io::Result<()> {
        let op = Op::connect(self.fd.clone(), socket_addr, false)?;
        let completion = op.await;
        completion.meta.result?;
        Ok(())
    }

    /// Sends data on the socket to the remote address to which it is connected.
    pub async fn send<T: IoBuf>(&self, buf: T) -> crate::BufResult<usize, T> {
        let op = Op::send_msg(self.fd.clone(), buf, None).unwrap();
        op.wait().await
    }

    /// Receives a single datagram message on the socket from the remote address to
    /// which it is connected. On success, returns the number of bytes read.
    pub async fn recv<T: IoBufMut>(&self, buf: T) -> crate::BufResult<usize, T> {
        let op = Op::recv(self.fd.clone(), buf).unwrap();
        op.result().await
    }

    /// Creates new `UdpSocket` from a `std::net::UdpSocket`.
    pub fn from_std(socket: std::net::UdpSocket) -> io::Result<Self> {
        #[cfg(unix)]
        let fd = socket.as_raw_fd();
        #[cfg(windows)]
        let fd = socket.as_raw_socket();
        match SharedFd::new::<false>(fd) {
            Ok(shared) => {
                #[cfg(unix)]
                let _ = socket.into_raw_fd();
                #[cfg(windows)]
                let _ = socket.into_raw_socket();
                Ok(Self::from_shared_fd(shared))
            }
            Err(e) => Err(e),
        }
    }

    /// Set value for the `SO_REUSEADDR` option on this socket.
    #[allow(unused_variables)]
    pub fn set_reuse_address(&self, reuse: bool) -> io::Result<()> {
        #[cfg(unix)]
        let r = {
            let socket = unsafe { socket2::Socket::from_raw_fd(self.fd.as_raw_fd()) };
            let r = socket.set_reuse_address(reuse);
            let _ = socket.into_raw_fd();
            r
        };
        #[cfg(windows)]
        let r = {
            let socket = unsafe { socket2::Socket::from_raw_socket(self.fd.as_raw_socket()) };
            let _ = socket.into_raw_socket();
            Ok(())
        };
        r
    }

    /// Set value for the `SO_REUSEPORT` option on this socket.
    #[allow(unused_variables)]
    pub fn set_reuse_port(&self, reuse: bool) -> io::Result<()> {
        #[cfg(unix)]
        let r = {
            let socket = unsafe { socket2::Socket::from_raw_fd(self.fd.as_raw_fd()) };
            let r = socket.set_reuse_port(reuse);
            let _ = socket.into_raw_fd();
            r
        };
        #[cfg(windows)]
        let r = {
            let socket = unsafe { socket2::Socket::from_raw_socket(self.fd.as_raw_socket()) };
            let _ = socket.into_raw_socket();
            Ok(())
        };
        r
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

#[cfg(unix)]
impl AsRawFd for UdpSocket {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.fd.raw_fd()
    }
}

#[cfg(windows)]
impl AsRawSocket for UdpSocket {
    fn as_raw_socket(&self) -> RawSocket {
        self.fd.raw_socket()
    }
}

/// Cancelable related methods
impl UdpSocket {
    /// Receives a single datagram message on the socket. On success, returns the number
    /// of bytes read and the origin.
    pub async fn cancelable_recv_from<T: IoBufMut>(
        &self,
        buf: T,
        c: CancelHandle,
    ) -> crate::BufResult<(usize, SocketAddr), T> {
        if c.canceled() {
            return (Err(operation_canceled()), buf);
        }

        let op = Op::recv_msg(self.fd.clone(), buf).unwrap();
        let _guard = c.associate_op(op.op_canceller());
        op.wait().await
    }

    /// Sends data on the socket to the given address. On success, returns the
    /// number of bytes written.
    pub async fn cancelable_send_to<T: IoBuf>(
        &self,
        buf: T,
        socket_addr: SocketAddr,
        c: CancelHandle,
    ) -> crate::BufResult<usize, T> {
        if c.canceled() {
            return (Err(operation_canceled()), buf);
        }

        let op = Op::send_msg(self.fd.clone(), buf, Some(socket_addr)).unwrap();
        let _guard = c.associate_op(op.op_canceller());
        op.wait().await
    }

    /// Sends data on the socket to the remote address to which it is connected.
    pub async fn cancelable_send<T: IoBuf>(
        &self,
        buf: T,
        c: CancelHandle,
    ) -> crate::BufResult<usize, T> {
        if c.canceled() {
            return (Err(operation_canceled()), buf);
        }

        let op = Op::send_msg(self.fd.clone(), buf, None).unwrap();
        let _guard = c.associate_op(op.op_canceller());
        op.wait().await
    }

    /// Receives a single datagram message on the socket from the remote address to
    /// which it is connected. On success, returns the number of bytes read.
    pub async fn cancelable_recv<T: IoBufMut>(
        &self,
        buf: T,
        c: CancelHandle,
    ) -> crate::BufResult<usize, T> {
        if c.canceled() {
            return (Err(operation_canceled()), buf);
        }

        let op = Op::recv(self.fd.clone(), buf).unwrap();
        let _guard = c.associate_op(op.op_canceller());
        op.result().await
    }
}
