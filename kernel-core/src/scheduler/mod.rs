//! Platform-Independent Scheduler Logic
//!
//! Contains task state management and round-robin scheduling algorithm.
//! Platform crates provide the actual context switch implementation
//! (ARM64: register save/restore + ERET, x86_64: register save/restore + IRETQ).
//!
//! # Platform Integration
//!
//! Platform crates must:
//! 1. Allocate task stacks
//! 2. Implement `ContextSwitchOps` for arch-specific register save/restore
//! 3. Call `schedule()` from their timer interrupt handler
//! 4. Call `init()` at boot

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Maximum number of tasks
pub const MAX_TASKS: usize = 16;

/// Scheduler tick rate (Hz). The platform's timer interrupt fires at
/// this rate and calls `platform::schedule()` on each tick. The std
/// shim divides `Duration` by this to get the integer ticks for
/// `SYS_SLEEP` — Phase 14 Tier 3 #48, the `std::thread::sleep` lowering
/// target.
///
/// Value is a best-effort estimate: on QEMU's LAPIC the bus clock is
/// 1 GHz, divided by 16, with an init count of 1_000_000 → ~62.5 Hz.
/// On real hardware the LAPIC bus clock varies and the actual rate
/// may drift. For std::thread::sleep this is fine — Rust's contract
/// is "at least the requested duration", not "exactly". Code that
/// needs precise timing should use `platform::wall_clock()`.
///
/// Source of truth for the rate is `kernel-x86_64/src/apic.rs::init`
/// (where the LAPIC timer is programmed). If that changes, this const
/// must change too — they're not enforced linked because kernel-core
/// has no platform-side counterpart to compute against.
pub const SCHEDULER_TICK_HZ: u64 = 62;

/// Task stack size (16KB per task).
/// NOTE 2026-05-12: tried bumping to 32KB but it deterministically broke
/// SYS_SPAWN's memcmp path (likely due to a layout-dependent bug elsewhere
/// in the bootloader page-mapping or .bss size limit). Reverted to 16KB.
/// Per-task stack size. Stack-overflow detection uses canaries
/// (`init_stack_canaries`) + PF-handler check (`check_stack_canaries`);
/// real unmapped guard pages are still future work.
///
/// **Bumped from 16 KiB → 64 KiB on 2026-05-18** to absorb the
/// layout-sensitivity bug behind task #36. The pattern: adding
/// new code to the binary (USB, larger env block, etc.) changes
/// LLVM's inlining + spill decisions, which sometimes pushes a
/// function's stack frame past the 16 KiB cliff. Overflow writes
/// PAST a slot's bottom into the previous slot's TOP — including
/// the iret-RIP slot at `[top - 56]` — corrupting the next context
/// switch's return address. Surfaces later as #GP at a non-canonical
/// RIP (stuck-bit pattern, bits 56+58 set) during user-program
/// execution.
///
/// **Bumped 64 KiB → 128 KiB on 2026-05-20 (task #41).** Real unmapped
/// guard pages now sit below every task stack (`init_stack_guard_pages`),
/// so an overflow faults precisely instead of smashing the neighbour. The
/// guard immediately exposed that the demo-runner task (`init_loader`)
/// actually uses just over 64 KiB during the FS-snapshot demo — it had
/// been silently spilling into the unused top of the adjacent slot. 128 KiB
/// restores headroom above that real watermark.
///
/// Bumped to 256 KiB: the demo-runner (`init_loader`) is a single giant
/// function whose LLVM-chosen frame grows with kernel code size, and the M22
/// agent + TUI modules pushed its peak (during DEMO 26's `remove_child` FS path
/// + a timer frame) past 128 KiB → a #DF in slot 5's guard page. 256 KiB
/// restores generous headroom over the ~64 KiB real watermark and absorbs
/// future code-size growth. NOTE: this stack is layout-sensitive — a deep-demo
/// #DF whose CR2 sits just below RSP in the `TASK_STACKS` range means bump this.
///
/// At 256 KiB × MAX_TASKS = 4 MiB of BSS, still comfortable against the
/// 16 MiB heap budget. Overflow is a hard fault, not silent corruption.
pub const TASK_STACK_SIZE: usize = 256 * 1024;

