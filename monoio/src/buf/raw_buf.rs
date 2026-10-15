use std::ptr::null;

#[cfg(windows)]
use windows_sys::Win32::Networking::WinSock::WSABUF;

use super::{IoBuf, IoBufMut, IoVecBuf, IoVecBufMut};

/// RawBuf is not a real buf. It only hold the pointer of the buffer.
/// Users must make sure the buffer behind the pointer is always valid.
/// Which means, user must:
/// 1. await the future with RawBuf Ready before drop the real buffer
/// 2. make sure the pointer and length is valid before the future Ready
pub struct RawBuf {
    ptr: *const u8,
    len: usize,
}

impl RawBuf {
    /// Create a empty RawBuf.
    /// # Safety
    /// do not use uninitialized RawBuf directly.
    #[inline]
    pub unsafe fn uninit() -> Self {
        Self {
            ptr: null(),
            len: 0,
        }
    }

    /// Create a new RawBuf with given pointer and length.
    /// # Safety
    /// make sure the pointer and length is valid when RawBuf is used.
    #[inline]
    pub const unsafe fn new(ptr: *const u8, len: usize) -> Self {
        Self { ptr, len }
    }
}

unsafe impl IoBuf for RawBuf {
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        self.ptr
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        self.len
    }
}

unsafe impl IoBufMut for RawBuf {
    #[inline]
    fn write_ptr(&mut self) -> *mut u8 {
        self.ptr as *mut u8
    }

    #[inline]
    fn bytes_total(&mut self) -> usize {
        self.len
    }

    #[inline]
    unsafe fn set_init(&mut self, _pos: usize) {}
}

impl RawBuf {
    /// Create a new RawBuf with the first iovec part.
    /// # Safety
    /// make sure the pointer and length is valid when RawBuf is used.
    #[inline]
    pub unsafe fn new_from_iovec_mut<T: IoVecBufMut>(data: &mut T) -> Option<Self> { unsafe {
        #[cfg(unix)]
        {
            if data.write_iovec_len() == 0 {
                return None;
            }
            let iovec = *data.write_iovec_ptr();
            Some(Self::new(iovec.iov_base as *const u8, iovec.iov_len))
        }
        #[cfg(windows)]
        {
            if data.write_wsabuf_len() == 0 {
                return None;
            }
            let wsabuf = *data.write_wsabuf_ptr();
            Some(Self::new(wsabuf.buf as *const u8, wsabuf.len as _))
        }
    }}

    /// Create a new RawBuf with the first iovec part.
    /// # Safety
    /// make sure the pointer and length is valid when RawBuf is used.
    #[inline]
    pub unsafe fn new_from_iovec<T: IoVecBuf>(data: &T) -> Option<Self> { unsafe {
        #[cfg(unix)]
        {
            if data.read_iovec_len() == 0 {
                return None;
            }
            let iovec = *data.read_iovec_ptr();
            Some(Self::new(iovec.iov_base as *const u8, iovec.iov_len))
        }
        #[cfg(windows)]
        {
            if data.read_wsabuf_len() == 0 {
                return None;
            }
            let wsabuf = *data.read_wsabuf_ptr();
            Some(Self::new(wsabuf.buf as *const u8, wsabuf.len as _))
        }
    }}
}

/// RawBufVectored behaves like RawBuf.
/// And user must obey the following restrictions:
/// 1. await the future with RawBuf Ready before drop the real buffer
/// 2. make sure the pointer and length is valid before the future Ready
pub struct RawBufVectored {
    #[cfg(unix)]
    ptr: *const libc::iovec,
    #[cfg(windows)]
    ptr: *const WSABUF,
    len: usize,
}

impl RawBufVectored {
    /// Create a new RawBuf with given pointer and length.
    /// # Safety
    /// make sure the pointer and length is valid when RawBuf is used.
    #[cfg(unix)]
    #[inline]
    pub const unsafe fn new(ptr: *const libc::iovec, len: usize) -> Self {
        Self { ptr, len }
    }

    /// Create a new RawBuf with given pointer and length.
    /// # Safety
    /// make sure the pointer and length is valid when RawBuf is used.
    #[cfg(windows)]
    #[inline]
    pub const unsafe fn new(ptr: *const WSABUF, len: usize) -> Self {
        Self { ptr, len }
    }
}

unsafe impl IoVecBuf for RawBufVectored {
    #[cfg(unix)]
    #[inline]
    fn read_iovec_ptr(&self) -> *const libc::iovec {
        self.ptr
    }

    #[cfg(unix)]
    #[inline]
    fn read_iovec_len(&self) -> usize {
        self.len
    }

    #[cfg(windows)]
    #[inline]
    fn read_wsabuf_ptr(&self) -> *const WSABUF {
        self.ptr
    }

    #[cfg(windows)]
    #[inline]
    fn read_wsabuf_len(&self) -> usize {
        self.len
    }
}

unsafe impl IoVecBufMut for RawBufVectored {
    #[cfg(unix)]
    fn write_iovec_ptr(&mut self) -> *mut libc::iovec {
        self.ptr as *mut libc::iovec
    }

    #[cfg(unix)]
    fn write_iovec_len(&mut self) -> usize {
        self.len
    }

    #[cfg(windows)]
    #[inline]
    fn write_wsabuf_ptr(&mut self) -> *mut WSABUF {
        self.ptr as *mut WSABUF
    }

    #[cfg(windows)]
    #[inline]
    fn write_wsabuf_len(&mut self) -> usize {
        self.len
    }

    unsafe fn set_init(&mut self, _pos: usize) {}
}
