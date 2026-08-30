//! Syscall Interface - Platform Independent
//!
//! Defines syscall numbers, the dispatch table, and handler implementations.
//! Platform crates provide the trap entry point (ARM64: SVC, x86_64: SYSCALL).
//!
//! # Syscall Numbers
//!
//! | Range  | Category        | Examples                    |
//! |--------|-----------------|-----------------------------|
//! | 0-9    | Core            | write, read, exit, yield    |
//! | 10-19  | File operations | open, close, seek, stat     |
//! | 20-29  | Semantic objects| create, query, link, search |
//! | 30-39  | Memory          | alloc, free, pool_info      |
//! | 40-49  | Process         | spawn, wait, getpid         |
//! | 50-59  | LLM services    | context, redact, summarize  |
//! | 60-69  | Crypto/Storage  | encrypt, decrypt, persist   |
//! | 70-79  | System          | time, uptime, reboot        |

/// Syscall number constants
pub mod numbers {
    // Core (0-9)
    pub const SYS_WRITE: u64 = 0;
    pub const SYS_READ: u64 = 1;
    pub const SYS_EXIT: u64 = 2;
    pub const SYS_YIELD: u64 = 3;
    pub const SYS_GETPID: u64 = 4;
    pub const SYS_SLEEP: u64 = 5;
    pub const SYS_INFO: u64 = 6;

    // File operations (10-19)
    pub const SYS_OPEN: u64 = 10;
    pub const SYS_CLOSE: u64 = 11;
    pub const SYS_FREAD: u64 = 12;
    pub const SYS_FWRITE: u64 = 13;
    pub const SYS_SEEK: u64 = 14;
    pub const SYS_STAT: u64 = 15;  // legacy: returns size only, kept for compat
    pub const SYS_MKDIR: u64 = 16;
    pub const SYS_UNLINK: u64 = 17;
    pub const SYS_READDIR: u64 = 18;
    pub const SYS_FSYNC: u64 = 19;       // Phase 14 Tier 2: flush namespace to disk

    // Semantic objects (20-29)
    pub const SYS_SEM_CREATE: u64 = 20;
    pub const SYS_SEM_READ: u64 = 21;
    pub const SYS_SEM_WRITE: u64 = 22;
    pub const SYS_SEM_DELETE: u64 = 23;
    pub const SYS_SEM_LINK: u64 = 24;
    pub const SYS_SEM_QUERY: u64 = 25;
    pub const SYS_SEM_SEARCH: u64 = 26;
    pub const SYS_SEM_META: u64 = 27;

    // Memory (30-39)
    pub const SYS_ALLOC: u64 = 30;        // frame-granular, tier-aware (Phase 1)
    pub const SYS_FREE: u64 = 31;         // frame-granular pair
    pub const SYS_POOL_INFO: u64 = 32;
    pub const SYS_BRK: u64 = 33;          // Linux-style heap grow (TBD)
    pub const SYS_HEAP_ALLOC: u64 = 34;   // (size, align) → ptr (Phase 14 prereq)
    pub const SYS_HEAP_FREE: u64 = 35;    // (ptr, size, align) → 0/err
    // Extended file ops (36-38). Overflowed the 10-19 range, parked
    // in 36-39 next to the heap-alloc cluster. Phase 14 Tier 2.
    pub const SYS_RENAME: u64 = 36;       // (old_ptr, old_len, new_ptr, new_len) → 0/err
    pub const SYS_TRUNCATE: u64 = 37;     // (path_ptr, path_len, new_size) → 0/err
    pub const SYS_STATX: u64 = 38;        // (path_ptr, path_len, out_struct_ptr) → 0/err
    // Map fresh, zeroed, USER-accessible frames into the caller's
    // address space at `addr` (M25 Tier 2 #50 — backs the user-space
    // heap allocator). Unlike SYS_HEAP_ALLOC (which returns a KERNEL
    // heap pointer only valid in Ring 0), this gives Ring-3 code memory
    // it can actually write.
    pub const SYS_MMAP_ANON: u64 = 39;    // (addr, size) → addr / u64::MAX

    // Process (40-49)
    pub const SYS_SPAWN: u64 = 40;
    pub const SYS_WAIT: u64 = 41;
    pub const SYS_KILL: u64 = 42;
    pub const SYS_EXEC: u64 = 43;
    pub const SYS_DUP: u64 = 44;
    pub const SYS_DUP2: u64 = 45;
    pub const SYS_PIPE: u64 = 46;

    // LLM services (50-59)
    pub const SYS_LLM_QUERY: u64 = 50;
    pub const SYS_LLM_CONTEXT: u64 = 51;
    pub const SYS_LLM_REDACT: u64 = 52;
    pub const SYS_LLM_SUMMARIZE: u64 = 53;
    pub const SYS_LLM_ACCESS: u64 = 54;
    pub const SYS_LLM_STREAM_START: u64 = 55;
    pub const SYS_LLM_STREAM_READ: u64 = 56;
    pub const SYS_LLM_SET_POLICY: u64 = 57;
    pub const SYS_LLM_GET_POLICY: u64 = 58;

    // Crypto/Storage (60-69)
    pub const SYS_ENCRYPT: u64 = 60;
    pub const SYS_DECRYPT: u64 = 61;
    pub const SYS_HASH: u64 = 62;
    pub const SYS_PERSIST: u64 = 63;
    pub const SYS_RESTORE: u64 = 64;

    // System (70-79)
    pub const SYS_TIME: u64 = 70;
    pub const SYS_UPTIME: u64 = 71;
    pub const SYS_REBOOT: u64 = 72;
    pub const SYS_SYSINFO: u64 = 73;

    // User identity & isolation (80-89). Backed by `security::users`.
    pub const SYS_GETUID:        u64 = 80; // -> current user id
    pub const SYS_SETUID:        u64 = 81; // (uid) -> 0 / err
    pub const SYS_CREATE_USER:   u64 = 82; // (name_ptr, name_len, tier, group) -> uid / err
    pub const SYS_LOOKUP_USER:   u64 = 83; // (uid, out_ptr, out_len) -> bytes_written / err

    // Per-process environment + CWD (74-77). Phase 14 prereq #3 —
    // std::env::{var, vars, current_dir, set_current_dir} reach here.
    pub const SYS_GET_CWD: u64 = 74; // (buf_ptr, buf_len) -> bytes_written / err
    pub const SYS_SET_CWD: u64 = 75; // (path_ptr, path_len) -> 0 / err
    pub const SYS_GET_ENV: u64 = 76; // (key_ptr, key_len, buf_ptr, buf_len) -> bytes / 0=not-found / err
    pub const SYS_SET_ENV: u64 = 77; // (key_ptr, key_len, val_ptr, val_len) -> 0 / err

    // Threading & sync (90-99). Phase 14 Tier 3 prereqs (#45, #46, #47).
    pub const SYS_FUTEX_WAIT:   u64 = 90; // (u32_addr, expected) -> 0 woken / 1 mismatch / err
    pub const SYS_FUTEX_WAKE:   u64 = 91; // (u32_addr, max_count) -> count_woken
    pub const SYS_THREAD_SPAWN: u64 = 92; // (entry_va, arg) -> tid / err
    pub const SYS_THREAD_JOIN:  u64 = 93; // (tid) -> exit_code / err
    pub const SYS_WAITNB:       u64 = 94; // (pid_hint) -> child_pid / 0 (none yet) / err

    // Networking for Ring 3 (100-104). M25 `std::net` backing. The smoltcp
    // stack lives in kernel-core::net; these expose a small blocking socket
    // surface to user space. DNS is offered as a one-shot resolve rather
    // than raw UDP sockets (the only UDP need today).
    // All non-blocking: each does a single net::poll() + one attempt and
    // returns immediately. The std-shim drives the wait loop in *user* space
    // (yield/sleep between tries) so the kernel never runs a long
    // interrupts-enabled poll loop in a Ring-3 task's context — which would
    // get the timer preempting into context_switch thousands of times and
    // trip the task#40 resume race (#56).
    pub const SYS_DNS_RESOLVE:  u64 = 100; // (host_ptr, host_len) -> ipv4 (u32 BE) / u64::MAX
    pub const SYS_TCP_CONNECT:  u64 = 101; // (ipv4_be, port) -> fd / u64::MAX (SYN queued, not yet established)
    pub const SYS_TCP_READ:     u64 = 102; // (fd, buf_ptr, buf_len) -> n (0=EOF) / WOULDBLOCK / u64::MAX
    pub const SYS_TCP_WRITE:    u64 = 103; // (fd, buf_ptr, buf_len) -> n / WOULDBLOCK / u64::MAX
    pub const SYS_TCP_CLOSE:    u64 = 104; // (fd) -> 0 / u64::MAX
    pub const SYS_TCP_STATE:    u64 = 105; // (fd) -> 0 closed / 1 connecting / 2 established / u64::MAX

    // Read-only system introspection (shell `ps`/`free`; safe at any tier —
    // exposes task metadata + heap totals, never secrets or mutable state).
    pub const SYS_PS:           u64 = 110; // (buf_ptr, buf_len) -> task record count; 24B/rec
    pub const SYS_ASK:          u64 = 111; // (prompt_ptr, prompt_len, out_ptr, out_len) -> answer len
    pub const SYS_AGENT:        u64 = 112; // (flags) -> 0/err; runs the interactive split-pane agent TUI
    pub const SYS_EDIT:         u64 = 113; // (path_ptr, path_len) -> 0/err; runs the modal text editor
    pub const SYS_USBINFO:      u64 = 114; // () -> 0; dumps every USB port + enumerated slot to the TTY
    pub const SYS_USBENUM:      u64 = 115; // () -> port_count; re-runs xHCI port enumeration (hot-plug retry)
    pub const SYS_NETINFO:      u64 = 116; // () -> 0/err; read-only net stack + active NIC diagnostics
    pub const SYS_TTY_SUPPRESS: u64 = 117; // (on: u64) -> 0; 1 silences keyboard input from
                                           // committing to the cooked-mode line discipline.
    // M14 iGPU: display diagnostics + internal-panel backlight control.
    pub const SYS_FBINFO:       u64 = 118; // () -> 0; prints GOP framebuffer geometry/format
                                           // and native-panel comparison to the TTY.
    pub const SYS_BACKLIGHT:    u64 = 119; // (op, arg) -> percent | u64::MAX. op: 0=get,
                                           // 1=set arg%, 2=up, 3=down, 4=restore. Clamped
                                           // to a visible floor by the platform impl.
    pub const SYS_DEMOS:        u64 = 120; // () -> 0; run the full boot DEMO suite on demand
                                           // (the `demos` builtin). Blocks in the caller's
                                           // context; ESC aborts the run early.
    pub const SYS_PAIR:         u64 = 121; // (qr_ptr, qr_len) -> 1 ok / 0 fail; run the M56
                                           // pairing handshake against the phone in the QR
                                           // payload. CONSOLE ONLY (agent cannot enroll a device).
    pub const SYS_PAIRED:       u64 = 122; // () -> count; print the paired-devices list (read-only).
    pub const SYS_UNPAIR:       u64 = 124; // (id_ptr, id_len) -> 1/0; forget a paired device.
                                           // CONSOLE ONLY.
    // M14-E: first app-facing framebuffer surface. Metadata is returned as
    // eight little-endian u64 words: width, height, stride_pixels,
    // bytes_per_pixel, format_code (1=RGB, 2=BGR, 3=U8, 0=unknown), byte_len,
    // native_width_or_0, native_height_or_0. FB_BLIT copies a user-owned
    // row-major u32 RGB buffer to (x,y,w,h) and presents the damage.
    pub const SYS_FB_META:      u64 = 128; // (out_ptr, out_len>=64) -> 0/err
    pub const SYS_FB_BLIT:      u64 = 129; // (xy_pack, wh_pack, pixels_ptr, pixel_count) -> 0/err
    pub const SYS_MODESET:      u64 = 130; // (op) -> 0/err; guarded M14-F modeset status/plan/verify/poke
    pub const SYS_FB_WAIT_VBLANK: u64 = 131; // () -> 0 on frame boundary | u64::MAX; read-only Pipe A scanline wrap
    pub const SYS_WIFI_SCAN:    u64 = 123; // () -> n; scans WiFi, prints numbered network list
                                           // sem-sh wraps external commands so typing during a
                                           // child run doesn't buffer into the next prompt.
    pub const SYS_WIFI_CONNECT: u64 = 125; // (idx, pass_ptr, pass_len) -> 1/0; connect to network idx
    pub const SYS_VOUCH:        u64 = 126; // (path_ptr, path_len, grant_tier) -> 1/0; mark a namespace tool safe to run at grant_tier. Interactive console ONLY (the agent cannot reach this).
    pub const SYS_VOUCHES:      u64 = 127; // () -> count; print the active vouch grants (audit list)
    pub const SYS_NETLOG:       u64 = 132; // (target_ptr, target_len) -> bytes sent; drain the kernel
                                           // log ring and UDP-send it to "a.b.c.d[:port]" (port 9000
                                           // default) for off-box debugging (`nc -u -l 9000` on a Mac).
    // Self-dev loop (docs/semos_selfdev_loop_plan.md §3.1): session vouch.
    // Where SYS_VOUCH binds one tool's bytes to a tier, SYS_VOUCH_SESSION
    // raises the ceiling for ALL namespace (agent-authored) executables for a
    // bounded time — the human opens the door once instead of per tool.
    pub const SYS_VOUCH_SESSION: u64 = 133; // (tier, duration_secs, pw_ptr, pw_len) -> 1 ok / 0 err.
                                            // Interactive console ONLY. duration_secs==0 revokes.
                                            // First call sets the vouch password (hash kept in RAM);
                                            // later calls must match it (constant-time compare).
    pub const SYS_GET_VOUCH:    u64 = 134;  // () -> (tier << 32) | remaining_secs, or 0 when no
                                            // live session. Any task may query (read-only).
    pub const SYS_SELFDEV:      u64 = 135;  // (demo_n) -> 0 / u64::MAX; run one self-dev
                                            // demo (80|83|87|88) on demand (the `selfdev`
                                            // builtin). CONSOLE ONLY — demos install into
                                            // /apps; the agent must never trigger a
                                            // self-modify cycle. Blocks in the caller's
                                            // context like SYS_DEMOS.
    // SYS_SYSINFO (73) is wired to heap stats: (buf_ptr, buf_len>=24) -> 0/err,
    // writes [used:u64][free:u64][free_blocks:u64].

    // M27 DEMO 80 — read-only sysroot blob staged on a SATA disk (Layer B).
    // NOTE: 120-122 were SYS_DEMOS/PAIR/PAIRED first; the duplicate numbers
    // shadowed those arms in dispatch (first match wins). Renumbered to
    // 136-138 — keep the two tables collision-free.
    /// SYS_SYSROOT_INFO(idx, name_buf_ptr, name_buf_len) -> file byte length,
    /// or u64::MAX if idx is out of range / no blob. Writes the file name (up to
    /// name_buf_len bytes) into name_buf_ptr.
    pub const SYS_SYSROOT_INFO: u64 = 136;
    /// SYS_SYSROOT_READ(idx, offset, buf_ptr, buf_len) -> bytes read (0 = EOF),
    /// or u64::MAX on error. Streams file `idx` from disk at byte `offset`.
    pub const SYS_SYSROOT_READ: u64 = 137;
    /// SYS_FLASH_SYSROOT() -> bytes copied, or u64::MAX on error. Copies
    /// sysroot.img off the FAT USB stick (usb0) onto the SATA disk (sata0).
    pub const SYS_FLASH_SYSROOT: u64 = 138;
    /// SYS_KB_POLL(out_ptr, out_len_bytes) -> event count | u64::MAX.
    /// Non-blocking drain of the raw key-event ring: u32 records, bit 31 =
    /// pressed, bit 7 = PS/2-extended, bits 6:0 = normalized set-1 scancode.
    /// Pumps USB HID itself while a fullscreen app owns input.
    pub const SYS_KB_POLL: u64 = 139;
    /// SYS_FB_CLAIM(on) -> 0. 1 = claim screen+keyboard for a fullscreen app
    /// (FULLSCREEN_APP_ACTIVE + SUPPRESS_TTY_INPUT, stale input drained,
    /// framebuffer cleared); 0 = release. Auto-released by reset_tty_flags
    /// on SYS_EXIT.
    pub const SYS_FB_CLAIM: u64 = 140;

    /// Returned by SYS_TCP_READ / SYS_TCP_WRITE when the socket isn't ready
    /// yet (no data / tx full). Distinct from 0 (EOF on read) and u64::MAX
    /// (hard error). The shim retries after yielding.
    pub const NET_WOULDBLOCK: u64 = u64::MAX - 1;
}

