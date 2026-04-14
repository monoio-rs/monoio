#[cfg(all(unix, any(feature = "legacy", feature = "poll-io")))]
use std::os::unix::prelude::AsRawFd;
use std::{io, net::SocketAddr};

#[cfg(all(target_os = "linux", feature = "iouring"))]
use io_uring::{opcode, types};
use socket2::SockAddr;
#[cfg(all(windows, any(feature = "legacy", feature = "poll-io")))]
use {
    std::os::windows::io::AsRawSocket,
    windows_sys::Win32::Networking::WinSock::{send, WSASendMsg, SOCKET_ERROR},
};

use super::{super::shared_fd::SharedFd, Op, OpAble};
#[cfg(any(feature = "legacy", feature = "poll-io"))]
use super::{driver::ready::Direction, MaybeFd};
#[cfg(unix)]
use crate::net::unix::SocketAddr as UnixSocketAddr;
use crate::{
    buf::{IoBuf, IoVecBufMut, IoVecMeta, MsgMeta},
    BufResult,
};

pub(crate) struct Send<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    #[allow(unused)]
    fd: SharedFd,

    pub(crate) buf: T,
}

impl<T: IoBuf> Op<Send<T>> {
    pub(crate) fn send(fd: SharedFd, buf: T) -> io::Result<Self> {
        Op::submit_with(Send { fd, buf })
    }

    #[allow(unused)]
    pub(crate) fn send_raw(fd: &SharedFd, buf: T) -> Send<T> {
        Send {
            fd: fd.clone(),
            buf,
        }
    }

    pub(crate) async fn result(self) -> BufResult<usize, T> {
        let complete = self.await;
        (
            complete.meta.result.map(|v| v.into_inner() as _),
            complete.data.buf,
        )
    }
}

impl<T: IoBuf> OpAble for Send<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        #[allow(deprecated)]
        #[cfg(feature = "zero-copy")]
        fn zero_copy_flag_guard<T: IoBuf>(buf: &T) -> libc::c_int {
            // TODO: use libc const after supported.
            const MSG_ZEROCOPY: libc::c_int = 0x4000000;
            // According to Linux's documentation, zero copy introduces extra overhead and
            // is only considered effective for at writes over around 10 KB.
            // see also: https://www.kernel.org/doc/html/v4.16/networking/msg_zerocopy.html
            const MSG_ZEROCOPY_THRESHOLD: usize = 10 * 1024 * 1024;
            if buf.bytes_init() >= MSG_ZEROCOPY_THRESHOLD {
                libc::MSG_NOSIGNAL as libc::c_int | MSG_ZEROCOPY
            } else {
                libc::MSG_NOSIGNAL as libc::c_int
            }
        }

        #[cfg(feature = "zero-copy")]
        let flags = zero_copy_flag_guard(&self.buf);
        #[cfg(not(feature = "zero-copy"))]
        #[allow(deprecated)]
        let flags = libc::MSG_NOSIGNAL as libc::c_int;

        opcode::Send::new(
            types::Fd(self.fd.raw_fd()),
            self.buf.read_ptr(),
            self.buf.bytes_init() as _,
        )
        .flags(flags)
        .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd
            .registered_index()
            .map(|idx| (Direction::Write, idx))
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), unix))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        let fd = self.fd.as_raw_fd();
        #[cfg(target_os = "linux")]
        #[allow(deprecated)]
        let flags = libc::MSG_NOSIGNAL as _;
        #[cfg(not(target_os = "linux"))]
        let flags = 0;

        crate::syscall!(send@NON_FD(
            fd,
            self.buf.read_ptr() as _,
            self.buf.bytes_init(),
            flags
        ))
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), windows))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        let fd = self.fd.as_raw_socket();
        crate::syscall!(
            send@NON_FD(fd as _, self.buf.read_ptr(), self.buf.bytes_init() as _, 0),
            PartialOrd::lt,
            0
        )
    }
}

pub(crate) struct SendMsg<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    #[allow(unused)]
    fd: SharedFd,

    /// Reference to the in-flight buffer.
    pub(crate) buf: T,
    /// For multiple message send in the future
    pub(crate) info: Box<(Option<SockAddr>, IoVecMeta, MsgMeta)>,
}

