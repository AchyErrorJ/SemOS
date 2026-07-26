//! Boot-time demos — agent / shell / TUI features (M22+).
//!
//! Extracted from `main.rs` to keep the boot file readable. **New demos go
//! here**, not in main.rs. These are dependency-clean (each uses `crate::agent`
//! / `crate::tui` / `kernel_core` via local `use`s); `tty` and the print
//! macros are the only crate-level imports needed.
//!
//! TODO (future): the OLDER demos still in `main.rs` (DEMO 0-46 era) are
//! interleaved with boot/runtime helpers (`spawn_named`, `user_syscall`,
//! `pump_keyboard`, `enable_sse`, `sem_demo_one`, `StatX`/`FutexWord`, …) with
//! no clean cut. Migrating them means making those helpers `pub(crate)` +
//! rewriting cross-refs, in layout-validated stages. Do it when it matters, or
//! when the kernel stack/layout is being reorganised anyway (a re-layout is
//! coming regardless, so fold this in then).

use crate::tty;
use crate::println;
use alloc::boxed::Box;

/// DEMO 47: M22 Claude agent core (no network). Exercises the agent's
/// Messages-API request framing, response parsing (text + tool_use), and tool
/// dispatch (write_file then read_file) end-to-end with canned data — the
/// reasoning machinery that Stage B (live TLS) and Stage C (loop+TUI) drive.
pub(crate) fn agent_demo() {
    use crate::agent::{self, Message};
    use alloc::string::String;

    // 1. Request framing: a real request body with system + user msg + tools.
    let msgs = [Message::text("user", "Read /agent-test.txt and summarize it.")];
    let req = agent::build_request("claude-opus-4-7", 1024, "You are a helpful agent.", &msgs);
    let req_ok = req.contains("\"model\":\"claude-opus-4-7\"")
        && req.contains("\"tools\":")
        && req.contains("read_file")
        && req.contains("Read /agent-test.txt");
    if req_ok {
        println!("  [DEMO 47] PASS: built Messages-API request ({} B, model+system+tools+messages)", req.len());
    } else {
        println!("  [DEMO 47] FAIL: request framing");
    }

    // 2. Parse a tool_use response (as Claude would return).
    let canned_tu = "{\"id\":\"msg_1\",\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read_file\",\"input\":{\"path\":\"/agent-test.txt\"}}],\"stop_reason\":\"tool_use\"}";
    let r = agent::parse_response(canned_tu);
    let tu_ok = match &r.tool_use {
        Some(t) => t.name == "read_file" && t.id == "toolu_1" && t.input_json.contains("/agent-test.txt"),
        None => false,
    };
    if tu_ok {
        println!("  [DEMO 47] PASS: parsed tool_use → name=read_file id=toolu_1 input has path");
    } else {
        println!("  [DEMO 47] FAIL: tool_use parse");
    }

    // 3. Tool dispatch: write_file then read_file (the inputs Claude sent).
    let _ = agent::run_tool(
        "write_file",
        "{\"path\":\"/agent-test.txt\",\"content\":\"AGENT_FILE_OK\"}",
    );
    let read_result = match &r.tool_use {
        Some(t) => agent::run_tool(&t.name, &t.input_json),
        None => String::new(),
    };
    let tool_ok = read_result.contains("AGENT_FILE_OK");
    if tool_ok {
        println!("  [DEMO 47] PASS: tool dispatch — write_file + read_file round-tripped \"AGENT_FILE_OK\"");
    } else {
        println!("  [DEMO 47] FAIL: tool dispatch got {:?}", read_result);
    }

    // 4. Parse a text response.
    let canned_txt = "{\"content\":[{\"type\":\"text\",\"text\":\"The file says it is OK.\"}]}";
    let rt = agent::parse_response(canned_txt);
    let text_ok = rt.text.as_deref() == Some("The file says it is OK.");
    if text_ok {
        println!("  [DEMO 47] PASS: parsed assistant text response");
    } else {
        println!("  [DEMO 47] FAIL: text parse got {:?}", rt.text);
    }

    // 5. Build a follow-up request carrying the tool_result.
    let follow_ok = if let Some(t) = &r.tool_use {
        let follow = [
            Message::text("user", "Read /agent-test.txt and summarize it."),
            Message::tool_result(&t.id, &read_result),
        ];
        let req2 = agent::build_request("claude-opus-4-7", 1024, "", &follow);
        req2.contains("tool_result") && req2.contains("toolu_1") && req2.contains("AGENT_FILE_OK")
    } else {
        false
    };
    if follow_ok {
        println!("  [DEMO 47] PASS: follow-up request carries the tool_result back to the model");
    } else {
        println!("  [DEMO 47] FAIL: tool_result follow-up");
    }

    if req_ok && tu_ok && tool_ok && text_ok && follow_ok {
        println!("  [DEMO 47] => M22 stage A: agent protocol + tools work (read_file/write_file); next: live TLS");
    }
}

/// DEMO 48: M22 stage B — send the agent's Messages-API request to
/// api.anthropic.com over the Phase-8 TLS transport and read the response.
/// With no API key we expect HTTP 401 — which proves the agent's request was
/// framed correctly, encrypted, sent, and a parseable HTTP response came back
/// (the same "401 round-trip" Phase 8 used as its acceptance). Stage C adds
/// the key, the conversation loop, and the tool round-trips.
pub(crate) fn agent_live_demo() {
    use crate::agent::{self, Message};

    // Build a real Messages-API request body (model + tools + one user turn).
    let msgs = [Message::text("user", "Say hello in one short sentence.")];
    let body = agent::build_request("claude-haiku-4-5-20251001", 64, "", &msgs);
    // No key → expect 401. (Stage C reads it from /etc/anthropic-api-key.)
    // One-shot: Connection: close + read-until-EOF via send_over_tls.
    let http = agent::build_http_request(&body, "", false);
    println!("  [DEMO 48] sending {}-byte agent request over TLS (no key → expect 401)...", http.len());

    let mut resp = [0u8; 4096];
    match agent::send_over_tls(http.as_bytes(), &mut resp) {
        Ok(n) => {
            let resp = &resp[..n];
            let status = agent::http_status(resp).unwrap_or(0);
            println!("  [DEMO 48] received {} bytes, HTTP status {}", n, status);
            if status == 401 {
                println!("  [DEMO 48] PASS: agent request reached Anthropic over TLS — 401 (auth) as expected, no key");
                println!("  [DEMO 48] => M22 stage B: request framing + TLS send/recv work end-to-end; stage C adds key + loop");
            } else if status != 0 {
                // Any HTTP status still proves the round-trip; a key would 200.
                println!("  [DEMO 48] PASS: agent round-trip OK (HTTP {} — request reached the API)", status);
            } else {
                println!("  [DEMO 48] FAIL: no HTTP status parsed from {} bytes", n);
            }
        }
        Err(e) => {
            // Network/TLS issue (intermittent SLIRP); not an agent-logic failure.
            println!("  [DEMO 48] SKIPPED: transport error ({}) — retry boot if -netdev flaked", e);
        }
    }
}

