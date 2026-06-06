//! iwlwifi TX/RX queue descriptors — M11 stage 3.
//!
//! Defines the DMA ring layouts for the Firmware / Frame Handler (FH).
//! These match the Linux `iwlwifi` driver's `iwl-trans.h` and
//! `iwl-fh.h` headers so metal bring-up can reuse known-good bit
//! layouts.
//!
//! QEMU-safe: pure data structures, no hardware access.

// ============================================================================
// TX queue
// ============================================================================

/// Number of TX queue entries (TFDs).  Power-of-two for cheap wrap.
/// Linux uses 256 for most queues; we keep it small for BSS.
pub const TX_QUEUE_SIZE: usize = 256;

/// A Transmit Frame Descriptor (TFD) is what the driver writes to tell
/// the NIC where the frame lives in host memory.  It contains:
///   - a command header (metadata for the ucode)
///   - a list of buffer addresses (up to 20 segments for TSO)
///
/// For v1 we model a minimal TFD with one segment (single-buffer frames).
/// Multi-segment (scatter-gather) is the follow-up when TSO lands.
#[repr(C, align(4))]
#[derive(Clone, Copy)]
pub struct TxTfd {
    /// Command type + flags.  Bit 31 = last segment, bits 30:24 = segment count.
    pub cmd: u32,
    /// Length of this segment in bytes.
    pub length: u16,
    /// Reserved / padding.
    pub _rsvd: u16,
    /// Physical address of the frame buffer (low 32 bits).
    pub addr_low: u32,
    /// Physical address of the frame buffer (high 32 bits).
    pub addr_high: u32,
}

impl TxTfd {
    pub const fn zeroed() -> Self {
        Self {
            cmd: 0,
            length: 0,
            _rsvd: 0,
            addr_low: 0,
            addr_high: 0,
        }
    }

    /// Build a TFD for a single-buffer frame.
    /// `phys` is the DMA address of the frame buffer, `len` is its length.
    pub fn single_segment(phys: u64, len: u16) -> Self {
        Self {
            cmd: (1 << 24) | (1 << 31), // 1 segment, last=1
            length: len,
            _rsvd: 0,
            addr_low: phys as u32,
            addr_high: (phys >> 32) as u32,
        }
    }
}

/// TX queue state kept by the driver.
pub struct TxQueue {
    /// Ring of TFDs — DMA-visible, page-aligned.
    pub descriptors: [TxTfd; TX_QUEUE_SIZE],
    /// Write index (driver pushes here).
    pub write_idx: u16,
    /// Read index (NIC consumes here).
    pub read_idx: u16,
    /// Queue ID (0 = command, 1..N = data).
    pub qid: u8,
    /// True if the queue has been configured in the NIC.
    pub active: bool,
}

impl TxQueue {
    pub const fn new(qid: u8) -> Self {
        Self {
            descriptors: [TxTfd::zeroed(); TX_QUEUE_SIZE],
            write_idx: 0,
            read_idx: 0,
            qid,
            active: false,
        }
    }

    /// Number of free slots in the ring.
    pub fn free_slots(&self) -> usize {
        let used = (self.write_idx.wrapping_sub(self.read_idx)) as usize;
        TX_QUEUE_SIZE - used
    }

    /// True if there are completed frames to reclaim.
    pub fn has_completed(&self) -> bool {
        self.read_idx != self.write_idx
    }
}

// ============================================================================
// RX queue
// ============================================================================

/// Number of RX buffers.  Linux uses 512; we use 128 for BSS.
pub const RX_QUEUE_SIZE: usize = 128;

/// An RX buffer descriptor.  The driver fills this with a physical address
/// of an empty buffer; the NIC DMAs the received frame into it, then writes
/// a status block.
#[repr(C, align(4))]
#[derive(Clone, Copy)]
pub struct RxBuffer {
    /// Physical address of the host buffer (low).
    pub addr_low: u32,
    /// Physical address of the host buffer (high).
    pub addr_high: u32,
}

impl RxBuffer {
    pub const fn zeroed() -> Self {
        Self {
            addr_low: 0,
            addr_high: 0,
        }
    }

    pub fn set_addr(&mut self, phys: u64) {
        self.addr_low = phys as u32;
        self.addr_high = (phys >> 32) as u32;
    }
}

/// RX queue state.
pub struct RxQueue {
    /// Ring of buffer descriptors.
    pub descriptors: [RxBuffer; RX_QUEUE_SIZE],
    /// Write index (driver posts empty buffers here).
    pub write_idx: u16,
    /// Read index (driver pulls filled buffers from here).
    pub read_idx: u16,
    /// True if the queue is configured.
    pub active: bool,
}

impl RxQueue {
    pub const fn new() -> Self {
        Self {
            descriptors: [RxBuffer::zeroed(); RX_QUEUE_SIZE],
            write_idx: 0,
            read_idx: 0,
            active: false,
        }
    }

    /// Number of buffers the driver has posted but the NIC hasn't filled.
    pub fn pending(&self) -> usize {
        (self.write_idx.wrapping_sub(self.read_idx)) as usize
    }

    /// Number of free slots to post new empty buffers.
    pub fn free_slots(&self) -> usize {
        RX_QUEUE_SIZE - self.pending()
    }
}

// ============================================================================
// Command queue (HCMD) — queue 0, driver → ucode control plane
// ============================================================================

/// Maximum size of a host command payload.  Linux uses 324 bytes.
pub const MAX_HCMD_PAYLOAD: usize = 512;

/// Host command header — every command sent to the ucode starts with this.
/// `cmd_id` selects the operation (e.g., SCAN_REQ_CMD, PHY_CONTEXT_CMD).
#[repr(C, packed)]
pub struct HcmdHeader {
    /// Command ID — see `iwlwifi` `commands.h` for the full list.
    pub cmd_id: u8,
    /// Version / flags.
    pub flags: u8,
    /// Index into the command queue (driver-managed sequence number).
    pub idx: u16,
    /// Length of the payload following this header.
    pub length: u16,
}

impl HcmdHeader {
    pub const fn new(cmd_id: u8, idx: u16, length: u16) -> Self {
        Self {
            cmd_id,
            flags: 0,
            idx,
            length,
        }
    }
}

/// Well-known command IDs (subset — expand as needed).
pub mod cmds {
    /// Initialize the ALIVE handshake.
    pub const INIT_COMPLETE: u8 = 0x01;
    /// Start a scan.
    pub const SCAN_REQ_CMD: u8 = 0x80;
    /// Abort a scan.
    pub const SCAN_ABORT_CMD: u8 = 0x81;
    /// Add / update a MAC context (STA profile).
    pub const MAC_CONTEXT_CMD: u8 = 0x28;
    /// Add / update a PHY context (channel / bandwidth).
    pub const PHY_CONTEXT_CMD: u8 = 0x08;
    /// Association / authentication (legacy path).
    pub const ASSOCIATION_CMD: u8 = 0xB1;
    /// TX power management.
    pub const TXPWR_TABLE_CMD: u8 = 0x97;
}

/// Response header — every notification / response from ucode starts with this.
#[repr(C, packed)]
pub struct RespHeader {
    /// Command ID that this is a response for.
    pub cmd_id: u8,
    /// Status / flags.
    pub flags: u8,
    /// Sequence index matching the original HCMD.
    pub idx: u16,
    /// Length of payload.
    pub length: u16,
}
