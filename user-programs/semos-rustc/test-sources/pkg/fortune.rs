// semos-pkg package `fortune` 1.0.0 (kind=lib): provides write_fortune()
// for dependent packages. A lib has NO _start, NO #[panic_handler], and NO
// extern "C" stubs — the semos-pkg build prepends the crate attributes AND
// the shared sys_* stub block (two extern blocks declaring the same symbol
// are E0428, so stubs live exactly once, in the builder's prelude). Rust
// items are order-independent within a module, so the prelude's stubs are
// in scope here regardless of concatenation order.

fn write_fortune() {
    const F: &[u8] = b"fortune: a journaled thought survives the crash\n";
    unsafe {
        sys_write(1, F.as_ptr(), F.len() as u64);
    }
}
