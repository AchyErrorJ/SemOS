//! x86_64 Page Table Management
//!
//! Implements 4-level paging (PML4 → PDPT → PD → PT) with per-process
//! address spaces. Security tiers are enforced by only mapping pool
//! regions the process is authorized to access.
//!
//! # x86_64 Page Table Structure
//!
//! | Level | Name | Entry covers | Index bits     |
//! |-------|------|-------------|----------------|
//! | 4     | PML4 | 512 GB      | VA[47:39]      |
//! | 3     | PDPT | 1 GB        | VA[38:30]      |
//! | 2     | PD   | 2 MB        | VA[29:21]      |
//! | 1     | PT   | 4 KB        | VA[20:12]      |
//!
//! # Memory Layout
//!
//! | Virtual Address Range              | Use                         |
//! |------------------------------------|-----------------------------|
//! | 0x0000_0000_0040_0000 - ...        | User code                   |
//! | 0x0000_0000_0080_0000 - ...        | User data                   |
//! | 0x0000_0000_00C0_0000 - ...        | User heap                   |
//! | 0x0000_007F_FFFF_0000              | User stack top (grows down) |
//! | 0xFFFF_8000_0000_0000 + phys       | Kernel physical map         |
//! | Kernel text/data                   | Bootloader mapped           |
//!
//! # Page Table Entry Format (64-bit)
//!
//! | Bit(s) | Name    | Description                          |
//! |--------|---------|--------------------------------------|
//! | 0      | P       | Present                              |
//! | 1      | R/W     | Read/Write (0 = read-only)           |
//! | 2      | U/S     | User/Supervisor (1 = user accessible)|
//! | 3      | PWT     | Page Write-Through                   |
//! | 4      | PCD     | Page Cache Disable                   |
//! | 5      | A       | Accessed                             |
//! | 6      | D       | Dirty (PT level only)                |
//! | 7      | PS/PAT  | Page Size (1=huge page) / PAT        |
//! | 8      | G       | Global                               |
//! | 12-51  | ADDR    | Physical address of next table/frame |
//! | 63     | NX      | No Execute                           |

use spin::Mutex;
use crate::println;

/// Page sizes
pub const PAGE_SIZE_4K: u64 = 4096;
pub const PAGE_SIZE_2M: u64 = 2 * 1024 * 1024;

/// Number of entries per page table
const ENTRIES: usize = 512;

/// Page table entry flags
mod flags {
    pub const PRESENT: u64      = 1 << 0;
    pub const WRITABLE: u64     = 1 << 1;
    pub const USER: u64         = 1 << 2;
    pub const WRITE_THROUGH: u64 = 1 << 3;
    pub const NO_CACHE: u64     = 1 << 4;
    pub const ACCESSED: u64     = 1 << 5;
    pub const DIRTY: u64        = 1 << 6;
    pub const HUGE_PAGE: u64    = 1 << 7;  // 2MB at PD level, 1GB at PDPT level
    pub const GLOBAL: u64       = 1 << 8;
    pub const NO_EXECUTE: u64   = 1 << 63;

    /// Address mask for 4KB-aligned physical addresses in PTE
    pub const ADDR_MASK: u64    = 0x000F_FFFF_FFFF_F000;
}

/// A single page table entry
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Create an entry pointing to the next-level table
    pub fn table(phys_addr: u64, user: bool) -> Self {
        let mut entry = (phys_addr & flags::ADDR_MASK)
            | flags::PRESENT
            | flags::WRITABLE;
        if user {
            entry |= flags::USER;
        }
        Self(entry)
    }

    /// Create a 4KB page entry
    pub fn page_4k(phys_addr: u64, writable: bool, user: bool, no_execute: bool) -> Self {
        let mut entry = (phys_addr & flags::ADDR_MASK) | flags::PRESENT;
        if writable { entry |= flags::WRITABLE; }
        if user { entry |= flags::USER; }
        if no_execute { entry |= flags::NO_EXECUTE; }
        Self(entry)
    }

    /// Create a 2MB huge page entry (set at PD level)
    pub fn page_2m(phys_addr: u64, writable: bool, user: bool, no_execute: bool) -> Self {
        let mut entry = (phys_addr & !0x1F_FFFF) // 2MB aligned
            | flags::PRESENT
            | flags::HUGE_PAGE;
        if writable { entry |= flags::WRITABLE; }
        if user { entry |= flags::USER; }
        if no_execute { entry |= flags::NO_EXECUTE; }
        Self(entry)
    }

    pub fn is_present(&self) -> bool { self.0 & flags::PRESENT != 0 }
    pub fn is_huge(&self) -> bool { self.0 & flags::HUGE_PAGE != 0 }
    pub fn phys_addr(&self) -> u64 { self.0 & flags::ADDR_MASK }
    pub fn raw(&self) -> u64 { self.0 }
}