/// DEMO 50: M22 TUI — lay the split-pane agent terminal over the framebuffer
/// (status bar / conversation | activity / prompt) and render a representative
/// conversation, then verify headlessly by pixel readback. Because `Aa::Sharp`
/// fills glyphs in a *solid* colour, we confirm each role rendered in its exact
/// colour by counting matching pixels — and confirm the LEFT pane holds the
/// conversation (user/assistant) while the RIGHT pane holds tool activity
/// (tool_use/tool_result), with no colour bleed across the divider: that mutual
/// exclusion proves the side-by-side split. Mirrors the DEMO 35/39 discipline:
/// do ALL readback into locals first (the boot console scrolls the whole
/// framebuffer on a newline), then print verdicts.
pub(crate) fn agent_tui_demo() {
    use crate::tui::{self, Tui};

    let mut t: Box<Tui> = match Tui::new("claude-haiku-4-5") {
        Some(t) => Box::new(t),
        None => {
            println!("  [DEMO 50] SKIPPED: no framebuffer");
            return;
        }
    };

    // Drive the panes exactly as the agent loop would as a turn unfolds.
    t.set_status("thinking");
    t.push_user("Read /README and summarize it.");
    t.push_assistant("I'll read the file first.");
    t.set_status("running read_file");
    t.push_tool_call("read_file", "{\"path\":\"/README\"}");
    t.push_tool_result("Semantic OS is a bare-metal x86_64 Rust kernel with a four-tier LLM security model.");
    t.push_assistant("Semantic OS is a bare-metal x86_64 Rust kernel with a four-tier LLM security model.");
    t.set_status("done");
    t.set_prompt("ask a follow-up");

    // ---- readback (no println until all counts are captured) ----
    let (sx, sy, sw, sh) = t.status_rect();
    let (tx, ty, tw, th) = t.transcript_rect(); // left pane
    let (ax, ay, aw, ah) = t.activity_rect(); // right pane
    let (px, py, pw, ph) = t.prompt_rect();

    let status_ink = tui::count_non_bg(sx, sy, sw, sh, tui::STATUS_BG_C);
    let prompt_ink = tui::count_non_bg(px, py, pw, ph, tui::PROMPT_BG_C);

    // Conversation colours must land in the LEFT pane, tool colours in the
    // RIGHT pane — that's the split. Cross-check each colour in each pane.
    let l_user = tui::count_color(tx, ty, tw, th, tui::ROLE_COLORS[0]);
    let l_asst = tui::count_color(tx, ty, tw, th, tui::ROLE_COLORS[1]);
    let l_tool = tui::count_color(tx, ty, tw, th, tui::ROLE_COLORS[2]);
    let r_tool = tui::count_color(ax, ay, aw, ah, tui::ROLE_COLORS[2]);
    let r_result = tui::count_color(ax, ay, aw, ah, tui::ROLE_COLORS[3]);
    let r_user = tui::count_color(ax, ay, aw, ah, tui::ROLE_COLORS[0]);

    // ---- verdicts ----
    let chrome_ok = status_ink > 100 && prompt_ink > 20;
    // Left pane: conversation present, no tool ink. Right pane: tool activity
    // present, no conversation ink. The mutual exclusion proves side-by-side.
    let left_ok = l_user > 10 && l_asst > 10 && l_tool == 0;
    let right_ok = r_tool > 10 && r_result > 10 && r_user == 0;

    if chrome_ok {
        println!("  [DEMO 50] PASS: chrome — status {} ink px, prompt {} ink px", status_ink, prompt_ink);
    } else {
        println!("  [DEMO 50] FAIL: chrome — status {} prompt {}", status_ink, prompt_ink);
    }
    if left_ok {
        println!("  [DEMO 50] PASS: left pane = conversation (user={} assistant={} px, no tool ink)", l_user, l_asst);
    } else {
        println!("  [DEMO 50] FAIL: left pane user={} assistant={} tool-bleed={}", l_user, l_asst, l_tool);
    }
    if right_ok {
        println!("  [DEMO 50] PASS: right pane = activity (tool={} result={} px, no convo ink)", r_tool, r_result);
    } else {
        println!("  [DEMO 50] FAIL: right pane tool={} result={} convo-bleed={}", r_tool, r_result, r_user);
    }
    if chrome_ok && left_ok && right_ok {
        println!("  [DEMO 50] => M22 TUI: side-by-side panes — conversation | activity, with status + prompt");
    }
}

/// DEMO 59: `$PATH` search. Installs an app into the conventional `/apps`
/// directory, then runs it from the shell by its **bare name** — sem-sh's
/// `exec_external` searches `$PATH` (default `/bin:/apps`), so an app installed
/// anywhere on PATH is runnable without typing its full path ("always on
/// path"). Closes the convenience half of the install-anywhere vision.
pub(crate) fn path_search_demo() {
    use crate::agent;
    use kernel_core::fs::paths::Namespace;
    use kernel_core::semantic::object::SecurityTier;
    use kernel_core::syscall::{dispatch, numbers::*};

    // Ensure /apps exists (conventional install dir; on the default PATH).
    let apps = "/apps";
    let _ = dispatch(SYS_MKDIR, apps.as_ptr() as u64, apps.len() as u64, 0, 0);

    // Install an app there (copy a small embedded ELF). Unlink first so the
    // demo is reboot-safe — once persistence (DEMO 60) fsyncs the namespace, a
    // restored /apps/greet would otherwise make create_file fail.
    let _ = Namespace::unlink("/apps/greet");
    let installed = match kernel_core::fs::ramfs::get_fs().and_then(|fs| fs.find("hello-std.elf")) {
        Some(f) => Namespace::create_file("/apps/greet", SecurityTier::Public, f.data()).is_ok(),
        None => false,
    };
    if !installed {
        println!("  [DEMO 59] FAIL: could not install /apps/greet");
        return;
    }

    // Run it by BARE NAME — PATH search must find /apps/greet.
    let out = agent::run_tool("bash", "{\"command\":\"greet\"}");
    if out.contains("Hello from semos-std!") {
        println!("  [DEMO 59] PASS: bare `greet` resolved via $PATH to /apps/greet and ran");
        println!("  [DEMO 59] => apps installed anywhere on PATH are runnable by name");
    } else {
        println!("  [DEMO 59] FAIL: bare `greet` did not run — output = {:?}", out.trim());
    }
}

