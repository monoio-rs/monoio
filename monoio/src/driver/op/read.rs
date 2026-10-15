use std::io;
#[cfg(all(unix, any(feature = "legacy", feature = "poll-io")))]
use std::os::unix::prelude::AsRawFd;

#[cfg(any(feature = "legacy", feature = "poll-io"))]
pub(crate) use impls::*;
#[cfg(all(target_os = "linux", feature = "iouring"))]
use io_uring::{opcode, types};

use super::{super::shared_fd::SharedFd, Op, OpAble};
#[cfg(any(feature = "legacy", feature = "poll-io"))]
use super::{driver::ready::Direction, MaybeFd};
use crate::{
    buf::{IoBufMut, IoVecBufMut},
    BufResult,
};

macro_rules! read_result {
    ($($name:ident<$T:ident : $Trait:ident> { $buf:ident }),* $(,)?) => {
        $(
            impl<$T: $Trait> super::Op<$name<$T>> {
                pub(crate) async fn result(self) -> BufResult<usize, $T> {
                    let complete = self.await;

                    // Convert the operation result to `usize`
                    let res = complete.meta.result.map(|v| v.into_inner() as usize);
                    // Recover the buffer
                    let mut buf = complete.data.$buf;

                    if let Ok(read_len) = res {
                        // Safety: the kernel wrote `n` bytes to the buffer
                        unsafe { buf.set_init(read_len) };
                    }

                    (res, buf)
                }
            }
        )*
    }
}

read_result! {
    Read<T: IoBufMut> { buf },
    ReadAt<T: IoBufMut> { buf },
    ReadVec<T: IoVecBufMut> { buf_vec },
}

#[cfg(not(windows))]
read_result! {
    ReadVecAt<T: IoVecBufMut> { buf_vec },
}

pub(crate) struct Read<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    fd: SharedFd,
    /// Reference to the in-flight buffer.
    pub(crate) buf: T,
}

impl<T: IoBufMut> Op<Read<T>> {
    pub(crate) fn read(fd: SharedFd, buf: T) -> io::Result<Op<Read<T>>> {
        Op::submit_with(Read { fd, buf })
    }
}

impl<T: IoBufMut> OpAble for Read<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        // Refers to https://docs.rs/io-uring/latest/io_uring/opcode/struct.Read.html.
        // If `offset` is set to `-1`, the offset will use (and advance) the file position, like
        // the read(2) syscall.
        opcode::Read::new(
            types::Fd(self.fd.raw_fd()),
            self.buf.write_ptr(),
            self.buf.bytes_total() as _,
        )
        .offset(-1i64 as u64)
        .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd.registered_index().map(|idx| (Direction::Read, idx))
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        #[cfg(unix)]
        let fd = self.fd.as_raw_fd();

        #[cfg(windows)]
        let fd = self.fd.raw_handle() as _;

        read(fd, self.buf.write_ptr(), self.buf.bytes_total())
    }
}

pub(crate) struct ReadAt<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    fd: SharedFd,
    /// Reference to the in-flight buffer.
    pub(crate) buf: T,
    offset: u64,
}

impl<T: IoBufMut> Op<ReadAt<T>> {
    pub(crate) fn read_at(fd: SharedFd, buf: T, offset: u64) -> io::Result<Op<ReadAt<T>>> {
        Op::submit_with(ReadAt { fd, offset, buf })
    }
}

impl<T: IoBufMut> OpAble for ReadAt<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        opcode::Read::new(
            types::Fd(self.fd.raw_fd()),
            self.buf.write_ptr(),
            self.buf.bytes_total() as _,
        )
        .offset(self.offset)
        .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd.registered_index().map(|idx| (Direction::Read, idx))
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        #[cfg(unix)]
        let fd = self.fd.as_raw_fd();
        #[cfg(windows)]
        let fd = self.fd.raw_handle() as _;

        let buf = self.buf.write_ptr();
        let len = self.buf.bytes_total();

        read_at(fd, buf, len, self.offset)
    }
}

pub(crate) struct ReadVec<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    fd: SharedFd,
    /// Reference to the in-flight buffer.
    pub(crate) buf_vec: T,
}

