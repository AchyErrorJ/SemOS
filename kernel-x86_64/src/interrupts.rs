//! Interrupt Handling for x86_64
//!
//! This module sets up the Interrupt Descriptor Table (IDT) for handling
//! CPU exceptions and hardware interrupts.
//!
//! # x86_64 Exception Vectors
//!
//! | Vector | Name                  | Type      |
//! |--------|-----------------------|-----------|
//! | 0      | Divide Error          | Fault     |
//! | 1      | Debug                 | Fault/Trap|
//! | 2      | NMI                   | Interrupt |
//! | 3      | Breakpoint            | Trap      |
//! | 4      | Overflow              | Trap      |
//! | 5      | Bound Range Exceeded  | Fault     |
//! | 6      | Invalid Opcode        | Fault     |
//! | 7      | Device Not Available  | Fault     |
//! | 8      | Double Fault          | Abort     |
//! | 10     | Invalid TSS           | Fault     |
//! | 11     | Segment Not Present   | Fault     |
//! | 12     | Stack-Segment Fault   | Fault     |
//! | 13     | General Protection    | Fault     |
//! | 14     | Page Fault            | Fault     |
//! | 16     | x87 FP Exception      | Fault     |
//! | 17     | Alignment Check       | Fault     |
//! | 18     | Machine Check         | Abort     |
//! | 19     | SIMD FP Exception     | Fault     |
//! | 20     | Virtualization        | Fault     |
//! | 32+    | Hardware Interrupts   | Interrupt |

use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use spin::Lazy;
use crate::println;

/// The Interrupt Descriptor Table
static IDT: Lazy<InterruptDescriptorTable> = Lazy::new(|| {
    let mut idt = InterruptDescriptorTable::new();

    // CPU Exceptions
    idt.divide_error.set_handler_fn(divide_error_handler);
    idt.debug.set_handler_fn(debug_handler);
    idt.non_maskable_interrupt.set_handler_fn(nmi_handler);
    idt.breakpoint.set_handler_fn(breakpoint_handler);
    idt.overflow.set_handler_fn(overflow_handler);
    idt.bound_range_exceeded.set_handler_fn(bound_range_handler);
    idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
    idt.device_not_available.set_handler_fn(device_not_available_handler);
    unsafe {
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(crate::gdt::DOUBLE_FAULT_IST_INDEX);
    }
    idt.invalid_tss.set_handler_fn(invalid_tss_handler);
    idt.segment_not_present.set_handler_fn(segment_not_present_handler);
    idt.stack_segment_fault.set_handler_fn(stack_segment_fault_handler);
    idt.general_protection_fault.set_handler_fn(general_protection_handler);
    idt.page_fault.set_handler_fn(page_fault_handler);
    idt.x87_floating_point.set_handler_fn(x87_fp_handler);
    idt.alignment_check.set_handler_fn(alignment_check_handler);
    idt.machine_check.set_handler_fn(machine_check_handler);
    idt.simd_floating_point.set_handler_fn(simd_fp_handler);
    idt.virtualization.set_handler_fn(virtualization_handler);

    // Hardware interrupts (PIC: vectors 32-47)
    idt[32].set_handler_fn(timer_interrupt_handler);
    idt[33].set_handler_fn(keyboard_interrupt_handler);

    // APIC spurious interrupt (vector 255). Must be installed even when the
    // APIC is not in use, so a stray interrupt during init doesn't triple-fault.
    idt[255].set_handler_fn(spurious_interrupt_handler);

    idt
});

extern "x86-interrupt" fn spurious_interrupt_handler(_stack_frame: InterruptStackFrame) {
    // Spurious interrupts must NOT be EOI'd per Intel SDM — just return.
}

