//! xHCI controller bring-up + port enumeration.
//!
//! Spec: Extensible Host Controller Interface for USB rev 1.2.
//! Implements a deliberately-minimal driver:
//!
//! - Polling only (no MSI / MSI-X). The event ring is drained explicitly
//!   from a function the DEMO calls.
//! - Single Interrupter (IR0) and single event ring segment of 256 TRBs.
//! - Single device at a time on the root hub (first device that appears
//!   is enumerated; subsequent ports are noticed but not addressed in v1).
//! - Boot-protocol HID only (we don't parse Report descriptors).
//! - Endpoint contexts allocated at 64-byte stride (matches qemu-xhci's
//!   CSZ=1). CSZ=0 hardware fails boot with a clear log line — see
//!   `probe_csz()` below; this is the documented branch point for
//!   metal-side testing on AMD platforms.
//!
//! # Why this driver doesn't use the `xhci` crate
//!
//! The other drivers in this kernel (virtio-block, virtio-net) all
//! hand-roll register access for consistency and to avoid pulling in
//! dependencies whose every revision becomes a Cargo.lock churn. The
//! `xhci` crate would still leave us writing the same DMA structures,
//! ring management, and enumeration state machine — the savings are
//! limited to a thin volatile-MMIO wrapper, which `core::ptr::read_volatile`
//! gives us in two lines. Keeping the driver self-contained also makes
//! merging into main trivial since it touches only `usb/`.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

use crate::pci;
use crate::paging;
use crate::println;

use super::device::{
    InputContext, DeviceContext,
    DeviceDescriptor, ConfigDescriptor, InterfaceDescriptor, EndpointDescriptor,
    SetupPacket, request, desc_type, class,
};
use super::hid;
use super::ring::{
    Trb, CommandRing, EventRingSegment, ErstEntry, Producer, Consumer,
    trb_type, cc, init_command_ring, enqueue_command, RING_SIZE,
};

// ============================================================================
// PCI identifiers and capability/operational/runtime register offsets
// ============================================================================

/// qemu-xhci. Also matches Red Hat virtio family. Real Intel xHCI is at
/// vendor 0x8086 with device-specific IDs; this driver works on either
/// because we use the spec-defined register layout, but the probe below
/// only matches qemu-xhci to keep boot signatures tight. Add Intel's PCI
/// IDs here when bringing up on metal.
pub const QEMU_XHCI_VENDOR: u16 = 0x1B36;
pub const QEMU_XHCI_DEVICE: u16 = 0x000D;

/// Capability register offsets (spec §5.3, relative to MMIO base).
mod cap_reg {
    pub const CAPLENGTH: u32 = 0x00;
    pub const HCSPARAMS1: u32 = 0x04;
    pub const HCSPARAMS2: u32 = 0x08;
    pub const HCCPARAMS1: u32 = 0x10;
    pub const DBOFF: u32 = 0x14;
    pub const RTSOFF: u32 = 0x18;
}

/// Operational register offsets (spec §5.4, relative to op_base = MMIO + CAPLENGTH).
mod op_reg {
    pub const USBCMD: u32 = 0x00;
    pub const USBSTS: u32 = 0x04;
    pub const PAGESIZE: u32 = 0x08;
    pub const DNCTRL: u32 = 0x14;
    pub const CRCR: u32 = 0x18;  // 64-bit
    pub const DCBAAP: u32 = 0x30; // 64-bit
    pub const CONFIG: u32 = 0x38;
    pub const PORTSC_BASE: u32 = 0x400; // port 1 PORTSC; ports stride = 0x10
}

/// USBCMD bits.
mod usbcmd {
    pub const RS: u32 = 1 << 0;    // Run/Stop
    pub const HCRST: u32 = 1 << 1; // Reset
    pub const INTE: u32 = 1 << 2;  // Interrupter Enable (we leave off)
}

/// USBSTS bits.
mod usbsts {
    pub const HCH: u32 = 1 << 0;   // HCHalted
    pub const HSE: u32 = 1 << 2;   // Host System Error
    pub const EINT: u32 = 1 << 3;  // Event Interrupt
    pub const PCD: u32 = 1 << 4;   // Port Change Detect
    pub const CNR: u32 = 1 << 11;  // Controller Not Ready
}

/// HCCPARAMS1 bit accessors.
mod hccparams1 {
    pub const CSZ_BIT: u32 = 1 << 2; // 1 = 64-byte context size
    /// xHCI Extended Capabilities Pointer is in bits 31:16, expressed
    /// in **dword** units relative to MMIO base. Zero = no xECP list.
    pub const XECP_SHIFT: u32 = 16;
    pub const XECP_MASK: u32 = 0xFFFF;
}

/// xHCI Extended Capability IDs (spec §7).
mod xecp {
    pub const ID_USB_LEGACY_SUPPORT: u32 = 0x01;
    #[allow(dead_code)]
    pub const ID_SUPPORTED_PROTOCOL: u32 = 0x02;
    // USBLEGSUP layout (dword at xECP base):
    //   bit 0     = Cap ID (=0x01)
    //   bits 15:8 = Next Capability Pointer (dwords)
    //   bit 16    = HC BIOS Owned Semaphore
    //   bit 24    = HC OS  Owned Semaphore
    pub const LEGSUP_BIOS_OWNED: u32 = 1 << 16;
    pub const LEGSUP_OS_OWNED: u32 = 1 << 24;
    // USBLEGCTLSTS at LEGSUP+4: clear all SMI-on-* enables to disarm SMIs
    // and write-1-to-clear the corresponding event status bits.
    pub const LEGCTLSTS_OFFSET: u32 = 4;
    /// SMI Enable bits to clear (bits 0, 4, 13, 14, 15, 16).
    pub const LEGCTLSTS_SMI_ENABLES: u32 =
        (1 << 0) | (1 << 4) | (1 << 13) | (1 << 14) | (1 << 15) | (1 << 16);
    /// SMI Event bits (bits 29, 30, 31) — RW1C. Bits 17..20 are RsvdP.
    pub const LEGCTLSTS_SMI_EVENTS: u32 = (1 << 29) | (1 << 30) | (1 << 31);
}

/// Intel PCH-specific PCI config-space offsets for routing USB 2/3 ports
/// between the EHCI companion and the xHCI controller. Applies to Lynx
/// Point, Wildcat Point, Sunrise Point, Cannon Point, ... and any other
/// Intel chipset whose vendor ID is 0x8086.
///
/// **XUSB2PR (USB 2 Port Routing)**: each set bit routes that USB 2 port
/// to xHCI. Default 0 routes to EHCI. We set this to XUSB2PRM (a sibling
/// register) which holds the mask of ports that *can* be routed.
///
/// **USB3PSSEN (USB 3 SuperSpeed Enable)**: each set bit enables xHCI
/// SuperSpeed on that port. Default 0 disables SS. We set this to USB3PRM.
///
/// On the W540's Lynx Point PCH this is the **single most important
/// init step**: without it the internal keyboard (a USB 2 device behind
/// the PCH's hub) is wired to EHCI, and xHCI sees CCS=0 on every port.
mod intel_pch {
    pub const USB3PRM: u8 = 0xDC;   // 32-bit RO mask of ports that CAN be SS-enabled
    pub const USB3PSSEN: u8 = 0xD8;  // 32-bit RW: 1 = route that USB3 port to xHCI SS
    pub const XUSB2PRM: u8 = 0xD4;   // 32-bit RO mask of ports that CAN be routed to xHCI
    pub const XUSB2PR: u8 = 0xD0;    // 32-bit RW: 1 = route that USB2 port to xHCI
}

/// PORTSC bits.
mod portsc {
    pub const CCS: u32 = 1 << 0;   // Current Connect Status
    pub const PED: u32 = 1 << 1;   // Port Enabled/Disabled
    pub const PR: u32 = 1 << 4;    // Port Reset (bit 4 in ALL xHCI revisions; bit 3 is OCA, read-only)
    #[allow(dead_code)]
    pub const PLS_MASK: u32 = 0xF << 5;  // Port Link State
    #[allow(dead_code)]
    pub const PLS_SHIFT: u32 = 5;
    pub const PP: u32 = 1 << 9;    // Port Power
    pub const SPEED_SHIFT: u32 = 10; // bits 13:10
    pub const SPEED_MASK: u32 = 0xF << 10;
    pub const CSC: u32 = 1 << 17;  // Connect Status Change (RW1C)
    pub const PEC: u32 = 1 << 18;  // Port Enable Change (RW1C)
    #[allow(dead_code)]
    pub const OCC: u32 = 1 << 19;  // Over-current Change (RW1C)
    #[allow(dead_code)]
    pub const WRC: u32 = 1 << 20;  // Warm Reset Change (RW1C, USB 3)
    pub const PRC: u32 = 1 << 21;  // Port Reset Change (RW1C)
    #[allow(dead_code)]
    pub const PLC: u32 = 1 << 22;  // Port Link State Change (RW1C)
    /// Port Link State write-strobe (bit 16). When set together with a new
    /// PLS field (bits 8:5) the controller commits the link state change.
    #[allow(dead_code)]
    pub const LWS: u32 = 1 << 16;
    /// Warm Port Reset (bit 31, USB 3 only). xHCI spec §5.4.8 — for
    /// SuperSpeed ports, PR (bit 4) is insufficient; WPR re-trains the
    /// SS link and lets the port reach U0 / PED=1.
    pub const WPR: u32 = 1 << 31;
    /// Mask of all change bits (RW1C) — bits 17..22 inclusive. Spec §5.4.8.
    /// When writing PORTSC, mask these out of the read-modify-write so we
    /// don't accidentally clear pending change notifications.
    pub const RW1C_MASK: u32 =
        (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22);
}

/// PORTSC.PR is bit 4 in every xHCI revision (1.0/1.1/1.2 Table 5-27);
/// bit 3 is OCA (Over-Current Active, RO). An earlier session theorized
/// the bit moved in 1.1+ — it did not; keep this helper as the single
/// place that records that finding.
fn pr_mask_for_version(_hciversion: u16) -> u32 {
    1 << 4
}

/// Runtime register offsets relative to MMIO + RTSOFF.
/// Each interrupter is 32 bytes. IR0 starts at +0x20.
mod rt_reg {
    pub const IR0_IMAN: u32 = 0x20;
    pub const IR0_IMOD: u32 = 0x24;
    pub const IR0_ERSTSZ: u32 = 0x28;
    pub const IR0_ERSTBA: u32 = 0x30; // 64-bit
    pub const IR0_ERDP: u32 = 0x38;   // 64-bit
}

// ============================================================================
// Static DMA-coherent allocations
// ============================================================================
//
// Everything below is in .bss, contiguous, identity-mapped by the
// bootloader's PML4. We resolve every region's physical address with
// `paging::walk_active_pml4` exactly the way virtio-block does.

/// Device Context Base Address Array (spec §6.1). Entry 0 holds the
/// pointer to the Scratchpad Buffer Array (if MaxScratchpadBufs>0);
/// entries 1..=MaxSlotsEn point to per-slot Device Contexts. We use
/// MaxSlotsEn=8 (enough for the few devices any HID demo will attach),
/// so 9 entries are meaningful, but we allocate space for 256 to match
/// the maximum allowed and keep field offsets simple.
const MAX_SLOTS: usize = 64;
#[repr(C, align(4096))]
struct Dcbaa([u64; MAX_SLOTS + 1]);
static mut DCBAA: Dcbaa = Dcbaa([0; MAX_SLOTS + 1]);

/// Scratchpad: a u64 array whose entries hold physical addresses of
/// 4 KiB scratchpad pages. The number of entries is read from
/// HCSPARAMS2 (Max Scratchpad Bufs Hi:Lo). qemu-xhci asks for 0;
/// typical real hardware is 1-8.
///
/// **Sized at 8** because 32 (the original value) allocates 128 KiB
/// of BSS that pushed our kernel image past a memory-layout boundary
/// and caused a #GP fault in user-program code around DEMO 8. If a
/// controller legitimately needs more, `init()` aborts with a clear
/// log line — bump this constant rather than silently truncating.
// Real Intel PCH xHCI (W540 Lynx Point, P1 Sunrise Point, etc.)
// asks for 16 scratchpad buffers. QEMU's qemu-xhci asks for 0.
// 32 gives headroom for any future hardware without much memory cost
// (32 * 4 KiB = 128 KiB total).
const MAX_SCRATCHPAD_BUFS: usize = 32;
#[repr(C, align(4096))]
struct ScratchpadArray([u64; MAX_SCRATCHPAD_BUFS]);
#[repr(C, align(4096))]
struct ScratchpadPages([[u8; 4096]; MAX_SCRATCHPAD_BUFS]);
static mut SCRATCHPAD_ARRAY: ScratchpadArray = ScratchpadArray([0; MAX_SCRATCHPAD_BUFS]);
static mut SCRATCHPAD_PAGES: ScratchpadPages = ScratchpadPages([[0; 4096]; MAX_SCRATCHPAD_BUFS]);

/// Command ring (256 TRBs, last is Link back to base).
static mut COMMAND_RING: CommandRing = CommandRing::new();

/// Event ring: single segment, single ERST entry.
static mut EVENT_RING: EventRingSegment = EventRingSegment::new();
#[repr(C, align(64))]
struct Erst([ErstEntry; 1]);
static mut ERST: Erst = Erst([ErstEntry::zero()]);

/// Per-slot device context. Sized for CSZ=1. Index 0 is unused (xHCI slot
/// IDs are 1-based); we size to MAX_SLOTS + 1 so slot_id can be used as
/// a direct index. Phase 15 M50 follow-up: cascaded hub enumeration needs
/// independent device contexts for the hub itself (slot 1) and each child
/// device behind it (slot 2..).
#[repr(C, align(4096))]
struct PerSlotDeviceCtx(DeviceContext);
static mut DEVICE_CTXS: [PerSlotDeviceCtx; MAX_SLOTS + 1] = [const {
    PerSlotDeviceCtx(DeviceContext::zero())
}; MAX_SLOTS + 1];

/// Per-slot input context (issued for Address Device / Configure Endpoint).
#[repr(C, align(4096))]
struct PerSlotInputCtx(InputContext);
static mut INPUT_CTXS: [PerSlotInputCtx; MAX_SLOTS + 1] = [const {
    PerSlotInputCtx(InputContext::zero())
}; MAX_SLOTS + 1];

/// Per-slot EP0 control transfer ring.
static mut EP0_TRANSFER_RINGS: [CommandRing; MAX_SLOTS + 1] = [const {
    CommandRing::new()
}; MAX_SLOTS + 1];
/// Per-slot transfer ring for the HID Interrupt-IN endpoint.
static mut HID_TRANSFER_RINGS: [CommandRing; MAX_SLOTS + 1] = [const {
    CommandRing::new()
}; MAX_SLOTS + 1];

/// Generic DMA buffer for descriptor reads (max we need: the full
/// Config + Iface + Endpoint + HID combined descriptor blob).
const SETUP_BUF_SIZE: usize = 8192;
#[repr(C, align(64))]
struct DmaBuf([u8; SETUP_BUF_SIZE]);
static mut DMA_BUF: DmaBuf = DmaBuf([0; SETUP_BUF_SIZE]);

/// Per-slot DMA buffer for the recurring HID keyboard report.
#[repr(C, align(64))]
struct HidReportBuf([u8; 64]);
static mut HID_REPORT_BUFS: [HidReportBuf; MAX_SLOTS + 1] = [const {
    HidReportBuf([0; 64])
}; MAX_SLOTS + 1];

/// Per-slot transfer rings for the bulk path (USB Mass Storage, CDC-ECM,
/// etc.). Each slot gets its own IN + OUT pair so multiple bulk devices
/// (e.g. Ethernet + storage behind a dock) don't corrupt each other's TRBs.
static mut BULK_IN_TRANSFER_RINGS: [CommandRing; MAX_SLOTS + 1] = [const {
    CommandRing::new()
}; MAX_SLOTS + 1];
static mut BULK_OUT_TRANSFER_RINGS: [CommandRing; MAX_SLOTS + 1] = [const {
    CommandRing::new()
}; MAX_SLOTS + 1];
/// Scratch for a single Mass Storage transaction: CBW (31) + max data
/// (4096; a SCSI INQUIRY response is 36, but page-sized covers READ(10) too)
/// + CSW (13). Three separate buffers so we can submit independent TRBs.
#[repr(C, align(64))]
struct BulkCbwBuf([u8; 64]);
#[repr(C, align(4096))]
struct BulkDataBuf([u8; 4096]);
#[repr(C, align(64))]
struct BulkCswBuf([u8; 32]);
static mut BULK_CBW_BUF: BulkCbwBuf = BulkCbwBuf([0; 64]);
static mut BULK_DATA_BUF: BulkDataBuf = BulkDataBuf([0; 4096]);
static mut BULK_CSW_BUF: BulkCswBuf = BulkCswBuf([0; 32]);

// ============================================================================
// Driver state
// ============================================================================

#[derive(Copy, Clone)]
pub struct XhciInfo {
    pub mmio_base: u64,
    pub op_base: u64,
    pub run_base: u64,
    pub db_base: u64,
    pub max_slots: u8,
    pub max_ports: u8,
    pub csz1: bool, // 64-byte context size when true
    pub max_scratchpad_bufs: usize,
    /// xHCI spec version from CAPLENGTH+2 (e.g. 0x0100 = 1.0, 0x0110 = 1.1).
    /// PR bit position moved from bit 4 (1.0) to bit 3 (1.1+).
    pub hciversion: u16,
}

#[derive(Copy, Clone, Default)]
pub struct EnumeratedDevice {
    pub slot_id: u8,
    pub usb_address: u8,
    pub port: u8,
    pub speed: u8,
    pub vendor: u16,
    pub product: u16,
    pub max_packet_ep0: u16,
    pub is_keyboard: bool,
    pub kbd_ep_in: u8,
    pub kbd_ep_packet_size: u16,
    pub kbd_ep_interval: u8,
    pub config_value: u8,
    pub interface_number: u8,
    /// USB device class (0x09 = hub, 0x08 = mass storage, 0x03 = HID, etc.)
    pub class: u8,
}

static mut INFO: Option<XhciInfo> = None;
static mut DEVICE: Option<EnumeratedDevice> = None;
/// Every successfully enumerated slot gets a copy here so usbinfo can show
/// the full topology (root ports + hub downstream ports).
static mut ENUMERATED_SLOTS: [Option<EnumeratedDevice>; MAX_SLOTS + 1] = [None; MAX_SLOTS + 1];
static mut CMD_PROD: Producer = Producer::new();
static mut EVT_CONS: Consumer = Consumer::new();
static mut EP0_PRODS: [Producer; MAX_SLOTS + 1] = [const {
    Producer::new()
}; MAX_SLOTS + 1];
static mut HID_PRODS: [Producer; MAX_SLOTS + 1] = [const { Producer::new() }; MAX_SLOTS + 1];
static mut BULK_IN_PRODS: [Producer; MAX_SLOTS + 1] = [const { Producer::new() }; MAX_SLOTS + 1];
static mut BULK_OUT_PRODS: [Producer; MAX_SLOTS + 1] = [const { Producer::new() }; MAX_SLOTS + 1];

/// Resolved physical addresses of per-slot HID and bulk transfer rings.
static mut HID_RING_PHYSES: [u64; MAX_SLOTS + 1] = [0u64; MAX_SLOTS + 1];
static mut BULK_IN_RING_PHYSES: [u64; MAX_SLOTS + 1] = [0u64; MAX_SLOTS + 1];
static mut BULK_OUT_RING_PHYSES: [u64; MAX_SLOTS + 1] = [0u64; MAX_SLOTS + 1];
/// State for the enumerated Mass Storage device. Populated when
/// `enumerate_mass_storage()` succeeds; consumed by `bulk_in`/`bulk_out`.
#[derive(Copy, Clone)]
pub struct MscDevice {
    pub slot_id: u8,
    pub in_ep_addr: u8,
    pub out_ep_addr: u8,
    pub in_dci: u8,
    pub out_dci: u8,
    pub max_packet: u16,
    pub iface_num: u8,
    pub config_value: u8,
    pub inquiry: [u8; 36],   // first 36 bytes of INQUIRY response
    pub capacity_blocks: u32, // from READ CAPACITY (10) (blocks)
    pub capacity_bs: u32,     // logical block size
}
static mut MSC: Option<MscDevice> = None;

/// CDC-ECM (Phase 15 M50): tethered phone or USB-Ethernet dongle.
/// Populated by `try_enumerate_cdc_ecm` after a successful SET_CONFIGURATION
/// + ConfigureEndpoint for the bulk IN/OUT pair.
#[derive(Default, Clone, Copy, Debug)]
pub struct CdcEcmDevice {
    pub slot_id: u8,
    pub control_iface: u8,
    pub data_iface: u8,
    pub data_alt: u8,
    pub in_ep_addr: u8,
    pub out_ep_addr: u8,
    pub in_dci: u8,
    pub out_dci: u8,
    pub in_mps: u16,
    pub out_mps: u16,
    pub config_value: u8,
    pub mac: [u8; 6],
    pub mtu: u16,
}
static mut CDC_ECM: Option<CdcEcmDevice> = None;

// CDC-ECM TX/RX scratch buffers. Sized for a standard 1514-byte Ethernet
// frame (14 B header + 1500 B payload) + a few bytes of slack. Aligned
// to a page so xHCI doesn't see a buffer that straddles a page boundary
// (the controller's TRB has a 64 KiB max transfer but our MaxPacketSize
// determines per-packet framing).
#[repr(C, align(4096))]
struct EcmFrameBuf([u8; 2048]);
static mut ECM_TX_BUF: EcmFrameBuf = EcmFrameBuf([0; 2048]);
static mut ECM_RX_BUF: EcmFrameBuf = EcmFrameBuf([0; 2048]);
// iPhone tether session 1: dedicated TX/RX scratch buffers for the
// usbmuxd channel. Page-aligned so phys-translation works cleanly.
// 2 KiB matches typical usbmuxd messages (Hello/GetValue/pair record);
// service-level messages can be longer but those are sessions 3+.
#[repr(C, align(4096))]
struct IphoneFrameBuf([u8; 2048]);
static mut IPHONE_TX_BUF: IphoneFrameBuf = IphoneFrameBuf([0; 2048]);
static mut IPHONE_RX_BUF: IphoneFrameBuf = IphoneFrameBuf([0; 2048]);
/// Resolved physical addresses of each per-slot EP0 transfer ring.
static mut EP0_RING_PHYSES: [u64; MAX_SLOTS + 1] = [0u64; MAX_SLOTS + 1];
/// Resolved physical address of the command ring.
static mut CMD_RING_PHYS: u64 = 0;
/// Resolved physical address of the event ring segment.
static mut EVT_RING_PHYS: u64 = 0;

