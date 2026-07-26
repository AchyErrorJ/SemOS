//! Host (x86_64) side of M56 pairing: identity persistence, the paired-device
//! store, and the `SYS_PAIR` handshake orchestration.
//!
//! The wire + crypto contract lives entirely in `kernel_core::pairing` (KAT'd
//! against the committed reference vectors by boot DEMO 86). This file only does
//! the I/O the syscall needs: filesystem persistence, the TCP handshake loop,
//! and the interactive SAS confirmation on the console. It interoperates with
//! the `companion-ios` app's `PairingListener`.
//!
//! Security notes (M56 v1):
//! - The SemOS static private identity key is stored **plaintext** at
//!   `/etc/paired-devices/self.key` (tier Secret, so out of LLM reach). Secure-
//!   Enclave-style wrapping is deferred (pairing-v1.md §9 Q3); the device itself
//!   is the trust boundary.
//! - Cross-reboot persistence of the identity + records depends on the FS
//!   snapshot being saved/restored; the records are written into the namespace,
//!   which the snapshot mechanism carries when it runs.

use alloc::format;
use alloc::string::String;

use kernel_core::fs::paths::Namespace;
use kernel_core::net::{self, Ipv4Address, TcpStream};
use kernel_core::pairing::{
    build_confirm, build_hello, confirm_mac, decode_payload, derive_session, pairing_id,
    parse_frame_header, verify_confirm, PairRecord, FRAME_ACK, FRAME_CONFIRM, FRAME_HEADER_LEN,
    NONCE_LEN, VERSION,
};
use kernel_core::semantic::object::SecurityTier;

const DIR_ETC: &str = "/etc";
const DIR_PAIRED: &str = "/etc/paired-devices";
const SELF_KEY: &str = "/etc/paired-devices/self.key";

/// Iterations of `net::poll()` we spin waiting on a network step before giving
/// up. Each iteration hlts to the next timer tick (~10 ms at 100 Hz), so this
/// is roughly a 20 s budget — generous for a LAN peer, bounded so a missing
/// phone can't wedge the shell forever.
const NET_POLL_BUDGET: u32 = 2000;

// ============================================================================
// Filesystem store
// ============================================================================

fn ensure_dirs() {
    // mkdir is idempotent-ish: ignore "already exists". resolve tells us if the
    // dir is already there.
    if Namespace::resolve(DIR_ETC).is_err() {
        let _ = Namespace::mkdir(DIR_ETC);
    }
    if Namespace::resolve(DIR_PAIRED).is_err() {
        let _ = Namespace::mkdir(DIR_PAIRED);
    }
}

/// Load the SemOS static identity, generating + persisting one on first use.
/// Returns `(private, public)`.
fn load_or_create_identity() -> ([u8; 32], [u8; 32]) {
    use kernel_core::crypto::x25519::x25519_base;
    ensure_dirs();

    let mut priv_key = [0u8; 32];
    if let Ok(32) = Namespace::read_file_into(SELF_KEY, &mut priv_key) {
        return (priv_key, x25519_base(&priv_key));
    }

    // First boot with pairing: mint a fresh identity.
    if kernel_core::platform::random_bytes(&mut priv_key).is_err() {
        // random_bytes should not fail (RDRAND probed at boot); if it does we
        // must not proceed with a zero key.
        crate::println!("  [pair] FATAL: RNG failed generating identity key");
        return ([0u8; 32], [0u8; 32]);
    }
    let pub_key = x25519_base(&priv_key);
    match Namespace::create_file(SELF_KEY, SecurityTier::Secret, &priv_key) {
        Ok(_) => crate::println!("  [pair] minted new SemOS identity at {}", SELF_KEY),
        Err(_) => crate::println!("  [pair] warning: could not persist identity (kept in memory)"),
    }
    (priv_key, pub_key)
}

