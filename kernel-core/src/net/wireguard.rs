//! WireGuard data plane (SemNet S1) — Noise_IK handshake + transport crypto.
//!
//! Implements the wire protocol from the WireGuard whitepaper: handshake
//! initiation/response (both roles) and counter-based transport messages with
//! a replay window. The cookie/anti-DoS extension (message type 3) is
//! deliberately not implemented yet — `mac2` fields are zeroed, and incoming
//! cookie replies are ignored. Under no CPU load Linux never sends them.
//!
//! This module is pure protocol: no sockets, no time, no RNG, no heap (like
//! the rest of kernel-core, storage is fixed-size). Callers supply randomness
//! and a seconds counter (for TAI64N timestamps); the kernel side owns UDP
//! plumbing. Unit-tested in-process (initiator and responder Devices talking
//! to each other); interop-tested against Linux `wg` in QEMU (SemNet S1
//! acceptance).

use crate::crypto::blake2s::{blake2s_256, keyed_blake2s_256, Blake2s};
use crate::crypto::poly1305::{aead_decrypt, aead_encrypt};
use crate::crypto::x25519::{x25519, x25519_base};
use crate::crypto::{CryptoKey, Nonce, TAG_SIZE};

// --- wire constants --------------------------------------------------------

pub const MSG_HANDSHAKE_INITIATION: u32 = 1;
pub const MSG_HANDSHAKE_RESPONSE: u32 = 2;
pub const MSG_COOKIE_REPLY: u32 = 3;
pub const MSG_TRANSPORT: u32 = 4;

pub const INITIATION_LEN: usize = 148;
pub const RESPONSE_LEN: usize = 92;
pub const TRANSPORT_HEADER_LEN: usize = 16;

const CONSTRUCTION: &[u8] = b"Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s";
const IDENTIFIER: &[u8] = b"WireGuard v1 zx2c4 Jason@zx2c4.com";
const LABEL_MAC1: &[u8] = b"mac1----";

/// TAI64N epoch offset (2^62 + 10) as in the WireGuard reference code.
const TAI64N_BASE: u64 = 0x4000_0000_0000_000a;

// --- errors ----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgError {
    BadLength,
    BadMessageType,
    BadMac1,
    DecryptFailed,
    UnknownPeer,
    UnknownIndex,
    Replay,
    StaleTimestamp,
    NoSession,
    NoPendingHandshake,
    NoRoom,
    BufferTooSmall,
}

pub type WgResult<T> = Result<T, WgError>;

// --- KDF / hash helpers (whitepaper §5.3) -----------------------------------

fn mac(key: &[u8; 32], input: &[u8]) -> [u8; 32] {
    keyed_blake2s_256(key, input)
}

fn kdf1(ck: &[u8; 32], input: &[u8]) -> [u8; 32] {
    let t1 = mac(ck, input);
    mac(&t1, &[0x01])
}

fn kdf2(ck: &[u8; 32], input: &[u8]) -> ([u8; 32], [u8; 32]) {
    let t1 = mac(ck, input);
    let t2 = mac(&t1, &[0x01]);
    let mut t3in = [0u8; 33];
    t3in[..32].copy_from_slice(&t2);
    t3in[32] = 0x02;
    let t3 = mac(&t1, &t3in);
    (t2, t3)
}

fn kdf3(ck: &[u8; 32], input: &[u8]) -> ([u8; 32], [u8; 32], [u8; 32]) {
    let t1 = mac(ck, input);
    let t2 = mac(&t1, &[0x01]);
    let mut t3in = [0u8; 33];
    t3in[..32].copy_from_slice(&t2);
    t3in[32] = 0x02;
    let t3 = mac(&t1, &t3in);
    let mut t4in = [0u8; 33];
    t4in[..32].copy_from_slice(&t3);
    t4in[32] = 0x03;
    let t4 = mac(&t1, &t4in);
    (t2, t3, t4)
}

