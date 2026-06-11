//! Firmware-table probes: SMBIOS/DMI (machine identity) + ACPI DMAR (VT-d).
//!
//! Both are read-only diagnostics that report what the platform firmware
//! published. They answer two questions before the VT-d subsystem is built:
//!   1. What machine is this, really? (SMBIOS Type 1 System Information)
//!   2. Does it expose an IOMMU we can program? (ACPI DMAR table presence
//!      + the DRHD register base(s) the VT-d driver will need)
//!
//! All physical reads go through `paging::phys_to_virt` (the bootloader
//! identity-maps physical RAM at that offset).

use crate::println;
use crate::paging::phys_to_virt;

// ---- physical-memory read helpers -----------------------------------------

unsafe fn rd8(phys: u64) -> u8 {
    core::ptr::read_volatile(phys_to_virt(phys) as *const u8)
}
unsafe fn rd16(phys: u64) -> u16 {
    core::ptr::read_volatile(phys_to_virt(phys) as *const u16)
}
unsafe fn rd32(phys: u64) -> u32 {
    core::ptr::read_volatile(phys_to_virt(phys) as *const u32)
}
unsafe fn rd64(phys: u64) -> u64 {
    core::ptr::read_volatile(phys_to_virt(phys) as *const u64)
}

// ============================================================================
// SMBIOS / DMI — machine identity
// ============================================================================

/// Scan the legacy BIOS segment for the SMBIOS entry point and print the
/// system Manufacturer / Product Name / Version (DMI Type 1). This is the
/// authoritative machine model — the case badge can lie, this can't.
pub fn dmi_system_info() {
    // SMBIOS 2.x entry point ("_SM_") or 3.x ("_SM3_") lives on a 16-byte
    // boundary in 0x000F0000–0x000FFFFF on BIOS systems.
    let mut table_addr: u64 = 0;
    let mut table_len: u64 = 0;
    // Readability check: if the legacy BIOS segment reads all-0xFF the
    // region isn't shadowed/mapped (common on UEFI/CSM) and a legacy scan
    // can't work — SMBIOS would then need the EFI config table instead.
    let probe0 = unsafe { rd32(0xF_0000) };
    let probe1 = unsafe { rd32(0xF_8000) };
    if probe0 == 0xFFFF_FFFF && probe1 == 0xFFFF_FFFF {
        println!("[dmi] BIOS segment 0xF0000 reads all-0xFF (not shadowed; likely UEFI/CSM) — SMBIOS via legacy scan unavailable");
        return;
    }
    'scan: for off in (0xF_0000u64..0x10_0000u64).step_by(16) {
        let sig0 = unsafe { rd32(off) };
        if sig0 == u32::from_le_bytes(*b"_SM_") {
            // 32-bit entry point: struct table addr @ +0x18, count @ +0x1C.
            table_addr = unsafe { rd32(off + 0x18) } as u64;
            table_len = unsafe { rd16(off + 0x16) } as u64;
            println!("[dmi] SMBIOS 2.x entry @ 0x{:05X}", off);
            break 'scan;
        }
        // "_SM3_" is 5 bytes; check the first 4 then the 5th.
        if sig0 == u32::from_le_bytes(*b"_SM3") && unsafe { rd8(off + 4) } == b'_' {
            table_addr = unsafe { rd64(off + 0x10) };
            table_len = unsafe { rd32(off + 0x0C) } as u64;
            println!("[dmi] SMBIOS 3.x entry @ 0x{:05X}", off);
            break 'scan;
        }
    }
    if table_addr == 0 {
        println!("[dmi] no SMBIOS entry point found (UEFI boot?) — machine ID unavailable");
        return;
    }

    // Walk DMI structures looking for Type 1 (System Information).
    let end = table_addr + table_len;
    let mut p = table_addr;
    while p + 4 <= end {
        let stype = unsafe { rd8(p) };
        let flen = unsafe { rd8(p + 1) } as u64; // formatted-area length
        let strings = p + flen; // string table starts after the formatted area

        if stype == 1 {
            // Type 1: Manufacturer @ +4, Product @ +5, Version @ +6 (string idx).
            let man = unsafe { rd8(p + 4) };
            let prod = unsafe { rd8(p + 5) };
            let ver = unsafe { rd8(p + 6) };
            print_dmi_string("Manufacturer", strings, end, man);
            print_dmi_string("Product Name", strings, end, prod);
            print_dmi_string("Version", strings, end, ver);
        }
        // Advance past the string table (terminated by a double NUL).
        let mut s = strings;
        // A structure with no strings still has a terminating double-NUL.
        if unsafe { rd8(s) } == 0 && unsafe { rd8(s + 1) } == 0 {
            p = s + 2;
            if stype == 127 { break; } // End-of-table marker.
            continue;
        }
        loop {
            // skip one NUL-terminated string
            while s < end && unsafe { rd8(s) } != 0 { s += 1; }
            s += 1; // past the NUL
            if s >= end || unsafe { rd8(s) } == 0 {
                s += 1; // past the final NUL of the double-NUL
                break;
            }
        }
        p = s;
        if stype == 127 { break; }
    }
}

