# SemOS — Agent Briefing

SemOS is a from-scratch, agent-native, sovereign OS in Rust: `kernel-core` (platform-agnostic) + `kernel-x86_64` + `kernel-aarch64`, user programs in `user-programs/`, and an on-device rustc (`semos-rustc`). The thesis: an LLM agent lives in the OS and extends it at runtime; **security tiers (Public/Internal/Sensitive/Secret) are the capability fence** (`current_task_max_tier()`), with console-only gates for trust operations (`is_vouch_authority()`).

## Dev loop (two machines)

- **This machine (WSL2) = dev.** Edit, build, commit, push here.
- **T540p (Linux) = test target.** User runs `git pull && bash tools/esp-install/build-and-flash.sh` there, reboots into SemOS, tests.
- Keep `origin/main` always pushed — the test machine can only pull what's pushed.
- Getting logs back: SemOS has a `netlog <ip> [port]` shell builtin (UDP, port 9000 default) that drains the kernel log ring to a listener. Run `nc -u -k -l 9000 | tee -a semos.log` on the dev machine; the user runs `netlog <dev-ip>` on the T540p. **WSL2 caveat:** default WSL2 networking is NAT'd and unreachable from the LAN — needs `networkingMode=mirrored` in `C:\Users\<user>\.wslconfig` + `wsl --shutdown` (user does this on the Windows side), or use the Mac as the listener instead.

## First run on a new dev machine (do this unprompted, then report)

1. Verify the toolchain builds: `cd kernel-x86_64 && cargo build --release` (needs nightly + `rust-src` component; the repo's `.cargo/config.toml` sets `build-std`).
2. **Ask the user for the Kimi key** and write it to `<repo>/.kimi-key` (`chmod 600`). It is **gitignored by design** and does not travel over git. Without it the kernel bakes an empty key and `ask`/`agent` fail on-device with "no ANTHROPIC_KEY configured in this build".
3. Confirm the key lands in the ELF: `strings kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64 | grep -c "sk-kimi-"` should print ≥1 after a build with the key present.
4. Report ready, with the exact pull/flash command for the T540p.

## Hard rules (the user holds these firmly)

- **Never commit secrets.** `.kimi-key` and `.anthropic-key` stay gitignored. Before every commit: `git diff --cached | grep -icE "sk-kimi-[a-z0-9]{6}"` must print 0.
- **Commit/push only when the user asks.** Report and wait.
- The LLM endpoint is compile-time config via `option_env!` in `kernel-x86_64/src/agent.rs` (KIMI_API_KEY/KIMI_BASE_URL/KIMI_MODEL, defaults to Kimi `api.kimi.com/coding`, model `kimi-k2.7`). Changing the key requires a rebuild — `build.rs` has `rerun-if-env-changed`, and `build-and-flash.sh` auto-sources `.kimi-key`.
- Empty-but-set env vars must be treated as absent (see `first_nonempty` in agent.rs) — the user's shell exports empty ANTHROPIC_* vars.
- The Kimi model is a reasoning model: requests must send `"thinking":{"type":"disabled"}` or the response is a huge thinking block that overflows the 8 KB response buffer and yields no text.

## State of the work (as of 2026-08-03, commit 7edd109)

- **Done:** Rungs 1–3 of the agent harness — `run_agent` tool loop (read_file/write_file/bash, live-rendered in the `agent` TUI), tier-clamped to Public, `compile` tool driving on-device `semos-rustc`. netlog (SYS_NETLOG=132). Ctrl+C aborts the agent loop. M53/M54 validated on hardware (real TLS + first agent session).
- **Queued next: TUI scrollback/overflow fix.** Diagnosis done: `TtyConsole` (kernel-x86_64/src/tty.rs) manages a cursor but the rasterizers (`font.rs fill_glyph`, `framebuffer.rs fb_blit`) clip only to the global framebuffer, never to the pane rect — so text spills over the divider into adjacent panes. `show_scrollback` (tty.rs:122) also redraws whole logical lines with no wrap. Fix = pane-relative clipping in the glyph/blit path + wrap-on-redraw.
- **Specced, not built:** iCloud access via companion relay (SemOS ships bytes over the SYS_PAIR channel; the companion app does Apple's OAuth). Belongs to the Phase 16/18 phone arc.
- `semos-rustc` on-device is ~iter 8: compiles trivial no_std/no_main sources only (DEMO 80 shape). Don't promise the agent arbitrary Rust compilation yet.

## Reference points

- Roadmap: `docs/MASTER_ROADMAP.md` (milestone discipline + phase map).
- Agent harness: `kernel-x86_64/src/agent.rs` (`run_agent`, `run_tool`, `AGENT_TIER` = the agent's clearance mutation point).
- Pairing protocol: `docs/pairing-v1.md` + canonical vectors in `docs/pairing-v1-test-vectors.md`.
- Patina (userland automation language, tier-aware capabilities): `docs/Patina_v0_Spec.md`.
