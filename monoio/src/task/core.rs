use std::{
    cell::UnsafeCell,
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
};

use super::{
    raw::{self, Vtable},
    state::State,
    utils::UnsafeCellExt,
    Schedule,
};

#[repr(C)]
pub(crate) struct Cell<T: Future, S> {
    pub(crate) header: Header,
    pub(crate) core: Core<T, S>,
    pub(crate) trailer: Trailer,
}

pub(crate) struct Core<T: Future, S> {
    /// Scheduler used to drive this future
    pub(crate) scheduler: S,
    /// Either the future or the output
    pub(crate) stage: CoreStage<T>,
}
pub(crate) struct CoreStage<T: Future> {
    stage: UnsafeCell<Stage<T>>,
}

pub(crate) enum Stage<T: Future> {
    Running(T),
    Finished(T::Output),
    Consumed,
}

#[repr(C)]
pub(crate) struct Header {
    /// State
    pub(crate) state: State,
    /// Table of function pointers for executing actions on the task.
    pub(crate) vtable: &'static Vtable,
    /// Thread ID(sync: used for wake task on its thread; sync disabled: do checking)
    pub(crate) owner_id: usize,
}

pub(crate) struct Trailer {
    /// Consumer task waiting on completion of this task.
    pub(crate) waker: UnsafeCell<Option<Waker>>,
}

impl<T: Future, S: Schedule> Cell<T, S> {
    /// Allocates a new task cell, containing the header, trailer, and core
    /// structures.
    pub(crate) fn new(owner_id: usize, future: T, scheduler: S) -> Box<Cell<T, S>> {
        Box::new(Cell {
            header: Header {
                state: State::new(),
                vtable: raw::vtable::<T, S>(),
                owner_id,
            },
            core: Core {
                scheduler,
                stage: CoreStage {
                    stage: UnsafeCell::new(Stage::Running(future)),
                },
            },
            trailer: Trailer {
                waker: UnsafeCell::new(None),
            },
        })
    }
}

impl<T: Future> CoreStage<T> {
    pub(crate) fn with_mut<R>(&self, f: impl FnOnce(*mut Stage<T>) -> R) -> R {
        self.stage.with_mut(f)
    }

    pub(crate) fn poll(&self, mut cx: Context<'_>) -> Poll<T::Output> {
        let res = {
            self.with_mut(|ptr| {
                // Safety: The caller ensures mutual exclusion to the field.
                let future = match unsafe { &mut *ptr } {
                    Stage::Running(future) => future,
                    _ => unreachable!("unexpected stage"),
                };

                // Safety: The caller ensures the future is pinned.
                let future = unsafe { Pin::new_unchecked(future) };

                future.poll(&mut cx)
            })
        };

        if res.is_ready() {
            self.drop_future_or_output();
        }

        res
    }

    /// Drop the future
    ///
    /// # Safety
    ///
    /// The caller must ensure it is safe to mutate the `stage` field.
    pub(crate) fn drop_future_or_output(&self) {
        // Safety: the caller ensures mutual exclusion to the field.
        unsafe {
            self.set_stage(Stage::Consumed);
        }
    }

    /// Store the task output
    ///
    /// # Safety
    ///
    /// The caller must ensure it is safe to mutate the `stage` field.
    pub(crate) fn store_output(&self, output: T::Output) {
        // Safety: the caller ensures mutual exclusion to the field.
        unsafe {
            self.set_stage(Stage::Finished(output));
        }
    }

    /// Take the task output
    ///
    /// # Safety
    ///
    /// The caller must ensure it is safe to mutate the `stage` field.
    pub(crate) fn take_output(&self) -> T::Output {
        use std::mem;

        self.with_mut(|ptr| {
            // Safety:: the caller ensures mutual exclusion to the field.
            match mem::replace(unsafe { &mut *ptr }, Stage::Consumed) {
                Stage::Finished(output) => output,
                _ => panic!("JoinHandle polled after completion"),
            }
        })
    }

    unsafe fn set_stage(&self, stage: Stage<T>) { unsafe {
        self.with_mut(|ptr| *ptr = stage)
    }}
}

impl Header {
    #[allow(unused)]
    pub(crate) fn get_owner_id(&self) -> usize {
        // safety: If there are concurrent writes, then that write has violated
        // the safety requirements on `set_owner_id`.
        self.owner_id
    }
}

impl Trailer {
    pub(crate) unsafe fn set_waker(&self, waker: Option<Waker>) { unsafe {
        self.waker.with_mut(|ptr| {
            *ptr = waker;
        });
    }}

    pub(crate) unsafe fn will_wake(&self, waker: &Waker) -> bool { unsafe {
        self.waker
            .with(|ptr| (*ptr).as_ref().unwrap().will_wake(waker))
    }}

    pub(crate) fn wake_join(&self) {
        self.waker.with(|ptr| match unsafe { &*ptr } {
            Some(waker) => waker.wake_by_ref(),
            None => panic!("waker missing"),
        });
    }
}
