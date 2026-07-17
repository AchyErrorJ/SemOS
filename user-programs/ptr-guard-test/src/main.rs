//! ptr-guard-test.elf — regression demo for the 2026-07-17 code review's
//! critical Ring-3 pointer-validation findings (P0 fix).
//!
//! Before the fix, any Ring-3 task — including a tier-0 sandboxed agent —
//! could:
//!   * SYS_WRITE a kernel pointer → arbitrary kernel memory disclosure to
//!     the TTY/serial (vouch table, Secret-tier objects, TLS keys), and
//!   * SYS_LLM_CONTEXT with a kernel out_ptr → arbitrary kernel memory
//!     WRITE (self-elevate max_tier, overwrite VOUCH_TABLE).
//!
//! The syscall layer now routes every caller pointer through
//! read_caller_slice/write_to_caller, which enforce USER_ADDR_LIMIT for
//! Ring-3 callers. This program attacks the fixed handlers and asserts
//! each one returns u64::MAX while the machine (and the valid path)
//! keeps working. Run from sem-sh: `ptr-guard-test.elf`, or flip the
//! gated demo slot next to DEMO 6 in kernel-x86_64/src/main.rs.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

// --- Syscall numbers ---
const SYS_WRITE:       u64 = 0;
const SYS_EXIT:        u64 = 2;
const SYS_SEM_CREATE:  u64 = 20;
const SYS_LLM_CONTEXT: u64 = 51;

/// A canonical, mapped kernel address (direct-map base). Pre-fix, SYS_WRITE
/// of this pointer printed kernel memory; post-fix it must be refused.
const KERNEL_ADDR: u64 = 0xffff_8000_0000_0000;

// --- Static buffers (in .bss, user space) ---
const CTX_BUF_SIZE: usize = 4096;
const CANARY: u8 = 0xAA;
static mut CTX_BUF:    [u8; CTX_BUF_SIZE] = [CANARY; CTX_BUF_SIZE];
static mut SUID_PAIRS: [(u64, u64); 1]    = [(0, 0)];

static mut PASSES: u32 = 0;
static mut FAILS:  u32 = 0;

