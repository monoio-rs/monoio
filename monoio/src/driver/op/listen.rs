use crate::driver::op::{Op, OpAble};

pub(crate) struct Listen {
    pub(crate) socket: socket2::Socket,
    pub(crate) backlog: i32,
}

impl Op<Listen> {
    pub(crate) fn listen(socket: socket2::Socket, backlog: i32) -> std::io::Result<Self> {
        Op::submit_with(Listen { socket, backlog })
    }
}

impl OpAble for Listen {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        use std::os::fd::AsRawFd;

        use io_uring::{opcode, types};

        opcode::Listen::new(types::Fd(self.socket.as_raw_fd()), self.backlog).build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(super::super::ready::Direction, usize)> {
        None
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_call(&mut self) -> std::io::Result<super::MaybeFd> {
        use std::os::fd::AsRawFd;

        crate::syscall!(listen@NON_FD(self.socket.as_raw_fd(), self.backlog))
    }
}