/// DEMO 58: install anywhere / run anywhere. "Installs" an executable by
/// writing its ELF bytes to a path in the semantic namespace (not the
/// compile-time ramfs `/bin`), then runs it from the shell by that path —
/// which `SYS_SPAWN` now supports (resolve path → read object bytes → spawn,
/// tier-checked). This is the core of the "apps installed anywhere are always
/// runnable" vision; persistence across reboot (via `SYS_FSYNC`) is the
/// natural follow-on since namespace files already persist to disk.
pub(crate) fn install_anywhere_demo() {
    use crate::agent;
    use kernel_core::fs::paths::Namespace;
    use kernel_core::semantic::object::SecurityTier;

    // Install: copy a small embedded ELF to a namespace path at Public tier.
    // Unlink first so it's reboot-safe (see DEMO 59).
    let _ = Namespace::unlink("/myapp");
    let n = match kernel_core::fs::ramfs::get_fs().and_then(|fs| fs.find("hello-std.elf")) {
        Some(f) => {
            let bytes = f.data();
            let len = bytes.len();
            match Namespace::create_file("/myapp", SecurityTier::Public, bytes) {
                Ok(_) => len,
                Err(e) => {
                    println!("  [DEMO 58] FAIL: install failed (create_file: {:?})", e);
                    return;
                }
            }
        }
        None => {
            println!("  [DEMO 58] SKIPPED: hello-std.elf not embedded");
            return;
        }
    };

    // Run it from the shell by its namespace path — previously impossible
    // (spawn only knew the hardcoded /bin table). Output captured via bash.
    let out = agent::run_tool("bash", "{\"command\":\"/myapp\"}");
    if out.contains("Hello from semos-std!") {
        println!("  [DEMO 58] PASS: installed a {} B ELF at /myapp and ran it from the shell", n);
        println!("  [DEMO 58] => install anywhere / run anywhere — spawn from any namespace path");
    } else {
        println!("  [DEMO 58] FAIL: /myapp did not run — output = {:?}", out.trim());
    }
}

/// DEMO 60: persistence — installed apps survive reboot, including files over
/// the old 64 KiB limit. Namespace files persist to virtio0 via `SYS_FSYNC`
/// and restore at boot (`Namespace::load`). FIRST boot (fresh disk): install a
/// small runnable app (`/apps/persistent-tool`, hello-std) AND a large one
/// (`/apps/big-tool`, sem-sh ≈124 KiB — which the old u16 content_len would
/// have refused to persist) and fsync. LATER boot: both are restored, so we run
/// the small one and confirm the large one came back byte-for-byte — proving
/// persistence and the u32 content-length bump. Two boots, shared vdisk.
pub(crate) fn persistence_install_demo() {
    use crate::agent;
    use kernel_core::fs::paths::Namespace;
    use kernel_core::semantic::object::SecurityTier;
    use kernel_core::syscall::{dispatch, numbers::*};

    let small = "/apps/persistent-tool";
    let big = "/apps/big-tool";

    // The large file's expected size (its ramfs ELF is still embedded, so we
    // can compare on either boot without hardcoding a number).
    let big_expected = kernel_core::fs::ramfs::get_fs()
        .and_then(|fs| fs.find("sem-sh.elf"))
        .map(|f| f.data().len())
        .unwrap_or(0);

    // Restored content length of `big` from the live namespace (0 if absent).
    let big_restored = Namespace::resolve(big)
        .ok()
        .and_then(|suid| unsafe {
            kernel_core::semantic::registry::global_registry()
                .get(&suid)
                .and_then(|o| o.content.as_bytes())
                .map(|b| b.len())
        })
        .unwrap_or(0);

    if Namespace::resolve(small).is_ok() {
        // --- Later boot: both restored from disk. ---
        let out = agent::run_tool("bash", "{\"command\":\"/apps/persistent-tool\"}");
        let ran = out.contains("Hello from semos-std!");
        let big_ok = big_restored == big_expected && big_restored > 64 * 1024;
        if ran {
            println!("  [DEMO 60] PASS: {} survived reboot and ran", small);
        } else {
            println!("  [DEMO 60] FAIL: restored app didn't run — out = {:?}", out.trim());
        }
        if big_ok {
            println!("  [DEMO 60] PASS: {} ({} B, >64 KiB) survived reboot byte-for-byte", big, big_restored);
        } else {
            println!("  [DEMO 60] FAIL: large file persistence — restored={} expected={}", big_restored, big_expected);
        }
        // Stage-1 large-file proof: a 1 MiB doc (> the old 256 KiB content cap
        // AND the old 1 MiB snapshot) restored with its byte pattern intact.
        const DOC: usize = 1024 * 1024;
        let doc_ok = Namespace::resolve("/apps/bigdoc")
            .ok()
            .and_then(|suid| unsafe {
                kernel_core::semantic::registry::global_registry()
                    .get(&suid)
                    .and_then(|o| o.content.as_bytes())
                    .map(|b| {
                        b.len() == DOC
                            && b[1000] == (1000 % 251) as u8
                            && b[DOC - 1] == ((DOC - 1) % 251) as u8
                    })
            })
            .unwrap_or(false);
        if doc_ok {
            println!("  [DEMO 60] PASS: /apps/bigdoc (1 MiB, > old 256 KiB cap) survived reboot, pattern intact");
            println!("  [DEMO 60] => large files persist (Model A stage 1): cap 256 KiB→2 MiB, snapshot 1→4 MiB");
        } else {
            println!("  [DEMO 60] FAIL: 1 MiB doc did not persist correctly");
        }
        return;
    }

    // --- First boot: install small + large, then flush to disk. ---
    let apps = "/apps";
    let _ = dispatch(SYS_MKDIR, apps.as_ptr() as u64, apps.len() as u64, 0, 0);
    let fs = kernel_core::fs::ramfs::get_fs();
    let small_elf = fs.and_then(|f| f.find("hello-std.elf")).map(|f| f.data());
    let big_elf = fs.and_then(|f| f.find("sem-sh.elf")).map(|f| f.data());
    match (small_elf, big_elf) {
        (Some(s), Some(b)) => {
            let ok = Namespace::create_file(small, SecurityTier::Public, s).is_ok()
                && Namespace::create_file(big, SecurityTier::Public, b).is_ok();
            if !ok {
                println!("  [DEMO 60] FAIL: could not install test apps");
                return;
            }
        }
        _ => {
            println!("  [DEMO 60] SKIPPED: test ELFs not embedded");
            return;
        }
    }
    // Synthesize a 1 MiB document (heap buffer, checkable pattern) and install
    // it — proves the lifted cap + 4 MiB snapshot persist files past the old
    // 256 KiB / 1 MiB limits. create_file copies it; free the staging buffer.
    const DOC: usize = 1024 * 1024;
    let dbuf = kernel_core::memory::heap::allocate(DOC, 8);
    if !dbuf.is_null() {
        let s = unsafe { core::slice::from_raw_parts_mut(dbuf, DOC) };
        let mut i = 0;
        while i < DOC {
            s[i] = (i % 251) as u8;
            i += 1;
        }
        let _ = Namespace::create_file("/apps/bigdoc", SecurityTier::Public, s);
        kernel_core::memory::heap::deallocate(dbuf, DOC, 8);
    }
    if dispatch(SYS_FSYNC, 0, 0, 0, 0) == 0 {
        println!("  [DEMO 60] first boot: installed {} (12 KiB) + {} ({} B) + /apps/bigdoc (1 MiB) + fsync'd",
            small, big, big_expected);
        println!("  [DEMO 60] => reboot with the same vdisk to verify all three persist");
    } else {
        println!("  [DEMO 60] FAIL: fsync failed (snapshot too large, or no virtio0)");
    }
}

