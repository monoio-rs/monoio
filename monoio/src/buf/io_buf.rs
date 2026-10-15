use std::{ops, rc::Rc, sync::Arc};

use super::Slice;
use crate::buf::slice::SliceMut;

/// An `io_uring` compatible buffer.
///
/// The `IoBuf` trait is implemented by buffer types that can be passed to
/// io_uring operations. Users will not need to use this trait directly, except
/// for the [`slice`] method.
///
/// # Slicing
///
/// Because buffers are passed by ownership to the runtime, Rust's slice API
/// (`&buf[..]`) cannot be used. Instead, `monoio` provides an owned slice
/// API: [`slice()`]. The method takes ownership of the buffer and returns a
/// `Slice<Self>` type that tracks the requested offset.
///
/// [`slice()`]: IoBuf::slice
/// # Safety
/// impl it safely
pub unsafe trait IoBuf: Unpin + 'static {
    /// Returns a raw pointer to the vector's buffer.
    ///
    /// This method is to be used by the `monoio` runtime and it is not
    /// expected for users to call it directly.
    ///
    /// `monoio` Runtime will `Box::pin` the buffer. Runtime makes sure
    /// the buffer will not be moved, and the implement must ensure
    /// `as_ptr` returns the same valid address.
    /// Kernel will read `bytes_init`-length data from the pointer.
    fn read_ptr(&self) -> *const u8;

    /// Number of initialized bytes.
    ///
    /// This method is to be used by the `monoio` runtime and it is not
    /// expected for users to call it directly.
    ///
    /// For `Vec`, this is identical to `len()`.
    fn bytes_init(&self) -> usize;

    /// Returns a slice of the buffer.
    #[inline]
    fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.read_ptr(), self.bytes_init()) }
    }

    /// Returns a view of the buffer with the specified range.
    #[inline]
    fn slice(self, range: impl ops::RangeBounds<usize>) -> Slice<Self>
    where
        Self: Sized,
    {
        let (begin, end) = parse_range(range, self.bytes_init());
        Slice::new(self, begin, end)
    }

    /// Returns a view of the buffer with the specified range without boundary
    /// checking.
    ///
    /// # Safety
    /// Range must be within the bounds of the buffer.
    #[inline]
    unsafe fn slice_unchecked(self, range: impl ops::RangeBounds<usize>) -> Slice<Self>
    where
        Self: Sized,
    { unsafe {
        let (begin, end) = parse_range(range, self.bytes_init());
        Slice::new_unchecked(self, begin, end)
    }}
}

unsafe impl IoBuf for Vec<u8> {
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        self.len()
    }
}

unsafe impl IoBuf for Box<[u8]> {
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        self.len()
    }
}

unsafe impl IoBuf for &'static [u8] {
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        <[u8]>::len(self)
    }
}

unsafe impl<const N: usize> IoBuf for Box<[u8; N]> {
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        self.len()
    }
}

unsafe impl<const N: usize> IoBuf for &'static [u8; N] {
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        self.len()
    }
}

unsafe impl<const N: usize> IoBuf for &'static mut [u8; N] {
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        self.len()
    }
}

unsafe impl IoBuf for &'static str {
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        <str>::len(self)
    }
}

#[cfg(feature = "bytes")]
unsafe impl IoBuf for bytes::Bytes {
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        self.len()
    }
}

#[cfg(feature = "bytes")]
unsafe impl IoBuf for bytes::BytesMut {
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        self.len()
    }
}

unsafe impl<T> IoBuf for Rc<T>
where
    T: IoBuf,
{
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        <T as IoBuf>::read_ptr(self)
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        <T as IoBuf>::bytes_init(self)
    }
}

unsafe impl<T> IoBuf for Arc<T>
where
    T: IoBuf,
{
    #[inline]
    fn read_ptr(&self) -> *const u8 {
        <T as IoBuf>::read_ptr(self)
    }

    #[inline]
    fn bytes_init(&self) -> usize {
        <T as IoBuf>::bytes_init(self)
    }
}

