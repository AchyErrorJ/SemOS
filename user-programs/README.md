# Semantic OS user programs

Real Rust crates that compile to ELF binaries the kernel loads and runs in
Ring 3 via `SYS_SPAWN`. Replaces the hand-assembled byte-string ELFs in
`kernel-core/src/process/elf.rs`.

## Build

Each program is its own `no_std`, `no_main` crate. Build manually before
the kernel:

```sh
cd user-programs/hello
cargo build --release
```

Output ends up at `target/x86_64-unknown-none/release/<crate-name>`.

The kernel embeds the binary via `include_bytes!` from a fixed relative
path and registers it with the in-memory ramfs at boot, so the kernel
build will fail loudly if the user binary is missing.

## What's there

- `hello/` — minimal program: `SYS_WRITE("Hello from real Rust ELF!\n")` then
  `SYS_EXIT(0)`. Replaces the hand-assembled `hello.elf`. Used as DEMO 0.
- `snake/` — first fullscreen Ring-3 game: claims screen+keyboard
  (`SYS_FB_CLAIM`), steers with raw key events (`SYS_KB_POLL`, arrows/WASD,
  press+release, PS/2 + USB), blits dirty 16×16 cells only (`SYS_FB_BLIT`),
  paced by `SYS_FB_WAIT_VBLANK` with a tick-sleep fallback. ESC/Ctrl+C/q
  quits; the claim auto-releases on exit (`reset_tty_flags`).

## Layout requirements

The kernel ELF loader (`kernel-core/src/process/elf.rs`) expects:

- `ET_EXEC` with `PT_LOAD` segments at `vaddr >= 0x400000` (`USER_CODE_BASE`).
- The user's stack is mapped separately at `0x7FFFFFF000` (`USER_STACK_TOP`)
  by `spawn_from_elf`.
- Entry point is read from the ELF header.

Each program's `link.ld` controls the load layout. `hello/link.ld` puts
text at `0x400000` and follows with rodata/data/bss. `build.rs` passes
the script path to `rust-lld` with `-T`.

## Why no workspace?

The other kernel crates (`kernel-core`, `kernel-x86_64`, `x86_64-runner`)
each opt out of the parent workspace with a `[workspace]` block. User
programs need a different target (`x86_64-unknown-none`), different
build-std settings, and a custom linker script — easier to keep them
fully separate than to coerce the workspace.

## Adding a new program

1. Copy `hello/` to `user-programs/<name>/`, rename in `Cargo.toml` and `[[bin]]`.
2. Edit `src/main.rs` for your logic. Add syscall stubs as needed (see
   `kernel-core/src/syscall/mod.rs::numbers` for the syscall numbers).
3. `cargo build --release` in the new directory.
4. In `kernel-x86_64/src/main.rs`, add another `include_bytes!` + `fs.add(...)`
   block alongside the existing `hello-rs.elf` registration.
5. Have `init_loader_task` (in `kernel-x86_64/src/main.rs`) call
   `spawn_named_at("<name>.elf", tier)` to actually run it.
