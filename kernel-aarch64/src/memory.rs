//! Physical frame allocator, sized at runtime from the device tree.
//!
//! The old pool was a fixed `[u64; 512]` bitmap in `.bss` covering exactly the
//! 128 MiB QEMU `-M virt` hands out. A Mac has 8–128 GiB, and a static bitmap
//! for 128 GiB would be 4 MiB of `.bss` in an image we have to load before we
//! know whether the machine even has that much RAM.
//!
//! So the bitmap is **sized from the RAM we discover and carved out of that
//! RAM**. One bit per 4 KiB frame is 32 KiB of metadata per GiB — 2 MiB for a
//! 64 GiB Mac, and it is the only memory that has to be written at boot. (The
//! obvious alternative, threading a free-list through the frames themselves,
//! needs no static metadata at all but has to *touch every free frame* to link
//! it; on a 64 GiB machine that is ~128 MiB of writes across the whole address
//! space, and every one of those frames must already be mapped. The bitmap
//! keeps the boot-time working set to the bitmap itself.)
//!
//! Bit semantics: **1 = unavailable** (allocated, reserved, or a hole between
//! banks), **0 = free**. The bitmap spans one contiguous range from the lowest
//! bank base to the highest bank end, so a frame's index is pure arithmetic;
//! gaps between banks are simply born set and never handed out.
//!
//! Usage: `add_bank()` for each RAM range, `reserve()` for each region that
//! must never be allocated, then `finalize()` once. `alloc`/`free`/`stats`
//! behave as before.

use core::sync::atomic::{AtomicBool, Ordering};

const FRAME_SIZE: u64 = 4096;

/// Real trees have one or two banks; Apple's has a handful.
const MAX_BANKS: usize = 8;
/// Kernel image, stack, DTB, the `/memory` reservation block, and every
/// `/reserved-memory` child have to fit here.
const MAX_RESERVED: usize = 32;

/// A half-open physical range `[base, end)`.
#[derive(Clone, Copy)]
struct Region {
    base: u64,
    end: u64,
}

const EMPTY: Region = Region { base: 0, end: 0 };

static mut BANKS: [Region; MAX_BANKS] = [EMPTY; MAX_BANKS];
static mut BANK_COUNT: usize = 0;
static mut RESERVED: [Region; MAX_RESERVED] = [EMPTY; MAX_RESERVED];
static mut RESERVED_COUNT: usize = 0;

struct Pool {
    /// Physical address of frame index 0.
    span_base: u64,
    /// Frames covered by the bitmap, including holes between banks.
    frame_count: usize,
    /// The bitmap, living in the RAM it describes.
    bitmap: *mut u64,
    words: usize,
    /// Frames that were ever available (bank frames minus reserved).
    usable: usize,
    allocated: usize,
    /// Where the last search left off, so alloc isn't O(span) every call.
    hint: usize,
}

impl Pool {
    const fn empty() -> Self {
        Self {
            span_base: 0,
            frame_count: 0,
            bitmap: core::ptr::null_mut(),
            words: 0,
            usable: 0,
            allocated: 0,
            hint: 0,
        }
    }
}

static mut POOL: Pool = Pool::empty();
static POOL_INIT: AtomicBool = AtomicBool::new(false);

const fn align_up(v: u64, a: u64) -> u64 {
    (v.wrapping_add(a - 1)) & !(a - 1)
}

const fn align_down(v: u64, a: u64) -> u64 {
    v & !(a - 1)
}

/// Declare a bank of usable RAM. Ends are page-aligned inward, so a bank never
/// claims a partial frame.
pub unsafe fn add_bank(base: u64, size: u64) {
    let b = align_up(base, FRAME_SIZE);
    let e = align_down(base.saturating_add(size), FRAME_SIZE);
    if e <= b || BANK_COUNT >= MAX_BANKS {
        return;
    }
    BANKS[BANK_COUNT] = Region { base: b, end: e };
    BANK_COUNT += 1;
}

