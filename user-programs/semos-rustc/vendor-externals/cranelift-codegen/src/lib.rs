//! Cranelift code generation library.
#![deny(missing_docs)]
// Display feature requirements in the documentation when building on docs.rs
#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![no_std]
// Various bits and pieces of this crate might only be used for one platform or
// another, but it's not really too useful to learn about that all the time. On
// CI we build at least one version of this crate with `--features all-arch`
// which means we'll always detect truly dead code, otherwise if this is only
// built for one platform we don't have to worry too much about trimming
// everything down.
#![cfg_attr(not(feature = "all-arch"), allow(dead_code))]
#![expect(clippy::allow_attributes_without_reason, reason = "crate not migrated")]

#[allow(unused_imports)] // #[macro_use] is required for no_std
#[macro_use]
extern crate alloc;

#[cfg(feature = "std")]
#[macro_use]
extern crate std;

#[cfg(not(feature = "std"))]
use hashbrown::{HashMap, hash_map};
#[cfg(feature = "std")]
use hashbrown::{HashMap, hash_map};

// no_std OnceLock shim — `std::sync::OnceLock` doesn't exist in no_std,
// but the isa/*/abi.rs files use it to lazily build per-arch MachineEnv
// register tables. This shim provides just-enough OnceLock to make
// `static FOO: OnceLock<MachineEnv> = OnceLock::new(); FOO.get_or_init(|| ...)`
// work in static context. Race-then-leak: if two threads enter init
// simultaneously, both produce a value but only the first stick — the
// later loser's value is dropped (codegen tables are tiny + readonly
// so the leak doesn't matter).
pub(crate) mod once {
    use core::cell::UnsafeCell;
    use core::mem::MaybeUninit;
    use core::sync::atomic::{AtomicU8, Ordering};

    /// Minimal `std::sync::OnceLock` substitute for no_std.
    pub struct OnceLock<T> {
        state: AtomicU8,
        value: UnsafeCell<MaybeUninit<T>>,
    }

    // Safe because all access goes through `get_or_init`, which uses
    // atomic state to serialize the write and prevents reads before
    // the write completes.
    unsafe impl<T: Send + Sync> Sync for OnceLock<T> {}
    unsafe impl<T: Send> Send for OnceLock<T> {}

    const UNINIT: u8 = 0;
    const INIT_LOCK: u8 = 1;
    const READY: u8 = 2;

    impl<T> OnceLock<T> {
        pub const fn new() -> Self {
            Self {
                state: AtomicU8::new(UNINIT),
                value: UnsafeCell::new(MaybeUninit::uninit()),
            }
        }

        pub fn get_or_init<F: FnOnce() -> T>(&self, init: F) -> &T {
            loop {
                match self.state.load(Ordering::Acquire) {
                    READY => return unsafe { (*self.value.get()).assume_init_ref() },
                    UNINIT => {
                        if self.state.compare_exchange(
                            UNINIT, INIT_LOCK, Ordering::Acquire, Ordering::Acquire
                        ).is_ok() {
                            let value = init();
                            unsafe { (*self.value.get()).write(value); }
                            self.state.store(READY, Ordering::Release);
                            return unsafe { (*self.value.get()).assume_init_ref() };
                        }
                    }
                    _ => core::hint::spin_loop(),
                }
            }
        }
    }
}

// Stage G iter 6: no_std float ops shim. Used by ir/immediates.rs +
// isa/s390x/lower/isle.rs + isa/riscv64/inst/args.rs.
pub(crate) mod no_std_float;

pub use crate::context::Context;
pub use crate::value_label::{LabelValueLoc, ValueLabelsRanges, ValueLocRange};
pub use crate::verifier::verify_function;
pub use crate::write::write_function;

pub use cranelift_bforest as bforest;
pub use cranelift_bitset as bitset;
pub use cranelift_control as control;
pub use cranelift_entity as entity;
#[cfg(feature = "unwind")]
pub use gimli;

// Pull in generated the `isle_numerics_methods` macro.
include!(concat!(env!("ISLE_DIR"), "/isle_numerics.rs"));

#[macro_use]
mod machinst;

pub mod binemit;
pub mod cfg_printer;
pub mod cursor;
pub mod data_value;
pub mod dbg;
pub mod dominator_tree;
pub mod flowgraph;
pub mod ir;
pub mod isa;
pub mod loop_analysis;
pub mod print_errors;
pub mod settings;
pub mod timing;
pub mod traversals;
pub mod verifier;
pub mod write;

pub use crate::entity::packed_option;
pub use crate::machinst::buffer::{
    FinalizedMachCallSite, FinalizedMachExceptionHandler, FinalizedMachReloc,
    FinalizedRelocTarget, MachCallSite, MachSrcLoc, MachTextSectionBuilder, MachTrap,
    OpenPatchRegion, PatchRegion,
};
pub use crate::machinst::{
    CallInfo, CompiledCode, Final, MachBuffer, MachBufferFinalized, MachInst, MachInstEmit,
    MachInstEmitState, MachLabel, RealReg, Reg, RelocDistance, TextSectionBuilder, VCodeConstant,
    VCodeConstantData, VCodeConstants, VCodeInst, Writable,
};

mod alias_analysis;
mod constant_hash;
mod context;
mod ctxhash;
mod egraph;
mod inst_predicates;
mod isle_prelude;
mod legalizer;
mod nan_canonicalization;
mod opts;
mod ranges;
mod remove_constant_phis;
mod result;
mod scoped_hash_map;
mod take_and_replace;
mod unreachable_code;
mod value_label;

#[cfg(feature = "souper-harvest")]
mod souper_harvest;

pub use crate::result::{CodegenError, CodegenResult, CompileError};
pub use crate::take_and_replace::TakeAndReplace;

#[cfg(feature = "incremental-cache")]
pub mod incremental_cache;

/// Even when trace logging is disabled, the trace macro has a significant performance cost so we
/// disable it by default.
#[macro_export]
macro_rules! trace {
    ($($tt:tt)*) => {
        if cfg!(any(feature = "trace-log", debug_assertions)) {
            ::log::trace!($($tt)*);
        }
    };
}

/// Dynamic check for whether trace logging is enabled.
#[macro_export]
macro_rules! trace_log_enabled {
    () => {
        cfg!(any(feature = "trace-log", debug_assertions))
            && ::log::log_enabled!(::log::Level::Trace)
    };
}

include!(concat!(env!("OUT_DIR"), "/version.rs"));