impl<T: IoBuf> Op<SendMsg<T>> {
    pub(crate) fn send_msg(
        fd: SharedFd,
        buf: T,
        socket_addr: Option<SocketAddr>,
    ) -> io::Result<Self> {
        let mut info: Box<(Option<SockAddr>, IoVecMeta, MsgMeta)> = Box::new((
            socket_addr.map(Into::into),
            IoVecMeta::from(&buf),
            unsafe { std::mem::zeroed() },
        ));

        #[cfg(unix)]
        {
            info.2.msg_iov = info.1.write_iovec_ptr();
            info.2.msg_iovlen = info.1.write_iovec_len() as _;
            match info.0.as_ref() {
                Some(socket_addr) => {
                    info.2.msg_name = socket_addr.as_ptr() as *mut libc::c_void;
                    info.2.msg_namelen = socket_addr.len();
                }
                None => {
                    info.2.msg_name = std::ptr::null_mut();
                    info.2.msg_namelen = 0;
                }
            }
        }
        #[cfg(windows)]
        {
            info.2.lpBuffers = info.1.write_wsabuf_ptr();
            info.2.dwBufferCount = info.1.write_wsabuf_len() as _;
            match info.0.as_ref() {
                Some(socket_addr) => {
                    info.2.name = socket_addr.as_ptr() as *mut _;
                    info.2.namelen = socket_addr.len();
                }
                None => {
                    info.2.name = std::ptr::null_mut();
                    info.2.namelen = 0;
                }
            }
        }

        Op::submit_with(SendMsg { fd, buf, info })
    }

    pub(crate) async fn wait(self) -> BufResult<usize, T> {
        let complete = self.await;
        let res = complete.meta.result.map(|v| v.into_inner() as _);
        let buf = complete.data.buf;
        (res, buf)
    }
}

impl<T: IoBuf> OpAble for SendMsg<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        #[allow(deprecated)]
        const FLAGS: u32 = libc::MSG_NOSIGNAL as u32;
        opcode::SendMsg::new(types::Fd(self.fd.raw_fd()), &*self.info.2)
            .flags(FLAGS)
            .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd
            .registered_index()
            .map(|idx| (Direction::Write, idx))
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), unix))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        #[cfg(target_os = "linux")]
        #[allow(deprecated)]
        const FLAGS: libc::c_int = libc::MSG_NOSIGNAL as libc::c_int;
        #[cfg(not(target_os = "linux"))]
        const FLAGS: libc::c_int = 0;
        let fd = self.fd.as_raw_fd();
        crate::syscall!(sendmsg@NON_FD(fd, &*self.info.2, FLAGS))
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), windows))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        let fd = self.fd.as_raw_socket();
        let mut nsent = 0;
        let ret = unsafe {
            WSASendMsg(
                fd as _,
                &*self.info.2,
                0,
                &mut nsent,
                std::ptr::null_mut(),
                None,
            )
        };
        if ret == SOCKET_ERROR {
            Err(io::Error::last_os_error())
        } else {
            Ok(MaybeFd::new_non_fd(nsent))
        }
    }
}

#[cfg(unix)]
pub(crate) struct SendMsgUnix<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    #[allow(unused)]
    fd: SharedFd,

    /// Reference to the in-flight buffer.
    pub(crate) buf: T,
    /// For multiple message send in the future
    pub(crate) info: Box<(Option<UnixSocketAddr>, IoVecMeta, libc::msghdr)>,
}

#[cfg(unix)]
impl<T: IoBuf> Op<SendMsgUnix<T>> {
    pub(crate) fn send_msg_unix(
        fd: SharedFd,
        buf: T,
        socket_addr: Option<UnixSocketAddr>,
    ) -> io::Result<Self> {
        let mut info: Box<(Option<UnixSocketAddr>, IoVecMeta, libc::msghdr)> =
            Box::new((socket_addr, IoVecMeta::from(&buf), unsafe {
                std::mem::zeroed()
            }));

        info.2.msg_iov = info.1.write_iovec_ptr();
        info.2.msg_iovlen = info.1.write_iovec_len() as _;

        match info.0.as_ref() {
            Some(socket_addr) => {
                info.2.msg_name = socket_addr.as_ptr() as *mut libc::c_void;
                info.2.msg_namelen = socket_addr.len();
            }
            None => {
                info.2.msg_name = std::ptr::null_mut();
                info.2.msg_namelen = 0;
            }
        }

        Op::submit_with(SendMsgUnix { fd, buf, info })
    }

    pub(crate) async fn wait(self) -> BufResult<usize, T> {
        let complete = self.await;
        let res = complete.meta.result.map(|v| v.into_inner() as _);
        let buf = complete.data.buf;
        (res, buf)
    }
}

#[cfg(unix)]
impl<T: IoBuf> OpAble for SendMsgUnix<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        #[allow(deprecated)]
        const FLAGS: u32 = libc::MSG_NOSIGNAL as u32;
        opcode::SendMsg::new(types::Fd(self.fd.raw_fd()), &mut self.info.2 as *mut _)
            .flags(FLAGS)
            .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd
            .registered_index()
            .map(|idx| (Direction::Write, idx))
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        #[cfg(target_os = "linux")]
        #[allow(deprecated)]
        const FLAGS: libc::c_int = libc::MSG_NOSIGNAL as libc::c_int;
        #[cfg(not(target_os = "linux"))]
        const FLAGS: libc::c_int = 0;
        let fd = self.fd.as_raw_fd();
        crate::syscall!(sendmsg@NON_FD(fd, &mut self.info.2 as *mut _, FLAGS))
    }
}

