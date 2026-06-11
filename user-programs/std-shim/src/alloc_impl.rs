//! User-space heap allocator backed by SYS_MMAP_ANON.
//!
//! Phase 14 M25 #50. The kernel's SYS_HEAP_ALLOC returns kernel-heap
//! pointers that fault on a Ring-3 write, so the std-shim runs its own
//! allocator over USER-accessible memory mmap'd into the process's
//! address space.
//!
//! ## Why a bump allocator
//!
//! The first cut used a hand-rolled first-fit free list. It had a
//! subtle corruption bug (a freed block's stored header back-pointer
//! could read back as a stale value pointing into the code segment,
//! so a later dealloc wrote through it and #PF'd). Rather than keep
//! chasing it, this is a dead-simple bump allocator: alloc advances a
//! pointer, dealloc is a no-op. No free list means nothing to corrupt.
//!
//! Trade-off: freed memory is never reused, so long-running, heavily-
//! allocating workloads (a real rustc compile) would exhaust the heap.
//! That's fine for M25's current scope — short-lived programs whose
//! whole heap is reclaimed by the kernel on exit. A reclaiming
//! allocator (fixed free list, or a vendored `linked_list_allocator`)
//! is a tracked follow-up for when we actually drive rustc.
//!
//! The region grows on demand: when the bump pointer would pass the
//! mapped limit, we mmap another chunk contiguous with the last one.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};
use crate::arch::{SYS_MMAP_ANON, syscall2};

/// User heap base — matches kernel's `paging::user_layout::USER_HEAP_BASE`.
///
/// MUST sit above the loaded binary's highest segment. The old value
/// (12 MiB) sat *inside* a large program's `.text`: semos-rustc's code
/// segment runs 0x400000..~0x5331248 (~83 MiB), so the first heap grow
/// (1 MiB at 0xC00000) mapped RW/NX pages straight over `run_compiler`'s
/// code → `INSTRUCTION_FETCH | PROTECTION_VIOLATION` the instant the
/// program called it (M27 Phase 5b iter 8 root cause). 4 GiB is clear of
/// any binary's code/data (which live at 4 MiB..~100 MiB) and ~508 GiB
/// below the user stacks (~512 GiB), so the bump region can grow freely.
const USER_HEAP_BASE: u64 = 0x0000_0001_0000_0000; // 4 GiB
/// mmap granularity when growing (1 MiB).
const GROW_CHUNK: u64 = 1024 * 1024;

struct HeapState {
    /// Next address to hand out (bump pointer).
    next: u64,
    /// One past the last mapped byte. `next` may not exceed this.
    end: u64,
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
                next: USER_HEAP_BASE,
                end: USER_HEAP_BASE,
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

    /// Ensure `[next, next+bytes)` is mapped, growing the region via
    /// SYS_MMAP_ANON if needed. Returns false on syscall OOM.
    unsafe fn ensure(&self, st: &mut HeapState, bytes: u64) -> bool {
        if st.next + bytes <= st.end {
            return true;
        }
        // Need to map from st.end upward enough to cover the request.
        let need = st.next + bytes - st.end;
        let chunk = if need > GROW_CHUNK {
            (need + 0xFFF) & !0xFFF
        } else {
            GROW_CHUNK
        };
        if syscall2(SYS_MMAP_ANON, st.end, chunk) == u64::MAX {
            return false;
        }
        st.end += chunk;
        true
    }
}

#[inline]
fn align_up(val: u64, align: u64) -> u64 {
    (val + align - 1) & !(align - 1)
}

unsafe impl GlobalAlloc for SemosAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.acquire();
        let st = &mut *self.state.get();
        let align = layout.align().max(1) as u64;
        let size = (layout.size().max(1) as u64 + 7) & !7; // round to 8

        let addr = align_up(st.next, align);
        if !self.ensure(st, (addr - st.next) + size) {
            self.release();
            return core::ptr::null_mut();
        }
        st.next = addr + size;
        self.release();
        addr as *mut u8
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator: no per-allocation free. Memory is reclaimed
        // wholesale by the kernel when the process exits.
    }
}
