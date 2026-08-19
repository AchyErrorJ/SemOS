#!/usr/bin/env bash
# One-shot: build the special user programs, then kernel + images.
# Log: /tmp/finish-build.log
export PATH="$HOME/.cargo/bin:$PATH"
cd /home/aesir/SemOS
exec > /tmp/finish-build.log 2>&1
set -x

cd user-programs/cg-clif-hello && cargo build --release; echo "cg-clif-hello=$?"
cd /home/aesir/SemOS/user-programs/semos-cc-hello && cargo build --release; echo "semos-cc-hello=$?"
cd /home/aesir/SemOS/user-programs/semos-cc && cargo build --release; echo "semos-cc=$?"
cd /home/aesir/SemOS/user-programs/semos-rustc && RUSTFLAGS='--cap-lints=allow' cargo build --release; echo "semos-rustc=$?"

ls -la /home/aesir/SemOS/user-programs/semos-rustc/target/x86_64-unknown-none/release/semos-rustc
ls -la /home/aesir/SemOS/user-programs/semos-cc/target/x86_64-unknown-none/release/semos-cc
ls -la /home/aesir/SemOS/user-programs/semos-cc-hello/target/x86_64-unknown-none/release/semos-cc-hello
ls -la /home/aesir/SemOS/user-programs/cg-clif-hello/target/x86_64-unknown-none/release/cg-clif-hello

cd /home/aesir/SemOS/kernel-x86_64 && cargo build --release; echo "kernel=$?"
cd /home/aesir/SemOS/x86_64-runner && cargo run --release; echo "runner=$?"
echo DONE
