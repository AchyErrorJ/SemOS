//! Apple Interrupt Controller (AIC), version 2 — the M1 Pro/Max/Ultra (`t600x`).
//!
//! Apple does not use an ARM GIC. Two things about the AIC drive this design,
//! and both come from Linux's `drivers/irqchip/irq-apple-aic.c` rather than from
//! anything we can probe under QEMU:
//!
//! 1. **The timer is not an AIC interrupt at all.** The ARMv8 generic timer is
//!    delivered straight to the CPU as an **FIQ**, and is identified by reading
//!    `CNTP_CTL_EL0` — there is no controller register to ack. The AIC handles
//!    *device* interrupts. So a preemptive scheduler on Apple needs the FIQ
//!    vector, not this file; see `timer_firing()` in `main.rs`.
//! 2. **AIC2's register offsets are not constants.** Only `IRQ_CFG` (0x2000) is
//!    fixed; the mask/software-trigger registers sit after a variable-length
//!    IRQ-config array whose size comes from `AIC2_INFO3.MAX_IRQ` at probe time.
//!    Hardcoding offsets from another SoC would compile, boot, and silently
//!    never deliver an interrupt.
//!
//! The event register lives in a **second `reg` range**: the die count is not
//! discoverable from the capability registers, so the device tree spells it out.
//!
//! Reading the event register **acknowledges and masks** the interrupt. EOI is
//! therefore an *unmask*, not an ack.
//!
//! Single-die only (`t6000`/`t6001`). `t6002` (M1 Ultra) is two dies and needs
//! `die_stride`; we would rather fail loudly there than silently drive die 0.

use core::sync::atomic::{AtomicBool, Ordering};

// Fixed AIC2 offsets.
const AIC2_INFO1: u64 = 0x0004; // [15:0] = nr_irq
const AIC2_INFO3: u64 = 0x000c; // [15:0] = max_irq
const AIC2_CONFIG: u64 = 0x0014; // bit 0 = enable
const AIC2_CONFIG_ENABLE: u32 = 1 << 0;
const AIC2_IRQ_CFG: u64 = 0x2000;

// Event register field encoding (shared with AIC1).
const AIC_EVENT_TYPE_SHIFT: u32 = 16;
const AIC_EVENT_TYPE_MASK: u32 = 0xFF;
const AIC_EVENT_NUM_MASK: u32 = 0xFFFF;

pub const EVENT_TYPE_FIQ: u32 = 0;
pub const EVENT_TYPE_IRQ: u32 = 1;
pub const EVENT_TYPE_IPI: u32 = 4;

struct Aic2 {
    base: u64,
    /// Second `reg` range — reading it acks and masks the pending interrupt.
    event: u64,
    nr_irq: u32,
    max_irq: u32,
    /// Computed at probe: the IRQ-config array's length decides where these sit.
    mask_set: u64,
    mask_clr: u64,
}

static mut AIC: Aic2 = Aic2 {
    base: 0,
    event: 0,
    nr_irq: 0,
    max_irq: 0,
    mask_set: 0,
    mask_clr: 0,
};
static PRESENT: AtomicBool = AtomicBool::new(false);

#[inline]
unsafe fn r32(a: u64) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}
#[inline]
unsafe fn w32(a: u64, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v);
}

/// Is an AIC2 present and initialized?
pub fn present() -> bool {
    PRESENT.load(Ordering::Acquire)
}

/// `(nr_irq, max_irq)` as reported by the hardware. For boot logs.
pub fn info() -> (u32, u32) {
    unsafe { (AIC.nr_irq, AIC.max_irq) }
}

/// Bring up AIC2 from its device-tree `reg` ranges.
///
/// `regs[0]` is the register base, `regs[1]` the event register. Returns false
/// if the tree did not give us both — without the event range there is no way to
/// ack an interrupt, and a controller we cannot ack would wedge the CPU on the
/// first device IRQ.
pub unsafe fn init(regs: &[(u64, u64)]) -> bool {
    if regs.len() < 2 {
        return false;
    }
    let base = regs[0].0;
    let event = regs[1].0;

    let nr_irq = r32(base + AIC2_INFO1) & 0xFFFF;
    let max_irq = r32(base + AIC2_INFO3) & 0xFFFF;
    if max_irq == 0 || nr_irq == 0 {
        return false;
    }

    // Offsets after the IRQ-config array. Mirrors the arithmetic in Linux's
    // aic_of_ic_init(): one u32 per IRQ for the config array, then one u32 per
    // 32 IRQs for each of SW_SET, SW_CLR, MASK_SET, MASK_CLR, HW_STATE.
    let stride = 4 * (max_irq as u64 >> 5);
    let mut off = AIC2_IRQ_CFG + 4 * max_irq as u64;
    off += stride; // SW_SET
    off += stride; // SW_CLR
    let mask_set = off;
    off += stride;
    let mask_clr = off;

    AIC.base = base;
    AIC.event = event;
    AIC.nr_irq = nr_irq;
    AIC.max_irq = max_irq;
    AIC.mask_set = mask_set;
    AIC.mask_clr = mask_clr;

    // Mask every device interrupt: we have no device drivers yet, and an
    // unmasked source we do not service would re-fire forever.
    let words = max_irq.div_ceil(32) as u64;
    for w in 0..words {
        w32(base + mask_set + w * 4, 0xFFFF_FFFF);
    }

    // Enable the controller.
    let cfg = r32(base + AIC2_CONFIG);
    w32(base + AIC2_CONFIG, cfg | AIC2_CONFIG_ENABLE);

    PRESENT.store(true, Ordering::Release);
    true
}

/// Read one pending event. **This is the acknowledgement** — the read masks the
/// interrupt as a side effect. Returns `(type, irq)`, or `None` when the
/// controller has nothing left (event == 0).
pub unsafe fn next_event() -> Option<(u32, u32)> {
    if !present() {
        return None;
    }
    let event = r32(AIC.event);
    if event == 0 {
        return None;
    }
    let ty = (event >> AIC_EVENT_TYPE_SHIFT) & AIC_EVENT_TYPE_MASK;
    let irq = event & AIC_EVENT_NUM_MASK;
    Some((ty, irq))
}

/// Unmask a device interrupt — this is the EOI, since the event read already
/// acked and masked it.
pub unsafe fn unmask(irq: u32) {
    if !present() || irq >= AIC.max_irq {
        return;
    }
    let reg = AIC.mask_clr + 4 * (irq as u64 >> 5);
    w32(AIC.base + reg, 1 << (irq & 31));
}

/// Mask a device interrupt.
pub unsafe fn mask(irq: u32) {
    if !present() || irq >= AIC.max_irq {
        return;
    }
    let reg = AIC.mask_set + 4 * (irq as u64 >> 5);
    w32(AIC.base + reg, 1 << (irq & 31));
}
