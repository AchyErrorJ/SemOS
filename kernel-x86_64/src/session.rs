//! Interactive landing/session and idle-heartbeat tasks.
//!
//! Extracted from `main.rs` for the 2026-07-17 review TCB split: this module
//! owns the post-demo interactive shell loop plus the background idle/iPhone
//! keepalive tasks.

use crate::*;

/// M10 watchdog: print "boot reached idle" once with the current tick count
/// (the proof-of-life), then attempt to spawn a continuous-heartbeat task in
/// a dedicated scheduler slot. The spawn itself works (`spawn_task` returns
/// `Some(slot)`); the slot's state advances to `Ready` via `mark_ready`. But
/// **the scheduler currently doesn't pick the new slot** after init's hlt —
/// the round-robin `pick_next` returns slot 6's expected role and yet the
/// task's entry println never fires. That's a real scheduler issue (separate
/// from M10), filed for follow-up. The continuous mode below remains in tree
/// because the moment the scheduler issue is fixed it'll start working with
/// zero code change — the wiring is correct.
/// M10 watchdog: prints proof-of-life ("kernel reached idle"), then spawns a
/// dedicated `kernel_idle_task` for the continuous heartbeat. The scheduler
/// picks it up normally (confirmed during the 2026-05-28 instrumented run —
/// `IDLE_RUN_COUNT` ramped to thousands of iterations). The earlier "no
/// continuous beats" was a tick-rate bug in this function, not a scheduler
/// issue: my heartbeat used `/100` assuming 100 Hz, but the kernel timer
/// runs at `SCHEDULER_TICK_HZ` (~62 Hz on QEMU), so 5 wall-clock seconds
/// gave `elapsed_s = 3` and the `>= 5` branch never fired.
pub(crate) fn idle_with_heartbeat() -> ! {
    use kernel_core::syscall::{dispatch, numbers::SYS_SLEEP};
    let ticks = kernel_core::platform::ticks();
    println!("[heartbeat] kernel reached idle — ticks={} (M10 watchdog)", ticks);
    match crate::context::spawn_task("idle-heartbeat", kernel_idle_task) {
        Some(slot) => println!("[heartbeat] kernel idle task in slot {} (continuous beat)", slot),
        None => println!("[heartbeat] could not spawn idle task — only the one-shot above"),
    }
    // SYS_SLEEP (not bare hlt) — moves init to Blocked state so the scheduler
    // is FORCED to pick others (including the idle task). The earlier hlt
    // version had init perpetually Ready, which starved slot 6's forward
    // progress despite pick_next correctly picking it. SYS_SLEEP is the
    // canonical "yield until much later" call.
    loop {
        let _ = dispatch(SYS_SLEEP, 100, 0, 0, 0); // ~1.6 s @ 62 Hz, then wake + sleep again
    }
}

/// Continuous M10 watchdog. Emits a beat every 5 wall-clock seconds via
/// `kernel_core::scheduler::SCHEDULER_TICK_HZ` — must match the actual timer
/// rate, NOT a 100 Hz assumption.
/// Dedicated background task for the iPhone tether keepalive. Runs the
/// 1 Hz carrier poll + self-healing re-enumeration in its OWN scheduler
/// slot so its blocking USB transfers (a carrier check, or a full
/// re-enumeration) are preempted by the timer and never starve the
/// keyboard pump (which lives in the loader task). The network stack
/// itself is driven by the shell's TCP path, not here.
#[cfg(feature = "interactive")]
fn ipheth_keepalive_task() {
    use kernel_core::syscall::{dispatch, numbers::SYS_SLEEP};
    loop {
        usb::ehci::ipheth_tick(); // internally rate-limited to ~1 s
        // ~0.5 s between checks; the tick no-ops until its 1 s boundary,
        // so this just bounds how soon we notice a carrier-up after Trust.
        let _ = dispatch(SYS_SLEEP, 31, 0, 0, 0);
    }
}

