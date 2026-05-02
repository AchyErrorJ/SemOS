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
        // User-mode page fault — kill the task
        println!("PAGE FAULT in user task (CS=0x{:X}, addr={:?}, error={:?})",
            cs, Cr2::read(), error_code);
        kill_current_task();
        return;
    }
    // Kernel page fault.
    // Right now this fires intermittently after a Ring 3 task exits — see
    // task #40. Rather than hlt the whole kernel and lose every running
    // task, log it loudly and try to recover by killing the (kernel-mode)
    // task that took the fault. The leaked iret frame on its kernel stack
    // is bounded; we lose that one task's state but the rest of the
    // system (other Ready tasks) keeps running.
    println!("KERNEL PAGE FAULT — recovering by killing current task");
    println!("  Accessed Address: {:?}", Cr2::read());
    println!("  Error Code:       {:?}", error_code);
    println!("  RIP:              {:?}", stack_frame.instruction_pointer);
    println!("  RSP:              {:?}", stack_frame.stack_pointer);
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
    println!("[kernel] Killing task {} due to fault", idx);
    unsafe {
        let tasks = &raw mut kernel_core::scheduler::TASKS;
        (*tasks)[idx].state = kernel_core::scheduler::TaskState::Exited;
    }
    // Yield: actively call schedule() to context-switch off this task.
    // We're inside the page-fault handler, but that's fine — context_switch
    // saves the current (fault-handler) RSP/RIP into the dying task's slot,
    // which pick_next will never pick again because we just marked it
    // Exited. The leaked iret frame at the top of this task's kernel stack
    // is bounded — it goes away when we eventually free per-task resources.
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

/// Timer tick counter
static TIMER_TICKS: spin::Mutex<u64> = spin::Mutex::new(0);

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    {
        let mut ticks = TIMER_TICKS.lock();
        *ticks += 1;
    }

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

    send_eoi(1);
}

/// Get the current timer tick count.
pub fn get_ticks() -> u64 {
    *TIMER_TICKS.lock()
}
