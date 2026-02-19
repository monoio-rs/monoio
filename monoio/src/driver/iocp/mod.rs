mod afd;
mod core;
mod event;
mod state;
mod waker;

pub use core::*;
use std::{
    collections::VecDeque,
    os::windows::prelude::{AsRawHandle, RawHandle, RawSocket},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub use afd::*;
pub use event::*;
pub use state::*;
pub use waker::*;
use windows_sys::Win32::{
    Foundation::WAIT_TIMEOUT,
    System::IO::{OVERLAPPED, OVERLAPPED_ENTRY},
};

pub struct Poller {
    is_polling: AtomicBool,
    cp: Arc<CompletionPort>,
    update_queue: Mutex<VecDeque<Pin<Arc<Mutex<SockState>>>>>,
    afd: Mutex<Vec<Arc<Afd>>>,
}

impl Poller {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            is_polling: AtomicBool::new(false),
            cp: Arc::new(CompletionPort::new(0)?),
            update_queue: Mutex::new(VecDeque::new()),
            afd: Mutex::new(Vec::new()),
        })
    }

    pub fn poll(&self, events: &mut Events, timeout: Option<Duration>) -> std::io::Result<()> {
        events.clear();

        if timeout.is_none() {
            loop {
                let len = self.poll_inner(&mut events.statuses, &mut events.events, None)?;
                if len == 0 {
                    continue;
                }
                break Ok(());
            }
        } else {
            self.poll_inner(&mut events.statuses, &mut events.events, timeout)?;
            Ok(())
        }
    }

    pub fn poll_inner(
        &self,
        entries: &mut [OVERLAPPED_ENTRY],
        events: &mut Vec<Event>,
        timeout: Option<Duration>,
    ) -> std::io::Result<usize> {
        self.is_polling.swap(true, Ordering::AcqRel);

        unsafe { self.update_sockets_events() }?;

        let result = self.cp.get_many(entries, timeout);

        self.is_polling.store(false, Ordering::Relaxed);

        match result {
            Ok(iocp_events) => Ok(unsafe { self.feed_events(events, iocp_events) }),
            Err(ref e) if e.raw_os_error() == Some(WAIT_TIMEOUT as i32) => Ok(0),
            Err(e) => Err(e),
        }
    }

    unsafe fn update_sockets_events(&self) -> std::io::Result<()> {
        let mut queue = self.update_queue.lock().unwrap();
        for sock in queue.iter_mut() {
            let mut sock_internal = sock.lock().unwrap();
            if !sock_internal.delete_pending {
                sock_internal.update(sock)?;
            }
        }

        queue.retain(|sock| sock.lock().unwrap().error.is_some());

        let mut afd = self.afd.lock().unwrap();
        afd.retain(|g| Arc::strong_count(g) > 1);
        Ok(())
    }

    unsafe fn feed_events(&self, events: &mut Vec<Event>, entries: &[OVERLAPPED_ENTRY]) -> usize {
        let mut n = 0;
        let mut update_queue = self.update_queue.lock().unwrap();
        for entry in entries.iter() {
            if entry.lpOverlapped.is_null() {
                events.push(Event::from_entry(entry));
                n += 1;
                continue;
            }

            let sock_state = from_overlapped(entry.lpOverlapped);
            let mut sock_guard = sock_state.lock().unwrap();
            if let Some(e) = sock_guard.feed_event() {
                events.push(e);
                n += 1;
            }

            if !sock_guard.delete_pending {
                update_queue.push_back(sock_state.clone());
            }
        }
        let mut afd = self.afd.lock().unwrap();
        afd.retain(|sock| Arc::strong_count(sock) > 1);
        n
    }

    pub fn register(
        &self,
        state: &mut SocketState,
        token: mio::Token,
        interests: mio::Interest,
    ) -> std::io::Result<()> {
        if state.inner.is_none() {
            let flags = interests_to_afd_flags(interests);

            let inner = {
                let sock = self._alloc_sock_for_rawsocket(state.socket)?;
                let event = Event {
                    flags,
                    data: token.0 as u64,
                };
                sock.lock().unwrap().set_event(event);
                sock
            };

            self.queue_state(inner.clone());
            unsafe { self.update_sockets_events_if_polling()? };
            state.inner = Some(inner);
            state.token = token;
            state.interest = interests;

            Ok(())
        } else {
            Err(std::io::ErrorKind::AlreadyExists.into())
        }
    }

    pub fn reregister(
        &self,
        state: &mut SocketState,
        token: mio::Token,
        interests: mio::Interest,
    ) -> std::io::Result<()> {
        if let Some(inner) = state.inner.as_mut() {
            {
                let event = Event {
                    flags: interests_to_afd_flags(interests),
                    data: token.0 as u64,
                };

                inner.lock().unwrap().set_event(event);
            }

            state.token = token;
            state.interest = interests;

            self.queue_state(inner.clone());
            unsafe { self.update_sockets_events_if_polling() }
        } else {
            Err(std::io::ErrorKind::NotFound.into())
        }
    }

    pub fn deregister(&mut self, state: &mut SocketState) -> std::io::Result<()> {
        if let Some(inner) = state.inner.as_mut() {
            {
                let mut sock_state = inner.lock().unwrap();
                sock_state.mark_delete();
            }
            state.inner = None;
            Ok(())
        } else {
            Err(std::io::ErrorKind::NotFound.into())
        }
    }

    /// This function is called by register() and reregister() to start an
    /// IOCTL_AFD_POLL operation corresponding to the registered events, but
    /// only if necessary.
    ///
    /// Since it is not possible to modify or synchronously cancel an AFD_POLL
    /// operation, and there can be only one active AFD_POLL operation per
    /// (socket, completion port) pair at any time, it is expensive to change
    /// a socket's event registration after it has been submitted to the kernel.
    ///
    /// Therefore, if no other threads are polling when interest in a socket
    /// event is (re)registered, the socket is added to the 'update queue', but
    /// the actual syscall to start the IOCTL_AFD_POLL operation is deferred
    /// until just before the GetQueuedCompletionStatusEx() syscall is made.
    ///
    /// However, when another thread is already blocked on
    /// GetQueuedCompletionStatusEx() we tell the kernel about the registered
    /// socket event(s) immediately.
    unsafe fn update_sockets_events_if_polling(&self) -> std::io::Result<()> {
        if self.is_polling.load(Ordering::Acquire) {
            self.update_sockets_events()
        } else {
            Ok(())
        }
    }

    fn queue_state(&self, sock_state: Pin<Arc<Mutex<SockState>>>) {
        let mut update_queue = self.update_queue.lock().unwrap();
        update_queue.push_back(sock_state);
    }

    fn _alloc_sock_for_rawsocket(
        &self,
        raw_socket: RawSocket,
    ) -> std::io::Result<Pin<Arc<Mutex<SockState>>>> {
        const POLL_GROUP__MAX_GROUP_SIZE: usize = 32;

        let mut afd_group = self.afd.lock().unwrap();
        if afd_group.is_empty() {
            self._alloc_afd_group(&mut afd_group)?;
        } else {
            // + 1 reference in Vec
            if Arc::strong_count(afd_group.last().unwrap()) > POLL_GROUP__MAX_GROUP_SIZE {
                self._alloc_afd_group(&mut afd_group)?;
            }
        }
        let afd = match afd_group.last() {
            Some(arc) => arc.clone(),
            None => unreachable!("Cannot acquire afd"),
        };

        Ok(Arc::pin(Mutex::new(SockState::new(raw_socket, afd)?)))
    }

    fn _alloc_afd_group(&self, afd_group: &mut Vec<Arc<Afd>>) -> std::io::Result<()> {
        let afd = Afd::new(&self.cp)?;
        let arc = Arc::new(afd);
        afd_group.push(arc);
        Ok(())
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        loop {
            let count: usize;
            let mut statuses: [OVERLAPPED_ENTRY; 1024] = unsafe { std::mem::zeroed() };

            let result = self
                .cp
                .get_many(&mut statuses, Some(Duration::from_millis(0)));
            match result {
                Ok(events) => {
                    count = events.iter().len();
                    for event in events.iter() {
                        if event.lpOverlapped.is_null() {
                        } else {
                            // drain sock state to release memory of Arc reference
                            let _ = from_overlapped(event.lpOverlapped);
                        }
                    }
                }
                Err(_) => break,
            }

            if count == 0 {
                break;
            }
        }

        let mut afd_group = self.afd.lock().unwrap();
        afd_group.retain(|g| Arc::strong_count(g) > 1);
    }
}

