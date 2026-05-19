//! `std::io`-shaped surface: print, write_str, error types.
//!
//! Only stdout (fd=1) is wired to SYS_WRITE for now; full Read/Write
//! traits land alongside `fs::File` in M25 Tier 2.

use crate::arch::{SYS_WRITE, syscall2};

/// Write a UTF-8 string to stdout (fd=1) via SYS_WRITE. Returns the
/// number of bytes the kernel accepted, or `u64::MAX` on error.
#[inline]
pub fn write_str(s: &str) -> u64 {
    unsafe {
        syscall2(SYS_WRITE, s.as_ptr() as u64, s.len() as u64)
    }
}

/// Implements `core::fmt::Write` against stdout. Lets `write!` /
/// `writeln!` / `format_args!` route to the kernel.
pub struct Stdout;

impl core::fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = write_str(s);
        if n == u64::MAX { Err(core::fmt::Error) } else { Ok(()) }
    }
}

/// `print!` — writes to stdout, no newline. Routes through SYS_WRITE.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use $crate::__core::fmt::Write;
        let _ = ::core::write!($crate::io::Stdout, $($arg)*);
    }};
}

/// `println!` — print + newline.
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {{
        $crate::print!($($arg)*);
        $crate::print!("\n");
    }};
}

/// `eprintln!` — same as `println!` today (we don't have separate
/// stderr routing; SYS_WRITE goes to the serial console which is
/// the only sink).
#[macro_export]
macro_rules! eprintln {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {{
        $crate::print!($($arg)*);
        $crate::print!("\n");
    }};
}
