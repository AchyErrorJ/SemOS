//! AArch64 page-table management.
//!
//! Provides the boot identity map, a per-process `AddressSpace`, and the
//! Platform-trait wrappers that kernel-core uses to load ELF segments and
//! manage user stacks.

use kernel_core::scheduler::MAX_TASKS;

const PAGE_SIZE: usize = 4096;
const ENTRIES: usize = 512;
const MAX_SUBTABLES: usize = 256;

// ---- Descriptor bit definitions ---------------------------------------------

const VALID: u64 = 1;
// Table/page entries: bits[1:0] == 0b11.  Block entries: bits[1:0] == 0b01.
const DESC_TABLE: u64 = 0b11;
const DESC_BLOCK: u64 = 0b01;

const AF: u64 = 1 << 10; // Access flag
const ATTR0: u64 = 0 << 2; // MAIR Attr0 = device-nGnRnE
const ATTR1: u64 = 1 << 2; // MAIR Attr1 = normal WB
const SH_INNER: u64 = 0b11 << 8; // Inner shareable

const UXN: u64 = 1 << 54; // Unprivileged execute-never
const PXN: u64 = 1 << 53; // Privileged execute-never

const AP_RW_EL1: u64 = 0b00 << 6;
const AP_RO_EL1: u64 = 0b10 << 6;
const AP_RW_EL0: u64 = 0b01 << 6;
const AP_RO_EL0: u64 = 0b11 << 6;

const OUTPUT_MASK_1G: u64 = 0x0000_FFFF_C000_0000; // level-1 block
const OUTPUT_MASK_2M: u64 = 0x0000_FFFF_FFE0_0000; // level-2 block
const OUTPUT_MASK_4K: u64 = 0x0000_FFFF_FFFF_F000; // level-3 page / table

/// Permission class for a user mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagePermission {
    ReadOnly,
    ReadWrite,
    ReadExecute,
    ReadWriteExecute,
}

/// A 4 KiB-aligned page table with 512 entries.
#[repr(C, align(4096))]
#[derive(Clone, Copy)]
pub struct PageTable {
    entries: [u64; ENTRIES],
}

impl PageTable {
    const fn empty() -> Self {
        Self { entries: [0; ENTRIES] }
    }
}

// ---- Boot identity map ------------------------------------------------------

/// The boot L1 table used by the kernel. User address spaces copy its entries
/// so kernel mappings remain accessible after a TTBR0 switch.
static mut BOOT_L1_TABLE: PageTable = PageTable::empty();

/// A level-1 1 GiB block descriptor for the boot identity map.
fn block_1g_desc(phys: u64, attr_idx: u64, normal: bool) -> u64 {
    let mut d = (phys & OUTPUT_MASK_1G)
        | DESC_BLOCK
        | (attr_idx << 2)
        | AF;
    if normal {
        d |= SH_INNER;
    }
    d
}

/// Build the boot identity map and enable the MMU (SCTLR_EL1.M). Caches left
/// off (C/I=0) for safety — correct, just uncached.
pub unsafe fn enable_identity_mmu() {
    let l1 = core::ptr::addr_of_mut!(BOOT_L1_TABLE.entries) as *mut u64;
    *l1.add(0) = block_1g_desc(0x0000_0000, 0, false); // device (UART)
    *l1.add(1) = block_1g_desc(0x4000_0000, 1, true); // normal RAM (kernel)

    let ttbr0 = core::ptr::addr_of!(BOOT_L1_TABLE) as u64;
    // MAIR: Attr0 = 0x00 device-nGnRnE, Attr1 = 0xFF normal write-back.
    let mair: u64 = (0xFF << 8) | 0x00;
    // TCR: T0SZ=25, IRGN0/ORGN0=WB, SH0=inner, TG0=4KiB, EPD1 (no TTBR1), IPS=40-bit.
    let tcr: u64 = 25 | (0b01 << 8) | (0b01 << 10) | (0b11 << 12) | (1 << 23) | (0b010 << 32);

    core::arch::asm!(
        "msr mair_el1, {mair}",
        "msr tcr_el1,  {tcr}",
        "msr ttbr0_el1,{ttbr}",
        "dsb sy",
        "isb",
        "mrs {tmp}, sctlr_el1",
        "orr {tmp}, {tmp}, #1",   // SCTLR_EL1.M = 1
        "msr sctlr_el1, {tmp}",
        "isb",
        mair = in(reg) mair,
        tcr = in(reg) tcr,
        ttbr = in(reg) ttbr0,
        tmp = out(reg) _,
    );
}

