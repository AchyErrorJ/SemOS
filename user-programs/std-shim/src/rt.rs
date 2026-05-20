//! Runtime startup glue: captures the SysV initial stack pointer and
//! provides the panic handler body.
//!
//! The `main!()` macro's naked `_start` stashes rsp here via [`init`]
//! before calling the user's main. [`crate::env::args`] reads the
//! argc/argv layout from it later.

use core::sync::atomic::{AtomicU64, Ordering};

/// The SysV stack pointer at process entry. `[sp]` is `argc`, followed
/// by the argv pointer array, a NULL, the envp pointer array, and a
/// NULL — see kernel `setup_user_argv`.
static INIT_SP: AtomicU64 = AtomicU64::new(0);

/// Record the initial stack pointer. Called once from `_start`.
pub fn init(sp: u64) {
    INIT_SP.store(sp, Ordering::SeqCst);
}

/// The captured initial stack pointer (0 if `_start` never ran, e.g.
/// in a unit-test harness).
pub fn initial_sp() -> u64 {
    INIT_SP.load(Ordering::SeqCst)
}

/// Panic handler body. Prints a short message to stdout (the only sink
/// today) and exits 101 — matching upstream's process-abort code.
pub fn handle_panic(info: &core::panic::PanicInfo) -> ! {
    use core::fmt::Write;
    let mut out = crate::io::Stdout;
    let _ = out.write_str("\n*** semos-std panic");
    if let Some(loc) = info.location() {
        let _ = write!(out, " at {}:{}", loc.file(), loc.line());
    }
    // PanicInfo's message is available on this toolchain via `message()`.
    let _ = write!(out, ": {}", info.message());
    let _ = out.write_str(" ***\n");
    crate::process::exit(101)
}
