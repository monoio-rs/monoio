//! Read readiness for an owned Unix descriptor, without socket operations.

use std::{
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, IntoRawFd, OwnedFd, RawFd},
};

use crate::driver::{op::Op, shared_fd::SharedFd};

/// An owned Unix descriptor registered for read readiness on the current runtime.
///
/// The descriptor must support the driver's readiness mechanism. Its owner must
/// use nonblocking operations after readiness: a wake can be stale. This type
/// neither reads the descriptor nor assumes that it is a socket. Only one task
/// should wait for read readiness on a descriptor at a time.
#[derive(Debug)]
pub struct AsyncFd {
    fd: SharedFd,
}

impl AsyncFd {
    /// Register an owned descriptor for read readiness.
    ///
    /// This must be called within a Monoio runtime. An unsuccessful registration
    /// closes the supplied descriptor; successful registration transfers its
    /// ownership to this wrapper. Dropping the wrapper deregisters and closes it.
    pub fn new(fd: OwnedFd) -> io::Result<Self> {
        let shared = SharedFd::new_with_interest::<false, true>(fd.as_raw_fd())?;
        let _ = fd.into_raw_fd();
        Ok(Self { fd: shared })
    }

    /// Wait until the descriptor may be readable, including a hangup or error.
    ///
    /// No bytes are consumed. Perform the descriptor's own nonblocking operation
    /// to distinguish data, EOF, process exit, or an error. If that operation
    /// returns `WouldBlock`, call this method again to rearm readiness.
    pub async fn readable(&self) -> io::Result<()> {
        Op::poll_read(&self.fd, false)?.wait().await
    }
}

impl AsRawFd for AsyncFd {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl AsFd for AsyncFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        // SAFETY: self retains the registered descriptor for the entire borrow.
        unsafe { BorrowedFd::borrow_raw(self.as_raw_fd()) }
    }
}
