// head1 v1 (buggy): print the first line of /apps/data/motd.txt.
//
// M4 self-repair demo (DEMO 88): v1 ships the "motd is never empty"
// assumption. After motd.txt is truncated to zero bytes, every run traps —
// the failure the scripted agent detects, diagnoses, and repairs.
//
// Guest constraints (same as wc.rs): no argv (input path compiled in), no
// slice indexing (panic_bounds_check is unlinkable), no div/rem, wrapping_*
// arithmetic only.
#![no_std]
#![no_main]

extern "C" {
    fn sys_exit(code: u64) -> !;
    fn sys_write(fd: u64, buf: *const u8, len: u64) -> i64;
    fn sys_open(path_ptr: *const u8, path_len: u64, flags: u64) -> i64;
    fn sys_close(fd: u64) -> i64;
    fn sys_fread(fd: u64, buf: *mut u8, len: u64) -> i64;
}

const INPUT_PATH: &[u8] = b"/apps/data/motd.txt";

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let fd = unsafe { sys_open(INPUT_PATH.as_ptr(), INPUT_PATH.len() as u64, 0) };
    if fd < 0 {
        unsafe { sys_exit(1) }
    }
    let mut buf = [0u8; 512];
    let n = unsafe { sys_fread(fd as u64, buf.as_mut_ptr(), buf.len() as u64) };
    unsafe { sys_close(fd as u64) };
    if n < 0 {
        unsafe { sys_exit(1) }
    }
    // v1 BUG: no n == 0 check — "motd always has content". A hosted build
    // would panic here (and our spin-loop panic handler would hang the
    // task); model the trap as a wild deref so the kernel kills the task
    // with the fault sentinel 0xFA01FA17 — the same detection signal a
    // real crash gives the health check, without stalling the demo.
    // Fault via an indirect CALL to address 1, NOT any pointer deref: raw
    // derefs (and read*/read_volatile) carry ub/precondition checks that pull
    // panic_null_pointer_dereference / panic_fmt — unlinkable in aot_semos.
    // A fn-pointer call has no instrumentation -> instruction-fetch #PF.
    if n == 0 {
        let f: fn() = unsafe { core::mem::transmute(1usize) };
        f();
    }
    // Print up to (excluding) the first newline, then one newline.
    let mut k = 0u64;
    while k < n as u64 {
        let b = unsafe { core::ptr::read(buf.as_ptr().add(k as usize)) };
        if b == b'\n' {
            break;
        }
        k = k.wrapping_add(1);
    }
    unsafe { sys_write(1, buf.as_ptr(), k) };
    unsafe { sys_write(1, b"\n".as_ptr(), 1) };
    unsafe { sys_exit(0) }
    loop {}
}