fn save_record(id: &str, rec: &PairRecord) -> bool {
    ensure_dirs();
    let path = format!("{}/{}", DIR_PAIRED, id);
    // Overwrite if this device was paired before.
    if Namespace::resolve(&path).is_ok() {
        return Namespace::write_file(&path, &rec.to_bytes()).is_ok();
    }
    Namespace::create_file(&path, SecurityTier::Secret, &rec.to_bytes()).is_ok()
}

// ============================================================================
// TCP framing helpers (non-blocking read/write + net::poll, per net::tcp)
// ============================================================================

/// Send every byte of `data`, polling between partial writes. Returns false on
/// a transport error or if the budget runs out.
fn write_all(stream: &mut TcpStream, data: &[u8]) -> bool {
    let mut sent = 0;
    let mut budget = NET_POLL_BUDGET;
    while sent < data.len() {
        match stream.write(&data[sent..]) {
            Ok(0) => {
                if budget == 0 {
                    return false;
                }
                budget -= 1;
                net::poll();
                x86_64::instructions::hlt();
            }
            Ok(n) => sent += n,
            Err(_) => return false,
        }
    }
    // Push the segment out.
    net::poll();
    true
}

/// Read exactly `buf.len()` bytes, polling. Returns false on EOF/error/timeout.
fn read_exact(stream: &mut TcpStream, buf: &mut [u8]) -> bool {
    let mut got = 0;
    let mut budget = NET_POLL_BUDGET;
    while got < buf.len() {
        match stream.read(&mut buf[got..]) {
            Ok(0) => {
                if budget == 0 {
                    return false;
                }
                budget -= 1;
                net::poll();
                x86_64::instructions::hlt();
            }
            Ok(n) => got += n,
            Err(_) => return false, // Eof / NotConnected
        }
    }
    true
}

/// Read one framed message: a 3-byte header then `len-1` body bytes. Returns
/// `(type, body_len)` with the body written into `body_out`.
fn read_frame(stream: &mut TcpStream, body_out: &mut [u8]) -> Option<(u8, usize)> {
    let mut hdr = [0u8; FRAME_HEADER_LEN];
    if !read_exact(stream, &mut hdr) {
        return None;
    }
    let (ty, body_len) = parse_frame_header(&hdr)?;
    if body_len > body_out.len() {
        return None;
    }
    if !read_exact(stream, &mut body_out[..body_len]) {
        return None;
    }
    Some((ty, body_len))
}

// ============================================================================
// Console SAS confirmation
// ============================================================================

/// Block until the user answers y/n at the console. Keeps the network alive
/// (the phone may deliver its CONFIRM meanwhile). Returns true on y/Y.
fn console_confirm() -> bool {
    loop {
        if let Some(k) = crate::keyboard::read_key() {
            match k {
                b'y' | b'Y' => return true,
                b'n' | b'N' | b'\n' | b'\r' | 0x1b => return false,
                _ => {}
            }
        }
        net::poll();
        x86_64::instructions::hlt();
    }
}

// ============================================================================
// SYS_PAIR orchestration
// ============================================================================

