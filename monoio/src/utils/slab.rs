//! Slab.
//! Part of code and design forked from tokio.

use std::{
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
};

/// Pre-allocated storage for a uniform data type
#[derive(Default)]
pub(crate) struct Slab<T> {
    // pages of continued memory
    pages: [Option<Page<T>>; NUM_PAGES],
    // cached write page id
    w_page_id: usize,
    // current generation
    generation: u32,
}

const NUM_PAGES: usize = 26;
const PAGE_INITIAL_SIZE: usize = 64;
const COMPACT_INTERVAL: u32 = 2048;

impl<T> Slab<T> {
    /// Create a new slab.
    pub(crate) const fn new() -> Slab<T> {
        Slab {
            pages: [
                None, None, None, None, None, None, None, None, None, None, None, None, None, None,
                None, None, None, None, None, None, None, None, None, None, None, None,
            ],
            w_page_id: 0,
            generation: 0,
        }
    }

    /// Get slab len.
    #[allow(unused)]
    pub(crate) fn len(&self) -> usize {
        self.pages.iter().fold(0, |acc, page| match page {
            Some(page) => acc + page.used,
            None => acc,
        })
    }

    pub(crate) fn get(&mut self, key: usize) -> Option<Ref<'_, T>> {
        let page_id = get_page_id(key);
        // here we make 2 mut ref so we must make it safe.
        let slab = unsafe { &mut *(self as *mut Slab<T>) };
        let page = match unsafe { self.pages.get_unchecked_mut(page_id) } {
            Some(page) => page,
            None => return None,
        };
        let index = key - page.prev_len;
        match page.get_entry_mut(index) {
            None => None,
            Some(entry) => match entry {
                Entry::Vacant(_) => None,
                Entry::Occupied(_) => Some(Ref { slab, page, index }),
            },
        }
    }

    /// Insert an element into slab. The key is returned.
    /// Note: If the slab is out of slot, it will panic.
    pub(crate) fn insert(&mut self, val: T) -> usize {
        let begin_id = self.w_page_id;
        for i in begin_id..NUM_PAGES {
            unsafe {
                let page = match self.pages.get_unchecked_mut(i) {
                    Some(page) => page,
                    None => {
                        let page = Page::new(
                            PAGE_INITIAL_SIZE << i,
                            (PAGE_INITIAL_SIZE << i) - PAGE_INITIAL_SIZE,
                        );
                        let r = self.pages.get_unchecked_mut(i);
                        *r = Some(page);
                        r.as_mut().unwrap_unchecked()
                    }
                };
                if let Some(slot) = page.alloc() {
                    page.set(slot, val);
                    self.w_page_id = i;
                    return slot + page.prev_len;
                }
            }
        }
        panic!("out of slot");
    }

    /// Remove an element from slab.
    #[allow(unused)]
    pub(crate) fn remove(&mut self, key: usize) -> Option<T> {
        let page_id = get_page_id(key);
        let page = match unsafe { self.pages.get_unchecked_mut(page_id) } {
            Some(page) => page,
            None => return None,
        };
        let val = page.remove(key - page.prev_len);
        self.mark_remove();
        val
    }

    pub(crate) fn mark_remove(&mut self) {
        // compact
        self.generation = self.generation.wrapping_add(1);
        if self.generation.is_multiple_of(COMPACT_INTERVAL) {
            // reset write page index
            self.w_page_id = 0;
            // find the last allocated page and try to drop
            if let Some((id, last_page)) = self
                .pages
                .iter_mut()
                .enumerate()
                .rev()
                .find_map(|(id, p)| p.as_mut().map(|p| (id, p)))
            {
                if last_page.is_empty() && id > 0 {
                    unsafe {
                        *self.pages.get_unchecked_mut(id) = None;
                    }
                }
            }
        }
    }
}

// Forked from tokio.
fn get_page_id(key: usize) -> usize {
    const POINTER_WIDTH: u32 = std::mem::size_of::<usize>() as u32 * 8;
    const PAGE_INDEX_SHIFT: u32 = PAGE_INITIAL_SIZE.trailing_zeros() + 1;

    let slot_shifted = (key.saturating_add(PAGE_INITIAL_SIZE)) >> PAGE_INDEX_SHIFT;
    ((POINTER_WIDTH - slot_shifted.leading_zeros()) as usize).min(NUM_PAGES - 1)
}

/// Ref point to a valid slot.
pub(crate) struct Ref<'a, T> {
    slab: &'a mut Slab<T>,
    page: &'a mut Page<T>,
    index: usize,
}

impl<T> Ref<'_, T> {
    #[allow(unused)]
    pub(crate) fn remove(self) -> T {
        // # Safety
        // We make sure the index is valid.
        let val = unsafe { self.page.remove(self.index).unwrap_unchecked() };
        self.slab.mark_remove();
        val
    }
}

impl<T> AsRef<T> for Ref<'_, T> {
    fn as_ref(&self) -> &T {
        // # Safety
        // We make sure the index is valid.
        unsafe { self.page.get(self.index).unwrap_unchecked() }
    }
}

impl<T> AsMut<T> for Ref<'_, T> {
    fn as_mut(&mut self) -> &mut T {
        // # Safety
        // We make sure the index is valid.
        unsafe { self.page.get_mut(self.index).unwrap_unchecked() }
    }
}

impl<T> Deref for Ref<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T> DerefMut for Ref<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}

enum Entry<T> {
    Vacant(usize),
    Occupied(T),
}

impl<T> Entry<T> {
    fn as_ref(&self) -> Option<&T> {
        match self {
            Entry::Vacant(_) => None,
            Entry::Occupied(inner) => Some(inner),
        }
    }

