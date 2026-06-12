#!/usr/bin/env bash
# M27 DEMO 80 option B — regenerate the patched library source tree (sysroot-src/)
# that build-core.sh compiles into core + compiler_builtins for x86_64-unknown-none.
#
# WHY a patched copy: the vendored rustc (built into rustc-host) is a SemOS-
# customized compiler that lines up with NO single upstream nightly's library —
# it lacks `rustc_do_not_implement_via_object` + `eii_extern_target` yet already
# renamed macro-transparency `semitransparent`->`semiopaque` and expanded the
# `offload`/`va_copy` intrinsic signatures. The closest base is nightly-2025-12-23
# (rustc 1.94.0-nightly, 4f14395c3); sysroot-src.patch carries the ~6 deltas that
# bring its `core` in line with the vendored compiler. (1.95/2026-02-01 core is too
# new: 223 const-trait errors; pristine 1.94 core: 36 attr/intrinsic errors; patched: 0.)
#
# sysroot-src/ is .gitignore'd (47 MB, regenerable). Run this once before build-core.sh.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
BASE_TOOLCHAIN="${1:-nightly-2025-12-23-x86_64-pc-windows-msvc}"
SRC="$HOME/.rustup/toolchains/$BASE_TOOLCHAIN/lib/rustlib/src/rust/library"

if [ ! -d "$SRC/core" ]; then
  echo "ERROR: rust-src for $BASE_TOOLCHAIN not found at $SRC" >&2
  echo "Install: rustup component add rust-src --toolchain $BASE_TOOLCHAIN" >&2
  exit 1
fi

echo "=== copying library tree from $BASE_TOOLCHAIN ==="
rm -rf "$HERE/sysroot-src"
mkdir -p "$HERE/sysroot-src"
cp -r "$SRC" "$HERE/sysroot-src/library"

echo "=== applying sysroot-src.patch (vendored-compiler deltas) ==="
( cd "$HERE/sysroot-src/library/core" && patch -p1 --binary < "$HERE/sysroot-src.patch" )

echo "=== done. Now run: bash build-core.sh ==="