pub fn run_pair(qr_ptr: u64, qr_len: u64) -> u64 {
    // The handshake needs the timer (poll cadence) and the keyboard (SAS
    // confirm); enable interrupts for the duration, as the editor/agent do.
    x86_64::instructions::interrupts::enable();

    let bytes = match unsafe { kernel_core::syscall::read_caller_slice(qr_ptr, qr_len) } {
        Some(b) => b,
        None => return 0,
    };
    let qr = match core::str::from_utf8(bytes) {
        Ok(s) => s.trim(),
        Err(_) => {
            crate::println!("  [pair] invalid QR string (not UTF-8)");
            return 0;
        }
    };
    let payload = match decode_payload(qr) {
        Ok(p) => p,
        Err(e) => {
            crate::println!("  [pair] bad pairing string: {:?}", e);
            return 0;
        }
    };

    let (sem_priv, sem_pub) = load_or_create_identity();
    let mut nonce_s = [0u8; NONCE_LEN];
    if kernel_core::platform::random_bytes(&mut nonce_s).is_err() {
        crate::println!("  [pair] RNG failed");
        return 0;
    }

    let ip = Ipv4Address::new(payload.ip[0], payload.ip[1], payload.ip[2], payload.ip[3]);
    crate::println!(
        "  [pair] connecting to phone {}.{}.{}.{}:{} ...",
        payload.ip[0], payload.ip[1], payload.ip[2], payload.ip[3], payload.port
    );
    let mut stream = match TcpStream::connect(ip, payload.port) {
        Ok(s) => s,
        Err(_) => {
            crate::println!("  [pair] TCP connect failed");
            return 0;
        }
    };

    // Wait for the handshake to establish.
    let mut budget = NET_POLL_BUDGET;
    while !stream.is_established() {
        if stream.is_closed() || budget == 0 {
            crate::println!("  [pair] connection did not establish");
            stream.close();
            return 0;
        }
        budget -= 1;
        net::poll();
        x86_64::instructions::hlt();
    }

    // HELLO -> phone.
    let mut hello = [0u8; 64];
    let hlen = build_hello(&sem_pub, &nonce_s, &mut hello);
    if !write_all(&mut stream, &hello[..hlen]) {
        crate::println!("  [pair] failed sending HELLO");
        stream.close();
        return 0;
    }

    // <- ACK.
    let mut body = [0u8; 128];
    match read_frame(&mut stream, &mut body) {
        Some((ty, n)) if ty == FRAME_ACK && n >= 1 && body[0] == VERSION => {}
        _ => {
            crate::println!("  [pair] no/!bad ACK from phone");
            stream.close();
            return 0;
        }
    }

    // Derive the shared session. own = sem_priv, peer = phone_pub.
    let sess = derive_session(
        VERSION, &payload.phone_pub, &payload.nonce_p, &sem_pub, &nonce_s, &sem_priv,
        &payload.phone_pub,
    );

    crate::println!("  [pair] ----------------------------------------");
    crate::println!("  [pair]  Verify this matches the code on your phone:");
    crate::println!("  [pair]      SAS = {:06}", sess.sas);
    crate::println!("  [pair]  Press 'y' if they match, any other key to abort.");
    crate::println!("  [pair] ----------------------------------------");
    if !console_confirm() {
        crate::println!("  [pair] aborted at SAS check");
        stream.close();
        return 0;
    }

    // CONFIRM -> phone (our MAC uses the "confirm-sem" label).
    let our_mac = confirm_mac(&sess.session_key, &sess.transcript_hash, true);
    let mut cf = [0u8; 40];
    let cflen = build_confirm(&our_mac, &mut cf);
    if !write_all(&mut stream, &cf[..cflen]) {
        crate::println!("  [pair] failed sending CONFIRM");
        stream.close();
        return 0;
    }

    // <- CONFIRM from phone (verify against the "confirm-phone" label).
    match read_frame(&mut stream, &mut body) {
        Some((ty, n)) if ty == FRAME_CONFIRM => {
            if !verify_confirm(&sess.session_key, &sess.transcript_hash, false, &body[..n]) {
                crate::println!("  [pair] AUTH FAILED: phone CONFIRM did not verify (possible MITM)");
                stream.close();
                return 0;
            }
        }
        _ => {
            crate::println!("  [pair] no/!bad CONFIRM from phone");
            stream.close();
            return 0;
        }
    }

    // Persist the pairing.
    let mut idbuf = [0u8; 16];
    pairing_id(&payload.phone_pub, &sem_pub, &mut idbuf);
    let id = core::str::from_utf8(&idbuf).unwrap_or("");
    let rec = PairRecord {
        phone_pub: payload.phone_pub,
        ip: payload.ip,
        port: payload.port,
        created_at: kernel_core::platform::wall_clock().unwrap_or(0),
    };
    if save_record(id, &rec) {
        crate::println!("  [pair] PAIRED: device {}", id);
    } else {
        crate::println!("  [pair] paired, but could not persist the record");
    }
    stream.close();
    1
}

