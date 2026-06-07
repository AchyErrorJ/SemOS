//! semos-rustc — M27 Phase 5c Stage G iter 10 smoke binary.
//!
//! Stage G iters 6-9 ported cranelift-{codegen,frontend,module,object}
//! plus `rustc_codegen_cranelift` to no_std on x86_64-unknown-none. This
//! binary exercises the lowest layer of that pipeline directly: build a
//! trivial Cranelift IR function (`extern "C" fn main() -> i32 { 42 }`),
//! lower it via the x86 backend, emit an ELF object via cranelift-object,
//! and print a hash + size summary.
//!
//! This is not yet DEMO 80 — that needs rustc_driver_impl plus a SemOS-
//! native AOT driver. But it proves the Cranelift codegen pipeline runs
//! target-side and produces an ELF on SemOS, which was Stage G's whole
//! point.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use cranelift_codegen::ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName, types};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use semos_std::println;

fn build_object() -> Result<Vec<u8>, &'static str> {
    // 1. Build an x86_64 ISA with default flags.
    let mut flag_builder = settings::builder();
    flag_builder.set("is_pic", "false").map_err(|_| "set is_pic")?;
    flag_builder.set("opt_level", "speed").map_err(|_| "set opt_level")?;
    let flags = settings::Flags::new(flag_builder);

    let triple = target_lexicon::triple!("x86_64-unknown-none");
    let isa_builder = cranelift_codegen::isa::lookup(triple).map_err(|_| "isa lookup")?;
    let isa = isa_builder.finish(flags).map_err(|_| "isa finish")?;

    // 2. cranelift-object builder for ELF emission.
    let obj_builder = ObjectBuilder::new(
        isa,
        b"semos-rustc-smoke".to_vec(),
        cranelift_module::default_libcall_names(),
    )
    .map_err(|_| "object builder")?;
    let mut module = ObjectModule::new(obj_builder);

    // 3. Build `extern "C" fn main() -> i32 { 42 }`.
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I32));
    let main_id = module
        .declare_function("main", Linkage::Export, &sig)
        .map_err(|_| "declare main")?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let mut fbx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fbx);
        let block = builder.create_block();
        builder.append_block_params_for_function_params(block);
        builder.switch_to_block(block);
        builder.seal_block(block);
        let v = builder.ins().iconst(types::I32, 42);
        builder.ins().return_(&[v]);
        builder.finalize();
    }

    module.define_function(main_id, &mut ctx).map_err(|_| "define function")?;

    // 4. Finalize and emit the ELF.
    let product = module.finish();
    let bytes = product.emit().map_err(|_| "object emit")?;
    Ok(bytes)
}

semos_std::main!(fn main() {
    println!("semos-rustc Phase 5c Stage G iter 10 smoke");
    match build_object() {
        Ok(bytes) => {
            // Tiny fingerprint so we can compare across runs.
            let mut sum: u64 = 0;
            for &b in &bytes {
                sum = sum.wrapping_mul(31).wrapping_add(b as u64);
            }
            println!(
                "cg_clif pipeline: emitted ELF object, {} bytes, hash={:#018x}",
                bytes.len(),
                sum
            );
            if bytes.len() >= 4 && &bytes[..4] == b"\x7fELF" {
                println!("ELF magic OK");
            } else {
                println!("ELF magic MISSING");
            }
        }
        Err(stage) => {
            println!("cg_clif pipeline failed at stage: {}", stage);
        }
    }
});
