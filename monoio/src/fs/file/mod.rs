use std::{future::Future, io, path::Path};

use crate::{
    buf::{IoBuf, IoBufMut, IoVecBuf, IoVecBufMut},
    driver::{op::Op, shared_fd::SharedFd},
    fs::OpenOptions,
    io::{as_fd::{AsReadFd, AsWriteFd, SharedFdWrapper}, AsyncReadRent, AsyncWriteRent},
    BufResult,
};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as file_impl;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as file_impl;

use crate::io::{AsyncReadRentAt, AsyncWriteRentAt};

/// A reference to an open file on the filesystem.
///
/// An instance of a `File` can be read and/or written depending on what options
/// it was opened with. The `File` type provides **positional** read and write
/// operations. The file does not maintain an internal cursor. The caller is
/// required to specify an offset when issuing an operation.
///
/// While files are automatically closed when they go out of scope, the
/// operation happens asynchronously in the background. It is recommended to
/// call the `close()` function in order to guarantee that the file successfully
/// closed before exiting the scope. Closing a file does not guarantee writes
/// have persisted to disk. Use [`sync_all`] to ensure all writes have reached
/// the filesystem.
///
/// [`sync_all`]: File::sync_all
///
/// # Examples
///
/// Creates a new file and write data to it:
///
/// ```no_run
/// use monoio::fs::File;
///
/// #[monoio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     // Open a file
///     let file = File::create("hello.txt").await?;
///
///     // Write some data
///     let (res, buf) = file.write_at(&b"hello world"[..], 0).await;
///     let n = res?;
///
///     println!("wrote {} bytes", n);
///
///     // Sync data to the file system.
///     file.sync_all().await?;
///
///     // Close the file
///     file.close().await?;
///
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct File {
    /// Open file descriptor
    fd: SharedFd,
}

