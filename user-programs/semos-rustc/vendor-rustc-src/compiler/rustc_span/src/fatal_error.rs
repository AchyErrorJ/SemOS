// M27 R4 B1 (per docs/m27-recon/R4_blockers.md + plan §1.9):
// SemOS panic=abort. panic_unwind / catch_unwind are not available on the
// target. We keep the API surface (`FatalError::raise()` / `catch_fatal_errors`)
// so the rest of the rustc tree compiles unchanged, but the SEMANTICS change:
//   - `FatalError::raise()` aborts the whole rustc process (it cannot return
//     up the stack to `catch_fatal_errors`).
//   - `catch_fatal_errors` just runs the closure. If a FatalError was raised
//     anywhere inside, the process has already exited; there is no Err arm.
// User-facing limitation: "one error per compile" on SemOS. Documented in
// plan §1.9. Revisit when SemOS gets a stack-unwinder.

use alloc::boxed::Box;

/// Used as a return value to signify a fatal error occurred.
#[derive(Copy, Clone, Debug)]
#[must_use]
pub struct FatalError;

pub use rustc_data_structures::FatalErrorMarker;

// Don't implement Send on FatalError. This makes it impossible to `panic_any!(FatalError)`.
// We don't want to invoke the panic handler and print a backtrace for fatal errors.
impl !Send for FatalError {}

impl FatalError {
    pub fn raise(self) -> ! {
        // M27 R4 B1: was `std::panic::resume_unwind(Box::new(FatalErrorMarker))`.
        // SemOS has no unwinder; this aborts the process instead.
        let _payload: Box<FatalErrorMarker> = Box::new(FatalErrorMarker);
        // `panic!` lowers to abort on panic=abort targets, which is what
        // SemOS uses (per `compiler/rustc_target/src/spec/targets/
        // x86_64_unknown_none.rs`).
        panic!("rustc fatal error");
    }
}

impl core::fmt::Display for FatalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "fatal error")
    }
}

// `core::error::Error` is stable as of 1.81. Use it instead of `std::error::Error`.
impl core::error::Error for FatalError {}

/// Runs a closure and catches unwinds triggered by fatal errors.
///
/// The compiler currently unwinds with a special sentinel value to abort
/// compilation on fatal errors. This function catches that sentinel and turns
/// the panic into a `Result` instead.
///
/// **SemOS note (M27 R4 B1):** there is no unwinder on SemOS. Any FatalError
/// inside `f` will have already terminated the process before control returns
/// here. The `Result` is therefore always `Ok(...)` on SemOS; the `Err` arm
/// is preserved only for API compatibility with the rest of the tree.
pub fn catch_fatal_errors<F: FnOnce() -> R, R>(f: F) -> Result<R, FatalError> {
    // M27 R4 B1: was `panic::catch_unwind(panic::AssertUnwindSafe(f)).map_err(...)`.
    // catch_unwind on panic=abort just calls the closure (and the optimizer
    // erases the catch frame anyway), so this is the semantically-equivalent
    // shape for SemOS. If any FatalError is raised the process is already gone.
    Ok(f())
}
