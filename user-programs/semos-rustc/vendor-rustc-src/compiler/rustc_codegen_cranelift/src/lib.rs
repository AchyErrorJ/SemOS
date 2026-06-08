// M27 Phase 5c Stage G iter 7: no_std port for x86_64-unknown-none.
// cg_clif itself runs target-side inside semos-rustc.
#![cfg_attr(target_os = "none", no_std)]

// tidy-alphabetical-start
// Note: please avoid adding other feature gates where possible
#![feature(rustc_private)]
// Only used to define intrinsics in `compiler_builtins.rs`.
#![feature(f16)]
#![feature(f128)]
// Note: please avoid adding other feature gates where possible
#![warn(rust_2018_idioms)]
#![warn(unreachable_pub)]
#![warn(unused_lifetimes)]
// tidy-alphabetical-end

// Stage G iter 7: macro_use pulls vec! / format! / write! into scope so
// downstream modules don't each need to import them.
#[macro_use]
extern crate alloc;

#[cfg(not(target_os = "none"))]
extern crate std;

#[macro_use]
extern crate rustc_middle;
extern crate rustc_abi;
extern crate rustc_ast;
extern crate rustc_codegen_ssa;
extern crate rustc_const_eval;
extern crate rustc_data_structures;
extern crate rustc_errors;
extern crate rustc_fs_util;
extern crate rustc_hir;
extern crate rustc_incremental;
extern crate rustc_index;
extern crate rustc_log;
extern crate rustc_metadata;
extern crate rustc_session;
extern crate rustc_span;
extern crate rustc_symbol_mangling;
extern crate rustc_target;

// M27 Phase 5c Stage G iter 7: rustc_driver dropped — upstream's
// comment said it prevents host-rustc duplication. Phase 5b wires it
// at the semos-rustc binary level (and only if needed).
// #[allow(unused_extern_crates)]
// extern crate rustc_driver;

use core::any::Any;
use core::cell::OnceCell;
#[cfg(not(target_os = "none"))]
use std::env;
use alloc::sync::Arc;

use cranelift_codegen::isa::TargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use rustc_codegen_ssa::traits::CodegenBackend;
use rustc_codegen_ssa::{CodegenResults, TargetConfig};
// Stage G iter 8: rustc_log re-exports `tracing` host-only; target side
// gets no-op info!/debug!/trace! macros from a local stub.
#[cfg(not(target_os = "none"))]
use rustc_log::tracing::info;
#[cfg(target_os = "none")]
macro_rules! info { ($($tt:tt)*) => {} }
use rustc_middle::dep_graph::{WorkProduct, WorkProductId};
use rustc_session::Session;
use rustc_session::config::OutputFilenames;
use rustc_span::{Symbol, sym};
use rustc_target::spec::{Abi, Arch, Env, Os};

pub use crate::config::*;
use crate::prelude::*;

mod abi;
mod allocator;
mod analyze;
mod base;
mod cast;
mod codegen_f16_f128;
mod codegen_i128;
mod common;
mod compiler_builtins;
// Stage G iter 8: ConcurrencyLimiter is a jobserver-helper for parallel
// codegen — only used by driver::aot (also host-only) and depends on
// std::sync::Mutex's .lock().unwrap() and HelperThread which semos_std
// doesn't provide.
#[cfg(not(target_os = "none"))]
mod concurrency_limiter;
mod config;
mod constant;
mod debuginfo;
mod discriminant;
mod driver;
// Stage G iter 8: global_asm + toolchain invoke external assembler/linker
// binaries via std::process::Command — host-only. Target-side SemOS
// builds skip them; cg_clif emits ELFs directly via cranelift-object.
#[cfg(not(target_os = "none"))]
mod global_asm;
mod inline_asm;
mod intrinsics;
mod linkage;
mod main_shim;
mod num;
mod optimize;
mod pointer;
mod pretty_clif;
#[cfg(not(target_os = "none"))]
mod toolchain;
mod unsize;
mod unwind_module;
mod value_and_place;
mod vtable;

mod prelude {
    // M27 Phase 5c Stage G iter 7: alloc prelude reexports for no_std mode.
    // Files that `use crate::prelude::*` get Vec/String/Box/BTreeMap/Arc
    // without per-file alloc imports.
    pub(crate) use alloc::boxed::Box;
    pub(crate) use alloc::collections::BTreeMap;
    pub(crate) use alloc::string::{String, ToString};
    pub(crate) use alloc::sync::Arc;
    pub(crate) use alloc::vec::Vec;

