// DEMO 93 (headline) FEATURE source: `greet` — the roadmap headline demo:
// "ask the agent to add a `greet` command, it works seconds later, the
// kernel never rebuilt". With SemFS write-through journaling it gains the
// persistence beat: the agent-added command survives a hard power cycle.
//
// Prints one fixed greeting line. No argv (cg_clif has no assembler for the
// rsp-grab trampoline std-shim uses, so args cannot reach _start) — the
// greeting is compiled in. Same guest-source constraints as wc.rs: raw
// pointer write, no slice indexing, no panic paths.
#![no_std]
#![no_main]

extern "C" {
    fn sys_write(fd: u64, buf: *const u8, len: u64) -> i64;
    fn sys_exit(code: u64) -> !;
}

// Keep byte-identical to GREET_EXPECTED in kernel-x86_64/src/main.rs —
// DEMO 93 verifies the guest's output byte-exact, both pre-install in
// isolation and post-install by bare name.
const GREETING: &[u8] = b"hello from SemOS: this command was added by the agent\n";

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    unsafe {
        sys_write(1, GREETING.as_ptr(), GREETING.len() as u64);
        sys_exit(0);
    }
    loop {}
}