/// Declare a region that must never be allocated. Rounded *outward* to whole
/// frames: reserving too much is a leak, reserving too little is corruption.
pub unsafe fn reserve(base: u64, size: u64) {
    if size == 0 || RESERVED_COUNT >= MAX_RESERVED {
        return;
    }
    let b = align_down(base, FRAME_SIZE);
    let e = align_up(base.saturating_add(size), FRAME_SIZE);
    if e <= b {
        return;
    }
    RESERVED[RESERVED_COUNT] = Region { base: b, end: e };
    RESERVED_COUNT += 1;
}

unsafe fn overlaps_reserved(base: u64, end: u64) -> Option<Region> {
    for i in 0..RESERVED_COUNT {
        let r = RESERVED[i];
        if base < r.end && r.base < end {
            return Some(r);
        }
    }
    None
}

/// Set (mark unavailable) frames `[from, to)`, a word at a time.
unsafe fn set_bits(bm: *mut u64, from: usize, to: usize) {
    let mut i = from;
    while i < to {
        let word = i / 64;
        let bit = i % 64;
        if bit == 0 && to - i >= 64 {
            *bm.add(word) = u64::MAX;
            i += 64;
        } else {
            *bm.add(word) |= 1u64 << bit;
            i += 1;
        }
    }
}

/// Clear (mark free) frames `[from, to)`, a word at a time.
unsafe fn clear_bits(bm: *mut u64, from: usize, to: usize) {
    let mut i = from;
    while i < to {
        let word = i / 64;
        let bit = i % 64;
        if bit == 0 && to - i >= 64 {
            *bm.add(word) = 0;
            i += 64;
        } else {
            *bm.add(word) &= !(1u64 << bit);
            i += 1;
        }
    }
}

/// Build the bitmap from the declared banks and reservations.
///
/// Returns `false` if there is no RAM to manage or nowhere to put the bitmap.
/// Must be called exactly once, and before any `alloc`.
pub unsafe fn finalize() -> bool {
    if BANK_COUNT == 0 {
        return false;
    }

    // 1. The bitmap spans lowest bank base .. highest bank end.
    let mut span_base = u64::MAX;
    let mut span_end = 0u64;
    for i in 0..BANK_COUNT {
        if BANKS[i].base < span_base {
            span_base = BANKS[i].base;
        }
        if BANKS[i].end > span_end {
            span_end = BANKS[i].end;
        }
    }
    if span_end <= span_base {
        return false;
    }

    let frame_count = ((span_end - span_base) / FRAME_SIZE) as usize;
    let words = frame_count.div_ceil(64);
    let bitmap_bytes = (words as u64) * 8;

    // 2. Place the bitmap in the first page-aligned run of a bank that is big
    //    enough and clear of every reservation. It has to live in RAM we are
    //    about to manage — there is nowhere else to put it.
    let mut placement: Option<u64> = None;
    'banks: for i in 0..BANK_COUNT {
        let bank = BANKS[i];
        let mut start = bank.base;
        // Each retry jumps past one reservation's end, and `start` only ever
        // increases, so this cannot spin.
        for _ in 0..(RESERVED_COUNT + 2) {
            let end = start.saturating_add(bitmap_bytes);
            if end > bank.end {
                continue 'banks;
            }
            match overlaps_reserved(start, end) {
                Some(r) => start = align_up(r.end, FRAME_SIZE),
                None => {
                    placement = Some(start);
                    break 'banks;
                }
            }
        }
    }
    let bitmap_addr = match placement {
        Some(a) => a,
        None => return false,
    };
    let bm = bitmap_addr as *mut u64;

    // 3. Everything starts unavailable — that makes the holes between banks,
    //    and the padding bits past `frame_count`, free of charge.
    for w in 0..words {
        *bm.add(w) = u64::MAX;
    }

    // 4. Open up the banks.
    for i in 0..BANK_COUNT {
        let bank = BANKS[i];
        let from = ((bank.base - span_base) / FRAME_SIZE) as usize;
        let to = ((bank.end - span_base) / FRAME_SIZE) as usize;
        clear_bits(bm, from, to.min(frame_count));
    }

    // 5. Close the reservations back up, plus the bitmap's own frames — it is
    //    sitting in a bank we just declared free.
    for i in 0..RESERVED_COUNT {
        let r = RESERVED[i];
        if r.end <= span_base || r.base >= span_end {
            continue;
        }
        let from = ((r.base.max(span_base) - span_base) / FRAME_SIZE) as usize;
        let to = ((r.end.min(span_end) - span_base) / FRAME_SIZE) as usize;
        set_bits(bm, from, to.min(frame_count));
    }
    let bm_from = ((bitmap_addr - span_base) / FRAME_SIZE) as usize;
    let bm_to = ((align_up(bitmap_addr + bitmap_bytes, FRAME_SIZE) - span_base) / FRAME_SIZE) as usize;
    set_bits(bm, bm_from, bm_to.min(frame_count));

    // 6. Whatever bits are still clear is what we can hand out.
    let mut set = 0usize;
    for w in 0..words {
        set += (*bm.add(w)).count_ones() as usize;
    }

    let pool = &raw mut POOL;
    (*pool).span_base = span_base;
    (*pool).frame_count = frame_count;
    (*pool).bitmap = bm;
    (*pool).words = words;
    (*pool).usable = frame_count.saturating_sub(set);
    (*pool).allocated = 0;
    (*pool).hint = 0;
    POOL_INIT.store(true, Ordering::Release);
    true
}

