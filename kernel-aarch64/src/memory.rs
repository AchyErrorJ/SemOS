//! Unified 4 KiB physical frame pool for the aarch64 kernel.
//!
//! This is a simple bitmap allocator over the RAM region that QEMU `-M virt`
//! leaves free after the kernel image. Security-tier separation is left for a
//! later phase; for now all frames come from one pool.

use core::sync::atomic::{AtomicBool, Ordering};

const FRAME_SIZE: usize = 4096;
const MAX_FRAMES: usize = 32_768; // 128 MiB worth of 4 KiB frames
const BITMAP_WORDS: usize = MAX_FRAMES / 64;

/// The physical frame pool.
struct FramePool {
    base: u64,
    frame_count: usize,
    bitmap: [u64; BITMAP_WORDS],
    allocated: usize,
}

impl FramePool {
    const fn empty() -> Self {
        Self {
            base: 0,
            frame_count: 0,
            bitmap: [0; BITMAP_WORDS],
            allocated: 0,
        }
    }
}

static mut POOL: FramePool = FramePool::empty();
static POOL_INIT: AtomicBool = AtomicBool::new(false);

/// Initialize the pool over `[base, base+size)`.
/// `base` is rounded up to a 4 KiB boundary and `size` is truncated.
pub unsafe fn init(base: u64, size: usize) {
    let aligned_base = (base + FRAME_SIZE as u64 - 1) & !(FRAME_SIZE as u64 - 1);
    let delta = (aligned_base - base) as usize;
    let size = size.saturating_sub(delta);
    let mut frame_count = size / FRAME_SIZE;
    if frame_count > MAX_FRAMES {
        frame_count = MAX_FRAMES;
    }
    let pool = &raw mut POOL;
    (*pool).base = aligned_base;
    (*pool).frame_count = frame_count;
    (*pool).bitmap = [0; BITMAP_WORDS];
    (*pool).allocated = 0;
    POOL_INIT.store(true, Ordering::Release);
}

/// Allocate a single 4 KiB physical frame. Returns its physical address, or
/// `None` if the pool is exhausted.
pub unsafe fn alloc() -> Option<u64> {
    if !POOL_INIT.load(Ordering::Acquire) {
        return None;
    }
    let pool = &raw mut POOL;
    for word in 0..BITMAP_WORDS {
        let bits = (*pool).bitmap[word];
        if bits == u64::MAX {
            continue;
        }
        let bit = bits.trailing_ones() as usize;
        if bit >= 64 {
            continue;
        }
        let idx = word * 64 + bit;
        if idx >= (*pool).frame_count {
            return None;
        }
        (*pool).bitmap[word] |= 1 << bit;
        (*pool).allocated += 1;
        return Some((*pool).base + (idx * FRAME_SIZE) as u64);
    }
    None
}

/// Free a previously-allocated frame. Returns `true` if the address was inside
/// the pool and allocated.
pub unsafe fn free(addr: u64) -> bool {
    if !POOL_INIT.load(Ordering::Acquire) {
        return false;
    }
    let pool = &raw mut POOL;
    if addr < (*pool).base {
        return false;
    }
    let offset = (addr - (*pool).base) as usize;
    let idx = offset / FRAME_SIZE;
    if idx >= (*pool).frame_count || offset % FRAME_SIZE != 0 {
        return false;
    }
    let word = idx / 64;
    let bit = idx % 64;
    let mask = 1u64 << bit;
    if ((*pool).bitmap[word] & mask) == 0 {
        return false;
    }
    (*pool).bitmap[word] &= !mask;
    (*pool).allocated -= 1;
    true
}

/// Return `(total_frames, used_frames, free_frames)`.
pub fn stats() -> (usize, usize, usize) {
    unsafe {
        let pool = &raw const POOL;
        let total = (*pool).frame_count;
        let used = (*pool).allocated;
        (total, used, total.saturating_sub(used))
    }
}
