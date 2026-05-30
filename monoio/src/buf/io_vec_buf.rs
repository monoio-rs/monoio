// use super::shared_buf::Shared;

#[cfg(windows)]
use windows_sys::Win32::Networking::WinSock::WSABUF;

/// An `io_uring` compatible iovec buffer.
///
/// # Safety
/// See the safety note of the methods.
#[allow(clippy::unnecessary_safety_doc)]
pub unsafe trait IoVecBuf: Unpin + 'static {
    /// Returns a raw pointer to iovec struct.
    /// struct iovec {
    ///     void  *iov_base;    /* Starting address */
    ///     size_t iov_len;     /* Number of bytes to transfer */
    /// };
    /// \[iovec1\]\[iovec2\]\[iovec3\]...
    /// ^ The pointer
    ///
    /// # Safety
    /// The implementation must ensure that, while the runtime owns the value,
    /// the pointer returned by `stable_mut_ptr` **does not** change.
    /// Also, the value pointed must be a valid iovec struct.
    #[cfg(unix)]
    fn read_iovec_ptr(&self) -> *const libc::iovec;

    /// Returns the count of iovec struct behind the pointer.
    ///
    /// # Safety
    /// There must be really that number of iovec here.
    #[cfg(unix)]
    fn read_iovec_len(&self) -> usize;

    /// Returns a raw pointer to WSABUF struct.
    #[cfg(windows)]
    fn read_wsabuf_ptr(&self) -> *const WSABUF;

    /// Returns the count of WSABUF struct behind the pointer.
    #[cfg(windows)]
    fn read_wsabuf_len(&self) -> usize;
}

/// A intermediate struct that impl IoVecBuf and IoVecBufMut.
#[derive(Clone)]
pub struct VecBuf {
    #[cfg(unix)]
    iovecs: Vec<libc::iovec>,
    #[cfg(windows)]
    wsabufs: Vec<WSABUF>,
    raw: Vec<Vec<u8>>,
}

#[cfg(unix)]
unsafe impl IoVecBuf for VecBuf {
    fn read_iovec_ptr(&self) -> *const libc::iovec {
        self.iovecs.read_iovec_ptr()
    }
    fn read_iovec_len(&self) -> usize {
        self.iovecs.read_iovec_len()
    }
}

#[cfg(unix)]
unsafe impl IoVecBuf for Vec<libc::iovec> {
    fn read_iovec_ptr(&self) -> *const libc::iovec {
        self.as_ptr()
    }

    fn read_iovec_len(&self) -> usize {
        self.len()
    }
}

#[cfg(windows)]
unsafe impl IoVecBuf for VecBuf {
    fn read_wsabuf_ptr(&self) -> *const WSABUF {
        self.wsabufs.read_wsabuf_ptr()
    }
    fn read_wsabuf_len(&self) -> usize {
        self.wsabufs.read_wsabuf_len()
    }
}

#[cfg(windows)]
unsafe impl IoVecBuf for Vec<WSABUF> {
    fn read_wsabuf_ptr(&self) -> *const WSABUF {
        self.as_ptr()
    }

    fn read_wsabuf_len(&self) -> usize {
        self.len()
    }
}

impl From<Vec<Vec<u8>>> for VecBuf {
    fn from(vs: Vec<Vec<u8>>) -> Self {
        #[cfg(unix)]
        {
            let iovecs = vs
                .iter()
                .map(|v| libc::iovec {
                    iov_base: v.as_ptr() as _,
                    iov_len: v.len(),
                })
                .collect();
            Self { iovecs, raw: vs }
        }
        #[cfg(windows)]
        {
            let wsabufs = vs
                .iter()
                .map(|v| WSABUF {
                    buf: v.as_ptr() as _,
                    len: v.len() as _,
                })
                .collect();
            Self { wsabufs, raw: vs }
        }
    }
}

impl From<VecBuf> for Vec<Vec<u8>> {
    fn from(vb: VecBuf) -> Self {
        vb.raw
    }
}

// /// SliceVec impl IoVecBuf and IoVecBufMut.
// pub struct SliceVec<T> {
//     iovecs: Vec<libc::iovec>,
//     indices: Vec<(usize, usize)>,
//     buf: T,
// }

// impl<T> SliceVec<T> {
//     /// New SliceVec.
//     pub fn new(buf: T) -> Self {
//         Self {
//             iovecs: Default::default(),
//             indices: Default::default(),
//             buf,
//         }
//     }