/// Initialize interrupts.
pub fn init() {
    IDT.load();

    // Diagnostic: read IDT[14] (#PF) raw bytes to verify the
    // 64-bit handler offset was written intact. Each IDT entry is
    // 16 bytes; the handler offset is split across offset_low (u16
    // at +0), offset_middle (u16 at +6), offset_high (u32 at +8).
    // task#40 followup: the int log shows IDT[14] resolving to
    // offset_high=0 (handler addr truncated to low 32 bits). Verify
    // whether the truncation is there from boot or whether something
    // zeroes it later.
    unsafe {
        let idt_ptr = &*IDT as *const InterruptDescriptorTable as *const u8;
        let pf_entry = idt_ptr.add(14 * 16);
        let lo = core::ptr::read(pf_entry.add(0) as *const u16);
        let mid = core::ptr::read(pf_entry.add(6) as *const u16);
        let hi = core::ptr::read(pf_entry.add(8) as *const u32);
        let opts = core::ptr::read(pf_entry.add(4) as *const u16);
        let sel = core::ptr::read(pf_entry.add(2) as *const u16);
        let handler = ((hi as u64) << 32) | ((mid as u64) << 16) | (lo as u64);
        crate::println!(
            "[idt-dbg] post-load IDT[14] (#PF) lo=0x{:04x} mid=0x{:04x} hi=0x{:08x} -> handler=0x{:016x}  sel=0x{:04x} opts=0x{:04x}",
            lo, mid, hi, handler, sel, opts);
        crate::println!(
            "[idt-dbg]   expected page_fault_handler addr is high-half (0x100_0000_xxxx); hi=0 means truncated"
        );
    }

    // Initialize the PICs (remap to vectors 32-47)
    unsafe {
        init_pics();
    }

    // Enable interrupts
    x86_64::instructions::interrupts::enable();
}

// ============================================================================
// PIC Initialization (8259)
// ============================================================================

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

/// Initialize the 8259 PICs.
///
/// Remaps IRQ 0-7 to vectors 32-39, IRQ 8-15 to vectors 40-47.
unsafe fn init_pics() {
    use x86_64::instructions::port::Port;

    let mut pic1_cmd: Port<u8> = Port::new(PIC1_COMMAND);
    let mut pic1_data: Port<u8> = Port::new(PIC1_DATA);
    let mut pic2_cmd: Port<u8> = Port::new(PIC2_COMMAND);
    let mut pic2_data: Port<u8> = Port::new(PIC2_DATA);

    // ICW1: Initialize + ICW4 needed
    pic1_cmd.write(0x11);
    pic2_cmd.write(0x11);

    // ICW2: Vector offsets
    pic1_data.write(32); // PIC1 starts at vector 32
    pic2_data.write(40); // PIC2 starts at vector 40

    // ICW3: Cascade configuration
    pic1_data.write(4); // PIC2 at IRQ2
    pic2_data.write(2); // Cascade identity

    // ICW4: 8086 mode
    pic1_data.write(0x01);
    pic2_data.write(0x01);

    // Mask all interrupts except timer (IRQ0) and keyboard (IRQ1)
    pic1_data.write(0b11111100); // Enable IRQ0, IRQ1
    pic2_data.write(0b11111111); // Disable all on PIC2
}

/// Send End-Of-Interrupt to PIC.
fn send_eoi(irq: u8) {
    unsafe {
        use x86_64::instructions::port::Port;
        let mut pic1_cmd: Port<u8> = Port::new(PIC1_COMMAND);
        let mut pic2_cmd: Port<u8> = Port::new(PIC2_COMMAND);

        if irq >= 8 {
            pic2_cmd.write(0x20);
        }
        pic1_cmd.write(0x20);
    }
}

// ============================================================================
// Exception Handlers
// ============================================================================

extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: DIVIDE ERROR");
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn debug_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: DEBUG");
    println!("{:#?}", stack_frame);
}

extern "x86-interrupt" fn nmi_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: NMI");
    println!("{:#?}", stack_frame);
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT at {:?}", stack_frame.instruction_pointer);
}

