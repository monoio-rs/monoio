use std::{
    io,
    mem::{ManuallyDrop, MaybeUninit},
    os::unix::prelude::{AsRawFd, FromRawFd, IntoRawFd, RawFd},
    path::Path,
};

use super::{socket_addr::SocketAddr, UnixStream};
use crate::{
    driver::{op::Op, shared_fd::SharedFd},
    io::{stream::Stream, CancelHandle},
    net::ListenerOpts,
};

/// UnixListener
pub struct UnixListener {
    fd: SharedFd,
    sys_listener: Option<std::os::unix::net::UnixListener>,
}

impl UnixListener {
    fn apply_listener_config(
        sys_listener: &socket2::Socket,
        config: &ListenerOpts,
    ) -> io::Result<()> {
        if config.reuse_port {
            // Unix domain socket does not support SO_REUSEPORT.
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Unix sockets do not support SO_REUSEPORT",
            ));
        }

        if config.reuse_addr {
            sys_listener.set_reuse_address(true)?;
        }
        if let Some(send_buf_size) = config.send_buf_size {
            sys_listener.set_send_buffer_size(send_buf_size)?;
        }
        if let Some(recv_buf_size) = config.recv_buf_size {
            sys_listener.set_recv_buffer_size(recv_buf_size)?;
        }

        Ok(())
    }

    pub(crate) fn from_shared_fd(fd: SharedFd) -> Self {
        let sys_listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd.raw_fd()) };
        Self {
            fd,
            sys_listener: Some(sys_listener),
        }
    }

    /// Creates a new `UnixListener` bound to the specified socket with custom
    /// config.
    pub fn bind_with_config<P: AsRef<Path>>(
        path: P,
        config: &ListenerOpts,
    ) -> io::Result<UnixListener> {
        let sys_listener =
            socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
        let addr = socket2::SockAddr::unix(path)?;

        Self::apply_listener_config(&sys_listener, config)?;

        sys_listener.bind(&addr)?;
        sys_listener.listen(config.backlog)?;

        let fd = SharedFd::new::<false>(sys_listener.into_raw_fd())?;

        Ok(Self::from_shared_fd(fd))
    }

    /// Creates a new `UnixListener` bound to the specified socket with custom
    /// config asynchronously.
    pub async fn async_bind_with_config<P: AsRef<Path>>(
        path: P,
        config: &ListenerOpts,
    ) -> io::Result<UnixListener> {
        let sys_listener = {
            let completion = Op::socket(socket2::Domain::UNIX, socket2::Type::STREAM, None)?.await;
            let fd = completion.meta.result?.into_inner();
            unsafe { socket2::Socket::from_raw_fd(fd as _) }
        };
        let addr = socket2::SockAddr::unix(path)?;

        Self::apply_listener_config(&sys_listener, config)?;

        #[cfg(feature = "bind")]
        let sys_listener = {
            let completion = Op::bind(sys_listener, addr)?.await;
            completion.meta.result?;
            completion.data.socket
        };

        #[cfg(not(feature = "bind"))]
        sys_listener.bind(&addr)?;

        #[cfg(feature = "listen")]
        let sys_listener = {
            let completion = Op::listen(sys_listener, config.backlog)?.await;
            completion.meta.result?;
            completion.data.socket
        };

        #[cfg(not(feature = "listen"))]
        sys_listener.listen(config.backlog)?;

        let fd = SharedFd::new::<false>(sys_listener.into_raw_fd())?;

        Ok(Self::from_shared_fd(fd))
    }

    /// Creates a new `UnixListener` bound to the specified socket with default
    /// config.
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<UnixListener> {
        Self::bind_with_config(path, &ListenerOpts::default().reuse_port(false))
    }

    /// Creates a new `UnixListener` bound to the specified socket with default
    /// config asynchronously.
    pub async fn async_bind<P: AsRef<Path>>(path: P) -> io::Result<UnixListener> {
        Self::async_bind_with_config(path, &ListenerOpts::default().reuse_port(false)).await
    }

    /// Accept
    pub async fn accept(&self) -> io::Result<(UnixStream, SocketAddr)> {
        let op = Op::accept(&self.fd)?;

        // Await the completion of the event
        let completion = op.await;

        // Convert fd
        let fd = completion.meta.result?;

        // Construct stream
        let stream = UnixStream::from_shared_fd(SharedFd::new::<false>(fd.into_inner() as _)?);

        // Construct SocketAddr
        let mut storage = unsafe { std::mem::MaybeUninit::assume_init(completion.data.addr.0) };
        let storage: *mut libc::sockaddr_storage = &mut storage as *mut _;
        let raw_addr_un: libc::sockaddr_un = unsafe { *storage.cast() };
        let raw_addr_len = completion.data.addr.1;

        let addr = SocketAddr::from_parts(raw_addr_un, raw_addr_len);

        Ok((stream, addr))
    }

    /// Cancelable accept
    pub async fn cancelable_accept(&self, c: CancelHandle) -> io::Result<(UnixStream, SocketAddr)> {
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
        let stream = UnixStream::from_shared_fd(SharedFd::new::<false>(fd.into_inner() as _)?);

        // Construct SocketAddr
        let mut storage = unsafe { std::mem::MaybeUninit::assume_init(completion.data.addr.0) };
        let storage: *mut libc::sockaddr_storage = &mut storage as *mut _;
        let raw_addr_un: libc::sockaddr_un = unsafe { *storage.cast() };
        let raw_addr_len = completion.data.addr.1;

        let addr = SocketAddr::from_parts(raw_addr_un, raw_addr_len);

        Ok((stream, addr))
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

    /// Creates new `UnixListener` from a `std::os::unix::net::UnixListener`.
    pub fn from_std(sys_listener: std::os::unix::net::UnixListener) -> io::Result<Self> {
        match SharedFd::new::<false>(sys_listener.as_raw_fd()) {
            Ok(shared) => Ok(Self {
                fd: shared,
                sys_listener: Some(sys_listener),
            }),
            Err(e) => Err(e),
        }
    }
}

impl Stream for UnixListener {
    type Item = io::Result<(UnixStream, SocketAddr)>;

    #[inline]
    async fn next(&mut self) -> Option<Self::Item> {
        Some(self.accept().await)
    }
}

impl std::fmt::Debug for UnixListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnixListener")
            .field("fd", &self.fd)
            .finish()
    }
}

impl IntoRawFd for UnixListener {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        let mut this = ManuallyDrop::new(self);
        #[allow(invalid_value)]
        #[allow(clippy::uninit_assumed_init)]
        let (mut fd, mut sys_listener) = unsafe {
            (
                MaybeUninit::uninit().assume_init(),
                MaybeUninit::uninit().assume_init(),
            )
        };
        std::mem::swap(&mut this.fd, &mut fd);
        std::mem::swap(&mut this.sys_listener, &mut sys_listener);
        let _ = sys_listener.take().unwrap().into_raw_fd();

        fd.try_unwrap()
            .expect("unexpected multiple reference to rawfd")
    }
}

impl AsRawFd for UnixListener {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.fd.raw_fd()
    }
}

impl Drop for UnixListener {
    #[inline]
    fn drop(&mut self) {
        let _ = self.sys_listener.take().unwrap().into_raw_fd();
    }
}
