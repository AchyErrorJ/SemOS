// semos-pkg package `motd` 1.0.0 (kind=bin, deps=fortune): message of the
// day. Calls write_fortune() from the `fortune` lib package — unbuildable
// without it, which is what exercises the DAG resolver. The semos-pkg build
// concatenates dep lib sources before this file and prepends the crate
// attributes + shared sys_* stub prelude. EXPECT bytes: the banner +
// fortune's line.

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    const BANNER: &[u8] = b"message of the day:\n";
    unsafe {
        sys_write(1, BANNER.as_ptr(), BANNER.len() as u64);
    }
    write_fortune();
    unsafe {
        sys_exit(0);
    }
    loop {}
}
