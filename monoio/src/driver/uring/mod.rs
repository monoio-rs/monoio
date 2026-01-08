//! Monoio Uring Driver.

use std::{
    cell::UnsafeCell,
    io,
    mem::ManuallyDrop,
    os::unix::prelude::{AsRawFd, RawFd},
    rc::Rc,
    task::{Context, Poll},
    time::Duration,
};

use io_uring::{cqueue, opcode, types::Timespec, IoUring};
use lifecycle::MaybeFdLifecycle;

use super::{
    op::{CompletionMeta, Op, OpAble},
    // ready::Ready,
    // scheduled_io::ScheduledIo,
    util::timespec,
    Driver,
    Inner,
    CURRENT,
};
use crate::utils::slab::Slab;

mod lifecycle;
#[cfg(feature = "sync")]
mod waker;
#[cfg(feature = "sync")]
pub(crate) use waker::UnparkHandle;

#[allow(unused)]
pub(crate) const CANCEL_USERDATA: u64 = u64::MAX;
pub(crate) const TIMEOUT_USERDATA: u64 = u64::MAX - 1;
#[allow(unused)]
pub(crate) const EVENTFD_USERDATA: u64 = u64::MAX - 2;
#[cfg(feature = "poll-io")]
pub(crate) const POLLER_USERDATA: u64 = u64::MAX - 3;

pub(crate) const MIN_REVERSED_USERDATA: u64 = u64::MAX - 3;

/// Driver with uring.
pub struct IoUringDriver {
    inner: Rc<UnsafeCell<UringInner>>,

    // Used for drop
    #[cfg(feature = "sync")]
    thread_id: usize,
}

pub(crate) struct UringInner {
    /// In-flight operations
    ops: Ops,

    // Used as timeout buffer
    timespec: Timespec,

    // Used as read eventfd buffer
    #[cfg(feature = "sync")]
    eventfd_read_dst: [u8; 8],

    #[cfg(feature = "poll-io")]
    poll: super::poll::Poll,
    #[cfg(feature = "poll-io")]
    poller_installed: bool,

    /// IoUring bindings
    uring: ManuallyDrop<IoUring>,

    /// Shared waker
    #[cfg(feature = "sync")]
    shared_waker: std::sync::Arc<waker::EventWaker>,

    // Mark if eventfd is in the ring
    #[cfg(feature = "sync")]
    eventfd_installed: bool,

    // Waker receiver
    #[cfg(feature = "sync")]
    waker_receiver: flume::Receiver<std::task::Waker>,

    // Uring support ext_arg
    ext_arg: bool,
}

// When dropping the driver, all in-flight operations must have completed. This
// type wraps the slab and ensures that, on drop, the slab is empty.
struct Ops {
    slab: Slab<MaybeFdLifecycle>,
}

impl IoUringDriver {
    const DEFAULT_ENTRIES: u32 = 1024;

    pub(crate) fn new(b: &io_uring::Builder) -> io::Result<IoUringDriver> {
        Self::new_with_entries(b, Self::DEFAULT_ENTRIES)
    }

    #[cfg(not(feature = "sync"))]
    pub(crate) fn new_with_entries(
        urb: &io_uring::Builder,
        entries: u32,
    ) -> io::Result<IoUringDriver> {
        let uring = ManuallyDrop::new(urb.build(entries)?);

        let inner = Rc::new(UnsafeCell::new(UringInner {
            #[cfg(feature = "poll-io")]
            poll: super::poll::Poll::with_capacity(entries as usize)?,
            #[cfg(feature = "poll-io")]
            poller_installed: false,
            ops: Ops::new(),
            timespec: Timespec::new(),
            ext_arg: uring.params().is_feature_ext_arg(),
            uring,
        }));

        Ok(IoUringDriver { inner })
    }

