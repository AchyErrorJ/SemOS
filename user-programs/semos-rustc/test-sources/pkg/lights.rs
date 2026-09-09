// semos-pkg package `lights` 1.0.0 (kind=bin, no deps) + hub intent block
// (DEMO 98). The binary is the manual control path (prints a state line —
// the real action is the kernel-dispatched UDP datagram from the hub); the
// intent block is what extends the hub's vocabulary. sys_* stubs come from
// the builder's prelude (see fortune.rs's header note).

const OUT: &[u8] = b"lights: manual toggle (hub intent is the real action)\n";

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    unsafe {
        sys_write(1, OUT.as_ptr(), OUT.len() as u64);
        sys_exit(0);
    }
    loop {}
}
