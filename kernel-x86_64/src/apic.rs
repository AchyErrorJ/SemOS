//! Local APIC Support
//!
//! Replaces the legacy 8259 PIC + PIT with the modern Local APIC timer.
//! The Local APIC is per-CPU and lives at physical address `0xFEE00000`
//! by default. We access it through the bootloader's physical memory map.
//!
//! # Init sequence
//!
//! 1. Verify CPUID feature bit (EDX bit 9 from CPUID leaf 1).
//! 2. Disable the 8259 PIC by masking all IRQs.
//! 3. Read APIC base from IA32_APIC_BASE MSR (0x1B); ensure enable bit (11) is set.
//! 4. Enable the APIC by setting the Spurious Interrupt Vector Register (SVR)
//!    bit 8, with spurious vector = 255.
//! 5. Configure the timer LVT to fire on vector 32 (same as the PIT did)
//!    in periodic mode with divide-by-16.
//! 6. Set the initial counter to a reasonable starting value.
//!
//! # End-Of-Interrupt
//!
//! Writing 0 to the EOI register (offset 0xB0) acknowledges any in-service
//! interrupt. Replaces the OCW2 0x20 command sent to the PIC.

use core::ptr::{read_volatile, write_volatile};

/// Default physical base of the Local APIC MMIO region.
const APIC_DEFAULT_BASE: u64 = 0xFEE0_0000;

// --- Register offsets (MMIO bytes from APIC base) ---
const REG_ID: usize          = 0x020;
const REG_VERSION: usize     = 0x030;
const REG_EOI: usize         = 0x0B0;
const REG_SVR: usize         = 0x0F0; // Spurious Interrupt Vector
const REG_LVT_TIMER: usize   = 0x320;
const REG_TIMER_INIT: usize  = 0x380;
const REG_TIMER_CUR: usize   = 0x390;
const REG_TIMER_DIV: usize   = 0x3E0;

// --- LVT timer mode bits ---
const TIMER_MASKED: u32   = 1 << 16;
const TIMER_PERIODIC: u32 = 1 << 17;

// --- Spurious Interrupt Vector ---
const SVR_ENABLE: u32 = 1 << 8;
const SPURIOUS_VECTOR: u32 = 0xFF;

// --- Timer divide values (for REG_TIMER_DIV) ---
// Bit pattern is non-contiguous: bits 0-1 and 3.
// 0b1011 = divide by 1, 0b0000 = /2, 0b0001 = /4, 0b0010 = /8,
// 0b0011 = /16, 0b1000 = /32, 0b1001 = /64, 0b1010 = /128.
const DIV_BY_16: u32 = 0b0011;

/// Vector that timer interrupts fire on (matches the PIT slot we already use).
pub const TIMER_VECTOR: u8 = 32;

/// Cached APIC base (virtual address). Zero until init() runs.
static mut APIC_VIRT_BASE: u64 = 0;

/// Detect Local APIC support via CPUID.1:EDX[9].
fn cpuid_has_apic() -> bool {
    let edx: u32;
    unsafe {
        // CPUID leaf 1: returns features in EDX.
        // We need to clobber RBX because CPUID writes it.
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") _,
            out("edx") edx,
            options(nostack, preserves_flags),
        );
    }
    (edx & (1 << 9)) != 0
}

/// Read the IA32_APIC_BASE MSR (0x1B).
fn read_apic_base_msr() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x1Bu32,
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Write the IA32_APIC_BASE MSR.
fn write_apic_base_msr(value: u64) {
    let lo = value as u32;
    let hi = (value >> 32) as u32;
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") 0x1Bu32,
            in("eax") lo,
            in("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// Read a 32-bit APIC register.
fn read_reg(offset: usize) -> u32 {
    unsafe {
        let addr = (APIC_VIRT_BASE as usize + offset) as *const u32;
        read_volatile(addr)
    }
}

/// Write a 32-bit APIC register.
fn write_reg(offset: usize, value: u32) {
    unsafe {
        let addr = (APIC_VIRT_BASE as usize + offset) as *mut u32;
        write_volatile(addr, value);
    }
}

/// Disable the legacy 8259 PIC by masking all IRQs.
/// Must be done before enabling the APIC, or you'll get duplicate timer interrupts.
fn mask_legacy_pic() {
    use x86_64::instructions::port::Port;
    unsafe {
        let mut pic1_data: Port<u8> = Port::new(0x21);
        let mut pic2_data: Port<u8> = Port::new(0xA1);
        pic1_data.write(0xFF);
        pic2_data.write(0xFF);
    }
}

/// Initialize the Local APIC and start its periodic timer.
///
/// Returns `true` on success, `false` if no APIC was detected.
pub fn init() -> bool {
    if !cpuid_has_apic() {
        crate::println!("[apic] No Local APIC detected via CPUID");
        return false;
    }

    // Mask the legacy PIC first so we don't get spurious double-interrupts.
    mask_legacy_pic();

    // Read & enable the APIC via the MSR (bit 11 = global enable).
    let mut base = read_apic_base_msr();
    let phys_base = base & 0xFFFF_F000;
    base |= 1 << 11; // global enable
    write_apic_base_msr(base);

    // Map the APIC MMIO region to a virtual address using the bootloader's
    // physical-memory map. The MMIO is uncached by default in the firmware
    // mapping; that's fine for register access.
    unsafe {
        APIC_VIRT_BASE = crate::paging::phys_to_virt(phys_base);
    }

    // Enable the APIC via the Spurious Interrupt Vector Register.
    write_reg(REG_SVR, SVR_ENABLE | SPURIOUS_VECTOR);

    // Configure the timer: vector 32, periodic, no mask.
    write_reg(REG_LVT_TIMER, (TIMER_VECTOR as u32) | TIMER_PERIODIC);

    // Divide config: /16 (bus_clock / 16 = timer clock).
    write_reg(REG_TIMER_DIV, DIV_BY_16);

    // Initial count — picks the timer rate. On QEMU, the LAPIC bus clock is
    // 1 GHz, so /16 gives 62.5 MHz. An init count of ~625_000 → ~100 Hz.
    // On real hardware this rate varies; for now we just want it firing.
    write_reg(REG_TIMER_INIT, 1_000_000);

    let id = read_reg(REG_ID) >> 24;
    let version = read_reg(REG_VERSION) & 0xFF;
    crate::println!("[apic] Local APIC ID={} version=0x{:X} base=0x{:X} (virt=0x{:X})",
        id, version, phys_base,
        unsafe { APIC_VIRT_BASE });

    true
}

/// Returns the BSP's Local APIC ID, or None if the APIC isn't ready.
/// Used by `ioapic` to program redirection table destinations.
pub fn local_apic_id() -> Option<u32> {
    if unsafe { APIC_VIRT_BASE } == 0 {
        return None;
    }
    Some(read_reg(REG_ID) >> 24)
}

/// End-Of-Interrupt — must be called from every interrupt handler that
/// fired through the APIC.
#[inline]
pub fn eoi() {
    if unsafe { APIC_VIRT_BASE } != 0 {
        write_reg(REG_EOI, 0);
    }
}

/// Whether the APIC has been successfully initialized.
#[inline]
pub fn is_active() -> bool {
    unsafe { APIC_VIRT_BASE != 0 }
}