/// DEMO 57: shell `&&` / `||` short-circuit chaining via the agent bash tool.
/// `true && echo A` runs A; `false && echo B` skips B; `false || echo C` runs C.
pub(crate) fn shell_scripting_demo() {
    use crate::agent;

    let out = agent::run_tool(
        "bash",
        "{\"command\":\"true && echo CHAINED ; false && echo NOPE ; false || echo RECOVER\"}",
    );
    let chained = out.contains("CHAINED");
    let recovered = out.contains("RECOVER");
    let skipped = !out.contains("NOPE");

    if chained && recovered && skipped {
        println!("  [DEMO 57] PASS: && runs on success, || runs on failure, both short-circuit");
        println!("  [DEMO 57] => shell scripting: conditional chaining works ({:?})", out.trim());
    } else {
        println!("  [DEMO 57] FAIL: chained={} recovered={} skipped_nope={} out={:?}",
            chained, recovered, skipped, out.trim());
    }
}

/// DEMO 56: the agent shell sandbox. The agent's `bash` tool spawns sem-sh at
/// tier 0 (Public) — the LLM is the least-trusted component, so its shell runs
/// with the lowest clearance. We create a **Secret**-tier file and a Public
/// file from the kernel (tier 3), then have the agent's shell try to `cat`
/// both: SYS_OPEN's tier check denies the Secret one (caller tier 0 < object
/// tier 3) while the Public one reads fine. Proves "the LLM can't see secrets"
/// using the existing tier enforcement — no new mechanism, just running the
/// agent where it belongs in the 4-tier model.
pub(crate) fn agent_sandbox_demo() {
    use crate::agent;
    use kernel_core::syscall::{dispatch, numbers::*, open_flags};

    // Helper: create a file at a given tier (flags = CREATE | tier<<4) and write.
    fn make(path: &str, tier: u64, content: &[u8]) -> bool {
        let flags = open_flags::CREATE | (tier << 4);
        let fd = dispatch(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, flags, 0);
        if fd == u64::MAX {
            return false;
        }
        dispatch(SYS_FWRITE, fd, content.as_ptr() as u64, content.len() as u64, 0);
        dispatch(SYS_CLOSE, fd, 0, 0, 0);
        true
    }

    let sec_ok = make("/sec-doc", 3, b"TOPSECRET_XYZZY");
    let pub_ok = make("/pub-doc", 0, b"PUBLICOK_ABCDE");
    if !sec_ok || !pub_ok {
        println!("  [DEMO 56] FAIL: could not seed test files (sec={} pub={})", sec_ok, pub_ok);
        return;
    }

    let secret_out = agent::run_tool("bash", "{\"command\":\"cat /sec-doc\"}");
    let public_out = agent::run_tool("bash", "{\"command\":\"cat /pub-doc\"}");

    // Try to MODIFY the secret from the sandboxed shell — the redirect's open
    // must be tier-denied, leaving the file untouched. Read it back as the
    // kernel (tier 3) to confirm.
    let _ = agent::run_tool("bash", "{\"command\":\"echo HACKED > /sec-doc\"}");
    let mut rb = [0u8; 64];
    let path = "/sec-doc";
    let fd = dispatch(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 0, 0);
    let n = if fd == u64::MAX {
        0
    } else {
        let r = dispatch(SYS_FREAD, fd, rb.as_mut_ptr() as u64, rb.len() as u64, 0);
        dispatch(SYS_CLOSE, fd, 0, 0, 0);
        if r == u64::MAX { 0 } else { (r as usize).min(rb.len()) }
    };
    let after = core::str::from_utf8(&rb[..n]).unwrap_or("");

    let secret_blocked = !secret_out.contains("TOPSECRET_XYZZY");
    let public_readable = public_out.contains("PUBLICOK_ABCDE");
    let write_denied = after.contains("TOPSECRET_XYZZY") && !after.contains("HACKED");

    if secret_blocked {
        println!("  [DEMO 56] PASS: Secret-tier /sec-doc NOT readable by the agent shell (tier-denied)");
    } else {
        println!("  [DEMO 56] FAIL: secret leaked to the agent shell: {:?}", secret_out.trim());
    }
    if write_denied {
        println!("  [DEMO 56] PASS: agent shell could NOT modify /sec-doc (write tier-denied, content intact)");
    } else {
        println!("  [DEMO 56] FAIL: secret was modified — after = {:?}", after);
    }
    if public_readable {
        println!("  [DEMO 56] PASS: Public-tier /pub-doc readable by the agent shell (as expected)");
    } else {
        println!("  [DEMO 56] FAIL: public file unreadable: {:?}", public_out.trim());
    }
    if secret_blocked && write_denied && public_readable {
        println!("  [DEMO 56] => LLM shell at tier 0: can't SEE secrets, can't MODIFY protected state");
    }
}

