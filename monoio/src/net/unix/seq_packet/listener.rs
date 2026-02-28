use std::{
    io,
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::prelude::IntoRawFd,
    },
    path::Path,
};

use super::UnixSeqpacket;
use crate::{
    driver::{op::Op, shared_fd::SharedFd},
    io::stream::Stream,
    net::{new_socket, unix::SocketAddr},
};

const DEFAULT_BACKLOG: libc::c_int = 128;

/// Listener for UnixSeqpacket
pub struct UnixSeqpacketListener {
    fd: SharedFd,
}

impl UnixSeqpacketListener {
    /// Creates a new `UnixSeqpacketListener` bound to the specified path with custom backlog
    pub fn bind_with_backlog<P: AsRef<Path>>(path: P, backlog: libc::c_int) -> io::Result<Self> {
        let addr = socket2::SockAddr::unix(path)?;
        let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::SEQPACKET, None)?;

        #[cfg(feature = "legacy")]
        if crate::driver::op::is_legacy() {
            socket.set_nonblocking(true)?;
        }

        socket.bind(&addr)?;
        socket.listen(backlog)?;

        Ok(Self {
            fd: SharedFd::new::<false>(socket.into_raw_fd())?,
        })
    }

    /// Creates a new `UnixSeqpacketListener` bound to the specified path with custom backlog
    /// asynchronously.
    pub async fn async_bind_with_backlog<P: AsRef<Path>>(
        path: P,
        backlog: libc::c_int,
    ) -> io::Result<Self> {
        let addr = socket2::SockAddr::unix(path)?;
        let socket = new_socket(socket2::Domain::UNIX, socket2::Type::SEQPACKET).await?;
        let socket = unsafe { socket2::Socket::from_raw_fd(socket) };

        #[cfg(feature = "bind")]
        let socket = {
            let completion = Op::bind(socket, addr)?.await;
            completion.meta.result?;
            completion.data.socket
        };

        #[cfg(not(feature = "bind"))]
        socket.bind(&addr)?;

        #[cfg(feature = "listen")]
        let socket = {
            let completion = Op::listen(socket, backlog)?.await;
            completion.meta.result?;
            completion.data.socket
        };

        #[cfg(not(feature = "listen"))]
        socket.listen(backlog)?;

        Ok(Self {
            fd: SharedFd::new::<false>(socket.into_raw_fd())?,
        })
    }

    /// Creates a new `UnixSeqpacketListener` bound to the specified path with default backlog(128)
    #[inline]
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        Self::bind_with_backlog(path, DEFAULT_BACKLOG)
    }

    /// Creates a new `UnixSeqpacketListener` bound to the specified path with default backlog(128)
    /// asynchronously.
    #[inline]
    pub async fn async_bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        Self::async_bind_with_backlog(path, DEFAULT_BACKLOG).await
    }

    /// Accept a UnixSeqpacket
    pub async fn accept(&self) -> io::Result<(UnixSeqpacket, SocketAddr)> {
        let op = Op::accept(&self.fd)?;

        // Await the completion of the event
        let completion = op.await;

        // Convert fd
        let fd = completion.meta.result?;

        // Construct stream
        let stream = UnixSeqpacket::from_shared_fd(SharedFd::new::<false>(fd.into_inner() as _)?);

        // Construct SocketAddr
        let mut storage = unsafe { std::mem::MaybeUninit::assume_init(completion.data.addr.0) };
        let storage: *mut libc::sockaddr_storage = &mut storage as *mut _;
        let raw_addr_un: libc::sockaddr_un = unsafe { *storage.cast() };
        let raw_addr_len = completion.data.addr.1;

        let addr = SocketAddr::from_parts(raw_addr_un, raw_addr_len);

        Ok((stream, addr))
    }
}

impl AsRawFd for UnixSeqpacketListener {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.fd.raw_fd()
    }
}

impl std::fmt::Debug for UnixSeqpacketListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnixSeqpacketListener")
            .field("fd", &self.fd)
            .finish()
    }
}

impl Stream for UnixSeqpacketListener {
    type Item = io::Result<(UnixSeqpacket, SocketAddr)>;

    #[inline]
    async fn next(&mut self) -> Option<Self::Item> {
        Some(self.accept().await)
    }
}
