//! x86_64 Context Switch
//!
//! Provides the TaskContext struct and naked assembly context switch routine.
//! This is the x86_64 equivalent of ARM64's callee-saved register save/restore.
//!
//! # x86_64 Callee-Saved Registers (System V ABI)
//!
//! | Register | Purpose            | Offset |
//! |----------|--------------------|--------|
//! | rbx      | Callee-saved       | 0      |
//! | rbp      | Frame pointer      | 8      |
//! | r12      | Callee-saved       | 16     |
//! | r13      | Callee-saved       | 24     |
//! | r14      | Callee-saved       | 32     |
//! | r15      | Callee-saved       | 40     |
//! | rsp      | Stack pointer      | 48     |
//! | rip      | Return address     | 56     |
//! | rflags   | Flags register     | 64     |
//!
//! Total: 72 bytes per context
//!
//! # Ring 3 Task Entry
//!
//! When a user-mode task is first scheduled, the context switch JMPs to
//! `ring3_entry_trampoline`, which finds an IRETQ frame pre-pushed onto
//! the kernel stack:
//!
//! ```text
//!   [rsp + 32]  SS        (User Data selector | RPL 3)
//!   [rsp + 24]  User RSP  (top of user stack)
//!   [rsp + 16]  RFLAGS    (0x202 = IF + reserved)
//!   [rsp +  8]  CS        (User Code selector | RPL 3)
//!   [rsp +  0]  RIP       (user entry point)
//! ```
//!
//! IRETQ pops all five fields and transitions to Ring 3.

use kernel_core::scheduler::{self, MAX_TASKS};

/// Saved CPU context for a task.
/// Contains all callee-saved registers needed to resume execution,
/// plus CR3 for per-process page table switching.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TaskContext {
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rsp: u64,
    pub rip: u64,
    pub rflags: u64,
    /// CR3 value (PML4 physical address) for this task's address space.
    /// 0 = use kernel page tables (shared address space).
    pub cr3: u64,
}

impl TaskContext {
    pub const fn empty() -> Self {
        Self {
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rsp: 0,
            rip: 0,
            rflags: 0x202, // IF=1 (interrupts enabled), reserved bit 1 always set
            cr3: 0,        // 0 = inherit kernel page tables
        }
    }
}

/// Per-task context storage (indexed by scheduler task slot)
pub static mut CONTEXTS: [TaskContext; MAX_TASKS] = [TaskContext::empty(); MAX_TASKS];

/// FXSAVE/FXRSTOR area — 512 bytes per task, 16-byte aligned.
/// Holds the FPU/MMX/SSE state for each task during context switch.
/// Stored in a parallel array (not in TaskContext) so the naked asm
/// register save/restore offsets remain stable.
/// `Copy` is purely so the static array can be initialised with `[X; N]`;
/// the kernel never moves or clones these by value at runtime — all
/// access is via &raw const/mut and addr arithmetic.
#[repr(C, align(16))]
#[derive(Copy, Clone)]
pub struct FxSaveArea([u8; 512]);

pub static mut FXSAVE_AREAS: [FxSaveArea; MAX_TASKS] = [FxSaveArea([0; 512]); MAX_TASKS];

/// Initialize a task slot's FXSAVE area with valid default state.
/// Must be called for each newly-spawned task before its first run, otherwise
/// fxrstor would load garbage (in particular, MXCSR=0 raises #GP on some CPUs).
pub fn init_fxsave_for(slot: usize) {
    if slot >= MAX_TASKS { return; }
    unsafe {
        let areas = &raw mut FXSAVE_AREAS;
        // Zero the area, then set MXCSR (offset 24, 4 bytes) to 0x1F80 — the
        // architectural reset value (all SSE exceptions masked, round-to-nearest).
        let buf = &mut (*areas)[slot].0;
        for b in buf.iter_mut() { *b = 0; }
        let mxcsr: u32 = 0x1F80;
        let bytes = mxcsr.to_le_bytes();
        buf[24..28].copy_from_slice(&bytes);
        // Also set FCW (offset 0, 2 bytes) to 0x037F — the x87 default.
        let fcw: u16 = 0x037F;
        let fcw_bytes = fcw.to_le_bytes();
        buf[0..2].copy_from_slice(&fcw_bytes);
    }
}

/// Task stacks (16KB each, 16-byte aligned) — used as the primary stack
/// for kernel-mode tasks, and as the user-mode stack for Ring 3 tasks.
#[repr(C, align(16))]
#[derive(Copy, Clone)]
struct TaskStack([u8; scheduler::TASK_STACK_SIZE]);

static mut TASK_STACKS: [TaskStack; MAX_TASKS] =
    [TaskStack([0; scheduler::TASK_STACK_SIZE]); MAX_TASKS];