/// DEMO 55: shell `fetch` — `fetch http://example.com/` through the agent's
/// bash tool. sem-sh's `fetch` builtin does an HTTP/1.1 GET over the kernel's
/// TCP stack (`semos_std::net`) and writes the response to stdout, which the
/// bash tool captures. Needs SLIRP networking (same as DEMO 36). Proves the
/// shell can pull external data — and, via the bash tool, so can the agent.
pub(crate) fn shell_fetch_demo() {
    use crate::agent;

    let out = agent::run_tool("bash", "{\"command\":\"fetch http://example.com/\"}");
    let got_http = out.contains("HTTP/1.1") || out.contains("HTTP/1.0");
    let got_body = out.contains("Example Domain") || out.contains("<html") || out.contains("<HTML");

    if got_http && got_body {
        println!("  [DEMO 55] PASS: fetch http://example.com/ → {} B (HTTP response + HTML body)", out.len());
        println!("  [DEMO 55] => shell `fetch` pulls the web over the kernel TCP stack (agent can too)");
    } else if got_http {
        println!("  [DEMO 55] PASS: fetch got an HTTP response ({} B) — body check soft", out.len());
    } else {
        let preview: alloc::string::String = out.chars().take(80).collect();
        println!("  [DEMO 55] SKIPPED/soft: fetch returned {} B (network or host unavailable): {:?}",
            out.len(), preview);
    }
}

/// DEMO 54: the agentic shell. Runs `ask <question>` through the agent's bash
/// tool — sem-sh's `ask` builtin → `SYS_ASK` → the kernel LLM agent → back. The
/// keyless build can't reach the network, so `ask` returns a clear "no key"
/// message (not a hang) — which still proves the whole Ring-3 → kernel-agent
/// bridge end to end. With a key baked in, it returns a real Claude answer.
pub(crate) fn agent_ask_demo() {
    use crate::agent;

    let out = agent::run_tool("bash", "{\"command\":\"ask what is two plus two\"}");
    let trimmed = out.trim();

    if agent::api_key().is_empty() {
        // Bridge works iff the answer is the agent's own "no key" message —
        // meaning the call reached agent::ask through SYS_ASK and returned.
        if out.contains("no ANTHROPIC_KEY") {
            println!("  [DEMO 54] PASS: ask reached the kernel agent via SYS_ASK (keyless): {:?}", trimmed);
            println!("  [DEMO 54] => agentic shell wired — bake ANTHROPIC_KEY for live `ask` answers");
        } else {
            println!("  [DEMO 54] FAIL: ask bridge — out = {:?}", out);
        }
    } else {
        // Keyed: a real answer (anything that isn't one of our error strings).
        let answered = !trimmed.is_empty() && !trimmed.starts_with("ask:");
        if answered {
            println!("  [DEMO 54] PASS: ask → live Claude: {:?}", trimmed);
            println!("  [DEMO 54] => the shell can talk to the OS's LLM");
        } else {
            println!("  [DEMO 54] FAIL: ask (keyed) returned {:?}", out);
        }
    }
}

/// DEMO 53: shell introspection builtins. Runs `ps`, `free`, `uptime` through
/// the agent's bash tool (→ sem-sh → new SYS_PS / SYS_SYSINFO / SYS_TIME) and
/// checks each produced sane output. These are read-only and tier-safe — they
/// expose task metadata + heap totals, never secrets or mutable state — so the
/// agent can inspect the system it runs on without being able to change it.
pub(crate) fn shell_introspection_demo() {
    use crate::agent;

    let ps = agent::run_tool("bash", "{\"command\":\"ps\"}");
    // The header + at least one kernel-mode task (the demo runner) must appear.
    let ps_ok = ps.contains("STATE") && ps.contains("TIER") && ps.contains("kernel");

    let free = agent::run_tool("bash", "{\"command\":\"free\"}");
    let free_ok = free.contains("heap:") && free.contains("free blocks");

    let uptime = agent::run_tool("bash", "{\"command\":\"uptime\"}");
    let uptime_ok = uptime.contains("ticks");

    if ps_ok {
        let lines = ps.split('\n').filter(|l| !l.is_empty()).count();
        println!("  [DEMO 53] PASS: ps listed the task table ({} lines, shows STATE+TIER)", lines);
    } else {
        println!("  [DEMO 53] FAIL: ps output = {:?}", ps);
    }
    if free_ok {
        println!("  [DEMO 53] PASS: free reported heap usage — {:?}", free.trim());
    } else {
        println!("  [DEMO 53] FAIL: free output = {:?}", free);
    }
    if uptime_ok {
        println!("  [DEMO 53] PASS: uptime reported — {:?}", uptime.trim());
    } else {
        println!("  [DEMO 53] FAIL: uptime output = {:?}", uptime);
    }
    if ps_ok && free_ok && uptime_ok {
        println!("  [DEMO 53] => shell is a system interface: read-only ps/free/uptime, tier-safe");
    }
}

/// DEMO 52: M22 agent `bash` tool. Calls `agent::run_tool("bash", …)` directly
/// (no key/network needed) — it spawns `/bin/sem-sh -c "<cmd>"`, captures the
/// shell's stdout over a pipe, and returns it. Seeds a file with the agent's
/// own write_file tool, then runs `echo … ; cat <file>` through the shell and
/// checks both the echo and the file contents came back — proving the agent
/// gets the OS's real command surface (builtins + `;` sequencing + capture).
pub(crate) fn agent_bash_tool_demo() {
    use crate::agent;

    // Seed a multi-line file via the agent's write_file tool.
    let _ = agent::run_tool(
        "write_file",
        "{\"path\":\"/bashtest\",\"content\":\"alpha\\nNEEDLE_LINE\\nbeta\"}",
    );
    // Run echo (builtin) + grep (new builtin) through the shell, captured.
    let out = agent::run_tool(
        "bash",
        "{\"command\":\"echo RUN_OK ; grep NEEDLE /bashtest\"}",
    );

    let echo_ok = out.contains("RUN_OK");
    let grep_hit = out.contains("NEEDLE_LINE");
    let grep_filtered = !out.contains("alpha"); // grep dropped non-matching lines
    if echo_ok && grep_hit && grep_filtered {
        println!("  [DEMO 52] PASS: bash tool ran sem-sh (echo + grep), captured {} B", out.len());
        println!("  [DEMO 52] => M22: agent `bash` tool gives Claude the real shell (sem-sh + grep)");
    } else {
        println!("  [DEMO 52] FAIL: echo={} grep_hit={} filtered={} out={:?}",
            echo_ok, grep_hit, grep_filtered, out);
    }
}