/// Task state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TaskState {
    Empty = 0,
    Ready = 1,
    Running = 2,
    Blocked = 3,
    Exited = 4,
}

/// Why a task is blocked — checked by `pick_next()` to auto-unblock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// Not blocked (or generic block with no auto-unblock).
    None,
    /// Sleeping until `wake_at` ticks.
    Sleep,
    /// Waiting for data on a pipe (read end).
    PipeRead(usize),
    /// Waiting for space on a pipe (write end).
    PipeWrite(usize),
    /// Waiting for a child process to exit.
    WaitChild,
    /// Tier 3.2 (task #46): blocked in `SYS_FUTEX_WAIT` on a u32 word
    /// at this virtual address. Woken explicitly by `futex_wake` /
    /// `SYS_FUTEX_WAKE`. Not auto-unblocked by pick_next — that would
    /// race against the wake-side count semantics.
    Futex(u64),
    /// Tier 3.1 (task #45): blocked in `SYS_THREAD_JOIN` waiting for
    /// the named scheduler slot to reach `TaskState::Exited`. pick_next
    /// auto-unblocks once the target's state is Exited.
    JoinTask(usize),
}

/// Platform-independent task metadata.
/// The arch-specific context (registers, stack pointer) is stored
/// separately by the platform crate.
#[derive(Clone, Copy)]
pub struct TaskInfo {
    /// Task ID
    pub id: usize,
    /// Task state
    pub state: TaskState,
    /// Task name (for debugging)
    pub name: &'static str,
    /// Number of times this task has been scheduled
    pub run_count: u64,
    /// Maximum security tier this task can access (0=Public, 3=Secret)
    pub max_tier: u8,
    /// true = runs in kernel mode, false = runs in user mode
    pub is_kernel: bool,
    /// Tick count at which a Blocked(sleep) task should wake up.
    /// Only meaningful when state == Blocked. 0 = not a timed block.
    pub wake_at: u64,
    /// Why this task is blocked (for auto-unblock in pick_next).
    pub block_reason: BlockReason,
    /// Effective user identity for this task. Used everywhere we need to
    /// answer "who is this task acting for?" — policy evaluation, redaction
    /// context, request ownership. Inherited from the parent at spawn, or
    /// rewritten via `SYS_SETUID`. Before this field existed, the kernel
    /// was using the scheduler slot index as a proxy, which is wrong:
    /// slots get recycled, and one user can run many concurrent tasks.
    pub user_id: u8,
    /// Tier 3 (task #45): exit code published when this task transitions
    /// to `Exited`. Read by `SYS_THREAD_JOIN` once the join target is
    /// observed Exited. Default 0 — overwritten by `SYS_EXIT(code)`.
    pub exit_code: u64,
}

impl TaskInfo {
    pub const fn empty() -> Self {
        Self {
            id: 0,
            state: TaskState::Empty,
            name: "",
            run_count: 0,
            max_tier: 0,
            is_kernel: true,
            wake_at: 0,
            block_reason: BlockReason::None,
            // 255 == `security::user_ids::NOBODY`. We don't import the
            // constant here because TaskInfo::empty must stay `const`.
            user_id: 255,
            exit_code: 0,
        }
    }
}

/// Task table
pub static mut TASKS: [TaskInfo; MAX_TASKS] = [TaskInfo::empty(); MAX_TASKS];

/// Current running task index
pub static CURRENT_TASK: AtomicUsize = AtomicUsize::new(0);

/// Next task ID to assign
pub static NEXT_TASK_ID: AtomicUsize = AtomicUsize::new(1);

/// Total context switches
pub static CONTEXT_SWITCHES: AtomicU64 = AtomicU64::new(0);