/// A page table — 512 entries, 4KB aligned
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; ENTRIES],
}

impl PageTable {
    pub const fn empty() -> Self {
        Self {
            entries: [PageTableEntry::empty(); ENTRIES],
        }
    }

    pub fn entry(&self, index: usize) -> &PageTableEntry {
        &self.entries[index]
    }

    pub fn entry_mut(&mut self, index: usize) -> &mut PageTableEntry {
        &mut self.entries[index]
    }

    /// Clear all entries
    pub fn clear(&mut self) {
        for e in self.entries.iter_mut() {
            *e = PageTableEntry::empty();
        }
    }
}

// ============================================================================
// Page Table Frame Allocator
// ============================================================================

/// Maximum page table frames we can allocate (for page table structures
/// themselves AND, via `map_user_stack`, the 16-frame user stack — so each
/// process draws ~30-40 frames from this pool). The boot demo cascade spawns a
/// dozen short-lived processes; since one-shot demos don't reap, their frames
/// accumulate and the old 512 cap drained right around the agent `bash` /
/// introspection demos (DEMO 52/53), failing the next spawn's stack/segment
/// maps. Two mitigations now exist: the agent `bash` tool reaps its own child
/// at exit (`reap_slot`), so a session looping shell commands stays flat; and
/// this pool is bumped to 2048 (8 MiB reserved) so the non-reaping demo cascade
/// has ample headroom. The fuller fix — free PT frames on every process exit —
/// is still a separate refactor (`reclaim_dead_address_spaces` exists for it).
///
/// M27 D.2-followup: 2048→32768 (128 MiB) to accommodate the Cranelift-
/// bearing semos-cc — ~5.4 MiB ELF mapped + user heap that grows on-demand
/// via SYS_MMAP_ANON as Cranelift allocates during IR construction +
/// codegen. Without enough headroom the user-heap mmap returns null and
/// semos-cc panics inside the Cranelift compile path.
///
/// M27 iter 7: 32768→131072 (512 MiB) for semos-rustc. The 88 MB ELF
/// consumes 22.5K leaf PT_POOL frames per spawn; even with the iter 7
/// reclaim walker, two concurrent ASes (parent sem-sh + child semos-rustc
/// mid-spawn) plus the heap interior PTs blow through 32K. 131072 gives
/// 4× the per-spawn working set as headroom.
const MAX_PT_FRAMES: usize = 131072;

/// Pool of pre-allocated 4KB frames for page table structures.
/// These come from the kernel's usable memory, separate from the security pools.
struct PageTableFramePool {
    /// Physical addresses of available frames
    frames: [u64; MAX_PT_FRAMES],
    /// Number of available frames
    count: usize,
}

impl PageTableFramePool {
    const fn new() -> Self {
        Self {
            frames: [0; MAX_PT_FRAMES],
            count: 0,
        }
    }

    /// Add a frame to the pool
    fn push(&mut self, phys: u64) -> bool {
        if self.count >= MAX_PT_FRAMES {
            return false;
        }
        self.frames[self.count] = phys;
        self.count += 1;
        true
    }

    /// Allocate a frame (returns physical address of a zeroed 4KB page)
    fn alloc(&mut self) -> Option<u64> {
        if self.count == 0 {
            return None;
        }
        self.count -= 1;
        let phys = self.frames[self.count];
        // Zero the frame
        unsafe {
            let virt = phys_to_virt(phys);
            core::ptr::write_bytes(virt as *mut u8, 0, 4096);
        }
        Some(phys)
    }

    /// Return a frame to the pool
    fn free(&mut self, phys: u64) {
        if self.count < MAX_PT_FRAMES {
            self.frames[self.count] = phys;
            self.count += 1;
        }
    }
}

static PT_POOL: Mutex<PageTableFramePool> = Mutex::new(PageTableFramePool::new());

/// Allocate a page table frame
pub fn alloc_pt_frame() -> Option<u64> {
    PT_POOL.lock().alloc()
}

/// Debug-only: read the current count of free frames in PT_POOL.
#[allow(non_snake_case)]
pub fn PT_POOL_DEBUG_count() -> usize {
    PT_POOL.lock().count
}

/// Free a page table frame
pub fn free_pt_frame(phys: u64) {
    PT_POOL.lock().free(phys);
}

// ============================================================================
// Physical-to-Virtual Address Translation
// ============================================================================

/// The offset at which physical memory is mapped into virtual space.
/// Set by the bootloader and stored during init.
static mut PHYS_MEM_OFFSET: u64 = 0;

