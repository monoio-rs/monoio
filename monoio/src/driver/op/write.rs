use std::io;
#[cfg(unix)]
use std::os::unix::prelude::AsRawFd;
#[cfg(all(windows, any(feature = "legacy", feature = "poll-io")))]
use std::os::windows::io::AsRawHandle;

#[cfg(any(feature = "legacy", feature = "poll-io"))]
pub(crate) use impls::*;
#[cfg(all(target_os = "linux", feature = "iouring"))]
use io_uring::{opcode, types};
#[cfg(all(windows, any(feature = "legacy", feature = "poll-io")))]
use windows_sys::Win32::{Foundation::TRUE, Storage::FileSystem::WriteFile};

use super::{super::shared_fd::SharedFd, Op, OpAble};
#[cfg(any(feature = "legacy", feature = "poll-io"))]
use super::{driver::ready::Direction, MaybeFd};
use crate::{
    buf::{IoBuf, IoVecBuf},
    BufResult,
};

macro_rules! write_result {
    ($($name:ident<$T:ident : $Trait:ident> { $buf:ident }), * $(,)?) => {
        $(
            impl<$T: $Trait> super::Op<$name<$T>> {
                pub(crate) async fn result(self) -> BufResult<usize, $T> {
                    let complete = self.await;
                    (complete.meta.result.map(|v| v.into_inner() as _), complete.data.$buf)
                }
            }
        )*
    };
}

write_result! {
    Write<T: IoBuf> { buf },
    WriteAt<T: IoBuf> { buf },
    WriteVec<T: IoVecBuf> { buf_vec },
}

#[cfg(not(windows))]
write_result! {
    WriteVecAt<T: IoVecBuf> { buf_vec },
}

pub(crate) struct Write<T> {
    fd: SharedFd,
    pub(crate) buf: T,
}

impl<T: IoBuf> Op<Write<T>> {
    pub(crate) fn write(fd: SharedFd, buf: T) -> io::Result<Self> {
        Op::submit_with(Write { fd, buf })
    }
}

impl<T: IoBuf> OpAble for Write<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        // Refers to https://docs.rs/io-uring/latest/io_uring/opcode/struct.Write.html.
        //
        // If `offset` is set to `-1`, the offset will use (and advance) the file position, like
        // the write(2) system calls
        opcode::Write::new(
            types::Fd(self.fd.as_raw_fd()),
            self.buf.read_ptr(),
            self.buf.bytes_init() as _,
        )
        .offset(-1i64 as _)
        .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(crate::driver::ready::Direction, usize)> {
        self.fd
            .registered_index()
            .map(|idx| (Direction::Write, idx))
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        #[cfg(windows)]
        let fd = self.fd.as_raw_handle() as _;
        #[cfg(unix)]
        let fd = self.fd.as_raw_fd();
        write(fd, self.buf.read_ptr(), self.buf.bytes_init())
    }
}

pub(crate) struct WriteAt<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    #[allow(unused)]
    fd: SharedFd,
    /// Refers to https://docs.rs/io-uring/latest/io_uring/opcode/struct.Write.html.
    ///
    /// If `offset` is set to `-1`, the offset will use (and advance) the file position, like
    /// the write(2) system calls.
    offset: u64,
    pub(crate) buf: T,
}

impl<T: IoBuf> Op<WriteAt<T>> {
    pub(crate) fn write_at(fd: SharedFd, buf: T, offset: u64) -> io::Result<Op<WriteAt<T>>> {
        Op::submit_with(WriteAt { fd, offset, buf })
    }
}

impl<T: IoBuf> OpAble for WriteAt<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        opcode::Write::new(
            types::Fd(self.fd.raw_fd()),
            self.buf.read_ptr(),
            self.buf.bytes_init() as _,
        )
        .offset(self.offset)
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
        #[cfg(windows)]
        let fd = self.fd.as_raw_handle() as _;
        #[cfg(unix)]
        let fd = self.fd.as_raw_fd();

        write_at(fd, self.buf.read_ptr(), self.buf.bytes_init(), self.offset)
    }
}

pub(crate) struct WriteVec<T> {
    /// Holds a strong ref to the FD, preventing the file from being closed
    /// while the operation is in-flight.
    fd: SharedFd,
    pub(crate) buf_vec: T,
}

impl<T: IoVecBuf> Op<WriteVec<T>> {
    pub(crate) fn writev(fd: SharedFd, buf_vec: T) -> io::Result<Self> {
        Op::submit_with(WriteVec { fd, buf_vec })
    }

    pub(crate) fn writev_raw(fd: &SharedFd, buf_vec: T) -> WriteVec<T> {
        WriteVec {
            fd: fd.clone(),
            buf_vec,
        }
    }
}

impl<T: IoVecBuf> OpAble for WriteVec<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        let ptr = self.buf_vec.read_iovec_ptr() as *const _;
        let len = self.buf_vec.read_iovec_len() as _;
        // Refers to https://docs.rs/io-uring/latest/io_uring/opcode/struct.Write.html.
        //
        // If `offset` is set to `-1`, the offset will use (and advance) the file position, like
        // the writev(2) system calls
        opcode::Writev::new(types::Fd(self.fd.raw_fd()), ptr, len)
            .offset(-1i64 as u64)
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
        #[cfg(windows)]
        let fd = self.fd.as_raw_handle() as _;
        #[cfg(unix)]
        let fd = self.fd.as_raw_fd();

        let (buf_vec, len) = {
            #[cfg(unix)]
            {
                (self.buf_vec.read_iovec_ptr(), self.buf_vec.read_iovec_len())
            }
            #[cfg(windows)]
            {
                (
                    self.buf_vec.read_wsabuf_ptr(),
                    self.buf_vec.read_wsabuf_len(),
                )
            }
        };

        write_vectored(fd, buf_vec, len)
    }
}