/// Stack-overflow canary written at the LOWEST address of each TASK_STACK
/// at boot. If a task overflows its stack downward, it'll smash this value.
/// `check_stack_canaries()` (called from PF handler) detects this and
/// reports loudly. Real unmapped guard pages are a follow-up — this is the
/// cheap detection variant.
pub const STACK_CANARY: u64 = 0xDEAD_BEEF_CAFE_BABE;

/// Sentinel at [TASK_STACK[N].top - 56] = the would-be timer-iret-RIP slot.
/// PRE-RESUME check reads this. If it's still the sentinel, the slot was
/// scheduled but never timer-preempted (no iret push ever happened). If 0,
/// something zeroed it. If a kernel code addr, the timer pushed a valid RIP.
pub const IRET_RIP_SENTINEL: u64 = 0xCAFE_BABE_F00D_BEEF;

/// Initialize stack canaries:
///   - bottom of every TASK_STACK gets STACK_CANARY (overflow detection)
///   - [top - 56] of slots 1..=3 gets IRET_RIP_SENTINEL (task #40 hunt)
/// Call once after init, before any task is scheduled.
pub fn init_stack_canaries() {
    unsafe {
        let stacks = &raw mut TASK_STACKS;
        for slot in 0..MAX_TASKS {
            let bottom = (*stacks)[slot].0.as_mut_ptr() as *mut u64;
            *bottom = STACK_CANARY;
            if slot >= 1 && slot <= 3 {
                let top = (*stacks)[slot].0.as_mut_ptr() as u64
                    + scheduler::TASK_STACK_SIZE as u64;
                let iret_rip_slot = (top - 56) as *mut u64;
                *iret_rip_slot = IRET_RIP_SENTINEL;
            }
        }
    }
}

/// Check every TASK_STACK's bottom canary. Returns the first slot whose
/// canary has been smashed, or None if all are intact.
pub fn check_stack_canaries() -> Option<usize> {
    unsafe {
        let stacks = &raw const TASK_STACKS;
        for slot in 0..MAX_TASKS {
            let bottom = (*stacks)[slot].0.as_ptr() as *const u64;
            if core::ptr::read_volatile(bottom) != STACK_CANARY {
                return Some(slot);
            }
        }
    }
    None
}

/// Address of the stack-bottom canary for a given slot. Used by PF handler
/// to print what the canary was clobbered TO.
pub fn stack_bottom_addr(slot: usize) -> u64 {
    unsafe {
        let stacks = &raw const TASK_STACKS;
        (*stacks)[slot].0.as_ptr() as u64
    }
}


/// Per-task kernel stacks (8KB each) — used for Ring 3 → Ring 0 transitions.
/// When an interrupt or SYSCALL fires while a Ring 3 task is running,
/// the CPU loads RSP from TSS.RSP0, which points to this task's kernel stack.
const KERNEL_STACK_PER_TASK: usize = 8 * 1024;

#[repr(C, align(16))]
#[derive(Copy, Clone)]
struct PerTaskKernelStack([u8; KERNEL_STACK_PER_TASK]);

static mut PER_TASK_KERNEL_STACKS: [PerTaskKernelStack; MAX_TASKS] =
    [PerTaskKernelStack([0; KERNEL_STACK_PER_TASK]); MAX_TASKS];

/// Public debug accessor — same as kernel_stack_top.
pub fn debug_kstack_top(slot: usize) -> u64 { kernel_stack_top(slot) }

// ============================================================================
// Task #40 diagnostic: context-switch ring buffer
// ============================================================================
//
// A circular log of the most recent `schedule -> context_switch` calls. Each
// entry captures `(current, next, CONTEXTS[next].rip)` immediately before the
// jmp into `context_switch`. The PF handler dumps this on a kernel-mode RIP=0
// fault, so we can see whether the next-task's saved rip was 0 at switch-in
// time — which would prove the "context_switch jumps to 0, then a pending
// timer pushes an iret frame with RIP=0" hypothesis.
//
// Designed to be as low-overhead as possible: no conditional branches in the
// hot path, no Rust function calls, just three writes and one wrap-around mask.

#[derive(Copy, Clone)]
#[repr(C)]
pub struct CtxLogEntry {
    pub cur: u32,
    pub next: u32,
    pub next_rip: u64,
    pub next_rsp: u64,
}

pub const CTX_LOG_LEN: usize = 64;

pub static mut CTX_LOG: [CtxLogEntry; CTX_LOG_LEN] = [CtxLogEntry {
    cur: 0,
    next: 0,
    next_rip: 0,
    next_rsp: 0,
}; CTX_LOG_LEN];

/// Monotonic write index. We only mod CTX_LOG_LEN at access time so that the
/// PF handler can also tell *how many* switches have happened since boot.
pub static mut CTX_LOG_IDX: u64 = 0;

/// Get the top (highest address) of a task's kernel stack
fn kernel_stack_top(slot: usize) -> u64 {
    unsafe {
        let stacks = &raw const PER_TASK_KERNEL_STACKS;
        (*stacks)[slot].0.as_ptr().add(KERNEL_STACK_PER_TASK) as u64
    }
}

