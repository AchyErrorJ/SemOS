//! xHCI Transfer Request Block (TRB) ring primitives.
//!
//! The xHCI controller communicates with software through three kinds of
//! ring of 16-byte TRBs:
//!
//! 1. **Command Ring** (driver → controller): we post commands here. The
//!    controller consumes them and posts a Command Completion Event back
//!    on the event ring.
//! 2. **Event Ring** (controller → driver): the controller posts events
//!    here (command completions, port status change, transfer events).
//!    We consume them by polling — no MSI/MSI-X in this driver.
//! 3. **Transfer Ring** (driver → controller, per endpoint): we post
//!    Normal/Setup/Data/Status TRBs to move data to/from an endpoint.
//!    The controller posts a Transfer Event on the event ring when done.
//!
//! All three rings live in DMA-coherent memory that is physically
//! contiguous and identity-mapped. We allocate them as `static mut`
//! `#[repr(C, align(64))]` blobs in BSS, the same pattern virtio-block
//! and virtio-net use for vring storage.
//!
//! # Cycle bit (spec §4.9.2)
//!
//! Each TRB carries a "cycle bit" (C, bit 0 of dword 3) which the
//! producer toggles on each pass through the ring to signal ownership.
//! - **Software producer** (command/transfer rings): set TRB.C = PCS,
//!   advance the enqueue pointer, flip PCS when the ring wraps.
//! - **Software consumer** (event ring): an event is valid iff its C
//!   bit matches the current Consumer Cycle State; flip CCS on wrap.
//!
//! # TRB types (spec §6.4)
//!
//! We use a small subset:
//!   - Command ring: `NoOpCommand` (4), `EnableSlot` (9), `AddressDevice`
//!     (11), `ConfigureEndpoint` (12).
//!   - Transfer ring: `Setup` (2), `Data` (3), `Status` (4), `Normal` (1).
//!   - Event ring: `TransferEvent` (32), `CommandCompletion` (33),
//!     `PortStatusChange` (34).
//!   - Link TRB (6) terminates each ring, pointing back to the head with
//!     the Toggle Cycle bit set to flip PCS.

use core::sync::atomic::{fence, Ordering};

/// Raw 16-byte TRB. Spec §4.11. All TRBs are 4 dwords; meaning depends
/// on `trb_type` (bits 15:10 of dword 3).
#[repr(C, align(16))]
#[derive(Copy, Clone, Default)]
pub struct Trb {
    pub parameter: u64,     // dwords 0+1
    pub status: u32,        // dword 2
    pub control: u32,       // dword 3 (includes C bit at bit 0, trb_type at 15:10)
}

impl Trb {
    pub const fn zero() -> Self {
        Self { parameter: 0, status: 0, control: 0 }
    }
    /// Cycle bit (bit 0 of control).
    #[inline] pub fn cycle(&self) -> bool { (self.control & 1) != 0 }
    /// TRB type (bits 15:10 of control).
    #[inline] pub fn trb_type(&self) -> u8 { ((self.control >> 10) & 0x3F) as u8 }
    /// Completion code (bits 31:24 of status) — only meaningful on event TRBs.
    #[inline] pub fn completion_code(&self) -> u8 { ((self.status >> 24) & 0xFF) as u8 }
    /// TRB Transfer Length Remaining (bits 23:0 of status) on Transfer Events —
    /// bytes the HC did NOT move (useful for short bulk reads).
    #[inline] pub fn transfer_remaining(&self) -> u32 { self.status & 0x00FF_FFFF }
    /// Slot ID (bits 31:24 of control) — only meaningful on certain event TRBs.
    #[inline] pub fn slot_id(&self) -> u8 { ((self.control >> 24) & 0xFF) as u8 }
    /// Endpoint ID (bits 21:16 of control) — only on Transfer Events.
    #[inline] pub fn endpoint_id(&self) -> u8 { ((self.control >> 16) & 0x1F) as u8 }
}

/// TRB type constants. Spec §6.4.6 Table 6-91.
pub mod trb_type {
    // Transfer ring
    pub const NORMAL: u8 = 1;
    pub const SETUP_STAGE: u8 = 2;
    pub const DATA_STAGE: u8 = 3;
    pub const STATUS_STAGE: u8 = 4;
    // Common
    pub const LINK: u8 = 6;
    pub const NO_OP_TRANSFER: u8 = 8;
    // Command ring
    pub const ENABLE_SLOT_CMD: u8 = 9;
    pub const DISABLE_SLOT_CMD: u8 = 10;
    pub const ADDRESS_DEVICE_CMD: u8 = 11;
    pub const CONFIGURE_ENDPOINT_CMD: u8 = 12;
    pub const EVALUATE_CONTEXT_CMD: u8 = 13;
    pub const RESET_ENDPOINT_CMD: u8 = 14;
    pub const NO_OP_CMD: u8 = 23;
    // Event ring
    pub const TRANSFER_EVENT: u8 = 32;
    pub const COMMAND_COMPLETION_EVENT: u8 = 33;
    pub const PORT_STATUS_CHANGE_EVENT: u8 = 34;
}

/// Completion codes we care about (status[31:24] of event TRBs).
/// Spec §6.4.5 Table 6-90.
pub mod cc {
    pub const SUCCESS: u8 = 1;
    pub const SHORT_PACKET: u8 = 13;
    pub const STALL_ERROR: u8 = 6;
}