fn mix_hash(h: &[u8; 32], data: &[u8]) -> [u8; 32] {
    let mut st = Blake2s::new();
    st.update(h);
    st.update(data);
    st.finalize()
}

fn initial_ck() -> [u8; 32] {
    blake2s_256(CONSTRUCTION)
}

fn initial_h(ck: &[u8; 32]) -> [u8; 32] {
    mix_hash(ck, IDENTIFIER)
}

/// MAC1 key = HASH("mac1----" || recipient_static_pubkey), first 16 bytes.
fn mac1_key(recipient_static_pub: &[u8; 32]) -> [u8; 16] {
    let mut st = Blake2s::new();
    st.update(LABEL_MAC1);
    st.update(recipient_static_pub);
    let h = st.finalize();
    let mut k = [0u8; 16];
    k.copy_from_slice(&h[..16]);
    k
}

fn compute_mac1(key16: &[u8; 16], msg_prefix: &[u8]) -> [u8; 16] {
    let mut key32 = [0u8; 32];
    key32[..16].copy_from_slice(key16);
    let m = keyed_blake2s_256(&key32, msg_prefix);
    let mut out = [0u8; 16];
    out.copy_from_slice(&m[..16]);
    out
}

fn aead_seal(key: &[u8; 32], aad: &[u8; 32], plaintext: &[u8], out: &mut [u8]) -> usize {
    let k = CryptoKey::from_bytes(*key);
    let n = Nonce::zero();
    let mut tag = [0u8; TAG_SIZE];
    aead_encrypt(&k, &n, aad, plaintext, out, &mut tag).expect("aead_encrypt buffer");
    let n_pt = plaintext.len();
    out[n_pt..n_pt + TAG_SIZE].copy_from_slice(&tag);
    n_pt + TAG_SIZE
}

fn aead_open<'a>(key: &[u8; 32], aad: &[u8; 32], sealed: &[u8], out: &'a mut [u8]) -> WgResult<usize> {
    if sealed.len() < TAG_SIZE {
        return Err(WgError::DecryptFailed);
    }
    let (ct, tag) = sealed.split_at(sealed.len() - TAG_SIZE);
    let k = CryptoKey::from_bytes(*key);
    let n = Nonce::zero();
    let mut tag_arr = [0u8; TAG_SIZE];
    tag_arr.copy_from_slice(tag);
    aead_decrypt(&k, &n, aad, ct, &tag_arr, out).map_err(|_| WgError::DecryptFailed)?;
    Ok(ct.len())
}

fn wg_nonce(counter: u64) -> Nonce {
    let mut b = [0u8; 12];
    b[4..12].copy_from_slice(&counter.to_le_bytes());
    Nonce::from_bytes(b)
}

fn tai64n_now(unix_secs: u64) -> [u8; 12] {
    let mut out = [0u8; 12];
    out[..8].copy_from_slice(&(TAI64N_BASE + unix_secs).to_be_bytes());
    out[8..12].copy_from_slice(&0u32.to_be_bytes());
    out
}

// --- replay window (whitepaper: 2048-bit bitmap) ----------------------------

const REPLAY_WINDOW_BITS: u64 = 2048;

struct ReplayWindow {
    highest: u64,
    bitmap: [u64; (REPLAY_WINDOW_BITS / 64) as usize],
    seen_any: bool,
}

impl ReplayWindow {
    fn new() -> Self {
        Self { highest: 0, bitmap: [0; 32], seen_any: false }
    }

