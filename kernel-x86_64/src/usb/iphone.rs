//! Apple iPhone USB tethering via `ipheth` — adapted from Linux's
//! `drivers/net/usb/ipheth.c` (GPL-2.0).
//!
//! iPhone Personal Hotspot via USB does NOT use CDC-ECM. The roadmap
//! was wrong about that. iPhones expose a proprietary vendor-specific
//! interface (class 0xFF / subclass 0xFD / protocol 0x01, ipheth) at
//! alt setting 1. Once we:
//!   1. SET_CONFIGURATION,
//!   2. SET_INTERFACE on the ipheth iface number to alt=1,
//!   3. Configure its bulk IN + OUT endpoints,
//!   4. GET_MACADDR via vendor control transfer,
//!   5. Poll carrier via vendor control transfer (status 0x04 = "on"),
//! the iPhone forwards Ethernet frames over the bulk endpoints, with
//! a 2-byte alignment padding (IPHETH_IP_ALIGN) on the RX side.
//!
//! No usbmuxd / lockdownd / plist involved for tethering. The "Trust
//! This Computer?" prompt appears automatically on the iPhone the first
//! time we issue control transfers; data only starts flowing after the
//! user taps Trust.
//!
//! Reference: linux/drivers/net/usb/ipheth.c (see lines 47-76 for the
//! constant set we mirror here).

use crate::println;

/// Apple's USB vendor ID — assigned by USB-IF.
pub const APPLE_VENDOR_ID: u16 = 0x05AC;

/// ipheth (iPhone Ethernet) interface class triple. Same values as
/// Linux's `IPHETH_USBINTF_{CLASS,SUBCLASS,PROTO}`.
pub const IPHETH_CLASS: u8 = 0xFF;
pub const IPHETH_SUBCLASS: u8 = 0xFD;
pub const IPHETH_PROTOCOL: u8 = 0x01;

/// Alt setting that exposes the bulk endpoints. iPhone's alt 0 has no
/// endpoints (it's the "advertised but not usable" alt); alt 1 is
/// where the bulk IN/OUT live. Matches Linux's `IPHETH_ALT_INTFNUM`.
pub const IPHETH_ALT: u8 = 1;

/// 2-byte alignment padding on RX bulk transfers. iPhone prepends two
/// bytes so the Ethernet payload lands at an even offset. We strip
/// these before handing the frame to smoltcp.
pub const IPHETH_IP_ALIGN: usize = 2;

/// Control transfer constants (Linux ipheth.c lines 67-76).
pub const IPHETH_CMD_GET_MACADDR: u8 = 0x00;
pub const IPHETH_CMD_ENABLE_NCM: u8 = 0x04;
pub const IPHETH_CMD_CARRIER_CHECK: u8 = 0x45;

/// Carrier-on byte in a CARRIER_CHECK response.
pub const IPHETH_CARRIER_ON: u8 = 0x04;

/// Per-iPhone-slot state cached after a successful ipheth enumeration.
#[derive(Copy, Clone, Debug)]
pub struct IphoneDevice {
    pub slot_id: u8,
    /// Interface number that holds the ipheth class triple. Linux's
    /// IPHETH_INTFNUM hint is `2`, but we search rather than assume.
    pub ipheth_iface: u8,
    pub ipheth_in_ep: u8,
    pub ipheth_out_ep: u8,
    pub ipheth_in_dci: u8,
    pub ipheth_out_dci: u8,
    pub ipheth_in_mps: u16,
    pub ipheth_out_mps: u16,
    pub config_value: u8,
    /// MAC address from GET_MACADDR (vendor request 0x00).
    pub mac: [u8; 6],
    /// True once we've received any bulk-in URB. Linux flips this on
    /// first packet; mirrors the iPhone's "pairing confirmed" state.
    pub confirmed_pairing: bool,
}

static mut IPHONE: Option<IphoneDevice> = None;

/// Diagnostic: print every interface descriptor in a config blob.
pub fn dump_config_interfaces(blob: &[u8]) {
    let mut i = 0usize;
    while i + 2 <= blob.len() {
        let len = blob[i] as usize;
        if len < 2 || i + len > blob.len() { break; }
        let dtype = blob[i + 1];
        if dtype == 0x04 && len >= 9 {
            let iface = blob[i + 2];
            let alt = blob[i + 3];
            let num_ep = blob[i + 4];
            let class = blob[i + 5];
            let subclass = blob[i + 6];
            let protocol = blob[i + 7];
            println!(
                "[iphone-diag] iface={} alt={} class=0x{:02X} subclass=0x{:02X} protocol=0x{:02X} endpoints={}",
                iface, alt, class, subclass, protocol, num_ep
            );
        }
        i += len;
    }
}

