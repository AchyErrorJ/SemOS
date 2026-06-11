//! iwlwifi firmware image — embed + parse the TLV ucode blob.
//!
//! The 7260 ucode (`iwlwifi-7260-17.ucode`, ~1 MB) is the standard Linux
//! TLV format:
//!
//! ```text
//!   struct iwl_tlv_ucode_header {   // 88 bytes
//!     u32  zero;                    // 0x00  (0 → marks TLV format)
//!     u32  magic;                   // 0x04  = 0x0a4c5749 ("IWL\n")
//!     u8   human_readable[64];      // 0x08  version string
//!     u32  ver;                     // 0x48
//!     u32  build;                   // 0x4C
//!     u64  ignore;                  // 0x50
//!     u8   data[];                  // 0x58  TLV entries
//!   }
//!   struct iwl_ucode_tlv { u32 type; u32 length; u8 data[length]; } // padded to 4
//! ```
//!
//! For the 7260 (gen1, pre-secboot) the firmware sections are carried in
//! `SEC_INIT` (init image, used for calibration) and `SEC_RT` (runtime
//! image) TLVs. Each section's data begins with a u32 load address
//! (`offset`), followed by the section bytes. Special offsets separate
//! CPU1/CPU2 (0xFFFFFFFF) and paging blocks (0xAAAAAAAA).

use crate::println;

/// The embedded 7260 firmware blob (repo root).
pub static FW_7260: &[u8] =
    include_bytes!("../../../iwlwifi-7260-17.ucode");

const TLV_MAGIC: u32 = 0x0a4c_5749; // "IWL\n"
const HEADER_LEN: usize = 0x58;

// TLV types we care about (iwl-fw-file.h).
mod tlv {
    // Old-style (implied-address) section TLVs.
    pub const INST: u32 = 1;      // runtime instructions
    pub const DATA: u32 = 2;      // runtime data
    pub const INIT: u32 = 3;      // init instructions
    pub const INIT_DATA: u32 = 4; // init data
    // New-style (address-prefixed) section TLVs.
    pub const FLAGS: u32 = 18;
    pub const SEC_RT: u32 = 19;
    pub const SEC_INIT: u32 = 20;
    pub const PHY_SKU: u32 = 23;
    pub const NUM_OF_CPU: u32 = 27;
    pub const API_CHANGES_SET: u32 = 29;
    pub const ENABLED_CAPABILITIES: u32 = 30;
}

// 7000-series implied load addresses for old-style INST/DATA TLVs
// (IWLAGN_RTC_{INST,DATA}_LOWER_BOUND).
const RTC_INST_ADDR: u32 = 0x0000_0000;
const RTC_DATA_ADDR: u32 = 0x0080_0000;

/// Section load-address separators.
pub const CPU1_CPU2_SEPARATOR: u32 = 0xFFFF_FFFF;
pub const PAGING_SEPARATOR: u32 = 0xAAAA_AAAA;

/// One firmware section: a load address plus a slice of the blob.
#[derive(Copy, Clone)]
pub struct FwSection {
    pub addr: u32,
    /// Byte offset into `FW_7260` where the section data starts.
    pub off: usize,
    pub len: usize,
}

const MAX_SECTIONS: usize = 16;

/// One firmware image (init or runtime): an ordered list of sections.
#[derive(Copy, Clone)]
pub struct FwImage {
    pub sections: [FwSection; MAX_SECTIONS],
    pub count: usize,
}

impl FwImage {
    const fn new() -> Self {
        Self {
            sections: [FwSection { addr: 0, off: 0, len: 0 }; MAX_SECTIONS],
            count: 0,
        }
    }
    fn push(&mut self, s: FwSection) {
        if self.count < MAX_SECTIONS {
            self.sections[self.count] = s;
            self.count += 1;
        }
    }
    pub fn total_bytes(&self) -> usize {
        self.sections[..self.count].iter().map(|s| s.len).sum()
    }
}

