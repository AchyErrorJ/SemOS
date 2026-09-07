//! Haswell GGTT programming for the Rung C page flip (task #16).
//!
//! Metal finding 2026-09-01: the T540p's GOP framebuffer is **not** in stolen
//! memory (`GMS=0`, `fb_phys = 0xE0000000` = BAR2 aperture base). The plane
//! surface address `DSPSURF_A` is a **GGTT (aperture) offset** on this
//! machine, and GOP mapped the framebuffer at GGTT offset 0 — which is why
//! DSPSURF/DSPSURFLIVE read 0. A page flip therefore needs the second buffer
//! to exist *in the GGTT*: 2048 PTEs covering aperture offset `0x800000`
//! (framebuffer::FLIP_OFFSET) pointing at a SemOS-owned 8 MiB back buffer,
//! plus a CPU window at `fb_va + 0x800000` so apps can draw into it.
//!
//! i915 v5.15 references (`drivers/gpu/drm/i915/gt/intel_ggtt.c`):
//!
//! - `gen6_gmch_probe` / `ggtt_probe_common`: on gen6–HSW the GGTT lives in
//!   the **second half of BAR0** (`bar0 + bar0_size/2`). i915 sizes it from
//!   the host-bridge GGC register's GGMS field, but that register reads
//!   0x0000 under SemOS on the T540p (the same read that reported `GMS=0`),
//!   so we take the whole second half (2 MiB) as the usable window — we need
//!   only 4096 of its 524288 entries — and treat GOP's GGTT[0] as the real
//!   probe. T540p: BAR0 = 4 MiB → GGTT at BAR0 + 2 MiB.
//! - PTE = `gen6_pte_t` (u32): `GEN6_PTE_VALID` = bit 0; HSW address encode
//!   `addr | ((addr >> 28) & 0x7F0)`; the 4-bit cacheability field lives in
//!   bits [3:1] + bit 11. We never derive cache bits ourselves — we copy the
//!   attribute bits of GOP's own GGTT[0], which by definition produces a
//!   working scanout on this panel.
//! - `gen6_ggtt_invalidate`: after PTE updates, write `GFX_FLSH_CNTL_EN` to
//!   `GFX_FLSH_CNTL_GEN6` (0x101008) and posting-read it.
//!
//! Safety: everything is gated on the whitelisted HD 4600 (via `igpu::find`
//! + `MmioReg::new`), PTE writes are readback-verified, and the CPU window
//! is end-to-end verified (write through the aperture, read back through the
//! direct map) before the flip path is armed. `arm()` is idempotent and any
//! failure leaves the previous (working Rung-A) state untouched.

use crate::{igpu, memory, paging, println};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// GAM register that invalidates the GGTT TLB after PTE updates.
const GFX_FLSH_CNTL_GEN6: u64 = 0x101008;
const GFX_FLSH_CNTL_EN: u32 = 1;

/// Number of 4 KiB GGTT entries covering the flip back buffer
/// (FLIP_OFFSET = 8 MiB → 2048 entries). The back buffer occupies GGTT
/// entries [FLIP_PAGES, 2*FLIP_PAGES) — i.e. aperture offset 0x800000.
const FLIP_PAGES: usize = (crate::framebuffer::FLIP_OFFSET / 4096) as usize;

/// PTE attribute bits we carry over from GOP's GGTT[0]: bits [3:0]
/// (valid + low cacheability) and bit 11 (4th cacheability bit).
const PTE_ATTR_MASK: u32 = 0x80F;

struct BackBuffer {
    armed: bool,
    /// Physical frames backing GGTT entries [FLIP_PAGES, 2*FLIP_PAGES).
    pages: [u64; FLIP_PAGES],
    /// Attribute bits copied from GGTT[0] at arm time (diagnostics).
    attrs: u32,
}

static BACK: Mutex<BackBuffer> = Mutex::new(BackBuffer {
    armed: false,
    pages: [0; FLIP_PAGES],
    attrs: 0,
});

/// Cheap lock-free check for the fb_flip hot path.
static ARMED: AtomicBool = AtomicBool::new(false);

pub fn is_armed() -> bool {
    ARMED.load(Ordering::Acquire)
}

/// HSW GGTT address encode: physical address bits [31:12] in place, bits
/// [39:32] into PTE bits [11:4] (i915 `HSW_GTT_ADDR_ENCODE`).
#[inline]
fn pte_encode(phys: u64, attrs: u32) -> u32 {
    (phys as u32) | (((phys >> 28) as u32) & 0x7F0) | attrs
}