/// CR3 value of the bootloader's page tables. Captured during paging::init()
/// so kernel tasks (which advertise cr3 = 0) can switch back to a known-good
/// PML4 instead of inheriting whatever isolated address space the previous
/// task was using.
static mut BOOT_CR3: u64 = 0;

/// Get the bootloader's PML4 physical address.
#[inline]
pub fn boot_cr3() -> u64 {
    unsafe { BOOT_CR3 }
}

/// Convert a physical address to a virtual address using the bootloader's mapping
#[inline]
pub fn phys_to_virt(phys: u64) -> u64 {
    unsafe { phys + PHYS_MEM_OFFSET }
}

/// Convert a virtual address (in the physical map region) back to physical.
///
/// **Only valid for addresses in the bootloader's physical-memory map region.**
/// For arbitrary kernel virtual addresses (e.g. function pointers in the
/// kernel image), use [`walk_active_pml4`] instead — the kernel image is at
/// a different virtual offset than the physical-memory map and this helper
/// will produce garbage (often a wrap-around).
#[inline]
pub fn virt_to_phys(virt: u64) -> u64 {
    unsafe { virt - PHYS_MEM_OFFSET }
}

/// Walk the currently active page tables (CR3) to translate any kernel
/// virtual address to its physical address.
///
/// Returns `None` if any level along the path is not present, or if a
/// huge page is encountered (we conservatively reject those for now —
/// add 2MiB / 1GiB handling when we actually use huge pages).
/// Same as [`walk_active_pml4`] but walks a SPECIFIC PML4 (i.e. some
/// other process's address space) instead of the currently-loaded CR3.
/// Used when the kernel needs to read or write a user-virtual address
/// in a process that isn't currently running — e.g. writing argv onto
/// a newly-spawned process's user stack before its first scheduling.
pub fn walk_pml4_for(cr3: u64, virt: u64) -> Option<u64> {
    let cr3 = cr3 & 0x000F_FFFF_FFFF_F000;
    let pml4_idx = ((virt >> 39) & 0x1FF) as usize;
    let pdpt_idx = ((virt >> 30) & 0x1FF) as usize;
    let pd_idx   = ((virt >> 21) & 0x1FF) as usize;
    let pt_idx   = ((virt >> 12) & 0x1FF) as usize;
    let page_off = virt & 0xFFF;
    unsafe {
        let pml4 = &*(phys_to_virt(cr3) as *const PageTable);
        let pml4e = pml4.entry(pml4_idx);
        if !pml4e.is_present() { return None; }
        let pdpt_phys = pml4e.0 & flags::ADDR_MASK;
        let pdpt = &*(phys_to_virt(pdpt_phys) as *const PageTable);
        let pdpte = pdpt.entry(pdpt_idx);
        if !pdpte.is_present() { return None; }
        if pdpte.0 & flags::HUGE_PAGE != 0 { return None; }
        let pd_phys = pdpte.0 & flags::ADDR_MASK;
        let pd = &*(phys_to_virt(pd_phys) as *const PageTable);
        let pde = pd.entry(pd_idx);
        if !pde.is_present() { return None; }
        if pde.0 & flags::HUGE_PAGE != 0 {
            let base_2m = pde.0 & 0x000F_FFFF_FFE0_0000;
            let off_2m = virt & 0x1F_FFFF;
            return Some(base_2m + off_2m);
        }
        let pt_phys = pde.0 & flags::ADDR_MASK;
        let pt = &*(phys_to_virt(pt_phys) as *const PageTable);
        let pte = pt.entry(pt_idx);
        if !pte.is_present() { return None; }
        let page_phys = pte.0 & flags::ADDR_MASK;
        Some(page_phys + page_off)
    }
}