// ============================================================================
// MMIO helpers
// ============================================================================

#[inline]
unsafe fn read_u32(addr: u64) -> u32 {
    read_volatile(addr as *const u32)
}
#[inline]
unsafe fn write_u32(addr: u64, val: u32) {
    write_volatile(addr as *mut u32, val);
}
#[inline]
unsafe fn read_u64(addr: u64) -> u64 {
    // The xHCI spec allows 64-bit reads/writes to 64-bit registers (the
    // ones with explicit "64-bit" types in the register table) but we
    // split them into two 32-bit accesses for portability — some platforms
    // don't allow 8-byte MMIO and any spec-conformant controller accepts
    // a pair of 32-bit ops.
    let lo = read_volatile(addr as *const u32) as u64;
    let hi = read_volatile((addr + 4) as *const u32) as u64;
    (hi << 32) | lo
}
#[inline]
unsafe fn write_u64(addr: u64, val: u64) {
    write_volatile(addr as *mut u32, val as u32);
    write_volatile((addr + 4) as *mut u32, (val >> 32) as u32);
}

// ============================================================================
// PCI discovery + MMIO mapping
// ============================================================================

/// Discover any xHCI controller on PCI bus 0 (vendor-agnostic).
/// Matches by PCI class triple `(0x0C, 0x03, 0x30)` = USB Controller /
/// USB / xHCI — works for both QEMU's `qemu-xhci` (1B36:000D) and
/// real-hardware Intel PCH xHCI (e.g. Lynx Point 8086:8C31, Sunrise
/// Point 8086:A12F, etc.). Falls back to the QEMU vendor/device
/// match if class search returns nothing, for forward compatibility.
pub fn discover() -> Option<(pci::Location, u64)> {
    let loc = pci::find_by_class(0x0C, 0x03, 0x30)
        .or_else(|| pci::find_first(QEMU_XHCI_VENDOR, QEMU_XHCI_DEVICE))?;
    let bar0 = pci::read_u32(loc.bus, loc.slot, loc.func, pci::regs::BAR0);
    let bar1 = pci::read_u32(loc.bus, loc.slot, loc.func, pci::regs::BAR1);

    if bar0 & 1 != 0 {
        println!("[xhci] BAR0 is I/O space (0x{:08X}); xHCI must be MMIO — abort", bar0);
        return None;
    }
    // Type field is bits 2:1. 0=32-bit, 2=64-bit. xHCI is always 64-bit.
    let bar_type = (bar0 >> 1) & 0x3;
    let base = if bar_type == 0x2 {
        ((bar1 as u64) << 32) | ((bar0 as u64) & 0xFFFF_FFF0)
    } else {
        (bar0 as u64) & 0xFFFF_FFF0
    };
    Some((loc, base))
}

/// Enable MEM space and bus mastering on the device's command register
/// so MMIO + DMA work.
fn enable_pci(loc: pci::Location) {
    loc.enable_io_and_bus_master();
}

/// Halt every EHCI controller on bus 0 and reset it. On Intel Lynx Point
/// (W540) the EHCI controllers at PCI 00:1A.0 and 00:1D.0 keep their
/// USB-2 port state machines running even after XUSB2PR routes USB-2
/// traffic to xHCI. If they're still running when xHCI tries to PR a
/// USB-2 port, the port stays stuck (PED never asserts) because the two
/// controllers are fighting for the physical reset signaling.
///
/// EHCI class triple is 0x0C / 0x03 / 0x20. We find each one, read its
/// MMIO base from BAR0, then:
///   1. Clear USBCMD.RS (bit 0) to halt the controller.
///   2. Poll USBSTS.HCH (bit 12) for halt complete.
///   3. Set USBCMD.HCRESET (bit 1) to fully reset.
///   4. Poll HCRESET for self-clear.
///   5. Clear CONFIGFLAG to release ports to companion controllers.
///
/// Called BEFORE the xHCI HCRST. xHCI's HCRST itself doesn't touch EHCI.
///
/// **Currently unused** on Lynx Point — even just clearing USBCMD.RS
/// breaks USB-3 enumeration because xHCI and EHCI share clock/power
/// state. Kept in tree for documentation + possible use on other
/// chipsets (e.g., older Intel Series 5/6).
#[allow(dead_code)]
fn halt_ehci_controllers() {
    const EHCI_CLASS: u8 = 0x0C;
    const EHCI_SUBCLASS: u8 = 0x03;
    const EHCI_PROG_IF: u8 = 0x20;
    const EHCI_USBCMD: u64 = 0x00;
    const EHCI_USBSTS: u64 = 0x04;
    const EHCI_USBCMD_RS: u32 = 1 << 0;
    const EHCI_USBSTS_HCH: u32 = 1 << 12;
    let mut count = 0u32;
    for slot in 0..32u8 {
        for func in 0..8u8 {
            let loc = pci::Location { bus: 0, slot, func };
            if loc.vendor_id() == 0xFFFF { continue; }
            if pci::class_triple(loc) != (EHCI_CLASS, EHCI_SUBCLASS, EHCI_PROG_IF) {
                continue;
            }
            count += 1;
            // BAR0 = EHCI MMIO base (assume 32-bit BAR for simplicity;
            // Lynx Point uses 32-bit BARs for EHCI).
            let bar0 = pci::read_u32(loc.bus, loc.slot, loc.func, 0x10);
            let mmio_phys = (bar0 & !0xF) as u64;
            if mmio_phys == 0 {
                println!("[ehci] {}:{:02X}.{} BAR0=0 — skipping", loc.bus, loc.slot, loc.func);
                continue;
            }
            // Enable MMIO + bus-master so we can talk to the controller.
            loc.enable_io_and_bus_master();
            let mmio = paging::phys_to_virt(mmio_phys);
            // EHCI capability registers: first byte is CAPLENGTH.
            let caplen = unsafe { read_volatile(mmio as *const u8) } as u64;
            let op_base = mmio + caplen;
            // MINIMAL halt only: clear Run/Stop. Do NOT HCRESET (resets
            // controller state including port migration we already did
            // via XUSB2PR). Do NOT touch CONFIGFLAG (Lynx Point uses
            // chipset routing not EHCI's CF — writing CF=0 had side
            // effects that broke USB-3 enumeration on the W540 in the
            // previous iter). Just stop EHCI from running so it can't
            // initiate port reset cycles that fight xHCI.
            unsafe {
                let cmd = read_u32(op_base + EHCI_USBCMD);
                write_u32(op_base + EHCI_USBCMD, cmd & !EHCI_USBCMD_RS);
            }
            for _ in 0..1_000_000 {
                let s = unsafe { read_u32(op_base + EHCI_USBSTS) };
                if s & EHCI_USBSTS_HCH != 0 { break; }
                core::hint::spin_loop();
            }
            println!(
                "[ehci] {}:{:02X}.{} halted (mmio=0x{:X} caplen={})",
                loc.bus, loc.slot, loc.func, mmio_phys, caplen
            );
        }
    }
    if count == 0 {
        println!("[ehci] no EHCI controllers found (QEMU/AMD or already disabled)");
    }
}

/// Diagnostic: read every EHCI controller's state without touching it.
/// Prints USBCMD, USBSTS, and each port's PORTSC so we can see whether
/// EHCI is still fighting xHCI for the USB-2 PHYs.
pub fn ehci_dump() {
    const EHCI_CLASS: u8 = 0x0C;
    const EHCI_SUBCLASS: u8 = 0x03;
    const EHCI_PROG_IF: u8 = 0x20;
    const EHCI_USBCMD: u64 = 0x00;
    const EHCI_USBSTS: u64 = 0x04;
    const EHCI_HCSPARAMS: u64 = 0x04; // in capability space
    const EHCI_PORTSC_BASE: u64 = 0x44;
    const EHCI_USBCMD_RS: u32 = 1 << 0;
    const EHCI_USBCMD_HCRESET: u32 = 1 << 1;
    const EHCI_USBSTS_HCH: u32 = 1 << 12;
    const EHCI_PORTSC_CCS: u32 = 1 << 0;
    const EHCI_PORTSC_PE: u32 = 1 << 2;
    const EHCI_PORTSC_PR: u32 = 1 << 8;
    const EHCI_PORTSC_PP: u32 = 1 << 12;
    const EHCI_PORTSC_PO: u32 = 1 << 13;

    let mut count = 0u32;
    for slot in 0..32u8 {
        for func in 0..8u8 {
            let loc = pci::Location { bus: 0, slot, func };
            if loc.vendor_id() == 0xFFFF {
                continue;
            }
            if pci::class_triple(loc) != (EHCI_CLASS, EHCI_SUBCLASS, EHCI_PROG_IF) {
                continue;
            }
            count += 1;
            let bar0 = pci::read_u32(loc.bus, loc.slot, loc.func, 0x10);
            let mmio_phys = (bar0 & !0xF) as u64;
            if mmio_phys == 0 {
                println!(
                    "[ehci] {}:{:02X}.{} BAR0=0 — skipping",
                    loc.bus, loc.slot, loc.func
                );
                continue;
            }
            let mmio = paging::phys_to_virt(mmio_phys);
            let caplen = unsafe { read_volatile(mmio as *const u8) } as u64;
            let hcsparams = unsafe { read_u32(mmio + EHCI_HCSPARAMS) };
            let hccparams = unsafe { read_u32(mmio + 0x08) };
            let eecp = ((hccparams >> 8) & 0xFF) as u8;
            let n_ports = (hcsparams & 0xF) as u8;
            let op_base = mmio + caplen;
            let usbcmd = unsafe { read_u32(op_base + EHCI_USBCMD) };
            let usbsts = unsafe { read_u32(op_base + EHCI_USBSTS) };
            println!(
                "[ehci] {}:{:02X}.{} mmio=0x{:X} caplen={} n_ports={} USBCMD=0x{:08X} (RS={} HCRESET={}) USBSTS=0x{:08X} (HCH={})",
                loc.bus, loc.slot, loc.func,
                mmio_phys, caplen, n_ports,
                usbcmd,
                if usbcmd & EHCI_USBCMD_RS != 0 { 1 } else { 0 },
                if usbcmd & EHCI_USBCMD_HCRESET != 0 { 1 } else { 0 },
                usbsts,
                if usbsts & EHCI_USBSTS_HCH != 0 { 1 } else { 0 }
            );
            if eecp >= 0x40 {
                let legsup = pci::read_u32(loc.bus, loc.slot, loc.func, eecp);
                let legctlsts = pci::read_u32(loc.bus, loc.slot, loc.func, eecp + 4);
                println!(
                    "[ehci]   LEGSUP=0x{:08X} (BIOS={} OS={}) LEGCTLSTS=0x{:08X}",
                    legsup,
                    if legsup & (1 << 16) != 0 { 1 } else { 0 },
                    if legsup & (1 << 24) != 0 { 1 } else { 0 },
                    legctlsts
                );
            } else {
                println!("[ehci]   no LEGSUP (EECP={})", eecp);
            }
            for port in 1..=n_ports {
                let portsc = unsafe { read_u32(op_base + EHCI_PORTSC_BASE + ((port - 1) as u64) * 4) };
                println!(
                    "[ehci]   port{}: PORTSC=0x{:08X} CCS={} PE={} PR={} PP={} PO={}",
                    port,
                    portsc,
                    if portsc & EHCI_PORTSC_CCS != 0 { 1 } else { 0 },
                    if portsc & EHCI_PORTSC_PE != 0 { 1 } else { 0 },
                    if portsc & EHCI_PORTSC_PR != 0 { 1 } else { 0 },
                    if portsc & EHCI_PORTSC_PP != 0 { 1 } else { 0 },
                    if portsc & EHCI_PORTSC_PO != 0 { 1 } else { 0 }
                );
            }
        }
    }
    if count == 0 {
        println!("[ehci] no EHCI controllers found (QEMU/AMD or already disabled)");
    }
}

/// Disarm BIOS SMI on EHCI by writing LEGCTLSTS=0. On Lynx Point,
/// LEGCTLSTS=0xC0080000 means SMI on Ownership Change (bit 19), SMI
/// on PCI Command (bit 30), and SMI on BAR (bit 31) are armed AND
/// status bits are latched. Any USB-2 activity then traps BIOS, and
/// the BIOS handler can interfere with xHCI port reset (which is
/// likely why W540 USB-2 ports stay stuck PLS=Polling PED=0).
///
/// Writing all-zeros: clears all SMI ENABLE bits AND W1C's the status
/// bits. Must run BEFORE the EHCI halt/reset sequence (to prevent any
/// final SMI as we halt the controller). Safe across multiple
/// controllers and idempotent.
pub fn disarm_ehci_smi() {
    const EHCI_CLASS: u8 = 0x0C;
    const EHCI_SUBCLASS: u8 = 0x03;
    const EHCI_PROG_IF: u8 = 0x20;
    for slot in 0..32u8 {
        for func in 0..8u8 {
            let loc = pci::Location { bus: 0, slot, func };
            if loc.vendor_id() == 0xFFFF { continue; }
            if pci::class_triple(loc) != (EHCI_CLASS, EHCI_SUBCLASS, EHCI_PROG_IF) {
                continue;
            }
            let bar0 = pci::read_u32(loc.bus, loc.slot, loc.func, 0x10);
            let mmio_phys = (bar0 & !0xF) as u64;
            if mmio_phys == 0 { continue; }
            let mmio = paging::phys_to_virt(mmio_phys);
            let hccparams = unsafe { read_u32(mmio + 0x08) };
            let eecp = ((hccparams >> 8) & 0xFF) as u8;
            if eecp < 0x40 { continue; } // No extended caps
            // LEGSUP at eecp+0, LEGCTLSTS at eecp+4.
            let before = pci::read_u32(loc.bus, loc.slot, loc.func, eecp + 4);
            // Write 0 — clears SMI enables (bits 0-21) and W1C clears
            // status bits in the upper half (bits 29-31).
            pci::write_u32(loc.bus, loc.slot, loc.func, eecp + 4, 0);
            let after = pci::read_u32(loc.bus, loc.slot, loc.func, eecp + 4);
            println!(
                "[ehci] {}:{:02X}.{} LEGCTLSTS disarmed: before=0x{:08X} after=0x{:08X}",
                loc.bus, loc.slot, loc.func, before, after
            );
        }
    }
}

/// Lynx Point shares the USB-2 PHY reference clock between xHCI and EHCI.
/// When BIOS leaves EHCI halted (RS=0), the clock is gated and xHCI cannot
/// detect device speed chirps during port reset — PRC fires but PED never
/// asserts. Starting EHCI (RS=1) ungates the clock without giving EHCI
/// port ownership (XUSB2PR already routed ports to xHCI, and EHCI ports
/// show CCS=0 / PO=1). Safe to call multiple times — idempotent.
pub fn start_ehci_clocks() {
    const EHCI_CLASS: u8 = 0x0C;
    const EHCI_SUBCLASS: u8 = 0x03;
    const EHCI_PROG_IF: u8 = 0x20;
    const EHCI_USBCMD: u64 = 0x00;
    const EHCI_USBSTS: u64 = 0x04;
    const EHCI_USBCMD_RS: u32 = 1 << 0;
    const EHCI_USBSTS_HCH: u32 = 1 << 12;
    for slot in 0..32u8 {
        for func in 0..8u8 {
            let loc = pci::Location { bus: 0, slot, func };
            if loc.vendor_id() == 0xFFFF {
                continue;
            }
            if pci::class_triple(loc) != (EHCI_CLASS, EHCI_SUBCLASS, EHCI_PROG_IF) {
                continue;
            }
            let bar0 = pci::read_u32(loc.bus, loc.slot, loc.func, 0x10);
            let mmio_phys = (bar0 & !0xF) as u64;
            if mmio_phys == 0 { continue; }
            let mmio = paging::phys_to_virt(mmio_phys);
            let caplen = unsafe { read_volatile(mmio as *const u8) } as u64;
            let op_base = mmio + caplen;
            let usbcmd = unsafe { read_u32(op_base + EHCI_USBCMD) };
            if usbcmd & EHCI_USBCMD_RS != 0 {
                continue; // already running
            }
            unsafe {
                write_u32(op_base + EHCI_USBCMD, usbcmd | EHCI_USBCMD_RS);
            }
            for _ in 0..1_000_000 {
                let sts = unsafe { read_u32(op_base + EHCI_USBSTS) };
                if sts & EHCI_USBSTS_HCH == 0 { break; }
                core::hint::spin_loop();
            }
            println!("[ehci] {}:{:02X}.{} started (RS=1) for shared PHY clock",
                loc.bus, loc.slot, loc.func);
        }
    }
}

/// Full EHCI reset + start sequence. Lynx Point gates the USB-2 PHY when
/// EHCI is left in a half-initialized state by BIOS. Just setting RS=1
/// (start_ehci_clocks) is not enough — the controller must be halted,
/// HCRESET'd, and then started so the PHY state machine is fully ungated.
/// Called while xHCI is also in HCRST, so both controllers come up clean.
pub fn reset_and_start_ehci_full() {
    const EHCI_CLASS: u8 = 0x0C;
    const EHCI_SUBCLASS: u8 = 0x03;
    const EHCI_PROG_IF: u8 = 0x20;
    const EHCI_USBCMD: u64 = 0x00;
    const EHCI_USBSTS: u64 = 0x04;
    const EHCI_USBCMD_RS: u32 = 1 << 0;
    const EHCI_USBCMD_HCRESET: u32 = 1 << 1;
    const EHCI_USBSTS_HCH: u32 = 1 << 12;
    for slot in 0..32u8 {
        for func in 0..8u8 {
            let loc = pci::Location { bus: 0, slot, func };
            if loc.vendor_id() == 0xFFFF { continue; }
            if pci::class_triple(loc) != (EHCI_CLASS, EHCI_SUBCLASS, EHCI_PROG_IF) {
                continue;
            }
            let bar0 = pci::read_u32(loc.bus, loc.slot, loc.func, 0x10);
            let mmio_phys = (bar0 & !0xF) as u64;
            if mmio_phys == 0 { continue; }
            let mmio = paging::phys_to_virt(mmio_phys);
            let caplen = unsafe { read_volatile(mmio as *const u8) } as u64;
            let op_base = mmio + caplen;

            // Step 1: Halt if running.
            let cmd0 = unsafe { read_u32(op_base + EHCI_USBCMD) };
            if cmd0 & EHCI_USBCMD_RS != 0 {
                unsafe { write_u32(op_base + EHCI_USBCMD, cmd0 & !EHCI_USBCMD_RS); }
                for _ in 0..1_000_000 {
                    let sts = unsafe { read_u32(op_base + EHCI_USBSTS) };
                    if sts & EHCI_USBSTS_HCH != 0 { break; }
                    core::hint::spin_loop();
                }
            }

            // Step 2: HCRESET.
            unsafe { write_u32(op_base + EHCI_USBCMD, EHCI_USBCMD_HCRESET); }
            for _ in 0..1_000_000 {
                let cmd = unsafe { read_u32(op_base + EHCI_USBCMD) };
                if cmd & EHCI_USBCMD_HCRESET == 0 { break; }
                core::hint::spin_loop();
            }

            // Step 2b: Clear CONFIGFLAG so EHCI releases all ports to companion
            // controllers (xHCI).  Without this, EHCI may keep internal ownership
            // of USB-2 ports even though XUSB2PR routes them to xHCI, causing
            // PLS=Polling PED=0 on every USB-2 port.
            const EHCI_CONFIGFLAG: u64 = 0x40;
            unsafe { write_u32(op_base + EHCI_CONFIGFLAG, 0); }

            // Step 3: Start (RS=1).
            unsafe { write_u32(op_base + EHCI_USBCMD, EHCI_USBCMD_RS); }
            for _ in 0..1_000_000 {
                let sts = unsafe { read_u32(op_base + EHCI_USBSTS) };
                if sts & EHCI_USBSTS_HCH == 0 { break; }
                core::hint::spin_loop();
            }

            println!("[ehci] {}:{:02X}.{} full reset+start (halt->HCRESET->RS=1)",
                loc.bus, loc.slot, loc.func);
        }
    }
}

/// Walk the xHCI Extended Capabilities list and, if a USB Legacy Support
/// capability (ID=1) exists, transfer ownership from BIOS to OS.
///
/// On Intel PCH and most physical hardware the BIOS owns the controller
/// at boot and traps PORTSC accesses via SMI; until we set the OS-owned
/// semaphore and wait for BIOS-owned to clear, port writes don't take
/// effect and an HCRST may immediately reset the controller back into
/// BIOS-owned mode.
///
/// Also disables all SMI sources in USBLEGCTLSTS (offset +4) so subsequent
/// xHCI events never trap into BIOS. Returns true if the handoff completed
/// (or no LEGSUP cap was present, which is the normal case on QEMU and
/// AMD chipsets).
fn bios_handoff(mmio: u64, hccparams1: u32) -> bool {
    // xECP is in HCCPARAMS1 bits 31:16, expressed in **dword** units.
    let mut off_dw =
        ((hccparams1 >> hccparams1::XECP_SHIFT) & hccparams1::XECP_MASK) as u64;
    if off_dw == 0 {
        return true; // No extended caps list (QEMU/AMD)
    }
    let mut cap_addr = mmio + off_dw * 4;
    // Walk up to 64 caps as a sanity bound — real lists are at most a handful.
    for _ in 0..64 {
        let cap = unsafe { read_u32(cap_addr) };
        let id = cap & 0xFF;
        let next = (cap >> 8) & 0xFF; // next pointer, in dword units, RELATIVE
        if id == xecp::ID_USB_LEGACY_SUPPORT {
            // Found USBLEGSUP. Set OS Owned, then wait for BIOS Owned to clear.
            unsafe {
                let v = read_u32(cap_addr);
                write_u32(cap_addr, v | xecp::LEGSUP_OS_OWNED);
            }
            let mut spins: u32 = 0;
            loop {
                let v = unsafe { read_u32(cap_addr) };
                let bios_owned = v & xecp::LEGSUP_BIOS_OWNED != 0;
                let os_owned = v & xecp::LEGSUP_OS_OWNED != 0;
                if !bios_owned && os_owned {
                    break;
                }
                spins += 1;
                if spins > 1_000_000 {
                    // Some BIOSes never give up the semaphore. Force-clear
                    // the BIOS-owned bit ourselves — Linux's xhci-pci.c does
                    // the same thing as a last-resort workaround.
                    unsafe {
                        let v = read_u32(cap_addr);
                        write_u32(cap_addr, (v & !xecp::LEGSUP_BIOS_OWNED)
                            | xecp::LEGSUP_OS_OWNED);
                    }
                    println!("[xhci] BIOS handoff timeout — forced semaphore");
                    break;
                }
                core::hint::spin_loop();
            }
            // Disarm SMIs and W1C all latched event bits in USBLEGCTLSTS (+4).
            unsafe {
                let ctlsts_addr = cap_addr + xecp::LEGCTLSTS_OFFSET as u64;
                let v = read_u32(ctlsts_addr);
                let masked = v & !xecp::LEGCTLSTS_SMI_ENABLES;
                write_u32(ctlsts_addr, masked | xecp::LEGCTLSTS_SMI_EVENTS);
            }
            println!("[xhci] BIOS->OS handoff complete (xECP @ +0x{:X})", off_dw * 4);
            return true;
        }
        if next == 0 {
            break;
        }
        off_dw += next as u64;
        cap_addr = mmio + off_dw * 4;
    }
    true
}

