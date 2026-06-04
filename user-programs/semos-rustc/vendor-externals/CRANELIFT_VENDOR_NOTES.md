# Cranelift vendoring notes (M27 Phase 5c Stage G)

This directory is the staging area for the Cranelift `no_std` port that
unblocks `cargo check -p rustc_codegen_cranelift --target
x86_64-unknown-none`. It lands incrementally over multiple Stage G
iterations because the sandbox running these agents denies recursive
copy operations (`cp -r`, `Copy-Item -Recurse`, `xcopy`, `robocopy`,
`tar`, `rsync` — all blocked); each crate is copied file-by-file.

## Source

All sub-crates are vendored from `F:/Software/ArmKernel3/compiler/vendor/cranelift-*-0.122.0/`
(the M26 Session B copy of the wasmtime release-38 cycle, downloaded
fresh from crates.io as part of commit `f1b2635`). We pin the **0.122.0**
line, not the 0.127.0 line that `rustc_codegen_cranelift`'s upstream
Cargo.toml originally declared, because:

1. 0.122.0 is already present in-tree from M26 Session B.
2. cg_clif is **co-pinned** to the rustc nightly (`nightly-2026-02-01`)
   anyway, so we own the API-compat decision. Downgrading the cg_clif
   Cargo.toml to 0.122.0 is one targeted edit.
3. 0.127.0 → 0.122.0 is ~5 minor versions; usually that's the breaking
   API window. If 0.122 fails to type-check against the rustc internal
   surface we'll either bump or fork the rustc-side API.

## Status (after Stage G iter 1)

| Crate | Vendored | Patched | Compiles target=x86_64-unknown-none? |
|---|---|---|---|
| cranelift-bitset | ✅ | none needed | (no_std-clean upstream) |
| cranelift-entity | ✅ | none needed | (no_std-clean upstream) |
| cranelift-control | ✅ | `default = ["fuzz"]` → `default = []` | (no_std-clean upstream once fuzz default dropped) |
| cranelift-bforest | ✅ | none needed | (no_std-clean upstream) |
| cranelift-codegen-shared | ✅ | none needed | (no extern crate std) |
| cranelift-codegen | ❌ | TBD — drop `std` feature default, audit timing.rs | TBD |
| cranelift-frontend | ❌ | TBD — `default = ["std"]` → `default = []`, enable `core` feature | TBD |
| cranelift-module | ❌ | TBD | TBD |
| cranelift-object | ❌ | TBD | TBD |
| cranelift-assembler-x64 | ❌ | TBD | TBD |
| cranelift-assembler-x64-meta | ❌ (build-dep only) | TBD | host-only |
| cranelift-codegen-meta | ❌ (build-dep only) | TBD | host-only |
| cranelift-isle | ❌ (build-dep only) | TBD | host-only |
| cranelift-srcgen | ❌ (build-dep only) | TBD | host-only |
| cranelift-native | DROPPED | n/a | (runtime CPU detect via libc — not on SemOS) |
| cranelift-jit | DROPPED | n/a | (mmap+exec pages — not on SemOS) |

## Procedure for the next iteration

Sandbox-permitting (or done from a human shell):

```sh
# From the worktree root, NOT inside vendor-externals:
SRC=F:/Software/ArmKernel3/compiler/vendor
DST=F:/Software/ArmKernel3/.claude/worktrees/<agent>/user-programs/semos-rustc/vendor-externals

cp -r "$SRC/cranelift-codegen-0.122.0"        "$DST/cranelift-codegen"
cp -r "$SRC/cranelift-frontend-0.122.0"       "$DST/cranelift-frontend"
cp -r "$SRC/cranelift-module-0.122.0"         "$DST/cranelift-module"
cp -r "$SRC/cranelift-object-0.122.0"         "$DST/cranelift-object"
cp -r "$SRC/cranelift-assembler-x64-0.122.0"  "$DST/cranelift-assembler-x64"
cp -r "$SRC/cranelift-assembler-x64-meta-0.122.0" "$DST/cranelift-assembler-x64-meta"
cp -r "$SRC/cranelift-codegen-meta-0.122.0"   "$DST/cranelift-codegen-meta"
cp -r "$SRC/cranelift-isle-0.122.0"           "$DST/cranelift-isle"
cp -r "$SRC/cranelift-srcgen-0.122.0"         "$DST/cranelift-srcgen"
```