/// Return the physical address of the boot L1 table (the kernel's TTBR0).
pub fn boot_ttbr0() -> u64 {
    core::ptr::addr_of!(BOOT_L1_TABLE) as u64
}

// ---- Helpers ----------------------------------------------------------------

#[inline]
fn phys_to_virt(p: u64) -> u64 {
    // The kernel identity-map covers all RAM and the device region we use.
    p
}

#[inline]
unsafe fn page_table_from_phys(p: u64) -> *mut PageTable {
    phys_to_virt(p) as *mut PageTable
}

#[inline]
unsafe fn zero_page(p: u64) {
    core::ptr::write_bytes(phys_to_virt(p) as *mut u8, 0, PAGE_SIZE);
}

#[inline]
fn is_table(desc: u64) -> bool {
    (desc & 0b11) == DESC_TABLE
}
#[inline]
fn is_block(desc: u64) -> bool {
    (desc & 0b11) == DESC_BLOCK
}
#[inline]
fn is_valid(desc: u64) -> bool {
    (desc & VALID) != 0
}
#[inline]
fn desc_phys(desc: u64) -> u64 {
    desc & OUTPUT_MASK_4K
}

fn table_desc(phys: u64) -> u64 {
    (phys & OUTPUT_MASK_4K) | DESC_TABLE | AF
}

fn page_4k_desc(phys: u64, perm: PagePermission) -> u64 {
    let mut d = (phys & OUTPUT_MASK_4K)
        | DESC_TABLE // 0b11 for level-3 page
        | AF
        | ATTR1
        | SH_INNER;
    match perm {
        PagePermission::ReadOnly => {
            d |= AP_RO_EL0 | UXN | PXN;
        }
        PagePermission::ReadWrite => {
            d |= AP_RW_EL0 | UXN | PXN;
        }
        PagePermission::ReadExecute => {
            d |= AP_RO_EL0 | PXN; // UXN = 0 so user can execute
        }
        PagePermission::ReadWriteExecute => {
            d |= AP_RW_EL0 | PXN; // UXN = 0
        }
    }
    d
}

/// Create a 2 MiB block descriptor that mirrors the attributes of a 1 GiB
/// parent block descriptor.
fn block_2m_from_parent(parent: u64, phys: u64) -> u64 {
    // Inherit attribute bits: AttrIndx, SH, AP, UXN, PXN, AF.
    let attr_mask = ATTR1 | ATTR0 | SH_INNER | (0b11 << 6) | UXN | PXN | AF;
    (parent & attr_mask) | (phys & OUTPUT_MASK_2M) | DESC_BLOCK | VALID
}

/// Create a 4 KiB page descriptor that mirrors the attributes of a 2 MiB
/// parent block descriptor.
fn page_4k_from_parent(parent: u64, phys: u64) -> u64 {
    let attr_mask = ATTR1 | ATTR0 | SH_INNER | (0b11 << 6) | UXN | PXN | AF;
    (parent & attr_mask) | (phys & OUTPUT_MASK_4K) | DESC_TABLE | VALID
}

/// Allocate a fresh, zeroed page-table frame.
unsafe fn alloc_pt_frame() -> Option<u64> {
    let p = crate::memory::alloc()?;
    zero_page(p);
    Some(p)
}

// ---- AddressSpace -----------------------------------------------------------

