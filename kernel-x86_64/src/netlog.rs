//! netlog — mirror the kernel log to a LAN listener over UDP.
//!
//! The whole point: when the agent (or anything) misbehaves on hardware there's
//! no way to see the log without standing in front of the screen. `serial::_print`
//! is the single choke point every `print!`/`println!` funnels through, so we
//! copy each byte into a small ring there — cheap, no alloc, no net — and a
//! separate `SYS_NETLOG` syscall drains the ring and UDP-sends it to your Mac
//! (run `nc -u -l 9000` there).
//!
//! Split deliberately:
//!   * `_print` → `netlog::write_str` runs with interrupts OFF holding the
//!     serial lock. It must not touch the network stack: UDP send needs
//!     `net::poll`, which prints status (recursion) and could re-enter `_print`
//!     (deadlock). So this stage only copies bytes.
//!   * `SYS_NETLOG` → `send_log` runs in normal syscall context and owns the
//!     UDP socket. Send happens here, never from inside `_print`.
//!
//! UDP is fire-and-forget — no handshake, loss-tolerant, and it can't stall the
//! console the way a TCP connect would if the Mac isn't listening. The cost is
//! best-effort delivery, which is fine for a debug log.
//!
//! The UDP send itself lives in `kernel_core::net::netlog::send_udp` — the
//! socket set and smoltcp aren't reachable from this crate.

use core::fmt;

/// Ring capacity. Big enough to hold the boot banner + a whole agent session's
/// worth of lines; on overflow the oldest bytes are dropped (it's a debug log,
/// not a journal — losing the head is the right trade).
const CAP: usize = 8192;

/// The log ring. `head` is the next write index, `len` the valid byte count;
/// the oldest byte is at `(head + CAP - len) % CAP`. Single-writer (the serial
/// `_print`, single-core) + a syscall-context reader that only ever *shrinks*
/// `len`, so a plain `static mut` with the existing access discipline is fine.
static mut BUF: [u8; CAP] = [0; CAP];
static mut HEAD: usize = 0;
static mut LEN: usize = 0;

/// Append `s` to the ring. Called from `serial::_print` with interrupts off —
/// keep this a pure memory copy, no locks, no alloc, no printing.
pub fn write_str(s: &str) {
    unsafe {
        let buf = &mut *core::ptr::addr_of_mut!(BUF);
        let head = core::ptr::addr_of_mut!(HEAD);
        let len = core::ptr::addr_of_mut!(LEN);
        for &b in s.as_bytes() {
            buf[*head] = b;
            *head = (*head + 1) % CAP;
            if *len < CAP {
                *len += 1;
            }
            // On overflow (len already CAP) the byte we just overwrote was the
            // oldest — exactly the drop-oldest behaviour we want.
        }
    }
}

/// Format into the ring via the `core::fmt` machinery (shared with serial in
/// `_print`). Errors are ignored — best-effort log.
struct RingWriter;
impl fmt::Write for RingWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_str(s);
        Ok(())
    }
}

/// Mirror a pre-formatted message into the ring. Cheap; safe under the serial
/// lock / interrupts-off because it only copies bytes.
pub fn mirror(args: fmt::Arguments) {
    use fmt::Write;
    let _ = RingWriter.write_fmt(args);
}

/// Parse `a.b.c.d` (ASCII, no leading +/-, each octet 0..=255) into an IPv4
/// octet array. No alloc, no FromStr dependency — netlog is a debug tool.
fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut idx = 0usize;
    for part in s.split('.') {
        if idx >= 4 || part.is_empty() || part.len() > 3 {
            return None;
        }
        let mut v: u32 = 0;
        for &c in part.as_bytes() {
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + (c - b'0') as u32;
        }
        if v > 255 {
            return None;
        }
        octets[idx] = v as u8;
        idx += 1;
    }
    if idx == 4 { Some(octets) } else { None }
}

/// Parse `ip` or `ip:port`. Port defaults to 9000. Returns (octets, port).
fn parse_target(s: &str) -> Option<([u8; 4], u16)> {
    let (ip, port) = match s.rfind(':') {
        // Only treat the tail as a port if there's exactly one ':' (a bare
        // IPv4 literal has none; anything else we don't try to be clever about).
        Some(c) if s.matches(':').count() == 1 => {
            let p: u32 = s[c + 1..].parse().ok()?;
            if p == 0 || p > 65535 {
                return None;
            }
            (&s[..c], p as u16)
        }
        _ => (s, 9000u16),
    };
    Some((parse_ipv4(ip)?, port))
}

/// SYS_NETLOG entry point: drain the ring and UDP-send it to `target` ("a.b.c.d"
/// or "a.b.c.d:port"). Runs in normal syscall context. Returns the number of
/// bytes sent (0 if the log is empty, the stack is down, or the target is bad).
pub fn run(target_ptr: u64, target_len: u64) -> u64 {
    // Copy the target string out of caller memory. A user caller can't hand us
    // a kernel pointer here: the syscall dispatcher validated the range first.
    let mut tbuf = [0u8; 64];
    let n = (target_len as usize).min(tbuf.len());
    if n == 0 {
        return 0;
    }
    let target = unsafe {
        core::ptr::copy_nonoverlapping(target_ptr as *const u8, tbuf.as_mut_ptr(), n);
        match core::str::from_utf8(&tbuf[..n]) {
            Ok(s) => s.trim(),
            Err(_) => return 0,
        }
    };
    let (octets, port) = match parse_target(target) {
        Some(t) => t,
        None => return 0,
    };
    send_log(octets, port)
}

/// Drain the ring and hand it to the kernel-core UDP sender in MTU-sized
/// datagrams. Best-effort: returns bytes actually handed to the socket.
fn send_log(octets: [u8; 4], port: u16) -> u64 {
    // Snapshot the ring out from under the writer (drain). We take the bytes
    // now so the socket send below never races `_print` appending more.
    let mut logbuf = [0u8; CAP];
    let count = unsafe {
        let buf = &*core::ptr::addr_of!(BUF);
        let head = *core::ptr::addr_of!(HEAD);
        let len = *core::ptr::addr_of!(LEN);
        let start = (head + CAP - len) % CAP;
        for i in 0..len {
            logbuf[i] = buf[(start + i) % CAP];
        }
        // Mark drained: new writes start a fresh ring.
        *core::ptr::addr_of_mut!(LEN) = 0;
        len
    };
    if count == 0 {
        return 0;
    }
    let dest = kernel_core::net::Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]);
    kernel_core::net::netlog::send_udp(dest, port, &logbuf[..count]) as u64
}