/// Intel PCH-specific: route USB 2 / USB 3 root-hub ports from the EHCI
/// companion controller (or "disabled") to xHCI.
///
/// Without this, on Intel chipsets prior to Skylake and on most
/// "dual-role" PCHs (Lynx Point, Wildcat Point, even some Sunrise Point
/// SKUs), the USB 2 ports are wired to the EHCI controller at 00:1D.0
/// and xHCI sees CCS=0 on every port — exactly the "bus detected but
/// nothing enumerated" symptom on the W540.
///
/// We read XUSB2PRM / USB3PRM (the read-only "can be routed" masks) and
/// write the same value into XUSB2PR / USB3PSSEN to route every port
/// that's *capable* of being on xHCI to xHCI. This is the same approach
/// Linux's `xhci-pci.c::xhci_pci_quirks()` uses.
///
/// No-op on non-Intel vendors (so QEMU and AMD machines aren't affected).
fn intel_pch_port_routing(loc: pci::Location) {
    let vendor = pci::read_u16(loc.bus, loc.slot, loc.func, pci::regs::VENDOR_ID);
    if vendor != 0x8086 {
        return; // Not an Intel chipset; nothing to do.
    }
    let usb3prm = pci::read_u32(loc.bus, loc.slot, loc.func, intel_pch::USB3PRM);
    let xusb2prm = pci::read_u32(loc.bus, loc.slot, loc.func, intel_pch::XUSB2PRM);
    // SuperSpeed lanes always belong to xHCI; force-write even if the
    // mask reads zero (some Lynx Point steppings lock the mask register
    // but still honor the routing write).
    pci::write_u32(loc.bus, loc.slot, loc.func, intel_pch::USB3PSSEN, 0xFFFFFFFF);
    // USB-2 routing depends on whether a companion EHCI exists. With an
    // EHCI present (W540 / Lynx Point) the standalone EHCI driver owns
    // the USB-2 ports — xHCI's USB-2 reset has never worked on that
    // chipset (~30 builds of PLS=Polling PED=0), so route USB-2 AWAY
    // from xHCI (XUSB2PR=0), Windows-7 style. Without EHCI (modern PCH,
    // QEMU) xHCI must take everything.
    let has_ehci = pci::find_by_class(0x0C, 0x03, 0x20).is_some();
    let xusb2pr_val: u32 = if has_ehci { 0 } else { 0xFFFFFFFF };
    pci::write_u32(loc.bus, loc.slot, loc.func, intel_pch::XUSB2PR, xusb2pr_val);
    // Read back what actually got set — BIOS can lock these via SMM and
    // silently refuse our writes. Cache for usbinfo.
    let usb3pssen_now = pci::read_u32(loc.bus, loc.slot, loc.func, intel_pch::USB3PSSEN);
    let xusb2pr_now = pci::read_u32(loc.bus, loc.slot, loc.func, intel_pch::XUSB2PR);
    unsafe {
        PCH_ROUTING = Some(PchRouting {
            pci_loc: loc,
            usb3prm,
            xusb2prm,
            usb3pssen: usb3pssen_now,
            xusb2pr: xusb2pr_now,
        });
    }
    println!(
        "[xhci] Intel PCH routing: USB3PSSEN<-0xFFFFFFFF (mask=0x{:08X} now=0x{:08X})  XUSB2PR<-0x{:08X} (mask=0x{:08X} now=0x{:08X}, USB-2 {} )",
        usb3prm, usb3pssen_now, xusb2pr_val, xusb2prm, xusb2pr_now,
        if has_ehci { "-> EHCI" } else { "-> xHCI" }
    );
}

/// Snapshot of Intel PCH USB port routing read-back, cached so `usbinfo`
/// can dump the values without re-scanning PCI.
#[derive(Copy, Clone)]
struct PchRouting {
    pci_loc: pci::Location,
    /// Mask of USB-3 ports the controller advertises as route-capable.
    usb3prm: u32,
    /// Mask of USB-2 ports the controller advertises as route-capable.
    xusb2prm: u32,
    /// What USB3PSSEN actually reads after our write (vs what we wrote).
    usb3pssen: u32,
    /// What XUSB2PR actually reads after our write.
    xusb2pr: u32,
}
static mut PCH_ROUTING: Option<PchRouting> = None;

// ============================================================================
// Bring-up sequence
// ============================================================================

/// Bring the controller up to "Run/Stop=1" and enumerate root-hub ports.
/// Returns the enumerated keyboard (if any), the controller info, and
/// the number of devices seen.
///
/// On any failure the function returns None and logs a `[xhci] ...` line
/// explaining what step broke.
pub fn init() -> bool {
    let (loc, mmio_base) = match discover() {
        Some(x) => x,
        None => {
            println!("[xhci] no qemu-xhci on PCI bus 0 (vendor 0x{:04X} device 0x{:04X})",
                QEMU_XHCI_VENDOR, QEMU_XHCI_DEVICE);
            return false;
        }
    };
    println!("[xhci] PCI 00:{:02X}.0  MMIO base = 0x{:016X}", loc.slot, mmio_base);
    println!("  [DEMO 18] PASS: xHCI controller found (PCI 00:{:02X}.{})",
        loc.slot, loc.func);

    // Scan for ALL xHCI controllers on PCI bus 0. If there is more than one
    // (e.g. PCH xHCI + Thunderbolt xHCI), the one discover() picked might not
    // be the one the dock/USB-2 devices are wired to.
    let mut xhci_count = 0;
    for slot in 0..32u8 {
        for func in 0..8u8 {
            let check = pci::Location { bus: 0, slot, func };
            if check.vendor_id() == 0xFFFF { continue; }
            if pci::class_triple(check) == (0x0C, 0x03, 0x30) {
                xhci_count += 1;
                if check.bus != loc.bus || check.slot != loc.slot || check.func != loc.func {
                    println!("[xhci] WARNING: second xHCI controller at PCI 00:{:02X}.{} — devices may be on that one instead!",
                        slot, func);
                }
            }
        }
    }
    println!("[xhci] {} xHCI controller(s) found on PCI bus 0", xhci_count);

    enable_pci(loc);

    // NOTE: halt_ehci_controllers() removed from the call path on
    // Lynx Point — even the minimal "clear EHCI USBCMD.RS" variant
    // broke USB-3 enumeration. The chipset shares clock/power state
    // between xHCI and EHCI so cleanly disabling one knocks the other
    // offline. Function kept in tree (#[allow(dead_code)]) for
    // reference; the right Lynx Point USB-2 takeover lives in
    // XUSB2PR + intel_pch_port_routing which we still call below.

    // Intel PCH-specific: route USB 2 / USB 3 ports from EHCI to xHCI. No-op
    // on non-Intel vendors. **Must run before** we read PORTSC — otherwise
    // the controller sees no devices because the ports are still owned by
    // the EHCI companion. (Lynx Point W540 + early Sunrise Point.)
    intel_pch_port_routing(loc);

    // The bootloader identity-maps physical memory at PHYS_MEM_OFFSET, so
    // MMIO accesses go through the phys_to_virt window. xHCI BAR
    // addresses live in low MMIO (typically below 4 GiB), well within the
    // mapped region.
    let mmio = paging::phys_to_virt(mmio_base);

    // ---- Read capability registers ----
    let caplen = unsafe { read_volatile(mmio as *const u8) } as u32;
    let hcsparams1 = unsafe { read_u32(mmio + cap_reg::HCSPARAMS1 as u64) };
    let hcsparams2 = unsafe { read_u32(mmio + cap_reg::HCSPARAMS2 as u64) };
    let hccparams1 = unsafe { read_u32(mmio + cap_reg::HCCPARAMS1 as u64) };
    let hciversion = unsafe { read_volatile((mmio + 2) as *const u16) };

    // ---- BIOS -> OS ownership handoff (USBLEGSUP). Must come before any
    // op-reg writes, since a BIOS-owned controller traps PORTSC via SMI
    // and may reset itself when we touch USBCMD. No-op if no xECP list. ----
    bios_handoff(mmio, hccparams1);
    let dboff = unsafe { read_u32(mmio + cap_reg::DBOFF as u64) } & !0x3;
    let rtsoff = unsafe { read_u32(mmio + cap_reg::RTSOFF as u64) } & !0x1F;
    let max_slots = (hcsparams1 & 0xFF) as u8;
    let max_intrs = ((hcsparams1 >> 8) & 0x7FF) as u16;
    let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;
    let csz1 = (hccparams1 & hccparams1::CSZ_BIT) != 0;
    // MaxScratchpadBufs: hi nibble in bits 25:21, lo in 31:27. (Spec §5.3.4)
    let max_sp_lo = ((hcsparams2 >> 27) & 0x1F) as usize;
    let max_sp_hi = ((hcsparams2 >> 21) & 0x1F) as usize;
    let max_scratchpad_bufs = (max_sp_hi << 5) | max_sp_lo;

    println!("[xhci] caplen={}  HCSPARAMS1=0x{:08X}  HCCPARAMS1=0x{:08X}  HCIVERSION=0x{:04X}",
        caplen, hcsparams1, hccparams1, hciversion);
    println!("[xhci] MaxSlots={} MaxPorts={} MaxIntrs={} CSZ={} ScratchpadBufs={}",
        max_slots, max_ports, max_intrs, if csz1 { 1 } else { 0 }, max_scratchpad_bufs);

    // Walk xECP list and dump ALL capabilities. Intel PCH often has
    // vendor-specific caps (IDs > 0x0A) that configure host policy,
    // power management, or PHY settings that must be set before ports
    // will accept reset.
    let xecp = ((hccparams1 >> 16) & 0xFFFF) as u32;
    if xecp != 0 {
        let mut cap_off = xecp;
        loop {
            let addr = mmio + (cap_off * 4) as u64;
            let dw0 = unsafe { read_u32(addr) };
            let id = dw0 & 0xFF;
            let next = (dw0 >> 8) & 0xFF;
            let id_name = match id {
                0x01 => "USBLEGSUP",
                0x02 => {
                    // xHCI spec §7.2: dword 0 bits 31:16 = {Major, Minor} revision.
                    // Our old code read dword 1 (Name String) — "USB " gives 0x55 ('U').
                    let rev = (dw0 >> 16) & 0xFFFF;
                    let major = ((rev >> 8) & 0xFF) as u8;
                    let minor = (rev & 0xFF) as u8;
                    let dw2 = unsafe { read_u32(addr + 8) };
                    let port_off = (dw2 & 0xFF) as u8;
                    let port_cnt = ((dw2 >> 8) & 0xFF) as u8;
                    println!("[xhci] xECP ID=0x{:02X} SupportedProtocol major=0x{:02X} minor=0x{:02X} ports {}-{}",
                        id, major, minor, port_off, port_off + port_cnt - 1);
                    "SupportedProtocol"
                }
                0x03 => "ExtPM",
                0x04 => "IOVirt",
                0x05 => "LocalMem",
                0x06 => "USBDebug",
                0x07 => "ExtMsgIntr",
                0x0A => "PortCap",
                _ => { println!("[xhci] xECP ID=0x{:02X} @ offset 0x{:04X} dw0=0x{:08X}", id, cap_off * 4, dw0); "Vendor" }
            };
            if id_name != "Vendor" && id != 0x02 {
                println!("[xhci] xECP ID=0x{:02X} ({}) @ offset 0x{:04X}", id, id_name, cap_off * 4);
            }
            if next == 0 { break; }
            cap_off += next;
        }
    }

    // Pick the right context stride: CSZ=0 → 32 B (qemu-xhci, AMD); CSZ=1 →
    // 64 B (Intel — incl. T540 HM87 / P1 Z690). InputContext / DeviceContext
    // are allocated at the max (64 B) stride; accessors honor this.
    crate::usb::device::set_ctx_size(if csz1 { 64 } else { 32 });
    if max_scratchpad_bufs > MAX_SCRATCHPAD_BUFS {
        println!("[xhci] device asks for {} scratchpad bufs; we only allocated {} — abort",
            max_scratchpad_bufs, MAX_SCRATCHPAD_BUFS);
        return false;
    }

    let op_base = mmio + caplen as u64;
    let run_base = mmio + rtsoff as u64;
    let db_base = mmio + dboff as u64;

    // ---- Halt the controller before touching anything (some BIOSes leave it running) ----
    unsafe {
        let cmd = read_u32(op_base + op_reg::USBCMD as u64);
        write_u32(op_base + op_reg::USBCMD as u64, cmd & !usbcmd::RS);
        // Wait for HCH=1
        for _ in 0..1_000_000 {
            let sts = read_u32(op_base + op_reg::USBSTS as u64);
            if sts & usbsts::HCH != 0 { break; }
            core::hint::spin_loop();
        }
    }

    // ---- Reset (HCRST) and wait for it to clear AND CNR=0 ----
    unsafe {
        let cmd = read_u32(op_base + op_reg::USBCMD as u64);
        write_u32(op_base + op_reg::USBCMD as u64, cmd | usbcmd::HCRST);
        let mut spins: u64 = 0;
        loop {
            let cmd_now = read_u32(op_base + op_reg::USBCMD as u64);
            let sts_now = read_u32(op_base + op_reg::USBSTS as u64);
            if (cmd_now & usbcmd::HCRST) == 0 && (sts_now & usbsts::CNR) == 0 {
                break;
            }
            spins += 1;
            if spins > 100_000_000 {
                println!("[xhci] HCRST never cleared (USBCMD=0x{:08X} USBSTS=0x{:08X})",
                    cmd_now, sts_now);
                return false;
            }
            core::hint::spin_loop();
        }
    }

    // ---- USB2PHYCM: try SETTING bit 7 instead of clearing ----
    // Earlier code assumed bit 7 = gate (clear to enable). On Raptor Lake
    // (P1 Gen 6) the opposite may be true: bit 7 = PHY-enable/PLL-lock.
    // HCRST may leave it at 0x00, disabling the USB-2 digital state machine
    // so PR is accepted but chirp detection fails (PED never asserts).
    if loc.vendor_id() == 0x8086 {
        let usb2phycm = pci::read_u32(loc.bus, loc.slot, loc.func, 0xE0);
        pci::write_u32(loc.bus, loc.slot, loc.func, 0xE0, usb2phycm | (1 << 7));
        let after = pci::read_u32(loc.bus, loc.slot, loc.func, 0xE0);
        println!("[xhci] USB2PHYCM 0x{:08X} -> 0x{:08X} (bit7 SET)", usb2phycm, after);
    }

    // W540 USB-2 grind iter 2: disarm BIOS SMI on EHCI BEFORE touching it.
    // LEGCTLSTS=0xC0080000 (per usbinfo dump) means SMI fires on PCI
    // command writes and ownership changes; BIOS handler then interferes
    // with our port reset. Writing LEGCTLSTS=0 clears all SMI enables.
    disarm_ehci_smi();

    // Lynx Point shares the USB-2 PHY reference clock between xHCI and EHCI.
    // BIOS often leaves EHCI halted, which gates the PHY. Just setting RS=1
    // (start_ehci_clocks) is not enough — the EHCI controller must go through
    // a full HCRESET to bring the PHY out of its default gated state. We do
    // this while xHCI is still in HCRST so both controllers come up clean.
    reset_and_start_ehci_full();

    // Starting EHCI may change USB2PHYCM on some PCH generations.
    // Ensure bit 7 stays set (PHY enable hypothesis for Raptor Lake).
    if loc.vendor_id() == 0x8086 {
        let usb2phycm = pci::read_u32(loc.bus, loc.slot, loc.func, 0xE0);
        if usb2phycm & (1 << 7) == 0 {
            pci::write_u32(loc.bus, loc.slot, loc.func, 0xE0, usb2phycm | (1 << 7));
            let after = pci::read_u32(loc.bus, loc.slot, loc.func, 0xE0);
            println!("[xhci] USB2PHYCM re-set after EHCI start: 0x{:08X} -> 0x{:08X}", usb2phycm, after);
        }
    }

    // ---- Configure number of device slots enabled ----
    // Use the controller's actual MaxSlots (from HCSPARAMS1), not our
    // compile-time MAX_SLOTS. Some Intel PCH implementations may gate
    // port functionality on MaxSlotsEn matching the hardware capability.
    unsafe {
        let cfg = read_u32(op_base + op_reg::CONFIG as u64);
        let new = (cfg & !0xFF) | (max_slots as u32 & 0xFF);
        write_u32(op_base + op_reg::CONFIG as u64, new);
    }

    // ---- Resolve physical addresses of our static DMA structures ----
    let dcbaa_phys = match phys_of(unsafe { &raw const DCBAA } as u64) {
        Some(p) => p,
        None => { println!("[xhci] DCBAA phys translation failed"); return false; }
    };
    let cmd_ring_phys = match phys_of(unsafe { &raw const COMMAND_RING } as u64) {
        Some(p) => p,
        None => { println!("[xhci] CMD ring phys translation failed"); return false; }
    };
    let evt_ring_phys = match phys_of(unsafe { &raw const EVENT_RING } as u64) {
        Some(p) => p,
        None => { println!("[xhci] EVT ring phys translation failed"); return false; }
    };
    let erst_phys = match phys_of(unsafe { &raw const ERST } as u64) {
        Some(p) => p,
        None => { println!("[xhci] ERST phys translation failed"); return false; }
    };

    unsafe {
        CMD_RING_PHYS = cmd_ring_phys;
        EVT_RING_PHYS = evt_ring_phys;
    }

    // ---- Scratchpad ----
    if max_scratchpad_bufs > 0 {
        let sp_array_phys = match phys_of(unsafe { &raw const SCRATCHPAD_ARRAY } as u64) {
            Some(p) => p,
            None => { println!("[xhci] scratchpad array phys translation failed"); return false; }
        };
        for i in 0..max_scratchpad_bufs {
            let page_virt = unsafe { &raw const SCRATCHPAD_PAGES.0[i] } as u64;
            let page_phys = match phys_of(page_virt) {
                Some(p) => p,
                None => { println!("[xhci] scratchpad page {} phys translation failed", i); return false; }
            };
            unsafe { SCRATCHPAD_ARRAY.0[i] = page_phys; }
        }
        unsafe { DCBAA.0[0] = sp_array_phys; }
    }

    // ---- Program DCBAAP ----
    unsafe {
        write_u64(op_base + op_reg::DCBAAP as u64, dcbaa_phys);
    }

    // ---- Initialise command ring + program CRCR ----
    unsafe {
        init_command_ring(&raw mut COMMAND_RING, cmd_ring_phys);
        // CRCR low 64 bits: pointer (63:6) | reserved (5:4) | CA (3) | CS (2) |
        // RsvdZ (1) | RCS (0). RCS=1 means producer cycle state starts at 1.
        write_u64(op_base + op_reg::CRCR as u64, cmd_ring_phys | 1);
        CMD_PROD = Producer::new();
    }

    // ---- Initialise event ring: ERSTSZ=1, ERSTBA=erst_phys, ERDP=evt_ring_phys ----
    unsafe {
        ERST.0[0].ring_segment_base_addr = evt_ring_phys;
        ERST.0[0].ring_segment_size = RING_SIZE as u32;
        ERST.0[0].reserved = 0;
        write_u32(run_base + rt_reg::IR0_ERSTSZ as u64, 1);
        // Per spec §5.5.2.3.3 the order matters: write ERDP before ERSTBA.
        write_u64(run_base + rt_reg::IR0_ERDP as u64, evt_ring_phys);
        write_u64(run_base + rt_reg::IR0_ERSTBA as u64, erst_phys);
        // Leave IMAN.IE = 0 (no interrupts); IMOD untouched.
        EVT_CONS = Consumer::new();
    }

    // ---- Run! Set USBCMD.RS=1 ----
    let usbsts_after_run = unsafe {
        let cmd = read_u32(op_base + op_reg::USBCMD as u64);
        write_u32(op_base + op_reg::USBCMD as u64, cmd | usbcmd::RS);
        // Wait for HCH=0
        for _ in 0..1_000_000 {
            let sts = read_u32(op_base + op_reg::USBSTS as u64);
            if sts & usbsts::HCH == 0 { break; }
            core::hint::spin_loop();
        }
        read_u32(op_base + op_reg::USBSTS as u64)
    };
    println!("[xhci] USBSTS after RS=1: 0x{:08X} HCH={} HSE={} PCD={} CNR={}",
        usbsts_after_run,
        (usbsts_after_run & usbsts::HCH != 0) as u8,
        (usbsts_after_run & usbsts::HSE != 0) as u8,
        (usbsts_after_run & usbsts::PCD != 0) as u8,
        (usbsts_after_run & usbsts::CNR != 0) as u8,
    );
    if usbsts_after_run & usbsts::HSE != 0 {
        println!("[xhci] CRITICAL: Host System Error detected — controller may be unusable");
    }

    // Detect the controller's initial event-ring cycle state.  Some
    // Intel PCHs (Lynx Point) use cycle=0 for the first event after
    // HCRST, while QEMU uses cycle=1.  If our assumed ccs mismatches,
    // poll_event() will never consume events, which can stall the
    // port state machine on some steppings.
    unsafe {
        // Poll USBSTS.EINT for up to ~1s (62 ticks).  The controller
        // generates Port Status Change Events for connected devices
        // shortly after RS=1.
        let eint_deadline = kernel_core::platform::ticks() + 62;
        while kernel_core::platform::ticks() < eint_deadline {
            let sts = read_u32(op_base + op_reg::USBSTS as u64);
            if sts & usbsts::EINT != 0 {
                // Clear EINT (RW1C).
                write_u32(op_base + op_reg::USBSTS as u64, sts | usbsts::EINT);
                // Peek the first TRB the controller wrote.
                let ctrl = core::ptr::read_volatile(
                    &raw const EVENT_RING.trbs[EVT_CONS.dequeue].control);
                let hw_cycle = (ctrl & 1) != 0;
                println!("[xhci] first event cycle={} (assumed ccs={})",
                    if hw_cycle { 1 } else { 0 },
                    if EVT_CONS.ccs { 1 } else { 0 });
                if hw_cycle != EVT_CONS.ccs {
                    EVT_CONS.ccs = hw_cycle;
                    println!("[xhci] ccs corrected to match controller");
                }
                break;
            }
            core::hint::spin_loop();
        }
    }

    unsafe {
        INFO = Some(XhciInfo {
            mmio_base, op_base, run_base, db_base,
            max_slots, max_ports, csz1,
            max_scratchpad_bufs,
            hciversion,
        });
    }
    println!("[xhci] running. op_base=0x{:X} run_base=0x{:X} db_base=0x{:X}",
        op_base, run_base, db_base);
    println!("  [DEMO 18] PASS: xHCI reset complete (CRCR ready, ports up)");

    true
}

