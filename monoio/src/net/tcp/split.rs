use std::{future::Future, io, net::SocketAddr};

use super::TcpStream;
use crate::{
    buf::{IoBuf, IoBufMut, IoVecBuf, IoVecBufMut},
    io::{
        as_fd::{AsReadFd, AsWriteFd, SharedFdWrapper},
        AsyncReadRent, AsyncWriteRent, OwnedReadHalf, OwnedWriteHalf, ReadHalf, WriteHalf,
    },
};

/// ReadHalf.
pub type TcpReadHalf<'a> = ReadHalf<'a, TcpStream>;
/// WriteHalf
pub type TcpWriteHalf<'a> = WriteHalf<'a, TcpStream>;

#[allow(clippy::cast_ref_to_mut)]
impl<'t> AsReadFd for TcpReadHalf<'t> {
    fn as_reader_fd(&mut self) -> &SharedFdWrapper {
        let raw_stream = unsafe { &mut *(self.0 as *const TcpStream as *mut TcpStream) };
        raw_stream.as_reader_fd()
    }
}

#[allow(clippy::cast_ref_to_mut)]
impl<'t> AsyncReadRent for TcpReadHalf<'t> {
    type ReadFuture<'a, B> = impl std::future::Future<Output = crate::BufResult<usize, B>> where
        't: 'a, B: IoBufMut + 'a;
    type ReadvFuture<'a, B> = impl std::future::Future<Output = crate::BufResult<usize, B>> where
        't: 'a, B: IoVecBufMut + 'a,;

    fn read<T: IoBufMut>(&mut self, buf: T) -> Self::ReadFuture<'_, T> {
        // Submit the read operation
        let raw_stream = unsafe { &mut *(self.0 as *const TcpStream as *mut TcpStream) };
        raw_stream.read(buf)
    }

    fn readv<T: IoVecBufMut>(&mut self, buf: T) -> Self::ReadvFuture<'_, T> {
        // Submit the read operation
        let raw_stream = unsafe { &mut *(self.0 as *const TcpStream as *mut TcpStream) };
        raw_stream.readv(buf)
    }
}

#[allow(clippy::cast_ref_to_mut)]
impl<'t> AsWriteFd for TcpWriteHalf<'t> {
    fn as_writer_fd(&mut self) -> &SharedFdWrapper {
        let raw_stream = unsafe { &mut *(self.0 as *const TcpStream as *mut TcpStream) };
        raw_stream.as_writer_fd()
    }
}

#[allow(clippy::cast_ref_to_mut)]
impl<'t> AsyncWriteRent for TcpWriteHalf<'t> {
    type WriteFuture<'a, B> = impl Future<Output = crate::BufResult<usize, B>> where
        't: 'a, B: IoBuf + 'a;
    type WritevFuture<'a, B> = impl Future<Output = crate::BufResult<usize, B>> where
        't: 'a, B: IoVecBuf + 'a;
    type FlushFuture<'a> = impl Future<Output = io::Result<()>> where
        't: 'a;
    type ShutdownFuture<'a> = impl Future<Output = io::Result<()>> where
        't: 'a;

    fn write<T: IoBuf>(&mut self, buf: T) -> Self::WriteFuture<'_, T> {
        // Submit the write operation
        let raw_stream = unsafe { &mut *(self.0 as *const TcpStream as *mut TcpStream) };
        raw_stream.write(buf)
    }

    fn writev<T: IoVecBuf>(&mut self, buf_vec: T) -> Self::WritevFuture<'_, T> {
        let raw_stream = unsafe { &mut *(self.0 as *const TcpStream as *mut TcpStream) };
        raw_stream.writev(buf_vec)
    }

    fn flush(&mut self) -> Self::FlushFuture<'_> {
        // Tcp stream does not need flush.
        async move { Ok(()) }
    }

    fn shutdown(&mut self) -> Self::ShutdownFuture<'_> {
        let raw_stream = unsafe { &mut *(self.0 as *const TcpStream as *mut TcpStream) };
        raw_stream.shutdown()
    }
}

/// OwnedReadHalf.
pub type TcpOwnedReadHalf = OwnedReadHalf<TcpStream>;
/// OwnedWriteHalf
pub type TcpOwnedWriteHalf = OwnedWriteHalf<TcpStream>;

// impl Error for ReuniteError{}

impl TcpOwnedReadHalf {
    /// Returns the remote address that this stream is connected to.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        unsafe { &*self.0.get() }.peer_addr()
    }

    /// Returns the local address that this stream is bound to.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        unsafe { &*self.0.get() }.local_addr()
    }
}

impl AsReadFd for TcpOwnedReadHalf {
    fn as_reader_fd(&mut self) -> &SharedFdWrapper {
        let raw_stream = unsafe { &mut *self.0.get() };
        raw_stream.as_reader_fd()
    }
}

// impl AsyncReadRent for OwnedReadHalf {
//     type ReadFuture<'a, B> = impl std::future::Future<Output = crate::BufResult<usize, B>> where
//         B: IoBufMut + 'a;
//     type ReadvFuture<'a, B> = impl std::future::Future<Output = crate::BufResult<usize, B>> where
//         B: IoVecBufMut + 'a;

//     fn read<T: IoBufMut>(&mut self, buf: T) -> Self::ReadFuture<'_, T> {
//         // Submit the read operation
//         let raw_stream = unsafe { &mut *self.0.get() };
//         raw_stream.read(buf)
//     }

//     fn readv<T: IoVecBufMut>(&mut self, buf: T) -> Self::ReadvFuture<'_, T> {
//         // Submit the read operation
//         let raw_stream = unsafe { &mut *self.0.get() };
//         raw_stream.readv(buf)
//     }
// }

impl TcpOwnedWriteHalf {
    /// Returns the remote address that this stream is connected to.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        unsafe { &*self.0.get() }.peer_addr()
    }

    /// Returns the local address that this stream is bound to.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        unsafe { &*self.0.get() }.local_addr()
    }
}

impl AsWriteFd for TcpOwnedWriteHalf {
    fn as_writer_fd(&mut self) -> &SharedFdWrapper {
        let raw_stream = unsafe { &mut *self.0.get() };
        raw_stream.as_writer_fd()
    }
}
