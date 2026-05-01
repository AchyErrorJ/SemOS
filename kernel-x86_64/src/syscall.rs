//! x86_64 SYSCALL/SYSRET Entry Point
//!
//! Sets up the MSRs for fast system calls and provides the assembly
//! entry point that bridges to kernel_core::syscall::dispatch().
//!
//! # SYSCALL Convention (Semantic OS)
//!
//! | Register | Purpose              |
//! |----------|----------------------|
//! | rax      | Syscall number       |
//! | rdi      | Argument 0           |
//! | rsi      | Argument 1           |
//! | rdx      | Argument 2           |
//! | r10      | Argument 3           |
//! | rax      | Return value         |
//!
//! Note: SYSCALL clobbers rcx (saved RIP) and r11 (saved RFLAGS).
//! We follow the Linux convention of using r10 instead of rcx for arg3.
//!
//! # MSR Setup
//!
//! | MSR    | Value | Purpose                               |
//! |--------|-------|---------------------------------------|
//! | EFER   | +SCE  | Enable SYSCALL/SYSRET                 |
//! | STAR   | segs  | Kernel CS/SS in [47:32], User in [63:48] |
//! | LSTAR  | addr  | RIP for SYSCALL entry                 |
//! | SFMASK | flags | RFLAGS bits to clear on SYSCALL       |
//!
//! # Stack Switching
//!
//! When a SYSCALL arrives from Ring 3, we must switch to the kernel stack
//! before doing anything (the user stack is untrusted). We read the kernel
//! RSP from `gdt::KERNEL_RSP` and swap it in, saving the user RSP so we
//! can restore it on SYSRETQ.

use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask};
use x86_64::registers::rflags::RFlags;
use x86_64::addr::VirtAddr;

/// Initialize SYSCALL/SYSRET support.
///
/// Must be called after GDT is loaded.
pub fn init() {
    // Enable the SCE (System Call Extensions) bit in EFER
    unsafe {
        let efer = Efer::read();
        Efer::write(efer | EferFlags::SYSTEM_CALL_EXTENSIONS);
    }

    // STAR MSR setup using our GDT selectors:
    //   [47:32] = 0x08 — Kernel CS (kernel SS = 0x08 + 8 = 0x10 = Kernel Data)
    //   [63:48] = 0x10 — User base (SYSRET: SS = 0x10+8 = 0x18|3, CS = 0x10+16 = 0x20|3)
    //
    // This matches our GDT layout:
    //   0x08 = Kernel Code, 0x10 = Kernel Data
    //   0x18 = User Data,   0x20 = User Code
    let kernel_cs: u64 = crate::gdt::kernel_code_selector() as u64;
    let user_base: u64 = (crate::gdt::user_data_selector() as u64) - 8;
    // user_base = 0x18 - 8 = 0x10. SYSRET: SS = 0x10+8=0x18|3, CS = 0x10+16=0x20|3. Correct.

    unsafe {
        let star_value: u64 = (user_base << 48) | (kernel_cs << 32);
        core::arch::asm!(
            "wrmsr",
            in("ecx") 0xC000_0081u32, // IA32_STAR
            in("edx") (star_value >> 32) as u32,
            in("eax") star_value as u32,
        );
    }

    // LSTAR: Set the syscall entry point
    unsafe {
        LStar::write(VirtAddr::new(syscall_entry as *const () as u64));
    }

    // SFMASK: Clear IF (disable interrupts) and DF (direction flag) on SYSCALL entry
    // This ensures we have a safe environment before touching any kernel state
    unsafe {
        SFMask::write(RFlags::INTERRUPT_FLAG | RFlags::DIRECTION_FLAG);
    }

    crate::println!("    STAR: kernel_cs=0x{:02X} user_base=0x{:02X}", kernel_cs, user_base);
    crate::println!("    LSTAR: 0x{:016X}", syscall_entry as *const () as u64);
}