// ============================================================================
// SYS_PAIRED / SYS_UNPAIR
// ============================================================================

pub fn run_paired_list() -> u64 {
    if Namespace::resolve(DIR_PAIRED).is_err() {
        crate::println!("paired: no devices");
        return 0;
    }
    let mut count = 0u64;
    let mut names: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    let _ = Namespace::readdir(DIR_PAIRED, |name, _suid| {
        if name != "self.key" {
            names.push(String::from(name));
        }
    });
    crate::println!("paired devices ({}):", names.len());
    for name in &names {
        let path = format!("{}/{}", DIR_PAIRED, name);
        let mut buf = [0u8; kernel_core::pairing::RECORD_LEN];
        match Namespace::read_file_into(&path, &mut buf) {
            Ok(n) => match PairRecord::from_bytes(&buf[..n]) {
                Some(r) => {
                    crate::println!(
                        "  {}  ip={}.{}.{}.{}:{}  created_at={}",
                        name, r.ip[0], r.ip[1], r.ip[2], r.ip[3], r.port, r.created_at
                    );
                    count += 1;
                }
                None => crate::println!("  {}  (corrupt record)", name),
            },
            Err(_) => crate::println!("  {}  (unreadable)", name),
        }
    }
    count
}

pub fn run_unpair(id_ptr: u64, id_len: u64) -> u64 {
    let bytes = match unsafe { kernel_core::syscall::read_caller_slice(id_ptr, id_len) } {
        Some(b) => b,
        None => return 0,
    };
    let id = match core::str::from_utf8(bytes) {
        Ok(s) => s.trim(),
        Err(_) => return 0,
    };
    if id.is_empty() || id == "self.key" || id.contains('/') {
        crate::println!("unpair: invalid id");
        return 0;
    }
    let path = format!("{}/{}", DIR_PAIRED, id);
    match Namespace::unlink(&path) {
        Ok(_) => {
            crate::println!("unpair: forgot device {}", id);
            1
        }
        Err(_) => {
            crate::println!("unpair: no such device {}", id);
            0
        }
    }
}

// ============================================================================
// Store self-test (DEMO 86) — exercises the persistence layer without a phone
// ============================================================================

/// Validate the identity + record store end-to-end in the namespace FS:
/// identity is stable within a boot, a record round-trips through
/// save/read/parse, and unpair removes it. Returns true on success.
pub fn store_self_test() -> bool {
    use kernel_core::pairing::RECORD_LEN;

    // 1. Identity is generated and stable across calls within a boot.
    let (p1, pub1) = load_or_create_identity();
    let (p2, pub2) = load_or_create_identity();
    let id_stable = p1 == p2 && pub1 == pub2 && p1 != [0u8; 32];

    // 2. A record round-trips through save -> read -> parse.
    let test_id = "0test0test0test0";
    let rec = PairRecord { phone_pub: [7u8; 32], ip: [10, 0, 0, 5], port: 9000, created_at: 12345 };
    let saved = save_record(test_id, &rec);
    let path = format!("{}/{}", DIR_PAIRED, test_id);
    let mut buf = [0u8; RECORD_LEN];
    let read_ok = matches!(Namespace::read_file_into(&path, &mut buf), Ok(n) if n == RECORD_LEN);
    let parsed = PairRecord::from_bytes(&buf)
        .map(|r| r.phone_pub == [7u8; 32] && r.ip == [10, 0, 0, 5] && r.port == 9000 && r.created_at == 12345)
        .unwrap_or(false);

    // 3. unpair removes it.
    let removed = Namespace::unlink(&path).is_ok();
    let gone = Namespace::resolve(&path).is_err();

    id_stable && saved && read_ok && parsed && removed && gone
}