    pub(crate) use cranelift_codegen::Context;
    pub(crate) use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
    pub(crate) use cranelift_codegen::ir::function::Function;
    pub(crate) use cranelift_codegen::ir::{
        AbiParam, Block, FuncRef, Inst, InstBuilder, MemFlags, Signature, SourceLoc, StackSlot,
        StackSlotData, StackSlotKind, TrapCode, Type, Value, types,
    };
    pub(crate) use cranelift_module::{self, DataDescription, FuncId, Linkage, Module};
    pub(crate) use rustc_abi::{BackendRepr, FIRST_VARIANT, FieldIdx, Scalar, Size, VariantIdx};
    pub(crate) use rustc_data_structures::fx::{FxHashMap, FxIndexMap};
    pub(crate) use rustc_hir::def_id::{DefId, LOCAL_CRATE};
    pub(crate) use rustc_index::Idx;
    pub(crate) use rustc_middle::mir::{self, *};
    pub(crate) use rustc_middle::ty::layout::{LayoutOf, TyAndLayout};
    pub(crate) use rustc_middle::ty::{
        self, FloatTy, Instance, InstanceKind, IntTy, Ty, TyCtxt, UintTy,
    };
    pub(crate) use rustc_span::Span;

    pub(crate) use crate::abi::*;
    pub(crate) use crate::base::{codegen_operand, codegen_place};
    pub(crate) use crate::cast::*;
    pub(crate) use crate::common::*;
    pub(crate) use crate::debuginfo::{DebugContext, UnwindContext};
    pub(crate) use crate::pointer::Pointer;
    pub(crate) use crate::value_and_place::{CPlace, CValue};
}

struct PrintOnPanic<F: Fn() -> String>(F);
impl<F: Fn() -> String> Drop for PrintOnPanic<F> {
    fn drop(&mut self) {
        // Stage G iter 8: thread::panicking() + println! host-only.
        #[cfg(not(target_os = "none"))]
        if ::std::thread::panicking() {
            println!("{}", (self.0)());
        }
        #[cfg(target_os = "none")]
        let _ = &self.0;
    }
}

pub struct CraneliftCodegenBackend {
    pub config: OnceCell<BackendConfig>,
}

