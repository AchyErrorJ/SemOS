// head1 v2 (fixed): print the first line of /apps/data/motd.txt.
//
// M4 self-repair demo (DEMO 88): the repair the scripted agent writes after
// diagnosing the v1 crash on a zero-byte motd.txt. One-line behavioral fix:
// empty input is not an error — there is no first line, so print nothing
// and exit 0.
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
    // v2 FIX: empty input is not an error — no first line, nothing to print.
    if n == 0 {
        unsafe { sys_exit(0) }
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