extern "x86-interrupt" fn overflow_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: OVERFLOW");
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn bound_range_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BOUND RANGE EXCEEDED");
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    let cs = stack_frame.code_segment.0;
    if cs & 3 == 3 {
        println!("INVALID OPCODE in user task (CS=0x{:X}, RIP={:?})",
            cs, stack_frame.instruction_pointer);
        kill_current_task();
        return;
    }
    println!("EXCEPTION: INVALID OPCODE at {:?}", stack_frame.instruction_pointer);
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn device_not_available_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: DEVICE NOT AVAILABLE");
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    println!("EXCEPTION: DOUBLE FAULT");
    let cr2 = x86_64::registers::control::Cr2::read_raw();
    let slot = kernel_core::scheduler::current_task_index();
    println!("  CR2=0x{:x} (orig-fault addr if #PF)  current_slot={}", cr2, slot);
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn invalid_tss_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    println!("EXCEPTION: INVALID TSS (error: {})", error_code);
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn segment_not_present_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    println!("EXCEPTION: SEGMENT NOT PRESENT (error: {})", error_code);
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn stack_segment_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    println!("EXCEPTION: STACK SEGMENT FAULT (error: {})", error_code);
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn general_protection_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    // Check if the fault came from Ring 3 (user mode)
    let cs = stack_frame.code_segment.0;
    if cs & 3 == 3 {
        // User-mode fault — kill the task, don't crash the kernel
        println!("GP FAULT in user task (CS=0x{:X}, error=0x{:X}, RIP={:?})",
            cs, error_code, stack_frame.instruction_pointer);
        kill_current_task();
        return;
    }
    // Kernel fault — this is fatal
    println!("EXCEPTION: GENERAL PROTECTION FAULT (error: 0x{:X})", error_code);
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;
    let cs = stack_frame.code_segment.0;
    if cs & 3 == 3 {
        // User-mode page fault — kill the task. Almost always this fires
        // on the byte after a user `syscall` (SYS_EXIT returns to padding
        // that page-faults), so we keep the message short to avoid drowning
        // out the demo output. Use stderr-style brevity.
        //
        // Normally silent — this fires on the post-SYS_EXIT padding byte
        // for programs that don't loop after exit, so logging every one
        // would drown the demo output. The fault exit-code sentinel set
        // in kill_current_task() is what lets parents/pollers detect a
        // real crash (vs SYS_EXIT(0)). Flip USER_PF_VERBOSE to debug.
        const USER_PF_VERBOSE: bool = true; // M27 iter8: trace the semos-rustc run_compiler entry fault
        if USER_PF_VERBOSE {
            let cr2 = Cr2::read_raw();
            let rip = stack_frame.instruction_pointer.as_u64();
            let ursp = stack_frame.stack_pointer.as_u64();
            crate::println!(
                "[user-pf] slot={} RIP=0x{:x} CR2=0x{:x} uRSP=0x{:x} err={:?}",
                kernel_core::scheduler::current_task_index(),
                rip, cr2, ursp, error_code,
            );
        } else {
            let _ = (cs, error_code, Cr2::read_raw());
        }
        kill_current_task();
        return;
    }
    // Kernel page fault.
    // Historically fired ~20% of boots after the Ring 3 redact.elf SYS_EXIT
    // chain. Apparent fix 2026-05-06: the syscall_entry naked function was
    // clobbering caller-saved syscall arg registers (rdx/rsi/r10/r8/r9)
    // around the dispatch reorder, violating the Linux x86-64 syscall ABI
    // that user-mode Rust code (and rustc) assume. Saving and restoring
    // those registers around the dispatch call took the rate from ~20% to
    // 0/30 in stability runs. The diagnostic dump below is kept anyway —
    // if task#40 returns, this captures everything we'd need.
    //
    // Confirmed (2026-05-04 GDB+kernel-instrumented session, pre-fix):
    //   * the failing iretq is in the *timer wrapper* (just before the
    //     `iretq` at offset +0x6c of timer_interrupt_handler).
    //   * saved_RSP lands inside slot 4's per-task kstack near the top.
    //   * the iret frame at saved_RSP has RIP=0; CS/RFLAGS/SS are sometimes
    //     valid kernel values (0x8 / 0x10216 / 0x10) and sometimes all-zero
    //     — meaning a kernel-mode interrupt fired at the moment a kernel
    //     instruction had already loaded 0 into RIP (the next-instruction
    //     fetch never happened because the interrupt was serviced first).
    //   * static review of every indirect-call site reachable from the
    //     redact.elf syscall path (handle_write -> platform::log,
    //     pick_next -> platform::ticks, plus all of dispatch's match arms)
    //     found NO null function pointer in code — the platform vtable
    //     methods are all real .text symbols.
    //   * adding *any* code in schedule()'s hot path between fxrstor and
    //     context_switch — even just a memory load + compare — closes the
    //     race window and the bug stops reproducing. So the bug cannot be
    //     observed under instrumentation; we only see it by its aftermath.
    // Working hypothesis: a torn write of `static mut PLATFORM: &dyn
    // Platform` (16-byte fat pointer, two 8-byte stores) leaves the
    // vtable ptr stale or zero for a single read. Unconfirmed.
    // Memory: see ~/.claude/.../memory/project_semantic_os_task40.md
    // Rather than hlt, recover by killing the faulting task; pick_next
    // picks something else and the rest of the system keeps running.
    let cur = kernel_core::scheduler::current_task_index();
    println!("[kernel] PAGE FAULT in slot {} at RIP={:?} — recovering (task #40)",
        cur, stack_frame.instruction_pointer);
    // Stack-canary check: if any TASK_STACK[slot] bottom has been smashed,
    // we have a stack overflow somewhere — report it loudly.
    if let Some(smashed_slot) = crate::context::check_stack_canaries() {
        let bottom_addr = crate::context::stack_bottom_addr(smashed_slot);
        let actual = unsafe { core::ptr::read_volatile(bottom_addr as *const u64) };
        println!("[kernel] !! STACK OVERFLOW: TASK_STACKS[{}] canary at 0x{:x} = 0x{:016x} (expected 0x{:016x})",
            smashed_slot, bottom_addr, actual, crate::context::STACK_CANARY);
    }
    // The verbose state dump below is load-bearing — it adds enough
    // serial-output latency that subsequent demos reliably complete
    // (without it the kernel #PF cascade kills more tasks than it
    // should). Real fix is task #40's root cause; until then this
    // doubles as a workaround AND a diagnostic.
    let _ = (Cr2::read(), error_code, stack_frame.stack_pointer);
    unsafe {
        let tasks = &raw const kernel_core::scheduler::TASKS;
        for i in 0..kernel_core::scheduler::MAX_TASKS {
            let t = &(*tasks)[i];
            if matches!(t.state, kernel_core::scheduler::TaskState::Empty) { continue; }
            println!("    slot {} ({}): {:?}", i, t.name, t.state);
        }
    }

    // Task #40 diagnostic: when the kernel-mode RIP is exactly 0, dump
    // enough state to see *which* stack the failing iretq read from and
    // what each task's saved context looks like. We expect this to
    // reveal whether KERNEL_RSP/TSS.RSP0 is wrong, or whether one task's
    // saved RSP points into another task's per-task kernel stack.
    // task#40 minimal dump: summary line + the last 16 context-switch
    // events. If the next_rip column is 0 anywhere, that switch jumped
    // to address 0 and produced this fault.
    if stack_frame.instruction_pointer.as_u64() == 0 {
        let saved_rsp = stack_frame.stack_pointer.as_u64();
        let kernel_rsp = unsafe { crate::gdt::KERNEL_RSP };
        let cur_slot = kernel_core::scheduler::current_task_index();
        println!("[task#40] saved_RSP=0x{:x}  KERNEL_RSP=0x{:x}  cur_slot={}",
            saved_rsp, kernel_rsp, cur_slot);
        // Dump CONTEXTS[cur_slot].rip *right now* — if it's 0, the slot's
        // saved rip is genuinely 0 (someone wrote 0 there); if non-zero,
        // the rip got corrupted in-flight (compiler/CPU caching, fat-ptr
        // tear, or stack-pop racing with a write).
        unsafe {
            let contexts = &raw const crate::context::CONTEXTS;
            let c = &(*contexts)[cur_slot];
            println!("[task#40] CONTEXTS[{}]: rip=0x{:x} rsp=0x{:x} cr3=0x{:x}",
                cur_slot, c.rip, c.rsp, c.cr3);
        }
        // Dump quadwords WAY below saved_RSP. The actual zero return-address
        // popped by `retq` lives ~150-200 bytes below saved_RSP (depth of
        // schedule's frame + timer_handler frame). Plot a wide range so we
        // can see WHICH stack slot was 0 to begin with.
        unsafe {
            for off in -50i64..8i64 {
                let addr = saved_rsp.wrapping_add((off * 8) as u64);
                let val = core::ptr::read_volatile(addr as *const u64);
                let mark = if off == 0 {
                    " <-- saved_RSP"
                } else if val == 0 {
                    " ZERO"
                } else {
                    ""
                };
                println!("  [rsp{:+4}]  0x{:016x} = 0x{:016x}{}", off * 8, addr, val, mark);
            }
        }
        unsafe {
            let log_ptr = &raw const crate::context::CTX_LOG;
            let idx = crate::context::CTX_LOG_IDX as usize;
            let count = idx.min(16);
            for k in 0..count {
                let pos = (idx - count + k) & (crate::context::CTX_LOG_LEN - 1);
                let e = (*log_ptr)[pos];
                let mark = if e.next_rip == 0 { " !! next_rip=0" } else { "" };
                println!("  [ctx-{:3}] cur={} next={}  next_rip=0x{:x}{}",
                    idx - count + k, e.cur, e.next, e.next_rip, mark);
            }
        }
    }

    kill_current_task();
}