/// DEMO 51: M22 TUI interactive input. Drives the real input path — cooked-mode
/// line discipline (`tty::input_push`, the same entry the PS/2 ISR and USB HID
/// poll feed) → the TUI prompt pane → `Tui::read_line`. Headless QEMU has no
/// keyboard, so we *inject* keystrokes (incl. an edit and Backspace) exactly as
/// a keyboard would, render the in-progress line, verify by pixel readback that
/// the prompt pane shows the typed text, then commit with Enter and confirm
/// `read_line` returns the assembled line. On metal the same `read_line` reads a
/// real keyboard (USB via `pump_keyboard`, PS/2 via its ISR).
pub(crate) fn agent_tui_input_demo() {
    use crate::tui::{self, Tui};

    let mut t: Box<Tui> = match Tui::new("claude-haiku-4-5") {
        Some(t) => Box::new(t),
        None => {
            println!("  [DEMO 51] SKIPPED: no framebuffer");
            return;
        }
    };

    // Simulate the user typing a question (no Enter yet). Throw in a stray 'X'
    // then Backspace to exercise in-line editing through the discipline.
    let typed = b"summarize /README";
    for &b in typed {
        tty::input_push(b);
    }
    tty::input_push(b'X');
    tty::input_push(0x08); // Backspace — removes the 'X'

    // Echo the in-progress line into the prompt pane and verify it rendered.
    t.refresh_prompt();
    let (px, py, pw, ph) = t.prompt_rect();
    let prompt_ink = tui::count_non_bg(px, py, pw, ph, tui::PROMPT_BG_C);

    // Press Enter, then read the committed line back through read_line.
    tty::input_push(b'\n');
    let mut line = [0u8; 128];
    let len = t.read_line(&mut line);
    let got = &line[..len];

    let render_ok = prompt_ink > 200; // "› summarize /README_" is plenty of ink
    let line_ok = got == typed; // 'X' was backspaced out

    if render_ok {
        println!("  [DEMO 51] PASS: typed line echoed to prompt pane ({} ink px)", prompt_ink);
    } else {
        println!("  [DEMO 51] FAIL: prompt ink {} (expected typed text)", prompt_ink);
    }
    if line_ok {
        if let Ok(s) = core::str::from_utf8(got) {
            println!("  [DEMO 51] PASS: read_line committed \"{}\" ({} B, Backspace applied)", s, len);
        }
    } else {
        println!("  [DEMO 51] FAIL: read_line returned {:?}", got);
    }
    if render_ok && line_ok {
        println!("  [DEMO 51] => M22 TUI: keyboard input → prompt echo → committed line works");
    }
}

/// DEMO 49: M22 stage C — the agent reasoning loop against a LIVE Claude model.
/// Creates /README, asks Claude to read it via the read_file tool and
/// summarize; runs the loop (send → parse tool_use → run_tool → tool_result →
/// resend → text). Proves the whole agent end-to-end against the real API.
/// Gated on a baked-in ANTHROPIC_KEY (see agent::api_key).
pub(crate) fn agent_loop_demo() {
    use crate::agent::{self, Message};
    use crate::tui::Tui;
    use alloc::string::String;
    use alloc::vec;

    let key = agent::api_key();
    let model = "claude-haiku-4-5-20251001";
    let sys = "You are a terse assistant on a tiny OS. Use the read_file tool to read files before answering. Reply in one short sentence.";

    // Live mirror of the conversation on the framebuffer TUI (if present). The
    // same loop that prints to serial drives the panes — so this is the real
    // agent UI, not a mock. `None` on a headless run; calls are no-ops then.
    let mut tui = Tui::new("claude-haiku-4-5").map(Box::new);

    // Get the user's question through the TUI prompt. On metal `read_line`
    // blocks on the real keyboard; headless, we inject the keystrokes first so
    // the same input path (line discipline → prompt echo → read_line) runs
    // deterministically and Claude still gets a real, user-supplied question.
    let default_q = "Use the read_file tool to read /README, then summarize it in one short sentence.";
    let question_owned = if let Some(t) = tui.as_mut() {
        for &b in default_q.as_bytes() {
            tty::input_push(b);
        }
        tty::input_push(b'\n');
        let mut qbuf = [0u8; 256];
        let n = t.read_line(&mut qbuf);
        String::from_utf8_lossy(&qbuf[..n]).into_owned()
    } else {
        String::from(default_q)
    };
    let question = question_owned.as_str();
    println!("  [DEMO 49] prompt: \"{}\"", question);
    if let Some(t) = tui.as_mut() {
        t.push_user(question);
        t.set_status("connecting");
    }

    // Seed a file for Claude to read.
    let readme = "Semantic OS is a bare-metal x86_64 Rust kernel with a four-tier LLM security model.";
    let _ = agent::run_tool(
        "write_file",
        &alloc::format!("{{\"path\":\"/README\",\"content\":\"{}\"}}", readme),
    );

    let mut msgs = vec![Message::text("user", question)];

    let mut resp = Box::new([0u8; 8192]);
    let mut body = Box::new([0u8; 8192]);

    // ONE keep-alive TLS connection for the whole conversation — both turns
    // ride it, so the multi-turn loop does a single handshake instead of a
    // reconnect per turn (which is what tripped the single-socket flake).
    let mut session = match agent::Session::open() {
        Ok(s) => s,
        Err(e) => {
            println!("  [DEMO 49] SKIPPED: session open failed ({})", e);
            if let Some(t) = tui.as_mut() { t.push_error(e); t.set_status("error"); }
            return;
        }
    };

    // --- Turn 1: expect a read_file tool_use ---
    println!("  [DEMO 49] turn 1: asking Claude (with key) to use read_file...");
    let req1 = agent::build_request(model, 256, sys, &msgs);
    let http1 = agent::build_http_request(&req1, key, true);
    let n1 = match session.request(http1.as_bytes(), &mut resp[..]) {
        Ok(n) => n,
        Err(e) => {
            println!("  [DEMO 49] SKIPPED: transport error ({})", e);
            if let Some(t) = tui.as_mut() { t.push_error(e); t.set_status("error"); }
            session.close();
            return;
        }
    };
    let status1 = agent::http_status(&resp[..n1]).unwrap_or(0);
    let bn1 = agent::decode_body(&resp[..n1], &mut body[..]);
    let r1 = agent::parse_response(&String::from_utf8_lossy(&body[..bn1]));
    let tu = match r1.tool_use {
        Some(t) => t,
        None => {
            println!("  [DEMO 49] FAIL: turn 1 had no tool_use (HTTP {}). text={:?}", status1, r1.text);
            if let Some(t) = tui.as_mut() { t.push_error("no tool_use in turn 1"); t.set_status("error"); }
            return;
        }
    };
    println!("  [DEMO 49] PASS: Claude requested tool '{}' with input {}", tu.name, tu.input_json);
    if let Some(t) = tui.as_mut() {
        t.push_tool_call(&tu.name, &tu.input_json);
        t.set_status("running read_file");
    }
    let used_read = tu.name == "read_file";

    // Run the tool the model asked for.
    let tool_result = agent::run_tool(&tu.name, &tu.input_json);
    println!("  [DEMO 49] ran tool → {} bytes back", tool_result.len());
    if let Some(t) = tui.as_mut() {
        t.push_tool_result(&tool_result);
        t.set_status("thinking");
    }

    // --- Turn 2: replay assistant tool_use + our tool_result, expect text ---
    msgs.push(Message::assistant_tool_use(&tu));
    msgs.push(Message::tool_result(&tu.id, &tool_result));
    let req2 = agent::build_request(model, 256, sys, &msgs);
    let http2 = agent::build_http_request(&req2, key, true);
    // SAME connection — no reconnect between turns.
    let n2 = match session.request(http2.as_bytes(), &mut resp[..]) {
        Ok(n) => n,
        Err(e) => {
            println!("  [DEMO 49] SKIPPED: transport error on turn 2 ({})", e);
            if let Some(t) = tui.as_mut() { t.push_error(e); t.set_status("error"); }
            session.close();
            return;
        }
    };
    let bn2 = agent::decode_body(&resp[..n2], &mut body[..]);
    let r2 = agent::parse_response(&String::from_utf8_lossy(&body[..bn2]));
    let summary = r2.text.unwrap_or_default();

    if used_read && !summary.is_empty() {
        println!("  [DEMO 49] PASS: Claude summarized after the tool call:");
        println!("  [DEMO 49]   \"{}\"", summary.trim());
        println!("  [DEMO 49] => M22: native agent loop works end-to-end against live Claude");
        if let Some(t) = tui.as_mut() {
            t.push_assistant(summary.trim());
            t.set_status("done");
            t.set_prompt("");
        }
    } else {
        println!("  [DEMO 49] FAIL: used_read={} summary={:?}", used_read, summary);
        if let Some(t) = tui.as_mut() { t.push_error("empty summary"); t.set_status("error"); }
    }

    // Done with the conversation — tear the one connection down.
    session.close();
}

