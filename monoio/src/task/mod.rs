//! Task impl
// Heavily borrowed from tokio.
// Copyright (c) 2021 Tokio Contributors, licensed under the MIT license.

mod utils;
pub(crate) mod waker_fn;

mod core;
use self::core::{Cell, Header};

mod harness;
use self::harness::Harness;

mod join;
#[allow(unreachable_pub)] // https://github.com/rust-lang/rust/issues/57411
pub use self::join::JoinHandle;

mod raw;
use self::raw::RawTask;

mod state;

mod waker;

use std::{future::Future, marker::PhantomData, ptr::NonNull};

/// An owned handle to the task, tracked by ref count, not sendable
#[repr(transparent)]
pub(crate) struct Task<S: 'static> {
    raw: RawTask,
    _p: PhantomData<S>,
}

impl<S: 'static> Task<S> {
    unsafe fn from_raw(ptr: NonNull<Header>) -> Task<S> { unsafe {
        Task {
            raw: RawTask::from_raw(ptr),
            _p: PhantomData,
        }
    }}

    fn header(&self) -> &Header {
        self.raw.header()
    }

    pub(crate) fn run(self) {
        self.raw.poll();
    }

    #[cfg(feature = "sync")]
    pub(crate) unsafe fn finish(&mut self, val_slot: *mut ()) {
        self.raw.finish(val_slot);
    }
}

impl<S: 'static> Drop for Task<S> {
    fn drop(&mut self) {
        // Decrement the ref count
        if self.header().state.ref_dec() {
            // Deallocate if this is the final ref count
            self.raw.dealloc();
        }
    }
}

pub(crate) trait Schedule: Sized + 'static {
    /// Schedule the task
    fn schedule(&self, task: Task<Self>);
    /// Schedule the task to run in the near future, yielding the thread to
    /// other tasks.
    fn yield_now(&self, task: Task<Self>) {
        self.schedule(task);
    }
}

pub(crate) fn new_task<T, S>(
    owner_id: usize,
    task: T,
    scheduler: S,
) -> (Task<S>, JoinHandle<T::Output>)
where
    S: Schedule,
    T: Future + 'static,
    T::Output: 'static,
{
    unsafe { new_task_holding(owner_id, task, scheduler) }
}

pub(crate) unsafe fn new_task_holding<T, S>(
    owner_id: usize,
    task: T,
    scheduler: S,
) -> (Task<S>, JoinHandle<T::Output>)
where
    S: Schedule,
    T: Future,
{
    let raw = RawTask::new::<T, S>(owner_id, task, scheduler);
    let task = Task {
        raw,
        _p: PhantomData,
    };
    let join = JoinHandle::new(raw);

    (task, join)
}
