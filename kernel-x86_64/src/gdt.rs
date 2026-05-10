//! Global Descriptor Table (GDT) and Task State Segment (TSS)
//!
//! Sets up the segment descriptors needed for Ring 0 ↔ Ring 3 transitions
//! and the TSS that provides kernel stacks for privilege-level changes.
//!
//! # GDT Layout
//!
//! | Index | Selector | Description                  | DPL |
//! |-------|----------|------------------------------|-----|
//! | 0     | 0x00     | Null descriptor              | -   |
//! | 1     | 0x08     | Kernel Code (64-bit)         | 0   |
//! | 2     | 0x10     | Kernel Data                  | 0   |
//! | 3     | 0x18     | User Data                    | 3   |
//! | 4     | 0x20     | User Code (64-bit)           | 3   |
//! | 5-6   | 0x28     | TSS (16-byte descriptor)     | 0   |
//!
//! # Why this order?
//!
//! SYSRET loads CS = STAR[63:48] + 16, SS = STAR[63:48] + 8.
//! With STAR[63:48] = 0x18 (user data base):
//!   - SS = 0x18 | 3 = user data (index 3)
//!   - CS = 0x28 | 3... wait, that's TSS.
//!
//! Actually, SYSRET in 64-bit mode:
//!   - CS = STAR[63:48] + 16 = user_base + 16
//!   - SS = STAR[63:48] + 8  = user_base + 8
//!
//! So we need: user_base + 8 = User Data, user_base + 16 = User Code.
//! If user_base = 0x10: SS = 0x18 (User Data), CS = 0x20 (User Code). But 0x10 is Kernel Data!
//!
//! The trick: STAR stores the base without RPL. CPU adds RPL=3 automatically.
//! Standard layout: Kernel CS=0x08, Kernel DS=0x10, User DS=0x18, User CS=0x20
//! STAR[47:32] = 0x08 (kernel), STAR[63:48] = 0x10 (user base)
//! SYSRET: SS = 0x10+8 = 0x18|3 = User Data, CS = 0x10+16 = 0x20|3 = User Code ✓
//! SYSCALL: CS = 0x08 = Kernel Code, SS = 0x08+8 = 0x10 = Kernel Data ✓

use spin::Lazy;
use x86_64::structures::gdt::{GlobalDescriptorTable, Descriptor, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::registers::segmentation::{CS, DS, SS, Segment};
use x86_64::instructions::tables::load_tss;
use x86_64::VirtAddr;
use x86_64::PrivilegeLevel;

/// Kernel stack size for Ring 3 → Ring 0 transitions (32KB)
const KERNEL_STACK_SIZE: usize = 32 * 1024;

/// Double fault stack size (IST1) (16KB)
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;

/// IST index for double fault handler (0-based, IST1 = index 0)
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Kernel stack for privilege transitions
#[repr(C, align(16))]
struct KernelStack([u8; KERNEL_STACK_SIZE]);

/// Double fault stack
#[repr(C, align(16))]
struct DoubleFaultStack([u8; DOUBLE_FAULT_STACK_SIZE]);

static mut KERNEL_STACK: KernelStack = KernelStack([0; KERNEL_STACK_SIZE]);
static mut DOUBLE_FAULT_STACK: DoubleFaultStack = DoubleFaultStack([0; DOUBLE_FAULT_STACK_SIZE]);

/// Per-CPU kernel stack pointer (stored so syscall entry can find it)
/// This is the RSP to load when entering Ring 0 from Ring 3.
pub static mut KERNEL_RSP: u64 = 0;

/// Task State Segment
static TSS: Lazy<TaskStateSegment> = Lazy::new(|| {
    let mut tss = TaskStateSegment::new();

    // RSP0: kernel stack used when transitioning from Ring 3 to Ring 0
    // (interrupts and exceptions that occur in Ring 3)
    tss.privilege_stack_table[0] = {
        let stack_start = unsafe {
            let stack = &raw const KERNEL_STACK;
            VirtAddr::new((*stack).0.as_ptr() as u64)
        };
        stack_start + KERNEL_STACK_SIZE as u64
    };

    // IST1: separate stack for double fault handler
    // (prevents triple fault if kernel stack overflows)
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
        let stack_start = unsafe {
            let stack = &raw const DOUBLE_FAULT_STACK;
            VirtAddr::new((*stack).0.as_ptr() as u64)
        };
        stack_start + DOUBLE_FAULT_STACK_SIZE as u64
    };

    tss
});