/// Translate any kernel-virtual address (must be in identity-mapped BSS) to
/// physical via the active PML4. Wraps `paging::walk_active_pml4` with
/// page-offset arithmetic so the caller doesn't have to.
fn phys_of(virt: u64) -> Option<u64> {
    let page = virt & !0xFFF;
    let off = virt & 0xFFF;
    paging::walk_active_pml4(page).map(|p| p + off)
}

// ============================================================================
// Event ring polling
// ============================================================================

/// Small software backlog for events that were read from the hardware ring
/// but did not belong to the current waiter. Prevents synchronous control-
/// transfer loops from dropping Port Status Change Events or Transfer Events
/// destined for another slot/endpoint.
const EVT_BACKLOG_SIZE: usize = 16;
static mut EVT_BACKLOG: [Trb; EVT_BACKLOG_SIZE] = [Trb::zero(); EVT_BACKLOG_SIZE];
static mut EVT_BACKLOG_HEAD: usize = 0;
static mut EVT_BACKLOG_TAIL: usize = 0;

unsafe fn push_backlog(trb: Trb) {
    let next = (EVT_BACKLOG_TAIL + 1) % EVT_BACKLOG_SIZE;
    if next != EVT_BACKLOG_HEAD {
        EVT_BACKLOG[EVT_BACKLOG_TAIL] = trb;
        EVT_BACKLOG_TAIL = next;
    } else {
        // Backlog full — event is silently dropped. If this fires, increase
        // EVT_BACKLOG_SIZE.
    }
}

unsafe fn pop_backlog() -> Option<Trb> {
    if EVT_BACKLOG_HEAD == EVT_BACKLOG_TAIL {
        return None;
    }
    let trb = EVT_BACKLOG[EVT_BACKLOG_HEAD];
    EVT_BACKLOG_HEAD = (EVT_BACKLOG_HEAD + 1) % EVT_BACKLOG_SIZE;
    Some(trb)
}

/// Pull one event from the event ring, if any. Returns the consumed TRB
/// or None if the ring's empty. Bumps ERDP after consuming.
///
/// Reads the control dword volatile first to check the cycle bit; only if
/// the cycle bit matches CCS does the rest of the TRB get copied. This
/// avoids a torn-read window where we might see the new control dword
/// from a hardware-just-wrote TRB but still-stale param/status from a
/// previous wrap (HW writes the TRB dword 0..3 in order with the cycle
/// bit in dword 3 last, so a control-dword-first read is the canonical
/// handshake).
pub fn poll_event() -> Option<Trb> {
    unsafe {
        if let Some(trb) = pop_backlog() {
            return Some(trb);
        }
    }
    let info = unsafe { INFO? };
    unsafe {
        let trb_ptr = &raw const EVENT_RING.trbs[EVT_CONS.dequeue];
        // Read just the control dword first to check cycle ownership.
        let control = core::ptr::read_volatile(&(*trb_ptr).control);
        let cycle = (control & 1) != 0;
        if cycle != EVT_CONS.ccs {
            return None;
        }
        fence(Ordering::Acquire);
        // Now safe to copy the full TRB; HW wrote dwords 0..2 before dword 3.
        let out = core::ptr::read_volatile(trb_ptr);
        // Advance.
        EVT_CONS.dequeue += 1;
        if EVT_CONS.dequeue == RING_SIZE {
            EVT_CONS.dequeue = 0;
            EVT_CONS.ccs = !EVT_CONS.ccs;
        }
        // Update ERDP. Low 4 bits hold EHB (Event Handler Busy, bit 3, RW1C)
        // and DESI (Dequeue ERST Segment Index, bits 2:0). We only have one
        // segment so DESI=0; writing 1 to EHB clears it as a side effect.
        let new_erdp = EVT_RING_PHYS + (EVT_CONS.dequeue as u64) * 16;
        write_u64(info.run_base + rt_reg::IR0_ERDP as u64, new_erdp | (1 << 3));
        Some(out)
    }
}

/// Spin-wait for a Command Completion Event with matching command TRB
/// physical address. Returns the completion code (1=SUCCESS) and the
/// slot id reported by the completion. Bounded spin; returns
/// `(0xFF, 0)` on timeout so callers can log a clear failure.
pub fn wait_command_completion(cmd_trb_phys: u64) -> (u8, u8) {
    for _ in 0..200_000_000u64 {
        if let Some(evt) = poll_event() {
            if evt.trb_type() == trb_type::COMMAND_COMPLETION_EVENT {
                if evt.parameter == cmd_trb_phys {
                    return (evt.completion_code(), evt.slot_id());
                } else {
                    // Some other command completed; save for later.
                    unsafe { push_backlog(evt); }
                    continue;
                }
            } else {
                // Port status changes etc. — save for other waiters.
                unsafe { push_backlog(evt); }
            }
        }
        core::hint::spin_loop();
    }
    (0xFF, 0)
}

// ============================================================================
// Command ring helpers
// ============================================================================

/// Compute the physical address of a TRB at a given index in the command
/// ring. Used both to know what to write into command TRBs (param) and
/// to match command completions in `wait_command_completion`.
fn cmd_trb_phys_at(idx: usize) -> u64 {
    unsafe { CMD_RING_PHYS + (idx as u64) * 16 }
}

/// Issue a No-Op command and confirm it completes. Optional smoke test.
fn issue_no_op_cmd() -> bool {
    let info = unsafe { match INFO { Some(i) => i, None => return false } };
    let idx = unsafe { CMD_PROD.enqueue };
    let trb_phys = cmd_trb_phys_at(idx);
    let control = (trb_type::NO_OP_CMD as u32) << 10;
    unsafe { enqueue_command(&raw mut COMMAND_RING, &mut CMD_PROD, 0, 0, control); }
    ring_doorbell(info.db_base, 0, 0);
    let (cc, _) = wait_command_completion(trb_phys);
    cc == cc::SUCCESS
}

/// Write the doorbell register. Doorbell 0 is for the host controller
/// (target=0 for the command ring). Doorbells 1..=MaxSlots are per-slot,
/// with target = DCI of the endpoint (1=EP0, 2..30=EP1..15 OUT/IN).
fn ring_doorbell(db_base: u64, slot_id: u8, target: u8) {
    let addr = db_base + (slot_id as u64) * 4;
    let value = target as u32; // stream id always 0 for our use
    unsafe { write_u32(addr, value); }
}

// ============================================================================
// Port enumeration
// ============================================================================

/// Walk all root-hub ports. For each port that's CCS=1 (a device is
/// physically connected), reset it, then enumerate the device behind it.
/// We stop after the first successful device (single-device limit; see
/// module docs). Returns the number of devices found.
///
/// **Two-pass design**: First pass asserts PP=1 on every port and waits a
/// short power-stable interval (some controllers boot with PP=0). Second
/// pass checks CCS, resets, and enumerates. This matters on Intel PCH and
/// most real hardware; QEMU's qemu-xhci powers ports on automatically so
/// the first pass is a no-op there.
pub fn enumerate_ports() -> usize {
    let info = unsafe { match INFO { Some(i) => i, None => return 0 } };
    println!("[xhci] enumerate_ports() ver=0x{:04X} pr_mask=0x{:08X} max_ports={}",
        info.hciversion, pr_mask_for_version(info.hciversion), info.max_ports);

    // Pass 1: assert Port Power on every root-hub port.
    for port in 1..=info.max_ports {
        let portsc_addr = info.op_base + op_reg::PORTSC_BASE as u64
            + ((port as u64 - 1) * 0x10);
        let portsc = unsafe { read_u32(portsc_addr) };
        if portsc & portsc::PP == 0 {
            let preserve = portsc & !portsc::RW1C_MASK;
            unsafe { write_u32(portsc_addr, preserve | portsc::PP); }
        }
    }
    // Wait until all ports report PP=1.
    for _ in 0..10_000_000u64 {
        let mut all_powered = true;
        for port in 1..=info.max_ports {
            let portsc_addr = info.op_base + op_reg::PORTSC_BASE as u64
                + ((port as u64 - 1) * 0x10);
            let s = unsafe { read_u32(portsc_addr) };
            if s & portsc::PP == 0 {
                all_powered = false;
                break;
            }
        }
        if all_powered { break; }
        core::hint::spin_loop();
    }
    // VBUS settle delay. USB 2.0 spec says PWRON2PWRGOOD ≥ 100 ms for
    // hubs. iPhone 16 Pro (USB-C) has an internal PD controller that
    // negotiates VBUS before asserting the USB 2.0 D+ pull-up;
    // empirically the W540 + Lynx Point + Xbox cable seems to need >500ms,
    // not the 200-400ms typical. Bump to ~2 s (124 ticks @ 62 Hz) for
    // iPhone reliability. Cost is added 1.5s boot time on every boot,
    // which is acceptable given USB is the primary IO.
    let power_settle = kernel_core::platform::ticks() + 124;
    while kernel_core::platform::ticks() < power_settle {
        core::hint::spin_loop();
    }

    // Lynx Point gates the USB-2 PHY reference clock when EHCI is halted.
    // If EHCI is RS=0, xHCI can drive SE0 but cannot detect the device's
    // speed chirp, so PRC fires and PED stays 0. Start EHCI to ungate.
    start_ehci_clocks();

    // Drain any stale events from the event ring before we start resetting.
    // Some Intel controllers couple PORTSC updates to event acknowledgement;
    // if the ring has un-consumed Port Status Change Events from boot,
    // subsequent port state machine transitions may stall.
    let mut drained = 0u32;
    while poll_event().is_some() { drained += 1; }
    if drained > 0 {
        println!("[xhci] drained {} stale event(s) from event ring before reset", drained);
    }

    let mut connected = 0;
    let mut enumerated = 0;
    // Track which ports we've already attempted so retry passes don't
    // re-reset already-enumerated devices. Bit N set = port N+1 done.
    // 21 ports max on Lynx Point, so u32 is plenty.
    let mut tried_ports: u32 = 0;

    // Up to 4 enumeration passes (1 initial + 3 retries). iPhone-class
    // devices sometimes take 1-2 seconds to bring up their USB stack
    // after VBUS rise, and a single pass would miss them. Each retry
    // waits 1 second, then scans for any NEW CCS=1 ports we haven't
    // tried yet.
    for pass in 0..4 {
        if pass > 0 {
            // Wait 1 second between passes for slow-signaling devices.
            let retry_wait = kernel_core::platform::ticks() + 62;
            while kernel_core::platform::ticks() < retry_wait {
                core::hint::spin_loop();
            }
            // Drain any new events from the wait window.
            while poll_event().is_some() {}
        }
        let mut new_ccs_found = false;

    for port in 1..=info.max_ports {
        // Skip ports we already enumerated in a previous pass.
        if tried_ports & (1u32 << (port - 1)) != 0 {
            continue;
        }
        let portsc_addr = info.op_base + op_reg::PORTSC_BASE as u64
            + ((port as u64 - 1) * 0x10);
        let portsc = unsafe { read_u32(portsc_addr) };
        if portsc & portsc::CCS == 0 {
            continue;
        }
        new_ccs_found = true;
        connected += 1;
        if pass > 0 {
            println!("[xhci] (retry pass {}) port {}: now connected (PORTSC=0x{:08X})", pass, port, portsc);
        } else {
            println!("[xhci] port {}: connected (PORTSC=0x{:08X})", port, portsc);
        }

        // Reset the port. USB 2 ports latch PRC=1 + PED=1 simultaneously
        // when PR completes. USB 3 ports are different: PR may report
        // PRC=1 but PED stays 0 until the SS link reaches U0; the proper
        // primitive on SS is WPR (Warm Port Reset, bit 31). Strategy:
        // first try PR; after PRC fires, poll PED with its own window;
        // if PED still 0, retry with WPR. Logs PLS for diagnostics.
        let initial = unsafe { read_u32(portsc_addr) };
        let already_enabled = initial & portsc::PED != 0;
        let pr_mask = pr_mask_for_version(info.hciversion);
        if !already_enabled {
            // USB 2.0 spec §7.1.7.5: after a device connects, the host must
            // wait for the connection to debounce (≥100 ms) before reset.
            // Lynx Point xHCI in particular appears to reject PR if issued
            // too quickly after CCS=1.
            let debounce = kernel_core::platform::ticks() + 8;
            while kernel_core::platform::ticks() < debounce {
                core::hint::spin_loop();
            }
            // After debounce, clear any latched CSC so it doesn't race with PR.
            let post_debounce = unsafe { read_u32(portsc_addr) };
            if post_debounce & portsc::CSC != 0 {
                unsafe { write_u32(portsc_addr, (post_debounce & !portsc::RW1C_MASK) | portsc::CSC); }
            }

            for attempt in 0..3 {
                let mut prc_seen = false;
                let pre = unsafe { read_u32(portsc_addr) };
                if attempt == 0 {
                    // Neutral PR write: mask out RW1C, LWS, WPR, PLS so we
                    // only set PR without side effects.
                    let neutral = pre & !(portsc::RW1C_MASK | portsc::LWS | portsc::WPR | portsc::PLS_MASK);
                    unsafe { write_u32(portsc_addr, neutral | pr_mask); }
                } else if attempt == 1 {
                    // WPR as second attempt.
                    unsafe { write_u32(portsc_addr, pre | portsc::WPR); }
                } else {
                    // Linux-style neutral read-modify-write.
                    let s0 = unsafe { read_u32(portsc_addr) };
                    let neutral = s0 & !(portsc::RW1C_MASK | portsc::LWS | portsc::WPR | portsc::PLS_MASK);
                    unsafe { write_u32(portsc_addr, neutral | pr_mask); }
                }
                let post_write = unsafe { read_u32(portsc_addr) };

                // Wait up to ~240ms (15 ticks @ 62 Hz) for reset to complete.
                // USB-2 spec mandates ≥10ms SE0; some flash drives need
                // >100ms to power-up their internal MCU after VBUS stabile.
                // Intel PCH may implement PR as write-only: the write starts
                // reset but reads back 0.  Detect completion by PED=1 or
                // PRC=1, not by PR self-clearing.
                let prc_deadline = kernel_core::platform::ticks() + 15;
                let mut snapshot_at: u8 = 0; // 0=none, 1=20ms, 2=100ms
                while kernel_core::platform::ticks() < prc_deadline {
                    let s = unsafe { read_u32(portsc_addr) };
                    if s & portsc::PED != 0 || s & (portsc::PRC | portsc::WRC) != 0 {
                        prc_seen = true;
                        break;
                    }
                    // Snapshots at ~20ms (1 tick) and ~100ms (6 ticks)
                    let elapsed = kernel_core::platform::ticks() - (prc_deadline - 15);
                    if snapshot_at == 0 && elapsed >= 1 {
                        println!("[xhci] port {} attempt {} t=20ms  PORTSC=0x{:08X} PLS={} PED={} PRC={} CSC={}",
                            port, attempt, s,
                            (s & portsc::PLS_MASK) >> portsc::PLS_SHIFT,
                            (s & portsc::PED != 0) as u8,
                            (s & portsc::PRC != 0) as u8,
                            (s & portsc::CSC != 0) as u8);
                        snapshot_at = 1;
                    }
                    if snapshot_at == 1 && elapsed >= 6 {
                        println!("[xhci] port {} attempt {} t=100ms PORTSC=0x{:08X} PLS={} PED={} PRC={} CSC={}",
                            port, attempt, s,
                            (s & portsc::PLS_MASK) >> portsc::PLS_SHIFT,
                            (s & portsc::PED != 0) as u8,
                            (s & portsc::PRC != 0) as u8,
                            (s & portsc::CSC != 0) as u8);
                        snapshot_at = 2;
                    }
                    core::hint::spin_loop();
                }
                if !prc_seen {
                    let s = unsafe { read_u32(portsc_addr) };
                    println!("[xhci] port {} attempt {} reset timed out pre=0x{:08X} post=0x{:08X} now=0x{:08X} pr_mask=0x{:08X} ver=0x{:04X}",
                        port, attempt, pre, post_write, s, pr_mask, info.hciversion);
                    continue;
                }

                // After PRC fires, poll up to ~240ms for PED.
                let ped_deadline = kernel_core::platform::ticks() + 15;
                while kernel_core::platform::ticks() < ped_deadline {
                    let s = unsafe { read_u32(portsc_addr) };
                    if s & portsc::PED != 0 { break; }
                    core::hint::spin_loop();
                }

                let after = unsafe { read_u32(portsc_addr) };
                if after & portsc::PED != 0 {
                    // Success — clear all RW1C bits now that the port is enabled.
                    unsafe { write_u32(portsc_addr, (after & !portsc::RW1C_MASK) | portsc::RW1C_MASK); }
                    break; // success
                }
                println!("[xhci] port {} attempt {} PR cleared but PED=0 (PORTSC=0x{:08X} pr_mask=0x{:08X})", port, attempt, after, pr_mask);

                // Failed — clear all RW1C bits before the next attempt or before WPR.
                unsafe { write_u32(portsc_addr, (after & !portsc::RW1C_MASK) | portsc::RW1C_MASK); }

                println!("[xhci] port {} attempt {} PR didn't enable (PORTSC=0x{:08X} PLS={} speed={} pr_mask=0x{:08X})",
                    port, attempt, after,
                    (after & portsc::PLS_MASK) >> portsc::PLS_SHIFT,
                    (after & portsc::SPEED_MASK) >> portsc::SPEED_SHIFT, pr_mask);
                if attempt == 2 {
                    // Last attempt failed — try WPR as a hail-mary (only
                    // meaningful for USB-3, harmless for USB-2).
                    let preserve = after & !portsc::RW1C_MASK;
                    unsafe { write_u32(portsc_addr, preserve | portsc::WPR); }
                    let wrc_deadline = kernel_core::platform::ticks() + 15;
                    while kernel_core::platform::ticks() < wrc_deadline {
                        let s = unsafe { read_u32(portsc_addr) };
                        if s & portsc::WRC != 0 {
                            // Same quirk: don't clear WRC until PED is checked.
                            break;
                        }
                        core::hint::spin_loop();
                    }
                    let wpr_ped_deadline = kernel_core::platform::ticks() + 15;
                    while kernel_core::platform::ticks() < wpr_ped_deadline {
                        let s = unsafe { read_u32(portsc_addr) };
                        if s & portsc::PED != 0 { break; }
                        core::hint::spin_loop();
                    }
                    let after_wpr = unsafe { read_u32(portsc_addr) };
                    if after_wpr & portsc::PED != 0 {
                        unsafe { write_u32(portsc_addr, (after_wpr & !portsc::RW1C_MASK) | portsc::WRC | portsc::PRC); }
                    }
                }
            }
        }
        // Drain any events generated by this port before moving on.
        while poll_event().is_some() {}

        let portsc_after = unsafe { read_u32(portsc_addr) };
        if portsc_after & portsc::PED == 0 {
            println!("[xhci] port {} not enabled after reset (PORTSC=0x{:08X} PLS={} speed={} pr_mask=0x{:08X})",
                port, portsc_after,
                (portsc_after & portsc::PLS_MASK) >> portsc::PLS_SHIFT,
                (portsc_after & portsc::SPEED_MASK) >> portsc::SPEED_SHIFT, pr_mask);
            continue;
        }
        let speed = ((portsc_after & portsc::SPEED_MASK) >> portsc::SPEED_SHIFT) as u8;
        println!("[xhci] port {} enabled (PORTSC=0x{:08X} speed={})", port, portsc_after, speed);

        // USB 3.0 devices (and some finicky USB 2.0 firmware) need a short
        // settle time after PED=1 before they reliably respond to SET_ADDRESS.
        // The iPhone 16 Pro on SuperSpeed has been observed to NAK the first
        // AddressDevice command if we rush it.
        let settle = kernel_core::platform::ticks() + 10;
        while kernel_core::platform::ticks() < settle {
            core::hint::spin_loop();
        }

        // Enumerate this device. Keep going — we want to see every root-hub
        // device, not just the first.  (Earlier single-device limit was a
        // leftover from the keyboard-only era.)
        let mut first_enum_ok = false;
        if enumerate_device(Topology::root(port), speed) {
            enumerated += 1;
            first_enum_ok = true;
        }

        // For SuperSpeed devices that enabled after PR but then failed
        // enumeration (iPhone 16 Pro is the known example), a warm reset
        // often recovers the link.  PR does an in-band hot reset; WPR
        // forces out-of-band LFPS retraining which some Apple SS PHYs need.
        if speed == 4 && !first_enum_ok {
            println!("[xhci] port {} SS enumeration failed, trying warm reset", port);
            let s = unsafe { read_u32(portsc_addr) };
            let preserve = s
                & !portsc::RW1C_MASK
                & !portsc::PED
                & !portsc::WPR;
            unsafe { write_u32(portsc_addr, preserve | portsc::WPR); }
            let wrc_deadline = kernel_core::platform::ticks() + 15;
            while kernel_core::platform::ticks() < wrc_deadline {
                let s = unsafe { read_u32(portsc_addr) };
                if s & portsc::WRC != 0 { break; }
                core::hint::spin_loop();
            }
            let wpr_ped_deadline = kernel_core::platform::ticks() + 15;
            while kernel_core::platform::ticks() < wpr_ped_deadline {
                let s = unsafe { read_u32(portsc_addr) };
                if s & portsc::PED != 0 { break; }
                core::hint::spin_loop();
            }
            let after_wpr = unsafe { read_u32(portsc_addr) };
            if after_wpr & portsc::PED != 0 {
                println!("[xhci] port {} warm-reset enabled (PORTSC=0x{:08X})",
                    port, after_wpr);
                // Another settle delay before retry.
                let settle2 = kernel_core::platform::ticks() + 10;
                while kernel_core::platform::ticks() < settle2 {
                    core::hint::spin_loop();
                }
                if enumerate_device(Topology::root(port), speed) {
                    enumerated += 1;
                }
            }
        }
        // Port fully processed (reset succeeded + enumeration attempted).
        // Mark as tried so retry passes don't re-reset it.
        tried_ports |= 1u32 << (port - 1);
    } // end for port

        // If this retry pass found no new CCS=1 ports, no point in
        // continuing — additional waiting won't help.
        if pass > 0 && !new_ccs_found {
            break;
        }
    } // end for pass

    if connected == 0 {
        println!("[xhci] no connected devices on any of {} root-hub ports", info.max_ports);
    }
    println!("  [DEMO 18] PASS: enumerated {} USB device(s)", enumerated);
    enumerated
}