/// DEMO 33: HTTP chunked-transfer-encoding decoder (M13).
///
/// Feeds hardcoded chunked byte streams into `kernel_core::net::decode_chunked`
/// and asserts the reassembled body matches. No network needed — this is the
/// unit-level acceptance for the decoder that the NetworkLlmProvider response
/// path now uses. Three sub-checks:
///   1. normal multi-chunk body reassembles correctly
///   2. an empty body (`0\r\n\r\n`) decodes to zero bytes
///   3. a truncated chunk errors cleanly (no panic / no over-read)
/// Plus a bonus check that hex sizes and trailing headers are handled.
pub(crate) fn chunked_decode_demo() {
    use kernel_core::net::decode_chunked;
    let mut all_ok = true;
    let mut out = [0u8; 256];

    // --- check 1: normal multi-chunk ---------------------------------------
    // "4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n" -> "Wikipedia"
    let input1: &[u8] = b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
    match decode_chunked(input1, &mut out) {
        Ok(n) if &out[..n] == b"Wikipedia" => {
            println!("  [DEMO 33] PASS: multi-chunk decoded {} bytes -> \"Wikipedia\"", n);
        }
        Ok(n) => {
            println!("  [DEMO 33] FAIL: multi-chunk wrong output ({} bytes)", n);
            all_ok = false;
        }
        Err(e) => {
            println!("  [DEMO 33] FAIL: multi-chunk errored: {:?}", e);
            all_ok = false;
        }
    }

    // --- check 2: empty body -----------------------------------------------
    let input2: &[u8] = b"0\r\n\r\n";
    match decode_chunked(input2, &mut out) {
        Ok(0) => println!("  [DEMO 33] PASS: empty body decoded to 0 bytes"),
        Ok(n) => {
            println!("  [DEMO 33] FAIL: empty body decoded {} bytes (want 0)", n);
            all_ok = false;
        }
        Err(e) => {
            println!("  [DEMO 33] FAIL: empty body errored: {:?}", e);
            all_ok = false;
        }
    }

    // --- check 3: truncated input must error cleanly -----------------------
    // declares 9 bytes but only 4 are present, no terminator.
    let input3: &[u8] = b"9\r\nWiki";
    match decode_chunked(input3, &mut out) {
        Err(e) => println!("  [DEMO 33] PASS: truncated input errored cleanly: {:?}", e),
        Ok(n) => {
            println!("  [DEMO 33] FAIL: truncated input decoded {} bytes (want error)", n);
            all_ok = false;
        }
    }

    // --- check 4: hex sizes + trailing headers -----------------------------
    // 0xC = 12 data bytes ("Hello, world"), then CRLF, then a trailer header
    // after the final chunk. (The data must be exactly 12 bytes followed by
    // CRLF — a 13-byte "Hello, world!" here would be malformed framing.)
    let input4: &[u8] = b"C\r\nHello, world\r\n0\r\nX-Trailer: ok\r\n\r\n";
    match decode_chunked(input4, &mut out) {
        Ok(n) if &out[..n] == b"Hello, world" => {
            println!("  [DEMO 33] PASS: hex size + trailer decoded -> \"Hello, world\"");
        }
        Ok(n) => {
            println!("  [DEMO 33] FAIL: hex/trailer wrong output ({} bytes): {:?}",
                n, core::str::from_utf8(&out[..n]).unwrap_or("<non-utf8>"));
            all_ok = false;
        }
        Err(e) => {
            println!("  [DEMO 33] FAIL: hex/trailer errored: {:?}", e);
            all_ok = false;
        }
    }

    if all_ok {
        println!("  [DEMO 33] PASS: all chunked-decoder sub-checks green");
        println!("  [DEMO 33] => M13 closed — chunked bodies de-framed before JSON extract");
    } else {
        println!("  [DEMO 33] FAIL: one or more chunked-decoder sub-checks failed");
    }
}

