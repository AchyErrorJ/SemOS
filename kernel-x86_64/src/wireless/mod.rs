//! 802.11 protocol layer + Intel iwlwifi scaffolding (M11 v1).
//!
//! QEMU has no wireless emulation, so the iwlwifi *device* layer (firmware
//! upload + command queues + PHY init) is hardware-gated to the T440p/P1.
//! What we *can* ship now — and what we'll need on day 1 of metal — is the
//! **802.11 protocol layer**: byte-correct frame builders for the management
//! frames an STA emits during scan/auth/assoc, plus the EAPOL-Key frames for
//! the WPA2 four-way handshake. DEMO 65 validates the builders against the
//! IEEE 802.11 spec layout.
//!
//! When the T440p lands, `iwlwifi_device.rs` (TBD) wraps the firmware-upload
//! + TX/RX queues and feeds these frames into a real radio.

// ============================================================================
// iwlwifi device-ID table (scaffolding for hardware bring-up)
// ============================================================================
//
// The Intel WiFi PCI vendor ID is 0x8086 across the AX/AC families. Device IDs
// change per chip generation. These are the IDs we'll actually meet on the
// two-machine plan (T440p first, then P1 Gen 6).
pub const INTEL_VENDOR_ID: u16 = 0x8086;

/// Verbose iwlwifi bring-up trace. The full firmware-load → ALIVE → calibration
/// → MAC → scan-setup sequence prints ~50 lines + per-command `send_cmd` register
/// dumps every boot. Once it works, that's just noise before the shell. Flip to
/// `true` + rebuild to restore the trace when debugging the device. The `wifi`
/// shell command's own progress (`[wifi] ...`) and any FIRMWARE FAULT line print
/// regardless — only the routine bring-up chatter is gated.
pub const WIFI_VERBOSE: bool = false;

/// `wifidbg!(...)` — `println!` only when `WIFI_VERBOSE` is set. Used for the
/// boot bring-up trace and the routine `send_cmd` register dumps.
macro_rules! wifidbg {
    ($($arg:tt)*) => {{ if $crate::wireless::WIFI_VERBOSE { $crate::println!($($arg)*); } }};
}
pub(crate) use wifidbg;

/// (device_id, family_name) — extend as needed for other chips.
pub const IWLWIFI_DEVICES: &[(u16, &str)] = &[
    // ThinkPad T440p stage 1 — Intel Wireless 7260 family (mini-PCIe).
    (0x08B1, "Wireless 7260"),
    (0x08B2, "Wireless 7260"),
    (0x08B3, "Wireless 3160"),
    (0x08B4, "Wireless 3160"),
    // ThinkPad P1 Gen 6 stage 2 — AX211 (Wi-Fi 6E, Raptor Lake).
    (0x51F0, "Wi-Fi 6E AX211"),
    (0x51F1, "Wi-Fi 6E AX211"),
    (0x54F0, "Wi-Fi 6E AX211"),
];

/// Is this device an iwlwifi NIC we know how to drive?
pub fn is_known_iwlwifi(vendor: u16, device: u16) -> Option<&'static str> {
    if vendor != INTEL_VENDOR_ID {
        return None;
    }
    for &(d, name) in IWLWIFI_DEVICES {
        if d == device {
            return Some(name);
        }
    }
    None
}

// ============================================================================
// 802.11 frame layout
// ============================================================================
//
// All multi-byte fields are little-endian (the standard's "transmission order"
// for the over-the-air representation lines up with LE on x86).
//
// MAC header (Management/Data, 24 bytes, no QoS / no HT control):
//   off  size  field
//   0    2     Frame Control
//   2    2     Duration / ID
//   4    6     Address 1 (RA / DA)
//   10   6     Address 2 (TA / SA)
//   16   6     Address 3 (BSSID for STA-to-AP)
//   22   2     Sequence Control (frag:4, seq:12)

pub type MacAddr = [u8; 6];

/// Broadcast destination — used for Probe Request DA + Address 3.
pub const BCAST: MacAddr = [0xFF; 6];

#[repr(u8)]
pub enum FrameType {
    Management = 0,
    Control = 1,
    Data = 2,
}

/// Management subtypes we use during scan + association.
pub mod mgmt {
    pub const ASSOC_REQ: u8 = 0;
    pub const ASSOC_RESP: u8 = 1;
    pub const PROBE_REQ: u8 = 4;
    pub const PROBE_RESP: u8 = 5;
    pub const BEACON: u8 = 8;
    pub const AUTH: u8 = 11;
    pub const DEAUTH: u8 = 12;
}