/// Scheduler initialized flag
pub static SCHEDULER_INITIALIZED: AtomicUsize = AtomicUsize::new(0);

/// Get the max tier for the current task
pub fn current_task_max_tier() -> u8 {
    let current = CURRENT_TASK.load(Ordering::SeqCst);
    unsafe {
        let tasks = &raw const TASKS;
        (*tasks)[current].max_tier
    }
}

/// Is the current task a Ring-3 (user-mode) task? The syscall layer uses
/// this to decide whether pointer arguments must satisfy USER_ADDR_LIMIT:
/// kernel tasks legitimately `dispatch()` with kernel-space buffers
/// (agent.rs, editor.rs, demos), so validation is per-caller, not global.
pub fn current_task_is_user() -> bool {
    let current = CURRENT_TASK.load(Ordering::SeqCst);
    unsafe {
        let tasks = &raw const TASKS;
        !(*tasks)[current].is_kernel
    }
}

/// Get the current task index
pub fn current_task_index() -> usize {
    CURRENT_TASK.load(Ordering::SeqCst)
}

/// Get the effective user id for the current task. Replaces the older
/// pattern of `current_task_index() as u8`, which conflated scheduler
/// slots with user identity.
pub fn current_user_id() -> u8 {
    let current = CURRENT_TASK.load(Ordering::SeqCst);
    unsafe {
        let tasks = &raw const TASKS;
        (*tasks)[current].user_id
    }
}

/// Rewrite the effective user id for the current task. Caller is
/// responsible for enforcing setuid policy — this just mutates the field.
pub fn set_current_user_id(uid: u8) {
    let current = CURRENT_TASK.load(Ordering::SeqCst);
    unsafe {
        let tasks = &raw mut TASKS;
        (*tasks)[current].user_id = uid;
    }
}

/// Round-robin scheduling: find the next ready task.
/// Returns (current_index, next_index). Platform crate performs the actual context switch.
pub fn pick_next() -> Option<(usize, usize)> {
    if SCHEDULER_INITIALIZED.load(Ordering::SeqCst) == 0 {
        return None;
    }

    unsafe {
        let tasks = &raw mut TASKS;
        let current = CURRENT_TASK.load(Ordering::SeqCst);

        // Check blocked tasks for unblock conditions
        let now = crate::platform::ticks();
        for i in 0..MAX_TASKS {
            if (*tasks)[i].state != TaskState::Blocked {
                continue;
            }
            let should_wake = match (*tasks)[i].block_reason {
                BlockReason::Sleep => {
                    (*tasks)[i].wake_at > 0 && now >= (*tasks)[i].wake_at
                }
                BlockReason::PipeRead(pipe_id) => {
                    crate::ipc::pipe::has_data(pipe_id)
                }
                BlockReason::PipeWrite(pipe_id) => {
                    crate::ipc::pipe::has_space(pipe_id)
                }
                BlockReason::JoinTask(target_slot) => {
                    // Tier 3.1: unblock once the join target has exited.
                    target_slot < MAX_TASKS
                        && (*tasks)[target_slot].state == TaskState::Exited
                }
                BlockReason::Futex(_)
                | BlockReason::WaitChild
                | BlockReason::None => {
                    // Futex is woken explicitly by `futex_wake`. WaitChild
                    // is woken explicitly by process::exit(). None blocks
                    // are woken externally.
                    false
                }
            };
            if should_wake {
                (*tasks)[i].state = TaskState::Ready;
                (*tasks)[i].wake_at = 0;
                (*tasks)[i].block_reason = BlockReason::None;
            }
        }

        // Mark current task as ready (if it was running)
        if (*tasks)[current].state == TaskState::Running {
            (*tasks)[current].state = TaskState::Ready;
        }

        // Find next ready task (round-robin)
        let mut next = current;
        for _ in 0..MAX_TASKS {
            next = (next + 1) % MAX_TASKS;
            if (*tasks)[next].state == TaskState::Ready {
                break;
            }
        }

        // If no ready task found, stay with current (or task 0)
        if (*tasks)[next].state != TaskState::Ready {
            if (*tasks)[current].state == TaskState::Ready {
                next = current;
            } else {
                next = 0;
            }
        }

        // Skip if same task
        if next == current && (*tasks)[current].state == TaskState::Ready {
            (*tasks)[current].state = TaskState::Running;
            return None;
        }

        // Update state
        (*tasks)[next].state = TaskState::Running;
        (*tasks)[next].run_count += 1;
        CURRENT_TASK.store(next, Ordering::SeqCst);
        CONTEXT_SWITCHES.fetch_add(1, Ordering::Relaxed);

        Some((current, next))
    }
}

