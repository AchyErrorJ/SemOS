//! `std::ffi`-shaped minimal aliases — added 2026-05-30 for M27 R4 B5.
//!
//! SemOS has no concept of "platform-native string encoding distinct
//! from UTF-8" — everything is UTF-8 because the kernel's string APIs
//! all are. So `OsString` is `String` and `OsStr` is `str`, and the
//! conversion methods are trivial.
//!
//! This is enough for the dozen rustc_* crates that use OsString as an
//! opaque byte container (path components, env-var values). Code that
//! genuinely needs non-UTF-8 paths won't work on SemOS today, but
//! neither does the kernel itself today.

use core_alloc::string::String;

/// `std::ffi::OsString` shim — on SemOS, identical to `String`.
pub type OsString = String;

/// `std::ffi::OsStr` shim — on SemOS, identical to `str`.
pub type OsStr = str;

/// Convenience for `String::from(&str)` when callers want the explicit
/// `OsString::from(...)` shape rustc uses.
pub trait OsStrExt {
    fn to_os_string(&self) -> OsString;
}

impl OsStrExt for str {
    fn to_os_string(&self) -> OsString {
        OsString::from(self)
    }
}
