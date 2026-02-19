use std::{
    cell::UnsafeCell,
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, ToSocketAddrs},
};

#[cfg(unix)]
use {
    libc::{sockaddr_in, sockaddr_in6, AF_INET, AF_INET6},
    std::os::unix::prelude::{AsRawFd, FromRawFd, IntoRawFd, RawFd},
};
#[cfg(windows)]
use {
    std::os::windows::prelude::{AsRawSocket, FromRawSocket, IntoRawSocket, RawSocket},
    windows_sys::Win32::Networking::WinSock::{
        AF_INET, AF_INET6, SOCKADDR_IN as sockaddr_in, SOCKADDR_IN6 as sockaddr_in6,
    },
};

use super::stream::TcpStream;
use crate::{
    driver::{op::Op, shared_fd::SharedFd},
    io::{stream::Stream, CancelHandle},
    net::ListenerOpts,
};

/// TcpListener
pub struct TcpListener {
    fd: SharedFd,
    sys_listener: Option<std::net::TcpListener>,
    meta: UnsafeCell<ListenerMeta>,
}

impl TcpListener {
    #[allow(unreachable_code, clippy::diverging_sub_expression, unused_variables)]
    pub(crate) fn from_shared_fd(fd: SharedFd) -> Self {
        #[cfg(unix)]
        let sys_listener = unsafe { std::net::TcpListener::from_raw_fd(fd.raw_fd()) };
        #[cfg(windows)]
        let sys_listener = unsafe { std::net::TcpListener::from_raw_socket(fd.raw_socket()) };
        Self {
            fd,
            sys_listener: Some(sys_listener),
            meta: UnsafeCell::new(ListenerMeta::default()),
        }
    }

    /// Bind to address with config
    pub fn bind_with_config<A: ToSocketAddrs>(addr: A, opts: &ListenerOpts) -> io::Result<Self> {
        let addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::other("empty address"))?;

        let domain = if addr.is_ipv6() {
            socket2::Domain::IPV6
        } else {
            socket2::Domain::IPV4
        };
        let sys_listener =
            socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;

        #[cfg(feature = "legacy")]
        Self::set_non_blocking(&sys_listener)?;

        let addr = socket2::SockAddr::from(addr);
        #[cfg(unix)]
        if opts.reuse_port {
            sys_listener.set_reuse_port(true)?;
        }
        if opts.reuse_addr {
            sys_listener.set_reuse_address(true)?;
        }
        if let Some(send_buf_size) = opts.send_buf_size {
            sys_listener.set_send_buffer_size(send_buf_size)?;
        }
        if let Some(recv_buf_size) = opts.recv_buf_size {
            sys_listener.set_recv_buffer_size(recv_buf_size)?;
        }
        if opts.tcp_fast_open {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            super::tfo::set_tcp_fastopen(&sys_listener, opts.backlog)?;
            #[cfg(any(target_os = "ios", target_os = "macos"))]
            let _ = super::tfo::set_tcp_fastopen_force_enable(&sys_listener);
        }
        sys_listener.bind(&addr)?;
        sys_listener.listen(opts.backlog)?;

        #[cfg(any(target_os = "ios", target_os = "macos"))]
        if opts.tcp_fast_open {
            super::tfo::set_tcp_fastopen(&sys_listener)?;
        }

        #[cfg(unix)]
        let fd = sys_listener.into_raw_fd();

        #[cfg(windows)]
        let fd = sys_listener.into_raw_socket();

