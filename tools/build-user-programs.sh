#!/usr/bin/env bash
# Build every user program (release) so the kernel's include_bytes! resolve.
export PATH="$HOME/.cargo/bin:$PATH"
set -u
cd "$(dirname "$0")/.."
fail=0
for d in user-programs/*/; do
    [ -f "$d/Cargo.toml" ] || continue
    p=$(basename "$d")
    # semos-rustc (vendored rustc port) builds separately: it needs
    # RUSTFLAGS='--cap-lints=allow' or host rustc ICEs rendering lint
    # warnings from the vendored sources.
    [ "$p" = "semos-rustc" ] && continue
    echo "=== $p ==="
    if ! ( cd "$d" && cargo build --release ); then
        echo "FAIL $p"
        fail=1
    fi
done
exit $fail
