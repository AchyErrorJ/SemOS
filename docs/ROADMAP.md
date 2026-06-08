# Semantic OS — Roadmap

**No dates here.** This document tracks **what must happen** and **what unblocks what**. Time is a side effect of doing things in the right order, not a thing to plan in.

> **Next agent:** start with the latest handoff — [`HANDOFF_2026-05-22.md`](HANDOFF_2026-05-22.md) — for current state, build/test gotchas, and suggested next steps.

When you finish a milestone, flip its checkbox in this file and update the [Project memory file](../../../Users/jerro/.claude/projects/F--Software-ArmKernel3/memory/project_semantic_os_kernel.md). When a milestone reveals new sub-work, add it to that milestone's checklist or split it into a follow-up milestone.

Phase 8 (network → first remote LLM call) is closed. See [`PHASE_8_ROADMAP.md`](PHASE_8_ROADMAP.md) for that phase's historical detail. The current frontier is Phase 9.

---

## How to read this

```
M0  [✅] = done and committed, exercised by a boot-time DEMO
M0  [🔨] = in progress on a branch
M0  [⏸️] = paused, blocker known and documented
M0  [  ] = pending; checklist below is the contract for "done"
```

A milestone is **done** when:
1. Code compiles clean in both `kernel-core` and `kernel-x86_64`.
2. A boot-time DEMO `N` in `kernel-x86_64/src/main.rs` emits `PASS:`/`FAIL:` lines that grep cleanly out of the QEMU serial log.
3. The project memory file is updated with the lessons learned (not just the fact that it shipped).
4. Follow-up work the milestone uncovered is captured as a separate milestone or task, not left implicit.

---

# Done (status snapshot)

