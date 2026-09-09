//! The smart-home hub pipeline (DEMO 98; the north-star "voice assistant /
//! smart hub" loop): a command channel from the voice gateway, intents
//! installed by packages, actions dispatched to device shims.
//!
//! Architecture (v1):
//!   gateway (voice/phone upstream)  —TCP :9001→  hub task (this module)
//!   hub task —UDP datagram→  device shim (10.0.2.2:<port>) — the lamp etc.
//!
//! Intents are DATA, installed by `semos install` into
//! /var/lib/hub/intents/<name>.intent (SemFS-journaled — the hub's
//! vocabulary survives hard kills with no reinstall; DEMO 98's boot-2 beat).
//! Intent spec (one line, `|` separated):
//!   patterns=lights,light|action=udp:9002:$CMD|reply=lights command sent
//! `$CMD` in the action expands to the received command text, so one intent
//! covers "lights on" / "lights off" / "lights 50%".
//!
//! TCP is single-connection in this kernel (NET_TCP), so actions ride UDP
//! (fire-and-forget, same primitive as netlog) while the hub holds the
//! gateway channel. Replies go back over the gateway TCP.
//!
//! Console-gated: `hub start` / `hub stop` (SYS_HUB ops 1/2), `hub intents`
//! (read-only op 3). The hub task PARKS on stop (task_exit_stub never marks
//! slots Exited — see selfdev80_test_task).

use kernel_core::net::{self, Ipv4Address, TcpStream};
use kernel_core::net::netlog::send_udp;

/// QEMU slirp host gateway (the shim's TCP command channel).
const GATEWAY_PORT: u16 = 9001;
/// Poll cadence while waiting for gateway bytes (ticks @ 62 Hz).
const POLL_TICKS: u64 = 6; // ~100 ms

static HUB_RUNNING: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
static HUB_STOP: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

pub const HUB_OP_START: u64 = 1;
pub const HUB_OP_STOP: u64 = 2;
pub const HUB_OP_INTENTS: u64 = 3;

fn gateway_ip() -> Ipv4Address {
    Ipv4Address::new(10, 0, 2, 2)
}

/// One installed intent, parsed from its .intent spec.
struct Intent {
    name: alloc::string::String,
    patterns: alloc::vec::Vec<alloc::string::String>,
    action_port: u16,
    action_template: alloc::string::String, // "$CMD" expands to the command
    reply: alloc::string::String,
}

/// Load all installed intents from the (journaled) namespace. Called per
/// command so installs take effect without a hub restart.
fn load_intents() -> alloc::vec::Vec<Intent> {
    use kernel_core::fs::paths::Namespace;
    let mut out = alloc::vec::Vec::new();
    let mut names = alloc::vec::Vec::new();
    {
        let mut collect = |n: &str| names.push(alloc::string::String::from(n));
        if Namespace::for_each_child("/var/lib/hub/intents", &mut collect).is_err() {
            return out;
        }
    }
    for fname in names {
        if !fname.ends_with(".intent") {
            continue;
        }
        let path = alloc::format!("/var/lib/hub/intents/{}", fname);
        let mut buf = [0u8; 512];
        let n = match Namespace::read_file_into(&path, &mut buf) {
            Ok(n) => n,
            Err(_) => continue,
        };
        let spec = match core::str::from_utf8(&buf[..n]) {
            Ok(s) => s.trim(),
            Err(_) => continue,
        };
        // patterns=a,b|action=udp:PORT:TMPL|reply=text
        let mut patterns = alloc::vec::Vec::new();
        let mut port: u16 = 0;
        let mut tmpl = alloc::string::String::new();
        let mut reply = alloc::string::String::new();
        for field in spec.split('|') {
            if let Some(v) = field.strip_prefix("patterns=") {
                for p in v.split(',') {
                    if !p.is_empty() {
                        patterns.push(alloc::string::String::from(p));
                    }
                }
            } else if let Some(v) = field.strip_prefix("action=udp:") {
                let mut it = v.splitn(2, ':');
                port = it.next().and_then(|p| p.parse().ok()).unwrap_or(0);
                tmpl = alloc::string::String::from(it.next().unwrap_or(""));
            } else if let Some(v) = field.strip_prefix("reply=") {
                reply = alloc::string::String::from(v);
            }
        }
        let name = fname.trim_end_matches(".intent");
        if !patterns.is_empty() && port != 0 {
            out.push(Intent {
                name: alloc::string::String::from(name),
                patterns,
                action_port: port,
                action_template: tmpl,
                reply,
            });
        }
    }
    out
}

/// Match one command line against the installed intents; on a match, fire
/// the UDP action. Returns the reply text, or None when nothing matched.
fn dispatch_command(cmd: &str) -> Option<alloc::string::String> {
    let intents = load_intents();
    let lower: alloc::string::String = cmd
        .bytes()
        .map(|b| (b as char).to_ascii_lowercase())
        .collect();
    for intent in &intents {
        let hit = intent.patterns.iter().any(|p| lower.contains(p.as_str()));
        if !hit {
            continue;
        }
        let payload = intent.action_template.replace("$CMD", cmd);
        let sent = send_udp(gateway_ip(), intent.action_port, payload.as_bytes());
        crate::println!(
            "[hub] command {:?} → intent '{}' → udp 10.0.2.2:{} ({} bytes)",
            cmd, intent.name, intent.action_port, sent
        );
        return Some(alloc::format!("OK {} — {}", intent.reply, intent.name));
    }
    None
}

