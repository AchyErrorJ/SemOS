//! Minimal DNS A-record resolver over smoltcp's UDP socket — M12.
//!
//! Replaces the hardcoded Anthropic IP in DEMO 16 with a real query to
//! QEMU SLIRP's DNS server at 10.0.2.3 (port 53). This is a *client-only*
//! resolver: it builds one A-record query, sends it, polls a bounded
//! number of times for the reply, and parses out the first A record.
//!
//! Design constraints, mirroring [`super::tcp`]:
//!   - `no_std`, no `alloc`. All scratch space is caller-owned static
//!     storage (smoltcp's `Borrowed` ManagedSlice path) or stack arrays.
//!   - Single in-flight query at a time (one socket slot).
//!   - Never panics on a malformed response — every length / bounds check
//!     returns `None` instead of indexing out of range.
//!   - Bounded poll loop so a dropped/never-answered query times out
//!     gracefully rather than hanging the boot.
//!
//! # Wire format (RFC 1035)
//!
//! Query we emit:
//! ```text
//!   Header (12 B): ID | flags=0x0100 (RD) | QDCOUNT=1 | AN=NS=AR=0
//!   Question:      QNAME (length-prefixed labels, 0-terminated)
//!                  QTYPE=A(1) QCLASS=IN(1)
//! ```
//! Response: same header echoed (ID must match, QR set), the question
//! repeated, then ANCOUNT resource records. We skip the question, walk
//! the answer RRs honouring 0xC0 name-compression pointers, and return
//! the first RR with TYPE=A / CLASS=IN / RDLENGTH=4.

use smoltcp::iface::SocketHandle;
use smoltcp::socket::udp;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

// ============================================================================
// SLIRP-provided resolver + protocol constants
// ============================================================================

/// DNS server port. The server address itself is per-interface and
/// comes from `super::state::dns_server()` (SLIRP default 10.0.2.3, or
/// the iPhone tether's 172.20.10.1 — set by `init_with_ipconfig`).
const DNS_PORT: u16 = 53;

/// Ephemeral local port for the query. RFC 6335 reserves 49152-65535;
/// we pin one because only one query is ever in flight. Distinct from
/// the TCP layer's 49152 so the two never clash if both are open.
const LOCAL_PORT: u16 = 49153;

const QTYPE_A: u16 = 1;
const QCLASS_IN: u16 = 1;
/// Recursion-Desired, query (QR=0), standard opcode.
const FLAGS_RD: u16 = 0x0100;

// ============================================================================
// Static per-socket storage (mirrors tcp.rs's static-buffer discipline)
// ============================================================================

/// One UDP datagram slot each way is enough — we send one query and wait
/// for one reply. Payloads cap at 512 B (classic DNS-over-UDP limit).
const UDP_PAYLOAD: usize = 512;

static mut UDP_RX_META: [udp::PacketMetadata; 4] = [udp::PacketMetadata::EMPTY; 4];
static mut UDP_TX_META: [udp::PacketMetadata; 4] = [udp::PacketMetadata::EMPTY; 4];
static mut UDP_RX_PAYLOAD: [u8; UDP_PAYLOAD * 4] = [0; UDP_PAYLOAD * 4];
static mut UDP_TX_PAYLOAD: [u8; UDP_PAYLOAD * 4] = [0; UDP_PAYLOAD * 4];

// ============================================================================
// Tiny fixed-size cache (host -> IP). No alloc: inline byte buffers.
// ============================================================================

const CACHE_SLOTS: usize = 8;
/// Max hostname length we cache. api.anthropic.com is 17 bytes; 64 is
/// generous headroom while keeping the table small.
const CACHE_HOST_MAX: usize = 64;

#[derive(Clone, Copy)]
struct CacheEntry {
    used: bool,
    host_len: usize,
    host: [u8; CACHE_HOST_MAX],
    ip: [u8; 4],
}

impl CacheEntry {
    const EMPTY: CacheEntry = CacheEntry {
        used: false,
        host_len: 0,
        host: [0; CACHE_HOST_MAX],
        ip: [0; 4],
    };
}

