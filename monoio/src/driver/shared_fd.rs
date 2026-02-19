#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{
    AsRawHandle, AsRawSocket, FromRawSocket, OwnedSocket, RawHandle, RawSocket,
};
use std::{cell::UnsafeCell, io, rc::Rc};

use super::CURRENT;
#[cfg(windows)]
use crate::driver::iocp::SocketState as RawFd;

// Tracks in-flight operations on a file descriptor. Ensures all in-flight
// operations complete before submitting the close.
#[derive(Clone, Debug)]
pub(crate) struct SharedFd {
    inner: Rc<Inner>,
}

struct Inner {
    // Open file descriptor
    #[cfg(any(unix, windows))]
    fd: RawFd,

    // Waker to notify when the close operation completes.
    state: UnsafeCell<State>,
}

enum State {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    Uring(UringState),
    #[cfg(all(windows, feature = "iocp"))]
    Iocp(IocpState),
    #[cfg(feature = "legacy")]
    Legacy(Option<usize>),
}

#[cfg(feature = "poll-io")]
impl State {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    #[allow(unreachable_patterns)]
    pub(crate) fn cvt_uring_poll(&mut self, fd: RawFd) -> io::Result<()> {
        let state = match self {
            State::Uring(state) => state,
            _ => return Ok(()),
        };
        // TODO: only Init state can convert?
        if matches!(state, UringState::Init) {
            let mut source = mio::unix::SourceFd(&fd);
            crate::syscall!(fcntl@RAW(fd, libc::F_SETFL, libc::O_NONBLOCK))?;
            let reg = CURRENT
                .with(|inner| match inner {
                    #[cfg(all(target_os = "linux", feature = "iouring"))]
                    crate::driver::Inner::Uring(r) => super::IoUringDriver::register_poll_io(
                        r,
                        &mut source,
                        super::ready::RW_INTERESTS,
                    ),
                    #[cfg(feature = "legacy")]
                    crate::driver::Inner::Legacy(_) => panic!("unexpected legacy runtime"),
                })
                .inspect_err(|_| {
                    let _ = crate::syscall!(fcntl@RAW(fd, libc::F_SETFL, 0));
                })?;
            *state = UringState::Legacy(Some(reg));
        } else {
            return Err(io::Error::other("not clear uring state"));
        }
        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "iouring")))]
    #[inline]
    pub(crate) fn cvt_uring_poll(&mut self, _fd: RawFd) -> io::Result<()> {
        Ok(())
    }

    #[cfg(all(target_os = "linux", feature = "iouring"))]
    pub(crate) fn cvt_comp(&mut self, fd: RawFd) -> io::Result<()> {
        let inner = match self {
            Self::Uring(UringState::Legacy(inner)) => inner,
            _ => return Ok(()),
        };
        let Some(token) = inner else {
            return Err(io::Error::other("empty token"));
        };
        let mut source = mio::unix::SourceFd(&fd);
        crate::syscall!(fcntl@RAW(fd, libc::F_SETFL, 0))?;
        CURRENT
            .with(|inner| match inner {
                #[cfg(all(target_os = "linux", feature = "iouring"))]
                crate::driver::Inner::Uring(r) => {
                    super::IoUringDriver::deregister_poll_io(r, &mut source, *token)
                }
                #[cfg(feature = "legacy")]
                crate::driver::Inner::Legacy(_) => panic!("unexpected legacy runtime"),
            })
            .inspect_err(|_| {
                let _ = crate::syscall!(fcntl@RAW(fd, libc::F_SETFL, libc::O_NONBLOCK));
            })?;
        *self = State::Uring(UringState::Init);
        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "iouring")))]
    #[inline]
    pub(crate) fn cvt_comp(&mut self, _fd: RawFd) -> io::Result<()> {
        Ok(())
    }
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner").field("fd", &self.fd).finish()
    }
}

#[cfg(all(target_os = "linux", feature = "iouring"))]
enum UringState {
    /// Initial state
    Init,

    /// Waiting for all in-flight operation to complete.
    Waiting(Option<std::task::Waker>),

    /// The FD is closing
    Closing(super::op::Op<super::op::close::Close>),

    /// The FD is fully closed
    Closed,

    /// Poller
    #[cfg(feature = "poll-io")]
    Legacy(Option<usize>),
}

#[cfg(all(windows, feature = "iocp"))]
enum IocpState {
    /// Initial state
    Init,

