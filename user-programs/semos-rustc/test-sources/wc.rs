// DEMO 87 (M3) FEATURE source: `wc` — the first SemOS-compiled guest that
// READS a file, via the sys_open/sys_fread/sys_close stubs added to
// aot_semos for M3. Counts lines/words/bytes of INPUT_PATH (compiled in —
// no argv in the no_std crt yet: cg_clif has no assembler for the rsp-grab
// trampoline std-shim uses, so args cannot reach _start).
//
// Raw pointer reads/writes instead of slice indexing: the sysroot-blob core
// can lack core::panicking shims (M2 hit panic_const_add_overflow), and
// unchecked ptr ops keep the generated code free of panic paths. All
// arithmetic is wrapping_* for the same reason, and / or % are avoided
// entirely (div-by-zero guards pull panic_const_div/rem_by_zero).
#![no_std]
#![no_main]

extern "C" {
    fn sys_write(fd: u64, buf: *const u8, len: u64) -> i64;
    fn sys_exit(code: u64) -> !;
    fn sys_open(path_ptr: *const u8, path_len: u64, flags: u64) -> i64;
    fn sys_close(fd: u64) -> i64;
    fn sys_fread(fd: u64, buf: *mut u8, len: u64) -> i64;
}

const INPUT_PATH: &[u8] = b"/tmp/agentgen/m3/data/sample.txt";

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

fn print_str(s: &[u8]) {
    unsafe {
        sys_write(1, s.as_ptr(), s.len() as u64);
    }
}

// Powers-of-10 table read via raw pointer: dynamic array indexing would
// call core::panicking::panic_bounds_check, which aot_semos can't link yet
// (only sys_* stubs resolve). Same reason there's no / or % anywhere in
// this file: div/rem by a runtime divisor pulls panic_const_div/rem_by_zero.
static POW10: [u64; 20] = [
    1,
    10,
    100,
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
    1_000_000_000,
    10_000_000_000,
    100_000_000_000,
    1_000_000_000_000,
    10_000_000_000_000,
    100_000_000_000_000,
    1_000_000_000_000_000,
    10_000_000_000_000_000,
    100_000_000_000_000_000,
    1_000_000_000_000_000_000,
    10_000_000_000_000_000_000,
];

fn pow10(i: u64) -> u64 {
    unsafe { core::ptr::read(POW10.as_ptr().add(i as usize)) }
}

fn print_num(n: u64) {
    // Highest power of ten <= n, scanning down from 10^19.
    let mut i = 19u64;
    while i > 0 && pow10(i) > n {
        i = i.wrapping_sub(1);
    }
    let mut rem = n;
    loop {
        let p = pow10(i);
        let mut digit = 0u8;
        while rem >= p {
            rem = rem.wrapping_sub(p);
            digit = digit.wrapping_add(1);
        }
        let b = [b'0'.wrapping_add(digit)];
        unsafe {
            sys_write(1, b.as_ptr(), 1);
        }
        if i == 0 {
            break;
        }
        i = i.wrapping_sub(1);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let fd = unsafe { sys_open(INPUT_PATH.as_ptr(), INPUT_PATH.len() as u64, 0) };
    if fd < 0 {
        print_str(b"wc: open failed\n");
        unsafe { sys_exit(1) };
    }
    let mut lines = 0u64;
    let mut words = 0u64;
    let mut bytes = 0u64;
    let mut in_word = false;
    let mut buf = [0u8; 256];
    loop {
        let n = unsafe { sys_fread(fd as u64, buf.as_mut_ptr(), buf.len() as u64) };
        if n < 0 {
            print_str(b"wc: read failed\n");
            unsafe {
                sys_close(fd as u64);
                sys_exit(1);
            }
        }
        if n == 0 {
            break;
        }
        bytes = bytes.wrapping_add(n as u64);
        let mut k = 0u64;
        while k < n as u64 {
            let b = unsafe { core::ptr::read(buf.as_ptr().add(k as usize)) };
            if b == b'\n' {
                lines = lines.wrapping_add(1);
            }
            let space = b == b' ' || b == b'\n' || b == b'\t' || b == b'\r';
            if space {
                in_word = false;
            } else if !in_word {
                in_word = true;
                words = words.wrapping_add(1);
            }
            k = k.wrapping_add(1);
        }
    }
    unsafe {
        sys_close(fd as u64);
    }
    print_num(lines);
    print_str(b" ");
    print_num(words);
    print_str(b" ");
    print_num(bytes);
    print_str(b"\n");
    unsafe {
        sys_exit(0);
    }
    loop {}
}
