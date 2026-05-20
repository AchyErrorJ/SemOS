//! `std::io`-shaped surface: print macros, Read/Write traits, Error.
//!
//! M25 #51 expands this from "print only" to the Read/Write traits
//! that `fs::File`, pipes, and sockets implement, plus the blanket
//! `read_to_end` / `read_to_string` / `write_all` helpers cargo and
//! rustc lean on. Error is a thin wrapper over an i32 code for now;
//! it grows an ErrorKind enum when callers start matching on it.

use core_alloc::string::String;
use core_alloc::vec::Vec;
use crate::arch::{SYS_WRITE, syscall2};

/// I/O error. Minimal today — wraps a numeric code. `ErrorKind`-style
/// classification is a follow-up once shim callers need to branch on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error {
    code: i32,
}

impl Error {
    pub const fn from_raw(code: i32) -> Self {
        Self { code }
    }
    /// Generic "operation failed" — used where the kernel only gives us
    /// a u64::MAX sentinel with no further detail.
    pub const fn other() -> Self {
        Self { code: -1 }
    }
    pub const fn raw_code(&self) -> i32 {
        self.code
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "io error (code {})", self.code)
    }
}

pub type Result<T> = core::result::Result<T, Error>;

/// Default scratch size for `read_to_end` chunks. Matches the kernel's
/// per-call SYS_FREAD cap so each read maps to one syscall.
const READ_CHUNK: usize = 4096;

/// The `Read` trait — same shape as `std::io::Read`, subset of methods.
pub trait Read {
    /// Read up to `buf.len()` bytes. Returns the count (0 == EOF).
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;

    /// Read everything to EOF, appending to `buf`. Returns total bytes read.
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        let start = buf.len();
        let mut chunk = [0u8; READ_CHUNK];
        loop {
            match self.read(&mut chunk)? {
                0 => break,
                n => buf.extend_from_slice(&chunk[..n]),
            }
        }
        Ok(buf.len() - start)
    }

    /// Read everything to EOF as UTF-8 (lossy). Returns bytes read.
    fn read_to_string(&mut self, out: &mut String) -> Result<usize> {
        let mut bytes = Vec::new();
        let n = self.read_to_end(&mut bytes)?;
        out.push_str(&String::from_utf8_lossy(&bytes));
        Ok(n)
    }
}

/// The `Write` trait — same shape as `std::io::Write`, subset of methods.
pub trait Write {
    /// Write some bytes; returns the count actually written.
    fn write(&mut self, buf: &[u8]) -> Result<usize>;

    /// Flush buffered output. Default: no-op (we're unbuffered).
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }

    /// Write the entire buffer, looping over short writes.
    fn write_all(&mut self, mut buf: &[u8]) -> Result<()> {
        while !buf.is_empty() {
            match self.write(buf)? {
                0 => return Err(Error::other()), // wrote nothing → give up
                n => buf = &buf[n..],
            }
        }
        Ok(())
    }

    /// `write!`/`writeln!` support via core::fmt.
    fn write_fmt(&mut self, args: core::fmt::Arguments<'_>) -> Result<()> {
        // Route fmt through a small adapter that calls write_all.
        struct Adapter<'a, W: Write + ?Sized> {
            inner: &'a mut W,
            err: Result<()>,
        }
        impl<W: Write + ?Sized> core::fmt::Write for Adapter<'_, W> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                match self.inner.write_all(s.as_bytes()) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        self.err = Err(e);
                        Err(core::fmt::Error)
                    }
                }
            }
        }
        let mut a = Adapter { inner: self, err: Ok(()) };
        match core::fmt::write(&mut a, args) {
            Ok(()) => Ok(()),
            Err(_) => a.err,
        }
    }
}

/// Write a UTF-8 string to stdout (fd=1) via SYS_WRITE. Returns the
/// number of bytes accepted, or `u64::MAX` on error.
#[inline]
pub fn write_str(s: &str) -> u64 {
    unsafe { syscall2(SYS_WRITE, s.as_ptr() as u64, s.len() as u64) }
}

/// Implements `core::fmt::Write` against stdout for the print macros.
pub struct Stdout;

impl core::fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = write_str(s);
        if n == u64::MAX {
            Err(core::fmt::Error)
        } else {
            Ok(())
        }
    }
}

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let n = unsafe { syscall2(SYS_WRITE, buf.as_ptr() as u64, buf.len() as u64) };
        if n == u64::MAX {
            Err(Error::other())
        } else {
            Ok(n as usize)
        }
    }
}

/// `print!` — writes to stdout, no newline.
///
/// Uses fully-qualified `core::fmt::Write::write_fmt` so it stays
/// unambiguous even when the caller has `semos_std::io::Write` (which
/// also has a `write_fmt`) in scope.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        let _ = $crate::__core::fmt::Write::write_fmt(
            &mut $crate::io::Stdout,
            ::core::format_args!($($arg)*),
        );
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

/// `eprintln!` — same sink as `println!` (single serial console).
#[macro_export]
macro_rules! eprintln {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {{
        $crate::print!($($arg)*);
        $crate::print!("\n");
    }};
}
