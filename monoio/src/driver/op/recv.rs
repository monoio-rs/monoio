#[cfg(all(unix, any(feature = "legacy", feature = "poll-io")))]
use std::os::unix::prelude::AsRawFd;
use std::{
    io,
    mem::{transmute, MaybeUninit},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
};

#[cfg(all(target_os = "linux", feature = "iouring"))]
use io_uring::{opcode, types};
#[cfg(unix)]
use {
    crate::net::unix::SocketAddr as UnixSocketAddr,
    libc::{sockaddr_in, sockaddr_in6, sockaddr_storage, socklen_t, AF_INET, AF_INET6},
};
#[cfg(all(windows, any(feature = "legacy", feature = "poll-io")))]
use {
    std::os::windows::io::AsRawSocket,
    windows_sys::Win32::Networking::WinSock::recv,
    windows_sys::{
        core::GUID,
        Win32::{
            Networking::WinSock::{
                WSAGetLastError, WSAIoctl, AF_INET, AF_INET6, LPFN_WSARECVMSG,
                LPWSAOVERLAPPED_COMPLETION_ROUTINE, SIO_GET_EXTENSION_FUNCTION_POINTER, SOCKADDR,
                SOCKADDR_IN as sockaddr_in, SOCKADDR_IN6 as sockaddr_in6,
                SOCKADDR_STORAGE as sockaddr_storage, SOCKET, SOCKET_ERROR, WSAID_WSARECVMSG,
                WSAMSG,
            },
            System::IO::OVERLAPPED,
        },
    },
};

use super::{super::shared_fd::SharedFd, Op, OpAble};
#[cfg(any(feature = "legacy", feature = "poll-io"))]
use super::{driver::ready::Direction, MaybeFd};
use crate::{
    buf::{IoBufMut, IoVecBufMut, IoVecMeta, MsgMeta},
    BufResult,
};

pub(crate) struct Recv<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    #[allow(unused)]
    fd: SharedFd,

    /// Reference to the in-flight buffer.
    pub(crate) buf: T,
}

impl<T: IoBufMut> Op<Recv<T>> {
    pub(crate) fn recv(fd: SharedFd, buf: T) -> io::Result<Self> {
        Op::submit_with(Recv { fd, buf })
    }

    #[allow(unused)]
    pub(crate) fn recv_raw(fd: &SharedFd, buf: T) -> Recv<T> {
        Recv {
            fd: fd.clone(),
            buf,
        }
    }

    pub(crate) async fn result(self) -> BufResult<usize, T> {
        let complete = self.await;
        let res = complete.meta.result.map(|v| v.into_inner() as _);
        let mut buf = complete.data.buf;

        if let Ok(n) = res {
            // Safety: the kernel wrote `n` bytes to the buffer.
            unsafe {
                buf.set_init(n);
            }
        }
        (res, buf)
    }
}

impl<T: IoBufMut> OpAble for Recv<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        opcode::Recv::new(
            types::Fd(self.fd.raw_fd()),
            self.buf.write_ptr(),
            self.buf.bytes_total() as _,
        )
        .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd.registered_index().map(|idx| (Direction::Read, idx))
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), unix))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        let fd = self.fd.as_raw_fd();
        crate::syscall!(recv@NON_FD(
            fd,
            self.buf.write_ptr() as _,
            self.buf.bytes_total().min(u32::MAX as usize),
            0
        ))
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), windows))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        let fd = self.fd.as_raw_socket();
        crate::syscall!(
            recv@NON_FD(
                fd as _,
                self.buf.write_ptr(),
                self.buf.bytes_total().min(i32::MAX as usize) as _,
                0
            ),
            PartialOrd::lt,
            0
        )
    }
}

pub(crate) struct RecvMsg<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    #[allow(unused)]
    fd: SharedFd,

    /// Reference to the in-flight buffer.
    pub(crate) buf: T,
    /// For multiple message recv in the future
    pub(crate) info: Box<(MaybeUninit<sockaddr_storage>, IoVecMeta, MsgMeta)>,
}

