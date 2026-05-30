//! Detect if current platform support io_uring.

#[cfg(all(target_os = "linux", feature = "iouring"))]
macro_rules! err_to_false {
    ($e: expr) => {
        match $e {
            Ok(x) => x,
            Err(_) => {
                return false;
            }
        }
    };
}
#[cfg(all(target_os = "linux", feature = "iouring"))]
fn detect_uring_inner() -> bool {
    let val = std::env::var("MONOIO_FORCE_LEGACY_DRIVER");
    match val {
        Ok(v) if matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes") => {
            return false;
        }
        _ => {}
    }

    use io_uring::opcode::*;
    auto_array::auto_array!(
        const USED_OP: [u8; _] = [
            Accept::CODE,
            AsyncCancel::CODE,
            Close::CODE,
            Connect::CODE,
            Fsync::CODE,
            #[cfg(feature = "mkdirat")]
            MkDirAt::CODE,
            OpenAt::CODE,
            PollAdd::CODE,
            ProvideBuffers::CODE,
            Read::CODE,
            Readv::CODE,
            Recv::CODE,
            #[cfg(feature = "renameat")]
            RenameAt::CODE,
            Send::CODE,
            SendMsg::CODE,
            RecvMsg::CODE,
            #[cfg(feature = "splice")]
            Splice::CODE,
            Statx::CODE,
            #[cfg(feature = "symlinkat")]
            SymlinkAt::CODE,
            Timeout::CODE,
            #[cfg(feature = "unlinkat")]
            UnlinkAt::CODE,
            Write::CODE,
            Writev::CODE,
        ];
    );

    let uring = err_to_false!(io_uring::IoUring::new(2));
    let mut probe = io_uring::Probe::new();
    err_to_false!(uring.submitter().register_probe(&mut probe));
    USED_OP.iter().all(|op| probe.is_supported(*op))
}

/// Detect if current platform supports our needed uring ops.
#[cfg(all(target_os = "linux", feature = "iouring"))]
pub fn detect_uring() -> bool {
    static mut URING_SUPPORTED: bool = false;
    static INIT: std::sync::Once = std::sync::Once::new();

    unsafe {
        INIT.call_once(|| {
            URING_SUPPORTED = detect_uring_inner();
        });
        URING_SUPPORTED
    }
}

/// Detect if current platform supports our needed uring ops.
#[cfg(not(all(target_os = "linux", feature = "iouring")))]
pub fn detect_uring() -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    #[test]
    fn test_detect() {
        assert!(
            super::detect_uring(),
            "io_uring or ops not supported on current platform"
        )
    }
}
