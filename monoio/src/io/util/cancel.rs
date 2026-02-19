use std::{cell::RefCell, collections::HashSet, rc::Rc};

use crate::driver::op::OpCanceller;

/// CancelHandle is used to pass to io actions with CancelableAsyncReadRent.
/// Create a CancelHandle with Canceller::handle.
#[derive(Clone)]
pub struct CancelHandle {
    shared: Rc<RefCell<Shared>>,
}

/// Canceller is a user-hold struct to cancel io operations.
/// A canceller can associate with multiple io operations.
#[derive(Default)]
pub struct Canceller {
    shared: Rc<RefCell<Shared>>,
}

pub(crate) struct AssociateGuard {
    op_canceller: OpCanceller,
    shared: Rc<RefCell<Shared>>,
}

#[derive(Default)]
struct Shared {
    canceled: bool,
    slot_ref: HashSet<OpCanceller>,
}

impl Canceller {
    /// Create a new Canceller.
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }

    /// Cancel all related operations.
    pub fn cancel(self) -> Self {
        let mut slot = HashSet::new();
        {
            let mut shared = self.shared.borrow_mut();
            shared.canceled = true;
            std::mem::swap(&mut slot, &mut shared.slot_ref);
        }

        for op_canceller in slot.iter() {
            unsafe { op_canceller.cancel() };
        }
        slot.clear();
        Canceller {
            shared: Rc::new(RefCell::new(Shared {
                canceled: false,
                slot_ref: slot,
            })),
        }
    }

    /// Create a CancelHandle which can be used to pass to io operation.
    #[inline]
    pub fn handle(&self) -> CancelHandle {
        CancelHandle {
            shared: self.shared.clone(),
        }
    }
}

impl CancelHandle {
    pub(crate) fn canceled(&self) -> bool {
        self.shared.borrow().canceled
    }

    pub(crate) fn associate_op(self, op_canceller: OpCanceller) -> AssociateGuard {
        {
            let mut shared = self.shared.borrow_mut();
            shared.slot_ref.insert(op_canceller.clone());
        }
        AssociateGuard {
            op_canceller,
            shared: self.shared,
        }
    }
}

impl Drop for AssociateGuard {
    fn drop(&mut self) {
        let mut shared = self.shared.borrow_mut();
        shared.slot_ref.remove(&self.op_canceller);
    }
}

pub(crate) fn operation_canceled() -> std::io::Error {
    std::io::Error::from_raw_os_error(libc::ECANCELED)
}

pub(crate) fn is_operation_canceled(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(libc::ECANCELED)
}
