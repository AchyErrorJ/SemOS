//! iwlwifi LMAC scan command builder — `SCAN_OFFLOAD_REQUEST_CMD` (0x51).
//!
//! The 7260 with -17 firmware reports `UMAC_SCAN` capability CLEAR and the
//! NEWSCAN ucode flag SET, so it uses the *unified LMAC scan*: a single big
//! host command (`iwm_scan_req_lmac`) carrying the scan parameters, a channel
//! list, and an embedded probe-request template. The firmware scans and
//! reports beacons/probe-responses via the RX path, then a
//! `SCAN_OFFLOAD_COMPLETE` (0x6d) notification.
//!
//! Wire layout + field values cross-referenced against the OpenBSD `iwm(4)`
//! driver (`iwm_lmac_scan`, `iwm_fill_probe_req`) and Linux `iwlwifi`, which
//! target this exact firmware family. The channel array is sized at the
//! firmware's `N_SCAN_CHANNELS` (TLV 31 = 40 on this blob) because the probe
//! template sits at a fixed offset *past all 40 slots*, filled or not.

use super::MacAddr;

/// Host command IDs.
pub const SCAN_OFFLOAD_REQUEST_CMD: u8 = 0x51;
pub const SCAN_OFFLOAD_COMPLETE: u8 = 0x6d; // scan-done notification
pub const SCAN_ITERATION_COMPLETE: u8 = 0xe7;

// Structure sizes (bytes), from the iwm headers.
const N_SCAN_CHANNELS: usize = 40; // fw TLV 31 (IWM_UCODE_TLV_N_SCAN_CHANNELS)
const CHAN_CFG: usize = 12; // sizeof(iwm_scan_channel_cfg_lmac)
const FIXED_LEN: usize = 764; // sizeof(iwm_scan_req_lmac) fixed part
const PROBE_V1_LEN: usize = 528; // sizeof(iwm_scan_probe_req_v1)

/// Total LMAC scan command payload length.
pub const SCAN_CMD_LEN: usize = FIXED_LEN + N_SCAN_CHANNELS * CHAN_CFG + PROBE_V1_LEN;

// scan_flags (enum iwm_lmac_scan_flags)
const FLAG_PASS_ALL: u32 = 1 << 0;
const FLAG_PASSIVE: u32 = 1 << 1;
const FLAG_ITER_COMPLETE: u32 = 1 << 3;
const FLAG_EXTENDED_DWELL: u32 = 1 << 7;

// channel cfg flags
const UNIFIED_SCAN_CHANNEL_PARTIAL: u32 = 1 << 28;