/// Syscall dispatch — called by platform trap handler with
/// (syscall_number, arg0, arg1, arg2, arg3).
/// Returns the result value.
pub fn dispatch(num: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    use numbers::*;

    match num {
        // Core (0-9)
        SYS_WRITE => handle_write(arg0, arg1),
        SYS_READ => handle_read(arg0, arg1, arg2),
        SYS_EXIT => handle_exit(arg0),
        SYS_YIELD => { handle_yield(); 0 },
        SYS_GETPID => handle_getpid(),
        SYS_SLEEP => handle_sleep(arg0),
        SYS_INFO => handle_info(),

        // File I/O (10-19)
        SYS_OPEN => handle_open(arg0, arg1, arg2),
        SYS_CLOSE => handle_close(arg0),
        SYS_FREAD => handle_fread(arg0, arg1, arg2),
        SYS_FWRITE => handle_fwrite(arg0, arg1, arg2),
        SYS_SEEK => handle_seek(arg0, arg1),
        SYS_STAT => handle_stat(arg0, arg1),
        SYS_MKDIR => handle_mkdir(arg0, arg1),
        SYS_UNLINK => handle_unlink(arg0, arg1),
        SYS_READDIR => handle_readdir(arg0, arg1, arg2, arg3),
        SYS_FSYNC => handle_fsync(),
        SYS_RENAME => handle_rename(arg0, arg1, arg2, arg3),
        SYS_TRUNCATE => handle_truncate(arg0, arg1, arg2),
        SYS_STATX => handle_statx(arg0, arg1, arg2),

        // Sysroot blob (Layer B) — read-only crate metadata staged on a SATA disk.
        SYS_SYSROOT_INFO => handle_sysroot_info(arg0, arg1, arg2),
        SYS_SYSROOT_READ => handle_sysroot_read(arg0, arg1, arg2, arg3),
        SYS_FLASH_SYSROOT => handle_flash_sysroot(),

        // Semantic objects (20-29)
        SYS_SEM_CREATE => handle_sem_create(arg0, arg1, arg2, arg3),
        SYS_SEM_READ => handle_sem_read(arg0, arg1, arg2),
        SYS_SEM_WRITE => handle_sem_write(arg0, arg1, arg2, arg3),
        SYS_SEM_DELETE => handle_sem_delete(arg0, arg1),
        SYS_SEM_LINK => handle_sem_link(arg0, arg1, arg2, arg3),
        SYS_SEM_QUERY => handle_sem_query(arg0, arg1, arg2),
        SYS_SEM_SEARCH => handle_sem_search(arg0, arg1, arg2, arg3),
        SYS_SEM_META => handle_sem_meta(arg0, arg1, arg2),

        // Memory (30-39)
        SYS_ALLOC => handle_alloc(arg0, arg1),
        SYS_FREE => handle_free(arg0, arg1),
        SYS_POOL_INFO => handle_pool_info(arg0),
        SYS_HEAP_ALLOC => handle_heap_alloc(arg0, arg1),
        SYS_HEAP_FREE => handle_heap_free(arg0, arg1, arg2),
        SYS_MMAP_ANON => handle_mmap_anon(arg0, arg1),

        // Process (40-49)
        SYS_SPAWN => handle_spawn(arg0, arg1, arg2, arg3),
        SYS_VOUCH => handle_vouch(arg0, arg1, arg2),
        SYS_VOUCHES => handle_vouches(),
        SYS_VOUCH_SESSION => handle_vouch_session(arg0, arg1, arg2, arg3),
        SYS_GET_VOUCH => handle_get_vouch(),
        SYS_WAIT => handle_wait(arg0),
        SYS_KILL => handle_kill(arg0),
        SYS_DUP => handle_dup(arg0),
        SYS_DUP2 => handle_dup2(arg0, arg1),
        SYS_PIPE => handle_pipe(arg0),

        // LLM services (50-59)
        SYS_LLM_QUERY => handle_llm_query(arg0, arg1, arg2),
        SYS_LLM_CONTEXT => handle_llm_context(arg0, arg1, arg2),
        SYS_LLM_REDACT => handle_llm_redact(arg0, arg1, arg2),
        SYS_LLM_SUMMARIZE => handle_llm_summarize(arg0, arg1, arg2),
        SYS_LLM_ACCESS => handle_llm_access(arg0, arg1, arg2, arg3),
        SYS_LLM_STREAM_START => handle_llm_stream_start(arg0, arg1, arg2),
        SYS_LLM_STREAM_READ => handle_llm_stream_read(arg0, arg1, arg2),
        SYS_LLM_SET_POLICY => handle_llm_set_policy(arg0, arg1, arg2, arg3),
        SYS_LLM_GET_POLICY => handle_llm_get_policy(arg0, arg1, arg2, arg3),

        // Crypto/Storage (60-69)
        SYS_ENCRYPT => handle_encrypt(arg0, arg1, arg2, arg3),
        SYS_DECRYPT => handle_decrypt(arg0, arg1, arg2, arg3),
        SYS_HASH => handle_hash(arg0, arg1, arg2),

        // System (70-79)
        SYS_TIME => crate::platform::ticks(),
        SYS_SYSINFO => handle_sysinfo(arg0, arg1),
        SYS_GET_CWD => handle_get_cwd(arg0, arg1),
        SYS_SET_CWD => handle_set_cwd(arg0, arg1),
        SYS_GET_ENV => handle_get_env(arg0, arg1, arg2, arg3),
        SYS_SET_ENV => handle_set_env(arg0, arg1, arg2, arg3),

        // Read-only introspection
        SYS_PS => handle_ps(arg0, arg1),

        // Agentic shell: one-shot LLM query (the `ask` builtin)
        SYS_ASK => handle_ask(arg0, arg1, arg2, arg3),

        // Interactive agent terminal (the `agent` builtin) — blocks in the
        // caller's context running the framebuffer TUI chat loop.
        SYS_AGENT => crate::platform::get().run_agent_tui(arg0),

        // Modal text editor (the `edit` builtin) — blocks in the caller's
        // context running the framebuffer editor over the named file.
        SYS_EDIT => crate::platform::get().run_editor(arg0, arg1),

        // USB diagnostic (the `usbinfo` builtin) — dumps every port +
        // enumerated slot to the TTY so the user can debug enumeration
        // without serial output (real bare-metal hardware).
        SYS_USBINFO => crate::platform::get().run_usbinfo(),

        // Hot-plug retry — re-runs xHCI port enumeration from the shell.
        SYS_USBENUM => crate::platform::get().run_usbenum(),

        // Read-only network + NIC diagnostics (the `netinfo` builtin).
        SYS_NETINFO => crate::platform::get().run_netinfo(),

        // WiFi scan (the `wifi` builtin) — scan + print numbered network list.
        SYS_WIFI_SCAN => crate::platform::get().run_wifi_scan(),

        // WiFi connect (`wifi connect <idx> <password>`) — associate + WPA2.
        SYS_WIFI_CONNECT => crate::platform::get().run_wifi_connect(arg0, arg1, arg2),

        // Toggle the TTY input-suppression flag from user-space.
        // Used by sem-sh to silence keyboard input while a child command runs.
        SYS_TTY_SUPPRESS => crate::platform::get().tty_suppress(arg0 != 0),

        // Run the full boot DEMO suite on demand (the `demos` builtin) —
        // blocks in the caller's context, like the agent/edit TUIs above.
        SYS_DEMOS => crate::platform::get().run_demos(),

        // Run ONE self-dev demo on demand (the `selfdev` builtin). Console-only
        // like SYS_PAIR: demos 83/87/88 install executables into /apps, so the
        // agent must never be able to trigger a self-modify cycle.
        SYS_SELFDEV => {
            if !is_vouch_authority() {
                crate::platform::log("[selfdev] DENIED: caller is not the interactive console\n");
                return u64::MAX;
            }
            crate::platform::get().run_selfdev(arg0)
        }

        // M56 pairing. `pair` and `unpair` mutate device trust, so they are
        // gated to the interactive console (same authority as SYS_VOUCH) — the
        // agent must never be able to enroll or forget a device. `paired` is a
        // read-only listing, allowed at any tier.
        SYS_PAIR => {
            if !is_vouch_authority() {
                crate::platform::log("[pair] DENIED: caller is not the interactive console\n");
                return 0;
            }
            crate::platform::get().run_pair(arg0, arg1)
        }
        SYS_PAIRED => crate::platform::get().run_paired_list(),
        SYS_UNPAIR => {
            if !is_vouch_authority() {
                crate::platform::log("[unpair] DENIED: caller is not the interactive console\n");
                return 0;
            }
            crate::platform::get().run_unpair(arg0, arg1)
        }
        // netlog: drain the kernel log ring and UDP-send it to a LAN target.
        // Read-only over the log (and the log is already tier-agnostic debug
        // output), so no console gate. The target string is copied out of
        // validated caller memory by the platform impl.
        SYS_NETLOG => crate::platform::get().run_netlog(arg0, arg1),

        // M14 display diagnostics and safe backlight control.
        SYS_FBINFO => crate::platform::get().run_fbinfo(),
        SYS_BACKLIGHT => crate::platform::get().run_backlight(arg0, arg1),
        SYS_FB_META => crate::platform::get().fb_meta(arg0, arg1),
        SYS_FB_BLIT => crate::platform::get().fb_blit(arg0, arg1, arg2, arg3),
        SYS_MODESET => crate::platform::get().run_modeset(arg0),
        SYS_FB_WAIT_VBLANK => crate::platform::get().fb_wait_vblank(),
        SYS_KB_POLL => crate::platform::get().kb_poll(arg0, arg1),
        SYS_FB_CLAIM => crate::platform::get().fb_claim(arg0),

        // User identity & isolation (80-89)
        SYS_GETUID        => handle_getuid(),
        SYS_SETUID        => handle_setuid(arg0),
        SYS_CREATE_USER   => handle_create_user(arg0, arg1, arg2, arg3),
        SYS_LOOKUP_USER   => handle_lookup_user(arg0, arg1, arg2),

        // Threading & sync (90-99) — Phase 14 Tier 3
        SYS_FUTEX_WAIT    => handle_futex_wait(arg0, arg1),
        SYS_FUTEX_WAKE    => handle_futex_wake(arg0, arg1),
        SYS_THREAD_SPAWN  => handle_thread_spawn(arg0, arg1),
        SYS_THREAD_JOIN   => handle_thread_join(arg0),
        SYS_WAITNB        => handle_waitnb(arg0),

        // Networking (100-104) — M25 std::net backing
        SYS_DNS_RESOLVE   => handle_dns_resolve(arg0, arg1),
        SYS_TCP_CONNECT   => handle_tcp_connect(arg0, arg1),
        SYS_TCP_READ      => handle_tcp_read(arg0, arg1, arg2),
        SYS_TCP_WRITE     => handle_tcp_write(arg0, arg1, arg2),
        SYS_TCP_CLOSE     => handle_tcp_close(arg0),
        SYS_TCP_STATE     => handle_tcp_state(arg0),

        _ => {
            crate::platform::log("[syscall] Unknown syscall: ");
            crate::platform::log_num(num);
            crate::platform::log("\n");
            u64::MAX // Error
        }
    }
}

// --- User pointer validation ---

/// Maximum user-space virtual address (lower half of canonical form).
/// Any pointer above this is in kernel space and must be rejected.
const USER_ADDR_LIMIT: u64 = 0x0000_8000_0000_0000;

/// Validate that a user pointer + length is in user-space and doesn't overflow.
fn validate_user_ptr(ptr: u64, len: u64) -> bool {
    if ptr == 0 || len == 0 { return false; }
    if ptr >= USER_ADDR_LIMIT { return false; }
    // Check for overflow
    match ptr.checked_add(len) {
        Some(end) if end <= USER_ADDR_LIMIT => true,
        _ => false,
    }
}

/// Does the current syscall caller need user-range enforcement?
/// Ring-3 tasks always do; kernel tasks call `dispatch()` directly with
/// kernel-space buffers (agent pipe capture, editor saves, pipe demos), so
/// their pointers are trusted. This per-caller split is what closes the
/// 2026-07-17 review's critical findings without breaking kernel callers.
#[inline]
fn caller_needs_validation() -> bool {
    crate::scheduler::current_task_is_user()
}

/// Read a byte slice from the syscall caller's address space. For Ring-3
/// callers the range is validated against USER_ADDR_LIMIT first, so a user
/// task can never trick a handler into reading kernel memory. Returns None
/// if the pointer is invalid. `pub` so platform-side syscall handlers
/// (editor path, wifi password) share the same enforcement.
///
/// # Safety
/// Range-checked for user callers; the memory must still be mapped (an
/// in-range-but-unmapped user pointer faults — the kernel #PF handler kills
/// the faulting task rather than the machine; a copy-with-recovery path is
/// future work).
pub unsafe fn read_caller_slice(ptr: u64, len: u64) -> Option<&'static [u8]> {
    if caller_needs_validation() && !validate_user_ptr(ptr, len) {
        return None;
    }
    if len == 0 || (len as usize) as u64 != len { return None; }
    Some(core::slice::from_raw_parts(ptr as *const u8, len as usize))
}

/// Mutable counterpart of `read_caller_slice`. A Ring-3 caller can never
/// obtain a `&mut [u8]` into kernel memory through this.
unsafe fn caller_slice_mut(ptr: u64, len: u64) -> Option<&'static mut [u8]> {
    if caller_needs_validation() && !validate_user_ptr(ptr, len) {
        return None;
    }
    if len == 0 || (len as usize) as u64 != len { return None; }
    Some(core::slice::from_raw_parts_mut(ptr as *mut u8, len as usize))
}

/// Read a string from the caller's address space. Returns None if invalid
/// pointer (for a Ring-3 caller) or not UTF-8.
unsafe fn read_caller_str(ptr: u64, len: u64) -> Option<&'static str> {
    let slice = read_caller_slice(ptr, len)?;
    core::str::from_utf8(slice).ok()
}

/// Write bytes to the caller's address space. For Ring-3 callers the range
/// is validated first, so a user task can never write kernel memory through
/// a syscall output buffer. Returns false if the pointer is invalid.
unsafe fn write_to_caller(ptr: u64, data: &[u8]) -> bool {
    if caller_needs_validation() && !validate_user_ptr(ptr, data.len() as u64) {
        return false;
    }
    let dest = core::slice::from_raw_parts_mut(ptr as *mut u8, data.len());
    dest.copy_from_slice(data);
    true
}

// --- Handler implementations ---

