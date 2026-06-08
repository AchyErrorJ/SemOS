#!/usr/bin/env bash
# M27 Stage H iter 1 — alloc-prelude sweep
#
# For each .rs file in rustc_mir_transform / rustc_mir_build /
# rustc_hir_analysis / rustc_passes, insert a cfg-gated alloc-prelude
# import line after the file's leading `//!` doc-comment / `#![attr]` /
# blank-line block. This makes bare `Vec`/`String`/`Box`/`ToString`/
# `ToOwned` resolve on `target_os = "none"` builds where the std
# prelude isn't auto-injected.

set -euo pipefail

PRELUDE='#[cfg(target_os = "none")] use alloc::{boxed::Box, string::{String, ToString}, vec::Vec, borrow::ToOwned};'
PRELUDE_TAG='use alloc::{boxed::Box, string::{String, ToString}, vec::Vec, borrow::ToOwned}'

ROOT="${1:-F:/Software/ArmKernel3/user-programs/semos-rustc/vendor-rustc-src/compiler}"

for crate in rustc_passes rustc_mir_transform rustc_mir_build rustc_hir_analysis rustc_hir_typeck rustc_interface rustc_driver_impl; do
  count=0
  while IFS= read -r f; do
    # Skip if the exact import is already present anywhere in the file.
    if grep -qF "$PRELUDE_TAG" "$f"; then continue; fi

    awk -v prelude="$PRELUDE" '
      BEGIN { inserted = 0; in_header = 1; in_block_comment = 0 }
      {
        if (in_header && !inserted) {
          if (in_block_comment) {
            print
            if ($0 ~ /\*\//) { in_block_comment = 0 }
            next
          }
          # Stay in header for inner attrs, inner doc comments, blank lines,
          # and line / block comments at the top of the file. /*! ... */
          # is a block-style inner doc comment and absolutely must precede
          # any item; same for #![attr].
          if ($0 ~ /^#!\[/) { print; next }
          if ($0 ~ /^\/\/!/) { print; next }
          if ($0 ~ /^\/\//) { print; next }
          if ($0 ~ /^[[:space:]]*$/) { print; next }
          if ($0 ~ /^\/\*/) {
            print
            # If the block comment closes on the same line, stay in header.
            if ($0 !~ /\*\//) { in_block_comment = 1 }
            next
          }
          # Header ended at this regular item — insert the prelude first.
          print prelude
          inserted = 1
          in_header = 0
        }
        print
      }
      END { if (!inserted) print prelude }
    ' "$f" > "$f.sweep.tmp" && mv "$f.sweep.tmp" "$f"

    count=$((count + 1))
  done < <(find "$ROOT/$crate/src" -name "*.rs" -type f)
  echo "$crate: $count files updated"
done