fn put16(o: &mut [u8], off: usize, v: u16) {
    o[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(o: &mut [u8], off: usize, v: u32) {
    o[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// Build a PASSIVE LMAC scan over the first `channels` 2.4 GHz channels into
/// `out`. Passive means the firmware only listens for beacons (no probe is
/// transmitted), so this needs no aux-station / active-scan setup. Returns the
/// command payload length (`SCAN_CMD_LEN`). `out` must be >= `SCAN_CMD_LEN`.
pub fn build_passive_scan(out: &mut [u8], sta: &MacAddr, channels: u8) -> usize {
    for b in out[..SCAN_CMD_LEN].iter_mut() {
        *b = 0;
    }

    // --- fixed part (SCAN_REQUEST_FIXED_PART_API_S_VER_7) ---
    // off 0: reserved1 (already 0)
    out[4] = channels; // n_channels
    out[5] = 10; // active_dwell  (unused for passive, kept for validity)
    out[6] = 150; // passive_dwell — longer so each channel reliably catches a beacon
    out[7] = 44; // fragmented_dwell
    out[8] = 90; // extended_dwell
    // off 9: reserved2
    put16(out, 10, 0x01B7); // rx_chain_select: rx_ant=0x3 → valid|force_sel|force_mimo|driver_force
    put32(out, 12, FLAG_PASS_ALL | FLAG_ITER_COMPLETE | FLAG_EXTENDED_DWELL | FLAG_PASSIVE);
    // off 16: max_out_time = 0, off 20: suspend_time = 0
    put32(out, 24, 1); // flags = IWM_PHY_BAND_24
    put32(out, 28, (1 << 2) | (1 << 6)); // filter: ACCEPT_GRP | IN_BEACON

    // tx_cmd[0] @32 (2.4 GHz) — present for command validity even in passive.
    put32(out, 32, (1 << 13) | (1 << 12)); // tx_flags: SEQ_CTL | BT_DIS
    put32(out, 36, 10 | (1 << 9) | (1 << 14)); // rate_n_flags: 1M_PLCP | CCK | ant A
    out[40] = 1; // sta_id = IWM_AUX_STA_ID
    // tx_cmd[1] @44 (5 GHz)
    put32(out, 44, (1 << 13) | (1 << 12));
    put32(out, 48, 13 | (1 << 14)); // 6M_PLCP | ant A
    out[52] = 1;

    // direct_scan[20] @56..736 — zero (no directed SSID for a passive scan).
    put32(out, 736, 2); // scan_prio = IWM_SCAN_PRIORITY_HIGH
    put32(out, 740, 1); // iter_num = 1
    // off 744: delay = 0
    // schedule[0] @748 (iwm_scan_schedule_lmac: u16 delay, u8 iterations,
    // u8 full_scan_mul). CRITICAL: the firmware runs schedule[0].iterations
    // scan passes — leaving it 0 means the scan is accepted but never runs.
    out[750] = 3; // schedule[0].iterations — 3 full passes to reliably catch APs
    out[751] = 1; // schedule[0].full_scan_mul
    // off 752: schedule[1] = 0; off 756: channel_opt[2] = 0

    // --- data[]: channel cfg array (sized at N_SCAN_CHANNELS, fill `channels`) ---
    let data = FIXED_LEN;
    let n = channels as usize;
    for i in 0..n {
        let c = data + i * CHAN_CFG;
        put32(out, c, UNIFIED_SCAN_CHANNEL_PARTIAL);
        put16(out, c + 4, (i as u16) + 1); // channel_num 1..=channels
        put16(out, c + 6, 1); // iter_count
        // c+8: iter_interval = 0
    }

    // --- probe template (iwm_scan_probe_req_v1) at data + 40*12 ---
    let preq = data + N_SCAN_CHANNELS * CHAN_CFG;
    // segments: mac_header(4) band_data[2](8) common_data(4), then buf[512]
    let buf = preq + 16;

    // 802.11 probe-request MAC header (24 bytes).
    out[buf] = 0x40; // fc0: mgmt, subtype probe-req
    // fc1=0 (DIR_NODS); dur @buf+2 = 0
    out[buf + 4..buf + 10].copy_from_slice(&[0xFF; 6]); // addr1 = broadcast
    out[buf + 10..buf + 16].copy_from_slice(sta); // addr2 = our MAC
    out[buf + 16..buf + 22].copy_from_slice(&[0xFF; 6]); // addr3 = broadcast
    // seq @buf+22 = 0 (HW fills)
    let mut frm = buf + 24;
    // SSID IE (empty — HW inserts the SSID for directed scans).
    out[frm] = 0x00; // EID SSID
    out[frm + 1] = 0x00; // len 0
    frm += 2;
    let mac_header_len = (frm - buf) as u16; // 26

    // Supported-rates IE (2.4 GHz). band_data[0] points here.
    let band0_off = (frm - buf) as u16;
    out[frm] = 0x01; // EID supported rates
    let rates: [u8; 8] = [0x82, 0x84, 0x8B, 0x96, 0x0C, 0x12, 0x18, 0x24];
    out[frm + 1] = rates.len() as u8;
    out[frm + 2..frm + 2 + rates.len()].copy_from_slice(&rates);
    frm += 2 + rates.len();
    let band0_len = (frm - buf) as u16 - band0_off;
    let common_off = (frm - buf) as u16;

    // Probe segment descriptors (offsets are relative to buf).
    put16(out, preq, 0); // mac_header.offset
    put16(out, preq + 2, mac_header_len); // mac_header.len
    put16(out, preq + 4, band0_off); // band_data[0].offset
    put16(out, preq + 6, band0_len); // band_data[0].len
    // preq+8: band_data[1] = 0 (no 5 GHz)
    put16(out, preq + 12, common_off); // common_data.offset
    put16(out, preq + 14, 0); // common_data.len (no HT/VHT in this minimal probe)

    SCAN_CMD_LEN
}