impl File {
    /// Attempts to open a file in read-only mode.
    ///
    /// See the [`OpenOptions::open`] method for more details.
    ///
    /// # Errors
    ///
    /// This function will return an error if `path` does not already exist.
    /// Other errors may also be returned according to [`OpenOptions::open`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use monoio::fs::File;
    ///
    /// #[monoio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let f = File::open("foo.txt").await?;
    ///
    ///     // Close the file
    ///     f.close().await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn open(path: impl AsRef<Path>) -> io::Result<File> {
        OpenOptions::new().read(true).open(path).await
    }

    /// Opens a file in write-only mode.
    ///
    /// This function will create a file if it does not exist,
    /// and will truncate it if it does.
    ///
    /// See the [`OpenOptions::open`] function for more details.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use monoio::fs::File;
    ///
    /// #[monoio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let f = File::create("foo.txt").await?;
    ///
    ///     // Close the file
    ///     f.close().await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn create(path: impl AsRef<Path>) -> io::Result<File> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .await
    }

    pub(crate) fn from_shared_fd(fd: SharedFd) -> File {
        File { fd }
    }

    async fn read<T: IoBufMut>(&mut self, buf: T) -> crate::BufResult<usize, T> {
        file_impl::read(self.fd.clone(), buf).await
    }

    async fn read_vectored<T: IoVecBufMut>(&mut self, buf_vec: T) -> crate::BufResult<usize, T> {
        file_impl::read_vectored(self.fd.clone(), buf_vec).await
    }

    /// Read some bytes at the specified offset from the file into the specified
    /// buffer, returning how many bytes were read.
    ///
    /// # Return
    ///
    /// The method returns the operation result and the same buffer value passed
    /// as an argument.
    ///
    /// If the method returns [`Ok(n)`], then the read was successful. A nonzero
    /// `n` value indicates that the buffer has been filled with `n` bytes of
    /// data from the file. If `n` is `0`, then one of the following happened:
    ///
    /// 1. The specified offset is the end of the file.
    /// 2. The buffer specified was 0 bytes in length.
    ///
    /// It is not an error if the returned value `n` is smaller than the buffer
    /// size, even when the file contains enough data to fill the buffer.
    ///
    /// # Platform-specific behavior
    ///
    /// - On unix-like platform
    ///     - this function will **not** change the file pointer, and the `pos` always start from
    ///       the begin of file.
    /// - On windows
    ///     - this function will change the file pointer, but the `pos` always start from the begin
    ///       of file.
    ///
    /// Addtionally,
    ///
    /// - On Unix and Windows (without the `iouring` feature enabled or not support the `iouring`):
    ///     - If the sync feature is enabled and the thread pool is attached, this operation will be
    ///       executed on the blocking thread pool, preventing it from blocking the current thread.
    ///     - If the sync feature is enabled but the thread pool is not attached, or if the sync
    ///       feature is disabled, the operation will be executed on the local thread, blocking the
    ///       current thread.
    ///
    /// - On Linux (with iouring enabled and supported):
    ///
    ///     This operation will use io-uring to execute the task asynchronously.
    ///
    ///
    /// # Errors
    ///
    /// If this function encounters any form of I/O or other error, an error
    /// variant will be returned. The buffer is returned on error.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use monoio::fs::File;
    ///
    /// #[monoio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let f = File::open("foo.txt").await?;
    ///     let buffer = vec![0; 10];
    ///
    ///     // Read up to 10 bytes
    ///     let (res, buffer) = f.read_at(buffer, 0).await;
    ///     let n = res?;
    ///
    ///     println!("The bytes: {:?}", &buffer[..n]);
    ///
    ///     // Close the file
    ///     f.close().await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn read_at<T: IoBufMut>(&self, buf: T, pos: u64) -> crate::BufResult<usize, T> {
        file_impl::read_at(self.fd.clone(), buf, pos).await
    }

    /// Read the exact number of bytes required to fill `buf` at the specified
    /// offset from the file.
    ///
    /// This function reads as many as bytes as necessary to completely fill the
    /// specified buffer `buf`.
    ///
    /// # Return
    ///
    /// The method returns the operation result and the same buffer value passed
    /// as an argument.
    ///
    /// If the method returns [`Ok(())`], then the read was successful.
    ///
    /// # Errors
    ///
    /// If this function encounters an error of the kind
    /// [`ErrorKind::Interrupted`] then the error is ignored and the
    /// operation will continue.
    ///
    /// If this function encounters an "end of file" before completely filling
    /// the buffer, it returns an error of the kind
    /// [`ErrorKind::UnexpectedEof`]. The buffer is returned on error.
    ///
    /// If this function encounters any form of I/O or other error, an error
    /// variant will be returned. The buffer is returned on error.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use monoio::fs::File;
    ///
    /// #[monoio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let f = File::open("foo.txt").await?;
    ///     let buffer = vec![0; 10];
    ///
    ///     // Read up to 10 bytes
    ///     let (res, buffer) = f.read_exact_at(buffer, 0).await;
    ///     res?;
    ///
    ///     println!("The bytes: {:?}", buffer);
    ///
    ///     // Close the file
    ///     f.close().await?;
    ///     Ok(())
    /// }
    /// ```
    ///
    /// [`ErrorKind::Interrupted`]: std::io::ErrorKind::Interrupted
    /// [`ErrorKind::UnexpectedEof`]: std::io::ErrorKind::UnexpectedEof
    pub async fn read_exact_at<T: IoBufMut>(
        &self,
        mut buf: T,
        pos: u64,
    ) -> crate::BufResult<(), T> {
        let len = buf.bytes_total();
        let mut read = 0;
        while read < len {
            let slice = unsafe { buf.slice_mut_unchecked(read..len) };
            let (res, slice) = self.read_at(slice, pos + read as u64).await;
            buf = slice.into_inner();
            match res {
                Ok(0) => {
                    return (
                        Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "failed to fill whole buffer",
                        )),
                        buf,
                    )
                }
                Ok(n) => {
                    read += n;
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return (Err(e), buf),
            };
        }

        (Ok(()), buf)
    }

    async fn write<T: IoBuf>(&mut self, buf: T) -> crate::BufResult<usize, T> {
        file_impl::write(self.fd.clone(), buf).await
    }

    async fn write_vectored<T: IoVecBuf>(&mut self, buf_vec: T) -> crate::BufResult<usize, T> {
        file_impl::write_vectored(self.fd.clone(), buf_vec).await
    }

    /// Write a buffer into this file at the specified offset, returning how
    /// many bytes were written.
    ///
    /// This function will attempt to write the entire contents of `buf`, but
    /// the entire write may not succeed, or the write may also generate an
    /// error. The bytes will be written starting at the specified offset.
    ///
    /// # Return
    ///
    /// The method returns the operation result and the same buffer value passed
    /// in as an argument. A return value of `0` typically means that the
    /// underlying file is no longer able to accept bytes and will likely not be
    /// able to in the future as well, or that the buffer provided is empty.
    ///
    /// # Platform-specific behavior
    ///
    /// - On Unix and Windows (without the `iouring` feature enabled or not support the `iouring`):
    ///     - If the sync feature is enabled and the thread pool is attached, this operation will be
    ///       executed on the blocking thread pool, preventing it from blocking the current thread.
    ///     - If the sync feature is enabled but the thread pool is not attached, or if the sync
    ///       feature is disabled, the operation will be executed on the local thread, blocking the
    ///       current thread.
    ///
    /// - On Linux (with iouring enabled and supported):
    ///
    ///     This operation will use io-uring to execute the task asynchronously.
    ///
    /// # Errors
    ///
    /// Each call to `write` may generate an I/O error indicating that the
    /// operation could not be completed. If an error is returned then no bytes
    /// in the buffer were written to this writer.
    ///
    /// It is **not** considered an error if the entire buffer could not be
    /// written to this writer.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use monoio::fs::File;
    ///
    /// #[monoio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let file = File::create("foo.txt").await?;
    ///
    ///     // Writes some prefix of the byte string, not necessarily all of it.
    ///     let (res, _) = file.write_at(&b"some bytes"[..], 0).await;
    ///     let n = res?;
    ///
    ///     println!("wrote {} bytes", n);
    ///
    ///     // Close the file
    ///     file.close().await?;
    ///     Ok(())
    /// }
    /// ```
    ///
    /// [`Ok(n)`]: Ok
    pub async fn write_at<T: IoBuf>(&self, buf: T, pos: u64) -> crate::BufResult<usize, T> {
        file_impl::write_at(self.fd.clone(), buf, pos).await
    }

    /// Attempts to write an entire buffer into this file at the specified
    /// offset.
    ///
    /// This method will continuously call [`write_at`] until there is no more
    /// data to be written or an error of non-[`ErrorKind::Interrupted`]
    /// kind is returned. This method will not return until the entire
    /// buffer has been successfully written or such an error occurs.
    ///
    /// If the buffer contains no data, this will never call [`write_at`].
    ///
    /// # Return
    ///
    /// The method returns the operation result and the same buffer value passed
    /// in as an argument.
    ///
    /// # Errors
    ///
    /// This function will return the first error of
    /// non-[`ErrorKind::Interrupted`] kind that [`write_at`] returns.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use monoio::fs::File;
    ///
    /// #[monoio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let file = File::create("foo.txt").await?;
    ///
    ///     // Writes some prefix of the byte string, not necessarily all of it.
    ///     let (res, _) = file.write_all_at(&b"some bytes"[..], 0).await;
    ///     res?;
    ///
    ///     println!("wrote all bytes");
    ///
    ///     // Close the file
    ///     file.close().await?;
    ///     Ok(())
    /// }
    /// ```
    ///
    /// [`write_at`]: File::write_at
    /// [`ErrorKind::Interrupted`]: std::io::ErrorKind::Interrupted
    pub async fn write_all_at<T: IoBuf>(&self, mut buf: T, pos: u64) -> crate::BufResult<(), T> {
        let len = buf.bytes_init();
        let mut written = 0;
        while written < len {
            let slice = unsafe { buf.slice_unchecked(written..len) };
            let (res, slice) = self.write_at(slice, pos + written as u64).await;
            buf = slice.into_inner();
            match res {
                Ok(0) => {
                    return (
                        Err(io::Error::new(
                            io::ErrorKind::WriteZero,
                            "failed to write whole buffer",
                        )),
                        buf,
                    )
                }
                Ok(n) => written += n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return (Err(e), buf),
            };
        }

        (Ok(()), buf)
    }

    /// Attempts to sync all OS-internal metadata to disk.
    ///
    /// This function will attempt to ensure that all in-memory data reaches the
    /// filesystem before completing.
    ///
    /// This can be used to handle errors that would otherwise only be caught
    /// when the `File` is closed.  Dropping a file will ignore errors in
    /// synchronizing this in-memory data.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use monoio::fs::File;
    ///
    /// #[monoio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let f = File::create("foo.txt").await?;
    ///     let (res, buf) = f.write_at(&b"Hello, world!"[..], 0).await;
    ///     let n = res?;
    ///
    ///     f.sync_all().await?;
    ///
    ///     // Close the file
    ///     f.close().await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn sync_all(&self) -> io::Result<()> {
        let op = Op::fsync(&self.fd).unwrap();
        let completion = op.await;

        completion.meta.result?;
        Ok(())
    }

    /// Attempts to sync file data to disk.
    ///
    /// This method is similar to [`sync_all`], except that it may not
    /// synchronize file metadata to the filesystem.
    ///
    /// This is intended for use cases that must synchronize content, but don't
    /// need the metadata on disk. The goal of this method is to reduce disk
    /// operations.
    ///
    /// Note that some platforms may simply implement this in terms of
    /// [`sync_all`].
    ///
    /// [`sync_all`]: File::sync_all
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use monoio::fs::File;
    ///
    /// #[monoio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let f = File::create("foo.txt").await?;
    ///     let (res, buf) = f.write_at(&b"Hello, world!"[..], 0).await;
    ///     let n = res?;
    ///
    ///     f.sync_data().await?;
    ///
    ///     // Close the file
    ///     f.close().await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn sync_data(&self) -> io::Result<()> {
        let op = Op::datasync(&self.fd).unwrap();
        let completion = op.await;

        completion.meta.result?;
        Ok(())
    }

    #[inline]
    fn flush(&mut self) -> impl Future<Output = io::Result<()>> {
        std::future::ready(Ok(()))
    }

    /// Closes the file.
    ///
    /// The method completes once the close operation has completed,
    /// guaranteeing that resources associated with the file have been released.
    ///
    /// If `close` is not called before dropping the file, the file is closed in
    /// the background, but there is no guarantee as to **when** the close
    /// operation will complete.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use monoio::fs::File;
    ///
    /// #[monoio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     // Open the file
    ///     let f = File::open("foo.txt").await?;
    ///     // Close the file
    ///     f.close().await?;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub async fn close(self) -> io::Result<()> {
        self.fd.close().await;
        Ok(())
    }
}