/// A user (or kernel-isolated) address space.
#[derive(Clone, Copy)]
pub struct AddressSpace {
    pub ttbr0: u64,
    subtables: [u64; MAX_SUBTABLES],
    subtable_count: usize,
    pub max_tier: u8,
}

static mut ADDRESS_SPACES: [Option<AddressSpace>; MAX_TASKS] = [None; MAX_TASKS];

/// Create a new address space: fresh L1 table with the kernel identity entries
/// copied from the boot table.
pub unsafe fn new_address_space(max_tier: u8) -> Option<AddressSpace> {
    let l1_phys = alloc_pt_frame()?;
    let l1 = page_table_from_phys(l1_phys);
    // Copy boot entries so kernel mappings remain visible after TTBR0 switches.
    let boot = core::ptr::addr_of!(BOOT_L1_TABLE) as *const PageTable;
    (*l1).entries.copy_from_slice(&(*boot).entries);

    Some(AddressSpace {
        ttbr0: l1_phys,
        subtables: [0; MAX_SUBTABLES],
        subtable_count: 0,
        max_tier,
    })
}

/// Store an address space so `destroy_address_space` can find it later.
pub unsafe fn store_address_space(space: AddressSpace) {
    let arr = &raw mut ADDRESS_SPACES;
    for i in 0..MAX_TASKS {
        if (*arr)[i].is_none() {
            (*arr)[i] = Some(space);
            return;
        }
    }
}

unsafe fn find_address_space(ttbr0: u64) -> Option<&'static mut AddressSpace> {
    let arr = &raw mut ADDRESS_SPACES;
    for i in 0..MAX_TASKS {
        if let Some(ref mut s) = (*arr)[i] {
            if s.ttbr0 == ttbr0 {
                return Some(s);
            }
        }
    }
    None
}

/// Return a mutable pointer to the L1 table of the given address space.
unsafe fn l1_of(space: &mut AddressSpace) -> *mut PageTable {
    page_table_from_phys(space.ttbr0)
}

/// Ensure `l1[idx]` points to a valid L2 table. If it currently holds a 1 GiB
/// block, split it into 512 2 MiB block entries in a newly-allocated L2 table.
unsafe fn ensure_l2(space: &mut AddressSpace, l1_idx: usize) -> Option<*mut PageTable> {
    let l1 = l1_of(space);
    let desc = (*l1).entries[l1_idx];
    if is_valid(desc) {
        if is_table(desc) {
            return Some(page_table_from_phys(desc_phys(desc)));
        }
        if is_block(desc) {
            // Split 1 GiB block into 512 × 2 MiB blocks.
            let l2_phys = alloc_pt_frame()?;
            let l2 = page_table_from_phys(l2_phys);
            let base = desc & OUTPUT_MASK_1G;
            for i in 0..ENTRIES {
                let phys = base + (i as u64) * (2 * 1024 * 1024);
                (*l2).entries[i] = block_2m_from_parent(desc, phys);
            }
            (*l1).entries[l1_idx] = table_desc(l2_phys);
            return Some(l2);
        }
    }
    // Invalid: allocate fresh L2 table.
    let l2_phys = alloc_pt_frame()?;
    let l2 = page_table_from_phys(l2_phys);
    (*l1).entries[l1_idx] = table_desc(l2_phys);
    Some(l2)
}

/// Ensure `l2[idx]` points to a valid L3 table. If it currently holds a 2 MiB
/// block, split it into 512 4 KiB page entries in a newly-allocated L3 table.
unsafe fn ensure_l3(l2: *mut PageTable, l2_idx: usize) -> Option<*mut PageTable> {
    let desc = (*l2).entries[l2_idx];
    if is_valid(desc) {
        if is_table(desc) {
            return Some(page_table_from_phys(desc_phys(desc)));
        }
        if is_block(desc) {
            let l3_phys = alloc_pt_frame()?;
            let l3 = page_table_from_phys(l3_phys);
            let base = desc & OUTPUT_MASK_2M;
            for i in 0..ENTRIES {
                let phys = base + (i as u64) * PAGE_SIZE as u64;
                (*l3).entries[i] = page_4k_from_parent(desc, phys);
            }
            (*l2).entries[l2_idx] = table_desc(l3_phys);
            return Some(l3);
        }
    }
    let l3_phys = alloc_pt_frame()?;
    let l3 = page_table_from_phys(l3_phys);
    (*l2).entries[l2_idx] = table_desc(l3_phys);
    Some(l3)
}

