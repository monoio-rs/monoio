use std::{
    fs::File as StdFile,
    io,
    os::fd::{AsRawFd, IntoRawFd, RawFd},
};

#[cfg(all(not(feature = "iouring"), feature = "sync"))]
pub(crate) use asyncified::*;
#[cfg(any(feature = "iouring", not(feature = "sync")))]
pub(crate) use iouring::*;

use super::File;
use crate::{
    buf::{IoBuf, IoBufMut, IoVecBuf, IoVecBufMut},
    driver::{op::Op, shared_fd::SharedFd},
    fs::{metadata::FileAttr, Metadata},
    io::as_fd::{AsReadFd, AsWriteFd, SharedFdWrapper},
};

impl File {
    /// Converts a [`std::fs::File`] to a [`monoio::fs::File`](File).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// // This line could block. It is not recommended to do this on the monoio
    /// // runtime.
    /// let std_file = std::fs::File::open("foo.txt").unwrap();
    /// let file = monoio::fs::File::from_std(std_file);
    /// ```
    pub fn from_std(std: StdFile) -> std::io::Result<File> {
        Ok(File {
            fd: SharedFd::new_without_register(std.into_raw_fd()),
        })
    }

    /// Queries metadata about the underlying file.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use monoio::fs::File;
    ///
    /// #[monoio::main]
    /// async fn main() -> std::io::Result<()> {
    ///     let mut f = File::open("foo.txt").await?;
    ///     let metadata = f.metadata().await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn metadata(&self) -> io::Result<Metadata> {
        metadata(self.fd.clone()).await
    }
}

impl AsRawFd for File {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.raw_fd()
    }
}

impl AsReadFd for File {
    fn as_reader_fd(&mut self) -> &crate::io::as_fd::SharedFdWrapper {
        SharedFdWrapper::new(&self.fd)
    }
}

impl AsWriteFd for File {
    fn as_writer_fd(&mut self) -> &SharedFdWrapper {
        SharedFdWrapper::new(&self.fd)
    }
}

pub(crate) async fn metadata(fd: SharedFd) -> std::io::Result<Metadata> {
    #[cfg(target_os = "linux")]
    let flags = libc::AT_STATX_SYNC_AS_STAT | libc::AT_EMPTY_PATH;
    #[cfg(target_os = "linux")]
    let op = Op::statx_using_fd(fd, flags)?;
    #[cfg(target_os = "macos")]
    let op = Op::statx_using_fd(fd, true)?;

    op.result().await.map(FileAttr::from).map(Metadata)
}

#[cfg(any(feature = "iouring", not(feature = "sync")))]
mod iouring {
    use super::*;
    use crate::uring_op;

    uring_op!(read<IoBufMut>(read, buf));
    uring_op!(read_at<IoBufMut>(read_at, buf, pos: u64));
    uring_op!(read_vectored<IoVecBufMut>(readv, buf_vec));

    uring_op!(write<IoBuf>(write, buf));
    uring_op!(write_at<IoBuf>(write_at, buf, pos: u64));
    uring_op!(write_vectored<IoVecBuf>(writev, buf_vec));
}

#[cfg(all(not(feature = "iouring"), feature = "sync"))]
mod asyncified {
    use super::*;
    use crate::{
        asyncify_op,
        driver::op::{read, write},
    };

    asyncify_op!(R, read<IoBufMut>(read::read, IoBufMut::write_ptr, IoBufMut::bytes_total));
    asyncify_op!(R, read_at<IoBufMut>(read::read_at, IoBufMut::write_ptr, IoBufMut::bytes_total, pos: u64));
    asyncify_op!(R, read_vectored<IoVecBufMut>(read::read_vectored, IoVecBufMut::write_iovec_ptr, IoVecBufMut::write_iovec_len));

    asyncify_op!(W, write<IoBuf>(write::write, IoBuf::read_ptr, IoBuf::bytes_init));
    asyncify_op!(W, write_at<IoBuf>(write::write_at, IoBuf::read_ptr, IoBuf::bytes_init, pos: u64));
    asyncify_op!(W, write_vectored<IoVecBuf>(write::write_vectored, IoVecBuf::read_iovec_ptr, IoVecBuf::read_iovec_len));
}
