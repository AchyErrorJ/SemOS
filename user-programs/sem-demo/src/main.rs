//! sem-demo.elf — Ring 3 user program that proves the Semantic OS
//! security thesis end-to-end from user space.
//!
//! Sequence:
//!   1. SYS_SEM_CREATE: register a Sensitive (tier 2) semantic object
//!      whose content contains PII (email, credit card).
//!   2. SYS_SEM_READ:   read the object back. Caller's max_tier == 2,
//!      so the kernel returns the **verbatim** PII.
//!   3. SYS_LLM_CONTEXT: ask the kernel to package the same object for
//!      LLM consumption. Even though we just read the raw bytes, the
//!      kernel applies tier-based redaction at the LLM/syscall boundary
//!      and we get the **masked** version back.
//!   4. Print both, side by side, to make the contrast visible on serial.
//!   5. SYS_EXIT(0).
//!
//! What this proves the kernel does that no user-space sandbox can:
//!   - same data, same caller, same byte buffer;
//!   - the kernel chooses redaction based on *intended downstream use*
//!     (raw read vs LLM context), not on caller capability;
//!   - the policy lives in Ring 0 so user code can't bypass it.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

// --- Syscall numbers (mirrored from kernel-core/src/syscall/mod.rs::numbers) ---
const SYS_WRITE:       u64 = 0;
const SYS_EXIT:        u64 = 2;
const SYS_SEM_CREATE:  u64 = 20;
const SYS_SEM_READ:    u64 = 21;
const SYS_LLM_CONTEXT: u64 = 51;

// --- Object identity ---
// Pick a SUID well outside the kernel-side demo's namespace
// (kernel-side uses 0x1000_0000_0000_0001 / _0002).
const SUID_HIGH: u64 = 0x1000_0000_0000_0042;
const SUID_LOW:  u64 = 0x5345_4D44_454D_4F00; // "SEMDEMO\0" little-endian-ish
const TIER_SENSITIVE: u64 = 2;

// --- Strings (in .rodata) ---
static PII_CONTENT: &[u8] =
    b"Sensitive: email=alice@example.com card=4111-1111-1111-1111";
static LBL_DIRECT:  &[u8] = b"  DIRECT READ: ";
static LBL_LLM:     &[u8] = b"  LLM CONTEXT: ";
static MSG_CREATE_FAIL: &[u8] = b"  [sem-demo] SYS_SEM_CREATE failed\n";
static MSG_READ_FAIL:   &[u8] = b"  [sem-demo] SYS_SEM_READ failed\n";
static MSG_CTX_FAIL:    &[u8] = b"  [sem-demo] SYS_LLM_CONTEXT failed\n";
static NL: &[u8] = b"\n";

// --- Buffers (in .bss) ---
// Read buffer for SYS_SEM_READ direct verbatim copy.
const READ_BUF_SIZE: usize = 256;
static mut READ_BUF: [u8; READ_BUF_SIZE] = [0; READ_BUF_SIZE];
// LLM-context output buffer. The kernel writes one or more entries as
// [u64 length][content bytes]... per entry.
const CTX_BUF_SIZE: usize = 1024;
static mut CTX_BUF:  [u8; CTX_BUF_SIZE] = [0; CTX_BUF_SIZE];
// SUID pairs argument for SYS_LLM_CONTEXT: an array of (high, low) u64
// pairs the kernel reads as &[(u64,u64)].
static mut SUID_PAIRS: [(u64, u64); 1] = [(0, 0); 1];

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start() -> ! {
    unsafe {
        // 1. Create a Sensitive object containing PII.
        // content_info packs (ptr_low_32 | len << 32) per the syscall ABI.
        let content_info =
            (PII_CONTENT.as_ptr() as u64 & 0xFFFF_FFFF)
            | ((PII_CONTENT.len() as u64) << 32);
        let create_rc = sys4(SYS_SEM_CREATE, SUID_HIGH, SUID_LOW, TIER_SENSITIVE, content_info);
        if create_rc == u64::MAX {
            sys2(SYS_WRITE, MSG_CREATE_FAIL.as_ptr() as u64, MSG_CREATE_FAIL.len() as u64);
            sys_exit(1);
        }

        // 2. Read it back directly. We have max_tier=2, object is tier 2,
        // so this is allowed and returns the verbatim bytes.
        let read_buf_ptr = (&raw mut READ_BUF) as *mut u8 as u64;
        let read_len = sys3(SYS_SEM_READ, SUID_HIGH, SUID_LOW, read_buf_ptr);
        if read_len == u64::MAX {
            sys2(SYS_WRITE, MSG_READ_FAIL.as_ptr() as u64, MSG_READ_FAIL.len() as u64);
            sys_exit(1);
        }

        // Print: "  DIRECT READ: <verbatim PII>\n"
        sys2(SYS_WRITE, LBL_DIRECT.as_ptr() as u64, LBL_DIRECT.len() as u64);
        sys2(SYS_WRITE, read_buf_ptr, read_len);
        sys2(SYS_WRITE, NL.as_ptr() as u64, NL.len() as u64);

        // 3. Ask the kernel for the LLM-bound context view of the same SUID.
        // Even though *we* could read raw, the LLM-context path applies
        // tier-based redaction inside the kernel.
        (*(&raw mut SUID_PAIRS))[0] = (SUID_HIGH, SUID_LOW);
        let suid_pairs_ptr = (&raw const SUID_PAIRS) as *const _ as u64;
        let ctx_buf_ptr = (&raw mut CTX_BUF) as *mut u8 as u64;
        let ctx_total = sys3(SYS_LLM_CONTEXT, suid_pairs_ptr, 1, ctx_buf_ptr);
        if ctx_total == u64::MAX || ctx_total < 8 {
            sys2(SYS_WRITE, MSG_CTX_FAIL.as_ptr() as u64, MSG_CTX_FAIL.len() as u64);
            sys_exit(1);
        }

        // 4. Decode the first entry from CTX_BUF: [u64 length][content...].
        let buf = &*(&raw const CTX_BUF);
        let mut len_bytes = [0u8; 8];
        len_bytes.copy_from_slice(&buf[0..8]);
        let entry_len = u64::from_le_bytes(len_bytes) as usize;
        let max_entry = CTX_BUF_SIZE.saturating_sub(8);
        let entry_len = if entry_len > max_entry { max_entry } else { entry_len };

        sys2(SYS_WRITE, LBL_LLM.as_ptr() as u64, LBL_LLM.len() as u64);
        sys2(SYS_WRITE, ctx_buf_ptr + 8, entry_len as u64);
        sys2(SYS_WRITE, NL.as_ptr() as u64, NL.len() as u64);

        sys_exit(0)
    }
}

// --- Syscall helpers ---

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
    unsafe { sys_exit(1) }
}
