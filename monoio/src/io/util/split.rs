use std::{
    cell::UnsafeCell,
    error::Error,
    fmt::{self, Debug},
    future::Future,
    rc::Rc,
};

use crate::io::{AsyncReadRent, AsyncWriteRent};

/// Owned Read Half Part
#[derive(Debug)]
pub struct OwnedReadHalf<T>(pub Rc<UnsafeCell<T>>);
/// Owned Write Half Part
#[derive(Debug)]
pub struct OwnedWriteHalf<T>(pub Rc<UnsafeCell<T>>)
where
    T: AsyncWriteRent;
/// Borrowed Write Half Part
#[derive(Debug)]
pub struct WriteHalf<'cx, T>(pub &'cx T);
/// Borrowed Read Half Part
#[derive(Debug)]
pub struct ReadHalf<'cx, T>(pub &'cx T);

/// This is a dummy unsafe trait to inform monoio,
/// the object with has this `Split` trait can be safely split
/// to read/write object in both form of `Owned` or `Borrowed`.
///
/// # Safety
/// monoio cannot guarantee whether the custom object can be
/// safely split to divided objects. Users should ensure the safety
/// by themselves.
pub unsafe trait Split {}

/// Inner split trait
pub trait Splitable {
    /// Owned Read Split
    type OwnedRead;
    /// Owned Write Split
    type OwnedWrite;

    /// Borrowed Read Split
    type Read<'cx>
    where
        Self: 'cx;
    /// Borrowed Write Split
    type Write<'cx>
    where
        Self: 'cx;

    /// Split into owned parts
    fn into_split(self) -> (Self::OwnedRead, Self::OwnedWrite);

    /// Split into borrowed parts
    fn split(&mut self) -> (Self::Read<'_>, Self::Write<'_>);
}

impl<T> Splitable for T
where
    T: Split + AsyncReadRent + AsyncWriteRent,
{
    type Read<'cx> = ReadHalf<'cx, T> where Self: 'cx;

    type Write<'cx> = WriteHalf<'cx, T> where Self: 'cx;

    type OwnedRead = OwnedReadHalf<T>;

    type OwnedWrite = OwnedWriteHalf<T>;

    fn into_split(self) -> (Self::OwnedRead, Self::OwnedWrite) {
        let shared = Rc::new(UnsafeCell::new(self));
        (OwnedReadHalf(shared.clone()), OwnedWriteHalf(shared))
    }

    fn split(&mut self) -> (Self::Read<'_>, Self::Write<'_>) {
        (ReadHalf(&*self), WriteHalf(&*self))
    }
}

impl<Inner> AsyncReadRent for OwnedReadHalf<Inner>
where
    Inner: AsyncReadRent,
{
    type ReadFuture<'a, T> = impl std::future::Future<Output = crate::BufResult<usize, T>>
    where
        Self: 'a,
        T: crate::buf::IoBufMut + 'a;

    type ReadvFuture<'a, T> = impl std::future::Future<Output = crate::BufResult<usize, T>>
    where
        Self: 'a,
        T: crate::buf::IoVecBufMut + 'a;

    fn read<T: crate::buf::IoBufMut>(&mut self, buf: T) -> Self::ReadFuture<'_, T> {
        let stream = unsafe { &mut *self.0.get() };
        stream.read(buf)
    }

    fn readv<T: crate::buf::IoVecBufMut>(&mut self, buf: T) -> Self::ReadvFuture<'_, T> {
        let stream = unsafe { &mut *self.0.get() };
        stream.readv(buf)
    }
}

impl<Inner> AsyncWriteRent for OwnedWriteHalf<Inner>
where
    Inner: AsyncWriteRent,
{
    type WriteFuture<'a, T> =  impl Future<Output = crate::BufResult<usize, T>>
    where
        Self: 'a,
        T: crate::buf::IoBuf + 'a;

    type WritevFuture<'a, T> =  impl Future<Output = crate::BufResult<usize, T>>
    where
        Self: 'a,
        T: crate::buf::IoVecBuf + 'a;

    type FlushFuture<'a> = impl Future<Output = std::io::Result<()>>
    where
        Self: 'a;

    type ShutdownFuture<'a> = impl Future<Output = std::io::Result<()>>
    where
        Self: 'a;

    fn write<T: crate::buf::IoBuf>(&mut self, buf: T) -> Self::WriteFuture<'_, T> {
        let stream = unsafe { &mut *self.0.get() };
        stream.write(buf)
    }

    fn writev<T: crate::buf::IoVecBuf>(&mut self, buf_vec: T) -> Self::WritevFuture<'_, T> {
        let stream = unsafe { &mut *self.0.get() };
        stream.writev(buf_vec)
    }

    fn flush(&mut self) -> Self::FlushFuture<'_> {
        let stream = unsafe { &mut *self.0.get() };
        stream.flush()
    }

    fn shutdown(&mut self) -> Self::ShutdownFuture<'_> {
        let stream = unsafe { &mut *self.0.get() };
        stream.shutdown()
    }
}

impl<T> OwnedReadHalf<T>
where
    T: AsyncWriteRent + Debug,
{
    /// reunite write half
    pub fn reunite(self, other: OwnedWriteHalf<T>) -> Result<T, ReuniteError<T>> {
        reunite(self, other)
    }
}

impl<T> OwnedWriteHalf<T>
where
    T: AsyncWriteRent + Debug,
{
    /// reunite read half
    pub fn reunite(self, other: OwnedReadHalf<T>) -> Result<T, ReuniteError<T>> {
        reunite(other, self)
    }
}

impl<T> Drop for OwnedWriteHalf<T>
where
    T: AsyncWriteRent,
{
    fn drop(&mut self) {
        let write = unsafe { &mut *self.0.get() };
        // Notes:: shutdown is an async function but rust currently does not support async drop
        // this drop will only execute sync part of `shutdown` function.
        write.shutdown();
    }
}

pub(crate) fn reunite<T: AsyncWriteRent + Debug>(
    read: OwnedReadHalf<T>,
    write: OwnedWriteHalf<T>,
) -> Result<T, ReuniteError<T>> {
    if Rc::ptr_eq(&read.0, &write.0) {
        drop(write);
        // This unwrap cannot fail as the api does not allow creating more than two
        // Arcs, and we just dropped the other half.
        Ok(Rc::try_unwrap(read.0)
            .expect("try_unwrap failed in reunite")
            .into_inner())
    } else {
        Err(ReuniteError(read, write))
    }
}

/// Error indicating that two halves were not from the same socket, and thus
/// could not be reunited.
#[derive(Debug)]
pub struct ReuniteError<T: AsyncWriteRent>(pub OwnedReadHalf<T>, pub OwnedWriteHalf<T>);

impl<T> fmt::Display for ReuniteError<T>
where
    T: AsyncWriteRent + Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tried to reunite halves")
    }
}

impl<T> Error for ReuniteError<T> where T: AsyncWriteRent + Debug {}
