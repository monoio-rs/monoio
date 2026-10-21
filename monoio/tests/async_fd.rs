#![cfg(unix)]

use std::{
    future::{poll_fn, Future},
    io::{self, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    pin::pin,
    task::Poll,
};

use monoio::io::AsyncFd;

fn pipe() -> io::Result<(OwnedFd, std::fs::File)> {
    let mut descriptors = [-1; 2];
    // SAFETY: pipe initializes exactly the two descriptor slots provided.
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe returned two new, independently owned descriptors.
    let reader = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
    let writer = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
    for fd in [&reader, &writer] {
        // SAFETY: fcntl operates on a live owned descriptor.
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        if flags == -1
            || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((reader, std::fs::File::from(writer)))
}

fn read(fd: &AsyncFd, bytes: &mut [u8]) -> io::Result<usize> {
    // SAFETY: the descriptor and writable buffer remain valid for this call.
    let count = unsafe { libc::read(fd.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
    if count < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(usize::try_from(count).unwrap())
    }
}

#[monoio::test_all]
async fn a_pipe_waits_rearms_and_reports_eof_without_socket_reads() -> io::Result<()> {
    let (reader, mut writer) = pipe()?;
    let reader = AsyncFd::new(reader)?;
    for value in [7, 9] {
        let mut ready = pin!(reader.readable());
        assert!(poll_fn(|cx| Poll::Ready(ready.as_mut().poll(cx)))
            .await
            .is_pending());
        writer.write_all(&[value])?;
        ready.await?;
        let mut byte = [0];
        assert_eq!(read(&reader, &mut byte)?, 1);
        assert_eq!(byte, [value]);
        assert_eq!(
            read(&reader, &mut byte).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
    drop(writer);
    reader.readable().await?;
    assert_eq!(read(&reader, &mut [0])?, 0);
    Ok(())
}

#[cfg(target_os = "linux")]
#[monoio::test_all]
async fn an_eventfd_uses_its_counter_read_after_readiness() -> io::Result<()> {
    // SAFETY: eventfd takes no pointers and returns a new descriptor.
    let raw = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: eventfd just returned this independently owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut writer = std::fs::File::from(fd.try_clone()?);
    let reader = AsyncFd::new(fd)?;
    let mut ready = pin!(reader.readable());
    assert!(poll_fn(|cx| Poll::Ready(ready.as_mut().poll(cx)))
        .await
        .is_pending());
    writer.write_all(&3u64.to_ne_bytes())?;
    ready.await?;
    let mut counter = [0; 8];
    assert_eq!(read(&reader, &mut counter)?, counter.len());
    assert_eq!(u64::from_ne_bytes(counter), 3);
    Ok(())
}

#[cfg(target_os = "macos")]
#[monoio::test_all]
async fn a_kqueue_uses_only_read_interest_and_rearms_after_consuming_an_event() -> io::Result<()> {
    use std::ptr;

    // SAFETY: kqueue takes no arguments and returns a new descriptor.
    let raw = unsafe { libc::kqueue() };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: kqueue just returned this independently owned descriptor.
    let queue = AsyncFd::new(unsafe { OwnedFd::from_raw_fd(raw) })?;
    let mut change = libc::kevent {
        ident: 1,
        filter: libc::EVFILT_USER,
        flags: libc::EV_ADD | libc::EV_CLEAR,
        fflags: 0,
        data: 0,
        udata: ptr::null_mut(),
    };
    // SAFETY: one valid change and no output events or blocking wait.
    if unsafe {
        libc::kevent(
            queue.as_raw_fd(),
            &change,
            1,
            ptr::null_mut(),
            0,
            ptr::null(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    for _ in 0..2 {
        let mut ready = pin!(queue.readable());
        assert!(poll_fn(|cx| Poll::Ready(ready.as_mut().poll(cx)))
            .await
            .is_pending());
        change.flags = 0;
        change.fflags = libc::NOTE_TRIGGER;
        // SAFETY: one valid trigger and no output events or blocking wait.
        if unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                &change,
                1,
                ptr::null_mut(),
                0,
                ptr::null(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        ready.await?;
        let mut event = change;
        let now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: one writable event, no changes, and a zero-time observation.
        let count = unsafe { libc::kevent(queue.as_raw_fd(), ptr::null(), 0, &mut event, 1, &now) };
        assert_eq!(count, 1);
        let ident = event.ident;
        assert_eq!(ident, 1);
        assert_eq!(event.filter, libc::EVFILT_USER);
        assert_eq!(event.flags & libc::EV_ERROR, 0);
    }
    Ok(())
}