/// Allocate one 4 KiB frame. Returns its physical address.
pub unsafe fn alloc() -> Option<u64> {
    if !POOL_INIT.load(Ordering::Acquire) {
        return None;
    }
    let pool = &raw mut POOL;
    let words = (*pool).words;
    let bm = (*pool).bitmap;

    for step in 0..words {
        let word = ((*pool).hint + step) % words;
        let bits = *bm.add(word);
        if bits == u64::MAX {
            continue;
        }
        let bit = bits.trailing_ones() as usize;
        let idx = word * 64 + bit;
        if idx >= (*pool).frame_count {
            continue;
        }
        *bm.add(word) = bits | (1u64 << bit);
        (*pool).allocated += 1;
        (*pool).hint = word;
        return Some((*pool).span_base + (idx as u64) * FRAME_SIZE);
    }
    None
}

/// Free a frame. Returns `false` — without touching the bitmap — if the address
/// is not a frame this pool ever handed out. Reserved frames are rejected here
/// too: a stray free of the DTB or the kernel image would otherwise quietly add
/// it to the free pool, and the corruption would surface far from the cause.
pub unsafe fn free(addr: u64) -> bool {
    if !POOL_INIT.load(Ordering::Acquire) {
        return false;
    }
    let pool = &raw mut POOL;
    if addr < (*pool).span_base || addr % FRAME_SIZE != 0 {
        return false;
    }
    let idx = ((addr - (*pool).span_base) / FRAME_SIZE) as usize;
    if idx >= (*pool).frame_count {
        return false;
    }
    if overlaps_reserved(addr, addr + FRAME_SIZE).is_some() {
        return false;
    }
    let mut in_bank = false;
    for i in 0..BANK_COUNT {
        if addr >= BANKS[i].base && addr < BANKS[i].end {
            in_bank = true;
            break;
        }
    }
    if !in_bank {
        return false;
    }

    let word = idx / 64;
    let bit = idx % 64;
    let mask = 1u64 << bit;
    let bm = (*pool).bitmap;
    if (*bm.add(word)) & mask == 0 {
        return false; // not allocated
    }
    *bm.add(word) &= !mask;
    (*pool).allocated -= 1;
    (*pool).hint = word;
    true
}

/// Return `(total_frames, used_frames, free_frames)`, where total counts only
/// frames that were ever allocatable.
pub fn stats() -> (usize, usize, usize) {
    unsafe {
        let pool = &raw const POOL;
        let total = (*pool).usable;
        let used = (*pool).allocated;
        (total, used, total.saturating_sub(used))
    }
}

/// Where the bitmap landed and how big it is — `(addr, bytes)`. For boot logs.
pub fn bitmap_info() -> (u64, u64) {
    unsafe {
        let pool = &raw const POOL;
        ((*pool).bitmap as u64, ((*pool).words as u64) * 8)
    }
}