extern "x86-interrupt" fn x87_fp_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: x87 FLOATING POINT");
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn alignment_check_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    println!("EXCEPTION: ALIGNMENT CHECK (error: {})", error_code);
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn machine_check_handler(stack_frame: InterruptStackFrame) -> ! {
    println!("EXCEPTION: MACHINE CHECK");
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn simd_fp_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: SIMD FLOATING POINT");
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn virtualization_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: VIRTUALIZATION");
    println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

// ============================================================================
// User Task Fault Handling
// ============================================================================

/// Kill the current task when it causes a fault in Ring 3.
///
/// Marks the task as Exited in the scheduler, then context-switches away
/// to a different task. We never return from this call — when the
/// scheduler next picks an Exited task, it skips it.
///
/// Calling schedule() from within an interrupt handler is safe here:
/// context_switch saves our current state (mid-handler) into the dying
/// task's CONTEXTS slot. Since the task is Exited, pick_next will never
/// resume that saved state. The leaked stack frames on the dying task's
/// kernel stack are bounded (one task's worth) and will be reclaimed when
/// we eventually free per-task resources.
fn kill_current_task() {
    let idx = kernel_core::scheduler::current_task_index();
    let _ = idx; // silenced "[kernel] reaped task N" — expected after exit
    unsafe {
        let tasks = &raw mut kernel_core::scheduler::TASKS;
        // Sentinel exit code so SYS_THREAD_JOIN / DEMO pollers can
        // distinguish a faulted task from one that called SYS_EXIT(0).
        // Without this, a Ring-3 process that #PFs (e.g., user-stack
        // overflow during a large stack frame) reads as "exited 0" to
        // the parent, hiding the real failure. 0xFA01_FA17 == "fa01 fault".
        (*tasks)[idx].exit_code = 0xFA01_FA17;
        (*tasks)[idx].state = kernel_core::scheduler::TaskState::Exited;

        // Reclaim the dying task's address space (frees subtable + PML4
        // frames back to the page-table pool). We're still on this CR3,
        // but kernel higher-half mappings get freed too only if they
        // were tracked in space.subtables — which they aren't, since
        // map_4k only adds user-mapping subtables. So freeing here is
        // safe even before we leave the CR3.
        // (cleanup deferred to slot reuse — see context::reap_exited_slot,
        //  called by alloc_task_slot via Platform::reap_slot)
    }
    crate::context::schedule();
    // If pick_next found nothing better (every other slot Exited too),
    // schedule returns. Halt with interrupts on and try again on next tick.
    loop {
        unsafe { core::arch::asm!("sti; hlt", options(nomem, nostack)); }
    }
}

// ============================================================================
// Hardware Interrupt Handlers
// ============================================================================

/// Timer tick counter. **AtomicU64, not Mutex** — a spin::Mutex here would
/// deadlock: a normal-context reader (e.g. the M10 heartbeat) holds the lock
/// briefly; if the timer ISR fires in that window it tries to lock the same
/// mutex, can't get CPU back to the reader, and the ISR spins forever. Atomic
/// load/store is exactly what we need — single u64, no internal invariants.
static TIMER_TICKS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

extern "x86-interrupt" fn timer_interrupt_handler(stack_frame: InterruptStackFrame) {
    // task#40 diagnostic: if the CPU just pushed an iret frame with
    // RIP=0, log it. This catches "kernel was at RIP=0 when timer
    // fired" — the alternative theory is "iret frame got overwritten
    // between push and pop", which we'd distinguish from this firing.
    if stack_frame.instruction_pointer.as_u64() == 0 {
        let rsp = stack_frame.stack_pointer.as_u64();
        crate::println!("[timer-trap] RIP=0! cur_slot={} saved_RSP=0x{:x} CS=0x{:x} RFLAGS=0x{:x}",
            kernel_core::scheduler::current_task_index(),
            rsp,
            stack_frame.code_segment.0,
            stack_frame.cpu_flags.bits());
        // Dump 24 quadwords ABOVE saved_RSP (the stack the CPU was about
        // to fetch from / had just popped from). If RIP became 0 via a
        // `ret`, the popped-zero is at saved_RSP - 8 (now consumed). The
        // surrounding region shows what else is on the stack — return
        // addresses, saved regs, etc. — and helps trace the call chain.
        unsafe {
            for off in -8i64..16i64 {
                let addr = rsp.wrapping_add((off * 8) as u64);
                let val = core::ptr::read_volatile(addr as *const u64);
                let mark = if off == 0 { " <-- RSP" } else if off == -1 { " <-- popped" } else { "" };
                crate::println!("  [rsp{:+3}]  0x{:016x} = 0x{:016x}{}", off * 8, addr, val, mark);
            }
        }
    }
    TIMER_TICKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    // Polling fallback for the PS/2 keyboard: real hardware (W540 etc.)
    // routes legacy IRQ 1 through an IOAPIC pin that isn't pin 1 (ACPI
    // MADT Interrupt Source Override entries we don't yet parse). The
    // i8042 keyboard works fine — controller responds, scan is enabled
    // — but its interrupt never reaches the right CPU vector. Poll the
    // i8042 status port at every timer tick (62 Hz); the helper drains
    // up to 8 bytes per call, discarding trackpoint bytes (AUX-tagged)
    // and dispatching keyboard scancodes.
    let _ = crate::keyboard::poll_one_scancode();
    crate::keyboard::report_poll_stats(
        TIMER_TICKS.load(core::sync::atomic::Ordering::Relaxed),
    );

    // Send EOI before context switch — APIC if active, else legacy PIC.
    if crate::apic::is_active() {
        crate::apic::eoi();
    } else {
        send_eoi(0);
    }

    // Run the scheduler — may switch to a different task
    crate::context::schedule();
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    // Read the scancode from the PS/2 data port
    let mut port: Port<u8> = Port::new(0x60);
    let scancode = unsafe { port.read() };

    // Delegate to the keyboard driver
    crate::keyboard::handle_scancode(scancode);

    // EOI — must match the deliverer. On real hardware the IRQ is
    // routed via IOAPIC -> LAPIC (PIC is masked), so the LAPIC ISR bit
    // for vector 33 needs an LAPIC EOI to clear. Without this the LAPIC
    // refuses any further keyboard IRQs (one-shot dead keyboard, the
    // exact symptom seen on the W540). On QEMU's i8259 path apic
    // wasn't active and PIC EOI is correct.
    if crate::apic::is_active() {
        crate::apic::eoi();
    } else {
        send_eoi(1);
    }
}

/// Get the current timer tick count.
pub fn get_ticks() -> u64 {
    TIMER_TICKS.load(core::sync::atomic::Ordering::Relaxed)
}
