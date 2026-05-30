//! semos-cc — M27 D.2: the host compiler ported to Ring 3 on SemOS.
//!
//! Same shape as `compiler/src/main.rs` (D.1) but built no_std + semos-std,
//! running as a user program. When SYS_SPAWNed it:
//!
//!   1. Hand-emits a 47-byte `_start` shim that does
//!      `SYS_WRITE("semos-cc D2\n") → call add(1,2) → SYS_EXIT(rax)`.
//!   2. Inlines the same 13-byte `add(i64,i64)` Cranelift produced in D.1
//!      (`push rbp / mov rbp,rsp / lea rax,[rdi+rsi] / mov rsp,rbp / pop rbp / ret`).
//!      Live Cranelift codegen on SemOS is a follow-up task — the bytes are
//!      a known-good snapshot from D.1 so we can validate the rest of the
//!      pipeline (ELF wrap → FS write → SYS_SPAWN) end-to-end first.
//!   3. Wraps shim + marker + add bytes into an ET_EXEC at entry 0x400078.
//!   4. Writes the resulting ELF to `/d2-emitted.elf` via the install-anywhere
//!      path namespace (semos-std::fs::write → SYS_OPEN(CREATE) + SYS_FWRITE).
//!
//! Pairs with DEMO 73 which then `SYS_SPAWN`s `/d2-emitted.elf` and asserts
//! exit==3 + the "semos-cc" marker — the same pass condition as DEMO 72,
//! but with the ELF produced *on* SemOS by another SemOS program.

#![no_std]
#![no_main]

use cranelift_codegen::ir::types::I64;
use cranelift_codegen::ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName};
use cranelift_codegen::isa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::{verify_function, Context};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use semos_std::vec::Vec;
use semos_std::{fs, main, println};
use target_lexicon::Triple;

// ---- SemOS user layout (must match kernel-x86_64::paging::user_layout) ----
const USER_CODE_BASE: u64 = 0x400000;
const ELF_HEADER_SIZE: u64 = 64;
const PHDR_SIZE: u64 = 56;
const ENTRY_FILE_OFFSET: u64 = ELF_HEADER_SIZE + PHDR_SIZE; // 0x78
const ENTRY_VADDR: u64 = USER_CODE_BASE + ENTRY_FILE_OFFSET; // 0x400078

// ELF constants the kernel's loader checks.
const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_R: u32 = 4;

// SemOS syscall numbers (must match kernel-core::syscall::numbers).
const SYS_WRITE_N: u32 = 0;
const SYS_EXIT_N: u32 = 2;

/// Marker the kernel-side DEMO greps for in the emitted-child's stdout.
const MARKER: &[u8] = b"semos-cc D2\n";

/// Hand-emitted `_start` shim — exactly 47 bytes, layout identical to the
/// D.1 host emitter.
const SHIM_LEN: usize = 47;

/// Build Cranelift IR for `i64 add(i64, i64) { a + b }` — same IR the
/// D.1 host compiler used; Cranelift will lower it to the System V
/// add function the `_start` shim calls.
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

/// Lower the add() IR to x86_64 machine bytes via Cranelift's codegen.
/// Returns the bytes (or panics — there's nothing to recover from on
/// SemOS if the compiler itself fails).
fn compile_add_bytes() -> Vec<u8> {
    println!("semos-cc: STAGE 2: parsing target triple");
    let triple: Triple = "x86_64-unknown-none"
        .parse()
        .expect("parse target triple");
    let mut flag_builder = settings::builder();
    flag_builder
        .set("opt_level", "speed")
        .expect("set opt_level");
    flag_builder
        .set("is_pic", "false")
        .expect("set is_pic");
    let flags = settings::Flags::new(flag_builder);

    println!("semos-cc: STAGE 3: building x86_64 ISA");
    let isa = isa::lookup(triple)
        .expect("isa lookup")
        .finish(flags)
        .expect("isa finish");

    println!("semos-cc: STAGE 4a: building IR");
    let func = build_add_function();
    println!("semos-cc: STAGE 4b: verifying IR");
    verify_function(&func, isa.as_ref()).expect("verify add");

    println!("semos-cc: STAGE 4c: lowering to machine code");
    let mut ctx = Context::for_function(func);
    let compiled = ctx
        .compile(isa.as_ref(), &mut Default::default())
        .expect("compile add");
    compiled.code_buffer().to_vec()
}

/// Where to write the emitted ELF. Install-anywhere (any absolute path that
/// isn't `/bin/<name>`) routes through `spawn_namespace_elf`, so the kernel
/// can SPAWN this without a hardcoded table entry.
const OUT_PATH: &str = "/d2-emitted.elf";