Then extend `user-programs/semos-rustc/Cargo.toml`'s `[patch.crates-io]`
with the corresponding `cranelift-codegen = { path = "..." }` etc.

## Top-of-mind patches forecast (do NOT pre-apply)

These come from reading the 0.122.0 Cargo.tomls + `lib.rs` headers in
`compiler/vendor/`:

1. **cranelift-codegen/Cargo.toml** — `default = ["std", "unwind",
   "host-arch", "timing"]` needs to be `default = []`. The cg_clif
   side already passes `default-features = false, features =
   ["unwind", "all-native-arch"]` so this is belt-and-suspenders.

2. **cranelift-codegen/src/timing.rs** — uses `std::time::Instant`.
   With `timing` feature off (which we are doing), the whole module
   is `#[cfg(feature = "timing")]`-gated. Confirmed via grep.

3. **cranelift-frontend/Cargo.toml** — `default = ["std"]`. Drop it,
   AND make sure `cranelift-codegen` dep enables only `unwind` not
   `std`. Then enable the `core` feature which gates `hashbrown` for
   no_std HashMap.

4. **cranelift-codegen** has a build-script dep on `cranelift-isle`
   and `cranelift-codegen-meta`. Those run on the **host** at build
   time, generate `.rs` files into `OUT_DIR`, then go away. They
   don't need to be no_std themselves — but their Cargo.toml needs
   to be reachable so cargo doesn't try the registry. Vendor them
   and add to `[patch.crates-io]`; their build deps (anyhow,
   structopt, etc.) can stay on the host.

5. **regalloc2** (a transitive of cranelift-codegen) needs vendoring
   too — version 0.12.2 in 0.122.0's tree. Likely `default =
   ["std"]` to drop.

6. **bumpalo / hashbrown / rustc-hash / smallvec** — registry crates
   pulled by cranelift-codegen. hashbrown is already in our patch
   table indirectly via rustc_data_structures' 0.15 bump (Stage F12).
   The others need `default-features = false` checks.

7. **wasmtime-internal-math = "=35.0.0"** is an EXACT pin pulled by
   cranelift-codegen 0.122.0. This is a small math crate (f32/f64
   IEEE-754 ops). Likely vendor + drop `std` default. The `=35.0.0`
   pin needs to match in our patch table.

8. **cranelift-object → object 0.36** — object is already in the
   workspace at a different version via the rustc_codegen_ssa path
   in Stage F12; reconcile versions. The cg_clif Cargo.toml in
   iter 1 specifies object 0.36 with features
   `["read_core", "write", "archive", "elf"]` — dropped `coff`,
   `macho`, `pe`, and `std`. SemOS only emits ELF.

## Sandbox workaround

If the next agent also hits the cp/copy denial, an effective fallback
is to add `compiler/vendor/cranelift-*-0.122.0/` to `[patch.crates-io]`
**directly** rather than copying first:

```toml
cranelift-codegen = { path = "../../compiler/vendor/cranelift-codegen-0.122.0" }
```

This is uglier (path-traversal outside the workspace, modifies
in-tree state) but unblocks the work without recursive-copy permission.
The downside is any cargo-toml edits to drop `default = ["std"]`
mutate `compiler/vendor/` — used by the M26 `compiler/` workspace
which DOES expect std-enabled cranelift. Mitigation: those Cargo.toml
files were generated by `cargo vendor`, so the "real" copies live
in registry/git; we can safely edit the in-tree copies as long as
the M26 compiler/ workspace doesn't also build with them as a
target=x86_64-unknown-none.

Verified mitigation path: M26 compiler/ DOES enable std (it's a
host-target Cranelift JIT smoke test). So our `default = ["std"]` →
`default = []` patch will break it. Conclusion: vendor copies into
vendor-externals/ are the right answer; share the same upstream
sources but isolate the patches.