/// Create a new task context that will begin execution at `entry`.
/// Returns the initialized context with a fresh stack.
pub fn create_context(slot: usize, entry: fn()) -> TaskContext {
    // Stack grows downward on x86_64; start at the top, 16-byte aligned
    let stack_top = unsafe {
        let stacks = &raw mut TASK_STACKS;
        (*stacks)[slot].0.as_ptr().add(scheduler::TASK_STACK_SIZE) as u64
    };

    // The fix to context_switch (pop return address before save, jmp on
    // restore) makes the resume path equivalent to `ret`. So a fresh task
    // looks like one that just consumed its return address — RSP must be
    // 16-aligned and point above a "fake" return slot.
    //
    // We push a sentinel return address (`task_exit_stub`) on the new
    // task's stack at stack_top - 8. If the entry function ever returns,
    // it will pop that sentinel and execute it, halting the task instead
    // of triple-faulting on garbage.
    let stack_aligned_top = stack_top & !0xF;
    let fake_ret_slot = stack_aligned_top - 8;
    unsafe {
        *(fake_ret_slot as *mut u64) = task_exit_stub as u64;
    }
    // Saved RSP points at the fake return slot — exactly where `ret` would
    // jump from. Combined with `jmp [rsi+56]` to entry, the entry function
    // sees RSP % 16 == 8, the standard post-`call` state.
    let rsp = fake_ret_slot;

    TaskContext {
        rbx: 0,
        rbp: 0,
        r12: 0,
        r13: 0,
        r14: 0,
        r15: 0,
        rsp,
        rip: entry as u64,
        rflags: 0x202,
        cr3: 0, // Kernel task — use boot page tables
    }
}

/// Sentinel that runs if a kernel task's entry function ever returns.
/// Rather than triple-faulting on garbage popped from a corrupted stack,
/// halt this CPU and wait for the next interrupt to schedule something else.
extern "C" fn task_exit_stub() -> ! {
    // TODO: mark current task as Exited so the scheduler stops choosing it.
    // For now just block and wait — a timer interrupt will reschedule.
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}

/// Create a context for a Ring 3 user-mode task.
///
/// The task's context is set up so that when context_switch JMPs to its
/// `rip`, it lands on `ring3_entry_trampoline`. That trampoline executes
/// IRETQ to drop to Ring 3 using a pre-built interrupt return frame on
/// the per-task kernel stack.
///
/// # Arguments
/// * `slot` - Scheduler slot index
/// * `user_rip` - The entry point in user space
/// * `user_rsp` - The user-mode stack pointer (top of user stack)
fn create_ring3_context(slot: usize, user_rip: u64, user_rsp: u64) -> TaskContext {
    let selectors = crate::gdt::selectors();
    let user_cs = selectors.user_code.0 as u64;
    let user_ss = selectors.user_data.0 as u64;

    // Build the IRETQ frame on the per-task kernel stack.
    // The trampoline will execute IRETQ which pops:
    //   RIP, CS, RFLAGS, RSP, SS  (5 * 8 = 40 bytes)
    let kstack_top = kernel_stack_top(slot);
    // Align to 16 bytes
    let kstack_aligned = kstack_top & !0xF;
    // IRETQ frame (grows downward):
    //   kstack - 8:  SS
    //   kstack - 16: User RSP
    //   kstack - 24: RFLAGS
    //   kstack - 32: CS
    //   kstack - 40: RIP
    let frame_base = kstack_aligned - 40;
    unsafe {
        let p = frame_base as *mut u64;
        *p.add(0) = user_rip;           // RIP — user entry point
        *p.add(1) = user_cs;            // CS  — User Code (DPL 3)
        *p.add(2) = 0x202;              // RFLAGS — IF=1, reserved bit 1
        *p.add(3) = user_rsp;           // RSP — user stack
        *p.add(4) = user_ss;            // SS  — User Data (DPL 3)
    }

    // The context_switch will restore RSP = frame_base and JMP to the trampoline.
    // The trampoline does IRETQ, popping the 5 values above → Ring 3.
    TaskContext {
        rbx: 0,
        rbp: 0,
        r12: 0,
        r13: 0,
        r14: 0,
        r15: 0,
        rsp: frame_base,
        rip: ring3_entry_trampoline as *const () as u64,
        rflags: 0x202,
        cr3: 0, // Will be set by caller
    }
}