unsafe impl<T> IoBuf for std::mem::ManuallyDrop<T>
where
    T: IoBuf,
{
    fn read_ptr(&self) -> *const u8 {
        <T as IoBuf>::read_ptr(self)
    }

    fn bytes_init(&self) -> usize {
        <T as IoBuf>::bytes_init(self)
    }
}

/// A mutable `io_uring` compatible buffer.
///
/// The `IoBufMut` trait is implemented by buffer types that can be passed to
/// io_uring operations. Users will not need to use this trait directly.
///
/// # Safety
/// See the safety note of the methods.
pub unsafe trait IoBufMut: Unpin + 'static {
    /// Returns a raw mutable pointer to the vector's buffer.
    ///
    /// `monoio` Runtime will `Box::pin` the buffer. Runtime makes sure
    /// the buffer will not be moved, and the implement must ensure
    /// `as_ptr` returns the same valid address.
    /// Kernel will write `bytes_init`-length data to the pointer.
    fn write_ptr(&mut self) -> *mut u8;

    /// Total size of the buffer, including uninitialized memory, if any.
    ///
    /// This method is to be used by the `monoio` runtime and it is not
    /// expected for users to call it directly.
    ///
    /// For `Vec`, this is identical to `capacity()`.
    fn bytes_total(&mut self) -> usize;

    /// Updates the number of initialized bytes.
    ///
    /// The specified `pos` becomes the new value returned by
    /// `IoBuf::bytes_init`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that all bytes starting at `stable_mut_ptr()` up
    /// to `pos` are initialized and owned by the buffer.
    unsafe fn set_init(&mut self, pos: usize);

    /// Returns a view of the buffer with the specified range.
    ///
    /// This method is similar to Rust's slicing (`&buf[..]`), but takes
    /// ownership of the buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use monoio::buf::{IoBuf, IoBufMut};
    ///
    /// let buf = b"hello world".to_vec();
    /// buf.slice(5..10);
    /// ```
    #[inline]
    fn slice_mut(mut self, range: impl ops::RangeBounds<usize>) -> SliceMut<Self>
    where
        Self: Sized,
        Self: IoBuf,
    {
        let (begin, end) = parse_range(range, self.bytes_total());
        SliceMut::new(self, begin, end)
    }

    /// Returns a view of the buffer with the specified range.
    ///
    /// # Safety
    /// Begin must within the initialized bytes, end must be within the
    /// capacity.
    #[inline]
    unsafe fn slice_mut_unchecked(mut self, range: impl ops::RangeBounds<usize>) -> SliceMut<Self>
    where
        Self: Sized,
    { unsafe {
        let (begin, end) = parse_range(range, self.bytes_total());
        SliceMut::new_unchecked(self, begin, end)
    }}
}

unsafe impl IoBufMut for Vec<u8> {
    #[inline]
    fn write_ptr(&mut self) -> *mut u8 {
        self.as_mut_ptr()
    }

    #[inline]
    fn bytes_total(&mut self) -> usize {
        self.capacity()
    }

    #[inline]
    unsafe fn set_init(&mut self, init_len: usize) { unsafe {
        self.set_len(init_len);
    }}
}

unsafe impl IoBufMut for Box<[u8]> {
    #[inline]
    fn write_ptr(&mut self) -> *mut u8 {
        self.as_mut_ptr()
    }

    #[inline]
    fn bytes_total(&mut self) -> usize {
        self.len()
    }

    #[inline]
    unsafe fn set_init(&mut self, _: usize) {}
}

unsafe impl<const N: usize> IoBufMut for Box<[u8; N]> {
    #[inline]
    fn write_ptr(&mut self) -> *mut u8 {
        self.as_mut_ptr()
    }

    #[inline]
    fn bytes_total(&mut self) -> usize {
        self.len()
    }

    #[inline]
    unsafe fn set_init(&mut self, _: usize) {}
}

