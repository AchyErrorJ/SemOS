//! semos-rustc — M27 Phase 5b iter 2: static cg_clif backend injection.
//!
//! iter 1 (commit `f9ae4fb`) proved the binary links and reaches
//! `rustc_driver_impl::run_compiler`. The driver then needs a codegen
//! backend; upstream rustc dlopens one from a shared object, but on
//! SemOS that path is cfg-stubbed to panic ("requires statically-linked
//! backend"). iter 2 plugs the gap by overriding
//! `Callbacks::config(&mut Config)` to set
//! `config.make_codegen_backend = Some(<cg_clif factory>)`. The driver
//! takes that precedence path (interface.rs:454) and never touches the
//! dlopen loader.
//!
//! After iter 2, `run_compiler(["rustc", "--version"], &mut cb)` should
//! parse args, print version, and exit cleanly. A real `.rs` source
//! invocation reaches the AOT driver (`cg_clif::driver::aot`) which is
//! currently host-gated — iter 3 wires a SemOS-native AOT path.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use rustc_codegen_ssa::traits::CodegenBackend;
use rustc_driver_impl::{Callbacks, Compilation};
use rustc_interface::interface::{Config, Compiler};
use rustc_middle::ty::TyCtxt;
use rustc_session::config::Options;
use rustc_target::spec::Target;
use semos_std::println;

/// Flip to `true` to restore the pipeline-checkpoint trace (crate parsed /
/// expansion / analysis / "run_compiler returned cleanly"). Default off — these
/// printed on every compile.
const RUSTC_DEBUG: bool = false;

macro_rules! rdbg {
    ($($arg:tt)*) => {{ if RUSTC_DEBUG { semos_std::println!($($arg)*); } }};
}

struct SemosCallbacks;

impl Callbacks for SemosCallbacks {
    fn config(&mut self, config: &mut Config) {
        // Inject cg_clif as the static codegen backend. `make_codegen_backend`
        // takes precedence over the dylib loader (rustc_interface/src/
        // interface.rs:454) so the dlopen path that's cfg-stubbed to panic
        // on SemOS never fires.
        config.make_codegen_backend = Some(Box::new(
            |_opts: &Options, _target: &Target| -> Box<dyn CodegenBackend> {
                rustc_codegen_cranelift::__rustc_codegen_backend()
            },
        ));
    }

    // iter 8 diagnostics: print pipeline checkpoints so we can see how far
    // a real compile gets when diagnostics are otherwise swallowed.
    fn after_crate_root_parsing(
        &mut self,
        _c: &Compiler,
        _k: &mut rustc_ast::ast::Crate,
    ) -> Compilation {
        rdbg!("[semos-rustc] checkpoint: crate root parsed");
        Compilation::Continue
    }

    fn after_expansion<'tcx>(&mut self, _c: &Compiler, _tcx: TyCtxt<'tcx>) -> Compilation {
        rdbg!("[semos-rustc] checkpoint: macro expansion done");
        Compilation::Continue
    }

    fn after_analysis<'tcx>(&mut self, _c: &Compiler, _tcx: TyCtxt<'tcx>) -> Compilation {
        rdbg!("[semos-rustc] checkpoint: analysis (typeck+borrowck) done — entering codegen");
        Compilation::Continue
    }
}

/// Phase 5b iter 6: shell argv came from semos_std::env::args(). We
/// intercept `--version`/`-V` and `--help`/`-h` BEFORE calling into
/// rustc_driver_impl because the upstream diagnostic init
/// (rustc_session::session::mk_emitter → AnnotateSnippetEmitter +
/// Translator + stderr_destination) still pulls cfg-gated host
/// surface that silently aborts before any rustc-side println fires.
/// Intercepting here keeps `semos-rustc --version` working end-to-end
/// (proves binary load + semos_std::io::stdout) while we grind through
/// the rest of the rustc diag stack in later iters.
fn intercept_short_circuit(args: &[String]) -> bool {
    for a in args {
        match a.as_str() {
            "--version" | "-V" => {
                println!("rustc 1.86.0-semos (M27 Phase 5b iter 6)");
                println!("backend: cranelift (statically linked)");
                return true;
            }
            "--help" | "-h" => {
                println!("semos-rustc — SemOS-native Rust compiler (M27 port)");
                println!("");
                println!("USAGE: semos-rustc [OPTIONS] <INPUT>");
                println!("");
                println!("OPTIONS:");
                println!("  -V, --version    Print version info and exit");
                println!("  -h, --help       Print this help");
                println!("  -o <PATH>        Write output executable to <PATH>");
                println!("");
                println!("EXAMPLE:");
                println!("  semos-rustc /hello.rs -o /tmp/hello.elf");
                return true;
            }
            _ => {}
        }
    }
    false
}