/// Map a single 4 KiB page into the address space.
pub unsafe fn map_4k(space: &mut AddressSpace, virt: u64, phys: u64, perm: PagePermission) -> bool {
    // For this phase keep user mappings in the first 1 GiB, away from the
    // kernel identity blocks at indices 0 and 1 (device + RAM).
    if virt >= (1u64 << 30) {
        return false;
    }
    let l1_idx = ((virt >> 30) & 0x1FF) as usize;
    let l2_idx = ((virt >> 21) & 0x1FF) as usize;
    let l3_idx = ((virt >> 12) & 0x1FF) as usize;

    let l2 = match ensure_l2(space, l1_idx) {
        Some(t) => t,
        None => return false,
    };
    let l3 = match ensure_l3(l2, l2_idx) {
        Some(t) => t,
        None => return false,
    };
    (*l3).entries[l3_idx] = page_4k_desc(phys, perm);
    true
}

/// Walk the address space and free every private table frame and every leaf
/// data frame that came from our pool. Inherited boot entries are left alone.
pub unsafe fn destroy_address_space(ttbr0: u64) {
    let space = match find_address_space(ttbr0) {
        Some(s) => s,
        None => return,
    };
    let l1 = page_table_from_phys(space.ttbr0);
    let boot = core::ptr::addr_of!(BOOT_L1_TABLE) as *const PageTable;

    for i in 0..ENTRIES {
        let desc = (*l1).entries[i];
        let boot_desc = (*boot).entries[i];
        if !is_valid(desc) || desc == boot_desc {
            continue;
        }
        if !is_table(desc) {
            continue;
        }
        let l2_phys = desc_phys(desc);
        let l2 = page_table_from_phys(l2_phys);
        for j in 0..ENTRIES {
            let l2_desc = (*l2).entries[j];
            if !is_valid(l2_desc) || !is_table(l2_desc) {
                continue;
            }
            let l3_phys = desc_phys(l2_desc);
            let l3 = page_table_from_phys(l3_phys);
            for k in 0..ENTRIES {
                let l3_desc = (*l3).entries[k];
                if is_valid(l3_desc) {
                    // Free leaf frames that came from our pool.  Mirrored
                    // device pages are outside the pool, so `free` returns
                    // false harmlessly.
                    crate::memory::free(desc_phys(l3_desc));
                }
            }
            crate::memory::free(l3_phys);
        }
        crate::memory::free(l2_phys);
    }
    crate::memory::free(space.ttbr0);

    // Clear the stored reference.
    let arr = &raw mut ADDRESS_SPACES;
    for i in 0..MAX_TASKS {
        if let Some(s) = (*arr)[i] {
            if s.ttbr0 == ttbr0 {
                (*arr)[i] = None;
                break;
            }
        }
    }
}

// ---- Platform wrappers ------------------------------------------------------