/// Trampoline that executes IRETQ to enter Ring 3.
///
/// When this function is reached via context_switch's `jmp [rsi + 56]`,
/// RSP already points to a pre-built IRETQ frame:
///   [rsp+0]  RIP, [rsp+8] CS, [rsp+16] RFLAGS, [rsp+24] RSP, [rsp+32] SS
///
/// IRETQ pops all five and transitions to user mode.
#[unsafe(naked)]
extern "C" fn ring3_entry_trampoline() {
    core::arch::naked_asm!(
        // Zero out general-purpose registers to prevent kernel data leaks
        "xor rax, rax",
        "xor rbx, rbx",
        "xor rcx, rcx",
        "xor rdx, rdx",
        "xor rsi, rsi",
        "xor rdi, rdi",
        "xor rbp, rbp",
        "xor r8, r8",
        "xor r9, r9",
        "xor r10, r10",
        // r11-r15 were already zeroed by context_switch's register restore
        // (the TaskContext was initialized with all zeros for those fields)
        "iretq",
    );
}

/// Same as [`ring3_entry_trampoline`] but passes a single argument to
/// the new thread via rdi (SysV AMD64 first-arg register). The caller
/// stuffs the arg into `TaskContext.rbx` — context_switch restores
/// rbx, this trampoline moves it into rdi, then zeros rbx itself
/// before iretq so no kernel data lingers.
///
/// Used by Ring 3 `SYS_THREAD_SPAWN` (task #45) — the std-shim's
/// `thread::spawn` lowers to "call entry(arg)" inside the new
/// thread, which means the new thread must start with `arg` in rdi.
#[unsafe(naked)]
extern "C" fn ring3_thread_trampoline() {
    core::arch::naked_asm!(
        // Move the arg (placed in rbx by spawn) into rdi.
        "mov rdi, rbx",
        // Now zero everything else (including rbx, post-move).
        "xor rax, rax",
        "xor rbx, rbx",
        "xor rcx, rcx",
        "xor rdx, rdx",
        "xor rsi, rsi",
        "xor rbp, rbp",
        "xor r8, r8",
        "xor r9, r9",
        "xor r10, r10",
        // r11-r15 already zero from TaskContext init.
        "iretq",
    );
}

/// Build a Ring 3 context for a *thread* (same address space as the
/// parent), entering at `user_rip` with `arg` in rdi.
///
/// Like [`create_ring3_context`] but stashes `arg` in rbx so
/// [`ring3_thread_trampoline`] can hand it off via SysV ABI.
fn create_ring3_thread_context(slot: usize, user_rip: u64, user_rsp: u64, arg: u64) -> TaskContext {
    let selectors = crate::gdt::selectors();
    let user_cs = selectors.user_code.0 as u64;
    let user_ss = selectors.user_data.0 as u64;

    let kstack_top = kernel_stack_top(slot);
    let kstack_aligned = kstack_top & !0xF;
    let frame_base = kstack_aligned - 40;
    unsafe {
        let p = frame_base as *mut u64;
        *p.add(0) = user_rip;
        *p.add(1) = user_cs;
        *p.add(2) = 0x202;
        *p.add(3) = user_rsp;
        *p.add(4) = user_ss;
    }

    TaskContext {
        rbx: arg, // ← will land in rdi via ring3_thread_trampoline
        rbp: 0,
        r12: 0,
        r13: 0,
        r14: 0,
        r15: 0,
        rsp: frame_base,
        rip: ring3_thread_trampoline as *const () as u64,
        rflags: 0x202,
        cr3: 0, // Caller sets to parent's CR3.
    }
}

/// Spawn a Ring 3 thread that shares its parent's address space.
///
/// Maps a fresh user stack inside the parent's CR3 (one virtual
/// stack-region per scheduler slot, spaced `THREAD_STACK_VSTRIDE`
/// apart so each thread's stack is unique without overlapping with
/// the parent's `USER_STACK_TOP`), then constructs a Ring 3 context
/// that enters `entry_va` with `arg` in rdi.
///
/// Phase 14 Tier 3 (#45) Ring 3 same-AS branch — the kernel-mode
/// branch lives in `context::spawn_task`; the Platform `spawn_thread`
/// trait method dispatches based on cr3.
pub fn spawn_ring3_thread_in_cr3(
    name: &'static str,
    parent_cr3: u64,
    entry_va: u64,
    arg: u64,
    max_tier: u8,
) -> Option<usize> {
    let slot = scheduler::alloc_task_slot(name, max_tier, false)?;

    // Pick the new thread's user-stack virtual region: USER_STACK_TOP
    // shifted down by slot * THREAD_STACK_VSTRIDE (1 MiB stride).
    // With MAX_TASKS=16 slots, the highest-slot thread ends up at
    // USER_STACK_TOP - ~16 MiB which is still well inside the lower
    // half. The stride is far larger than USER_STACK_SIZE so threads
    // can't accidentally walk into each other's stack.
    const THREAD_STACK_VSTRIDE: u64 = 0x10_0000; // 1 MiB virtual spacing
    let user_stack_size = crate::paging::user_layout::USER_STACK_SIZE;
    let stack_pages = (user_stack_size / crate::paging::PAGE_SIZE_4K) as usize;
    let user_stack_top = crate::paging::user_layout::USER_STACK_TOP
        .saturating_sub((slot as u64) * THREAD_STACK_VSTRIDE);

    // Physical backing comes from this slot's TASK_STACKS. Same
    // strategy as spawn_user_task; safe because each slot gets its
    // own TASK_STACKS region.
    let stack_virt_base = unsafe {
        let stacks = &raw const TASK_STACKS;
        (*stacks)[slot].0.as_ptr() as u64
    };
    for i in 0..stack_pages {
        let page_kvirt = stack_virt_base + (i as u64 * crate::paging::PAGE_SIZE_4K);
        let page_phys = match crate::paging::walk_active_pml4(page_kvirt) {
            Some(p) => p,
            None => return None,
        };
        let page_virt = (user_stack_top - user_stack_size)
            + (i as u64 * crate::paging::PAGE_SIZE_4K);
        if !map_page_in_space(
            parent_cr3,
            page_virt,
            page_phys,
            crate::paging::PagePermission::ReadWrite,
        ) {
            return None;
        }
    }

    // 16-byte align the user RSP per SysV ABI.
    let user_rsp = user_stack_top & !0xF;

    let mut ctx = create_ring3_thread_context(slot, entry_va, user_rsp, arg);
    ctx.cr3 = parent_cr3;

    unsafe {
        let contexts = &raw mut CONTEXTS;
        (*contexts)[slot] = ctx;
    }
    init_fxsave_for(slot);
    scheduler::mark_ready(slot);
    Some(slot)
}

