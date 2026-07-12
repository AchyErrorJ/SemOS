//! Early-console UART, retargeted at runtime from the device tree.
//!
//! Two register layouts, picked by the console node's `compatible`:
//!
//! * **PL011** (`arm,pl011`) — QEMU `-M virt`. Data register at +0x00, flag
//!   register at +0x18 with TXFF at bit 5.
//! * **Apple s5l** (`apple,s5l-uart`) — every Apple Silicon Mac. A descendant
//!   of the Samsung S3C UART: status at +0x10 (TX-buffer-empty at bit 1), TX
//!   holding register at +0x20, and 32-bit accesses only.
//!
//! Until `init_from_fdt` runs we have to print *something* — the FDT parse
//! itself logs, and a parse failure must be able to say so — so the statics
//! start on the PL011 base QEMU uses. That guess is only ever wrong on hardware
//! whose real console we are about to discover anyway.
//!
//! Neither path programs baud, line control, or the FIFOs: whoever loaded us
//! (QEMU, or m1n1, which prints its own banner) already brought the console up,
//! and re-initializing it mid-boot only risks dropping bytes on a working link.

use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use crate::fdt::Node;

/// PL011 UART0 on QEMU `-M virt` — the pre-FDT default.
const DEFAULT_BASE: u64 = 0x0900_0000;

// PL011 register offsets.
const PL011_DR: u64 = 0x00;
const PL011_FR: u64 = 0x18;
const PL011_FR_TXFF: u32 = 1 << 5; // TX FIFO full

// Apple s5l register offsets.
const S5L_UTRSTAT: u64 = 0x10;
const S5L_UTXH: u64 = 0x20;
const S5L_UTRSTAT_TXBE: u32 = 1 << 1; // TX buffer empty

/// Which register layout the console UART uses.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UartKind {
    Pl011,
    AppleS5L,
}

impl UartKind {
    /// The `compatible` string a device tree uses for this UART.
    pub fn name(self) -> &'static str {
        match self {
            UartKind::Pl011 => "arm,pl011",
            UartKind::AppleS5L => "apple,s5l-uart",
        }
    }

    fn from_tag(tag: u8) -> UartKind {
        match tag {
            TAG_APPLE_S5L => UartKind::AppleS5L,
            _ => UartKind::Pl011,
        }
    }

    fn tag(self) -> u8 {
        match self {
            UartKind::Pl011 => TAG_PL011,
            UartKind::AppleS5L => TAG_APPLE_S5L,
        }
    }
}

const TAG_PL011: u8 = 0;
const TAG_APPLE_S5L: u8 = 1;

static UART_BASE: AtomicU64 = AtomicU64::new(DEFAULT_BASE);
static UART_TAG: AtomicU8 = AtomicU8::new(TAG_PL011);

/// A TX register that never drains would otherwise hang the boot with no
/// output at all. Bound the wait and push the byte anyway: a garbled line is a
/// far better failure than a silent one.
const TX_SPIN_LIMIT: u32 = 100_000;

/// Point the console at the UART the device tree describes.
///
/// Returns the `(kind, base)` adopted, or `None` if the tree has no console
/// node we can drive — in which case the caller keeps printing to the
/// pre-FDT default and should say so.
pub fn init_from_fdt(fdt: &crate::fdt::Fdt) -> Option<(UartKind, u64)> {
    let node = fdt.stdout_uart()?;
    let (base, _size) = node.reg?;
    let kind = classify(&node)?;

    // Order matters: a reader that saw the new tag must not still see the old
    // base. Publish the base first, then the tag that authorizes its layout.
    UART_BASE.store(base, Ordering::Relaxed);
    UART_TAG.store(kind.tag(), Ordering::Release);
    Some((kind, base))
}

/// Map a console node's `compatible` list onto a driver we actually have.
fn classify(node: &Node) -> Option<UartKind> {
    if node.is_compatible("apple,s5l-uart") {
        Some(UartKind::AppleS5L)
    } else if node.is_compatible("arm,pl011") {
        Some(UartKind::Pl011)
    } else {
        None
    }
}

/// The UART the console is currently driving.
pub fn current() -> (UartKind, u64) {
    let tag = UART_TAG.load(Ordering::Acquire);
    (UartKind::from_tag(tag), UART_BASE.load(Ordering::Relaxed))
}

#[inline]
unsafe fn mmio_r32(addr: u64) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

#[inline]
unsafe fn mmio_w32(addr: u64, v: u32) {
    core::ptr::write_volatile(addr as *mut u32, v);
}

/// Emit one byte on the console UART, and mirror it to the framebuffer console
/// if one exists.
///
/// The mirror lives here, at the bottom, on purpose: every existing caller —
/// the FDT log, the memory report, the panic handler — reaches the screen
/// without knowing a screen exists. On a Mac with no second machine, this is
/// the only output there is.
#[inline]
pub fn uart_put(b: u8) {
    crate::fb::putc(b);

    let (kind, base) = current();
    unsafe {
        match kind {
            UartKind::Pl011 => {
                let mut spins = 0;
                while mmio_r32(base + PL011_FR) & PL011_FR_TXFF != 0 && spins < TX_SPIN_LIMIT {
                    spins += 1;
                    core::hint::spin_loop();
                }
                mmio_w32(base + PL011_DR, b as u32);
            }
            UartKind::AppleS5L => {
                let mut spins = 0;
                while mmio_r32(base + S5L_UTRSTAT) & S5L_UTRSTAT_TXBE == 0 && spins < TX_SPIN_LIMIT {
                    spins += 1;
                    core::hint::spin_loop();
                }
                mmio_w32(base + S5L_UTXH, b as u32);
            }
        }
    }
}

/// Emit a string over the console UART, expanding `\n` to `\r\n`.
pub fn uart_str(s: &str) {
    for &b in s.as_bytes() {
        if b == b'\n' {
            uart_put(b'\r');
        }
        uart_put(b);
    }
}