/// Map `size` bytes of fresh, zeroed, RW user memory into `ttbr0` at `addr`.
pub unsafe fn map_user_region(ttbr0: u64, addr: u64, size: u64) -> bool {
    let space = match find_address_space(ttbr0) {
        Some(s) => s,
        None => return false,
    };
    let start = addr & !(PAGE_SIZE as u64 - 1);
    let end = (addr + size + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
    let mut page = start;
    while page < end {
        let frame = match crate::memory::alloc() {
            Some(f) => f,
            None => return false,
        };
        zero_page(frame);
        if !map_4k(space, page, frame, PagePermission::ReadWrite) {
            crate::memory::free(frame);
            return false;
        }
        page += PAGE_SIZE as u64;
    }
    true
}

/// Map a user stack of `stack_size` bytes ending at `stack_top`.
pub unsafe fn map_user_stack(ttbr0: u64, stack_top: u64, stack_size: u64) -> Option<u64> {
    let space = match find_address_space(ttbr0) {
        Some(s) => s,
        None => return None,
    };
    let stack_bottom = stack_top - stack_size;
    let pages = (stack_size + PAGE_SIZE as u64 - 1) / PAGE_SIZE as u64;
    for i in 0..pages {
        let virt = stack_bottom + i * PAGE_SIZE as u64;
        let frame = crate::memory::alloc()?;
        zero_page(frame);
        if !map_4k(space, virt, frame, PagePermission::ReadWrite) {
            return None;
        }
    }
    Some(stack_top & !0xF)
}

/// Map an ELF load segment into the address space. `data` is the file content,
/// `memsz` the in-memory size (zero-fill the tail).
pub unsafe fn map_elf_segment(
    ttbr0: u64,
    virt_addr: u64,
    data: &[u8],
    memsz: usize,
    executable: bool,
    writable: bool,
) -> bool {
    let space = match find_address_space(ttbr0) {
        Some(s) => s,
        None => return false,
    };
    let perm = match (executable, writable) {
        (true, false) => PagePermission::ReadExecute,
        (false, true) => PagePermission::ReadWrite,
        (true, true) => PagePermission::ReadWriteExecute,
        (false, false) => PagePermission::ReadOnly,
    };

    let start_page = virt_addr & !(PAGE_SIZE as u64 - 1);
    let end = virt_addr + memsz as u64;
    let end_page = (end + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

    let mut page = start_page;
    while page < end_page {
        let frame = match crate::memory::alloc() {
            Some(f) => f,
            None => return false,
        };
        zero_page(frame);

        // Copy file data that falls within this page.
        let frame_virt = phys_to_virt(frame) as *mut u8;
        let page_offset_in_seg = if page >= virt_addr {
            (page - virt_addr) as usize
        } else {
            0
        };
        let copy_start_in_page = if virt_addr > page {
            (virt_addr - page) as usize
        } else {
            0
        };
        if page_offset_in_seg < data.len() {
            let remaining = data.len() - page_offset_in_seg;
            let copy_len = remaining.min(PAGE_SIZE - copy_start_in_page);
            core::ptr::copy_nonoverlapping(
                data.as_ptr().add(page_offset_in_seg),
                frame_virt.add(copy_start_in_page),
                copy_len,
            );
        }

        if !map_4k(space, page, frame, perm) {
            crate::memory::free(frame);
            return false;
        }
        page += PAGE_SIZE as u64;
    }
    true
}

// ---- TTBR0 / TLB ------------------------------------------------------------

/// Read the current `TTBR0_EL1` value.
pub unsafe fn read_ttbr0() -> u64 {
    let v: u64;
    core::arch::asm!("mrs {}, ttbr0_el1", out(reg) v);
    v & 0x0000_FFFF_FFFF_FFFF
}

/// Switch `TTBR0_EL1` and flush the local TLB.
pub unsafe fn write_ttbr0(ttbr0: u64) {
    core::arch::asm!(
        "msr ttbr0_el1, {ttbr}",
        "tlbi vmalle1",
        "dsb ish",
        "isb",
        ttbr = in(reg) ttbr0,
    );
}

/// Flush the entire stage-1 TLB for the current VMID.
pub unsafe fn tlb_flush_all() {
    core::arch::asm!(
        "tlbi vmalle1",
        "dsb ish",
        "isb",
    );
}

// ---- Reclamation (stub for this phase) --------------------------------------

pub fn reclaim_dead_address_spaces() -> usize {
    // TODO: scan `crate::context::CONTEXTS` for dead TTBR0 values and destroy
    // them.  Not needed until concurrent user spawn/reap is exercised.
    0
}