fn emit_start_shim(rel32_to_add: i32) -> [u8; SHIM_LEN] {
    let marker_vaddr: u64 = ENTRY_VADDR + SHIM_LEN as u64;
    let marker_len: u32 = MARKER.len() as u32;

    let mut s = [0u8; SHIM_LEN];
    let mut p = 0usize;

    // mov rdi, marker_vaddr            ; 48 BF <imm64>      (10 B)
    s[p] = 0x48; s[p + 1] = 0xBF; p += 2;
    s[p..p + 8].copy_from_slice(&marker_vaddr.to_le_bytes()); p += 8;

    // mov esi, marker_len              ; BE <imm32>         (5 B)
    s[p] = 0xBE; p += 1;
    s[p..p + 4].copy_from_slice(&marker_len.to_le_bytes()); p += 4;

    // mov eax, SYS_WRITE               ; B8 <imm32>         (5 B)
    s[p] = 0xB8; p += 1;
    s[p..p + 4].copy_from_slice(&SYS_WRITE_N.to_le_bytes()); p += 4;

    // syscall                          ; 0F 05              (2 B)
    s[p] = 0x0F; s[p + 1] = 0x05; p += 2;

    // mov edi, 1                       ; SystemV arg0       (5 B)
    s[p] = 0xBF; p += 1;
    s[p..p + 4].copy_from_slice(&1u32.to_le_bytes()); p += 4;

    // mov esi, 2                       ; SystemV arg1       (5 B)
    s[p] = 0xBE; p += 1;
    s[p..p + 4].copy_from_slice(&2u32.to_le_bytes()); p += 4;

    // call rel32                       ; E8 <imm32>         (5 B)
    s[p] = 0xE8; p += 1;
    s[p..p + 4].copy_from_slice(&rel32_to_add.to_le_bytes()); p += 4;

    // mov rdi, rax                     ; 48 89 C7           (3 B)
    s[p] = 0x48; s[p + 1] = 0x89; s[p + 2] = 0xC7; p += 3;

    // mov eax, SYS_EXIT                ; B8 <imm32>         (5 B)
    s[p] = 0xB8; p += 1;
    s[p..p + 4].copy_from_slice(&SYS_EXIT_N.to_le_bytes()); p += 4;

    // syscall                          ; 0F 05              (2 B)
    s[p] = 0x0F; s[p + 1] = 0x05; p += 2;

    assert!(p == SHIM_LEN, "_start shim length drifted");
    s
}

fn emit_elf(shim: &[u8], marker: &[u8], add_bytes: &[u8]) -> Vec<u8> {
    let body_len = shim.len() + marker.len() + add_bytes.len();
    let total = ENTRY_FILE_OFFSET as usize + body_len;
    let mut buf: Vec<u8> = Vec::with_capacity(total);
    buf.resize(total, 0);

    // ELF64 header
    buf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    buf[4] = 2; // ELFCLASS64
    buf[5] = 1; // ELFDATA2LSB
    buf[6] = 1; // EV_CURRENT
    buf[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
    buf[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
    buf[20..24].copy_from_slice(&1u32.to_le_bytes());
    buf[24..32].copy_from_slice(&ENTRY_VADDR.to_le_bytes());
    buf[32..40].copy_from_slice(&ELF_HEADER_SIZE.to_le_bytes());
    buf[52..54].copy_from_slice(&(ELF_HEADER_SIZE as u16).to_le_bytes());
    buf[54..56].copy_from_slice(&(PHDR_SIZE as u16).to_le_bytes());
    buf[56..58].copy_from_slice(&1u16.to_le_bytes());

    // Program header — one R+X PT_LOAD covering everything.
    let ph = ELF_HEADER_SIZE as usize;
    let total_u64 = total as u64;
    buf[ph     ..ph +  4].copy_from_slice(&PT_LOAD.to_le_bytes());
    buf[ph +  4..ph +  8].copy_from_slice(&(PF_R | PF_X).to_le_bytes());
    buf[ph +  8..ph + 16].copy_from_slice(&0u64.to_le_bytes());
    buf[ph + 16..ph + 24].copy_from_slice(&USER_CODE_BASE.to_le_bytes());
    buf[ph + 24..ph + 32].copy_from_slice(&USER_CODE_BASE.to_le_bytes());
    buf[ph + 32..ph + 40].copy_from_slice(&total_u64.to_le_bytes());
    buf[ph + 40..ph + 48].copy_from_slice(&total_u64.to_le_bytes());
    buf[ph + 48..ph + 56].copy_from_slice(&0x1000u64.to_le_bytes());

    // Body: shim | marker | add
    let mut off = ENTRY_FILE_OFFSET as usize;
    buf[off..off + shim.len()].copy_from_slice(shim);
    off += shim.len();
    buf[off..off + marker.len()].copy_from_slice(marker);
    off += marker.len();
    buf[off..off + add_bytes.len()].copy_from_slice(add_bytes);

    buf
}

main!(fn main() {
    println!("semos-cc: D.2 emitter — building SemOS ELF on SemOS");

    // Lower add() via live Cranelift codegen — no inlined snapshot.
    println!("semos-cc: STAGE 1: invoking Cranelift to compile add(i64,i64)");
    let add_bytes = compile_add_bytes();
    println!("semos-cc: STAGE 5: Cranelift emitted {} bytes for add()", add_bytes.len());

    // call rel32: end-of-call (shim offset 37) → start of add (= SHIM_LEN + marker)
    let rel32: i32 = (SHIM_LEN as i32 - 37) + MARKER.len() as i32;
    let shim = emit_start_shim(rel32);
    let elf = emit_elf(&shim, MARKER, &add_bytes);

    println!("semos-cc: emitted {} B ELF (entry 0x{:X})", elf.len(), ENTRY_VADDR);

    match fs::write(OUT_PATH, &elf) {
        Ok(()) => {
            println!("semos-cc: wrote {} → exit 0", OUT_PATH);
        }
        Err(_) => {
            println!("semos-cc: FAIL writing {}", OUT_PATH);
            semos_std::process::exit(1);
        }
    }
});
