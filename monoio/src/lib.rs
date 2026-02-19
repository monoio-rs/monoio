#![doc = include_str!("../../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs, unreachable_pub)]
#![allow(stable_features)]
#![allow(clippy::macro_metavars_in_unsafe)]
#![cfg_attr(feature = "unstable", feature(io_error_more))]
#![cfg_attr(feature = "unstable", feature(lazy_cell))]
#![cfg_attr(feature = "unstable", feature(stmt_expr_attributes))]
#![cfg_attr(feature = "unstable", feature(thread_local))]

#[macro_use]
pub mod macros;
#[cfg(feature = "macros")]
#[doc(hidden)]
pub use monoio_macros::select_priv_declare_output_enum;
#[macro_use]
mod driver;
pub(crate) mod builder;
#[allow(dead_code)]
pub(crate) mod runtime;
mod scheduler;
pub mod time;

extern crate alloc;

#[cfg(feature = "sync")]
pub mod blocking;

pub mod buf;
pub mod fs;
pub mod io;
pub mod net;
pub mod task;
pub mod utils;

use std::future::Future;

#[cfg(feature = "sync")]
pub use blocking::spawn_blocking;
pub use builder::{Buildable, RuntimeBuilder};
pub use driver::Driver;
#[cfg(all(target_os = "linux", feature = "iouring"))]
pub use driver::IoUringDriver;
#[cfg(all(windows, feature = "iocp"))]
pub use driver::IocpDriver;
#[cfg(feature = "legacy")]
pub use driver::LegacyDriver;
#[cfg(feature = "macros")]
pub use monoio_macros::{main, test, test_all};
pub use runtime::{spawn, Runtime};
#[cfg(any(all(target_os = "linux", feature = "iouring"), feature = "legacy"))]
pub use {builder::FusionDriver, runtime::FusionRuntime};

/// Start a monoio runtime.
///
/// # Examples
///
/// Basic usage
///
/// ```no_run
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     #[cfg(not(all(target_os = "linux", feature = "iouring")))]
///     let r = monoio::start::<monoio::LegacyDriver, _>(async {
///         // Open a file
///         let file = monoio::fs::File::open("hello.txt").await?;
///
///         let buf = vec![0; 4096];
///         // Read some data, the buffer is passed by ownership and
///         // submitted to the kernel. When the operation completes,
///         // we get the buffer back.
///         let (res, buf) = file.read_at(buf, 0).await;
///         let n = res?;
///
///         // Display the contents
///         println!("{:?}", &buf[..n]);
///
///         Ok(())
///     });
///     #[cfg(all(target_os = "linux", feature = "iouring"))]
///     let r = Ok(());
///     r
/// }
/// ```
pub fn start<D, F>(future: F) -> F::Output
where
    F: Future,
    F::Output: 'static,
    D: Buildable + Driver,
{
    let mut rt = builder::Buildable::build(builder::RuntimeBuilder::<D>::new())
        .expect("Unable to build runtime.");
    rt.block_on(future)
}

/// A specialized `Result` type for `io-uring` operations with buffers.
///
/// This type is used as a return value for asynchronous `io-uring` methods that
/// require passing ownership of a buffer to the runtime. When the operation
/// completes, the buffer is returned whether or not the operation completed
/// successfully.
///
/// # Examples
///
/// ```no_run
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     #[cfg(not(all(target_os = "linux", feature = "iouring")))]
///     let r = monoio::start::<monoio::LegacyDriver, _>(async {
///         // Open a file
///         let file = monoio::fs::File::open("hello.txt").await?;
///
///         let buf = vec![0; 4096];
///         // Read some data, the buffer is passed by ownership and
///         // submitted to the kernel. When the operation completes,
///         // we get the buffer back.
///         let (res, buf) = file.read_at(buf, 0).await;
///         let n = res?;
///
///         // Display the contents
///         println!("{:?}", &buf[..n]);
///
///         Ok(())
///     });
///     #[cfg(all(target_os = "linux", feature = "iouring"))]
///     let r = Ok(());
///     r
/// }
/// ```
pub type BufResult<T, B> = (std::io::Result<T>, B);