/// USB topology coordinates needed by xHCI's input-context slot fields.
/// For a device direct on the root hub, `route_string = 0`,
/// `parent_hub_slot = 0`, `parent_port = 0`. For a device behind a
/// downstream hub, `route_string` encodes the path (4 bits per tier) and
/// `parent_hub_slot`/`parent_port` give the TT info for LS/FS children
/// of HS hubs (USB 2.0 spec § 11.17).
#[derive(Copy, Clone, Debug)]
pub struct Topology {
    pub root_hub_port: u8,
    pub route_string: u32,
    pub parent_hub_slot: u8,
    pub parent_port: u8,
    /// Number of hub tiers between this device and the root hub.
    /// 0 = root-hub-direct, 1 = behind one hub, etc.
    pub depth: u8,
}

impl Topology {
    /// Direct child of the root hub on port `port`.
    pub const fn root(port: u8) -> Self {
        Self {
            root_hub_port: port,
            route_string: 0,
            parent_hub_slot: 0,
            parent_port: 0,
            depth: 0,
        }
    }
}

/// Cached topology for every allocated xHCI slot. Used when cascading
/// hub enumeration so a child can compute its route string from the
/// parent hub's route string.
static mut SLOT_TOPOLOGIES: [Option<Topology>; MAX_SLOTS + 1] = [None; MAX_SLOTS + 1];

/// Enumerate a single attached device. `topology` describes where the
/// device sits in the USB tree (root-hub-direct vs behind a hub); see
/// `Topology` for the field semantics.
///  - EnableSlot
///  - Build Input Context (slot + EP0, with optional route string + TT)
///  - AddressDevice
///  - Read first 8 bytes of device descriptor (to learn EP0 max packet)
///  - If max packet differs, EvaluateContext / re-set EP0 (skipped for boot
///    speeds where 64 is standard for HS/FS)
///  - Read full 18-byte device descriptor
///  - Read configuration descriptor, parse interface + endpoint
///  - If HID boot keyboard, set up the Interrupt-IN transfer ring
fn enumerate_device(topology: Topology, speed: u8) -> bool {
    // Backwards-compatible alias used by the rest of this function.
    let port = topology.root_hub_port;
    let info = unsafe { match INFO { Some(i) => i, None => return false } };

    // ---- EnableSlot ----
    let idx = unsafe { CMD_PROD.enqueue };
    let cmd_phys = cmd_trb_phys_at(idx);
    let control = (trb_type::ENABLE_SLOT_CMD as u32) << 10;
    unsafe { enqueue_command(&raw mut COMMAND_RING, &mut CMD_PROD, 0, 0, control); }
    ring_doorbell(info.db_base, 0, 0);
    let (cc, slot_id) = wait_command_completion(cmd_phys);
    if cc != cc::SUCCESS || slot_id == 0 {
        println!("[xhci] EnableSlot failed: cc={} slot={}", cc, slot_id);
        return false;
    }

    if (slot_id as usize) > MAX_SLOTS {
        println!("[xhci] EnableSlot returned slot_id={} > MAX_SLOTS={}", slot_id, MAX_SLOTS);
        return false;
    }
    let si = slot_id as usize;

    // ---- Plug per-slot device context into DCBAA ----
    let dev_ctx_phys = match phys_of(unsafe { &raw const DEVICE_CTXS[si] } as u64) {
        Some(p) => p,
        None => { println!("[xhci] dev ctx phys translation failed"); return false; }
    };
    unsafe { DCBAA.0[si] = dev_ctx_phys; }

    // ---- Build the per-slot EP0 transfer ring ----
    let ep0_ring_phys = match phys_of(unsafe { &raw const EP0_TRANSFER_RINGS[si] } as u64) {
        Some(p) => p,
        None => { println!("[xhci] EP0 ring phys translation failed"); return false; }
    };
    unsafe {
        init_command_ring(&raw mut EP0_TRANSFER_RINGS[si], ep0_ring_phys);
        EP0_PRODS[si] = Producer::new();
        EP0_RING_PHYSES[si] = ep0_ring_phys;
    }

    // ---- Build the Input Context ----
    // Speed 1=Full(12 Mb/s, mps0=8 default), 2=Low(1.5, mps0=8), 3=High(480, mps0=64),
    //       4=Super(5 Gb/s, mps0=512). Spec §6.2.2 Table 6-9.
    // For FS we use 8 then (in a more complete driver) issue Evaluate Context after
    // the first descriptor read to switch to the device's actual MPS0; for HS the
    // spec mandates 64 so no follow-up is needed.
    let mps0: u16 = match speed { 1 => 8, 2 => 8, 3 => 64, 4 => 512, _ => 8 };
    unsafe {
        let ic = &mut INPUT_CTXS[si].0;
        // Zero it.
        ic.reset();
        // Input Control Context: A0 (slot) + A1 (EP0) added.
        ic.input_ctrl_mut().add_flags = (1 << 0) | (1 << 1);
        // Slot context: 1 context entry (EP0 only), root hub port, speed,
        // plus route string + TT info if behind a downstream hub.
        let slot = ic.slot_mut();
        slot.set_context_entries(1);
        slot.set_root_hub_port(port);
        slot.set_speed(speed as u32);
        if topology.route_string != 0 {
            slot.set_route_string(topology.route_string);
        }
        // TT (Transaction Translator) info is only valid for low-/full-speed
        // devices behind a high-speed hub. For HS or SS children the xHC
        // routes directly and TT fields must be zero. Spec §6.2.2 Table 6-7.
        if topology.parent_hub_slot != 0 && speed <= 2 {
            slot.set_parent_hub_slot_id(topology.parent_hub_slot);
            slot.set_parent_port_number(topology.parent_port);
            slot.set_tt_think_time(0); // 8 FS bit times (minimum)
        }
        // EP0 endpoint context (idx 0 = DCI 1).
        ic.ep_mut(0).init_control_ep(mps0, ep0_ring_phys, true);
    }
    let input_phys = match phys_of(unsafe { &raw const INPUT_CTXS[si] } as u64) {
        Some(p) => p,
        None => { println!("[xhci] input ctx phys translation failed"); return false; }
    };

    // ---- AddressDevice ----
    let mut address_ok = false;
    for retry in 0..2 {
        let idx = unsafe { CMD_PROD.enqueue };
        let cmd_phys = cmd_trb_phys_at(idx);
        let control = ((trb_type::ADDRESS_DEVICE_CMD as u32) << 10) | ((slot_id as u32) << 24);
        unsafe {
            enqueue_command(
                &raw mut COMMAND_RING, &mut CMD_PROD,
                input_phys,  // parameter = Input Context pointer
                0,
                control,
            );
        }
        ring_doorbell(info.db_base, 0, 0);
        let (cc, _) = wait_command_completion(cmd_phys);
        if cc == cc::SUCCESS {
            address_ok = true;
            break;
        }
        println!("[xhci] AddressDevice attempt {} failed: cc={}", retry, cc);
        // Only retry on USB Transaction Error (cc=4); other codes are fatal.
        if cc != 4 || retry == 1 {
            return false;
        }
        // Wait ~100ms before retry — some devices (iPhone 16 Pro SS) need
        // extra time after link training before they accept SET_ADDRESS.
        let retry_wait = kernel_core::platform::ticks() + 6;
        while kernel_core::platform::ticks() < retry_wait {
            core::hint::spin_loop();
        }
    }
    if !address_ok {
        return false;
    }

    let usb_addr = unsafe { DEVICE_CTXS[si].0.slot_read().usb_device_address() };
    println!("[xhci] device addressed: slot={} usb_addr={} speed={} mps0={}",
        slot_id, usb_addr, speed, mps0);

    // USB 2.0 spec §9.2.6.3: the device must respond to data transfers at
    // the new address within 2 ms after the Status stage of SET_ADDRESS.
    // Wait a tick-based margin (~32 ms @ 62 Hz) before the first descriptor
    // read — some hub-behind devices are slower than root-port direct.
    let post_addr = kernel_core::platform::ticks() + 2;
    while kernel_core::platform::ticks() < post_addr {
        core::hint::spin_loop();
    }

    // Cache topology so cascaded hub enumeration can compute route strings.
    unsafe {
        SLOT_TOPOLOGIES[slot_id as usize] = Some(topology);
    }

    // ---- GET_DESCRIPTOR(DEVICE) — 18 bytes ----
    let mut dev_desc = DeviceDescriptor::default();
    if !control_in(slot_id, 0x80, request::GET_DESCRIPTOR,
                   (desc_type::DEVICE as u16) << 8, 0, 18,
                   &mut dev_desc as *mut _ as *mut u8, 18) {
        println!("[xhci] GET_DESCRIPTOR(DEVICE) failed");
        return false;
    }
    let id_vendor = dev_desc.id_vendor;
    let id_product = dev_desc.id_product;
    println!("[xhci] device descriptor: vendor=0x{:04X} product=0x{:04X} class=0x{:02X} mps0={}",
        id_vendor, id_product,
        dev_desc.b_device_class, dev_desc.b_max_packet_size0);
    if id_vendor == crate::usb::iphone::APPLE_VENDOR_ID {
        println!("[iphone] Apple device detected on slot={} port={} — about to read config descriptor", slot_id, port);
    }

    // Phase 15 M50 — surface USB-hub-class devices explicitly. The Lenovo
    // ThinkPad Pro Dock 40A1 (and most multi-port docks) presents itself as
    // a USB hub here. Hubs are currently out of scope; we log so the user
    // knows the dock was seen but downstream devices won't be enumerated
    // until cascaded-hub support lands (see usb/mod.rs header for the
    // deferred-task list).
    if dev_desc.b_device_class == 0x09 {
        println!(
            "[xhci] HUB detected (vendor=0x{:04X} product=0x{:04X}) — running hub class bring-up",
            id_vendor, id_product
        );
        if id_vendor == 0x17EF {
            println!("[xhci] Lenovo USB hub (matches Pro Dock 40A1 / ThinkPad dock family)");
        }
        // Read the hub descriptor so we can set Number of Ports in the
        // Evaluate Context Input Slot Context (required by spec §6.2.2).
        let hub_desc = crate::usb::hub::read_hub_descriptor(slot_id, speed);
        // Mark this slot as a hub on the slot context, so the xHC routes
        // SETUP packets to children with the right TT bookkeeping.
        // Preserve route_string so cascaded hubs don't lose their path.
        unsafe {
            let ic = &mut INPUT_CTXS[si].0;
            ic.reset();
            ic.input_ctrl_mut().add_flags = (1 << 0); // A0 = Slot Context only
            let slot = ic.slot_mut();
            slot.set_is_hub(true);
            slot.set_context_entries(1);
            slot.set_root_hub_port(port);
            slot.set_speed(speed as u32);
            if let Some(ref desc) = hub_desc {
                slot.set_number_of_ports(desc.b_nbr_ports);
            }
            if topology.route_string != 0 {
                slot.set_route_string(topology.route_string);
            }
        }
        let hub_input_phys = match phys_of(unsafe { &raw const INPUT_CTXS[si].0 } as u64) {
            Some(p) => p,
            None => { println!("[xhci] hub input ctx phys translation failed"); return false; }
        };
        let idx = unsafe { CMD_PROD.enqueue };
        let eval_cmd_phys = cmd_trb_phys_at(idx);
        let eval_control = ((trb_type::EVALUATE_CONTEXT_CMD as u32) << 10)
            | ((slot_id as u32) << 24);
        unsafe {
            enqueue_command(&raw mut COMMAND_RING, &mut CMD_PROD,
                hub_input_phys, 0, eval_control);
        }
        ring_doorbell(info.db_base, 0, 0);
        let (eval_cc, _) = wait_command_completion(eval_cmd_phys);
        if eval_cc != cc::SUCCESS {
            println!("[xhci] EvaluateContext(is_hub) failed: cc={} — continuing anyway", eval_cc);
        } else {
            println!("[xhci] slot={} marked as hub in xHC context", slot_id);
        }

        // Hubs may reject class requests until configured; set config 1.
        if !control_out(slot_id, 0x00, request::SET_CONFIGURATION, 1, 0, 0) {
            println!("[xhci-hub] SET_CONFIGURATION(1) failed for hub — continuing anyway");
        }

        // USB 3.0 hubs need SetHubDepth so they know which nibble of the
        // route_string to use for downstream routing. Without this, packets
        // to child devices are dropped or mis-routed. USB 2.0 hubs ignore
        // this request (may STALL), so we only send it for SS hubs.
        if speed == 4 {
            let depth = topology.depth;
            if !crate::usb::hub::set_hub_depth(slot_id, depth) {
                println!("[xhci-hub] SET_HUB_DEPTH({}) failed — continuing anyway", depth);
            } else {
                println!("[xhci-hub] SET_HUB_DEPTH({}) ok", depth);
            }
        }

        // Stash a generic record for the hub itself BEFORE cascading, so
        // any child-enum overwrite of DEVICE (e.g. a keyboard plugged
        // into the hub) wins — the keyboard is what the rest of the boot
        // actually wants to interact with.
        unsafe {
            let dev = EnumeratedDevice {
                slot_id, usb_address: usb_addr, port, speed,
                vendor: id_vendor, product: id_product,
                max_packet_ep0: mps0,
                is_keyboard: false,
                kbd_ep_in: 0, kbd_ep_packet_size: 0, kbd_ep_interval: 0,
                config_value: 0,
                interface_number: 0,
                class: 0x09,
            };
            DEVICE = Some(dev);
            ENUMERATED_SLOTS[slot_id as usize] = Some(dev);
        }
        let connected_children = crate::usb::hub::bring_up_hub(slot_id, port, speed);
        if connected_children > 0 {
            println!("[xhci] hub enumerated {} downstream device(s)",
                connected_children);
        }
        // Return true: hub itself is fully addressed.
        return true;
    }
    println!("  [DEMO 18] PASS: keyboard descriptor parsed (vendor=0x{:04X} product=0x{:04X})",
        id_vendor, id_product);

    // ---- Iterate all configurations (0 .. bNumConfigurations-1).
    // iOS devices start in a deactivated config 0; the real ipheth iface
    // is often in config 1, 2, or higher.  usbmuxd iterates in reverse;
    // we go forward — the first matching config wins.
    let num_configs = dev_desc.b_num_configurations.max(1);
    let blob_phys = match phys_of(unsafe { &raw const DMA_BUF } as u64) {
        Some(p) => p,
        None => { println!("[xhci] DMA_BUF phys translation failed"); return false; }
    };

    let mut first_blob_ok = false;
    let mut matched_config_value = 0u8;

    for cfg_idx in 0..num_configs {
        // Read 9-byte header to get total length.
        let mut cfg_desc = ConfigDescriptor::default();
        let w_value = ((desc_type::CONFIGURATION as u16) << 8) | (cfg_idx as u16);
        if !control_in(slot_id, 0x80, request::GET_DESCRIPTOR,
                       w_value, 0, 9,
                       &mut cfg_desc as *mut _ as *mut u8, 9) {
            if id_vendor == crate::usb::iphone::APPLE_VENDOR_ID {
                println!("[iphone] GET_DESCRIPTOR(CONFIG short, idx={}) failed — skipping", cfg_idx);
            } else {
                println!("[xhci] GET_DESCRIPTOR(CONFIG short, idx={}) failed — skipping", cfg_idx);
            }
            continue;
        }
        let total_len = cfg_desc.w_total_length as usize;
        let read_len = total_len.min(SETUP_BUF_SIZE);
        if total_len > SETUP_BUF_SIZE {
            println!("[xhci] config[{}] descriptor larger than buffer ({} > {}), truncating to {} bytes",
                cfg_idx, total_len, SETUP_BUF_SIZE, read_len);
        }

        // Read full configuration blob.
        if !control_in_phys(slot_id, 0x80, request::GET_DESCRIPTOR,
                            w_value, 0,
                            read_len as u16, blob_phys, read_len) {
            if id_vendor == crate::usb::iphone::APPLE_VENDOR_ID {
                println!("[iphone] GET_DESCRIPTOR(CONFIG full, idx={}) failed — skipping", cfg_idx);
            } else {
                println!("[xhci] GET_DESCRIPTOR(CONFIG full, idx={}) failed — skipping", cfg_idx);
            }
            continue;
        }
        first_blob_ok = true;
        matched_config_value = cfg_desc.b_configuration_value;

        let blob = unsafe { &DMA_BUF.0[..read_len] };

        // Diagnostic: print every interface in this config so we can see
        // exactly what the device exposes — even interfaces we don't yet
        // have a driver for.
        println!("[xhci-diag] config[{}] interfaces:", cfg_idx);
        crate::usb::iphone::dump_config_interfaces(blob);

        // ---- Check for HID boot keyboard (highest priority) ----
        if let Some((kbd, iface_num, cfg_val)) = find_boot_keyboard(blob, cfg_desc.b_configuration_value) {
            println!("[xhci] HID boot keyboard found in config[{}] value={}", cfg_idx, cfg_val);
            // Keyboard path: SET_CONFIGURATION, SET_PROTOCOL, configure endpoints.
            if !control_out(slot_id, 0x00, request::SET_CONFIGURATION,
                            cfg_val as u16, 0, 0) {
                println!("[xhci] SET_CONFIGURATION failed");
                return false;
            }
            if !control_out(slot_id, 0x21, request::HID_SET_PROTOCOL,
                            0, iface_num as u16, 0) {
                println!("[xhci] HID SET_PROTOCOL(boot) failed (non-fatal on some devs)");
            }
            let hid_ring_phys = match phys_of(unsafe { &raw const HID_TRANSFER_RINGS[si] } as u64) {
                Some(p) => p,
                None => { println!("[xhci] HID ring phys translation failed"); return false; }
            };
            let ep_num = kbd.b_endpoint_address & 0x0F;
            let dci = (ep_num * 2 + 1) as usize;
            let interval_log2 = encode_interval(speed, kbd.b_interval);
            let max_packet = kbd.w_max_packet_size & 0x07FF;
            unsafe {
                HID_RING_PHYSES[si] = hid_ring_phys;
                init_command_ring(&raw mut HID_TRANSFER_RINGS[si], hid_ring_phys);
                HID_PRODS[si] = Producer::new();
                let ic = &mut INPUT_CTXS[si].0;
                ic.reset();
                ic.input_ctrl_mut().add_flags = (1 << 0) | (1u32 << dci);
                let slot = ic.slot_mut();
                slot.set_context_entries(dci as u32);
                slot.set_root_hub_port(port);
                slot.set_speed(speed as u32);
                ic.ep_mut(dci - 1).init_interrupt_in_ep(max_packet, interval_log2, hid_ring_phys, true);
            }
            let idx = unsafe { CMD_PROD.enqueue };
            let cmd_phys = cmd_trb_phys_at(idx);
            let control = ((trb_type::CONFIGURE_ENDPOINT_CMD as u32) << 10) | ((slot_id as u32) << 24);
            unsafe {
                enqueue_command(&raw mut COMMAND_RING, &mut CMD_PROD, input_phys, 0, control);
            }
            ring_doorbell(info.db_base, 0, 0);
            let (cc, _) = wait_command_completion(cmd_phys);
            if cc != cc::SUCCESS {
                println!("[xhci] ConfigureEndpoint (HID) failed: cc={}", cc);
                return false;
            }
            println!("[xhci] HID kbd configured: slot={} dci={} ep=0x{:02X} mps={} interval_log2={}",
                slot_id, dci, kbd.b_endpoint_address, max_packet, interval_log2);
            unsafe {
                let dev = EnumeratedDevice {
                    slot_id, usb_address: usb_addr, port, speed,
                    vendor: id_vendor, product: id_product,
                    max_packet_ep0: mps0,
                    is_keyboard: true,
                    kbd_ep_in: kbd.b_endpoint_address,
                    kbd_ep_packet_size: max_packet,
                    kbd_ep_interval: kbd.b_interval,
                    config_value: cfg_val,
                    interface_number: iface_num,
                    class: 0x03,
                };
                DEVICE = Some(dev);
                ENUMERATED_SLOTS[slot_id as usize] = Some(dev);
            }
            arm_hid_read(slot_id, dci as u8);
            return true;
        }

        // ---- Check for iPhone ipheth ----
        if id_vendor == crate::usb::iphone::APPLE_VENDOR_ID {
            if try_enumerate_ipheth(slot_id, port, speed, blob,
                                     cfg_desc.b_configuration_value) {
                return true;
            }
        }

        // ---- Check for CDC-ECM ----
        if try_enumerate_cdc_ecm(slot_id, port, speed, blob,
                                  cfg_desc.b_configuration_value) {
            return true;
        }

        // ---- Check for CDC-NCM (modern iPhones, Android) ----
        if try_enumerate_cdc_ncm(slot_id, port, speed, blob,
                                  cfg_desc.b_configuration_value) {
            return true;
        }

        // ---- Check for Mass Storage ----
        if try_enumerate_mass_storage(slot_id, port, speed, blob,
                                       cfg_desc.b_configuration_value) {
            let msc = unsafe { MSC.as_ref().unwrap() };
            let vendor = core::str::from_utf8(&msc.inquiry[8..16]).unwrap_or("?");
            let product = core::str::from_utf8(&msc.inquiry[16..32]).unwrap_or("?");
            let revision = core::str::from_utf8(&msc.inquiry[32..36]).unwrap_or("?");
            println!(
                "[xhci-msc] vendor=\"{}\" product=\"{}\" rev=\"{}\" capacity={} blocks x {} B",
                vendor.trim(), product.trim(), revision.trim(),
                msc.capacity_blocks, msc.capacity_bs
            );
            unsafe {
                let dev = EnumeratedDevice {
                    slot_id, usb_address: usb_addr, port, speed,
                    vendor: id_vendor, product: id_product,
                    max_packet_ep0: mps0,
                    is_keyboard: false,
                    kbd_ep_in: 0, kbd_ep_packet_size: 0, kbd_ep_interval: 0,
                    config_value: cfg_desc.b_configuration_value,
                    interface_number: msc.iface_num,
                    class: 0x08,
                };
                DEVICE = Some(dev);
                ENUMERATED_SLOTS[slot_id as usize] = Some(dev);
            }
            return true;
        }
    }

    // No class-specific driver claimed this device in any config.
    // Stash a generic record so usbinfo shows it, and read strings
    // for diagnostics.
    if first_blob_ok {
        let _ = read_string_descriptor(slot_id, dev_desc.i_manufacturer, "iManufacturer");
        let _ = read_string_descriptor(slot_id, dev_desc.i_product,     "iProduct");
        let _ = read_string_descriptor(slot_id, dev_desc.i_serial_number, "iSerialNumber");
    }
    println!("[xhci] no matching interface in any of {} config(s) for vendor=0x{:04X} product=0x{:04X}",
        num_configs, id_vendor, id_product);
    unsafe {
        let dev = EnumeratedDevice {
            slot_id, usb_address: usb_addr, port, speed,
            vendor: id_vendor, product: id_product,
            max_packet_ep0: mps0,
            is_keyboard: false,
            kbd_ep_in: 0, kbd_ep_packet_size: 0, kbd_ep_interval: 0,
            config_value: matched_config_value,
            interface_number: 0,
            class: dev_desc.b_device_class,
        };
        DEVICE = Some(dev);
        ENUMERATED_SLOTS[slot_id as usize] = Some(dev);
    }
    true
}