    /// Waiting for all in-flight operation to complete.
    Waiting(Option<std::task::Waker>),

    /// The FD is closing
    Closing(super::op::Op<super::op::close::Close>),

    /// The FD is fully closed
    Closed,
}

#[cfg(unix)]
impl AsRawFd for SharedFd {
    fn as_raw_fd(&self) -> RawFd {
        self.raw_fd()
    }
}

#[cfg(windows)]
impl AsRawSocket for SharedFd {
    fn as_raw_socket(&self) -> RawSocket {
        self.raw_socket()
    }
}

#[cfg(windows)]
impl AsRawHandle for SharedFd {
    fn as_raw_handle(&self) -> RawHandle {
        self.raw_handle()
    }
}

impl SharedFd {
    #[cfg(unix)]
    #[allow(unreachable_code, unused)]
    pub(crate) fn new<const FORCE_LEGACY: bool>(fd: RawFd) -> io::Result<SharedFd> {
        enum Reg {
            Uring,
            #[cfg(feature = "poll-io")]
            UringLegacy(io::Result<usize>),
            Legacy(io::Result<usize>),
        }

        #[cfg(all(target_os = "linux", feature = "iouring", feature = "legacy"))]
        let state = match CURRENT.with(|inner| match inner {
            super::Inner::Uring(inner) => match FORCE_LEGACY {
                false => Reg::Uring,
                true => {
                    #[cfg(feature = "poll-io")]
                    {
                        let mut source = mio::unix::SourceFd(&fd);
                        Reg::UringLegacy(super::IoUringDriver::register_poll_io(
                            inner,
                            &mut source,
                            super::ready::RW_INTERESTS,
                        ))
                    }
                    #[cfg(not(feature = "poll-io"))]
                    Reg::Uring
                }
            },
            super::Inner::Legacy(inner) => {
                let mut source = mio::unix::SourceFd(&fd);
                Reg::Legacy(super::legacy::LegacyDriver::register(
                    inner,
                    &mut source,
                    super::ready::RW_INTERESTS,
                ))
            }
        }) {
            Reg::Uring => State::Uring(UringState::Init),
            #[cfg(feature = "poll-io")]
            Reg::UringLegacy(idx) => State::Uring(UringState::Legacy(Some(idx?))),
            Reg::Legacy(idx) => State::Legacy(Some(idx?)),
        };

        #[cfg(all(not(feature = "legacy"), target_os = "linux", feature = "iouring"))]
        let state = State::Uring(UringState::Init);
        #[cfg(all(not(feature = "legacy"), windows, feature = "iocp"))]
        let state = State::Iocp(IocpState::Init);

        #[cfg(all(
            unix,
            feature = "legacy",
            not(all(target_os = "linux", feature = "iouring")),
            not(all(windows, feature = "iocp"))
        ))]
        let state = {
            let reg = CURRENT.with(|inner| match inner {
                super::Inner::Legacy(inner) => {
                    let mut source = mio::unix::SourceFd(&fd);
                    super::legacy::LegacyDriver::register(
                        inner,
                        &mut source,
                        super::ready::RW_INTERESTS,
                    )
                }
            });

            State::Legacy(Some(reg?))
        };

        #[cfg(all(
            not(feature = "legacy"),
            not(all(target_os = "linux", feature = "iouring"))
        ))]
        #[allow(unused)]
        let state = super::util::feature_panic();

        #[allow(unreachable_code)]
        Ok(SharedFd {
            inner: Rc::new(Inner {
                fd,
                state: UnsafeCell::new(state),
            }),
        })
    }

    #[allow(unused_variables)]
    #[cfg(windows)]
    pub(crate) fn new<const FORCE_LEGACY: bool>(fd: RawSocket) -> io::Result<SharedFd> {
        const RW_INTERESTS: mio::Interest = mio::Interest::READABLE.add(mio::Interest::WRITABLE);

        let mut fd = RawFd::new(fd);

        let state = {
            let reg: io::Result<usize> = CURRENT.with(|inner| match inner {
                #[cfg(feature = "legacy")]
                super::Inner::Legacy(inner) => {
                    super::legacy::LegacyDriver::register(inner, &mut fd, RW_INTERESTS)
                }
                #[cfg(feature = "iocp")]
                super::Inner::Iocp(_) => Ok(0),
            });

            cfg_if::cfg_if! {
                if #[cfg(feature = "iocp")] {
                    State::Iocp(IocpState::Init)
                } else if #[cfg(feature = "legacy")] {
                    State::Legacy(Some(reg?))
                } else {
                    unreachable!("you need to enable 'iocp' or 'legacy' feature")
                }
            }
        };

        #[allow(unreachable_code)]
        Ok(SharedFd {
            inner: Rc::new(Inner {
                fd,
                state: UnsafeCell::new(state),
            }),
        })
    }

    #[cfg(unix)]
    #[allow(unreachable_code, unused)]
    pub(crate) fn new_without_register(fd: RawFd) -> SharedFd {
        let state = CURRENT.with(|inner| match inner {
            #[cfg(all(target_os = "linux", feature = "iouring"))]
            super::Inner::Uring(_) => State::Uring(UringState::Init),
            #[cfg(feature = "legacy")]
            super::Inner::Legacy(_) => State::Legacy(None),
            #[cfg(all(
                not(feature = "legacy"),
                not(all(target_os = "linux", feature = "iouring"))
            ))]
            _ => {
                super::util::feature_panic();
            }
        });

        SharedFd {
            inner: Rc::new(Inner {
                fd,
                state: UnsafeCell::new(state),
            }),
        }
    }

    #[cfg(windows)]
    #[allow(unreachable_code, unused)]
    pub(crate) fn new_without_register(fd: RawSocket) -> SharedFd {
        let state = CURRENT.with(|inner| match inner {
            #[cfg(feature = "iocp")]
            super::Inner::Iocp(_) => State::Iocp(IocpState::Init),
            #[cfg(feature = "legacy")]
            super::Inner::Legacy(_) => State::Legacy(None),
        });

        SharedFd {
            inner: Rc::new(Inner {
                fd: RawFd::new(fd),
                state: UnsafeCell::new(state),
            }),
        }
    }

    #[cfg(unix)]
    /// Returns the RawFd
    pub(crate) fn raw_fd(&self) -> RawFd {
        self.inner.fd
    }

    #[cfg(windows)]
    /// Returns the RawSocket
    pub(crate) fn raw_socket(&self) -> RawSocket {
        self.inner.fd.socket
    }

    #[cfg(windows)]
    pub(crate) fn raw_handle(&self) -> RawHandle {
        self.inner.fd.socket as _
    }

    #[cfg(unix)]
    /// Try unwrap Rc, then deregister if registered and return rawfd.
    /// Note: this action will consume self and return rawfd without closing it.
    pub(crate) fn try_unwrap(self) -> Result<RawFd, Self> {
        use std::mem::{ManuallyDrop, MaybeUninit};

        let fd = self.inner.fd;
        match Rc::try_unwrap(self.inner) {
            Ok(inner) => {
                // Only drop Inner's state, skip its drop impl.
                let mut inner_skip_drop = ManuallyDrop::new(inner);
                #[allow(invalid_value)]
                #[allow(clippy::uninit_assumed_init)]
                let mut state = unsafe { MaybeUninit::uninit().assume_init() };
                std::mem::swap(&mut inner_skip_drop.state, &mut state);

                #[cfg(feature = "legacy")]
                let state = unsafe { &*state.get() };

                #[cfg(feature = "legacy")]
                #[allow(irrefutable_let_patterns)]
                if let State::Legacy(idx) = state {
                    if CURRENT.is_set() {
                        CURRENT.with(|inner| {
                            match inner {
                                #[cfg(all(target_os = "linux", feature = "iouring"))]
                                super::Inner::Uring(_) => {
                                    unreachable!("try_unwrap legacy fd with uring runtime")
                                }
                                super::Inner::Legacy(inner) => {
                                    // deregister it from driver(Poll and slab) and close fd
                                    if let Some(idx) = idx {
                                        let mut source = mio::unix::SourceFd(&fd);
                                        let _ = super::legacy::LegacyDriver::deregister(
                                            inner,
                                            *idx,
                                            &mut source,
                                        );
                                    }
                                }
                            }
                        })
                    }
                }
                Ok(fd)
            }
            Err(inner) => Err(Self { inner }),
        }
    }

    #[cfg(windows)]
    /// Try to unwrap Rc, then deregister if registered and return rawfd.
    /// Note: this action will consume self and return rawfd without closing it.
    pub(crate) fn try_unwrap(self) -> Result<RawSocket, Self> {
        match Rc::try_unwrap(self.inner) {
            Ok(mut _inner) => {
                let fd = &mut _inner.fd;
                let state = unsafe { &*_inner.state.get() };

                match state {
                    #[cfg(feature = "legacy")]
                    State::Legacy(idx) => {
                        if CURRENT.is_set() {
                            CURRENT.with(|inner| {
                                match inner {
                                    #[cfg(feature = "iocp")]
                                    super::Inner::Iocp(_) => {}
                                    super::Inner::Legacy(inner) => {
                                        // deregister it from driver(Poll and slab) and close fd
                                        if let Some(idx) = idx {
                                            let _ = super::legacy::LegacyDriver::deregister(
                                                inner, *idx, fd,
                                            );
                                        }
                                    }
                                }
                            })
                        }
                    }
                    #[cfg(feature = "iocp")]
                    State::Iocp(_) => {}
                }
                Ok(fd.socket)
            }
            Err(inner) => Err(Self { inner }),
        }
    }

    #[allow(unused)]
    pub(crate) fn registered_index(&self) -> Option<usize> {
        let state = unsafe { &*self.inner.state.get() };
        match state {
            #[cfg(all(target_os = "linux", feature = "iouring", feature = "poll-io"))]
            State::Uring(UringState::Legacy(s)) => *s,
            #[cfg(all(target_os = "linux", feature = "iouring"))]
            State::Uring(_) => None,
            #[cfg(all(windows, feature = "iocp"))]
            State::Iocp(_) => None,
            #[cfg(feature = "legacy")]
            State::Legacy(s) => *s,
            #[cfg(all(
                not(feature = "legacy"),
                not(all(target_os = "linux", feature = "iouring"))
            ))]
            _ => {
                super::util::feature_panic();
            }
        }
    }

    /// An FD cannot be closed until all in-flight operation have completed.
    /// This prevents bugs where in-flight reads could operate on the incorrect
    /// file descriptor.
    pub(crate) async fn close(self) {
        // Here we only submit close op for uring mode.
        // Fd will be closed when Inner drops for legacy mode.
        #[cfg(all(target_os = "linux", feature = "iouring"))]
        {
            let fd = self.inner.fd;
            let mut this = self;
            #[allow(irrefutable_let_patterns)]
            if let State::Uring(uring_state) = unsafe { &mut *this.inner.state.get() } {
                if Rc::get_mut(&mut this.inner).is_some() {
                    *uring_state = match super::op::Op::close(fd) {
                        Ok(op) => UringState::Closing(op),
                        Err(_) => {
                            let _ = unsafe { std::fs::File::from_raw_fd(fd) };
                            return;
                        }
                    };
                }
                this.inner.closed().await;
            }
        }
    }

    #[cfg(feature = "poll-io")]
    #[inline]
    pub(crate) fn cvt_poll(&mut self) -> io::Result<()> {
        let state = unsafe { &mut *self.inner.state.get() };
        #[cfg(unix)]
        let r = state.cvt_uring_poll(self.inner.fd);
        #[cfg(windows)]
        let r = Ok(());
        r
    }

    #[cfg(feature = "poll-io")]
    #[inline]
    pub(crate) fn cvt_comp(&mut self) -> io::Result<()> {
        let state = unsafe { &mut *self.inner.state.get() };
        #[cfg(unix)]
        let r = state.cvt_comp(self.inner.fd);
        #[cfg(windows)]
        let r = Ok(());
        r
    }
}

