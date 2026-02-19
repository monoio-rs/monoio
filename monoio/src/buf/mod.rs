//! Utilities for working with buffers.
//!
//! `io_uring` APIs require passing ownership of buffers to the runtime. The
//! crate defines [`IoBuf`] and [`IoBufMut`] traits which are implemented by
//! buffer types that respect the `io_uring` contract.
// Heavily borrowed from tokio-uring.
// Copyright (c) 2021 Tokio-uring Contributors, licensed under the MIT license.

mod io_buf;
pub use io_buf::{IoBuf, IoBufMut};

mod io_vec_buf;
pub use io_vec_buf::{IoVecBuf, IoVecBufMut, VecBuf};

mod slice;
pub use slice::{IoVecWrapper, IoVecWrapperMut, Slice, SliceMut};

mod raw_buf;
pub use raw_buf::{RawBuf, RawBufVectored};

mod vec_wrapper;
pub(crate) use vec_wrapper::{read_vec_meta, write_vec_meta, IoVecMeta};

mod msg;
pub use msg::{MsgBuf, MsgBufMut, MsgMeta};

#[cfg(all(target_os = "linux", feature = "iouring"))]
mod ring_buf;
#[cfg(all(target_os = "linux", feature = "iouring"))]
pub use ring_buf::{
    AnyRecvMsgParser, Ipv4RecvMsgParser, Ipv6RecvMsgParser, RawBuffer, RecvMsgParser,
    RecvMsgRingBuf, RingBuf, UserRecvMsgRingBuf, UserRingBuf, UserRingBufBuilder,
};

pub(crate) fn deref(buf: &impl IoBuf) -> &[u8] {
    // Safety: the `IoBuf` trait is marked as unsafe and is expected to be
    // implemented correctly.
    unsafe { std::slice::from_raw_parts(buf.read_ptr(), buf.bytes_init()) }
}