impl<T: IoBufMut> Op<RecvMsg<T>> {
    pub(crate) fn recv_msg(fd: SharedFd, mut buf: T) -> io::Result<Self> {
        let mut info: Box<(MaybeUninit<sockaddr_storage>, IoVecMeta, MsgMeta)> =
            Box::new((MaybeUninit::uninit(), IoVecMeta::from(&mut buf), unsafe {
                std::mem::zeroed()
            }));

        #[cfg(unix)]
        {
            info.2.msg_iov = info.1.write_iovec_ptr();
            info.2.msg_iovlen = info.1.write_iovec_len() as _;
            info.2.msg_name = &mut info.0 as *mut _ as *mut libc::c_void;
            info.2.msg_namelen = std::mem::size_of::<sockaddr_storage>() as socklen_t;
        }
        #[cfg(windows)]
        {
            info.2.lpBuffers = info.1.write_wsabuf_ptr();
            info.2.dwBufferCount = info.1.write_wsabuf_len() as _;
            info.2.name = &mut info.0 as *mut _ as *mut SOCKADDR;
            info.2.namelen = std::mem::size_of::<sockaddr_storage>() as _;
        }

        Op::submit_with(RecvMsg { fd, buf, info })
    }

    pub(crate) async fn wait(self) -> BufResult<(usize, SocketAddr), T> {
        let complete = self.await;
        let res = complete.meta.result.map(|v| v.into_inner() as _);
        let mut buf = complete.data.buf;

        let res = res.map(|n| {
            let storage = unsafe { complete.data.info.0.assume_init() };

            let addr = unsafe {
                match storage.ss_family as _ {
                    AF_INET => {
                        // Safety: if the ss_family field is AF_INET then storage must be a
                        // sockaddr_in.
                        let addr: &sockaddr_in = transmute(&storage);
                        #[cfg(unix)]
                        let ip = Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes());
                        #[cfg(windows)]
                        let ip = Ipv4Addr::from(addr.sin_addr.S_un.S_addr.to_ne_bytes());
                        let port = u16::from_be(addr.sin_port);
                        SocketAddr::V4(SocketAddrV4::new(ip, port))
                    }
                    AF_INET6 => {
                        // Safety: if the ss_family field is AF_INET6 then storage must be a
                        // sockaddr_in6.
                        let addr: &sockaddr_in6 = transmute(&storage);
                        #[cfg(unix)]
                        let ip = Ipv6Addr::from(addr.sin6_addr.s6_addr);
                        #[cfg(windows)]
                        let ip = Ipv6Addr::from(addr.sin6_addr.u.Byte);
                        let port = u16::from_be(addr.sin6_port);
                        #[cfg(unix)]
                        let scope_id = addr.sin6_scope_id;
                        #[cfg(windows)]
                        let scope_id = addr.Anonymous.sin6_scope_id;
                        SocketAddr::V6(SocketAddrV6::new(ip, port, addr.sin6_flowinfo, scope_id))
                    }
                    _ => {
                        unreachable!()
                    }
                }
            };

            // Safety: the kernel wrote `n` bytes to the buffer.
            unsafe { buf.set_init(n) };

            (n, addr)
        });
        (res, buf)
    }
}

/// see https://github.com/microsoft/windows-rs/issues/2530
#[cfg(all(any(feature = "legacy", feature = "poll-io"), windows))]
static WSA_RECV_MSG: std::sync::OnceLock<
    unsafe extern "system" fn(
        SOCKET,
        *mut WSAMSG,
        *mut u32,
        *mut OVERLAPPED,
        LPWSAOVERLAPPED_COMPLETION_ROUTINE,
    ) -> i32,
> = std::sync::OnceLock::new();

