// semos-pkg package `cowsay` 1.0.0 (kind=bin, no deps): the classic cow,
// fixed saying (no argv in the no_std crt — cg_clif lacks the rsp-grab
// trampoline). The semos-pkg build prepends #![no_std]/#![no_main].
// EXPECT bytes (packaged in the registry, verified byte-exact at install):
// the OUT blob below. sys_* stubs come from the builder's prelude (shared,
// declared once — see fortune.rs's header note).

const OUT: &[u8] = b" ______\n< moo! >\n ------\n        \\   ^__^\n         \\  (oo)\\_______\n            (__)\\       )\\/\\\n                ||----w |\n                ||     ||\n";

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