/// Naked syscall entry point.
///
/// When SYSCALL executes:
///   - RCX = saved user RIP (return address)
///   - R11 = saved user RFLAGS
///   - CS  = STAR[47:32] (kernel code)
///   - SS  = STAR[47:32] + 8 (kernel data)  [selector only, RSP unchanged!]
///   - RIP = LSTAR (this function)
///   - RFLAGS &= ~SFMASK
///
/// CRITICAL: RSP still points to the user stack! We must switch to the
/// kernel stack immediately, before pushing anything.
///
/// Flow:
///   1. Swap to kernel stack (save user RSP in a scratch register)
///   2. Save user context on kernel stack
///   3. Remap registers to dispatch(num, arg0, arg1, arg2, arg3)
///   4. Call dispatch
///   5. Restore user context
///   6. Swap back to user stack
///   7. SYSRETQ
#[unsafe(naked)]
extern "C" fn syscall_entry() {
    core::arch::naked_asm!(
        // --- Step 1: Switch to kernel stack ---
        // Save user RSP in a scratch location. We use SWAPGS-style manual swap:
        // r15 is callee-saved — we'll save it on the kernel stack shortly.
        // For now, stash user RSP in r15 temporarily.
        "mov r15, rsp",                     // r15 = user RSP
        "mov rsp, [rip + {kernel_rsp}]",    // rsp = kernel RSP

        // --- Step 2: Save user context on kernel stack ---
        "push r15",          // user RSP
        "push rcx",          // user RIP (SYSCALL saved it in RCX)
        "push r11",          // user RFLAGS (SYSCALL saved it in R11)
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",          // save r15 again (we clobbered it for user RSP)

        // --- Step 3: Remap to dispatch(num, arg0, arg1, arg2, arg3) ---
        // Incoming:  rax=num, rdi=arg0, rsi=arg1, rdx=arg2, r10=arg3
        // dispatch:  rdi=num, rsi=arg0, rdx=arg1, rcx=arg2, r8=arg3
        "mov r8, r10",       // arg3: r10 -> r8
        "mov rcx, rdx",      // arg2: rdx -> rcx
        "mov rdx, rsi",      // arg1: rsi -> rdx
        "mov rsi, rdi",      // arg0: rdi -> rsi
        "mov rdi, rax",      // num:  rax -> rdi

        // Re-enable interrupts during syscall handling
        "sti",

        // --- Step 4: Call Rust dispatch ---
        "call {dispatch}",

        // Return value is in rax — leave it for userspace

        // Disable interrupts before returning
        "cli",

        // --- Step 5: Restore user context ---
        "pop r15",           // (the extra r15 save)
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "pop r11",           // user RFLAGS -> r11 for SYSRETQ
        "pop rcx",           // user RIP -> rcx for SYSRETQ

        // --- Step 6: Restore user stack ---
        "pop rsp",           // user RSP (was pushed first)

        // --- Step 7: Return to user mode ---
        // SYSRETQ: RIP=RCX, RFLAGS=R11, CS/SS from STAR user fields
        "sysretq",

        dispatch = sym kernel_core::syscall::dispatch,
        kernel_rsp = sym crate::gdt::KERNEL_RSP,
    );
}

/// Issue a syscall from kernel mode (for testing).
///
/// In kernel mode we can call dispatch directly, but this exercises
/// the actual SYSCALL instruction path. Since we're already in Ring 0,
/// the stack switch reads KERNEL_RSP (which points to our kernel stack)
/// and we effectively stay on the same stack.
#[allow(dead_code)]
pub fn test_syscall() {
    // Issuing the real SYSCALL instruction from kernel mode would survive the
    // dispatch but blow up on SYSRET — SYSRET unconditionally drops to Ring 3,
    // which would try to execute kernel code in user mode and fault. Instead,
    // call dispatch directly to verify the syscall table is wired up.
    // The actual SYSCALL/SYSRET path is exercised by Ring 3 user tasks.
    let result = kernel_core::syscall::dispatch(6, 0, 0, 0, 0); // SYS_INFO
    crate::println!("    Dispatch test returned: {}", result);
}