/// DEMO 80 Layer B: enumerate the SATA sysroot blob (staged by
/// `tools/pack-sysroot-blob.py`) and decode each staged `.rmeta` via the c3
/// probe. The `libcore` rmeta (~57 MB) is too big to embed, so it is streamed
/// from disk in 64 KiB chunks through `SYS_SYSROOT_READ`.
fn c3_disk_probe(cfg_version: &'static str) {
    use semos_std::arch::{SYS_SYSROOT_INFO, SYS_SYSROOT_READ, syscall3, syscall4};

    println!("[c3-disk] enumerating SATA sysroot blob...");
    let mut files: Vec<(u64, String, u64)> = Vec::new();
    let mut idx: u64 = 0;
    loop {
        let mut name = [0u8; 128];
        let len = unsafe {
            syscall3(SYS_SYSROOT_INFO, idx, name.as_mut_ptr() as u64, name.len() as u64)
        };
        if len == u64::MAX {
            break;
        }
        let nlen = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        let nm = match core::str::from_utf8(&name[..nlen]) {
            Ok(s) => String::from(s),
            Err(_) => String::from("<bad-utf8>"),
        };
        println!("[c3-disk]   [{}] {} ({} bytes)", idx, nm, len);
        files.push((idx, nm, len));
        idx += 1;
    }
    if files.is_empty() {
        println!("[c3-disk] no sysroot blob staged (attach sysroot.img as a SATA/AHCI drive)");
        return;
    }

    for (i, nm, len) in files {
        // c3 probe validates rmeta metadata encoding only; .rlib archives
        // contain an rmeta but are not raw MetadataBlobs.
        if !nm.ends_with(".rmeta") {
            continue;
        }
        println!("[c3-disk] reading {} from disk...", nm);
        let mut bytes: Vec<u8> = Vec::with_capacity(len as usize);
        let mut off: u64 = 0;
        let mut chunk = [0u8; 65536];
        let mut ok = true;
        while off < len {
            let n = unsafe {
                syscall4(SYS_SYSROOT_READ, i, off, chunk.as_mut_ptr() as u64, chunk.len() as u64)
            };
            if n == u64::MAX {
                println!("[c3-disk] read error at offset {}", off);
                ok = false;
                break;
            }
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..n as usize]);
            off += n;
        }
        if !ok {
            continue;
        }
        println!("[c3-disk] read {} bytes (expected {}); probing...", bytes.len(), len);
        rustc_metadata::locator::semos_c3_probe(bytes, cfg_version);
    }
}

/// Derive a crate name from a staged sysroot filename:
/// `libcore-53344cc650ffcdf9.rmeta` → `core`. Strips the `lib` prefix, the
/// `.rmeta`/`.rlib` extension, and the trailing `-<hash>` (the hash has no `-`).
fn crate_name_from_libfile(fname: &str) -> Option<&str> {
    let stem = fname.strip_prefix("lib")?;
    let stem = stem.strip_suffix(".rmeta").or_else(|| stem.strip_suffix(".rlib"))?;
    Some(match stem.rfind('-') {
        Some(pos) => &stem[..pos],
        None => stem,
    })
}

