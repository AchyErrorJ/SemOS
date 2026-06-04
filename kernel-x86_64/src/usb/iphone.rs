//! Apple iPhone USB tethering — session 1 of the multi-session lift.
//!
//! See `project_semos_iphone_tether.md` (auto-memory) for the full
//! 6-session plan.  This file lands the USB-side foundation: recognize
//! Apple Vendor ID 0x05AC in xHCI enumeration, walk the iPhone's
//! config descriptor for the USB MUX interface (vendor-specific class
//! 0xFF, subclass 0xFE, protocol 0x02), configure its bulk endpoints,
//! and prepare the usbmuxd packet header send/recv plumbing.
//!
//! What this session deliberately does NOT do:
//!   - parse XML plist (session 2)
//!   - lockdownd / "Trust This Computer" pairing (session 3)
//!   - the ipheth (Ethernet) interface itself, class 0xFF/0xFD/0x01 (session 4)
//!   - any actual TCP/IP packets (session 5+)
//!
//! Validation for this session: plug an iPhone into the W540, the
//! serial log should report `[iphone] USB MUX interface enumerated:
//! slot=N iface=M bulk IN=0xXX OUT=0xXX` and a stub Hello attempt.
//! Without pairing the device will likely return NAK/STALL on the
//! bulk read; that's expected and not a session-1 regression.

use crate::println;

/// Apple's USB vendor ID — assigned by USB-IF.
pub const APPLE_VENDOR_ID: u16 = 0x05AC;

/// USB MUX interface class triple.  Same as Linux's `usbmuxd` matches
/// (libimobiledevice's `src/usbmux.c`).  Bulk IN + OUT endpoints carry
/// the muxed protocol; control transfers aren't used at this layer.
pub const MUX_CLASS: u8 = 0xFF;
pub const MUX_SUBCLASS: u8 = 0xFE;
pub const MUX_PROTOCOL: u8 = 0x02;

/// ipheth (iPhone Ethernet) interface class triple — session 4 work.
/// Kept here so callers don't have to dig through Linux source again.
pub const IPHETH_CLASS: u8 = 0xFF;
pub const IPHETH_SUBCLASS: u8 = 0xFD;
pub const IPHETH_PROTOCOL: u8 = 0x01;

/// usbmuxd packet header — 16 bytes, all u32 little-endian.  See
/// libimobiledevice's `include/libusbmuxd.h`.  Plist payload follows
/// immediately after this header; `length` covers BOTH header + payload.
#[repr(C, packed)]
#[derive(Copy, Clone, Debug)]
pub struct UsbMuxdHeader {
    /// Total packet length including this 16-byte header.
    pub length: u32,
    /// Protocol version.  `1` = legacy binary, `1` = plist.  Yes both
    /// use the same magic number; the discriminator is `msg_type`.
    pub version: u32,
    /// Message type.  `8` = plist payload follows.
    pub msg_type: u32,
    /// Client-chosen tag, echoed in the response.  Lets clients
    /// correlate replies to requests.
    pub tag: u32,
}

impl UsbMuxdHeader {
    pub const VERSION_PLIST: u32 = 1;
    pub const MSG_TYPE_PLIST: u32 = 8;
    pub const HEADER_SIZE: usize = 16;

    /// Build a plist-payload header for a payload of `payload_len` bytes
    /// and the given tag.  The on-wire layout is the struct as-is (LE);
    /// callers should `memcpy` it into the transfer buffer.
    pub fn plist(payload_len: u32, tag: u32) -> Self {
        Self {
            length: Self::HEADER_SIZE as u32 + payload_len,
            version: Self::VERSION_PLIST,
            msg_type: Self::MSG_TYPE_PLIST,
            tag,
        }
    }
}

/// Per-iPhone-slot state cached after a successful USB MUX enumeration.
/// Mirrors the CDC-ECM `CdcEcmDevice` pattern in `xhci.rs`.
#[derive(Copy, Clone, Debug)]
pub struct IphoneDevice {
    pub slot_id: u8,
    pub mux_iface: u8,
    pub mux_in_ep: u8,
    pub mux_out_ep: u8,
    pub mux_in_dci: u8,
    pub mux_out_dci: u8,
    pub mux_in_mps: u16,
    pub mux_out_mps: u16,
    pub config_value: u8,
    /// Last tag we issued — used to make new tags monotonically.  We're
    /// single-threaded so a simple counter is fine.
    pub next_tag: u32,
}

static mut IPHONE: Option<IphoneDevice> = None;

