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
                            *ref_mut = Lifecycle::Completed(
                                stored_result,
                                stored_flags & !IORING_CQE_F_MORE,
                            );
                        }
                        _ => unreachable!("lifecycle state mismatch"),
                    }
                }
                Lifecycle::WaitingMore(_, _, _) => {
                    let old = std::mem::replace(ref_mut, Lifecycle::Submitted);
                    match old {
                        Lifecycle::WaitingMore(waker, stored_result, stored_flags) => {
                            *ref_mut = Lifecycle::Completed(
                                stored_result,
                                stored_flags & !IORING_CQE_F_MORE,
                            );
                            waker.wake();
                        }
                        _ => unreachable!("lifecycle state mismatch"),
                    }
                }
                Lifecycle::IgnoredMore(..) => {
                    self.remove();
                }
                _ => unreachable!("lifecycle state mismatch"),
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
                        _ => unreachable!("lifecycle state mismatch"),
                    }
                }
                Lifecycle::Ignored(_) => {
                    let old = std::mem::replace(ref_mut, Lifecycle::Submitted);
                    match old {
                        Lifecycle::Ignored(data) => {
                            *ref_mut = Lifecycle::IgnoredMore(data);
                        }
                        _ => unreachable!("lifecycle state mismatch"),
                    }
                }
                _ => unreachable!("lifecycle state mismatch"),
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
                    _ => unreachable!("lifecycle state mismatch"),
                }
            }
            Lifecycle::Ignored(..) => {
                self.remove();
            }
            Lifecycle::Completed(..) => unreachable!("lifecycle state mismatch"),
            _ => unreachable!("lifecycle state mismatch"),
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
                    _ => unreachable!("lifecycle state mismatch"),
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
                        _ => unreachable!("lifecycle state mismatch"),
                    }
                }
                return Poll::Pending;
            }
            _ => {}
        }

        match self.remove().lifecycle {
            Lifecycle::Completed(result, flags) => Poll::Ready(CompletionMeta { result, flags }),
            _ => unreachable!("lifecycle state mismatch"),
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
                    _ => unreachable!("lifecycle state mismatch"),
                }
                return false;
            }
            Lifecycle::Completed(..) => {
                self.remove();
            }
            Lifecycle::Ignored(..) | Lifecycle::IgnoredMore(..) => {
                unreachable!("lifecycle state mismatch")
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
    };

    use super::*;
    use crate::utils::slab::Slab;

    fn vtable_ref() -> &'static RawWakerVTable {
        &RawWakerVTable::new(
            |ptr| {
                let arc = unsafe { Arc::from_raw(ptr as *const AtomicBool) };
                let cloned = arc.clone();
                std::mem::forget(arc);
                RawWaker::new(Arc::into_raw(cloned) as *const (), vtable_ref())
            },
            |ptr| {
                let arc = unsafe { Arc::from_raw(ptr as *const AtomicBool) };
                arc.store(true, Ordering::SeqCst);
            },
            |ptr| {
                let arc = unsafe { Arc::from_raw(ptr as *const AtomicBool) };
                arc.store(true, Ordering::SeqCst);
                std::mem::forget(arc);
            },
            |ptr| {
                unsafe { Arc::from_raw(ptr as *const AtomicBool) };
            },
        )
    }

    /// Create a waker that sets a flag when woken.
    fn flag_waker() -> (Waker, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();
        let raw = RawWaker::new(Arc::into_raw(flag_clone) as *const (), vtable_ref());
        let waker = unsafe { Waker::from_raw(raw) };
        (waker, flag)
    }

    fn insert_lifecycle(slab: &mut Slab<MaybeFdLifecycle>) -> usize {
        slab.insert(MaybeFdLifecycle::new(false))
    }

    /// Submitted → MORE CQE → CompletedMore → NOTIF CQE → Completed → poll → Ready
    #[test]
    fn multi_cqe_no_poll_between() {
        let mut slab = Slab::new();
        let key = insert_lifecycle(&mut slab);

        // First CQE with MORE flag
        let r = slab.get(key).unwrap();
        unsafe { r.complete(Ok(42), IORING_CQE_F_MORE) };

        // Notification CQE
        let r = slab.get(key).unwrap();
        unsafe { r.complete(Ok(0), IORING_CQE_F_NOTIF) };

        // Poll should return Ready with the result from the first CQE
        let (waker, _flag) = flag_waker();
        let mut cx = Context::from_waker(&waker);
        let r = slab.get(key).unwrap();
        match r.poll_op(&mut cx) {
            Poll::Ready(meta) => {
                assert_eq!(meta.result.unwrap().into_inner(), 42);
                // MORE flag should be cleared
                assert_eq!(meta.flags & IORING_CQE_F_MORE, 0);
            }
            Poll::Pending => panic!("expected Ready"),
        }
    }

    /// Submitted → poll → Waiting → MORE CQE → WaitingMore → poll (Pending)
    /// → NOTIF CQE (wakes) → poll → Ready
    #[test]
    fn multi_cqe_poll_between() {
        let mut slab = Slab::new();
        let key = insert_lifecycle(&mut slab);

        let (waker, woken) = flag_waker();
        let mut cx = Context::from_waker(&waker);

        // First poll: Submitted → Waiting
        let r = slab.get(key).unwrap();
        assert!(r.poll_op(&mut cx).is_pending());

        // First CQE with MORE: Waiting → WaitingMore (should NOT wake)
        woken.store(false, Ordering::SeqCst);
        let r = slab.get(key).unwrap();
        unsafe { r.complete(Ok(100), IORING_CQE_F_MORE) };
        assert!(!woken.load(Ordering::SeqCst), "should not wake on MORE CQE");

        // Poll again: WaitingMore → still Pending
        let r = slab.get(key).unwrap();
        assert!(r.poll_op(&mut cx).is_pending());

        // Notification CQE: WaitingMore → Completed, wakes
        woken.store(false, Ordering::SeqCst);
        let r = slab.get(key).unwrap();
        unsafe { r.complete(Ok(0), IORING_CQE_F_NOTIF) };
        assert!(woken.load(Ordering::SeqCst), "should wake on NOTIF CQE");

        // Final poll: Completed → Ready
        let r = slab.get(key).unwrap();
        match r.poll_op(&mut cx) {
            Poll::Ready(meta) => {
                assert_eq!(meta.result.unwrap().into_inner(), 100);
                assert_eq!(meta.flags & IORING_CQE_F_MORE, 0);
            }
            Poll::Pending => panic!("expected Ready"),
        }
    }

    /// Submitted → MORE CQE → CompletedMore → drop_op → IgnoredMore
    /// → NOTIF CQE → slot removed
    #[test]
    fn multi_cqe_drop_before_notification_from_completed_more() {
        let mut slab = Slab::new();
        let key = insert_lifecycle(&mut slab);

        // First CQE with MORE
        let r = slab.get(key).unwrap();
        unsafe { r.complete(Ok(10), IORING_CQE_F_MORE) };

        // Drop the op (future dropped before notification)
        let r = slab.get(key).unwrap();
        let mut data: Option<()> = Some(());
        let finished = r.drop_op(&mut data);
        assert!(!finished, "op not finished yet, notification pending");

        // Notification CQE arrives — should clean up the slot
        let r = slab.get(key).unwrap();
        unsafe { r.complete(Ok(0), IORING_CQE_F_NOTIF) };

        // Slot should have been removed
        assert!(slab.get(key).is_none());
    }

    /// Submitted → poll → Waiting → MORE CQE → WaitingMore → drop_op → IgnoredMore
    /// → NOTIF CQE → slot removed
    #[test]
    fn multi_cqe_drop_before_notification_from_waiting_more() {
        let mut slab = Slab::new();
        let key = insert_lifecycle(&mut slab);

        let (waker, _flag) = flag_waker();
        let mut cx = Context::from_waker(&waker);

        // Poll: Submitted → Waiting
        let r = slab.get(key).unwrap();
        assert!(r.poll_op(&mut cx).is_pending());

        // First CQE with MORE: Waiting → WaitingMore
        let r = slab.get(key).unwrap();
        unsafe { r.complete(Ok(20), IORING_CQE_F_MORE) };

        // Drop the op
        let r = slab.get(key).unwrap();
        let mut data: Option<()> = Some(());
        let finished = r.drop_op(&mut data);
        assert!(!finished, "op not finished yet, notification pending");

        // Notification CQE arrives — should clean up the slot
        let r = slab.get(key).unwrap();
        unsafe { r.complete(Ok(0), IORING_CQE_F_NOTIF) };

        // Slot should have been removed
        assert!(slab.get(key).is_none());
    }

    /// Normal single-CQE completion path (regression guard).
    #[test]
    fn single_cqe_submitted_to_completed() {
        let mut slab = Slab::new();
        let key = insert_lifecycle(&mut slab);

        // Complete without MORE flag
        let r = slab.get(key).unwrap();
        unsafe { r.complete(Ok(7), 0) };

        // Poll should return Ready
        let (waker, _flag) = flag_waker();
        let mut cx = Context::from_waker(&waker);
        let r = slab.get(key).unwrap();
        match r.poll_op(&mut cx) {
            Poll::Ready(meta) => {
                assert_eq!(meta.result.unwrap().into_inner(), 7);
                assert_eq!(meta.flags, 0);
            }
            Poll::Pending => panic!("expected Ready"),
        }
    }

    /// Single-CQE with waker: Submitted → poll → Waiting → complete → wakes → poll → Ready
    #[test]
    fn single_cqe_with_waker() {
        let mut slab = Slab::new();
        let key = insert_lifecycle(&mut slab);

        let (waker, woken) = flag_waker();
        let mut cx = Context::from_waker(&waker);

        // Poll: Submitted → Waiting
        let r = slab.get(key).unwrap();
        assert!(r.poll_op(&mut cx).is_pending());

        // Complete: Waiting → Completed, wakes
        woken.store(false, Ordering::SeqCst);
        let r = slab.get(key).unwrap();
        unsafe { r.complete(Ok(99), 0) };
        assert!(woken.load(Ordering::SeqCst));

        // Poll: Completed → Ready
        let r = slab.get(key).unwrap();
        match r.poll_op(&mut cx) {
            Poll::Ready(meta) => {
                assert_eq!(meta.result.unwrap().into_inner(), 99);
            }
            Poll::Pending => panic!("expected Ready"),
        }
    }
}