/// Initialize scheduler (call from platform crate after setting up task 0)
pub fn init_core() {
    unsafe {
        let tasks = &raw mut TASKS;
        (*tasks)[0] = TaskInfo {
            id: 0,
            state: TaskState::Running,
            name: "kernel",
            run_count: 0,
            max_tier: 3, // Kernel has Secret access
            is_kernel: true,
            wake_at: 0,
            block_reason: BlockReason::None,
            // The bootstrap kernel task runs as SYSTEM. Spawned children
            // inherit this until something explicitly drops privilege.
            user_id: 0, // == security::user_ids::SYSTEM
            exit_code: 0,
        };
    }
    CURRENT_TASK.store(0, Ordering::SeqCst);
    SCHEDULER_INITIALIZED.store(1, Ordering::SeqCst);
    crate::platform::log("[OK] Scheduler core initialized\n");
}

/// Allocate a task slot and return its index, or None if full.
///
/// The slot is reserved in `Blocked` state with no block reason. It will not
/// be chosen by `pick_next` until the platform spawn code finishes setting up
/// the per-task context (registers, stack, page tables) and calls
/// [`mark_ready`]. This avoids a TOCTOU window where a timer interrupt could
/// fire between slot allocation and context initialization, causing
/// `context_switch` to load a zero RSP from the uninitialized context.
pub fn alloc_task_slot(name: &'static str, max_tier: u8, is_kernel: bool) -> Option<usize> {
    // Default: inherit the spawning task's user identity. Callers that need
    // a different uid should follow with `set_user_id`.
    alloc_task_slot_with_user(name, max_tier, is_kernel, current_user_id())
}

/// Like `alloc_task_slot` but lets the caller pin an explicit user id on
/// the new slot. Used by spawn paths that want to drop privilege at the
/// moment of creation rather than after the child starts running.
pub fn alloc_task_slot_with_user(
    name: &'static str,
    max_tier: u8,
    is_kernel: bool,
    user_id: u8,
) -> Option<usize> {
    unsafe {
        let tasks = &raw mut TASKS;
        // Reusable slots = Empty (never used) OR Exited (finished and
        // never picked again by pick_next). Without this, MAX_TASKS=8
        // becomes a hard cap on the *cumulative* spawn count instead of
        // the *concurrent* one, and a few demo binaries exhaust it.
        // Per-task resources (kstack, page tables, fxsave area) are
        // still tied to the slot index — overwriting an Exited slot's
        // TaskInfo is safe; reclaiming its address-space frames is a
        // future task (TODO: hook AddressSpace::destroy from cleanup).
        for i in 1..MAX_TASKS {
            let was_exited = matches!((*tasks)[i].state, TaskState::Exited);
            let reusable = was_exited
                || matches!((*tasks)[i].state, TaskState::Empty);
            if reusable {
                // Reap any platform-side per-slot resources from the
                // previous tenant (e.g. the AddressSpace's PML4 +
                // subtable frames on x86_64). Done here, not at exit
                // time, so we never destroy state still in use by the
                // dying task's kernel-mode unwind path.
                if was_exited {
                    crate::platform::get().reap_slot(i);
                }
                let task_id = NEXT_TASK_ID.fetch_add(1, Ordering::SeqCst);
                (*tasks)[i] = TaskInfo {
                    id: task_id,
                    state: TaskState::Blocked,
                    name,
                    run_count: 0,
                    max_tier,
                    is_kernel,
                    wake_at: 0,
                    // BlockReason::None means "blocked indefinitely until
                    // explicitly woken" — pick_next won't auto-unblock it.
                    block_reason: BlockReason::None,
                    user_id,
                    exit_code: 0,
                };
                return Some(i);
            }
        }
    }
    None
}