/// Information Element IDs (subset).
pub mod ie {
    pub const SSID: u8 = 0;
    pub const SUPPORTED_RATES: u8 = 1;
    pub const DS_PARAMETER_SET: u8 = 3;
    pub const RSN: u8 = 48; // for WPA2
}

/// Encode the Frame Control field (2 bytes, little-endian on the wire).
/// Layout: ProtoVer(2) | Type(2) | Subtype(4) | ToDS(1) | FromDS(1) | flags...
pub fn frame_control(ftype: FrameType, subtype: u8) -> u16 {
    // ProtoVer = 0; flags all 0 for a STA-emitted mgmt frame.
    ((subtype as u16) << 4) | ((ftype as u16) << 2)
}

/// Write the 24-byte MAC header into `out`. Returns the cursor after it.
fn write_mac_header(
    out: &mut [u8],
    fc: u16,
    duration: u16,
    addr1: &MacAddr,
    addr2: &MacAddr,
    addr3: &MacAddr,
    seq_ctl: u16,
) -> usize {
    out[0..2].copy_from_slice(&fc.to_le_bytes());
    out[2..4].copy_from_slice(&duration.to_le_bytes());
    out[4..10].copy_from_slice(addr1);
    out[10..16].copy_from_slice(addr2);
    out[16..22].copy_from_slice(addr3);
    out[22..24].copy_from_slice(&seq_ctl.to_le_bytes());
    24
}

/// Append an Information Element `id len data[..]`. Returns the new cursor.
fn write_ie(out: &mut [u8], cur: usize, id: u8, data: &[u8]) -> usize {
    out[cur] = id;
    out[cur + 1] = data.len() as u8;
    let body = cur + 2;
    out[body..body + data.len()].copy_from_slice(data);
    body + data.len()
}

/// Default 802.11b/g supported-rates set (in 500 kbps units, hi bit = basic).
/// 1, 2, 5.5, 11 Mbps = 0x02, 0x04, 0x0B, 0x16 → with basic-bit set on 1/2
/// per common-practice for a STA: 0x82, 0x84, 0x8B, 0x96.
pub const DEFAULT_RATES: &[u8] = &[0x82, 0x84, 0x8B, 0x96];

/// Build a Probe Request frame into `out`. `ssid.is_empty()` → wildcard probe.
/// Returns the total frame length on success, or None if `out` is too small.
pub fn build_probe_request(out: &mut [u8], src: &MacAddr, ssid: &[u8]) -> Option<usize> {
    // Bound: header(24) + SSID IE(2 + ssid.len()) + Rates IE(2 + 8) = 36 + ssid.len()
    if out.len() < 24 + 2 + ssid.len() + 2 + DEFAULT_RATES.len() {
        return None;
    }
    let fc = frame_control(FrameType::Management, mgmt::PROBE_REQ);
    let mut cur = write_mac_header(out, fc, 0, &BCAST, src, &BCAST, 0);
    cur = write_ie(out, cur, ie::SSID, ssid);
    cur = write_ie(out, cur, ie::SUPPORTED_RATES, DEFAULT_RATES);
    Some(cur)
}

/// Build an Open System Authentication request (seq = 1, status = 0).
/// Returns total length or None on too-small buffer.
pub fn build_open_auth_request(out: &mut [u8], src: &MacAddr, bssid: &MacAddr) -> Option<usize> {
    // header(24) + auth body(6)
    if out.len() < 24 + 6 {
        return None;
    }
    let fc = frame_control(FrameType::Management, mgmt::AUTH);
    let mut cur = write_mac_header(out, fc, 0, bssid, src, bssid, 0);
    // Auth Algorithm Number (2) = 0 (Open System)
    // Auth Transaction Seq Number (2) = 1
    // Status Code (2) = 0
    out[cur..cur + 2].copy_from_slice(&0u16.to_le_bytes());
    out[cur + 2..cur + 4].copy_from_slice(&1u16.to_le_bytes());
    out[cur + 4..cur + 6].copy_from_slice(&0u16.to_le_bytes());
    cur += 6;
    Some(cur)
}