fn kernel_idle_task() {
    const TICK_HZ: u64 = kernel_core::scheduler::SCHEDULER_TICK_HZ;
    const BEAT_TICKS: u64 = 5 * TICK_HZ;
    let start = kernel_core::platform::ticks();
    println!("[heartbeat] idle task started (ticks={}, expecting one beat every {} ticks)",
        start, BEAT_TICKS);
    let mut next_emit = start + BEAT_TICKS;
    loop {
        let now = kernel_core::platform::ticks();
        if now >= next_emit {
            let elapsed_s = now.saturating_sub(start) / TICK_HZ;
            println!("[heartbeat] T+{}s — alive (ticks={})", elapsed_s, now);
            next_emit = now + BEAT_TICKS;
        }
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}

/// Feature-gated interactive landing (`--features interactive`). Instead of
/// idling after the demo suite, drop into the live `sem-sh` shell driven by the
/// real keyboard: USB/PS2 IRQ → `tty::input_push` → Console line discipline →
/// the shell's `SYS_READ(fd 0)`; the shell's stdout is the framebuffer console.
/// This is the "shell IS the OS interface" landing — run apps from any PATH,
/// pipe, and reach Claude via the `ask`/`fetch` builtins. Unlike the agent's
/// own sandboxed shell (tier 0), the interactive user's shell runs at tier 3 so
/// it can reach every security tier. When the user types `exit` we reap the
/// child (freeing its address-space frames) and relaunch — a login-shell loop.
#[cfg(feature = "interactive")]
pub(crate) fn interactive_session() {
    use kernel_core::syscall::{dispatch, numbers::*};
    use kernel_core::scheduler::{self, TaskState};

    println!();
    println!("================================================================");
    println!("  Interactive sem-sh — your keyboard is live.");
    println!("  Try:  ls /bin  |  ps  |  free  |  cat /apps/...  |  ask \"hello\"");
    println!("  'exit' relaunches the shell; power off the VM to stop.");
    println!("================================================================");
    println!();

    // Pin FD/stdio resolution to this (the loader) task before spawning.
    kernel_core::process::set_kernel_task_id(Some(scheduler::current_task_index()));

    // If an iPhone tether is live, run its carrier keepalive in a
    // dedicated task so its blocking USB transfers never stall the
    // keyboard pump that lives in this (the loader) task.
    if usb::ehci::ipheth_active() {
        match crate::context::spawn_task("ipheth-keepalive", ipheth_keepalive_task) {
            Some(slot) => println!("[ipheth] keepalive task in slot {}", slot),
            None => println!("[ipheth] could not spawn keepalive task"),
        }
    }

    loop {
        // No args → REPL. The child inherits fd 0 = Console (keyboard line
        // discipline) and fd 1 = Console (framebuffer). We do NOT synthesize
        // keystrokes — the real keyboard drives stdin.
        let path = "/bin/sem-sh";
        let pid = dispatch(SYS_SPAWN, path.as_ptr() as u64, path.len() as u64, 3, 0);
        if pid == u64::MAX {
            println!("  [interactive] could not spawn /bin/sem-sh — idling.");
            return;
        }
        let child = kernel_core::process::ProcessId(pid as u32);
        let cs = kernel_core::process::get(child).and_then(|p| p.task_id);
        if let Some(slot) = cs {
            // The human-driven interactive shell is the sole authority allowed
            // to vouch runtime-created tools.  Without this, SYS_VOUCH always
            // denies with "caller is not the interactive console" because the
            // vouch authority remains usize::MAX.
            kernel_core::syscall::set_vouch_authority(slot);
            println!("  [interactive] sem-sh slot {} is vouch authority", slot);
        }

        // Wait for the user to `exit`. While the shell blocks on SYS_READ(fd 0)
        // we must *service the USB keyboard*: QEMU (and real hardware) deliver
        // keystrokes to the USB HID device, whose event ring is only drained
        // when someone polls it — nothing else does while we idle here. Each
        // tick we pump the ring into the line discipline (+ echo), then SLEEP
        // to yield to the shell. Re-pin after each sleep (the current slot
        // drifts while we yield).
        let mut prev_keys = [0u8; 6];
        if let Some(slot) = cs {
            while scheduler::task_state(slot) != TaskState::Exited {
                // Skip our pump while a fullscreen app (agent TUI / editor) owns
                // the keyboard (the shell is blocked in its syscall and that app
                // pumps the HID ring itself) — else we'd steal/split keystrokes.
                if !FULLSCREEN_APP_ACTIVE.load(core::sync::atomic::Ordering::Relaxed) {
                    pump_console_input(&mut prev_keys);
                    // Apply any scroll requested from the PS/2 keyboard IRQ
                    // (PageUp/PageDown/End on the built-in keyboard).
                    crate::framebuffer::apply_pending_scroll();
                }
                // Tick the network stack so background work (DHCP lease
                // acquisition on real Ethernet, ARP, socket timers) makes
                // progress even while the shell is idle at its prompt. Cheap
                // no-op until the stack is initialized.
                kernel_core::net::poll();
                let _ = dispatch(SYS_SLEEP, 1, 0, 0, 0);
                kernel_core::process::set_kernel_task_id(Some(scheduler::current_task_index()));
            }
            // We are the child's waiter and it has exited: reap now so its PT
            // frames return to the pool instead of leaking across relaunches.
            kernel_core::platform::get().reap_slot(slot);
        }
        println!();
        println!("  [interactive] shell exited — relaunching (power off to stop).");
    }
}

/// Drain the USB HID keyboard event ring into the TTY line discipline, with
/// edge detection so a held key isn't re-emitted on every report (a HID report
/// lists *all* currently-pressed keys; a key already in `prev` is still-held,
/// not a new press). `input_push` echoes only to serial, so we also echo
/// printables/Enter/Backspace to the framebuffer console here — otherwise the
/// user can't see what they type on the plain console (the TUI uses `peek_line`
/// for that instead). Arrow keys become `ESC [ A/B/C/D` for the line editor.
#[cfg(feature = "interactive")]
fn pump_console_input(prev: &mut [u8; 6]) {
    usb::xhci::poll_hid(|rep| {
        let shift = rep.shift_held();
        for &k in rep.keys.iter() {
            if k == 0 || prev.contains(&k) {
                continue; // empty slot, or a key that was already held
            }
            // Scrollback paging — consumed here, never delivered to the shell.
            // PageUp/Down scroll the console through history; End jumps to live.
            match k {
                0x4B => {
                    crate::framebuffer::scroll_view(15);
                    continue;
                }
                0x4E => {
                    crate::framebuffer::scroll_view(-15);
                    continue;
                }
                0x4D => {
                    crate::framebuffer::scroll_view(-1_000_000);
                    continue;
                }
                _ => {}
            }
            // If scrolled into history and the user starts typing, snap back to
            // live first so their keystrokes are visible.
            if crate::framebuffer::is_scrolled() {
                crate::framebuffer::scroll_view(-1_000_000);
            }
            if let Some(c) = usb::hid::keycode_to_ascii(k, shift) {
                // For Backspace, check whether the line discipline actually has
                // something to delete *before* input_push consumes it — else an
                // erase echo at an empty prompt would chew into the "sem-sh$ "
                // prompt text itself (the line discipline stops at column 0, but
                // our framebuffer echo wouldn't have).
                let erasing = if c == 0x08 || c == 0x7F {
                    let mut pk = [0u8; 256];
                    let (_, cur) = tty::peek_line(&mut pk);
                    cur > 0
                } else {
                    false
                };
                tty::input_push(c);
                match c {
                    0x20..=0x7E => print!("{}", c as char),
                    b'\n' | b'\r' => println!(),
                    0x08 | 0x7F if erasing => print!("\u{8} \u{8}"), // erase last glyph
                    _ => {}
                }
            } else {
                // Arrow keys (HID usage 0x4F..0x52) → ANSI for the line editor.
                let letter = match k {
                    0x4F => Some(b'C'),
                    0x50 => Some(b'D'),
                    0x51 => Some(b'B'),
                    0x52 => Some(b'A'),
                    _ => None,
                };
                if let Some(letter) = letter {
                    tty::input_push(0x1B);
                    tty::input_push(b'[');
                    tty::input_push(letter);
                }
            }
        }
        *prev = rep.keys;
    });
}
