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

// M27 DEMO 80 — read-only sysroot blob staged on a SATA disk (Layer B).
// Renumbered 120-122 -> 136-138: those collided with SYS_DEMOS/PAIR/PAIRED
// (kernel dispatch matched the sysroot arms first). Keep both tables in sync.
pub const SYS_SYSROOT_INFO: u64 = 136; // (idx, name_buf_ptr, name_buf_len) -> len | MAX
pub const SYS_SYSROOT_READ: u64 = 137; // (idx, offset, buf_ptr, buf_len) -> n (0=EOF) | MAX
pub const SYS_FLASH_SYSROOT: u64 = 138; // () -> bytes copied | MAX; FAT usb0 -> sata0

// Game kit — raw input + fullscreen claim for Ring-3 apps.
pub const SYS_KB_POLL: u64 = 139;  // (out_ptr, out_len_bytes) -> n u32 events | MAX; non-blocking
pub const SYS_FB_CLAIM: u64 = 140; // (on) -> 0; 1 = claim screen+keyboard, 0 = release
pub const SYS_FB_FLIP: u64 = 141;  // () -> 0 | MAX; vblank-latched double-buffer swap (claimed apps only)

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
pub const SYS_NETINFO:      u64 = 116; // () -> 0/err; read-only network + active NIC diagnostics
pub const SYS_TTY_SUPPRESS: u64 = 117; // (on: u64) -> 0; toggles kbd input drop for cooked-mode
pub const SYS_FBINFO:       u64 = 118; // () -> 0; print framebuffer + native panel info
pub const SYS_BACKLIGHT:    u64 = 119; // (op,arg) -> percent | MAX; brightness control
pub const SYS_DEMOS:        u64 = 120; // () -> 0; run the full boot DEMO suite on demand
pub const SYS_PAIR:         u64 = 121; // (qr_ptr,qr_len) -> 1/0; M56 pairing handshake (console only)
pub const SYS_PAIRED:       u64 = 122; // () -> count; list paired devices
pub const SYS_UNPAIR:       u64 = 124; // (id_ptr,id_len) -> 1/0; forget a device (console only)
pub const SYS_NETLOG:       u64 = 132; // (target_ptr,target_len) -> bytes sent; UDP-send the kernel log ring
pub const SYS_FB_META:      u64 = 128; // (out_ptr,out_len>=64) -> 0/err
pub const SYS_FB_BLIT:      u64 = 129; // (xy_pack,wh_pack,pixels_ptr,pixel_count) -> 0/err
pub const SYS_MODESET:      u64 = 130; // (op) -> 0/err; guarded modeset control
pub const SYS_FB_WAIT_VBLANK: u64 = 131; // () -> 0/err; read-only vblank pacing
pub const SYS_WIFI_SCAN:    u64 = 123; // () -> n; scans WiFi, prints numbered network list
pub const SYS_WIFI_CONNECT: u64 = 125; // (idx, pass_ptr, pass_len) -> 1/0; connect to network idx
pub const SYS_VOUCH:        u64 = 126; // (path_ptr, path_len, grant_tier) -> 1/0; vouch a tool safe (console only)
pub const SYS_VOUCHES:      u64 = 127; // () -> count; print the active vouch grants
pub const SYS_VOUCH_SESSION: u64 = 133; // (tier, duration_secs, pw_ptr, pw_len) -> 1/0; session ceiling (console only)
pub const SYS_GET_VOUCH:    u64 = 134; // () -> (tier << 32) | remaining_secs, 0 when no live session
pub const SYS_SELFDEV:      u64 = 135; // (demo_n) -> 0/MAX; run self-dev demo 80|83|87|88 (console only)
pub const SYS_SEMOSPKG:     u64 = 142; // (op, name_ptr, name_len) -> 0/MAX; semos-pkg: 1=update 2=list 3=fetch 4=install 5=remove (list read-only; rest console only)

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