impl CodegenBackend for CraneliftCodegenBackend {
    fn locale_resource(&self) -> &'static str {
        // FIXME(rust-lang/rust#100717) - cranelift codegen backend is not yet translated
        ""
    }

    fn name(&self) -> &'static str {
        "cranelift"
    }

    fn init(&self, sess: &Session) {
        use rustc_session::config::{InstrumentCoverage, Lto};
        match sess.lto() {
            Lto::No | Lto::ThinLocal => {}
            Lto::Thin | Lto::Fat => {
                sess.dcx().warn("LTO is not supported. You may get a linker error.")
            }
        }

        if sess.opts.cg.instrument_coverage() != InstrumentCoverage::No {
            sess.dcx()
                .fatal("`-Cinstrument-coverage` is LLVM specific and not supported by Cranelift");
        }

        let config = self.config.get_or_init(|| {
            BackendConfig::from_opts(&sess.opts.cg.llvm_args)
                .unwrap_or_else(|err| sess.dcx().fatal(err))
        });

        if config.jit_mode && !sess.opts.output_types.should_codegen() {
            sess.dcx().fatal("JIT mode doesn't work with `cargo check`");
        }
    }

    fn target_config(&self, sess: &Session) -> TargetConfig {
        // FIXME return the actually used target features. this is necessary for #[cfg(target_feature)]
        let target_features = match sess.target.arch {
            Arch::X86_64 if sess.target.os != Os::None => {
                // x86_64 mandates SSE2 support and rustc requires the x87 feature to be enabled
                vec![sym::fxsr, sym::sse, sym::sse2, Symbol::intern("x87")]
            }
            Arch::AArch64 => match &sess.target.os {
                Os::None => vec![],
                // On macOS the aes, sha2 and sha3 features are enabled by default and ring
                // fails to compile on macOS when they are not present.
                Os::MacOs => vec![sym::neon, sym::aes, sym::sha2, sym::sha3],
                // AArch64 mandates Neon support
                _ => vec![sym::neon],
            },
            _ => vec![],
        };
        // FIXME do `unstable_target_features` properly
        let unstable_target_features = target_features.clone();

        // FIXME(f16_f128): `rustc_codegen_llvm` currently disables support on Windows GNU
        // targets due to GCC using a different ABI than LLVM. Therefore `f16` and `f128`
        // won't be available when using a LLVM-built sysroot.
        let has_reliable_f16_f128 = !(sess.target.arch == Arch::X86_64
            && sess.target.os == Os::Windows
            && sess.target.env == Env::Gnu
            && sess.target.abi != Abi::Llvm);

        TargetConfig {
            target_features,
            unstable_target_features,
            // `rustc_codegen_cranelift` polyfills functionality not yet
            // available in Cranelift.
            has_reliable_f16: has_reliable_f16_f128,
            has_reliable_f16_math: has_reliable_f16_f128,
            has_reliable_f128: has_reliable_f16_f128,
            has_reliable_f128_math: has_reliable_f16_f128,
        }
    }

    fn print_version(&self) {
        // Stage G iter 8: println! host-only; SemOS target has no stdout
        // here (semos-rustc owns its own output).
        #[cfg(not(target_os = "none"))]
        println!("Cranelift version: {}", cranelift_codegen::VERSION);
    }

    fn codegen_crate(&self, tcx: TyCtxt<'_>) -> Box<dyn Any> {
        info!("codegen crate {}", tcx.crate_name(LOCAL_CRATE));
        let config = self.config.get().unwrap();
        if config.jit_mode {
            #[cfg(feature = "jit")]
            driver::jit::run_jit(tcx, config.jit_args.clone());

            #[cfg(not(feature = "jit"))]
            tcx.dcx().fatal("jit support was disabled when compiling rustc_codegen_cranelift");
        } else {
            // Stage G iter 8: driver::aot is host-only.
            // Phase 5b iter 3: target side calls the SemOS-native
            // aot_semos::run_aot stub. iter 3 returns an empty
            // OngoingCodegen; iter 4+ wires real codegen.
            #[cfg(not(target_os = "none"))]
            { driver::aot::run_aot(tcx) }
            #[cfg(target_os = "none")]
            { driver::aot_semos::run_aot(tcx) }
        }
    }

    fn join_codegen(
        &self,
        ongoing_codegen: Box<dyn Any>,
        sess: &Session,
        outputs: &OutputFilenames,
    ) -> (CodegenResults, FxIndexMap<WorkProductId, WorkProduct>) {
        #[cfg(not(target_os = "none"))]
        { ongoing_codegen.downcast::<driver::aot::OngoingCodegen>().unwrap().join(sess, outputs) }
        // Phase 5b iter 3: target side uses the aot_semos stub.
        #[cfg(target_os = "none")]
        { ongoing_codegen.downcast::<driver::aot_semos::OngoingCodegen>().unwrap().join(sess, outputs) }
    }
}

/// Determine if the Cranelift ir verifier should run.
///
/// Returns true when `-Zverify-llvm-ir` is passed, the `CG_CLIF_ENABLE_VERIFIER` env var is set to
/// 1 or when cg_clif is compiled with debug assertions enabled or false otherwise.
fn enable_verifier(sess: &Session) -> bool {
    // Stage G iter 8: env::var is host-only; target side never gets the
    // toggle (debug_assertions still picks it up on debug builds).
    #[cfg(target_os = "none")]
    let from_env = false;
    #[cfg(not(target_os = "none"))]
    let from_env = env::var("CG_CLIF_ENABLE_VERIFIER").as_deref() == Ok("1");
    sess.verify_llvm_ir() || cfg!(debug_assertions) || from_env
}

fn target_triple(sess: &Session) -> target_lexicon::Triple {
    match sess.target.llvm_target.parse() {
        Ok(triple) => triple,
        Err(err) => sess.dcx().fatal(format!("target not recognized: {}", err)),
    }
}