impl AsRawHandle for Poller {
    fn as_raw_handle(&self) -> RawHandle {
        self.cp.as_raw_handle()
    }
}

pub fn from_overlapped(ptr: *mut OVERLAPPED) -> Pin<Arc<Mutex<SockState>>> {
    let sock_ptr: *const Mutex<SockState> = ptr as *const _;
    unsafe { Pin::new_unchecked(Arc::from_raw(sock_ptr)) }
}

pub fn into_overlapped(sock_state: Pin<Arc<Mutex<SockState>>>) -> *mut std::ffi::c_void {
    let overlapped_ptr: *const Mutex<SockState> =
        unsafe { Arc::into_raw(Pin::into_inner_unchecked(sock_state)) };
    overlapped_ptr as *mut _
}

pub fn interests_to_afd_flags(interests: mio::Interest) -> u32 {
    let mut flags = 0;

    if interests.is_readable() {
        flags |= READABLE_FLAGS | READ_CLOSED_FLAGS | ERROR_FLAGS;
    }

    if interests.is_writable() {
        flags |= WRITABLE_FLAGS | WRITE_CLOSED_FLAGS | ERROR_FLAGS;
    }

    flags
}

/// Monoio IOCP Driver.
use std::{
    cell::UnsafeCell,
    mem::ManuallyDrop,
    rc::Rc,
    task::{Context, Poll},
};

