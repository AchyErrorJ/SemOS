//! aarch64 per-task context + kernel-core scheduler glue.
//!
//! The platform maintains a saved stack pointer per scheduler slot. The actual
//! register frame is stored on the task's own stack in the 272-byte layout that
//! `irq_entry` in `main.rs` expects (x0-x30, ELR_EL1, SPSR_EL1). A context
//! switch is just a change of `SP_EL1` handed back to `irq_entry`.

use core::sync::atomic::Ordering;

use kernel_core::scheduler::MAX_TASKS;

/// Per-task saved state. The frame itself lives on the task stack; this struct
/// holds the SP to restore plus the address-space root for user tasks.
#[derive(Clone, Copy)]
pub struct TaskContext {
    pub sp: u64,
    pub ttbr0: u64,
}

impl TaskContext {
    pub const fn empty() -> Self {
        Self { sp: 0, ttbr0: 0 }
    }
}

/// Saved context pointer for every scheduler slot. Indexed by scheduler slot.
pub static mut CONTEXTS: [TaskContext; MAX_TASKS] =
    [TaskContext::empty(); MAX_TASKS];

/// Per-task kernel stacks.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Stack([u8; kernel_core::scheduler::TASK_STACK_SIZE]);

static mut TASK_STACKS: [Stack; MAX_TASKS] =
    [Stack([0; kernel_core::scheduler::TASK_STACK_SIZE]); MAX_TASKS];

/// Lay a fresh 272-byte trap frame at the top of a task stack so that the first
/// `eret` from `irq_entry` lands at `entry` in EL1h with IRQs enabled.
/// Returns the stack pointer at which the frame lives.
unsafe fn seed_task_stack(stack_top: u64, entry: u64) -> u64 {
    const FRAME_SIZE: usize = 34 * 8; // x0-x30 (31 regs) + ELR + SPSR + padding
    let sp = (stack_top - FRAME_SIZE as u64) & !0xF;
    let f = sp as *mut u64;
    for i in 0..34 {
        *f.add(i) = 0;
    }
    *f.add(30) = entry; // x30 / LR
    *f.add(31) = entry; // ELR_EL1
    *f.add(32) = 0x5;   // SPSR_EL1 = EL1h, IRQs enabled
    sp
}

/// Build the initial context for scheduler slot `slot` and store its frame SP.
unsafe fn create_context(slot: usize, entry: u64) {
    if slot >= MAX_TASKS {
        return;
    }
    let stack_addr = core::ptr::addr_of!(TASK_STACKS[slot]) as u64;
    let stack_top = stack_addr + kernel_core::scheduler::TASK_STACK_SIZE as u64;
    let sp = seed_task_stack(stack_top, entry);
    let contexts = &raw mut CONTEXTS;
    (*contexts)[slot].sp = sp;
    (*contexts)[slot].ttbr0 = crate::mmu::boot_ttbr0();
}

/// Spawn a kernel-mode task that starts at `entry`.
pub fn spawn_task(name: &'static str, entry: fn() -> !) {
    unsafe {
        let slot = kernel_core::scheduler::alloc_task_slot(name, 3, true)
            .expect("no free task slots");
        create_context(slot, entry as usize as u64);
        kernel_core::scheduler::mark_ready(slot);
    }
}

/// Timer-driven scheduler entry point. `cur_sp` is the interrupted task's
/// saved-frame SP (passed by `irq_entry`). Stores it, asks kernel-core who
/// runs next, switches TTBR0 if needed, and returns the next task's frame SP.
#[no_mangle]
pub extern "C" fn timer_schedule(cur_sp: u64) -> u64 {
    unsafe {
        let current = kernel_core::scheduler::CURRENT_TASK.load(Ordering::SeqCst);
        let contexts = &raw mut CONTEXTS;
        (*contexts)[current].sp = cur_sp;

        if let Some((_cur, next)) = kernel_core::scheduler::pick_next() {
            let next_ttbr0 = (*contexts)[next].ttbr0;
            let current_ttbr0 = crate::mmu::read_ttbr0();
            if next_ttbr0 != 0 && next_ttbr0 != current_ttbr0 {
                crate::mmu::write_ttbr0(next_ttbr0);
            }
            return (*contexts)[next].sp;
        }
        cur_sp
    }
}

/// Voluntary yield entry point. Timer preemption is sufficient for the first
/// integration phase, so this is a no-op. A real synchronous switch will be
/// added when user-mode syscalls need SYS_YIELD.
pub fn schedule() {
    // Deliberately empty: rely on timer preemption.
}