/// Perform context switch between two tasks.
///
/// Saves current registers into `old`, restores from `new`, and jumps
/// to the new task's saved RIP.
///
/// # Safety
/// Both pointers must be valid TaskContext pointers.
#[unsafe(naked)]
pub unsafe extern "C" fn context_switch(old: *mut TaskContext, new: *const TaskContext) {
    // old = rdi, new = rsi (System V calling convention).
    //
    // The save/restore must be symmetric with how a normal `call`/`ret` pair
    // would leave the stack. A `call` pushed a return address; we POP it
    // (saving it as the task's RIP) so the saved RSP matches the post-`ret`
    // position. On restore we load RSP and JMP to the saved RIP — which is
    // exactly what `ret` would have produced. Without the pop, the saved RSP
    // would still contain the stale return address, and every subsequent
    // stack-relative operation in the original caller would be off by 8
    // bytes — eventually corrupting the iretq frame at handler exit.
    core::arch::naked_asm!(
        // Save current callee-saved registers into old (rdi)
        "mov [rdi + 0],  rbx",
        "mov [rdi + 8],  rbp",
        "mov [rdi + 16], r12",
        "mov [rdi + 24], r13",
        "mov [rdi + 32], r14",
        "mov [rdi + 40], r15",
        // Pop the return address so the saved RSP matches a post-`ret` state.
        "pop rax",
        "mov [rdi + 56], rax", // save return address as RIP
        "mov [rdi + 48], rsp", // save RSP (without return address on top)
        // Save rflags
        "pushfq",
        "pop rax",
        "mov [rdi + 64], rax",

        // Restore new context from new (rsi)
        "mov rbx, [rsi + 0]",
        "mov rbp, [rsi + 8]",
        "mov r12, [rsi + 16]",
        "mov r13, [rsi + 24]",
        "mov r14, [rsi + 32]",
        "mov r15, [rsi + 40]",
        "mov rsp, [rsi + 48]",
        // Restore rflags
        "mov rax, [rsi + 64]",
        "push rax",
        "popfq",
        // Jump to the new task's saved RIP — equivalent to `ret` minus the pop.
        "jmp [rsi + 56]",
    );
}

