// Copyright 2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! An implementation of union-find. See the `unify` module for more
//! details.
//!
//! M27 SemOS fork: `no_std` + drop the `log` dependency (debug!() calls
//! are replaced with a no-op shim). Single-thread + no-host-IO is fine
//! for unification; ena's algorithm is purely in-memory.

#![cfg_attr(feature = "bench", feature(test))]
#![no_std]

extern crate alloc;

// Local no-op replacement for the `log` crate's `debug!` macro. We drop
// the upstream `log` dep entirely — the `log` crate uses `std::sync` /
// `std::error` for its global logger, none of which is available on the
// SemOS target. Unification correctness doesn't depend on any of these
// debug traces firing.
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => { () };
}

#[cfg(feature = "persistent")]
extern crate dogged;

pub mod snapshot_vec;
pub mod undo_log;
pub mod unify;
