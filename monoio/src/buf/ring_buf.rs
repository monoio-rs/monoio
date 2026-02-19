//! Buffer ring support for io_uring provided buffers.

use std::{
    cell::UnsafeCell,
    io,
    marker::PhantomData,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    ops::{Deref, DerefMut},
    ptr::NonNull,
    sync::atomic::{AtomicU16, Ordering},
};

#[cfg(all(target_os = "linux", feature = "iouring"))]
pub(super) use io_uring::types::BufRingEntry;

mod sealed {
    pub trait Sealed {}
}

/// Trait for buffer ring implementations.
///
/// This trait is sealed and cannot be implemented outside of this crate.
pub trait RingBuf: sealed::Sealed {
    /// Buffer type returned by this ring.
    type Buffer<'a>:
    where
        Self: 'a;

    /// Buffer group ID.
    fn bgid(&self) -> u16;
    /// Size of each buffer.
    fn buffer_size(&self) -> usize;
    /// Number of buffers.
    fn buffer_count(&self) -> usize;

    /// # Safety
    /// buf_id and len must come from a valid io_uring completion.
    unsafe fn get_buf(&self, buf_id: u16, len: usize) -> Self::Buffer<'_>;
}

/// Buffer ring with recvmsg address parsing support.
pub trait RecvMsgRingBuf: RingBuf {
    /// Parser type for address extraction.
    type Parser: RecvMsgParser + Unpin + 'static;

    /// Parse a recvmsg completion and return the address and payload buffer.
    ///
    /// # Safety
    /// buf_id and len must come from a valid io_uring recvmsg completion.
    unsafe fn parse_recvmsg(
        &self,
        buf_id: u16,
        len: usize,
        msghdr: &libc::msghdr,
    ) -> io::Result<(<Self::Parser as RecvMsgParser>::Address, Self::Buffer<'_>)>;
}

const RECVMSG_OUT_HEADER_SIZE: usize = std::mem::size_of::<[u32; 4]>();

/// Trait for parsing recvmsg address headers.
///
/// This trait is sealed and cannot be implemented outside of this crate.
pub trait RecvMsgParser: sealed::Sealed {
    /// Parsed address type.
    type Address;
    /// Size of sockaddr structure.
    const NAME_LEN: libc::socklen_t;

    /// Parse address from raw data.
    fn parse_address(name_data: &[u8]) -> io::Result<Self::Address>;

    /// Minimum buffer size for headers.
    #[inline]
    fn min_buffer_size() -> usize {
        RECVMSG_OUT_HEADER_SIZE + Self::NAME_LEN as usize
    }
}

/// IPv4 address parser.
pub struct Ipv4RecvMsgParser;

impl sealed::Sealed for Ipv4RecvMsgParser {}

impl RecvMsgParser for Ipv4RecvMsgParser {
    type Address = SocketAddrV4;

    const NAME_LEN: libc::socklen_t = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;

    fn parse_address(name_data: &[u8]) -> io::Result<Self::Address> {
        assert!(name_data.len() >= Self::NAME_LEN as usize);
        let sockaddr: &libc::sockaddr_in =
            unsafe { &*(name_data.as_ptr() as *const libc::sockaddr_in) };
        let ip = Ipv4Addr::from(sockaddr.sin_addr.s_addr.to_ne_bytes());
        let port = u16::from_be(sockaddr.sin_port);
        Ok(SocketAddrV4::new(ip, port))
    }
}

/// IPv6 address parser.
pub struct Ipv6RecvMsgParser;

impl sealed::Sealed for Ipv6RecvMsgParser {}

impl RecvMsgParser for Ipv6RecvMsgParser {
    type Address = SocketAddrV6;

    const NAME_LEN: libc::socklen_t = std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t;

    fn parse_address(name_data: &[u8]) -> io::Result<Self::Address> {
        assert!(name_data.len() >= Self::NAME_LEN as usize);
        let sockaddr: &libc::sockaddr_in6 =
            unsafe { &*(name_data.as_ptr() as *const libc::sockaddr_in6) };
        let ip = Ipv6Addr::from(sockaddr.sin6_addr.s6_addr);
        let port = u16::from_be(sockaddr.sin6_port);
        Ok(SocketAddrV6::new(
            ip,
            port,
            sockaddr.sin6_flowinfo,
            sockaddr.sin6_scope_id,
        ))
    }
}

/// Parser for any address family.
pub struct AnyRecvMsgParser;

impl sealed::Sealed for AnyRecvMsgParser {}

impl RecvMsgParser for AnyRecvMsgParser {
    type Address = SocketAddr;

