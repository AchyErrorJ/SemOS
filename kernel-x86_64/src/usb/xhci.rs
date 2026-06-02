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
}

/// PORTSC bits.
mod portsc {
    pub const CCS: u32 = 1 << 0;   // Current Connect Status
    pub const PED: u32 = 1 << 1;   // Port Enabled/Disabled
    pub const PR: u32 = 1 << 4;    // Port Reset
    pub const PLS_MASK: u32 = 0xF << 5;  // Port Link State
    pub const PP: u32 = 1 << 9;    // Port Power
    pub const SPEED_SHIFT: u32 = 10; // bits 13:10
    pub const SPEED_MASK: u32 = 0xF << 10;
    pub const CSC: u32 = 1 << 17;  // Connect Status Change (RW1C)
    pub const PEC: u32 = 1 << 18;  // Port Enable Change (RW1C)
    pub const PRC: u32 = 1 << 21;  // Port Reset Change (RW1C)
    /// Mask of all change bits (RW1C). When writing PORTSC, preserving the
    /// connect/enable status bits requires we clear these out of our read
    /// so we don't accidentally clear them. Spec §5.4.8.
    pub const RW1C_MASK: u32 = (1 << 17) | (1 << 18) | (1 << 20) | (1 << 21) | (1 << 22);
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
const MAX_SLOTS: usize = 8;
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

/// Device context for slot 1 (the device we enumerate). Sized for CSZ=1.
#[repr(C, align(4096))]
struct PerSlotDeviceCtx(DeviceContext);
static mut DEVICE_CTX_SLOT1: PerSlotDeviceCtx = PerSlotDeviceCtx(DeviceContext::zero());

/// Input context used to issue Address Device / Configure Endpoint for slot 1.
#[repr(C, align(4096))]
struct PerSlotInputCtx(InputContext);
static mut INPUT_CTX_SLOT1: PerSlotInputCtx = PerSlotInputCtx(InputContext::zero());

/// Transfer ring for EP0 (Control) on the enumerated device.
static mut EP0_TRANSFER_RING: CommandRing = CommandRing::new();
/// Transfer ring for the HID Interrupt-IN endpoint on the enumerated device.
static mut HID_TRANSFER_RING: CommandRing = CommandRing::new();

/// Generic DMA buffer for descriptor reads (max we need: the full
/// Config + Iface + Endpoint + HID combined descriptor blob).
const SETUP_BUF_SIZE: usize = 512;
#[repr(C, align(64))]
struct DmaBuf([u8; SETUP_BUF_SIZE]);
static mut DMA_BUF: DmaBuf = DmaBuf([0; SETUP_BUF_SIZE]);

/// DMA buffer for the recurring 8-byte HID keyboard report.
#[repr(C, align(64))]
struct HidReportBuf([u8; 64]);
static mut HID_REPORT_BUF: HidReportBuf = HidReportBuf([0; 64]);

/// Transfer rings + DMA scratch for the bulk path (USB Mass Storage,
/// CDC-ECM, etc.). One IN ring + one OUT ring is enough for v1 since we
/// only enumerate one bulk device at a time. The IN/OUT split means the
/// HC can have a TRB pending on each direction simultaneously.
static mut BULK_IN_TRANSFER_RING: CommandRing = CommandRing::new();
static mut BULK_OUT_TRANSFER_RING: CommandRing = CommandRing::new();
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
}

static mut INFO: Option<XhciInfo> = None;
static mut DEVICE: Option<EnumeratedDevice> = None;
static mut CMD_PROD: Producer = Producer::new();
static mut EVT_CONS: Consumer = Consumer::new();
static mut EP0_PROD: Producer = Producer::new();
static mut HID_PROD: Producer = Producer::new();
static mut BULK_IN_PROD: Producer = Producer::new();
static mut BULK_OUT_PROD: Producer = Producer::new();