pub fn walk_active_pml4(virt: u64) -> Option<u64> {
    let cr3 = read_cr3() & 0x000F_FFFF_FFFF_F000;
    let pml4_idx = ((virt >> 39) & 0x1FF) as usize;
    let pdpt_idx = ((virt >> 30) & 0x1FF) as usize;
    let pd_idx   = ((virt >> 21) & 0x1FF) as usize;
    let pt_idx   = ((virt >> 12) & 0x1FF) as usize;
    let page_off = virt & 0xFFF;

    unsafe {
        let pml4 = &*(phys_to_virt(cr3) as *const PageTable);
        let pml4e = pml4.entry(pml4_idx);
        if !pml4e.is_present() { return None; }

        let pdpt_phys = pml4e.0 & flags::ADDR_MASK;
        let pdpt = &*(phys_to_virt(pdpt_phys) as *const PageTable);
        let pdpte = pdpt.entry(pdpt_idx);
        if !pdpte.is_present() { return None; }
        // 1 GiB huge page (PS bit set in PDPTE) — not currently expected.
        if pdpte.0 & flags::HUGE_PAGE != 0 { return None; }

        let pd_phys = pdpte.0 & flags::ADDR_MASK;
        let pd = &*(phys_to_virt(pd_phys) as *const PageTable);
        let pde = pd.entry(pd_idx);
        if !pde.is_present() { return None; }
        // 2 MiB huge page — common for kernel image. Compute physical from
        // the 2 MiB-aligned base plus the offset within the 2 MiB page.
        if pde.0 & flags::HUGE_PAGE != 0 {
            let base_2m = pde.0 & 0x000F_FFFF_FFE0_0000;
            let off_2m = virt & 0x1F_FFFF;
            return Some(base_2m + off_2m);
        }

        let pt_phys = pde.0 & flags::ADDR_MASK;
        let pt = &*(phys_to_virt(pt_phys) as *const PageTable);
        let pte = pt.entry(pt_idx);
        if !pte.is_present() { return None; }

        let page_phys = pte.0 & flags::ADDR_MASK;
        Some(page_phys + page_off)
    }
}

/// Get a mutable reference to a page table at a physical address
unsafe fn table_at_phys(phys: u64) -> &'static mut PageTable {
    let virt = phys_to_virt(phys);
    &mut *(virt as *mut PageTable)
}

/// Turn a single 4 KiB page in the ACTIVE (kernel/boot) address space into
/// an unmapped guard page — clears its PRESENT bit so any access faults.
///
/// The kernel image is mapped with 2 MiB huge pages, so if `virt` lands in a
/// huge mapping this first *splits* that 2 MiB page into a fresh 512-entry
/// 4 KiB page table reproducing the existing mapping (flags preserved), then
/// installs it at the PD level. Only then is the target PTE cleared, so all
/// neighbouring kernel data in the same 2 MiB region stays mapped.
///
/// Because every process address space byte-copies the kernel PML4 (see
/// `AddressSpace::new`), it shares these lower page tables — so the unmap is
/// visible under *all* CR3s, not just the boot one. Idempotent: a second
/// guard page in an already-split 2 MiB region just clears its own PTE.
///
/// Operates through raw `u64` pointers (not typed `&mut PageTable`) to avoid
/// the overlapping-`&mut` aliasing hazard documented on `AddressSpace::new`.
/// Returns false if the path isn't mapped or a PT frame can't be allocated.
pub fn install_guard_page(virt: u64) -> bool {
    let virt = virt & !(PAGE_SIZE_4K - 1);
    let cr3 = read_cr3();
    let pml4_idx = ((virt >> 39) & 0x1FF) as usize;
    let pdpt_idx = ((virt >> 30) & 0x1FF) as usize;
    let pd_idx   = ((virt >> 21) & 0x1FF) as usize;
    let pt_idx   = ((virt >> 12) & 0x1FF) as usize;

    unsafe {
        let pml4 = phys_to_virt(cr3) as *const u64;
        let pml4e = *pml4.add(pml4_idx);
        if pml4e & flags::PRESENT == 0 { return false; }

        let pdpt = phys_to_virt(pml4e & flags::ADDR_MASK) as *const u64;
        let pdpte = *pdpt.add(pdpt_idx);
        if pdpte & flags::PRESENT == 0 || pdpte & flags::HUGE_PAGE != 0 { return false; }

        let pd = phys_to_virt(pdpte & flags::ADDR_MASK) as *mut u64;
        let pde = *pd.add(pd_idx);
        if pde & flags::PRESENT == 0 { return false; }

        // Split a 2 MiB huge page into 512 × 4 KiB, preserving its flags.
        if pde & flags::HUGE_PAGE != 0 {
            let base_2m = pde & 0x000F_FFFF_FFE0_0000;
            // Carry over the mapping's protection bits (W/U/global/NX); drop PS.
            let carry = pde & (flags::WRITABLE | flags::USER | flags::GLOBAL | flags::NO_EXECUTE);
            let pt_phys = match alloc_pt_frame() { Some(p) => p, None => return false };
            let pt = phys_to_virt(pt_phys) as *mut u64;
            for i in 0..ENTRIES {
                let page_phys = base_2m + (i as u64) * PAGE_SIZE_4K;
                *pt.add(i) = page_phys | flags::PRESENT | carry;
            }
            // Point the PD entry at the new table (present + writable; user bit
            // mirrors the original so kernel-only stays kernel-only).
            *pd.add(pd_idx) = pt_phys | flags::PRESENT | flags::WRITABLE | (pde & flags::USER);
        }

        // Clear PRESENT on the target 4 KiB page → guard page.
        let pt_phys = *pd.add(pd_idx) & flags::ADDR_MASK;
        let pt = phys_to_virt(pt_phys) as *mut u64;
        *pt.add(pt_idx) &= !flags::PRESENT;
    }

    // Evict any cached translation for this page. `invlpg` (unlike a CR3
    // reload) invalidates the *2 MiB* entry that the split replaced even
    // when it's GLOBAL — kernel pages are global, so a CR3 reload alone
    // would leave the guard readable through a stale huge-page TLB entry.
    invlpg(virt);
    true
}