/// Zero-copy send using IORING_OP_SEND_ZC (Linux 6.0+).
///
/// This operation produces two CQEs: the first indicates send completion,
/// and the second (notification) signals that the kernel has released the buffer.
/// The future only resolves after both CQEs arrive, ensuring buffer safety.
#[cfg(all(target_os = "linux", feature = "iouring"))]
pub(crate) struct SendZc<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    #[allow(unused)]
    fd: SharedFd,

    pub(crate) buf: T,
}

#[cfg(all(target_os = "linux", feature = "iouring"))]
impl<T: IoBuf> Op<SendZc<T>> {
    pub(crate) fn send_zc(fd: SharedFd, buf: T) -> io::Result<Self> {
        Op::submit_with(SendZc { fd, buf })
    }

    pub(crate) async fn result(self) -> BufResult<usize, T> {
        let complete = self.await;
        (
            complete.meta.result.map(|v| v.into_inner() as _),
            complete.data.buf,
        )
    }
}

#[cfg(all(target_os = "linux", feature = "iouring"))]
impl<T: IoBuf> OpAble for SendZc<T> {
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        #[allow(deprecated)]
        const FLAGS: i32 = libc::MSG_NOSIGNAL;

        opcode::SendZc::new(
            types::Fd(self.fd.raw_fd()),
            self.buf.read_ptr(),
            self.buf.bytes_init() as _,
        )
        .flags(FLAGS)
        .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd
            .registered_index()
            .map(|idx| (Direction::Write, idx))
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        // Fallback to normal send on legacy driver
        let fd = self.fd.as_raw_fd();
        #[allow(deprecated)]
        let flags = libc::MSG_NOSIGNAL as _;

        crate::syscall!(send@NON_FD(
            fd,
            self.buf.read_ptr() as _,
            self.buf.bytes_init(),
            flags
        ))
    }
}

/// Zero-copy sendmsg using IORING_OP_SENDMSG_ZC (Linux 6.1+).
///
/// Like `SendZc`, this produces two CQEs and the future only resolves
/// after the notification CQE arrives.
#[cfg(all(target_os = "linux", feature = "iouring"))]
pub(crate) struct SendMsgZc<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    #[allow(unused)]
    fd: SharedFd,

    /// Reference to the in-flight buffer.
    pub(crate) buf: T,
    pub(crate) info: Box<(Option<SockAddr>, IoVecMeta, MsgMeta)>,
}

#[cfg(all(target_os = "linux", feature = "iouring"))]
impl<T: IoBuf> Op<SendMsgZc<T>> {
    pub(crate) fn send_msg_zc(
        fd: SharedFd,
        buf: T,
        socket_addr: Option<SocketAddr>,
    ) -> io::Result<Self> {
        let mut info: Box<(Option<SockAddr>, IoVecMeta, MsgMeta)> = Box::new((
            socket_addr.map(Into::into),
            IoVecMeta::from(&buf),
            unsafe { std::mem::zeroed() },
        ));

        info.2.msg_iov = info.1.write_iovec_ptr();
        info.2.msg_iovlen = info.1.write_iovec_len() as _;
        match info.0.as_ref() {
            Some(socket_addr) => {
                info.2.msg_name = socket_addr.as_ptr() as *mut libc::c_void;
                info.2.msg_namelen = socket_addr.len();
            }
            None => {
                info.2.msg_name = std::ptr::null_mut();
                info.2.msg_namelen = 0;
            }
        }

        Op::submit_with(SendMsgZc { fd, buf, info })
    }

    pub(crate) async fn wait(self) -> BufResult<usize, T> {
        let complete = self.await;
        let res = complete.meta.result.map(|v| v.into_inner() as _);
        let buf = complete.data.buf;
        (res, buf)
    }
}

#[cfg(all(target_os = "linux", feature = "iouring"))]
impl<T: IoBuf> OpAble for SendMsgZc<T> {
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        #[allow(deprecated)]
        const FLAGS: u32 = libc::MSG_NOSIGNAL as u32;
        opcode::SendMsgZc::new(types::Fd(self.fd.raw_fd()), &*self.info.2)
            .flags(FLAGS)
            .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd
            .registered_index()
            .map(|idx| (Direction::Write, idx))
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        // Fallback to normal sendmsg on legacy driver
        #[allow(deprecated)]
        const FLAGS: libc::c_int = libc::MSG_NOSIGNAL as libc::c_int;
        let fd = self.fd.as_raw_fd();
        crate::syscall!(sendmsg@NON_FD(fd, &*self.info.2, FLAGS))
    }
}