/// Write `buf` to the TTY console (the Console FD sink). This is the old
/// global `handle_write` body, now reachable both as the default stdout
/// path and as the `FdEntry::Console` action.
///
/// The buffer goes through `read_caller_slice`: a Ring-3 caller passing a
/// kernel address gets u64::MAX instead of a kernel-memory disclosure
/// (2026-07-17 review, critical #1). Kernel-mode callers (demos, agent)
/// still print kernel buffers directly.
fn console_write(buf_ptr: u64, buf_len: u64) -> u64 {
    let len = buf_len as usize;
    if len > 4096 { return u64::MAX; }
    if len == 0 { return 0; }
    let slice = match unsafe { read_caller_slice(buf_ptr, buf_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };
    if let Ok(s) = core::str::from_utf8(slice) {
        crate::platform::log(s);
    }
    buf_len
}

/// SYS_WRITE(buf, len) — write to stdout, i.e. the running process's FD 1.
/// Delegates to `handle_fwrite(1, ..)` so FD 1 routes to whatever it points
/// at: Console → TTY, a redirected pipe → the pipe, a redirected file → the
/// file (positional). The no-process fallback resolves FD 1 = Console.
fn handle_write(buf_ptr: u64, buf_len: u64) -> u64 {
    handle_fwrite(1, buf_ptr, buf_len)
}

/// Copy out the running process's FD entry at `fd`, or None if there's no
/// process for the live slot or the slot is Empty.
fn current_fd_entry(fd: u64) -> Option<crate::process::FdEntry> {
    crate::process::with_current_fds_mut(|fds| fds.get(fd as i32).copied()).flatten()
}

/// Write `buf` into pipe `pipe_id` (write end). Mirrors the old fwrite pipe
/// branch: blocks the task (BlockReason::PipeWrite) when the pipe is full,
/// returns u64::MAX on a broken pipe, else the byte count.
fn pipe_write_blocking(pipe_id: usize, buf_ptr: u64, buf_len: u64) -> u64 {
    let len = (buf_len as usize).min(4096);
    if len == 0 { return 0; }
    // Validated: a Ring-3 caller pointing at kernel memory gets u64::MAX,
    // not a kernel-memory disclosure into the pipe.
    let data = match unsafe { read_caller_slice(buf_ptr, len as u64) } {
        Some(s) => s,
        None => return u64::MAX,
    };
    match crate::ipc::pipe_write(pipe_id, data) {
        Some(0) => {
            crate::platform::log("[syscall] write: broken pipe\n");
            u64::MAX
        }
        Some(n) => n as u64,
        None => {
            let idx = crate::scheduler::current_task_index();
            unsafe {
                let tasks = &raw mut crate::scheduler::TASKS;
                (*tasks)[idx].state = crate::scheduler::TaskState::Blocked;
                (*tasks)[idx].block_reason = crate::scheduler::BlockReason::PipeWrite(pipe_id);
            }
            0
        }
    }
}

/// Read from pipe `pipe_id` (read end) into the user buffer (non-blocking).
///
/// Returns: `n>0` = bytes read; `0` = true EOF (no writers left); and
/// `NET_WOULDBLOCK` = empty but a writer is still open (try again later).
/// The distinct would-block sentinel — vs. collapsing both empty cases to 0 —
/// is what lets a consumer like `cat` block in user space until real EOF,
/// which concurrent pipelines need (the producer writes while the consumer
/// reads). Kernel-context readers that just poll treat WOULDBLOCK as "nothing
/// right now". For a sequential pipe the writer is already closed, so this
/// returns EOF (0) immediately.
fn pipe_read_blocking(pipe_id: usize, buf_ptr: u64, buf_len: u64) -> u64 {
    let len = (buf_len as usize).min(4096);
    let mut tmp = [0u8; 4096];
    let read_buf = &mut tmp[..len];
    match crate::ipc::pipe_read(pipe_id, read_buf) {
        Some(n) => {
            if n > 0 {
                if !unsafe { write_to_caller(buf_ptr, &read_buf[..n]) } {
                    return u64::MAX;
                }
            }
            n as u64 // n>0 = data, n==0 = EOF (no writers left)
        }
        None => numbers::NET_WOULDBLOCK, // empty, writer still open
    }
}

/// SYS_READ(fd, buf_ptr, buf_len) → bytes read, or u64::MAX on bad fd.
///
/// Per-process routing via the FD table: a pipe read-end reads the pipe; a
/// Console fd (or fd 0 with no per-process entry) drains the TTY line
/// discipline (cooked mode — bytes appear a line at a time on Enter,
/// non-blocking, 0 = nothing ready). File reads go through SYS_FREAD.
fn handle_read(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    let len = buf_len as usize;
    if len == 0 {
        return 0;
    }
    if len > 4096 {
        return u64::MAX;
    }
    match current_fd_entry(fd) {
        Some(crate::process::FdEntry::Pipe { pipe_id, is_read_end: true }) => {
            pipe_read_blocking(pipe_id as usize, buf_ptr, buf_len)
        }
        Some(crate::process::FdEntry::Pipe { is_read_end: false, .. }) => u64::MAX,
        Some(crate::process::FdEntry::Console) => stdin_drain(buf_ptr, len),
        // No per-process entry: fd 0 still means stdin (kernel-context demos,
        // processes whose slot we can't resolve). Other fds are bad here.
        None if fd == 0 => stdin_drain(buf_ptr, len),
        _ => u64::MAX,
    }
}

/// Drain the TTY line discipline into the user buffer.
fn stdin_drain(buf_ptr: u64, len: usize) -> u64 {
    let slice = match unsafe { caller_slice_mut(buf_ptr, len as u64) } {
        Some(s) => s,
        None => return u64::MAX,
    };
    crate::platform::stdin_read(slice) as u64
}

fn handle_exit(code: u64) -> u64 {
    // Release any pipe-end FDs this process held so the other end of a pipe
    // sees EOF / broken-pipe (a producer exiting closes its write end → the
    // downstream reader's blocking read returns EOF). Must run before we mark
    // the task Exited.
    crate::process::release_pipe_fds();
    // Publish the exit code before transitioning to Exited so any
    // SYS_THREAD_JOIN waiter (task #45) can read it via
    // scheduler::task_exit_code once pick_next unblocks it.
    crate::scheduler::set_current_exit_code(code);
    // Mark task as exited so pick_next will skip it forever.
    let idx = crate::scheduler::current_task_index();
    unsafe {
        let tasks = &raw mut crate::scheduler::TASKS;
        (*tasks)[idx].state = crate::scheduler::TaskState::Exited;
    }
    // DIAGNOSTIC: visible on screen + serial.
    crate::platform::log("[exit] slot=");
    crate::platform::log_num(idx as u64);
    crate::platform::log(" code=");
    crate::platform::log_num(code);
    crate::platform::log("\n");
    // Process-level Zombie transition: use the slot-keyed lookup
    // (`pid_for_slot`) instead of the stale `current_pid()` global so we
    // mark the RIGHT process. This makes SYS_WAIT's legacy PROCESS_TABLE
    // fallback return immediately for Ring-3 children, and wakes a parent
    // that may be blocked in `process::wait`.
    {
        let slot = crate::scheduler::current_task_index();
        if let Some(pid) = crate::process::pid_for_slot(slot) {
            unsafe {
                if let Some(proc) = crate::process::get_mut(pid) {
                    crate::platform::log("[exit] marking PID=");
                    crate::platform::log_num(pid.0 as u64);
                    crate::platform::log(" Zombie\n");
                    proc.exit(crate::process::ExitStatus::failure(code as i32));
                    if let Some(parent_pid) = proc.parent {
                        if let Some(parent) = crate::process::get_mut(parent_pid) {
                            if parent.state == crate::process::ProcessState::Waiting {
                                parent.state = crate::process::ProcessState::Running;
                            }
                        }
                    }
                } else {
                    crate::platform::log("[exit] pid_for_slot returned PID with no proc\n");
                }
            }
        } else {
            crate::platform::log("[exit] no process for slot\n");
        }
    }
    // Reset any console flags the exiting process may have left set so the
    // next scheduled task (usually the shell) can draw its prompt/output.
    crate::platform::get().reset_tty_flags();
    // Yield immediately so the scheduler picks something else
    // (including any join_block waiter). Without this, the caller
    // returns from dispatch and keeps executing kernel-mode code on
    // an Exited slot — and if IF was cleared by a prior schedule()
    // call, a following `hlt` halts the CPU indefinitely.
    crate::platform::log("[exit] scheduling\n");
    crate::platform::schedule();
    crate::platform::log("[exit] schedule returned\n");
    // Returns 0; the syscall asm will SYSRET back to Ring 3, the user RIP
    // points at whatever follows the `syscall` instruction (usually
    // padding/zeros) and the task will page-fault on its next instruction
    // fetch. The page-fault handler kills the (already-exited) task and
    // the next timer tick picks something else.
    // TODO: cleaner shutdown — modify the iret frame to point to a
    // kernel-mode trampoline that calls schedule directly.
    0
}

fn handle_yield() {
    // Yield the rest of this time slice. The platform's schedule() will
    // pick the next ready task and context-switch. When the caller is
    // eventually re-scheduled, this returns and SYSRET sends control
    // back to the user's instruction after `syscall`.
    crate::platform::schedule();
}

fn handle_getpid() -> u64 {
    crate::scheduler::current_task_index() as u64
}

fn handle_info() -> u64 {
    crate::platform::log("Semantic OS Kernel Core v0.1.0\n");
    0
}

/// SYS_SYSINFO(buf_ptr, buf_len) — write heap stats `[used:u64][free:u64]
/// [free_blocks:u64]` (24 bytes LE) into the caller buffer. Read-only; backs
/// the shell `free` builtin. Returns 0 on success, u64::MAX if the buffer is
/// too small.
fn handle_sysinfo(buf_ptr: u64, buf_len: u64) -> u64 {
    if buf_ptr == 0 || (buf_len as usize) < 24 {
        return u64::MAX;
    }
    let (used, free, blocks) = crate::memory::heap::stats();
    let buf = match unsafe { caller_slice_mut(buf_ptr, 24) } {
        Some(b) => b,
        None => return u64::MAX,
    };
    buf[0..8].copy_from_slice(&(used as u64).to_le_bytes());
    buf[8..16].copy_from_slice(&(free as u64).to_le_bytes());
    buf[16..24].copy_from_slice(&(blocks as u64).to_le_bytes());
    0
}

/// SYS_ASK(prompt_ptr, prompt_len, out_ptr, out_len) — one-shot LLM query for
/// the shell `ask` builtin. Hands the prompt to the platform's network agent
/// and writes the plain-text answer into the caller buffer; returns its length.
/// The platform impl runs the (multi-second, network) call synchronously with
/// interrupts enabled. Bounded prompt size keeps the request sane.
fn handle_ask(prompt_ptr: u64, prompt_len: u64, out_ptr: u64, out_len: u64) -> u64 {
    const ASK_PROMPT_CAP: usize = 16 * 1024;
    const ASK_OUT_CAP: usize = 8 * 1024;

    if prompt_ptr == 0
        || out_ptr == 0
        || prompt_len == 0
        || prompt_len > ASK_PROMPT_CAP as u64
        || out_len == 0
    {
        return 0;
    }

    let out_cap = (out_len as usize).min(ASK_OUT_CAP);
    if out_cap == 0 {
        return 0;
    }
    if caller_needs_validation() && !validate_user_ptr(out_ptr, out_cap as u64) {
        return 0;
    }

    // Copy-in/copy-out for the long-running, interrupts-enabled path. The old
    // version handed `llm_ask` direct references to caller memory while the
    // network stack ran for seconds with preemption enabled, so a sibling
    // thread in the same address space could race the prompt/output buffers
    // (2026-07-17 review, P2 TOCTOU class). Fixed-size locked scratch keeps
    // kernel-core allocator-free and avoids large stack frames.
    static ASK_PROMPT_SCRATCH: crate::sync::Mutex<[u8; ASK_PROMPT_CAP]> =
        crate::sync::Mutex::new([0; ASK_PROMPT_CAP]);
    static ASK_OUT_SCRATCH: crate::sync::Mutex<[u8; ASK_OUT_CAP]> =
        crate::sync::Mutex::new([0; ASK_OUT_CAP]);

    let mut prompt_guard = ASK_PROMPT_SCRATCH.lock();
    let prompt_len = prompt_len as usize;
    let prompt_src = match unsafe { read_caller_slice(prompt_ptr, prompt_len as u64) } {
        Some(s) => s,
        None => return 0,
    };
    prompt_guard[..prompt_len].copy_from_slice(prompt_src);

    let mut out_guard = ASK_OUT_SCRATCH.lock();
    let n = crate::platform::get()
        .llm_ask(&prompt_guard[..prompt_len], &mut out_guard[..out_cap])
        .min(out_cap);
    if n > 0 && !unsafe { write_to_caller(out_ptr, &out_guard[..n]) } {
        return 0;
    }
    n as u64
}

/// SYS_PS(buf_ptr, buf_len) — write one 24-byte record per live scheduler task
/// into the caller buffer and return the count. Read-only; backs the shell
/// `ps` builtin and lets the agent see what's running (and at which security
/// tier). Record layout (LE): slot:u32 @0, id:u32 @4, run_count:u64 @8,
/// state:u8 @16, max_tier:u8 @17, is_kernel:u8 @18, user_id:u8 @19, pad @20.
fn handle_ps(buf_ptr: u64, buf_len: u64) -> u64 {
    const REC: usize = 24;
    if buf_ptr == 0 {
        return 0;
    }
    let cap = (buf_len as usize) / REC;
    if cap == 0 {
        return 0;
    }
    let buf = match unsafe { caller_slice_mut(buf_ptr, buf_len) } {
        Some(b) => b,
        None => return 0,
    };
    let tasks = unsafe { &*core::ptr::addr_of!(crate::scheduler::TASKS) };
    let mut n = 0usize;
    for (i, t) in tasks.iter().enumerate() {
        if t.state == crate::scheduler::TaskState::Empty {
            continue;
        }
        if n >= cap {
            break;
        }
        let o = n * REC;
        buf[o..o + 4].copy_from_slice(&(i as u32).to_le_bytes());
        buf[o + 4..o + 8].copy_from_slice(&(t.id as u32).to_le_bytes());
        buf[o + 8..o + 16].copy_from_slice(&t.run_count.to_le_bytes());
        buf[o + 16] = t.state as u8;
        buf[o + 17] = t.max_tier;
        buf[o + 18] = if t.is_kernel { 1 } else { 0 };
        buf[o + 19] = t.user_id;
        buf[o + 20..o + 24].copy_from_slice(&[0u8; 4]);
        n += 1;
    }
    n as u64
}

fn handle_alloc(tier: u64, _size: u64) -> u64 {
    // Verify tier access
    let max_tier = crate::scheduler::current_task_max_tier();
    if tier as u8 > max_tier {
        crate::platform::log("[syscall] alloc: tier access denied\n");
        return 0;
    }
    // Allocate a physical frame from the requested tier's pool
    match crate::platform::get().alloc_frame(tier as u8) {
        Some(addr) => addr,
        None => {
            crate::platform::log("[syscall] alloc: pool exhausted for tier ");
            crate::platform::log_num(tier);
            crate::platform::log("\n");
            0
        }
    }
}

fn handle_free(ptr: u64, _size: u64) -> u64 {
    if ptr == 0 { return u64::MAX; }
    if crate::platform::get().free_frame(ptr) {
        0 // success
    } else {
        u64::MAX // error: address not recognized
    }
}

// ============================================================================
// Per-process env + CWD (Phase 14 prereq #3)
// ============================================================================

/// SYS_GET_CWD(buf_ptr, buf_len) → bytes_written, or u64::MAX on error
/// (no current process, or buffer too small for the CWD).
fn handle_get_cwd(buf_ptr: u64, buf_len: u64) -> u64 {
    let pid = crate::process::current_pid();
    let len_out = buf_len as usize;
    // Only `cwd_len` bytes (≤128) are ever written, bounded by the
    // `cwd_len > len_out` check below — so any non-empty caller buffer is
    // safe. The old `len_out > 256` upper cap wrongly rejected generous
    // buffers: semos_std::env::current_dir() passes a 4 KiB scratch buffer,
    // so every call returned u64::MAX → rustc's "Current directory is
    // invalid" fatal (M27 iter 8). Keep only a sane sanity ceiling.
    if len_out == 0 || len_out > (1 << 20) { return u64::MAX; }

    let cwd_bytes: [u8; 128];
    let cwd_len: usize;
    unsafe {
        let proc = match crate::process::get(pid) {
            Some(p) => p,
            None => return u64::MAX,
        };
        cwd_bytes = proc.cwd;
        cwd_len = proc.cwd_len;
    }

    if cwd_len > len_out { return u64::MAX; }
    if !unsafe { write_to_caller(buf_ptr, &cwd_bytes[..cwd_len]) } { return u64::MAX; }
    cwd_len as u64
}

/// SYS_SET_CWD(path_ptr, path_len) → 0 on success, u64::MAX on error
/// (path not absolute, too long, or current process not present).
fn handle_set_cwd(path_ptr: u64, path_len: u64) -> u64 {
    let len = path_len as usize;
    if len == 0 || len > 128 { return u64::MAX; }
    let path = match unsafe { read_caller_str(path_ptr, path_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };
    let pid = crate::process::current_pid();
    unsafe {
        let proc = match crate::process::get_mut(pid) {
            Some(p) => p,
            None => return u64::MAX,
        };
        if proc.set_cwd(path) { 0 } else { u64::MAX }
    }
}

/// SYS_GET_ENV(key_ptr, key_len, val_buf_ptr, val_buf_len) →
///   bytes_written on success, 0 if key not found, u64::MAX on error
///   (bad UTF-8 in key, buffer too small, current process missing).
fn handle_get_env(key_ptr: u64, key_len: u64, val_buf_ptr: u64, val_buf_len: u64) -> u64 {
    let klen = key_len as usize;
    let vlen_out = val_buf_len as usize;
    if klen == 0 || klen > 64 || vlen_out == 0 || vlen_out > 1024 { return u64::MAX; }

    let key = match unsafe { read_caller_str(key_ptr, key_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };
    let pid = crate::process::current_pid();
    unsafe {
        let proc = match crate::process::get(pid) {
            Some(p) => p,
            None => return u64::MAX,
        };
        match proc.env_get(key) {
            Some(value) => {
                if value.len() > vlen_out { return u64::MAX; }
                if !write_to_caller(val_buf_ptr, value) { return u64::MAX; }
                value.len() as u64
            }
            None => 0,
        }
    }
}

/// SYS_SET_ENV(key_ptr, key_len, val_ptr, val_len) → 0 on success,
/// u64::MAX on error (bad UTF-8, env block full, key too long).
fn handle_set_env(key_ptr: u64, key_len: u64, val_ptr: u64, val_len: u64) -> u64 {
    let klen = key_len as usize;
    let vlen = val_len as usize;
    if klen == 0 || klen > 64 || vlen > 1024 { return u64::MAX; }
    let key = match unsafe { read_caller_str(key_ptr, key_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };
    // vlen == 0 is legal (clears the value); the slice helpers reject
    // zero-length ranges, so special-case it.
    let val = if vlen == 0 {
        ""
    } else {
        match unsafe { read_caller_str(val_ptr, val_len) } {
            Some(s) => s,
            None => return u64::MAX,
        }
    };
    let pid = crate::process::current_pid();
    unsafe {
        let proc = match crate::process::get_mut(pid) {
            Some(p) => p,
            None => return u64::MAX,
        };
        if proc.env_set(key, val) { 0 } else { u64::MAX }
    }
}

fn handle_pool_info(tier: u64) -> u64 {
    crate::platform::log("[syscall] pool_info for tier ");
    crate::platform::log_num(tier);
    crate::platform::log("\n");
    0
}

/// SYS_HEAP_ALLOC(size, align) → ptr (0 on failure).
///
/// General-purpose allocator backing for the Phase 14 std shim. Unlike
/// SYS_ALLOC which is frame-granular and tier-aware, this routes
/// through the kernel's free-list heap (memory::heap) and returns
/// arbitrarily-sized + arbitrarily-aligned blocks.
///
/// Used by:
///  - `std::alloc::GlobalAlloc::alloc` once the std shim lands (M25)
///  - Any kernel-internal code that wants `Vec`/`Box`/`String` (today
///    none — kernel-core is no_alloc — but this opens the door)
fn handle_heap_alloc(size: u64, align: u64) -> u64 {
    let ptr = crate::memory::heap::allocate(size as usize, align as usize);
    ptr as u64
}

/// SYS_MMAP_ANON(addr, size) → addr on success, u64::MAX on failure.
///
/// Map `size` bytes (rounded up to whole pages) of fresh, zeroed,
/// USER-accessible memory into the calling process's address space
/// starting at virtual `addr`. Backs the user-space heap allocator in
/// semos-std (M25 Tier 2 #50).
///
/// `addr` must be page-aligned and in the lower half (user space).
/// The memory is mapped ReadWrite with the user bit set, so Ring-3
/// code can actually touch it — unlike SYS_HEAP_ALLOC's kernel-heap
/// pointers, which fault on user write.
fn handle_mmap_anon(addr: u64, size: u64) -> u64 {
    if size == 0 { return u64::MAX; }
    if addr & 0xFFF != 0 { return u64::MAX; }        // must be page-aligned
    if addr >= 0x0000_8000_0000_0000 { return u64::MAX; } // user (lower half) only

    let cr3 = crate::platform::get().current_cr3();
    // cr3==0 would be the kernel boot tables — refuse, this is a
    // user-process-only facility.
    if cr3 == 0 { return u64::MAX; }

    if crate::platform::get().map_user_region(cr3, addr, size) {
        addr
    } else {
        crate::platform::log("[mmap] map_user_region FAILED (pool exhausted?)\n");
        u64::MAX
    }
}

/// SYS_HEAP_FREE(ptr, size, align) → 0 on success, u64::MAX on bad ptr.
///
/// `size` and `align` are accepted for compatibility with
/// `std::alloc::dealloc`; the kernel-side heap stores the actual size
/// in the block header and ignores the args. Caller may pass 0.
fn handle_heap_free(ptr: u64, size: u64, align: u64) -> u64 {
    if ptr == 0 { return u64::MAX; }
    crate::memory::heap::deallocate(ptr as *mut u8, size as usize, align as usize);
    0
}

// --- File I/O handlers ---

/// SYS_OPEN(path_ptr, path_len) → fd or u64::MAX on error.
///
/// All FDs (path / ramfs / pipe / console) now live in the running process's
/// `FdTable` — there are no separate global FD-number ranges anymore. Path
/// opens store an `FdEntry::Path{suid,position,is_directory}`; ramfs opens
/// wrap the ramfs handle in `FdEntry::Ramfs`.

/// SYS_OPEN flag bits (passed in `flags` arg).
pub mod open_flags {
    /// Create the file if it doesn't exist. Tier is taken from bits 1-2.
    pub const CREATE: u64 = 1 << 0;
    /// Open as a directory (must already exist). Used by SYS_READDIR.
    pub const DIRECTORY: u64 = 1 << 1;
    // Bits 4-5 reserved for tier when CREATE is set:
    // tier = (flags >> 4) & 0x3 → 0=Public, 1=Internal, 2=Sensitive, 3=Secret.
}

// ============================================================================
// SYS_OPEN — path namespace first, ramfs fallback
// ============================================================================

fn handle_open(path_ptr: u64, path_len: u64, flags: u64) -> u64 {
    let name = match unsafe { read_caller_str(path_ptr, path_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };

    // Path-namespace entries are absolute (start with '/'). Anything else
    // falls straight through to ramfs's flat-name lookup.
    if name.starts_with('/') {
        return handle_open_path(name, flags);
    }

    // Ramfs path: existing behaviour for embedded files.
    let fs = match crate::fs::ramfs::get_fs() {
        Some(fs) => fs,
        None => return u64::MAX,
    };
    let fd_table = match crate::fs::ramfs::get_fd_table_mut() {
        Some(t) => t,
        None => return u64::MAX,
    };

    match fd_table.open(fs, name) {
        Some(ramfs_fd) => {
            // Wrap the ramfs handle in a per-process FD; the ramfs fd table
            // remains the backing store (it owns the read cursor).
            match crate::process::with_current_fds_mut(|t| {
                t.alloc(crate::process::FdEntry::Ramfs { handle: ramfs_fd as u32 })
            })
            .flatten()
            {
                Some(user_fd) => user_fd as u64,
                None => {
                    fd_table.close(ramfs_fd);
                    u64::MAX
                }
            }
        }
        None => {
            crate::platform::log("[syscall] open: file not found: ");
            crate::platform::log(name);
            crate::platform::log("\n");
            u64::MAX
        }
    }
}

/// Open through the path namespace. Resolves the path, possibly creates
/// the entry per CREATE flag, allocates a path FD, returns its number.
fn handle_open_path(path: &str, flags: u64) -> u64 {
    use crate::fs::paths::{Namespace, FsError};
    use crate::semantic::object::SecurityTier;

    // M27 DEMO 80: read-only sysroot files streamed from the SATA blob, exposed
    // at /sysroot/<name> (no namespace object). Used by the rustc crate loader to
    // open `--extern core=/sysroot/libcore-<hash>.rmeta` etc.
    if let Some(rest) = path.strip_prefix("/sysroot/") {
        if !rest.is_empty() && !rest.contains('/') {
            if let Some(idx) = crate::sysroot_blob::find(rest) {
                return match crate::process::with_current_fds_mut(|t| {
                    t.alloc(crate::process::FdEntry::SysrootBlob { idx: idx as u32, position: 0 })
                })
                .flatten()
                {
                    Some(fd) => fd as u64,
                    None => u64::MAX,
                };
            }
        }
    }

    let want_create = (flags & open_flags::CREATE) != 0;
    let want_dir = (flags & open_flags::DIRECTORY) != 0;

    let suid = match Namespace::resolve(path) {
        Ok(s) => s,
        Err(FsError::NotFound) if want_create => {
            // Create the file with the requested tier (default Public).
            let tier = match (flags >> 4) & 0x3 {
                0 => SecurityTier::Public,
                1 => SecurityTier::Internal,
                2 => SecurityTier::Sensitive,
                _ => SecurityTier::Secret,
            };
            match Namespace::create_file(path, tier, &[]) {
                Ok(s) => s,
                Err(_) => {
                    crate::platform::log("[open] create_file failed: ");
                    crate::platform::log(path);
                    crate::platform::log("\n");
                    return u64::MAX;
                }
            }
        }
        Err(e) => {
            crate::platform::log("[open] resolve failed (");
            crate::platform::log(match e {
                FsError::NotFound => "NotFound",
                FsError::NotADirectory => "NotADirectory",
                FsError::NotAbsolute => "NotAbsolute",
                _ => "other",
            });
            crate::platform::log(") for ");
            crate::platform::log(path);
            crate::platform::log("\n");
            return u64::MAX;
        }
    };

    // Security: caller's max_tier must cover the object's tier. The
    // current task's max_tier is the strongest filter; without it any
    // user could open Secret objects by path. SecurityTier is repr(u8)
    // so a numeric ≥ comparison matches the SecurityTier::can_access
    // semantics (higher value = stronger clearance).
    let caller_tier = crate::scheduler::current_task_max_tier();
    let obj_tier = {
        let registry = crate::semantic::registry::global_registry();
        match registry.get(&suid) {
            Some(o) => o.tier,
            None => {
                crate::platform::log("[open] DANGLING entry (resolve ok, object missing) for ");
                crate::platform::log(path);
                crate::platform::log("\n");
                return u64::MAX;
            }
        }
    };
    if caller_tier < (obj_tier as u8) {
        crate::platform::log("[syscall] open: tier denied for ");
        crate::platform::log(path);
        crate::platform::log("\n");
        return u64::MAX;
    }

    // Is it a directory? Path-namespace dirs have ContentType::Structured.
    let is_dir = {
        let registry = crate::semantic::registry::global_registry();
        registry.get(&suid).map(|o| {
            o.content_type == crate::semantic::object::ContentType::Structured
        }).unwrap_or(false)
    };
    if want_dir && !is_dir {
        // Explicit dir-open of a non-dir is an error.
        return u64::MAX;
    }

    match crate::process::with_current_fds_mut(|t| {
        t.alloc(crate::process::FdEntry::Path { suid, position: 0, is_directory: is_dir })
    })
    .flatten()
    {
        Some(fd) => fd as u64,
        None => {
            crate::platform::log("[open] FD table full for ");
            crate::platform::log(path);
            crate::platform::log("\n");
            u64::MAX
        }
    }
}

/// SYS_CLOSE(fd) → 0 on success, u64::MAX on error. All FD kinds live in the
/// running process's table now; closing releases any backing resource (pipe
/// endpoint, ramfs handle) and frees the slot.
fn handle_close(fd: u64) -> u64 {
    use crate::process::FdEntry;
    crate::process::with_current_fds_mut(|t| {
        match t.get(fd as i32).copied() {
            Some(FdEntry::Pipe { pipe_id, is_read_end }) => {
                if is_read_end {
                    crate::ipc::close_read_end(pipe_id as usize);
                } else {
                    crate::ipc::close_write_end(pipe_id as usize);
                }
                t.close(fd as i32);
                0
            }
            Some(FdEntry::Ramfs { handle }) => {
                if let Some(rt) = crate::fs::ramfs::get_fd_table_mut() {
                    rt.close(handle as usize);
                }
                t.close(fd as i32);
                0
            }
            Some(FdEntry::Console)
            | Some(FdEntry::Path { .. })
            | Some(FdEntry::SysrootBlob { .. }) => {
                t.close(fd as i32);
                0
            }
            _ => u64::MAX, // Empty / no such FD
        }
    })
    .unwrap_or(u64::MAX)
}

/// SYS_FREAD(fd, buf_ptr, buf_len) → bytes read, 0 = EOF, u64::MAX = error
fn handle_fread(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    use crate::process::FdEntry;
    let len = buf_len as usize;
    // Match the path-FD FWRITE upper bound (task #44). Pipe/ramfs paths use
    // stack tmp buffers — 4 KiB ceiling there.
    if len == 0 || len > crate::semantic::object::MAX_FILE_CONTENT { return u64::MAX; }

    match current_fd_entry(fd) {
        Some(FdEntry::Pipe { pipe_id, is_read_end: true }) => {
            pipe_read_blocking(pipe_id as usize, buf_ptr, buf_len)
        }
        Some(FdEntry::Pipe { is_read_end: false, .. }) => u64::MAX,
        Some(FdEntry::Console) => stdin_drain(buf_ptr, len),

        // Path-namespace FD: read the SemanticObject at the cursor, advance it.
        Some(FdEntry::Path { suid, position, is_directory }) => {
            if is_directory { return u64::MAX; } // use SYS_READDIR
            let registry = crate::semantic::registry::global_registry();
            let obj = match registry.get(&suid) {
                Some(o) => o,
                None => return u64::MAX,
            };
            let bytes = match obj.content.as_bytes() {
                Some(b) => b,
                None => return u64::MAX,
            };
            let pos = position as usize;
            if pos >= bytes.len() { return 0; }
            let n = (bytes.len() - pos).min(len);
            if !unsafe { write_to_caller(buf_ptr, &bytes[pos..pos + n]) } { return u64::MAX; }
            crate::process::with_current_fds_mut(|t| {
                if let Some(FdEntry::Path { position, .. }) = t.get_mut(fd as i32) {
                    *position = (pos + n) as u32;
                }
            });
            n as u64
        }

        // M27 DEMO 80: sysroot blob file — stream from the SATA disk at cursor.
        Some(FdEntry::SysrootBlob { idx, position }) => {
            let buf = match unsafe { caller_slice_mut(buf_ptr, len as u64) } {
                Some(b) => b,
                None => return u64::MAX,
            };
            match crate::sysroot_blob::read(idx as usize, position, buf) {
                Some(n) => {
                    if n > 0 {
                        crate::process::with_current_fds_mut(|t| {
                            if let Some(FdEntry::SysrootBlob { position: p, .. }) = t.get_mut(fd as i32) {
                                *p = position + n as u64;
                            }
                        });
                    }
                    n as u64
                }
                None => u64::MAX,
            }
        }

        // Legacy ramfs file: delegate to the ramfs fd table via the handle.
        Some(FdEntry::Ramfs { handle }) => {
            let fs = match crate::fs::ramfs::get_fs() {
                Some(fs) => fs,
                None => return u64::MAX,
            };
            let fd_table = match crate::fs::ramfs::get_fd_table_mut() {
                Some(t) => t,
                None => return u64::MAX,
            };
            let mut tmp = [0u8; 4096];
            let read_len = len.min(4096);
            let read_buf = &mut tmp[..read_len];
            match fd_table.read(fs, handle as usize, read_buf) {
                Some(n) => {
                    if n > 0 {
                        if !unsafe { write_to_caller(buf_ptr, &read_buf[..n]) } {
                            return u64::MAX;
                        }
                    }
                    n as u64
                }
                None => u64::MAX,
            }
        }

        _ => u64::MAX, // Empty / no such FD
    }
}

/// SYS_SYSROOT_INFO(idx, name_buf_ptr, name_buf_len) → file byte length, or
/// u64::MAX if idx is out of range / no blob. Writes the file name (NUL-padded,
/// up to name_buf_len bytes) into name_buf_ptr.
fn handle_sysroot_info(idx: u64, name_buf_ptr: u64, name_buf_len: u64) -> u64 {
    let cap = name_buf_len as usize;
    if cap == 0 || cap > 256 {
        return u64::MAX;
    }
    let mut tmp = [0u8; 256];
    let n = cap.min(tmp.len());
    match crate::sysroot_blob::info(idx as usize, &mut tmp[..n]) {
        Some(len) => {
            if !unsafe { write_to_caller(name_buf_ptr, &tmp[..n]) } { return u64::MAX; }
            len
        }
        None => u64::MAX,
    }
}

/// SYS_SYSROOT_READ(idx, offset, buf_ptr, buf_len) → bytes read (0 = EOF), or
/// u64::MAX on error. Streams file `idx` from the SATA blob at byte `offset`.
fn handle_sysroot_read(idx: u64, offset: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    let len = buf_len as usize;
    if len == 0 || len > crate::semantic::object::MAX_FILE_CONTENT {
        return u64::MAX;
    }
    // SAFETY: range-validated for Ring-3 callers; must still be mapped.
    let buf = match unsafe { caller_slice_mut(buf_ptr, buf_len) } {
        Some(b) => b,
        None => return u64::MAX,
    };
    match crate::sysroot_blob::read(idx as usize, offset, buf) {
        Some(n) => n as u64,
        None => u64::MAX,
    }
}

/// SYS_FLASH_SYSROOT() → bytes copied, or u64::MAX on error. Copies sysroot.img
/// off the FAT USB stick (usb0) onto the SATA disk (sata0); logs the reason on
/// failure (serial), since the user-facing return is just success/fail.
fn handle_flash_sysroot() -> u64 {
    match crate::sysroot_blob::flash_from_usb() {
        Ok(n) => n,
        Err(e) => {
            crate::platform::log("[flash] FAILED: ");
            crate::platform::log(e);
            crate::platform::log("\n");
            u64::MAX
        }
    }
}

/// SYS_FWRITE(fd, buf_ptr, buf_len) → bytes written or u64::MAX
fn handle_fwrite(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    use crate::process::FdEntry;
    match current_fd_entry(fd) {
        Some(FdEntry::Console) => console_write(buf_ptr, buf_len),
        Some(FdEntry::Pipe { pipe_id, is_read_end: false }) => {
            pipe_write_blocking(pipe_id as usize, buf_ptr, buf_len)
        }
        Some(FdEntry::Pipe { is_read_end: true, .. }) => u64::MAX,

        // Path-namespace FD: positional write at the FD cursor (so multiple
        // sequential writes — e.g. shell `echo` emitting text then newline,
        // or io::Write::write_all looping — accumulate instead of clobbering).
        // Small files splice through a fixed stack buffer (kernel-core has no
        // allocator); larger writes fall back to whole-file overwrite.
        Some(FdEntry::Path { suid, position, is_directory }) => {
            if is_directory { return u64::MAX; }
            // task #44: accept up to MAX_FILE_CONTENT (64 KiB); ≤256 B stays
            // inline, larger routes through heap-Allocated via from_bytes.
            let len = buf_len as usize;
            if len > crate::semantic::object::MAX_FILE_CONTENT { return u64::MAX; }
            if len == 0 { return 0; }
            // Validated: Ring-3 callers cannot source file contents from
            // kernel memory (2026-07-17 review, critical #1).
            let data = match unsafe { read_caller_slice(buf_ptr, buf_len) } {
                Some(s) => s,
                None => return u64::MAX,
            };
            let pos = position as usize;
            let mut registry = crate::semantic::registry::global_registry();
            let obj = match registry.get_mut(&suid) {
                Some(o) => o,
                None => return u64::MAX,
            };

            const SPLICE_CAP: usize = 4096;
            let existing_len = obj.content.len();
            let new_content = if pos + len <= SPLICE_CAP && existing_len <= SPLICE_CAP {
                // Splice `data` in at `pos`, extending the file if needed.
                let mut tmp = [0u8; SPLICE_CAP];
                if let Some(ex) = obj.content.as_bytes() {
                    tmp[..existing_len].copy_from_slice(ex);
                }
                tmp[pos..pos + len].copy_from_slice(data);
                let new_len = core::cmp::max(pos + len, existing_len);
                crate::semantic::object::ObjectContent::from_bytes(&tmp[..new_len])
            } else {
                // Too big to splice on the stack — whole-file overwrite.
                crate::semantic::object::ObjectContent::from_bytes(data)
            };
            obj.content = match new_content {
                Some(c) => c,
                None => return u64::MAX,
            };
            // Advance the cursor past what we wrote.
            crate::process::with_current_fds_mut(|t| {
                if let Some(FdEntry::Path { position, .. }) = t.get_mut(fd as i32) {
                    *position = (pos + len) as u32;
                }
            });
            len as u64
        }

        // ramfs is read-only.
        Some(FdEntry::Ramfs { .. }) => {
            crate::platform::log("[syscall] fwrite: read-only filesystem\n");
            u64::MAX
        }

        _ => u64::MAX, // Empty / no such FD
    }
}

/// SYS_SEEK(fd, position) → 0 on success, u64::MAX on error
fn handle_seek(fd: u64, position: u64) -> u64 {
    use crate::process::FdEntry;
    match current_fd_entry(fd) {
        Some(FdEntry::Path { .. }) => {
            crate::process::with_current_fds_mut(|t| {
                if let Some(FdEntry::Path { position: p, .. }) = t.get_mut(fd as i32) {
                    *p = position as u32;
                }
            });
            0
        }
        Some(FdEntry::Ramfs { handle }) => {
            let fd_table = match crate::fs::ramfs::get_fd_table_mut() {
                Some(t) => t,
                None => return u64::MAX,
            };
            if fd_table.seek(handle as usize, position as usize) { 0 } else { u64::MAX }
        }
        _ => u64::MAX,
    }
}

/// SYS_STAT(path_ptr, path_len) → file size, or u64::MAX if not found
fn handle_stat(path_ptr: u64, path_len: u64) -> u64 {
    let name = match unsafe { read_caller_str(path_ptr, path_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };

    // Path-namespace first (absolute paths).
    if name.starts_with('/') {
        use crate::fs::paths::Namespace;
        let suid = match Namespace::resolve(name) {
            Ok(s) => s,
            Err(_) => return u64::MAX,
        };
        let registry = crate::semantic::registry::global_registry();
        return match registry.get(&suid) {
            Some(o) => o.content.len() as u64,
            None => u64::MAX,
        };
    }

    // Ramfs fallback.
    let fs = match crate::fs::ramfs::get_fs() {
        Some(fs) => fs,
        None => return u64::MAX,
    };
    match fs.find(name) {
        Some(file) => file.size() as u64,
        None => u64::MAX,
    }
}

/// SYS_MKDIR(path_ptr, path_len) → 0 on success, u64::MAX on error.
fn handle_mkdir(path_ptr: u64, path_len: u64) -> u64 {
    let path = match unsafe { read_caller_str(path_ptr, path_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };
    match crate::fs::paths::Namespace::mkdir(path) {
        Ok(_) => 0,
        Err(_) => u64::MAX,
    }
}

/// SYS_UNLINK(path_ptr, path_len) → 0 on success, u64::MAX on error.
/// Removes the file or empty directory at the given absolute path.
fn handle_unlink(path_ptr: u64, path_len: u64) -> u64 {
    let path = match unsafe { read_caller_str(path_ptr, path_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };
    match crate::fs::paths::Namespace::unlink(path) {
        Ok(()) => 0,
        Err(_) => u64::MAX,
    }
}

// ============================================================================
// Phase 14 Tier 2 extended file ops (FSYNC / RENAME / TRUNCATE / STATX)
// ============================================================================

/// `StatX` is what SYS_STATX writes into the user-provided struct
/// pointer. Layout-stable so std::fs::Metadata can shim onto it.
///
/// All 64-bit fields so the layout is portable regardless of
/// alignment of the surrounding caller-side buffer.
#[repr(C)]
pub struct StatX {
    /// File size in bytes.
    pub size: u64,
    /// Object's SUID high half. Caller treats this as opaque.
    pub suid_high: u64,
    /// Object's SUID low half.
    pub suid_low: u64,
    /// Unix-epoch wall-clock seconds of creation. 0 if unknown.
    pub created_at: u64,
    /// Unix-epoch wall-clock seconds of last write. 0 if unknown.
    pub modified_at: u64,
    /// File type: 0=Binary, 1=Text, 2=Vector, 3=Directory (Structured), 4=Reference.
    pub file_type: u32,
    /// Security tier: 0=Public, 1=Internal, 2=Sensitive, 3=Secret.
    pub tier: u32,
    /// Reserved for future use; always 0.
    pub _reserved: [u64; 3],
}

const _: [(); 72] = [(); core::mem::size_of::<StatX>()];

fn encode_statx(st: &StatX, out: &mut [u8; core::mem::size_of::<StatX>()]) {
    out[0..8].copy_from_slice(&st.size.to_le_bytes());
    out[8..16].copy_from_slice(&st.suid_high.to_le_bytes());
    out[16..24].copy_from_slice(&st.suid_low.to_le_bytes());
    out[24..32].copy_from_slice(&st.created_at.to_le_bytes());
    out[32..40].copy_from_slice(&st.modified_at.to_le_bytes());
    out[40..44].copy_from_slice(&st.file_type.to_le_bytes());
    out[44..48].copy_from_slice(&st.tier.to_le_bytes());
    out[48..56].copy_from_slice(&st._reserved[0].to_le_bytes());
    out[56..64].copy_from_slice(&st._reserved[1].to_le_bytes());
    out[64..72].copy_from_slice(&st._reserved[2].to_le_bytes());
}

/// SYS_FSYNC() — flush the path namespace to virtio0. No args, no
/// per-FD selection (the snapshot covers everything). Returns 0 on
/// success, u64::MAX if virtio0 isn't present or the save failed.
///
/// std::fs::File::sync_all maps to this; cargo's atomic-rename
/// build flow depends on it.
fn handle_fsync() -> u64 {
    let dev = match crate::drivers::registry::get_block("virtio0") {
        Some(d) => d,
        None => {
            crate::platform::log("[syscall] fsync: no virtio0\n");
            return u64::MAX;
        }
    };
    match crate::fs::paths::Namespace::save(dev) {
        Ok(_) => 0,
        Err(_) => u64::MAX,
    }
}

/// SYS_RENAME(old_ptr, old_len, new_ptr, new_len) — atomically rename.
/// In our path namespace this means: remove old name from old parent,
/// add new name to new parent. SUID stays the same → atomic. Both
/// parent dirs must exist; new name must not.
fn handle_rename(old_ptr: u64, old_len: u64, new_ptr: u64, new_len: u64) -> u64 {
    let old_path = match unsafe { read_caller_str(old_ptr, old_len) } {
        Some(p) => p,
        None => return u64::MAX,
    };
    let new_path = match unsafe { read_caller_str(new_ptr, new_len) } {
        Some(p) => p,
        None => return u64::MAX,
    };
    match crate::fs::paths::Namespace::rename(old_path, new_path) {
        Ok(()) => 0,
        Err(_) => u64::MAX,
    }
}

/// SYS_TRUNCATE(path_ptr, path_len, new_size) — set file content
/// length to `new_size`. Shrinks (drops tail bytes) or extends with
/// zeros. Errors if `new_size > 256` (today's inline-only limit) —
/// follow-up task #44 wires the Allocated content path.
fn handle_truncate(path_ptr: u64, path_len: u64, new_size: u64) -> u64 {
    let path = match unsafe { read_caller_str(path_ptr, path_len) } {
        Some(p) => p,
        None => return u64::MAX,
    };
    match crate::fs::paths::Namespace::truncate(path, new_size as usize) {
        Ok(()) => 0,
        Err(_) => u64::MAX,
    }
}

/// SYS_STATX(path_ptr, path_len, out_struct_ptr) — fill a [`StatX`]
/// at `out_struct_ptr` with the object's metadata. Caller is
/// responsible for the struct buffer being large enough
/// (`core::mem::size_of::<StatX>()` bytes).
fn handle_statx(path_ptr: u64, path_len: u64, out_ptr: u64) -> u64 {
    let path = match unsafe { read_caller_str(path_ptr, path_len) } {
        Some(p) => p,
        None => return u64::MAX,
    };
    // The StatX out-struct is written through a raw `&mut *out_ptr` below —
    // for a Ring-3 caller it must be a user-space buffer, not a kernel
    // address (same class as the review's critical #2).
    if caller_needs_validation()
        && !validate_user_ptr(out_ptr, core::mem::size_of::<StatX>() as u64)
    {
        return u64::MAX;
    }
    // M27 DEMO 80: sysroot blob files (/sysroot/<name>) are read-only, disk-
    // backed, and have no namespace object. The rustc crate loader stats them
    // (Path::exists / is_file) before opening, so report size + regular-file.
    if let Some(rest) = path.strip_prefix("/sysroot/") {
        if let Some(idx) = crate::sysroot_blob::find(rest) {
            let stat = StatX {
                size: crate::sysroot_blob::file_len(idx).unwrap_or(0),
                suid_high: 0,
                suid_low: 0,
                created_at: 0,
                modified_at: 0,
                file_type: 0, // Binary (is_file() == file_type != 3)
                tier: 0, // Public
                _reserved: [0; 3],
            };
            let mut bytes = [0u8; core::mem::size_of::<StatX>()];
            encode_statx(&stat, &mut bytes);
            return if unsafe { write_to_caller(out_ptr, &bytes) } { 0 } else { u64::MAX };
        }
    }
    let suid = match crate::fs::paths::Namespace::resolve(path) {
        Ok(s) => s,
        Err(_) => return u64::MAX,
    };
    let registry = crate::semantic::registry::global_registry();
    let obj = match registry.get(&suid) {
        Some(o) => o,
        None => return u64::MAX,
    };
    let stat = StatX {
        size: obj.content.len() as u64,
        suid_high: suid.high,
        suid_low: suid.low,
        created_at: obj.created_at,
        modified_at: obj.modified_at,
        file_type: obj.content_type as u32,
        tier: obj.tier as u32,
        _reserved: [0; 3],
    };
    let mut bytes = [0u8; core::mem::size_of::<StatX>()];
    encode_statx(&stat, &mut bytes);
    if unsafe { write_to_caller(out_ptr, &bytes) } { 0 } else { u64::MAX }
}

/// SYS_READDIR(fd, idx, name_buf_ptr, name_buf_len) → name length on
/// success, 0 if no entry at idx (end of dir), u64::MAX on error.
///
/// Caller opens a directory with `SYS_OPEN(path, OPEN_FLAGS_DIRECTORY)`,
/// then walks indices 0..N until SYS_READDIR returns 0. Each call
/// writes the entry name into `name_buf` and returns its length.
///
/// SUIDs are intentionally not exposed at this layer — the user-space
/// surface is paths and names. To get the entry's metadata, the caller
/// joins the name to the dir path and calls SYS_STAT or SYS_OPEN.
fn handle_readdir(fd: u64, idx: u64, name_buf_ptr: u64, name_buf_len: u64) -> u64 {
    let dir_suid = match current_fd_entry(fd) {
        Some(crate::process::FdEntry::Path { suid, is_directory: true, .. }) => suid,
        _ => return u64::MAX,
    };

    let buf_len = name_buf_len as usize;
    if buf_len == 0 || buf_len > 256 { return u64::MAX; }

    // Walk the packed directory bytes to the requested index, then
    // copy that entry's name out. The path namespace exposes this
    // via the visitor-callback API.
    let registry = crate::semantic::registry::global_registry();
    let obj = match registry.get(&dir_suid) {
        Some(o) => o,
        None => return u64::MAX,
    };
    let bytes = match obj.content.as_bytes() {
        Some(b) => b,
        None => return u64::MAX,
    };
    if bytes.is_empty() { return 0; }
    let entries = match crate::fs::paths::DirEntries::parse(bytes) {
        Ok(e) => e,
        Err(_) => return u64::MAX,
    };
    for (i, item) in entries.enumerate() {
        if i as u64 != idx { continue; }
        let (name, _suid) = match item {
            Ok(t) => t,
            Err(_) => return u64::MAX,
        };
        let n = name.len().min(buf_len);
        if !unsafe { write_to_caller(name_buf_ptr, &name.as_bytes()[..n]) } {
            return u64::MAX;
        }
        return n as u64;
    }
    0 // index past end → caller stops walking
}

// --- Sleep handler ---

/// SYS_SLEEP(ticks) → 0
/// Blocks the current task for the given number of timer ticks.
///
/// Yields immediately via `platform::schedule()` so the block takes
/// effect right away rather than waiting for the next timer tick — the
/// pre-#46 behaviour where the caller kept executing for a full slice
/// after setting Blocked broke any test that needed a sibling to
/// observably progress before we polled.
fn handle_sleep(ticks: u64) -> u64 {
    if ticks == 0 { return 0; }
    let wake_at = crate::platform::ticks() + ticks;
    let idx = crate::scheduler::current_task_index();
    unsafe {
        let tasks = &raw mut crate::scheduler::TASKS;
        (*tasks)[idx].state = crate::scheduler::TaskState::Blocked;
        (*tasks)[idx].wake_at = wake_at;
        (*tasks)[idx].block_reason = crate::scheduler::BlockReason::Sleep;
    }
    crate::platform::schedule();
    0
}

// --- Pipe / FD management handlers ---

/// SYS_PIPE(out_ptr) → 0 on success, u64::MAX on error
///
/// Creates a pipe and writes [read_fd, write_fd] (two u64s) to `out_ptr`.
fn handle_pipe(out_ptr: u64) -> u64 {
    use crate::process::FdEntry;
    // Validate the out-pointer first (a Ring-3 caller must not aim this at
    // kernel memory) — before allocating anything, so a rejected call can't
    // leak pipe slots and FDs.
    if caller_needs_validation() && !validate_user_ptr(out_ptr, 16) {
        return u64::MAX;
    }
    let pipe_id = match crate::ipc::create_pipe() {
        Some(id) => id,
        None => {
            crate::platform::log("[syscall] pipe: no free pipe slots\n");
            return u64::MAX;
        }
    };

    // Allocate the two FDs in the CURRENT process's table.
    let fds = crate::process::with_current_fds_mut(|t| {
        let read_fd = t.alloc(FdEntry::Pipe { pipe_id: pipe_id as u32, is_read_end: true })?;
        match t.alloc(FdEntry::Pipe { pipe_id: pipe_id as u32, is_read_end: false }) {
            Some(write_fd) => Some((read_fd, write_fd)),
            None => {
                t.close(read_fd);
                None
            }
        }
    })
    .flatten();

    let (read_fd, write_fd) = match fds {
        Some(p) => p,
        None => {
            crate::ipc::close_read_end(pipe_id);
            crate::ipc::close_write_end(pipe_id);
            return u64::MAX;
        }
    };

    // Write the two FDs through the syscall copy-out helper, not a raw
    // pointer store, so this path follows the same audit rule as the rest of
    // the 2026-07-17 pointer fix.
    let mut out = [0u8; 16];
    out[0..8].copy_from_slice(&(read_fd as u64).to_le_bytes());
    out[8..16].copy_from_slice(&(write_fd as u64).to_le_bytes());
    if unsafe { write_to_caller(out_ptr, &out) } { 0 } else { u64::MAX }
}

/// SYS_DUP(old_fd) → new_fd or u64::MAX. Duplicates the entry into the
/// lowest free slot of the running process's FD table. Note: pipe ends are
/// duplicated by mapping only — the ipc endpoint refcount is unchanged, so
/// the first close releases it (matches the prior behavior).
fn handle_dup(old_fd: u64) -> u64 {
    let result = crate::process::with_current_fds_mut(|t| {
        let new = t.dup(old_fd as i32)?;
        // A duplicated pipe end is a new reference — bump the ipc refcount.
        if let Some(crate::process::FdEntry::Pipe { pipe_id, is_read_end }) =
            t.get(new).copied()
        {
            if is_read_end {
                crate::ipc::dup_read_end(pipe_id as usize);
            } else {
                crate::ipc::dup_write_end(pipe_id as usize);
            }
        }
        Some(new)
    })
    .flatten();
    match result {
        Some(new) => new as u64,
        None => u64::MAX,
    }
}

/// SYS_DUP2(old_fd, new_fd) → new_fd or u64::MAX. Used for stdio redirection
/// (e.g. `dup2(pipe_write_fd, 1)` to send a process's stdout to a pipe).
fn handle_dup2(old_fd: u64, new_fd: u64) -> u64 {
    let new = new_fd as usize;
    if new >= crate::process::MAX_FDS { return u64::MAX; }

    crate::process::with_current_fds_mut(|t| {
        // If the target FD currently holds a pipe end, release that ipc
        // endpoint before overwriting it.
        if let Some(crate::process::FdEntry::Pipe { pipe_id, is_read_end }) =
            t.get(new as i32).copied()
        {
            if is_read_end {
                crate::ipc::close_read_end(pipe_id as usize);
            } else {
                crate::ipc::close_write_end(pipe_id as usize);
            }
        }
        let r = t.dup2(old_fd as i32, new as i32);
        // If we copied a pipe end onto new_fd, that's a new reference.
        if r.is_some() {
            if let Some(crate::process::FdEntry::Pipe { pipe_id, is_read_end }) =
                t.get(new as i32).copied()
            {
                if is_read_end {
                    crate::ipc::dup_read_end(pipe_id as usize);
                } else {
                    crate::ipc::dup_write_end(pipe_id as usize);
                }
            }
        }
        r
    })
    .flatten()
    .map(|n| n as u64)
    .unwrap_or(u64::MAX)
}

// --- Process management handlers ---

/// SYS_SPAWN(path_ptr, path_len, max_tier) → PID or u64::MAX on error
///
/// Loads an ELF binary from ramfs and spawns it as a Ring 3 process.
/// Caller-supplied argv/envp blob layout, pointed to by arg3 when
/// non-zero. argv_blob / envp_blob is `[count: u32][len1: u32][bytes1]
/// [len2: u32][bytes2]...` — each item is a u32 length prefix followed
/// by raw bytes (NOT null-terminated; the kernel adds null terminators
/// when writing to the user stack).
#[repr(C)]
pub struct SpawnArgs {
    pub argv_blob_ptr: u64,
    pub argv_blob_len: u32,
    pub envp_blob_ptr: u64,
    pub envp_blob_len: u32,
}

/// Maximum total bytes accepted in argv_blob OR envp_blob. Bounded
/// to keep the kernel-side scratch buffers small.
const MAX_BLOB_BYTES: usize = 1024;
/// Maximum items (per side). Matches the platform impl's cap.
const MAX_BLOB_ITEMS: usize = 32;

/// Parse a `[count: u32][len: u32][bytes]...` blob into a slice of
/// byte-slice references. Refs point into the caller-supplied blob,
/// valid for the lifetime of the borrow.
fn parse_argv_blob<'a>(blob: &'a [u8], items_out: &mut [&'a [u8]]) -> Option<usize> {
    if blob.len() < 4 { return None; }
    let count = u32::from_le_bytes(blob[0..4].try_into().unwrap()) as usize;
    if count > items_out.len() { return None; }
    let mut cursor = 4usize;
    for i in 0..count {
        if cursor + 4 > blob.len() { return None; }
        let len = u32::from_le_bytes(blob[cursor..cursor+4].try_into().unwrap()) as usize;
        cursor += 4;
        if cursor + len > blob.len() { return None; }
        items_out[i] = &blob[cursor..cursor+len];
        cursor += len;
    }
    Some(count)
}

/// Parse the optional `SpawnArgs` blob (arg3) into argv/envp item slices.
/// Returns `(argc, envc)`, `Some((0,0))` when `spawn_args_ptr == 0` (legacy
/// no-args callers), or `None` on a malformed/oversized blob.
///
/// Copy-in is deliberate: spawn is a complex path and eventually writes argv
/// onto the new user stack, so it must not keep borrowing caller memory after
/// the initial validation/read step (2026-07-17 review, P2 TOCTOU class).
/// The item refs point into the kernel scratch buffers supplied by the caller.
fn parse_spawn_args<'a>(
    spawn_args_ptr: u64,
    argv_blob_scratch: &'a mut [u8; MAX_BLOB_BYTES],
    envp_blob_scratch: &'a mut [u8; MAX_BLOB_BYTES],
    argv_items: &mut [&'a [u8]],
    envp_items: &mut [&'a [u8]],
) -> Option<(usize, usize)> {
    if spawn_args_ptr == 0 {
        return Some((0, 0));
    }

    // Copy the repr(C) SpawnArgs bytes, then decode the field offsets
    // explicitly (u64, u32, padding, u64, u32). This avoids creating a raw
    // reference to caller memory.
    let sa_bytes = unsafe {
        read_caller_slice(spawn_args_ptr, core::mem::size_of::<SpawnArgs>() as u64)?
    };
    let argv_blob_ptr = u64::from_le_bytes(sa_bytes[0..8].try_into().ok()?);
    let argv_blob_len = u32::from_le_bytes(sa_bytes[8..12].try_into().ok()?);
    let envp_blob_ptr = u64::from_le_bytes(sa_bytes[16..24].try_into().ok()?);
    let envp_blob_len = u32::from_le_bytes(sa_bytes[24..28].try_into().ok()?);

    if argv_blob_len as usize > MAX_BLOB_BYTES || envp_blob_len as usize > MAX_BLOB_BYTES {
        return None;
    }
    let mut argc = 0usize;
    let mut envc = 0usize;
    if argv_blob_ptr != 0 && argv_blob_len > 0 {
        let len = argv_blob_len as usize;
        let blob = unsafe { read_caller_slice(argv_blob_ptr, argv_blob_len as u64)? };
        argv_blob_scratch[..len].copy_from_slice(blob);
        argc = parse_argv_blob(&argv_blob_scratch[..len], argv_items)?;
    }
    if envp_blob_ptr != 0 && envp_blob_len > 0 {
        let len = envp_blob_len as usize;
        let blob = unsafe { read_caller_slice(envp_blob_ptr, envp_blob_len as u64)? };
        envp_blob_scratch[..len].copy_from_slice(blob);
        envc = parse_argv_blob(&envp_blob_scratch[..len], envp_items)?;
    }
    Some((argc, envc))
}

/// Spawn an executable stored at a path-namespace path (NOT ramfs `/bin`) —
/// the "install anywhere / run anywhere" path. Resolves the path, tier-checks
/// the caller against the object (the LLM at tier 0 can't run a higher-tier
/// binary, just as it can't read one), reads its ELF bytes from the object's
/// heap content, and spawns. Returns the PID or `u64::MAX`. Task name is the
/// generic `"user-app"` for now (the scheduler wants a `&'static str`;
/// per-path names are a follow-up).
fn spawn_namespace_elf(path: &str, spawn_tier: u8, spawn_args_ptr: u64) -> u64 {
    use crate::fs::paths::Namespace;

    let suid = match Namespace::resolve(path) {
        Ok(s) => s,
        Err(_) => {
            crate::platform::log("[syscall] spawn: namespace path not found: ");
            crate::platform::log(path);
            crate::platform::log("\n");
            return u64::MAX;
        }
    };

    // Hold the registry borrow for the rest of the call (it's &'static mut and
    // spawn doesn't touch the registry, so the ELF byte borrow stays valid).
    let registry = crate::semantic::registry::global_registry();
    let obj = match registry.get(&suid) {
        Some(o) => o,
        None => return u64::MAX,
    };
    // Security: caller must be cleared for the executable's tier — mirrors the
    // SYS_OPEN read check, so a sandboxed (tier-0) agent can't run secrets.
    let caller_tier = crate::scheduler::current_task_max_tier();
    if caller_tier < (obj.tier as u8) {
        crate::platform::log("[syscall] spawn: tier denied for namespace exec: ");
        crate::platform::log(path);
        crate::platform::log("\n");
        return u64::MAX;
    }
    let elf_data: &[u8] = match obj.content.as_bytes() {
        Some(b) => b,
        None => return u64::MAX,
    };
    // Deny-by-default execution privilege. A namespace executable is a
    // runtime/agent-created tool (e.g. one the on-device compiler just emitted).
    // It runs at **tier 0 (Public — cannot read Sensitive/Secret objects like
    // credentials or keys)** no matter who launches it, UNLESS it has been
    // vouched (SYS_VOUCH, interactive-console only) AND its bytes are unchanged
    // since the vouch (SHA-256 recheck — closes bait-and-switch). This is the
    // capability fence on agent-authored code: "any created tool drops to tier 0
    // unless it can be shown to be safe." Baked-in /bin programs don't reach this
    // path — they're shipped-trusted and run at the launcher's clearance.
    let exec_cap: u8 = match vouch_lookup(&suid) {
        // Constant-time compare: the recheck binds the grant to the exact
        // vouched bytes — don't give a byte-prefix timing oracle on the
        // stored hash (2026-07-17 review, medium #4.1).
        Some((tier, hash)) if crate::crypto::ct_eq(&crate::crypto::sha256::hash(elf_data), &hash) => tier,
        _ => 0, // unvouched, or bytes changed since vouch → tier 0
    };
    // Session vouch (SYS_VOUCH_SESSION): while the human holds a live session
    // the ceiling applies to EVERY namespace tool, not just individually
    // vouched ones. Per-path vouch still wins when it's higher (it binds the
    // exact bytes; the session is the broader but weaker grant).
    let exec_cap = exec_cap.max(session_ceiling());
    let fenced_tier = spawn_tier.min(exec_cap);
    if fenced_tier != spawn_tier {
        if exec_cap == 0 {
            crate::platform::log("[syscall] spawn: unvouched tool fenced to tier 0: ");
            crate::platform::log(path);
            crate::platform::log("\n");
        } else {
            crate::platform::log("[syscall] spawn: vouched tool capped below caller tier: ");
            crate::platform::log(path);
            crate::platform::log("\n");
        }
    }
    spawn_elf_bytes(elf_data, "user-app", fenced_tier, spawn_args_ptr)
}

// ============================================================================
// Vouch mechanism (v1) — how an agent-authored tool earns the right to run with
// privilege. Design: docs/VOUCH_MECHANISM_DESIGN_2026-06-15.md.
// ============================================================================

use core::sync::atomic::{AtomicUsize, Ordering as VouchOrdering};

/// The scheduler task index allowed to call SYS_VOUCH — the **human-driven
/// interactive shell**, set by `interactive_session` after it spawns sem-sh.
/// `usize::MAX` = none (deny). The LLM agent runs as a different task (and its
/// own tools are tier-0), so it can never match → it cannot vouch its own code.
static VOUCH_AUTHORITY_TASK: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Mark `task` as the sole vouch authority (the human console). Kernel-only.
pub fn set_vouch_authority(task: usize) {
    VOUCH_AUTHORITY_TASK.store(task, VouchOrdering::SeqCst);
}

fn is_vouch_authority() -> bool {
    let a = VOUCH_AUTHORITY_TASK.load(VouchOrdering::SeqCst);
    a != usize::MAX && crate::scheduler::current_task_index() == a
}

/// One ephemeral vouch grant. Lives in RAM only — **vouches clear on reboot**,
/// so deny-by-default re-asserts every boot and trust is re-granted deliberately.
#[derive(Clone, Copy)]
struct VouchEntry {
    suid: crate::semantic::suid::SUID,
    tier: u8,
    hash: [u8; 32],
    used: bool,
    /// The path vouched, kept for the `vouches` audit list (cosmetic).
    path: [u8; 64],
    path_len: u8,
}
const MAX_VOUCHES: usize = 32;
/// The vouch grant table, behind the kernel mutex (was `static mut` under
/// the false "syscalls are serialized" assumption — 2026-07-17 review, P1).
static VOUCH_TABLE: crate::sync::Mutex<[VouchEntry; MAX_VOUCHES]> =
    crate::sync::Mutex::new([VouchEntry {
        suid: crate::semantic::suid::SUID { high: 0, low: 0 },
        tier: 0,
        hash: [0; 32],
        used: false,
        path: [0; 64],
        path_len: 0,
    }; MAX_VOUCHES]);

/// Look up a vouch grant for `suid` → (granted_tier, vouched-bytes hash).
fn vouch_lookup(suid: &crate::semantic::suid::SUID) -> Option<(u8, [u8; 32])> {
    let table = VOUCH_TABLE.lock();
    table.iter().find(|e| e.used && e.suid == *suid).map(|e| (e.tier, e.hash))
}

/// SYS_VOUCH(path_ptr, path_len, grant_tier) -> 1 ok / 0 err. Mark a namespace
/// tool safe to run at `grant_tier`. Gate (the agent CANNOT pass it):
///   1. caller is the interactive console (the vouch authority task), AND
///   2. caller holds clearance >= grant_tier (can't grant more than you have).
/// Binds the grant to SHA-256 of the ELF (spawn rechecks → anti bait-and-switch).
fn handle_vouch(path_ptr: u64, path_len: u64, grant_tier: u64) -> u64 {
    if !is_vouch_authority() {
        crate::platform::log("[vouch] DENIED: caller is not the interactive console\n");
        return 0;
    }
    let grant = grant_tier as u8;
    if grant > 3 || grant > crate::scheduler::current_task_max_tier() {
        crate::platform::log("[vouch] DENIED: grant exceeds caller clearance\n");
        return 0;
    }
    let path = match unsafe { read_caller_str(path_ptr, path_len) } {
        Some(s) => s,
        None => return 0,
    };
    let suid = match crate::fs::paths::Namespace::resolve(path) { Ok(s) => s, Err(_) => return 0 };
    let registry = crate::semantic::registry::global_registry();
    let hash = match registry.get(&suid).and_then(|o| o.content.as_bytes()) {
        Some(elf) => crate::crypto::sha256::hash(elf),
        None => return 0,
    };
    // Stash the path (truncated to 64 bytes) for the audit list.
    let mut pbuf = [0u8; 64];
    let plen = path.len().min(64);
    pbuf[..plen].copy_from_slice(&path.as_bytes()[..plen]);
    let mut table = VOUCH_TABLE.lock();
    // Update an existing grant for this object, else take a free slot.
    if let Some(e) = table.iter_mut().find(|e| e.used && e.suid == suid) {
        e.tier = grant;
        e.hash = hash;
        e.path = pbuf;
        e.path_len = plen as u8;
    } else if let Some(e) = table.iter_mut().find(|e| !e.used) {
        *e = VouchEntry { suid, tier: grant, hash, used: true, path: pbuf, path_len: plen as u8 };
    } else {
        crate::platform::log("[vouch] table full\n");
        return 0;
    }
    crate::platform::log("[vouch] vouched: ");
    crate::platform::log(path);
    crate::platform::log("\n");
    1
}

/// SYS_VOUCHES() -> count. Print the current vouch grants (path + tier) to the
/// console for audit. Any task may list (read-only, no secrets exposed).
fn handle_vouches() -> u64 {
    let table = VOUCH_TABLE.lock();
    let mut n = 0u64;
    // Session ceiling first — it applies to every namespace tool.
    let v = handle_get_vouch();
    if v == 0 {
        crate::platform::log("[vouches] no live session vouch (namespace exec ceiling: 0)\n");
    } else {
        crate::platform::log("[vouches] SESSION vouch live: tier ceiling ");
        crate::platform::log(match (v >> 32) as u8 {
            1 => "1 (internal)  ",
            2 => "2 (sensitive) ",
            _ => "3 (secret)    ",
        });
        crate::platform::log("\n");
    }
    crate::platform::log("[vouches] active per-path grants (reset on reboot):\n");
    for e in table.iter() {
        if !e.used {
            continue;
        }
        n += 1;
        let path = core::str::from_utf8(&e.path[..e.path_len as usize]).unwrap_or("<?>");
        crate::platform::log("  tier ");
        crate::platform::log(match e.tier {
            0 => "0 (sandboxed) ",
            1 => "1 (internal)  ",
            2 => "2 (sensitive) ",
            _ => "3 (secret)    ",
        });
        crate::platform::log(path);
        crate::platform::log("\n");
    }
    if n == 0 {
        crate::platform::log("  (none)\n");
    }
    n
}

// ============================================================================
// Session vouch (self-dev loop, docs/semos_selfdev_loop_plan.md §3.1)
//
// The per-path vouch above binds ONE tool's exact bytes to a tier. The
// session vouch is the human opening the door for a whole working session:
// for `duration`, every namespace (agent-authored) executable may run at up
// to `tier` without an individual vouch. The agent still can't grant it —
// the syscall is gated to the same interactive-console authority as
// SYS_VOUCH, and a password (set on first use, SHA-256 + constant-time
// compare) must accompany every grant so a borrowed/idle console can't be
// driven by injected input alone.
//
// Deliberate deviations from the plan text, with reasons:
//   * State is a kernel global, not a PCB field — there is exactly one
//     interactive console session, and the ceiling is a property of THAT
//     session, not of any one process.
//   * Password echo: SemOS has no echo-off in the TTY line discipline yet,
//     so the password is visible on the local screen while typed (shoulder
//     -surf risk only; the agent never sees the console). Filed for follow-up.
// ============================================================================

/// Live session-vouch state. In RAM only: reboot clears it (deny-by-default
/// re-asserts every boot), and expiry is checked on every read, so an expired
/// session downgrades itself with no timer task.
struct VouchSession {
    /// SHA-256 of the vouch password; `pw_set` distinguishes "no password
    /// yet" from an all-zero hash.
    pw_hash: [u8; 32],
    pw_set: bool,
    active: bool,
    tier: u8,
    /// platform::ticks() value at which the session ends.
    expiry_ticks: u64,
}

static VOUCH_SESSION: crate::sync::Mutex<VouchSession> =
    crate::sync::Mutex::new(VouchSession {
        pw_hash: [0; 32],
        pw_set: false,
        active: false,
        tier: 0,
        expiry_ticks: 0,
    });

/// The current session ceiling: the granted tier while a session is live,
/// 0 otherwise. Called from `spawn_namespace_elf` on every agent-tool spawn.
/// Lazily clears (and audit-logs) an expired session.
pub fn session_ceiling() -> u8 {
    let mut s = VOUCH_SESSION.lock();
    if !s.active {
        return 0;
    }
    if crate::platform::ticks() >= s.expiry_ticks {
        s.active = false;
        s.tier = 0;
        crate::platform::log("[vouch] session expired — namespace exec ceiling back to 0\n");
        return 0;
    }
    s.tier
}

/// SYS_VOUCH_SESSION(tier, duration_secs, pw_ptr, pw_len) -> 1 ok / 0 err.
///
/// Grant (or with duration_secs==0, revoke) the session ceiling. Gates:
///   1. caller is the interactive console (the agent cannot reach this), AND
///   2. tier <= the console's own clearance (can't grant more than you have), AND
///   3. password: first call SETS it; later calls must match (ct_eq).
fn handle_vouch_session(tier: u64, duration_secs: u64, pw_ptr: u64, pw_len: u64) -> u64 {
    if !is_vouch_authority() {
        crate::platform::log("[vouch] session DENIED: caller is not the interactive console\n");
        return 0;
    }
    if tier > 3 || (tier as u8) > crate::scheduler::current_task_max_tier() {
        crate::platform::log("[vouch] session DENIED: tier exceeds caller clearance\n");
        return 0;
    }
    let pw = match unsafe { read_caller_str(pw_ptr, pw_len) } {
        Some(s) if !s.is_empty() && s.len() <= 64 => s,
        _ => {
            crate::platform::log("[vouch] session DENIED: bad password (empty or > 64 chars)\n");
            return 0;
        }
    };
    let pw_hash = crate::crypto::sha256::hash(pw.as_bytes());
    let mut s = VOUCH_SESSION.lock();
    if !s.pw_set {
        s.pw_hash = pw_hash;
        s.pw_set = true;
        crate::platform::log("[vouch] session password set (this console, this boot only)\n");
    } else if !crate::crypto::ct_eq(&s.pw_hash, &pw_hash) {
        crate::platform::log("[vouch] session DENIED: wrong password\n");
        return 0;
    }
    if duration_secs == 0 {
        s.active = false;
        s.tier = 0;
        crate::platform::log("[vouch] session revoked — namespace exec ceiling back to 0\n");
        return 1;
    }
    let hz = crate::scheduler::SCHEDULER_TICK_HZ;
    let now = crate::platform::ticks();
    s.active = true;
    s.tier = tier as u8;
    s.expiry_ticks = now.saturating_add(duration_secs.saturating_mul(hz));
    crate::platform::log("[vouch] SESSION tier=");
    crate::platform::log(match tier {
        0 => "0 (public)    ",
        1 => "1 (internal)  ",
        2 => "2 (sensitive) ",
        _ => "3 (secret)    ",
    });
    crate::platform::log(" granted for ");
    // u64 → decimal without alloc (audit line only).
    let mut buf = [0u8; 20];
    let mut v = duration_secs;
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    crate::platform::log(unsafe { core::str::from_utf8_unchecked(&buf[i..]) });
    crate::platform::log("s — namespace exec ceiling raised\n");
    1
}

/// SYS_GET_VOUCH() -> (tier << 32) | remaining_secs, or 0 when no session is
/// live. Read-only; any task (the agent included) may query its ceiling so it
/// can fail fast instead of attempting a spawn that would be fenced.
fn handle_get_vouch() -> u64 {
    let tier = session_ceiling(); // also lazily expires
    if tier == 0 {
        return 0;
    }
    let s = VOUCH_SESSION.lock();
    let now = crate::platform::ticks();
    let remaining_secs = s.expiry_ticks.saturating_sub(now) / crate::scheduler::SCHEDULER_TICK_HZ;
    ((tier as u64) << 32) | remaining_secs
}

fn handle_spawn(path_ptr: u64, path_len: u64, max_tier: u64, spawn_args_ptr: u64) -> u64 {
    // Validate tier access — can't spawn at a higher tier than yourself
    let caller_tier = crate::scheduler::current_task_max_tier();
    let spawn_tier = (max_tier as u8).min(caller_tier);

    let path = match unsafe { read_caller_str(path_ptr, path_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };

    // Resolve `path` to ELF bytes and spawn. Three routes, all funnelled through
    // `spawn_elf_bytes` — no hardcoded program table, so any ELF dropped into
    // ramfs OR the path namespace is runnable by name. This is the keystone for
    // agent-authored command-modules: the agent compiles `/apps/greet` on-device,
    // and `greet` at the shell Just Works (the shell searches $PATH=/bin:/apps).
    if let Some(stripped) = path.strip_prefix("/bin/") {
        // /bin/<name> → ramfs "<name>.elf", then "<name>".
        let fs = match crate::fs::ramfs::get_fs() {
            Some(fs) => fs,
            None => return u64::MAX,
        };
        let name = intern_prog_name(stripped);
        // Compose "<stripped>.elf" without alloc.
        let mut composed = [0u8; 64];
        let dot_elf = b".elf";
        if stripped.len() + dot_elf.len() <= composed.len() {
            composed[..stripped.len()].copy_from_slice(stripped.as_bytes());
            composed[stripped.len()..stripped.len() + dot_elf.len()].copy_from_slice(dot_elf);
            let with_elf = unsafe {
                core::str::from_utf8_unchecked(&composed[..stripped.len() + dot_elf.len()])
            };
            if let Some(file) = fs.find(with_elf) {
                return spawn_elf_bytes(file.data(), name, spawn_tier, spawn_args_ptr);
            }
        }
        if let Some(file) = fs.find(stripped) {
            return spawn_elf_bytes(file.data(), name, spawn_tier, spawn_args_ptr);
        }
        crate::platform::log("[syscall] spawn: /bin/ path not found in ramfs: ");
        crate::platform::log(stripped);
        crate::platform::log("\n");
        return u64::MAX;
    }

    if path.starts_with('/') {
        // Absolute path → path-namespace executable (the agent-authored case:
        // resolve + spawn directly from the registry object's bytes, tier-gated).
        return spawn_namespace_elf(path, spawn_tier, spawn_args_ptr);
    }

    // Legacy: bare ramfs name (preserves existing callers).
    let fs = match crate::fs::ramfs::get_fs() {
        Some(fs) => fs,
        None => return u64::MAX,
    };
    let file = match fs.find(path) {
        Some(f) => f,
        None => {
            crate::platform::log("[syscall] spawn: file not found in ramfs: ");
            crate::platform::log(path);
            crate::platform::log("\n");
            return u64::MAX;
        }
    };
    spawn_elf_bytes(file.data(), intern_prog_name(path), spawn_tier, spawn_args_ptr)
}

/// Map a program base name to a `'static` process name for the scheduler/logs.
/// COSMETIC ONLY — never a gate. Unknown names get the generic "user", so any
/// ELF (incl. one the agent just compiled) is spawnable; the old hardcoded
/// table used to *reject* unknown names here, which blocked agent-authored
/// command-modules.
fn intern_prog_name(name: &str) -> &'static str {
    // Strip a trailing ".elf" for the cosmetic comparison.
    let base = name.strip_suffix(".elf").unwrap_or(name);
    match base {
        "init" => "init",
        "shell" => "shell",
        "sem-sh" => "sem-sh",
        "semos-rustc" => "semos-rustc",
        _ => "user",
    }
}

/// Parse the SpawnArgs blob (argv/envp) and spawn `elf_data` Ring-3 at
/// `spawn_tier`. The single spawn path shared by every route (/bin, namespace,
/// legacy ramfs name). `arg3==0` → empty argv/envp (old API).
fn spawn_elf_bytes(elf_data: &[u8], name: &'static str, spawn_tier: u8, spawn_args_ptr: u64) -> u64 {
    let mut argv_blob_scratch = [0u8; MAX_BLOB_BYTES];
    let mut envp_blob_scratch = [0u8; MAX_BLOB_BYTES];
    let mut argv_items: [&[u8]; MAX_BLOB_ITEMS] = [&[]; MAX_BLOB_ITEMS];
    let mut envp_items: [&[u8]; MAX_BLOB_ITEMS] = [&[]; MAX_BLOB_ITEMS];
    let (argc, envc) = match parse_spawn_args(
        spawn_args_ptr,
        &mut argv_blob_scratch,
        &mut envp_blob_scratch,
        &mut argv_items,
        &mut envp_items,
    ) {
        Some(t) => t,
        None => {
            crate::platform::log("[syscall] spawn: argv/envp blob malformed\n");
            return u64::MAX;
        }
    };
    match crate::process::spawn_from_elf_with_args(
        name, elf_data, spawn_tier,
        &argv_items[..argc],
        &envp_items[..envc],
    ) {
        Some(pid) => pid.0 as u64,
        None => {
            crate::platform::log("[syscall] spawn: failed to load ELF\n");
            u64::MAX
        }
    }
}

/// SYS_WAIT(pid) → exit code or u64::MAX on error
///
/// Waits for a child process to exit and returns its exit code.
/// If pid == 0, waits for any child.
///
/// Ring-3 children (the `std::process::Command` case) never reach the
/// PROCESS_TABLE Zombie state — see the `handle_exit` note: current_pid()
/// isn't refreshed on context switch, so exit can't mark the right
/// Process as Zombie. The exit code DOES land on the scheduler slot
/// (`set_current_exit_code`). So when the child has a scheduler `task_id`
/// we block on that slot via the same `join_block` path SYS_THREAD_JOIN
/// uses, which is reliable for Ring-3 children. We fall back to the
/// PROCESS_TABLE `wait()` path only when there's no task_id (the legacy
/// kernel-parent case that already worked).
fn handle_wait(pid: u64) -> u64 {
    use crate::scheduler::TaskState;

    crate::platform::log("[wait] pid=");
    crate::platform::log_num(pid);
    crate::platform::log(" caller_slot=");
    crate::platform::log_num(crate::scheduler::current_task_index() as u64);
    crate::platform::log("\n");

    if pid == 0 {
        // Wait for any child.
        crate::platform::log("[wait] waitpid_any path\n");
        return match crate::process::waitpid_any() {
            Some((_child_pid, status)) => status.code as u64,
            None => u64::MAX,
        };
    }

    let child_pid = crate::process::ProcessId(pid as u32);

    // Preferred path: block on the child's scheduler slot.
    if let Some(slot) = crate::process::get(child_pid).and_then(|p| p.task_id) {
        crate::platform::log("[wait] child slot=");
        crate::platform::log_num(slot as u64);
        crate::platform::log(" state=");
        crate::platform::log_num(crate::scheduler::task_state(slot) as u64);
        crate::platform::log("\n");
        if slot < crate::scheduler::MAX_TASKS
            && slot != crate::scheduler::current_task_index()
        {
            // Fast path: already exited.
            if crate::scheduler::task_state(slot) == TaskState::Exited {
                crate::platform::log("[wait] fast path\n");
                return crate::scheduler::task_exit_code(slot);
            }
            // Empty slot → nothing to wait for (stale pid).
            if crate::scheduler::task_state(slot) != TaskState::Empty {
                crate::platform::log("[wait] join_block path\n");
                crate::scheduler::join_block(slot);
                crate::platform::log("[wait] about to schedule\n");
                crate::platform::schedule();
                crate::platform::log("[wait] schedule returned, state=");
                crate::platform::log_num(crate::scheduler::task_state(slot) as u64);
                crate::platform::log(" code=");
                crate::platform::log_num(crate::scheduler::task_exit_code(slot));
                crate::platform::log("\n");
                return crate::scheduler::task_exit_code(slot);
            }
            crate::platform::log("[wait] child slot Empty, fallback\n");
        } else {
            crate::platform::log("[wait] slot invalid/self, fallback\n");
        }
    } else {
        crate::platform::log("[wait] no task_id, fallback\n");
    }

    // Legacy fallback: PROCESS_TABLE-based wait (kernel-parent case).
    crate::platform::log("[wait] legacy wait\n");
    match crate::process::wait(child_pid) {
        Some(status) => {
            crate::platform::log("[wait] legacy returned code=");
            crate::platform::log_num(status.code as u64);
            crate::platform::log("\n");
            status.code as u64
        }
        None => {
            crate::platform::log("[wait] legacy returned None\n");
            u64::MAX
        }
    }
}

/// SYS_KILL(pid) → 0 on success, u64::MAX on error
///
/// Terminates a process. The caller must be the parent or have Secret tier.
fn handle_kill(pid: u64) -> u64 {
    let target_pid = crate::process::ProcessId(pid as u32);
    let caller_pid = crate::process::current_pid();
    let caller_tier = crate::scheduler::current_task_max_tier();

    // Check permission: must be parent or have Secret tier
    let allowed = if let Some(target) = crate::process::get(target_pid) {
        target.parent == Some(caller_pid) || caller_tier >= 3
    } else {
        false
    };

    if !allowed {
        crate::platform::log("[syscall] kill: permission denied\n");
        return u64::MAX;
    }

    // Mark the process as zombie
    unsafe {
        if let Some(proc) = crate::process::get_mut(target_pid) {
            proc.exit(crate::process::ExitStatus::killed(9));
            // Also mark the scheduler task as exited
            if let Some(task_id) = proc.task_id {
                let tasks = &raw mut crate::scheduler::TASKS;
                (*tasks)[task_id].state = crate::scheduler::TaskState::Exited;
            }
            0
        } else {
            u64::MAX
        }
    }
}

// ============================================================================
// Semantic Object Syscalls (20-29)
// ============================================================================

/// Helper: convert u8 tier to SecurityTier
fn tier_from_u8(t: u8) -> crate::memory::SecurityTier {
    match t {
        0 => crate::memory::SecurityTier::Public,
        1 => crate::memory::SecurityTier::Internal,
        2 => crate::memory::SecurityTier::Sensitive,
        _ => crate::memory::SecurityTier::Secret,
    }
}

/// SYS_SEM_CREATE(suid_high, suid_low, tier, content_ptr | content_len<<32)
/// Creates a semantic object. Returns 0 on success, u64::MAX on error.
fn handle_sem_create(suid_high: u64, suid_low: u64, tier: u64, content_info: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let obj_tier = (tier as u8).min(max_tier);

    let suid = crate::semantic::SUID::new(suid_high, suid_low);
    let security_tier = tier_from_u8(obj_tier);
    let owner = crate::scheduler::current_user_id();

    let content_ptr = content_info & 0xFFFF_FFFF;
    let content_len = (content_info >> 32) as usize;

    let obj = if content_ptr != 0 && content_len > 0 && content_len <= 1024 {
        let data = match unsafe { read_caller_slice(content_ptr, content_len as u64) } {
            Some(d) => d,
            None => return u64::MAX,
        };
        match crate::semantic::SemanticObject::with_content(suid, security_tier, owner, data) {
            Some(o) => o,
            None => return u64::MAX,
        }
    } else {
        crate::semantic::SemanticObject::new(suid, security_tier, owner)
    };

    {
        let mut registry = crate::semantic::registry::global_registry();
        if registry.insert(obj) { 0 } else { u64::MAX }
    }
}

/// SYS_SEM_READ(suid_high, suid_low, out_ptr) → content length or u64::MAX
/// Reads the content of a semantic object into the buffer at out_ptr (max 1024 bytes).
fn handle_sem_read(suid_high: u64, suid_low: u64, out_ptr: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let suid = crate::semantic::SUID::new(suid_high, suid_low);

    unsafe {
        let registry = crate::semantic::registry::global_registry();
        match registry.get(&suid) {
            Some(obj) => {
                if (obj.tier as u8) > max_tier {
                    crate::platform::log("[syscall] sem_read: tier access denied\n");
                    return u64::MAX;
                }
                match obj.content.as_bytes() {
                    Some(data) => {
                        if out_ptr != 0 && !data.is_empty() {
                            if !write_to_caller(out_ptr, data) { return u64::MAX; }
                        }
                        data.len() as u64
                    }
                    None => 0, // Empty content
                }
            }
            None => u64::MAX,
        }
    }
}

/// SYS_SEM_WRITE(suid_high, suid_low, data_ptr, data_len) → 0 or u64::MAX
/// Updates the content of an existing semantic object.
fn handle_sem_write(suid_high: u64, suid_low: u64, data_ptr: u64, data_len: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let suid = crate::semantic::SUID::new(suid_high, suid_low);
    let len = data_len as usize;
    if len > 1024 { return u64::MAX; }

    unsafe {
        let mut registry = crate::semantic::registry::global_registry();
        match registry.get_mut(&suid) {
            Some(obj) => {
                if (obj.tier as u8) > max_tier {
                    return u64::MAX;
                }
                if obj.flags.is_immutable() {
                    return u64::MAX;
                }
                // A zero-length write clears the content; the slice helpers
                // reject zero-length ranges, so special-case it.
                let data: &[u8] = if len == 0 {
                    &[]
                } else {
                    match read_caller_slice(data_ptr, data_len) {
                        Some(d) => d,
                        None => return u64::MAX,
                    }
                };
                // task #44: from_bytes promotes >256 B writes to
                // heap-Allocated transparently.
                match crate::semantic::ObjectContent::from_bytes(data) {
                    Some(content) => {
                        obj.content = content;
                        0
                    }
                    None => u64::MAX,
                }
            }
            None => u64::MAX,
        }
    }
}

/// SYS_SEM_DELETE(suid_high, suid_low) → 0 or u64::MAX
fn handle_sem_delete(suid_high: u64, suid_low: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let suid = crate::semantic::SUID::new(suid_high, suid_low);

    {
        let mut registry = crate::semantic::registry::global_registry();
        // Check tier before removing
        if let Some(obj) = registry.get(&suid) {
            if (obj.tier as u8) > max_tier {
                return u64::MAX;
            }
        } else {
            return u64::MAX;
        }
        match registry.remove(&suid) {
            Some(_) => 0,
            None => u64::MAX,
        }
    }
}

/// SYS_SEM_LINK(src_high, src_low, dst_high, dst_low) → 0 or u64::MAX
/// Links two semantic objects (src → dst with References relation).
fn handle_sem_link(src_high: u64, src_low: u64, dst_high: u64, dst_low: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let src_suid = crate::semantic::SUID::new(src_high, src_low);
    let dst_suid = crate::semantic::SUID::new(dst_high, dst_low);

    {
        let mut registry = crate::semantic::registry::global_registry();
        match registry.get_mut(&src_suid) {
            Some(obj) => {
                if (obj.tier as u8) > max_tier {
                    return u64::MAX;
                }
                if obj.add_link(dst_suid, crate::semantic::object::RelationType::References) {
                    0
                } else {
                    u64::MAX
                }
            }
            None => u64::MAX,
        }
    }
}

/// SYS_SEM_QUERY(tier_filter, out_ptr, max_results) → count or u64::MAX
/// Lists semantic objects at or below the given tier. Writes SUID pairs to out_ptr.
fn handle_sem_query(tier_filter: u64, out_ptr: u64, max_results: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let filter = (tier_filter as u8).min(max_tier);
    let limit = (max_results as usize).min(64);

    unsafe {
        let registry = crate::semantic::registry::global_registry();
        let mut count = 0usize;
        let mut written = 0usize;
        let mut out = [0u8; 64 * 16];

        for obj in registry.iter() {
            if (obj.tier as u8) <= filter {
                if written < limit {
                    let o = written * 16;
                    out[o..o + 8].copy_from_slice(&obj.suid.high.to_le_bytes());
                    out[o + 8..o + 16].copy_from_slice(&obj.suid.low.to_le_bytes());
                    written += 1;
                }
                count += 1;
            }
        }
        if written > 0 && !write_to_caller(out_ptr, &out[..written * 16]) {
            return u64::MAX;
        }
        count as u64
    }
}

/// SYS_SEM_SEARCH(query_ptr, query_dims, max_results, out_ptr) → count
/// Vector similarity search. query_ptr points to f32 array, results written to out_ptr.
fn handle_sem_search(query_ptr: u64, query_dims: u64, max_results: u64, out_ptr: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let dims = query_dims as usize;
    let limit = (max_results as usize).min(16);

    if dims == 0 || dims > 384 { return u64::MAX; }

    let query_bytes = match unsafe { read_caller_slice(query_ptr, (dims * 4) as u64) } {
        Some(b) => b,
        None => return u64::MAX,
    };
    // Copy the caller's byte representation into aligned f32 storage. This
    // avoids both TOCTOU on the query vector and the old unaligned
    // `u8* as *const f32` cast.
    let mut query_storage = [0f32; 384];
    for i in 0..dims {
        let o = i * 4;
        let bits = u32::from_le_bytes([
            query_bytes[o],
            query_bytes[o + 1],
            query_bytes[o + 2],
            query_bytes[o + 3],
        ]);
        query_storage[i] = f32::from_bits(bits);
    }
    let query = &query_storage[..dims];

    unsafe {
        let search = crate::semantic::search::global_search();
        let mut results = [crate::semantic::SearchResult::new(0, 0, 0.0); 16];
        match search.find_similar(query, max_tier, limit, &mut results[..limit]) {
            Ok(count) => {
                // Write results: each result is (suid_high, suid_low, score_bits).
                let mut out = [0u8; 16 * 24];
                for i in 0..count {
                    let o = i * 24;
                    out[o..o + 8].copy_from_slice(&results[i].suid_high.to_le_bytes());
                    out[o + 8..o + 16].copy_from_slice(&results[i].suid_low.to_le_bytes());
                    out[o + 16..o + 24]
                        .copy_from_slice(&(results[i].score.to_bits() as u64).to_le_bytes());
                }
                if count > 0 && !write_to_caller(out_ptr, &out[..count * 24]) {
                    return u64::MAX;
                }
                count as u64
            }
            Err(_) => 0,
        }
    }
}

/// SYS_SEM_META(suid_high, suid_low, out_ptr) → 0 or u64::MAX
/// Reads metadata about a semantic object: [tier, owner, content_len, link_count, flags].
fn handle_sem_meta(suid_high: u64, suid_low: u64, out_ptr: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let suid = crate::semantic::SUID::new(suid_high, suid_low);

    unsafe {
        let registry = crate::semantic::registry::global_registry();
        match registry.get(&suid) {
            Some(obj) => {
                if (obj.tier as u8) > max_tier {
                    return u64::MAX;
                }
                let link_count = obj.get_links().iter().filter(|l| l.is_some()).count();
                let mut out = [0u8; 40];
                out[0..8].copy_from_slice(&(obj.tier as u64).to_le_bytes());
                out[8..16].copy_from_slice(&(obj.owner as u64).to_le_bytes());
                out[16..24].copy_from_slice(&(obj.content.len() as u64).to_le_bytes());
                out[24..32].copy_from_slice(&(link_count as u64).to_le_bytes());
                out[32..40].copy_from_slice(&(obj.flags.as_u32() as u64).to_le_bytes());
                if write_to_caller(out_ptr, &out) { 0 } else { u64::MAX }
            }
            None => u64::MAX,
        }
    }
}

// ============================================================================
// LLM Service Syscalls (50-59)
// ============================================================================

/// SYS_LLM_QUERY(prompt_ptr, prompt_len, out_ptr) → response length or u64::MAX
/// Submits a prompt to the LLM provider. Writes response to out_ptr (max 4096 bytes).
fn handle_llm_query(prompt_ptr: u64, prompt_len: u64, out_ptr: u64) -> u64 {
    let len = prompt_len as usize;
    if len == 0 || len > 1024 { return u64::MAX; }
    let tier = crate::scheduler::current_task_max_tier();
    let task_id = crate::scheduler::current_user_id();

    let prompt = match unsafe { read_caller_slice(prompt_ptr, prompt_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };

    unsafe {
        let mut provider = crate::llm::provider::global_provider();
        let request = crate::llm::provider::LlmRequest::new(task_id, tier, prompt);

        match provider.submit(request) {
            Ok(request_id) => {
                // Process immediately (mock provider)
                provider.process_pending();
                // Try to get the response
                match provider.get_response(request_id) {
                    Some(response) => {
                        if response.is_success() {
                            let content = response.content();
                            if out_ptr != 0 && !content.is_empty() {
                                if !write_to_caller(out_ptr, content) { return u64::MAX; }
                            }
                            content.len() as u64
                        } else {
                            u64::MAX
                        }
                    }
                    None => u64::MAX,
                }
            }
            Err(_) => u64::MAX,
        }
    }
}

/// SYS_LLM_CONTEXT(suid_pairs_ptr, count, out_ptr) → context size or u64::MAX
/// Builds an LLM context from a list of SUID pairs. Writes serialized context to out_ptr.
///
/// FIXED 2026-05-14: avoid 258KB LlmContext stack allocation (task #40 root cause).
/// Process entries one-by-one using static scratch buffer instead of build_from_suids.
fn handle_llm_context(suid_pairs_ptr: u64, count: u64, out_ptr: u64) -> u64 {
    let n = (count as usize).min(32);
    let tier = crate::scheduler::current_task_max_tier();

    // Copy-in: a Ring-3 caller pointing suid_pairs at kernel memory gets
    // u64::MAX, not a kernel-memory read used as object IDs; parsing into
    // aligned kernel storage also closes the unaligned `(u8*) as (u64,u64)*`
    // cast and TOCTOU window for the object list.
    let suid_bytes: &[u8] = if n > 0 {
        match unsafe { read_caller_slice(suid_pairs_ptr, (n * 16) as u64) } {
            Some(s) => s,
            None => return u64::MAX,
        }
    } else {
        &[]
    };
    let mut suids = [(0u64, 0u64); 32];
    for i in 0..n {
        let o = i * 16;
        suids[i].0 = u64::from_le_bytes([
            suid_bytes[o],
            suid_bytes[o + 1],
            suid_bytes[o + 2],
            suid_bytes[o + 3],
            suid_bytes[o + 4],
            suid_bytes[o + 5],
            suid_bytes[o + 6],
            suid_bytes[o + 7],
        ]);
        suids[i].1 = u64::from_le_bytes([
            suid_bytes[o + 8],
            suid_bytes[o + 9],
            suid_bytes[o + 10],
            suid_bytes[o + 11],
            suid_bytes[o + 12],
            suid_bytes[o + 13],
            suid_bytes[o + 14],
            suid_bytes[o + 15],
        ]);
    }

    // Static scratch buffer for processing one entry at a time, behind the
    // kernel mutex: the old "safe because syscalls are serialized" comment
    // was wrong — an interrupts-enabled handler (llm_ask/agent TUI) can be
    // preempted and another task can enter this handler mid-loop
    // (2026-07-17 review, P1).
    static CONTEXT_SCRATCH: crate::sync::Mutex<[u8; 4096]> =
        crate::sync::Mutex::new([0; 4096]);

    // out_ptr == 0 is the size-query form. Any other pointer is a write
    // target: for a Ring-3 caller it must be a user-space address, and the
    // write window is capped at the user boundary — never 32 KiB of
    // attacker-chosen content into kernel memory (2026-07-17 review,
    // critical #2: self-elevation via max_tier / VOUCH_TABLE overwrite).
    const OUT_CAP: u64 = 32768;
    let out_limit: usize = if out_ptr == 0 {
        0
    } else if caller_needs_validation() {
        if !validate_user_ptr(out_ptr, 1) {
            return u64::MAX;
        }
        core::cmp::min(OUT_CAP, USER_ADDR_LIMIT - out_ptr) as usize
    } else {
        OUT_CAP as usize
    };

    unsafe {
        let registry = crate::semantic::registry::global_registry();
        let redactor = crate::llm::context_builder::global_redactor();
        let mut scratch_guard = CONTEXT_SCRATCH.lock();
        let scratch = &mut scratch_guard[..];

        let mut total_size = 0usize;
        let mut offset = 0usize;
        let do_write = out_ptr != 0;

        for (suid_high, suid_low) in suids[..n].iter().copied() {
            let suid = crate::semantic::SUID::new(suid_high, suid_low);

            if let Some(object) = registry.get(&suid) {
                let obj_tier = object.tier as u8;
                if obj_tier > tier {
                    continue; // Can't access this tier
                }

                // Apply tier-based processing (same logic as build_from_suids)
                let obj_content_bytes = match object.content.as_bytes() {
                    Some(bytes) => bytes,
                    None => continue, // Skip objects with no content
                };

                let content = match obj_tier {
                    0 => obj_content_bytes, // Tier 0: verbatim
                    1 => {
                        // Tier 1: summarize (placeholder - use redactor for now)
                        let n = redactor.redact(obj_content_bytes, scratch);
                        &scratch[..n]
                    }
                    2 => {
                        // Tier 2: redact
                        let n = redactor.redact(obj_content_bytes, scratch);
                        &scratch[..n]
                    }
                    _ => continue, // Tier 3+: exclude
                };

                let entry_len = content.len();
                total_size += entry_len + 8; // +8 for length prefix

                // Write to output buffer if provided
                if do_write && offset + entry_len + 8 <= out_limit {
                    // Write length prefix
                    let len_bytes = (entry_len as u64).to_le_bytes();
                    let len_dst = match out_ptr.checked_add(offset as u64) {
                        Some(p) => p,
                        None => return u64::MAX,
                    };
                    if !write_to_caller(len_dst, &len_bytes) {
                        return u64::MAX;
                    }
                    offset += 8;

                    // Write content
                    let content_dst = match out_ptr.checked_add(offset as u64) {
                        Some(p) => p,
                        None => return u64::MAX,
                    };
                    if entry_len > 0 && !write_to_caller(content_dst, content) {
                        return u64::MAX;
                    }
                    offset += entry_len;
                }
            }
        }

        if !do_write {
            total_size as u64 // Size query
        } else {
            offset as u64 // Bytes written
        }
    }
}

/// SYS_LLM_REDACT(input_ptr, input_len, out_ptr) → output length or u64::MAX
/// Redacts sensitive patterns (emails, SSNs, etc.) from text.
fn handle_llm_redact(input_ptr: u64, input_len: u64, out_ptr: u64) -> u64 {
    let len = input_len as usize;
    if len == 0 || len > 4096 { return u64::MAX; }

    // Use a static scratch buffer instead of a stack-allocated 4 KiB array.
    // The per-task kernel stack is 8 KiB, and a 4 KiB local plus the
    // syscall entry, dispatch, and handler frames overflows it silently
    // (no guard page yet). Behind the kernel mutex — the old
    // "single-threaded → static is safe" comment predated preemptible,
    // interrupts-enabled syscall handlers (2026-07-17 review, P1).
    static REDACT_SCRATCH: crate::sync::Mutex<[u8; 4096]> =
        crate::sync::Mutex::new([0; 4096]);

    let input = match unsafe { read_caller_slice(input_ptr, input_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };

    {
        let redactor = crate::llm::context_builder::global_redactor();
        let mut scratch_guard = REDACT_SCRATCH.lock();
        let scratch_slice: &mut [u8] = &mut scratch_guard[..];
        let out_len = redactor.redact(input, scratch_slice);
        if out_ptr != 0 && out_len > 0 {
            if !unsafe { write_to_caller(out_ptr, &scratch_slice[..out_len]) } { return u64::MAX; }
        }
        out_len as u64
    }
}

/// SYS_LLM_SUMMARIZE(input_ptr, input_len, out_ptr) → output length or u64::MAX
/// Summarizes text content.
fn handle_llm_summarize(input_ptr: u64, input_len: u64, out_ptr: u64) -> u64 {
    let len = input_len as usize;
    if len == 0 || len > 4096 { return u64::MAX; }

    let input = match unsafe { read_caller_slice(input_ptr, input_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };

    unsafe {
        let summarizer = crate::llm::context_builder::global_summarizer();
        let summary = summarizer.summarize(input);
        let data = summary.as_bytes();
        if out_ptr != 0 && !data.is_empty() {
            if !write_to_caller(out_ptr, data) { return u64::MAX; }
        }
        data.len() as u64
    }
}

/// SYS_LLM_ACCESS(requester_id, current_tier, requested_tier, justification_ptr | len<<32)
/// Submits a tier escalation request. Returns request ID or u64::MAX.
fn handle_llm_access(requester_id: u64, current_tier: u64, requested_tier: u64, justification_info: u64) -> u64 {
    let just_ptr = justification_info & 0xFFFF_FFFF;
    let just_len = (justification_info >> 32) as usize;

    let justification = if just_ptr != 0 && just_len > 0 && just_len <= 256 {
        match unsafe { read_caller_slice(just_ptr, just_len as u64) } {
            Some(s) => s,
            None => return u64::MAX,
        }
    } else {
        b"No justification provided"
    };

    {
        let mut queue = crate::llm::access_request::global_escalation_queue();
        let request = crate::llm::access_request::AccessRequest::for_tier(
            requester_id as u8,
            current_tier as u8,
            requested_tier as u8,
            justification,
        );
        match queue.submit(request) {
            Ok(id) => id as u64,
            Err(_) => u64::MAX,
        }
    }
}

/// SYS_LLM_STREAM_START(prompt_ptr, prompt_len, context_ptr) → request_id or u64::MAX
/// Start a streaming LLM request. Returns request ID for polling with SYS_LLM_STREAM_READ.
/// context_ptr points to serialized context data (same format as SYS_LLM_CONTEXT output).
pub fn handle_llm_stream_start(prompt_ptr: u64, prompt_len: u64, _context_ptr: u64) -> u64 {
    let len = prompt_len as usize;
    if len == 0 || len > 1024 { return u64::MAX; }

    let prompt = match unsafe { read_caller_slice(prompt_ptr, prompt_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };

    let task_id = crate::scheduler::current_user_id();
    let tier = crate::scheduler::current_task_max_tier();

    {
        let mut provider = crate::llm::provider::global_provider();
        let request = crate::llm::provider::LlmRequest::new(task_id, tier, prompt);

        match provider.submit(request) {
            Ok(request_id) => request_id,
            Err(_) => u64::MAX,
        }
    }
}

/// SYS_LLM_STREAM_READ(request_id, out_ptr, out_len) → bytes_read or error_code
/// Read chunk from streaming LLM response. Returns:
/// - > 0: bytes read (response continues)
/// - 0: response complete
/// - u64::MAX: error or invalid request_id
/// Special values: u64::MAX-1 = still processing, u64::MAX-2 = cancelled
pub fn handle_llm_stream_read(request_id: u64, out_ptr: u64, out_len: u64) -> u64 {
    let max_len = out_len as usize;
    if max_len == 0 || out_ptr == 0 { return u64::MAX; }

    unsafe {
        let provider = crate::llm::provider::global_provider();

        // Check request status
        match provider.get_status(request_id) {
            Some(crate::llm::provider::RequestState::Queued) |
            Some(crate::llm::provider::RequestState::Processing) => {
                // Still processing
                u64::MAX - 1
            },
            Some(crate::llm::provider::RequestState::Completed) => {
                // Get response and copy to output buffer
                if let Some(response) = provider.get_response(request_id) {
                    if response.is_success() {
                        let content = response.content();
                        let copy_len = content.len().min(max_len);
                        if copy_len > 0 && !write_to_caller(out_ptr, &content[..copy_len]) {
                            return u64::MAX;
                        }
                        copy_len as u64
                    } else {
                        response.error_code as u64
                    }
                } else {
                    u64::MAX
                }
            },
            Some(crate::llm::provider::RequestState::Cancelled) => {
                u64::MAX - 2
            },
            Some(crate::llm::provider::RequestState::Failed) => {
                // Try to get error code from response
                if let Some(response) = provider.get_response(request_id) {
                    response.error_code as u64
                } else {
                    u64::MAX
                }
            },
            _ => u64::MAX, // Invalid request ID
        }
    }
}

/// SYS_LLM_SET_POLICY(suid_high, suid_low, policy_data_ptr, policy_data_len) → result
/// Create or update a security policy. Returns:
/// - SUID.high: success, policy stored
/// - u64::MAX: error (permission denied, invalid data, etc.)
/// - u64::MAX-1: policy validation failed
/// - u64::MAX-2: insufficient privilege
pub fn handle_llm_set_policy(suid_high: u64, suid_low: u64, policy_data_ptr: u64, policy_data_len: u64) -> u64 {
    let data_len = policy_data_len as usize;
    if data_len == 0 || data_len > crate::security::policy::MAX_POLICY_SIZE || policy_data_ptr == 0 {
        return u64::MAX - 1; // Invalid parameters
    }

    let policy_suid = crate::semantic::SUID::new(suid_high, suid_low);

    // Validate this is a policy SUID
    if !crate::security::policy_suids::is_policy_suid(&policy_suid) {
        return u64::MAX - 1; // Not a valid policy SUID
    }

    let requester_id = crate::scheduler::current_user_id();
    let _requester_tier = crate::scheduler::current_task_max_tier();

    // Read policy data from the caller's address space (validated for Ring 3).
    let policy_data = match unsafe { read_caller_slice(policy_data_ptr, policy_data_len) } {
        Some(d) => d,
        None => return u64::MAX - 1,
    };

    // Deserialize policy object
    let policy = match crate::security::policy::PolicyObject::deserialize(policy_data) {
        Ok(p) => p,
        Err(_) => return u64::MAX - 1, // Invalid policy data
    };

    // Check permissions
    {
        let mut registry = crate::semantic::registry::global_registry();

        // If policy already exists, check if requester can modify it
        if let Some(existing_obj) = registry.get(&policy_suid) {
            if let Some(existing_content) = existing_obj.content.as_bytes() {
                if let Ok(existing_policy) = crate::security::policy::PolicyObject::deserialize(existing_content) {
                    if !existing_policy.can_modify(requester_id) {
                        return u64::MAX - 2; // Insufficient privilege
                    }
                }
            }
        } else {
            // Creating new policy - check if user has permission to create policies
            // System policies can only be created by admin/system
            if crate::security::policy_suids::is_system_policy(&policy_suid) {
                if requester_id != crate::security::user_ids::ADMIN &&
                   requester_id != crate::security::user_ids::SYSTEM {
                    return u64::MAX - 2; // Insufficient privilege
                }
            }
        }

        // Validate policy structure
        if !policy.is_active() && policy.rule_count == 0 {
            return u64::MAX - 1; // Empty/invalid policy
        }

        // Create semantic object for the policy
        // Policies are stored at Secret tier by default for security
        let policy_obj = match crate::semantic::SemanticObject::with_content(
            policy_suid,
            crate::memory::SecurityTier::Secret,
            policy.owner,
            policy_data,
        ) {
            Some(obj) => obj,
            None => return u64::MAX, // Failed to create object
        };

        // Store in registry
        if registry.insert(policy_obj) {
            // Policy stored successfully
            suid_high
        } else {
            u64::MAX // Registry insertion failed
        }
    }
}

/// SYS_LLM_GET_POLICY(suid_high, suid_low, out_ptr, out_len) → bytes_read or error
/// Retrieve a security policy by SUID. Returns:
/// - >0: bytes written to output buffer
/// - 0: policy not found
/// - u64::MAX: error (permission denied, buffer too small)
/// - u64::MAX-2: insufficient privilege
pub fn handle_llm_get_policy(suid_high: u64, suid_low: u64, out_ptr: u64, out_len: u64) -> u64 {
    let buffer_len = out_len as usize;
    if buffer_len == 0 || out_ptr == 0 {
        return u64::MAX;
    }

    let policy_suid = crate::semantic::SUID::new(suid_high, suid_low);

    // Validate this is a policy SUID
    if !crate::security::policy_suids::is_policy_suid(&policy_suid) {
        return u64::MAX;
    }

    let requester_id = crate::scheduler::current_user_id();

    unsafe {
        let registry = crate::semantic::registry::global_registry();

        if let Some(policy_obj) = registry.get(&policy_suid) {
            if let Some(policy_content) = policy_obj.content.as_bytes() {
                // Parse policy to check read permissions
                if let Ok(policy) = crate::security::policy::PolicyObject::deserialize(policy_content) {
                    // Check if requester can read this policy
                    // System policies are readable by admin/system only
                    // User policies are readable by owner + admin
                    let can_read = if crate::security::policy_suids::is_system_policy(&policy_suid) {
                        requester_id == crate::security::user_ids::ADMIN ||
                        requester_id == crate::security::user_ids::SYSTEM
                    } else {
                        requester_id == policy.owner ||
                        requester_id == crate::security::user_ids::ADMIN
                    };

                    if !can_read {
                        return u64::MAX - 2; // Insufficient privilege
                    }

                    // Copy policy data to output buffer
                    let copy_len = policy_content.len().min(buffer_len);
                    if copy_len > 0 && !write_to_caller(out_ptr, &policy_content[..copy_len]) {
                        return u64::MAX;
                    }

                    copy_len as u64
                } else {
                    u64::MAX // Policy deserialization failed
                }
            } else {
                u64::MAX // Policy has no content
            }
        } else {
            0 // Policy not found
        }
    }
}

// ============================================================================
// Crypto Syscalls (60-69)
// ============================================================================

/// SYS_ENCRYPT(key_ptr, nonce_ptr, data_ptr, data_len) → 0 or u64::MAX
/// In-place ChaCha20 encryption. key=32 bytes, nonce=12 bytes.
fn handle_encrypt(key_ptr: u64, nonce_ptr: u64, data_ptr: u64, data_len: u64) -> u64 {
    let len = data_len as usize;
    if len == 0 || len > 4096 { return u64::MAX; }

    unsafe {
        let key_bytes = match read_caller_slice(key_ptr, 32) {
            Some(s) => s,
            None => return u64::MAX,
        };
        let nonce_bytes = match read_caller_slice(nonce_ptr, 12) {
            Some(s) => s,
            None => return u64::MAX,
        };
        let data = match caller_slice_mut(data_ptr, data_len) {
            Some(s) => s,
            None => return u64::MAX,
        };

        let mut key_arr = [0u8; 32];
        key_arr.copy_from_slice(key_bytes);
        let mut nonce_arr = [0u8; 12];
        nonce_arr.copy_from_slice(nonce_bytes);

        let key = crate::crypto::CryptoKey::from_bytes(key_arr);
        let nonce = crate::crypto::Nonce::from_bytes(nonce_arr);

        crate::crypto::chacha20::encrypt(&key, &nonce, data);
        0
    }
}

/// SYS_DECRYPT(key_ptr, nonce_ptr, data_ptr, data_len) → 0 or u64::MAX
/// In-place ChaCha20 decryption (same as encrypt — XOR cipher).
fn handle_decrypt(key_ptr: u64, nonce_ptr: u64, data_ptr: u64, data_len: u64) -> u64 {
    // ChaCha20 is a stream cipher — encrypt and decrypt are the same operation
    handle_encrypt(key_ptr, nonce_ptr, data_ptr, data_len)
}

/// SYS_HASH(data_ptr, data_len, out_ptr) → 0 or u64::MAX
/// SHA-256 hash. Writes 32-byte digest to out_ptr.
fn handle_hash(data_ptr: u64, data_len: u64, out_ptr: u64) -> u64 {
    let len = data_len as usize;
    if len == 0 || len > 65536 { return u64::MAX; }
    if out_ptr == 0 { return u64::MAX; }

    let data = match unsafe { read_caller_slice(data_ptr, data_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };

    let digest = crate::crypto::sha256::hash(data);
    if !unsafe { write_to_caller(out_ptr, &digest) } { return u64::MAX; }
    0
}

/// Convenience function for userspace: write a string
pub fn write(s: &str) {
    dispatch(numbers::SYS_WRITE, s.as_ptr() as u64, s.len() as u64, 0, 0);
}

/// Convenience function for userspace: exit
pub fn exit(code: i32) -> ! {
    dispatch(numbers::SYS_EXIT, code as u64, 0, 0, 0);
    loop { core::hint::spin_loop(); }
}

/// Convenience function for userspace: yield
pub fn yield_now() {
    dispatch(numbers::SYS_YIELD, 0, 0, 0, 0);
}

// ============================================================================
// User identity & isolation (SYS_GETUID, SYS_SETUID, SYS_CREATE_USER, SYS_LOOKUP_USER)
// ============================================================================

/// SYS_GETUID → current task's effective user id (extended to u64).
fn handle_getuid() -> u64 {
    crate::scheduler::current_user_id() as u64
}

/// SYS_SETUID(uid) → 0 on success, u64::MAX on policy denial / unknown user.
///
/// Per `security::users::can_setuid_to`:
/// - SYSTEM can become anyone.
/// - ADMIN can drop to any non-SYSTEM user.
/// - Ordinary users cannot setuid.
fn handle_setuid(uid: u64) -> u64 {
    if uid > 255 { return u64::MAX; }
    let target = uid as u8;
    let requester = crate::scheduler::current_user_id();
    let registry = crate::security::users::global_user_registry();
    if !crate::security::users::can_setuid_to(requester, target, &registry) {
        return u64::MAX;
    }
    crate::scheduler::set_current_user_id(target);
    0
}

/// SYS_CREATE_USER(name_ptr, name_len, tier, group) → assigned uid or u64::MAX.
///
/// `tier` is the new user's default max security tier (0..=3, clamped to the
/// caller's own tier). `group` is the new user's group id. Only SYSTEM/ADMIN
/// may create users.
fn handle_create_user(name_ptr: u64, name_len: u64, tier: u64, group: u64) -> u64 {
    let requester = crate::scheduler::current_user_id();
    if !crate::security::users::is_privileged(requester) {
        return u64::MAX;
    }

    let len = name_len as usize;
    if len == 0 || len > crate::security::users::MAX_USERNAME_LEN { return u64::MAX; }
    let name = match unsafe { read_caller_str(name_ptr, name_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };

    // Clamp the new user's tier to the caller's. Without this an admin could
    // mint a user with a higher tier than themselves — exactly the kind of
    // privilege-laundering this module is meant to prevent.
    let requester_tier = crate::scheduler::current_task_max_tier();
    let new_tier_raw = (tier as u8).min(requester_tier).min(3);
    let new_tier = tier_from_u8(new_tier_raw);

    let group_id = (group & 0xFF) as u8;
    {
        let mut registry = crate::security::users::global_user_registry();
        match registry.create_user(name, group_id, new_tier) {
            Ok(uid) => uid as u64,
            Err(_) => u64::MAX,
        }
    }
}

/// SYS_LOOKUP_USER(uid, out_ptr, out_len) → bytes written or u64::MAX.
///
/// Writes a compact UTF-8 textual record into `out`:
///   `uid=<id> name=<name> tier=<n> group=<g> flags=<hex>`
/// This keeps the syscall ABI simple — user space can split on whitespace
/// without needing a binary layout shared between kernel and user-programs.
fn handle_lookup_user(uid: u64, out_ptr: u64, out_len: u64) -> u64 {
    if uid > 255 { return u64::MAX; }
    let target = uid as u8;
    let cap = out_len as usize;

    let mut scratch = [0u8; 96];
    let written = {
        let registry = crate::security::users::global_user_registry();
        match registry.lookup(target) {
            None => return 0, // 0 == "not found", distinct from u64::MAX (= error)
            Some(acc) => format_user_record(acc, &mut scratch),
        }
    };
    if written > cap { return u64::MAX; }
    if !unsafe { write_to_caller(out_ptr, &scratch[..written]) } { return u64::MAX; }
    written as u64
}

/// Render a [`UserAccount`] into the wire format used by SYS_LOOKUP_USER.
/// Lives next to the syscall so the format stays in sync with the comment
/// above. Returns bytes written.
fn format_user_record(acc: &crate::security::users::UserAccount, out: &mut [u8]) -> usize {
    let mut p = 0;
    p += write_str(&mut out[p..], b"uid=");
    p += write_dec(&mut out[p..], acc.id as u64);
    p += write_str(&mut out[p..], b" name=");
    p += copy_clamped(&mut out[p..], acc.name());
    p += write_str(&mut out[p..], b" tier=");
    p += write_dec(&mut out[p..], acc.default_max_tier as u64);
    p += write_str(&mut out[p..], b" group=");
    p += write_dec(&mut out[p..], acc.group as u64);
    p += write_str(&mut out[p..], b" flags=0x");
    p += write_hex(&mut out[p..], acc.flags.0 as u64);
    p
}

fn write_str(dst: &mut [u8], s: &[u8]) -> usize {
    let n = dst.len().min(s.len());
    dst[..n].copy_from_slice(&s[..n]);
    n
}

fn copy_clamped(dst: &mut [u8], src: &[u8]) -> usize {
    let n = dst.len().min(src.len());
    dst[..n].copy_from_slice(&src[..n]);
    n
}

fn write_dec(dst: &mut [u8], n: u64) -> usize {
    if n == 0 {
        if dst.is_empty() { return 0; }
        dst[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut k = 0;
    let mut v = n;
    while v > 0 && k < tmp.len() {
        tmp[k] = b'0' + (v % 10) as u8;
        v /= 10;
        k += 1;
    }
    let n_out = k.min(dst.len());
    for i in 0..n_out {
        dst[i] = tmp[k - 1 - i];
    }
    n_out
}

fn write_hex(dst: &mut [u8], n: u64) -> usize {
    if n == 0 {
        if dst.is_empty() { return 0; }
        dst[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 16];
    let mut k = 0;
    let mut v = n;
    while v > 0 && k < tmp.len() {
        let nibble = (v & 0xF) as u8;
        tmp[k] = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
        v >>= 4;
        k += 1;
    }
    let n_out = k.min(dst.len());
    for i in 0..n_out {
        dst[i] = tmp[k - 1 - i];
    }
    n_out
}

// ============================================================================
// Phase 14 Tier 3 — threading + sync syscalls (#45, #46, #47)
// ============================================================================

/// SYS_FUTEX_WAIT(addr, expected) → 0 on wake, 1 on value mismatch,
/// u64::MAX on bad pointer.
///
/// Atomically check `*(addr as *const u32) == expected`; if so, block
/// the current task with `BlockReason::Futex(addr)` until a matching
/// `SYS_FUTEX_WAKE(addr, _)` fires. The compare-and-block sequence is
/// race-free against other tasks' wakes because syscall handlers run
/// non-preemptively on this kernel (single CPU, no IRQ in handler).
///
/// The 1-return-value path matches Linux's EAGAIN — std::sync::Mutex
/// retries on EAGAIN.
fn handle_futex_wait(addr: u64, expected: u64) -> u64 {
    if addr == 0 || addr & 0x3 != 0 { return u64::MAX; }
    // SAFETY: range-validated for Ring-3 callers (a user task must not make
    // the kernel read — and block on — an arbitrary kernel address). The
    // remaining gap is mapping verification, tracked as task #41 follow-up
    // (guard pages + general user-pointer hardening).
    if caller_needs_validation() && !validate_user_ptr(addr, 4) { return u64::MAX; }
    let observed = unsafe { core::ptr::read_volatile(addr as *const u32) };
    if observed as u64 != expected { return 1; }

    crate::scheduler::futex_block(addr);
    // Yield immediately so the wait actually takes effect — without
    // this the syscall returns and the task continues running until
    // the next timer tick.
    crate::platform::schedule();
    0
}

/// SYS_FUTEX_WAKE(addr, max_count) → count actually woken.
///
/// Transitions up to `max_count` tasks blocked on `addr` back to Ready.
/// max_count = u64::MAX wakes all; 0 wakes none and returns 0.
fn handle_futex_wake(addr: u64, max_count: u64) -> u64 {
    if addr == 0 { return 0; }
    let max = if max_count > usize::MAX as u64 {
        usize::MAX
    } else {
        max_count as usize
    };
    crate::scheduler::futex_wake(addr, max) as u64
}

/// SYS_THREAD_SPAWN(entry_va, arg) → tid (slot index) / u64::MAX.
///
/// Ring-3 same-AS thread spawn is task #45 — needs spawn_thread on the
/// Platform with user-stack mapping in the parent's CR3. The handler is
/// already wired into the dispatch table so the syscall surface stays
/// stable; today it returns u64::MAX. The kernel-mode DEMO 27 exercises
/// the underlying scheduler primitives directly via context::spawn_task.
fn handle_thread_spawn(entry_va: u64, arg: u64) -> u64 {
    // Look up current task's cr3 from the saved context (platform-side).
    // Until task #45 lands the Ring-3 spawn path, we just reject.
    let _ = (entry_va, arg);
    let max_tier = crate::scheduler::current_task_max_tier();
    let cr3 = crate::platform::get().current_cr3();
    match crate::platform::get().spawn_thread("thread", cr3, entry_va, arg, max_tier) {
        Some(slot) => slot as u64,
        None => u64::MAX,
    }
}

/// SYS_THREAD_JOIN(tid) → exit_code / u64::MAX on bad tid.
///
/// Block the current task until scheduler slot `tid` reaches Exited
/// (pick_next auto-unblocks via `BlockReason::JoinTask`). When we
/// resume, read the published exit code.
///
/// `tid` is the scheduler slot index returned by SYS_THREAD_SPAWN.
fn handle_thread_join(tid: u64) -> u64 {
    let slot = tid as usize;
    if slot >= crate::scheduler::MAX_TASKS { return u64::MAX; }
    if slot == crate::scheduler::current_task_index() { return u64::MAX; }

    // Fast path: already exited.
    use crate::scheduler::TaskState;
    if crate::scheduler::task_state(slot) == TaskState::Exited {
        return crate::scheduler::task_exit_code(slot);
    }
    // Reject joining slots that are Empty / never spawned — there's
    // no "thread" to wait for. The caller probably has a stale tid.
    if crate::scheduler::task_state(slot) == TaskState::Empty {
        return u64::MAX;
    }

    crate::scheduler::join_block(slot);
    crate::platform::schedule();
    // On resume the slot should be Exited (pick_next's auto-unblock
    // condition). Read the code.
    crate::scheduler::task_exit_code(slot)
}

/// SYS_WAITNB(pid_hint) → child_pid (32-bit) on reap, 0 if no child
/// has exited yet, u64::MAX if caller has no children at all.
///
/// Non-blocking variant of SYS_WAIT — matches Linux waitpid with
/// WNOHANG. Cargo's parallel job manager polls this. `pid_hint` is
/// ignored for now (we always reap any zombie); a future revision can
/// honor it for targeted reaping.
fn handle_waitnb(pid_hint: u64) -> u64 {
    let _ = pid_hint;
    let current = crate::process::current_pid();
    unsafe {
        // Check children's state directly via the process table — we
        // can't just call wait() because that blocks; we need to peek.
        let proc = match crate::process::get(current) {
            Some(p) => p,
            None => return u64::MAX,
        };
        if proc.child_count == 0 { return u64::MAX; }
        // Look for a zombie among the children.
        for &child_pid in proc.children.iter().filter_map(|c| c.as_ref()) {
            if let Some(child) = crate::process::get(child_pid) {
                if child.state == crate::process::ProcessState::Zombie {
                    // Reap via the normal blocking wait — at this point
                    // it's guaranteed to return immediately because the
                    // child is already a zombie.
                    let _ = crate::process::wait(child_pid);
                    return child_pid.0 as u64;
                }
            }
        }
    }
    // Children exist but none has exited yet.
    0
}

// ============================================================================
// Networking for Ring 3 (100-105) — M25 std::net backing
// ============================================================================
//
// The smoltcp stack (kernel-core::net) has a single static TCP socket, so we
// expose exactly one TCP connection at a time to user space. `NET_TCP` holds
// it; the user-visible fd is the constant `NET_FD`. DNS is a one-shot resolve
// (no raw UDP sockets exposed).
//
// **All of these are non-blocking (#56).** Each does exactly ONE
// `net::poll()` + one attempt and returns. The std-shim drives the wait loop
// in user space (yield/sleep between tries). This is deliberate: the earlier
// design ran a multi-second interrupts-enabled poll loop *inside* the syscall,
// in a Ring-3 task's kernel context — the timer preempted into
// `schedule()`/`context_switch` thousands of times and tripped the task#40
// resume race (#PF in schedule's epilogue → #DF). Short syscalls keep the
// in-kernel, interrupts-enabled window tiny, like every other syscall.

/// The single user-visible TCP fd. Nonzero so 0 can't be mistaken for it.
const NET_FD: u64 = 3;
/// Behind the kernel mutex: the TCP handlers do a `net::poll()` (which can
/// take IRQs and, on other paths, preempt) between checking and mutating
/// the slot — the check/set must be atomic (2026-07-17 review, P1).
static NET_TCP: crate::sync::Mutex<Option<crate::net::TcpStream>> =
    crate::sync::Mutex::new(None);

/// SYS_DNS_RESOLVE(host_ptr, host_len) → IPv4 packed big-endian (first
/// octet in the high byte) or u64::MAX on failure. (DNS resolve internally
/// bounds its own poll loop on the boot/caller context — fine; it's the
/// long-lived TCP path that needed the non-blocking split.)
fn handle_dns_resolve(host_ptr: u64, host_len: u64) -> u64 {
    if host_len == 0 || host_len > 255 { return u64::MAX; }
    let host = match unsafe { read_caller_str(host_ptr, host_len) } {
        Some(s) => s,
        None => return u64::MAX,
    };
    match crate::net::resolve(host) {
        Some(ip) => {
            let b = ip.as_bytes();
            ((b[0] as u64) << 24) | ((b[1] as u64) << 16) | ((b[2] as u64) << 8) | (b[3] as u64)
        }
        None => u64::MAX,
    }
}

/// SYS_TCP_CONNECT(ipv4_be, port) → NET_FD with the SYN queued (NOT yet
/// established — the shim polls SYS_TCP_STATE), or u64::MAX on error.
fn handle_tcp_connect(ipv4_be: u64, port: u64) -> u64 {
    use crate::net::{TcpStream, Ipv4Address};
    if !crate::net::is_initialized() { return u64::MAX; }
    {
        let mut net_tcp = NET_TCP.lock();
        if net_tcp.is_some() { return u64::MAX; } // one connection at a time
        let ip = Ipv4Address::new(
            (ipv4_be >> 24) as u8, (ipv4_be >> 16) as u8,
            (ipv4_be >> 8) as u8, ipv4_be as u8,
        );
        match TcpStream::connect(ip, port as u16) {
            Ok(stream) => {
                crate::net::poll(); // one poll to emit the SYN
                *net_tcp = Some(stream);
                NET_FD
            }
            Err(_) => u64::MAX,
        }
    }
}

/// SYS_TCP_STATE(fd) → 0 closed / 1 connecting / 2 established / u64::MAX.
/// One poll, then report the socket state — lets the shim drive the
/// handshake from user space.
fn handle_tcp_state(fd: u64) -> u64 {
    if fd != NET_FD { return u64::MAX; }
    {
        let net_tcp = NET_TCP.lock();
        let stream = match net_tcp.as_ref() { Some(s) => s, None => return u64::MAX };
        crate::net::poll();
        if stream.is_established() { 2 }
        else if stream.is_closed() { 0 }
        else { 1 }
    }
}

/// SYS_TCP_READ(fd, buf_ptr, buf_len) → n (0=peer closed) / NET_WOULDBLOCK
/// (nothing ready) / u64::MAX. Single poll + one recv attempt.
fn handle_tcp_read(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    use crate::net::TcpError;
    if fd != NET_FD || buf_len == 0 { return u64::MAX; }
    let buf = match unsafe { caller_slice_mut(buf_ptr, buf_len) } {
        Some(b) => b,
        None => return u64::MAX,
    };
    {
        let mut net_tcp = NET_TCP.lock();
        let stream = match net_tcp.as_mut() { Some(s) => s, None => return u64::MAX };
        crate::net::poll();
        match stream.read(buf) {
            Ok(n) if n > 0 => n as u64,
            Ok(_) => numbers::NET_WOULDBLOCK,   // connected but nothing ready
            Err(TcpError::Eof) => 0,            // peer closed cleanly
            Err(_) => u64::MAX,
        }
    }
}

/// SYS_TCP_WRITE(fd, buf_ptr, buf_len) → n / NET_WOULDBLOCK / u64::MAX.
/// Single send attempt + one poll to push it onto the wire.
fn handle_tcp_write(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if fd != NET_FD || buf_len == 0 { return u64::MAX; }
    let buf = match unsafe { read_caller_slice(buf_ptr, buf_len) } {
        Some(b) => b,
        None => return u64::MAX,
    };
    {
        let mut net_tcp = NET_TCP.lock();
        let stream = match net_tcp.as_mut() { Some(s) => s, None => return u64::MAX };
        let r = match stream.write(buf) {
            Ok(n) if n > 0 => n as u64,
            Ok(_) => numbers::NET_WOULDBLOCK,   // tx ring full
            Err(_) => u64::MAX,
        };
        crate::net::poll(); // emit whatever was just queued
        r
    }
}

/// SYS_TCP_CLOSE(fd) → 0. Sends FIN, polls a couple of times to flush, frees.
fn handle_tcp_close(fd: u64) -> u64 {
    if fd != NET_FD { return u64::MAX; }
    {
        let mut net_tcp = NET_TCP.lock();
        if let Some(mut stream) = net_tcp.take() {
            stream.close();
            for _ in 0..8 { crate::net::poll(); }
            stream.release();
        }
    }
    0
}