    /// Bit `i` of the bitmap records counter `highest - i`. On a newer
    /// counter the whole window shifts right by the difference.
    fn check_and_mark(&mut self, counter: u64) -> WgResult<()> {
        if !self.seen_any {
            self.seen_any = true;
            self.highest = counter;
            self.bitmap[0] = 1;
            return Ok(());
        }
        if counter > self.highest {
            let shift = counter - self.highest;
            if shift >= REPLAY_WINDOW_BITS {
                self.bitmap = [0; 32];
            } else {
                let old = self.bitmap;
                self.bitmap = [0; 32];
                let word_shift = (shift / 64) as usize;
                let bit_shift = (shift % 64) as u32;
                for i in 0..32 {
                    let dst = i + word_shift;
                    if dst >= 32 {
                        break;
                    }
                    self.bitmap[dst] |= old[i] << bit_shift;
                    if bit_shift != 0 && dst + 1 < 32 {
                        self.bitmap[dst + 1] |= old[i] >> (64 - bit_shift);
                    }
                }
            }
            self.highest = counter;
            self.bitmap[0] |= 1;
            return Ok(());
        }
        let diff = self.highest - counter;
        if diff >= REPLAY_WINDOW_BITS {
            return Err(WgError::Replay);
        }
        let word = (diff / 64) as usize;
        let bit = diff % 64;
        if self.bitmap[word] & (1u64 << bit) != 0 {
            return Err(WgError::Replay);
        }
        self.bitmap[word] |= 1u64 << bit;
        Ok(())
    }
}

// --- device / peer / session ------------------------------------------------

/// A live transport session for one peer.
struct Session {
    local_index: u32,
    remote_index: u32,
    send_key: [u8; 32],
    recv_key: [u8; 32],
    send_counter: u64,
    recv_window: ReplayWindow,
}

/// State of an initiation WE sent and are awaiting a response for.
struct PendingInit {
    local_index: u32,
    e_priv: [u8; 32],
    ck: [u8; 32],
    h: [u8; 32],
}

pub struct Peer {
    pub public_key: [u8; 32],
    pub psk: [u8; 32],
    /// UDP endpoint (IPv4 + port), if known/static.
    pub endpoint: Option<([u8; 4], u16)>,
    /// Allowed tunnel IPs (informational for S1; routing is caller's job).
    pub allowed_ips: [u8; 4],
    session: Option<Session>,
    pending: Option<PendingInit>,
    last_tai64n: [u8; 12],
}

/// Maximum peers per device. A SemOS node in a small tailnet needs a
/// handful; linear scans over this are cheap. Bump when needed.
pub const MAX_PEERS: usize = 8;

/// One WireGuard device: a static keypair plus a fixed-size peer table.
pub struct Device {
    private_key: [u8; 32],
    public_key: [u8; 32],
    pub peers: [Option<Peer>; MAX_PEERS],
}