fn build_isa(sess: &Session, jit: bool) -> Arc<dyn TargetIsa + 'static> {
    use target_lexicon::BinaryFormat;

    let target_triple = crate::target_triple(sess);

    let mut flags_builder = settings::builder();
    flags_builder.set("is_pic", if jit { "false" } else { "true" }).unwrap();
    let enable_verifier = if enable_verifier(sess) { "true" } else { "false" };
    flags_builder.set("enable_verifier", enable_verifier).unwrap();
    flags_builder.set("regalloc_checker", enable_verifier).unwrap();

    let frame_ptr =
        { sess.target.options.frame_pointer }.ratchet(sess.opts.cg.force_frame_pointers);
    let preserve_frame_pointer = frame_ptr != rustc_target::spec::FramePointer::MayOmit;
    flags_builder
        .set("preserve_frame_pointers", if preserve_frame_pointer { "true" } else { "false" })
        .unwrap();

    let tls_model = match target_triple.binary_format {
        BinaryFormat::Elf => "elf_gd",
        BinaryFormat::Macho => "macho",
        BinaryFormat::Coff => "coff",
        _ => "none",
    };
    flags_builder.set("tls_model", tls_model).unwrap();

    flags_builder.set("enable_llvm_abi_extensions", "true").unwrap();

    if let Some(align) = sess.opts.unstable_opts.min_function_alignment {
        flags_builder
            .set("log2_min_function_alignment", &align.bytes().ilog2().to_string())
            .unwrap();
    }

    use rustc_session::config::OptLevel;
    match sess.opts.optimize {
        OptLevel::No => {
            flags_builder.set("opt_level", "none").unwrap();
        }
        OptLevel::Less
        | OptLevel::More
        | OptLevel::Size
        | OptLevel::SizeMin
        | OptLevel::Aggressive => {
            flags_builder.set("opt_level", "speed_and_size").unwrap();
        }
    }

    if let target_lexicon::OperatingSystem::Windows = target_triple.operating_system {
        // FIXME remove dependency on this from the Rust ABI. cc bytecodealliance/wasmtime#9510
        flags_builder.enable("enable_multi_ret_implicit_sret").unwrap();
    }

    if let target_lexicon::Architecture::S390x = target_triple.architecture {
        // FIXME remove dependency on this from the Rust ABI. cc bytecodealliance/wasmtime#9510
        flags_builder.enable("enable_multi_ret_implicit_sret").unwrap();
    }

    if let target_lexicon::Architecture::Aarch64(_)
    | target_lexicon::Architecture::Riscv64(_)
    | target_lexicon::Architecture::X86_64 = target_triple.architecture
    {
        // Windows depends on stack probes to grow the committed part of the stack.
        // On other platforms it helps prevents stack smashing.
        flags_builder.enable("enable_probestack").unwrap();
        flags_builder.set("probestack_strategy", "inline").unwrap();
    } else {
        // __cranelift_probestack is not provided and inline stack probes are only supported on
        // AArch64, Riscv64 and x86_64.
        flags_builder.set("enable_probestack", "false").unwrap();
    }

    let flags = settings::Flags::new(flags_builder);

    let isa_builder = match sess.opts.cg.target_cpu.as_deref() {
        // Stage G iter 8: cranelift-native runtime CPU detection is
        // host-only (uses libc cpuid via std). SemOS target hardcodes
        // x86_64 features at build time; "native" falls through to
        // explicit lookup of the build target.
        Some("native") => {
            #[cfg(not(target_os = "none"))]
            { cranelift_native::builder_with_options(true).unwrap() }
            #[cfg(target_os = "none")]
            { cranelift_codegen::isa::lookup(target_triple.clone()).unwrap() }
        }
        Some(value) => {
            let mut builder =
                cranelift_codegen::isa::lookup(target_triple.clone()).unwrap_or_else(|err| {
                    sess.dcx().fatal(format!("can't compile for {}: {}", target_triple, err));
                });
            if builder.enable(value).is_err() {
                sess.dcx()
                    .fatal("the specified target cpu isn't currently supported by Cranelift.");
            }
            builder
        }
        None => {
            let mut builder =
                cranelift_codegen::isa::lookup(target_triple.clone()).unwrap_or_else(|err| {
                    sess.dcx().fatal(format!("can't compile for {}: {}", target_triple, err));
                });
            if target_triple.architecture == target_lexicon::Architecture::X86_64 {
                // Only set the target cpu on x86_64 as Cranelift is missing
                // the target cpu list for most other targets.
                builder.enable(sess.target.cpu.as_ref()).unwrap();
            }
            builder
        }
    };

    match isa_builder.finish(flags) {
        Ok(target_isa) => target_isa,
        Err(err) => sess.dcx().fatal(format!("failed to build TargetIsa: {}", err)),
    }
}

/// This is the entrypoint for a hot plugged rustc_codegen_cranelift
#[unsafe(no_mangle)]
pub fn __rustc_codegen_backend() -> Box<dyn CodegenBackend> {
    Box::new(CraneliftCodegenBackend { config: OnceCell::new() })
}