        Ok(Self::from_shared_fd(SharedFd::new::<false>(fd)?))
    }

    /// Bind to address
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        const DEFAULT_CFG: ListenerOpts = ListenerOpts::new();
        Self::bind_with_config(addr, &DEFAULT_CFG)
    }

    /// Accept
    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let op = Op::accept(&self.fd)?;

        // Await the completion of the event
        let completion = op.await;

        // Convert fd
        let fd = completion.meta.result?;

        // Construct stream
        let stream = TcpStream::from_shared_fd(SharedFd::new::<false>(fd.into_inner() as _)?);

        // Construct SocketAddr
        let storage = completion.data.addr.0.as_ptr();
        let addr = unsafe {
            match (*storage).ss_family as _ {
                AF_INET => {
                    // Safety: if the ss_family field is AF_INET then storage must be a sockaddr_in.
                    let addr: &sockaddr_in = &*(storage as *const sockaddr_in);
                    #[cfg(unix)]
                    let ip = Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes());
                    #[cfg(windows)]
                    let ip = Ipv4Addr::from(addr.sin_addr.S_un.S_addr.to_ne_bytes());
                    let port = u16::from_be(addr.sin_port);
                    SocketAddr::V4(SocketAddrV4::new(ip, port))
                }
                AF_INET6 => {
                    // Safety: if the ss_family field is AF_INET6 then storage must be a
                    // sockaddr_in6.
                    let addr: &sockaddr_in6 = &*(storage as *const sockaddr_in6);
                    #[cfg(unix)]
                    let ip = Ipv6Addr::from(addr.sin6_addr.s6_addr);
                    #[cfg(windows)]
                    let ip = Ipv6Addr::from(addr.sin6_addr.u.Byte);
                    let port = u16::from_be(addr.sin6_port);
                    #[cfg(unix)]
                    let scope_id = addr.sin6_scope_id;
                    #[cfg(windows)]
                    let scope_id = addr.Anonymous.sin6_scope_id;
                    SocketAddr::V6(SocketAddrV6::new(ip, port, addr.sin6_flowinfo, scope_id))
                }
                _ => {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
            }
        };

        Ok((stream, addr))
    }

    /// Cancelable accept
    pub async fn cancelable_accept(&self, c: CancelHandle) -> io::Result<(TcpStream, SocketAddr)> {
        use crate::io::operation_canceled;

        if c.canceled() {
            return Err(operation_canceled());
        }
        let op = Op::accept(&self.fd)?;
        let _guard = c.associate_op(op.op_canceller());

        // Await the completion of the event
        let completion = op.await;

        // Convert fd
        let fd = completion.meta.result?;

        // Construct stream
        let stream = TcpStream::from_shared_fd(SharedFd::new::<false>(fd.into_inner() as _)?);

        // Construct SocketAddr
        let storage = completion.data.addr.0.as_ptr();
        let addr = unsafe {
            match (*storage).ss_family as _ {
                AF_INET => {
                    // Safety: if the ss_family field is AF_INET then storage must be a sockaddr_in.
                    let addr: &sockaddr_in = &*(storage as *const sockaddr_in);
                    #[cfg(unix)]
                    let ip = Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes());
                    #[cfg(windows)]
                    let ip = Ipv4Addr::from(addr.sin_addr.S_un.S_addr.to_ne_bytes());
                    let port = u16::from_be(addr.sin_port);
                    SocketAddr::V4(SocketAddrV4::new(ip, port))
                }
                AF_INET6 => {
                    // Safety: if the ss_family field is AF_INET6 then storage must be a
                    // sockaddr_in6.
                    let addr: &sockaddr_in6 = &*(storage as *const sockaddr_in6);
                    #[cfg(unix)]
                    let ip = Ipv6Addr::from(addr.sin6_addr.s6_addr);
                    #[cfg(windows)]
                    let ip = Ipv6Addr::from(addr.sin6_addr.u.Byte);
                    let port = u16::from_be(addr.sin6_port);
                    #[cfg(unix)]
                    let scope_id = addr.sin6_scope_id;
                    #[cfg(windows)]
                    let scope_id = addr.Anonymous.sin6_scope_id;
                    SocketAddr::V6(SocketAddrV6::new(ip, port, addr.sin6_flowinfo, scope_id))
                }
                _ => {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
            }
        };

        Ok((stream, addr))
    }

    /// Returns the local address that this listener is bound to.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        let meta = self.meta.get();
        if let Some(addr) = unsafe { &*meta }.local_addr {
            return Ok(addr);
        }
        self.sys_listener
            .as_ref()
            .unwrap()
            .local_addr()
            .inspect(|&addr| {
                unsafe { &mut *meta }.local_addr = Some(addr);
            })
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

    /// Creates new `TcpListener` from a `std::net::TcpListener`.
    pub fn from_std(stdl: std::net::TcpListener) -> io::Result<Self> {
        #[cfg(unix)]
        let fd = stdl.as_raw_fd();
        #[cfg(windows)]
        let fd = stdl.as_raw_socket();
        match SharedFd::new::<false>(fd) {
            Ok(shared) => {
                #[cfg(unix)]
                let _ = stdl.into_raw_fd();
                #[cfg(windows)]
                let _ = stdl.into_raw_socket();
                Ok(Self::from_shared_fd(shared))
            }
            Err(e) => Err(e),
        }
    }
}

impl Stream for TcpListener {
    type Item = io::Result<(TcpStream, SocketAddr)>;

    #[inline]
    async fn next(&mut self) -> Option<Self::Item> {
        Some(self.accept().await)
    }
}

impl std::fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpListener").field("fd", &self.fd).finish()
    }
}

#[cfg(unix)]
impl AsRawFd for TcpListener {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.fd.raw_fd()
    }
}

#[cfg(windows)]
impl AsRawSocket for TcpListener {
    #[inline]
    fn as_raw_socket(&self) -> RawSocket {
        self.fd.raw_socket()
    }
}

impl Drop for TcpListener {
    #[inline]
    fn drop(&mut self) {
        let listener = self.sys_listener.take().unwrap();
        #[cfg(unix)]
        let _ = listener.into_raw_fd();
        #[cfg(windows)]
        let _ = listener.into_raw_socket();
    }
}

#[derive(Debug, Default, Clone)]
struct ListenerMeta {
    local_addr: Option<SocketAddr>,
}
