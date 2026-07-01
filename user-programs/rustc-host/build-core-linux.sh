#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HOST="$ROOT/user-programs/rustc-host"
RUSTC="$HOST/target/x86_64-unknown-linux-gnu/release/rustc-host"
SRC="$HOST/sysroot-src/library"
DEPS="$HOST/sysroot-test/target/x86_64-unknown-none/release/deps"
RELDEPS="$HOST/sysroot-test/target/release/deps"

export RUSTC_BOOTSTRAP=1
mkdir -p "$DEPS" "$RELDEPS"

echo "=== rustc-host ==="
"$RUSTC" --version || true
ls -lh "$RUSTC"

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
  --cap-lints allow --cfg procmacro_stub -A unexpected_cfgs

echo "=== core done; artifacts ==="
ls -lh "$DEPS"/libcore-* 2>/dev/null || true

echo "=== compiling compiler_builtins STUB ==="
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

echo "=== compiler_builtins done; artifacts ==="
ls -lh "$DEPS"/libcompiler_builtins-* 2>/dev/null || true

echo "=== self-test: compile a no_std prelude snippet against produced sysroot ==="
PT=/tmp/semos-sysroot-selftest.rs
printf '#![no_std]\npub fn f() -> Option<i32> { Some(41).map(|x| x + 1) }\npub fn g(s: &str) -> usize { s.len() }\n' > "$PT"
"$RUSTC" --edition 2021 --crate-type lib --target x86_64-unknown-none \
  --extern "core=$DEPS/libcore-53344cc650ffcdf9.rmeta" \
  --extern "compiler_builtins=$DEPS/libcompiler_builtins-fb74582bf62b1baa.rmeta" \
  -L "dependency=$DEPS" --cfg procmacro_stub -A unexpected_cfgs \
  --emit=metadata -o /tmp/semos-sysroot-selftest.rmeta "$PT"
echo "SELFTEST OK: rustc-host core + compiler_builtins load and resolve the prelude"