/// Build an Association Request: STA tells the AP it wants to join.
/// Capability info + listen interval + SSID IE + Supported Rates IE.
pub fn build_association_request(
    out: &mut [u8],
    src: &MacAddr,
    bssid: &MacAddr,
    ssid: &[u8],
) -> Option<usize> {
    if out.len() < 24 + 4 + 2 + ssid.len() + 2 + DEFAULT_RATES.len() {
        return None;
    }
    let fc = frame_control(FrameType::Management, mgmt::ASSOC_REQ);
    let mut cur = write_mac_header(out, fc, 0, bssid, src, bssid, 0);
    // Capability Info (2): ESS bit (0) set so the AP knows we're an infrastructure STA.
    out[cur..cur + 2].copy_from_slice(&(1u16).to_le_bytes());
    // Listen Interval (2): how often we'll wake (in beacon intervals).
    out[cur + 2..cur + 4].copy_from_slice(&(10u16).to_le_bytes());
    cur += 4;
    cur = write_ie(out, cur, ie::SSID, ssid);
    cur = write_ie(out, cur, ie::SUPPORTED_RATES, DEFAULT_RATES);
    Some(cur)
}

/// The RSN Information Element for **WPA2-PSK with CCMP** — the suite we support.
/// The STA must advertise this in its Association Request or the AP won't begin
/// the 4-way handshake (it'd treat us as an open/legacy client). IEEE 802.11i
/// RSN IE (element id 48), 20-byte value. The `00 0F AC` prefix is the IEEE OUI;
/// the trailing byte is the suite selector (04 = CCMP, 02 = PSK).
pub const RSN_IE_WPA2_PSK_CCMP: [u8; 22] = [
    48,                     // Element ID = RSN
    20,                     // Length of what follows
    0x01, 0x00,             // RSN Version = 1
    0x00, 0x0F, 0xAC, 0x04, // Group Cipher Suite      = CCMP
    0x01, 0x00,             // Pairwise Cipher count   = 1
    0x00, 0x0F, 0xAC, 0x04, // Pairwise Cipher Suite   = CCMP
    0x01, 0x00,             // AKM Suite count         = 1
    0x00, 0x0F, 0xAC, 0x02, // AKM Suite               = PSK
    0x00, 0x00,             // RSN Capabilities
];

/// Association Request for a **WPA2-PSK/CCMP** network: same as
/// `build_association_request` but appends the RSN IE so the AP starts the
/// 4-way handshake. This is the assoc frame the connect() path sends after the
/// open-auth exchange succeeds.
pub fn build_association_request_wpa2(
    out: &mut [u8],
    src: &MacAddr,
    bssid: &MacAddr,
    ssid: &[u8],
) -> Option<usize> {
    let mut cur = build_association_request(out, src, bssid, ssid)?;
    if cur + RSN_IE_WPA2_PSK_CCMP.len() > out.len() {
        return None;
    }
    out[cur..cur + RSN_IE_WPA2_PSK_CCMP.len()].copy_from_slice(&RSN_IE_WPA2_PSK_CCMP);
    cur += RSN_IE_WPA2_PSK_CCMP.len();
    Some(cur)
}

// ============================================================================
// WPA2 EAPOL-Key (four-way handshake)
// ============================================================================
//
// EAPOL frame (Ethernet payload):
//   off  size  field
//   0    1     Protocol Version (= 2)
//   1    1     Packet Type      (= 3 = EAPOL-Key)
//   2    2     Packet Body Length (big-endian, length of what follows)
//   4    1     Descriptor Type  (= 2 = WPA2 RSN Key Descriptor)
//   5    2     Key Information  (big-endian; bits select pairwise / install / ack / mic / secure)
//   7    2     Key Length       (big-endian; 0 for CCMP pairwise post-Msg2)
//   9    8     Replay Counter   (big-endian)
//   17   32    Key Nonce
//   49   16    Key IV
//   65   8     Key RSC
//   73   8     Reserved (Key ID)
//   81   16    Key MIC
//   97   2     Key Data Length  (big-endian)
//   99   N     Key Data (RSN IE, GTK KDE, etc.)
//
// The four-way handshake: AP→STA Msg1 (ANonce), STA→AP Msg2 (SNonce + MIC),
// AP→STA Msg3 (GTK + MIC), STA→AP Msg4 (ACK + MIC). We need Msg2 + Msg4 sent
// by the STA. The MIC computation needs the PTK derived from PMK + ANonce +
// SNonce + MACs — vendored separately when crypto wiring lands.