// --- _start ----------------------------------------------------------------

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start() -> ! {
    print(b"================================================================\n");
    print(b"  PTR-GUARD TEST: Ring-3 kernel read/write primitive regression\n");
    print(b"  Every attack below must return u64::MAX and the machine must\n");
    print(b"  survive. (2026-07-17 review, critical findings #1 and #2)\n");
    print(b"================================================================\n");

    // Attack 1: SYS_WRITE of a kernel address → kernel-memory disclosure
    // pre-fix. Must be refused.
    let r = unsafe { sys2(SYS_WRITE, KERNEL_ADDR, 64) };
    check(b"1. SYS_WRITE(kernel addr) refused", r == u64::MAX);

    // Attack 2: SYS_WRITE of the null page. Must be refused.
    let r = unsafe { sys2(SYS_WRITE, 0, 64) };
    check(b"2. SYS_WRITE(null) refused", r == u64::MAX);

    // Attack 3: SYS_LLM_CONTEXT reading suid pairs from kernel memory.
    let ctx_ptr = (&raw mut CTX_BUF) as *mut u8 as u64;
    let r = unsafe { sys3(SYS_LLM_CONTEXT, KERNEL_ADDR, 1, ctx_ptr) };
    check(b"3. LLM_CONTEXT(kernel suids) refused", r == u64::MAX);

    // Attack 4: SYS_LLM_CONTEXT writing to a kernel address — the
    // arbitrary-kernel-write primitive. Must be refused before any write.
    let pairs_ptr = (&raw const SUID_PAIRS) as *const _ as u64;
    let r = unsafe { sys3(SYS_LLM_CONTEXT, pairs_ptr, 1, KERNEL_ADDR) };
    check(b"4. LLM_CONTEXT(kernel out_ptr) refused", r == u64::MAX);

    // Canary: nothing may have written into our user buffer during the
    // refused calls above.
    let intact = unsafe { (&*(&raw const CTX_BUF)).iter().all(|&b| b == CANARY) };
    check(b"5. user canary untouched by refused calls", intact);

    // Sanity 6: the valid path still works. A normal user-space write must
    // print (and return the byte count, not u64::MAX).
    let msg = b"    [ptr-guard] valid SYS_WRITE round-trip\n";
    let r = unsafe { sys2(SYS_WRITE, msg.as_ptr() as u64, msg.len() as u64) };
    check(b"6. valid SYS_WRITE accepted", r == msg.len() as u64);

    // Sanity 7: a fully-valid LLM_CONTEXT round-trip. Create a tier-0
    // object, then build a 1-object context into our user buffer.
    let content = b"ptr-guard sentinel";
    let suid_h = 0x1000_0000_0000_0A11;
    let suid_l = 0xCAFE_F00D_0000_0A11;
    let info = (content.as_ptr() as u64 & 0xFFFF_FFFF) | ((content.len() as u64) << 32);
    let created = unsafe { sys4(SYS_SEM_CREATE, suid_h, suid_l, 0, info) };
    if created != 0 {
        check(b"7. valid LLM_CONTEXT accepted", false);
    } else {
        unsafe { (*(&raw mut SUID_PAIRS))[0] = (suid_h, suid_l); }
        let written = unsafe { sys3(SYS_LLM_CONTEXT, pairs_ptr, 1, ctx_ptr) };
        // Expect 8-byte length prefix + content length.
        let expect = (8 + content.len()) as u64;
        let ok = written == expect
            && unsafe { &*(&raw const CTX_BUF) }[8..8 + content.len()] == *content;
        check(b"7. valid LLM_CONTEXT accepted", ok);
    }

    // Summary + exit code (0 = all pass) so a harness can check it.
    print(b"\n  [ptr-guard] passes=");
    print_dec(unsafe { *(&raw const PASSES) } as u64);
    print(b" fails=");
    print_dec(unsafe { *(&raw const FAILS) } as u64);
    print(b"\n");
    let fails = unsafe { *(&raw const FAILS) };
    if fails == 0 {
        print(b"  [ptr-guard] ALL PASS - primitives closed, machine alive\n");
    } else {
        print(b"  [ptr-guard] FAILURES PRESENT - see lines above\n");
    }
    unsafe { sys_exit(if fails == 0 { 0 } else { 1 }) }
}

fn check(label: &'static [u8], ok: bool) {
    print(b"    [");
    print(if ok { b"PASS" } else { b"FAIL" });
    print(b"] ");
    print(label);
    print(b"\n");
    unsafe {
        if ok { *(&raw mut PASSES) += 1; } else { *(&raw mut FAILS) += 1; }
    }
}

fn print(s: &[u8]) {
    unsafe { sys2(SYS_WRITE, s.as_ptr() as u64, s.len() as u64); }
}

fn print_dec(mut n: u64) {
    if n == 0 { print(b"0"); return; }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    print(&buf[i..]);
}

// --- syscall stubs ---------------------------------------------------------

#[inline(always)]
unsafe fn sys2(num: u64, a0: u64, a1: u64) -> u64 {
    let ret: u64;
    asm!(
        "syscall",
        in("rax") num, in("rdi") a0, in("rsi") a1,
        lateout("rax") ret,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    ret
}

#[inline(always)]
unsafe fn sys3(num: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let ret: u64;
    asm!(
        "syscall",
        in("rax") num, in("rdi") a0, in("rsi") a1, in("rdx") a2,
        lateout("rax") ret,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    ret
}

#[inline(always)]
unsafe fn sys4(num: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let ret: u64;
    asm!(
        "syscall",
        in("rax") num,
        in("rdi") a0, in("rsi") a1, in("rdx") a2, in("r10") a3,
        lateout("rax") ret,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    ret
}

#[inline(always)]
unsafe fn sys_exit(code: u64) -> ! {
    asm!(
        "syscall",
        in("rax") SYS_EXIT,
        in("rdi") code,
        options(nostack),
    );
    loop {}
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    print(b"  [ptr-guard] PANIC\n");
    unsafe { sys_exit(101) };
}