/// Called from the timer interrupt to perform a schedule + context switch.
///
/// This is the glue between kernel-core's `pick_next()` and the
/// platform-specific context switch assembly.
/// Switches CR3 if the next task has its own address space, and updates
/// the TSS RSP0 to the next task's per-task kernel stack.
pub fn schedule() {
    // Disable interrupts for the entire switch. Required because schedule
    // is now reachable from syscall handlers (SYS_YIELD) where IF=1; an
    // intervening timer would re-enter schedule and corrupt FPU/CR3/TSS
    // state mid-switch. The outgoing task's saved RFLAGS captures IF=0
    // here, but `context_switch`'s `popfq` restores the *new* task's IF
    // from its own saved RFLAGS, so kernel tasks resumed normally
    // (saved-IF=1) get interrupts back. Tasks that called schedule
    // voluntarily resume with IF=0 until SYSRET/IRETQ restores user IF.
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    if let Some((current, next)) = scheduler::pick_next() {
        unsafe {
            let contexts = &raw mut CONTEXTS;
            let fxsave_areas = &raw mut FXSAVE_AREAS;

            // Save FPU/SSE state of the outgoing task.
            let cur_fx = (*fxsave_areas)[current].0.as_mut_ptr();
            core::arch::asm!("fxsave [{}]", in(reg) cur_fx, options(nostack, preserves_flags));

            // Switch page tables. Kernel tasks (cr3 = 0) get the bootloader's
            // PML4; isolated/Ring 3 tasks get their own. Critically, kernel
            // tasks must explicitly switch BACK to BOOT_CR3 — otherwise they
            // inherit whatever isolated address space the previous task was
            // using, and the kernel's own data structures may be inaccessible.
            let saved_cr3 = (*contexts)[next].cr3;
            let target_cr3 = if saved_cr3 == 0 {
                crate::paging::boot_cr3()
            } else {
                saved_cr3
            };
            let current_cr3 = crate::paging::read_cr3();
            if target_cr3 != current_cr3 {
                crate::paging::write_cr3(target_cr3);
            }

            // Update TSS RSP0 to the next task's kernel stack.
            let next_kstack_top = kernel_stack_top(next);
            crate::gdt::set_kernel_stack(next_kstack_top);

            // Restore FPU/SSE state of the incoming task.
            let next_fx = (*fxsave_areas)[next].0.as_ptr();
            core::arch::asm!("fxrstor [{}]", in(reg) next_fx, options(nostack, preserves_flags));

            let old_ctx = &mut (*contexts)[current] as *mut TaskContext;
            let new_ctx = &(*contexts)[next] as *const TaskContext;
            // Task #40 diagnostic — record this switch in the ring buffer
            // before jumping into context_switch. Three writes, no branches.
            let log_ptr = &raw mut CTX_LOG;
            let idx_ptr = &raw mut CTX_LOG_IDX;
            let i = (*idx_ptr) as usize & (CTX_LOG_LEN - 1);
            (*log_ptr)[i] = CtxLogEntry {
                cur: current as u32,
                next: next as u32,
                next_rip: (*contexts)[next].rip,
                next_rsp: (*contexts)[next].rsp,
            };
            *idx_ptr = (*idx_ptr).wrapping_add(1);

            // Task #40 diagnostic: re-read CONTEXTS[next].rip *immediately*
            // before context_switch's `jmp [rsi+0x38]`. If this is 0 here but
            // CTX_LOG (8 instructions earlier) saw non-zero, something wrote 0
            // between the two reads. If non-zero here AND context_switch still
            // jumps to 0, it's the CPU/compiler reordering the load past the
            // function call. The volatile_read + compiler_fence rules out the
            // latter; the print rules out the former.
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
            let ctx_ptr = &(*contexts)[next] as *const TaskContext;
            let rip_addr = ctx_ptr as u64 + 0x38; // .rip offset
            let rsp_addr = ctx_ptr as u64 + 0x30; // .rsp offset
            let rip_check = core::ptr::read_volatile(rip_addr as *const u64);
            let rsp_check = core::ptr::read_volatile(rsp_addr as *const u64);
            if next >= 1 && next <= 3 {
                crate::println!(
                    "[task#40] SW cur={} next={} ctx=0x{:x} rip_at_0x{:x}=0x{:x} rsp=0x{:x}",
                    current, next, ctx_ptr as u64, rip_addr, rip_check, rsp_check,
                );
            }
            if rip_check == 0 {
                crate::println!(
                    "[task#40] PRE-SWITCH rip=0! cur={} next={} ctx_addr=0x{:x}",
                    current, next,
                    &(*contexts)[next] as *const TaskContext as u64,
                );
            }

            // Task #40 pre-resume sentinel probe (2026-05-13):
            // Read [TASK_STACK[N].top - 56] (the timer-iret-RIP slot) and
            // classify each resume of slots 1/2/3. The sentinel was seeded at
            // boot by init_stack_canaries(); the value at PRE-RESUME tells us:
            //   SENTINEL → slot has never been timer-preempted (no iret push)
            //   0        → something zeroed the slot AFTER the push
            //   kernel   → normal: timer pushed a valid RIP, iretq will work
            if next >= 1 && next <= 3 {
                let stacks_ptr = &raw const TASK_STACKS;
                let stack_top = (*stacks_ptr)[next].0.as_ptr() as u64
                    + scheduler::TASK_STACK_SIZE as u64;
                let iret_rip_pos = stack_top - 56;
                let iret_rip_val = core::ptr::read_volatile(iret_rip_pos as *const u64);
                let class = if iret_rip_val == IRET_RIP_SENTINEL {
                    "SENTINEL"
                } else if iret_rip_val == 0 {
                    "ZERO"
                } else if iret_rip_val >= 0x1000_0000_0000 && iret_rip_val < 0x1100_0000_0000 {
                    "KERNEL"
                } else {
                    "OTHER"
                };
                crate::println!(
                    "[task#40] PRE-RESUME slot {} cur={} iret_rip[0x{:x}]=0x{:x} [{}]",
                    next, current, iret_rip_pos, iret_rip_val, class,
                );
            }

            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
            context_switch(old_ctx, new_ctx);
        }
    }
}