pub const EAPOL_VERSION: u8 = 2;
pub const EAPOL_TYPE_KEY: u8 = 3;
pub const EAPOL_KEY_DESC_RSN: u8 = 2;

bitflags::bitflags! {
    /// EAPOL-Key Information field bits (16-bit, transmitted big-endian).
    pub struct KeyInfo: u16 {
        const KEY_DESC_VER_AES_CMAC = 2; // WPA2/CCMP
        const KEY_TYPE_PAIRWISE = 1 << 3;
        const INSTALL           = 1 << 6;
        const ACK               = 1 << 7;
        const MIC               = 1 << 8;
        const SECURE            = 1 << 9;
        const ERROR             = 1 << 10;
        const REQUEST           = 1 << 11;
        const ENCRYPTED_KEY_DATA = 1 << 12;
    }
}

/// Build an EAPOL-Key Msg2 (STA → AP) with the given SNonce and replay counter.
/// MIC field is left zero — caller fills it after HMAC-SHA1 / AES-CMAC over
/// the frame with KCK (derived from the PTK). `key_data` is the RSN IE the
/// STA advertises (its chosen ciphers/AKM); empty for now if not configured.
pub fn build_eapol_msg2(
    out: &mut [u8],
    snonce: &[u8; 32],
    replay: u64,
    key_data: &[u8],
) -> Option<usize> {
    let body_len: usize = 1 + 2 + 2 + 8 + 32 + 16 + 8 + 8 + 16 + 2 + key_data.len();
    let total = 4 + body_len;
    if out.len() < total {
        return None;
    }
    let ki = KeyInfo::KEY_DESC_VER_AES_CMAC
        | KeyInfo::KEY_TYPE_PAIRWISE
        | KeyInfo::MIC;
    out[0] = EAPOL_VERSION;
    out[1] = EAPOL_TYPE_KEY;
    out[2..4].copy_from_slice(&(body_len as u16).to_be_bytes());
    out[4] = EAPOL_KEY_DESC_RSN;
    out[5..7].copy_from_slice(&ki.bits().to_be_bytes());
    out[7..9].copy_from_slice(&0u16.to_be_bytes()); // Key Length = 0 in Msg2
    out[9..17].copy_from_slice(&replay.to_be_bytes());
    out[17..49].copy_from_slice(snonce);
    // IV(16) + RSC(8) + Reserved(8) + MIC(16) all zero on a fresh build.
    for b in &mut out[49..49 + 16 + 8 + 8 + 16] {
        *b = 0;
    }
    out[97..99].copy_from_slice(&(key_data.len() as u16).to_be_bytes());
    out[99..99 + key_data.len()].copy_from_slice(key_data);
    Some(total)
}

/// Compute + insert the EAPOL-Key MIC (key-descriptor version 2 = HMAC-SHA1-128)
/// over a fully-built EAPOL-Key frame, in place. The MIC covers the entire frame
/// with its own 16-byte MIC field (bytes 81..97) treated as zero — so we zero it
/// first, then HMAC-SHA1(KCK, frame)[..16] back into it.
pub fn finalize_eapol_mic(frame: &mut [u8], kck: &[u8]) {
    if frame.len() < 97 {
        return;
    }
    for b in &mut frame[81..97] {
        *b = 0;
    }
    let mic = wpa2::eapol_mic(kck, frame);
    frame[81..97].copy_from_slice(&mic);
}

/// Build EAPOL-Key Msg4 (STA → AP, the handshake-completing ACK). Same layout as
/// Msg2 but: SNonce zeroed (Msg4 carries no nonce), SECURE bit set, no key data.
/// MIC left zero — caller runs `finalize_eapol_mic` with the KCK.
pub fn build_eapol_msg4(out: &mut [u8], replay: u64) -> Option<usize> {
    let body_len: usize = 1 + 2 + 2 + 8 + 32 + 16 + 8 + 8 + 16 + 2;
    let total = 4 + body_len;
    if out.len() < total {
        return None;
    }
    for b in &mut out[..total] {
        *b = 0;
    }
    let ki = KeyInfo::KEY_DESC_VER_AES_CMAC
        | KeyInfo::KEY_TYPE_PAIRWISE
        | KeyInfo::MIC
        | KeyInfo::SECURE;
    out[0] = EAPOL_VERSION;
    out[1] = EAPOL_TYPE_KEY;
    out[2..4].copy_from_slice(&(body_len as u16).to_be_bytes());
    out[4] = EAPOL_KEY_DESC_RSN;
    out[5..7].copy_from_slice(&ki.bits().to_be_bytes());
    // key_len(0) + replay + zero nonce/IV/RSC/rsvd/MIC + key_data_len(0).
    out[9..17].copy_from_slice(&replay.to_be_bytes());
    Some(total)
}

