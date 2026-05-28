//! CDC-ECM (USB Ethernet Control Model) — protocol layer (M11-prereq v1).
//!
//! The "fallback before Wi-Fi" path from the M11 plan: a USB-to-Ethernet
//! adapter speaking CDC-ECM lets the TLS stack run over real metal without
//! waiting on iwlwifi bring-up. Once a T440p is in hand (plus any cheap
//! USB-Ethernet dongle), this becomes the second-fastest path to "TLS to
//! Anthropic on metal" after wiring it into the xHCI bulk-endpoint rings.
//!
//! Scope v1: the *protocol* (class constants, configuration-descriptor walk,
//! CDC Ethernet Functional Descriptor → 48-bit MAC). Live bulk-endpoint TX/RX
//! in xHCI is the follow-up — its exact wiring depends on the host xHCI's
//! Endpoint Context layout, which we'd rather decide once with a real device.

// ============================================================================
// USB class / subclass / protocol IDs that identify a CDC-ECM function
// ============================================================================
pub const CLASS_CDC_CONTROL: u8 = 0x02;     // Interface class for Comm.
pub const SUBCLASS_ECM: u8 = 0x06;          // Ethernet Networking Control Model
pub const PROTOCOL_NONE: u8 = 0x00;
pub const CLASS_CDC_DATA: u8 = 0x0A;        // Data interface class

// Standard USB descriptor types we have to walk through.
pub const DESC_DEVICE: u8 = 0x01;
pub const DESC_CONFIG: u8 = 0x02;
pub const DESC_INTERFACE: u8 = 0x04;
pub const DESC_ENDPOINT: u8 = 0x05;
pub const DESC_CS_INTERFACE: u8 = 0x24; // class-specific (CDC functional desc.)

// CDC Functional Descriptor subtypes.
pub const CDC_FD_HEADER: u8 = 0x00;
pub const CDC_FD_UNION: u8 = 0x06;
pub const CDC_FD_ETHERNET: u8 = 0x0F; // Ethernet Networking Functional Descriptor

// Endpoint descriptor field bits.
pub const EP_DIR_IN: u8 = 0x80;
pub const EP_XFER_BULK: u8 = 0x02;
pub const EP_XFER_INTERRUPT: u8 = 0x03;

/// One bulk endpoint pair from the Data interface.
#[derive(Default, Clone, Copy, Debug)]
pub struct BulkEndpoints {
    pub in_addr: u8,
    pub in_mps: u16,
    pub out_addr: u8,
    pub out_mps: u16,
}

/// What we pull out of a CDC-ECM configuration descriptor blob.
#[derive(Default, Clone, Copy, Debug)]
pub struct EcmFunction {
    /// True if we found a CDC-ECM control interface in the blob.
    pub found: bool,
    /// Interface number of the Communications (control) interface.
    pub control_iface: u8,
    /// Interface number of the Data interface (where the bulk endpoints live).
    pub data_iface: u8,
    /// Alternate setting of the Data interface that exposes the bulk pair.
    /// Alt 0 is usually "disabled" (no endpoints); alt 1 is "active" on most
    /// devices, including QEMU's `-device usb-net`.
    pub data_alt: u8,
    /// Bulk endpoints found at `data_iface`/`data_alt`.
    pub bulk: BulkEndpoints,
    /// String-descriptor index for the MAC address (iMACAddress).
    pub i_mac: u8,
    /// MTU in bytes from wMaxSegmentSize.
    pub mtu: u16,
    /// Multicast filter count (wNumberMCFilters low 15 bits).
    pub mc_filters: u16,
}