    const NAME_LEN: libc::socklen_t =
        std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;

    fn parse_address(name_data: &[u8]) -> io::Result<Self::Address> {
        if name_data.len() < 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "address data too short",
            ));
        }

        let family = u16::from_ne_bytes([name_data[0], name_data[1]]);

        match family as i32 {
            libc::AF_INET => Ipv4RecvMsgParser::parse_address(name_data).map(SocketAddr::V4),
            libc::AF_INET6 => Ipv6RecvMsgParser::parse_address(name_data).map(SocketAddr::V6),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported address family: {}", family),
            )),
        }
    }
}

/// Builder for [`UserRingBuf`].
pub struct UserRingBufBuilder {
    buffer_count: u16,
    buffer_size: usize,
    group_id: u16,
}

impl Default for UserRingBufBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl UserRingBufBuilder {
    /// Create new builder with defaults (16 buffers, 1500 bytes, group 0).
    pub fn new() -> Self {
        Self {
            buffer_count: 16,
            buffer_size: 1500,
            group_id: 0,
        }
    }

    /// Set buffer count.
    pub fn buffer_count(mut self, count: u16) -> Self {
        self.buffer_count = count;
        self
    }

    /// Set buffer size.
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size;
        self
    }

    /// Set buffer group ID.
    pub fn group_id(mut self, id: u16) -> Self {
        self.group_id = id;
        self
    }

    /// Build and register the buffer ring.
    pub fn build(self) -> io::Result<UserRingBuf> {
        let mut ring = UserRingBuf::new(self.buffer_count, self.buffer_size as u32, self.group_id)?;
        ring.register()?;
        Ok(ring)
    }
}

/// io_uring provided buffer ring.
///
/// The ring entry array is allocated via `mmap` with `MAP_ANONYMOUS | MAP_PRIVATE | MAP_POPULATE`
/// to ensure the memory is page-aligned.
///
/// Automatically unregisters from io_uring and unmaps memory on drop.
pub struct UserRingBuf {
    data: UnsafeCell<Box<[u8]>>,
    buf_count: u16,
    buf_ring_ptr: Option<NonNull<BufRingEntry>>,
    ring_entries: u16,
    buf_len: usize,
    shared_tail: NonNull<AtomicU16>,
    buf_group_id: u16,
    ring_mmap_size: usize,
    registered: bool,
}