    #[cfg(feature = "sync")]
    pub(crate) fn new_with_entries(
        urb: &io_uring::Builder,
        entries: u32,
    ) -> io::Result<IoUringDriver> {
        let uring = ManuallyDrop::new(urb.build(entries)?);

        // Create eventfd and register it to the ring.
        let waker = {
            let fd = crate::syscall!(eventfd@RAW(0, libc::EFD_CLOEXEC))?;
            unsafe {
                use std::os::unix::io::FromRawFd;
                std::fs::File::from_raw_fd(fd)
            }
        };

        let (waker_sender, waker_receiver) = flume::unbounded::<std::task::Waker>();

        let inner = Rc::new(UnsafeCell::new(UringInner {
            #[cfg(feature = "poll-io")]
            poller_installed: false,
            #[cfg(feature = "poll-io")]
            poll: super::poll::Poll::with_capacity(entries as usize)?,
            ops: Ops::new(),
            timespec: Timespec::new(),
            ext_arg: uring.params().is_feature_ext_arg(),
            uring,
            shared_waker: std::sync::Arc::new(waker::EventWaker::new(waker)),
            eventfd_installed: false,
            eventfd_read_dst: [0_u8; 8],
            waker_receiver,
        }));

        let thread_id = crate::builder::BUILD_THREAD_ID.with(|id| *id);
        let driver = IoUringDriver { inner, thread_id };

        // Register unpark handle
        super::thread::register_unpark_handle(thread_id, driver.unpark().into());
        super::thread::register_waker_sender(thread_id, waker_sender);
        Ok(driver)
    }

    #[allow(unused)]
    fn num_operations(&self) -> usize {
        let inner = self.inner.get();
        unsafe { (*inner).ops.slab.len() }
    }

    // Flush to make enough space
    fn flush_space(inner: &mut UringInner, need: usize) -> io::Result<()> {
        let sq = inner.uring.submission();
        debug_assert!(sq.capacity() >= need);
        if sq.len() + need > sq.capacity() {
            drop(sq);
            inner.submit()?;
        }
        Ok(())
    }

    #[cfg(feature = "sync")]
    fn install_eventfd(&self, inner: &mut UringInner, fd: RawFd) {
        let entry = opcode::Read::new(
            io_uring::types::Fd(fd),
            inner.eventfd_read_dst.as_mut_ptr(),
            8,
        )
        .build()
        .user_data(EVENTFD_USERDATA);

        let mut sq = inner.uring.submission();
        let _ = unsafe { sq.push(&entry) };
        inner.eventfd_installed = true;
    }

    #[cfg(feature = "poll-io")]
    fn install_poller(&self, inner: &mut UringInner, fd: RawFd) {
        let entry = opcode::PollAdd::new(io_uring::types::Fd(fd), libc::POLLIN as _)
            .build()
            .user_data(POLLER_USERDATA);

        let mut sq = inner.uring.submission();
        let _ = unsafe { sq.push(&entry) };
        inner.poller_installed = true;
    }

    fn install_timeout(&self, inner: &mut UringInner, duration: Duration) {
        inner.timespec = timespec(duration);
        let entry = opcode::Timeout::new(&inner.timespec as *const Timespec)
            .build()
            .user_data(TIMEOUT_USERDATA);

        let mut sq = inner.uring.submission();
        let _ = unsafe { sq.push(&entry) };
    }

