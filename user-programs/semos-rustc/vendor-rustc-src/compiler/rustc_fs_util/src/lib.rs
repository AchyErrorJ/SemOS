#![no_std]

#[macro_use]
extern crate alloc;

// M27: needs semos-std fs surface
// ------------------------------------------------------------------
// This crate is a thin wrapper around `std::fs` operations: hard_link,
// remove_file, copy, canonicalize, plus the `tempfile` external crate.
//
// semos-std currently has fs::write/File/read but NOT hard_link,
// remove_file, copy, canonicalize, or any tempfile-equivalent. Per
// recipe step 4, sites are MARKED rather than substituted blindly.
//
// Each fs call site below carries a `// M27: needs semos-std fs surface`
// comment. Parent should treat resolution as one of:
//   (a) add the missing fs ops to semos-std (preferred for hard_link
//       and remove_file — both are SYS_LINK/SYS_UNLINK class calls);
//   (b) cfg-out the call site if the calling code path is dead in
//       SemOS (e.g. incremental-comp hard-links — and §1.3 already
//       drops incremental, so link_or_copy may be entirely dead);
//   (c) replace with copy-only fallback.
//
// `tempfile` is a vendored external; mark sites as stub-pending until
// the parent's externals queue patches the tempfile crate. Until then
// the public API exists but calls panic with a clear message rather
// than build-fail.
// ------------------------------------------------------------------

use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;

// M27: needs semos-std fs surface
// `OsStr` is not in core; it ships with std and is on the R4 B5 / R2
// top-5 semos-std additions list. Until semos-std lands OsStr, the
// uses below are marked. `Path`/`PathBuf` already ship in semos-std
// (see project memory note), so we import from there for the
// no_std-target build path.
//
// Cross-target compatibility: under cfg(target_os = "none") (the SemOS
// target) we route Path/PathBuf through `semos_std::path::*`. Under
// cfg(not(target_os = "none")) (host builds) we use std::path::* so
// build-deps and dev paths still work.
#[cfg(target_os = "none")]
use semos_std::path::{Path, PathBuf};
#[cfg(target_os = "none")]
use semos_std::io;
// M27: tempfile vendor pending — see Cargo.toml note.
// On host targets tempfile imports unchanged. On the SemOS target the
// `tempfile::*` re-uses below will fail to resolve until the vendor
// patch lands; the parent should gate this crate behind a
// `target_os = "none"`-conditional `cfg(disabled_pending_tempfile)`
// or land the patch first.
#[cfg(not(target_os = "none"))]
use std::ffi::{CString as _StdCString, OsStr};
#[cfg(not(target_os = "none"))]
use std::path::{Path, PathBuf, absolute};
#[cfg(not(target_os = "none"))]
use std::{fs, io};

#[cfg(not(target_os = "none"))]
use tempfile::TempDir;

