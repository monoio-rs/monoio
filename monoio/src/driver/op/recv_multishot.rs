//! Multishot receive operations for io_uring.

use std::io;

#[cfg(all(target_os = "linux", feature = "iouring"))]
use io_uring::{opcode, types};

use super::{super::shared_fd::SharedFd, OpAble};
#[cfg(any(feature = "legacy", feature = "poll-io"))]
use super::{driver::ready::Direction, MaybeFd};
use crate::buf::RecvMsgParser;

/// Multishot recv operation for connected sockets.
pub(crate) struct RecvMultishotOp {
    fd: SharedFd,
    bgid: u16,
}

impl RecvMultishotOp {
    pub(crate) fn new(fd: SharedFd, bgid: u16) -> Self {
        Self { fd, bgid }
    }
}

impl OpAble for RecvMultishotOp {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    const MULTISHOT: bool = true;
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    const SKIP_CANCEL: bool = false;

    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        opcode::RecvMulti::new(types::Fd(self.fd.raw_fd()), self.bgid).build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd.registered_index().map(|idx| (Direction::Read, idx))
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "multishot recv not supported on legacy driver",
        ))
    }
}

/// Multishot recvmsg operation with address parsing.
pub(crate) struct RecvMsgMultishotOp {
    fd: SharedFd,
    pub(crate) msghdr: Box<libc::msghdr>,
    bgid: u16,
}

impl RecvMsgMultishotOp {
    pub(crate) fn new<P: RecvMsgParser>(fd: SharedFd, bgid: u16) -> Self {
        let mut msghdr: libc::msghdr = unsafe { std::mem::zeroed() };
        msghdr.msg_namelen = P::NAME_LEN;
        Self {
            fd,
            msghdr: Box::new(msghdr),
            bgid,
        }
    }
}

impl OpAble for RecvMsgMultishotOp {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    const MULTISHOT: bool = true;
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    const SKIP_CANCEL: bool = false;

    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        opcode::RecvMsgMulti::new(
            types::Fd(self.fd.raw_fd()),
            &*self.msghdr as *const _,
            self.bgid,
        )
        .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd.registered_index().map(|idx| (Direction::Read, idx))
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "multishot recvmsg not supported on legacy driver",
        ))
    }
}