    fn inner_park(&self, timeout: Option<Duration>) -> io::Result<()> {
        let inner = unsafe { &mut *self.inner.get() };

        #[allow(unused_mut)]
        let mut need_wait = true;

        #[cfg(feature = "sync")]
        {
            // Process foreign wakers
            while let Ok(w) = inner.waker_receiver.try_recv() {
                w.wake();
                need_wait = false;
            }

            // Set status as not awake if we are going to sleep
            if need_wait {
                inner
                    .shared_waker
                    .awake
                    .store(false, std::sync::atomic::Ordering::Release);
            }

            // Process foreign wakers left
            while let Ok(w) = inner.waker_receiver.try_recv() {
                w.wake();
                need_wait = false;
            }
        }

        if need_wait {
            // Install timeout and eventfd for unpark if sync is enabled

            // 1. alloc spaces
            let mut space = 0;
            #[cfg(feature = "sync")]
            if !inner.eventfd_installed {
                space += 1;
            }
            #[cfg(feature = "poll-io")]
            if !inner.poller_installed {
                space += 1;
            }
            if timeout.is_some() {
                space += 1;
            }
            if space != 0 {
                Self::flush_space(inner, space)?;
            }

            // 2.1 install poller
            #[cfg(feature = "poll-io")]
            if !inner.poller_installed {
                self.install_poller(inner, inner.poll.as_raw_fd());
            }

            // 2.2 install eventfd and timeout
            #[cfg(feature = "sync")]
            if !inner.eventfd_installed {
                self.install_eventfd(inner, inner.shared_waker.as_raw_fd());
            }

            // 2.3 install timeout and submit_and_wait with timeout
            if let Some(duration) = timeout {
                match inner.ext_arg {
                    // Submit and Wait with timeout in an TimeoutOp way.
                    // Better compatibility(5.4+).
                    false => {
                        self.install_timeout(inner, duration);
                        inner.uring.submit_and_wait(1)?;
                    }
                    // Submit and Wait with enter args.
                    // Better performance(5.11+).
                    true => {
                        let timespec = timespec(duration);
                        let args = io_uring::types::SubmitArgs::new().timespec(&timespec);
                        if let Err(e) = inner.uring.submitter().submit_with_args(1, &args) {
                            if e.raw_os_error() != Some(libc::ETIME) {
                                return Err(e);
                            }
                        }
                    }
                }
            } else {
                // Submit and Wait without timeout
                inner.uring.submit_and_wait(1)?;
            }
        } else {
            // Submit only
            inner.uring.submit()?;
        }

        // Set status as awake
        #[cfg(feature = "sync")]
        inner
            .shared_waker
            .awake
            .store(true, std::sync::atomic::Ordering::Release);

        // Process CQ
        inner.tick()?;

        Ok(())
    }

    #[cfg(feature = "poll-io")]
    #[inline]
    pub(crate) fn register_poll_io(
        this: &Rc<UnsafeCell<UringInner>>,
        source: &mut impl mio::event::Source,
        interest: mio::Interest,
    ) -> io::Result<usize> {
        let inner = unsafe { &mut *this.get() };
        inner.poll.register(source, interest)
    }

    #[cfg(feature = "poll-io")]
    #[inline]
    pub(crate) fn deregister_poll_io(
        this: &Rc<UnsafeCell<UringInner>>,
        source: &mut impl mio::event::Source,
        token: usize,
    ) -> io::Result<()> {
        let inner = unsafe { &mut *this.get() };
        inner.poll.deregister(source, token)
    }
}

impl Driver for IoUringDriver {
    /// Enter the driver context. This enables using uring types.
    fn with<R>(&self, f: impl FnOnce() -> R) -> R {
        // TODO(ihciah): remove clone
        let inner = Inner::Uring(self.inner.clone());
        CURRENT.set(&inner, f)
    }

    fn submit(&self) -> io::Result<()> {
        let inner = unsafe { &mut *self.inner.get() };
        inner.submit()?;
        inner.tick()?;
        Ok(())
    }

    fn park(&self) -> io::Result<()> {
        self.inner_park(None)
    }

    fn park_timeout(&self, duration: Duration) -> io::Result<()> {
        self.inner_park(Some(duration))
    }

    #[cfg(feature = "sync")]
    type Unpark = waker::UnparkHandle;

    #[cfg(feature = "sync")]
    fn unpark(&self) -> Self::Unpark {
        UringInner::unpark(&self.inner)
    }
}