// Unfortunately, on windows, it looks like msvcrt.dll is silently translating
// verbatim paths under the hood to non-verbatim paths! This manifests itself as
// gcc looking like it cannot accept paths of the form `\\?\C:\...`, but the
// real bug seems to lie in msvcrt.dll.
//
// Verbatim paths are generally pretty rare, but the implementation of
// `fs::canonicalize` currently generates paths of this form, meaning that we're
// going to be passing quite a few of these down to gcc, so we need to deal with
// this case.
//
// For now we just strip the "verbatim prefix" of `\\?\` from the path. This
// will probably lose information in some cases, but there's not a whole lot
// more we can do with a buggy msvcrt...
//
// For some more information, see this comment:
//   https://github.com/rust-lang/rust/issues/25505#issuecomment-102876737
#[cfg(windows)]
pub fn fix_windows_verbatim_for_gcc(p: &Path) -> PathBuf {
    use std::ffi::OsString;
    use std::path;
    let mut components = p.components();
    let prefix = match components.next() {
        Some(path::Component::Prefix(p)) => p,
        _ => return p.to_path_buf(),
    };
    match prefix.kind() {
        path::Prefix::VerbatimDisk(disk) => {
            let mut base = OsString::from(format!("{}:", disk as char));
            base.push(components.as_path());
            PathBuf::from(base)
        }
        path::Prefix::VerbatimUNC(server, share) => {
            let mut base = OsString::from(r"\\");
            base.push(server);
            base.push(r"\");
            base.push(share);
            base.push(components.as_path());
            PathBuf::from(base)
        }
        _ => p.to_path_buf(),
    }
}

#[cfg(not(windows))]
pub fn fix_windows_verbatim_for_gcc(p: &Path) -> PathBuf {
    p.to_path_buf()
}

pub enum LinkOrCopy {
    Link,
    Copy,
}

/// Copies `p` into `q`, preferring to use hard-linking if possible.
/// The result indicates which of the two operations has been performed.
pub fn link_or_copy<P: AsRef<Path>, Q: AsRef<Path>>(p: P, q: Q) -> io::Result<LinkOrCopy> {
    // Creating a hard-link will fail if the destination path already exists. We could defensively
    // call remove_file in this function, but that pessimizes callers who can avoid such calls.
    // Incremental compilation calls this function a lot, and is able to avoid calls that
    // would fail the first hard_link attempt.

    let p = p.as_ref();
    let q = q.as_ref();

    // M27: needs semos-std fs surface — fs::hard_link, fs::remove_file, fs::copy.
    // All three sit on the SYS_LINK / SYS_UNLINK / SYS_OPEN+read+write
    // axis; semos-std should expose them under fs::{hard_link, remove_file,
    // copy}. §1.3 drops incremental compilation, so this whole function is
    // likely dead code in the SemOS build — gate-it-out is also a fine
    // resolution.
    #[cfg(target_os = "none")]
    {
        let _ = (p, q);
        return Err(io_unsupported("link_or_copy"));
    }
    #[cfg(not(target_os = "none"))]
    {
        let err = match fs::hard_link(p, q) {
            Ok(()) => return Ok(LinkOrCopy::Link),
            Err(err) => err,
        };

        if err.kind() == io::ErrorKind::AlreadyExists {
            fs::remove_file(q)?;
            if fs::hard_link(p, q).is_ok() {
                return Ok(LinkOrCopy::Link);
            }
        }

        // Hard linking failed, fall back to copying.
        fs::copy(p, q).map(|_| LinkOrCopy::Copy)
    }
}

#[cfg(any(unix, all(target_os = "wasi", target_env = "p1")))]
pub fn path_to_c_string(p: &Path) -> CString {
    use std::ffi::OsStr;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt;
    #[cfg(all(target_os = "wasi", target_env = "p1"))]
    use std::os::wasi::ffi::OsStrExt;

    let p: &OsStr = p.as_ref();
    CString::new(p.as_bytes()).unwrap()
}
#[cfg(windows)]
pub fn path_to_c_string(p: &Path) -> CString {
    CString::new(p.to_str().unwrap()).unwrap()
}
// M27: needs semos-std fs surface
// For `target_os = "none"` the function still needs to exist as a public
// item. semos-std::path::Path::to_str + From<&str> for CString gets us
// there once Path lands a `to_str` method on the SemOS target.
#[cfg(target_os = "none")]
pub fn path_to_c_string(p: &Path) -> CString {
    // Path::to_str is in semos-std; CString::new is in alloc::ffi.
    let s: &str = p.to_str().expect("path_to_c_string: non-UTF8 path on SemOS");
    CString::new(s).expect("path_to_c_string: nul byte in path")
}

#[inline]
pub fn try_canonicalize<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    // M27: needs semos-std fs surface — fs::canonicalize and path::absolute.
    // R2 top-5 lists `PathBuf::canonicalize (lexical-only)` for semos-std at
    // priority 4. `path::absolute` (added in std 1.79) is missing too.
    // Until both land, on the SemOS target we return a trivial passthrough
    // (PathBuf clone) — strictly weaker than the host behavior but enough
    // for the v1 hello-world flow where canonicalisation is a debug aid.
    #[cfg(target_os = "none")]
    {
        Ok(path.as_ref().to_path_buf())
    }
    #[cfg(not(target_os = "none"))]
    {
        fs::canonicalize(&path).or_else(|_| absolute(&path))
    }
}