static mut CACHE: [CacheEntry; CACHE_SLOTS] = [CacheEntry::EMPTY; CACHE_SLOTS];
/// Round-robin victim for cache replacement when full.
static mut CACHE_NEXT: usize = 0;

fn cache_lookup(host: &str) -> Option<Ipv4Address> {
    let hb = host.as_bytes();
    unsafe {
        let cache = &*core::ptr::addr_of!(CACHE);
        for e in cache.iter() {
            if e.used && e.host_len == hb.len() && &e.host[..e.host_len] == hb {
                return Some(Ipv4Address::new(e.ip[0], e.ip[1], e.ip[2], e.ip[3]));
            }
        }
    }
    None
}

fn cache_insert(host: &str, ip: Ipv4Address) {
    let hb = host.as_bytes();
    if hb.len() > CACHE_HOST_MAX {
        return; // don't cache absurdly long names
    }
    let ipb = ip.as_bytes();
    unsafe {
        let cache = &mut *core::ptr::addr_of_mut!(CACHE);
        // Prefer an empty / matching slot; else round-robin replace.
        let mut slot = None;
        for (i, e) in cache.iter().enumerate() {
            if !e.used || (e.host_len == hb.len() && e.host[..e.host_len] == *hb) {
                slot = Some(i);
                break;
            }
        }
        let idx = slot.unwrap_or_else(|| {
            let i = CACHE_NEXT % CACHE_SLOTS;
            CACHE_NEXT = CACHE_NEXT.wrapping_add(1);
            i
        });
        let e = &mut cache[idx];
        e.used = true;
        e.host_len = hb.len();
        e.host[..hb.len()].copy_from_slice(hb);
        e.ip.copy_from_slice(&ipb[..4]);
    }
}

// ============================================================================
// Query builder
// ============================================================================

/// Encode an A-record query for `host` into `out`, returning the byte
/// length. Returns `None` if the name doesn't fit or has a bad label
/// (label > 63 B, or empty/double-dot). `txid` goes into the header ID.
fn build_query(host: &str, txid: u16, out: &mut [u8]) -> Option<usize> {
    // Header is 12 bytes; QTYPE+QCLASS trailer is 4; QNAME needs the
    // dotted name plus one length byte per label plus the root 0.
    let mut p = 0usize;
    // ID
    out.get_mut(p..p + 2)?.copy_from_slice(&txid.to_be_bytes());
    p += 2;
    // flags
    out.get_mut(p..p + 2)?.copy_from_slice(&FLAGS_RD.to_be_bytes());
    p += 2;
    // QDCOUNT=1, ANCOUNT=0, NSCOUNT=0, ARCOUNT=0
    for v in [1u16, 0, 0, 0] {
        out.get_mut(p..p + 2)?.copy_from_slice(&v.to_be_bytes());
        p += 2;
    }

    // QNAME: each dot-separated label is length-prefixed.
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        *out.get_mut(p)? = label.len() as u8;
        p += 1;
        out.get_mut(p..p + label.len())?
            .copy_from_slice(label.as_bytes());
        p += label.len();
    }
    // Root label (zero length terminator).
    *out.get_mut(p)? = 0;
    p += 1;

    // QTYPE, QCLASS
    out.get_mut(p..p + 2)?.copy_from_slice(&QTYPE_A.to_be_bytes());
    p += 2;
    out.get_mut(p..p + 2)?.copy_from_slice(&QCLASS_IN.to_be_bytes());
    p += 2;

    Some(p)
}

// ============================================================================
// Response parser
// ============================================================================

/// Advance `pos` past a (possibly compressed) DNS name in `buf`.
/// Returns the offset of the first byte *after* the name in the record
/// stream. Compression pointers (top two bits set, 0xC0) terminate the
/// in-stream name with a 2-byte pointer; uncompressed names end at the
/// 0 root label. Returns `None` on malformed input.
fn skip_name(buf: &[u8], mut pos: usize) -> Option<usize> {
    let mut guard = 0; // bound the loop against pathological label chains
    loop {
        guard += 1;
        if guard > 128 {
            return None;
        }
        let len = *buf.get(pos)?;
        if len & 0xC0 == 0xC0 {
            // Compression pointer: 2 bytes total, name ends here.
            // We don't need to follow it just to skip the name.
            return Some(pos + 2);
        }
        if len == 0 {
            // Root label terminates the name.
            return Some(pos + 1);
        }
        // Ordinary label: skip its length byte + the label bytes.
        pos = pos + 1 + len as usize;
    }
}

