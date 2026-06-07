//! CDC-NCM (USB Network Control Model) — minimal protocol layer.
//!
//! Modern iPhones (iOS 16+) and some Android devices use CDC-NCM instead of
//! CDC-ECM or ipheth for USB tethering.  The descriptor layout is similar to
//! CDC-ECM: a Communications control interface (class 0x02) with subclass
//! 0x0D (NCM) and a Data interface (class 0x0A) carrying bulk IN/OUT.
//!
//! This module mirrors `cdc_ecm.rs` but searches for the NCM subclass and
//! skips the Ethernet-specific functional descriptor (CDC-NCM does not
//! expose the MAC through a class-specific descriptor; the host typically
//! uses a hardcoded or locally-administered MAC).

// USB class / subclass / protocol IDs.
pub const CLASS_CDC_CONTROL: u8 = 0x02;
pub const SUBCLASS_NCM: u8 = 0x0D;          // Network Control Model
pub const PROTOCOL_NONE: u8 = 0x00;
pub const CLASS_CDC_DATA: u8 = 0x0A;

// Standard USB descriptor types.
pub const DESC_INTERFACE: u8 = 0x04;
pub const DESC_ENDPOINT: u8 = 0x05;

// Endpoint descriptor field bits.
pub const EP_DIR_IN: u8 = 0x80;
pub const EP_XFER_BULK: u8 = 0x02;

/// One bulk endpoint pair from the Data interface.
#[derive(Default, Clone, Copy, Debug)]
pub struct BulkEndpoints {
    pub in_addr: u8,
    pub in_mps: u16,
    pub out_addr: u8,
    pub out_mps: u16,
}

/// What we pull out of a CDC-NCM configuration descriptor blob.
#[derive(Default, Clone, Copy, Debug)]
pub struct NcmFunction {
    /// True if we found a CDC-NCM control interface.
    pub found: bool,
    /// Interface number of the Communications (control) interface.
    pub control_iface: u8,
    /// Interface number of the Data interface (where the bulk endpoints live).
    pub data_iface: u8,
    /// Alternate setting of the Data interface that exposes the bulk pair.
    pub data_alt: u8,
    /// Bulk endpoints found at `data_iface`/`data_alt`.
    pub bulk: BulkEndpoints,
    /// MTU — default to 1514 (typical Ethernet + 4-byte FCS) if not found.
    pub mtu: u16,
}

/// Walk a configuration descriptor blob and extract the CDC-NCM function.
pub fn parse_config(blob: &[u8]) -> NcmFunction {
    let mut ncm = NcmFunction {
        mtu: 1514,
        ..Default::default()
    };
    let mut in_ncm_control = false;
    let mut in_ncm_data = false;
    let mut cur_iface: u8 = 0xFF;
    let mut cur_alt: u8 = 0;

    let mut i = 0usize;
    while i + 2 <= blob.len() {
        let len = blob[i] as usize;
        let kind = blob[i + 1];
        if len < 2 || i + len > blob.len() {
            break;
        }
        let d = &blob[i..i + len];
        match kind {
            DESC_INTERFACE if len >= 9 => {
                cur_iface = d[2];
                cur_alt = d[3];
                let class = d[5];
                let sub = d[6];
                in_ncm_control = false;
                in_ncm_data = false;
                if class == CLASS_CDC_CONTROL && sub == SUBCLASS_NCM {
                    ncm.found = true;
                    ncm.control_iface = cur_iface;
                    in_ncm_control = true;
                } else if class == CLASS_CDC_DATA && ncm.found {
                    let num_eps = d[4];
                    if num_eps >= 2 {
                        ncm.data_iface = cur_iface;
                        ncm.data_alt = cur_alt;
                    }
                    in_ncm_data = num_eps >= 2 && cur_iface == ncm.data_iface;
                }
            }
            DESC_ENDPOINT if in_ncm_data && len >= 7 => {
                let addr = d[2];
                let attr = d[3] & 0x03;
                let mps = u16::from_le_bytes([d[4], d[5]]);
                if attr == EP_XFER_BULK {
                    if addr & EP_DIR_IN != 0 {
                        ncm.bulk.in_addr = addr;
                        ncm.bulk.in_mps = mps;
                    } else {
                        ncm.bulk.out_addr = addr;
                        ncm.bulk.out_mps = mps;
                    }
                }
            }
            _ => {}
        }
        i += len;
    }
    ncm
}