/// The hub task: hold the gateway channel, serve commands forever.
/// Reconnects after disconnects; parks when `hub stop` is requested.
fn hub_task() {
    use kernel_core::syscall::{dispatch, numbers::SYS_SLEEP};
    // Let DHCP + the shell settle before dialing out.
    let _ = dispatch(SYS_SLEEP, 3 * 62, 0, 0, 0);
    'outer: loop {
        if HUB_STOP.load(core::sync::atomic::Ordering::Relaxed) {
            break;
        }
        if !net::is_initialized() {
            let _ = dispatch(SYS_SLEEP, 62, 0, 0, 0);
            continue;
        }
        let mut stream = match TcpStream::connect(gateway_ip(), GATEWAY_PORT) {
            Ok(s) => s,
            Err(_) => {
                crate::println!("[hub] gateway unreachable — retrying in 2s");
                let _ = dispatch(SYS_SLEEP, 2 * 62, 0, 0, 0);
                continue;
            }
        };
        crate::println!("[hub] connected to gateway 10.0.2.2:{}", GATEWAY_PORT);
        let mut line = alloc::vec::Vec::<u8>::new();
        let mut buf = [0u8; 256];
        loop {
            if HUB_STOP.load(core::sync::atomic::Ordering::Relaxed) {
                break;
            }
            net::poll();
            match stream.read(&mut buf) {
                Ok(0) => {
                    let _ = dispatch(SYS_SLEEP, POLL_TICKS, 0, 0, 0);
                }
                Ok(n) => {
                    for &b in &buf[..n] {
                        if b == b'\n' {
                            let cmd = alloc::string::String::from(
                                alloc::string::String::from_utf8_lossy(&line).trim()
                            );
                            line.clear();
                            if cmd.is_empty() {
                                continue;
                            }
                            crate::println!("[hub] heard: {:?}", cmd);
                            let reply = match dispatch_command(&cmd) {
                                Some(r) => {
                                    crate::println!(
                                        "[DEMO 98] PASS: hub heard {:?} → intent matched → action dispatched",
                                        cmd
                                    );
                                    r
                                }
                                None => alloc::string::String::from("ERR unknown command"),
                            };
                            let mut out = reply.into_bytes();
                            out.push(b'\n');
                            let mut off = 0;
                            while off < out.len() {
                                net::poll();
                                match stream.write(&out[off..]) {
                                    Ok(0) => {
                                        let _ = dispatch(SYS_SLEEP, POLL_TICKS, 0, 0, 0);
                                    }
                                    Ok(w) => off += w,
                                    Err(_) => break,
                                }
                            }
                        } else {
                            line.push(b);
                            if line.len() > 512 {
                                line.clear(); // runaway line guard
                            }
                        }
                    }
                }
                Err(_) => {
                    crate::println!("[hub] gateway channel closed — reconnecting");
                    break;
                }
            }
        }
        stream.close();
        drop(stream);
        let _ = dispatch(SYS_SLEEP, 62, 0, 0, 0);
    }
    crate::println!("[hub] stopped");
    HUB_RUNNING.store(false, core::sync::atomic::Ordering::Relaxed);
    // Park (task_exit_stub never marks slots Exited — a returning task
    // wedges the machine; see selfdev80_test_task).
    loop {
        let _ = dispatch(SYS_SLEEP, 62 * 60, 0, 0, 0);
    }
}

/// SYS_HUB backing. The console gate for start/stop is at the dispatcher.
pub fn run_hub(op: u64) -> u64 {
    match op {
        HUB_OP_START => {
            if HUB_RUNNING.swap(true, core::sync::atomic::Ordering::Relaxed) {
                crate::println!("[hub] already running");
                return 0;
            }
            HUB_STOP.store(false, core::sync::atomic::Ordering::Relaxed);
            match crate::context::spawn_task("hub", hub_task) {
                Some(slot) => {
                    crate::println!("[hub] started (task slot {})", slot);
                    0
                }
                None => {
                    HUB_RUNNING.store(false, core::sync::atomic::Ordering::Relaxed);
                    crate::println!("[hub] could not spawn hub task");
                    u64::MAX
                }
            }
        }
        HUB_OP_STOP => {
            if !HUB_RUNNING.load(core::sync::atomic::Ordering::Relaxed) {
                crate::println!("[hub] not running");
                return u64::MAX;
            }
            HUB_STOP.store(true, core::sync::atomic::Ordering::Relaxed);
            crate::println!("[hub] stop requested");
            0
        }
        HUB_OP_INTENTS => {
            let intents = load_intents();
            if intents.is_empty() {
                crate::println!("[hub] no intents installed (semos install <pkg> with an intent block)");
            }
            for i in &intents {
                crate::println!(
                    "  {} — patterns: {} → udp :{}",
                    i.name,
                    i.patterns.join(", "),
                    i.action_port
                );
            }
            0
        }
        _ => {
            crate::println!("hub: unknown op {}", op);
            u64::MAX
        }
    }
}
