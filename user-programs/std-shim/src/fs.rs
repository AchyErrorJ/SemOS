//! `std::fs`-shaped surface — File, OpenOptions, and free helpers.
//!
//! M25 #51. `File` owns a kernel FD, implements `io::Read` + `io::Write`,
//! and closes on Drop. `OpenOptions` mirrors std's builder. The free
//! functions `read` / `read_to_string` / `write` cover the common
//! one-shot cases cargo/rustc use.
//!
//! Limitations carried over from the kernel FS:
//! - FWRITE is full-file overwrite (cursor resets) — append/seek-write
//!   aren't faithful yet. `write_all` in one call is correct; multiple
//!   `write` calls clobber. Tracked with the broader FS-cursor work.
//! - Content cap is MAX_FILE_CONTENT (64 KiB) per file.

use core_alloc::string::String;
use core_alloc::vec::Vec;
use crate::arch::{
    SYS_OPEN, SYS_CLOSE, SYS_FREAD, SYS_FWRITE, SYS_UNLINK, SYS_MKDIR,
    syscall1, syscall2, syscall3,
};
use crate::io::{self, Read, Write};

/// Bit layout mirrors kernel `syscall::open_flags`.
pub mod open_flags {
    pub const CREATE: u64 = 1 << 0;
    pub const DIRECTORY: u64 = 1 << 1;
    pub const TIER_SHIFT: u32 = 4;
}

// ---------------------------------------------------------------------
// Raw FD helpers (kept public — lower-level than File, used by the shim
// internals and by programs that want to avoid the File wrapper).
// ---------------------------------------------------------------------

/// Open a path; returns the raw FD or None on error.
pub fn open_raw(path: &str, flags: u64) -> Option<u64> {
    let r = unsafe { syscall3(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, flags) };
    if r == u64::MAX { None } else { Some(r) }
}

/// Close a raw FD.
pub fn close_raw(fd: u64) {
    unsafe {
        let _ = syscall1(SYS_CLOSE, fd);
    }
}

/// Read from a raw FD. Returns bytes read (0 == EOF) or None on error.
pub fn read_raw(fd: u64, buf: &mut [u8]) -> Option<usize> {
    let r = unsafe { syscall3(SYS_FREAD, fd, buf.as_mut_ptr() as u64, buf.len() as u64) };
    if r == u64::MAX { None } else { Some(r as usize) }
}

/// Write to a raw FD. Returns bytes written or None on error.
pub fn write_raw(fd: u64, buf: &[u8]) -> Option<usize> {
    let r = unsafe { syscall3(SYS_FWRITE, fd, buf.as_ptr() as u64, buf.len() as u64) };
    if r == u64::MAX { None } else { Some(r as usize) }
}

// ---------------------------------------------------------------------
// OpenOptions
// ---------------------------------------------------------------------

/// Builder mirroring `std::fs::OpenOptions`. Only the flags the kernel
/// honors today have an effect; the rest are recorded for API fidelity
/// and to drive the open() decision (e.g. create+truncate → CREATE).
#[derive(Clone, Copy, Debug)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
}

impl OpenOptions {
    pub const fn new() -> Self {
        Self {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
        }
    }
    pub fn read(&mut self, v: bool) -> &mut Self { self.read = v; self }
    pub fn write(&mut self, v: bool) -> &mut Self { self.write = v; self }
    pub fn append(&mut self, v: bool) -> &mut Self { self.append = v; self }
    pub fn truncate(&mut self, v: bool) -> &mut Self { self.truncate = v; self }
    pub fn create(&mut self, v: bool) -> &mut Self { self.create = v; self }
    pub fn create_new(&mut self, v: bool) -> &mut Self { self.create_new = v; self }

    pub fn open(&self, path: &str) -> io::Result<File> {
        let mut flags = 0u64;
        if self.create || self.create_new {
            flags |= open_flags::CREATE;
        }
        match open_raw(path, flags) {
            Some(fd) => Ok(File { fd }),
            None => Err(io::Error::other()),
        }
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------
// File
// ---------------------------------------------------------------------

/// An open file. Closes its FD on Drop.
pub struct File {
    fd: u64,
}

impl File {
    /// Open `path` read-only.
    pub fn open(path: &str) -> io::Result<File> {
        OpenOptions::new().read(true).open(path)
    }

    /// Create (or truncate) `path` for writing.
    pub fn create(path: &str) -> io::Result<File> {
        OpenOptions::new().write(true).create(true).truncate(true).open(path)
    }

    /// Raw FD accessor (escape hatch for syscalls the File API doesn't
    /// wrap yet, e.g. SYS_FSYNC).
    pub fn as_raw_fd(&self) -> u64 {
        self.fd
    }
}

impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        read_raw(self.fd, buf).ok_or_else(io::Error::other)
    }
}

impl Write for File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        write_raw(self.fd, buf).ok_or_else(io::Error::other)
    }
}

impl Drop for File {
    fn drop(&mut self) {
        close_raw(self.fd);
    }
}

// ---------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------

/// Read an entire file into a byte vector.
pub fn read(path: &str) -> io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Read an entire file into a String (UTF-8, lossy).
pub fn read_to_string(path: &str) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut s = String::new();
    f.read_to_string(&mut s)?;
    Ok(s)
}

/// Write a buffer to a file, creating/truncating it.
pub fn write(path: &str, contents: &[u8]) -> io::Result<()> {
    let mut f = File::create(path)?;
    f.write_all(contents)
}

/// Create a directory.
pub fn create_dir(path: &str) -> io::Result<()> {
    let r = unsafe { syscall2(SYS_MKDIR, path.as_ptr() as u64, path.len() as u64) };
    if r == 0 { Ok(()) } else { Err(io::Error::other()) }
}

/// Remove a file or empty directory.
pub fn remove_file(path: &str) -> io::Result<()> {
    let r = unsafe { syscall2(SYS_UNLINK, path.as_ptr() as u64, path.len() as u64) };
    if r == 0 { Ok(()) } else { Err(io::Error::other()) }
}
