#!/usr/bin/env bash
# Build kernel + boot images; verify all embedded ELFs exist first.
export PATH="$HOME/.cargo/bin:$PATH"
set -u
cd "$(dirname "$0")/.."

missing=0
# semos-cc-hello is not a cargo crate: it's emitted by the host tool in
# compiler/ (cd compiler && cargo run) and a 0-byte placeholder suffices
# for the kernel build (upstream ships one too).
for p in hello sem-demo exfil-demo ptr-guard-test thread-demo sync-demo cg-clif-hello semos-cc hello-std vec-demo std-demo spawn-demo net-demo fb-demo sem-sh semos-rustc; do
    f="user-programs/$p/target/x86_64-unknown-none/release/$p"
    if [ ! -f "$f" ]; then
        echo "MISSING: $f"
        missing=1
    fi
done
if [ ! -f compiler/out/semos_cc_hello.elf ]; then
    echo "MISSING: compiler/out/semos_cc_hello.elf"
    missing=1
fi
[ "$missing" = "1" ] && exit 1

( cd kernel-x86_64 && cargo build --release ) || { echo KERNEL_FAILED; exit 1; }
( cd x86_64-runner && cargo run --release ) || { echo RUNNER_FAILED; exit 1; }
echo BUILD_ALL_OK
