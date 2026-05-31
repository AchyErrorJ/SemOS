use alloc::borrow::Cow;

use rustc_session::Session;

// M27 §1.7: drop the external-linker subsystem on SemOS.
// `link`, `linker`, `command`, `apple` modules drive `std::process::Command`
// invocation of `ld`/`lld`/codesign/ar etc. Per M27_RUSTC_PORT_PLAN §1.7
// (R2 §2.2 + R4 surfaced), cg_clif emits ET_EXEC bytes directly, so
// semos-rustc bypasses the SSA link step. Gate these four whole modules
// at the `mod` line, same shape E2 used for `proc_macro_server` in
// rustc_expand.
#[cfg(not(target_os = "none"))]
pub mod apple;
// M27 §1.7: `archive` builds ar-format rlibs (via ar_archive_writer + tempfile).
// On SemOS, cg_clif emits ET_EXEC directly and we never produce rlibs/staticlibs,
// so the archive subsystem is unreachable. Gate at the mod line.
#[cfg(not(target_os = "none"))]
pub mod archive;
#[cfg(not(target_os = "none"))]
pub(crate) mod command;
#[cfg(not(target_os = "none"))]
pub mod link;
#[cfg(not(target_os = "none"))]
pub(crate) mod linker;
pub mod lto;
pub mod metadata;
// M27 §1.7: rpath is consumed only by back::link (gated); gate it too.
#[cfg(not(target_os = "none"))]
pub(crate) mod rpath;
pub mod symbol_export;
pub mod write;

/// The target triple depends on the deployment target, and is required to
/// enable features such as cross-language LTO, and for picking the right
/// Mach-O commands.
///
/// Certain optimizations also depend on the deployment target.
#[cfg(not(target_os = "none"))]
pub fn versioned_llvm_target(sess: &Session) -> Cow<'_, str> {
    if sess.target.is_like_darwin {
        apple::add_version_to_llvm_target(&sess.target.llvm_target, sess.apple_deployment_target())
            .into()
    } else {
        // FIXME(madsmtm): Certain other targets also include a version,
        // we might want to move that here as well.
        Cow::Borrowed(&sess.target.llvm_target)
    }
}

/// SemOS-only stub: no Apple target on SemOS.
#[cfg(target_os = "none")]
pub fn versioned_llvm_target(sess: &Session) -> Cow<'_, str> {
    Cow::Borrowed(&sess.target.llvm_target)
}
