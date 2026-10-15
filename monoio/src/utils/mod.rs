//! Common utils

pub(crate) mod auto_array;
pub(crate) mod box_into_inner;
pub(crate) mod linked_list;
#[allow(dead_code)]
pub(crate) mod slab;
#[allow(dead_code)]
pub(crate) mod thread_id;
pub(crate) mod uring_detect;

mod rand;
pub use rand::thread_rng_n;
pub use uring_detect::detect_uring;

pub use crate::driver::op::is_legacy;

#[cfg(feature = "signal")]
mod ctrlc;
#[cfg(feature = "signal")]
pub use self::ctrlc::{CtrlC, Error as CtrlCError};

#[cfg(feature = "utils")]
mod bind_to_cpu_set;
#[cfg(feature = "utils")]
pub use bind_to_cpu_set::{bind_to_cpu_set, BindError};
