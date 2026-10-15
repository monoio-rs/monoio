use libc::mode_t;

use crate::fs::{file_type::FileType, permissions::FilePermissions};

pub(crate) struct FileAttr {
    #[cfg(target_os = "linux")]
    pub(crate) stat: libc::stat64,
    #[cfg(target_os = "macos")]
    pub(crate) stat: libc::stat,
    #[cfg(target_os = "linux")]
    pub(crate) statx_extra_fields: Option<StatxExtraFields>,
}

#[cfg(unix)]
impl FileAttr {
    pub(crate) fn size(&self) -> u64 {
        self.stat.st_size as u64
    }

    pub(crate) fn perm(&self) -> FilePermissions {
        FilePermissions {
            mode: (self.stat.st_mode as mode_t),
        }
    }

    pub(crate) fn file_type(&self) -> FileType {
        FileType {
            mode: self.stat.st_mode as mode_t,
        }
    }
}

/// Extra fields that are available in `statx` struct.
#[cfg(target_os = "linux")]
pub(crate) struct StatxExtraFields {
    pub(crate) stx_mask: u32,
    pub(crate) stx_btime: libc::statx_timestamp,
}

/// Convert a `statx` struct to not platform-specific `FileAttr`.
/// Current implementation is only for Linux.
#[cfg(target_os = "linux")]
impl From<libc::statx> for FileAttr {
    fn from(buf: libc::statx) -> Self {
        let mut stat: libc::stat64 = unsafe { std::mem::zeroed() };

        stat.st_dev = libc::makedev(buf.stx_dev_major, buf.stx_dev_minor) as _;
        stat.st_ino = buf.stx_ino as libc::ino64_t;
        stat.st_nlink = buf.stx_nlink as libc::nlink_t;
        stat.st_mode = buf.stx_mode as libc::mode_t;
        stat.st_uid = buf.stx_uid as libc::uid_t;
        stat.st_gid = buf.stx_gid as libc::gid_t;
        stat.st_rdev = libc::makedev(buf.stx_rdev_major, buf.stx_rdev_minor) as _;
        stat.st_size = buf.stx_size as libc::off64_t;
        stat.st_blksize = buf.stx_blksize as libc::blksize_t;
        stat.st_blocks = buf.stx_blocks as libc::blkcnt64_t;
        stat.st_atime = buf.stx_atime.tv_sec as libc::time_t;
        // `i64` on gnu-x86_64-x32, `c_ulong` otherwise.
        stat.st_atime_nsec = buf.stx_atime.tv_nsec as _;
        stat.st_mtime = buf.stx_mtime.tv_sec as libc::time_t;
        stat.st_mtime_nsec = buf.stx_mtime.tv_nsec as _;
        stat.st_ctime = buf.stx_ctime.tv_sec as libc::time_t;
        stat.st_ctime_nsec = buf.stx_ctime.tv_nsec as _;

        let extra = StatxExtraFields {
            stx_mask: buf.stx_mask,
            stx_btime: buf.stx_btime,
        };

        Self {
            stat,
            statx_extra_fields: Some(extra),
        }
    }
}

#[cfg(target_os = "linux")]
impl From<libc::stat64> for FileAttr {
    fn from(value: libc::stat64) -> Self {
        Self {
            stat: value,
            statx_extra_fields: None,
        }
    }
}

#[cfg(target_os = "macos")]
impl From<libc::stat> for FileAttr {
    fn from(stat: libc::stat) -> Self {
        Self { stat }
    }
}
