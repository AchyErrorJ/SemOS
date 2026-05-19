//! User-space heap allocator backed by SYS_MMAP_ANON.
//!
//! Phase 14 M25 Tier 2 #50. The kernel's SYS_HEAP_ALLOC returns
//! kernel-heap pointers that fault on a Ring-3 write, so the std-shim
//! can't use it directly. Instead we mmap a USER-accessible region
//! (SYS_MMAP_ANON maps fresh zeroed frames into our own address space)
//! and run a first-fit free-list allocator over it.
//!
//! The region grows on demand: when the free list can't satisfy a
//! request, we mmap another 1 MiB chunk contiguous with the last and
//! add it as a free block.
//!
//! Block layout. Every allocation is carved from a free block whose
//! header (`BlockHeader`) sits at the block base. The returned payload
//! pointer is alignment-adjusted, and the 8 bytes immediately before
//! the payload always hold the block-base pointer so `dealloc` can
//! recover the header unambiguously regardless of alignment padding.
//!
//! Single-threaded today; a spin flag guards the free list so a
//! future threaded program won't corrupt it.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};
use crate::arch::{SYS_MMAP_ANON, syscall2};

/// User heap base — matches kernel's `paging::user_layout::USER_HEAP_BASE`.
const USER_HEAP_BASE: u64 = 0x0000_0000_00C0_0000; // 12 MiB
/// mmap granularity when growing (1 MiB).
const GROW_CHUNK: u64 = 1024 * 1024;

const HEADER_SIZE: usize = core::mem::size_of::<BlockHeader>();
/// Back-pointer slot size stored just before each payload.
const BACKPTR: usize = core::mem::size_of::<usize>();

/// Free-list node / block header at each block's base.
#[repr(C)]
struct BlockHeader {
    /// Payload capacity in bytes (everything after this header).
    size: usize,
    /// Next free block (only valid while on the free list).
    next: *mut BlockHeader,
}

struct HeapState {
    free_head: *mut BlockHeader,
    /// Next virtual address to mmap on growth.
    brk: u64,
}

pub struct SemosAllocator {
    state: UnsafeCell<HeapState>,
    lock: AtomicBool,
}

// SAFETY: all access serialized by the spin `lock`.
unsafe impl Sync for SemosAllocator {}

impl SemosAllocator {
    pub const fn new() -> Self {
        Self {
            state: UnsafeCell::new(HeapState {
                free_head: core::ptr::null_mut(),
                brk: USER_HEAP_BASE,
            }),
            lock: AtomicBool::new(false),
        }
    }

    #[inline]
    fn acquire(&self) {
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    #[inline]
    fn release(&self) {
        self.lock.store(false, Ordering::Release);
    }

    /// mmap a fresh chunk big enough for `min_payload` and prepend it
    /// as one free block. Returns false on syscall OOM.
    unsafe fn grow(&self, st: &mut HeapState, min_payload: usize) -> bool {
        let need = (HEADER_SIZE + min_payload) as u64;
        let chunk = if need > GROW_CHUNK {
            (need + 0xFFF) & !0xFFF
        } else {
            GROW_CHUNK
        };
        let addr = st.brk;
        if syscall2(SYS_MMAP_ANON, addr, chunk) == u64::MAX {
            return false;
        }
        st.brk += chunk;

        let block = addr as *mut BlockHeader;
        (*block).size = chunk as usize - HEADER_SIZE;
        (*block).next = st.free_head;
        st.free_head = block;
        true
    }

    unsafe fn alloc_inner(&self, layout: Layout) -> *mut u8 {
        self.acquire();
        let st = &mut *self.state.get();
        let align = layout.align().max(BACKPTR);
        let want = layout.size().max(1);

        let p = self.carve(st, want, align);
        if !p.is_null() {
            self.release();
            return p;
        }
        // Grow + retry once. Reserve align + backptr slack on top of want.
        if !self.grow(st, want + align + BACKPTR) {
            self.release();
            return core::ptr::null_mut();
        }
        let p = self.carve(st, want, align);
        self.release();
        p
    }

    /// First-fit scan. On success returns an `align`-aligned payload
    /// pointer with the block base stored in the word before it.
    unsafe fn carve(&self, st: &mut HeapState, want: usize, align: usize) -> *mut u8 {
        let mut prev: *mut *mut BlockHeader = &mut st.free_head;
        let mut cur = st.free_head;
        while !cur.is_null() {
            let block_base = cur as usize;
            let block_end = block_base + HEADER_SIZE + (*cur).size;
            // Earliest the payload can start: after the header, with room
            // for the back-pointer slot, then aligned up.
            let payload = align_up(block_base + HEADER_SIZE + BACKPTR, align);
            if payload + want <= block_end {
                let used_end = payload + want;
                let remaining = block_end - used_end;
                if remaining >= HEADER_SIZE + 16 {
                    // Split a tail free block.
                    (*cur).size = (used_end - (block_base + HEADER_SIZE)) as usize;
                    let tail = used_end as *mut BlockHeader;
                    (*tail).size = remaining - HEADER_SIZE;
                    (*tail).next = (*cur).next;
                    *prev = tail;
                } else {
                    *prev = (*cur).next;
                }
                // Stash block base in the word before the payload.
                let slot = (payload - BACKPTR) as *mut usize;
                *slot = block_base;
                return payload as *mut u8;
            }
            prev = &mut (*cur).next;
            cur = (*cur).next;
        }
        core::ptr::null_mut()
    }

    unsafe fn dealloc_inner(&self, ptr: *mut u8, _layout: Layout) {
        if ptr.is_null() {
            return;
        }
        self.acquire();
        let st = &mut *self.state.get();
        // Recover header from the back-pointer slot.
        let slot = (ptr as usize - BACKPTR) as *const usize;
        let header = *slot as *mut BlockHeader;
        // Prepend to free list. No coalescing yet (acceptable for M25
        // workloads; add a merge pass if rustc churn fragments badly).
        (*header).next = st.free_head;
        st.free_head = header;
        self.release();
    }
}

#[inline]
fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

unsafe impl GlobalAlloc for SemosAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.alloc_inner(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.dealloc_inner(ptr, layout)
    }
}