/// Resolved physical address of the head of the HID transfer ring; needed
/// when we re-arm the Normal TRB and ring the doorbell.
static mut HID_RING_PHYS: u64 = 0;
static mut BULK_IN_RING_PHYS: u64 = 0;
static mut BULK_OUT_RING_PHYS: u64 = 0;
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
/// Resolved physical address of the EP0 transfer ring.
static mut EP0_RING_PHYS: u64 = 0;
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

    enable_pci(loc);

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

    println!("[xhci] caplen={}  HCSPARAMS1=0x{:08X}  HCCPARAMS1=0x{:08X}",
        caplen, hcsparams1, hccparams1);
    println!("[xhci] MaxSlots={} MaxPorts={} MaxIntrs={} CSZ={} ScratchpadBufs={}",
        max_slots, max_ports, max_intrs, if csz1 { 1 } else { 0 }, max_scratchpad_bufs);

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

    // ---- Configure number of device slots enabled ----
    unsafe {
        let cfg = read_u32(op_base + op_reg::CONFIG as u64);
        let new = (cfg & !0xFF) | (MAX_SLOTS as u32 & 0xFF);
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
    unsafe {
        let cmd = read_u32(op_base + op_reg::USBCMD as u64);
        write_u32(op_base + op_reg::USBCMD as u64, cmd | usbcmd::RS);
        // Wait for HCH=0
        for _ in 0..1_000_000 {
            let sts = read_u32(op_base + op_reg::USBSTS as u64);
            if sts & usbsts::HCH == 0 { break; }
            core::hint::spin_loop();
        }
    }

    unsafe {
        INFO = Some(XhciInfo {
            mmio_base, op_base, run_base, db_base,
            max_slots, max_ports, csz1,
            max_scratchpad_bufs,
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
                    // Some other command completed; keep looking.
                    continue;
                }
            } else {
                // Port status changes etc. are fine to drop here; the
                // enumeration path polls PORTSC directly.
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
pub fn enumerate_ports() -> usize {
    let info = unsafe { match INFO { Some(i) => i, None => return 0 } };

    let mut connected = 0;
    let mut enumerated = 0;

    for port in 1..=info.max_ports {
        let portsc_addr = info.op_base + op_reg::PORTSC_BASE as u64
            + ((port as u64 - 1) * 0x10);
        let portsc = unsafe { read_u32(portsc_addr) };
        if portsc & portsc::CCS == 0 {
            continue;
        }
        connected += 1;
        println!("[xhci] port {}: connected (PORTSC=0x{:08X})", port, portsc);

        // Reset the port. USB 3 ports auto-train so PED gets set without a
        // host-initiated reset; USB 2 needs an explicit PR. Setting PR is
        // safe on both. After PR is written, wait for PRC=1.
        // When writing PORTSC, mask off RW1C change bits to avoid clearing them.
        let preserve = portsc & !portsc::RW1C_MASK;
        unsafe { write_u32(portsc_addr, preserve | portsc::PR); }

        let mut prc_seen = false;
        for _ in 0..2_000_000 {
            let s = unsafe { read_u32(portsc_addr) };
            if s & portsc::PRC != 0 {
                prc_seen = true;
                // Clear PRC by writing 1 to it.
                unsafe { write_u32(portsc_addr, (s & !portsc::RW1C_MASK) | portsc::PRC); }
                break;
            }
            core::hint::spin_loop();
        }
        if !prc_seen {
            println!("[xhci] port {} reset never completed", port);
            continue;
        }
        let portsc_after = unsafe { read_u32(portsc_addr) };
        if portsc_after & portsc::PED == 0 {
            println!("[xhci] port {} not enabled after reset (PORTSC=0x{:08X})", port, portsc_after);
            continue;
        }
        let speed = ((portsc_after & portsc::SPEED_MASK) >> portsc::SPEED_SHIFT) as u8;

        // Enumerate this device. If it succeeds, remember it and stop.
        if enumerate_device(port, speed) {
            enumerated += 1;
            break;
        }
    }

    println!("  [DEMO 18] PASS: enumerated {} USB device(s)", enumerated);
    let _ = connected;
    enumerated
}

/// Enumerate a single attached device on `port`:
///  - EnableSlot
///  - Build Input Context (slot + EP0)
///  - AddressDevice
///  - Read first 8 bytes of device descriptor (to learn EP0 max packet)
///  - If max packet differs, EvaluateContext / re-set EP0 (skipped for boot
///    speeds where 64 is standard for HS/FS)
///  - Read full 18-byte device descriptor
///  - Read configuration descriptor, parse interface + endpoint
///  - If HID boot keyboard, set up the Interrupt-IN transfer ring
fn enumerate_device(port: u8, speed: u8) -> bool {
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

    // ---- Plug per-slot device context into DCBAA ----
    let dev_ctx_phys = match phys_of(unsafe { &raw const DEVICE_CTX_SLOT1 } as u64) {
        Some(p) => p,
        None => { println!("[xhci] dev ctx phys translation failed"); return false; }
    };
    unsafe { DCBAA.0[slot_id as usize] = dev_ctx_phys; }

    // ---- Build the EP0 transfer ring ----
    let ep0_ring_phys = match phys_of(unsafe { &raw const EP0_TRANSFER_RING } as u64) {
        Some(p) => p,
        None => { println!("[xhci] EP0 ring phys translation failed"); return false; }
    };
    unsafe {
        init_command_ring(&raw mut EP0_TRANSFER_RING, ep0_ring_phys);
        EP0_PROD = Producer::new();
        EP0_RING_PHYS = ep0_ring_phys;
    }

    // ---- Build the Input Context ----
    // Speed 1=Full(12 Mb/s, mps0=8 default), 2=Low(1.5, mps0=8), 3=High(480, mps0=64),
    //       4=Super(5 Gb/s, mps0=512). Spec §6.2.2 Table 6-9.
    // For FS we use 8 then (in a more complete driver) issue Evaluate Context after
    // the first descriptor read to switch to the device's actual MPS0; for HS the
    // spec mandates 64 so no follow-up is needed.
    let mps0: u16 = match speed { 1 => 8, 2 => 8, 3 => 64, 4 => 512, _ => 8 };
    unsafe {
        let ic = &mut INPUT_CTX_SLOT1.0;
        // Zero it.
        ic.reset();
        // Input Control Context: A0 (slot) + A1 (EP0) added.
        ic.input_ctrl_mut().add_flags = (1 << 0) | (1 << 1);
        // Slot context: 1 context entry (EP0 only), root hub port, speed.
        let slot = ic.slot_mut();
        slot.set_context_entries(1);
        slot.set_root_hub_port(port);
        slot.set_speed(speed as u32);
        // EP0 endpoint context (idx 0 = DCI 1).
        ic.ep_mut(0).init_control_ep(mps0, ep0_ring_phys, true);
    }
    let input_phys = match phys_of(unsafe { &raw const INPUT_CTX_SLOT1 } as u64) {
        Some(p) => p,
        None => { println!("[xhci] input ctx phys translation failed"); return false; }
    };

    // ---- AddressDevice ----
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
    if cc != cc::SUCCESS {
        println!("[xhci] AddressDevice failed: cc={}", cc);
        return false;
    }

    let usb_addr = unsafe { DEVICE_CTX_SLOT1.0.slot_read().usb_device_address() };
    println!("[xhci] device addressed: slot={} usb_addr={} speed={} mps0={}",
        slot_id, usb_addr, speed, mps0);

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
    println!("  [DEMO 18] PASS: keyboard descriptor parsed (vendor=0x{:04X} product=0x{:04X})",
        id_vendor, id_product);

    // ---- GET_DESCRIPTOR(CONFIGURATION, 0) — first 9 bytes for total length ----
    let mut cfg_desc = ConfigDescriptor::default();
    if !control_in(slot_id, 0x80, request::GET_DESCRIPTOR,
                   (desc_type::CONFIGURATION as u16) << 8, 0, 9,
                   &mut cfg_desc as *mut _ as *mut u8, 9) {
        println!("[xhci] GET_DESCRIPTOR(CONFIG short) failed");
        return false;
    }
    let total_len = cfg_desc.w_total_length as usize;
    if total_len > SETUP_BUF_SIZE {
        println!("[xhci] config descriptor too large ({}) — abort", total_len);
        return false;
    }

    // ---- Read full configuration blob into DMA_BUF ----
    let blob_phys = match phys_of(unsafe { &raw const DMA_BUF } as u64) {
        Some(p) => p,
        None => { println!("[xhci] DMA_BUF phys translation failed"); return false; }
    };
    if !control_in_phys(slot_id, 0x80, request::GET_DESCRIPTOR,
                        (desc_type::CONFIGURATION as u16) << 8, 0,
                        total_len as u16, blob_phys, total_len) {
        println!("[xhci] GET_DESCRIPTOR(CONFIG full) failed");
        return false;
    }

    // ---- Parse the descriptor chain looking for HID boot keyboard ----
    let blob = unsafe { &DMA_BUF.0[..total_len] };
    let (kbd, iface_num, cfg_val) = match find_boot_keyboard(blob, cfg_desc.b_configuration_value) {
        Some(x) => x,
        None => {
            println!("[xhci] no HID boot keyboard — trying Mass Storage path");
            // Stash a generic device record first so the DEMOs see vendor/product
            // even if the MSC enumeration also doesn't match (e.g. it's actually
            // some other class entirely).
            unsafe {
                DEVICE = Some(EnumeratedDevice {
                    slot_id, usb_address: usb_addr, port, speed,
                    vendor: id_vendor, product: id_product,
                    max_packet_ep0: mps0,
                    is_keyboard: false,
                    kbd_ep_in: 0, kbd_ep_packet_size: 0, kbd_ep_interval: 0,
                    config_value: cfg_desc.b_configuration_value,
                    interface_number: 0,
                });
            }
            // Try the Mass Storage path. Returns true if it found + configured
            // an MSC device; false if no MSC interface was in the descriptor.
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
            }
            return true;
        }
    };

    // ---- SET_CONFIGURATION ----
    if !control_out(slot_id, 0x00, request::SET_CONFIGURATION,
                    cfg_val as u16, 0, 0) {
        println!("[xhci] SET_CONFIGURATION failed");
        return false;
    }

    // ---- SET_PROTOCOL (boot=0) on the HID interface ----
    if !control_out(slot_id, 0x21, request::HID_SET_PROTOCOL,
                    0, iface_num as u16, 0) {
        println!("[xhci] HID SET_PROTOCOL(boot) failed (non-fatal on some devs)");
        // Not fatal — qemu's usb-kbd defaults to boot protocol.
    }

    // ---- ConfigureEndpoint to add the HID Interrupt-IN endpoint ----
    let hid_ring_phys = match phys_of(unsafe { &raw const HID_TRANSFER_RING } as u64) {
        Some(p) => p,
        None => { println!("[xhci] HID ring phys translation failed"); return false; }
    };
    let ep_num = kbd.b_endpoint_address & 0x0F;
    let dci = (ep_num * 2 + 1) as usize; // IN endpoint → DCI = 2*N+1
    // Encode the interval as log2(b_interval) for HS/SS, b_interval directly for LS/FS.
    // For QEMU usb-kbd in HS this is something like 7 (128 microframes).
    let interval_log2 = encode_interval(speed, kbd.b_interval);
    let max_packet = kbd.w_max_packet_size & 0x07FF;

    unsafe {
        init_command_ring(&raw mut HID_TRANSFER_RING, hid_ring_phys);
        HID_PROD = Producer::new();
        HID_RING_PHYS = hid_ring_phys;

        let ic = &mut INPUT_CTX_SLOT1.0;
        // Reuse the input context. Add-flag for slot (A0) and the HID EP (Adci).
        // We also need to bump context entries to dci.
        ic.reset();
        ic.input_ctrl_mut().add_flags = (1 << 0) | (1u32 << dci);
        let slot = ic.slot_mut();
        slot.set_context_entries(dci as u32);
        slot.set_root_hub_port(port);
        slot.set_speed(speed as u32);
        // EP0 was already configured during AddressDevice — ConfigureEndpoint
        // re-states the slot but doesn't re-add EP0 (A1 not set above).
        // Configure the HID EP at index dci-1 in our eps array.
        ic.ep_mut(dci - 1).init_interrupt_in_ep(max_packet, interval_log2, hid_ring_phys, true);
    }

    let idx = unsafe { CMD_PROD.enqueue };
    let cmd_phys = cmd_trb_phys_at(idx);
    let control = ((trb_type::CONFIGURE_ENDPOINT_CMD as u32) << 10) | ((slot_id as u32) << 24);
    unsafe {
        enqueue_command(
            &raw mut COMMAND_RING, &mut CMD_PROD,
            input_phys, 0, control,
        );
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
        DEVICE = Some(EnumeratedDevice {
            slot_id, usb_address: usb_addr, port, speed,
            vendor: id_vendor, product: id_product,
            max_packet_ep0: mps0,
            is_keyboard: true,
            kbd_ep_in: kbd.b_endpoint_address,
            kbd_ep_packet_size: max_packet,
            kbd_ep_interval: kbd.b_interval,
            config_value: cfg_val,
            interface_number: iface_num,
        });
    }

    // Prime the HID transfer ring with one Normal TRB pointing at HID_REPORT_BUF
    // so the controller fills it on the first scheduled interval.
    arm_hid_read(slot_id, dci as u8);

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

fn control_in(
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

    let status_trb_phys = unsafe {
        let idx_setup = EP0_PROD.enqueue;
        // Setup
        enqueue_command(&raw mut EP0_TRANSFER_RING, &mut EP0_PROD,
            setup_param, setup_status, setup_control);
        // Data
        enqueue_command(&raw mut EP0_TRANSFER_RING, &mut EP0_PROD,
            out_phys, data_status, data_control);
        // Status — remember its physical address so we can match its Transfer Event.
        let status_idx = EP0_PROD.enqueue;
        let phys = EP0_RING_PHYS + (status_idx as u64) * 16;
        enqueue_command(&raw mut EP0_TRANSFER_RING, &mut EP0_PROD,
            0, status_status, status_control);
        let _ = idx_setup;
        phys
    };

    // Doorbell: slot_id, target = 1 (EP0 DCI).
    ring_doorbell(info.db_base, slot_id, 1);

    // Spin for a Transfer Event matching our Status TRB. We accept SUCCESS
    // or SHORT_PACKET (the device returning less than requested is fine for
    // descriptor reads).
    for _ in 0..200_000_000u64 {
        if let Some(evt) = poll_event() {
            if evt.trb_type() == trb_type::TRANSFER_EVENT {
                let cc = evt.completion_code();
                if cc == cc::SUCCESS || cc == cc::SHORT_PACKET {
                    return true;
                }
                if cc == cc::STALL_ERROR {
                    println!("[xhci] control_in stall (cc={})", cc);
                    return false;
                }
                println!("[xhci] control_in bad cc={} (param=0x{:X} want 0x{:X})",
                    cc, evt.parameter, status_trb_phys);
                return false;
            }
        }
        core::hint::spin_loop();
    }
    println!("[xhci] control_in timeout");
    false
}

fn control_out(
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

    let _status_trb_phys = unsafe {
        enqueue_command(&raw mut EP0_TRANSFER_RING, &mut EP0_PROD,
            setup_param, setup_status, setup_control);
        let status_idx = EP0_PROD.enqueue;
        let phys = EP0_RING_PHYS + (status_idx as u64) * 16;
        enqueue_command(&raw mut EP0_TRANSFER_RING, &mut EP0_PROD,
            0, status_status, status_control);
        phys
    };

    ring_doorbell(info.db_base, slot_id, 1);

    for _ in 0..200_000_000u64 {
        if let Some(evt) = poll_event() {
            if evt.trb_type() == trb_type::TRANSFER_EVENT {
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

/// Post a single Normal TRB on the HID transfer ring to receive the next
/// 8-byte report into HID_REPORT_BUF, then ring the doorbell.
fn arm_hid_read(slot_id: u8, dci: u8) {
    let info = unsafe { match INFO { Some(i) => i, None => return } };
    let buf_phys = match phys_of(unsafe { &raw const HID_REPORT_BUF } as u64) {
        Some(p) => p,
        None => { println!("[xhci] HID_REPORT_BUF phys translation failed"); return; }
    };
    // Normal TRB: parameter=buf phys, status=length (8), control:
    // trb_type=Normal, IOC=1, ISP=1 (interrupt on short packet).
    let control = ((trb_type::NORMAL as u32) << 10)
        | (1u32 << 5)  /* IOC */
        | (1u32 << 2); /* ISP */
    unsafe {
        enqueue_command(&raw mut HID_TRANSFER_RING, &mut HID_PROD,
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
    while let Some(evt) = poll_event() {
        if evt.trb_type() == trb_type::TRANSFER_EVENT {
            let cc = evt.completion_code();
            if cc == cc::SUCCESS || cc == cc::SHORT_PACKET {
                let buf = unsafe { &HID_REPORT_BUF.0[..8] };
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
    let (ring, prod) = unsafe {
        if is_in {
            (&raw mut BULK_IN_TRANSFER_RING, &mut BULK_IN_PROD)
        } else {
            (&raw mut BULK_OUT_TRANSFER_RING, &mut BULK_OUT_PROD)
        }
    };
    // Normal TRB: parameter = buf phys, status = (len << 0) (TRB Transfer Length),
    // control = trb_type Normal | IOC=1 | ISP=1 (interrupt on short packet).
    let control = ((trb_type::NORMAL as u32) << 10) | (1u32 << 5) | (1u32 << 2);
    unsafe {
        enqueue_command(ring, prod, buf_phys, len, control);
    }
    ring_doorbell(info.db_base, slot_id, dci);

    // Poll the event ring for a TRANSFER_EVENT. Bounded to avoid hanging the
    // boot if the device misbehaves.
    let mut spins: u32 = 0;
    loop {
        if let Some(evt) = poll_event() {
            if evt.trb_type() == trb_type::TRANSFER_EVENT {
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

    // Translate ring phys + program them on the EPs.
    let in_phys = phys_of(unsafe { &raw const BULK_IN_TRANSFER_RING } as u64)?;
    let out_phys = phys_of(unsafe { &raw const BULK_OUT_TRANSFER_RING } as u64)?;
    unsafe {
        init_command_ring(&raw mut BULK_IN_TRANSFER_RING, in_phys);
        init_command_ring(&raw mut BULK_OUT_TRANSFER_RING, out_phys);
        BULK_IN_PROD = Producer::new();
        BULK_OUT_PROD = Producer::new();
        BULK_IN_RING_PHYS = in_phys;
        BULK_OUT_RING_PHYS = out_phys;

        let ic = &mut INPUT_CTX_SLOT1.0;
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

    let input_phys = phys_of(unsafe { &raw const INPUT_CTX_SLOT1 } as u64)?;

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
