use std::{
    future::Future,
    ptr::NonNull,
    task::{Poll, Waker},
};

use crate::task::{Cell, Harness, Header, Schedule};

pub(crate) struct RawTask {
    ptr: NonNull<Header>,
}

impl Clone for RawTask {
    fn clone(&self) -> Self {
        *self
    }
}

impl Copy for RawTask {}

pub(crate) struct Vtable {
    /// Poll the future
    pub(crate) poll: unsafe fn(NonNull<Header>),
    /// Deallocate the memory
    pub(crate) dealloc: unsafe fn(NonNull<Header>),

    /// Read the task output, if complete
    pub(crate) try_read_output: unsafe fn(NonNull<Header>, *mut (), &Waker),

    /// The join handle has been dropped
    pub(crate) drop_join_handle_slow: unsafe fn(NonNull<Header>),

    /// Set future output
    #[cfg(feature = "sync")]
    pub(crate) finish: unsafe fn(NonNull<Header>, *mut ()),
}

/// Get the vtable for the requested `T` and `S` generics.
pub(super) fn vtable<T: Future, S: Schedule>() -> &'static Vtable {
    &Vtable {
        poll: poll::<T, S>,
        dealloc: dealloc::<T, S>,
        try_read_output: try_read_output::<T, S>,
        drop_join_handle_slow: drop_join_handle_slow::<T, S>,
        #[cfg(feature = "sync")]
        finish: finish::<T, S>,
    }
}

impl RawTask {
    pub(crate) fn new<T, S>(owner_id: usize, task: T, scheduler: S) -> RawTask
    where
        T: Future,
        S: Schedule,
    {
        let ptr = Box::into_raw(Cell::new(owner_id, task, scheduler));
        let ptr = unsafe { NonNull::new_unchecked(ptr as *mut Header) };

        RawTask { ptr }
    }

    pub(crate) unsafe fn from_raw(ptr: NonNull<Header>) -> RawTask {
        RawTask { ptr }
    }

    pub(crate) fn header(&self) -> &Header {
        unsafe { self.ptr.as_ref() }
    }

    /// Safety: mutual exclusion is required to call this function.
    pub(crate) fn poll(self) {
        let vtable = self.header().vtable;
        unsafe { (vtable.poll)(self.ptr) }
    }

    pub(crate) fn dealloc(self) {
        let vtable = self.header().vtable;
        unsafe {
            (vtable.dealloc)(self.ptr);
        }
    }

    /// Safety: `dst` must be a `*mut Poll<super::Result<T::Output>>` where `T`
    /// is the future stored by the task.
    pub(crate) unsafe fn try_read_output(self, dst: *mut (), waker: &Waker) { unsafe {
        let vtable = self.header().vtable;
        (vtable.try_read_output)(self.ptr, dst, waker);
    }}

    pub(crate) fn drop_join_handle_slow(self) {
        let vtable = self.header().vtable;
        unsafe { (vtable.drop_join_handle_slow)(self.ptr) }
    }

    #[cfg(feature = "sync")]
    pub(crate) unsafe fn finish(self, val_slot: *mut ()) {
        let vtable = self.header().vtable;
        unsafe { (vtable.finish)(self.ptr, val_slot) }
    }
}

unsafe fn poll<T: Future, S: Schedule>(ptr: NonNull<Header>) { unsafe {
    let harness = Harness::<T, S>::from_raw(ptr);
    harness.poll();
}}

unsafe fn dealloc<T: Future, S: Schedule>(ptr: NonNull<Header>) { unsafe {
    let harness = Harness::<T, S>::from_raw(ptr);
    harness.dealloc();
}}

#[cfg(feature = "sync")]
unsafe fn finish<T: Future, S: Schedule>(ptr: NonNull<Header>, val: *mut ()) {
    let harness = Harness::<T, S>::from_raw(ptr);
    let val = &mut *(val as *mut Option<<T as Future>::Output>);
    harness.finish(val.take().unwrap());
}

unsafe fn try_read_output<T: Future, S: Schedule>(
    ptr: NonNull<Header>,
    dst: *mut (),
    waker: &Waker,
) { unsafe {
    let out = &mut *(dst as *mut Poll<T::Output>);

    let harness = Harness::<T, S>::from_raw(ptr);
    harness.try_read_output(out, waker);
}}

unsafe fn drop_join_handle_slow<T: Future, S: Schedule>(ptr: NonNull<Header>) { unsafe {
    let harness = Harness::<T, S>::from_raw(ptr);
    harness.drop_join_handle_slow()
}}