impl Device {
    pub fn new(private_key: [u8; 32]) -> Self {
        let public_key = x25519_base(&private_key);
        Self { private_key, public_key, peers: Default::default() }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    pub fn add_peer(
        &mut self,
        public_key: [u8; 32],
        psk: Option<[u8; 32]>,
        endpoint: Option<([u8; 4], u16)>,
        allowed_ips: [u8; 4],
    ) -> WgResult<usize> {
        let idx = self.peers.iter().position(|p| p.is_none()).ok_or(WgError::NoRoom)?;
        self.peers[idx] = Some(Peer {
            public_key,
            psk: psk.unwrap_or([0u8; 32]),
            endpoint,
            allowed_ips,
            session: None,
            pending: None,
            last_tai64n: [0u8; 12],
        });
        Ok(idx)
    }

    pub fn peer_count(&self) -> usize {
        self.peers.iter().filter(|p| p.is_some()).count()
    }

    fn peer_mut(&mut self, idx: usize) -> WgResult<&mut Peer> {
        self.peers.get_mut(idx).and_then(|p| p.as_mut()).ok_or(WgError::UnknownPeer)
    }

    pub fn is_established(&self, peer_idx: usize) -> bool {
        matches!(self.peers.get(peer_idx), Some(Some(p)) if p.session.is_some())
    }

    /// Find the peer that owns this receiver/sender index — either a live
    /// session's local index or a pending initiation's index.
    fn lookup_index(&self, index: u32) -> Option<usize> {
        self.peers.iter().position(|p| {
            matches!(p, Some(p) if
                p.session.as_ref().map(|s| s.local_index == index).unwrap_or(false)
                || p.pending.as_ref().map(|s| s.local_index == index).unwrap_or(false))
        })
    }

    /// Build a handshake initiation for `peer_idx` into `out` (148 bytes).
    pub fn create_initiation(
        &mut self,
        peer_idx: usize,
        rand: &mut dyn FnMut(&mut [u8]),
        unix_secs: u64,
        out: &mut [u8; INITIATION_LEN],
    ) -> WgResult<()> {
        let our_static_pub = self.public_key;
        let our_static_priv = self.private_key;
        let peer = self.peer_mut(peer_idx)?;

        let mut ck = initial_ck();
        let mut h = initial_h(&ck);
        h = mix_hash(&h, &peer.public_key);

        let mut e_priv = [0u8; 32];
        rand(&mut e_priv);
        let e_pub = x25519_base(&e_priv);

        let mut local_index = [0u8; 4];
        rand(&mut local_index);
        let local_index = u32::from_le_bytes(local_index);

        out[0..4].copy_from_slice(&MSG_HANDSHAKE_INITIATION.to_le_bytes());
        out[4..8].copy_from_slice(&local_index.to_le_bytes());
        out[8..40].copy_from_slice(&e_pub);

        ck = kdf1(&ck, &e_pub);
        h = mix_hash(&h, &e_pub);

        let (new_ck, k) = kdf2(&ck, &x25519(&e_priv, &peer.public_key));
        ck = new_ck;
        let mut static_enc = [0u8; 48];
        aead_seal(&k, &h, &our_static_pub, &mut static_enc);
        out[40..88].copy_from_slice(&static_enc);
        h = mix_hash(&h, &static_enc);

        let (new_ck, k) = kdf2(&ck, &x25519(&our_static_priv, &peer.public_key));
        ck = new_ck;
        let ts = tai64n_now(unix_secs);
        let mut ts_enc = [0u8; 28];
        aead_seal(&k, &h, &ts, &mut ts_enc);
        out[88..116].copy_from_slice(&ts_enc);
        h = mix_hash(&h, &ts_enc);

        let mac1 = compute_mac1(&mac1_key(&peer.public_key), &out[..116]);
        out[116..132].copy_from_slice(&mac1);
        out[132..148].copy_from_slice(&[0u8; 16]); // mac2: no cookie yet

        peer.pending = Some(PendingInit { local_index, e_priv, ck, h });
        Ok(())
    }

    /// Handle an incoming handshake initiation (type 1). On success writes the
    /// 92-byte response into `out` and returns the new peer's table index.
    pub fn consume_initiation(
        &mut self,
        msg: &[u8],
        rand: &mut dyn FnMut(&mut [u8]),
        out: &mut [u8; RESPONSE_LEN],
    ) -> WgResult<usize> {
        if msg.len() != INITIATION_LEN {
            return Err(WgError::BadLength);
        }
        // Cheap anti-DoS first: mac1 must verify against OUR static key.
        let mac1_ok = compute_mac1(&mac1_key(&self.public_key), &msg[..116]);
        if !crate::crypto::ct_eq(&mac1_ok, &msg[116..132]) {
            return Err(WgError::BadMac1);
        }

        let their_index = u32::from_le_bytes([msg[4], msg[5], msg[6], msg[7]]);
        let mut e_pub_them = [0u8; 32];
        e_pub_them.copy_from_slice(&msg[8..40]);

        let mut ck = initial_ck();
        let mut h = initial_h(&ck);
        h = mix_hash(&h, &self.public_key);

        ck = kdf1(&ck, &e_pub_them);
        h = mix_hash(&h, &e_pub_them);

        let (new_ck, k) = kdf2(&ck, &x25519(&self.private_key, &e_pub_them));
        ck = new_ck;
        let mut their_static = [0u8; 32];
        aead_open(&k, &h, &msg[40..88], &mut their_static)?;
        let static_enc = &msg[40..88];
        h = mix_hash(&h, static_enc);

        let (new_ck, k) = kdf2(&ck, &x25519(&self.private_key, &their_static));
        ck = new_ck;
        let mut their_ts = [0u8; 12];
        aead_open(&k, &h, &msg[88..116], &mut their_ts)?;
        let ts_enc = &msg[88..116];
        h = mix_hash(&h, ts_enc);

        // Who is this?
        let peer_idx = self
            .peers
            .iter()
            .position(|p| matches!(p, Some(p) if crate::crypto::ct_eq(&p.public_key, &their_static)))
            .ok_or(WgError::UnknownPeer)?;
        let peer = self.peer_mut(peer_idx)?;

        // Timestamp must be fresh for this peer (replay protection).
        if their_ts <= peer.last_tai64n {
            return Err(WgError::StaleTimestamp);
        }
        peer.last_tai64n = their_ts;

        // --- build the response ---
        let mut e_priv = [0u8; 32];
        rand(&mut e_priv);
        let e_pub = x25519_base(&e_priv);
        let mut our_index = [0u8; 4];
        rand(&mut our_index);
        let our_index = u32::from_le_bytes(our_index);

        out[0..4].copy_from_slice(&MSG_HANDSHAKE_RESPONSE.to_le_bytes());
        out[4..8].copy_from_slice(&our_index.to_le_bytes());
        out[8..12].copy_from_slice(&their_index.to_le_bytes());
        out[12..44].copy_from_slice(&e_pub);

        ck = kdf1(&ck, &e_pub);
        h = mix_hash(&h, &e_pub);

        // ee: DH(our new ephemeral, their ephemeral)
        let (new_ck, _k) = kdf2(&ck, &x25519(&e_priv, &e_pub_them));
        ck = new_ck;
        // se: DH(our ephemeral, their static)
        let (new_ck, _k) = kdf2(&ck, &x25519(&e_priv, &their_static));
        ck = new_ck;
        let (new_ck, tau, k) = kdf3(&ck, &peer.psk);
        ck = new_ck;
        h = mix_hash(&h, &tau);

        let mut empty_enc = [0u8; 16];
        aead_seal(&k, &h, &[], &mut empty_enc);
        out[44..60].copy_from_slice(&empty_enc);
        h = mix_hash(&h, &empty_enc);

        let mac1 = compute_mac1(&mac1_key(&their_static), &out[..60]);
        out[60..76].copy_from_slice(&mac1);
        out[76..92].copy_from_slice(&[0u8; 16]);

        // Session keys: responder sends with T2... KDF2(ck, "") → (recv, send)
        let (t1, t2) = kdf2(&ck, &[]);
        peer.session = Some(Session {
            local_index: our_index,
            remote_index: their_index,
            send_key: t2,
            recv_key: t1,
            send_counter: 0,
            recv_window: ReplayWindow::new(),
        });
        Ok(peer_idx)
    }

    /// Handle an incoming handshake response (type 2) for an initiation we
    /// sent. Establishes the session; returns the peer index.
    pub fn consume_response(&mut self, msg: &[u8]) -> WgResult<usize> {
        if msg.len() != RESPONSE_LEN {
            return Err(WgError::BadLength);
        }
        let our_index = u32::from_le_bytes([msg[8], msg[9], msg[10], msg[11]]);
        // Verify mac1 (keyed to OUR static pubkey) before touching state.
        let mac1_ok = compute_mac1(&mac1_key(&self.public_key), &msg[..60]);
        if !crate::crypto::ct_eq(&mac1_ok, &msg[60..76]) {
            return Err(WgError::BadMac1);
        }
        let our_static_priv = self.private_key;
        let peer_idx = self.lookup_index(our_index).ok_or(WgError::UnknownIndex)?;
        let peer = self.peer_mut(peer_idx)?;
        let pending = peer.pending.take().ok_or(WgError::NoPendingHandshake)?;

        let their_index = u32::from_le_bytes([msg[4], msg[5], msg[6], msg[7]]);
        let mut e_pub_them = [0u8; 32];
        e_pub_them.copy_from_slice(&msg[12..44]);

        let mut ck = pending.ck;
        let mut h = pending.h;

        ck = kdf1(&ck, &e_pub_them);
        h = mix_hash(&h, &e_pub_them);

        // ee: DH(our ephemeral, their ephemeral)
        let (new_ck, _k) = kdf2(&ck, &x25519(&pending.e_priv, &e_pub_them));
        ck = new_ck;
        // se: DH(our static, their ephemeral) — NOT their static!
        let (new_ck, _k) = kdf2(&ck, &x25519(&our_static_priv, &e_pub_them));
        ck = new_ck;
        let (new_ck, tau, k) = kdf3(&ck, &peer.psk);
        ck = new_ck;
        h = mix_hash(&h, &tau);

        let mut empty = [0u8; 0];
        aead_open(&k, &h, &msg[44..60], &mut empty)?;
        let empty_enc = &msg[44..60];
        let _h_final = mix_hash(&h, empty_enc);

        // Initiator sends with T1, receives with T2.
        let (t1, t2) = kdf2(&ck, &[]);
        peer.session = Some(Session {
            local_index: our_index,
            remote_index: their_index,
            send_key: t1,
            recv_key: t2,
            send_counter: 0,
            recv_window: ReplayWindow::new(),
        });
        Ok(peer_idx)
    }

    /// Encrypt a tunnel packet for an established peer. Writes the full
    /// transport message (header + ciphertext + tag) into `out`.
    pub fn encrypt_transport(&mut self, peer_idx: usize, plaintext: &[u8], out: &mut [u8]) -> WgResult<usize> {
        let peer = self.peer_mut(peer_idx)?;
        let session = peer.session.as_mut().ok_or(WgError::NoSession)?;
        if out.len() < TRANSPORT_HEADER_LEN + plaintext.len() + TAG_SIZE {
            return Err(WgError::BufferTooSmall);
        }
        let counter = session.send_counter;
        session.send_counter += 1;

        out[0..4].copy_from_slice(&MSG_TRANSPORT.to_le_bytes());
        out[4..8].copy_from_slice(&session.remote_index.to_le_bytes());
        out[8..16].copy_from_slice(&counter.to_le_bytes());

        let key = CryptoKey::from_bytes(session.send_key);
        let nonce = wg_nonce(counter);
        let mut tag = [0u8; TAG_SIZE];
        let ct = &mut out[TRANSPORT_HEADER_LEN..TRANSPORT_HEADER_LEN + plaintext.len()];
        aead_encrypt(&key, &nonce, &[], plaintext, ct, &mut tag).map_err(|_| WgError::BufferTooSmall)?;
        out[TRANSPORT_HEADER_LEN + plaintext.len()..TRANSPORT_HEADER_LEN + plaintext.len() + TAG_SIZE]
            .copy_from_slice(&tag);
        Ok(TRANSPORT_HEADER_LEN + plaintext.len() + TAG_SIZE)
    }

    /// Decrypt an inbound transport message. Returns (peer_idx, plaintext_len).
    pub fn decrypt_transport(&mut self, msg: &[u8], out: &mut [u8]) -> WgResult<(usize, usize)> {
        if msg.len() < TRANSPORT_HEADER_LEN + TAG_SIZE {
            return Err(WgError::BadLength);
        }
        let receiver = u32::from_le_bytes([msg[4], msg[5], msg[6], msg[7]]);
        let counter = u64::from_le_bytes([
            msg[8], msg[9], msg[10], msg[11], msg[12], msg[13], msg[14], msg[15],
        ]);
        let peer_idx = self.lookup_index(receiver).ok_or(WgError::UnknownIndex)?;
        let peer = self.peer_mut(peer_idx)?;
        let session = peer.session.as_mut().ok_or(WgError::NoSession)?;
        if session.local_index != receiver {
            return Err(WgError::UnknownIndex);
        }

        session.recv_window.check_and_mark(counter)?;

        let sealed = &msg[TRANSPORT_HEADER_LEN..];
        let (ct, tag) = sealed.split_at(sealed.len() - TAG_SIZE);
        let key = CryptoKey::from_bytes(session.recv_key);
        let nonce = wg_nonce(counter);
        let mut tag_arr = [0u8; TAG_SIZE];
        tag_arr.copy_from_slice(tag);
        aead_decrypt(&key, &nonce, &[], ct, &tag_arr, out).map_err(|_| WgError::DecryptFailed)?;
        Ok((peer_idx, ct.len()))
    }

    /// Dispatch on the wire message type. `rand`/`unix_secs` are only needed
    /// for type 1 (we may build a response). Returns what happened.
    pub fn handle_message(
        &mut self,
        msg: &[u8],
        rand: &mut dyn FnMut(&mut [u8]),
        resp_out: &mut [u8; RESPONSE_LEN],
        plain_out: &mut [u8],
    ) -> WgResult<WgEvent> {
        if msg.len() < 4 {
            return Err(WgError::BadLength);
        }
        let ty = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]);
        match ty {
            MSG_HANDSHAKE_INITIATION => {
                let peer = self.consume_initiation(msg, rand, resp_out)?;
                Ok(WgEvent::SendResponse { peer_idx: peer })
            }
            MSG_HANDSHAKE_RESPONSE => {
                let peer = self.consume_response(msg)?;
                Ok(WgEvent::Established { peer_idx: peer, initiator: true })
            }
            MSG_TRANSPORT => {
                let (peer, len) = self.decrypt_transport(msg, plain_out)?;
                Ok(WgEvent::Transport { peer_idx: peer, len })
            }
            MSG_COOKIE_REPLY => Ok(WgEvent::Ignored), // cookies not implemented (S1)
            _ => Err(WgError::BadMessageType),
        }
    }
}

