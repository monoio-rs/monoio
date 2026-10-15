//! Network related
//! Currently, TCP/UnixStream/UnixDatagram are implemented.

mod listener_config;
pub mod tcp;
pub mod udp;
#[cfg(unix)]
pub mod unix;

pub use listener_config::ListenerOpts;
#[deprecated(since = "0.2.0", note = "use ListenerOpts")]
pub use listener_config::ListenerOpts as ListenerConfig;
pub use tcp::{TcpConnectOpts, TcpListener, TcpStream};
#[cfg(unix)]
pub use unix::{Pipe, UnixDatagram, UnixListener, UnixStream};
#[cfg(windows)]
use {
    std::os::windows::prelude::RawSocket,
    windows_sys::Win32::{
        Foundation::NO_ERROR,
        Networking::WinSock::{
            closesocket, ioctlsocket, socket, WSACleanup, WSAStartup, ADDRESS_FAMILY, FIONBIO,
            INVALID_SOCKET, WINSOCK_SOCKET_TYPE,
        },
    },
};

// Copied from mio.
#[cfg(unix)]
pub(crate) async fn new_socket(
    domain: socket2::Domain,
    socket_type: socket2::Type,
) -> std::io::Result<libc::c_int> {
    #[cfg(unix)]
    use crate::driver::op::Op;

    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "illumos",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    let socket_type = socket_type.cloexec().nonblocking();

    #[cfg(target_os = "linux")]
    let socket_type = {
        if crate::driver::op::is_legacy() {
            socket_type.cloexec().nonblocking()
        } else {
            socket_type.cloexec()
        }
    };

    // Gives a warning for platforms without SOCK_NONBLOCK.
    #[allow(clippy::let_and_return)]
    #[cfg(unix)]
    let socket = Op::socket(domain, socket_type, None)?
        .await
        .meta
        .result
        .map(|s| s.into_inner() as i32);

    // Mimic `libstd` and set `SO_NOSIGPIPE` on apple systems.
    #[cfg(target_vendor = "apple")]
    let socket = socket.and_then(|socket| {
        crate::syscall!(setsockopt@RAW(
            socket,
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            &1 as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t
        ))
        .map(|_| socket)
    });

    // Darwin doesn't have SOCK_NONBLOCK or SOCK_CLOEXEC.
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    let socket = socket.and_then(|socket| {
        // For platforms that don't support flags in socket, we need to
        // set the flags ourselves.
        crate::syscall!(fcntl@RAW(socket, libc::F_SETFL, libc::O_NONBLOCK))
            .and_then(|_| {
                crate::syscall!(fcntl@RAW(socket, libc::F_SETFD, libc::FD_CLOEXEC)).map(|_| socket)
            })
            .inspect_err(|_| {
                // If either of the `fcntl` calls failed, ensure the socket is
                // closed and return the error.
                let _ = crate::syscall!(close@RAW(socket));
            })
    });

    socket
}

#[allow(non_snake_case, missing_docs)]
#[cfg(windows)]
#[inline]
pub fn MAKEWORD(a: u8, b: u8) -> u16 {
    (a as u16) | ((b as u16) << 8)
}

#[cfg(windows)]
pub(crate) fn new_socket(
    domain: ADDRESS_FAMILY,
    socket_type: WINSOCK_SOCKET_TYPE,
) -> std::io::Result<RawSocket> {
    let _: i32 = crate::syscall!(
        WSAStartup@RAW(MAKEWORD(2, 2), std::ptr::null_mut()),
        PartialEq::eq,
        NO_ERROR as _
    )?;
    let socket = crate::syscall!(
        socket@RAW(domain as _, socket_type, 0),
        PartialEq::eq,
        INVALID_SOCKET
    )?;
    crate::syscall!(
        ioctlsocket@RAW(socket, FIONBIO, &mut 1),
        PartialEq::ne,
        NO_ERROR as _
    )
    .map(|_: i32| socket as RawSocket)
    .inspect_err(|_| {
        // If either of the `ioctlsocket` calls failed, ensure the socket is
        // closed and return the error.
        unsafe {
            closesocket(socket);
            WSACleanup();
        }
    })
}