#[cfg(not(windows))]
pub(crate) struct WriteVecAt<T> {
    fd: SharedFd,
    /// Refers to https://docs.rs/io-uring/latest/io_uring/opcode/struct.Write.html.
    ///
    /// If `offset` is set to `-1`, the offset will use (and advance) the file position, like
    /// the writev(2) system calls.
    offset: u64,
    buf_vec: T,
}

#[cfg(not(windows))]
impl<T: IoVecBuf> OpAble for WriteVecAt<T> {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        opcode::Writev::new(
            types::Fd(libc::AT_FDCWD),
            self.buf_vec.read_iovec_ptr(),
            self.buf_vec.read_iovec_len() as _,
        )
        .offset(self.offset)
        .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_interest(&self) -> Option<(crate::driver::ready::Direction, usize)> {
        self.fd
            .registered_index()
            .map(|idx| (Direction::Write, idx))
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        write_vectored_at(
            self.fd.raw_fd(),
            self.buf_vec.read_iovec_ptr(),
            self.buf_vec.read_iovec_len(),
            self.offset,
        )
    }
}

#[cfg(all(any(feature = "legacy", feature = "poll-io"), unix))]
pub(crate) mod impls {
    use libc::iovec;

    use super::*;

    /// A wrapper of [`libc::write`]
    pub(crate) fn write(fd: i32, buf: *const u8, len: usize) -> io::Result<MaybeFd> {
        crate::syscall!(write@NON_FD(fd, buf as _, len))
    }

    /// A wrapper of [`libc::write`]
    pub(crate) fn write_at(
        fd: i32,
        buf: *const u8,
        len: usize,
        offset: u64,
    ) -> io::Result<MaybeFd> {
        let offset =
            libc::off_t::try_from(offset).map_err(|_| io::Error::other("offset too big"))?;

        crate::syscall!(pwrite@NON_FD(fd, buf as _, len, offset))
    }

    /// A wrapper of [`libc::writev`]
    pub(crate) fn write_vectored(
        fd: i32,
        buf_vec: *const iovec,
        len: usize,
    ) -> io::Result<MaybeFd> {
        crate::syscall!(writev@NON_FD(fd, buf_vec as _, len as _))
    }

    /// A wrapper of [`libc::pwritev`]
    pub(crate) fn write_vectored_at(
        fd: i32,
        buf_vec: *const iovec,
        len: usize,
        offset: u64,
    ) -> io::Result<MaybeFd> {
        let offset =
            libc::off_t::try_from(offset).map_err(|_| io::Error::other("offset too big"))?;

        crate::syscall!(pwritev@NON_FD(fd, buf_vec as _, len as _, offset))
    }
}

#[cfg(all(any(feature = "legacy", feature = "poll-io"), windows))]
pub(crate) mod impls {
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_HANDLE_EOF},
        Networking::WinSock::WSABUF,
        System::IO::OVERLAPPED,
    };

    use super::*;

    /// A wrapper of [`windows_sys::Win32::Storage::FileSystem::WriteFile`]
    pub(crate) fn write(fd: isize, buf: *const u8, len: usize) -> io::Result<MaybeFd> {
        let mut bytes_write = 0;

        let ret = unsafe { WriteFile(fd, buf, len as _, &mut bytes_write, std::ptr::null_mut()) };
        if ret == TRUE {
            return Ok(MaybeFd::new_non_fd(bytes_write));
        }

        match unsafe { GetLastError() } {
            ERROR_HANDLE_EOF => Ok(MaybeFd::new_non_fd(bytes_write)),
            error => Err(io::Error::from_raw_os_error(error as _)),
        }
    }

    /// A wrapper of [`windows_sys::Win32::Storage::FileSystem::WriteFile`],
    /// using [`windows_sys::Win32::System::IO::OVERLAPPED`] to write at specific offset.
    pub(crate) fn write_at(
        fd: isize,
        buf: *const u8,
        len: usize,
        offset: u64,
    ) -> io::Result<MaybeFd> {
        let mut bytes_write = 0;

        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.Anonymous.Anonymous.Offset = offset as u32; // Lower 32 bits of the offset
        overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32; // Higher 32 bits of the offset

        let ret = unsafe {
            WriteFile(
                fd,
                buf,
                len as _,
                &mut bytes_write,
                &overlapped as *const _ as *mut _,
            )
        };

        if ret == TRUE {
            return Ok(MaybeFd::new_non_fd(bytes_write));
        }

        match unsafe { GetLastError() } {
            ERROR_HANDLE_EOF => Ok(MaybeFd::new_non_fd(bytes_write)),
            error => Err(io::Error::from_raw_os_error(error as _)),
        }
    }

    /// There is no `writev` like syscall of file on windows, but this will be used to send socket
    /// message.
    pub(crate) fn write_vectored(
        fd: usize,
        buf_vec: *const WSABUF,
        len: usize,
    ) -> io::Result<MaybeFd> {
        use windows_sys::Win32::Networking::WinSock::{WSAGetLastError, WSASend, WSAESHUTDOWN};

        let mut bytes_sent = 0;

        let ret = unsafe {
            WSASend(
                fd,
                buf_vec,
                len as _,
                &mut bytes_sent,
                0,
                std::ptr::null_mut(),
                None,
            )
        };

        match ret {
            0 => Ok(MaybeFd::new_non_fd(bytes_sent)),
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