/// Resolve a 1-based DMI string index within `[strings, end)` and print it.
fn print_dmi_string(label: &str, strings: u64, end: u64, index: u8) {
    if index == 0 {
        return; // 0 = "not specified"
    }
    let mut s = strings;
    let mut remaining = index;
    while remaining > 1 {
        while s < end && unsafe { rd8(s) } != 0 { s += 1; }
        s += 1;
        remaining -= 1;
        if s >= end { return; }
    }
    // Print the string at `s` (bounded).
    let mut buf = [0u8; 64];
    let mut n = 0;
    while s < end && n < buf.len() {
        let c = unsafe { rd8(s) };
        if c == 0 { break; }
        buf[n] = c;
        n += 1;
        s += 1;
    }
    if let Ok(text) = core::str::from_utf8(&buf[..n]) {
        println!("[dmi]   {}: {}", label, text);
    }
}

// ============================================================================
// ACPI DMAR — VT-d / IOMMU detection
// ============================================================================

/// From the RSDP, find the ACPI DMAR table. Reports whether VT-d is present
/// and, if so, the DRHD register base address(es) the IOMMU driver needs.
/// Returns the first DRHD register base, or None if no DMAR table.
pub fn acpi_find_dmar(rsdp_phys: u64) -> Option<u64> {
    // RSDP: revision @ +15, rsdt_addr(u32) @ +16, xsdt_addr(u64) @ +24.
    let revision = unsafe { rd8(rsdp_phys + 15) };
    let (entries_phys, entry_size, count) = if revision >= 2 {
        let xsdt = unsafe { rd64(rsdp_phys + 24) };
        let len = unsafe { rd32(xsdt + 4) } as u64;
        let count = (len.saturating_sub(36)) / 8;
        println!("[acpi] RSDP@0x{:X} rev={} → XSDT@0x{:X} ({} tables)", rsdp_phys, revision, xsdt, count);
        (xsdt + 36, 8u64, count)
    } else {
        let rsdt = unsafe { rd32(rsdp_phys + 16) } as u64;
        let len = unsafe { rd32(rsdt + 4) } as u64;
        let count = (len.saturating_sub(36)) / 4;
        println!("[acpi] RSDP@0x{:X} rev={} → RSDT@0x{:X} ({} tables)", rsdp_phys, revision, rsdt, count);
        (rsdt + 36, 4u64, count)
    };

    // Dump every table signature so we can confirm the walk works on real
    // hardware (and see whether DMAR is simply absent vs. the walk broken).
    let mut dmar_phys = 0u64;
    let mut line = [0u8; 5 * 24];
    let mut n = 0usize;
    for i in 0..count.min(48) {
        let entry = entries_phys + i * entry_size;
        let table_phys = if entry_size == 8 {
            unsafe { rd64(entry) }
        } else {
            unsafe { rd32(entry) as u64 }
        };
        let sig = unsafe { rd32(table_phys) };
        let b = sig.to_le_bytes();
        for &c in &b {
            if n < line.len() { line[n] = if c.is_ascii_graphic() { c } else { b'?' }; n += 1; }
        }
        if n < line.len() { line[n] = b' '; n += 1; }
        if sig == u32::from_le_bytes(*b"DMAR") {
            dmar_phys = table_phys;
        }
    }
    if let Ok(s) = core::str::from_utf8(&line[..n]) {
        println!("[acpi] tables: {}", s);
    }

    if dmar_phys == 0 {
        println!("[acpi] no DMAR table — VT-d not available (absent or disabled in BIOS)");
        return None;
    }

    // DMAR header: std SDT header (36) + host_addr_width(1) + flags(1) +
    // reserved(10), then remapping structures from offset 48.
    let dmar_len = unsafe { rd32(dmar_phys + 4) } as u64;
    let haw = unsafe { rd8(dmar_phys + 36) };
    let flags = unsafe { rd8(dmar_phys + 37) };
    println!("[acpi] DMAR found — VT-d AVAILABLE (host_addr_width={} bits, flags=0x{:02X})",
        haw as u16 + 1, flags);

    // Walk remapping structures; report each DRHD (type 0) register base.
    let mut first_drhd = None;
    let mut p = dmar_phys + 48;
    let end = dmar_phys + dmar_len;
    while p + 4 <= end {
        let rtype = unsafe { rd16(p) };
        let rlen = unsafe { rd16(p + 2) } as u64;
        if rlen == 0 { break; }
        if rtype == 0 {
            // DRHD: flags @ +4, segment @ +6, register_base(u64) @ +8.
            let drhd_flags = unsafe { rd8(p + 4) };
            let reg_base = unsafe { rd64(p + 8) };
            println!("[acpi]   DRHD register_base=0x{:016X} flags=0x{:02X} (INCLUDE_PCI_ALL={})",
                reg_base, drhd_flags, drhd_flags & 1);
            if first_drhd.is_none() {
                first_drhd = Some(reg_base);
            }
        }
        p += rlen;
    }
    first_drhd
}

/// One-shot probe: print machine identity + VT-d availability. Call after
/// paging init (phys_to_virt must work). `rsdp_phys` from BootInfo.
pub fn probe(rsdp_phys: Option<u64>) {
    println!("[*] Firmware probe (machine identity + VT-d)...");
    dmi_system_info();
    match rsdp_phys {
        Some(addr) => { let _ = acpi_find_dmar(addr); }
        None => println!("[acpi] no RSDP from bootloader — cannot check for VT-d"),
    }
}
