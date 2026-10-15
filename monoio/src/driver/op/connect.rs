use std::{io, net::SocketAddr};

#[cfg(all(target_os = "linux", feature = "iouring"))]
use io_uring::{opcode, types};
#[cfg(windows)]
use windows_sys::Win32::Networking::WinSock::{
    connect, socklen_t, AF_INET, AF_INET6, IN6_ADDR, IN6_ADDR_0, IN_ADDR, IN_ADDR_0, SOCKADDR_IN,
    SOCKADDR_IN6, SOCKADDR_IN6_0, SOCKET_ERROR,
};

use super::{super::shared_fd::SharedFd, Op, OpAble};
#[cfg(any(feature = "legacy", feature = "poll-io"))]
use super::{driver::ready::Direction, MaybeFd};

pub(crate) struct Connect {
    pub(crate) fd: SharedFd,
    socket_addr: Box<SocketAddrCRepr>,
    #[cfg(windows)]
    socket_addr_len: socklen_t,
    #[cfg(unix)]
    socket_addr_len: libc::socklen_t,
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    tfo: bool,
}

impl Op<Connect> {
    /// Submit a request to connect.
    pub(crate) fn connect(
        socket: SharedFd,
        addr: SocketAddr,
        _tfo: bool,
    ) -> io::Result<Op<Connect>> {
        let (raw_addr, raw_addr_length) = socket_addr(&addr);
        Op::submit_with(Connect {
            fd: socket,
            socket_addr: Box::new(raw_addr),
            socket_addr_len: raw_addr_length,
            #[cfg(any(target_os = "ios", target_os = "macos"))]
            tfo: _tfo,
        })
    }
}

impl OpAble for Connect {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        opcode::Connect::new(
            types::Fd(self.fd.raw_fd()),
            self.socket_addr.as_ptr(),
            self.socket_addr_len,
        )
        .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        None
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        // For ios/macos, if tfo is enabled, we will
        // call connectx here.
        // For linux/android, we have already set socket
        // via set_tcp_fastopen_connect.
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        if self.tfo {
            let mut endpoints: libc::sa_endpoints_t = unsafe { std::mem::zeroed() };
            endpoints.sae_dstaddr = self.socket_addr.as_ptr();
            endpoints.sae_dstaddrlen = self.socket_addr_len;

            return match crate::syscall!(connectx@RAW(
                self.fd.raw_fd(),
                &endpoints as *const _,
                libc::SAE_ASSOCID_ANY,
                libc::CONNECT_DATA_IDEMPOTENT | libc::CONNECT_RESUME_ON_READ_WRITE,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )) {
                Err(err) if err.raw_os_error() != Some(libc::EINPROGRESS) => Err(err),
                _ => Ok(MaybeFd::zero()),
            };
        }

        #[cfg(unix)]
        match crate::syscall!(connect@RAW(
            self.fd.raw_fd(),
            self.socket_addr.as_ptr(),
            self.socket_addr_len,
        )) {
            Err(err) if err.raw_os_error() != Some(libc::EINPROGRESS) => Err(err),
            _ => Ok(MaybeFd::zero()),
        }

        #[cfg(windows)]
        {
            let res = unsafe {
                connect(
                    self.fd.raw_socket() as _,
                    self.socket_addr.as_ptr().cast(),
                    self.socket_addr_len,
                )
            };
            if res == SOCKET_ERROR {
                let err = io::Error::last_os_error();
                if err.kind() != io::ErrorKind::WouldBlock {
                    return Err(err);
                }
            }
            Ok(MaybeFd::zero())
        }
    }
}

#[cfg(unix)]
pub(crate) struct ConnectUnix {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    pub(crate) fd: SharedFd,
    socket_addr: Box<(libc::sockaddr_un, libc::socklen_t)>,
}

#[cfg(unix)]
impl Op<ConnectUnix> {
    /// Submit a request to connect.
    pub(crate) fn connect_unix(
        socket: SharedFd,
        socket_addr: libc::sockaddr_un,
        socket_len: libc::socklen_t,
    ) -> io::Result<Op<ConnectUnix>> {
        Op::submit_with(ConnectUnix {
            fd: socket,
            socket_addr: Box::new((socket_addr, socket_len)),
        })
    }
}

#[cfg(unix)]
impl OpAble for ConnectUnix {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        opcode::Connect::new(
            types::Fd(self.fd.raw_fd()),
            &self.socket_addr.0 as *const _ as *const _,
            self.socket_addr.1,
        )
        .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        None
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        match crate::syscall!(connect@RAW(
            self.fd.raw_fd(),
            &self.socket_addr.0 as *const _ as *const _,
            self.socket_addr.1
        )) {
            Err(err) if err.raw_os_error() != Some(libc::EINPROGRESS) => Err(err),
            _ => Ok(MaybeFd::zero()),
        }
    }
}

