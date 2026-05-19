//! `std::env`-shaped: var, set_var, current_dir, set_current_dir.
//!
//! args() isn't done yet — argv arrives via SysV ABI at the user
//! stack on spawn but the std-shim hasn't wired the argc/argv parser
//! that splits it back into &[&str]. Tracked as M25 Tier 2 follow-up.

use crate::arch::{
    SYS_GET_CWD, SYS_SET_CWD, SYS_GET_ENV, SYS_SET_ENV,
    syscall2, syscall4,
};

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
