#[cfg(any(feature = "legacy", feature = "poll-io"))]
use crate::driver::op::MaybeFd;
use crate::driver::op::{Op, OpAble};

pub(crate) struct Bind {
    pub(crate) socket: socket2::Socket,
    pub(crate) address: socket2::SockAddr,
}

impl Op<Bind> {
    pub(crate) fn bind(
        socket: socket2::Socket,
        address: socket2::SockAddr,
    ) -> std::io::Result<Self> {
        Op::submit_with(Bind { socket, address })
    }
}

impl OpAble for Bind {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        use std::os::fd::AsRawFd;

        use io_uring::{opcode, types};

        opcode::Bind::new(
            types::Fd(self.socket.as_raw_fd()),
            self.address.as_ptr(),
            self.address.len(),
        )
        .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(super::super::ready::Direction, usize)> {
        None
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_call(&mut self) -> std::io::Result<MaybeFd> {
        use std::os::fd::AsRawFd;
        crate::syscall!(bind@NON_FD(self.socket.as_raw_fd(), self.address.as_ptr(), self.address.len()))
    }
}
