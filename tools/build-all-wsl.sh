#!/usr/bin/env bash
# Full WSL build: user programs -> kernel -> boot images.
export PATH="$HOME/.cargo/bin:$PATH"
set -u
cd "$(dirname "$0")/.."

echo "== user programs =="
bash tools/build-user-programs.sh || { echo "USER_PROGRAMS_FAILED"; exit 1; }

echo "== kernel =="
( cd kernel-x86_64 && cargo build --release ) || { echo "KERNEL_FAILED"; exit 1; }

echo "== boot images =="
( cd x86_64-runner && cargo run --release ) || { echo "RUNNER_FAILED"; exit 1; }

echo "BUILD_ALL_OK"