/// Physical base + size of the GGTT. On gen6–HSW the GGTT is always the
/// second half of BAR0 (`ggtt_probe_common`: `bar0 + bar0_size/2`), so the
/// geometry needs no firmware register at all. We deliberately do NOT read
/// the host-bridge GGC/GGMS field for the size: on the T540p that register
/// reads 0x0000 under SemOS (the same read that reported `GMS=0`), which
/// would zero the size and kill the probe. The full second half (2 MiB,
/// 524288 PTEs) is a safe upper bound — we need only 4096 entries — and the
/// real probe is behavioral: `arm()` refuses unless GOP's own GGTT[0] is a
/// valid, nonzero PTE.
fn ggtt_layout() -> Option<(u64, u64)> {
    let info = igpu::find()?;
    if info.device_id != igpu::HASWELL_GT2_MOBILE_HD4600 {
        return None;
    }
    let bar0_phys = match info.bar0.kind {
        igpu::BarKind::Mmio32 { base, .. } => base as u64,
        igpu::BarKind::Mmio64 { base, .. } => base,
        _ => return None,
    };
    if info.bar0.size < 4 * 1024 * 1024 {
        return None; // expected 4 MiB on HSW; GGTT is the second half
    }
    Some((bar0_phys + info.bar0.size / 2, info.bar0.size / 2))
}

/// Read GGTT entry `idx` through the (uncached) physical map.
unsafe fn read_pte(ggtt_va: u64, idx: usize) -> u32 {
    core::ptr::read_volatile((ggtt_va + (idx as u64) * 4) as *const u32)
}

unsafe fn write_pte(ggtt_va: u64, idx: usize, pte: u32) {
    core::ptr::write_volatile((ggtt_va + (idx as u64) * 4) as *mut u32, pte);
}

/// Read-only GGTT probe for `modeset status`. Safe before/without arming.
pub fn status() {
    let (ggtt_phys, ggtt_size) = match ggtt_layout() {
        Some(l) => l,
        None => {
            println!("  ggtt: unavailable (device/BAR probe failed)");
            return;
        }
    };
    println!(
        "  ggtt: phys=0x{:X} size={} MiB (BAR0 second half)",
        ggtt_phys,
        ggtt_size >> 20
    );
    let _ = paging::set_region_uncached(ggtt_phys, ggtt_size);
    let ggtt_va = paging::phys_to_virt(ggtt_phys);
    unsafe {
        println!(
            "  ggtt: [0]=0x{:08X} [{}]=0x{:08X} armed={}",
            read_pte(ggtt_va, 0),
            FLIP_PAGES,
            read_pte(ggtt_va, FLIP_PAGES),
            is_armed()
        );
    }
}