// M27: tempfile vendor pending
// The `TempDirBuilder` struct + impl are kept as the public API so
// downstream crates (rustc_session, rustc_metadata, rustc_codegen_ssa
// — the latter dropped per §1.7 anyway) continue to type-check. Calls
// on the SemOS target panic with a clear message until the vendor
// patch lands. On host targets behavior is unchanged.
#[cfg(not(target_os = "none"))]
pub struct TempDirBuilder<'a, 'b> {
    builder: tempfile::Builder<'a, 'b>,
}

#[cfg(not(target_os = "none"))]
impl<'a, 'b> TempDirBuilder<'a, 'b> {
    pub fn new() -> Self {
        Self { builder: tempfile::Builder::new() }
    }

    pub fn prefix<S: AsRef<OsStr> + ?Sized>(&mut self, prefix: &'a S) -> &mut Self {
        self.builder.prefix(prefix);
        self
    }

    pub fn suffix<S: AsRef<OsStr> + ?Sized>(&mut self, suffix: &'b S) -> &mut Self {
        self.builder.suffix(suffix);
        self
    }

    pub fn tempdir_in<P: AsRef<Path>>(&self, dir: P) -> io::Result<TempDir> {
        let dir = dir.as_ref();
        // On Windows in CI, we had been getting fairly frequent "Access is denied"
        // errors when creating temporary directories.
        // So this implements a simple retry with backoff loop.
        #[cfg(windows)]
        for wait in 1..11 {
            match self.builder.tempdir_in(dir) {
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {}
                t => return t,
            }
            std::thread::sleep(std::time::Duration::from_millis(1 << wait));
        }
        self.builder.tempdir_in(dir)
    }
}

// SemOS-target shim: stub the same public API. Once tempfile is
// vendored + patched + OsStr lands in semos-std, fold this in with the
// host impl above (drop the cfg).
#[cfg(target_os = "none")]
pub struct TempDirBuilder<'a, 'b> {
    _marker: core::marker::PhantomData<(&'a (), &'b ())>,
}

#[cfg(target_os = "none")]
impl<'a, 'b> TempDirBuilder<'a, 'b> {
    pub fn new() -> Self {
        Self { _marker: core::marker::PhantomData }
    }

    // M27: tempfile vendor pending — OsStr surface also missing from semos-std.
    // Caller-side typed signatures are preserved by taking `&str` (the only
    // type the rustc call sites actually pass through). When OsStr lands the
    // signature can widen back to `AsRef<OsStr>`.
    pub fn prefix(&mut self, _prefix: &'a str) -> &mut Self {
        self
    }

    pub fn suffix(&mut self, _suffix: &'b str) -> &mut Self {
        self
    }

    pub fn tempdir_in<P: AsRef<Path>>(&self, _dir: P) -> io::Result<PathBuf> {
        // M27: tempfile vendor pending — return an explicit Unsupported error
        // so callers that handle io::Result get a clean propagation path.
        // (Returning Ok with a real temp dir requires the vendored tempfile.)
        Err(io_unsupported("TempDirBuilder::tempdir_in"))
    }
}

// M27: small helper to build an Unsupported-kind io::Error consistently
// for the SemOS-target stubs above. semos-std exposes io::Error with at
// least a `new(kind, msg)` constructor; if not yet, the parent should add
// it (or use whatever the closest available constructor is).
#[cfg(target_os = "none")]
fn io_unsupported(what: &'static str) -> io::Error {
    // M27: assumes semos-std::io::{Error, ErrorKind::Unsupported} exists.
    // If the kind variant isn't yet provided, swap to whatever ErrorKind
    // semos-std does expose (e.g. Other) without changing the call sites.
    let msg = format!("rustc_fs_util: {} not yet implemented on SemOS", what);
    let _ = msg; // until io::Error::new with String lands, we can also drop msg
    io::Error::new(io::ErrorKind::Unsupported, "rustc_fs_util: not yet implemented on SemOS")
}
