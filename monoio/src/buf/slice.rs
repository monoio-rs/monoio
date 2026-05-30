use std::ops;

use super::{IoVecBuf, IoVecBufMut};
use crate::buf::{IoBuf, IoBufMut};

/// An owned view into a contiguous sequence of bytes.
/// SliceMut implements IoBuf and IoBufMut.
///
/// This is similar to Rust slices (`&buf[..]`) but owns the underlying buffer.
/// This type is useful for performing io_uring read and write operations using
/// a subset of a buffer.
///
/// Slices are created using [`IoBuf::slice`].
///
/// # Examples
///
/// Creating a slice
///
/// ```
/// use monoio::buf::{IoBuf, IoBufMut};
///
/// let buf = b"hello world".to_vec();
/// let slice = buf.slice_mut(..5);
///
/// assert_eq!(&slice[..], b"hello");
/// ```
pub struct SliceMut<T> {
    buf: T,
    begin: usize,
    end: usize,
}

impl<T: IoBuf + IoBufMut> SliceMut<T> {
    /// Create a SliceMut from a buffer and range.
    #[inline]
    pub fn new(mut buf: T, begin: usize, end: usize) -> Self {
        assert!(end <= buf.bytes_total());
        assert!(begin <= buf.bytes_init());
        assert!(begin <= end);
        Self { buf, begin, end }
    }
}

impl<T> SliceMut<T> {
    /// Create a SliceMut from a buffer and range without boundary checking.
    ///
    /// # Safety
    /// begin must be initialized, and end must be within the buffer capacity.
    #[inline]
    pub const unsafe fn new_unchecked(buf: T, begin: usize, end: usize) -> Self {
        Self { buf, begin, end }
    }

    /// Offset in the underlying buffer at which this slice starts.
    ///
    /// # Examples
    ///
    /// ```
    /// use monoio::buf::IoBuf;
    ///
    /// let buf = b"hello world".to_vec();
    /// let slice = buf.slice(1..5);
    ///
    /// assert_eq!(1, slice.begin());
    /// ```
    #[inline]
    pub const fn begin(&self) -> usize {
        self.begin
    }

    /// Offset in the underlying buffer at which this slice ends.
    ///
    /// # Examples
    ///
    /// ```
    /// use monoio::buf::IoBuf;
    ///
    /// let buf = b"hello world".to_vec();
    /// let slice = buf.slice(1..5);
    ///
    /// assert_eq!(5, slice.end());
    /// ```
    #[inline]
    pub const fn end(&self) -> usize {
        self.end
    }

    /// Gets a reference to the underlying buffer.
    ///
    /// This method escapes the slice's view.
    ///
    /// # Examples
    ///
    /// ```
    /// use monoio::buf::{IoBuf, IoBufMut};
    ///
    /// let buf = b"hello world".to_vec();
    /// let slice = buf.slice_mut(..5);
    ///
    /// assert_eq!(slice.get_ref(), b"hello world");
    /// assert_eq!(&slice[..], b"hello");
    /// ```
    #[inline]
    pub const fn get_ref(&self) -> &T {
        &self.buf
    }

    /// Gets a mutable reference to the underlying buffer.
    ///
    /// This method escapes the slice's view.
    ///
    /// # Examples
    ///
    /// ```
    /// use monoio::buf::{IoBuf, IoBufMut};
    ///
    /// let buf = b"hello world".to_vec();
    /// let mut slice = buf.slice_mut(..5);
    ///
    /// slice.get_mut()[0] = b'b';
    ///
    /// assert_eq!(slice.get_mut(), b"bello world");
    /// assert_eq!(&slice[..], b"bello");
    /// ```
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.buf
    }

    /// Unwraps this `Slice`, returning the underlying buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use monoio::buf::IoBuf;
    ///
    /// let buf = b"hello world".to_vec();
    /// let slice = buf.slice(..5);
    ///
    /// let buf = slice.into_inner();
    /// assert_eq!(buf, b"hello world");
    /// ```
    #[inline]
    pub fn into_inner(self) -> T {
        self.buf
    }
}

impl<T: IoBuf> ops::Deref for SliceMut<T> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        let buf_bytes = super::deref(&self.buf);
        let end = std::cmp::min(self.end, buf_bytes.len());
        &buf_bytes[self.begin..end]
    }
}

unsafe impl<T: IoBuf> IoBuf for SliceMut<T> {
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        super::deref(&self.buf)[self.begin..].as_ptr()
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        ops::Deref::deref(self).len()
    }
}

unsafe impl<T: IoBufMut> IoBufMut for SliceMut<T> {
    #[inline]
    fn write_ptr(&mut self) -> *mut u8 {
        unsafe { self.buf.write_ptr().add(self.begin) }
    }

    #[inline]
    fn bytes_total(&mut self) -> usize {
        self.end - self.begin
    }

    #[inline]
    unsafe fn set_init(&mut self, n: usize) { unsafe {
        self.buf.set_init(self.begin + n);
    }}
}