// ============================================================================
// Per-Process Address Space
// ============================================================================

/// User-space memory layout
pub mod user_layout {
    pub const USER_CODE_BASE: u64   = 0x0000_0000_0040_0000;  // 4MB
    pub const USER_DATA_BASE: u64   = 0x0000_0000_0080_0000;  // 8MB
    pub const USER_HEAP_BASE: u64   = 0x0000_0000_00C0_0000;  // 12MB
    pub const USER_STACK_TOP: u64   = 0x0000_007F_FFFF_0000;  // ~512GB - 64KB
    pub const USER_STACK_SIZE: u64  = 16 * 1024;              // 16KB — must NOT exceed TASK_STACK_SIZE; larger sizes alias adjacent slots' TASK_STACKS in spawn_user_task and corrupt their iret-RIP slot (task #40)
}

/// Page permissions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagePermission {
    /// Read + Execute (code)
    ReadExecute,
    /// Read + Write (data, stack, heap)
    ReadWrite,
    /// Read only
    ReadOnly,
    /// Read + Write + Execute
    ReadWriteExecute,
    /// Kernel only, read + write
    KernelReadWrite,
}

/// Maximum page tables tracked per process (for cleanup). With the
/// recursive `destroy()` walker (iter 7) the subtables[] fast-path is
/// only used as a hint — anything that overflows the cap still gets
/// freed via the tree walk. 32 is fine for small ELFs but kept here
/// historically; bumped to 256 so the walker isn't load-bearing for
/// every spawn.
const MAX_SUBTABLES: usize = 256;

/// Per-process address space.
///
/// Each process gets its own PML4 (top-level page table).
/// CR3 is switched to this PML4 on context switch.
/// Security tier enforcement: only memory pool regions at or below
/// the process's max_tier are mapped.
pub struct AddressSpace {
    /// Physical address of PML4 (loaded into CR3)
    pub cr3: u64,
    /// Allocated sub-table physical addresses (for cleanup)
    subtables: [u64; MAX_SUBTABLES],
    subtable_count: usize,
    /// Maximum security tier this address space can access
    pub max_tier: u8,
}

impl AddressSpace {
    /// Create a new address space.
    ///
    /// Allocates a PML4 and shares the kernel's mappings with the new space.
    /// The bootloader_api crate places the kernel and the physical-memory
    /// map at *low* virtual addresses (e.g. 0x10000000000 ≈ PML4 index 2),
    /// not in the classical higher half — so we copy *every* populated
    /// PML4 entry to inherit the kernel mappings.
    ///
    /// CRITICAL: copy from **`boot_cr3()`** (the clean kernel PML4), NOT
    /// the *live* CR3. When SYS_SPAWN is invoked by a Ring-3 process (e.g.
    /// `std::process::Command`), the live CR3 is the *caller's* address
    /// space, whose lower-half PML4 entries point at the caller's user
    /// page tables. Copying those would make the child SHARE the parent's
    /// lower tables — so mapping the child's ELF segments would scribble
    /// on the parent's address space and crash it (observed: parent SYSRETs
    /// to a corrupted RIP). The boot PML4 has only kernel mappings and no
    /// user entries, so it's the correct base for every new process
    /// regardless of who spawned it.
    pub fn new(max_tier: u8) -> Option<Self> {
        let pml4_phys = alloc_pt_frame()?;
        let kernel_cr3 = boot_cr3();

        // Raw memcpy the entire kernel PML4 (4KB) into the new PML4
        // frame. Going through the typed entry/entry_mut accessors creates
        // overlapping `&'static mut` references to two PageTables and is UB
        // even though we read from one and write to the other; LTO has been
        // observed to elide the writes. A byte-level copy through raw
        // pointers sidesteps the aliasing issue.
        unsafe {
            let src = phys_to_virt(kernel_cr3) as *const u8;
            let dst = phys_to_virt(pml4_phys) as *mut u8;
            core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE_4K as usize);
        }

