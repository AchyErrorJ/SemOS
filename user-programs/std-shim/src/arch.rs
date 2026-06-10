//! Raw syscall ABI to the Semantic OS kernel.
//!
//! Syscall numbers mirror `kernel-core/src/syscall/mod.rs::numbers`. Keep
//! these in sync — there's no compile-time enforcement across the ABI
//! boundary today.

use core::arch::asm;

// --- Syscall numbers (mirror of kernel-core::syscall::numbers) ---
pub const SYS_WRITE:        u64 = 0;
pub const SYS_READ:         u64 = 1;
pub const SYS_EXIT:         u64 = 2;
pub const SYS_YIELD:        u64 = 3;
pub const SYS_GETPID:       u64 = 4;
pub const SYS_SLEEP:        u64 = 5;

pub const SYS_OPEN:         u64 = 10;
pub const SYS_CLOSE:        u64 = 11;
pub const SYS_FREAD:        u64 = 12;
pub const SYS_FWRITE:       u64 = 13;
pub const SYS_SEEK:         u64 = 14;
pub const SYS_STAT:         u64 = 15;
pub const SYS_MKDIR:        u64 = 16;
pub const SYS_UNLINK:       u64 = 17;
pub const SYS_READDIR:      u64 = 18;
pub const SYS_FSYNC:        u64 = 19;

pub const SYS_HEAP_ALLOC:   u64 = 34;
pub const SYS_HEAP_FREE:    u64 = 35;
pub const SYS_RENAME:       u64 = 36;
pub const SYS_TRUNCATE:     u64 = 37;
pub const SYS_STATX:        u64 = 38;
pub const SYS_MMAP_ANON:    u64 = 39;

pub const SYS_SPAWN:        u64 = 40;
pub const SYS_WAIT:         u64 = 41;
pub const SYS_KILL:         u64 = 42;
pub const SYS_DUP:          u64 = 44;
pub const SYS_DUP2:         u64 = 45;
pub const SYS_PIPE:         u64 = 46;

pub const SYS_GET_CWD:      u64 = 74;
pub const SYS_SET_CWD:      u64 = 75;
pub const SYS_GET_ENV:      u64 = 76;
pub const SYS_SET_ENV:      u64 = 77;

pub const SYS_FUTEX_WAIT:   u64 = 90;
pub const SYS_FUTEX_WAKE:   u64 = 91;
pub const SYS_THREAD_SPAWN: u64 = 92;
pub const SYS_THREAD_JOIN:  u64 = 93;
pub const SYS_WAITNB:       u64 = 94;

pub const SYS_DNS_RESOLVE:  u64 = 100;
pub const SYS_TCP_CONNECT:  u64 = 101;
pub const SYS_TCP_READ:     u64 = 102;
pub const SYS_TCP_WRITE:    u64 = 103;
pub const SYS_TCP_CLOSE:    u64 = 104;
pub const SYS_TCP_STATE:    u64 = 105;
pub const SYS_TIME:         u64 = 70;  // () -> ticks (100 Hz APIC)
pub const SYS_SYSINFO:      u64 = 73;  // (buf,len>=24) -> 0; [used,free,free_blocks] u64 LE
pub const SYS_PS:           u64 = 110; // (buf,len) -> task count; 24-byte records
pub const SYS_ASK:          u64 = 111; // (prompt,len,out,outlen) -> answer length
pub const SYS_AGENT:        u64 = 112; // (flags) -> 0/err; runs the interactive agent TUI
pub const SYS_EDIT:         u64 = 113; // (path_ptr, path_len) -> 0/err; runs the modal editor
pub const SYS_USBINFO:      u64 = 114; // () -> 0; dumps every USB port + enum'd slot to the TTY
pub const SYS_USBENUM:      u64 = 115; // () -> port_count; re-runs xHCI port enumeration
pub const SYS_PONG:         u64 = 116; // () -> 0/err; runs the fullscreen pong game
pub const SYS_TTY_SUPPRESS: u64 = 117; // (on: u64) -> 0; toggles kbd input drop for cooked-mode

/// SYS_TCP_READ/WRITE return this when the socket isn't ready yet (retry
/// after yielding). Distinct from 0 (EOF) and u64::MAX (hard error).
pub const NET_WOULDBLOCK:   u64 = u64::MAX - 1;

/// 4-arg syscall. Returns the kernel's u64 return value.
#[inline(always)]
pub unsafe fn syscall4(num: u64, a: u64, b: u64, c: u64, d: u64) -> u64 {
    let ret: u64;
    asm!(
        "syscall",
        in("rax") num,
        in("rdi") a,
        in("rsi") b,
        in("rdx") c,
        in("r10") d,
        lateout("rax") ret,
        out("rcx") _,
        out("r11") _,
        options(nostack),
    );
    ret
}

/// 3-arg convenience wrapper around [`syscall4`].
#[inline(always)]
pub unsafe fn syscall3(num: u64, a: u64, b: u64, c: u64) -> u64 {
    syscall4(num, a, b, c, 0)
}

/// 2-arg convenience.
#[inline(always)]
pub unsafe fn syscall2(num: u64, a: u64, b: u64) -> u64 {
    syscall4(num, a, b, 0, 0)
}

/// 1-arg convenience.
#[inline(always)]
pub unsafe fn syscall1(num: u64, a: u64) -> u64 {
    syscall4(num, a, 0, 0, 0)
}

/// 0-arg convenience.
#[inline(always)]
pub unsafe fn syscall0(num: u64) -> u64 {
    syscall4(num, 0, 0, 0, 0)
}