impl UserRingBuf {
    fn new(buf_count: u16, buf_size: u32, group_id: u16) -> io::Result<Self> {
        if buf_count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "buffer count must be greater than 0",
            ));
        }
        if buf_size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "buffer size must be greater than 0",
            ));
        }
        if (buf_count & (buf_count - 1)) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "buffer count must be a power of two",
            ));
        }
        let ring_entries = buf_count;
        let entry_size = std::mem::size_of::<BufRingEntry>();
        let ring_mmap_size = entry_size * ring_entries as usize;

        let ring_mem = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                ring_mmap_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANONYMOUS | libc::MAP_PRIVATE | libc::MAP_POPULATE,
                -1,
                0,
            )
        };

        if ring_mem == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }

        let buf_ring_ptr = NonNull::new(ring_mem as *mut BufRingEntry).expect("mmap returned null");

        let buf_len = buf_size as usize;
        let mut data = vec![0u8; buf_count as usize * buf_len].into_boxed_slice();

        for bid in 0..buf_count {
            let ring_idx = bid & (ring_entries - 1);
            let entry = unsafe { &mut *buf_ring_ptr.as_ptr().add(ring_idx as usize) };
            let buf_ptr = data.as_mut_ptr().wrapping_add(bid as usize * buf_len);
            entry.set_addr(buf_ptr as u64);
            entry.set_len(buf_size);
            entry.set_bid(bid);
        }

        let data = UnsafeCell::new(data);

        let shared_tail = unsafe {
            NonNull::new_unchecked(BufRingEntry::tail(buf_ring_ptr.as_ptr()) as *mut AtomicU16)
        };

        unsafe {
            shared_tail.as_ref().store(buf_count, Ordering::Release);
        }

        Ok(Self {
            data,
            buf_count,
            buf_ring_ptr: Some(buf_ring_ptr),
            ring_entries,
            buf_len,
            shared_tail,
            buf_group_id: group_id,
            ring_mmap_size,
            registered: false,
        })
    }

    fn register(&mut self) -> io::Result<()> {
        if self.registered {
            return Ok(());
        }

        let ptr = self.buf_ring_ptr.expect("UserRingBuf already unmapped");
        crate::driver::CURRENT.with(|inner| {
            inner.register_buf_ring(ptr.as_ptr() as u64, self.ring_entries, self.buf_group_id)
        })?;

        self.registered = true;
        Ok(())
    }

    /// Unregister from io_uring. Does nothing if not registered.
    pub fn unregister(&mut self) -> io::Result<()> {
        if self.registered && crate::driver::CURRENT.is_set() {
            crate::driver::CURRENT.with(|inner| inner.unregister_buf_ring(self.buf_group_id))?;
            self.registered = false;
        }
        Ok(())
    }

    /// Buffer group ID.
    #[inline]
    pub fn bgid(&self) -> u16 {
        self.buf_group_id
    }

    /// # Safety
    /// buf_id and len must come from a valid io_uring completion.
    #[inline]
    pub(crate) unsafe fn get_buf(&self, buf_id: u16, len: usize) -> RawBuffer<'_> {
        debug_assert!(buf_id < self.buf_count);
        debug_assert!(len <= self.buf_len);

        RawBuffer {
            buf_id,
            offset: 0,
            len,
            ring: self,
        }
    }

    /// # Safety
    /// buf_id, offset, and len must come from a valid io_uring completion.
    #[inline]
    unsafe fn get_buf_offset(&self, buf_id: u16, offset: usize, len: usize) -> RawBuffer<'_> {
        debug_assert!(buf_id < self.buf_count);
        debug_assert!(offset + len <= self.buf_len);

        RawBuffer {
            buf_id,
            offset,
            len,
            ring: self,
        }
    }

    #[inline]
    fn return_buffer(&self, buf_id: u16) {
        let ptr = self.buf_ring_ptr.expect("UserRingBuf already unmapped");
        let tail = unsafe { self.shared_tail.as_ref() };
        let local_tail = tail.load(Ordering::Relaxed);
        let ring_idx = local_tail & (self.ring_entries - 1);

        let entry = unsafe { &mut *ptr.as_ptr().add(ring_idx as usize) };
        let buf_ptr = unsafe {
            (*self.data.get())
                .as_ptr()
                .add(buf_id as usize * self.buf_len)
        };
        entry.set_addr(buf_ptr as u64);
        entry.set_len(self.buf_len as u32);
        entry.set_bid(buf_id);

        tail.store(local_tail.wrapping_add(1), Ordering::Release);
    }

    /// Number of buffers in the ring.
    #[inline]
    pub fn buffer_count(&self) -> usize {
        self.buf_count as usize
    }

    /// Size of each buffer.
    #[inline]
    pub fn buffer_size(&self) -> usize {
        self.buf_len
    }
}

impl Drop for UserRingBuf {
    fn drop(&mut self) {
        let _ = self.unregister();

        if let Some(ptr) = self.buf_ring_ptr.take() {
            unsafe { libc::munmap(ptr.as_ptr() as *mut libc::c_void, self.ring_mmap_size) };
        }
    }
}

impl sealed::Sealed for UserRingBuf {}

impl RingBuf for UserRingBuf {
    type Buffer<'a> = RawBuffer<'a>;

    #[inline]
    fn bgid(&self) -> u16 {
        self.buf_group_id
    }

    #[inline]
    fn buffer_size(&self) -> usize {
        self.buf_len
    }

    #[inline]
    fn buffer_count(&self) -> usize {
        self.buf_count as usize
    }

    #[inline]
    unsafe fn get_buf(&self, buf_id: u16, len: usize) -> RawBuffer<'_> {
        UserRingBuf::get_buf(self, buf_id, len)
    }
}

/// Buffer borrowed from a [`UserRingBuf`].
pub struct RawBuffer<'ring> {
    buf_id: u16,
    offset: usize,
    len: usize,
    ring: &'ring UserRingBuf,
}