impl UringInner {
    fn tick(&mut self) -> io::Result<()> {
        let cq = self.uring.completion();

        for cqe in cq {
            let index = cqe.user_data();
            match index {
                #[cfg(feature = "sync")]
                EVENTFD_USERDATA => self.eventfd_installed = false,
                #[cfg(feature = "poll-io")]
                POLLER_USERDATA => {
                    self.poller_installed = false;
                    self.poll.tick(Some(Duration::ZERO))?;
                }
                _ if index >= MIN_REVERSED_USERDATA => (),
                // # Safety
                // Here we can make sure the result is valid.
                _ => unsafe { self.ops.complete(index as _, resultify(&cqe), cqe.flags()) },
            }
        }
        Ok(())
    }

    fn submit(&mut self) -> io::Result<()> {
        loop {
            match self.uring.submit() {
                #[cfg(feature = "unstable")]
                Err(ref e)
                    if matches!(e.kind(), io::ErrorKind::Other | io::ErrorKind::ResourceBusy) =>
                {
                    self.tick()?;
                }
                #[cfg(not(feature = "unstable"))]
                Err(ref e)
                    if matches!(e.raw_os_error(), Some(libc::EAGAIN) | Some(libc::EBUSY)) =>
                {
                    // This error is constructed with io::Error::last_os_error():
                    // https://github.com/tokio-rs/io-uring/blob/01c83bbce965d4aaf93ebfaa08c3aa8b7b0f5335/src/sys/mod.rs#L32
                    // So we can use https://doc.rust-lang.org/nightly/std/io/struct.Error.html#method.raw_os_error
                    // to get the raw error code.
                    self.tick()?;
                }
                e => return e.map(|_| ()),
            }
        }
    }

    fn new_op<T: OpAble>(data: T, inner: &mut UringInner, driver: Inner) -> Op<T> {
        Op {
            driver,
            index: inner.ops.insert(T::RET_IS_FD),
            data: Some(data),
        }
    }

    pub(crate) fn submit_with_data<T>(
        this: &Rc<UnsafeCell<UringInner>>,
        data: T,
    ) -> io::Result<Op<T>>
    where
        T: OpAble,
    {
        let inner = unsafe { &mut *this.get() };
        // If the submission queue is full, flush it to the kernel
        if inner.uring.submission().is_full() {
            inner.submit()?;
        }

        // Create the operation
        let mut op = Self::new_op(data, inner, Inner::Uring(this.clone()));

        // Configure the SQE
        let data_mut = unsafe { op.data.as_mut().unwrap_unchecked() };
        let sqe = OpAble::uring_op(data_mut).user_data(op.index as _);

        {
            let mut sq = inner.uring.submission();

            // Push the new operation
            if unsafe { sq.push(&sqe).is_err() } {
                unimplemented!("when is this hit?");
            }
        }

        // Submit the new operation. At this point, the operation has been
        // pushed onto the queue and the tail pointer has been updated, so
        // the submission entry is visible to the kernel. If there is an
        // error here (probably EAGAIN), we still return the operation. A
        // future `io_uring_enter` will fully submit the event.

        // CHIHAI: We are not going to do syscall now. If we are waiting
        // for IO, we will submit on `park`.
        // let _ = inner.submit();
        Ok(op)
    }

    pub(crate) fn poll_op(
        this: &Rc<UnsafeCell<UringInner>>,
        index: usize,
        cx: &mut Context<'_>,
    ) -> Poll<CompletionMeta> {
        let inner = unsafe { &mut *this.get() };
        let lifecycle = unsafe { inner.ops.slab.get(index).unwrap_unchecked() };
        lifecycle.poll_op(cx)
    }