/// Walk a configuration descriptor blob and extract the CDC-ECM function.
/// USB descriptors are `[len, type, ...]` tuples concatenated.
pub fn parse_config(blob: &[u8]) -> EcmFunction {
    let mut ecm = EcmFunction::default();
    let mut in_ecm_control = false;
    let mut in_ecm_data = false;
    let mut cur_iface: u8 = 0xFF;
    let mut cur_alt: u8 = 0;

    let mut i = 0usize;
    while i + 2 <= blob.len() {
        let len = blob[i] as usize;
        let kind = blob[i + 1];
        if len < 2 || i + len > blob.len() {
            break; // malformed — stop walking
        }
        let d = &blob[i..i + len];
        match kind {
            DESC_INTERFACE if len >= 9 => {
                // Standard Interface Descriptor:
                //   [0] bLength [1] bDescriptorType [2] bInterfaceNumber
                //   [3] bAlternateSetting [4] bNumEndpoints
                //   [5] bInterfaceClass [6] bInterfaceSubClass
                //   [7] bInterfaceProtocol [8] iInterface
                cur_iface = d[2];
                cur_alt = d[3];
                let class = d[5];
                let sub = d[6];
                in_ecm_control = false;
                in_ecm_data = false;
                if class == CLASS_CDC_CONTROL && sub == SUBCLASS_ECM {
                    ecm.found = true;
                    ecm.control_iface = cur_iface;
                    in_ecm_control = true;
                } else if class == CLASS_CDC_DATA && ecm.found {
                    // The Data interface belongs to the ECM function we found.
                    // Prefer an alt setting with bNumEndpoints >= 2.
                    let num_eps = d[4];
                    if num_eps >= 2 {
                        ecm.data_iface = cur_iface;
                        ecm.data_alt = cur_alt;
                    }
                    in_ecm_data = num_eps >= 2 && cur_iface == ecm.data_iface;
                }
            }
            DESC_CS_INTERFACE if in_ecm_control && len >= 3 => {
                let sub = d[2];
                if sub == CDC_FD_ETHERNET && len >= 13 {
                    // Ethernet Networking Functional Descriptor:
                    //   [0..2] len/type/subtype  [3] iMACAddress
                    //   [4..8] bmEthernetStatistics (u32 LE)
                    //   [8..10] wMaxSegmentSize (u16 LE)
                    //   [10..12] wNumberMCFilters (u16 LE)
                    //   [12] bNumberPowerFilters
                    ecm.i_mac = d[3];
                    ecm.mtu = u16::from_le_bytes([d[8], d[9]]);
                    ecm.mc_filters = u16::from_le_bytes([d[10], d[11]]) & 0x7FFF;
                }
            }
            DESC_ENDPOINT if in_ecm_data && len >= 7 => {
                // Endpoint Descriptor:
                //   [0..2] len/type  [2] bEndpointAddress
                //   [3] bmAttributes  [4..6] wMaxPacketSize (u16 LE)
                let addr = d[2];
                let attr = d[3] & 0x03;
                let mps = u16::from_le_bytes([d[4], d[5]]);
                if attr == EP_XFER_BULK {
                    if addr & EP_DIR_IN != 0 {
                        ecm.bulk.in_addr = addr;
                        ecm.bulk.in_mps = mps;
                    } else {
                        ecm.bulk.out_addr = addr;
                        ecm.bulk.out_mps = mps;
                    }
                }
            }
            _ => {}
        }
        i += len;
    }
    ecm
}

/// Decode a USB string descriptor (UTF-16LE inside `[len, 0x03, ...]`) that
/// holds the MAC as 12 ASCII hex digits (per CDC-ECM §5.4). Returns the
/// canonical 6-byte MAC, or None on malformed input.
pub fn parse_mac_string(string_desc: &[u8]) -> Option<[u8; 6]> {
    if string_desc.len() < 2 + 24 {
        return None;
    }
    if string_desc[1] != 0x03 {
        return None; // not a string descriptor
    }
    let mut mac = [0u8; 6];
    // 12 chars × 2 bytes (UTF-16LE) = 24 bytes after the 2-byte header.
    for i in 0..12 {
        let off = 2 + i * 2;
        let lo = string_desc[off];
        let hi = string_desc[off + 1];
        if hi != 0 {
            return None; // non-ASCII char — not a valid MAC string
        }
        let nibble = match lo {
            b'0'..=b'9' => lo - b'0',
            b'a'..=b'f' => lo - b'a' + 10,
            b'A'..=b'F' => lo - b'A' + 10,
            _ => return None,
        };
        if i % 2 == 0 {
            mac[i / 2] = nibble << 4;
        } else {
            mac[i / 2] |= nibble;
        }
    }
    Some(mac)
}
