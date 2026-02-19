use std::{
    ffi::{c_int, c_uint},
    io,
    sync::Arc,
};

use super::{CompletionPort, Event, Poller};

#[derive(Debug)]
pub struct Waker {
    token: mio::Token,
    port: Arc<CompletionPort>,
}

impl Waker {
    #[allow(unreachable_code, unused_variables)]
    pub fn new(poller: &Poller, token: mio::Token) -> io::Result<Waker> {
        Ok(Waker {
            token,
            port: poller.cp.clone(),
        })
    }

    pub fn wake(&self) -> io::Result<()> {
        let mut ev = Event::new(self.token);
        ev.set_readable();
        self.port.post(ev.to_entry())
    }
}

/// Custom thread waker based on eventfd.
use std::os::windows::prelude::{AsRawHandle, RawHandle};

use crate::driver::unpark::Unpark;

pub(crate) struct EventWaker {
    // RawFd
    raw: usize,
    // File hold the ownership of fd, only useful when drop
    _file: std::fs::File,
    // Atomic awake status
    pub(crate) awake: std::sync::atomic::AtomicBool,
}

impl EventWaker {
    pub(crate) fn new(file: std::fs::File) -> Self {
        Self {
            raw: file.as_raw_handle() as usize,
            _file: file,
            awake: std::sync::atomic::AtomicBool::new(true),
        }
    }

    pub(crate) fn wake(&self) -> std::io::Result<()> {
        // Skip wake if already awake
        if self.awake.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        // Write data into EventFd to wake the executor.
        let buf = 0x1u64.to_ne_bytes();
        unsafe {
            // SAFETY: Writing number to eventfd is thread safe.
            libc::write(self.raw as c_int, buf.as_ptr().cast(), buf.len() as c_uint);
            Ok(())
        }
    }
}

impl AsRawHandle for EventWaker {
    fn as_raw_handle(&self) -> RawHandle {
        self.raw as RawHandle
    }
}

#[derive(Clone)]
pub struct UnparkHandle(pub(crate) std::sync::Weak<EventWaker>);

impl Unpark for UnparkHandle {
    fn unpark(&self) -> std::io::Result<()> {
        if let Some(w) = self.0.upgrade() {
            w.wake()
        } else {
            Ok(())
        }
    }
}
