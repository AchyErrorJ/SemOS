//! Reference test-vector generator for SemOS Pairing Protocol v1.
//!
//! Pins the inputs from `docs/pairing-v1.md` §8, runs the exact handshake
//! math that the kernel will use, and writes a Markdown file of vectors for
//! the Swift CryptoKit implementation to reproduce.

use std::fs;
use std::path::Path;

const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

fn hex_encode(data: impl AsRef<[u8]>) -> String {
    let data = data.as_ref();
    let mut out = String::with_capacity(data.len() * 2);
    for &b in data {
        out.push(HEX_CHARS[(b >> 4) as usize] as char);
        out.push(HEX_CHARS[(b & 0xF) as usize] as char);
    }
    out
}

use kernel_core::crypto::{
    crockford, crc16,
    hkdf,
    sha256::{self, OUTPUT_SIZE},
    x25519,
};

const VERSION: u8 = 0x01;

fn main() {
    // Pinned test-vector inputs from docs/pairing-v1.md §8.
    let phone_priv = {
        let mut k = [0u8; 32];
        k[0] = 1;
        k
    };
    let sem_priv = {
        let mut k = [0u8; 32];
        k[0] = 2;
        k
    };
    let nonce_p = [0x50u8; 16];
    let nonce_s = [0x53u8; 16];

    // Public keys.
    let phone_pub = x25519::x25519_base(&phone_priv);
    let sem_pub = x25519::x25519_base(&sem_priv);

    // Shared secrets (must agree).
    let shared_from_phone = x25519::x25519(&phone_priv, &sem_pub);
    let shared_from_sem = x25519::x25519(&sem_priv, &phone_pub);
    assert_eq!(shared_from_phone, shared_from_sem, "X25519 shared secret mismatch");
    let shared = shared_from_sem;

    // Transcript and transcript hash.
    let mut transcript = Vec::new();
    transcript.extend_from_slice(b"SPv1");
    transcript.push(VERSION);
    transcript.extend_from_slice(&phone_pub);
    transcript.extend_from_slice(&nonce_p);
    transcript.extend_from_slice(&sem_pub);
    transcript.extend_from_slice(&nonce_s);
    let th = sha256::hash(&transcript);

    // HKDF key derivation.
    let mut salt = Vec::with_capacity(32);
    salt.extend_from_slice(&nonce_p);
    salt.extend_from_slice(&nonce_s);
    let prk = hkdf::extract(&salt, &shared);

    let mut session_key = [0u8; OUTPUT_SIZE];
    let mut session_info = b"semos-pair-v1 session".to_vec();
    session_info.extend_from_slice(&th);
    hkdf::expand(&prk, &session_info, &mut session_key);

    let mut sas_bytes = [0u8; 4];
    let mut sas_info = b"semos-pair-v1 sas".to_vec();
    sas_info.extend_from_slice(&th);
    hkdf::expand(&prk, &sas_info, &mut sas_bytes);

    // SAS: interpret 4 bytes as big-endian u32, mod 1_000_000, 6-digit zero-padded.
    let sas_number = u32::from_be_bytes(sas_bytes) % 1_000_000;
    let sas_display = format!("{:06}", sas_number);

    // Pairing id: first 8 bytes of SHA256("semos-pair-id" || phone_pub || sem_pub), hex.
    let mut pair_id_input = b"semos-pair-id".to_vec();
    pair_id_input.extend_from_slice(&phone_pub);
    pair_id_input.extend_from_slice(&sem_pub);
    let pair_id_hash = sha256::hash(&pair_id_input);
    let pairing_id = hex_encode(&pair_id_hash[..8]);

    // QR payload: magic || version || phone_pub || ip || port || nonce_p || crc16
    // Use a representative local-network address for the vector.
    let test_ip: [u8; 4] = [192, 168, 1, 42];
    let test_port: u16 = 8080;

    let mut qr_payload = Vec::with_capacity(59);
    qr_payload.extend_from_slice(b"SP");
    qr_payload.push(VERSION);
    qr_payload.extend_from_slice(&phone_pub);
    qr_payload.extend_from_slice(&test_ip);
    qr_payload.extend_from_slice(&test_port.to_be_bytes());
    qr_payload.extend_from_slice(&nonce_p);
    let crc = crc16::crc16_ccitt(&qr_payload);
    qr_payload.extend_from_slice(&crc.to_be_bytes());
    assert_eq!(qr_payload.len(), 59);

    // Crockford base32, hyphenated every 8 characters.
    let mut b32_raw = [0u8; 128];
    let b32_len = crockford::encode_into(&qr_payload, &mut b32_raw).expect("base32 encode");
    let b32_str = std::str::from_utf8(&b32_raw[..b32_len]).unwrap();
    let b32_grouped: String = b32_str
        .chars()
        .enumerate()
        .flat_map(|(i, c)| {
            if i > 0 && i % 8 == 0 {
                Some('-')
            } else {
                None
            }
            .into_iter()
            .chain(std::iter::once(c))
        })
        .collect();

    // Wire frames.
    // HELLO: len(u16 BE) || type=1 || version || sem_pub || nonce_s
    let mut hello_body = Vec::new();
    hello_body.push(VERSION);
    hello_body.extend_from_slice(&sem_pub);
    hello_body.extend_from_slice(&nonce_s);
    let mut hello_frame = Vec::new();
    hello_frame.extend_from_slice(&((hello_body.len() + 1) as u16).to_be_bytes());
    hello_frame.push(0x01);
    hello_frame.extend_from_slice(&hello_body);

    // ACK: len(u16 BE) || type=2 || version
    let mut ack_frame = Vec::new();
    ack_frame.extend_from_slice(&2u16.to_be_bytes());
    ack_frame.push(0x02);
    ack_frame.push(VERSION);

    // CONFIRM SemOS → phone: HMAC(session_key, "confirm-sem" || th)
    let mut sem_confirm_input = b"confirm-sem".to_vec();
    sem_confirm_input.extend_from_slice(&th);
    let sem_confirm_mac = sha256::hmac(&session_key, &sem_confirm_input);

    // CONFIRM phone → SemOS: HMAC(session_key, "confirm-phone" || th)
    let mut phone_confirm_input = b"confirm-phone".to_vec();
    phone_confirm_input.extend_from_slice(&th);
    let phone_confirm_mac = sha256::hmac(&session_key, &phone_confirm_input);

    // Build Markdown output.
    let out = format!(
        "# SemOS Pairing Protocol v1 — Reference Test Vectors\n\n\
        Generated by `tools/pairing-vectors` from the pinned inputs in `docs/pairing-v1.md` §8.\n\
        The Swift CryptoKit implementation must reproduce every value exactly.\n\n\
        > Note: the pinned private scalars `[1,0,...]` and `[2,0,...]` both clamp to\n\
        > the same effective scalar under RFC 7748, so `phone_pub == sem_pub` here.\n\
        > This is still a valid bit-identical KAT, but not a realistic independent-key\n\
        > pairing scenario.\n\n\
        ## Inputs\n\n\
        | Name | Value |\n        |---|---|\n\
        | `phone_static_priv` | `{}` |\n\
        | `sem_static_priv` | `{}` |\n\
        | `nonce_p` | `{}` |\n\
        | `nonce_s` | `{}` |\n\
        | `version` | `0x{:02X}` |\n\
        | advertised IPv4 | `{}` |\n\
        | advertised port | `{}` |\n\n\
        ## Intermediate values\n\n\
        | Name | Hex |\n        |---|---|\n\
        | `phone_pub` | `{}` |\n\
        | `sem_pub` | `{}` |\n\
        | `shared` | `{}` |\n\
        | `transcript` | `{}` |\n\
        | `th` (SHA256(transcript)) | `{}` |\n\
        | `salt` (`nonce_p || nonce_s`) | `{}` |\n\
        | `prk` (HKDF-extract) | `{}` |\n\
        | `session_key` (HKDF-expand) | `{}` |\n\
        | `sas_bytes` (HKDF-expand, 4 B) | `{}` |\n\
        | `pairing_id` (first 8 B of SHA256, hex) | `{}` |\n\n\
        ## Human-verified SAS\n\n\
        - `SAS` = **`{}`**\n\n\
        ## QR / pairing string\n\n\
        - 59-byte payload: `{}`\n\
        - Hyphenated base32: `{}`\n\n\
        ## Wire frames\n\n\
        | Frame | Bytes |\n        |---|---|\n\
        | `HELLO` (SemOS → phone) | `{}` |\n\
        | `ACK` (phone → SemOS) | `{}` |\n\
        | `CONFIRM-sem` (SemOS → phone) | `{}` |\n\
        | `CONFIRM-phone` (phone → SemOS) | `{}` |\n",
        hex_encode(phone_priv),
        hex_encode(sem_priv),
        hex_encode(nonce_p),
        hex_encode(nonce_s),
        VERSION,
        format!("{}.{}.{}.{}", test_ip[0], test_ip[1], test_ip[2], test_ip[3]),
        test_port,
        hex_encode(phone_pub),
        hex_encode(sem_pub),
        hex_encode(shared),
        hex_encode(&transcript),
        hex_encode(th),
        hex_encode(&salt),
        hex_encode(prk),
        hex_encode(session_key),
        hex_encode(sas_bytes),
        pairing_id,
        sas_display,
        hex_encode(&qr_payload),
        b32_grouped,
        hex_encode(&hello_frame),
        hex_encode(&ack_frame),
        hex_encode(sem_confirm_mac),
        hex_encode(phone_confirm_mac),
    );

    let out: String = out
        .lines()
        .map(|line| line.trim_start())
        .collect::<Vec<_>>()
        .join("\n");

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.parent().and_then(|p| p.parent()).expect("repo root");
    let path = repo_root.join("docs/pairing-v1-test-vectors.md");
    fs::write(&path, out).expect("write vectors file");
    println!("Wrote {}", path.display());
}