#[cfg(feature = "sync")]
pub(crate) use waker::UnparkHandle;
use windows_sys::Win32::Networking::WinSock::{
    setsockopt, SOCKET, SOL_SOCKET, SO_UPDATE_ACCEPT_CONTEXT, WSAENETDOWN,
};

use super::{
    op::{CompletionMeta, Op, OpAble},
    Driver, Inner, CURRENT,
};
#[cfg(feature = "iocp")]
use crate::driver::op::{Overlapped, Syscall};
#[cfg(feature = "iocp")]
use crate::{driver::lifecycle::MaybeFdLifecycle, utils::slab::Slab};

#[allow(unused)]
pub(crate) const CANCEL_USERDATA: u64 = u64::MAX;

pub(crate) const MIN_REVERSED_USERDATA: u64 = u64::MAX - 3;

/// Driver with IOCP.
#[cfg(feature = "iocp")]
pub struct IocpDriver {
    inner: Rc<UnsafeCell<IocpInner>>,

    // Used as read eventfd buffer
    #[cfg(feature = "sync")]
    eventfd_read_dst: *mut u8,

    // Used for drop
    #[cfg(feature = "sync")]
    thread_id: usize,
}

#[cfg(feature = "iocp")]
pub(crate) struct IocpInner {
    /// In-flight operations
    ops: Ops,

    /// IOCP bindings
    iocp: ManuallyDrop<CompletionPort>,

    /// Shared waker
    #[cfg(feature = "sync")]
    shared_waker: Arc<EventWaker>,

    // Mark if eventfd is in the ring
    #[cfg(feature = "sync")]
    eventfd_installed: bool,

    // Waker receiver
    #[cfg(feature = "sync")]
    waker_receiver: flume::Receiver<std::task::Waker>,
}

// When dropping the driver, all in-flight operations must have completed. This
// type wraps the slab and ensures that, on drop, the slab is empty.
#[cfg(feature = "iocp")]
struct Ops {
    slab: Slab<MaybeFdLifecycle>,
}