/// Enumerate the SATA sysroot blob and build `--extern <crate>=/sysroot/<file>`
/// args for each staged crate. Empty if no blob is staged.
///
/// If both `.rmeta` and `.rlib` are staged for the same crate, we pass the
/// `.rlib` — that is the artifact rustc needs for both metadata loading and
/// linking an executable. Passing the `.rmeta` would make codegen fail with
/// `metadata_lib_required`.
fn sysroot_extern_args() -> Vec<String> {
    use semos_std::arch::{SYS_SYSROOT_INFO, syscall3};

    #[derive(Clone)]
    struct Entry<'a> {
        fname: &'a str,
        is_rlib: bool,
    }

    // First pass: read the blob directory.
    let mut entries: alloc::vec::Vec<(u64, alloc::string::String)> = Vec::new();
    let mut idx: u64 = 0;
    loop {
        let mut name = [0u8; 128];
        let len = unsafe {
            syscall3(SYS_SYSROOT_INFO, idx, name.as_mut_ptr() as u64, name.len() as u64)
        };
        if len == u64::MAX {
            break;
        }
        let nlen = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        if let Ok(fname) = core::str::from_utf8(&name[..nlen]) {
            entries.push((idx, String::from(fname)));
        }
        idx += 1;
    }

    // Second pass: pick the .rlib for each crate if present, else fall back
    // to the .rmeta. Keep the original blob order for determinism.
    let mut chosen: alloc::vec::Vec<(u64, &str, bool)> = Vec::new();
    let mut seen: alloc::vec::Vec<&str> = Vec::new();
    for (i, fname) in entries.iter() {
        if let Some(crate_name) = crate_name_from_libfile(fname) {
            let is_rlib = fname.ends_with(".rlib");
            if let Some(pos) = seen.iter().position(|&n| n == crate_name) {
                if is_rlib {
                    chosen[pos] = (*i, fname.as_str(), true);
                }
            } else {
                seen.push(crate_name);
                chosen.push((*i, fname.as_str(), is_rlib));
            }
        }
    }

    let mut out = Vec::new();
    for (_i, fname, _is_rlib) in chosen {
        let crate_name = crate_name_from_libfile(fname).unwrap();
        let mut spec = String::from(crate_name);
        spec.push('=');
        spec.push_str("/sysroot/");
        spec.push_str(fname);
        println!("[sysroot] --extern {}", spec);
        out.push(String::from("--extern"));
        out.push(spec);
    }
    out
}

semos_std::main!(fn main() {
    // Skip argv[0] like rustc_driver_impl::run_compiler does internally.
    let argv: Vec<String> = semos_std::env::args().into_iter().skip(1).collect();

    if intercept_short_circuit(&argv) {
        return;
    }

    // DEMO 80 step C3: decode-compatibility self-test. Embeds a host-built
    // compiler_builtins.rmeta and asks rustc_metadata to decode it — proving
    // (or disproving) that host-built crate metadata is schema-compatible with
    // this semos-rustc before we build any disk-sysroot plumbing.
    if argv.iter().any(|a| a == "--c3-selftest") {
        let cfg_version = option_env!("CFG_VERSION").unwrap_or("1.84.0-semos-m27");

        // (1) RAM path: the embedded host-built compiler_builtins.rmeta. With the
        // rustc-host-built rmeta (matching symbol table) get_header should now
        // decode name="compiler_builtins" (vs the old "concat").
        static CB_RMETA: &[u8] = include_bytes!("../test-sources/compiler_builtins.rmeta");
        println!("[c3] embedded compiler_builtins.rmeta: {} bytes", CB_RMETA.len());
        rustc_metadata::locator::semos_c3_probe(CB_RMETA.to_vec(), cfg_version);

        // (2) Disk path (Layer B): enumerate + decode every rmeta staged on the
        // SATA sysroot blob — notably the ~57 MB libcore, too big to embed.
        c3_disk_probe(cfg_version);

        println!("[c3] selftest returned");
        return;
    }

    println!("semos-rustc Phase 5b iter 8 [BUILD-TAG abc123] — invoking rustc_driver_impl::run_compiler");
    println!("argv: {:?}", argv);
    println!("[main] about to call rustc_driver_impl::run_compiler");

    // Prepend the binary name back since run_compiler strips argv[0].
    let mut args: Vec<String> = vec![String::from("semos-rustc")];
    args.extend(argv);

    // M27 DEMO 80: auto-inject `--extern <crate>=/sysroot/<lib...>.rmeta` for
    // every crate staged in the SATA sysroot blob, so `semos-rustc /hello.rs`
    // resolves `core` (+ its implicit `compiler_builtins` dep) from disk without
    // the user typing --extern. No-op when no blob is staged.
    let externs = sysroot_extern_args();
    if externs.is_empty() {
        println!("[sysroot] no disk sysroot staged — compile will hit the core wall");
    }
    args.extend(externs);

    let mut cb = SemosCallbacks;
    rustc_driver_impl::run_compiler(&args, &mut cb);
    rdbg!("run_compiler returned cleanly");
    // Explicit exit: skip any leftover destructor work that might hang
    // in a threaded codegen path and prevent the shell from getting its
    // prompt back. The macro would call exit after main returns anyway.
    semos_std::process::exit(0);
});