unsafe impl<const N: usize> IoBufMut for &'static mut [u8; N] {
    #[inline]
    fn write_ptr(&mut self) -> *mut u8 {
        self.as_mut_ptr()
    }

    #[inline]
    fn bytes_total(&mut self) -> usize {
        self.len()
    }

    #[inline]
    unsafe fn set_init(&mut self, _: usize) {}
}

#[cfg(feature = "bytes")]
unsafe impl IoBufMut for bytes::BytesMut {
    #[inline]
    fn write_ptr(&mut self) -> *mut u8 {
        self.as_mut_ptr()
    }

    #[inline]
    fn bytes_total(&mut self) -> usize {
        self.capacity()
    }

    #[inline]
    unsafe fn set_init(&mut self, init_len: usize) { unsafe {
        if self.len() < init_len {
            self.set_len(init_len);
        }
    }}
}

unsafe impl<T> IoBufMut for std::mem::ManuallyDrop<T>
where
    T: IoBufMut,
{
    fn write_ptr(&mut self) -> *mut u8 {
        <T as IoBufMut>::write_ptr(self)
    }

    fn bytes_total(&mut self) -> usize {
        <T as IoBufMut>::bytes_total(self)
    }

    unsafe fn set_init(&mut self, pos: usize) { unsafe {
        <T as IoBufMut>::set_init(self, pos)
    }}
}

fn parse_range(range: impl ops::RangeBounds<usize>, end: usize) -> (usize, usize) {
    use core::ops::Bound;

    let begin = match range.start_bound() {
        Bound::Included(&n) => n,
        Bound::Excluded(&n) => n + 1,
        Bound::Unbounded => 0,
    };

    let end = match range.end_bound() {
        Bound::Included(&n) => n.checked_add(1).expect("out of range"),
        Bound::Excluded(&n) => n,
        Bound::Unbounded => end,
    };
    (begin, end)
}

#[cfg(test)]
mod tests {
    use std::mem::ManuallyDrop;

    use super::*;

