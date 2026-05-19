//! `std::fs`-shaped surface — read/write/stat by path.
//!
//! Minimal in-place reads and full-file overwrites against the path
//! namespace. The full `File` / `OpenOptions` / Read/Write traits land
//! in M25 Tier 2.

use crate::arch::{
    SYS_OPEN, SYS_CLOSE, SYS_FREAD, SYS_FWRITE, SYS_UNLINK, SYS_MKDIR,
    syscall1, syscall2, syscall3,
};

/// Same bit layout as kernel's `syscall::open_flags`.
pub mod open_flags {
    pub const CREATE:    u64 = 1 << 0;
    pub const DIRECTORY: u64 = 1 << 1;
    pub const TIER_SHIFT: u32 = 4;
}

/// Open a path. Returns the FD (>= 0) on success, None on error.
pub fn open(path: &str, flags: u64) -> Option<u64> {
    let r = unsafe {
        syscall3(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, flags)
    };
    if r == u64::MAX { None } else { Some(r) }
}

/// Close an FD.
pub fn close(fd: u64) {
    unsafe { let _ = syscall1(SYS_CLOSE, fd); }
}

/// Read from an FD into `buf`. Returns bytes read, 0 on EOF, None on error.
pub fn read(fd: u64, buf: &mut [u8]) -> Option<usize> {
    let r = unsafe {
        syscall3(SYS_FREAD, fd, buf.as_mut_ptr() as u64, buf.len() as u64)
    };
    if r == u64::MAX { None } else { Some(r as usize) }
}

/// Write `buf` to FD. Today the kernel does full-file overwrite;
/// returns bytes written.
pub fn write(fd: u64, buf: &[u8]) -> Option<usize> {
    let r = unsafe {
        syscall3(SYS_FWRITE, fd, buf.as_ptr() as u64, buf.len() as u64)
    };
    if r == u64::MAX { None } else { Some(r as usize) }
}

/// Create a directory at `path`.
pub fn create_dir(path: &str) -> Result<(), ()> {
    let r = unsafe {
        syscall2(SYS_MKDIR, path.as_ptr() as u64, path.len() as u64)
    };
    if r == 0 { Ok(()) } else { Err(()) }
}

/// Remove a file or empty directory.
pub fn remove(path: &str) -> Result<(), ()> {
    let r = unsafe {
        syscall2(SYS_UNLINK, path.as_ptr() as u64, path.len() as u64)
    };
    if r == 0 { Ok(()) } else { Err(()) }
}

/// Convenience: open a path, slurp its bytes into `buf`, close. Returns
/// the number of bytes read (or None on error). Caller picks the
/// buffer size — the std-shim doesn't ship `Vec` yet.
pub fn read_into(path: &str, buf: &mut [u8]) -> Option<usize> {
    let fd = open(path, 0)?;
    let n = read(fd, buf);
    close(fd);
    n
}

/// Convenience: open with CREATE, write entire buffer, close.
pub fn write_all(path: &str, content: &[u8]) -> Result<usize, ()> {
    let fd = open(path, open_flags::CREATE).ok_or(())?;
    let n = write(fd, content).ok_or(())?;
    close(fd);
    Ok(n)
}
