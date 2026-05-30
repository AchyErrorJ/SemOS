#!/usr/bin/env bash
# patch-nostd.sh — apply the standard "drop std from default features +
# add [workspace] header" patch to a vendored Cranelift dep that has its
# extern crate std gated behind a `std` feature.
#
# Use case: many transitive deps in compiler/vendor/ have
#   [features] default = ["std"]
#   #[cfg(feature = "std")] extern crate std;
# Switching `default` from `["std"]` to `[]` builds them no_std for our
# x86_64-unknown-none target, and stamping `[workspace]` at the top of
# Cargo.toml prevents cargo's upward workspace traversal from picking up
# semos-cc/Cargo.toml.
#
# Usage:  ./patch-nostd.sh <vendor-subdir>
#         e.g. ./patch-nostd.sh vendor/indexmap-2.14.0
#
# This is intentionally narrow — it ONLY does the two-line edit + checksum
# update. Anything weirder needs a hand patch.

set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 <vendor-crate-dir>" >&2
    exit 64
fi

crate_dir=$1
manifest=$crate_dir/Cargo.toml
checksum=$crate_dir/.cargo-checksum.json

if [[ ! -f "$manifest" ]]; then
    echo "no Cargo.toml at $manifest" >&2
    exit 65
fi
if [[ ! -f "$checksum" ]]; then
    echo "no .cargo-checksum.json at $checksum" >&2
    exit 65
fi

old_hash=$(sha256sum "$manifest" | awk '{print $1}')

# 1. Stamp [workspace] above [package] if not already present.
if ! grep -q '^\[workspace\]' "$manifest"; then
    sed -i '0,/^\[package\]/s//# D.2 port: own workspace root.\n[workspace]\n\n[package]/' "$manifest"
fi

# 2. Drop std from default features.
if grep -q 'default = \["std"\]' "$manifest"; then
    sed -i 's/^default = \["std"\]$/default = []/' "$manifest"
fi

new_hash=$(sha256sum "$manifest" | awk '{print $1}')

if [[ "$old_hash" == "$new_hash" ]]; then
    echo "no changes applied to $manifest" >&2
    exit 0
fi

# 3. Swap the Cargo.toml hash in the checksum json. Single-line json so a
#    simple sed works; we anchor on the exact old hash to avoid clobbering
#    other entries.
sed -i "s/\"Cargo.toml\":\"$old_hash\"/\"Cargo.toml\":\"$new_hash\"/" "$checksum"

# Verify the swap landed.
if ! grep -q "\"Cargo.toml\":\"$new_hash\"" "$checksum"; then
    echo "checksum swap failed — old hash not found in $checksum" >&2
    exit 66
fi

echo "patched $crate_dir  (Cargo.toml $old_hash -> $new_hash)"