pub enum WgEvent {
    /// Respond to the peer with the 92 bytes in `resp_out`.
    SendResponse { peer_idx: usize },
    /// Handshake complete (we initiated): transport keys are live.
    Established { peer_idx: usize, initiator: bool },
    /// Decrypted a transport message; plaintext is `len` bytes in `plain_out`.
    Transport { peer_idx: usize, len: usize },
    Ignored,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deterministic_rand(seed: u8) -> impl FnMut(&mut [u8]) {
        let mut state = seed.wrapping_add(1);
        move |buf: &mut [u8]| {
            for b in buf.iter_mut() {
                // xorshift-ish filler; deterministic per (seed, call order)
                state ^= state << 3;
                state ^= state >> 5;
                state = state.wrapping_mul(31).wrapping_add(7);
                *b = state;
            }
        }
    }

    fn two_devices() -> (Device, Device, usize, usize) {
        let dev_i = Device::new([0x11; 32]);
        let dev_r = Device::new([0x22; 32]);
        let pub_i = dev_i.public_key();
        let pub_r = dev_r.public_key();
        let mut dev_i = dev_i;
        let mut dev_r = dev_r;
        let pi = dev_i.add_peer(pub_r, None, Some(([10, 0, 2, 2], 51820)), [10, 77, 0, 1]).unwrap();
        let pr = dev_r.add_peer(pub_i, None, None, [10, 77, 0, 2]).unwrap();
        (dev_i, dev_r, pi, pr)
    }