/// Parse a DNS response, verifying the transaction ID matches `txid`,
/// and return the first A record's IPv4 address. Returns `None` on any
/// inconsistency (wrong ID, not a response, truncated, no A record).
fn parse_response(buf: &[u8], txid: u16) -> Option<Ipv4Address> {
    // Header is 12 bytes.
    if buf.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([buf[0], buf[1]]);
    if id != txid {
        return None;
    }
    let flags = u16::from_be_bytes([buf[2], buf[3]]);
    // QR bit (0x8000) must be set: this is a response.
    if flags & 0x8000 == 0 {
        return None;
    }
    // RCODE (low 4 bits) must be 0 (no error).
    if flags & 0x000F != 0 {
        return None;
    }
    let qdcount = u16::from_be_bytes([buf[4], buf[5]]);
    let ancount = u16::from_be_bytes([buf[6], buf[7]]);
    if ancount == 0 {
        return None;
    }

    let mut pos = 12usize;

    // Skip all questions: each is NAME + QTYPE(2) + QCLASS(2).
    for _ in 0..qdcount {
        pos = skip_name(buf, pos)?;
        pos = pos.checked_add(4)?; // QTYPE + QCLASS
        if pos > buf.len() {
            return None;
        }
    }

    // Walk answer RRs.
    for _ in 0..ancount {
        pos = skip_name(buf, pos)?;
        // TYPE(2) CLASS(2) TTL(4) RDLENGTH(2) = 10 bytes of fixed fields.
        let rtype = u16::from_be_bytes([*buf.get(pos)?, *buf.get(pos + 1)?]);
        let rclass = u16::from_be_bytes([*buf.get(pos + 2)?, *buf.get(pos + 3)?]);
        let rdlength = u16::from_be_bytes([*buf.get(pos + 8)?, *buf.get(pos + 9)?]) as usize;
        let rdata_start = pos + 10;
        let rdata_end = rdata_start.checked_add(rdlength)?;
        if rdata_end > buf.len() {
            return None;
        }
        if rtype == QTYPE_A && rclass == QCLASS_IN && rdlength == 4 {
            return Some(Ipv4Address::new(
                buf[rdata_start],
                buf[rdata_start + 1],
                buf[rdata_start + 2],
                buf[rdata_start + 3],
            ));
        }
        // Not an A record (CNAME, AAAA, etc.) — skip its RDATA and continue.
        pos = rdata_end;
    }

    None
}

// ============================================================================
// Public resolve()
// ============================================================================

