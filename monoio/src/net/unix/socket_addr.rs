//! SocketAddr for UDS.
//! Forked from mio.

use std::{
    ascii,
    cmp::Ordering,
    ffi::OsStr,
    fmt, io, mem,
    os::unix::prelude::{FromRawFd, OsStrExt, RawFd},
    path::Path,
};

use super::path_offset;

/// Unix SocketAddr.
/// There is no way to create a [`net::SocketAddr`] so we forked it from mio.
#[derive(Clone)]
pub struct SocketAddr {
    sockaddr: libc::sockaddr_un,
    socklen: libc::socklen_t,
}

struct AsciiEscaped<'a>(&'a [u8]);

enum AddressKind<'a> {
    Unnamed,
    Pathname(&'a Path),
    Abstract(&'a [u8]),
}

impl SocketAddr {
    fn address(&self) -> AddressKind<'_> {
        let offset = path_offset(&self.sockaddr);
        // Don't underflow in `len` below.
        if (self.socklen as usize) < offset {
            return AddressKind::Unnamed;
        }
        let len = self.socklen as usize - offset;
        let path = unsafe { &*(&self.sockaddr.sun_path as *const [libc::c_char] as *const [u8]) };

        // macOS seems to return a len of 16 and a zeroed sun_path for unnamed addresses
        if len == 0 || (cfg!(not(any(target_os = "linux", target_os = "android"))) && path[0] == 0)
        {
            AddressKind::Unnamed
        } else if self.sockaddr.sun_path[0] == 0 {
            AddressKind::Abstract(&path[1..len])
        } else {
            // Pathname addresses are NUL-terminated. Some platforms (notably
            // OpenBSD) may return a socklen that covers the full sockaddr_un,
            // including trailing padding NULs after the terminator.
            let pathname = &path[..len];
            let end = pathname.iter().position(|&b| b == 0).unwrap_or(pathname.len());
            AddressKind::Pathname(OsStr::from_bytes(&pathname[..end]).as_ref())
        }
    }

    #[allow(unused)]
    pub(crate) fn new<F>(f: F) -> io::Result<SocketAddr>
    where
        F: FnOnce(*mut libc::sockaddr, &mut libc::socklen_t) -> io::Result<libc::c_int>,
    {
        let mut sockaddr = {
            let sockaddr = mem::MaybeUninit::<libc::sockaddr_un>::zeroed();
            unsafe { sockaddr.assume_init() }
        };

        let raw_sockaddr = &mut sockaddr as *mut libc::sockaddr_un as *mut libc::sockaddr;
        let mut socklen = mem::size_of_val(&sockaddr) as libc::socklen_t;

        f(raw_sockaddr, &mut socklen)?;
        Ok(SocketAddr::from_parts(sockaddr, socklen))
    }

    pub(crate) fn from_parts(
        sockaddr: libc::sockaddr_un,
        mut socklen: libc::socklen_t,
    ) -> SocketAddr {
        fn sun_path_offset(addr: &libc::sockaddr_un) -> usize {
            let base: usize = (addr as *const libc::sockaddr_un).cast::<()>() as usize;
            let path: usize = (&addr.sun_path as *const libc::c_char).cast::<()>() as usize;
            path - base
        }

        if socklen == 0 {
            // When there is a datagram from unnamed unix socket
            // linux returns zero bytes of address
            socklen = sun_path_offset(&sockaddr) as libc::socklen_t; // i.e., zero-length address
        } else if sockaddr.sun_family != libc::AF_UNIX as libc::sa_family_t {
            panic!("file descriptor did not correspond to a Unix socket");
        }

        SocketAddr { sockaddr, socklen }
    }

    pub(crate) fn into_parts(self) -> (libc::sockaddr_un, libc::socklen_t) {
        (self.sockaddr, self.socklen)
    }

    /// Returns `true` if the address is unnamed.
    ///
    /// Documentation reflected in [`SocketAddr`]
    ///
    /// [`SocketAddr`]: std::os::unix::net::SocketAddr
    #[inline]
    pub fn is_unnamed(&self) -> bool {
        matches!(self.address(), AddressKind::Unnamed)
    }

    /// Returns the contents of this address if it is a `pathname` address.
    ///
    /// Documentation reflected in [`SocketAddr`]
    ///
    /// [`SocketAddr`]: std::os::unix::net::SocketAddr
    #[inline]
    pub fn as_pathname(&self) -> Option<&Path> {
        if let AddressKind::Pathname(path) = self.address() {
            Some(path)
        } else {
            None
        }
    }

    /// Returns the contents of this address if it is an abstract namespace
    /// without the leading null byte.
    // Link to std::os::unix::net::SocketAddr pending
    // https://github.com/rust-lang/rust/issues/85410.
    #[inline]
    pub fn as_abstract_namespace(&self) -> Option<&[u8]> {
        if let AddressKind::Abstract(path) = self.address() {
            Some(path)
        } else {
            None
        }
    }

    #[inline]
    pub(crate) fn as_ptr(&self) -> *const libc::sockaddr_un {
        &self.sockaddr as *const _
    }

    #[inline]
    pub(crate) fn len(&self) -> libc::socklen_t {
        self.socklen
    }
}