    fn handshake(dev_i: &mut Device, dev_r: &mut Device, pi: usize, _pr: usize) {
        let mut init = [0u8; INITIATION_LEN];
        dev_i
            .create_initiation(pi, &mut deterministic_rand(1), 1_700_000_000, &mut init)
            .unwrap();
        let mut resp = [0u8; RESPONSE_LEN];
        dev_r
            .consume_initiation(&init, &mut deterministic_rand(2), &mut resp)
            .unwrap();
        dev_i.consume_response(&resp).unwrap();
    }

    #[test]
    fn full_handshake_and_transport_both_ways() {
        let (mut dev_i, mut dev_r, pi, pr) = two_devices();
        handshake(&mut dev_i, &mut dev_r, pi, pr);
        assert!(dev_i.is_established(pi));
        assert!(dev_r.is_established(pr));

        // initiator -> responder
        let mut packet = [0u8; 256];
        let n = dev_i.encrypt_transport(pi, b"hello from initiator", &mut packet).unwrap();
        let mut plain = [0u8; 256];
        let (peer, plen) = dev_r.decrypt_transport(&packet[..n], &mut plain).unwrap();
        assert_eq!(peer, pr);
        assert_eq!(&plain[..plen], b"hello from initiator");

        // responder -> initiator
        let n = dev_r.encrypt_transport(pr, b"hello from responder", &mut packet).unwrap();
        let (peer, plen) = dev_i.decrypt_transport(&packet[..n], &mut plain).unwrap();
        assert_eq!(peer, pi);
        assert_eq!(&plain[..plen], b"hello from responder");
    }