        Some(Self {
            cr3: pml4_phys,
            subtables: [0; MAX_SUBTABLES],
            subtable_count: 0,
            max_tier,
        })
    }

    /// Map a 4KB page in this address space.
    ///
    /// Allocates intermediate tables (PDPT, PD, PT) as needed.
    pub fn map_4k(&mut self, virt: u64, phys: u64, perm: PagePermission) -> bool {
        // Only map user-space addresses (lower half)
        if virt >= 0x0000_8000_0000_0000 {
            return false;
        }

        let pml4_idx = ((virt >> 39) & 0x1FF) as usize;
        let pdpt_idx = ((virt >> 30) & 0x1FF) as usize;
        let pd_idx   = ((virt >> 21) & 0x1FF) as usize;
        let pt_idx   = ((virt >> 12) & 0x1FF) as usize;

        let is_user = match perm {
            PagePermission::KernelReadWrite => false,
            _ => true,
        };
        let writable = match perm {
            PagePermission::ReadExecute | PagePermission::ReadOnly => false,
            _ => true,
        };
        let no_exec = match perm {
            PagePermission::ReadWrite | PagePermission::ReadOnly
            | PagePermission::KernelReadWrite => true,
            _ => false,
        };

        unsafe {
            let pml4 = table_at_phys(self.cr3);

            let pdpt_phys = match self.ensure_table(pml4, pml4_idx, is_user) {
                Some(p) => p, None => return false,
            };
            let pdpt = table_at_phys(pdpt_phys);

            let pd_phys = match self.ensure_table(pdpt, pdpt_idx, is_user) {
                Some(p) => p, None => return false,
            };
            let pd = table_at_phys(pd_phys);

            let pt_phys = match self.ensure_table(pd, pd_idx, is_user) {
                Some(p) => p, None => return false,
            };
            let pt = table_at_phys(pt_phys);

            *pt.entry_mut(pt_idx) = PageTableEntry::page_4k(phys, writable, is_user, no_exec);
        }

        true
    }

    /// Map a 2MB huge page in this address space.
    pub fn map_2m(&mut self, virt: u64, phys: u64, perm: PagePermission) -> bool {
        if virt >= 0x0000_8000_0000_0000 {
            return false;
        }
        if virt & 0x1F_FFFF != 0 || phys & 0x1F_FFFF != 0 {
            return false; // Must be 2MB aligned
        }

        let pml4_idx = ((virt >> 39) & 0x1FF) as usize;
        let pdpt_idx = ((virt >> 30) & 0x1FF) as usize;
        let pd_idx   = ((virt >> 21) & 0x1FF) as usize;

        let is_user = !matches!(perm, PagePermission::KernelReadWrite);
        let writable = !matches!(perm, PagePermission::ReadExecute | PagePermission::ReadOnly);
        let no_exec = matches!(
            perm,
            PagePermission::ReadWrite | PagePermission::ReadOnly | PagePermission::KernelReadWrite
        );

        unsafe {
            let pml4 = table_at_phys(self.cr3);

            let pdpt_phys = match self.ensure_table(pml4, pml4_idx, is_user) {
                Some(p) => p, None => return false,
            };
            let pdpt = table_at_phys(pdpt_phys);

            let pd_phys = match self.ensure_table(pdpt, pdpt_idx, is_user) {
                Some(p) => p, None => return false,
            };
            let pd = table_at_phys(pd_phys);

            *pd.entry_mut(pd_idx) = PageTableEntry::page_2m(phys, writable, is_user, no_exec);
        }

        true
    }

    /// Map an entire security tier's memory pool into this address space.
    ///
    /// Uses 2MB pages for efficiency. The virtual address is chosen
    /// to mirror the physical address (identity-like mapping in user space).
    pub fn map_security_pool(&mut self, base: u64, size: usize, perm: PagePermission) -> bool {
        let mut offset = 0u64;
        while (offset as usize) < size {
            let phys = base + offset;
            let virt = phys; // Identity map for simplicity
            if !self.map_2m(virt, phys, perm) {
                return false;
            }
            offset += PAGE_SIZE_2M;
        }
        true
    }

    /// Ensure a table entry points to a sub-table, allocating if needed.
    /// Returns the physical address of the sub-table.
    unsafe fn ensure_table(
        &mut self,
        table: &mut PageTable,
        index: usize,
        user: bool,
    ) -> Option<u64> {
        let existing = table.entry(index);
        if existing.is_present() {
            let existing_user = (existing.0 & flags::USER) != 0;
            // If we want user access but the inherited sub-table is
            // kernel-only, we MUST allocate a fresh sub-table rather than
            // OR'ing the USER bit onto the shared inherited entry — the
            // sub-table is shared with the boot address space (and any other
            // process), so mutating its entries would cross-contaminate.
            // Allocating fresh costs us whatever mappings were inherited
            // through this slot, but since this only triggers for user-half
            // addresses, those were bootloader scratch we don't need.
            if user && !existing_user {
                let new_frame = alloc_pt_frame()?;
                *table.entry_mut(index) = PageTableEntry::table(new_frame, user);
                self.track_subtable(new_frame);
                return Some(new_frame);
            }
            return Some(existing.phys_addr());
        }
        let new_frame = alloc_pt_frame()?;
        *table.entry_mut(index) = PageTableEntry::table(new_frame, user);
        self.track_subtable(new_frame);
        Some(new_frame)
    }

    /// Track a subtable for cleanup
    fn track_subtable(&mut self, phys: u64) {
        if self.subtable_count < MAX_SUBTABLES {
            self.subtables[self.subtable_count] = phys;
            self.subtable_count += 1;
        }
    }

    /// Free all page table frames owned by this address space.
    ///
    /// Iter 7 (M27): walks the PML4 user-half (idx 0..256) and frees
    /// every leaf data frame + interior PT frame back to PT_POOL. The
    /// old version only freed `self.subtables[..32]` + PML4, leaking
    /// 22.5K leaf frames per 88 MB ELF spawn — after one semos-rustc
    /// invocation the pool was exhausted.
    ///
    /// Detection: a fresh AS COPIES boot_cr3's PML4 byte-for-byte
    /// (paging.rs:510), so any user-half entry that differs from boot
    /// is process-private. Identical entries are shared/inherited and
    /// MUST NOT be freed (they back kernel mappings + the bootloader
    /// scratch identity map).
    pub fn destroy(&mut self) {
        let mut stats = WalkStats::default();
        unsafe {
            let boot_pml4 = table_at_phys(boot_cr3());
            let proc_pml4 = table_at_phys(self.cr3);
            // User-half PML4 entries (0..256). Kernel half (256..512)
            // is shared with boot and never freed.
            for i in 0..256 {
                let proc_e = proc_pml4.entry(i);
                let boot_e = boot_pml4.entry(i);
                if !proc_e.is_present() { continue; }
                if proc_e.0 == boot_e.0 { continue; } // inherited
                free_pdpt(proc_e.phys_addr(), &mut stats);
            }
        }
        // Free the PML4 itself last.
        free_pt_frame(self.cr3);
        stats.pt_pool_frames += 1;
        let after = PT_POOL.lock().count;
        crate::serial::_print(format_args!(
            "[destroy] freed: {} leaves to pools, {} pages to PT_POOL; PT_POOL now {}\n",
            stats.leaf_to_pool, stats.pt_pool_frames, after));
        self.cr3 = 0;
        self.subtable_count = 0;
    }
}

