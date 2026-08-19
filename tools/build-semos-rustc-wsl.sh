#!/usr/bin/env bash
# semos-rustc build with memory watchdog. Logs to $HOME (persistent).
export PATH="$HOME/.cargo/bin:$PATH"
cd /home/aesir/SemOS/user-programs/semos-rustc

# Memory watchdog: append free/toplne every 5s so a VM death leaves a trail.
( while true; do
    echo "--- $(date +%H:%M:%S) ---"
    free -m | head -2
    ps -eo rss,comm --sort=-rss | head -4
  done > /home/aesir/mem-watch.log 2>&1 ) &
WATCH=$!

exec > /home/aesir/semos-rustc-build.log 2>&1
set -x
# NOTE: --cap-lints=allow in RUSTFLAGS breaks cargo's target-info probe
# ("output of --print=file-names missing"). -Awarnings achieves the same
# goal (no lint-warning rendering -> no host rustc ICE) without breaking it.
RUSTFLAGS='-Awarnings' cargo build --release -j 4
echo "semos-rustc exit=$?"
kill $WATCH
ls -la target/x86_64-unknown-none/release/semos-rustc
