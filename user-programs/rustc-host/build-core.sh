#!/usr/bin/env bash
# M27 DEMO 80 option B — drive rustc-host directly to compile core (+ compiler_builtins)
# for x86_64-unknown-none, bypassing cargo's `-Z build-std` probe (which greps rustc
# stderr for the English "unsupported crate type" text; our un-localized port emits the
# raw fluent ID, so cargo miscounts file-names). Flags are cargo's verbatim build-std
# invocation with the rustc binary swapped to rustc-host and json error-format dropped.
set -euo pipefail

HOST=F:/Software/ArmKernel3/user-programs/rustc-host
RUSTC="$HOST/target/x86_64-pc-windows-msvc/release/rustc-host.exe"
# Local PATCHED copy of the 1.94 (nightly-2025-12-23) library tree. The vendored
# compiler is a SemOS-customized rustc that lines up with NO single upstream
# nightly's core (it lacks `rustc_do_not_implement_via_object` + `eii_extern_target`
# yet already renamed macro-transparency `semitransparent`->`semiopaque` and expanded
# the `offload`/`va_copy` intrinsic sigs). sysroot-src/ holds 1.94 core with those few
# deltas patched to match the vendored compiler. See build-core.sh history.
SRC=F:/Software/ArmKernel3/user-programs/rustc-host/sysroot-src/library
DEPS="$HOST/sysroot-test/target/x86_64-unknown-none/release/deps"
RELDEPS="$HOST/sysroot-test/target/release/deps"

export RUSTC_BOOTSTRAP=1
mkdir -p "$DEPS" "$RELDEPS"

echo "=== compiling core ==="
"$RUSTC" --crate-name core --edition=2024 \
  "$SRC/core/src/lib.rs" \
  --crate-type lib --emit=dep-info,metadata,link \
  -C opt-level=3 -C panic=abort -C embed-bitcode=no \
  --warn=unexpected_cfgs \
  --check-cfg 'cfg(no_fp_fmt_parse)' \
  --check-cfg 'cfg(feature, values(any()))' \
  --check-cfg 'cfg(target_has_reliable_f16)' \
  --check-cfg 'cfg(target_has_reliable_f16_math)' \
  --check-cfg 'cfg(target_has_reliable_f128)' \
  --check-cfg 'cfg(target_has_reliable_f128_math)' \
  --check-cfg 'cfg(llvm_enzyme)' \
  --check-cfg 'cfg(docsrs,test)' \
  --check-cfg 'cfg(feature, values("debug_refcell", "llvm_enzyme", "optimize_for_size", "panic_immediate_abort"))' \
  -C metadata=e38da9d7ae867273 -C extra-filename=-53344cc650ffcdf9 \
  --out-dir "$DEPS" --target x86_64-unknown-none -C strip=debuginfo \
  -Z force-unstable-if-unmarked \
  -L "dependency=$DEPS" -L "dependency=$RELDEPS" \
  --cap-lints allow --cfg procmacro_stub -A unexpected_cfgs \
  -C 'link-arg=/STACK:16777216' -C 'link-arg=/HEAP:268435456,1048576'

echo "=== core done; artifacts: ==="
ls -la "$DEPS"/libcore-* 2>/dev/null || true

# compiler_builtins: STUB, not the full crate. core records an implicit
# compiler_builtins dependency, so loading core (for /hello.rs front-end name
# resolution) requires *a* crate named compiler_builtins with a matching
# rustc/version stamp present — but /hello.rs links nothing, so only the
# metadata shell is needed (design doc §7 scope guard: "No linking of a real
# core"). The full compiler-builtins crate trips a prelude-injection quirk under
# the host driver (bare core prelude macros `panic!`/`debug_assert!`/`matches!`
# don't auto-resolve via `--extern core`, though explicit `use core::panic` does
# — TODO if real codegen/link is ever needed). The `#[compiler_builtins]` attr
# marks this AS compiler_builtins so it doesn't self-depend; built against core.
echo "=== compiling compiler_builtins (STUB; satisfies core's implicit dep) ==="
CBSTUB=/tmp/semos-cbstub.rs
printf '#![feature(compiler_builtins)]\n#![compiler_builtins]\n#![no_builtins]\n#![no_std]\n' > "$CBSTUB"
"$RUSTC" --crate-name compiler_builtins --edition=2024 --crate-type lib \
  --emit=metadata,link -C panic=abort --target x86_64-unknown-none \
  -C metadata=c57a5d6e0460c83c -C extra-filename=-fb74582bf62b1baa \
  --out-dir "$DEPS" -Z force-unstable-if-unmarked \
  --extern "core=$DEPS/libcore-53344cc650ffcdf9.rmeta" \
  -L "dependency=$DEPS" -L "dependency=$RELDEPS" \
  --cap-lints allow --cfg procmacro_stub -A unexpected_cfgs \
  "$CBSTUB"

echo "=== compiler_builtins done; artifacts: ==="
ls -la "$DEPS"/libcompiler_builtins-* 2>/dev/null || true

echo
echo "=== self-test: compile a no_std prelude snippet against the produced sysroot ==="
PT=/tmp/semos-sysroot-selftest.rs
printf '#![no_std]\npub fn f() -> Option<i32> { Some(41).map(|x| x + 1) }\npub fn g(s: &str) -> usize { s.len() }\n' > "$PT"
"$RUSTC" --edition 2021 --crate-type lib --target x86_64-unknown-none \
  --extern "core=$DEPS/libcore-53344cc650ffcdf9.rmeta" \
  --extern "compiler_builtins=$DEPS/libcompiler_builtins-fb74582bf62b1baa.rmeta" \
  -L "dependency=$DEPS" --cfg procmacro_stub -A unexpected_cfgs \
  --emit=metadata -o /tmp/semos-sysroot-selftest.rmeta "$PT" \
  && echo "SELFTEST OK: rustc-host core + compiler_builtins load and resolve the prelude" \
  || echo "SELFTEST FAILED"
