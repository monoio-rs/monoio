//! IO utils

mod buf_reader;
mod buf_writer;
mod cancel;
mod copy;
mod prefixed_io;
mod split;

pub use buf_reader::BufReader;
pub use buf_writer::BufWriter;
pub(crate) use cancel::operation_canceled;
pub use cancel::{CancelHandle, Canceller};
pub use copy::copy;
#[cfg(all(target_os = "linux", feature = "splice"))]
pub use copy::{zero_copy, zero_copy_from_pipe, zero_copy_to_pipe};
pub use prefixed_io::PrefixedReadIo;
pub use split::{OwnedReadHalf, OwnedWriteHalf, Split, Splitable};