impl AsyncWriteRent for File {
    /// Writes the contents of a buffer to a file, returning the number of bytes written.
    ///
    /// This function attempts to write the entire buffer `buf`, but the write may not fully
    /// succeed, and it might also result in an error. A call to `write` represents *at most one*
    /// attempt to write to the underlying object.
    ///
    /// # Return
    ///
    /// If the return value is `(Ok(n), buf)`, it guarantees that `n <= buf.len()`. A return value
    /// of `0` typically indicates that the underlying object can no longer accept bytes and likely
    /// won't be able to in the future, or that the provided buffer is empty.
    ///
    /// # Errors
    ///
    /// Each `write` call may result in an I/O error, indicating that the operation couldn't be
    /// completed. If an error occurs, no bytes from the buffer were written to the file.
    ///
    /// It is **not** considered an error if the entire buffer could not be written to the file.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use monoio::io::AsyncWriteRent;
    ///
    /// #[monoio::main]
    /// async fn main() -> std::io::Result<()> {
    ///     let mut file = monoio::fs::File::create("example.txt").await?;
    ///     let (res, buf) = file.write("Hello, world").await;
    ///     res?;
    ///     Ok(())
    /// }
    /// ```
    async fn write<T: IoBuf>(&mut self, buf: T) -> crate::BufResult<usize, T> {
        self.write(buf).await
    }