/// Spawn a kernel task. Sets up a context with a fresh stack and
/// registers it with the kernel-core scheduler.
/// Kernel tasks share the boot page tables (cr3 = 0).
pub fn spawn_task(name: &'static str, entry: fn()) -> Option<usize> {
    let slot = scheduler::alloc_task_slot(name, 3, true)?;
    let ctx = create_context(slot, entry);
    unsafe {
        let contexts = &raw mut CONTEXTS;
        (*contexts)[slot] = ctx;
    }
    init_fxsave_for(slot);
    // Now that the context, stack, and FX-save area are all initialized,
    // promote the slot to Ready so the next timer tick can schedule it.
    scheduler::mark_ready(slot);
    Some(slot)
}

// ============================================================================
// Address Space Tracking
// ============================================================================

/// Tracked address spaces for proper cleanup on task exit.
/// Indexed by CR3 value — we store at most MAX_TASKS address spaces.
// AddressSpace isn't Copy (it owns frame allocations cleaned up on drop),
// so `[None; N]` is rejected. Longhand it is — bumped to match MAX_TASKS=16.
static mut ADDRESS_SPACES: [Option<crate::paging::AddressSpace>; MAX_TASKS] = [
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
];

/// Store an AddressSpace so it can be cleaned up later.
/// Called by platform_impl when creating address spaces for ELF processes.
pub fn store_address_space(space: crate::paging::AddressSpace) {
    unsafe {
        let spaces = &raw mut ADDRESS_SPACES;
        for slot in (*spaces).iter_mut() {
            if slot.is_none() {
                *slot = Some(space);
                return;
            }
        }
        // No slot available — leak it (better than crashing)
        core::mem::forget(space);
    }
}

/// Map a page in the address space identified by CR3.
pub fn map_page_in_space(
    cr3: u64,
    virt: u64,
    phys: u64,
    perm: crate::paging::PagePermission,
) -> bool {
    unsafe {
        let spaces = &raw mut ADDRESS_SPACES;
        for slot in (*spaces).iter_mut() {
            if let Some(ref mut space) = slot {
                if space.cr3 == cr3 {
                    return space.map_4k(virt, phys, perm);
                }
            }
        }
    }
    false
}

/// Destroy the address space identified by CR3, freeing all page table frames.
pub fn destroy_address_space(cr3: u64) {
    unsafe {
        let spaces = &raw mut ADDRESS_SPACES;
        for slot in (*spaces).iter_mut() {
            if let Some(ref mut space) = slot {
                if space.cr3 == cr3 {
                    space.destroy();
                    *slot = None;
                    return;
                }
            }
        }
    }
}

/// Spawn a Ring 3 user-mode task with a pre-existing address space (CR3).
///
/// Used by the Platform trait's `spawn_user_task` — the address space and
/// all mappings (code, stack, data) are already set up by the caller.
pub fn spawn_user_task_with_cr3(
    name: &'static str,
    user_rip: u64,
    user_rsp: u64,
    cr3: u64,
    max_tier: u8,
) -> Option<usize> {
    let slot = scheduler::alloc_task_slot(name, max_tier, false)?;

    // Create Ring 3 context with IRETQ trampoline
    let mut ctx = create_ring3_context(slot, user_rip, user_rsp);
    ctx.cr3 = cr3;

    unsafe {
        let contexts = &raw mut CONTEXTS;
        (*contexts)[slot] = ctx;
    }
    init_fxsave_for(slot);
    scheduler::mark_ready(slot);

    // (silenced) "Ring 3 ELF task '{}': RIP=... RSP=... CR3=..." debug line
    let _ = (name, user_rip, user_rsp, cr3);

    Some(slot)
}

/// Spawn a task with its own address space, restricted to a security tier.
///
/// The task gets isolated page tables that only map memory pools
/// at or below `max_tier`. This is the hardware enforcement of the
/// 4-tier security model.
/// The task still runs in Ring 0 (kernel code segment).
pub fn spawn_isolated_task(
    name: &'static str,
    entry: fn(),
    max_tier: u8,
) -> Option<usize> {
    let slot = scheduler::alloc_task_slot(name, max_tier, false)?;
    let mut ctx = create_context(slot, entry);

    // Create a restricted address space with its own PML4 (currently a copy
    // of the boot PML4 — full per-tier filtering is a separate concern).
    if let Some(space) = crate::paging::create_process_address_space(max_tier) {
        ctx.cr3 = space.cr3;
        // Note: AddressSpace is consumed here — cleanup happens when the task exits.
        // We store the cr3 in the context; the subtable tracking is lost.
        // TODO: store AddressSpace in a per-task table for proper cleanup.
        core::mem::forget(space);
    }

    unsafe {
        let contexts = &raw mut CONTEXTS;
        (*contexts)[slot] = ctx;
    }
    init_fxsave_for(slot);
    scheduler::mark_ready(slot);
    Some(slot)
}

