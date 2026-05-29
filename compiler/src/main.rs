//! semos-compiler — Phase 14 M26 v1.
//!
//! Host-side smoke test for the Cranelift pipeline. Builds an IR function
//! `i64 add(i64 a, i64 b) { return a + b; }`, lowers it to x86_64 machine
//! code via cranelift-codegen, and dumps the bytes. Validates the vendored
//! Cranelift compiles + the IR -> codegen pipeline works in our tree.
//!
//! NEXT (separate session): wrap the codegen output into a SemOS ET_EXEC
//! ELF (entry at USER_CODE_BASE = 0x400000, `_start` calling SYS_EXIT)
//! via cranelift-object, embed via include_bytes!, and SPAWN it. That's
//! the "rustc-on-metal v0" pipeline — but doing it correctly means
//! integrating with our linker script + user-program layout, which is
//! its own integration project.

use cranelift_codegen::ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName};
use cranelift_codegen::isa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::{verify_function, Context};
use cranelift_codegen::ir::types::I64;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use target_lexicon::Triple;

fn build_add_function() -> Function {
    let mut sig = Signature::new(isa::CallConv::SystemV);
    sig.params.push(AbiParam::new(I64));
    sig.params.push(AbiParam::new(I64));
    sig.returns.push(AbiParam::new(I64));

    let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let mut fbc = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut func, &mut fbc);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);

        let a = b.block_params(entry)[0];
        let bb = b.block_params(entry)[1];
        let sum = b.ins().iadd(a, bb);
        b.ins().return_(&[sum]);
        b.finalize();
    }
    func
}

fn main() -> Result<(), String> {
    println!("[semos-compiler] M26 smoke test — vendored Cranelift pipeline");

    // 1. Pick the target. x86_64-unknown-none, the same triple our user
    //    programs build against.
    let triple = "x86_64-unknown-none"
        .parse::<Triple>()
        .map_err(|e| format!("parse triple: {}", e))?;
    println!("[semos-compiler] target triple: {}", triple);

    let mut flag_builder = settings::builder();
    flag_builder
        .set("opt_level", "speed")
        .map_err(|e| format!("set opt_level: {}", e))?;
    flag_builder
        .set("is_pic", "false") // ET_EXEC, matches our linker.ld
        .map_err(|e| format!("set is_pic: {}", e))?;
    let flags = settings::Flags::new(flag_builder);

    let isa = isa::lookup(triple)
        .map_err(|e| format!("isa lookup: {}", e))?
        .finish(flags)
        .map_err(|e| format!("isa finish: {}", e))?;
    println!("[semos-compiler] ISA: {}", isa.name());

    // 2. Build a tiny IR function: i64 add(a, b) = a + b.
    let func = build_add_function();
    verify_function(&func, isa.as_ref())
        .map_err(|e| format!("verify: {}", e))?;
    println!("[semos-compiler] IR verified:\n{}", func.display());

    // 3. Lower to machine code.
    let mut ctx = Context::for_function(func);
    let compiled = ctx
        .compile(isa.as_ref(), &mut Default::default())
        .map_err(|e| format!("compile: {:?}", e))?;
    let bytes = compiled.code_buffer();
    println!(
        "[semos-compiler] compiled to {} bytes of x86_64 machine code",
        bytes.len()
    );

    // 4. Dump the bytes (small enough to hex-print).
    print!("[semos-compiler] machine code:");
    for (i, b) in bytes.iter().enumerate() {
        if i % 16 == 0 {
            print!("\n    {:04X}:", i);
        }
        print!(" {:02X}", b);
    }
    println!();

    // 5. Sanity: the first few bytes should look like a System V x86_64
    //    function prologue or a direct mov+add+ret depending on opt level.
    //    We don't disassemble here — `objdump -d --disassembler-options=intel`
    //    on the bytes will show the real instructions.
    if bytes.is_empty() {
        return Err("Cranelift emitted 0 bytes".into());
    }

    println!("[semos-compiler] OK — Cranelift pipeline live in tree");
    Ok(())
}