impl<T: IoVecBufMut> Op<ReadVec<T>> {
    pub(crate) fn readv(fd: SharedFd, buf_vec: T) -> io::Result<Self> {
        Op::submit_with(ReadVec { fd, buf_vec })
    }
}

impl<T: IoVecBufMut> OpAble for ReadVec<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        let ptr = self.buf_vec.write_iovec_ptr() as _;
        let len = self.buf_vec.write_iovec_len() as _;

        // Refersto https://docs.rs/io-uring/latest/io_uring/opcode/struct.Readv.html.
        // If `offset` is set to `-1`, the offset will use (and advance) the file position, like
        // the readv(2) syscall.
        opcode::Readv::new(types::Fd(self.fd.raw_fd()), ptr, len)
            .offset(-1i64 as u64)
            .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd.registered_index().map(|idx| (Direction::Read, idx))
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), unix))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        read_vectored(
            self.fd.raw_fd(),
            self.buf_vec.write_iovec_ptr(),
            self.buf_vec.write_iovec_len().min(i32::MAX as usize) as _,
        )
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), windows))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        // There is no `readv`-like syscall of file on windows, but this will be used to send
        // socket message.

        use windows_sys::Win32::Networking::WinSock::{WSAGetLastError, WSARecv, WSAESHUTDOWN};

        let mut nread = 0;
        let mut flags = 0;
        let ret = unsafe {
            WSARecv(
                self.fd.raw_socket() as _,
                self.buf_vec.write_wsabuf_ptr(),
                self.buf_vec.write_wsabuf_len().min(u32::MAX as usize) as _,
                &mut nread,
                &mut flags,
                std::ptr::null_mut(),
                None,
            )
        };
        match ret {
            0 => Ok(MaybeFd::new_non_fd(nread)),
            _ => {
                let error = unsafe { WSAGetLastError() };
                if error == WSAESHUTDOWN {
                    Ok(MaybeFd::zero())
                } else {
                    Err(io::Error::from_raw_os_error(error))
                }
            }
        }
    }
}

pub(crate) struct ReadVecAt<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    fd: SharedFd,
    /// Reference to the in-flight buffer.
    pub(crate) buf_vec: T,
    offset: u64,
}

impl<T: IoVecBufMut> Op<ReadVecAt<T>> {
    pub(crate) fn read_vectored_at(fd: SharedFd, buf_vec: T, offset: u64) -> io::Result<Self> {
        Op::submit_with(ReadVecAt {
            fd,
            buf_vec,
            offset,
        })
    }
}

impl<T: IoVecBufMut> OpAble for ReadVecAt<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        let ptr = self.buf_vec.write_iovec_ptr() as _;
        let len = self.buf_vec.write_iovec_len() as _;
        opcode::Readv::new(types::Fd(self.fd.raw_fd()), ptr, len)
            .offset(self.offset)
            .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd.registered_index().map(|idx| (Direction::Read, idx))
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), unix))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        read_vectored_at(
            self.fd.raw_fd(),
            self.buf_vec.write_iovec_ptr(),
            self.buf_vec.write_iovec_len().min(i32::MAX as usize) as _,
            self.offset,
        )
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), windows))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        // There is no `readv` like syscall of file on windows, but this will be used to send
        // socket message.

        use windows_sys::Win32::{
            Networking::WinSock::{WSAGetLastError, WSARecv, WSAESHUTDOWN},
            System::IO::OVERLAPPED,
        };

        let seek_offset = self.offset;
        let mut nread = 0;
        let mut flags = 0;
        let ret = unsafe {
            let mut overlapped: OVERLAPPED = std::mem::zeroed();
            overlapped.Anonymous.Anonymous.Offset = seek_offset as u32; // Lower 32 bits of the
                                                                        // offset
            overlapped.Anonymous.Anonymous.OffsetHigh = (seek_offset >> 32) as u32; // Higher 32 bits of the offset

            WSARecv(
                self.fd.raw_socket() as _,
                self.buf_vec.write_wsabuf_ptr(),
                self.buf_vec.write_wsabuf_len().min(u32::MAX as usize) as _,
                &mut nread,
                &mut flags,
                &overlapped as *const _ as *mut _,
                None,
            )
        };
        match ret {
            0 => Ok(MaybeFd::new_non_fd(nread)),
            _ => {
                let error = unsafe { WSAGetLastError() };
                if error == WSAESHUTDOWN {
                    Ok(MaybeFd::zero())
                } else {
                    Err(io::Error::from_raw_os_error(error))
                }
            }
        }
    }
}