/// An owned view into a contiguous sequence of bytes.
/// Slice implements IoBuf.
pub struct Slice<T> {
    buf: T,
    begin: usize,
    end: usize,
}

impl<T: IoBuf> Slice<T> {
    /// Create a Slice from a buffer and range.
    #[inline]
    pub fn new(buf: T, begin: usize, end: usize) -> Self {
        assert!(end <= buf.bytes_init());
        assert!(begin <= end);
        Self { buf, begin, end }
    }
}

impl<T> Slice<T> {
    /// Create a Slice from a buffer and range without boundary checking.
    ///
    /// # Safety
    /// begin and end must be within the buffer initialized range.
    #[inline]
    pub const unsafe fn new_unchecked(buf: T, begin: usize, end: usize) -> Self {
        Self { buf, begin, end }
    }

    /// Offset in the underlying buffer at which this slice starts.
    #[inline]
    pub const fn begin(&self) -> usize {
        self.begin
    }

    /// Ofset in the underlying buffer at which this slice ends.
    #[inline]
    pub const fn end(&self) -> usize {
        self.end
    }

    /// Gets a reference to the underlying buffer.
    #[inline]
    pub const fn get_ref(&self) -> &T {
        &self.buf
    }

    /// Gets a mutable reference to the underlying buffer.
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.buf
    }

    /// Unwraps this `Slice`, returning the underlying buffer.
    #[inline]
    pub fn into_inner(self) -> T {
        self.buf
    }
}

unsafe impl<T: IoBuf> IoBuf for Slice<T> {
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        unsafe { self.buf.read_ptr().add(self.begin) }
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        self.end - self.begin
    }
}

/// A wrapper to make IoVecBuf impl IoBuf.
pub struct IoVecWrapper<T> {
    // we must make sure raw contains at least one iovec.
    raw: T,
}

impl<T: IoVecBuf> IoVecWrapper<T> {
    /// Create a new IoVecWrapper with something that impl IoVecBuf.
    #[inline]
    pub fn new(buf: T) -> Result<Self, T> {
        #[cfg(unix)]
        if buf.read_iovec_len() == 0 {
            return Err(buf);
        }
        #[cfg(windows)]
        if buf.read_wsabuf_len() == 0 {
            return Err(buf);
        }
        Ok(Self { raw: buf })
    }

    /// Consume self and return raw iovec buf.
    #[inline]
    pub fn into_inner(self) -> T {
        self.raw
    }
}

unsafe impl<T: IoVecBuf> IoBuf for IoVecWrapper<T> {
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        #[cfg(unix)]
        {
            let iovec = unsafe { *self.raw.read_iovec_ptr() };
            iovec.iov_base as *const u8
        }
        #[cfg(windows)]
        {
            let wsabuf = unsafe { *self.raw.read_wsabuf_ptr() };
            wsabuf.buf as *const u8
        }
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        #[cfg(unix)]
        {
            let iovec = unsafe { *self.raw.read_iovec_ptr() };
            iovec.iov_len
        }
        #[cfg(windows)]
        {
            let wsabuf = unsafe { *self.raw.read_wsabuf_ptr() };
            wsabuf.len as _
        }
    }
}

/// A wrapper to make IoVecBufMut impl IoBufMut.
pub struct IoVecWrapperMut<T> {
    // we must make sure raw contains at least one iovec.
    raw: T,
}

impl<T: IoVecBufMut> IoVecWrapperMut<T> {
    /// Create a new IoVecWrapperMut with something that impl IoVecBufMut.
    #[inline]
    pub fn new(mut iovec_buf: T) -> Result<Self, T> {
        #[cfg(unix)]
        if iovec_buf.write_iovec_len() == 0 {
            return Err(iovec_buf);
        }
        #[cfg(windows)]
        if iovec_buf.write_wsabuf_len() == 0 {
            return Err(iovec_buf);
        }
        Ok(Self { raw: iovec_buf })
    }

    /// Consume self and return raw iovec buf.
    #[inline]
    pub fn into_inner(self) -> T {
        self.raw
    }
}

unsafe impl<T: IoVecBufMut> IoBufMut for IoVecWrapperMut<T> {
    fn write_ptr(&mut self) -> *mut u8 {
        #[cfg(unix)]
        {
            let iovec = unsafe { *self.raw.write_iovec_ptr() };
            iovec.iov_base as *mut u8
        }
        #[cfg(windows)]
        {
            let wsabuf = unsafe { *self.raw.write_wsabuf_ptr() };
            wsabuf.buf
        }
    }

    fn bytes_total(&mut self) -> usize {
        #[cfg(unix)]
        {
            let iovec = unsafe { *self.raw.write_iovec_ptr() };
            iovec.iov_len
        }
        #[cfg(windows)]
        {
            let wsabuf = unsafe { *self.raw.write_wsabuf_ptr() };
            wsabuf.len as _
        }
    }

    unsafe fn set_init(&mut self, _pos: usize) {}
}