    /// This function attempts to write the entire contents of `buf_vec`, but the write may not
    /// fully succeed, and it might also result in an error. The bytes will be written starting at
    /// the current file pointer.
    ///
    /// # Return
    ///
    /// The method returns the result of the operation along with the same array of buffers passed
    /// as an argument. A return value of `0` typically indicates that the underlying file can no
    /// longer accept bytes and likely won't be able to in the future, or that the provided buffer
    /// is empty.
    ///
    /// # Platform-specific behavior
    ///
    /// - On windows
    ///     - due to windows does not have syscall like `writev`, so the implement of this function
    ///       on windows is by internally calling the `WriteFile` syscall to write each buffer into
    ///       file.
    ///
    /// # Errors
    ///
    /// Each `write` call may result in an I/O error, indicating the operation couldn't be
    /// completed. If an error occurs, no bytes from the buffer were written to the file.
    ///
    /// It is **not** considered an error if the entire buffer could not be written to the file.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use monoio::io::AsyncWriteRent;
    ///
    /// #[monoio::main]
    /// async fn main() -> std::io::Result<()> {
    ///     let buf_vec = monoio::buf::VecBuf::from(vec![
    ///         "Hello".to_owned().into_bytes(),
    ///         "World".to_owned().into_bytes(),
    ///     ]);
    ///     let mut file = monoio::fs::File::create("example.txt").await?;
    ///     let (res, buf_vec) = file.writev(buf_vec).await;
    ///     res?;
    ///     Ok(())
    /// }
    /// ```
    async fn writev<T: crate::buf::IoVecBuf>(&mut self, buf_vec: T) -> crate::BufResult<usize, T> {
        self.write_vectored(buf_vec).await
    }