/// Number of TRBs in our command, event, and transfer rings. 256 is
/// generous for any practical use and keeps each ring at exactly one
/// 4 KiB page (256 * 16 = 4096).
pub const RING_SIZE: usize = 256;

/// A producer ring (command ring or transfer ring). Software writes,
/// controller reads. The last entry is a Link TRB pointing back to entry
/// 0 with Toggle Cycle set, so the controller never falls off the end.
#[repr(C, align(4096))]
pub struct CommandRing {
    pub trbs: [Trb; RING_SIZE],
}

impl CommandRing {
    pub const fn new() -> Self {
        Self { trbs: [Trb::zero(); RING_SIZE] }
    }
}

/// Event Ring Segment Table entry. The xHCI uses ERST to support
/// multiple segments per event ring; we use exactly one segment of
/// `RING_SIZE` TRBs, so the ERST has a single entry. Spec §6.5.
#[repr(C, align(64))]
pub struct ErstEntry {
    /// 64-bit physical address of the event ring segment, 64-byte aligned.
    pub ring_segment_base_addr: u64,
    /// Size of the segment in TRBs (16..4096). Low 16 bits only.
    pub ring_segment_size: u32,
    pub reserved: u32,
}

impl ErstEntry {
    pub const fn zero() -> Self {
        Self { ring_segment_base_addr: 0, ring_segment_size: 0, reserved: 0 }
    }
}

/// One segment of the event ring. The controller writes, software reads.
/// The xHCI itself manages wrapping; we just bump the dequeue pointer
/// and flip our CCS when we cross the wrap.
#[repr(C, align(4096))]
pub struct EventRingSegment {
    pub trbs: [Trb; RING_SIZE],
}

impl EventRingSegment {
    pub const fn new() -> Self {
        Self { trbs: [Trb::zero(); RING_SIZE] }
    }
}

/// Producer-side bookkeeping for a command/transfer ring.
/// Tracks the enqueue index and the Producer Cycle State.
pub struct Producer {
    pub enqueue: usize,
    pub pcs: bool,
}

impl Producer {
    pub const fn new() -> Self {
        Self { enqueue: 0, pcs: true }
    }
}

/// Consumer-side bookkeeping for an event ring.
pub struct Consumer {
    pub dequeue: usize,
    pub ccs: bool,
}

impl Consumer {
    pub const fn new() -> Self {
        Self { dequeue: 0, ccs: true }
    }
}

/// Append a TRB to a producer ring.
///
/// Writes the parameter/status fields first, then writes the control
/// dword last with the cycle bit, so the controller never sees a
/// half-built TRB (the controller checks the cycle bit to determine
/// ownership; if it's still the old cycle, the TRB is "not yet built"
/// from the controller's POV).
///
/// Bumps the enqueue index. If the next slot is the Link TRB at the
/// end, also rewrites the Link TRB with the toggled PCS and flips our
/// own PCS so the controller continues fetching past the wrap.
pub unsafe fn enqueue_command(
    ring: *mut CommandRing,
    state: &mut Producer,
    parameter: u64,
    status: u32,
    control_no_cycle: u32,
) {
    let entry = &mut (*ring).trbs[state.enqueue];
    entry.parameter = parameter;
    entry.status = status;
    // Release fence so the parameter/status writes are committed before
    // the control dword (which contains the cycle bit) becomes visible.
    fence(Ordering::Release);
    entry.control = control_no_cycle | if state.pcs { 1 } else { 0 };
    state.enqueue += 1;
    // Cmd ring uses the last slot as a Link TRB. When we hit it, flip PCS
    // and rewrite the Link TRB so the controller follows it with the new
    // cycle bit.
    if state.enqueue == RING_SIZE - 1 {
        let link = &mut (*ring).trbs[RING_SIZE - 1];
        // parameter = physical address of the ring base (set up at init time);
        // already in place — we only need to update the control with the new
        // cycle bit + TC (toggle cycle) so the controller flips its own CCS.
        let trb_type = (trb_type::LINK as u32) << 10;
        let toggle_cycle = 1u32 << 1;
        let cycle = if state.pcs { 1u32 } else { 0u32 };
        fence(Ordering::Release);
        link.control = trb_type | toggle_cycle | cycle;
        state.enqueue = 0;
        state.pcs = !state.pcs;
    }
}

/// Initialise the Link TRB at the end of a command/transfer ring.
/// `ring_phys` is the physical address of the ring base.
pub unsafe fn init_command_ring(ring: *mut CommandRing, ring_phys: u64) {
    // Zero the ring (BSS already does this, but be explicit).
    for trb in (*ring).trbs.iter_mut() {
        *trb = Trb::zero();
    }
    let link = &mut (*ring).trbs[RING_SIZE - 1];
    link.parameter = ring_phys;  // points back at slot 0
    link.status = 0;
    // Type=LINK (6), TC=1 (toggle cycle on wrap), C=0 (initial PCS in producer is 1
    // and the controller starts with CCS=1, so the first time we cross the wrap
    // the TC will flip both sides to 0).
    let trb_type = (trb_type::LINK as u32) << 10;
    let toggle_cycle = 1u32 << 1;
    link.control = trb_type | toggle_cycle;
}
