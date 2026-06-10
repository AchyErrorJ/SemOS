//! Various data structures used by the Rust compiler. The intention
//! is that code in here should not be *specific* to rustc, so that
//! it can be easily unit tested and so forth.
//!
//! # Note
//!
//! This API is completely unstable and subject to change.

// tidy-alphabetical-start
#![allow(internal_features)]
#![allow(rustc::default_hash_types)]
#![allow(rustc::potential_query_instability)]
#![feature(array_windows)]
#![deny(unsafe_op_in_unsafe_fn)]
#![feature(allocator_api)]
#![feature(ascii_char)]
#![feature(ascii_char_variants)]
#![feature(assert_matches)]
#![feature(auto_traits)]
#![feature(cfg_select)]
#![feature(const_default)]
#![feature(const_trait_impl)]
#![feature(core_intrinsics)]
#![feature(dropck_eyepatch)]
#![feature(extend_one)]
#![cfg_attr(not(target_os = "none"), feature(file_buffered))]
#![feature(map_try_insert)]
#![feature(min_specialization)]
#![feature(negative_impls)]
#![feature(never_type)]
#![feature(ptr_alignment_type)]
#![feature(rustc_attrs)]
#![feature(sized_hierarchy)]
#![feature(test)]
#![cfg_attr(not(target_os = "none"), feature(thread_id_value))]
#![feature(trusted_len)]
#![feature(type_alias_impl_trait)]
#![feature(unwrap_infallible)]
// tidy-alphabetical-end

// M27 Phase 2a A1: no_std + alloc. Host build paths still need `std`
// for parking_lot/jobserver/measureme/tempfile/stacker/memmap2 (host
// targets only — see cfg(not(target_os = "none")) gates in the
// affected modules). On the SemOS target (`x86_64-unknown-none`) the
// crate is fully no_std + alloc.
#![cfg_attr(target_os = "none", no_std)]

#[macro_use]
extern crate alloc;

// On host builds we still need std for the modules that aren't gated.
#[cfg(not(target_os = "none"))]
extern crate std;

// Temporarily re-export `assert_matches!`, so that the rest of the compiler doesn't
// have to worry about it being moved to a different module in std during stabilization.
// FIXME(#151359): Remove this when `feature(assert_matches)` is stable in stage0.
// (This doesn't necessarily need to be fixed during the beta bump itself.)
//
// M27 R4 B2: on SemOS target there is no `std::assert_matches`; pull
// from core (stable since 1.82 via assert_matches feature gate).
#[cfg(not(target_os = "none"))]
pub use std::assert_matches::{assert_matches, debug_assert_matches};
// M27 fix: nightly-2026-02-01 does NOT have `core::assert_matches`.
// Provide local macros that delegate to `assert!(matches!(...))`.
#[cfg(target_os = "none")]
mod assert_matches_stub {
    /// Assert that an expression matches a pattern (simplified stub).
    #[macro_export]
    macro_rules! assert_matches {
        // With custom message / format args
        ($e:expr, $p:pat $(if $g:expr)?, $($fmt:tt)+) => {
            match $e {
                $p $(if $g)? => {},
                ref __semos_val => {
                    panic!($($fmt)+);
                }
            }
        };
        // Without custom message
        ($e:expr, $p:pat $(if $g:expr)? $(,)?) => {
            match $e {
                $p $(if $g)? => {},
                ref __semos_val => {
                    panic!("assertion failed: {:?} does not match {}", __semos_val, stringify!($p));
                }
            }
        };
    }
    /// Debug-only variant of `assert_matches`.
    #[macro_export]
    macro_rules! debug_assert_matches {
        ($e:expr, $p:pat $(if $g:expr)?, $($fmt:tt)+) => {
            if cfg!(debug_assertions) {
                $crate::assert_matches!($e, $p $(if $g)?, $($fmt)+);
            }
        };
        ($e:expr, $p:pat $(if $g:expr)? $(,)?) => {
            if cfg!(debug_assertions) {
                $crate::assert_matches!($e, $p $(if $g)?);
            }
        };
    }
}

use core::fmt;

pub use atomic_ref::AtomicRef;
// M27 Phase 5b Stage F: ena vendored + no_std-patched at
// vendor-externals/ena (drops the `log` dep, blanket `#![no_std]`).
// Available on both targets; rustc_infer consumes these re-exports.
pub use ena::{snapshot_vec, undo_log, unify};
pub use rustc_index::static_assert_size;
// Re-export some data-structure crates which are part of our public API.
pub use {either, indexmap, smallvec, thin_vec};

pub mod aligned;
pub mod base_n;
pub mod binary_search_util;
pub mod fingerprint;
pub mod flat_map_in_place;
pub mod flock;
pub mod frozen;
pub mod fx;
pub mod graph;
pub mod intern;
pub mod jobserver;
pub mod marker;
pub mod memmap;
pub mod obligation_forest;
pub mod owned_slice;
pub mod packed;
pub mod profiling;
pub mod sharded;
pub mod small_c_str;
// M27 Phase 5b Stage F: ena now vendored no_std, so snapshot_map
// (which builds on ena's snapshot_vec) is available on both targets.
pub mod snapshot_map;
pub mod sorted_map;
pub mod sso;
pub mod stable_hasher;
pub mod stack;
pub mod steal;
pub mod svh;
pub mod sync;
pub mod tagged_ptr;
pub mod temp_dir;
pub mod thinvec;
pub mod thousands;
pub mod transitive_relation;
pub mod unhash;
pub mod union_find;
pub mod unord;
pub mod vec_cache;
pub mod work_queue;

mod atomic_ref;

/// This calls the passed function while ensuring it won't be inlined into the caller.
#[inline(never)]
#[cold]
pub fn outline<F: FnOnce() -> R, R>(f: F) -> R {
    f()
}

/// Returns a structure that calls `f` when dropped.
pub fn defer<F: FnOnce()>(f: F) -> OnDrop<F> {
    OnDrop(Some(f))
}

pub struct OnDrop<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> OnDrop<F> {
    /// Disables on-drop call.
    #[inline]
    pub fn disable(mut self) {
        self.0.take();
    }
}

impl<F: FnOnce()> Drop for OnDrop<F> {
    #[inline]
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

/// This is a marker for a fatal compiler error used with `resume_unwind`.
///
/// M27 R4 B1: on SemOS this marker is still recognized by
/// `rustc_span::fatal_error`'s catch_fatal_errors shim, which on the
/// `target_os = "none"` build returns `Ok(f())` and aborts on raise.
pub struct FatalErrorMarker;

/// Turns a closure that takes an `&mut Formatter` into something that can be display-formatted.
pub fn make_display(f: impl Fn(&mut fmt::Formatter<'_>) -> fmt::Result) -> impl fmt::Display {
    struct Printer<F> {
        f: F,
    }
    impl<F> fmt::Display for Printer<F>
    where
        F: Fn(&mut fmt::Formatter<'_>) -> fmt::Result,
    {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            (self.f)(fmt)
        }
    }

    Printer { f }
}

// See comment in compiler/rustc_middle/src/tests.rs and issue #27438.
#[doc(hidden)]
pub fn __noop_fix_for_windows_dllimport_issue() {}

/// `external_bitflags_debug!` — emits a `Debug` impl for an externally
/// declared bitflags type. The emitted tokens reference
/// `::core::fmt::*` so the macro works in downstream no_std crates.
#[macro_export]
macro_rules! external_bitflags_debug {
    ($Name:ident) => {
        impl ::core::fmt::Debug for $Name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::bitflags::parser::to_writer(self, f)
            }
        }
    };
}