/// Selectors for each GDT entry
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_data: SegmentSelector,
    pub user_code: SegmentSelector,
    pub tss: SegmentSelector,
}

/// GDT + selectors
struct GdtWithSelectors {
    gdt: GlobalDescriptorTable,
    selectors: Selectors,
}

static GDT: Lazy<GdtWithSelectors> = Lazy::new(|| {
    let mut gdt = GlobalDescriptorTable::new();

    // Index 1 (0x08): Kernel Code Segment — 64-bit, DPL 0
    let kernel_code = gdt.append(Descriptor::kernel_code_segment());

    // Index 2 (0x10): Kernel Data Segment — DPL 0
    let kernel_data = gdt.append(Descriptor::kernel_data_segment());

    // Index 3 (0x18): User Data Segment — DPL 3
    // Must come before User Code for SYSRET convention
    let user_data = gdt.append(Descriptor::user_data_segment());

    // Index 4 (0x20): User Code Segment — 64-bit, DPL 3
    let user_code = gdt.append(Descriptor::user_code_segment());

    // Index 5-6 (0x28): TSS — 16-byte descriptor (occupies 2 GDT slots)
    let tss = gdt.append(Descriptor::tss_segment(&TSS));

    GdtWithSelectors {
        gdt,
        selectors: Selectors {
            kernel_code,
            kernel_data,
            user_data,
            user_code,
            tss,
        },
    }
});

/// Initialize the GDT and TSS.
///
/// Loads our GDT (replacing the bootloader's), reloads all segment
/// registers, and loads the TSS.
pub fn init() {
    // Load the GDT
    GDT.gdt.load();

    // Reload segment registers
    unsafe {
        // CS must be reloaded via a far return
        CS::set_reg(GDT.selectors.kernel_code);

        // Data segments
        DS::set_reg(GDT.selectors.kernel_data);
        SS::set_reg(GDT.selectors.kernel_data);

        // Load the TSS
        load_tss(GDT.selectors.tss);

        // Store the kernel RSP for SYSCALL handler
        KERNEL_RSP = TSS.privilege_stack_table[0].as_u64();
    }

    crate::println!("    GDT: {} entries loaded", 7); // null + 4 segments + TSS(2)
    crate::println!("    Kernel CS: 0x{:02X}  DS: 0x{:02X}",
        GDT.selectors.kernel_code.0, GDT.selectors.kernel_data.0);
    crate::println!("    User   CS: 0x{:02X}  DS: 0x{:02X}",
        GDT.selectors.user_code.0, GDT.selectors.user_data.0);
    crate::println!("    TSS: 0x{:02X}", GDT.selectors.tss.0);
}

/// Get the segment selectors
pub fn selectors() -> &'static Selectors {
    &GDT.selectors
}

/// Get the user code selector value (for STAR MSR)
pub fn user_code_selector() -> u16 {
    GDT.selectors.user_code.0
}

/// Get the user data selector value (for STAR MSR)
pub fn user_data_selector() -> u16 {
    GDT.selectors.user_data.0
}

/// Get the kernel code selector value (for STAR MSR)
pub fn kernel_code_selector() -> u16 {
    GDT.selectors.kernel_code.0
}

/// Read-only view of the current TSS RSP0. Used by the PF-handler
/// diagnostic when chasing the intermittent kernel-mode RIP=0 fault
/// (task #40) so we can see whether RSP0 has drifted from the
/// expected per-task kernel stack top.
pub fn tss_rsp0_value() -> u64 {
    unsafe { TSS.privilege_stack_table[0].as_u64() }
}

/// Update the TSS's RSP0 to a different kernel stack.
/// Called during context switch when each task might have its own kernel stack.
///
/// # Safety
/// The new RSP must point to a valid kernel stack.
pub unsafe fn set_kernel_stack(rsp: u64) {
    // We can't mutate the Lazy TSS directly, but we can write to the
    // privilege_stack_table via a raw pointer since it's in static memory.
    let tss_ptr = &*TSS as *const TaskStateSegment as *mut TaskStateSegment;
    (*tss_ptr).privilege_stack_table[0] = VirtAddr::new(rsp);
    KERNEL_RSP = rsp;
}