    /// Flushes the file, ensuring that all buffered contents are written to their destination.
    ///
    /// # Platform-specific behavior
    ///
    /// Since the `File` structure doesn't contain any internal buffers, this function is currently
    /// a no-op.
    async fn flush(&mut self) -> std::io::Result<()> {
        self.flush().await
    }

    /// This function will call [`flush`] inside.
    async fn shutdown(&mut self) -> std::io::Result<()> {
        self.flush().await
    }
}

impl AsyncWriteRentAt for File {
    fn write_at<T: IoBuf>(
        &mut self,
        buf: T,
        pos: usize,
    ) -> impl Future<Output = BufResult<usize, T>> {
        File::write_at(self, buf, pos as u64)
    }
}

impl AsyncReadRent for File {
    /// Reads bytes from the file at the current file pointer into the specified buffer, returning
    /// the number of bytes read.
    ///
    /// # Return
    ///
    /// The method returns a tuple with the result of the operation and the same buffer passed as an
    /// argument.
    ///
    /// If the method returns [`(Ok(n), buf)`], a non-zero `n` means the buffer has been filled with
    /// `n` bytes of data from the file. If `n` is `0`, it indicates one of the following:
    ///
    /// 1. The current file pointer is at the end of the file.
    /// 2. The provided buffer was 0 bytes in length.
    ///
    /// It is not an error if `n` is smaller than the buffer size, even if there is enough data in
    /// the file to fill the buffer.
    ///
    /// # Platform-specific behavior
    ///
    /// - On Unix and Windows (without the `iouring` feature enabled or not support the `iouring`):
    ///     - If the sync feature is enabled and the thread pool is attached, this operation will be
    ///       executed on the blocking thread pool, preventing it from blocking the current thread.
    ///     - If the sync feature is enabled but the thread pool is not attached, or if the sync
    ///       feature is disabled, the operation will be executed on the local thread, blocking the
    ///       current thread.
    ///
    /// - On Linux (with iouring enabled and supported):
    ///
    ///     This operation will use io-uring to execute the task asynchronously.
    ///
    /// # Errors
    ///
    /// If an I/O or other error occurs, an error variant will be returned, and the buffer will also
    /// be returned.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use monoio::io::AsyncReadRent;
    ///
    /// #[monoio::main]
    /// async fn main() -> std::io::Result<()> {
    ///     let buf = Vec::with_capacity(1024);
    ///     let mut file = monoio::fs::File::open("example.txt").await?;
    ///     let (res, buf) = file.read(buf).await;
    ///     println!("bytes read: {}", res?);
    ///     Ok(())
    /// }
    /// ```
    async fn read<T: IoBufMut>(&mut self, buf: T) -> crate::BufResult<usize, T> {
        self.read(buf).await
    }