/// Parsed fields of an inbound EAPOL-Key frame (AP → STA, Msg1 or Msg3).
pub struct EapolKey<'a> {
    pub key_info: u16,
    pub replay: u64,
    /// ANonce (Msg1) or the same ANonce echoed in Msg3.
    pub nonce: [u8; 32],
    pub mic: [u8; 16],
    /// Encrypted key data (the GTK KDE in Msg3); empty in Msg1.
    pub key_data: &'a [u8],
}

impl EapolKey<'_> {
    /// True if this is Msg3 (has the MIC + KEY bits set, key data present).
    /// Msg1 has ACK set but no MIC; Msg3 has ACK+MIC+INSTALL+SECURE.
    pub fn has_mic(&self) -> bool {
        self.key_info & KeyInfo::MIC.bits() != 0
    }
}

/// Parse an inbound EAPOL-Key frame. Returns None if it is too short or not an
/// EAPOL-Key packet. Borrows the key-data slice from `frame`.
pub fn parse_eapol_key(frame: &[u8]) -> Option<EapolKey<'_>> {
    if frame.len() < 99 || frame[1] != EAPOL_TYPE_KEY {
        return None;
    }
    let key_info = u16::from_be_bytes([frame[5], frame[6]]);
    let mut replay = [0u8; 8];
    replay.copy_from_slice(&frame[9..17]);
    let mut nonce = [0u8; 32];
    nonce.copy_from_slice(&frame[17..49]);
    let mut mic = [0u8; 16];
    mic.copy_from_slice(&frame[81..97]);
    let kd_len = u16::from_be_bytes([frame[97], frame[98]]) as usize;
    let key_data = frame.get(99..99 + kd_len)?;
    Some(EapolKey {
        key_info,
        replay: u64::from_be_bytes(replay),
        nonce,
        mic,
        key_data,
    })
}

/// EAPOL handshake KAT: build Msg2 with the wpa2 PTK test vector's KCK + SNonce,
/// finalize the MIC, and check it against a Python-reference value (cross-impl,
/// so it catches frame-layout / MIC-coverage bugs a round trip would not).
pub fn eapol_self_test() -> bool {
    let kck = [
        0x4a, 0x7d, 0x0f, 0x9a, 0xd3, 0x0e, 0xf8, 0x50, 0x33, 0x15, 0x80, 0x26, 0x63, 0x82,
        0x77, 0xe3,
    ];
    let mut snonce = [0u8; 32];
    for i in 0..32 {
        snonce[i] = (0xa0 + i) as u8;
    }
    let mut frame = [0u8; 256];
    let n = match build_eapol_msg2(&mut frame, &snonce, 1, &RSN_IE_WPA2_PSK_CCMP) {
        Some(n) => n,
        None => {
            crate::println!("[wpa2] EAPOL self-test: Msg2 build FAIL");
            return false;
        }
    };
    finalize_eapol_mic(&mut frame[..n], &kck);
    let expect = [
        0xa5, 0x97, 0xf3, 0xa2, 0x18, 0xc1, 0x06, 0xbe, 0xe1, 0xde, 0x7b, 0x6f, 0x8c, 0x5f,
        0x2d, 0xfd,
    ];
    let len_ok = n == 121;
    let mic_ok = frame[81..97] == expect;
    crate::println!("[wpa2] EAPOL self-test: Msg2 len {}, MIC {}",
        if len_ok { "PASS" } else { "FAIL" },
        if mic_ok { "PASS" } else { "FAIL" });
    len_ok && mic_ok
}

// M11 device bring-up stubs (PCI probe, firmware skeleton, NetDevice wiring).
pub mod iwlwifi_pci;
pub mod iwlwifi_csr;
pub mod iwlwifi_queue;
pub mod iwlwifi_fw;
pub mod iwlwifi_fw_image;
pub mod iwlwifi_device;
pub mod iwlwifi_net;
pub mod iwlwifi_scan;
pub mod iwlwifi_sm;
pub mod wpa2;
