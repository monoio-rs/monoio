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
            // TODO: windows ioring support
            #[cfg(all(target_os = "linux", feature = "iouring"))]
            crate::driver::Inner::Uring(_) => Ok(()),
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

#[cfg(all(target_os = "linux", feature = "iouring"))]
mod multishot_impl {
    use std::io;

    use super::UdpSocket;
    use crate::{
        buf::{RecvMsgParser, RecvMsgRingBuf, RingBuf},
        driver::op::{
            recv_multishot::{RecvMsgMultishotOp, RecvMultishotOp},
            MultishotOp,
        },
        io::{is_operation_canceled, AssociateGuard, CancelHandle},
    };

    /// Multishot recv for connected sockets.
    ///
    /// Call [`stream()`](Self::stream) to get a [`RecvMultishotStream`] for receiving buffers.
    pub struct RecvMultishot<R: RingBuf + Unpin + 'static> {
        ring: R,
        op: MultishotOp<RecvMultishotOp>,
        cancellation_guard: Option<AssociateGuard>,
    }

    /// Stream for receiving buffers from a [`RecvMultishot`] operation.
    ///
    /// Holds a mutable borrow of the parent, preventing concurrent polling.
    /// Buffers returned by `next()` can be held across multiple calls.
    pub struct RecvMultishotStream<'a, R: RingBuf + Unpin + 'static> {
        ring: &'a R,
        op: &'a mut MultishotOp<RecvMultishotOp>,
        cancellation_guard: &'a Option<AssociateGuard>,
    }

    impl<R: RingBuf + Unpin + 'static> RecvMultishot<R> {
        /// Creates a stream for receiving buffers.
        ///
        /// The stream holds a mutable borrow, preventing concurrent polling.
        /// Buffers returned by the stream can outlive individual `next()` calls.
        #[inline]
        pub fn stream(&mut self) -> RecvMultishotStream<'_, R> {
            RecvMultishotStream {
                ring: &self.ring,
                op: &mut self.op,
                cancellation_guard: &self.cancellation_guard,
            }
        }

        /// Returns whether the multishot operation has terminated.
        #[inline]
        pub fn is_terminated(&self) -> bool {
            self.op.is_terminated()
        }

        /// Attempts to consume and return the buffer ring.
        ///
        /// Returns `Ok(ring)` if the multishot operation has terminated.
        /// Returns `Err(self)` if the operation is still in progress.
        pub fn try_into_ring(self) -> Result<R, Self> {
            if self.op.is_terminated() {
                Ok(self.ring)
            } else {
                Err(self)
            }
        }
    }

    impl<'a, R: RingBuf + Unpin + 'static> RecvMultishotStream<'a, R> {
        /// Receives the next buffer.
        ///
        /// Returns `None` when the operation is terminated or cancelled.
        /// The returned buffer has lifetime `'a`, allowing multiple buffers
        /// to be held simultaneously across `next()` calls.
        pub async fn next(&mut self) -> Option<io::Result<R::Buffer<'a>>> {
            let completion = match self.op.poll_next_async().await? {
                Ok(c) => c,
                Err(e) => {
                    if self.cancellation_guard.is_some() && is_operation_canceled(&e) {
                        return None;
                    }
                    return Some(Err(e));
                }
            };

            let buf_id = match completion.buffer_id() {
                Some(id) => id,
                None => {
                    return Some(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "multishot completion missing buffer ID",
                    )))
                }
            };

            let len = completion.value.into_inner() as usize;
            Some(Ok(unsafe { self.ring.get_buf(buf_id, len) }))
        }
    }

    /// Multishot recvmsg yielding `(Address, Buffer)` pairs.
    ///
    /// Call [`stream()`](Self::stream) to get a [`RecvMsgMultishotStream`] for receiving messages.
    pub struct RecvMsgMultishot<R: RecvMsgRingBuf + Unpin + 'static> {
        ring: R,
        op: MultishotOp<RecvMsgMultishotOp>,
        cancellation_guard: Option<AssociateGuard>,
    }

    /// Stream for receiving messages from a [`RecvMsgMultishot`] operation.
    ///
    /// Holds a mutable borrow of the parent, preventing concurrent polling.
    /// Buffers returned by `next()` can be held across multiple calls.
    pub struct RecvMsgMultishotStream<'a, R: RecvMsgRingBuf + Unpin + 'static> {
        ring: &'a R,
        op: &'a mut MultishotOp<RecvMsgMultishotOp>,
        cancellation_guard: &'a Option<AssociateGuard>,
    }

    impl<R: RecvMsgRingBuf + Unpin + 'static> RecvMsgMultishot<R> {
        /// Creates a stream for receiving messages.
        ///
        /// The stream holds a mutable borrow, preventing concurrent polling.
        /// Buffers returned by the stream can outlive individual `next()` calls.
        #[inline]
        pub fn stream(&mut self) -> RecvMsgMultishotStream<'_, R> {
            RecvMsgMultishotStream {
                ring: &self.ring,
                op: &mut self.op,
                cancellation_guard: &self.cancellation_guard,
            }
        }

        /// Returns whether the multishot operation has terminated.
        #[inline]
        pub fn is_terminated(&self) -> bool {
            self.op.is_terminated()
        }

        /// Attempts to consume and return the buffer ring.
        ///
        /// Returns `Ok(ring)` if the multishot operation has terminated.
        /// Returns `Err(self)` if the operation is still in progress.
        pub fn try_into_ring(self) -> Result<R, Self> {
            if self.op.is_terminated() {
                Ok(self.ring)
            } else {
                Err(self)
            }
        }
    }

    impl<'a, R: RecvMsgRingBuf + Unpin + 'static> RecvMsgMultishotStream<'a, R> {
        /// Receives the next `(address, buffer)` pair.
        ///
        /// Returns `None` when the operation is terminated or cancelled.
        pub async fn next(
            &mut self,
        ) -> Option<io::Result<(<R::Parser as RecvMsgParser>::Address, R::Buffer<'a>)>> {
            let completion = match self.op.poll_next_async().await? {
                Ok(c) => c,
                Err(e) => {
                    if self.cancellation_guard.is_some() && is_operation_canceled(&e) {
                        return None;
                    }
                    return Some(Err(e));
                }
            };

            let buf_id = match completion.buffer_id() {
                Some(id) => id,
                None => {
                    return Some(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "multishot completion missing buffer ID",
                    )))
                }
            };

            let total_len = completion.value.into_inner() as usize;
            let msghdr: &libc::msghdr = &self.op.data().expect("op data").msghdr;

            let (addr, buf) = match unsafe { self.ring.parse_recvmsg(buf_id, total_len, msghdr) } {
                Ok(result) => result,
                Err(e) => return Some(Err(e)),
            };

            Some(Ok((addr, buf)))
        }
    }

    impl UdpSocket {
        /// Multishot recv for connected sockets. Socket must be connected first.
        pub fn recv_multishot<R: RingBuf + Unpin + 'static>(
            &self,
            ring: R,
        ) -> io::Result<RecvMultishot<R>> {
            let buf_count = ring.buffer_count();
            let bgid = ring.bgid();
            let op = RecvMultishotOp::new(self.fd.clone(), bgid);
            let op = MultishotOp::new(op, buf_count)?;
            Ok(RecvMultishot {
                ring,
                op,
                cancellation_guard: None,
            })
        }

        /// Multishot recv with cancellation support.
        pub fn cancelable_recv_multishot<R: RingBuf + Unpin + 'static>(
            &self,
            ring: R,
            c: CancelHandle,
        ) -> io::Result<RecvMultishot<R>> {
            let buf_count = ring.buffer_count();
            let bgid = ring.bgid();
            let op = RecvMultishotOp::new(self.fd.clone(), bgid);
            let op = MultishotOp::new(op, buf_count)?;
            let cancellation_guard = c.associate_op(op.op_canceller());
            Ok(RecvMultishot {
                ring,
                op,
                cancellation_guard: Some(cancellation_guard),
            })
        }

        /// Multishot recvmsg returning sender address and payload.
        pub fn recvmsg_multishot<R: RecvMsgRingBuf + Unpin + 'static>(
            &self,
            ring: R,
        ) -> io::Result<RecvMsgMultishot<R>> {
            let buf_count = ring.buffer_count();
            let bgid = ring.bgid();
            let op = RecvMsgMultishotOp::new::<R::Parser>(self.fd.clone(), bgid);
            let op = MultishotOp::new(op, buf_count)?;
            Ok(RecvMsgMultishot {
                ring,
                op,
                cancellation_guard: None,
            })
        }

        /// Multishot recvmsg with cancellation support.
        pub fn cancelable_recvmsg_multishot<R: RecvMsgRingBuf + Unpin + 'static>(
            &self,
            ring: R,
            c: CancelHandle,
        ) -> io::Result<RecvMsgMultishot<R>> {
            let buf_count = ring.buffer_count();
            let bgid = ring.bgid();
            let op = RecvMsgMultishotOp::new::<R::Parser>(self.fd.clone(), bgid);
            let op = MultishotOp::new(op, buf_count)?;
            let cancellation_guard = c.associate_op(op.op_canceller());
            Ok(RecvMsgMultishot {
                ring,
                op,
                cancellation_guard: Some(cancellation_guard),
            })
        }
    }
}

#[cfg(all(target_os = "linux", feature = "iouring"))]
pub use multishot_impl::{
    RecvMsgMultishot, RecvMsgMultishotStream, RecvMultishot, RecvMultishotStream,
};

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
