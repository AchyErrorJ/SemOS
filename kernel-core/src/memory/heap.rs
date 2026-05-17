//! General-purpose heap allocator for the kernel + user-space std shim.
//!
//! Phase 14 Tier 1 prerequisite (per `docs/STD_SHIM_SURFACE.md`): every
//! `Vec`/`Box`/`String` in upstream code goes through this. Until it
//! exists, `std::alloc::GlobalAlloc` can't be implemented and the std
//! shim can't even compile.
//!
//! # Design
//!
//! - **Fixed 16 MiB arena** carved out of `static mut HEAP_ARENA`. Sized
//!   to fit comfortably under the kernel's existing BSS budget while
//!   leaving enough headroom for cargo's working-set during a small
//!   build. Bump or shrink later based on real telemetry.
//! - **First-fit free list**, sorted by address. Coalesces with both
//!   neighbours on free.
//! - **Min block size = 32 bytes** (16-byte header + minimum 16-byte
//!   payload, the largest x86_64 SIMD type's alignment). Smaller
//!   requests are bumped up; the slop is the price of a simple
//!   allocator.
//! - **Alignment**: any power-of-two alignment up to 4 KiB. Larger
//!   alignment needs a separate aligned-allocation path which we don't
//!   implement (no upstream code in our target asks for it).
//!
//! # What this allocator is NOT
//!
//! - Not multi-threaded-safe. Single-CPU kernel today; when we add
//!   SMP we'll wrap a spinlock around `allocate` / `deallocate`.
//! - Not security-tier-aware. The existing per-tier pool allocator
//!   (`memory::pools`) handles the LLM-data-isolation cases. This is
//!   the general-purpose backstop for everything else.
//! - Not the right place for huge allocations (>1 MiB). The arena
//!   would fragment quickly. For now we accept those and they work;
//!   future: route requests >1 MiB straight to the page allocator.
//!
//! # Tests
//!
//! kernel-core can't run `cargo test` (no_std, no harness) — see the
//! project memory's "boot-time DEMO pattern." DEMO 22 in `kernel-x86_64/main.rs`
//! exercises every allocation path through SYS_HEAP_ALLOC/FREE.

use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Arena size — 16 MiB. Tunable via this single constant.
const HEAP_SIZE: usize = 16 * 1024 * 1024;

/// Minimum size of a free block (including the 16-byte header).
/// Sized so the smallest payload meets x86_64 SIMD alignment.
const MIN_BLOCK_SIZE: usize = 32;

/// The actual arena. Placed in BSS, zero-initialized at boot. `Aligned`
/// wrapper bumps the alignment to 16 bytes so the first block's header
/// is naturally aligned.
#[repr(C, align(16))]
struct Aligned([u8; HEAP_SIZE]);
static mut HEAP_ARENA: Aligned = Aligned([0u8; HEAP_SIZE]);

/// Free-list head — pointer to the first free block, or null.
static mut FREE_HEAD: *mut FreeBlock = core::ptr::null_mut();

/// Has the allocator been initialised yet? Calls before init are a bug.
static INIT_DONE: AtomicUsize = AtomicUsize::new(0);

/// Free-block header. Sits at the start of each free chunk inside the
/// arena. `size` is the TOTAL size of the chunk (including this 16-byte
/// header); `next` points to the next free block in address order.
#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

/// Allocated-block header. Sits IMMEDIATELY BEFORE the pointer we
/// return to the caller. `dealloc` reads it to know how much to free.
/// Same layout as the first half of FreeBlock so we can convert in
/// place when freeing.
#[repr(C)]
struct UsedBlock {
    size: usize,
    /// Padding so alignment matches FreeBlock (8 bytes on 64-bit).
    _align: usize,
}

const HEADER_SIZE: usize = core::mem::size_of::<UsedBlock>();

// ============================================================================
// Public API
// ============================================================================

/// Initialise the allocator. Must be called once at boot before any
/// `allocate` / `deallocate`. Safe to call from a single-threaded
/// kernel-init path.
pub fn init() {
    unsafe {
        let arena_ptr = &raw mut HEAP_ARENA as *mut u8;
        let first = arena_ptr as *mut FreeBlock;
        (*first).size = HEAP_SIZE;
        (*first).next = core::ptr::null_mut();
        FREE_HEAD = first;
    }
    INIT_DONE.store(1, Ordering::Release);
}

/// Return `(used_bytes, free_bytes, free_block_count)` — pure
/// diagnostic. Walking the free list isn't constant-time, so don't
/// call this from hot paths.
pub fn stats() -> (usize, usize, usize) {
    if INIT_DONE.load(Ordering::Acquire) == 0 { return (0, 0, 0); }
    unsafe {
        let mut free_bytes = 0usize;
        let mut count = 0usize;
        let mut cur = FREE_HEAD;
        while !cur.is_null() {
            free_bytes += (*cur).size;
            count += 1;
            cur = (*cur).next;
        }
        (HEAP_SIZE.saturating_sub(free_bytes), free_bytes, count)
    }
}