/// Walk a config descriptor blob looking for the ipheth interface AT
/// ALT SETTING 1 (alt 0 has no endpoints — Apple's design). Returns
/// `(iface_num, bulk_in_addr, bulk_in_mps, bulk_out_addr, bulk_out_mps)`
/// on match. The caller is expected to SET_INTERFACE this iface to alt 1
/// before talking to the bulk endpoints.
pub fn find_ipheth_interface(blob: &[u8]) -> Option<(u8, u8, u16, u8, u16)> {
    let mut i = 0usize;
    let mut cur_match = false;
    let mut cur_iface = 0u8;
    let mut bulk_in_addr = 0u8;
    let mut bulk_in_mps = 0u16;
    let mut bulk_out_addr = 0u8;
    let mut bulk_out_mps = 0u16;
    while i + 2 <= blob.len() {
        let len = blob[i] as usize;
        if len < 2 || i + len > blob.len() { break; }
        let dtype = blob[i + 1];
        if dtype == 0x04 && len >= 9 {
            // Close out the previous interface descriptor first.
            if cur_match && bulk_in_addr != 0 && bulk_out_addr != 0 {
                return Some((cur_iface, bulk_in_addr, bulk_in_mps,
                              bulk_out_addr, bulk_out_mps));
            }
            cur_iface = blob[i + 2];
            let alt = blob[i + 3];
            let class = blob[i + 5];
            let subclass = blob[i + 6];
            let protocol = blob[i + 7];
            cur_match = alt == IPHETH_ALT
                && class == IPHETH_CLASS
                && subclass == IPHETH_SUBCLASS
                && protocol == IPHETH_PROTOCOL;
            bulk_in_addr = 0;
            bulk_out_addr = 0;
            bulk_in_mps = 0;
            bulk_out_mps = 0;
        } else if dtype == 0x05 && len >= 7 && cur_match {
            let ep_addr = blob[i + 2];
            let attrs = blob[i + 3];
            let mps = u16::from_le_bytes([blob[i + 4], blob[i + 5]]) & 0x07FF;
            let is_bulk = (attrs & 0x03) == 0x02;
            let is_in = (ep_addr & 0x80) != 0;
            if is_bulk && is_in && bulk_in_addr == 0 {
                bulk_in_addr = ep_addr;
                bulk_in_mps = mps;
            } else if is_bulk && !is_in && bulk_out_addr == 0 {
                bulk_out_addr = ep_addr;
                bulk_out_mps = mps;
            }
        }
        i += len;
    }
    if cur_match && bulk_in_addr != 0 && bulk_out_addr != 0 {
        Some((cur_iface, bulk_in_addr, bulk_in_mps,
              bulk_out_addr, bulk_out_mps))
    } else {
        None
    }
}

/// Public accessor — returns `Some` after `try_enumerate_iphone` lands.
pub fn iphone_device() -> Option<IphoneDevice> {
    unsafe { IPHONE }
}

/// Stash the enumerated iPhone state.
pub fn stash(dev: IphoneDevice) {
    unsafe { IPHONE = Some(dev); }
    println!(
        "[ipheth] cached: slot={} iface={} IN 0x{:02X} OUT 0x{:02X} MPS in/out {}/{} DCIs in/out {}/{} MAC {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        dev.slot_id, dev.ipheth_iface, dev.ipheth_in_ep, dev.ipheth_out_ep,
        dev.ipheth_in_mps, dev.ipheth_out_mps, dev.ipheth_in_dci, dev.ipheth_out_dci,
        dev.mac[0], dev.mac[1], dev.mac[2], dev.mac[3], dev.mac[4], dev.mac[5]
    );
}

/// Mark pairing as confirmed (called when the first bulk-in URB arrives).
pub fn mark_paired() {
    unsafe {
        if let Some(ref mut d) = IPHONE {
            if !d.confirmed_pairing {
                d.confirmed_pairing = true;
                println!("[ipheth] pairing confirmed — iPhone is forwarding frames");
            }
        }
    }
}