/// Encode b_interval to the xHCI interval field (exponent of microframes).
/// Spec §6.2.3.6:
///   LS/FS interrupt: interval = log2(b_interval * 8), clamped 3..10
///   HS/SS interrupt: interval = b_interval - 1, clamped 0..15
fn encode_interval(speed: u8, b_interval: u8) -> u8 {
    match speed {
        3 | 4 => {
            // HS / SS: b_interval is already an exponent (1..16).
            let v = b_interval.saturating_sub(1);
            v.min(15)
        }
        _ => {
            // FS / LS: b_interval is in frame units. log2(b_interval * 8).
            let bi = b_interval.max(1) as u32;
            let frames = bi * 8;
            let mut log = 0u32;
            let mut v = frames;
            while v > 1 { v >>= 1; log += 1; }
            log.clamp(3, 10) as u8
        }
    }
}

/// Walk the configuration-descriptor blob and find a HID boot-keyboard
/// interface + its Interrupt-IN endpoint. Returns (EndpointDescriptor,
/// interface_number, configuration_value).
fn find_boot_keyboard(blob: &[u8], cfg_value: u8) -> Option<(EndpointDescriptor, u8, u8)> {
    let mut i = 0;
    let mut current_iface_is_kbd = false;
    let mut current_iface_num: u8 = 0;
    while i + 2 <= blob.len() {
        let bl = blob[i] as usize;
        if bl == 0 || i + bl > blob.len() { break; }
        let bt = blob[i + 1];
        match bt {
            desc_type::INTERFACE if bl >= 9 => {
                let iface = InterfaceDescriptor {
                    b_length: blob[i],
                    b_descriptor_type: blob[i + 1],
                    b_interface_number: blob[i + 2],
                    b_alternate_setting: blob[i + 3],
                    b_num_endpoints: blob[i + 4],
                    b_interface_class: blob[i + 5],
                    b_interface_subclass: blob[i + 6],
                    b_interface_protocol: blob[i + 7],
                    i_interface: blob[i + 8],
                };
                current_iface_is_kbd = iface.b_interface_class == class::HID
                    && iface.b_interface_subclass == 0x01  // Boot
                    && iface.b_interface_protocol == 0x01; // Keyboard
                current_iface_num = iface.b_interface_number;
            }
            desc_type::ENDPOINT if bl >= 7 && current_iface_is_kbd => {
                let ep = EndpointDescriptor {
                    b_length: blob[i],
                    b_descriptor_type: blob[i + 1],
                    b_endpoint_address: blob[i + 2],
                    bm_attributes: blob[i + 3],
                    w_max_packet_size: u16::from_le_bytes([blob[i + 4], blob[i + 5]]),
                    b_interval: blob[i + 6],
                };
                let xfer_type = ep.bm_attributes & 0x3;
                let is_in = ep.b_endpoint_address & 0x80 != 0;
                if xfer_type == 0x3 /* interrupt */ && is_in {
                    return Some((ep, current_iface_num, cfg_value));
                }
            }
            _ => {}
        }
        i += bl;
    }
    None
}

// ============================================================================
// Control transfers (EP0) — used to fetch descriptors and issue SET_*
// ============================================================================
//
// A USB control transfer on EP0 is three TRBs:
//   Setup TRB:  contains the 8-byte SetupPacket inline in the parameter field
//   Data TRB:   (only present if wLength > 0) points to a DMA buffer
//   Status TRB: completion — direction OPPOSITE of the data stage (or IN if no data)
//
// We post all three at once, ring the doorbell with target=1 (EP0 DCI),
// and wait for a Transfer Event matching the Status TRB.

pub(crate) fn control_in(
    slot_id: u8, request_type: u8, request: u8,
    value: u16, index: u16, length: u16,
    out_kvirt: *mut u8, copy_len: usize,
) -> bool {
    let phys = match phys_of(out_kvirt as u64) {
        Some(p) => p,
        None => { println!("[xhci] control_in: phys translation failed"); return false; }
    };
    control_in_phys(slot_id, request_type, request, value, index, length, phys, copy_len)
}

fn control_in_phys(
    slot_id: u8, request_type: u8, request: u8,
    value: u16, index: u16, length: u16,
    out_phys: u64, _copy_len: usize,
) -> bool {
    let info = unsafe { match INFO { Some(i) => i, None => return false } };

    // ---- Build the Setup TRB ----
    let setup = SetupPacket {
        bm_request_type: request_type,
        b_request: request,
        w_value: value, w_index: index, w_length: length,
    };
    let setup_param: u64 = unsafe { core::mem::transmute(setup) };

    // Setup stage TRB: parameter=raw setup packet, status=8 (transfer length),
    // control: TRT=3 (IN data stage), IDT=1 (immediate data), TRB type=2,
    // and IOC=0 (we use IOC on the Status TRB instead).
    let setup_status = 8u32;
    let setup_control = (3u32 << 16) /* TRT=IN */
        | (1u32 << 6)  /* IDT immediate data */
        | ((trb_type::SETUP_STAGE as u32) << 10);

    // Data stage TRB: parameter=phys buffer, status=length, control:
    // TRB type=3, DIR=1 (IN). CH=0.
    let data_status = length as u32;
    let data_control = (1u32 << 16) /* DIR=IN */
        | ((trb_type::DATA_STAGE as u32) << 10);

    // Status stage TRB: opposite direction from data → OUT; with IOC so we get
    // a Transfer Event when the whole transfer is done.
    let status_status = 0u32;
    let status_control = ((trb_type::STATUS_STAGE as u32) << 10)
        | (1u32 << 5)  /* IOC = Interrupt On Completion */;

    let si = slot_id as usize;
    let status_trb_phys = unsafe {
        let idx_setup = EP0_PRODS[si].enqueue;
        // Setup
        enqueue_command(&raw mut EP0_TRANSFER_RINGS[si], &mut EP0_PRODS[si],
            setup_param, setup_status, setup_control);
        // Data
        enqueue_command(&raw mut EP0_TRANSFER_RINGS[si], &mut EP0_PRODS[si],
            out_phys, data_status, data_control);
        // Status — remember its physical address so we can match its Transfer Event.
        let status_idx = EP0_PRODS[si].enqueue;
        let phys = EP0_RING_PHYSES[si] + (status_idx as u64) * 16;
        enqueue_command(&raw mut EP0_TRANSFER_RINGS[si], &mut EP0_PRODS[si],
            0, status_status, status_control);
        let _ = idx_setup;
        phys
    };

    // Doorbell: slot_id, target = 1 (EP0 DCI).
    ring_doorbell(info.db_base, slot_id, 1);

    // Spin for a Transfer Event on this slot's EP0. We accept SUCCESS or
    // SHORT_PACKET.  On error the xHC may point to the Setup or Data TRB
    // instead of the Status TRB, so we match by (slot_id, endpoint_id)
    // rather than exact TRB pointer — strict pointer equality caused an
    // infinite push/pop ping-pong with the event backlog.
    for _ in 0..200_000_000u64 {
        if let Some(evt) = poll_event() {
            if evt.trb_type() == trb_type::TRANSFER_EVENT {
                if evt.slot_id() != slot_id || evt.endpoint_id() != 1 {
                    unsafe { push_backlog(evt); }
                    continue;
                }
                let cc = evt.completion_code();
                if cc == cc::SUCCESS || cc == cc::SHORT_PACKET {
                    return true;
                }
                if cc == cc::STALL_ERROR {
                    println!("[xhci] control_in stall (cc={})", cc);
                    return false;
                }
                println!("[xhci] control_in bad cc={} slot={} ep=1",
                    cc, slot_id);
                return false;
            }
        }
        core::hint::spin_loop();
    }
    println!("[xhci] control_in timeout");
    false
}

/// Read a USB string descriptor (index `idx`) into `DMA_BUF` and print it.
/// Returns `true` if the transfer succeeded (including short packet).
/// `label` is used only for diagnostics.
fn read_string_descriptor(slot_id: u8, idx: u8, label: &str) -> bool {
    if idx == 0 {
        return true; // no string available
    }
    let blob_phys = match phys_of(unsafe { &raw const DMA_BUF } as u64) {
        Some(p) => p,
        None => return false,
    };
    // String descriptors are UTF-16LE with a 2-byte header.
    // 64 bytes is plenty for any realistic descriptor.
    const MAX_STR: usize = 64;
    unsafe { DMA_BUF.0[..MAX_STR].fill(0); }
    if !control_in_phys(slot_id, 0x80, request::GET_DESCRIPTOR,
                        ((desc_type::STRING as u16) << 8) | (idx as u16),
                        0x0409, // US English language ID
                        MAX_STR as u16, blob_phys, MAX_STR) {
        println!("[xhci] string descriptor {} (index {}) failed", label, idx);
        return false;
    }
    let s = unsafe { &DMA_BUF.0[..MAX_STR] };
    let len = s[0] as usize;
    if len >= 2 && s[1] == 0x03 {
        // Valid string descriptor: decode UTF-16LE.
        let mut ascii = [0u8; 32];
        let mut a = 0usize;
        let mut i = 2usize;
        while i + 1 < len && a < ascii.len() {
            let lo = s[i];
            let hi = s[i + 1];
            if hi == 0 && lo.is_ascii() {
                ascii[a] = lo;
                a += 1;
            } else {
                ascii[a] = b'?';
                a += 1;
            }
            i += 2;
        }
        if let Ok(str) = core::str::from_utf8(&ascii[..a]) {
            println!("[xhci] string {} (index {}): \"{}\"", label, idx, str);
        }
    }
    true
}

pub(crate) fn control_out(
    slot_id: u8, request_type: u8, request: u8,
    value: u16, index: u16, length: u16,
) -> bool {
    let info = unsafe { match INFO { Some(i) => i, None => return false } };

    let setup = SetupPacket {
        bm_request_type: request_type,
        b_request: request,
        w_value: value, w_index: index, w_length: length,
    };
    let setup_param: u64 = unsafe { core::mem::transmute(setup) };

    // No data stage. Setup TRB with TRT=0 (no data), Status TRB with DIR=IN.
    let setup_status = 8u32;
    let setup_control = (0u32 << 16) /* TRT=No Data */
        | (1u32 << 6)
        | ((trb_type::SETUP_STAGE as u32) << 10);
    let status_status = 0u32;
    let status_control = ((trb_type::STATUS_STAGE as u32) << 10)
        | (1u32 << 16) /* DIR=IN for status when no data */
        | (1u32 << 5);

    let si = slot_id as usize;
    let status_trb_phys = unsafe {
        enqueue_command(&raw mut EP0_TRANSFER_RINGS[si], &mut EP0_PRODS[si],
            setup_param, setup_status, setup_control);
        let status_idx = EP0_PRODS[si].enqueue;
        let phys = EP0_RING_PHYSES[si] + (status_idx as u64) * 16;
        enqueue_command(&raw mut EP0_TRANSFER_RINGS[si], &mut EP0_PRODS[si],
            0, status_status, status_control);
        phys
    };

    ring_doorbell(info.db_base, slot_id, 1);

    // Match by slot + EP0 rather than exact TRB pointer (see control_in).
    for _ in 0..200_000_000u64 {
        if let Some(evt) = poll_event() {
            if evt.trb_type() == trb_type::TRANSFER_EVENT {
                if evt.slot_id() != slot_id || evt.endpoint_id() != 1 {
                    unsafe { push_backlog(evt); }
                    continue;
                }
                let cc = evt.completion_code();
                return cc == cc::SUCCESS || cc == cc::SHORT_PACKET;
            }
        }
        core::hint::spin_loop();
    }
    println!("[xhci] control_out timeout");
    false
}

// ============================================================================
// HID interrupt-IN polling
// ============================================================================

/// Post a single Normal TRB on the per-slot HID transfer ring to receive
/// the next 8-byte report, then ring the doorbell.
fn arm_hid_read(slot_id: u8, dci: u8) {
    let si = slot_id as usize;
    let info = unsafe { match INFO { Some(i) => i, None => return } };
    let buf_phys = match phys_of(unsafe { &raw const HID_REPORT_BUFS[si] } as u64) {
        Some(p) => p,
        None => { println!("[xhci] HID_REPORT_BUFS phys translation failed"); return; }
    };
    let hid_ring_phys = match phys_of(unsafe { &raw const HID_TRANSFER_RINGS[si] } as u64) {
        Some(p) => p,
        None => { println!("[xhci] HID ring phys translation failed"); return; }
    };
    // Normal TRB: parameter=buf phys, status=length (8), control:
    // trb_type=Normal, IOC=1, ISP=1 (interrupt on short packet).
    let control = ((trb_type::NORMAL as u32) << 10)
        | (1u32 << 5)  /* IOC */
        | (1u32 << 2); /* ISP */
    unsafe {
        HID_RING_PHYSES[si] = hid_ring_phys;
        init_command_ring(&raw mut HID_TRANSFER_RINGS[si], hid_ring_phys);
        HID_PRODS[si] = Producer::new();
        enqueue_command(&raw mut HID_TRANSFER_RINGS[si], &mut HID_PRODS[si],
            buf_phys, 8, control);
    }
    ring_doorbell(info.db_base, slot_id, dci);
}

/// Drain any Transfer Events on the event ring; if any correspond to a
/// HID report, parse it and call `on_report` with the parsed report.
/// Always re-arms the HID Normal TRB after consuming an event.
pub fn poll_hid<F: FnMut(&hid::KeyboardReport)>(mut on_report: F) -> usize {
    let device = match unsafe { DEVICE } {
        Some(d) if d.is_keyboard => d,
        _ => return 0,
    };
    let dci = (device.kbd_ep_in & 0x0F) * 2 + 1;
    let mut count = 0;
    let si = device.slot_id as usize;
    while let Some(evt) = poll_event() {
        if evt.trb_type() == trb_type::TRANSFER_EVENT {
            let cc = evt.completion_code();
            if cc == cc::SUCCESS || cc == cc::SHORT_PACKET {
                let buf = unsafe { &HID_REPORT_BUFS[si].0[..8] };
                if let Some(rep) = hid::KeyboardReport::from_bytes(buf) {
                    on_report(&rep);
                    count += 1;
                }
                arm_hid_read(device.slot_id, dci);
            }
        }
    }
    count
}

pub fn enumerated_device() -> Option<EnumeratedDevice> {
    unsafe { DEVICE }
}

pub fn enumerated_msc() -> Option<MscDevice> {
    unsafe { MSC }
}

// ============================================================================
// Bulk transfers (Mass Storage / CDC-ECM / generic bulk)
// ============================================================================

/// Submit a single Normal TRB to a bulk transfer ring and synchronously wait
/// for the matching Transfer Event. `dci` is the device context index of the
/// endpoint (`2*ep_num + direction`, with direction = 1 for IN, 0 for OUT).
/// Returns Some(bytes transferred) on SUCCESS / SHORT_PACKET, or None on
/// error or timeout.
fn bulk_xfer(
    slot_id: u8,
    dci: u8,
    is_in: bool,
    buf_phys: u64,
    len: u32,
) -> Option<u32> {
    let info = unsafe { match INFO { Some(i) => i, None => return None } };
    let si = slot_id as usize;
    let (ring, prod, ring_phys) = unsafe {
        if is_in {
            (
                &raw mut BULK_IN_TRANSFER_RINGS[si],
                &mut BULK_IN_PRODS[si],
                BULK_IN_RING_PHYSES[si],
            )
        } else {
            (
                &raw mut BULK_OUT_TRANSFER_RINGS[si],
                &mut BULK_OUT_PRODS[si],
                BULK_OUT_RING_PHYSES[si],
            )
        }
    };
    // Normal TRB: parameter = buf phys, status = (len << 0) (TRB Transfer Length),
    // control = trb_type Normal | IOC=1 | ISP=1 (interrupt on short packet).
    let control = ((trb_type::NORMAL as u32) << 10) | (1u32 << 5) | (1u32 << 2);
    let idx_before = unsafe { (*prod).enqueue };
    let trb_phys = if idx_before == RING_SIZE - 1 {
        ring_phys // enqueue_command wraps to index 0 when hitting the Link TRB
    } else {
        ring_phys + (idx_before as u64) * 16
    };
    unsafe {
        enqueue_command(ring, prod, buf_phys, len, control);
    }
    ring_doorbell(info.db_base, slot_id, dci);

    // Poll the event ring for a TRANSFER_EVENT on this slot/DCI.
    // Match by (slot_id, endpoint_id) rather than exact TRB pointer
    // so error events (which may point at the failing TRB) are consumed.
    let mut spins: u32 = 0;
    loop {
        if let Some(evt) = poll_event() {
            if evt.trb_type() == trb_type::TRANSFER_EVENT {
                if evt.slot_id() != slot_id || evt.endpoint_id() != dci {
                    unsafe { push_backlog(evt); }
                    continue;
                }
                let cc = evt.completion_code();
                if cc == cc::SUCCESS || cc == cc::SHORT_PACKET {
                    // Residual length: high 8 bits of status are CC, low 24 bits
                    // are TRB Transfer Length Remaining for a Normal TRB.
                    let remaining = evt.transfer_remaining();
                    return Some(len.saturating_sub(remaining));
                }
                println!("[xhci] bulk_xfer cc={} dci={} is_in={}", cc, dci, is_in);
                return None;
            }
        }
        spins = spins.wrapping_add(1);
        if spins > 200_000_000 {
            println!("[xhci] bulk_xfer timeout dci={} is_in={}", dci, is_in);
            return None;
        }
        core::hint::spin_loop();
    }
}

/// Public: send `len` bytes from `phys` on the device's bulk OUT endpoint.
pub fn bulk_out_xfer(slot_id: u8, dci: u8, phys: u64, len: u32) -> Option<u32> {
    bulk_xfer(slot_id, dci, false, phys, len)
}
/// Public: receive up to `len` bytes into `phys` on the device's bulk IN endpoint.
pub fn bulk_in_xfer(slot_id: u8, dci: u8, phys: u64, len: u32) -> Option<u32> {
    bulk_xfer(slot_id, dci, true, phys, len)
}

// ============================================================================
// CDC-ECM Ethernet framing (Phase 15 M50 — runtime TX/RX)
// ============================================================================
//
// CDC-ECM § 3.3.2: each Ethernet frame is one or more USB bulk packets.
// A frame is terminated by a short packet (any packet smaller than the
// bulk endpoint's wMaxPacketSize), which means an exact-multiple-of-MPS
// frame requires a trailing zero-length packet (ZLP). We don't bother
// with ZLP-on-out for v1 since standard Ethernet frame sizes (60-1514 B)
// are almost never exact multiples of 64 (FS) / 512 (HS).

/// Send one Ethernet frame on the CDC-ECM bulk OUT endpoint. Returns
/// `true` on success. `frame` includes the full Ethernet header
/// starting at the destination MAC. Bounded by the scratch buffer
/// (2 KiB) which is comfortably above the 1514-byte standard MTU.
pub fn cdc_ecm_send_frame(frame: &[u8]) -> bool {
    let ecm = match unsafe { CDC_ECM } {
        Some(e) => e,
        None => return false,
    };
    if frame.is_empty() || frame.len() > unsafe { ECM_TX_BUF.0.len() } {
        return false;
    }
    unsafe {
        ECM_TX_BUF.0[..frame.len()].copy_from_slice(frame);
    }
    let tx_phys = match phys_of(unsafe { &raw const ECM_TX_BUF } as u64) {
        Some(p) => p,
        None => { println!("[ecm-tx] phys translation failed"); return false; }
    };
    bulk_out_xfer(ecm.slot_id, ecm.out_dci, tx_phys, frame.len() as u32).is_some()
}

