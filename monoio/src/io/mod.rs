//! IO traits

mod async_buf_read;
mod async_buf_read_ext;
mod async_read_rent;
mod async_read_rent_ext;
mod async_rent_cancelable;
mod async_rent_cancelable_ext;
mod async_write_rent;
mod async_write_rent_ext;

pub mod sink;
pub mod stream;

pub mod as_fd;
#[cfg(all(target_os = "linux", feature = "splice"))]
pub mod splice;

pub use async_buf_read::AsyncBufRead;
pub use async_buf_read_ext::AsyncBufReadExt;
pub use async_read_rent::{AsyncReadRent, AsyncReadRentAt};
pub use async_read_rent_ext::AsyncReadRentExt;
pub use async_rent_cancelable::{CancelableAsyncReadRent, CancelableAsyncWriteRent};
pub use async_rent_cancelable_ext::{CancelableAsyncReadRentExt, CancelableAsyncWriteRentExt};
pub use async_write_rent::{AsyncWriteRent, AsyncWriteRentAt};
pub use async_write_rent_ext::AsyncWriteRentExt;

mod util;

#[cfg(feature = "poll-io")]
pub use tokio::io as poll_io;
pub(crate) use util::operation_canceled;
pub use util::{
    copy, BufReader, BufWriter, CancelHandle, Canceller, OwnedReadHalf, OwnedWriteHalf,
    PrefixedReadIo, Split, Splitable,
};
#[cfg(all(target_os = "linux", feature = "splice"))]
pub use util::{zero_copy, zero_copy_from_pipe, zero_copy_to_pipe};
#[cfg(feature = "poll-io")]
/// Convert a completion-based io to a poll-based io.
pub trait IntoPollIo: Sized {
    /// The poll-based io type.
    type PollIo;

    /// Convert a completion-based io to a poll-based io(able to get comp_io back).
    fn try_into_poll_io(self) -> Result<Self::PollIo, (std::io::Error, Self)>;

    /// Convert a completion-based io to a poll-based io.
    #[inline]
    fn into_poll_io(self) -> std::io::Result<Self::PollIo> {
        self.try_into_poll_io().map_err(|(e, _)| e)
    }
}

#[cfg(feature = "poll-io")]
/// Convert a poll-based io to a completion-based io.
pub trait IntoCompIo: Sized {
    /// The completion-based io type.
    type CompIo;

    /// Convert a poll-based io to a completion-based io(able to get poll_io back).
    fn try_into_comp_io(self) -> Result<Self::CompIo, (std::io::Error, Self)>;

    /// Convert a poll-based io to a completion-based io.
    #[inline]
    fn into_comp_io(self) -> std::io::Result<Self::CompIo> {
        self.try_into_comp_io().map_err(|(e, _)| e)
    }
}