impl fmt::Debug for SocketAddr {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.address() {
            AddressKind::Unnamed => write!(fmt, "(unnamed)"),
            AddressKind::Abstract(name) => write!(fmt, "{} (abstract)", AsciiEscaped(name)),
            AddressKind::Pathname(path) => write!(fmt, "{path:?} (pathname)"),
        }
    }
}

impl fmt::Display for AsciiEscaped<'_> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "\"")?;
        for byte in self.0.iter().cloned().flat_map(ascii::escape_default) {
            write!(fmt, "{}", byte as char)?;
        }
        write!(fmt, "\"")
    }
}

pub(crate) fn socket_addr(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let sockaddr = mem::MaybeUninit::<libc::sockaddr_un>::zeroed();

    // This is safe to assume because a `libc::sockaddr_un` filled with `0`
    // bytes is properly initialized.
    //
    // `0` is a valid value for `sockaddr_un::sun_family`; it is
    // `libc::AF_UNSPEC`.
    //
    // `[0; 108]` is a valid value for `sockaddr_un::sun_path`; it begins an
    // abstract path.
    let mut sockaddr = unsafe { sockaddr.assume_init() };

    sockaddr.sun_family = libc::AF_UNIX as libc::sa_family_t;

    let bytes = path.as_os_str().as_bytes();
    match (bytes.first(), bytes.len().cmp(&sockaddr.sun_path.len())) {
        // Abstract paths don't need a null terminator
        (Some(&0), Ordering::Greater) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path must be no longer than libc::sockaddr_un.sun_path",
            ));
        }
        (_, Ordering::Greater) | (_, Ordering::Equal) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path must be shorter than libc::sockaddr_un.sun_path",
            ));
        }
        _ => {}
    }

    for (dst, src) in sockaddr.sun_path.iter_mut().zip(bytes.iter()) {
        *dst = *src as libc::c_char;
    }

    let offset = path_offset(&sockaddr);
    let mut socklen = offset + bytes.len();

    match bytes.first() {
        // The struct has already been zeroes so the null byte for pathname
        // addresses is already there.
        Some(&0) | None => {}
        Some(_) => socklen += 1,
    }

    Ok((sockaddr, socklen as libc::socklen_t))
}

pub(crate) fn pair<T>(flags: libc::c_int) -> io::Result<(T, T)>
where
    T: FromRawFd,
{
    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "illumos",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    let flags = flags | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC;

    #[cfg(target_os = "linux")]
    let flags = {
        if crate::driver::op::is_legacy() {
            flags | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK
        } else {
            flags | libc::SOCK_CLOEXEC
        }
    };

    let mut fds = [-1; 2];
    crate::syscall!(socketpair@RAW(libc::AF_UNIX, flags, 0, fds.as_mut_ptr()))?;
    let pair = unsafe { (T::from_raw_fd(fds[0]), T::from_raw_fd(fds[1])) };
    Ok(pair)
}

pub(crate) fn local_addr(socket: RawFd) -> io::Result<SocketAddr> {
    SocketAddr::new(|sockaddr, socklen| crate::syscall!(getsockname@RAW(socket, sockaddr, socklen)))
}

pub(crate) fn peer_addr(socket: RawFd) -> io::Result<SocketAddr> {
    SocketAddr::new(|sockaddr, socklen| crate::syscall!(getpeername@RAW(socket, sockaddr, socklen)))
}