#[cfg(feature = "iocp")]
impl IocpDriver {
    const DEFAULT_ENTRIES: u32 = 1024;

    pub(crate) fn new() -> std::io::Result<IocpDriver> {
        Self::new_with_entries(Self::DEFAULT_ENTRIES)
    }

    #[cfg(not(feature = "sync"))]
    pub(crate) fn new_with_entries(_entries: u32) -> std::io::Result<IocpDriver> {
        let inner = Rc::new(UnsafeCell::new(IocpInner {
            ops: Ops::new(),
            iocp: ManuallyDrop::new(CompletionPort::new(0)?),
        }));

        Ok(IocpDriver { inner })
    }

    #[cfg(feature = "sync")]
    pub(crate) fn new_with_entries(_entries: u32) -> std::io::Result<IocpDriver> {
        // Create eventfd and register it to the ring.
        let waker = tempfile::tempfile()?;

        let (waker_sender, waker_receiver) = flume::unbounded::<std::task::Waker>();

        let inner = Rc::new(UnsafeCell::new(IocpInner {
            ops: Ops::new(),
            iocp: ManuallyDrop::new(CompletionPort::new(0)?),
            shared_waker: Arc::new(EventWaker::new(waker)),
            eventfd_installed: false,
            waker_receiver,
        }));

        let thread_id = crate::builder::BUILD_THREAD_ID.with(|id| *id);
        let driver = IocpDriver {
            inner,
            eventfd_read_dst: Box::leak(Box::new([0_u8; 8])) as *mut u8,
            thread_id,
        };

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

    fn inner_park(&self, timeout: Option<Duration>) -> std::io::Result<()> {
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

        let mut cq: [OVERLAPPED_ENTRY; 1024] = unsafe { std::mem::zeroed() };
        if need_wait {
            // submit_and_wait with timeout
            inner.iocp.get_many(&mut cq, timeout)?;
        } else {
            // Submit only
            inner.iocp.get_many(&mut cq, Some(Duration::ZERO))?;
        }

        // Set status as awake
        #[cfg(feature = "sync")]
        inner.shared_waker.awake.store(true, Ordering::Release);

        // Process CQ
        inner.tick(cq)?;

        Ok(())
    }
}

#[cfg(feature = "iocp")]
impl Driver for IocpDriver {
    /// Enter the driver context. This enables using iocp types.
    fn with<R>(&self, f: impl FnOnce() -> R) -> R {
        // TODO(ihciah): remove clone
        let inner = Inner::Iocp(self.inner.clone());
        CURRENT.set(&inner, f)
    }

    fn submit(&self) -> std::io::Result<()> {
        Ok(())
    }

    fn park(&self) -> std::io::Result<()> {
        self.inner_park(None)
    }

    fn park_timeout(&self, duration: Duration) -> std::io::Result<()> {
        self.inner_park(Some(duration))
    }

    #[cfg(feature = "sync")]
    type Unpark = UnparkHandle;

    #[cfg(feature = "sync")]
    fn unpark(&self) -> Self::Unpark {
        IocpInner::unpark(&self.inner)
    }
}

#[cfg(feature = "iocp")]
impl IocpInner {
    fn tick(&mut self, cq: [OVERLAPPED_ENTRY; 1024]) -> std::io::Result<()> {
        for entry in cq {
            let cqe = unsafe { *Box::from_raw(entry.lpOverlapped.cast::<Overlapped>()) };
            let index = cqe.user_data;
            match index {
                _ if index >= MIN_REVERSED_USERDATA as usize => (),
                // # Safety
                // Here we can make sure the result is valid.
                _ => unsafe { self.ops.complete(index as _, resultify(&cqe, &entry), 0) },
            }
        }
        Ok(())
    }

    fn new_op<T: OpAble>(data: T, inner: &mut IocpInner, driver: Inner) -> Op<T> {
        Op {
            driver,
            index: inner.ops.insert(T::RET_IS_FD),
            data: Some(data),
        }
    }

