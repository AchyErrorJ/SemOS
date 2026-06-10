// DEMO 80 source: minimum a SemOS-target Rust compile needs.
// `#![no_std]` since the SemOS-resident rustc has no std crate,
// `panic_handler` since `no_std` doesn't provide one, `no_main`
// because SemOS user programs supply their own `_start`.
#![no_std]
#![no_main]

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}
