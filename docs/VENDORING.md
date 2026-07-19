# Vendored Trees — Index

One line per vendored/third-party tree in this repo, with its brief and
upstream pin. Rule (per the 2026-07-17 code review): **every vendored
tree has a brief; every brief records upstream state and an update
policy.** If you add a tree, add a brief and a line here.

| Tree | Upstream | Pin | Brief |
|------|----------|-----|-------|
| `kernel-core/vendor/embedded-tls/` | drogue-iot/embedded-tls | 0.17/0.18 line, locally patched (see Cargo.toml comment) | [EMBEDDED_TLS_VENDORING_BRIEF.md](EMBEDDED_TLS_VENDORING_BRIEF.md) |
| smoltcp (crates.io dep, not in-tree) | smoltcp-rs/smoltcp | 0.11, `default-features = false` | [SMOLTCP_VENDORING_BRIEF.md](SMOLTCP_VENDORING_BRIEF.md) |
| `compiler/vendor/` (44 crates) | Cranelift (wasmtime) + support crates | cranelift-* 0.122.0 | [CRANELIFT_VENDORING_BRIEF.md](CRANELIFT_VENDORING_BRIEF.md) |
| `user-programs/semos-rustc/` (whole rustc tree) | rust-lang/rust | nightly-2026-02-01 (matches root toolchain pin) | [SEMOS_RUSTC_VENDORING_BRIEF.md](SEMOS_RUSTC_VENDORING_BRIEF.md) |
| `iwlwifi-7260-17.ucode` (firmware blob) | Intel / linux-firmware | -17 (T540p iwlwifi-7260) | License: [`LICENCE.iwlwifi_firmware`](../LICENCE.iwlwifi_firmware) (redistribution terms — must ship alongside the blob) |