    #[cfg(feature = "poll-io")]
    pub(crate) fn poll_legacy_op<T: OpAble>(
        this: &Rc<UnsafeCell<Self>>,
        data: &mut T,
        cx: &mut Context<'_>,
    ) -> Poll<CompletionMeta> {
        let inner = unsafe { &mut *this.get() };
        let (direction, index) = match data.legacy_interest() {
            Some(x) => x,
            None => {
                // if there is no index provided, it means the action does not rely on fd
                // readiness. do syscall right now.
                return Poll::Ready(CompletionMeta {
                    result: OpAble::legacy_call(data),
                    flags: 0,
                });
            }
        };

        // wait io ready and do syscall
        inner
            .poll
            .poll_syscall(cx, index, direction, || OpAble::legacy_call(data))
    }

    pub(crate) fn drop_op<T: 'static>(
        this: &Rc<UnsafeCell<UringInner>>,
        index: usize,
        data: &mut Option<T>,
        _skip_cancel: bool,
    ) {
        let inner = unsafe { &mut *this.get() };
        if index == usize::MAX {
            // already finished
            return;
        }
        if let Some(lifecycle) = inner.ops.slab.get(index) {
            let _must_finished = lifecycle.drop_op(data);
            #[cfg(feature = "async-cancel")]
            if !_must_finished && !_skip_cancel {
                unsafe {
                    let cancel = opcode::AsyncCancel::new(index as u64)
                        .build()
                        .user_data(u64::MAX);

                    // Try push cancel, if failed, will submit and re-push.
                    if inner.uring.submission().push(&cancel).is_err() {
                        let _ = inner.submit();
                        let _ = inner.uring.submission().push(&cancel);
                    }
                }
            }
        }
    }

    pub(crate) unsafe fn cancel_op(this: &Rc<UnsafeCell<UringInner>>, index: usize) {
        let inner = &mut *this.get();
        let cancel = opcode::AsyncCancel::new(index as u64)
            .build()
            .user_data(u64::MAX);
        if inner.uring.submission().push(&cancel).is_err() {
            let _ = inner.submit();
            let _ = inner.uring.submission().push(&cancel);
        }
    }

    #[cfg(feature = "sync")]
    pub(crate) fn unpark(this: &Rc<UnsafeCell<UringInner>>) -> waker::UnparkHandle {
        let inner = unsafe { &*this.get() };
        let weak = std::sync::Arc::downgrade(&inner.shared_waker);
        waker::UnparkHandle(weak)
    }
}

impl AsRawFd for IoUringDriver {
    fn as_raw_fd(&self) -> RawFd {
        unsafe { (*self.inner.get()).uring.as_raw_fd() }
    }
}

impl Drop for IoUringDriver {
    fn drop(&mut self) {
        trace!("MONOIO DEBUG[IoUringDriver]: drop");

        // Deregister thread id
        #[cfg(feature = "sync")]
        {
            use crate::driver::thread::{unregister_unpark_handle, unregister_waker_sender};
            unregister_unpark_handle(self.thread_id);
            unregister_waker_sender(self.thread_id);
        }
    }
}

impl Drop for UringInner {
    fn drop(&mut self) {
        // no need to wait for completion, as the kernel will clean up the ring asynchronically.
        let _ = self.uring.submitter().submit();
        unsafe {
            ManuallyDrop::drop(&mut self.uring);
        }
    }
}

impl Ops {
    const fn new() -> Self {
        Ops { slab: Slab::new() }
    }

    // Insert a new operation
    #[inline]
    pub(crate) fn insert(&mut self, is_fd: bool) -> usize {
        self.slab.insert(MaybeFdLifecycle::new(is_fd))
    }

    // Complete an operation
    // # Safety
    // Caller must make sure the result is valid.
    #[inline]
    unsafe fn complete(&mut self, index: usize, result: io::Result<u32>, flags: u32) {
        let lifecycle = unsafe { self.slab.get(index).unwrap_unchecked() };
        lifecycle.complete(result, flags);
    }
}

#[inline]
fn resultify(cqe: &cqueue::Entry) -> io::Result<u32> {
    let res = cqe.result();

    if res >= 0 {
        Ok(res as u32)
    } else {
        Err(io::Error::from_raw_os_error(-res))
    }
}