impl<T: IoBufMut> OpAble for RecvMsg<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        opcode::RecvMsg::new(types::Fd(self.fd.raw_fd()), &mut *self.info.2).build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd.registered_index().map(|idx| (Direction::Read, idx))
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), unix))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        let fd = self.fd.as_raw_fd();
        crate::syscall!(recvmsg@NON_FD(fd, &mut *self.info.2, 0))
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), windows))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        let fd = self.fd.as_raw_socket() as _;
        let func_ptr = WSA_RECV_MSG.get_or_init(|| unsafe {
            let mut wsa_recv_msg: LPFN_WSARECVMSG = None;
            let mut dw_bytes = 0;
            let r = WSAIoctl(
                fd,
                SIO_GET_EXTENSION_FUNCTION_POINTER,
                &WSAID_WSARECVMSG as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<GUID>() as u32,
                &mut wsa_recv_msg as *mut _ as *mut std::ffi::c_void,
                std::mem::size_of::<LPFN_WSARECVMSG>() as _,
                &mut dw_bytes,
                std::ptr::null_mut(),
                None,
            );
            // TODO: properly fix this clippy complaint
            #[allow(clippy::unnecessary_unwrap)]
            if r == SOCKET_ERROR || wsa_recv_msg.is_none() {
                panic!(
                    "init WSARecvMsg failed with {}",
                    io::Error::from_raw_os_error(WSAGetLastError())
                )
            } else {
                assert_eq!(dw_bytes, std::mem::size_of::<LPFN_WSARECVMSG>() as _);
                wsa_recv_msg.unwrap()
            }
        });
        let mut recved = 0;
        let r = unsafe {
            (func_ptr)(
                fd,
                &mut *self.info.2,
                &mut recved,
                std::ptr::null_mut(),
                None,
            )
        };
        if r == SOCKET_ERROR {
            unsafe { Err(io::Error::from_raw_os_error(WSAGetLastError())) }
        } else {
            Ok(MaybeFd::new_non_fd(recved))
        }
    }
}

#[cfg(unix)]
pub(crate) struct RecvMsgUnix<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    #[allow(unused)]
    fd: SharedFd,

    /// Reference to the in-flight buffer.
    pub(crate) buf: T,
    /// For multiple message recv in the future
    pub(crate) info: Box<(MaybeUninit<sockaddr_storage>, IoVecMeta, libc::msghdr)>,
}

#[cfg(unix)]
impl<T: IoBufMut> Op<RecvMsgUnix<T>> {
    pub(crate) fn recv_msg_unix(fd: SharedFd, mut buf: T) -> io::Result<Self> {
        let mut info: Box<(MaybeUninit<sockaddr_storage>, IoVecMeta, libc::msghdr)> =
            Box::new((MaybeUninit::uninit(), IoVecMeta::from(&mut buf), unsafe {
                std::mem::zeroed()
            }));

        info.2.msg_iov = info.1.write_iovec_ptr();
        info.2.msg_iovlen = info.1.write_iovec_len() as _;
        info.2.msg_name = &mut info.0 as *mut _ as *mut libc::c_void;
        info.2.msg_namelen = std::mem::size_of::<sockaddr_storage>() as socklen_t;

        Op::submit_with(RecvMsgUnix { fd, buf, info })
    }

    pub(crate) async fn wait(self) -> BufResult<(usize, UnixSocketAddr), T> {
        let complete = self.await;
        let res = complete.meta.result.map(|v| v.into_inner() as _);
        let mut buf = complete.data.buf;

        let res = res.map(|n| {
            let storage = unsafe { complete.data.info.0.assume_init() };
            let name_len = complete.data.info.2.msg_namelen;

            let addr = unsafe {
                let addr: &libc::sockaddr_un = transmute(&storage);
                UnixSocketAddr::from_parts(*addr, name_len)
            };

            // Safety: the kernel wrote `n` bytes to the buffer.
            unsafe {
                buf.set_init(n);
            }

            (n, addr)
        });
        (res, buf)
    }
}

#[cfg(unix)]
impl<T: IoBufMut> OpAble for RecvMsgUnix<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        opcode::RecvMsg::new(types::Fd(self.fd.raw_fd()), &mut self.info.2 as *mut _).build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd.registered_index().map(|idx| (Direction::Read, idx))
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        let fd = self.fd.as_raw_fd();
        crate::syscall!(recvmsg@NON_FD(fd, &mut self.info.2 as *mut _, 0))
    }
}