//     /// New SliceVec with given indices.
//     pub fn new_with_indices(buf: T, indices: Vec<(usize, usize)>) -> Self {
//         Self {
//             iovecs: Default::default(),
//             indices,
//             buf,
//         }
//     }
// }

// unsafe impl<T> IoVecBuf for SliceVec<T>
// where
//     T: Shared,
// {
//     fn stable_iovec_ptr(&self) -> *const libc::iovec {
//         self.iovecs.as_ptr()

//         // self.iovecs.clear();
//         // self.iovecs.reserve(self.indices.len());
//         // let base = self.buf.stable_ptr();
//         // for (begin, end) in self.indices.iter() {
//         //     self.iovecs.push(libc::iovec {
//         //         iov_base: unsafe { base.add(*begin) as *mut libc::c_void
// },         //         iov_len: end - begin,
//         //     });
//         // }
//         // self.iovecs.as_ptr()
//     }

//     fn iovec_len(&self) -> usize {
//         self.indices.len()
//     }
// }

// impl<T> SliceVec<T> where T: Shared {
//     pub fn write_all(&mut self, data: &[u8]) -> Result<(), std::io::Error> {
//         unimplemented!()
//     }
// }

/// A mutable `io_uring` compatible iovec buffer.
///
/// # Safety
/// See the safety note of the methods.
#[allow(clippy::unnecessary_safety_doc)]
pub unsafe trait IoVecBufMut: Unpin + 'static {
    /// Returns a raw mutable pointer to iovec struct.
    /// struct iovec {
    ///     void  *iov_base;    /* Starting address */
    ///     size_t iov_len;     /* Number of bytes to transfer */
    /// };
    /// \[iovec1\]\[iovec2\]\[iovec3\]...
    /// ^ The pointer
    ///
    /// # Safety
    /// The implementation must ensure that, while the runtime owns the value,
    /// the pointer returned by `write_iovec_ptr` **does not** change.
    /// Also, the value pointed must be a valid iovec struct.
    #[cfg(unix)]
    fn write_iovec_ptr(&mut self) -> *mut libc::iovec;

    /// Returns the count of iovec struct behind the pointer.
    #[cfg(unix)]
    fn write_iovec_len(&mut self) -> usize;

    /// Returns a raw mutable pointer to WSABUF struct.
    #[cfg(windows)]
    fn write_wsabuf_ptr(&mut self) -> *mut WSABUF;

    /// Returns the count of WSABUF struct behind the pointer.
    #[cfg(windows)]
    fn write_wsabuf_len(&mut self) -> usize;

    /// Updates the number of initialized bytes.
    ///
    /// The specified `pos` becomes the new value returned by
    /// `IoBuf::bytes_init`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that there are really pos data initialized.
    unsafe fn set_init(&mut self, pos: usize);
}

#[cfg(unix)]
unsafe impl IoVecBufMut for VecBuf {
    fn write_iovec_ptr(&mut self) -> *mut libc::iovec {
        self.read_iovec_ptr() as *mut _
    }

    fn write_iovec_len(&mut self) -> usize {
        self.read_iovec_len()
    }

    unsafe fn set_init(&mut self, mut len: usize) { unsafe {
        for (idx, iovec) in self.iovecs.iter_mut().enumerate() {
            if iovec.iov_len <= len {
                // set_init all
                self.raw[idx].set_len(iovec.iov_len);
                len -= iovec.iov_len;
            } else {
                if len > 0 {
                    self.raw[idx].set_len(len);
                }
                break;
            }
        }
    }}
}

#[cfg(windows)]
unsafe impl IoVecBufMut for VecBuf {
    fn write_wsabuf_ptr(&mut self) -> *mut WSABUF {
        self.read_wsabuf_ptr() as *mut _
    }

    fn write_wsabuf_len(&mut self) -> usize {
        self.read_wsabuf_len()
    }

    unsafe fn set_init(&mut self, mut len: usize) {
        for (idx, wsabuf) in self.wsabufs.iter_mut().enumerate() {
            if wsabuf.len as usize <= len {
                // set_init all
                self.raw[idx].set_len(wsabuf.len as _);
                len -= wsabuf.len as usize;
            } else {
                if len > 0 {
                    self.raw[idx].set_len(len);
                }
                break;
            }
        }
    }
}
