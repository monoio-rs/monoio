//! Uring state lifecycle.
//! Partly borrow from tokio-uring.

use std::{
    io,
    task::{Context, Poll, Waker},
};

use crate::{
    driver::op::{CompletionMeta, MaybeFd},
    utils::slab::Ref,
};

/// Flag indicating more CQEs will follow for this SQE (e.g. SEND_ZC).
const IORING_CQE_F_MORE: u32 = 2;
/// Flag indicating this CQE is a zero-copy notification.
const IORING_CQE_F_NOTIF: u32 = 8;

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

    /// First CQE with IORING_CQE_F_MORE received, waiting for notification CQE.
    /// The future has not been polled yet (was in Submitted state).
    CompletedMore(io::Result<MaybeFd>, u32),

    /// First CQE with IORING_CQE_F_MORE received, waiting for notification CQE.
    /// The future is actively polling (was in Waiting state).
    WaitingMore(Waker, io::Result<MaybeFd>, u32),

    /// The future was dropped but a notification CQE is still pending.
    #[allow(dead_code)]
    IgnoredMore(Box<dyn std::any::Any>),
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
        let is_fd = self.is_fd;
        let ref_mut = &mut self.lifecycle;

        // Handle notification CQE (second CQE of a multi-CQE op like SEND_ZC)
        if flags & IORING_CQE_F_NOTIF != 0 {
            match ref_mut {
                Lifecycle::CompletedMore(_, _) => {
                    // Move stored result into Completed state
                    let old = std::mem::replace(ref_mut, Lifecycle::Submitted);
                    match old {
                        Lifecycle::CompletedMore(stored_result, stored_flags) => {
                            *ref_mut = Lifecycle::Completed(stored_result, stored_flags);
                        }
                        _ => std::hint::unreachable_unchecked(),
                    }
                }
                Lifecycle::WaitingMore(_, _, _) => {
                    let old = std::mem::replace(ref_mut, Lifecycle::Submitted);
                    match old {
                        Lifecycle::WaitingMore(waker, stored_result, stored_flags) => {
                            *ref_mut = Lifecycle::Completed(stored_result, stored_flags);
                            waker.wake();
                        }
                        _ => std::hint::unreachable_unchecked(),
                    }
                }
                Lifecycle::IgnoredMore(..) => {
                    self.remove();
                }
                _ => std::hint::unreachable_unchecked(),
            }
            return;
        }

        // Handle first CQE with MORE flag (more CQEs will follow, e.g. SEND_ZC)
        if flags & IORING_CQE_F_MORE != 0 {
            let result = MaybeFd::new_result(result, is_fd);
            match ref_mut {
                Lifecycle::Submitted => {
                    *ref_mut = Lifecycle::CompletedMore(result, flags);
                }
                Lifecycle::Waiting(_) => {
                    let old = std::mem::replace(ref_mut, Lifecycle::Submitted);
                    match old {
                        Lifecycle::Waiting(waker) => {
                            // Don't wake yet — we need to wait for the notification CQE
                            *ref_mut = Lifecycle::WaitingMore(waker, result, flags);
                        }
                        _ => std::hint::unreachable_unchecked(),
                    }
                }
                Lifecycle::Ignored(_) => {
                    let old = std::mem::replace(ref_mut, Lifecycle::Submitted);
                    match old {
                        Lifecycle::Ignored(data) => {
                            *ref_mut = Lifecycle::IgnoredMore(data);
                        }
                        _ => std::hint::unreachable_unchecked(),
                    }
                }
                _ => std::hint::unreachable_unchecked(),
            }
            return;
        }

        // Normal single-CQE completion (existing behavior)
        let result = MaybeFd::new_result(result, is_fd);
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
            _ => std::hint::unreachable_unchecked(),
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
            // Multi-CQE: first CQE arrived but still waiting for notification
            Lifecycle::CompletedMore(_, _) => {
                let old = std::mem::replace(ref_mut, Lifecycle::Submitted);
                match old {
                    Lifecycle::CompletedMore(result, flags) => {
                        *ref_mut = Lifecycle::WaitingMore(cx.waker().clone(), result, flags);
                    }
                    _ => unsafe { std::hint::unreachable_unchecked() },
                }
                return Poll::Pending;
            }
            Lifecycle::WaitingMore(waker, _, _) => {
                if !waker.will_wake(cx.waker()) {
                    let old = std::mem::replace(ref_mut, Lifecycle::Submitted);
                    match old {
                        Lifecycle::WaitingMore(_, result, flags) => {
                            *ref_mut = Lifecycle::WaitingMore(cx.waker().clone(), result, flags);
                        }
                        _ => unsafe { std::hint::unreachable_unchecked() },
                    }
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

    // return if the op must has been finished
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
            // Multi-CQE: still waiting for notification, must keep the slot alive
            Lifecycle::CompletedMore(_, _) | Lifecycle::WaitingMore(_, _, _) => {
                let old = std::mem::replace(ref_mut, Lifecycle::Submitted);
                let boxed_data: Box<dyn std::any::Any> = if let Some(data) = data.take() {
                    Box::new(data)
                } else {
                    Box::new(())
                };
                match old {
                    Lifecycle::CompletedMore(_, _) | Lifecycle::WaitingMore(_, _, _) => {
                        *ref_mut = Lifecycle::IgnoredMore(boxed_data);
                    }
                    _ => unsafe { std::hint::unreachable_unchecked() },
                }
                return false;
            }
            Lifecycle::Completed(..) => {
                self.remove();
            }
            Lifecycle::Ignored(..) | Lifecycle::IgnoredMore(..) => unsafe {
                std::hint::unreachable_unchecked()
            },
        }
        true
    }
}
