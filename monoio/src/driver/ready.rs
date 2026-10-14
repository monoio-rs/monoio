//! Copied from tokio.
//! Ready and Interest.

use std::{fmt, ops};

const READABLE: u8 = 0b0_01;
const WRITABLE: u8 = 0b0_10;
const READ_CLOSED: u8 = 0b0_0100;
const WRITE_CLOSED: u8 = 0b0_1000;
const READ_CANCELED: u8 = 0b01_0000;
const WRITE_CANCELED: u8 = 0b10_0000;

/// Describes the readiness state of an I/O resources.
///
/// `Ready` tracks which operation an I/O resource is ready to perform.
#[cfg_attr(docsrs, doc(cfg(feature = "net")))]
#[derive(Clone, Copy, PartialEq, PartialOrd, Eq)]
pub(crate) struct Ready(u8);

impl Ready {
    /// Returns the empty `Ready` set.
    pub(crate) const EMPTY: Ready = Ready(0);

    /// Returns a `Ready` representing readable readiness.
    pub(crate) const READABLE: Ready = Ready(READABLE);

    /// Returns a `Ready` representing writable readiness.
    pub(crate) const WRITABLE: Ready = Ready(WRITABLE);

    /// Returns a `Ready` representing read closed readiness.
    pub(crate) const READ_CLOSED: Ready = Ready(READ_CLOSED);

    /// Returns a `Ready` representing write closed readiness.
    pub(crate) const WRITE_CLOSED: Ready = Ready(WRITE_CLOSED);

    /// Returns a `Ready` representing read canceled readiness.
    pub(crate) const READ_CANCELED: Ready = Ready(READ_CANCELED);

    /// Returns a `Ready` representing write canceled readiness.
    pub(crate) const WRITE_CANCELED: Ready = Ready(WRITE_CANCELED);

    /// Returns a `Ready` representing read or write canceled readiness.
    pub(crate) const CANCELED: Ready = Ready(READ_CANCELED | WRITE_CANCELED);

    pub(crate) const READ_ALL: Ready = Ready(READABLE | READ_CLOSED | READ_CANCELED);
    pub(crate) const WRITE_ALL: Ready = Ready(WRITABLE | WRITE_CLOSED | WRITE_CANCELED);

    #[cfg(windows)]
    pub(crate) fn from_mio(event: &crate::driver::iocp::Event) -> Ready {
        let mut ready = Ready::EMPTY;

        if event.is_readable() {
            ready |= Ready::READABLE;
        }

        if event.is_writable() {
            ready |= Ready::WRITABLE;
        }

        if event.is_read_closed() {
            ready |= Ready::READ_CLOSED;
        }

        if event.is_write_closed() {
            ready |= Ready::WRITE_CLOSED;
        }

        ready
    }

    #[cfg(unix)]
    // Must remain crate-private to avoid adding a public dependency on Mio.
    pub(crate) fn from_mio(event: &mio::event::Event) -> Ready {
        let mut ready = Ready::EMPTY;

        #[cfg(all(target_os = "freebsd", feature = "legacy"))]
        {
            if event.is_aio() {
                ready |= Ready::READABLE;
            }

            if event.is_lio() {
                ready |= Ready::READABLE;
            }
        }

        if event.is_readable() {
            ready |= Ready::READABLE;
        }

        if event.is_writable() {
            ready |= Ready::WRITABLE;
        }

        if event.is_read_closed() {
            ready |= Ready::READ_CLOSED;
        }

        if event.is_write_closed() {
            ready |= Ready::WRITE_CLOSED;
        }

        ready
    }

    /// Returns true if `Ready` is the empty set.
    pub(crate) fn is_empty(self) -> bool {
        self == Ready::EMPTY
    }

    /// Returns `true` if the value includes `readable`.
    pub(crate) fn is_readable(self) -> bool {
        !(self & Ready::READ_ALL).is_empty()
    }

    /// Returns `true` if the value includes writable `readiness`.
    pub(crate) fn is_writable(self) -> bool {
        !(self & Ready::WRITE_ALL).is_empty()
    }

    /// Returns `true` if the value includes read-closed `readiness`.
    pub(crate) fn is_read_closed(self) -> bool {
        self.contains(Ready::READ_CLOSED)
    }

    /// Returns `true` if the value includes write-closed `readiness`.
    pub(crate) fn is_write_closed(self) -> bool {
        self.contains(Ready::WRITE_CLOSED)
    }

    #[allow(dead_code)]
    pub(crate) fn is_canceled(self) -> bool {
        !(self & Ready::CANCELED).is_empty()
    }

    /// Returns true if `self` is a superset of `other`.
    ///
    /// `other` may represent more than one readiness operations, in which case
    /// the function only returns true if `self` contains all readiness
    /// specified in `other`.
    pub(crate) fn contains<T: Into<Self>>(self, other: T) -> bool {
        let other = other.into();
        (self & other) == other
    }
}

/// Readiness event interest.
///
/// Specifies the readiness events the caller is interested in when awaiting on
/// I/O resource readiness states.
#[derive(Clone, Copy, Eq, PartialEq)]
// TODO: Do we need to remove it?
#[allow(dead_code)]
pub(crate) struct Interest(mio::Interest);

impl Interest {
    /// Add together two `Interest` values.
    ///
    /// This function works from a `const` context.
    #[allow(dead_code)]
    pub(crate) const fn add(self, other: Interest) -> Interest {
        Interest(self.0.add(other.0))
    }
}

impl ops::BitOr for Interest {
    type Output = Self;

    #[inline]
    fn bitor(self, other: Self) -> Self {
        self.add(other)
    }
}

impl ops::BitOrAssign for Interest {
    #[inline]
    fn bitor_assign(&mut self, other: Self) {
        self.0 = (*self | other).0;
    }
}

impl fmt::Debug for Interest {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(fmt)
    }
}

impl ops::BitOr<Ready> for Ready {
    type Output = Ready;

    #[inline]
    fn bitor(self, other: Ready) -> Ready {
        Ready(self.0 | other.0)
    }
}

impl ops::BitOrAssign<Ready> for Ready {
    #[inline]
    fn bitor_assign(&mut self, other: Ready) {
        self.0 |= other.0;
    }
}

impl ops::BitAnd<Ready> for Ready {
    type Output = Ready;

    #[inline]
    fn bitand(self, other: Ready) -> Ready {
        Ready(self.0 & other.0)
    }
}

impl ops::Sub<Ready> for Ready {
    type Output = Ready;

    #[inline]
    fn sub(self, other: Ready) -> Ready {
        Ready(self.0 & !other.0)
    }
}

impl fmt::Debug for Ready {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Ready")
            .field("is_readable", &self.is_readable())
            .field("is_writable", &self.is_writable())
            .field("is_read_closed", &self.is_read_closed())
            .field("is_write_closed", &self.is_write_closed())
            .finish()
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Copy, Hash)]
pub(crate) enum Direction {
    Read,
    Write,
}

impl Direction {
    pub(crate) fn mask(self) -> Ready {
        match self {
            Direction::Read => Ready::READABLE | Ready::READ_CLOSED | Ready::READ_CANCELED,
            Direction::Write => Ready::WRITABLE | Ready::WRITE_CLOSED | Ready::WRITE_CANCELED,
        }
    }
}

#[allow(dead_code)]
pub(crate) const RW_INTERESTS: mio::Interest = mio::Interest::READABLE.add(mio::Interest::WRITABLE);
