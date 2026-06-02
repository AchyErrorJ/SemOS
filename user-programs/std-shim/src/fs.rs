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
    SYS_RENAME, SYS_STATX,
    syscall1, syscall2, syscall3, syscall4,
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

/// `std::fs::read_link` stub. SemOS has no symlinks (Phase 14 FS is
/// flat), so the function always returns the input path back. Returns
/// `crate::path::PathBuf` to match `std::fs::read_link`'s signature.
pub fn read_link<P: AsRef<crate::path::Path>>(path: P) -> io::Result<crate::path::PathBuf> {
    Ok(crate::path::PathBuf::from(path.as_ref().as_str()))
}

/// `std::fs::read_dir` stub. SemOS lacks a Phase-14 directory
/// iteration syscall; returns an empty iterator. The rustc front-end's
/// sysroot probing never runs on the SemOS target in the v1 cut.
pub fn read_dir<P: AsRef<crate::path::Path>>(_path: P) -> io::Result<ReadDir> {
    Ok(ReadDir { _priv: () })
}

pub struct ReadDir { _priv: () }
impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;
    fn next(&mut self) -> Option<Self::Item> { None }
}

pub struct DirEntry { _priv: () }
impl DirEntry {
    pub fn path(&self) -> crate::path::PathBuf { crate::path::PathBuf::from("") }
    pub fn file_name(&self) -> crate::ffi::OsString { String::new() }
}

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

/// `std::fs::rename`-shaped. Atomic at the object level (SUID preserved).
/// (Phase 4 G4 flagged.) Backed by SYS_RENAME from Phase 14 Tier 2.
pub fn rename<P: AsRef<str>, Q: AsRef<str>>(from: P, to: Q) -> io::Result<()> {
    let f = from.as_ref();
    let t = to.as_ref();
    let r = unsafe {
        syscall4(
            SYS_RENAME,
            f.as_ptr() as u64,
            f.len() as u64,
            t.as_ptr() as u64,
            t.len() as u64,
        )
    };
    if r == 0 { Ok(()) } else { Err(io::Error::other()) }
}

/// `std::fs::copy`-shaped. Reads `from` to memory then writes to `to`.
/// Returns bytes copied. (Phase 4 G1 flagged.) Not atomic — `to` is
/// created+truncated then filled.
pub fn copy<P: AsRef<str>, Q: AsRef<str>>(from: P, to: Q) -> io::Result<u64> {
    let bytes = read(from.as_ref())?;
    write(to.as_ref(), &bytes)?;
    Ok(bytes.len() as u64)
}

// ---------------------------------------------------------------------
// StatX — file metadata (Phase 14 Tier 2, SYS_STATX 38)
// ---------------------------------------------------------------------

/// File metadata. Layout mirrors the kernel's `StatX` struct byte-for-byte
/// (kernel-core/src/syscall/mod.rs); SYS_STATX writes into this directly.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatX {
    pub size: u64,
    pub suid_high: u64,
    pub suid_low: u64,
    pub created_at: u64,
    pub modified_at: u64,
    /// 0=Binary, 1=Text, 2=Vector, 3=Directory, 4=Reference.
    pub file_type: u32,
    /// 0=Public, 1=Internal, 2=Sensitive, 3=Secret.
    pub tier: u32,
    pub _reserved: [u64; 3],
}

impl StatX {
    pub fn is_dir(&self) -> bool { self.file_type == 3 }
    pub fn is_file(&self) -> bool { self.file_type != 3 }
    pub fn len(&self) -> u64 { self.size }
    pub fn modified(&self) -> u64 { self.modified_at }
    pub fn created(&self) -> u64 { self.created_at }
}

/// `std::fs::metadata`-shaped (subset). Returns the StatX or None if
/// the path doesn't exist / stat fails. (Phase 4 G4 flagged.)
pub fn stat(path: &str) -> Option<StatX> {
    let mut out = StatX::default();
    let r = unsafe {
        syscall3(
            SYS_STATX,
            path.as_ptr() as u64,
            path.len() as u64,
            &mut out as *mut StatX as u64,
        )
    };
    if r == 0 { Some(out) } else { None }
}

/// `std::fs::metadata`-shaped Result variant — for callers using the
/// `Result<Metadata>` shape directly.
pub fn metadata(path: &str) -> io::Result<StatX> {
    stat(path).ok_or(io::Error::other())
}