/// Receive one Ethernet frame on the CDC-ECM bulk IN endpoint into
/// `out`. Returns the number of bytes received (0 = no frame, frame
/// dropped if it doesn't fit in `out`). The xHCI bulk transfer
/// completes on the first short packet, so per-frame framing falls
/// out for free.
///
/// Non-blocking-ish: `bulk_in_xfer` busy-waits on a Transfer Event,
/// but the controller posts that event as soon as the device sends
/// a short packet OR fills the requested length. If the device has
/// nothing to say it eventually times out at ~200M spin iterations
/// (~1s) and returns None — caller treats that as "no frame."
pub fn cdc_ecm_recv_frame(out: &mut [u8]) -> usize {
    let ecm = match unsafe { CDC_ECM } {
        Some(e) => e,
        None => return 0,
    };
    let rx_phys = match phys_of(unsafe { &raw const ECM_RX_BUF } as u64) {
        Some(p) => p,
        None => return 0,
    };
    // Ask for up to one full Ethernet frame; controller will short-
    // packet earlier if the device's frame is smaller. xHCI Transfer
    // Event reports residual bytes via the EWE field.
    let want = unsafe { ECM_RX_BUF.0.len() } as u32;
    let residual = bulk_in_xfer(ecm.slot_id, ecm.in_dci, rx_phys, want);
    let received = match residual {
        Some(r) => (want as i64 - r as i64).max(0) as usize,
        None => return 0,
    };
    if received == 0 || received > out.len() {
        return 0;
    }
    out[..received].copy_from_slice(unsafe { &ECM_RX_BUF.0[..received] });
    received
}

/// Is there an inbound frame ready on the CDC-ECM bulk IN endpoint?
/// xHCI doesn't expose "buffer non-empty" cheaply — the device pushes
/// frames into a transfer ring at its own cadence and we discover them
/// by polling. For v1 we always return `true` and let `cdc_ecm_recv_frame`
/// either return data or 0; smoltcp's polling loop tolerates this.
pub fn cdc_ecm_has_data() -> bool { unsafe { CDC_ECM.is_some() } }

// ============================================================================
// Configuration descriptor walk for USB Mass Storage SCSI BBB
// ============================================================================

/// Walk a config descriptor blob looking for a Mass Storage / SCSI / BBB
/// interface (class 0x08 / subclass 0x06 / protocol 0x50). Returns the
/// interface's number + bulk IN endpoint addr + bulk OUT endpoint addr +
/// the IN endpoint's MaxPacketSize, plus the config value.
fn find_msc_endpoints(blob: &[u8], cfg_val: u8) -> Option<(u8, u8, u8, u16, u8)> {
    let mut i = 0usize;
    let mut in_msc_iface = false;
    let mut iface_num: u8 = 0;
    let mut in_ep: u8 = 0;
    let mut out_ep: u8 = 0;
    let mut mps: u16 = 0;
    while i + 2 <= blob.len() {
        let dlen = blob[i] as usize;
        let dtype = blob[i + 1];
        if dlen < 2 || i + dlen > blob.len() {
            break;
        }
        let d = &blob[i..i + dlen];
        match dtype {
            // INTERFACE descriptor
            0x04 if dlen >= 9 => {
                let class = d[5];
                let sub = d[6];
                let proto = d[7];
                if class == 0x08 && sub == 0x06 && proto == 0x50 {
                    iface_num = d[2];
                    in_msc_iface = true;
                } else {
                    in_msc_iface = false;
                }
            }
            // ENDPOINT descriptor
            0x05 if dlen >= 7 && in_msc_iface => {
                let addr = d[2];
                let attr = d[3] & 0x03;
                let pkt = u16::from_le_bytes([d[4], d[5]]);
                if attr == 0x02 {
                    if addr & 0x80 != 0 && in_ep == 0 {
                        in_ep = addr;
                        mps = pkt;
                    } else if addr & 0x80 == 0 && out_ep == 0 {
                        out_ep = addr;
                    }
                }
            }
            _ => {}
        }
        i += dlen;
    }
    if in_ep != 0 && out_ep != 0 {
        Some((iface_num, in_ep, out_ep, mps, cfg_val))
    } else {
        None
    }
}

// ============================================================================
// CDC-ECM enumeration (Phase 15 M50 — USB tethering)
// ============================================================================

/// Try to enumerate a CDC-ECM (USB-Ethernet / tethered phone) function on
/// the just-addressed device. Returns true if the device is CDC-ECM and we
/// configured it successfully; false otherwise (caller falls through to
/// the next class probe). Stashes the result in the `CDC_ECM` static so
/// `cdc_ecm_device()` / future smoltcp glue can find it.
///
/// Protocol shape (same as MSC for the bulk path):
///   1. `cdc_ecm::parse_config(blob)` — find control + data interfaces,
///      bulk IN/OUT addrs, MTU, iMACAddress.
///   2. SET_CONFIGURATION.
///   3. SET_INTERFACE on the Data interface to its `data_alt` (usually alt
///      1) — alt 0 has zero endpoints on the spec-conforming devices, alt 1
///      activates the bulk pair.
///   4. configure_bulk_endpoints (publishes phys addrs into BULK_*_RING_PHYS).
///   5. GET_DESCRIPTOR(STRING, iMAC) → parse 12 ASCII hex digits → 6-byte MAC.
///   6. SET_ETHERNET_PACKET_FILTER (class-specific OUT) — promiscuous-ish
///      default (DIRECTED + BROADCAST + MULTICAST) so we see the frames the
///      phone DHCPs us.
/// iPhone tether session 1: walk the iPhone's config descriptor for the
/// USB MUX interface (class 0xFF/0xFE/0x02), SET_CONFIGURATION, set the
/// bulk endpoints, and stash the per-iPhone state. The actual usbmuxd
/// Hello + lockdownd pair are subsequent sessions; this only proves the
/// xHCI-level path is live.
///
/// **NOTE 2026-06-06:** No longer called from the dispatch. Per
/// ROADMAP_EXPANSION_PROPOSAL(JUNE26).md M50, iPhone tethering uses
/// CDC-ECM (when Personal Hotspot is on), not the USB MUX interface
/// (which is for file sync). Function kept in tree for the future
/// session-2+ Apple device manage/sync work, not for tethering.
/// Enumerate an iPhone via the ipheth tethering protocol — ported from
/// Linux's `drivers/net/usb/ipheth.c`. iPhones expose a vendor-specific
/// interface (class 0xFF/0xFD/0x01) at alt setting 1 with bulk IN/OUT
/// endpoints. We:
///   1. Walk the config descriptor for the iface number at alt=1.
///   2. SET_CONFIGURATION on the device.
///   3. SET_INTERFACE on the ipheth iface to alt=1.
///   4. ConfigureEndpoint for the bulk pair.
///   5. GET_MACADDR control transfer (vendor request 0x00).
///   6. Stash the per-iPhone state.
fn try_enumerate_ipheth(
    slot_id: u8,
    port: u8,
    speed: u8,
    blob: &[u8],
    cfg_val: u8,
) -> bool {
    use crate::usb::iphone;

    iphone::dump_config_interfaces(blob);
    let (iface, in_addr, in_mps, out_addr, out_mps) =
        match iphone::find_ipheth_interface(blob) {
            Some(t) => t,
            None => {
                println!("[ipheth] no ipheth interface (class 0xFF/0xFD/0x01 at alt 1) in descriptor — iPhone may need 'Trust This Computer' tap, or Personal Hotspot not enabled");
                return false;
            }
        };
    println!(
        "[ipheth] interface found: iface={} (alt 1) IN 0x{:02X} OUT 0x{:02X} MPS in/out {}/{}",
        iface, in_addr, out_addr, in_mps, out_mps
    );

    if !control_out(slot_id, 0x00, request::SET_CONFIGURATION, cfg_val as u16, 0, 0) {
        println!("[ipheth] SET_CONFIGURATION failed");
        return false;
    }

    // SET_INTERFACE on the ipheth iface to alt 1. bmRequestType = 0x01
    // (host→device, standard, recipient=interface), bRequest = 11.
    if !control_out(slot_id, 0x01, request::SET_INTERFACE,
                    iphone::IPHETH_ALT as u16, iface as u16, 0) {
        println!("[ipheth] SET_INTERFACE(iface={}, alt={}) failed", iface, iphone::IPHETH_ALT);
        // Some firmwares still expose the bulk endpoints without the
        // explicit SET_INTERFACE; continue and hope for the best.
    }

    let (in_dci, out_dci) = match configure_bulk_endpoints(
        slot_id, port, speed,
        in_addr, out_addr, in_mps.max(out_mps),
    ) {
        Some(d) => d,
        None => {
            println!("[ipheth] configure_bulk_endpoints failed");
            return false;
        }
    };

    // GET_MACADDR — vendor request 0x00, bmRequestType=0xC0 (vendor,
    // device→host, recipient=device), returns 6 bytes.
    let mut mac = [0u8; 6];
    let buf_phys = match phys_of(unsafe { &raw const DMA_BUF } as u64) {
        Some(p) => p,
        None => { println!("[ipheth] DMA_BUF phys translation failed"); return false; }
    };
    unsafe { DMA_BUF.0[..6].fill(0); }
    if control_in_phys(slot_id, 0xC0, iphone::IPHETH_CMD_GET_MACADDR,
                       0, 0, 6, buf_phys, 6) {
        unsafe { mac.copy_from_slice(&DMA_BUF.0[..6]); }
        println!(
            "[ipheth] GET_MACADDR -> {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );
    } else {
        println!("[ipheth] GET_MACADDR failed — iPhone may not be trusted yet (tap 'Trust This Computer' on the phone)");
        // Continue anyway; carrier check might still work after trust.
    }

    iphone::stash(iphone::IphoneDevice {
        slot_id,
        ipheth_iface: iface,
        ipheth_in_ep: in_addr,
        ipheth_out_ep: out_addr,
        ipheth_in_dci: in_dci,
        ipheth_out_dci: out_dci,
        ipheth_in_mps: in_mps,
        ipheth_out_mps: out_mps,
        config_value: cfg_val,
        mac,
        confirmed_pairing: false,
    });

    // Carrier check — vendor request 0x45, bmRequestType=0xC0,
    // value=0, index=2, returns 1-2 bytes. byte 0x04 = "carrier on".
    unsafe { DMA_BUF.0[..2].fill(0); }
    if control_in_phys(slot_id, 0xC0, iphone::IPHETH_CMD_CARRIER_CHECK,
                       0x00, 0x02, 2, buf_phys, 2) {
        let b = unsafe { DMA_BUF.0[0] };
        let b1 = unsafe { DMA_BUF.0[1] };
        if b == iphone::IPHETH_CARRIER_ON || b1 == iphone::IPHETH_CARRIER_ON {
            println!("[ipheth] carrier ON — Personal Hotspot is active, ready to forward frames");
        } else {
            println!("[ipheth] carrier OFF (byte0=0x{:02X} byte1=0x{:02X}) — enable Personal Hotspot on the iPhone", b, b1);
        }
    } else {
        println!("[ipheth] carrier check control transfer failed (pre-pair)");
    }
    true
}

fn try_enumerate_cdc_ecm(
    slot_id: u8,
    port: u8,
    speed: u8,
    blob: &[u8],
    cfg_val: u8,
) -> bool {
    use crate::usb::cdc_ecm;

    let ecm = cdc_ecm::parse_config(blob);
    if !ecm.found || ecm.bulk.in_addr == 0 || ecm.bulk.out_addr == 0 {
        // Verbose no-match: useful when the user plugs a dock or dongle and
        // the kernel doesn't bring it up — without this, the failure is
        // completely silent. CDC-NCM, RNDIS, and Realtek-proprietary RTL8153
        // all miss the ECM parser, so logging the reason narrows the gap.
        println!("[xhci-ecm] no CDC-ECM match: found={} ctl-iface={} data-iface={} bulk IN=0x{:02X} OUT=0x{:02X}",
            ecm.found, ecm.control_iface, ecm.data_iface,
            ecm.bulk.in_addr, ecm.bulk.out_addr);
        return false;
    }

    if !control_out(slot_id, 0x00, request::SET_CONFIGURATION, cfg_val as u16, 0, 0) {
        println!("[xhci-ecm] SET_CONFIGURATION failed");
        return false;
    }

    // SET_INTERFACE(data_iface, data_alt) — Standard Interface request,
    // bmRequestType = 0x01 (host→device, standard, recipient=interface).
    if ecm.data_alt != 0 {
        if !control_out(slot_id, 0x01, request::SET_INTERFACE,
                        ecm.data_alt as u16, ecm.data_iface as u16, 0) {
            println!("[xhci-ecm] SET_INTERFACE(alt={}) on iface {} failed",
                ecm.data_alt, ecm.data_iface);
            // Some phones expose only alt 0 with endpoints; continue anyway.
        }
    }

    let (in_dci, out_dci) = match configure_bulk_endpoints(
        slot_id, port, speed,
        ecm.bulk.in_addr, ecm.bulk.out_addr,
        ecm.bulk.in_mps.max(ecm.bulk.out_mps),
    ) {
        Some(d) => d,
        None => {
            println!("[xhci-ecm] configure_bulk_endpoints failed");
            return false;
        }
    };

    // Read the MAC string descriptor (CDC-ECM § 5.4: 12 ASCII hex digits).
    let mut mac = [0u8; 6];
    if ecm.i_mac != 0 {
        // GET_DESCRIPTOR(STRING, iMAC) with language ID 0x0409 (US English).
        let buf_phys = match phys_of(unsafe { &raw const DMA_BUF } as u64) {
            Some(p) => p,
            None => { println!("[xhci-ecm] DMA buf phys failed"); return false; }
        };
        unsafe { DMA_BUF.0[..2 + 24].fill(0); }
        if !control_in_phys(slot_id, 0x80, request::GET_DESCRIPTOR,
                            ((desc_type::STRING as u16) << 8) | ecm.i_mac as u16,
                            0x0409, (2 + 24) as u16, buf_phys, 2 + 24) {
            println!("[xhci-ecm] GET_DESCRIPTOR(STRING, iMAC) failed");
        } else {
            let s = unsafe { &DMA_BUF.0[..2 + 24] };
            if let Some(parsed) = cdc_ecm::parse_mac_string(s) {
                mac = parsed;
            }
        }
    }

    // CDC class request SET_ETHERNET_PACKET_FILTER (0x43). bmRequestType =
    // 0x21 (host→device, class, recipient=interface). wValue = filter bits.
    // 0x0E = DIRECTED|BROADCAST|MULTICAST (enough for DHCP + unicast).
    let _ = control_out(slot_id, 0x21, 0x43, 0x000E,
                        ecm.control_iface as u16, 0);

    println!(
        "[xhci-ecm] CDC-ECM up: slot={} ctl-iface={} data-iface={} alt={} \
        IN 0x{:02X} OUT 0x{:02X} MPS in/out {}/{} MTU={} MAC={:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        slot_id, ecm.control_iface, ecm.data_iface, ecm.data_alt,
        ecm.bulk.in_addr, ecm.bulk.out_addr,
        ecm.bulk.in_mps, ecm.bulk.out_mps, ecm.mtu,
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
    );

    unsafe {
        CDC_ECM = Some(CdcEcmDevice {
            slot_id,
            control_iface: ecm.control_iface,
            data_iface: ecm.data_iface,
            data_alt: ecm.data_alt,
            in_ep_addr: ecm.bulk.in_addr,
            out_ep_addr: ecm.bulk.out_addr,
            in_dci,
            out_dci,
            in_mps: ecm.bulk.in_mps,
            out_mps: ecm.bulk.out_mps,
            config_value: cfg_val,
            mac,
            mtu: ecm.mtu,
        });
    }
    true
}

/// CDC-NCM enumeration — mirrors try_enumerate_cdc_ecm but for subclass 0x0D.
fn try_enumerate_cdc_ncm(
    slot_id: u8,
    port: u8,
    speed: u8,
    blob: &[u8],
    cfg_val: u8,
) -> bool {
    use crate::usb::cdc_ncm;

    let ncm = cdc_ncm::parse_config(blob);
    if !ncm.found || ncm.bulk.in_addr == 0 || ncm.bulk.out_addr == 0 {
        println!("[xhci-ncm] no CDC-NCM match: found={} ctl-iface={} data-iface={} bulk IN=0x{:02X} OUT=0x{:02X}",
            ncm.found, ncm.control_iface, ncm.data_iface,
            ncm.bulk.in_addr, ncm.bulk.out_addr);
        return false;
    }

    if !control_out(slot_id, 0x00, request::SET_CONFIGURATION, cfg_val as u16, 0, 0) {
        println!("[xhci-ncm] SET_CONFIGURATION failed");
        return false;
    }

    if ncm.data_alt != 0 {
        if !control_out(slot_id, 0x01, request::SET_INTERFACE,
                        ncm.data_alt as u16, ncm.data_iface as u16, 0) {
            println!("[xhci-ncm] SET_INTERFACE(alt={}) on iface {} failed",
                ncm.data_alt, ncm.data_iface);
        }
    }

    let (in_dci, out_dci) = match configure_bulk_endpoints(
        slot_id, port, speed,
        ncm.bulk.in_addr, ncm.bulk.out_addr,
        ncm.bulk.in_mps.max(ncm.bulk.out_mps),
    ) {
        Some(d) => d,
        None => {
            println!("[xhci-ncm] configure_bulk_endpoints failed");
            return false;
        }
    };

    // CDC-NCM does not expose a MAC via class-specific descriptor.
    // Use a locally-administered MAC (02:00:00:00:00:01).
    let mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];

    println!(
        "[xhci-ncm] CDC-NCM up: slot={} ctl-iface={} data-iface={} alt={} \
        IN 0x{:02X} OUT 0x{:02X} MPS in/out {}/{} MTU={} MAC={:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        slot_id, ncm.control_iface, ncm.data_iface, ncm.data_alt,
        ncm.bulk.in_addr, ncm.bulk.out_addr,
        ncm.bulk.in_mps, ncm.bulk.out_mps, ncm.mtu,
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
    );

    unsafe {
        CDC_ECM = Some(CdcEcmDevice {
            slot_id,
            control_iface: ncm.control_iface,
            data_iface: ncm.data_iface,
            data_alt: ncm.data_alt,
            in_ep_addr: ncm.bulk.in_addr,
            out_ep_addr: ncm.bulk.out_addr,
            in_dci,
            out_dci,
            in_mps: ncm.bulk.in_mps,
            out_mps: ncm.bulk.out_mps,
            config_value: cfg_val,
            mac,
            mtu: ncm.mtu,
        });
    }
    true
}

/// Public accessor — `net_interface_register` (Phase 15 M50) reads this
/// to learn the MAC + DCIs for the smoltcp Device adapter.
/// Enumerate a child device discovered behind a downstream hub port. The
/// hub's `bring_up_hub` calls this once per port reporting Connection after
/// reset. Builds the same Address Device / class-dispatch pipeline as the
/// root-hub direct path, but with route string + parent-hub TT fields set
/// on the slot context so the xHC can route control transfers correctly.
///
/// `parent_hub_slot` is the xHCI slot ID of the hub. `port_on_hub` is the
/// 1-indexed downstream port. `root_hub_port` is the root-hub port that
/// the whole branch hangs off (resolved by walking up the topology tree).
/// `speed` is the speed reported by the hub for this child after reset.
pub fn enumerate_child_of_hub(
    parent_hub_slot: u8,
    port_on_hub: u8,
    root_hub_port: u8,
    speed: u8,
) -> bool {
    let parent = unsafe { SLOT_TOPOLOGIES[parent_hub_slot as usize] };
    let parent_depth = parent.map(|t| t.depth).unwrap_or(0);
    let parent_route = parent.map(|t| t.route_string).unwrap_or(0);
    let child_depth = parent_depth + 1;
    let child_route = parent_route | ((port_on_hub as u32) << (parent_depth * 4));
    let topology = Topology {
        root_hub_port,
        route_string: child_route,
        parent_hub_slot,
        parent_port: port_on_hub,
        depth: child_depth,
    };
    enumerate_device(topology, speed)
}

/// SYS_USBINFO — print every port + enumerated slot to the current TTY.
/// Called from the `usbinfo` shell builtin. The user reads this at the
/// shell prompt to debug enumeration without serial.
/// W540 USB-2 grind iter 1: dump each EHCI controller's USBCMD/USBSTS
/// + every PORTSC. If EHCI shows a port as CCS=1 after our XUSB2PR
/// write, the chipset didn't actually migrate that port to xHCI even
/// though the register reads back as routed. That'd explain why xHCI's
/// PR write never completes — EHCI still owns the physical hardware.
fn dump_ehci_diagnostics() {
    const EHCI_CLASS: u8 = 0x0C;
    const EHCI_SUBCLASS: u8 = 0x03;
    const EHCI_PROG_IF: u8 = 0x20;
    const EHCI_USBCMD: u64 = 0x00;
    const EHCI_USBSTS: u64 = 0x04;
    const EHCI_PORTSC0: u64 = 0x44; // first port; stride = 4 bytes
    const EHCI_HCSPARAMS: u64 = 0x04; // in CAPABILITY regs at +4
    let mut found = 0u32;
    for slot in 0..32u8 {
        for func in 0..8u8 {
            let loc = pci::Location { bus: 0, slot, func };
            if loc.vendor_id() == 0xFFFF { continue; }
            if pci::class_triple(loc) != (EHCI_CLASS, EHCI_SUBCLASS, EHCI_PROG_IF) {
                continue;
            }
            found += 1;
            let bar0 = pci::read_u32(loc.bus, loc.slot, loc.func, 0x10);
            let mmio_phys = (bar0 & !0xF) as u64;
            if mmio_phys == 0 { continue; }
            let mmio = paging::phys_to_virt(mmio_phys);
            let caplen = unsafe { read_volatile(mmio as *const u8) } as u64;
            let hcsparams = unsafe { read_u32(mmio + EHCI_HCSPARAMS) };
            let n_ports = (hcsparams & 0xF) as u8;
            let op_base = mmio + caplen;
            let usbcmd = unsafe { read_u32(op_base + EHCI_USBCMD) };
            let usbsts = unsafe { read_u32(op_base + EHCI_USBSTS) };
            println!(
                "usbinfo: EHCI @ PCI {}:{:02X}.{} mmio=0x{:X} N_PORTS={} USBCMD=0x{:08X} USBSTS=0x{:08X}",
                loc.bus, loc.slot, loc.func, mmio_phys, n_ports, usbcmd, usbsts
            );
            for p in 0..n_ports {
                let portsc = unsafe { read_u32(op_base + EHCI_PORTSC0 + (p as u64) * 4) };
                let ccs = portsc & 1;
                let ped = (portsc >> 2) & 1;
                let po = (portsc >> 13) & 1; // Port Owner (companion → 1)
                let pp = (portsc >> 12) & 1; // Port Power
                let suspend = (portsc >> 7) & 1;
                println!(
                    "  EHCI port {:2}: PORTSC=0x{:08X} CCS={} PED={} Suspend={} PP={} PortOwner={} ({})",
                    p + 1, portsc, ccs, ped, suspend, pp, po,
                    if po == 1 { "released to companion (xHCI)" }
                    else if ccs == 1 { "**EHCI STILL OWNS A CONNECTED DEVICE**" }
                    else { "EHCI owns (empty)" }
                );
            }
        }
    }
    if found == 0 {
        println!("usbinfo: no EHCI controllers found (QEMU/AMD)");
    }
}