    /// Read some bytes at the specified offset from the file into the specified
    /// array of buffers, returning how many bytes were read.
    ///
    /// # Return
    ///
    /// The method returns the operation result and the same array of buffers
    /// passed as an argument.
    ///
    /// If the method returns [`Ok(n)`], then the read was successful. A nonzero
    /// `n` value indicates that the buffers have been filled with `n` bytes of
    /// data from the file. If `n` is `0`, then one of the following happened:
    ///
    /// 1. The specified offset is the end of the file.
    /// 2. The buffers specified were 0 bytes in length.
    ///
    /// It is not an error if the returned value `n` is smaller than the buffer
    /// size, even when the file contains enough data to fill the buffer.
    ///
    /// # Platform-specific behavior
    ///
    /// - On windows
    ///     - due to windows does not have syscall like `readv`, so the implement of this function
    ///       on windows is by internally calling the `ReadFile` syscall to fill each buffer.
    ///
    /// - On Unix and Windows (without the `iouring` feature enabled or not support the `iouring`):
    ///     - If the sync feature is enabled and the thread pool is attached, this operation will be
    ///       executed on the blocking thread pool, preventing it from blocking the current thread.
    ///     - If the sync feature is enabled but the thread pool is not attached, or if the sync
    ///       feature is disabled, the operation will be executed on the local thread, blocking the
    ///       current thread.
    ///
    /// - On Linux (with iouring enabled and supported):
    ///
    ///     This operation will use io-uring to execute the task asynchronously.
    ///
    ///
    /// # Errors
    ///
    /// If this function encounters any form of I/O or other error, an error
    /// variant will be returned. The buffer is returned on error.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use monoio::io::AsyncReadRent;
    ///
    /// #[monoio::main]
    /// async fn main() -> std::io::Result<()> {
    ///     let mut file = monoio::fs::File::open("example.txt").await?;
    ///     let buffers = monoio::buf::VecBuf::from(vec![
    ///         Vec::<u8>::with_capacity(10),
    ///         Vec::<u8>::with_capacity(10),
    ///     ]);
    ///
    ///     let (res, buffer) = file.readv(buffers).await;
    ///
    ///     println!("bytes read: {}", res?);
    ///     Ok(())
    /// }
    /// ```
    async fn readv<T: crate::buf::IoVecBufMut>(&mut self, buf: T) -> crate::BufResult<usize, T> {
        self.read_vectored(buf).await
    }
}

impl AsyncReadRentAt for File {
    fn read_at<T: IoBufMut>(
        &mut self,
        buf: T,
        pos: usize,
    ) -> impl Future<Output = BufResult<usize, T>> {
        File::read_at(self, buf, pos as u64)
    }
}

impl AsReadFd for File {
    #[inline]
    fn as_reader_fd(&mut self) -> &SharedFdWrapper {
        SharedFdWrapper::new(&self.fd)
    }
}

impl AsWriteFd for File {
    #[inline]
    fn as_writer_fd(&mut self) -> &SharedFdWrapper {
        SharedFdWrapper::new(&self.fd)
    }
}