/// Force a specific user id onto an already-allocated slot. Used by spawn
/// paths that need to do this between `alloc_task_slot` and `mark_ready`,
/// without needing to know the calling user's id at slot-alloc time.
pub fn set_slot_user_id(slot: usize, uid: u8) {
    if slot >= MAX_TASKS { return; }
    unsafe {
        let tasks = &raw mut TASKS;
        (*tasks)[slot].user_id = uid;
    }
}

/// Transition a freshly-allocated slot to `Ready`, making it eligible for
/// scheduling. Call this from the platform spawn code AFTER the per-task
/// context (registers, stack, FX-save area) is fully initialized.
pub fn mark_ready(slot: usize) {
    if slot >= MAX_TASKS { return; }
    unsafe {
        let tasks = &raw mut TASKS;
        // Only promote slots that are still in our reserved "Blocked + None"
        // state. Don't clobber Running, Exited, or genuinely-blocked tasks.
        if (*tasks)[slot].state == TaskState::Blocked
            && matches!((*tasks)[slot].block_reason, BlockReason::None)
        {
            (*tasks)[slot].state = TaskState::Ready;
        }
    }
}

/// Get scheduler statistics: (context_switches, current_task_index)
pub fn stats() -> (u64, usize) {
    let switches = CONTEXT_SWITCHES.load(Ordering::Relaxed);
    let current = CURRENT_TASK.load(Ordering::SeqCst);
    (switches, current)
}

/// Read a slot's current state (for join/wait callers).
pub fn task_state(slot: usize) -> TaskState {
    if slot >= MAX_TASKS { return TaskState::Empty; }
    unsafe {
        let tasks = &raw const TASKS;
        (*tasks)[slot].state
    }
}

/// Read a slot's name (debug). Returns "" for invalid slots.
pub fn task_name(slot: usize) -> &'static str {
    if slot >= MAX_TASKS { return ""; }
    unsafe {
        let tasks = &raw const TASKS;
        (*tasks)[slot].name
    }
}

/// Read a slot's run_count — how many times pick_next has picked it
/// (incremented on every transition into Running). Useful to confirm
/// whether the scheduler is actually visiting a slot, vs. visiting it
/// but the task's first instruction failing silently.
pub fn task_run_count(slot: usize) -> u64 {
    if slot >= MAX_TASKS { return 0; }
    unsafe {
        let tasks = &raw const TASKS;
        (*tasks)[slot].run_count
    }
}

/// Read a slot's BlockReason — debug accessor for the platform crate's
/// scheduler diagnostics (kernel-core has no println!, so the platform
/// walks slots itself and prints).
pub fn task_block_reason(slot: usize) -> BlockReason {
    if slot >= MAX_TASKS { return BlockReason::None; }
    unsafe {
        let tasks = &raw const TASKS;
        (*tasks)[slot].block_reason
    }
}

/// Read a slot's exit code. Only meaningful when `task_state(slot)` is
/// `Exited` — otherwise 0.
pub fn task_exit_code(slot: usize) -> u64 {
    if slot >= MAX_TASKS { return 0; }
    unsafe {
        let tasks = &raw const TASKS;
        (*tasks)[slot].exit_code
    }
}