    #[test]
    fn replay_is_rejected() {
        let (mut dev_i, mut dev_r, pi, pr) = two_devices();
        handshake(&mut dev_i, &mut dev_r, pi, pr);
        let mut packet = [0u8; 256];
        let n = dev_i.encrypt_transport(pi, b"once", &mut packet).unwrap();
        let mut plain = [0u8; 256];
        dev_r.decrypt_transport(&packet[..n], &mut plain).unwrap();
        let again = dev_r.decrypt_transport(&packet[..n], &mut plain);
        assert_eq!(again, Err(WgError::Replay));
        let _ = pr;
    }

    #[test]
    fn wrong_key_cannot_decrypt() {
        let (mut dev_i, mut dev_r, pi, _pr) = two_devices();
        handshake(&mut dev_i, &mut dev_r, pi, _pr);
        let mut packet = [0u8; 256];
        let n = dev_i.encrypt_transport(pi, b"secret", &mut packet).unwrap();
        // A third device that knows the peer's public key but shares no session.
        let mut stranger = Device::new([0x33; 32]);
        let _ = stranger.add_peer(dev_i.public_key(), None, None, [10, 77, 0, 2]);
        let mut plain = [0u8; 256];
        assert!(stranger.decrypt_transport(&packet[..n], &mut plain).is_err());
    }

