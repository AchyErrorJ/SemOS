//! IOAPIC driver — minimal IRQ routing for real-hardware interrupts.
//!
//! On legacy hardware (QEMU's i8259 + emulated devices), the 8259 PIC at
//! ports 0x20/0xA0 delivered IRQs 0–15 as CPU vectors 32–47. The kernel
//! historically masked the PIC (`apic::mask_legacy_pic`) and enabled the
//! Local APIC for its timer.
//!
//! On real hardware (W540's Lynx Point PCH and similar), legacy device
//! IRQs (PS/2 keyboard on IRQ 1, RTC on IRQ 8, etc.) are routed through
//! the *IOAPIC* instead of the masked PIC. Without programming an IOAPIC
//! redirection entry, the BSP never sees the interrupt — the keyboard
//! presses generate signals that go nowhere.
//!
//! This module installs the minimum: map the IOAPIC at the conventional
//! physical address (0xFEC00000) and program the redirection table
//! entry (RTE) for IRQ 1 to deliver as CPU vector 33 (which already has
//! `interrupts::keyboard_interrupt_handler` wired). Future expansion
//! (ACPI MADT parsing, multiple IOAPICs, MP-aware delivery) goes here.

use core::ptr::{read_volatile, write_volatile};

/// Conventional MMIO physical base for the primary IOAPIC. The ACPI
/// MADT (table type 1) reports the real address; on every PCH chipset
/// I've seen it's this value. If ACPI parsing lands later, swap to
/// reading from the MADT.
const IOAPIC_PHYS_BASE: u64 = 0xFEC0_0000;

/// IOAPIC register window — the two registers are at +0x00 (IOREGSEL,
/// 8-bit index of register to access) and +0x10 (IOWIN, 32-bit data).
const IOREGSEL: u64 = 0x00;
const IOWIN: u64 = 0x10;

/// Redirection table entry base index. Each RTE is 64 bits, accessed
/// as two 32-bit halves at consecutive register indices.
const IOAPIC_RTE_BASE: u8 = 0x10;

/// CPU vector to which the keyboard IRQ should be delivered. Must match
/// the IDT entry installed by `interrupts::init`.
pub const KEYBOARD_VECTOR: u8 = 33;

/// Cached MMIO virtual base, set by `init()`.
static mut IOAPIC_VIRT_BASE: u64 = 0;

unsafe fn read_reg(reg: u8) -> u32 {
    let base = unsafe { IOAPIC_VIRT_BASE };
    unsafe {
        write_volatile((base + IOREGSEL) as *mut u32, reg as u32);
        read_volatile((base + IOWIN) as *const u32)
    }
}

unsafe fn write_reg(reg: u8, val: u32) {
    let base = unsafe { IOAPIC_VIRT_BASE };
    unsafe {
        write_volatile((base + IOREGSEL) as *mut u32, reg as u32);
        write_volatile((base + IOWIN) as *mut u32, val);
    }
}

/// Program one redirection table entry. `irq` is the legacy IRQ line
/// (0..=23 typically). `vector` is the CPU vector to deliver to.
/// `dest_apic_id` is the destination Local APIC ID (BSP = 0 normally).
unsafe fn set_rte(irq: u8, vector: u8, dest_apic_id: u8) {
    let lo_idx = IOAPIC_RTE_BASE + irq * 2;
    let hi_idx = lo_idx + 1;
    // Low DWORD: vector, fixed delivery mode (000), physical destination,
    // active high, edge triggered, unmasked.
    let lo = vector as u32;
    // High DWORD: destination APIC ID in bits 24..32.
    let hi = (dest_apic_id as u32) << 24;
    unsafe {
        // Always write the destination half first, then unmask via the
        // low half — avoids a delivery to an unknown destination during
        // the brief window between writes.
        write_reg(hi_idx, hi);
        write_reg(lo_idx, lo);
    }
}

/// Initialize the IOAPIC and route the legacy keyboard IRQ to the
/// CPU's keyboard interrupt vector. Returns `true` on success.
///
/// Must be called AFTER `apic::init()` so the Local APIC is enabled.
/// Safe to call even on systems without a real IOAPIC — the MMIO read
/// will return all-zeros / 0xFF and the routing won't fire (which is
/// the QEMU behavior we already had).
pub fn init() -> bool {
    let virt = crate::paging::phys_to_virt(IOAPIC_PHYS_BASE);
    unsafe { IOAPIC_VIRT_BASE = virt; }

    // Read IOAPIC ID + version as a sanity check.
    let id = unsafe { read_reg(0x00) } >> 24;
    let ver_word = unsafe { read_reg(0x01) };
    let version = ver_word & 0xFF;
    let max_redir = (ver_word >> 16) & 0xFF;
    crate::println!("[ioapic] base=0x{:X} (virt=0x{:X}) id={} ver=0x{:X} max_redir={}",
        IOAPIC_PHYS_BASE, virt, id, version, max_redir);

    // Read the BSP's Local APIC ID so we route there.
    let bsp_id = crate::apic::local_apic_id().unwrap_or(0);
    unsafe { set_rte(1, KEYBOARD_VECTOR, bsp_id as u8); }
    crate::println!("[ioapic] routed IRQ 1 (PS/2 keyboard) -> vector {} on APIC id {}",
        KEYBOARD_VECTOR, bsp_id);
    true
}