/// DEMO 86: M56 pairing protocol KAT + self-test.
///
/// Per the project's crypto discipline, a round-trip test is NOT sufficient —
/// this asserts against the **committed reference vectors** in
/// `docs/pairing-v1-test-vectors.md`, which were produced by an independent
/// implementation (`tools/pairing-vectors`) and which the iOS app's
/// `PairingCryptoTests` also pin. Agreement here is the actual proof that
/// SemOS and the companion app will interoperate.
pub(crate) fn pairing_self_test_demo() {
    use kernel_core::crypto::x25519::x25519_base;
    use kernel_core::pairing::{
        build_hello, confirm_mac, decode_payload, derive_session, encode_payload, pairing_id,
        verify_confirm, PairingPayload, NONCE_LEN, VERSION,
    };

    /// Parse hex into `out`; returns bytes written.
    fn unhex(s: &str, out: &mut [u8]) -> usize {
        let b = s.as_bytes();
        let mut n = 0;
        let mut i = 0;
        while i + 1 < b.len() && n < out.len() {
            let hi = (b[i] as char).to_digit(16).unwrap_or(0) as u8;
            let lo = (b[i + 1] as char).to_digit(16).unwrap_or(0) as u8;
            out[n] = (hi << 4) | lo;
            n += 1;
            i += 2;
        }
        n
    }
    fn eq_hex(actual: &[u8], expect_hex: &str) -> bool {
        let mut e = [0u8; 64];
        let n = unhex(expect_hex, &mut e);
        n == actual.len() && &e[..n] == actual
    }

    // ---- Reference inputs, exactly as pinned in the committed vector doc ----
    let mut phone_priv = [0u8; 32];
    phone_priv[0] = 0x01;
    let mut sem_priv = [0u8; 32];
    sem_priv[0] = 0x02;
    let phone_pub = x25519_base(&phone_priv);
    let sem_pub = x25519_base(&sem_priv);
    let nonce_p = [0x50u8; NONCE_LEN];
    let nonce_s = [0x53u8; NONCE_LEN];

    let s_box = derive_session(VERSION, &phone_pub, &nonce_p, &sem_pub, &nonce_s, &sem_priv, &phone_pub);
    let s_phone = derive_session(VERSION, &phone_pub, &nonce_p, &sem_pub, &nonce_s, &phone_priv, &sem_pub);

    let mac_sem = confirm_mac(&s_box.session_key, &s_box.transcript_hash, true);
    let mac_phone = confirm_mac(&s_box.session_key, &s_box.transcript_hash, false);
    let mut pid = [0u8; 16];
    pairing_id(&phone_pub, &sem_pub, &mut pid);

    // KAT assertions against the committed reference vectors.
    let k_pub = eq_hex(&phone_pub, "2fe57da347cd62431528daac5fbb290730fff684afc4cfc2ed90995f58cb3b74");
    let k_th = eq_hex(&s_box.transcript_hash, "9e1f7c29636ff8c36d18f101435983d6f3d48df84be1f8158d27e4e5b5a7160a");
    let k_key = eq_hex(&s_box.session_key, "787bd15f1aeccb296c85854731b8cbab9f6994f2c19e6b4a81597686eeba7fe3");
    let k_sas = s_box.sas == 239413;
    let k_pid = &pid[..] == b"11b7cd9cfd0fdf50";
    let k_macs = eq_hex(&mac_sem, "04d2433da0945fbb7d32d8395d30f29b84659941c4b1d43e5909f1ff35fb2da3")
        && eq_hex(&mac_phone, "dc6f4b95232e6befe12f606a5448eba8df0dbc2163a068e7e2d9e02c5eab984d");

    // Both roles derive the same session (mutual agreement).
    let agree = s_box.session_key == s_phone.session_key
        && s_box.transcript_hash == s_phone.transcript_hash
        && s_box.sas == s_phone.sas;

    // QR payload bytes + base32 must match the committed vectors (port 8080).
    let payload = PairingPayload { version: VERSION, phone_pub, ip: [192, 168, 1, 42], port: 8080, nonce_p };
    let raw = payload.to_bytes();
    let k_payload = eq_hex(
        &raw,
        "5350012fe57da347cd62431528daac5fbb290730fff684afc4cfc2ed90995f58cb3b74c0a8012a1f9050505050505050505050505050505050ba55",
    );
    let mut enc = [0u8; 96];
    let elen = encode_payload(&payload, &mut enc);
    let enc_str = core::str::from_utf8(&enc[..elen]).unwrap_or("");
    // Committed string with hyphens stripped.
    let k_enc = enc_str == "AD802BZ5FPHMFKB28CAJHPNCBYXJJ1SGZZV89BY4SZ1EV44SBXCCPEVMR2M02AGZJ1850M2GA1850M2GA1850M2GA18BMN8";
    // Decoder must accept the HYPHENATED form the app actually displays.
    let hyph = "AD802BZ5-FPHMFKB2-8CAJHPNC-BYXJJ1SG-ZZV89BY4-SZ1EV44S-BXCCPEVM-R2M02AGZ-J1850M2G-A1850M2G-A1850M2G-A18BMN8";
    let k_hyph = match decode_payload(hyph) {
        Ok(d) => d.phone_pub == phone_pub && d.port == 8080 && d.ip == [192, 168, 1, 42] && d.nonce_p == nonce_p,
        Err(_) => false,
    };
    // CRC must reject a single-character transcription typo.
    let mut typo = [0u8; 96];
    typo[..elen].copy_from_slice(&enc[..elen]);
    typo[10] = if typo[10] == b'Z' { b'Y' } else { b'Z' };
    let k_typo = decode_payload(core::str::from_utf8(&typo[..elen]).unwrap_or("")).is_err();

    // Wire framing must match the committed HELLO frame.
    let mut hello = [0u8; 64];
    let hlen = build_hello(&sem_pub, &nonce_s, &mut hello);
    let k_hello = eq_hex(
        &hello[..hlen],
        "003201012fe57da347cd62431528daac5fbb290730fff684afc4cfc2ed90995f58cb3b7453535353535353535353535353535353",
    );
    // CONFIRM verification accepts the right MAC and rejects the wrong one.
    let k_verify = verify_confirm(&s_box.session_key, &s_box.transcript_hash, false, &mac_phone)
        && !verify_confirm(&s_box.session_key, &s_box.transcript_hash, false, &mac_sem);

    let ok = k_pub && k_th && k_key && k_sas && k_pid && k_macs && agree && k_payload && k_enc
        && k_hyph && k_typo && k_hello && k_verify;
    if ok {
        println!("  [DEMO 86] PASS: matches committed reference vectors (keys/th/session/SAS/id/MACs),");
        println!("  [DEMO 86]   QR payload+base32 exact, hyphenated decode OK, CRC catches typos,");
        println!("  [DEMO 86]   HELLO frame byte-exact, CONFIRM verify accept/reject correct.");
        println!("  [DEMO 86] => SemOS pairing core interoperates with the iOS companion app.");
    } else {
        println!(
            "  [DEMO 86] FAIL: pub={} th={} key={} sas={} pid={} macs={} agree={} payload={} enc={} hyph={} typo={} hello={} verify={}",
            k_pub, k_th, k_key, k_sas, k_pid, k_macs, agree, k_payload, k_enc, k_hyph, k_typo, k_hello, k_verify
        );
    }
}
