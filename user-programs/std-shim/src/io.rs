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

/// `std::io::ErrorKind`-shaped classification. Subset — extend as
/// rustc + cg_clif callers need more variants. (G4 of Phase 4 flagged
/// ErrorKind::Unsupported as the immediate need for §1.5-stubs.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    InvalidInput,
    InvalidData,
    UnexpectedEof,
    WriteZero,
    Interrupted,
    Other,
    /// Operation not supported on this target (SemOS-specific stub returns).
    Unsupported,
}

/// I/O error — now carries an `ErrorKind` + an optional numeric code +
/// an optional static message. Mirrors `std::io::Error` shape for the
/// subset of patterns rustc + cg_clif use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    code: i32,
    msg: Option<&'static str>,
}

impl Error {
    pub const fn from_raw(code: i32) -> Self {
        Self { kind: ErrorKind::Other, code, msg: None }
    }

    /// `std::io::Error::new(ErrorKind, msg)`-shaped. Takes a static str
    /// message (no allocation needed for typical rustc error patterns).
    /// (G4 of Phase 4 flagged.)
    pub const fn new(kind: ErrorKind, msg: &'static str) -> Self {
        Self { kind, code: -1, msg: Some(msg) }
    }

    /// Generic "operation failed" — used where the kernel only gives us
    /// a u64::MAX sentinel with no further detail.
    pub const fn other() -> Self {
        Self { kind: ErrorKind::Other, code: -1, msg: None }
    }

    pub const fn raw_code(&self) -> i32 {
        self.code
    }

    /// Returns the `ErrorKind` classification of this error.
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.msg {
            Some(m) => write!(f, "io error: {} (kind={:?})", m, self.kind),
            None => write!(f, "io error (kind={:?}, code={})", self.kind, self.code),
        }
    }
}

// Phase 5b Stage E iter 10: rustc_thread_pool casts `&io::Error` to
// `&dyn core::error::Error` for its IOError variant. Implement the
// trait — semos_std::io::Error already provides Display + Debug.
impl core::error::Error for Error {}

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

/// Stderr sink. On SemOS the kernel exposes a single serial console via
/// SYS_WRITE (fd=1), so Stderr currently shares the Stdout pipe; the
/// type exists separately so rustc's `io::Stderr` / `let s = io::stderr()`
/// patterns resolve without rewrites (M27 R4 B3-followup recommendation).
/// A real split sink can come later if SYS_WRITE grows an fd parameter.
pub struct Stderr;

impl core::fmt::Write for Stderr {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = write_str(s);
        if n == u64::MAX {
            Err(core::fmt::Error)
        } else {
            Ok(())
        }
    }
}

impl Write for Stderr {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let n = unsafe { syscall2(SYS_WRITE, buf.as_ptr() as u64, buf.len() as u64) };
        if n == u64::MAX {
            Err(Error::other())
        } else {
            Ok(n as usize)
        }
    }
}

/// Factory mirroring `std::io::stdout()`. Returns a fresh handle by
/// value (cheap — Stdout is a unit type).
pub fn stdout() -> Stdout {
    Stdout
}

/// Factory mirroring `std::io::stderr()`. Same caveat as [`Stderr`]:
/// shares the SYS_WRITE sink with Stdout today.
pub fn stderr() -> Stderr {
    Stderr
}

/// `std::io::IsTerminal`. On SemOS the kernel framebuffer console
/// IS the terminal — return `true` for Stdout/Stderr/Stdin handles.
/// Plain file descriptors / Vec buffers report `false`.
pub trait IsTerminal {
    fn is_terminal(&self) -> bool;
}
impl IsTerminal for Stdout { fn is_terminal(&self) -> bool { true } }
impl IsTerminal for Stderr { fn is_terminal(&self) -> bool { true } }

/// `std::io::BufWriter` — minimal stub. On SemOS we don't buffer
/// I/O; writes go straight through `SYS_WRITE`. The `BufWriter`
/// type exists so rustc call sites that wrap stderr in
/// `BufWriter::new(io::stderr())` compile and behave identically
/// (modulo perf).
pub struct BufWriter<W: Write> { inner: W }
impl<W: Write> BufWriter<W> {
    pub fn new(inner: W) -> Self { Self { inner } }
    pub fn into_inner(self) -> core::result::Result<W, BufWriter<W>> { Ok(self.inner) }
    pub fn get_ref(&self) -> &W { &self.inner }
    pub fn get_mut(&mut self) -> &mut W { &mut self.inner }
}
impl<W: Write> Write for BufWriter<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> { self.inner.write(buf) }
}
impl<W: Write> core::fmt::Write for BufWriter<W> where W: core::fmt::Write {
    fn write_str(&mut self, s: &str) -> core::fmt::Result { self.inner.write_str(s) }
}

/// `impl io::Write for Vec<u8>` — std has this; many rustc + cg_clif
/// callers `write!(buf, ...)` against a Vec buffer. (G1 of Phase 4
/// flagged as the canonical pattern.)
impl Write for Vec<u8> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.extend_from_slice(buf);
        Ok(buf.len())
    }
}

/// `impl io::Read for &[u8]` — std has this. Lets callers use a byte
/// slice as a Read source for pattern-matching paths that take
/// `impl io::Read`.
impl Read for &[u8] {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = core::cmp::min(buf.len(), self.len());
        buf[..n].copy_from_slice(&self[..n]);
        *self = &self[n..];
        Ok(n)
    }
}

/// `std::io::copy`-shaped. Reads from `reader` and writes to `writer`
/// until EOF; returns total bytes copied. (G1 of Phase 4 flagged.)
pub fn copy<R: Read + ?Sized, W: Write + ?Sized>(reader: &mut R, writer: &mut W) -> Result<u64> {
    let mut buf = [0u8; 4096];
    let mut total = 0u64;
    loop {
        match reader.read(&mut buf)? {
            0 => return Ok(total),
            n => {
                writer.write_all(&buf[..n])?;
                total += n as u64;
            }
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