/// Resolve `host` to an IPv4 address via SLIRP's DNS server.
///
/// Checks the in-memory cache first; on a miss, sends one A-record query
/// to 10.0.2.3:53 and polls a bounded number of times for the reply.
/// Returns `None` if the net stack isn't up, the socket can't be set up,
/// the query times out, or the response is malformed / has no A record.
/// On success the result is cached so a second call returns immediately
/// without a new query.
pub fn resolve(host: &str) -> Option<Ipv4Address> {
    if let Some(ip) = cache_lookup(host) {
        return Some(ip);
    }
    if !super::state::is_initialized() {
        return None;
    }

    // Derive a transaction ID from the platform tick counter. Weak, but
    // adequate for a single-host test environment (matches the seed
    // smoltcp uses for its own RNG today).
    let txid = (crate::platform::ticks() as u16) ^ 0x5EED;

    // Build the query on the stack.
    let mut query = [0u8; UDP_PAYLOAD];
    let qlen = build_query(host, txid, &mut query)?;

    let handle = match open_socket() {
        Some(h) => h,
        None => return None,
    };

    // Send (and periodically re-send) the query. UDP has no retransmit of
    // its own, so a datagram dropped while the neighbor (10.0.2.3) ARP
    // resolves — or lost for any reason — is gone for good. A stub resolver
    // must retransmit; without it the first cold lookup after the ARP entry
    // expires silently times out (observed: api.anthropic.com resolved warm,
    // example.com later returned nothing). We re-send every RESEND_EVERY
    // polls until a reply arrives or we hit MAX_POLLS.
    let remote = IpEndpoint::new(IpAddress::Ipv4(super::state::dns_server()), DNS_PORT);
    let send_query = || -> bool {
        unsafe {
            match super::state::sockets_mut() {
                Some(sockets) => sockets
                    .get_mut::<udp::Socket>(handle)
                    .send_slice(&query[..qlen], remote)
                    .is_ok(),
                None => false,
            }
        }
    };
    if !send_query() {
        close_socket(handle);
        return None;
    }

    // Wait on WALL-CLOCK, not iteration count. SLIRP forwards the query to
    // the host resolver, so a cold lookup can take tens to hundreds of ms —
    // far longer than a tight poll loop spends (4000 tight polls ≈ a few ms,
    // which is why a warm name resolved but a cold one silently timed out).
    // Budget ~3s of ticks, pacing with a spin so we actually consume time,
    // and retransmit ~4×/s to survive a dropped datagram.
    let tick_hz = crate::scheduler::SCHEDULER_TICK_HZ;
    let start = crate::platform::ticks();
    let deadline = start + tick_hz * 3;
    let resend_ticks = (tick_hz / 4).max(1);
    let mut last_send = start;
    let mut result = None;
    let mut rx_seen = false;
    let mut safety = 0u64;
    loop {
        super::state::poll();
        let got = unsafe {
            let sockets = match super::state::sockets_mut() {
                Some(s) => s,
                None => break,
            };
            let socket = sockets.get_mut::<udp::Socket>(handle);
            if socket.can_recv() {
                let mut rx = [0u8; UDP_PAYLOAD];
                match socket.recv_slice(&mut rx) {
                    Ok((n, _meta)) => { rx_seen = true; parse_response(&rx[..n], txid) }
                    Err(_) => None,
                }
            } else {
                None
            }
        };
        if let Some(ip) = got {
            result = Some(ip);
            break;
        }
        let now = crate::platform::ticks();
        if now >= deadline { break; }
        if !rx_seen && now.saturating_sub(last_send) >= resend_ticks {
            let _ = send_query();
            last_send = now;
        }
        // Pace the loop so it spends real time (lets the host resolve + the
        // device deliver) instead of spinning the poll millions of times.
        for _ in 0..20_000 { core::hint::spin_loop(); }
        // Safety net against a stuck tick counter (interrupts off, etc.).
        safety += 1;
        if safety > 5_000_000 { break; }
    }

    close_socket(handle);

    if let Some(ip) = result {
        cache_insert(host, ip);
    }
    result
}

// ============================================================================
// Socket lifecycle helpers
// ============================================================================

/// Build a UDP socket from the static buffers, bind it to the ephemeral
/// local port, and add it to the SocketSet. Returns its handle.
fn open_socket() -> Option<SocketHandle> {
    unsafe {
        let rx_meta: &'static mut [udp::PacketMetadata] =
            &mut *core::ptr::addr_of_mut!(UDP_RX_META);
        let tx_meta: &'static mut [udp::PacketMetadata] =
            &mut *core::ptr::addr_of_mut!(UDP_TX_META);
        let rx_payload: &'static mut [u8] = &mut *core::ptr::addr_of_mut!(UDP_RX_PAYLOAD);
        let tx_payload: &'static mut [u8] = &mut *core::ptr::addr_of_mut!(UDP_TX_PAYLOAD);

        let rx_buf = udp::PacketBuffer::new(rx_meta, rx_payload);
        let tx_buf = udp::PacketBuffer::new(tx_meta, tx_payload);
        let mut socket = udp::Socket::new(rx_buf, tx_buf);

        if socket.bind(LOCAL_PORT).is_err() {
            return None;
        }

        let sockets = super::state::sockets_mut()?;
        Some(sockets.add(socket))
    }
}

/// Close and remove the UDP socket, freeing its SocketSet slot for reuse.
fn close_socket(handle: SocketHandle) {
    unsafe {
        if let Some(sockets) = super::state::sockets_mut() {
            sockets.get_mut::<udp::Socket>(handle).close();
            sockets.remove(handle);
        }
    }
}