#[cfg(all(target_os = "linux", feature = "iouring"))]
impl Inner {
    /// Completes when the FD has been closed.
    /// Should only be called for uring mode.
    async fn closed(&self) {
        use std::task::Poll;

        crate::macros::support::poll_fn(|cx| {
            let state = unsafe { &mut *self.state.get() };

            #[allow(irrefutable_let_patterns)]
            if let State::Uring(uring_state) = state {
                use std::{future::Future, pin::Pin};

                return match uring_state {
                    UringState::Init => {
                        *uring_state = UringState::Waiting(Some(cx.waker().clone()));
                        Poll::Pending
                    }
                    UringState::Waiting(Some(waker)) => {
                        if !waker.will_wake(cx.waker()) {
                            waker.clone_from(cx.waker());
                        }

                        Poll::Pending
                    }
                    UringState::Waiting(None) => {
                        *uring_state = UringState::Waiting(Some(cx.waker().clone()));
                        Poll::Pending
                    }
                    UringState::Closing(op) => {
                        // Nothing to do if the close operation failed.
                        let _ = ready!(Pin::new(op).poll(cx));
                        *uring_state = UringState::Closed;
                        Poll::Ready(())
                    }
                    UringState::Closed => Poll::Ready(()),
                    #[cfg(feature = "poll-io")]
                    UringState::Legacy(_) => Poll::Ready(()),
                };
            }
            Poll::Ready(())
        })
        .await;
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let fd = &mut self.fd;
        let state = unsafe { &mut *self.state.get() };
        #[allow(unreachable_patterns)]
        match state {
            #[cfg(all(target_os = "linux", feature = "iouring"))]
            State::Uring(UringState::Init) | State::Uring(UringState::Waiting(..)) => {
                if super::op::Op::close(*fd).is_err() {
                    let _ = unsafe { std::fs::File::from_raw_fd(*fd) };
                };
            }
            #[cfg(all(windows, feature = "iocp"))]
            State::Iocp(IocpState::Init) | State::Iocp(IocpState::Waiting(..)) => {
                if super::op::Op::close(fd.socket).is_err() {
                    use std::os::windows::io::FromRawHandle;
                    let _ = unsafe { std::fs::File::from_raw_handle(fd.socket as _) };
                };
            }
            #[cfg(feature = "legacy")]
            State::Legacy(idx) => drop_legacy(fd, *idx),
            #[cfg(all(target_os = "linux", feature = "iouring", feature = "poll-io"))]
            State::Uring(UringState::Legacy(idx)) => drop_uring_legacy(*fd, *idx),
            _ => {}
        }
    }
}

