// M27 Phase 2a A1 + R4 B3: stack growth.
// Upstream uses `stacker::maybe_grow` to allocate a fresh stack chunk
// when recursive descent (parsing, trait selection, MIR build) gets
// close to the red zone. Option B in the recon's blocker write-up:
// stub to no-op + bump the user stack at the kernel level.
//
// Host build path is unchanged.

#[cfg(not(target_os = "none"))]
const RED_ZONE: usize = 100 * 1024; // 100k

#[cfg(not(target_os = "none"))]
#[cfg(not(target_os = "aix"))]
const STACK_PER_RECURSION: usize = 1024 * 1024; // 1MB
#[cfg(not(target_os = "none"))]
#[cfg(target_os = "aix")]
const STACK_PER_RECURSION: usize = 16 * 1024 * 1024; // 16MB

/// Grows the stack on demand to prevent stack overflow. Call this in strategic locations
/// to "break up" recursive calls. E.g. almost any call to `visit_expr` or equivalent can benefit
/// from this.
///
/// Should not be sprinkled around carelessly, as it causes a little bit of overhead.
#[inline]
#[cfg(not(target_os = "none"))]
pub fn ensure_sufficient_stack<R>(f: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(RED_ZONE, STACK_PER_RECURSION, f)
}

/// M27 R4 B3 — SemOS-target no-op shim for stacker. Risk: deeply
/// recursive callers (rustc_trait_selection's normalize, MIR build,
/// parser) may overflow the user stack. Mitigation: rely on the
/// kernel-side USER_PROC_STACK_SIZE bump (already at 1 MiB; may need
/// to go higher for hello-world). Real fix is vendoring psm + a
/// SemOS x86_64 backend (Option A), tracked for follow-up.
#[inline]
#[cfg(target_os = "none")]
pub fn ensure_sufficient_stack<R>(f: impl FnOnce() -> R) -> R {
    f()
}