    #[test]
    fn io_buf_vec() {
        let mut buf = Vec::with_capacity(10);
        buf.extend_from_slice(b"0123");
        let ptr = buf.as_mut_ptr();

        assert_eq!(buf.read_ptr(), ptr);
        assert_eq!(buf.bytes_init(), 4);

        assert_eq!(buf.write_ptr(), ptr);
        assert_eq!(buf.bytes_total(), 10);

        unsafe { buf.set_init(8) };
        assert_eq!(buf.bytes_init(), 8);
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn io_buf_str() {
        let s = "hello world";
        let ptr = s.as_ptr();

        assert_eq!(s.read_ptr(), ptr);
        assert_eq!(s.bytes_init(), 11);
    }

    #[test]
    fn io_buf_n() {
        let mut buf = Box::new([1, 2, 3, 4, 5]);
        let ptr = buf.as_mut_ptr();

        assert_eq!(buf.read_ptr(), ptr);
        assert_eq!(buf.bytes_init(), 5);
        assert_eq!(buf.write_ptr(), ptr);
        assert_eq!(buf.bytes_total(), 5);
    }

    #[test]
    fn io_buf_n_boxed() {
        let mut buf = Box::new([1, 2, 3, 4, 5]);
        let ptr = buf.as_mut_ptr();

        assert_eq!(buf.read_ptr(), ptr);
        assert_eq!(buf.bytes_init(), 5);
        assert_eq!(buf.write_ptr(), ptr);
        assert_eq!(buf.bytes_total(), 5);
    }

    #[test]
    fn io_buf_n_static() {
        let buf = &*Box::leak(Box::new([1, 2, 3, 4, 5]));
        let ptr = buf.as_ptr();

        assert_eq!(buf.read_ptr(), ptr);
        assert_eq!(buf.bytes_init(), 5);
    }

    #[test]
    fn io_buf_n_mut_static() {
        let mut buf = Box::leak(Box::new([1, 2, 3, 4, 5]));
        let ptr = buf.as_mut_ptr();

        assert_eq!(buf.read_ptr(), ptr);
        assert_eq!(buf.bytes_init(), 5);
        assert_eq!(buf.write_ptr(), ptr);
        assert_eq!(buf.bytes_total(), 5);
    }

    #[test]
    fn io_buf_rc_str() {
        let s = Rc::new("hello world");
        let ptr = s.as_ptr();

        assert_eq!(s.read_ptr(), ptr);
        assert_eq!(s.bytes_init(), 11);
    }

    #[test]
    fn io_buf_arc_str() {
        let s = Arc::new("hello world");
        let ptr = s.as_ptr();

        assert_eq!(s.read_ptr(), ptr);
        assert_eq!(s.bytes_init(), 11);
    }

    #[test]
    fn io_buf_slice_ref() {
        let s: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let ptr = s.as_ptr();

        assert_eq!(s.read_ptr(), ptr);
        assert_eq!(s.bytes_init(), 10);
    }

    #[test]
    fn io_buf_slice() {
        let mut buf = Vec::with_capacity(10);
        buf.extend_from_slice(b"0123");
        let ptr = buf.as_mut_ptr();

        let slice = buf.slice(1..3);
        assert_eq!((slice.begin(), slice.end()), (1, 3));
        assert_eq!(slice.read_ptr(), unsafe { ptr.add(1) });
        assert_eq!(slice.bytes_init(), 2);
        let buf = slice.into_inner();

        let mut slice = buf.slice_mut(1..8);
        assert_eq!((slice.begin(), slice.end()), (1, 8));
        assert_eq!(slice.write_ptr(), unsafe { ptr.add(1) });
        assert_eq!(slice.bytes_total(), 7);
        unsafe { slice.set_init(5) };
        assert_eq!(slice.bytes_init(), 5);
        assert_eq!(slice.into_inner().len(), 6);
    }

    #[test]
    fn io_buf_arc_slice() {
        let mut buf = Vec::with_capacity(10);
        buf.extend_from_slice(b"0123");
        let buf = Arc::new(buf);
        let ptr = buf.as_ptr();

        let slice = buf.slice(1..3);
        assert_eq!((slice.begin(), slice.end()), (1, 3));
        assert_eq!(slice.read_ptr(), unsafe { ptr.add(1) });
        assert_eq!(slice.bytes_init(), 2);
        let buf = Arc::into_inner(slice.into_inner()).unwrap();

        let mut slice = buf.slice_mut(1..8);
        assert_eq!((slice.begin(), slice.end()), (1, 8));
        assert_eq!(slice.bytes_total(), 7);
        unsafe { slice.set_init(5) };
        assert_eq!(slice.bytes_init(), 5);
        assert_eq!(slice.into_inner().len(), 6);
    }

    #[test]
    fn io_buf_manually_drop() {
        let mut buf = Vec::with_capacity(10);
        buf.extend_from_slice(b"0123");
        let mut buf = ManuallyDrop::new(buf);
        let ptr = buf.as_ptr();

        assert_eq!(buf.write_ptr(), ptr as _);
        assert_eq!(buf.read_ptr(), ptr);
        assert_eq!(buf.bytes_init(), 4);
        unsafe { buf.set_init(3) };
        assert_eq!(buf.bytes_init(), 3);
        assert_eq!(buf.bytes_total(), 10);

        let slice = buf.slice(1..3);
        assert_eq!((slice.begin(), slice.end()), (1, 3));
        assert_eq!(slice.read_ptr(), unsafe { ptr.add(1) });
        assert_eq!(slice.bytes_init(), 2);
        let buf = ManuallyDrop::into_inner(slice.into_inner());

        let mut slice = buf.slice_mut(1..8);
        assert_eq!((slice.begin(), slice.end()), (1, 8));
        assert_eq!(slice.bytes_total(), 7);
        unsafe { slice.set_init(5) };
        assert_eq!(slice.bytes_init(), 5);
        assert_eq!(slice.into_inner().len(), 6);
    }
}