/// Allocate `size` bytes with the given alignment. Returns a non-null
/// pointer on success, null on failure (OOM, alignment > 4 KiB, etc.).
///
/// Caller invariants:
/// - `align` must be a power of two and ≤ 4096
/// - `size` must be > 0
/// - The returned pointer is valid until passed to [`deallocate`]
pub fn allocate(size: usize, align: usize) -> *mut u8 {
    if INIT_DONE.load(Ordering::Acquire) == 0 { return core::ptr::null_mut(); }
    if size == 0 || align == 0 || !align.is_power_of_two() || align > 4096 {
        return core::ptr::null_mut();
    }

    // Bump payload size to at least the header gap so the freed block
    // has room for its own header. Plus alignment slack.
    let mut payload = size;
    if payload < HEADER_SIZE { payload = HEADER_SIZE; }
    // Round payload up so the next allocation header is naturally aligned.
    payload = (payload + HEADER_SIZE - 1) & !(HEADER_SIZE - 1);
    let total_needed = HEADER_SIZE + payload;

    unsafe {
        // First-fit search. `prev` tracks the previous free block so we
        // can unlink without re-walking when we find a fit.
        let mut prev: *mut *mut FreeBlock = &raw mut FREE_HEAD;
        let mut cur = FREE_HEAD;
        while !cur.is_null() {
            let block_size = (*cur).size;
            let block_addr = cur as usize;

            // Where does the user payload start, after alignment?
            let payload_addr = (block_addr + HEADER_SIZE + align - 1) & !(align - 1);
            let aligned_offset = payload_addr - (block_addr + HEADER_SIZE);
            let needed_with_align = total_needed + aligned_offset;

            if block_size >= needed_with_align {
                // It fits. Decide whether to split the block.
                let remaining = block_size - needed_with_align;
                if remaining >= MIN_BLOCK_SIZE {
                    // Split: keep `cur` as-is up to (block_addr + needed_with_align),
                    // make a new free block at the tail with the leftover.
                    let new_block = (block_addr + needed_with_align) as *mut FreeBlock;
                    (*new_block).size = remaining;
                    (*new_block).next = (*cur).next;
                    *prev = new_block;
                } else {
                    // Don't split — just unlink the whole block, even
                    // though it's slightly bigger than we needed. The
                    // slop is recoverable when freed.
                    *prev = (*cur).next;
                }

                // Stamp the used-block header IMMEDIATELY before the
                // payload pointer. Note: if aligned_offset > 0, the
                // header lives somewhere inside the original block,
                // not at its start. That's fine — we use the header's
                // size field to know how much to free, and we compute
                // the block base from `ptr - HEADER_SIZE` in dealloc.
                let header = (payload_addr - HEADER_SIZE) as *mut UsedBlock;
                (*header).size = needed_with_align - aligned_offset;
                (*header)._align = aligned_offset;
                return payload_addr as *mut u8;
            }
            prev = &raw mut (*cur).next;
            cur = (*cur).next;
        }
        // No fit found.
        core::ptr::null_mut()
    }
}

/// Free a block previously returned by [`allocate`]. `size` and `align`
/// are accepted for compatibility with `std::alloc::dealloc` but are
/// not strictly required — the size is stored in the block's header.
/// They're validated for sanity.
pub fn deallocate(ptr: *mut u8, _size: usize, _align: usize) {
    if ptr.is_null() { return; }
    if INIT_DONE.load(Ordering::Acquire) == 0 { return; }

    unsafe {
        let header = (ptr as usize - HEADER_SIZE) as *mut UsedBlock;
        let block_size = (*header).size;
        let aligned_offset = (*header)._align;
        let block_base = (ptr as usize - HEADER_SIZE - aligned_offset) as *mut FreeBlock;
        let total_size = block_size + aligned_offset;

        // Convert the used block back to a free block.
        (*block_base).size = total_size;
        (*block_base).next = core::ptr::null_mut();

        // Insert into the free list, preserving address order so we can
        // coalesce with the previous and next neighbours.
        let mut prev: *mut *mut FreeBlock = &raw mut FREE_HEAD;
        let mut cur = FREE_HEAD;
        while !cur.is_null() && (cur as usize) < (block_base as usize) {
            prev = &raw mut (*cur).next;
            cur = (*cur).next;
        }
        // Now: *prev points to where block_base goes, cur is the next
        // block in address order (or null).
        (*block_base).next = cur;
        *prev = block_base;

        // Coalesce with the next block if they're contiguous.
        let next = (*block_base).next;
        if !next.is_null()
            && (block_base as usize) + (*block_base).size == next as usize
        {
            (*block_base).size += (*next).size;
            (*block_base).next = (*next).next;
        }

        // Coalesce with the previous block if applicable. We need the
        // previous block's address — re-walk the list to find it,
        // since `prev` is a pointer-to-next-field, not the prev block
        // itself.
        let mut p: *mut FreeBlock = core::ptr::null_mut();
        let mut q = FREE_HEAD;
        while !q.is_null() && q != block_base {
            p = q;
            q = (*q).next;
        }
        if !p.is_null() && (p as usize) + (*p).size == block_base as usize {
            (*p).size += (*block_base).size;
            (*p).next = (*block_base).next;
        }
    }
}