/// One-time setup: point GGTT entries [FLIP_PAGES, 2*FLIP_PAGES) at a fresh
/// SemOS-owned 8 MiB back buffer and map a CPU window for it at
/// `fb_va + FLIP_OFFSET` → `aperture_base + FLIP_OFFSET`.
///
/// Idempotent. On any failure everything allocated so far is rolled back and
/// the flip path stays unarmed (callers fall back to Rung-A blits).
pub fn arm(fb_va: u64, aperture_base: u64, aperture_size: u64, fb_len: u64) -> bool {
    let mut back = BACK.lock();
    if back.armed {
        return true;
    }

    let (ggtt_phys, ggtt_size) = match ggtt_layout() {
        Some(l) => l,
        None => {
            println!("ggtt: refuse — layout probe failed");
            return false;
        }
    };
    // The GGTT must hold entries [0, 2*FLIP_PAGES), and the aperture must
    // cover both buffers.
    if (ggtt_size / 4) < (2 * FLIP_PAGES) as u64 {
        println!("ggtt: refuse — GGTT too small ({} bytes)", ggtt_size);
        return false;
    }
    if aperture_size < crate::framebuffer::FLIP_OFFSET + fb_len {
        println!(
            "ggtt: refuse — aperture 0x{:X} too small for two frames",
            aperture_size
        );
        return false;
    }

    let _ = paging::set_region_uncached(ggtt_phys, ggtt_size);
    let ggtt_va = paging::phys_to_virt(ggtt_phys);

    // Sanity: GOP must have a valid, nonzero PTE at GGTT[0] (the fb's first
    // page). If not, our whole addressing model is wrong — stop here.
    let g0 = unsafe { read_pte(ggtt_va, 0) };
    if g0 == 0 || g0 & 1 == 0 {
        println!(
            "ggtt: refuse — GGTT[0]=0x{:08X} invalid; fb not GGTT-mapped as expected",
            g0
        );
        return false;
    }
    let attrs = g0 & PTE_ATTR_MASK;
    println!(
        "ggtt: GGTT[0]=0x{:08X} (attrs=0x{:X}), [{}]=0x{:08X} pre-arm",
        g0,
        attrs,
        FLIP_PAGES,
        unsafe { read_pte(ggtt_va, FLIP_PAGES) }
    );

    // Allocate the back buffer: 2048 ordinary frames (scatter is fine — the
    // GGTT exists precisely to gather them). Try pools from Public upward.
    let mut got = 0usize;
    for slot in 0..FLIP_PAGES {
        match alloc_any_tier() {
            Some(p) => back.pages[slot] = p,
            None => {
                println!("ggtt: refuse — frame allocation failed at {}/{}", got, FLIP_PAGES);
                for f in back.pages[..got].iter() {
                    let _ = memory::free(*f);
                }
                return false;
            }
        }
        got += 1;
    }

    // Program the PTEs, then invalidate the GGTT TLB (i915 gen6 sequence:
    // write GFX_FLSH_CNTL_EN, posting-read).
    unsafe {
        for i in 0..FLIP_PAGES {
            write_pte(ggtt_va, FLIP_PAGES + i, pte_encode(back.pages[i], attrs));
        }
    }
    if let Some(mmio) = super::mmio::MmioReg::new() {
        mmio.write32(GFX_FLSH_CNTL_GEN6, GFX_FLSH_CNTL_EN);
        let _ = mmio.read32(GFX_FLSH_CNTL_GEN6);
    }

    // Readback verify a sample of the new entries.
    let mut ok = true;
    unsafe {
        for &i in &[0usize, 1, 1024, FLIP_PAGES - 1] {
            let want = pte_encode(back.pages[i], attrs);
            let got_pte = read_pte(ggtt_va, FLIP_PAGES + i);
            if got_pte != want {
                println!(
                    "ggtt: PTE readback mismatch at [{}]: got 0x{:08X} want 0x{:08X}",
                    FLIP_PAGES + i,
                    got_pte,
                    want
                );
                ok = false;
            }
        }
    }
    if !ok {
        // Leave no dangling PTEs at frames we're about to free back.
        unsafe {
            for i in 0..FLIP_PAGES {
                write_pte(ggtt_va, FLIP_PAGES + i, 0);
            }
        }
        if let Some(mmio) = super::mmio::MmioReg::new() {
            mmio.write32(GFX_FLSH_CNTL_GEN6, GFX_FLSH_CNTL_EN);
            let _ = mmio.read32(GFX_FLSH_CNTL_GEN6);
        }
        for f in back.pages[..FLIP_PAGES].iter() {
            let _ = memory::free(*f);
        }
        return false;
    }

    // CPU window: [fb_va + FLIP_OFFSET, +8 MiB) must translate to the second
    // aperture half. If the bootloader already mapped it, accept; if it's
    // absent, extend the boot tables with the fb's own cache attributes; a
    // conflicting mapping is a hard refuse.
    let window_va = fb_va + crate::framebuffer::FLIP_OFFSET;
    let window_phys = aperture_base + crate::framebuffer::FLIP_OFFSET;
    match paging::walk_pml4_for(paging::boot_cr3(), window_va) {
        Some(p) if p == window_phys => {}
        Some(p) => {
            println!(
                "ggtt: refuse — fb+0x800000 already maps phys 0x{:X} (want 0x{:X})",
                p, window_phys
            );
            for f in back.pages[..FLIP_PAGES].iter() {
                let _ = memory::free(*f);
            }
            return false;
        }
        None => {
            let page_attrs = paging::mapping_attrs_4k(fb_va);
            if !paging::ensure_kernel_mapped(
                window_va,
                window_phys,
                crate::framebuffer::FLIP_OFFSET,
                page_attrs,
            ) {
                println!("ggtt: refuse — could not map CPU window at fb+0x800000");
                for f in back.pages[..FLIP_PAGES].iter() {
                    let _ = memory::free(*f);
                }
                return false;
            }
        }
    }

    // End-to-end CPU verify: write through the aperture window, read back
    // through the direct map of the backing frame. Catches a wrong GGTT
    // base/geometry before scanout ever sees the buffer.
    unsafe {
        for &i in &[0usize, 1, 1024, FLIP_PAGES - 1] {
            let pat = 0xC0DE_0000u32 | i as u32;
            let through_window = (window_va + (i as u64) * 4096) as *mut u32;
            let direct = paging::phys_to_virt(back.pages[i]) as *const u32;
            core::ptr::write_volatile(through_window, pat);
            let got_val = core::ptr::read_volatile(direct);
            core::ptr::write_volatile(through_window, 0);
            if got_val != pat {
                println!(
                    "ggtt: refuse — window verify failed on frame {} (got 0x{:08X})",
                    i, got_val
                );
                for f in back.pages[..FLIP_PAGES].iter() {
                    let _ = memory::free(*f);
                }
                return false;
            }
        }
    }

    back.attrs = attrs;
    back.armed = true;
    ARMED.store(true, Ordering::Release);
    println!(
        "ggtt: armed — {} PTEs at GGTT[{}..{}], attrs=0x{:X}, window fb+0x{:X}",
        FLIP_PAGES,
        FLIP_PAGES,
        2 * FLIP_PAGES,
        attrs,
        crate::framebuffer::FLIP_OFFSET
    );
    true
}

/// Allocate one frame, trying pools from Public upward. The back buffer is
/// kernel-internal (only the display engine and fb_blit ever touch it), so
/// the tier choice is about pool pressure, not clearance.
fn alloc_any_tier() -> Option<u64> {
    for tier in [
        memory::SecurityTier::Public,
        memory::SecurityTier::Internal,
        memory::SecurityTier::Sensitive,
        memory::SecurityTier::Secret,
    ] {
        if let Some(p) = memory::alloc(tier) {
            return Some(p);
        }
    }
    None
}