| Phase | What landed |
|---|---|
| 1-6 | GDT/TSS, IDT, paging, APIC, framebuffer console, PCI bus, VirtIO block, snapshot persistence |
| 7 | Streaming LLM syscalls, security policy framework, context-aware redaction, network LLM provider (loopback), user identity + isolation |
| 8 | Crypto stack (SHA-256, HMAC, HKDF, X25519, ECDSA P-256, ChaCha20-Poly1305), virtio-net, smoltcp, TcpStream, RDRAND, embedded-tls vendored + crypto-shim, SPKI-pinning TlsVerifier, TLS-backed NetworkTransport, **first outbound HTTPS round-trip to api.anthropic.com** |
| 9 | Path namespace (M1), RTC + wall_clock (M2), FS Stage 3 syscalls (M4), FS Stage 2 persistence (M5) **with cross-boot vdisk verification**, USB driver (M3) **fully unblocked 2026-05-19 by main-kernel-stack bump (#42 fix)** |
| 14 prep (Tier 1) | Cranelift + cg_clif vendor placeholders + briefs (agent), heap allocator, argv/envp passthrough, per-process env+CWD |
| 14 prep (Tier 2) | SYS_FSYNC, SYS_RENAME, SYS_TRUNCATE, SYS_STATX, FWRITE>256 B via heap-Allocated ObjectContent (M26 "first compile" unblocked) |
| 14 prep (Tier 3) | SYS_THREAD_SPAWN/JOIN (kernel + Ring-3 same-AS), SYS_FUTEX_WAIT/WAKE, SYS_WAITNB, SCHEDULER_TICK_HZ const (parallel/threaded rustc unblocked) |
| 14 (M25 substantial) | `semos-std` crate: `#[global_allocator]` (Vec/String/Box), `io::{Read,Write}`, `fs::File`/`OpenOptions`, `env`, `sync::{Mutex,Once}`, `thread::spawn`+`JoinHandle<T>`, `process::Command` (spawn+wait), argv. hello-std/vec-demo/std-demo/spawn-demo run Ring 3 (DEMO 29–32). **Build at `opt-level=0` only** — any optimization miscompiles the syscall path (#54). Still missing: `net`, full `path`/`time` |
| 9/10 graphics+net | M6 framebuffer drawing API (DEMO 35); M13 HTTP chunked decoder (DEMO 33); M12 DNS resolver (DEMO 34, wall-clock wait + retransmit) |
| Structural | #41 real guard pages between all task stacks; #54 std-shim opt-level workaround; #55 sequential Ring-3 spawn; per-task kernel stack → 128 KiB. **task#40 / #56 FIXED (`8c2cb21`): context_switch was a *torn control transfer* (`popfq; jmp` window where a timer preempted mid-switch) — now an atomic IRETQ. Closes the whole layout-sensitivity / iret-RIP-corruption family.** |
| Cleanup 2026-05-22 | All HANDOFF open issues closed: **#55 re-verified** (`72a002f`, DEMO 28 → 0x2700); **DEMO 27 timing flake de-flaked** (`78ae59e`, poll-not-sleep); **M7/M8 wired into `tty::TtyConsole`** (`78ae59e`, DEMO 39 — the M19 renderer). Suite **132 PASS / 0 FAIL / 0 #DF** with `-netdev`. |
| M19 slice 1 2026-05-22 | **TTY stdin + ANSI** (`716eafd`, DEMO 40): cooked-mode line discipline (`SYS_READ` fd 0, Backspace), `AnsiTty` (SGR color / 2J / K / H) over the TTF console. Suite **135 PASS**. |
| M19 per-process stdio 2026-05-22 | **Full per-process FD-table refactor** (`673d948`+`efd444e`+`21dbd8f`, DEMO 41/42): all FDs (console/pipe/path/ramfs) live in the process `FdTable`; global PATH_FDS/PIPE_FDS deleted; stdio routable (dup2→pipe) + inherited on spawn; slot-keyed resolution + stale-task_id fix. Suite **140 PASS**. |
| M19 DONE 2026-05-22 | **TTY complete** (`9787cb7` line editing + history, `93ca47c` scrollback; DEMO 43/44): in-line cursor + arrow keys (PS/2 0xE0 + USB HID → ESC[ABCD) + 8-line history; TtyConsole scrollback ring. Suite **145 PASS / 0 FAIL / 0 #DF**. M19 ✅ — next is M20 native shell. |
| M20 stage A 2026-05-23 | **sem-sh native shell** (`5398720`, DEMO 45): REPL reading cooked stdin (M19) + script mode, quote-aware parser, builtins (echo/pwd/cd/exit), external ELF exec via Command. Suite **147 PASS**. Gotcha: new user crate must be non-PIE (build.rs+link.ld) or println crashes — see feedback memory. |
| -netdev DEMO 15 hang FIXED 2026-05-23 | `ad540dd`: embedded-io TcpStream read/write now bounded by a 10 s idle deadline. The TLS handshake's ServerHello read spun forever when SLIRP raced port 1 to ESTABLISHED then went silent — hung the boot 350 s+. 4 consecutive -netdev boots clean after. |
| M20 stage B 2026-05-23 | **sem-sh fs builtins + $VAR** (`b81251d`, DEMO 45): cat/ls/which/env builtins + `$VAR` expansion (inherited env). Suite **148 PASS**. |
| M20 DONE 2026-05-23 | **sem-sh redirection + pipes** (`96fbaf9`, DEMO 46): `>`/`<` redirection + `|` pipelines (sequential v1). Kernel: SYS_WRITE→handle_fwrite routing + positional Path writes. Suite **150 PASS / 0 FAIL / 0 #DF**. M20 ✅ — shell complete. |
| M19/M20 hardening 2026-05-23 | **pipe-end refcounting** (`0b4a6bb`) + **true `>>` append** (`763188a`) + **concurrent pipes** (`9d89dbb`: WOULDBLOCK reads + spawn-inherit refcount + exit-time FD cleanup + concurrent shell spawn). Suite **152 PASS / 0 FAIL / 0 #DF**. |
| M22 stage A 2026-05-23 | **Claude agent core** (`34ef9ee`, DEMO 47): `agent.rs` — Messages-API request framing + response parse (text + tool_use) + tool dispatch (read_file/write_file). No network. Suite **157 PASS**. |
| M22 stage B + net fix 2026-05-23 | **agent live TLS round-trip** (`9da1f51`, DEMO 48): build_http_request + send_over_tls → HTTP 401 from api.anthropic.com (proves framing+TLS send/recv). Required **TcpStream reconnect fix** (`efd8c3c`: free smoltcp socket on Drop — a successful connection's close leaked it, hanging the next connect). Also: DEMO 15 stall DIAGNOSED — timeout mechanism sound (ticks advances), residual flake is in net::poll for the bogus port-1 target only; real TLS (16/48) reliable. Suite **139 PASS / 0 FAIL / 0 #DF** (the "158" figure in this row's first draft was a miscount; verified 139 by booting the committed HEAD). Stage C: key + loop + bash/grep/glob + TUI. |
| FS large files — Model A, plan + stage 1 2026-05-27 | **Decision (talked through):** fix the FS for demanding design files via **Model A** — files live in RAM (now up to the 512 MB/pool ceiling), `as_bytes()`'s contiguous-`&[u8]` contract preserved (so the ~11 consumers + `spawn`'s ELF parse don't change), persisted to disk. **Model B** (disk-backed extents, content not resident, multi-GB out-of-core) **deferred until the hardware arrives** — same tier as the deferred GPU. **Per-file size: FIXED CEILING** (not "whatever frames are free"): a single file must never drain a tier pool — that would starve the app's own working set + other files into an OOM cascade; a predictable bound is good hygiene and a one-constant tune. **Stages:** (1) lift the heap-bound cap [done — see next row]; (2a) frame-backed content via a contiguous-frame allocator + `phys_to_virt` (escape the 16 MiB heap → 100s-of-MB in-RAM files, fixed ceiling ~128 MB so one file ≤ ¼ pool); (2b) per-file disk block allocator + persistence so the snapshot is metadata-only and large files persist (today's monolithic snapshot duplicates content into one buffer — the real blocker past a few MB). |
| FS large files — stage 1 2026-05-27 | **8× the per-file cap, persistable.** `MAX_FILE_CONTENT` 256 KiB→2 MiB and `MAX_SNAPSHOT_BYTES` 1 MiB→4 MiB (heap-backed scratch already), staying within the 16 MiB heap. Content is still one heap `Allocated` blob (contiguous → `as_bytes()` unchanged). DEMO 60 extended: installs a synthesized ~1 MiB file (`/apps/bigdoc`, > the old 256 KiB cap *and* the old 1 MiB snapshot) and verifies it survives reboot byte-pattern-intact. Heap-bound (~2 MiB/file) until stage 2a frame-backs content. Two-boot test (non-net): boot 2 → all three files restored incl. **/apps/bigdoc 1 MiB pattern-intact**; DEMO 26 oversize-rejection re-derived from `MAX_FILE_CONTENT+4 KiB` (no longer drifts). **145 PASS / 0 FAIL / 0 #DF.** Known follow-up: DEMO 5 (raw-snapshot demo) shares virtio0 sector 0 with the namespace snapshot and re-seeds it when its small read buffer hits the 1.1 MB namespace header — harmless for a 2-boot test (namespace already in RAM) but clobbers on-disk persistence on a 3rd boot; give DEMO 5 its own region or skip re-seed when a namespace magic is present. |
| Interactive mode 2026-05-27 | **the OS is drivable at the keyboard** (`30c5687` + `32a798c`). Cargo feature `interactive` (default off, so headless CI still runs the 60 demos + idles): boot ends by dropping into the live `sem-sh` shell. Fix that made typing work — the USB HID event ring is only drained when polled, and nothing polled it while we waited, so the shell's `SYS_READ` never saw keystrokes; now the wait loop pumps the ring into the line discipline (edge-detected so held keys don't repeat) with explicit framebuffer echo (`input_push` echoes only to serial). New shell builtins: `help` (lists builtins) and `agent` (launches the split-pane TUI as a Claude chat loop via **SYS_AGENT 112** → `Platform::run_agent_tui` → `agent::run_interactive`; `AGENT_TUI_ACTIVE` pauses the shell pump while the TUI owns the keyboard; `framebuffer::clear()` for overlay teardown). Validated windowed via QEMU `sendkey`: typed commands run, `help`/`agent` work, agent prompt echoes + `exit` clears back to the shell; Backspace at an empty prompt no longer eats the prompt. Default build still 141 PASS / 0 FAIL. **Next: native editor (M21).** |
| xHCI CSZ=1 2026-05-28 | **64-byte context layout for Intel chipsets (`8821df1`).** Previous boot-time REJECT on CSZ=1 is gone. `InputContext`/`DeviceContext` are now raw byte buffers at max stride (2112/2048 B, align(64)); accessors (`input_ctrl_mut`, `slot_mut`, `ep_mut(idx)`, `slot_read`, `ep_read(idx)`) compute offsets via `CTX_SIZE` set once at xhci bring-up (`set_ctx_size(if csz1 {64} else {32})`). 32-byte `SlotContext`/`EndpointContext` data formats unchanged — only their placement varies. **`TIMER_TICKS` is also now `AtomicU64`** (was `spin::Mutex<u64>`, latent ISR-vs-reader deadlock found while building the M10 watchdog). qemu-xhci (CSZ=0) regression-clean: 165 PASS / 0 FAIL / 0 #DF. Unblocks USB enumeration on the T540 HM87 (Intel CSZ=1). |
| M10 watchdog + audit 2026-05-28 | **Pre-flight v1 (`d77ba87`).** Audit: framebuffer-only diagnostics already in place (`serial::_print` mirrors), RTC century byte already handled, panic handler routes through both. New: `idle_with_heartbeat` prints `[heartbeat] kernel reached idle — ticks=N` at end of boot as proof-of-life on metal. Latent bug fixed: `TIMER_TICKS spin::Mutex<u64>` → `AtomicU64`, eliminating the ISR-vs-reader deadlock pattern. **Top M10 follow-up is xHCI CSZ=1 support** — Intel chipsets (incl. the T540 HM87) need 64-byte contexts; current code rejects at bring-up. Blocks USB on real Intel hardware (PS/2 keyboard still works). |
| USB Mass Storage v1 2026-05-28 | **USB stick CBW/CSW + SCSI (DEMO 68, `3a4587b`).** Protocol layer for reading a USB stick on the T540: class IDs 0x08/0x06/0x50 (SCSI BBB), 31-byte CBW build with 'USBC' signature + zero-padded CBWCB, 13-byte CSW parse (tag/residue/status), SCSI CDB builders for INQUIRY / READ CAPACITY (10) / READ (10) / WRITE (10) / TEST UNIT READY, INQUIRY + READ CAPACITY response parsers. Validated against six canned-byte checks. Hardware-ready for live xHCI bulk-endpoint TX/RX (same gating as CDC-ECM). |
| M27 Stage H iter 1 — Phase 5a alloc-prelude sweep 2026-06-08 | **5 of 6 broken `rustc_*` crates closed target-side; rustc_interface 416 → 37 (commit `fbe4d33`).** Phase 3 ports left bare `Vec`/`String`/`Box`/`ToString`/`ToOwned` unqualified in ~243 files across rustc_passes, rustc_mir_build, rustc_mir_transform, rustc_hir_analysis, rustc_hir_typeck, rustc_interface — bites only on target builds where the std prelude isn't auto-injected. New tooling `tools/m27-alloc-prelude-sweep.sh` awk-scans each .rs file, skips the `//!`/`/*!...*/`/`#![...]`/blank/`//` header block, and inserts a cfg-gated `use alloc::{boxed::Box, string::{String, ToString}, vec::Vec, borrow::ToOwned};` right after. Three other mechanical sweeps stacked on top: (a) strip ~190 `#[instrument(...)]` + `#[tracing::instrument(...)]` attribute lines (tracing-attributes proc-macro doesn't compile target-side, attribute is diagnostic-only); (b) body-level `std::{iter,mem,fmt,cmp,cell,ops,hash,marker,num,any,convert,slice}` → `core::*`; (c) `use core::borrow::Cow` walked back to `use alloc::borrow::Cow`. Hand fixes for residuals: hashbrown's `Equivalent` vs std's `Borrow` trait bound (drop a layer of `&`), cfg-gated `eprint!`/`thread_local!`/`#![feature(file_buffered)]`/diagnostic MIR dumps, `Iterator::join` → `.collect::<Vec<_>>().join(", ")`, `Entry::Occupied/Vacant` matches now go through `rustc_data_structures::fx::StdEntry` alias so generic arity matches FxHashMap's hashbrown backing (5-type Entry vs std's 4-type). 6 Cargo.toml additions for `rustc_error_messages` (needed by `rustc_fluent_macro::fluent_messages!` expansion) + 2 for `hashbrown`. rustc_interface's Phase 3 port was skipped — added the missing `#![cfg_attr(target_os = "none", no_std)]` + `#[macro_use] extern crate alloc;` header. Total error reduction: 1436 → 37 (-97%). Iter 2 picks up rustc_interface's residual fs/io/env/sync body refs + the `back::link`/`back::archive` cfg-gates per §1.7. |
| M27 Stage G iters 6-10 — Cranelift no_std + cg_clif + semos-rustc ELF 2026-06-07 | **Full Cranelift codegen pipeline target-buildable + `semos-rustc` emits a 6.1 MB ELF on x86_64-unknown-none (commits `7131356` → `c65d3ae`).** Iter 6: `cranelift-codegen` 430 → 0 via OnceLock shim (race-then-leak `AtomicU8 + UnsafeCell<MaybeUninit<T>>`) + libm-backed FloatNoStd trait (powi/sqrt/ceil/floor/trunc/round_ties_even routed to libm 0.2.16) + build.rs post-processing of ISLE-generated + opcode-generator files (rewrite `std::{marker::PhantomData, slice::from_ref, ops::Deref, vec::Vec, boxed::Box, string::String, default::Default, iter, cmp, fmt, mem, hash}` to core/alloc at write time, cheaper than forking cranelift-isle) + `rustc-hash` no_std `FxHashMap`/`FxHashSet` aliases backed by hashbrown 0.15. `cranelift-frontend` 7 → 0, `cranelift-module` 10 → 0 (cfg-gates + ExceptionTableData::new generic IntoIterator). `cranelift-object` 96 → 0 (first-time no_std port: `#![no_std]` + `#[macro_use] extern crate alloc` + hashbrown HashMap + `core::mem`/`core::cmp::max` + add `macho` feature for the object crate). Iters 7-9 close `rustc_codegen_cranelift` 653 → 0: cg_clif gets `#![cfg_attr(target_os = "none", no_std)]` + `#[macro_use] extern crate alloc`, drops `extern crate rustc_driver` (it was a host-rustc duplication-prevention dep), declares 17 `rustc_*` path deps + semos-std as target-only dep, gates `global_asm` / `toolchain` / `concurrency_limiter` / `driver::aot` / `cranelift_native::builder_with_options` to host-only, replaces `println!`/`info!`/`env::var`/`thread::panicking` with cfg-gated no-ops on target. Cranelift API forward-compat stubs added to bridge our 0.122.0 vs cg_clif's newer-API expectations: `ir::ExceptionTableItem` enum with `Tag(ExceptionTag, BlockCall)` + tuple `Into<(Option<ExceptionTag>, BlockCall)>` conversion so `ExceptionTableData::new` accepts both forms; `FinalizedMachExceptionHandler` enum widening the `FinalizedMachCallSite.exception_handlers` field type so cg_clif's `for &handler { match handler { Tag(t, l) => ... } }` works. Iter 9 small fixes: drop `StackSlotData.key` field, drop extra `None` arg from `LineProgram::new`, `osstr_as_utf8_bytes` cfg-split for semos_std OsStr (which is a `str` alias), `IndexSet::with_hasher(FxBuildHasher)` for no_std indexmap, semos_std::path adds `to_str()` + `Components::next_back()` (DoubleEndedIterator). Iter 10: `semos-rustc/src/main.rs` rewritten as a cg_clif smoke binary — constructs `extern "C" fn main() -> i32 { 42 }` via FunctionBuilder, lowers via x86 backend, emits an ELF via cranelift-object. Result: `cargo build --release -p semos-rustc --target x86_64-unknown-none` produces a 6.1 MB statically-linked ELF target-side. Not yet DEMO 80 (`rustc_driver_impl` still gated behind the Phase 5a sweep done in iter H1), but the Cranelift codegen pipeline now runs end-to-end from inside a SemOS Ring-3 program. |
| M27 rustc-on-SemOS Phase 5c Stage G iter 5b 2026-06-07 | **Cranelift no_std port — 4 sibling crates merged + cranelift-codegen source vendored + gimli FIXED end-to-end (commits `b227885`-`6db23b0` arc; head merge `2026-06-07`).** Yesterday's Stage G iter 5a left cranelift-codegen with only Cargo.toml patched (no source — sandbox `cp -r` denied); today's iter 5b cherry-picked the parallel-agent worktree (`agent-ac14830baa0b8fd70`) for `cranelift-frontend` + `cranelift-assembler-x64` + `cranelift-assembler-x64-meta` + `cranelift-srcgen`, then completed cranelift-codegen's ~280-file `src/**` vendoring from the registry copy, then ran a mechanical `use std::* → use core::*/alloc::*/hashbrown::*` sweep on every top-level import in the codegen tree. **gimli FIXED** (was the perpetual 5-error blocker since iter 3): `write/{cfi,op,unit}.rs`'s `std::collections::HashMap` → `alloc::collections::BTreeMap as HashMap` (3 sites in `mod convert` gated on `feature="read"` which cranelift-codegen does pull); `write/endian_vec.rs`'s `std::mem` → `core::mem`; `write/line.rs`'s `IndexMap::new()` → `with_hasher(FxBuildHasher::default())` (4 sites — the type aliases pin FxBuildHasher so `::new()` requires RandomState which is std-only); leb128.rs grew a 14-LOC `pub mod io` shim (Error + Write trait + `impl Write for &mut [u8]`) so the LEB128 writer fns no longer need `std::io::Write`. Workspace `Cargo.toml` got 4 new `[patch.crates-io]` entries. Per-crate verdict from `cargo check -p rustc_codegen_cranelift --target x86_64-unknown-none`: cranelift-{frontend,assembler-x64,assembler-x64-meta,srcgen} = 0 errors each (clean); gimli = 0 errors (was 5); cranelift-codegen = 430 errors remaining for iter 6, characterized as: **~241 cascade from one regalloc2::Reg missing Debug derive** (single root, downstream resolves), **~180 body-level `std::*` references** the sed sweep missed (need a multi-line aware pass on patterns like `std::fmt::format()`/`std::mem::take`/inline `std::cmp::min`), **9 `std::sync::OnceLock` uses** in `isa/{aarch64,s390x,riscv64,x64,pulley_shared}/abi.rs` (need spin::Once or a custom `OnceLock` shim — sketched in the commit body), **2 `std::sync::mpsc` uses** behind `#[cfg(feature="souper-harvest")]` which we don't enable (safe). Today also closed the W540 USB-2 enumeration bug that had blocked iPhone tether testing for 13+ hours: `disarm_ehci_smi()` writes EHCI's `USBLEGCTLSTS = 0` to clear SMI enables (was `0xC0080000`, bits 19/30/31 trapping BIOS on every USB-2 activity); the previously-stuck PLS=Polling ports now reach PED=1 cleanly. Plus the SuperSpeed hub descriptor type fix (0x2A vs 0x29) unblocked Lenovo Pro Dock 40A1 hub bring-up. iPhone enumeration itself remained intractable from software (D+/D- electrical-handshake level, not a chipset config we could find a quirk for); ported Linux's `drivers/net/usb/ipheth.c` driver (`usb/iphone.rs` rewrite — correct class 0xFF/0xFD/0x01 at alt setting 1, GET_MACADDR via vendor request 0x00, carrier check via vendor request 0x45 with 0x04 = "carrier on") and shipped a multi-pass `enumerate_ports` retry loop (2s VBUS settle + 3 retry passes with 1s waits) so the foundation is in place if a different cable/host configuration unblocks iPhone enumeration in the future. iwlwifi PCI probe banner + CSR sanity read (HW_REV + HW_IF_CONFIG + GP_CNTRL) wired into `init()` so the W540 boot now prints `[wireless] PCI scan for iwlwifi cards...` and `[iwlwifi-pci] found Wireless 7260 ...`. Cumulative Stage G progress through iter 5b: **15 cranelift sub-crates / supporting crates vendored** (bitset, entity, bforest, control, codegen-shared, object, anyhow, crc32fast, regalloc2, regex, aho-corasick, rustc-hash, memchr, frontend, assembler-x64, assembler-x64-meta, srcgen, module, gimli — 19 crates total) + 5 deps patched (`default = []` + feature surface fixes). Stage G iter 6 target: take cranelift-codegen 430 → 0 (regalloc2 Debug derive + body-level std sweep + OnceLock shim). |
| W540 USB enumeration unblocked 2026-06-06 | **EHCI SMI disarm + ipheth driver + SS hub support (multi-commit arc culminating in `b227885`).** The W540's USB-2 ports had been stuck in PLS=Polling PED=0 for the entire day, blocking iPhone tether testing. Root cause traced via per-EHCI usbinfo dump: BIOS left EHCI `LEGCTLSTS = 0xC0080000` with bits 19/30/31 set (SMI on Ownership Change, SMI on PCI Command, SMI on BAR) — every USB-2 hardware activity trapped into a BIOS SMI handler that interfered with our xHCI port reset. `disarm_ehci_smi()` walks PCI for class 0x0C/0x03/0x20 controllers, reads the EHCI Extended Capability Pointer from HCCPARAMS, and writes `LEGCTLSTS = 0` to clear all SMI enables and W1C status bits. Called BEFORE `reset_and_start_ehci_full()` so subsequent EHCI writes during halt/HCRESET/RS=1 don't fire SMI. After this, USB-2 ports reach PED=1 cleanly on the W540. Plus three other USB landings this arc: (a) **SuperSpeed hub descriptor type 0x2A** (USB 3.2 §10.15) instead of 0x29 (USB 2.0 §11.23.2.1) — `bring_up_hub` takes the enumerated speed and picks the right type, fixing the `control_in stall (cc=6)` on the Lenovo Pro Dock 40A1's hub bring-up; (b) **Multi-pass enumeration with 2s VBUS settle + up to 3 retry passes (1s between)** for slow-signaling devices, on the theory iPhones may need 1-2 seconds to bring up D+ after VBUS rise — uses `kernel_core::platform::ticks()` so interrupts-enabled syscalls work; (c) **`usbinfo` and `usbenum` shell builtins** (SYS_USBINFO=114, SYS_USBENUM=115) for bare-metal debug visibility since the W540 has no serial — `usbinfo` prints xECP Supported Protocol entries (USB-2 ports 1..15, USB-3 ports 16..21 on the W540), every EHCI controller's PortOwner + LEGCTLSTS, every xHCI PORTSC with PLS/speed names, and every enumerated slot's DEVICE/CDC-ECM/MSC/iPhone state. iPhone tethering protocol itself ported from Linux's `drivers/net/usb/ipheth.c` (the roadmap was wrong about CDC-ECM — iPhone tethering is the `ipheth` proprietary protocol: class `0xFF/0xFD/0x01` at alt setting 1, control transfers `bRequest=0x00` for GET_MACADDR and `bRequest=0x45` for CARRIER_CHECK returning 0x04 when Personal Hotspot is active). `usb/iphone.rs` reorganized around the correct constants; `try_enumerate_ipheth` runs ahead of CDC-ECM when vendor=0x05AC. iPhone enumeration on W540 remained blocked at the chipset/electrical level (D+/D- handshake — same iPhone works on Windows with the same cable so something at the W540 PHY layer is the gap); the driver foundation is shipped and waiting. Memory entry `project_semos_iphone_tether.md` captures the full failure characterization for the next session. |
| M27 rustc-on-SemOS Phase 5b complete 2026-06-04 | **All 37 patched `rustc_*` crates `cargo check -p <name>` clean against `x86_64-unknown-none`.** Stage F12 closed: rustc_traits 68→0, rustc_const_eval 226→0, rustc_ty_utils 566→0, rustc_codegen_ssa 384→0, rustc_lint 3282→0, rustc_borrowck 80→0. Plus one extra surface: rustc_public, rustc_public_bridge cleanups when the rustc_lint dep finally compiled. Cumulative through F12: **~23,100 errors cleared across 37 patched crates** (cumulative from F1). Key technical lifts this stage: `#![cfg_attr(target_os = "none", no_std)]` as the "big lever" pattern (single attribute drops error counts 10-100×), `ena 0.14.4` vendored at `vendor-externals/ena` with `log` dependency dropped and `debug!` macro replaced by a no-op (model for any future host-only dep), `object` crate's `write`-feature host-only with 5 cfg-gated fns in `rustc_codegen_ssa::back::metadata`, `thorin` + `wasm-encoder` cfg-out at the dispatcher level, rustc_proc_macro `bridge::client::ProcMacro` stub with phantom Client<I,O>, hashbrown 0.14→0.15 alignment (Entry type-equality), AUX-bit-tolerant PS/2 polling fallback on the W540 (legacy IRQ-1 routed through an IOAPIC pin we don't yet parse), framebuffer-echo for typed chars (serial-less hardware), ESC-to-skip-demos hot key. `semos_std` surface additions: `Duration::new(secs, nanos)` + `AddAssign/SubAssign/as_secs_f64`, `Path::MAIN_SEPARATOR(_STR)` + `is_relative`/`into_os_string`/`Default`/`From<&Path>`, `io::Read::read_exact` default impl. **Phase 5c (DEMO 80) is the next M27 gate**: wire `rustc_driver_impl` into `semos-rustc::main`, statically link `cg_clif`, compile-and-spawn `fn main() { println!("hi"); }` end-to-end. Per-stage execution diary at `docs/m27-port/EXPERIMENT_LOG.md` (also copied to `iCloud/Work/M27_rustc_port_EXPERIMENT_LOG.md` for archival). |
| M27 rustc-on-SemOS Phase 4 — codegen tier complete 2026-05-31 | **7 codegen-tier rustc_* crates patched across 2 waves (~258 files / ~115k LOC / ~793k tokens), commits `97a7b75` + `a6cf41f`.** Wave 1 (F1-F4, 4 parallel agents) hit a simultaneous late-bounce on session-limit just like Wave 2 of Phase 3 — user manually integrated partial agent outputs as `97a7b75` (~21 files: F1's codegen_ssa lib + back/mod (§1.7 whole-module cfg-gates on apple/command/link/linker), F2's mir_transform lib + dump_mir + pass_manager, F3's mir_build Cargo + lib, F4's passes 6 files; rustc_metadata entirely untouched). F1/F2/F4 wrote incremental notes that survived the bounce, F3 didn't. Recovery wave (G1-G4, 4 parallel agents, `a6cf41f`) closed everything: G1 rustc_codegen_ssa remainder 263k tokens (extended §1.7 cfg-gate list to also include back/{archive, rpath} since cg_clif emits ET_EXEC directly without rlibs; back/write.rs needed ~25 cfg-gate insertions for LLVM worker pool + mpsc + jobserver surface), G2 rustc_mir_transform remainder 108k tokens 2-3 t/LOC (textbook B1 LARGE-but-THIN recipe-following per F2's pre-port survey), G3 mir_build + mir_dataflow + monomorphize 172k tokens 3.9 t/LOC (cfg-gated 5 dump paths: dataflow graphviz, monomorphize partitioning/closure-profile/print_mono_items), G4 rustc_metadata full ARCHITECTURAL libloading drop per §1.2 + passes remainder 250k tokens (5 libloading functions cfg-gated host-only with SemOS DylibError::DlOpen stubs; libloading + tempfile moved to `[target.'cfg(not(target_os = "none"))'.dependencies]`; fs.rs/locator.rs/encoder.rs cfg-split). **MAJOR architectural insight surfaced by G4 (RECIPE.md §1.3 updated)**: `user-programs/std-shim` pins `target = "x86_64-unknown-none"` in its `.cargo/config.toml` and uses raw SemOS syscalls in its bodies — **`semos_std` is NOT host-buildable**, contradicting prompt language I'd been giving agents since Phase 3 Wave 1. Phase 3 agents (especially E3's inference triad at 2 t/LOC, E1's rustc_middle) made many unconditional `std::path/io/fs/sync/... → semos_std::*` substitutions that work on the SemOS target but break the host build. RECIPE.md now distinguishes alias substitutions (`core::*` / `alloc::*` / `hashbrown::*` — unconditional) from `semos_std::*` substitutions which **must cfg-split** with `#[cfg(not(target_os = "none"))] use std::*; #[cfg(target_os = "none")] use semos_std::*;` and `[target.'cfg(target_os = "none")'.dependencies]` for the Cargo.toml dep. Phase 5 integration will need a mechanical sweep (~1-2 sessions) to retroactively cfg-split all the unconditional `semos_std::*` Phase 3 substitutions before Phase 5's first build attempt. Three other surface gaps flagged for Phase-4.5 micro-wave: `Path::display()` (20+ sites — dominant blocker), `io::Seek` + `File::seek` + `SeekFrom`, `io::Error::new(ErrorKind, msg)` + `ErrorKind::Unsupported`, `fs::rename`, `File::open_buffered`, `io::copy`, `Path::metadata`/`exists`. Phase 4 lessons folded into RECIPE.md: the substitution table now has a "cfg-split required" sub-table; HANDOFF_TEMPLATE optional `§0 pre-port survey` (F2's pattern of grepping `\bstd::` across all crate files BEFORE patching, then writing a substitution-by-pattern table) was the load-bearing asset that let G2 hit 2-3 t/LOC. Cumulative through Phase 4: **48 crates / ~437k LOC of ~770k post-§1 internal rustc (~57%) / ~6.2M total session tokens / ~8 hrs wall across 3 sessions**. Phase 5 (integration: wire rustc_driver_impl into semos-rustc binary, statically link cg_clif, DEMO 80 for hello-world → SemOS ELF → SYS_SPAWN end-to-end) is the next gate; preceded by the semos_std cfg-sweep + Phase-4.5 surface additions. |
| M27 rustc-on-SemOS Phase 3 — semantics tier complete 2026-05-31 | **21 additional rustc_* crates patched no_std + semos-std across 3 waves in a single follow-on session (~2 hrs wall, ~2.8M tokens, ~258k LOC).** Cluster A frontend (Wave 1, 3 agents `c186403`): rustc_parse + rustc_parse_format (C1, ~32k LOC, 3.6 t/LOC, R2's Command::new claim in parser/diagnostics.rs was wrong, one architectural decision saved); rustc_ast_pretty + rustc_ast_lowering + rustc_ast_passes (C2, ~19.6k LOC, 3.6 t/LOC, ZERO markers — cleanest port since A6); rustc_attr_parsing + rustc_feature + rustc_builtin_macros + rustc_expand partial (C3, 13 architectural files + line-precise §3 recipes for 29 more). Cluster B semantics (Wave 2 `81b5e0d`, **simultaneous 5-agent late-bounce**: all 5 hit session limit at summary-write after 8-10 min of real work, ~100 files landed before bounce — a NEW failure mode at wave-orchestration layer; lesson codified as "probe-then-fleet when bucket-state unknown"; recovery wave (E1-E4, `d5b5bdb`) closed the cluster: rustc_middle's remaining 98 of 116 files (E1, 261k tokens), rustc_hir_typeck + rustc_expand remainder (E2, 172k tokens, 4.5 t/LOC), rustc_infer + rustc_trait_selection + rustc_const_eval (E3, 210k tokens, **2 t/LOC — cheapest port yet; B1 LARGE-but-THIN dominates so strongly that the inference triad has zero std::sync sites**), rustc_borrowck (E4, 187k tokens, R2's "sync:8" NEEDS-SHIM was phantom — crate was MECHANICAL with one cfg-gated polonius/legacy/facts.rs dump cluster). semos-std prep landed mid-Phase: `7978ce5` (io::Stderr + LocalKey<Cell<T>>::{get,set,take,replace} + LocalKey<RefCell<T>>::with_borrow{,_mut} std 1.73 sugar); `c9f0b2d` (sync::LazyLock + env::VarError + env::var() → Result<String, VarError> std-signature; sem-sh's 2 callers updated). Recipe evolution: D1 introduced `#![cfg_attr(target_os = "none", no_std)]` + `#[cfg(not(target_os = "none"))] extern crate std;` — cleaner than A3's cfg-body splits, now RECIPE.md §1.2 preferred default for new agent prompts; E1-E4 used it throughout. Cross-crate flag landed inline as `// M27 R4 B5 TODO(Phase 4/5)` at rustc_error_messages/src/lib.rs:602: IntoDiagArg trait def still uses std::path::PathBuf while 7 impl crates (hir, errors, middle, borrowck, const_eval, trait_selection, hir_typeck) now use semos_std::path::PathBuf — fix at Phase 4/5 integration, rustc_error_messages stays on the §1.8 fluent-deferral list. Cumulative session: ~5.4M tokens / ~322k LOC patched of ~770k post-§1 internal rustc / 41 crates done / 4 calendar hours wall across two sessions. Phase 4 (codegen tier: rustc_codegen_ssa + rustc_mir_* + rustc_monomorphize + rustc_passes + rustc_metadata, ~6-7 crates) is now the next M27 gate; the plan-estimated 5-10 calendar-sessions is the upper bound since §1.7 (cg_clif owns ET_EXEC) drops the codegen_ssa::back::link subsystem entirely. |
| M27 rustc-on-SemOS Phase 1+2a+2b — foundation tier complete 2026-05-31 | **19 of ~70 internal `rustc_*` crates patched no_std + semos-std in one swarm-driven session (~5 hours wall, ~2.59M tokens).** Phase 1 recon (4 parallel agents, R1 dep graph / R2 std-surface audit / R3 externals / R4 architectural-block audit) characterized the 77-crate rustc compiler tree; surfaced 3 additional decisions folded into §1.7/1.8/1.9 (cg_clif emits ET_EXEC directly skipping `rustc_codegen_ssa::back::link`, drop i18n entirely hardcoding English diagnostics, FatalError → process abort accepting "one error per compile" in v1). Phase 2a (6+ parallel agents) closed the foundation tier: rustc_hashes, rustc_arena, rustc_fs_util, rustc_log, rustc_lexer, rustc_graphviz (partial — needs semos_std::io shim), rustc_ast_ir, rustc_error_codes, rustc_index, rustc_serialize, rustc_macros, rustc_index_macros, rustc_type_ir_macros, rustc_fluent_macro, rustc_span (full 18/18 files), rustc_data_structures (33 files + 6 cfg(target_os = "none") host/target splits) + rustc_thread_pool stubbed (~600-line single-threaded shim replacing 7,476-line vendored rayon fork). Phase 2b (4 parallel agents) closed the foundation cycle: rustc_ast (came in at a remarkable 10 t/LOC — much less std-coupled than the recon's LOC sizing predicted; insight: classify by std-surface, not LOC), rustc_lint_defs (`builtin.rs` was 5,409 LOC of pure `declare_lint!` macro invocations needing zero edits — third efficiency tier alongside "recipe-following" and "config-only"), rustc_errors (§1.8 i18n removal landed; B3 late-bounce on session limit required B3-followup to document; key insight: rustc_errors only has ONE real FS site so the R3-budgeted 3-session fluent-bundle external port can probably be skipped entirely), and A1's deferred parking_lot collapse in rustc_data_structures' sync.rs + sync/lock.rs. **semos-std massively extended** in parallel: `sync::OnceLock<T>` (futex-backed), `process::abort_with_code(i32)`, `thread::LocalKey<T>` + `thread_local!` macro (single-threaded), `thread::ScopedKey<T>` + `scoped_thread_local!` macro, `ffi::OsString`/`OsStr` (UTF-8 aliases), `env::var_os`/`vars`/`vars_os`, `path::Path::canonicalize_lexical()`, `path::Components`/`Component`/`Path::strip_prefix()`/`Path::as_os_str()`/`Cow<Path>`/`Borrow<Path>`/`ToOwned for Path`. Codified the swarm recipe in `docs/m27-port/RECIPE.md` (canonical port pattern with all corrections discovered: `.cargo-checksum.json` is N/A for raw rustc-src, agents should never `git merge` — use `git show main:` + Write tool, `[workspace] members = []` to avoid dev-dep resolution, `cfg(target_os = "none")` host/target body split, doc-comment-vs-prelude ordering trap) and `docs/m27-port/HANDOFF_TEMPLATE.md` (line-precise per-file recipes section delivers ~10× efficiency on followups — A2 → A2-followup was 120 → 14 t/LOC on the same crate). Full session diary with per-agent token/tool-use/duration table at `docs/m27-port/EXPERIMENT_LOG.md`. Phase 3 (semantics tier — ~13 crates including 60k-LOC rustc_middle) and Phase 4 (codegen, ~6-7 crates) projected at ~21M additional tokens to reach a buildable semos-rustc; M27's "Cargo drives rustc on SemOS to produce a working binary" bullet still aspirational but the dep-graph + std-surface gap and the orchestration recipe are now concrete. |
| M27 D.2 live Cranelift on SemOS 2026-05-30 | **Live `cranelift-codegen` running Ring-3 on SemOS — DEMO 73 PASS with no inlined snapshot (`1a3ac52`, `632a4d9`, `8009d87`).** Cranelift 0.122's full no_std port closed: 7000+ build errors → 0 across ~14 vendored crates (per-crate detail in `user-programs/semos-cc/PORT_LOG.md`). `semos-cc/src/main.rs` no longer carries `const ADD_BYTES` — `compile_add_bytes()` constructs the IR via `cranelift-frontend::FunctionBuilder`, runs `verify_function`, lowers via `Context::compile`, splices the bytes into the ET_EXEC. semos-cc is now a 5.4 MiB ELF (Cranelift's weight) that boots Ring 3, runs through STAGE 1..5 of the codegen pipeline live, emits `/d2-emitted.elf`, and that emitted ELF then SYS_SPAWNs and exits 3 (live-codegen-produced `add(1,2)`). Kernel-side tuning that made it work: `MAX_PT_FRAMES` 2048→32768 (128 MiB page-table pool to hold the 5.4 MiB mapping plus user-heap growth) and `USER_PROC_STACK_SIZE` 64 KiB → 1 MiB (Cranelift's regalloc recursion blew the small stack; the resulting overflow scribbled past the exit frame and showed up as a mysterious `0xFA00_0497` exit before the bump). Notable port lessons committed in PORT_LOG.md: cargo's `target-applies-to-host = false` is silently a no-op on stable cargo (each meta build-dep needs its own `.cargo/config.toml`); `[workspace]` alone triggers dev-dep resolution (`members = []` is the safer opt-out); `hashbrown 0.15`'s `default-hasher` feature is what makes `HashMap::new()` work no_std; `core::sync::OnceLock` doesn't exist (write a `AtomicBool + UnsafeCell` shim); f32/f64 `sqrt/ceil/floor/trunc/round_ties_even` live on the *type* in std only — use `libm` via an extension trait. 170 PASS / 0 FAIL / 0 #DF. **Closes the M27 D.2 open follow-up flagged in `SELF_HOSTING_PLAN.md`.** D.3 (a parser front-end so the compiler can take novel source) is now the next D-step. |
| M27 D.2 Ring-3 emitter on SemOS 2026-05-29 | **Toolchain pipeline runs end-to-end *on* SemOS (DEMO 73).** New `user-programs/semos-cc/` is the D.1 host emitter ported to Ring 3 + `semos-std`: same `_start` shim (47 B) + ELF wrap (192 B, ET_EXEC at entry `0x400078`), same Cranelift-codegen'd `add(i64,i64)` body (13 B: `lea rax,[rdi+rsi]`) — but the bytes are inlined as a static `const ADD_BYTES` (snapshot from D.1) so we can validate the pipeline before tackling Cranelift's no_std port. The Ring-3 emitter calls `semos_std::fs::write("/d2-emitted.elf", &elf_bytes)` → SYS_OPEN(CREATE) + SYS_FWRITE → install-anywhere path namespace; the kernel's `spawn_namespace_elf` (any absolute path not under `/bin/` routes through it) then SPAWNs the emitted ELF directly from the registered object's bytes. DEMO 73 is two-stage: (A) `/bin/semos-cc` exits 0 with the "D.2 emitter" log on stdout — proves the Ring-3 emitter ran; (B) `/d2-emitted.elf` exits 3 with the "semos-cc" marker — proves the freshly-emitted ELF runs and the inlined Cranelift `add(1,2)` executed correctly inside it. Shared `ring3_spawn_capture(path, &mut cap)` helper for both stages (pipe → dup2 → SPAWN → poll scheduler slot → drain → exit-code read). Required adding `"semos-cc" => "semos-cc.elf"` to `handle_spawn`'s hardcoded `/bin/<name>` table — same gotcha as D.1 (captured in [[feedback_handle_spawn_bin_table]]). 170 PASS / 0 FAIL / 0 #DF. **D.2's milestone goal is met** ("the toolchain pipeline works end-to-end on SemOS" per `SELF_HOSTING_PLAN.md`); follow-up: live `cranelift-codegen` on SemOS (the 0.122 Cargo.toml shows `categories=["no-std"]` + `std=["serde?/std"]`, so `default-features=false, features=["x86"]` is the entry point). |
| M27 D.1 host semos-compiler → SemOS ELF 2026-05-29 | **First ELF assembled directly by *our* host compiler (DEMO 72).** `compiler/src/main.rs` extended: Cranelift lowers `i64 add(i64,i64)` to 13 bytes of `push rbp / mov rbp,rsp / lea rax,[rdi+rsi] / mov rsp,rbp / pop rbp / ret`; a hand-emitted 47-byte `_start` shim does `SYS_WRITE("semos-cc D1\n") → mov edi,1 / mov esi,2 / call rel32 → add() / mov rdi,rax → SYS_EXIT(rax)`; both glued into a 192-byte ET_EXEC at entry `0x400078` with one R+X PT_LOAD at `0x400000` (matches `kernel-core/src/process/elf.rs::create_hello_elf` shape byte-for-byte — no linker, no `cranelift-object`). Emitted ELF persisted to `compiler/out/semos_cc_hello.elf` (gitignored), kernel `include_bytes!`s it. DEMO 72 SPAWNs `/bin/semos-cc-hello`, asserts exit==3 (proves Cranelift's lea-add ran in Ring 3 with correct SystemV arg layout) AND "semos-cc" marker in captured stdout (proves the hand-emitted SYS_WRITE went through a real ELF mapping). Required one-line addition to `handle_spawn`'s hardcoded `/bin/<name>` → ramfs table (`semos-cc-hello` → `semos-cc-hello.elf`) — that table is the recurring gotcha for any new `/bin/`-spawnable program. 170 PASS / 0 FAIL / 0 #DF. Unblocks D.2 (port the same emitter to Ring 3 on SemOS). |
| M26 cg_clif e2e 2026-05-29 | **rustc Cranelift backend → SemOS ELF → SYS_SPAWN (DEMO 71, `0039b25`).** Real rustc-on-cg_clif acceptance: a Rust source file compiled with the rustup-distributed `rustc-codegen-cranelift-preview` component (1.95.0-nightly) on our pinned toolchain produces a 13,688-byte ELF that runs in Ring 3 and writes its marker via SYS_WRITE before SYS_EXIT(0). `user-programs/cg-clif-hello/` opts in via `cargo-features = ["codegen-backend"]` + `[profile.release] codegen-backend = "cranelift"`; core/compiler_builtins stay on LLVM via `[profile.release.package."*"] codegen-backend = "llvm"` because cg_clif can't yet lower core's `va_end`. DEMO 71 pipes stdout, polls for Exited, checks exit==0 AND captured stdout contains "cg_clif". 169 PASS / 0 FAIL. Closes the "vendor + JIT + emit a real ELF on SemOS" goal; rustc-on-metal is unblocked at the codegen-backend layer. |
| M26 vendor + smoke 2026-05-29 | **Cranelift vendored + JIT smoke (`f1b2635`).** New `compiler/` host crate depends on cranelift-codegen + cranelift-frontend + cranelift-module + cranelift-object 0.122; `cargo vendor --versioned-dirs` pulled in 44 crates (~25 MB) into `compiler/vendor/` for deterministic offline builds. `compiler/.cargo/config.toml` overrides the repo-root's aarch64 default (ARM-phase leftover) since semos-compiler is a HOST tool. Smoke test builds an IR `i64 add(i64,i64)`, verifies, lowers to x86_64 — emits a 13-byte System V function (`push rbp; mov rbp,rsp; lea rax,[rdi+rsi]; mov rsp,rbp; pop rbp; ret`). The Cranelift pipeline is now live in the tree. Next: wrap cranelift-object output into a SemOS ET_EXEC (entry 0x400000, _start hooking SYS_EXIT), so the emitted code runs via SYS_SPAWN — multi-session integration with our linker.ld + non-PIE layout. |
| M25 sync live-smoke 2026-05-29 | **Ring 3 sync-demo + DEMO 70 (`f087124`)** — `user-programs/sync-demo/` exercises the new sync surface end-to-end on metal-equivalent QEMU: Condvar wakeup fires (state goes 0→42 across thread boundary), mpsc 1..=5 ordering + disconnect (sum=15, then RecvError after last sender drops), RwLock holds 2 concurrent readers + writer succeeds after they drop. Exit 0 = full pass; 0x71..0x74 are stage-specific failure codes. 167 PASS / 0 FAIL. Closes the "compile-validated, never functionally smoked" follow-up from M25. |
| M25 stdlib complete 2026-05-29 | **`semos-std` finished (`7276f07`)** — Session A of `docs/SELF_HOSTING_PLAN.md` is done. Adds Condvar (futex seq-counter, no lost-wakeup) + RwLock (one u32 state: bit 31 writer, bits 30:0 reader count; CAS + futex_wait on contention) + mpsc (multi-producer single-consumer channel on Mutex<VecDeque<T>> + Condvar; sender clonable; last-sender-drop or receiver-drop wakes the other side with Disconnected/SendError) + HashMap+HashSet via vendored `hashbrown` 0.15 (`default-features = false` for no_std + alloc; deterministic default hasher) + BTreeMap/BTreeSet/VecDeque re-exports. All 10 user programs still build, kernel still 165 PASS / 0 FAIL with sem-sh embedded. Follow-up: a small Ring-3 `sync-demo` to functionally smoke-test Condvar wakeups + mpsc ordering on metal (the types compile, but live wakeup correctness needs a real producer/consumer test). M26 (Cranelift vendoring) is now the next gate toward rustc-on-metal. |
| xHCI bulk + MSC live 2026-05-28 | **Live USB Mass Storage on xHCI (DEMO 69, `63fd75e`).** Extends the xHCI driver from HID-interrupt-only to true bulk TX/RX. `init_bulk_in_ep`/`init_bulk_out_ep` (EP types 6/2), two static bulk transfer rings, `bulk_xfer` (Normal TRB + IOC + ISP, poll event ring), `Trb::transfer_remaining` for short-packet handling. New `try_enumerate_mass_storage` path walks the config descriptor for SCSI BBB (0x08/0x06/0x50), configures the bulk EPs via ConfigureEndpoint, runs INQUIRY + READ CAPACITY through 3-phase CBW/data/CSW using the existing `usb::mass_storage` protocol layer. Validated against QEMU `-device usb-storage`: vendor="QEMU" product="QEMU HARDDISK" rev="2.5+", 262144×512 B (128 MiB), all DEMO 69 PASS. Default config (usb-kbd) still 165 PASS, no regression. Follow-ups: CDC-ECM enumeration and the live gamepad/HID-report path reuse the same machinery; multi-device-at-once needs a separate per-slot context refactor. |
| AHCI/SATA 2026-05-28 | **SATA block driver — the T540 internal-disk path (DEMO 67, `ed2630f`).** PCI class-coded discovery (0x01/0x06/0x01), ABAR (BAR5) → MMIO, AHCI-mode enable (no HBA reset; HR severs the SATA PHY in QEMU's ich9-ahci and doesn't auto-relink — real-hardware follow-up adds HR + SCTL.DET cycle + CAP2.BOH handoff), port scan with short DET poll, per-port CL/FB setup, ATA Identify Device for block count, single-LBA READ/WRITE DMA EXT via a one-entry PRDT. Registered as `sata0` BlockDevice. First-boot in QEMU: port 0 SSTS=0x113 SIG=0x101, 131072×512 B (64 MiB), DMA round-trip clean. 159 PASS / 0 FAIL / 0 #DF. |
| CDC-ECM v1 2026-05-28 | **USB Ethernet descriptor parser (DEMO 66, `e79a3a3`).** The M11 fallback path — a USB-to-Ethernet dongle lets TLS run on metal before iwlwifi works. Protocol v1: class/subclass/protocol IDs (0x02/0x06 control, 0x0A data), `parse_config` walks the full configuration blob (skipping Header/Union functional descriptors, picking up CDC Ethernet Functional Descriptor for iMAC/MTU, finding the Data interface alt with bulk EPs), `parse_mac_string` decodes the UTF-16LE 12-hex-digit MAC string (CDC §5.4). Validated against a realistic config blob → iface 0 control, iface 1 alt 1 data, bulk 0x81/0x02 MPS 512, MAC `02:BA:DC:AF:E0:01`, MTU 1514. 157 PASS / 0 FAIL. Live xHCI bulk-endpoint TX/RX is the follow-up on real hardware. |
| M11 v1 (protocol) 2026-05-28 | **802.11 frame builders + iwlwifi PCI scaffolding (DEMO 65, `a0d487b`).** QEMU has no wireless emulation, so v1 = the pieces we'll need on day-1 of metal: `wireless::build_probe_request` / `build_open_auth_request` / `build_association_request` + `build_eapol_msg2` (WPA2 four-way handshake Msg2 with KeyInfo bitflags via bitflags 2.4; MIC left zero for the crypto layer to patch). iwlwifi PCI device-ID table covers T540 (7260/3160 family) and P1 Gen 6 (AX211, 0x51F0/0x51F1/0x54F0). DEMO 65 byte-validates each frame against the IEEE 802.11 layout (Probe Request FC=0x4000 + broadcast addrs + SSID IE, Open Auth algo=0/seq=1, EAPOL KeyInfo=0x010A = MIC+Pairwise+CCMP) and the PCI table. 154 PASS / 0 FAIL. Follow-ups (all hardware-gated): firmware-upload secboot, ALIVE event, PHY init (NVM+PNVM+regulatory+calibration), TX/RX command queues, four-way handshake MIC over the derived PTK. |
| M16 HID parser 2026-05-28 | **HID report descriptor parser + gamepad decode (DEMO 64, `d4b8e2d`).** Pure-module v1 since QEMU has no gamepad: `usb::hid_report::parse` walks a HID 1.11 descriptor (short items, global/local state, Usage Min/Max ranges, multi-usage Input, signed Logical Min/Max, Output/Feature offset advancement) → `ReportLayout` flat field table (no_std, no alloc). `decode_gamepad` extracts standard axes (X/Y/Z/Rx/Ry/Rz/Hat) + first 32 buttons. Validated against a canonical Generic-Desktop Game Pad descriptor + synthetic report `[0x42, 0xFE, 0x0A]` → `x=66, y=-2 (sign-extended), buttons=0b1010`. 150 PASS / 0 FAIL. Follow-ups (hardware-gated): fetch report descriptor via USB control transfer, route input reports in xHCI, expose a Gamepad input device. |
| M15 HD Audio 2026-05-28 | **Intel HDA controller + codec walk + PCM output (DEMO 63, `3f8fed2`).** PCI class-coded discovery (0x04/0x03/0x00), 64-bit MMIO BAR, controller reset, STATESTS-based codec discovery, walk root → AFG → first DAC + first Pin. Codec verbs via the **Immediate Command Interface** (ICI: ICO/IRI/IRS at 0x60/0x64/0x68) — CORB/RIRB-via-DMA was flaky in QEMU after the first verb. Pin: D0 + OUT_EN + EAPD. DAC: 48 kHz 16-bit stereo format, stream tag 1, unmute output amp. BDL with one entry pointing at a page-aligned 4 KiB PCM buffer holding a 440 Hz sine (16-step LUT). Output stream descriptor at MMIO `0x80 + 0x20*ISS`: CBL/LVI/FMT/BDPL/BDPU/CTL+RUN. **Validation:** LPIB sampled twice over a sleep advances (DMA active = playback). 147 PASS / 0 FAIL / 0 #DF. Follow-ups: CORB/RIRB on real metal, MSI-X, capture (ADC), gapless wrap. |
| M9 NVMe 2026-05-27 | **NVMe block driver (DEMO 62, `53cdc1a`).** PCI class-coded discovery (0x01/0x08/0x02), 64-bit MMIO BAR, admin queue bring-up (reset → AQA/ASQ/ACQ → CC.EN → CSTS.RDY), Identify Namespace (NSZE + active LBA format → block_count + block_size), Create-I/O-CQ + Create-I/O-SQ (qid 1), NVM Read/Write via PRP1 (one LBA/cmd, BlockDevice loops). Polled completions with phase-bit tracking. Page-aligned BSS queues/buffers for contiguous DMA. Registered as `nvme0`. First-boot validation in QEMU: PCI 00:04.0, MMIO=0xFEBF0000, 65536 blocks × 512 B, write+read byte-for-byte. 146 PASS / 0 FAIL / 0 #DF. Follow-ups: MSI-X, multi-block PRP lists, real error recovery. |
| M21 editor + console UX 2026-05-27 | **native modal editor + readable console.** `edit <file>` (`94581a8`, DEMO 61) launches a kernel-side vi-style editor (SYS_EDIT → `Platform::run_editor`): Normal/Insert/Command modes, `hjkl`/`0$`/`iaAoO`/`x`/`dd`/`gg`/`G`/`/n`, `:w :q :q! :wq`, Rust syntax highlighting (keywords/strings/comments/numbers) via the M7 TTF renderer, block/bar cursor, status line. Edit logic is pure (testable headlessly — DEMO 61 scripts gg→o→insert→Esc→:w + verifies the FS round-trip). Also: **2× console font** (`6230d97`, ~80×36 cells, readable) and a **scrollback pager** (`78b6bb2`, PageUp/PageDown/End over the byte ring, view freezes while reading). All keyless builds; 144 PASS / 0 FAIL / 0 #DF headless. Search-and-replace, multi-buffer, and the Ring-3 port are follow-ups. |
| Frame allocator 2026-05-27 | **per-app memory ceiling lifted + faster allocator** (toward hosting demanding apps). `MAX_FRAMES` 16384→131072: a tier pool was capped at ~64 MiB *regardless of RAM* (bitmap size), limiting any single app to ~64 MiB even on a big machine; now 512 MiB/pool (`pool_size = RAM/4` still binds in QEMU, so the ceiling shows on real hardware). Allocator rewritten from a from-zero linear bitmap scan (O(n)/alloc, O(n²) to fill) to **next-fit with a `next_word` cursor** (amortized O(1); free() biases the cursor back for prompt reuse). +56 KiB BSS, no layout #DF. Suite 155 PASS / 0 FAIL / 0 #DF. Note: past a few GB/pool a buddy/free-list allocator beats the bitmap. **FS large-file redesign is next** (the other half of the "demanding design app" assessment). |
| Snapshot u32 content_len 2026-05-27 | **large files persist** (lifts the 64 KiB persistence cap). Snapshot per-object `content_len` widened `u16`→`u32` (header +2 B, format VERSION 1→2 so stale snapshots are cleanly rejected as "fresh disk"); per-file check now bounds at `MAX_FILE_CONTENT` (256 KiB). `MAX_SNAPSHOT_BYTES` 64 KiB→1 MiB, and the save/load scratch moved from a stack array to a **heap** buffer (a 1 MiB stack buffer would overflow; a static would shift `.bss` and risk the layout #DF — heap is layout-safe). DEMO 60 extended: installs a 124 KiB ELF (`/apps/big-tool`, which the old u16 limit would have refused) + the small runnable app; two-boot test → boot 2 "loaded 163847 bytes" + both PASS (big file byte-for-byte). Why u32 not u64: a content length is bounded by `MAX_FILE_CONTENT` (256 KiB) and ultimately the 16 MiB heap — u32 (4 GiB) is already orders of magnitude past any reachable value; u64 would just waste header bytes addressing a range physics rules out. Suite 155 PASS / 0 FAIL / 0 #DF. |
| Install persistence 2026-05-26 | **installed apps survive reboot** (DEMO 60). On first boot (fresh disk) it installs `/apps/persistent-tool` + `SYS_FSYNC` (namespace → virtio0); on a later boot the boot-time `Namespace::load` restores it and the demo runs it. Two-boot test (shared vdisk): boot 1 installs, boot 2 → "loaded 39574 bytes from virtio0" + **DEMO 60 PASS: survived reboot and ran**, 0 FAIL. **Bug fixed:** the snapshot *deserialize* reconstructed object content via `from_inline` (256 B cap), so a restored 12 KiB ELF failed the whole load → flipped to `from_bytes` (heap-backed). Also made the install demos (58/59) unlink-before-create so the suite is reboot-safe. Notes: snapshot content_len is `u16`, so persisted files cap at 64 KiB (the 256 KiB in-memory cap can't all persist yet — needs a u32 format bump); QEMU testing needs `cache=writethrough` so writes survive an abrupt kill. Suite 155 PASS / 0 FAIL / 0 #DF (156 on a reboot, where DEMO 60 is a PASS). |
| demos.rs refactor (stage 1) 2026-05-26 | **extracted the recent agent/shell/TUI demos** (DEMO 47-59 era, 14 fns, ~785 lines) from the 6021-LOC `main.rs` into `kernel-x86_64/src/demos.rs` (`pub(crate)`, pulled in via `use crate::demos::*` in `init_loader_task`). main.rs → ~5240 LOC; **new demos now live in demos.rs**. Block was dependency-clean; layout shift didn't re-trigger the stack-guard #DF (256 KiB stack headroom held). Suite 156 PASS / 0 FAIL / 0 #DF. **TODO (deferred):** the older DEMO 0-46 era demos are interleaved with boot/runtime helpers (spawn_named, user_syscall, pump_keyboard, enable_sse, sem_demo_one, StatX/FutexWord) with no clean cut — migrating them is a layout-validated multi-stage job; do it *when it matters or when the kernel stack/layout is being reorganised anyway* (per user). |
| Install anywhere 2026-05-26 | **system-shell vision (4b/4): install anywhere / run anywhere.** `SYS_SPAWN` no longer needs the hardcoded `/bin` table — any absolute path routes to `spawn_namespace_elf`, which resolves the path, **tier-checks the caller against the executable** (a tier-0 agent can't run a higher-tier binary, mirroring the read gate), reads its ELF bytes from the object's heap content, and spawns. "Install" = write an ELF to a namespace path (persists to disk via `SYS_FSYNC`). Enabling changes: `MAX_CONTENT_SIZE` 64→256 KiB (covers the 124 KiB sem-sh; pure validation cap, heap-backed), and directories grew from 16→**64 entries** (dir content moved from 256 B inline to heap `from_bytes`, buffers 256 B→4 KiB) — a work OS needs more than 16 files. DEMO 58 installs a 12,720 B ELF at `/myapp` and runs it from the shell. Suite **155 PASS / 0 FAIL / 0 #DF**. Remaining: `$PATH` bare-name search (type `myapp`, not `/myapp`), per-path task names (vs generic `user-app`), reboot-persistence demo. |
| Shell scripting && / || 2026-05-26 | **system-shell vision (4c/4): conditional chaining.** sem-sh gains `&&` (run next only on success) and `||` (run next only on failure) with short-circuit, quote-aware and distinct from single-`|` pipes (`run_conditional` layer above `run_command`). DEMO 57 validates `true && echo CHAINED ; false && echo NOPE ; false || echo RECOVER` → CHAINED+RECOVER, NOPE skipped. Suite **154 PASS / 0 FAIL / 0 #DF**. **4b (PATH-anywhere) deferred:** `handle_spawn` only spawns `/bin/<name>` via a hardcoded name table; true "apps installed anywhere" needs namespace-stored executables + non-static-name spawn — a substantial kernel feature, not a shell add. `$()` command substitution + glob also remain. |
| Agent shell sandbox 2026-05-26 | **system-shell vision (4a/4): security — the LLM runs sandboxed.** The agent's `bash` tool now spawns sem-sh at **tier 0 (Public)** instead of tier 3 — the LLM is the least-trusted component in the 4-tier model, so its shell gets the lowest clearance. `SYS_OPEN`'s existing tier check (`caller_tier >= object_tier`) then denies the shell ANY Internal/Sensitive/Secret file, for both read and write — no new mechanism, just running the agent where it belongs. DEMO 56 proves it: a kernel-created Secret file is unreadable AND unmodifiable from the agent shell (`echo HACKED > /sec-doc` denied, content intact), while a Public file works. Directly fulfils "the LLM can't see secrets, can't modify protected state." Suite **153 PASS / 0 FAIL / 0 #DF**. (Remaining 4b/4c: PATH-anywhere exec + `&&`/`||`/`$()` scripting.) |
| Shell `fetch` 2026-05-25 | **system-shell vision (3/4): networking**. sem-sh gains `fetch <url>` — an HTTP/1.1 GET over the kernel TCP stack (`semos_std::net::TcpStream`, the same path DEMO 36's net-demo proved), writing the response to stdout so it pipes (`fetch ... | grep`). Pure Ring-3, no kernel change; the agent gets it via `bash`. HTTP only for now — the TLS stack is SPKI-pinned to the agent endpoint, so arbitrary HTTPS can't be validated (a CA-bundle verifier is the follow-up). DEMO 55 validates `fetch http://example.com/` → 837 B HTTP response + HTML. Suite **150 PASS / 0 FAIL / 0 #DF**. |
| recv-stall flake FIXED 2026-05-25 | **the live-agent reliability bug, root-caused and fixed.** A diagnostic (periodic `ticks()` log inside the net read spin) showed `now_tick` frozen across 16M spins: **interrupts were disabled during the recv spin**, so the timer IRQ never fired, `ticks()` never advanced, and the tick-based 30 s idle-timeout could never trip → a slow/stalled peer hung the kernel forever (the multi-session "DEMO 49 stuck at receiving" flake). DEMO 16/34/36 only escaped it by getting data fast. Fix: `enable_interrupts()` (new `Platform` hook) at the top of the `embedded_io` read/write spins in `net/tcp.rs` — a task-level blocking wait must let the timer fire + allow preemption. Validated live: DEMO 49 (agent loop) **and** DEMO 54 (`ask → "Two plus two is four."`) both pass, 0 stall spins, 3657 context switches. This also confirmed the TLS-from-syscall path for `ask` (RSP0 stack is fine). Keyless suite **149 PASS / 0 FAIL / 0 #DF**. |
| Agentic shell 2026-05-25 | **system-shell vision (2/4): the LLM in the shell**. sem-sh gains an `ask` builtin — `ask <question>` or `cmd | ask <question>` (pipes stdin in as context) — that reaches the kernel's network Claude agent via a new `SYS_ASK` (Ring-3 → `Platform::llm_ask` → `agent::ask`, a tool-free single-turn over one keep-alive TLS connection). The platform impl enables interrupts so the network call's wall-clock timeouts advance. Degrades to a clear message, never a hang, with no key / no network. DEMO 54 validates the full bridge **keyless** (Ring-3 `ask` → SYS_ASK → agent → back); live answers need a baked key. Suite **149 PASS / 0 FAIL / 0 #DF** keyless. Open: live `ask` reuses the Session path (so it's subject to the intermittent recv-stall flake), and the TLS-from-syscall stack depth on a Ring-3 RSP0 is still to be validated under load; security follow-up = tier-aware redaction so Secret content can't be sent to the API. |
| Shell-as-OS-interface 2026-05-25 | **system-shell vision (1/4): introspection**. sem-sh gains read-only `ps` (task table with **security tier** per task), `free` (heap usage), `uptime` — backed by new `SYS_PS` + wired `SYS_SYSINFO` (and existing `SYS_TIME`). All read-only and tier-safe: they expose task metadata + heap totals, never secrets or mutable state, so the agent can see the system it runs on but can't change it. DEMO 53 validates via the bash tool. **Spawn sustainability fix:** the agent `bash` tool now reaps its child at exit (`reap_slot`) so a command loop stays flat, and `MAX_PT_FRAMES` 512→2048 gives the non-reaping boot demo cascade headroom (the cumulative leak had drained the pool right at DEMO 52/53). `reclaim_dead_address_spaces` added as scaffolding for the eventual free-on-exit refactor. Suite **148 PASS / 0 FAIL / 0 #DF**. |
| M22 bash tool + grep 2026-05-25 | **agent `bash` tool**: `run_bash` spawns `/bin/sem-sh -c "<cmd>"` from kernel context, dups a pipe onto the child's stdout, and drains it interleaved (4 KiB pipe can't deadlock) — Claude gets the OS's real command surface (builtins, `;`/`|`, redirection, external ELF exec), not a reimplementation. Added a **`grep`** builtin to sem-sh (file + stdin-filter modes) so it's reachable via `bash`; bash tool description now advertises the available builtins to the model. DEMO 52 validates headlessly (write_file → `echo … ; grep NEEDLE /file` → captured + filtered). Suite **145 PASS / 0 FAIL / 0 #DF**. Open: wildcard glob expansion. |
| M22 split panes 2026-05-25 | **side-by-side TUI layout**: the middle row splits into a wider **conversation** pane (left, user+assistant) and a narrower **activity** pane (right, tool_use+tool_result), with a vertical accent divider; status bar + prompt stay full-width. DEMO 50 verifies the split by pixel readback — conversation colours appear only in the left rect and tool colours only in the right, with zero bleed across the divider (mutual exclusion proves the routing). The live agent (DEMO 49) renders into it via the same `push_*` methods. Closes the last M22 Claude-Code-parity item. Suite **144 PASS / 0 FAIL / 0 #DF** keyless. |
| M22 TLS keep-alive 2026-05-25 | **HTTP/1.1 keep-alive session** (`agent::Session`): a multi-turn conversation now rides ONE persistent TLS connection (`Connection: keep-alive` + exact response framing via `http::content_length` / `decode_chunked` as a completeness probe) instead of reconnecting per turn — that removes the inter-turn single-socket reconnect flake entirely AND returns the instant a body is framed (no trailing 30 s idle-timeout recv). Validated live: DEMO 49 ran both turns on one connect (`framed response 2001 B / 1990 B (conn kept alive)`, no reconnect before turn 2). Also gated DEMO 48 (no-key 401 test) to keyless boots so the keyed session opens on the 2nd TLS connect of the boot, not the 3rd (the flake worsens with connect count). `Session::request` reconnects+resends up to 4× to absorb the residual *initial*-connect flake. Suite **144 PASS / 0 FAIL / 0 #DF** keyless. |
| M22 TUI 2026-05-24 | **agent TUI** (DEMO 50 + DEMO 49 live integration): kernel-side three-pane terminal (`tui.rs`) over the M7/M8 `TtyConsole` — status bar / scrollback transcript / prompt, with role-coloured turns (user/assistant/tool_use/tool_result). DEMO 50 verifies every pane + each role's exact colour by pixel readback; DEMO 49's live loop drives the same panes as the conversation unfolds (real UI, not a mock). **Net stack fix:** adding the module overflowed the `init_loader` demo-runner task stack at DEMO 26 (`fs::paths::remove_child` + a timer frame tipped slot 5's 128 KiB `TASK_STACKS` guard → #DF) — the documented layout-sensitivity; bumped `TASK_STACK_SIZE` 128→256 KiB. Suite **142 PASS / 0 FAIL / 0 #DF** keyless. Remaining for full Claude-Code parity: side-by-side split panes + interactive keyboard input. |
| M22 stage C DONE 2026-05-23 | **native agent loop validated against LIVE Claude** (DEMO 49): seeds `/README`, asks Claude (real Anthropic API, `claude-haiku-4-5`) to use the `read_file` tool then summarize. Full loop runs: turn 1 → `tool_use(read_file {"path":"/README"})` → kernel runs the tool → turn 2 replays `assistant tool_use` + `user tool_result` → Claude returns the one-sentence summary. New: `agent::api_key()` (compile-time `option_env!("ANTHROPIC_KEY")`, key lands only in the gitignored binary), `Message::assistant_tool_use`, `decode_body` (chunked-aware), `send_over_tls` **3× retry loop**. **Two net reliability fixes were required and are the real value here:** (1) **rotating ephemeral local port** (`net/tcp.rs`: const `LOCAL_PORT` → `next_local_port()`) — the const port made the 3rd+ TLS reconnect in a boot hang in `poll_to_terminal` because SLIRP/peer hold the prior identical 4-tuple in TIME_WAIT and drop the new SYN; (2) **`IO_IDLE_TIMEOUT_TICKS` 10 s → 30 s** — an LLM's time-to-first-byte legitimately exceeds 10 s when *generating* a reply, which reported a premature EOF and failed the turn. DEMO 49 self-gates on a baked key (skipped in the committed keyless build). Suite still **139 PASS / 0 FAIL / 0 #DF** keyless; DEMO 49 PASS with key. M22 ✅. |

---

# Phase 9 — Bare-metal apps on top of the kernel

Goal: turn the kernel from "boots, makes a TLS call, sandboxes LLM access" into "first real app can run, read+write user files, and survive reboot."

```
                           Phase 9
                              │
            ┌─────────────────┼───────────────────────────┐
            ▼                 ▼                           ▼
       Filesystem      Time + identity                 Graphics
            │                 │                           │
   ┌────────┼─────────┐    ┌──┴───┐               ┌──────┼──────┐
   ▼        ▼         ▼    ▼      ▼               ▼      ▼      ▼
  M1     M4/5       M9    M2     M3              M6     M7     M8
 paths  syscalls  NVMe  RTC   USB-KBD             FB    fonts  vect.
        + persist
```

## M1 — Path namespace (Stage 1) `[✅]`

Hierarchical `/foo/bar` over SUID-addressed semantic objects.
Landed `872cfd2`. DEMO 17 covers it.

## M2 — RTC + wall_clock `[✅]`

MC146818 driver, `Platform::wall_clock()`, kernel-core free function wrapper.
Landed `991928b`. DEMO 19 covers it.

## M3 — USB stack (xHCI + HID keyboard) `[✅]`

Driver landed (`1301bcb`), USB enumeration unblocked by per-task
stack bump (`688a602`: TASK_STACK_SIZE 16 → 64 KiB). DEMO 18 covers it.

The "layout-sensitivity family" recurrences (#36, #40, #42) all
resolved 2026-05-18 to -19:
- #40 (kernel #PF at RIP=0): 258 KiB LlmContext stack overflow → static buffer
- #36 (USB triggers a layout-shift bug): TASK_STACK_SIZE 16→64 KiB
- #42 (small additions hang at "Initializing interrupts..."): the
  bootloader_api default `kernel_stack_size = 80 KiB` was being
  overflowed by `kernel_main`'s frame inflation from minor code
  changes. Fixed by setting `config.kernel_stack_size = 512 * 1024;`
  in BOOTLOADER_CONFIG (commit `b51e22a`). That single change
  unblocked the previously-reverted FWRITE>256 B work too (#44).

#41 — real unmapped guard pages between task stacks — **DONE 2026-05-20**
(`a9fa7d1`). Every TASK_STACK + per-task kernel stack now has an unmapped
guard page below it (2 MiB kernel PDE split into 4 KiB + PTE cleared +
`invlpg`; visible under all CR3s since process address spaces share the
kernel PML4). The whole layout-sensitivity family is now structurally
fixed: an overflow faults precisely instead of smashing the neighbour.
The guard immediately exposed two real latent overflows (per-task kernel
stack 8→64 KiB across #41/#55; TASK_STACK_SIZE 64→128 KiB). #55 (sequential
Ring-3 thread spawn / slot reuse) closed on the same fix (`e750ee8`).

**Status check after the #42 fix:**
- ✅ Root cause identified (main kernel stack default 80 KiB) and fixed
- ✅ `usb::init_and_enumerate()` runs without corrupting state (#36)
- ✅ DEMO 18 passes all 5 sub-checks against `qemu-xhci -device usb-kbd`
- ✅ All other DEMOs still pass in the same boot (28 DEMOs, 74 PASS lines)
- ⏳ ENV_BLOCK_SIZE bumped back to 2 KiB — not yet, but should be fine now
- ⏳ CSZ=1 metal validation on ThinkPad P1 — still pending hardware run

## M4 — FS Stage 3: `SYS_FS_*` syscalls `[✅]`

Path namespace exposed to user space via existing SYS_OPEN/CLOSE/
FREAD/FWRITE/STAT/MKDIR/UNLINK/READDIR numbers. Path-FD range
96..127 sits alongside legacy pipe/ramfs FDs. Tier-aware open
gate via `current_task_max_tier()`.
Landed `dfca48f`. DEMO 20 covers all 8 syscalls from Ring 0.
User-program port (fs-demo) still pending.

## M5 — FS Stage 2: snapshot persistence for the namespace `[✅]`

`Namespace::save(dev)` / `load(dev)` via `storage::snapshot`. Packed
FSNS format, BFS from root, RDRAND-backed `mint_suid` so persisted
SUIDs don't collide across boots, `created_at`/`modified_at` from
`platform::wall_clock()` populated on every mutation.
Landed `920e6da` (in-process roundtrip) + `1f62c08` (cross-boot
auto-load + idempotent DEMOs 17/20/21). Two-QEMU-cycle test
validates byte-exact restore with 450 s timestamp.
**Operational gotcha:** boot-time `Namespace::load(virtio0)` MUST
run AFTER `init_global_registry()` — that call clears the registry;
loading earlier wipes the entries. Verified by the log line
"loaded 643 bytes" still showing even when the data was wiped.
- [ ] Snapshot size limit (64 KiB today) documented as a "namespace
      metadata only" cap; large-object content goes into a separate
      per-object stream when that becomes necessary

## M6 — Framebuffer drawing API `[✅]`

Promote raw `set_pixel` to a real drawing surface.

Landed `6e972a2` (agent). DEMO 35 verified by pixel readback (111 PASS / 0
FAIL combined image). Implemented in `kernel-x86_64/src/framebuffer.rs`
(drawing API added to the existing console module). Detected live format on
QEMU: BGR, stride read from `FrameBufferInfo` (never assumed). `rgb(r,g,b)`
packs to the native order at write time. Only the user-mapped FB region is
deferred (a follow-up syscall) — core API + DEMO are done.

**Done when:**
- [x] `fb_fill_rect(x, y, w, h, color)`, `fb_blit(src, x, y, w, h)`,
      `fb_scroll(dx, dy)`, `fb_present()` as kernel-side functions — all clip
      to framebuffer bounds (no OOB writes)
- [x] Color format documented (BGR vs RGB) — derived from live
      `FrameBufferInfo`, packer switches on `PixelFormat`
- [ ] Shared-memory framebuffer region exposed to user space (mapped
      read/write into the process's address space for direct draw) —
      DEFERRED as follow-up to avoid scope creep; would use a new high
      syscall number (e.g. 60). Core drawing API + DEMO landed first.
- [x] Damage-rect / present model so apps don't tear writes — direct-render
      with accumulated damage rect; `fb_present()` is the commit point
      (back buffer skipped: ~3.5 MiB cost not justified for single surface;
      every pixel write funnels through `FbSurface` so it can be retargeted)
- [x] DEMO 35 draws a checkerboard + rect + blit + scroll, verified by
      reading pixels back from framebuffer memory (headless-safe)

## M7 — Font rasterization `[✅]`

Render text in real fonts, not just the 8x16 bitmap console. Landed
`b059960`. **Used `ttf-parser` instead of fontdue:** fontdue needs an
allocator and isn't available offline, and the kernel has no global
allocator. `ttf-parser` (cached, zero-allocation, no_std) gives glyph
outlines; we rasterize them ourselves.

**Done when:**
- [✅] Outline source: `ttf-parser` 0.25 (`default-features=false`,
      `no-std-float`) — zero-alloc, no_std. (fontdue substituted; see above.)
- [✅] A TTF embedded in the kernel: Noto Sans Regular (SIL OFL 1.1),
      `include_bytes!` in `kernel-x86_64/src/font.rs` (`assets/`).
- [✅] `fb_draw_text(x, baseline_y, str, px, color)` flattens outlines
      (lines + quad/cubic Béziers) into a fixed stack edge buffer and
      scanline-fills (even-odd, 1-bit; AA deferred to M8) via M6's fb_fill_rect.
- [✅] DEMO 37 renders a string at 16/24/40px, verified by pixel readback
      (60+ glyph px, <80% coverage, proportional to size). 114 PASS / 0 DF.
- [✅] Follow-up — routing a *console* through this: `tty::TtyConsole`
      (`78ae59e`, DEMO 39) renders a cursor-managed console (newline, wrap,
      region scroll) via `font::with_face`/`FaceCtx`. NOTE: it's a *region*
      console, not the default `print!` sink (the bitmap stays the boot sink —
      serial is grep truth + the ~16 KiB glyph-raster frame must not run on the
      #41/#55-sensitive interrupt/syscall print path). It's the M19 renderer.
- [ ] Follow-ups still open: kerning/shaping, a glyph cache (re-parses the
      face per `with_face` call today).

## M8 — 2D vector rasterizer (tiny-skia) `[✅]`

Anti-aliased lines/curves/fills for the design apps. Landed `cb6c726`.

**Two things landed together:**
- **Kernel global allocator** — the existing 16 MiB free-list heap arena
  (`kernel_core::memory::heap`, init'd at boot) is now wired as
  `#[global_allocator]` in kernel-x86_64 + `extern crate alloc`. The kernel
  has `Box`/`Vec`/`String` (kernel-core itself stays no-alloc — this is
  binary-side). Unblocks tiny-skia and future kernel work (TTY/shell/agent).
- **tiny-skia 0.11** (cached; `default-features=false` + `no-std-float` →
  no_std + alloc, Apache-2.0). `kernel-x86_64/src/gfx2d.rs` rasterizes paths
  with real AA into an in-heap `Pixmap`, then blits to the M6 framebuffer.

**Done when:**
- [✅] `tiny-skia` as a no_std + alloc dependency (NOT vendored/no_alloc —
      the kernel-allocator route is cleaner and unblocks alloc generally).
- [✅] `gfx2d::aa_scene` (fill + stroke) over M6's `fb_blit`. (Generic
      `fb_stroke_path`/`fb_fill_path` wrappers are the obvious next step.)
- [✅] DEMO 38 draws a filled circle + a stroked cubic Bézier; verified by
      pixel readback — 19748 lit px incl. 974 *blended* AA-edge px (the AA
      signature M7's 1-bit fill lacked). 116 PASS standard / 130 with -netdev.
- [✅] Follow-up — AA text: `gfx2d::aa_draw_text` (`78ae59e`) rasterizes TTF
      glyph outlines through tiny-skia with `anti_alias = true` and blits;
      it's the `Aa::Smooth` mode of `tty::TtyConsole` (DEMO 39: 1661 AA-edge px).
- [ ] Follow-ups still open: grow a real drawing API; gradients/clips.

## M9 — NVMe driver `[✅]`

Block storage on real hardware (**P1 stage only** — the T540 is SATA, not
NVMe, see Phase 10). QEMU's NVMe model proved the bring-up in-tree. v1 landed
`53cdc1a` (DEMO 62).

**Companion:** AHCI/SATA driver landed `ed2630f` (DEMO 67) — the T540 path.
Same `BlockDevice` shape; both NVMe and SATA register at boot, whichever
hardware is present takes effect. See `kernel-x86_64/src/ahci.rs`.

**Done when:**
- [✅] PCI discovery of NVMe controller (class 0x010802) — `find_by_class`
- [✅] Submission/completion queue pair setup (admin + I/O qid 1, polled)
- [✅] Identify Namespace (NSZE + active LBA format) — pulls block_count + block_size
- [✅] Read/Write commands via I/O SQ/CQ (NVM opcodes 0x02/0x01, PRP1)
- [✅] Wired as a `BlockDevice` named `nvme0` (`drivers::registry`)
- [✅] DEMO 62 writes a pattern to LBA 100 + reads it back byte-for-byte;
      first-boot validation 146 PASS / 0 FAIL / 0 #DF
- Follow-ups: MSI-X (interrupts vs polled), multi-block PRP lists, error
  recovery beyond a polled timeout. v1 = one LBA per command (BlockDevice
  layer loops), no interrupts.

---

# Phase 10 — Bare-metal readiness + Wi-Fi

Goal: the kernel boots on real hardware, runs the same DEMOs, and can
reach api.anthropic.com over Wi-Fi (currently TLS works via QEMU SLIRP
forwarding).

**Two-machine bring-up (T540 on the way 2026-05-28):**
- **Stage 1 — ThinkPad T540 (ACQUIRED, on the way).** i7-4600M Haswell,
  8 GB RAM, 256 GB SATA SSD, Win10 preinstalled. Removable mini-PCIe
  Wi-Fi (likely Intel 7260 AC), Intel HD 4600 iGPU only. Validate the
  **bootloader + kernel on real metal** (M10 pre-flight, first-boot,
  USB, task#40 on a real APIC), then **Wi-Fi (M11)** via iwlwifi 7260
  (different firmware blob than AX211 but same driver shape — our M11
  v1 PCI ID table + frame builders cover both already).
  **T540 deltas vs the earlier T440p plan:**
  - **SSD is SATA, not NVMe** (T540-era predates factory NVMe). M9 NVMe
    does NOT exercise here — that waits for the P1. To use the internal
    disk on the T540 we need a new **AHCI/SATA driver**. For initial
    metal bring-up we can avoid that by booting from USB.
  - **USB Mass Storage** class becomes a meaningful goal: boot-from-USB
    needs it on metal, independent of any AHCI work.
  - Windows 10 stays on the disk; we dual-boot off USB. Disable Secure
    Boot in firmware.
- **Stage 2 — ThinkPad P1 Gen 6 (the real target, later).** Only once
  proven on the T540. This is where **GPU work begins** (Phase 11/12,
  Iris Xe + NVIDIA) AND **M9 NVMe gets its first real-hardware test** —
  the T540's SATA SSD means M9 stays QEMU-only until then. HD Audio
  (M15), CDC-ECM, HID parser, 802.11 protocol layer are all QEMU /
  canned-test validated and ready for either machine.

## M10 — Pre-flight checklist for bare-metal boot `[🔨 v1 audit + watchdog]`

Find and fix everything that "passes in QEMU, fails on metal" before
the first real-hardware session. v1 landed `d77ba87` — audit + watchdog
+ one fixed latent bug.

**Done when:**
- [📝] Serial-over-USB plan documented — for now: skip serial entirely on
      the T540 (its serial header is internal and atypical) and rely on
      the framebuffer console as the only output channel. Revisit if a
      USB-serial debug path matters; for the T540 framebuffer is enough.
- [✅] **Framebuffer-only fallback verified** — `serial::_print` already
      mirrors output to `framebuffer::_print`, and the panic handler uses
      `println!`, so panics ARE visible on metal-without-serial. No code
      change needed; documented in code.
- [✅] **xHCI CSZ=1 (Intel 64-byte contexts)** — landed `8821df1`. The
      previous "abort on CSZ=1" branch is gone; `InputContext` /
      `DeviceContext` are raw byte buffers sized for the max (CSZ=1)
      layout, and accessors compute offsets using a runtime `CTX_SIZE`
      set once during xhci bring-up. CSZ=0 regression-clean in qemu-xhci
      (same 165 PASS); CSZ=1 path will exercise on the T540 day-one.
- [✅] **RTC firmware-century-byte assumption** — `rtc.rs:65-226` reads
      the ACPI FADT-set CENTURY register with a 0-fallback. Already
      handles real-BIOS variance.
- [📝] VT-d disabled in BIOS OR identity-IOMMU implemented — BIOS knob;
      no code. Confirm during T540 first-boot.
- [✅] **"Kernel didn't crash" watchdog — CONTINUOUS** (`34baed1`). The
      dedicated `kernel_idle_task` (slot 6) prints `[heartbeat] T+Ns —
      alive (ticks=N)` every 5 s forever; init uses `loop { SYS_SLEEP(N) }`
      so the scheduler is forced to give the idle task full slices.
      Real bugs fixed in the chase: (1) `TIMER_TICKS` was `spin::Mutex<u64>`,
      latent ISR-vs-reader deadlock; now `AtomicU64`. (2) The tick rate is
      `SCHEDULER_TICK_HZ = 62`, not 100; `semos_std::time` + the panic-log
      recovery script also fixed. The original "scheduler pick_next bug"
      claim was misdiagnosed — instrumentation showed the scheduler was
      fine; `hlt`-only idle was just too coarse to give slot 6 forward
      progress. See [[bug_scheduler_picknext_freshly_spawned]] memory.

## M11 — iwlwifi driver `[🔨 v1 protocol layer + PCI probe banner + CSR sanity read (HW_REV, RF_ID, HW_IF_CONFIG, GP_CNTRL); firmware upload next]`

802.11 over Intel WiFi. Two-stage hardware bring-up: T540 (7260/3160
mini-PCIe) first, then P1 Gen 6 (AX211). v1 in-tree protocol scaffolding
landed `a0d487b` (DEMO 65) since QEMU emulates no wireless — everything
else here waits for a T540 in hand.

**Done when:**
- [✅] **802.11 MAC: management frame builders** (Probe Request,
      Open Authentication, Association Request) + EAPOL-Key Msg2 —
      byte-validated against the spec layout in DEMO 65
- [✅] iwlwifi PCI device-ID table (T540 7260 family + P1 AX211)
- [ ] Intel firmware blobs (`iwlwifi-...ucode` + `.pnvm`) embedded
- [ ] Firmware upload + secboot succeeds; ALIVE event received
- [ ] PHY init: NVM + PNVM + regulatory + channel calibration
- [ ] WPA2 four-way handshake in software (MIC over derived PTK),
      CCMP encrypt/decrypt offloaded to firmware after keys installed
- [🔨 protocol; bulk now live] **CDC-ECM USB Ethernet path** as the
      fallback so the TLS stack can be exercised on metal before Wi-Fi
      works. Protocol v1 landed `e79a3a3` (DEMO 66): class constants,
      config descriptor walk (control + Ethernet functional + Data alt
      with bulk pair), MAC string decode. **xHCI bulk-endpoint TX/RX is
      now live** (`63fd75e`, used by USB Mass Storage DEMO 69), so the
      remaining work is ~150 lines: a `try_enumerate_cdc_ecm` path that
      mirrors `try_enumerate_mass_storage`, SET_INTERFACE to the data
      alt, then push/pull Ethernet frames via `bulk_in_xfer` /
      `bulk_out_xfer`. Could ship now (QEMU has `-device usb-net`) or
      wait for the T540 hardware to decide on the exact NetDevice wiring.
- [ ] DEMO repeats: associate to a hardcoded SSID, get DHCP, redo the
      Anthropic TLS round-trip over real Wi-Fi

## M12 — DNS resolver `[✅]`

Replace the hardcoded Anthropic IP in DEMO 16.

Landed `f19da16` (agent resolver + integration fix). `kernel-core/src/net/dns.rs`:
A-record query builder, compression-aware response parser, 8-entry cache, UDP
over the shared smoltcp `SocketSet`. The fix that made it work: **wait on
wall-clock (`platform::ticks()`, ~3s) not iteration count, and retransmit
~4×/s** — the agent's 4000-poll loop spent only a few ms, so a warm name
resolved but a cold SLIRP→host lookup timed out; UDP also has no retransmit, so
a datagram dropped pending the 10.0.2.3 ARP was lost. DEMO 34 resolves
example.com + checks the cache; DEMO 16 resolves api.anthropic.com (hardcoded
IP kept as fallback). Skips cleanly without `-netdev`. With network: 121 PASS.

**Done when:**
- [x] UDP socket on top of smoltcp
- [x] DNS request builder (A record, ID + flags + question)
- [x] Response parser (compression pointers handled)
- [x] `dns::resolve(host) -> Option<Ipv4Address>` with cache
- [x] DEMO 16 calls `dns::resolve` first (falls back to hardcoded IP)
- [x] DEMO 34 resolves example.com over SLIRP (10.0.2.3) + cache check

*De-flaked (was "known intermittent, not M12"):* DEMO 27's "sibling Blocked
after sleep" assertion is **FIXED 2026-05-22 (`78ae59e`)** — the fixed-sleep
one-shot now polls (1 tick × up to 200) and succeeds the instant Blocked is
seen ("Blocked after 1 tick").

## M13 — Chunked-transfer-encoding parser `[✅]`

DEMO 16's body preview showed `8d` (the chunk length header) before this.

Landed `d748556` (agent + integration). `kernel-core/src/net/http.rs`:
`decode_chunked(input, out)` (slice-in/slice-out — kernel-core has no
allocator) + `is_chunked(headers)`. Handles multi-chunk, hex/mixed-case
sizes, chunk extensions, trailing headers; errors cleanly on truncated
input. DEMO 33 (4 sub-checks, all green) validates it.

**Done when:**
- [x] `decode_chunked` decoder that produces the unchunked bytes
- [x] NetworkLlmProvider de-chunks the body before JSON extraction
- [x] DEMO 33 validates the decoder against crafted vectors (DEMO 16's live
      body preview will show JSON once a real authenticated call is made)

---

# Phase 11 — Rendering + media (post-Phase-9, post-network)

## M14 — iGPU (Iris Xe) rendering driver `[  ]`

3D rendering for the CAD verification view, video playback, retro
games. Intel docs are public; Linux's `i915` is permissively-licensed
reference material. NVIDIA dGPU does **not** get a graphics driver —
stays compute-only.

**Done when:**
- [ ] PCI discovery + MMIO map
- [ ] Display engine init (modesetting via Type-C eDP)
- [ ] Render engine: command streamer, batch buffer submission
- [ ] Simple test: clear screen to a color via the GPU
- [ ] Texture upload + sampling
- [ ] DEMO 27 draws a rotating textured cube

## M15 — HD Audio driver `[✅]`

Prerequisite for games and video playback. v1 landed `3f8fed2` (DEMO 63).
QEMU's `-device intel-hda -device hda-output` proved the full path in-tree.

**Done when:**
- [✅] Intel HDA controller bring-up (reset, GCTL, STATESTS poll)
- [✅] Codec enumeration (root → AFG → first DAC + first Pin Complex)
- [✅] PCM output stream (48 kHz, 16-bit stereo) — BDL + stream descriptor +
      RUN; verbs via the Immediate Command Interface (ICI) since CORB/RIRB
      via DMA was flaky in QEMU on the second verb
- [✅] DEMO 63 plays a 440 Hz sine through a cyclic 4 KiB BDL and verifies
      LPIB advanced (DMA active), 147 PASS / 0 FAIL / 0 #DF
- Follow-ups: CORB/RIRB path (real-hardware preferred), MSI-X interrupts,
  multi-stream / capture (ADC), gapless start (currently the cyclic loop
  has a small click at the buffer wrap; choose a buffer length that's a
  whole number of 440 Hz periods to fix).

## M16 — USB HID gamepad `[✅ parser + bulk xHCI ready; needs real gamepad]`

v1 landed `d4b8e2d` (DEMO 64): the report descriptor parser as a pure
module, validated by canned descriptor + synthetic report. QEMU has no
gamepad device, so live wiring is genuinely hardware-gated. xHCI now has
the bulk/interrupt-endpoint plumbing it would need (`63fd75e`); the
remaining work is fetching the device's report descriptor over a USB
control transfer + routing input reports through the parser → a
Gamepad input device. Small extension once a real gamepad is plugged in.

**Done when:**
- [✅] HID report descriptor parser (real one, not boot protocol) —
      `usb::hid_report::parse` handles short items, Usage Min/Max,
      multi-usage Input items, Output/Feature offset, signed extension
- [✅] Gamepad axis + button report parsing —
      `decode_gamepad()` returns `{x,y,z,rx,ry,rz,hat, buttons:u32}`
- [✅] DEMO 64 parses a canonical Game Pad descriptor (X+Y signed 8-bit
      + 4 buttons + padding) and round-trips a synthetic report
      (x=66, y=-2 sign-extended, buttons=0b1010). 150 PASS / 0 FAIL.
- Follow-ups: fetch a HID Report Descriptor over a USB control
  transfer in xHCI; route input reports through the parser; expose a
  Gamepad input device. All hardware-gated.

## M17 — Software video decoder (H.264 minimum) `[  ]`

Playback, not editing. Editing is post-EOY.

**Done when:**
- [ ] H.264 baseline profile decoder (vendored or own)
- [ ] Audio sync via M15
- [ ] DEMO 30 plays a short test clip from ramfs

---

# Phase 12 — Compute-only NVIDIA dGPU path

Local LLM inference as a v2 alternative to remote-via-Wi-Fi.
Tinygrad-NV-style: PTX direct submission, no graphics.

## M18 — NVIDIA dGPU compute driver `[  ]`

**Done when:**
- [ ] PCI discovery + MMIO map
- [ ] GSP firmware upload
- [ ] Channel allocation + DMA buffer mapping
- [ ] PTX kernel submission via host queue
- [ ] CUBLAS-equivalent for the matrix shapes Claude-small needs
- [ ] DEMO 31 runs a single transformer layer forward pass and
      prints the output

---

# Phase 13 — Self-development on the metal

Goal: the user sits at the ThinkPad P1 running Semantic OS, opens a
Claude Code-equivalent agent on the framebuffer, asks Claude to
modify the kernel, sees the change applied to source files on disk,
triggers a build, reboots into the changed kernel. North star — the
moment Semantic OS hosts its own development loop, every subsequent
phase moves faster.

Depends on: Phase 9 done (FS + paths + syscalls), Phase 10 done
(Wi-Fi + DNS, so the agent can reach Anthropic). Framebuffer +
fonts (M6 + M7) are visual prerequisites.

## M19 — TTY layer `[✅]`

The framebuffer console is write-only today. A shell needs bidirectional.
**Done 2026-05-22** — renderer + stdin line-editing/history + ANSI output +
per-process stdio + scrollback all landed and boot-validated (DEMO 39–44,
145 PASS). Remaining nice-to-haves (ANSI scroll-region escapes, raw/cbreak
mode) deferred to when M20/M22 actually need them.

**Renderer (`78ae59e`, DEMO 39):** `tty::TtyConsole` — cursor-managed console
with newline, wrap, region scroll, fg/bg color, M7-sharp / M8-AA glyph modes.

**stdin + ANSI (`716eafd`, DEMO 40):** cooked-mode line discipline + AnsiTty.

**Per-process stdio — full FD-table refactor (`673d948`+`efd444e`+`21dbd8f`,
DEMO 41/42):** every FD (console/pipe/path/ramfs) now lives in the running
process's `FdTable`; the global `PATH_FDS`/`PIPE_FDS` statics are gone. stdio
is routable (`dup2` a pipe onto fd 1) and **inherited across spawn**, so a
parent can redirect a child's stdio. Resolved via the live scheduler slot
(not the stale `current_pid()`), with a stale-`task_id` slot-reuse fix.
Validated 140 PASS / 0 FAIL / 0 #DF.

**Done when:**
- [✅] Buffered stdin with line-editing — cooked-mode line discipline
      (`tty::input_push`/`drain`) with an in-line cursor, mid-line insert/
      Backspace, **arrow keys** (`ESC[A/B/C/D`, emitted by PS/2 0xE0 + USB HID)
      and **8-entry command history** (Up/Down). Surfaced as `SYS_READ` fd 0.
      DEMO 43.
- [✅] ANSI escape sequence handler (`tty::AnsiTty`): SGR color
      (30-37/90-97/39/0), clear screen (`2J`), clear-to-eol (`K`), cursor
      position (`H`/`f`). Cursor positioning uses a nominal cell width (font
      is proportional). Scroll-region escapes not yet parsed.
- [✅] Scrollback — `TtyConsole` line-oriented scrollback ring (64 lines);
      `show_scrollback(top)` re-renders scrolled-off output. DEMO 44.
- [✅] Per-process stdin/stdout/stderr — done via the full per-process
      `FdTable` refactor (DEMO 41 routable stdout, DEMO 42 inherited-on-spawn).
- [✅] DEMOs 40–44: stdin+ANSI (40), pipe-redirected stdout (41), FD
      inheritance across spawn (42), line editing + history (43), scrollback
      (44). (Next free DEMO is 45.)

## M20 — Native shell (`sem-sh`) `[✅]`

Rust shell — no bash compatibility, just what we need. `user-programs/sem-sh`,
built on `semos-std`. **Done 2026-05-23** across stages A (`5398720`), B
(`b81251d`), C (`96fbaf9`); DEMO 45/46; 150 PASS.

**Done when:**
- [✅] Line editor on top of M19 with history (arrows + Up/Down); reads cooked
      lines via `SYS_READ(0)`.
- [✅] Command parser: argv splitting + quoting + `;`/newline + `$VAR` + the
      `< > >> |` metacharacters.
- [✅] Builtins: `echo`/`pwd`/`cd`/`exit`/`true`/`false`/`cat`/`ls`/`which`/`env`
      (`cat` with no args is a stdin filter; `env` prints named vars only).
- [✅] Exec native ELF programs via `process::Command` (`name` → `/bin/name`).
- [✅] Pipes (`|`) and file redirection (`>`, `>>`, `<`) — concurrent (external
      producers spawn under the scheduler; see follow-ups). Exposed two kernel
      fixes: SYS_WRITE now routes through `handle_fwrite` (so a redirected file
      fd 1 actually writes the file), and Path `handle_fwrite` is positional
      (sequential writes accumulate, not overwrite).
- [ ] Job control deferred to a follow-up; not in v1.
- [✅] DEMO 45 (REPL/builtins) + DEMO 46 (`echo > file; cat file; echo | cat`).

**Follow-ups:**
- [✅] per-fd pipe-end refcounting (`0b4a6bb`) — readers/writers counts; dup
      increments, close decrements, EOF at 0. Removed the shell's fragile
      close-ordering dependency.
- [✅] `>>` true-append (`763188a`) — `>` truncates, `>>` seeks to EOF; relies
      on the positional Path writes from stage C.
- [✅] **concurrent pipes** (`9d89dbb`) — external producer stages spawn
      concurrently (Command::spawn, no wait); the consumer blocks in user space
      on a WOULDBLOCK sentinel until EOF. Built on: spawn-inherit pipe
      refcount increment + **exit-time FD cleanup** (a producer's exit drops
      its write-end ref → consumer sees EOF). DEMO 46 `/bin/hello-std | cat`.
- [ ] bare `env` enumeration (needs an enumerate syscall).

**Gotcha (cost time in stage A):** a new user crate builds as PIE (ET_DYN)
unless it copies `build.rs` + `link.ld` + `.cargo/config` (non-PIE EXEC at
0x400000) — the kernel applies no relocations, so `println` crashes while raw
syscalls work. See `feedback_new_user_program_nonpie.md`.

## M21 — Native editor `[✅ v1]`

Edit source files in-place. Not vim-compatible, just usable. v1 landed
`94581a8` (DEMO 61): kernel-side modal editor, launched by sem-sh's `edit
<file>` (SYS_EDIT → `Platform::run_editor` → `editor::run`).

**Done when:**
- [✅] Modal (vi-style) — chosen with the owner; Normal/Insert/Command
- [✅] Open/save against FS Stage 3 syscalls (save = truncate + write)
- [✅] Basic Rust syntax highlighting (keywords + strings + comments +
      numbers) via the M7 TTF renderer; full tree-sitter is later
- [✅] Search (`/term` + `n`); **replace deferred** to a follow-up
- [ ] Multi-file open (tabs or buffers) — deferred; v1 is single-buffer
- [✅] DEMO 61 opens a file, edits a line (gg→o→insert→Esc→:w), saves,
      re-reads to verify; 144 PASS / 0 FAIL / 0 #DF headless
- Follow-ups: search-and-replace, multi-buffer, and the **Ring-3 port**
  (needs a user-space framebuffer surface — open M6 follow-up); v1 is
  kernel-side, reusing the agent-TUI stack.
- Keys: `h j k l`+arrows, `0 $`, `i a A o O`, `x`, `dd`, `gg`, `G`,
  `/`+`n`; `:w :q :q! :wq :x`; Insert: text/Enter/Backspace/Tab/Esc.

## M22 — Claude agent client (native Rust port) `[✅ agent loop live]`

The reason for all of the above. A TUI agent like Claude Code but
written for this kernel, talking to the Anthropic API over the
TLS stack from Phase 8 + Wi-Fi from Phase 10.

**Stage A (`34ef9ee`, DEMO 47):** the agent *core*, no network — lives in
`kernel-x86_64/src/agent.rs` (alloc + kernel syscall/TLS surface; the native
TUI Ring-3 wrapper is a later refactor, needs TLS exposed to Ring-3).
**Stage B (`9da1f51`, DEMO 48):** request over **live TLS** to api.anthropic
.com — `build_http_request` + `send_over_tls` → HTTP 401 round-trip (no key).
Required the TcpStream reconnect fix (`efd8c3c`: free the smoltcp socket on
Drop) so the agent can open a fresh connection per call.

**Stage C (DEMO 49):** the full reasoning loop, validated against the **live
Anthropic API** (`claude-haiku-4-5`). The kernel seeds `/README`, asks Claude
to read it via the `read_file` tool, runs the tool, replays
`assistant tool_use` + `user tool_result`, and gets back the summary. Required
two net-reliability fixes (the real engineering content): a **rotating
ephemeral local port** (the const port hung the 3rd+ reconnect on TIME_WAIT)
and a **30 s IO idle timeout** (10 s was shorter than an LLM's time-to-first-
byte), plus a **3× retry** in `send_over_tls` for the residual single-socket
reconnect flake. The key is supplied at compile time via
`option_env!("ANTHROPIC_KEY")` so it only ever lands in the gitignored binary;
DEMO 49 self-skips in the committed keyless build.

**Stage D (DEMO 50 + DEMO 49 integration):** the **TUI** — a kernel-side
three-pane terminal (`tui.rs`: status bar / scrollback transcript / prompt)
over the M7/M8 `TtyConsole` panes, with role-coloured turns (user / assistant /
tool_use / tool_result). DEMO 50 verifies every pane + role colour headlessly
by pixel readback (Sharp glyphs fill solid colour, so each role's exact colour
is counted). DEMO 49's **live** loop drives the same panes as the conversation
unfolds — `set_status` while it connects/runs a tool/thinks, `push_*` per turn —
so it's the real agent UI, not a mock. (Adding the module overflowed the
`init_loader` task stack at DEMO 26 → bumped `TASK_STACK_SIZE` 128→256 KiB.)

**Stage E (DEMO 51 + DEMO 49 prompt):** **interactive keyboard input**. The
cooked-mode line discipline (`tty::input_push`, fed by the PS/2 ISR and a new
`pump_keyboard` USB-HID poll) → `tty::peek_line` snapshot → `Tui::read_line`,
which echoes the in-progress line into the prompt pane (Backspace + arrow
editing all work) and returns the committed line on Enter. DEMO 51 validates
the path headlessly by injecting keystrokes (incl. an edit + Backspace),
pixel-checking the prompt echo, then confirming `read_line` returns the
assembled line. DEMO 49's live loop now reads its question through `read_line`
(real keyboard on metal; injected headless) → so you type a question and Claude
answers in the TUI.

**Done when:**
- [x] TUI render loop on M19/M20 — **side-by-side split panes** (status bar /
      conversation | activity / prompt): user+assistant turns on the left,
      tool_use+tool_result on the right, role colours, scrollback. DEMO 50
      verifies the split by pixel readback (conversation colours land only in
      the left rect, tool colours only in the right — no bleed across the
      divider). Live loop renders into it (DEMO 49).
- [x] Interactive keyboard input — `Tui::read_line` over the cooked line
      discipline + USB/PS2 pump, prompt echo + editing (DEMO 51); the live agent
      reads its question through it (DEMO 49).
- [x] Agent message loop — full send→parse→tools→resend loop live (DEMO 49).
- [x] Tool use: `read_file`/`write_file`/`bash` all live. `bash` spawns
      `/bin/sem-sh -c` and captures stdout (DEMO 52); `grep` added as a sem-sh
      builtin so it's reachable through `bash` (and `ls` covers directory
      listing). True wildcard `glob` expansion in the tokenizer is still open.
- [x] Multi-turn conversation — message model + multi-turn request building +
      tool_use/tool_result replay validated; context truncation/window still open.
- [~] API key: compile-time `option_env!` works; `/etc/anthropic-api-key`
      runtime load is the remaining persistent mechanism.
- [x] DEMO (stage C, DEMO 49): boots, asks Claude to read README and summarize;
      agent calls `read_file`, returns the summary (live key + net). ✅

**Remaining for a full Claude-Code-equivalent:** the Ring-3 TUI wrapper (needs
TLS exposed to Ring-3), `bash`/`grep`/`glob` tool dispatch, and context-window
management. The core loop — the hard part — is proven.

## M23 — Build pipeline (cross-build over network) `[  ]` — OPTIONAL FALLBACK

User has chosen Phase 14 (self-hosting on the metal) as the
committed build path. M23 stays in the roadmap as a fallback in
case Phase 14 stalls badly enough that we still want the
"changes-edit-reboot" loop working in the meantime via a network
build. **Skip this milestone unless Phase 14 hits a wall.**

If picked up:

**Done when:**
- [ ] Network protocol for "push these files, build, return image"
      (could be: git push → CI webhook → image download; or a
      simpler custom HTTP service)
- [ ] HTTPS POST + GET on top of the TLS transport (currently we
      only do POST in NetworkLlmProvider; need general HTTP)
- [ ] Saved disk image installed to the boot partition
- [ ] DEMO 36 pushes a no-op change, receives the new image, and
      writes it to a staging path (actual reboot is M24)

## M24 — Reboot-into-new-kernel `[  ]`

**Done when:**
- [ ] Replace the running kernel image on the boot device with the
      new one (BIOS/UEFI partition write)
- [ ] Trigger a clean reboot (ACPI / triple-fault / power cycle —
      pick the cleanest available)
- [ ] DEMO 37 (last in this phase): with the agent loop running,
      apply a self-modifying patch (say, change a banner string),
      build, reboot, verify the change is live

## Out of scope for this phase

- **Port rustc + LLVM as-is (Phase 14).** Achievable but bigger
  scope than Phase 13 needs. Moved to its own phase below — the
  realistic shape is **port std + adopt Cranelift**, not "rewrite
  LLVM from scratch."
- **JS runtime port (for running upstream Claude Code as-is).**
  Easier than rustc but still enormous; the native Rust agent
  (M22) bypasses the need.
- **Tree-sitter / LSP** — nice to have but not required for the
  "make a kernel change with Claude's help" loop.

---

# Phase 14 — Self-hosting compilation (M25 ✅, M26 ✅, M27 next)

Goal: rustc + cargo run *on* Semantic OS, building Semantic OS.
**User-chosen committed path** for kernel self-development. The
ThinkPad P1 running Semantic OS hosts its entire dev loop — edit,
build, reboot, no other machine in the loop. Phase 13 M23 (network
build server) is the fallback if this stalls.

**Progress (2026-05-29):** M25 stdlib complete + live-validated (sync-demo,
DEMO 70). M26 Cranelift vendored + cg_clif end-to-end (DEMO 71: rustc-
via-Cranelift → SemOS ELF → SYS_SPAWN → exit 0). The cross-build path
works today; M27 is moving rustc itself onto SemOS so it drives its own
compile loop. See [`SELF_HOSTING_PLAN.md`](SELF_HOSTING_PLAN.md) for the
session-by-session breakdown.

Runs in parallel with Phase 13's M19-M22 + M24 (TTY, shell, editor,
agent, reboot-into-new-kernel — all still needed regardless of where
the compiler runs). Independent of Phases 11/12 (rendering / NVIDIA
dGPU).

## Tracked as a research project on AI-assisted porting

User decision: this phase doubles as **research into AI usage** —
how productive is LLM-driven compiler porting, where does iteration
overhead actually come from, what's the real ratio of generated to
kept code on a project this size? **No wall-clock estimates** here
because they'd be guesses; we measure as we go.

Per-session metrics worth tracking (write into the commit body or
a `docs/RESEARCH-LOG.md` as the phase progresses):

- Tokens generated (rough — agent run length × rate)
- LOC added to repo
- LOC deleted (iteration cost)
- LOC kept after the session (net useful delta)
- Build attempts before clean
- Bugs caught at compile vs at test vs at runtime
- Subjective: was this session bottlenecked by agent throughput,
  by iteration cycles, by underlying-bug debugging, or by
  spec/code-reading time?

After 5-10 sessions we should have honest empirical numbers
to replace the LOC-budget guesses below.

## Starting LOC hypotheses (validate during research, don't trust)

| Component | LOC guess | Notes |
|---|---|---|
| std shim over our syscalls (M25) | ~30K | Probably the highest-iteration component — std's surface is broad and tests are unforgiving |
| Spawn/wait + thread/sync syscalls + scheduler upgrade | ~15K | Kernel-side prerequisite for std::process and std::thread |
| Memory allocator (jemalloc-class minimum) | ~8K | Could vendor an existing one; net new work smaller |
| Vendor + integrate Cranelift (M26) | mostly read+review | Cranelift exists (~150K LOC). Integration is the work |
| First rustc build on Semantic OS (M27) | iteration only | The test-suite phase. Open-ended |
| Self-bootstrap (M28) | the moment, not the work | Validation, not coding |

**Why not port LLVM from C++ to Rust?** ~10M LOC of C++. Cranelift
(~150K LOC of Rust, exists today) gives us a Rust-native codegen
backend that's "good enough" for self-hosting. Drop LLVM entirely
on Semantic OS; keep it on the build server (Phase 13 M23) if we
want the full optimizer.

**Why not run upstream Claude Code (Node.js) instead of M22's
native Rust port?** Node.js + V8 is ~5M LOC of C++. The native
Rust agent (M22) is ~4K LOC, ships with Phase 13.

## M25 — std shim over Semantic OS syscalls `[✅ stdlib complete — `semos-std`]`

Get upstream rustc's std dependencies satisfied on our kernel.
`user-programs/std-shim` (crate `semos-std`) is the implementation;
hello-std / vec-demo / std-demo exercise it as DEMO 29–31.
**Caveat (#54):** shim programs MUST build at `opt-level=0` — any
optimization miscompiles the `asm!`-based syscall wrappers.

**Tier 1 prereqs (all ✅ as of 2026-05-18 — M25 unblocked to start):**
- ✅ Real general-purpose allocator (heap alloc, `9a5850e` — `SYS_HEAP_ALLOC`/`SYS_HEAP_FREE`)
- ✅ argv/envp passthrough in SYS_SPAWN (`8937041` — `setup_user_argv` Platform
  method writes SysV layout to new process's user stack; SpawnArgs struct passed
  via syscall arg3)
- ✅ Per-process env block + CWD (`8a3c29f` — `SYS_GET_CWD`/`SET_CWD`/`GET_ENV`/`SET_ENV`,
  inherit-on-spawn)
- ✅ SYS_FS_* surface (`dfca48f` — already done for std::fs backing)

**Tier 2 prereqs (all ✅ as of 2026-05-19 — M26 "first compile" smoke test unblocked):**
- ✅ SYS_FSYNC (crash-safe writes for cargo) — `9129f19`
- ✅ SYS_RENAME (atomic rename) — `9129f19`
- ✅ SYS_TRUNCATE — `9129f19`
- ✅ FWRITE that handles >256 bytes (heap-Allocated ObjectContent) — `b51e22a`
- ✅ Enriched SYS_STATX (type + mtime + tier + size + suid) — `9129f19`

**Tier 3 prereqs (all ✅ as of 2026-05-19 — parallel/threaded rustc unblocked):**
- ✅ SYS_THREAD_SPAWN / SYS_THREAD_JOIN (kernel-mode `178c96d` + Ring-3 same-AS `5d6e241`)
- ✅ Mutex/Condvar lowering target: SYS_FUTEX_WAIT / SYS_FUTEX_WAKE (`178c96d`)
- ✅ SYS_WAITNB (WNOHANG non-blocking child wait, `178c96d`)
- ✅ SCHEDULER_TICK_HZ const for std::thread::sleep shim (`f6a9824`)
- *Note:* thread-local storage (per-thread `static`s) isn't done — std-shim
  routes TLS through the existing per-process env block for now; revisit if
  upstream std actually requires real TLS for parallel codegen.

**Done when (M25 itself):**
- [✅] `std::fs` routes to `SYS_FS_*` (M4) — `fs::File`/`OpenOptions` + `io::{Read,Write}`, DEMO 31 round-trip
- [✅] `std::process::Command` calls `SYS_SPAWN` / `SYS_WAIT` — `92ccbb5`,
      DEMO 32. Unblocked a broad bug: `AddressSpace::new` now copies the PML4
      from `boot_cr3()` not the live CR3, so a Ring-3 parent spawning a child
      no longer shares (and corrupts) its own page tables. `SYS_WAIT` joins
      the child's scheduler slot (Ring-3 children never hit PROCESS_TABLE
      Zombie). `spawn-demo` validates exit codes 0 and 0x2700 propagate.
- [✅] `std::thread` over a preemptive scheduler with `std::sync::{Mutex, Condvar, RwLock}` — `thread::spawn`+`JoinHandle<T>`, `Mutex`, `Once` done (DEMO 31); **`Condvar` + `RwLock` landed `7276f07`** (futex-backed; seq-counter Condvar avoids lost-wakeup, RwLock single-word reader-count/writer-bit). **Live functional smoke via Ring 3 sync-demo (DEMO 70, `f087124`)**: Condvar wakeup fires, mpsc 1..=5 ordering + disconnect, RwLock 2-reader+writer all pass on metal-equivalent QEMU.
- [✅] `std::net::{TcpStream}` over kernel-core::net — `b332cf0`, `f548688`,
      `8c2cb21`, `a708bc8`. Kernel syscalls SYS_DNS_RESOLVE + SYS_TCP_{CONNECT,
      READ,WRITE,CLOSE,STATE} (100-105, one TCP socket at a time,
      **non-blocking**: one net::poll + one try, NET_WOULDBLOCK sentinel) +
      `semos-std::net` (Ipv4Addr, resolve, TcpStream impl io::Read/Write that
      drives the wait in user space). **DEMO 36: net-demo resolves
      example.com, opens a TcpStream, sends an HTTP GET and reads the response
      end-to-end from Ring 3** (125 PASS with -netdev). Unblocked once the
      **task#40 torn-context-switch #DF was fixed (#56, `8c2cb21`)** — that
      was the real blocker, not the net path. UdpSocket not exposed (DNS is a
      one-shot resolve). Fixed en route: address-space GC in
      store_address_space; user stack decoupled from kernel TASK_STACK_SIZE.
- [✅] `std::env`, `std::path`, `std::time`, `std::collections`, `std::sync::mpsc` — `env` done; `time::{Instant,Duration}` over SYS_TIME (`ef3a3fc`); `path::{Path,PathBuf}` lexical (`d77256d`); `collections::{HashMap,HashSet}` via vendored `hashbrown` 0.15 + `BTreeMap/BTreeSet/VecDeque` from `alloc` (`7276f07`); `mpsc` MPSC channel on Mutex+Condvar+VecDeque (`7276f07`)
- [✅] A "hello world" program built against this std runs on Semantic OS — hello-std/vec-demo/std-demo (DEMO 29–31)

## M26 — Cranelift backend integration `[✅ cg_clif end-to-end on SemOS]`

Avoid the LLVM C++ port by adopting the Rust-native codegen.

**Prep already landed (2026-05-18, commit `8ed4aa7`):**
- ✅ Vendor placeholders + VENDOR_NOTEs in
  `kernel-core/vendor/cranelift/` and `vendor/rustc_codegen_cranelift/`
  pinning the versions (cranelift 0.121.0; cg_clif tied to nightly-2026-02-01).
  Sources themselves NOT YET copied — agent's sandbox blocked network +
  cargo execution; documented re-vendoring procedure in each VENDOR_NOTE.
- ✅ `docs/PHASE_14_CRANELIFT_BRIEF.md` (~450 LOC) — sub-crate
  architecture, MIR→CLIF→x86_64 pipeline, integration plan, 10 known-
  unknowns, what LLVM features we give up.
- ✅ `docs/STD_SHIM_SURFACE.md` — 65 std methods catalogued with their
  syscall dependencies, drove the Tier-1/2/3 prereq list above.

**Done when:**
- [✅] **Cranelift sources fully vendored** (`f1b2635`): `compiler/` host
      crate with cranelift-codegen + cranelift-frontend + cranelift-module
      + cranelift-object 0.122, `cargo vendor --versioned-dirs` pulled in
      44 transitive crates (~25 MB) into `compiler/vendor/`. Deterministic
      offline builds via `.cargo/config.toml`'s `vendored-sources`.
- [✅] **Host smoke**: `cargo run` in `compiler/` builds an IR function
      `i64 add(i64, i64)`, verifies, lowers to x86_64 — emits a 13-byte
      textbook System V function (push rbp; mov rbp,rsp; lea rax,[rdi+rsi];
      mov rsp,rbp; pop rbp; ret).
- [✅] **`rustc_codegen_cranelift` wired in** (`0039b25`) — via the
      rustup-distributed `rustc-codegen-cranelift-preview` component on
      our pinned nightly-2026-02-01. Avoids vendoring rustc-internal
      crates (which aren't on crates.io); `cargo build -Z codegen-backend`
      + `[profile.release] codegen-backend = "cranelift"` is enough. core
      / compiler_builtins stay on LLVM via a wildcard package override
      (cg_clif can't yet lower core's `va_end` intrinsic).
- [✅] **Smoke-on-metal**: `cg-clif-hello.elf` (13,688 B, produced by
      rustc with the Cranelift backend) `SYS_SPAWN`s on SemOS, writes
      its marker via `SYS_WRITE`, and exits with code 0. DEMO 71 PASS.
- [ ] Patches needed for cg_clif to lower `va_end` (so build-std also
      uses it; today it's a wildcard LLVM override for the standard
      library deps). Upstream cg_clif issue — track from the next
      nightly bump.

## M27 — First rustc build on Semantic OS `[🔨 Phase 5c Stage G + H in progress — Cranelift stack closed iters 6–10 (cranelift-{codegen, frontend, module, object} + cg_clif all 0 errors target-side; semos-rustc binary emits a 6.1 MB ELF). Stage H iter 1 closed 5 of 6 broken rustc_* crates via the alloc-prelude sweep (rustc_passes / rustc_mir_build / rustc_mir_transform / rustc_hir_analysis / rustc_hir_typeck), rustc_interface 416 → 37. Iter 2 target: close rustc_interface, then `cargo check -p rustc_driver` should reach 0]`

The cross-build path is live (DEMO 71, rustc-on-cg_clif on the dev box),
and `cranelift-codegen` itself now runs **on** SemOS in `semos-cc`
(DEMO 73, 2026-05-30). Phase 1 recon (4 agents, R1–R4) mapped the dep
graph, audited std surfaces, identified externals, and found exactly one
unmitigated blocker (B1: FatalError/catch_unwind → accepted as
"one-error-per-compile" in v1 per §1.9). Synthesis verdict: PROCEED.

See `docs/M27_RUSTC_PORT_PLAN.md` for the full phase plan,
`docs/m27-port/EXPERIMENT_LOG.md` for the per-stage execution diary
(through Stage F12 — final), and `docs/m27-recon/SYNTHESIS.md` for
reconciled scope estimates.

**Done when:**
- [✅] Phase 1 recon complete (R1–R4, SYNTHESIS.md, `f98c37a`)
- [✅] Plan amended with §1.7/§1.8/§1.9 + Phase 2a/2b split
- [✅] Phase 2a — zero-dep foundation crates (`rustc_data_structures`,
      `rustc_span`, `rustc_index`, `rustc_serialize`, `rustc_arena`,
      `rustc_hashes`, `rustc_graphviz`, `rustc_fs_util`, `rustc_lexer`,
      `rustc_error_codes`, plus semos-std additions: `OnceLock`,
      `thread_local!`, `OsString`, `canonicalize`, `abort_with_code`)
- [✅] Phase 2b — cycle-breakers (`rustc_ast` + `rustc_lint_defs` +
      `rustc_errors`)
- [✅] Phase 3 — middle layer (frontend + semantics clusters)
- [✅] Phase 4 — codegen layer (`rustc_codegen_ssa`, MIR, metadata)
- [✅] Phase 5a/5b — workspace integration: every internal `rustc_*` crate
      brought to `cargo check -p <crate>` clean against `x86_64-unknown-none`.
      **Stages D / E / F1–F12 all closed 2026-06-04.** Notable architectural
      lifts: ena 0.14.4 vendored + no_std-patched at `vendor-externals/ena`
      (drops `log` via a no-op `debug!` macro shim), rustc_incremental
      stubbed wholesale per §1.3, rustc_proc_macro `bridge::client::ProcMacro`
      stubbed, rustc_metadata's FileEncoder paths cfg-gated to no-op on
      SemOS, rustc_codegen_ssa's object/thorin/wasm host-only paths gated
      out per §1.7. **~23,100 errors cleared across 37 patched rustc_*
      compiler crates cumulatively.** Per-stage detail in `EXPERIMENT_LOG.md`;
      the iCloud-backed copy lives at `Work/M27_rustc_port_EXPERIMENT_LOG.md`.
- [🔨] Phase 5c — semos-rustc binary drives `rustc_driver`, compiles
      `fn main() { println!("hi"); }` end-to-end on SemOS (DEMO 80).
      Stage G iters 6–10 (2026-06-07): full Cranelift no_std port + cg_clif
      target-buildable. `cranelift-codegen` 430 → 0 (OnceLock shim, libm
      FloatNoStd trait, ISLE/opcode generator post-processing in build.rs),
      `cranelift-frontend` 7 → 0, `cranelift-module` 10 → 0, `cranelift-object`
      96 → 0 (first-time no_std port), `rustc_codegen_cranelift` 653 → 0
      across iters 7-9 (no_std + 17 rustc_* path deps + ExceptionTableItem
      and FinalizedMachExceptionHandler API stubs in cranelift-codegen +
      cfg-gate driver::aot/global_asm/toolchain/concurrency_limiter to
      host-only). Iter 10: `semos-rustc` binary now compiles + emits a
      6.1 MB ELF target-side via cranelift-object (`c65d3ae`).
      Stage H iter 1 (2026-06-08): alloc-prelude sweep over the 6
      rustc_* crates whose Phase 3 ports left bare `Vec`/`String`/`Box`
      unqualified — 5 of 6 close, rustc_interface 416 → 37. Tooling at
      `tools/m27-alloc-prelude-sweep.sh` for the next wave.
- [ ] Cargo (built against `semos-std`) drives a rustc invocation
      that produces a working binary, ON SemOS
- [ ] The "hello world" test from M25 compiles and runs end-to-end
      on Semantic OS without the cross-build server

## M28 — Self-bootstrap `[  ]`

The capstone moment for Phase 14.

**Done when:**
- [ ] `cargo build --release` of Semantic OS, run *on* Semantic OS,
      produces a working kernel image
- [ ] That image, when booted, can rebuild itself the same way

---

# Future scope — not yet specced, do not start

These are real eventual requirements but not on any current critical
path. List exists so the work isn't forgotten, not so anyone picks
one up speculatively.

- **Video editing** — software encoders, audio mixing, timeline UI,
  effects pipeline, real-time preview, Iris Xe QuickSync hw encode/decode,
  ICC color management. Triples the AV stack work over playback (M17).
  Depends on M14, M15, M17 solid first.
- **Geometry kernel port** — C++ ArchEngine_kernel → native Rust for
  LegibleStudios CAD app. Brief lives at `F:\Software\LegibleStudios\HANDOFF_2026-05-15_VULKAN_KERNEL_PORT.md`.
- **LegibleStudios full port** — Python+PyQt6 → native Rust for the
  whole app. Brief at `F:\Software\LegibleStudios\HANDOFF_2026-05-15_LEGIBLE_STUDIO_RUST_PORT.md`.
- **MarlOS port** — Tauri typesetter → native Rust on this kernel.
- **Marée, Brise, Claw Pen** — utility apps, design pending.

---

# Out of scope, settled — do not re-propose

These were considered and explicitly rejected. Don't reopen without
new information.

- **Linux ABI compatibility layer.** Rejected on security grounds:
  importing Linux syscalls re-imports the Linux attack surface,
  contradicting the kernel's ring-0 LLM-mediation thesis. Native
  Rust everywhere. C++ apps (including the LegibleStudios Vulkan
  engine) must be Rust-ported. This was reversed once during
  scoping and re-rejected — the security argument is why it stays out.
- **AAA / commercial games.** No Vulkan for NVIDIA, no Mesa-equivalent.
  Retro / native-Rust-port games only (M14 + M17 cover what's needed).
- **Complex text shaping** (Arabic, complex CJK). Latin and simple
  scripts only.

---

# Cross-cutting discipline (apply to every milestone)

Distilled from the lessons captured in the project memory file.
These aren't optional — they're how this codebase stays correct
under modification.

- **Round-trip self-tests are necessary but not sufficient for crypto
  primitives.** Always KAT against the controlling RFC's published
  bytes. See `feedback_crypto_kat_discipline.md` in memory. The
  Phase 8 Poly1305 bug hid behind perfect round-trip tests for weeks.
- **kernel-core can't run `cargo test`.** Boot-time DEMOs in
  `kernel-x86_64/src/main.rs` are the validation path. Each DEMO
  prints `PASS:`/`FAIL:` lines grepped from the QEMU serial log.
  Numbering: next free DEMO N is one past the last one in main.rs.
- **Build order:** if any user binary in `user-programs/<name>/`
  changed, `cargo build --release` there first; then
  `cargo build --release` from `kernel-x86_64/`; then
  `cargo run --release` from `x86_64-runner/` for the disk image.
- **QEMU flags that matter:** `-cpu max` (required — RDRAND probe
  aborts without it). `-rtc base=utc` (required for honest wall
  clock; default is `localtime` and adds host-TZ offset).
  `-device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0` (for M3).
  `-drive format=raw,file=vdisk.img,if=virtio` (for VirtIO block;
  create with `qemu-img create -f raw vdisk.img 16M` once).
- **Agent isolation cuts merge headaches but can hide interaction
  bugs.** If launching an agent on a worktree, either rebase the
  worktree onto current `main` before they start, or budget time
  for combined-image revalidation. (Lesson from the USB agent at
  M3 — its standalone tests passed but the merged image crashed.)
- **Don't `cargo test` kernel-core** — 96 pre-existing test errors
  from `#[cfg(test)]` blocks across `users.rs`, crypto modules, etc.
  Tests that need running get a public function + a boot DEMO.

---

# Maintaining this file

When something ships:
1. Flip its `[ ]` to `[✅]` and add the commit SHA in the description.
2. If the milestone uncovered new sub-work, add it to the next
   milestone's checklist or create a new milestone for it.
3. Update the project memory file (one-line entry in `MEMORY.md`
   index + paragraph or two in `project_semantic_os_kernel.md`).
4. If you learned something that applies to all future work, write
   a `feedback_*.md` memory file for it — that's where the
   cross-cutting discipline above came from.

When you reopen something that was `[✅]`:
1. Mark it `[🔨]` again with a one-line "why reopened".
2. Don't delete the prior entry — append.