/// Parsed firmware: the init + runtime images plus a couple of flags.
pub struct ParsedFw {
    pub init: FwImage,
    pub runtime: FwImage,
    pub num_cpus: u32,
    pub api_flags: u32,
}

fn rd32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Parse the embedded blob. Returns None if the header is not the expected
/// TLV format.
pub fn parse() -> Option<ParsedFw> {
    let b = FW_7260;
    if b.len() < HEADER_LEN {
        println!("[iwlwifi-fw] blob too small ({} bytes)", b.len());
        return None;
    }
    if rd32(b, 0) != 0 || rd32(b, 4) != TLV_MAGIC {
        println!("[iwlwifi-fw] not TLV format (zero=0x{:08X} magic=0x{:08X})",
            rd32(b, 0), rd32(b, 4));
        return None;
    }
    let ver = rd32(b, 0x48);
    // human_readable version string (NUL-terminated).
    let hr_end = b[8..0x48].iter().position(|&c| c == 0).unwrap_or(0x40) + 8;
    if let Ok(s) = core::str::from_utf8(&b[8..hr_end]) {
        println!("[iwlwifi-fw] {} ver={} ({} bytes)", s, ver, b.len());
    }

    let mut parsed = ParsedFw {
        init: FwImage::new(),
        runtime: FwImage::new(),
        num_cpus: 1,
        api_flags: 0,
    };

    let mut p = HEADER_LEN;
    while p + 8 <= b.len() {
        let ttype = rd32(b, p);
        let tlen = rd32(b, p + 4) as usize;
        let data = p + 8;
        if data + tlen > b.len() {
            println!("[iwlwifi-fw] truncated TLV type={} len={} at off={}", ttype, tlen, p);
            break;
        }
        match ttype {
            // New-style: 4-byte load address prefix.
            tlv::SEC_RT | tlv::SEC_INIT if tlen >= 4 => {
                let addr = rd32(b, data);
                let sec = FwSection { addr, off: data + 4, len: tlen - 4 };
                if ttype == tlv::SEC_RT {
                    parsed.runtime.push(sec);
                } else {
                    parsed.init.push(sec);
                }
            }
            // Old-style: implied fixed load address.
            tlv::INST => parsed.runtime.push(FwSection { addr: RTC_INST_ADDR, off: data, len: tlen }),
            tlv::DATA => parsed.runtime.push(FwSection { addr: RTC_DATA_ADDR, off: data, len: tlen }),
            tlv::INIT => parsed.init.push(FwSection { addr: RTC_INST_ADDR, off: data, len: tlen }),
            tlv::INIT_DATA => parsed.init.push(FwSection { addr: RTC_DATA_ADDR, off: data, len: tlen }),
            tlv::NUM_OF_CPU if tlen >= 4 => parsed.num_cpus = rd32(b, data),
            tlv::FLAGS if tlen >= 4 => parsed.api_flags = rd32(b, data),
            tlv::PHY_SKU | tlv::API_CHANGES_SET | tlv::ENABLED_CAPABILITIES => {}
            _ => {}
        }
        // Advance to the next TLV, padding length up to a 4-byte boundary.
        p = data + ((tlen + 3) & !3);
    }

    println!("[iwlwifi-fw] INIT image: {} section(s), {} bytes | RUNTIME image: {} section(s), {} bytes | cpus={} flags=0x{:08X}",
        parsed.init.count, parsed.init.total_bytes(),
        parsed.runtime.count, parsed.runtime.total_bytes(),
        parsed.num_cpus, parsed.api_flags);
    // Dump section load addresses so the boot log shows the layout we'll DMA.
    for (i, s) in parsed.init.sections[..parsed.init.count].iter().enumerate() {
        println!("[iwlwifi-fw]   INIT[{}] addr=0x{:08X} len={}", i, s.addr, s.len);
    }
    for (i, s) in parsed.runtime.sections[..parsed.runtime.count].iter().enumerate() {
        println!("[iwlwifi-fw]   RT[{}] addr=0x{:08X} len={}", i, s.addr, s.len);
    }

    Some(parsed)
}