/// Tier 3.2 (#46) — Futex wait. Block the current task on `addr` until
/// a matching [`futex_wake`] fires (or another waker explicitly
/// transitions this slot back to Ready). The caller is responsible for
/// the compare-and-wait pattern: load `*addr`, decide if waiting is
/// still warranted, then call this. Without preemption inside syscall
/// handlers (single-CPU model), the load-then-block sequence in the
/// SYS_FUTEX_WAIT handler is atomic against other tasks' wakes.
///
/// After this returns, the caller should call [`crate::platform::schedule`]
/// to immediately yield to another task — `schedule()` is what actually
/// suspends; this function only sets the state.
pub fn futex_block(addr: u64) {
    let idx = CURRENT_TASK.load(Ordering::SeqCst);
    unsafe {
        let tasks = &raw mut TASKS;
        (*tasks)[idx].state = TaskState::Blocked;
        (*tasks)[idx].block_reason = BlockReason::Futex(addr);
        (*tasks)[idx].wake_at = 0;
    }
}

/// Tier 3.2 (#46) — Futex wake. Transition up to `max` tasks blocked on
/// `addr` back to Ready, returning the count woken. Caller's choice
/// whether to yield afterwards (`crate::platform::schedule()`).
pub fn futex_wake(addr: u64, max: usize) -> usize {
    if max == 0 { return 0; }
    let mut woken = 0usize;
    unsafe {
        let tasks = &raw mut TASKS;
        for i in 0..MAX_TASKS {
            if (*tasks)[i].state != TaskState::Blocked { continue; }
            if (*tasks)[i].block_reason != BlockReason::Futex(addr) { continue; }
            (*tasks)[i].state = TaskState::Ready;
            (*tasks)[i].block_reason = BlockReason::None;
            woken += 1;
            if woken >= max { break; }
        }
    }
    woken
}

/// Tier 3.1 (#45) — Block the current task waiting for `target_slot` to
/// reach `Exited`. pick_next auto-unblocks once the target observed
/// Exited. Caller follows up with `crate::platform::schedule()`.
pub fn join_block(target_slot: usize) {
    let idx = CURRENT_TASK.load(Ordering::SeqCst);
    unsafe {
        let tasks = &raw mut TASKS;
        (*tasks)[idx].state = TaskState::Blocked;
        (*tasks)[idx].block_reason = BlockReason::JoinTask(target_slot);
        (*tasks)[idx].wake_at = 0;
    }
}

/// Tier 3.1 (#45) — Set the exit code for the current task. Called by
/// SYS_EXIT before it transitions the task to `Exited`. Published to
/// any `join_block` waiters via [`task_exit_code`].
pub fn set_current_exit_code(code: u64) {
    let idx = CURRENT_TASK.load(Ordering::SeqCst);
    unsafe {
        let tasks = &raw mut TASKS;
        (*tasks)[idx].exit_code = code;
    }
}

/// Print scheduler status
pub fn print_status() {
    crate::platform::log("\n=== Scheduler Status ===\n");
    let (switches, current) = stats();
    crate::platform::log("  Context switches: ");
    crate::platform::log_num(switches);
    crate::platform::log("\n  Current task: ");
    crate::platform::log_num(current as u64);
    crate::platform::log("\n");

    unsafe {
        let tasks = &raw const TASKS;
        crate::platform::log("  Tasks:\n");
        for i in 0..MAX_TASKS {
            let task = &(*tasks)[i];
            if task.state != TaskState::Empty {
                crate::platform::log("    [");
                crate::platform::log_num(i as u64);
                crate::platform::log("] ");
                crate::platform::log(task.name);
                if task.is_kernel {
                    crate::platform::log(" (kernel)");
                } else {
                    crate::platform::log(" (user)");
                }
                crate::platform::log(" - ");
                match task.state {
                    TaskState::Empty => crate::platform::log("empty"),
                    TaskState::Ready => crate::platform::log("ready"),
                    TaskState::Running => crate::platform::log("RUNNING"),
                    TaskState::Blocked => crate::platform::log("blocked"),
                    TaskState::Exited => crate::platform::log("exited"),
                }
                crate::platform::log(" (runs: ");
                crate::platform::log_num(task.run_count);
                crate::platform::log(")\n");
            }
        }
    }
    crate::platform::log("========================\n");
}