/// A type with the same memory layout as `libc::sockaddr`. Used in converting Rust level
/// SocketAddr* types into their system representation. The benefit of this specific
/// type over using `libc::sockaddr_storage` is that this type is exactly as large as it
/// needs to be and not a lot larger. And it can be initialized cleaner from Rust.
// Copied from mio.
#[repr(C)]
pub(crate) union SocketAddrCRepr {
    #[cfg(unix)]
    v4: libc::sockaddr_in,
    #[cfg(unix)]
    v6: libc::sockaddr_in6,
    #[cfg(windows)]
    v4: SOCKADDR_IN,
    #[cfg(windows)]
    v6: SOCKADDR_IN6,
}

impl SocketAddrCRepr {
    pub(crate) fn as_ptr(&self) -> *const libc::sockaddr {
        self as *const _ as *const libc::sockaddr
    }
}

#[cfg(windows)]
pub(crate) fn socket_addr(addr: &SocketAddr) -> (SocketAddrCRepr, i32) {
    match addr {
        SocketAddr::V4(addr) => {
            // `s_addr` is stored as BE on all machine and the array is in BE order.
            // So the native endian conversion method is used so that it's never swapped.
            let sin_addr = unsafe {
                let mut s_un = std::mem::zeroed::<IN_ADDR_0>();
                s_un.S_addr = u32::from_ne_bytes(addr.ip().octets());
                IN_ADDR { S_un: s_un }
            };

            let sockaddr_in = SOCKADDR_IN {
                sin_family: AF_INET, // 1
                sin_port: addr.port().to_be(),
                sin_addr,
                sin_zero: [0; 8],
            };

            let sockaddr = SocketAddrCRepr { v4: sockaddr_in };
            (sockaddr, std::mem::size_of::<SOCKADDR_IN>() as i32)
        }
        SocketAddr::V6(addr) => {
            let sin6_addr = unsafe {
                let mut u = std::mem::zeroed::<IN6_ADDR_0>();
                u.Byte = addr.ip().octets();
                IN6_ADDR { u }
            };
            let u = unsafe {
                let mut u = std::mem::zeroed::<SOCKADDR_IN6_0>();
                u.sin6_scope_id = addr.scope_id();
                u
            };

            let sockaddr_in6 = SOCKADDR_IN6 {
                sin6_family: AF_INET6, // 23
                sin6_port: addr.port().to_be(),
                sin6_addr,
                sin6_flowinfo: addr.flowinfo(),
                Anonymous: u,
            };

            let sockaddr = SocketAddrCRepr { v6: sockaddr_in6 };
            (sockaddr, std::mem::size_of::<SOCKADDR_IN6>() as i32)
        }
    }
}

#[cfg(unix)]
/// Converts a Rust `SocketAddr` into the system representation.
pub(crate) fn socket_addr(addr: &SocketAddr) -> (SocketAddrCRepr, libc::socklen_t) {
    match addr {
        SocketAddr::V4(addr) => {
            // `s_addr` is stored as BE on all machine and the array is in BE order.
            // So the native endian conversion method is used so that it's never swapped.
            let sin_addr = libc::in_addr {
                s_addr: u32::from_ne_bytes(addr.ip().octets()),
            };

            let sockaddr_in = libc::sockaddr_in {
                sin_family: libc::AF_INET as libc::sa_family_t,
                sin_port: addr.port().to_be(),
                sin_addr,
                sin_zero: [0; 8],
                #[cfg(any(
                    target_os = "dragonfly",
                    target_os = "freebsd",
                    target_os = "ios",
                    target_os = "macos",
                    target_os = "netbsd",
                    target_os = "openbsd"
                ))]
                sin_len: 0,
            };

            let sockaddr = SocketAddrCRepr { v4: sockaddr_in };
            let socklen = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
            (sockaddr, socklen)
        }
        SocketAddr::V6(addr) => {
            let sockaddr_in6 = libc::sockaddr_in6 {
                sin6_family: libc::AF_INET6 as libc::sa_family_t,
                sin6_port: addr.port().to_be(),
                sin6_addr: libc::in6_addr {
                    s6_addr: addr.ip().octets(),
                },
                sin6_flowinfo: addr.flowinfo(),
                sin6_scope_id: addr.scope_id(),
                #[cfg(any(
                    target_os = "dragonfly",
                    target_os = "freebsd",
                    target_os = "ios",
                    target_os = "macos",
                    target_os = "netbsd",
                    target_os = "openbsd"
                ))]
                sin6_len: 0,
                #[cfg(target_os = "illumos")]
                __sin6_src_id: 0,
            };

            let sockaddr = SocketAddrCRepr { v6: sockaddr_in6 };
            let socklen = std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t;
            (sockaddr, socklen)
        }
    }
}
