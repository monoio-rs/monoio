use std::{io, net::SocketAddr};

use super::TcpStream;
use crate::io::{
    as_fd::{AsReadFd, AsWriteFd, SharedFdWrapper},
    OwnedReadHalf, OwnedWriteHalf,
};

/// OwnedReadHalf.
pub type TcpOwnedReadHalf = OwnedReadHalf<TcpStream>;
/// OwnedWriteHalf
pub type TcpOwnedWriteHalf = OwnedWriteHalf<TcpStream>;

impl TcpOwnedReadHalf {
    /// Returns the remote address that this stream is connected to.
    #[inline]
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        unsafe { &*self.0.get() }.peer_addr()
    }

    /// Returns the local address that this stream is bound to.
    #[inline]
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        unsafe { &*self.0.get() }.local_addr()
    }

    /// Wait for read readiness.
    /// Note: Do not use it before every io. It is different from other runtimes!
    ///
    /// Every call to this method may pay a syscall cost.
    /// In uring impl, it will push a PollAdd op; in epoll impl, it will use
    /// inner readiness state; if !relaxed, it will call syscall poll after that.
    ///
    /// If relaxed, on legacy driver it may return false positive result.
    /// If you want to do io by your own, you must maintain io readiness and wait
    /// for io ready with relaxed=false.
    #[inline]
    pub async fn readable(&self, relaxed: bool) -> io::Result<()> {
        unsafe { &*self.0.get() }.readable(relaxed).await
    }
}

impl AsReadFd for TcpOwnedReadHalf {
    #[inline]
    fn as_reader_fd(&mut self) -> &SharedFdWrapper {
        let raw_stream = unsafe { &mut *self.0.get() };
        raw_stream.as_reader_fd()
    }
}

impl TcpOwnedWriteHalf {
    /// Returns the remote address that this stream is connected to.
    #[inline]
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        unsafe { &*self.0.get() }.peer_addr()
    }

    /// Returns the local address that this stream is bound to.
    #[inline]
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        unsafe { &*self.0.get() }.local_addr()
    }

    /// Wait for write readiness.
    /// Note: Do not use it before every io. It is different from other runtimes!
    ///
    /// Every call to this method may pay a syscall cost.
    /// In uring impl, it will push a PollAdd op; in epoll impl, it will use
    /// inner readiness state; if !relaxed, it will call syscall poll after that.
    ///
    /// If relaxed, on legacy driver it may return false positive result.
    /// If you want to do io by your own, you must maintain io readiness and wait
    /// for io ready with relaxed=false.
    #[inline]
    pub async fn writable(&self, relaxed: bool) -> io::Result<()> {
        unsafe { &*self.0.get() }.writable(relaxed).await
    }
}

impl AsWriteFd for TcpOwnedWriteHalf {
    #[inline]
    fn as_writer_fd(&mut self) -> &SharedFdWrapper {
        let raw_stream = unsafe { &mut *self.0.get() };
        raw_stream.as_writer_fd()
    }
}