#[cfg(all(any(feature = "legacy", feature = "poll-io"), unix))]
pub(crate) mod impls {
    use libc::iovec;

    use super::*;

    /// A wrapper for [`libc::read`]
    pub(crate) fn read(fd: i32, buf: *mut u8, len: usize) -> io::Result<MaybeFd> {
        crate::syscall!(read@NON_FD(fd, buf as _, len))
    }

    /// A wrapper of [`libc::pread`]
    pub(crate) fn read_at(fd: i32, buf: *mut u8, len: usize, offset: u64) -> io::Result<MaybeFd> {
        let offset =
            libc::off_t::try_from(offset).map_err(|_| io::Error::other("offset too big"))?;

        crate::syscall!(pread@NON_FD(fd, buf as _, len, offset))
    }

    /// A wrapper of [`libc::readv`]
    pub(crate) fn read_vectored(fd: i32, buf_vec: *mut iovec, len: usize) -> io::Result<MaybeFd> {
        crate::syscall!(readv@NON_FD(fd, buf_vec as _, len as _))
    }

    /// A wrapper of [`libc::preadv`]
    pub(crate) fn read_vectored_at(
        fd: i32,
        buf_vec: *mut iovec,
        len: usize,
        offset: u64,
    ) -> io::Result<MaybeFd> {
        let offset =
            libc::off_t::try_from(offset).map_err(|_| io::Error::other("offset too big"))?;

        crate::syscall!(preadv@NON_FD(fd, buf_vec as _, len as _, offset))
    }
}

#[cfg(all(any(feature = "legacy", feature = "poll-io"), windows))]
pub(crate) mod impls {
    use std::ffi::c_void;

    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_HANDLE_EOF, TRUE},
        Storage::FileSystem::ReadFile,
        System::IO::OVERLAPPED,
    };

    use super::*;

    /// A wrapper of [`windows_sys::Win32::Storage::FileSystem::ReadFile`]
    pub(crate) fn read(handle: isize, buf: *mut u8, len: usize) -> io::Result<MaybeFd> {
        let mut bytes_read = 0;
        let ret = unsafe {
            ReadFile(
                handle,
                buf.cast::<c_void>(),
                len as u32,
                &mut bytes_read,
                std::ptr::null_mut(),
            )
        };

        if ret == TRUE {
            return Ok(MaybeFd::new_non_fd(bytes_read));
        }

        match unsafe { GetLastError() } {
            ERROR_HANDLE_EOF => Ok(MaybeFd::new_non_fd(bytes_read)),
            error => Err(io::Error::from_raw_os_error(error as _)),
        }
    }

    /// A wrapper of [`windows_sys::Win32::Storage::FileSystem::ReadFile`] and using the
    /// [`windows_sys::Win32::System::IO::OVERLAPPED`] to read at specific position.
    pub(crate) fn read_at(
        handle: isize,
        buf: *mut u8,
        len: usize,
        offset: u64,
    ) -> io::Result<MaybeFd> {
        let mut bytes_read = 0;
        let ret = unsafe {
            // see https://learn.microsoft.com/zh-cn/windows/win32/api/fileapi/nf-fileapi-readfile
            let mut overlapped: OVERLAPPED = std::mem::zeroed();
            overlapped.Anonymous.Anonymous.Offset = offset as u32; // Lower 32 bits of the offset
            overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32; // Higher 32 bits of
                                                                               // the offset

            ReadFile(
                handle,
                buf.cast(),
                len as _,
                &mut bytes_read,
                &overlapped as *const _ as *mut _,
            )
        };

        if ret == TRUE {
            return Ok(MaybeFd::new_non_fd(bytes_read));
        }

        match unsafe { GetLastError() } {
            ERROR_HANDLE_EOF => Ok(MaybeFd::new_non_fd(bytes_read)),
            error => Err(io::Error::from_raw_os_error(error as _)),
        }
    }
}