#[derive(Default)]
struct WalkStats {
    leaf_to_pool: usize,    // leaf data frames returned to security pools
    pt_pool_frames: usize,  // anything returned to PT_POOL (leaves + interior PTs)
}

unsafe fn free_pdpt(phys: u64, stats: &mut WalkStats) {
    let tbl = table_at_phys(phys);
    for i in 0..512 {
        let e = tbl.entry(i);
        if !e.is_present() { continue; }
        if e.is_huge() { continue; }
        free_pd(e.phys_addr(), stats);
    }
    free_pt_frame(phys);
    stats.pt_pool_frames += 1;
}

unsafe fn free_pd(phys: u64, stats: &mut WalkStats) {
    let tbl = table_at_phys(phys);
    for i in 0..512 {
        let e = tbl.entry(i);
        if !e.is_present() { continue; }
        if e.is_huge() { continue; }
        free_pt(e.phys_addr(), stats);
    }
    free_pt_frame(phys);
    stats.pt_pool_frames += 1;
}

unsafe fn free_pt(phys: u64, stats: &mut WalkStats) {
    let tbl = table_at_phys(phys);
    for i in 0..512 {
        let e = tbl.entry(i);
        if !e.is_present() { continue; }
        let leaf = e.phys_addr();
        if crate::memory::free(leaf) {
            stats.leaf_to_pool += 1;
        } else {
            free_pt_frame(leaf);
            stats.pt_pool_frames += 1;
        }
    }
    free_pt_frame(phys);
    stats.pt_pool_frames += 1;
}

// ============================================================================
// CR3 / TLB Management
// ============================================================================

/// Read the current CR3 (PML4 physical address)
#[inline]
pub fn read_cr3() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) value, options(nostack, preserves_flags));
    }
    value & flags::ADDR_MASK
}

