// DEMO 83 (M2) BUGGY source: the program the scripted agent is asked to fix.
// Bug report (/tmp/agentgen/m2/bug-report.txt): sum_to(100) returns 4950,
// expected 5050 — off-by-one in the accumulator loop.
//
// Same shape as hello.rs: no_std/no_main, sys_write/sys_exit crt stubs
// injected by the semos-rustc linker.
#![no_std]
#![no_main]

extern "C" {
    fn sys_write(fd: u64, buf: *const u8, len: u64) -> i64;
    fn sys_exit(code: u64) -> !;
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

fn sum_to(n: u64) -> u64 {
    let mut acc = 0u64;
    let mut i = 1u64;
    while i < n {
        // BUG: exclusive bound drops `n` itself; should be `i <= n`.
        acc = acc.wrapping_add(i);
        i = i.wrapping_add(1);
    }
    acc
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    // Self-test: sum_to(100) must be 5050.
    let (msg, code): (&[u8], u64) = if sum_to(100) == 5050 {
        (b"calc selftest PASS: sum_to(100) = 5050\n", 0)
    } else {
        (b"calc selftest FAIL: sum_to(100) != 5050\n", 1)
    };
    unsafe {
        sys_write(1, msg.as_ptr(), msg.len() as u64);
        sys_exit(code);
    }
    loop {}
}
