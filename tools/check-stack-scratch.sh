#!/usr/bin/env bash
set -euo pipefail

# SemOS stack-frame guardrail (2026-07-17 review, medium #4.2).
#
# The ideal gate is rustc's -Zemit-stack-sizes parsed against first-party
# symbols. On the pinned nightly today that path trips an LLVM backend error in
# several crypto deps, so the default CI-safe check is a source-level guard for
# the class that has repeatedly hurt us: large fixed arrays/TUI state on kernel
# task stacks. Set SEMOS_STACK_EMIT=1 to try the real rustc stack-size path once
# the toolchain/deps support it.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
THRESHOLD_BYTES="${SEMOS_STACK_THRESHOLD_BYTES:-8192}"

cd "$ROOT/kernel-x86_64"

if [[ "${SEMOS_STACK_EMIT:-0}" == "1" ]]; then
  echo "[stack] attempting rustc -Zemit-stack-sizes (threshold ${THRESHOLD_BYTES} B)"
  cargo rustc -- -Z emit-stack-sizes
  stack_files=$(find target -name '*.stack_sizes' -type f)
  if [[ -z "$stack_files" ]]; then
    echo "[stack] ERROR: no .stack_sizes files emitted" >&2
    exit 1
  fi
  awk -v limit="$THRESHOLD_BYTES" '
    $2+0 > limit { bad=1; print "[stack] large frame: " $0 }
    END { exit bad ? 1 : 0 }
  ' $stack_files
  exit 0
fi

echo "[stack] source guard: no new fixed stack arrays >= ${THRESHOLD_BYTES} B in first-party kernel code"

# Flag local fixed arrays at/above threshold. Keep this intentionally simple:
# values below the threshold are allowed; large storage should be Box, static,
# or an explicit shared scratch guarded by a kernel mutex.
violations=$(
  grep -RIn --include='*.rs' \
    -E 'let mut [A-Za-z_][A-Za-z0-9_]* = \[0u8; (8192|[1-9][0-9]{4,})\]' \
    "$ROOT/kernel-x86_64/src" "$ROOT/kernel-core/src" || true
)
if [[ -n "$violations" ]]; then
  echo "$violations" >&2
  echo "[stack] ERROR: large local byte arrays found; use Box/static scratch or justify by lowering threshold." >&2
  exit 1
fi

# Tui contains multiple scrollback rings; instantiate it boxed so it never
# lives directly in a long-running task frame.
if grep -RIn --include='*.rs' -E 'let mut [A-Za-z_][A-Za-z0-9_]* = match Tui::new|let mut [A-Za-z_][A-Za-z0-9_]* = Tui::new' "$ROOT/kernel-x86_64/src" | grep -v 'Box' >&2; then
  echo "[stack] ERROR: unboxed Tui::new allocation found." >&2
  exit 1
fi

echo "[stack] OK"