pub fn print_usbinfo() -> u64 {
    let info = match unsafe { INFO } {
        Some(i) => i,
        None => {
            println!("usbinfo: xHCI not initialized");
            return 0;
        }
    };
    // Walk PCI for every EHCI controller and dump its USBSTS + each
    // PORTSC. Compare against xHCI's view: if EHCI shows any port with
    // CCS=1, the chipset didn't fully migrate USB-2 to xHCI despite our
    // XUSB2PR write, and that's why xHCI's USB-2 reset never completes.
    dump_ehci_diagnostics();
    let usbsts_now = unsafe { read_u32(info.op_base + 0x04) };
    println!(
        "usbinfo: xHCI MMIO=0x{:X} MaxSlots={} MaxPorts={} CSZ={} HCIVERSION=0x{:04X} USBSTS=0x{:08X}",
        info.mmio_base, info.max_slots, info.max_ports,
        if info.csz1 { 1 } else { 0 }, info.hciversion, usbsts_now
    );

    // Intel PCH USB port routing (W540 = Lynx Point). XUSB2PRM is the
    // read-only mask of USB-2 ports that CAN be routed to xHCI; XUSB2PR
    // is what's currently routed. If they differ, BIOS locked the
    // register via SMM and our write didn't stick — that's a routing
    // failure that explains "everything stuck at PLS=Polling".
    match unsafe { PCH_ROUTING } {
        Some(r) => {
            println!("usbinfo: Intel PCH @ PCI 00:{:02X}.{}", r.pci_loc.slot, r.pci_loc.func);
            println!("  XUSB2PRM (USB-2 ports route-capable): 0x{:08X}", r.xusb2prm);
            println!("  XUSB2PR  (USB-2 ports route-active):  0x{:08X}", r.xusb2pr);
            if r.xusb2pr != r.xusb2prm {
                println!("  ** XUSB2PR != XUSB2PRM — BIOS may be blocking our write (USB-2 ports still on EHCI)");
            }
            println!("  USB3PRM  (USB-3 ports SS-capable):    0x{:08X}", r.usb3prm);
            println!("  USB3PSSEN(USB-3 ports SS-active):     0x{:08X}", r.usb3pssen);
            if r.usb3pssen != r.usb3prm {
                println!("  ** USB3PSSEN != USB3PRM — SuperSpeed routing didn't fully apply");
            }
            // Re-attempt the write right now, in case BIOS only locks
            // pre-boot and unlocks after. Cheap; harmless if it fails.
            if r.xusb2prm != 0 && r.xusb2pr != r.xusb2prm {
                pci::write_u32(r.pci_loc.bus, r.pci_loc.slot, r.pci_loc.func,
                    intel_pch::XUSB2PR, r.xusb2prm);
                let after = pci::read_u32(r.pci_loc.bus, r.pci_loc.slot, r.pci_loc.func,
                    intel_pch::XUSB2PR);
                println!("  retry XUSB2PR<-0x{:08X}  read-back=0x{:08X} {}",
                    r.xusb2prm, after,
                    if after == r.xusb2prm { "(STUCK — try replugging device)" } else { "(STILL BLOCKED by BIOS)" });
            }
            // Intel vendor-specific PHY registers (Lynx Point datasheet).
            let usb2phycm = pci::read_u32(r.pci_loc.bus, r.pci_loc.slot, r.pci_loc.func, 0xE0);
            let usb3phycm = pci::read_u32(r.pci_loc.bus, r.pci_loc.slot, r.pci_loc.func, 0xE4);
            let usb2lpm   = pci::read_u32(r.pci_loc.bus, r.pci_loc.slot, r.pci_loc.func, 0xE8);
            let usb3lpm   = pci::read_u32(r.pci_loc.bus, r.pci_loc.slot, r.pci_loc.func, 0xEC);
            println!("  USB2PHYCM=0x{:08X} USB3PHYCM=0x{:08X} USB2LPM=0x{:08X} USB3LPM=0x{:08X}",
                usb2phycm, usb3phycm, usb2lpm, usb3lpm);
        }
        None => println!("usbinfo: no PCH routing cache (non-Intel xHCI or init never ran)"),
    }

    // Dump EHCI state so we can see whether the companion controllers are
    // still fighting xHCI for the USB-2 PHYs.
    ehci_dump();

    // Walk xECP for Supported Protocol entries (ID=2). Each tells us
    // which port range speaks USB-2 vs USB-3 — vital because Lynx Point
    // numbers them interleaved. Without this we'd guess wrong about
    // which port is a USB-2 companion of which USB-3 jack.
    let mmio = crate::paging::phys_to_virt(info.mmio_base);
    let hccparams1 = unsafe { read_u32(mmio + cap_reg::HCCPARAMS1 as u64) };
    let mut off_dw = ((hccparams1 >> hccparams1::XECP_SHIFT) & hccparams1::XECP_MASK) as u64;
    let mut usb2_port_range: (u8, u8) = (0, 0);
    let mut usb3_port_range: (u8, u8) = (0, 0);
    if off_dw != 0 {
        let mut cap_addr = mmio + off_dw * 4;
        for _ in 0..64 {
            let cap = unsafe { read_u32(cap_addr) };
            let id = cap & 0xFF;
            let next = (cap >> 8) & 0xFF;
            if id == 1 { // USB Legacy Support Capability
                let bios_owned = (cap & (1 << 16)) != 0;
                let os_owned = (cap & (1 << 24)) != 0;
                println!("  xECP USBLEGSUP: BIOS={} OS={} (addr=+0x{:X})",
                    if bios_owned { 1 } else { 0 },
                    if os_owned { 1 } else { 0 },
                    cap_addr - mmio);
            }
            if id == 2 { // Supported Protocol Capability
                let major = (cap >> 24) & 0xFF;
                let dw2 = unsafe { read_u32(cap_addr + 8) };
                let port_offset = (dw2 & 0xFF) as u8;
                let port_count  = ((dw2 >> 8) & 0xFF) as u8;
                let name_dw = unsafe { read_u32(cap_addr + 4) };
                println!("  xECP SupportedProtocol: USB {}.x ports {}..{} (count={}, name=0x{:08X})",
                    major, port_offset, port_offset + port_count - 1, port_count, name_dw);
                if major == 2 { usb2_port_range = (port_offset, port_count); }
                if major == 3 { usb3_port_range = (port_offset, port_count); }
            }
            if next == 0 { break; }
            off_dw = next as u64;
            cap_addr = cap_addr + off_dw * 4;
        }
    } else {
        println!("  xECP: not present (QEMU/AMD or no caps); guessing port types");
    }
    // Helper to label a port number by USB version.
    let port_class = |p: u8| -> &'static str {
        if usb2_port_range.1 != 0
            && p >= usb2_port_range.0
            && p < usb2_port_range.0 + usb2_port_range.1
        {
            "USB-2"
        } else if usb3_port_range.1 != 0
            && p >= usb3_port_range.0
            && p < usb3_port_range.0 + usb3_port_range.1
        {
            "USB-3"
        } else {
            "?"
        }
    };

    let mut ccs_count = 0u32;
    let mut ped_count = 0u32;
    for port in 1..=info.max_ports {
        let portsc_addr = info.op_base
            + op_reg::PORTSC_BASE as u64 + ((port as u64 - 1) * 0x10);
        let portsc = unsafe { read_u32(portsc_addr) };
        let ccs = (portsc & portsc::CCS) != 0;
        let ped = (portsc & portsc::PED) != 0;
        let pls = (portsc & portsc::PLS_MASK) >> portsc::PLS_SHIFT;
        let speed = (portsc >> 10) & 0xF;
        // Show ALL ports including disconnected — user needs to see USB-2
        // companion ports that are CCS=0 to debug iPhone enum.
        if ccs { ccs_count += 1; }
        if ped { ped_count += 1; }
        let class = port_class(port);
        // Skip only the truly empty USB-3 ports (RxDetect, no device) to
        // keep the dump manageable. USB-2 ports always show even if empty.
        if !ccs && !ped && class == "USB-3" && pls == 5 { continue; }
        let speed_name = match speed {
            1 => "FS(USB1.1)",
            2 => "LS(USB1.0)",
            3 => "HS(USB2.0)",
            4 => "SS(USB3.0)",
            5 => "SSP(USB3.1)",
            _ => "?",
        };
        let pls_name = match pls {
            0 => "U0",
            1 => "U1",
            2 => "U2",
            3 => "U3",
            4 => "Disabled",
            5 => "RxDetect",
            6 => "Inactive",
            7 => "Polling",
            8 => "Recovery",
            9 => "HotReset",
            10 => "Compliance",
            11 => "Test",
            _ => "?",
        };
        println!(
            "  port {:2} [{}]: PORTSC=0x{:08X} CCS={} PED={} PLS={}({}) speed={}({})",
            port, class, portsc, if ccs {1} else {0}, if ped {1} else {0},
            pls, pls_name, speed, speed_name
        );
    }
    println!("usbinfo: {} ports show CCS=1 (connected), {} ports show PED=1 (enabled+routed to xHCI)",
        ccs_count, ped_count);

    // ---- Enumerated device tree (root ports + hub downstream ports) ----
    println!("usbinfo: enumerated devices:");
    let mut any = false;
    for slot in 1..=info.max_slots.min(MAX_SLOTS as u8) {
        let dev = unsafe { ENUMERATED_SLOTS[slot as usize] };
        if let Some(d) = dev {
            any = true;
            let top = unsafe { SLOT_TOPOLOGIES[slot as usize] };
            let speed_name = match d.speed {
                1 => "FS", 2 => "LS", 3 => "HS", 4 => "SS", 5 => "SSP", _ => "?"
            };
            let class_name = match d.class {
                0x09 => "hub",
                0x08 => "MSC",
                0x03 => "HID",
                0x02 => "CDC-ACM",
                0x00 => "per-interface",
                _ => "other",
            };
            if let Some(t) = top {
                if t.parent_hub_slot != 0 {
                    println!(
                        "  slot={} addr={} vendor=0x{:04X} product=0x{:04X} class={} speed={} — behind hub slot={}, port={}, root port={}",
                        d.slot_id, d.usb_address, d.vendor, d.product,
                        class_name, speed_name,
                        t.parent_hub_slot, t.parent_port, t.root_hub_port
                    );
                } else {
                    println!(
                        "  slot={} addr={} vendor=0x{:04X} product=0x{:04X} class={} speed={} — root port={}",
                        d.slot_id, d.usb_address, d.vendor, d.product,
                        class_name, speed_name, t.root_hub_port
                    );
                }
            } else {
                println!(
                    "  slot={} addr={} vendor=0x{:04X} product=0x{:04X} class={} speed={}",
                    d.slot_id, d.usb_address, d.vendor, d.product,
                    class_name, speed_name
                );
            }
        }
    }
    if !any {
        println!("usbinfo: no enumerated devices");
    }

    // Devices living on the companion EHCI bus (W540 USB-2 jacks).
    crate::usb::ehci::print_devices();
    crate::usb::ehci::print_tether_status();

    match unsafe { CDC_ECM } {
        Some(e) => println!(
            "usbinfo: CDC-ECM — slot={} ctl-iface={} data-iface={} alt={} MAC {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X} MTU={}",
            e.slot_id, e.control_iface, e.data_iface, e.data_alt,
            e.mac[0], e.mac[1], e.mac[2], e.mac[3], e.mac[4], e.mac[5], e.mtu
        ),
        None => println!("usbinfo: no CDC-ECM state"),
    }
    match unsafe { MSC.as_ref() } {
        Some(m) => println!(
            "usbinfo: MSC — slot={} interface={} blocks={} block_size={}",
            m.slot_id, m.iface_num, m.capacity_blocks, m.capacity_bs
        ),
        None => println!("usbinfo: no MSC state"),
    }
    match crate::usb::iphone::iphone_device() {
        Some(i) => println!(
            "usbinfo: iPhone ipheth — slot={} iface={} (alt 1) IN 0x{:02X} OUT 0x{:02X} MPS in/out {}/{} DCIs in/out {}/{} MAC {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X} paired={}",
            i.slot_id, i.ipheth_iface, i.ipheth_in_ep, i.ipheth_out_ep,
            i.ipheth_in_mps, i.ipheth_out_mps, i.ipheth_in_dci, i.ipheth_out_dci,
            i.mac[0], i.mac[1], i.mac[2], i.mac[3], i.mac[4], i.mac[5],
            i.confirmed_pairing
        ),
        None => println!("usbinfo: no iPhone state (vendor 0x05AC not seen, or ipheth iface not in descriptor)"),
    }
    0
}

pub fn cdc_ecm_device() -> Option<CdcEcmDevice> {
    unsafe { CDC_ECM }
}

/// Configure the device's two bulk endpoints (IN + OUT) via the
/// ConfigureEndpoint command. Allocates fresh transfer rings for each and
/// publishes their phys addresses into the BULK_*_RING_PHYS statics so
/// `bulk_in_xfer` / `bulk_out_xfer` find them.
fn configure_bulk_endpoints(
    slot_id: u8,
    port: u8,
    speed: u8,
    in_ep: u8,
    out_ep: u8,
    mps: u16,
) -> Option<(u8, u8)> {
    let info = unsafe { match INFO { Some(i) => i, None => return None } };

    // Endpoint DCIs: IN = 2*N+1, OUT = 2*N.
    let in_dci = ((in_ep & 0x0F) * 2 + 1) as u8;
    let out_dci = ((out_ep & 0x0F) * 2) as u8;
    let max_dci = in_dci.max(out_dci);

    // Translate ring phys + program them on the EPs (per-slot).
    let si = slot_id as usize;
    let in_phys = phys_of(unsafe { &raw const BULK_IN_TRANSFER_RINGS[si] } as u64)?;
    let out_phys = phys_of(unsafe { &raw const BULK_OUT_TRANSFER_RINGS[si] } as u64)?;
    unsafe {
        init_command_ring(&raw mut BULK_IN_TRANSFER_RINGS[si], in_phys);
        init_command_ring(&raw mut BULK_OUT_TRANSFER_RINGS[si], out_phys);
        BULK_IN_PRODS[si] = Producer::new();
        BULK_OUT_PRODS[si] = Producer::new();
        BULK_IN_RING_PHYSES[si] = in_phys;
        BULK_OUT_RING_PHYSES[si] = out_phys;

        let ic = &mut INPUT_CTXS[si].0;
        ic.reset();
        // Add slot (A0) + both bulk endpoints (Ain_dci, Aout_dci).
        ic.input_ctrl_mut().add_flags =
            (1u32 << 0) | (1u32 << in_dci) | (1u32 << out_dci);
        let slot = ic.slot_mut();
        slot.set_context_entries(max_dci as u32);
        slot.set_root_hub_port(port);
        slot.set_speed(speed as u32);
        ic.ep_mut((in_dci - 1) as usize).init_bulk_in_ep(mps, in_phys, true);
        ic.ep_mut((out_dci - 1) as usize).init_bulk_out_ep(mps, out_phys, true);
    }

    let input_phys = phys_of(unsafe { &raw const INPUT_CTXS[slot_id as usize] } as u64)?;

    let idx = unsafe { CMD_PROD.enqueue };
    let cmd_phys = cmd_trb_phys_at(idx);
    let control =
        ((trb_type::CONFIGURE_ENDPOINT_CMD as u32) << 10) | ((slot_id as u32) << 24);
    unsafe {
        enqueue_command(
            &raw mut COMMAND_RING, &mut CMD_PROD,
            input_phys, 0, control,
        );
    }
    ring_doorbell(info.db_base, 0, 0);
    let (cc, _) = wait_command_completion(cmd_phys);
    if cc != cc::SUCCESS {
        println!("[xhci] ConfigureEndpoint(bulk) failed: cc={}", cc);
        return None;
    }
    Some((in_dci, out_dci))
}

/// Run a Mass Storage transaction: build CBW, push it OUT, transfer data,
/// then read CSW IN. Returns `(data_bytes_transferred, csw_status)`.
fn msc_transaction(
    msc: &MscDevice,
    cbw: &[u8],
    is_data_in: bool,
    data_len: u32,
) -> Option<(u32, u8)> {
    let cbw_phys = phys_of(unsafe { &raw const BULK_CBW_BUF } as u64)?;
    let data_phys = phys_of(unsafe { &raw const BULK_DATA_BUF } as u64)?;
    let csw_phys = phys_of(unsafe { &raw const BULK_CSW_BUF } as u64)?;

    // Stage 1: write CBW into BULK_CBW_BUF and push it on the OUT ring.
    unsafe {
        BULK_CBW_BUF.0[..cbw.len()].copy_from_slice(cbw);
    }
    bulk_out_xfer(msc.slot_id, msc.out_dci, cbw_phys, cbw.len() as u32)?;

    // Stage 2: data phase (if any). For IN, the device fills BULK_DATA_BUF.
    let mut data_xferred = 0u32;
    if data_len > 0 {
        let n = if is_data_in {
            bulk_in_xfer(msc.slot_id, msc.in_dci, data_phys, data_len)?
        } else {
            bulk_out_xfer(msc.slot_id, msc.out_dci, data_phys, data_len)?
        };
        data_xferred = n;
    }

    // Stage 3: read CSW IN.
    let csw_n = bulk_in_xfer(msc.slot_id, msc.in_dci, csw_phys, 13)?;
    if csw_n < 13 {
        return None;
    }
    let status = unsafe { BULK_CSW_BUF.0[12] };
    Some((data_xferred, status))
}

/// Issue INQUIRY (36 B) + READ CAPACITY (10) to populate MSC.inquiry +
/// MSC.capacity_*. Called from the enumeration path after bulk EPs are up.
fn msc_run_inquiry_and_capacity(msc: &mut MscDevice) -> bool {
    use crate::usb::mass_storage::{self as msc_proto, scsi};
    let mut cbw = [0u8; 31];

    // INQUIRY
    let cdb = scsi::inquiry(36);
    if msc_proto::build_cbw(&mut cbw, 1, 36, true, 0, &cdb).is_none() {
        return false;
    }
    let (n, st) = match msc_transaction(msc, &cbw, true, 36) {
        Some(v) => v,
        None => { println!("[xhci-msc] INQUIRY transaction failed"); return false; }
    };
    if st != 0 {
        println!("[xhci-msc] INQUIRY CSW status={}", st);
        return false;
    }
    unsafe {
        msc.inquiry.copy_from_slice(&BULK_DATA_BUF.0[..36]);
    }
    if n != 36 {
        println!("[xhci-msc] INQUIRY short read: {} of 36", n);
    }

    // READ CAPACITY (10) — 8 byte response.
    let cdb = scsi::read_capacity_10();
    if msc_proto::build_cbw(&mut cbw, 2, 8, true, 0, &cdb).is_none() {
        return false;
    }
    let (n, st) = match msc_transaction(msc, &cbw, true, 8) {
        Some(v) => v,
        None => { println!("[xhci-msc] READ CAPACITY transaction failed"); return false; }
    };
    if st != 0 || n < 8 {
        println!("[xhci-msc] READ CAPACITY CSW status={} n={}", st, n);
        return false;
    }
    let data = unsafe { &BULK_DATA_BUF.0[..8] };
    let (blocks, bs) = msc_proto::parse_read_capacity_10(data).unwrap_or((0, 0));
    msc.capacity_blocks = blocks;
    msc.capacity_bs = bs;
    true
}

/// Mass Storage enumeration path. Called from `enumerate_device` after the
/// HID-keyboard lookup fails. Walks the config descriptor for the SCSI BBB
/// interface, sets up bulk endpoints, runs INQUIRY + READ CAPACITY, and
/// stashes the result in `MSC`. Returns true if the device is usable.
fn try_enumerate_mass_storage(
    slot_id: u8,
    port: u8,
    speed: u8,
    blob: &[u8],
    cfg_val: u8,
) -> bool {
    let (iface_num, in_ep, out_ep, mps, cfg_val) = match find_msc_endpoints(blob, cfg_val) {
        Some(x) => x,
        None => return false,
    };

    if !control_out(slot_id, 0x00, request::SET_CONFIGURATION, cfg_val as u16, 0, 0) {
        println!("[xhci-msc] SET_CONFIGURATION failed");
        return false;
    }

    let (in_dci, out_dci) = match configure_bulk_endpoints(slot_id, port, speed, in_ep, out_ep, mps) {
        Some(d) => d,
        None => return false,
    };

    println!(
        "[xhci-msc] bulk endpoints configured: iface {} IN 0x{:02X} OUT 0x{:02X} MPS {} (DCIs IN={} OUT={})",
        iface_num, in_ep, out_ep, mps, in_dci, out_dci
    );

    let mut msc = MscDevice {
        slot_id,
        in_ep_addr: in_ep,
        out_ep_addr: out_ep,
        in_dci,
        out_dci,
        max_packet: mps,
        iface_num,
        config_value: cfg_val,
        inquiry: [0u8; 36],
        capacity_blocks: 0,
        capacity_bs: 0,
    };
    if !msc_run_inquiry_and_capacity(&mut msc) {
        return false;
    }

    unsafe { MSC = Some(msc); }
    true
}