    #[test]
    fn bad_mac1_is_rejected_cheaply() {
        let (mut dev_i, mut dev_r, pi, _pr) = two_devices();
        let mut init = [0u8; INITIATION_LEN];
        dev_i
            .create_initiation(pi, &mut deterministic_rand(1), 1_700_000_000, &mut init)
            .unwrap();
        init[116] ^= 0xff; // corrupt mac1
        let mut resp = [0u8; RESPONSE_LEN];
        let r = dev_r.consume_initiation(&init, &mut deterministic_rand(2), &mut resp);
        assert_eq!(r, Err(WgError::BadMac1));
    }

    #[test]
    fn stale_timestamp_rejected_on_replay_initiation() {
        let (mut dev_i, mut dev_r, pi, _pr) = two_devices();
        let mut init = [0u8; INITIATION_LEN];
        dev_i
            .create_initiation(pi, &mut deterministic_rand(1), 1_700_000_000, &mut init)
            .unwrap();
        let mut resp = [0u8; RESPONSE_LEN];
        dev_r
            .consume_initiation(&init, &mut deterministic_rand(2), &mut resp)
            .unwrap();
        // Replaying the exact same initiation must fail (same tai64n).
        let r = dev_r.consume_initiation(&init, &mut deterministic_rand(3), &mut resp);
        assert_eq!(r, Err(WgError::StaleTimestamp));
    }
}

