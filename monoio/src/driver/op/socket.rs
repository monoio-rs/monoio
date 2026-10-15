#[cfg(any(feature = "legacy", feature = "poll-io"))]
use crate::driver::op::MaybeFd;
use crate::driver::op::{Op, OpAble};

pub(crate) struct Socket {
    domain: socket2::Domain,
    ty: socket2::Type,
    protocol: Option<socket2::Protocol>,
}

impl Op<Socket> {
    pub(crate) fn socket(
        domain: socket2::Domain,
        ty: socket2::Type,
        protocol: Option<socket2::Protocol>,
    ) -> std::io::Result<Self> {
        // Follow the `socket2`'s behavior
        #[cfg(target_os = "linux")]
        let ty = ty.cloexec();

        Op::submit_with(Socket {
            domain,
            ty,
            protocol,
        })
    }
}

impl OpAble for Socket {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    const RET_IS_FD: bool = true;

    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        use io_uring::opcode;

        opcode::Socket::new(
            self.domain.into(),
            self.ty.into(),
            self.protocol.map_or(0, |p| p.into()),
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
        crate::syscall!(socket@FD(self.domain.into(), self.ty.into(), self.protocol.map_or(0, |p| p.into())))
    }
}
