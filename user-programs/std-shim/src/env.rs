//! `std::env`-shaped: args, var, set_var, current_dir, set_current_dir.
//!
//! M25 #51 adds `args()` — parses the SysV argc/argv layout the kernel
//! wrote at the initial stack pointer (captured by `rt::init`).

use core_alloc::string::{String, ToString};
use core_alloc::vec::Vec;
use crate::arch::{
    SYS_GET_CWD, SYS_SET_CWD, SYS_GET_ENV, SYS_SET_ENV,
    syscall2, syscall4,
};

/// Program arguments as owned Strings, including argv[0] (the program
/// name) if the spawner provided it. Mirrors `std::env::args` but
/// returns a `Vec<String>` rather than a lazy iterator.
///
/// Reads the SysV layout (`[argc][argv ptrs][NULL]...`) at the captured
/// initial stack pointer. Returns empty if `_start` never recorded it.
pub fn args() -> Vec<String> {
    let sp = crate::rt::initial_sp();
    if sp == 0 {
        return Vec::new();
    }
    unsafe {
        let argc = *(sp as *const u64) as usize;
        // Defensive cap — the kernel limits argv to 32 items, but never
        // trust a single read into a Vec capacity.
        let argc = argc.min(256);
        let argv = (sp + 8) as *const u64;
        let mut out = Vec::with_capacity(argc);
        for i in 0..argc {
            let ptr = *argv.add(i) as *const u8;
            if ptr.is_null() {
                break;
            }
            out.push(cstr_to_string(ptr));
        }
        out
    }
}

/// `std::env::args_os` — yields each arg as `OsString`. On SemOS
/// `OsString` is a `String` alias (Phase 14 UTF-8-only filesystem),
/// so this is just `args()` re-typed as an iterator.
pub fn args_os() -> core_alloc::vec::IntoIter<crate::ffi::OsString> {
    let v: Vec<crate::ffi::OsString> = args().into_iter().collect();
    v.into_iter()
}

/// Read a NUL-terminated C string into an owned String (UTF-8 lossy).
unsafe fn cstr_to_string(ptr: *const u8) -> String {
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
        if len > 4096 {
            break; // sanity bound
        }
    }
    let slice = core::slice::from_raw_parts(ptr, len);
    String::from_utf8_lossy(slice).to_string()
}

/// Read an env variable's value into `buf`. Returns the number of bytes
/// written, or 0 if the key isn't set. The shim version of
/// `std::env::var` would wrap this with String/UTF-8 validation.
pub fn get(key: &[u8], buf: &mut [u8]) -> Option<usize> {
    let n = unsafe {
        syscall4(
            SYS_GET_ENV,
            key.as_ptr() as u64,
            key.len() as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    if n == 0 || n == u64::MAX { None } else { Some(n as usize) }
}

/// Set an env variable.
pub fn set(key: &[u8], val: &[u8]) -> Result<(), ()> {
    let r = unsafe {
        syscall4(
            SYS_SET_ENV,
            key.as_ptr() as u64,
            key.len() as u64,
            val.as_ptr() as u64,
            val.len() as u64,
        )
    };
    if r == 0 { Ok(()) } else { Err(()) }
}

/// Read the current working directory into `buf`. Returns the number
/// of bytes written, or None on error.
pub fn current_dir(buf: &mut [u8]) -> Option<usize> {
    let n = unsafe {
        syscall2(SYS_GET_CWD, buf.as_mut_ptr() as u64, buf.len() as u64)
    };
    if n == u64::MAX { None } else { Some(n as usize) }
}

/// Set the current working directory.
pub fn set_current_dir(path: &[u8]) -> Result<(), ()> {
    let r = unsafe {
        syscall2(SYS_SET_CWD, path.as_ptr() as u64, path.len() as u64)
    };
    if r == 0 { Ok(()) } else { Err(()) }
}

// ---------------------------------------------------------------------
// Owned-String conveniences (now that alloc is available). These are
// the ergonomic `std::env`-shaped entry points; the byte-slice
// functions above remain for callers avoiding allocation.
// ---------------------------------------------------------------------

/// `std::env::VarError`-shaped. SemOS is UTF-8 throughout (all kernel
/// strings are valid UTF-8), so `NotUnicode` exists only for source
/// compatibility — it will never be returned. Added 2026-05-31 for M27
/// Phase 3 C3 (4+ rustc_* sites read `env::var`'s Result shape; the
/// adapter pattern is cleaner as a real enum here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarError {
    /// The key was not set in the environment.
    NotPresent,
    /// The value was not valid UTF-8 (unreachable on SemOS today; kept
    /// for source-compat with std).
    NotUnicode(crate::ffi::OsString),
}

impl core::fmt::Display for VarError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            VarError::NotPresent => f.write_str("environment variable not found"),
            VarError::NotUnicode(_) => f.write_str("environment variable was not valid unicode"),
        }
    }
}

/// `std::env::var`-shaped: returns the value of env var `key`, or
/// `Err(VarError::NotPresent)` if unset. Value capped at 512 bytes
/// (the kernel's env block size).
pub fn var(key: &str) -> Result<String, VarError> {
    let mut buf = [0u8; 512];
    match get(key.as_bytes(), &mut buf) {
        Some(n) => Ok(String::from_utf8_lossy(&buf[..n]).to_string()),
        None => Err(VarError::NotPresent),
    }
}

/// `std::env::set_var`-shaped.
pub fn set_var(key: &str, val: &str) -> Result<(), ()> {
    set(key.as_bytes(), val.as_bytes())
}

/// `std::env::var_os`-shaped. SemOS uses UTF-8 throughout so the OS-
/// string return type aliases `String` (see `ffi::OsString`). Added
/// 2026-05-30 for M27 R2 — 10+ rustc_* crates use `env::var_os` to
/// read configuration without forcing UTF-8 validation.
pub fn var_os(key: &str) -> Option<crate::ffi::OsString> {
    var(key).ok()
}

/// `std::env::vars`-shaped iterator — returns an empty iterator on
/// SemOS today because the kernel doesn't expose a "list all env
/// vars" syscall yet. Provided so callers compile against the same
/// shape they'd use with std. Replace with a real iterator when
/// SYS_LIST_ENV lands.
pub fn vars() -> core::iter::Empty<(String, String)> {
    core::iter::empty()
}

/// `std::env::vars_os`-shaped — same emptiness rationale as `vars`.
pub fn vars_os() -> core::iter::Empty<(crate::ffi::OsString, crate::ffi::OsString)> {
    core::iter::empty()
}

/// `std::env::current_dir`-shaped: returns the CWD as an owned String.
pub fn current_dir_string() -> Option<String> {
    let mut buf = [0u8; 512];
    let n = current_dir(&mut buf)?;
    Some(String::from_utf8_lossy(&buf[..n]).to_string())
}