impl RawBuffer<'_> {
    /// Buffer ID.
    #[inline]
    pub fn id(&self) -> u16 {
        self.buf_id
    }

    /// Data length.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the buffer is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Deref for RawBuffer<'_> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        let start = self.buf_id as usize * self.ring.buf_len + self.offset;
        unsafe { &(*self.ring.data.get())[start..start + self.len] }
    }
}

impl DerefMut for RawBuffer<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        let start = self.buf_id as usize * self.ring.buf_len + self.offset;
        unsafe { &mut (*self.ring.data.get())[start..start + self.len] }
    }
}

impl AsRef<[u8]> for RawBuffer<'_> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.deref()
    }
}

impl AsMut<[u8]> for RawBuffer<'_> {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        self.deref_mut()
    }
}

impl std::fmt::Debug for RawBuffer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawBuffer")
            .field("buf_id", &self.buf_id)
            .field("len", &self.len)
            .finish()
    }
}

impl Drop for RawBuffer<'_> {
    #[inline]
    fn drop(&mut self) {
        self.ring.return_buffer(self.buf_id);
    }
}

/// Buffer ring for recvmsg with address parsing. See [`UserRingBuf`] for cleanup.
pub struct UserRecvMsgRingBuf<P: RecvMsgParser> {
    inner: UserRingBuf,
    _parser: PhantomData<P>,
}

impl<P: RecvMsgParser> UserRecvMsgRingBuf<P> {
    /// Create new recvmsg buffer ring.
    pub fn new(buf_count: u16, payload_size: usize, group_id: u16) -> io::Result<Self> {
        let total_size = P::min_buffer_size() + payload_size;
        let mut inner = UserRingBuf::new(buf_count, total_size as u32, group_id)?;
        inner.register()?;
        Ok(Self {
            inner,
            _parser: PhantomData,
        })
    }

    /// Underlying buffer ring.
    #[inline]
    pub fn inner(&self) -> &UserRingBuf {
        &self.inner
    }

    /// Buffer group ID.
    #[inline]
    pub fn bgid(&self) -> u16 {
        self.inner.bgid()
    }

    /// Size of each buffer.
    #[inline]
    pub fn buffer_size(&self) -> usize {
        self.inner.buffer_size()
    }

    /// Number of buffers.
    #[inline]
    pub fn buffer_count(&self) -> usize {
        self.inner.buffer_count()
    }

    /// Unregister from io_uring. Does nothing if not registered.
    pub fn unregister(&mut self) -> io::Result<()> {
        self.inner.unregister()
    }
}

impl<P: RecvMsgParser> sealed::Sealed for UserRecvMsgRingBuf<P> {}

impl<P: RecvMsgParser> RingBuf for UserRecvMsgRingBuf<P> {
    type Buffer<'a>
        = RawBuffer<'a>
    where
        P: 'a;

    #[inline]
    fn bgid(&self) -> u16 {
        self.inner.bgid()
    }

    #[inline]
    fn buffer_size(&self) -> usize {
        self.inner.buffer_size()
    }

    #[inline]
    fn buffer_count(&self) -> usize {
        self.inner.buffer_count()
    }

    #[inline]
    unsafe fn get_buf(&self, buf_id: u16, len: usize) -> RawBuffer<'_> {
        self.inner.get_buf(buf_id, len)
    }
}

impl<P: RecvMsgParser + Unpin + 'static> RecvMsgRingBuf for UserRecvMsgRingBuf<P> {
    type Parser = P;

    #[inline]
    unsafe fn parse_recvmsg(
        &self,
        buf_id: u16,
        len: usize,
        msghdr: &libc::msghdr,
    ) -> io::Result<(P::Address, RawBuffer<'_>)> {
        debug_assert!(buf_id < self.inner.buf_count);
        debug_assert!(len <= self.inner.buf_len);

        let start = buf_id as usize * self.inner.buf_len;
        let raw = &(*self.inner.data.get())[start..start + len];
        let parsed = io_uring::types::RecvMsgOut::parse(raw, msghdr)
            .map_err(|()| io::Error::new(io::ErrorKind::InvalidData, "failed to parse recvmsg"))?;
        let addr = P::parse_address(parsed.name_data())?;
        let payload_len = parsed.payload_data().len();
        let header_len = len - payload_len;
        Ok((
            addr,
            self.inner.get_buf_offset(buf_id, header_len, payload_len),
        ))
    }
}