    fn as_mut(&mut self) -> Option<&mut T> {
        match self {
            Entry::Vacant(_) => None,
            Entry::Occupied(inner) => Some(inner),
        }
    }

    fn is_vacant(&self) -> bool {
        matches!(self, Entry::Vacant(_))
    }

    unsafe fn unwrap_unchecked(self) -> T { unsafe {
        match self {
            Entry::Vacant(_) => std::hint::unreachable_unchecked(),
            Entry::Occupied(inner) => inner,
        }
    }}
}

struct Page<T> {
    // continued buffer of fixed size
    slots: Box<[MaybeUninit<Entry<T>>]>,
    // number of occupied slots
    used: usize,
    // number of initialized slots
    initialized: usize,
    // next slot to write
    next: usize,
    // sum of previous page's slots count
    prev_len: usize,
}

impl<T> Page<T> {
    fn new(size: usize, prev_len: usize) -> Self {
        let mut buffer = Vec::with_capacity(size);
        unsafe { buffer.set_len(size) };
        let slots = buffer.into_boxed_slice();
        Self {
            slots,
            used: 0,
            initialized: 0,
            next: 0,
            prev_len,
        }
    }

    fn is_empty(&self) -> bool {
        self.used == 0
    }

    fn is_full(&self) -> bool {
        self.used == self.slots.len()
    }

    // alloc a slot
    // Safety: after slot is allocated, the caller must guarantee it will be
    // initialized
    unsafe fn alloc(&mut self) -> Option<usize> { unsafe {
        let next = self.next;
        if self.is_full() {
            // current page is full
            debug_assert_eq!(next, self.slots.len(), "next should eq to slots.len()");
            return None;
        } else if next >= self.initialized {
            // the slot to write is not initialized
            debug_assert_eq!(next, self.initialized, "next should eq to initialized");
            self.initialized += 1;
            self.next += 1;
        } else {
            // the slot has already been initialized
            // it must be Vacant
            let slot = self.slots.get_unchecked(next).assume_init_ref();
            match slot {
                Entry::Vacant(next_slot) => {
                    self.next = *next_slot;
                }
                _ => std::hint::unreachable_unchecked(),
            }
        }
        self.used += 1;
        Some(next)
    }}

    // set value of the slot
    // Safety: the slot must returned by Self::alloc.
    unsafe fn set(&mut self, slot: usize, val: T) { unsafe {
        let slot = self.slots.get_unchecked_mut(slot);
        *slot = MaybeUninit::new(Entry::Occupied(val));
    }}

    fn get(&self, slot: usize) -> Option<&T> {
        if slot >= self.initialized {
            return None;
        }
        unsafe { self.slots.get_unchecked(slot).assume_init_ref() }.as_ref()
    }

    fn get_mut(&mut self, slot: usize) -> Option<&mut T> {
        if slot >= self.initialized {
            return None;
        }
        unsafe { self.slots.get_unchecked_mut(slot).assume_init_mut() }.as_mut()
    }

    fn get_entry_mut(&mut self, slot: usize) -> Option<&mut Entry<T>> {
        if slot >= self.initialized {
            return None;
        }
        unsafe { Some(self.slots.get_unchecked_mut(slot).assume_init_mut()) }
    }

    fn remove(&mut self, slot: usize) -> Option<T> {
        if slot >= self.initialized {
            return None;
        }
        unsafe {
            let slot_mut = self.slots.get_unchecked_mut(slot).assume_init_mut();
            if slot_mut.is_vacant() {
                return None;
            }
            let val = std::mem::replace(slot_mut, Entry::Vacant(self.next));
            self.next = slot;
            self.used -= 1;

            Some(val.unwrap_unchecked())
        }
    }
}

impl<T> Drop for Page<T> {
    fn drop(&mut self) {
        let mut to_drop = std::mem::take(&mut self.slots).into_vec();

        unsafe {
            if self.is_empty() {
                // fast drop if empty
                to_drop.set_len(0);
            } else {
                // slow drop
                to_drop.set_len(self.initialized);
                std::mem::transmute::<Vec<MaybeUninit<Entry<T>>>, Vec<Entry<T>>>(to_drop);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove_one() {
        let mut slab = Slab::default();
        let key = slab.insert(10);
        assert_eq!(slab.get(key).unwrap().as_mut(), &10);
        assert_eq!(slab.remove(key), Some(10));
        assert!(slab.get(key).is_none());
        assert_eq!(slab.len(), 0);
    }

    #[test]
    fn insert_get_remove_many() {
        let mut slab = Slab::new();
        let mut keys = vec![];

        for i in 0..10 {
            for j in 0..10 {
                let val = (i * 10) + j;

                let key = slab.insert(val);
                keys.push((key, val));
                assert_eq!(slab.get(key).unwrap().as_mut(), &val);
            }

            for (key, val) in keys.drain(..) {
                assert_eq!(val, slab.remove(key).unwrap());
            }
        }
    }

    #[test]
    fn get_not_exist() {
        let mut slab = Slab::<i32>::new();
        assert!(slab.get(0).is_none());
        assert!(slab.get(1).is_none());
        assert!(slab.get(usize::MAX).is_none());
        assert!(slab.remove(0).is_none());
        assert!(slab.remove(1).is_none());
        assert!(slab.remove(usize::MAX).is_none());
    }

    #[test]
    fn insert_remove_big() {
        let mut slab = Slab::default();
        let keys = (0..1_000_000).map(|i| slab.insert(i)).collect::<Vec<_>>();
        keys.iter().zip(0..1_000_000).for_each(|(key, val)| {
            assert_eq!(slab.remove(*key).unwrap(), val);
        });
        keys.iter().for_each(|key| {
            assert!(slab.get(*key).is_none());
        });
        assert_eq!(slab.len(), 0);
    }
}