    pub(crate) fn submit_with_data<T>(
        this: &Rc<UnsafeCell<IocpInner>>,
        data: T,
    ) -> std::io::Result<Op<T>>
    where
        T: OpAble,
    {
        let inner = unsafe { &mut *this.get() };

        // Create the operation
        let mut op = Self::new_op(data, inner, Inner::Iocp(this.clone()));

        // Configure the SQE
        let data_mut = unsafe { op.data.as_mut().unwrap_unchecked() };
        OpAble::iocp_op(data_mut, &inner.iocp, op.index)?;

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
        this: &Rc<UnsafeCell<IocpInner>>,
        index: usize,
        cx: &mut Context<'_>,
    ) -> Poll<CompletionMeta> {
        let inner = unsafe { &mut *this.get() };
        let lifecycle = unsafe { inner.ops.slab.get(index).unwrap_unchecked() };
        lifecycle.poll_op(cx)
    }

    #[allow(unused_variables)]
    pub(crate) fn drop_op<T: 'static>(
        this: &Rc<UnsafeCell<IocpInner>>,
        index: usize,
        data: &mut Option<T>,
        _skip_cancel: bool,
    ) {
        todo!()
    }

    #[allow(unused_variables)]
    pub(crate) unsafe fn cancel_op(this: &Rc<UnsafeCell<IocpInner>>, index: usize) {
        todo!()
    }

    #[cfg(feature = "sync")]
    pub(crate) fn unpark(this: &Rc<UnsafeCell<IocpInner>>) -> UnparkHandle {
        let inner = unsafe { &*this.get() };
        let weak = Arc::downgrade(&inner.shared_waker);
        UnparkHandle(weak)
    }
}

#[cfg(feature = "iocp")]
impl AsRawHandle for IocpDriver {
    fn as_raw_handle(&self) -> RawHandle {
        unsafe { (*self.inner.get()).iocp.as_raw_handle() }
    }
}

#[cfg(feature = "iocp")]
impl Drop for IocpDriver {
    fn drop(&mut self) {
        trace!("MONOIO DEBUG[IocpDriver]: drop");

        #[cfg(feature = "sync")]
        unsafe {
            std::ptr::drop_in_place(self.eventfd_read_dst)
        };

        // Deregister thread id
        #[cfg(feature = "sync")]
        {
            use crate::driver::thread::{unregister_unpark_handle, unregister_waker_sender};
            unregister_unpark_handle(self.thread_id);
            unregister_waker_sender(self.thread_id);
        }
    }
}

#[cfg(feature = "iocp")]
impl Drop for IocpInner {
    fn drop(&mut self) {
        // no need to wait for completion, as the kernel will clean up the ring asynchronically.
        unsafe {
            ManuallyDrop::drop(&mut self.iocp);
        }
    }
}

#[cfg(feature = "iocp")]
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
    unsafe fn complete(&mut self, index: usize, result: std::io::Result<u32>, flags: u32) {
        let lifecycle = unsafe { self.slab.get(index).unwrap_unchecked() };
        lifecycle.complete(result, flags);
    }
}

#[cfg(feature = "iocp")]
#[inline]
fn resultify(cqe: &Overlapped, entry: &OVERLAPPED_ENTRY) -> std::io::Result<u32> {
    let res = match cqe.syscall {
        Syscall::accept => {
            if unsafe {
                setsockopt(
                    cqe.socket,
                    SOL_SOCKET,
                    SO_UPDATE_ACCEPT_CONTEXT,
                    std::ptr::from_ref(&cqe.from_fd).cast(),
                    std::ffi::c_int::try_from(size_of::<SOCKET>()).expect("overflow"),
                )
            } == 0
            {
                cqe.socket.try_into().expect("result overflow")
            } else {
                -WSAENETDOWN
            }
        }
        Syscall::recv | Syscall::WSARecv | Syscall::send | Syscall::WSASend => {
            entry.dwNumberOfBytesTransferred.try_into().unwrap()
        }
    };

    if res >= 0 {
        Ok(res as u32)
    } else {
        Err(std::io::Error::from_raw_os_error(-res))
    }
}