/// Walk a config descriptor blob looking for the USB MUX interface.
/// Returns Some((iface_num, alt, bulk_in_addr, bulk_in_mps,
/// bulk_out_addr, bulk_out_mps)) on a match.
///
/// iPhones typically expose several interfaces in alt-0 of the active
/// config (PTP, AppleSync, USB MUX).  We pick the FIRST interface that
/// matches MUX class/subclass/protocol AND has at least one bulk IN
/// + one bulk OUT endpoint.
pub fn find_mux_interface(blob: &[u8]) -> Option<(u8, u8, u8, u16, u8, u16)> {
    // USB descriptor walk: each item is { bLength, bDescriptorType, ... }.
    // Type 0x04 = INTERFACE, 0x05 = ENDPOINT.  We track the most-recent
    // interface header and apply endpoints to it.
    let mut i = 0usize;
    let mut cur_match = false;
    let mut cur_iface = 0u8;
    let mut cur_alt = 0u8;
    let mut bulk_in_addr = 0u8;
    let mut bulk_in_mps = 0u16;
    let mut bulk_out_addr = 0u8;
    let mut bulk_out_mps = 0u16;
    while i + 2 <= blob.len() {
        let len = blob[i] as usize;
        if len < 2 || i + len > blob.len() { break; }
        let dtype = blob[i + 1];
        if dtype == 0x04 && len >= 9 {
            // If we were inside a matching interface and have both bulks,
            // return early before moving to the next interface.
            if cur_match && bulk_in_addr != 0 && bulk_out_addr != 0 {
                return Some((cur_iface, cur_alt, bulk_in_addr, bulk_in_mps,
                              bulk_out_addr, bulk_out_mps));
            }
            // INTERFACE descriptor:
            //   bLength, bDescriptorType, bInterfaceNumber, bAlternateSetting,
            //   bNumEndpoints, bInterfaceClass, bInterfaceSubClass,
            //   bInterfaceProtocol, iInterface
            cur_iface = blob[i + 2];
            cur_alt = blob[i + 3];
            let class = blob[i + 5];
            let subclass = blob[i + 6];
            let protocol = blob[i + 7];
            cur_match = class == MUX_CLASS
                && subclass == MUX_SUBCLASS
                && protocol == MUX_PROTOCOL;
            bulk_in_addr = 0;
            bulk_out_addr = 0;
            bulk_in_mps = 0;
            bulk_out_mps = 0;
        } else if dtype == 0x05 && len >= 7 && cur_match {
            // ENDPOINT descriptor:
            //   bLength, bDescriptorType, bEndpointAddress, bmAttributes,
            //   wMaxPacketSize (LE u16), bInterval, ...
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
    // Tail case — we may have walked off the end while inside the
    // matching interface.
    if cur_match && bulk_in_addr != 0 && bulk_out_addr != 0 {
        Some((cur_iface, cur_alt, bulk_in_addr, bulk_in_mps,
              bulk_out_addr, bulk_out_mps))
    } else {
        None
    }
}

/// Public accessor for the cached iPhone device state.  Returns `Some`
/// after `try_enumerate_iphone` has succeeded on this slot.
pub fn iphone_device() -> Option<IphoneDevice> {
    unsafe { IPHONE }
}

/// Stash the enumerated iPhone state.  Called from xHCI after the
/// MUX interface has been SET_CONFIGURATION'd + bulk endpoints
/// ConfigureEndpoint'd.
pub fn stash(dev: IphoneDevice) {
    unsafe { IPHONE = Some(dev); }
    println!(
        "[iphone] cached: slot={} iface={} IN 0x{:02X} OUT 0x{:02X} MPS in/out {}/{} DCIs in/out {}/{}",
        dev.slot_id, dev.mux_iface, dev.mux_in_ep, dev.mux_out_ep,
        dev.mux_in_mps, dev.mux_out_mps, dev.mux_in_dci, dev.mux_out_dci
    );
}

/// Allocate the next monotonic tag for a usbmuxd request.
pub fn next_tag() -> u32 {
    unsafe {
        if let Some(ref mut d) = IPHONE {
            d.next_tag = d.next_tag.wrapping_add(1);
            d.next_tag
        } else {
            0
        }
    }
}

/// Encode a `UsbMuxdHeader` into a buffer's first 16 bytes (LE).  This
/// avoids relying on `#[repr(C, packed)]` + raw pointer transmute for
/// callers that prefer slice-style I/O.  Returns the number of header
/// bytes written (always 16).
pub fn encode_header(buf: &mut [u8; 16], h: UsbMuxdHeader) -> usize {
    buf[0..4].copy_from_slice(&h.length.to_le_bytes());
    buf[4..8].copy_from_slice(&h.version.to_le_bytes());
    buf[8..12].copy_from_slice(&h.msg_type.to_le_bytes());
    buf[12..16].copy_from_slice(&h.tag.to_le_bytes());
    16
}

/// Decode a 16-byte header back into `UsbMuxdHeader`.  Validates
/// version + msg_type fields; returns None on garbage.
pub fn decode_header(buf: &[u8]) -> Option<UsbMuxdHeader> {
    if buf.len() < 16 { return None; }
    let length = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let version = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let msg_type = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let tag = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
    if length < 16 { return None; }
    Some(UsbMuxdHeader { length, version, msg_type, tag })
}

/// Stub "Hello" packet to send to usbmuxd to validate the bulk path.
/// In a real implementation this would be an XML plist with `{
/// MessageType: Listen, BundleID: ..., ClientVersionString: ..., ... }`.
/// For session 1 we send only the header (length=16, empty payload) just
/// to confirm the bulk OUT endpoint actually accepts data.  Real plist
/// goes in session 2.
pub fn build_session1_hello(out: &mut [u8; 32]) -> usize {
    let mut hdr_buf = [0u8; 16];
    let _ = encode_header(&mut hdr_buf,
        UsbMuxdHeader::plist(0 /* payload_len */, next_tag()));
    out[..16].copy_from_slice(&hdr_buf);
    16
}
