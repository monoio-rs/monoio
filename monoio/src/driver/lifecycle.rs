//! Uring/Iocp state lifecycle.
//! Partly borrow from tokio-uring.

use std::{
    io,
    task::{Context, Poll, Waker},
};

use crate::{
    driver::op::{CompletionMeta, MaybeFd},
    utils::slab::Ref,
};

enum Lifecycle {
    /// The operation has been submitted to uring and is currently in-flight
    Submitted,

    /// The submitter is waiting for the completion of the operation
    Waiting(Waker),

    /// The submitter no longer has interest in the operation result. The state
    /// must be passed to the driver and held until the operation completes.
    #[allow(dead_code)]
    Ignored(Box<dyn std::any::Any>),

    /// The operation has completed.
    Completed(io::Result<MaybeFd>, u32),
}

pub(crate) struct MaybeFdLifecycle {
    is_fd: bool,
    lifecycle: Lifecycle,
}

impl MaybeFdLifecycle {
    #[inline]
    pub(crate) const fn new(is_fd: bool) -> Self {
        Self {
            is_fd,
            lifecycle: Lifecycle::Submitted,
        }
    }
}

impl Ref<'_, MaybeFdLifecycle> {
    // # Safety
    // Caller must make sure the result is valid since it may contain fd or a length hint.
    pub(crate) unsafe fn complete(mut self, result: io::Result<u32>, flags: u32) {
        let result = MaybeFd::new_result(result, self.is_fd);
        let ref_mut = &mut self.lifecycle;
        match ref_mut {
            Lifecycle::Submitted => {
                *ref_mut = Lifecycle::Completed(result, flags);
            }
            Lifecycle::Waiting(_) => {
                let old = std::mem::replace(ref_mut, Lifecycle::Completed(result, flags));
                match old {
                    Lifecycle::Waiting(waker) => {
                        waker.wake();
                    }
                    _ => std::hint::unreachable_unchecked(),
                }
            }
            Lifecycle::Ignored(..) => {
                self.remove();
            }
            Lifecycle::Completed(..) => std::hint::unreachable_unchecked(),
        }
    }

    #[allow(clippy::needless_pass_by_ref_mut)]
    pub(crate) fn poll_op(mut self, cx: &mut Context<'_>) -> Poll<CompletionMeta> {
        let ref_mut = &mut self.lifecycle;
        match ref_mut {
            Lifecycle::Submitted => {
                *ref_mut = Lifecycle::Waiting(cx.waker().clone());
                return Poll::Pending;
            }
            Lifecycle::Waiting(waker) => {
                if !waker.will_wake(cx.waker()) {
                    *ref_mut = Lifecycle::Waiting(cx.waker().clone());
                }
                return Poll::Pending;
            }
            _ => {}
        }

        match self.remove().lifecycle {
            Lifecycle::Completed(result, flags) => Poll::Ready(CompletionMeta { result, flags }),
            _ => unsafe { std::hint::unreachable_unchecked() },
        }
    }

    // return if the op must have been finished
    pub(crate) fn drop_op<T: 'static>(mut self, data: &mut Option<T>) -> bool {
        let ref_mut = &mut self.lifecycle;
        match ref_mut {
            Lifecycle::Submitted | Lifecycle::Waiting(_) => {
                if let Some(data) = data.take() {
                    *ref_mut = Lifecycle::Ignored(Box::new(data));
                } else {
                    *ref_mut = Lifecycle::Ignored(Box::new(())); // () is a ZST, so it does not
                                                                 // allocate
                };
                return false;
            }
            Lifecycle::Completed(..) => {
                self.remove();
            }
            Lifecycle::Ignored(..) => unsafe { std::hint::unreachable_unchecked() },
        }
        true
    }
}
