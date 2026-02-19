//! Uring state lifecycle.
//! Partly borrow from tokio-uring.

use std::{
    collections::VecDeque,
    io,
    task::{Context, Poll, Waker},
};

use crate::{
    driver::op::{CompletionMeta, MaybeFd},
    utils::slab::Ref,
};

pub(crate) const IORING_CQE_F_BUFFER: u32 = 1 << 0;
pub(crate) const IORING_CQE_F_MORE: u32 = 1 << 1;

#[derive(Debug)]
pub(crate) struct MultishotCqe {
    pub result: io::Result<MaybeFd>,
    pub flags: u32,
    pub is_final: bool,
}

#[derive(Debug)]
pub(crate) enum MultishotPollResult {
    Ready(MultishotCqe),
    Terminated(MultishotCqe),
    Pending,
    Done,
}

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

    /// Active multishot operation with queued completions
    Multishot {
        queue: VecDeque<MultishotCqe>,
        waker: Option<Waker>,
        terminated: bool,
    },
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

    #[inline]
    pub(crate) fn new_multishot(queue_capacity: usize, is_fd: bool) -> Self {
        Self {
            is_fd,
            lifecycle: Lifecycle::Multishot {
                queue: VecDeque::with_capacity(queue_capacity),
                waker: None,
                terminated: false,
            },
        }
    }
}

impl Ref<'_, MaybeFdLifecycle> {
    /// # Safety
    /// Caller must make sure the result is valid since it may contain fd or a length hint.
    pub(crate) unsafe fn complete(mut self, result: io::Result<u32>, flags: u32) {
        let is_final = (flags & IORING_CQE_F_MORE) == 0;
        let is_fd = self.is_fd;

        match &mut self.lifecycle {
            Lifecycle::Submitted => {
                let result = MaybeFd::new_result(result, is_fd);
                self.lifecycle = Lifecycle::Completed(result, flags);
            }

            Lifecycle::Waiting(_) => {
                let result = MaybeFd::new_result(result, is_fd);
                let old =
                    std::mem::replace(&mut self.lifecycle, Lifecycle::Completed(result, flags));
                if let Lifecycle::Waiting(waker) = old {
                    waker.wake();
                }
            }

            Lifecycle::Multishot {
                queue,
                waker,
                terminated,
            } => {
                queue.push_back(MultishotCqe {
                    result: MaybeFd::new_result(result, is_fd),
                    flags,
                    is_final,
                });
                if is_final {
                    *terminated = true;
                }
                if let Some(w) = waker.take() {
                    w.wake();
                }
            }

            Lifecycle::Ignored(..) => {
                let _drop_fd = MaybeFd::new_result(result, is_fd);
                if is_final {
                    self.remove();
                }
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

    pub(crate) fn drop_op<T: 'static>(mut self, data: &mut Option<T>) -> bool {
        let ref_mut = &mut self.lifecycle;
        let terminated = match ref_mut {
            Lifecycle::Submitted | Lifecycle::Waiting(_) => false,
            Lifecycle::Completed(..) => true,
            Lifecycle::Multishot { terminated, .. } => *terminated,
            Lifecycle::Ignored(..) => unsafe { std::hint::unreachable_unchecked() },
        };

        if terminated {
            self.remove();
            true
        } else {
            let boxed: Box<dyn std::any::Any> = if let Some(d) = data.take() {
                Box::new(d)
            } else {
                Box::new(())
            };
            *ref_mut = Lifecycle::Ignored(boxed);
            false
        }
    }

    pub(crate) fn poll_multishot(mut self, cx: &mut Context<'_>) -> MultishotPollResult {
        let ref_mut = &mut self.lifecycle;

        match ref_mut {
            Lifecycle::Multishot {
                queue,
                waker,
                terminated,
            } => {
                if let Some(cqe) = queue.pop_front() {
                    let is_final = cqe.is_final;

                    if *terminated && queue.is_empty() {
                        return MultishotPollResult::Terminated(cqe);
                    }

                    if is_final {
                        MultishotPollResult::Terminated(cqe)
                    } else {
                        MultishotPollResult::Ready(cqe)
                    }
                } else if *terminated {
                    MultishotPollResult::Done
                } else {
                    *waker = Some(cx.waker().clone());
                    MultishotPollResult::Pending
                }
            }

            _ => MultishotPollResult::Done,
        }
    }
}