/// Spawn a Ring 3 user-mode task with its own address space.
///
/// This is the full isolation story:
/// - Own page tables (CR3) restricted to `max_tier`
/// - Runs in Ring 3 (unprivileged) — can only interact with the kernel
///   through SYSCALL
/// - Gets a user-mode stack mapped in its address space
/// - First scheduling drops to Ring 3 via IRETQ trampoline
///
/// The `entry` function pointer must be mapped in the process's address
/// space at the same virtual address. For kernel-resident code that's
/// identity-mapped in user space, this works directly. For true user
/// binaries, you'd load them at a user-space address.
pub fn spawn_user_task(
    name: &'static str,
    entry: fn(),
    max_tier: u8,
) -> Option<usize> {
    let slot = scheduler::alloc_task_slot(name, max_tier, false)?;

    // Create a restricted address space
    let mut space = crate::paging::create_process_address_space(max_tier)?;

    // Map the user stack in the process's address space
    // We use the TASK_STACKS slot as the physical backing for the user stack.
    // This works because the bootloader identity-maps (or offset-maps) all
    // physical memory, and we map the stack's physical address into the
    // process's lower-half virtual space.
    let user_stack_virt = crate::paging::user_layout::USER_STACK_TOP;
    let user_stack_size = crate::paging::user_layout::USER_STACK_SIZE;

    // Map user stack pages (4KB each).
    // Same caveat as the entry mapping below: TASK_STACKS lives in the
    // kernel image .bss region, which is NOT in the physical-memory map.
    // We must walk the page tables per-page to get each stack page's real
    // physical backing. (If TASK_STACKS happens to span a page boundary
    // mid-allocation that's fine — page-by-page translation handles it.)
    let stack_pages = (user_stack_size / crate::paging::PAGE_SIZE_4K) as usize;
    let stack_virt_base = unsafe {
        let stacks = &raw const TASK_STACKS;
        (*stacks)[slot].0.as_ptr() as u64
    };

    for i in 0..stack_pages {
        let page_kvirt = stack_virt_base + (i as u64 * crate::paging::PAGE_SIZE_4K);
        let page_phys = match crate::paging::walk_active_pml4(page_kvirt) {
            Some(p) => p,
            None => {
                crate::println!("    [spawn_user_task] failed to translate stack page 0x{:X}", page_kvirt);
                return None;
            }
        };
        let page_virt = (user_stack_virt - user_stack_size)
            + (i as u64 * crate::paging::PAGE_SIZE_4K);
        space.map_4k(page_virt, page_phys, crate::paging::PagePermission::ReadWrite);
    }

    // Map the kernel text region into user space as read+execute.
    // For now, the entry function lives in kernel text which is in the
    // higher half. We need to make it accessible from Ring 3.
    // The simplest approach: map a single 4KB page containing the entry
    // point into the user's lower-half space at USER_CODE_BASE.
    let entry_addr = entry as u64;
    let entry_page = entry_addr & !0xFFF; // 4KB aligned
    let entry_offset = entry_addr & 0xFFF;

    // Map the entry page at USER_CODE_BASE.
    // The entry function lives in the kernel image at virtual_address_offset
    // (e.g. 0x10000000000), which is NOT the bootloader's physical-memory
    // map region. virt_to_phys would underflow into a bogus high address;
    // walk_active_pml4 does the real page-table translation.
    let user_code_virt = crate::paging::user_layout::USER_CODE_BASE;
    let entry_phys = match crate::paging::walk_active_pml4(entry_page) {
        Some(p) => p,
        None => {
            crate::println!("    [spawn_user_task] failed to translate entry virt 0x{:X}", entry_page);
            return None;
        }
    };
    space.map_4k(
        user_code_virt,
        entry_phys,
        crate::paging::PagePermission::ReadExecute,
    );

    // The user RIP is at USER_CODE_BASE + offset within the page
    let user_rip = user_code_virt + entry_offset;
    // User RSP starts at the top of the user stack, 16-byte aligned
    let user_rsp = user_stack_virt & !0xF;

    // Create the Ring 3 context (IRETQ trampoline + pre-built frame)
    let mut ctx = create_ring3_context(slot, user_rip, user_rsp);
    ctx.cr3 = space.cr3;

    // Consume the AddressSpace (we keep cr3 in the context)
    core::mem::forget(space);

    unsafe {
        let contexts = &raw mut CONTEXTS;
        (*contexts)[slot] = ctx;
    }
    init_fxsave_for(slot);
    scheduler::mark_ready(slot);

    // (silenced) Ring 3 task spawn debug line
    let _ = (name, user_rip, user_rsp, ctx.cr3);

    Some(slot)
}