#[allow(unused_mut)]
#[cfg(feature = "legacy")]
fn drop_legacy(fd: &mut RawFd, idx: Option<usize>) {
    if CURRENT.is_set() {
        CURRENT.with(|inner| {
            #[cfg(any(all(target_os = "linux", feature = "iouring"), feature = "legacy"))]
            match inner {
                #[cfg(all(target_os = "linux", feature = "iouring"))]
                super::Inner::Uring(_) => {
                    unreachable!("close legacy fd with uring runtime")
                }
                #[cfg(all(windows, feature = "iocp"))]
                super::Inner::Iocp(_) => {
                    unreachable!("close legacy fd with iocp runtime")
                }
                super::Inner::Legacy(inner) => {
                    // deregister it from driver(Poll and slab) and close fd
                    #[cfg(not(windows))]
                    if let Some(idx) = idx {
                        let mut source = mio::unix::SourceFd(fd);
                        let _ = super::legacy::LegacyDriver::deregister(inner, idx, &mut source);
                    }
                    #[cfg(windows)]
                    if let Some(idx) = idx {
                        let _ = super::legacy::LegacyDriver::deregister(inner, idx, fd);
                    }
                }
            }
        })
    }
    #[cfg(all(unix, feature = "legacy"))]
    let _ = unsafe { std::fs::File::from_raw_fd(*fd) };
    #[cfg(all(windows, feature = "legacy"))]
    let _ = unsafe { OwnedSocket::from_raw_socket(fd.socket) };
}

#[cfg(feature = "poll-io")]
fn drop_uring_legacy(fd: RawFd, idx: Option<usize>) {
    if CURRENT.is_set() {
        CURRENT.with(|inner| {
            match inner {
                #[cfg(feature = "legacy")]
                super::Inner::Legacy(_) => {
                    unreachable!("close uring fd with legacy runtime")
                }
                #[cfg(all(windows, feature = "iocp"))]
                super::Inner::Iocp(inner) => {}
                #[cfg(all(target_os = "linux", feature = "iouring"))]
                super::Inner::Uring(inner) => {
                    // deregister it from driver(Poll and slab) and close fd
                    if let Some(idx) = idx {
                        let mut source = mio::unix::SourceFd(&fd);
                        let _ = super::IoUringDriver::deregister_poll_io(inner, &mut source, idx);
                    }
                }
            }
        })
    }
    #[cfg(unix)]
    let _ = unsafe { std::fs::File::from_raw_fd(fd) };
    #[cfg(windows)]
    let _ = unsafe { OwnedSocket::from_raw_socket(fd.socket) };
}