/// Write CR3 (switch page tables). This flushes the TLB.
///
/// # Safety
/// The new CR3 must point to a valid PML4 with kernel mappings intact.
#[inline]
pub unsafe fn write_cr3(pml4_phys: u64) {
    core::arch::asm!("mov cr3, {}", in(reg) pml4_phys, options(nostack, preserves_flags));
}

/// Flush a single TLB entry for a virtual address
#[inline]
pub fn invlpg(virt: u64) {
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags));
    }
}

/// Flush entire TLB (by reloading CR3)
#[inline]
pub fn flush_tlb() {
    unsafe {
        let cr3 = read_cr3();
        write_cr3(cr3);
    }
}

// ============================================================================
// Initialization
// ============================================================================

/// Initialize the paging subsystem.
///
/// - Records the physical memory offset from the bootloader
/// - Seeds the page table frame pool from usable memory
/// - Verifies the current page table setup
pub fn init(boot_info: &bootloader_api::BootInfo) {
    // Get the physical memory offset from the bootloader
    let phys_offset = boot_info.physical_memory_offset
        .into_option()
        .expect("Bootloader must provide physical memory offset");

    unsafe {
        PHYS_MEM_OFFSET = phys_offset;
    }

    println!("    Physical memory offset: 0x{:016X}", phys_offset);

    // Read current CR3 (bootloader's PML4) and stash it for later — kernel
    // tasks default to this CR3 instead of leaving the previous task's
    // isolated address space active.
    let cr3 = read_cr3();
    unsafe { BOOT_CR3 = cr3; }
    println!("    Active CR3 (PML4):      0x{:016X}", cr3);

    // Seed the page table frame pool from usable memory
    // We take frames from the end of usable memory to avoid conflicts
    // with the security pools (which use the start of the largest region)
    use bootloader_api::info::MemoryRegionKind;
    let mut frames_added = 0;

    for region in boot_info.memory_regions.iter() {
        if region.kind != MemoryRegionKind::Usable {
            continue;
        }
        // Skip the legacy BIOS conventional memory (under 1 MiB) — it's
        // technically "usable" but contains the IVT, BDA, EBDA, video
        // buffer, etc. We don't want to put page tables there. Also, its
        // boundaries are typically not page-aligned (e.g. 0..0x9FC00),
        // which would yield misaligned frames.
        if region.end <= 0x100_000 {
            continue;
        }
        // Round endpoints to page boundaries: start UP, end DOWN. This
        // guarantees every produced frame address is page-aligned, even if
        // the BIOS reports oddly-shaped regions.
        let start_aligned = (region.start + PAGE_SIZE_4K - 1) & !(PAGE_SIZE_4K - 1);
        let end_aligned = region.end & !(PAGE_SIZE_4K - 1);
        if end_aligned <= start_aligned {
            continue;
        }
        let size = end_aligned - start_aligned;
        if size < (MAX_PT_FRAMES as u64 * PAGE_SIZE_4K) {
            continue; // Too small
        }

        // Take frames from the end of this region (now guaranteed aligned).
        let mut pool = PT_POOL.lock();
        let start = end_aligned - (MAX_PT_FRAMES as u64 * PAGE_SIZE_4K);
        for i in 0..MAX_PT_FRAMES {
            let addr = start + (i as u64 * PAGE_SIZE_4K);
            if pool.push(addr) {
                frames_added += 1;
            }
        }
        break; // Only need frames from one region
    }

    println!("    Page table frame pool:  {} frames ({} KB)",
        frames_added, frames_added * 4);

    // Verify we can read the current page tables
    unsafe {
        let pml4 = table_at_phys(cr3);
        let mut mapped_regions = 0;
        for i in 0..512 {
            if pml4.entry(i).is_present() {
                mapped_regions += 1;
            }
        }
        println!("    Active PML4 entries:     {}/512", mapped_regions);
    }
}

/// Create an address space for a process, mapping only allowed security tiers.
///
/// The process gets:
/// - Kernel higher-half mappings (inherited from boot PML4)
/// - Security pool memory for tiers 0..=max_tier (as 2MB pages)
pub fn create_process_address_space(max_tier: u8) -> Option<AddressSpace> {
    let mut space = AddressSpace::new(max_tier)?;

    // Map security pools based on tier access
    let pools = crate::memory::pool_info();
    for (tier_val, base, size) in pools.iter() {
        if *tier_val <= max_tier {
            let perm = if *tier_val <= 1 {
                // Public and Internal: read+write (data access)
                PagePermission::ReadWrite
            } else {
                // Sensitive and Secret: read+write but no execute
                PagePermission::ReadWrite
            };
            space.map_security_pool(*base, *size as usize, perm);
        }
    }

    Some(space)
}
