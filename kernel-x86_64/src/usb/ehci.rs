//! Standalone EHCI (USB 2.0) host controller driver.
//!
//! # Why this exists
//!
//! The W540's Lynx Point xHCI has never reliably completed a USB-2 port
//! reset (ports stuck PLS=Polling PED=0 across ~30 builds), and the
//! XUSB2PR port-routing mask reads zero — BIOS doesn't let us migrate
//! the USB-2 ports to xHCI. So instead of fighting the chipset, this
//! driver enumerates USB-2 devices the way Windows 7 did: through the
//! EHCI companion controller, which owns those ports by default.
//!
//! # Topology reality on Lynx Point
//!
//! The physical USB-2 jacks do NOT hang off EHCI root ports. Each EHCI
//! controller (W540 has two: 00:1A.0 and 00:1D.0) has an internal Intel
//! **Rate Matching Hub** (vendor 0x8087) permanently attached to root
//! port 1; the physical jacks are that hub's downstream ports. So hub
//! enumeration is part of the core bring-up path here, not an extra.
//!
//! # Design
//!
//! Strictly synchronous, one transfer in flight: a reclamation-head QH
//! (H=1) is the only permanent member of the async schedule; each
//! control/bulk transfer links one transfer QH behind it, polls the
//! final qTD for completion, then unlinks with the IAA doorbell
//! handshake. Slow, but deterministic and debuggable on serial-less
//! hardware. High-speed devices are native; low/full-speed children
//! behind the RMH get the QH split-transaction fields (hub addr/port)
//! so the HC issues SSPLIT/CSPLIT for us.
//!
//! EHCI structures require 32-bit physical addresses; we verify each
//! static lands below 4 GiB at init and bail loudly if not.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

use crate::pci;
use crate::paging;
use crate::println;
use super::iphone;

// ============================================================================
// Register layout
// ============================================================================

const EHCI_CLASS: u8 = 0x0C;
const EHCI_SUBCLASS: u8 = 0x03;
const EHCI_PROG_IF: u8 = 0x20;

/// Verbose enumeration logging. Off by default: gates the per-interface
/// config dumps for non-Apple devices and the per-attempt GET_DESCRIPTOR
/// retry prints — pure debugging surface that floods the boot log on a
/// dock full of devices. Apple-device config dumps (the ipheth debugging
/// surface) stay unconditional. Flip to `true` to debug usbenum.
const VERBOSE_ENUM: bool = false;

// Capability registers (relative to BAR0).
mod cap {
    pub const CAPLENGTH: u64 = 0x00; // u8
    pub const HCSPARAMS: u64 = 0x04;
    pub const HCCPARAMS: u64 = 0x08;
}

// Operational registers (relative to BAR0 + CAPLENGTH).
mod op {
    pub const USBCMD: u64 = 0x00;
    pub const USBSTS: u64 = 0x04;
    pub const USBINTR: u64 = 0x08;
    pub const CTRLDSSEGMENT: u64 = 0x10;
    pub const ASYNCLISTADDR: u64 = 0x18;
    pub const CONFIGFLAG: u64 = 0x40;
    pub const PORTSC_BASE: u64 = 0x44;
}

mod usbcmd {
    pub const RS: u32 = 1 << 0;
    pub const HCRESET: u32 = 1 << 1;
    pub const ASE: u32 = 1 << 5;
    pub const IAAD: u32 = 1 << 6;
}

mod usbsts {
    pub const IAA: u32 = 1 << 5;
    pub const HCH: u32 = 1 << 12;
    pub const ASS: u32 = 1 << 15;
}

mod portsc {
    pub const CCS: u32 = 1 << 0;
    pub const CSC: u32 = 1 << 1;
    pub const PED: u32 = 1 << 2;
    pub const PEDC: u32 = 1 << 3;
    pub const OCC: u32 = 1 << 5;
    pub const PR: u32 = 1 << 8;
    pub const LINE_MASK: u32 = 0x3 << 10;
    pub const LINE_K: u32 = 0x1 << 10; // K-state = low-speed device
    pub const PP: u32 = 1 << 12;
    pub const PO: u32 = 1 << 13;
    /// Write-1-to-clear bits that must be masked on read-modify-write.
    pub const RW1C: u32 = CSC | PEDC | OCC;
}

// Link pointer encoding (QH horizontal links and qTD next pointers).
const LINK_T: u32 = 1; // terminate
const LINK_TYP_QH: u32 = 1 << 1;

// qTD token bits.
mod token {
    pub const ACTIVE: u32 = 1 << 7;
    pub const HALTED: u32 = 1 << 6;
    pub const BUFERR: u32 = 1 << 5;
    pub const BABBLE: u32 = 1 << 4;
    pub const XACTERR: u32 = 1 << 3;
    pub const MISSED_UF: u32 = 1 << 2;
    pub const ERR_MASK: u32 = HALTED | BUFERR | BABBLE | XACTERR | MISSED_UF;
    pub const PID_OUT: u32 = 0 << 8;
    pub const PID_IN: u32 = 1 << 8;
    pub const PID_SETUP: u32 = 2 << 8;
    pub const CERR_3: u32 = 3 << 10;
    pub const IOC: u32 = 1 << 15;
    pub const TOTAL_SHIFT: u32 = 16;
    pub const TOTAL_MASK: u32 = 0x7FFF << 16;
    pub const DT: u32 = 1 << 31;
}

// Device speeds in QH "EPS" encoding (NOT the same as xHCI's enum).
pub const EPS_FS: u32 = 0;
pub const EPS_LS: u32 = 1;
pub const EPS_HS: u32 = 2;

// ============================================================================
// DMA structures
// ============================================================================

/// Queue Head: 12 architectural dwords, padded to 128 bytes so two QHs
/// never share a cache line and never cross a 4 KiB boundary.
/// dw[0]=horiz link, dw[1]=endpoint characteristics, dw[2]=endpoint
/// capabilities, dw[3]=current qTD, dw[4..12]=qTD overlay
/// (next, alt-next, token, buf0..buf4).
#[repr(C, align(128))]
struct Qh([u32; 32]);

/// qTD: 8 architectural dwords (next, alt-next, token, buf0..buf4),
/// padded to 64 bytes.
#[repr(C, align(64))]
struct Qtd([u32; 16]);

const NUM_QTDS: usize = 4;

/// All schedule structures in one page so nothing crosses a 4 KiB
/// boundary and a single phys_of() translation covers the lot.
#[repr(C, align(4096))]
struct AsyncPage {
    head: Qh,
    xfer: Qh,
    qtds: [Qtd; NUM_QTDS],
}

/// One schedule page PER CONTROLLER. The W540 has two EHCI controllers
/// running simultaneously; sharing one head QH between them means both
/// HCs traverse (and write back!) the same schedule — racing qTD token
/// writebacks corrupted transfers on real hardware (zero-byte reads,
/// cascading qTD halts). Each controller gets a private async list.
const MAX_CTLS: usize = 4;
static mut ASYNCPS: [AsyncPage; MAX_CTLS] = [const {
    AsyncPage {
        head: Qh([0; 32]),
        xfer: Qh([0; 32]),
        qtds: [const { Qtd([0; 16]) }; NUM_QTDS],
    }
}; MAX_CTLS];

/// Setup packets (8 bytes used).
#[repr(C, align(64))]
struct SetupBuf([u8; 64]);
static mut SETUP_BUF: SetupBuf = SetupBuf([0; 64]);

/// Data stage buffer — one page, page-aligned, so it is physically
/// contiguous and a single qTD buffer pointer covers it.
#[repr(C, align(4096))]
struct DataBuf([u8; 4096]);
static mut DATA_BUF: DataBuf = DataBuf([0; 4096]);

// ============================================================================
// Controller + device state
// ============================================================================

#[derive(Copy, Clone)]
struct Ctl {
    loc: pci::Location,
    op_base: u64,
    n_ports: u8,
    ppc: bool,
    /// Index into ASYNCPS — this controller's private schedule page.
    page: usize,
    /// Next USB device address to hand out (1..127, per controller bus).
    next_addr: u8,
    /// Physical addresses of this controller's schedule statics
    /// (32-bit, validated).
    head_phys: u32,
    xfer_phys: u32,
    qtd_phys: [u32; NUM_QTDS],
}

static mut CTLS: [Option<Ctl>; MAX_CTLS] = [None; MAX_CTLS];

/// Everything `control()`/`bulk()` need to know about one device.
#[derive(Copy, Clone)]
pub struct DevCtx {
    pub addr: u8,
    pub mps0: u16,
    pub eps: u32,
    /// Nearest high-speed hub (for LS/FS split transactions); 0 = none.
    pub hub_addr: u8,
    pub hub_port: u8,
}

#[derive(Copy, Clone)]
pub struct EhciDevice {
    pub ctl: u8,
    pub addr: u8,
    pub mps0: u16,
    pub vendor: u16,
    pub product: u16,
    pub class: u8,
    pub eps: u32,
    pub hub_addr: u8,
    pub hub_port: u8,
    pub is_iphone: bool,
}

const MAX_DEVICES: usize = 32;
static mut DEVICES: [Option<EhciDevice>; MAX_DEVICES] = [None; MAX_DEVICES];

fn record_device(d: EhciDevice) {
    unsafe {
        for slot in DEVICES.iter_mut() {
            if slot.is_none() {
                *slot = Some(d);
                return;
            }
        }
    }
}

// ============================================================================
// Small helpers
// ============================================================================

unsafe fn read_u32(addr: u64) -> u32 {
    read_volatile(addr as *const u32)
}

unsafe fn write_u32(addr: u64, val: u32) {
    write_volatile(addr as *mut u32, val);
}

fn phys_of(virt: u64) -> Option<u64> {
    let page = virt & !0xFFF;
    let off = virt & 0xFFF;
    paging::walk_active_pml4(page).map(|p| p + off)
}

/// Translate and require a 32-bit physical address (EHCI is 32-bit).
fn phys32_of(virt: u64) -> Option<u32> {
    match phys_of(virt) {
        Some(p) if p < 0x1_0000_0000 => Some(p as u32),
        Some(p) => {
            println!("[ehci] FATAL: structure at phys 0x{:X} is above 4 GiB", p);
            None
        }
        None => {
            println!("[ehci] FATAL: phys translation failed for 0x{:X}", virt);
            None
        }
    }
}

/// Sleep at least `ms` milliseconds using the scheduler tick (62 Hz, so
/// resolution is ~16 ms; always rounds up by one tick).
fn sleep_ms(ms: u64) {
    let ticks_needed = (ms * 62).div_ceil(1000) + 1;
    let end = kernel_core::platform::ticks() + ticks_needed;
    while kernel_core::platform::ticks() < end {
        core::hint::spin_loop();
    }
}

// ============================================================================
// Controller init
// ============================================================================

/// BIOS→OS handoff via the EHCI Legacy Support capability, which lives
/// in PCI CONFIG space at EECP (unlike xHCI's, which is MMIO). Sets the
/// OS-owned semaphore (bit 24), waits for BIOS-owned (bit 16) to clear,
/// then writes LEGCTLSTS=0 to disarm every SMI source.
fn bios_handoff(loc: pci::Location, mmio: u64) {
    let hccparams = unsafe { read_u32(mmio + cap::HCCPARAMS) };
    let eecp = ((hccparams >> 8) & 0xFF) as u8;
    if eecp < 0x40 {
        return; // no extended caps (QEMU)
    }
    let legsup = pci::read_u32(loc.bus, loc.slot, loc.func, eecp);
    if legsup & 0xFF == 0x01 {
        pci::write_u32(loc.bus, loc.slot, loc.func, eecp, legsup | (1 << 24));
        let mut bios_cleared = false;
        for _ in 0..1_000_000u32 {
            let v = pci::read_u32(loc.bus, loc.slot, loc.func, eecp);
            if v & (1 << 16) == 0 {
                bios_cleared = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !bios_cleared {
            println!("[ehci] {}:{:02X}.{} BIOS refused handoff (LEGSUP stuck) — continuing",
                loc.bus, loc.slot, loc.func);
        }
    }
    // Disarm all SMI enables + W1C the latched status bits. Same write
    // disarm_ehci_smi() does, repeated here so this driver is
    // self-sufficient if the xHCI-side call order changes.
    pci::write_u32(loc.bus, loc.slot, loc.func, eecp + 4, 0);
}

/// Bring one EHCI controller from whatever state BIOS left it in to
/// "running, async schedule enabled, all ports routed to EHCI".
/// `page` selects this controller's private ASYNCPS schedule page.
fn init_controller(loc: pci::Location, page: usize) -> Option<Ctl> {
    let bar0 = pci::read_u32(loc.bus, loc.slot, loc.func, 0x10);
    let mmio_phys = (bar0 & !0xF) as u64;
    if mmio_phys == 0 {
        println!("[ehci] {}:{:02X}.{} BAR0=0 — skipping", loc.bus, loc.slot, loc.func);
        return None;
    }
    loc.enable_io_and_bus_master();
    let mmio = paging::phys_to_virt(mmio_phys);

    bios_handoff(loc, mmio);

    let caplen = unsafe { read_volatile(mmio as *const u8) } as u64;
    let hcsparams = unsafe { read_u32(mmio + cap::HCSPARAMS) };
    let hccparams = unsafe { read_u32(mmio + cap::HCCPARAMS) };
    let n_ports = (hcsparams & 0xF) as u8;
    let ppc = hcsparams & (1 << 4) != 0;
    let addr64 = hccparams & 1 != 0;
    let op_base = mmio + caplen;

    // Halt if running.
    let cmd0 = unsafe { read_u32(op_base + op::USBCMD) };
    if cmd0 & usbcmd::RS != 0 {
        unsafe { write_u32(op_base + op::USBCMD, cmd0 & !usbcmd::RS); }
        for _ in 0..1_000_000u32 {
            if unsafe { read_u32(op_base + op::USBSTS) } & usbsts::HCH != 0 { break; }
            core::hint::spin_loop();
        }
    }

    // Full reset.
    unsafe { write_u32(op_base + op::USBCMD, usbcmd::HCRESET); }
    let mut reset_done = false;
    for _ in 0..1_000_000u32 {
        if unsafe { read_u32(op_base + op::USBCMD) } & usbcmd::HCRESET == 0 {
            reset_done = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !reset_done {
        println!("[ehci] {}:{:02X}.{} HCRESET never completed — skipping",
            loc.bus, loc.slot, loc.func);
        return None;
    }

    // Schedule structures (this controller's private page).
    let head_virt = unsafe { &raw const ASYNCPS[page].head } as u64;
    let xfer_virt = unsafe { &raw const ASYNCPS[page].xfer } as u64;
    let head_phys = phys32_of(head_virt)?;
    let xfer_phys = phys32_of(xfer_virt)?;
    let mut qtd_phys = [0u32; NUM_QTDS];
    for (i, item) in qtd_phys.iter_mut().enumerate() {
        *item = phys32_of(unsafe { &raw const ASYNCPS[page].qtds[i] } as u64)?;
    }

    // Reclamation head: H=1, self-linked, overlay halted so the HC
    // never tries to execute transfers on it.
    unsafe {
        let h = &raw mut ASYNCPS[page].head.0;
        for dw in (*h).iter_mut() { *dw = 0; }
        (*h)[0] = head_phys | LINK_TYP_QH; // points at itself
        (*h)[1] = 1 << 15;                 // H = head of reclamation list
        (*h)[4] = LINK_T;                  // overlay next qTD: terminate
        (*h)[5] = LINK_T;                  // overlay alt-next: terminate
        (*h)[6] = token::HALTED;           // overlay token: halted
    }
    fence(Ordering::SeqCst);

    unsafe {
        if addr64 {
            write_u32(op_base + op::CTRLDSSEGMENT, 0);
        }
        write_u32(op_base + op::USBINTR, 0); // we poll
        write_u32(op_base + op::ASYNCLISTADDR, head_phys);
        // Run with the async schedule on. Periodic schedule stays off —
        // this driver does control + bulk only. ITC=8 (default).
        write_u32(op_base + op::USBCMD, (8 << 16) | usbcmd::ASE | usbcmd::RS);
    }
    for _ in 0..1_000_000u32 {
        if unsafe { read_u32(op_base + op::USBSTS) } & usbsts::HCH == 0 { break; }
        core::hint::spin_loop();
    }
    // CONFIGFLAG=1: route all ports to EHCI (away from any companions).
    unsafe { write_u32(op_base + op::CONFIGFLAG, 1); }

    println!("[ehci] {}:{:02X}.{} up: mmio=0x{:X} n_ports={} ppc={} 64bit={}",
        loc.bus, loc.slot, loc.func, mmio_phys, n_ports, ppc as u8, addr64 as u8);

    Some(Ctl {
        loc, op_base, n_ports, ppc, page,
        next_addr: 1,
        head_phys, xfer_phys, qtd_phys,
    })
}

/// Discover every EHCI controller on bus 0 and bring each one up.
/// Returns the number of controllers running.
pub fn init() -> usize {
    let mut count = 0usize;
    for slot in 0..32u8 {
        for func in 0..8u8 {
            let loc = pci::Location { bus: 0, slot, func };
            if loc.vendor_id() == 0xFFFF { continue; }
            if pci::class_triple(loc) != (EHCI_CLASS, EHCI_SUBCLASS, EHCI_PROG_IF) {
                continue;
            }
            if count >= MAX_CTLS {
                println!("[ehci] more than {} controllers — ignoring extras", MAX_CTLS);
                break;
            }
            if let Some(ctl) = init_controller(loc, count) {
                unsafe { CTLS[count] = Some(ctl); }
                count += 1;
            }
        }
    }
    if count == 0 {
        println!("[ehci] no EHCI controllers found");
    }
    count
}

// ============================================================================
// Transfer engine
// ============================================================================

/// Token of the most recently failed qTD, for diagnostics on serial-less
/// hardware. Halted WITHOUT XactErr/Babble = endpoint STALL (a protocol
/// "no" from the device); Halted WITH XactErr = device not responding.
static mut LAST_ERR_TOKEN: u32 = 0;

fn err_token_name(tok: u32) -> &'static str {
    if tok & token::XACTERR != 0 { "XactErr/no-response" }
    else if tok & token::BABBLE != 0 { "babble" }
    else if tok & token::BUFERR != 0 { "buffer-error" }
    else if tok & token::MISSED_UF != 0 { "missed-uframe" }
    else if tok & token::HALTED != 0 { "STALL" }
    else { "?" }
}

/// Build one qTD in `page`'s pool. `buf_phys==0` means no buffer.
unsafe fn build_qtd(page: usize, idx: usize, next_phys: u32, alt_phys: u32, tok: u32, buf_phys: u32) {
    let q = &raw mut ASYNCPS[page].qtds[idx].0;
    (*q)[0] = if next_phys == 0 { LINK_T } else { next_phys };
    (*q)[1] = if alt_phys == 0 { LINK_T } else { alt_phys };
    (*q)[2] = tok;
    (*q)[3] = buf_phys;
    // Subsequent page pointers: our data buffer is exactly one page, so
    // only buf0 (+ buf1 in case the transfer straddles into the next
    // page after a non-zero offset — can't happen with page-aligned
    // DATA_BUF, but harmless to fill).
    (*q)[4] = if buf_phys != 0 { (buf_phys & !0xFFF).wrapping_add(0x1000) } else { 0 };
    (*q)[5] = 0;
    (*q)[6] = 0;
    (*q)[7] = 0;
}

/// QH endpoint-characteristics dword (dw1).
fn qh_dw1(dev: &DevCtx, ep: u8, mps: u16) -> u32 {
    let mut dw1 = (dev.addr as u32)
        | ((ep as u32 & 0xF) << 8)
        | (dev.eps << 12)
        | (1 << 14)                       // DTC: toggle from qTD token
        | ((mps as u32 & 0x7FF) << 16);
    // Control Endpoint Flag — required for LS/FS control endpoints so
    // the HC appends the PING/PRE protocol bits correctly. EP0 only.
    if dev.eps != EPS_HS && ep == 0 {
        dw1 |= 1 << 27;
    }
    dw1
}

/// QH endpoint-capabilities dword (dw2): Mult=1 + split-transaction
/// routing (nearest high-speed hub) for LS/FS devices.
fn qh_dw2(dev: &DevCtx) -> u32 {
    let mut dw2 = 1 << 30; // Mult = 1
    if dev.eps != EPS_HS && dev.hub_addr != 0 {
        dw2 |= (dev.hub_addr as u32) << 16;
        dw2 |= (dev.hub_port as u32) << 23;
        // C-mask: for async split transfers the HC manages SSPLIT/CSPLIT
        // scheduling itself; masks stay zero.
    }
    dw2
}

/// Link the transfer QH behind the head, wait for `last_qtd` to retire,
/// then unlink with the IAA handshake. Returns Err on transfer error or
/// timeout. On success the caller can inspect retired qTD tokens.
/// `last_qtd_idx` is also the count hint: only qTDs 0..=last_qtd_idx are
/// part of this transfer, so the error scan ignores stale retired tokens
/// from earlier transfers.
unsafe fn run_xfer(ctl: &Ctl, dw1: u32, dw2: u32, first_qtd: u32, last_qtd_idx: usize, timeout_ms: u64) -> Result<(), &'static str> {
    let pg = ctl.page;
    // INSERT the transfer QH between head and whatever head currently
    // points at (head itself when idle, the persistent ipheth RX QH when
    // armed) — and restore that link on unlink, so one-shot transfers
    // never orphan the RX QH.
    let chain_next = read_volatile(&raw const ASYNCPS[pg].head.0[0]);
    // Initialize the transfer QH.
    let x = &raw mut ASYNCPS[pg].xfer.0;
    for dw in (*x).iter_mut() { *dw = 0; }
    (*x)[0] = chain_next;
    (*x)[1] = dw1;
    (*x)[2] = dw2;
    (*x)[4] = first_qtd;  // overlay next qTD
    (*x)[5] = LINK_T;     // overlay alt-next
    fence(Ordering::SeqCst);

    // Link into the schedule (single atomic dword write).
    write_volatile(&raw mut ASYNCPS[pg].head.0[0], ctl.xfer_phys | LINK_TYP_QH);
    fence(Ordering::SeqCst);

    // Wait for the last qTD to retire (Active clears) or any error.
    let deadline = kernel_core::platform::ticks() + (timeout_ms * 62).div_ceil(1000) + 2;
    let mut result: Result<(), &'static str> = Err("timeout");
    'wait: while kernel_core::platform::ticks() < deadline {
        let tok = read_volatile(&raw const ASYNCPS[pg].qtds[last_qtd_idx].0[2]);
        if tok & token::ACTIVE == 0 {
            if tok & token::ERR_MASK != 0 {
                LAST_ERR_TOKEN = tok;
                result = Err("last qTD error");
            } else {
                result = Ok(());
            }
            break 'wait;
        }
        // An earlier qTD halting stops the queue without ever touching
        // the last one — check for that so errors don't become timeouts.
        for i in 0..=last_qtd_idx {
            let t = read_volatile(&raw const ASYNCPS[pg].qtds[i].0[2]);
            if t & token::ACTIVE == 0 && t & token::HALTED != 0 {
                LAST_ERR_TOKEN = t;
                result = Err("qTD halted");
                break 'wait;
            }
        }
        core::hint::spin_loop();
    }

    // Unlink: restore head's previous link, then ring the IAA doorbell
    // and wait for the advance interrupt status so we know the HC no
    // longer holds a cached pointer to the transfer QH.
    write_volatile(&raw mut ASYNCPS[pg].head.0[0], chain_next);
    fence(Ordering::SeqCst);
    let cmd = read_u32(ctl.op_base + op::USBCMD);
    write_u32(ctl.op_base + op::USBCMD, cmd | usbcmd::IAAD);
    for _ in 0..1_000_000u32 {
        let sts = read_u32(ctl.op_base + op::USBSTS);
        if sts & usbsts::IAA != 0 {
            write_u32(ctl.op_base + op::USBSTS, usbsts::IAA); // W1C
            break;
        }
        core::hint::spin_loop();
    }

    result
}

/// Control transfer. `data` semantics follow `w_length` + direction bit
/// in `bm_req_type` (0x80 = device-to-host). Returns the number of
/// bytes actually transferred in the data stage (0 for no-data), or
/// None on failure.
pub fn control(
    ctl_idx: usize,
    dev: &DevCtx,
    bm_req_type: u8,
    b_request: u8,
    w_value: u16,
    w_index: u16,
    data: Option<&mut [u8]>,
) -> Option<usize> {
    let ctl = unsafe { CTLS[ctl_idx]? };
    let is_in = bm_req_type & 0x80 != 0;
    let (w_length, data_buf) = match data {
        Some(b) => ((b.len().min(4096)) as u16, Some(b)),
        None => (0u16, None),
    };

    // Setup packet.
    unsafe {
        SETUP_BUF.0[0] = bm_req_type;
        SETUP_BUF.0[1] = b_request;
        SETUP_BUF.0[2..4].copy_from_slice(&w_value.to_le_bytes());
        SETUP_BUF.0[4..6].copy_from_slice(&w_index.to_le_bytes());
        SETUP_BUF.0[6..8].copy_from_slice(&w_length.to_le_bytes());
    }
    let setup_phys = phys32_of(&raw const SETUP_BUF as u64)?;
    let data_phys = phys32_of(&raw const DATA_BUF as u64)?;

    // Host-to-device data is staged through DATA_BUF.
    if !is_in && w_length > 0 {
        if let Some(ref b) = data_buf {
            unsafe { DATA_BUF.0[..w_length as usize].copy_from_slice(&b[..w_length as usize]); }
        }
    }

    let has_data = w_length > 0;
    // qTD 0: SETUP (DT=0). qTD 1: data (DT=1). qTD 2: status (DT=1,
    // opposite direction, zero length, IOC).
    let status_idx = if has_data { 2 } else { 1 };
    let pg = ctl.page;
    unsafe {
        let status_pid = if is_in && has_data { token::PID_OUT } else { token::PID_IN };
        build_qtd(
            pg, status_idx,
            0, 0,
            token::ACTIVE | token::CERR_3 | status_pid | token::IOC | token::DT,
            0,
        );
        if has_data {
            let data_pid = if is_in { token::PID_IN } else { token::PID_OUT };
            // alt-next jumps straight to status on a short IN read.
            build_qtd(
                pg, 1,
                ctl.qtd_phys[2], ctl.qtd_phys[2],
                token::ACTIVE | token::CERR_3 | data_pid | token::DT
                    | ((w_length as u32) << token::TOTAL_SHIFT),
                data_phys,
            );
        }
        // SETUP's successor is index 1 either way: the data qTD when
        // there's a data stage, the status qTD when there isn't.
        build_qtd(
            pg, 0,
            ctl.qtd_phys[1],
            0,
            token::ACTIVE | token::CERR_3 | token::PID_SETUP
                | (8 << token::TOTAL_SHIFT),
            setup_phys,
        );
    }

    let dw1 = qh_dw1(dev, 0, dev.mps0);
    let dw2 = qh_dw2(dev);
    let res = unsafe { run_xfer(&ctl, dw1, dw2, ctl.qtd_phys[0], status_idx, 1000) };
    if let Err(e) = res {
        let tok = unsafe { LAST_ERR_TOKEN };
        println!("[ehci] control failed ({}, tok=0x{:08X} {}): req=0x{:02X}/0x{:02X} val=0x{:04X} idx={} addr={}",
            e, tok, err_token_name(tok),
            bm_req_type, b_request, w_value, w_index, dev.addr);
        return None;
    }

    if has_data {
        let residual = unsafe {
            (read_volatile(&raw const ASYNCPS[pg].qtds[1].0[2]) & token::TOTAL_MASK)
                >> token::TOTAL_SHIFT
        } as usize;
        let got = (w_length as usize).saturating_sub(residual);
        if is_in {
            if let Some(b) = data_buf {
                let n = got.min(b.len());
                unsafe { b[..n].copy_from_slice(&DATA_BUF.0[..n]); }
            }
        }
        Some(got)
    } else {
        Some(0)
    }
}

/// Bulk transfer on endpoint `ep` (address WITHOUT direction bit;
/// `is_in` selects PID). `toggle` is the caller-maintained data toggle —
/// updated from the retired qTD so it stays correct across transfers.
/// Returns bytes actually transferred.
pub fn bulk(
    ctl_idx: usize,
    dev: &DevCtx,
    ep: u8,
    mps: u16,
    is_in: bool,
    toggle: &mut bool,
    buf: &mut [u8],
    len: usize,
) -> Option<usize> {
    let ctl = unsafe { CTLS[ctl_idx]? };
    let len = len.min(4096).min(buf.len());
    let data_phys = phys32_of(&raw const DATA_BUF as u64)?;
    if !is_in {
        unsafe { DATA_BUF.0[..len].copy_from_slice(&buf[..len]); }
    }
    let pg = ctl.page;
    unsafe {
        let pid = if is_in { token::PID_IN } else { token::PID_OUT };
        let dt = if *toggle { token::DT } else { 0 };
        build_qtd(
            pg, 0, 0, 0,
            token::ACTIVE | token::CERR_3 | pid | token::IOC | dt
                | ((len as u32) << token::TOTAL_SHIFT),
            data_phys,
        );
    }
    let dw1 = qh_dw1(dev, ep, mps);
    let dw2 = qh_dw2(dev);
    let res = unsafe { run_xfer(&ctl, dw1, dw2, ctl.qtd_phys[0], 0, 2000) };
    if res.is_err() {
        return None;
    }
    let final_tok = unsafe { read_volatile(&raw const ASYNCPS[pg].qtds[0].0[2]) };
    *toggle = final_tok & token::DT != 0;
    let residual = ((final_tok & token::TOTAL_MASK) >> token::TOTAL_SHIFT) as usize;
    let got = len.saturating_sub(residual);
    if is_in {
        unsafe { buf[..got].copy_from_slice(&DATA_BUF.0[..got]); }
    }
    Some(got)
}

// ============================================================================
// Enumeration
// ============================================================================

/// Reset one EHCI root port. Returns the attached device's EPS encoding
/// or None (nothing usable).
fn reset_root_port(ctl: &Ctl, port: u8) -> Option<u32> {
    let addr = ctl.op_base + op::PORTSC_BASE + ((port - 1) as u64) * 4;
    let sc = unsafe { read_u32(addr) };
    if sc & portsc::CCS == 0 {
        return None;
    }
    // Low-speed device on a ROOT port: K-state line status. With no
    // companion controller to hand it to, we can't drive it (root ports
    // are HS-only on EHCI). Behind the RMH this never happens — the hub
    // does the rate matching.
    if sc & portsc::LINE_MASK == portsc::LINE_K {
        println!("[ehci]   port {}: low-speed device on root port — unsupported (no companion)", port);
        return None;
    }
    // Debounce, then reset: PR=1 with PED cleared, ≥50 ms, PR=0, wait
    // for the HC to finish (PR self-clear can take 2 ms).
    sleep_ms(100);
    let pre = unsafe { read_u32(addr) } & !(portsc::RW1C | portsc::PED);
    unsafe { write_u32(addr, pre | portsc::PR); }
    sleep_ms(60);
    unsafe { write_u32(addr, pre & !portsc::PR); }
    let mut cleared = false;
    for _ in 0..1_000_000u32 {
        if unsafe { read_u32(addr) } & portsc::PR == 0 { cleared = true; break; }
        core::hint::spin_loop();
    }
    if !cleared {
        println!("[ehci]   port {}: PR stuck at 1", port);
        return None;
    }
    sleep_ms(20); // reset recovery
    let after = unsafe { read_u32(addr) };
    if after & portsc::PED != 0 {
        Some(EPS_HS)
    } else {
        // Full-speed device: EHCI can't run it on a root port without a
        // companion controller. (QEMU pure-EHCI has none; Lynx Point
        // jacks are behind the RMH so this is root-port-only.)
        println!("[ehci]   port {}: full-speed device on root port — unsupported (PORTSC=0x{:08X})", port, after);
        None
    }
}

/// Hub-class requests, EHCI flavor (mirrors usb/hub.rs which is
/// xHCI-slot-based). Feature numbers per USB 2.0 §11.24.2.
mod hub_feature {
    pub const PORT_RESET: u16 = 4;
    pub const PORT_POWER: u16 = 8;
    pub const C_PORT_CONNECTION: u16 = 16;
    pub const C_PORT_RESET: u16 = 20;
}

fn hub_get_port_status(ctl_idx: usize, dev: &DevCtx, port: u8) -> Option<(u16, u16)> {
    let mut buf = [0u8; 4];
    let got = control(ctl_idx, dev, 0xA3, 0 /*GET_STATUS*/, 0, port as u16, Some(&mut buf))?;
    if got < 4 { return None; }
    Some((
        u16::from_le_bytes([buf[0], buf[1]]),
        u16::from_le_bytes([buf[2], buf[3]]),
    ))
}

fn hub_set_port_feature(ctl_idx: usize, dev: &DevCtx, port: u8, feature: u16) -> bool {
    control(ctl_idx, dev, 0x23, 3 /*SET_FEATURE*/, feature, port as u16, None).is_some()
}

fn hub_clear_port_feature(ctl_idx: usize, dev: &DevCtx, port: u8, feature: u16) -> bool {
    control(ctl_idx, dev, 0x23, 1 /*CLEAR_FEATURE*/, feature, port as u16, None).is_some()
}

/// Bring up a hub device (the Lynx Point Rate Matching Hub, or a real
/// external hub) and enumerate every connected downstream port.
fn bring_up_hub(ctl_idx: usize, hub: &DevCtx, depth: u8) -> usize {
    if depth > 3 {
        println!("[ehci] hub depth > 3 — not descending further");
        return 0;
    }
    // Hub descriptor (class type 0x29 — HS hub; the RMH is one).
    let mut desc = [0u8; 16];
    match control(ctl_idx, hub, 0xA0, 6 /*GET_DESCRIPTOR*/, 0x2900, 0, Some(&mut desc)) {
        Some(n) if n >= 5 => {}
        _ => {
            println!("[ehci] hub addr={} descriptor read failed", hub.addr);
            return 0;
        }
    }
    let n_ports = desc[2];
    let pwr_good_ms = (desc[5] as u64) * 2;
    println!("[ehci] hub addr={}: {} downstream ports (pwr-good {} ms)",
        hub.addr, n_ports, pwr_good_ms);

    // Power every port first, then wait once.
    for port in 1..=n_ports {
        let _ = hub_set_port_feature(ctl_idx, hub, port, hub_feature::PORT_POWER);
    }
    sleep_ms(pwr_good_ms.max(50) + 50);

    let mut enumerated = 0usize;
    for port in 1..=n_ports {
        if bring_up_hub_port(ctl_idx, hub, port, depth) {
            enumerated += 1;
        }
    }
    enumerated
}

/// Reset one downstream port of `hub` and enumerate whatever is attached.
/// Shared by the full-scan path (`bring_up_hub`) and the incremental
/// hot-plug path (`enumerate_incremental`). Returns true if a device was
/// enumerated on this port.
///
/// `depth` is the hub's own depth; children enumerate at the same `depth`
/// (matching the original inline code — `bring_up_hub` is entered with
/// `depth + 1`, so a second-tier hub recursing through `enumerate_device`
/// → `bring_up_hub` still gets its own +1, preserving the descent guard).
///
/// Dock cascade hardening: if `enumerate_device` fails on a connected
/// port, the port is reset once more and enumeration retried a single
/// time before giving up. On the W540, an iPhone behind the Pro Dock's
/// hub enumerated once then failed to re-appear on later usbenum runs;
/// the single fresh reset before retry recovers that race.
fn bring_up_hub_port(ctl_idx: usize, hub: &DevCtx, port: u8, depth: u8) -> bool {
    let (status, change) = match hub_get_port_status(ctl_idx, hub, port) {
        Some(s) => s,
        None => {
            println!("[ehci] hub addr={} port {}: GET_PORT_STATUS failed", hub.addr, port);
            return false;
        }
    };
    if status & 0x0001 == 0 {
        return false; // nothing connected
    }
    println!("[ehci] hub addr={} port {}: connected (status=0x{:04X} change=0x{:04X})",
        hub.addr, port, status, change);
    let _ = hub_clear_port_feature(ctl_idx, hub, port, hub_feature::C_PORT_CONNECTION);

    let child_eps = match hub_reset_port(ctl_idx, hub, port) {
        Some(eps) => eps,
        None => return false,
    };
    // LS/FS children use split transactions through the nearest
    // HIGH-SPEED hub. If this hub is itself HS, that's it; if this
    // hub is LS/FS (it can't legally be LS, but FS hubs exist),
    // splits keep routing through the hub's own TT ancestor.
    let (tt_addr, tt_port) = if hub.eps == EPS_HS {
        (hub.addr, port)
    } else {
        (hub.hub_addr, hub.hub_port)
    };
    if enumerate_device(ctl_idx, child_eps, tt_addr, tt_port, depth) {
        return true;
    }
    // Dock cascade hardening: re-reset this port once (SET_FEATURE
    // PORT_RESET, wait for C_PORT_RESET — handled by hub_reset_port) and
    // retry enumerate_device once more before giving up. Covers the
    // iPhone-behind-dock case where the first enumerate raced the
    // device's own re-attach.
    println!("[ehci] hub addr={} port {}: enumerate failed — re-resetting once and retrying", hub.addr, port);
    let child_eps2 = match hub_reset_port(ctl_idx, hub, port) {
        Some(eps) => eps,
        None => return false,
    };
    enumerate_device(ctl_idx, child_eps2, tt_addr, tt_port, depth)
}

/// Reset a downstream hub port and return the attached child's EPS speed
/// encoding, or None if the port never enabled. Issues SET_FEATURE
/// PORT_RESET, waits for C_PORT_RESET, and — if the device dropped off
/// the bus during reset — debounces and retries the reset once. Assumes
/// the caller already cleared C_PORT_CONNECTION.
fn hub_reset_port(ctl_idx: usize, hub: &DevCtx, port: u8) -> Option<u32> {
    // Reset the port; wait for C_PORT_RESET.
    if !hub_set_port_feature(ctl_idx, hub, port, hub_feature::PORT_RESET) {
        println!("[ehci] hub addr={} port {}: SET_FEATURE(PORT_RESET) failed", hub.addr, port);
        return None;
    }
    let mut reset_done = false;
    for _ in 0..50 {
        sleep_ms(20);
        if let Some((_s, c)) = hub_get_port_status(ctl_idx, hub, port) {
            if c & 0x0010 != 0 { // C_PORT_RESET
                reset_done = true;
                break;
            }
        } else {
            break;
        }
    }
    if !reset_done {
        println!("[ehci] hub addr={} port {}: reset never completed", hub.addr, port);
        return None;
    }
    let _ = hub_clear_port_feature(ctl_idx, hub, port, hub_feature::C_PORT_RESET);
    sleep_ms(30); // reset recovery (spec: 10 ms minimum)

    let (mut status2, _c2) = hub_get_port_status(ctl_idx, hub, port)?;
    if status2 & 0x0001 == 0 {
        // Device dropped off the bus during reset (seen on W540 RMH
        // port 6). Give it a long debounce and one more try — some
        // devices disconnect/reconnect on their first reset.
        println!("[ehci] hub addr={} port {}: device vanished during reset (status=0x{:04X}) — re-checking",
            hub.addr, port, status2);
        sleep_ms(300);
        match hub_get_port_status(ctl_idx, hub, port) {
            Some((s, _)) if s & 0x0001 != 0 => {
                let _ = hub_clear_port_feature(ctl_idx, hub, port, hub_feature::C_PORT_CONNECTION);
                if hub_set_port_feature(ctl_idx, hub, port, hub_feature::PORT_RESET) {
                    let mut ok = false;
                    for _ in 0..50 {
                        sleep_ms(20);
                        if let Some((_s, c)) = hub_get_port_status(ctl_idx, hub, port) {
                            if c & 0x0010 != 0 { ok = true; break; }
                        } else { break; }
                    }
                    if ok {
                        let _ = hub_clear_port_feature(ctl_idx, hub, port, hub_feature::C_PORT_RESET);
                        sleep_ms(30);
                        if let Some((s2, _)) = hub_get_port_status(ctl_idx, hub, port) {
                            status2 = s2;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if status2 & 0x0002 == 0 {
        println!("[ehci] hub addr={} port {}: not enabled after reset (status=0x{:04X})",
            hub.addr, port, status2);
        return None;
    }
    let child_eps = if status2 & 0x0400 != 0 {
        EPS_HS
    } else if status2 & 0x0200 != 0 {
        EPS_LS
    } else {
        EPS_FS
    };
    Some(child_eps)
}

/// iPhone (ipheth) bring-up over EHCI: SET_CONFIGURATION, SET_INTERFACE
/// alt=1, GET_MACADDR, CARRIER_CHECK. Bulk data path comes later — this
/// proves enumeration + control protocol, which is exactly the part the
/// W540 has never reached.
fn try_ipheth(ctl_idx: usize, dev: &DevCtx, cfg_blob: &[u8], cfg_value: u8) -> bool {
    let (iface, in_ep, in_mps, out_ep, out_mps) = match iphone::find_ipheth_interface(cfg_blob) {
        Some(t) => t,
        None => {
            println!("[ehci-ipheth] Apple device but no ipheth interface (is Personal Hotspot ON?)");
            iphone::dump_config_interfaces(cfg_blob);
            return false;
        }
    };
    println!("[ehci-ipheth] ipheth iface={} IN=0x{:02X}({}) OUT=0x{:02X}({})",
        iface, in_ep, in_mps, out_ep, out_mps);

    if control(ctl_idx, dev, 0x00, 9 /*SET_CONFIGURATION*/, cfg_value as u16, 0, None).is_none() {
        println!("[ehci-ipheth] SET_CONFIGURATION({}) failed", cfg_value);
        return false;
    }
    if control(ctl_idx, dev, 0x01, 11 /*SET_INTERFACE*/, iphone::IPHETH_ALT as u16, iface as u16, None).is_none() {
        println!("[ehci-ipheth] SET_INTERFACE(alt=1) failed");
        return false;
    }
    // The "Trust This Computer?" prompt fires on the first vendor
    // control transfer. GET_MACADDR may fail until the user taps Trust.
    let mut mac = [0u8; 6];
    match control(ctl_idx, dev, 0xC0, iphone::IPHETH_CMD_GET_MACADDR, 0, iface as u16, Some(&mut mac)) {
        Some(6) => {
            println!("[ehci-ipheth] MAC {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
        }
        other => {
            println!("[ehci-ipheth] GET_MACADDR returned {:?} — tap Trust on the iPhone and re-run usbenum", other);
            return false;
        }
    }
    let mut carrier = [0u8; 2];
    match control(ctl_idx, dev, 0xC0, iphone::IPHETH_CMD_CARRIER_CHECK, 0, iface as u16, Some(&mut carrier[..1])) {
        Some(n) if n >= 1 => {
            let on = carrier[0] == iphone::IPHETH_CARRIER_ON;
            println!("[ehci-ipheth] carrier byte=0x{:02X} ({})", carrier[0],
                if on { "ON — hotspot active" } else { "off" });
        }
        _ => println!("[ehci-ipheth] CARRIER_CHECK failed (non-fatal)"),
    }
    iphone::stash(iphone::IphoneDevice {
        slot_id: dev.addr, // EHCI device address; not an xHCI slot
        ipheth_iface: iface,
        ipheth_in_ep: in_ep,
        ipheth_out_ep: out_ep,
        ipheth_in_dci: 0,
        ipheth_out_dci: 0,
        ipheth_in_mps: in_mps,
        ipheth_out_mps: out_mps,
        config_value: cfg_value,
        mac,
        confirmed_pairing: false,
    });
    // Bring up the bulk data path (persistent RX QH + first receive).
    ipheth_net_setup(ctl_idx, dev, in_ep, out_ep, in_mps, out_mps);
    true
}

/// Enumerate one freshly reset device sitting at USB address 0.
/// Sequential by construction — only one device is ever at address 0.
fn enumerate_device(ctl_idx: usize, eps: u32, hub_addr: u8, hub_port: u8, depth: u8) -> bool {
    // Address-0 context. HS control MPS is always 64; LS is 8; FS we
    // start at 8 (legal floor) and learn the real value from byte 7.
    let mut dev = DevCtx {
        addr: 0,
        mps0: match eps { EPS_HS => 64, EPS_LS => 8, _ => 8 },
        eps,
        hub_addr,
        hub_port,
    };

    // First contact is the flakiest transfer on real hardware (Linux
    // retries it 3x with a fresh port reset in between; we retry with a
    // settle delay, which has proven enough for the RMH).
    let mut first8 = [0u8; 8];
    let mut got_first8 = false;
    for attempt in 0..3 {
        match control(ctl_idx, &dev, 0x80, 6, 0x0100, 0, Some(&mut first8)) {
            Some(n) if n >= 8 => { got_first8 = true; break; }
            other => {
                if VERBOSE_ENUM {
                    println!("[ehci] GET_DESCRIPTOR(8) at addr 0 attempt {} failed ({:?})", attempt, other);
                }
                sleep_ms(50);
            }
        }
    }
    if !got_first8 {
        println!("[ehci] GET_DESCRIPTOR(8) at addr 0 failed after 3 attempts — giving up on this port");
        return false;
    }
    dev.mps0 = first8[7] as u16;

    // Assign an address.
    let new_addr = unsafe {
        match CTLS[ctl_idx].as_mut() {
            Some(c) => { let a = c.next_addr; c.next_addr += 1; a }
            None => return false,
        }
    };
    if control(ctl_idx, &dev, 0x00, 5 /*SET_ADDRESS*/, new_addr as u16, 0, None).is_none() {
        println!("[ehci] SET_ADDRESS({}) failed", new_addr);
        return false;
    }
    dev.addr = new_addr;
    sleep_ms(20); // §9.2.6.3 allows the device 2 ms; be generous

    let mut dd = [0u8; 18];
    match control(ctl_idx, &dev, 0x80, 6, 0x0100, 0, Some(&mut dd)) {
        Some(n) if n >= 18 => {}
        other => {
            println!("[ehci] full device descriptor failed ({:?})", other);
            return false;
        }
    }
    let vendor = u16::from_le_bytes([dd[8], dd[9]]);
    let product = u16::from_le_bytes([dd[10], dd[11]]);
    let class = dd[4];
    let speed_name = match eps { EPS_HS => "HS", EPS_LS => "LS", _ => "FS" };
    println!("[ehci] enumerated addr={} vendor=0x{:04X} product=0x{:04X} class=0x{:02X} speed={} mps0={}",
        new_addr, vendor, product, class, speed_name, dev.mps0);

    // Config descriptor: header first for wTotalLength, then the blob.
    let mut cfg9 = [0u8; 9];
    match control(ctl_idx, &dev, 0x80, 6, 0x0200, 0, Some(&mut cfg9)) {
        Some(n) if n >= 9 => {}
        other => {
            println!("[ehci] config header failed ({:?})", other);
            return false;
        }
    }
    let total = u16::from_le_bytes([cfg9[2], cfg9[3]]) as usize;
    let cfg_value = cfg9[5];
    let mut blob = [0u8; 1024];
    let want = total.min(blob.len());
    let got = match control(ctl_idx, &dev, 0x80, 6, 0x0200, 0, Some(&mut blob[..want])) {
        Some(n) => n,
        None => {
            println!("[ehci] full config read failed");
            return false;
        }
    };
    let blob = &blob[..got];

    record_device(EhciDevice {
        ctl: ctl_idx as u8,
        addr: new_addr,
        mps0: dev.mps0,
        vendor, product, class, eps,
        hub_addr, hub_port,
        is_iphone: vendor == iphone::APPLE_VENDOR_ID,
    });

    if class == 0x09 {
        // A hub (the RMH, or something the user plugged in).
        if vendor == 0x8087 {
            println!("[ehci] Intel Rate Matching Hub — physical jacks are its downstream ports");
        }
        if control(ctl_idx, &dev, 0x00, 9 /*SET_CONFIGURATION*/, cfg_value as u16, 0, None).is_none() {
            println!("[ehci] hub SET_CONFIGURATION failed");
            return false;
        }
        let children = bring_up_hub(ctl_idx, &dev, depth + 1);
        println!("[ehci] hub addr={}: {} children enumerated", new_addr, children);
        return true;
    }

    if vendor == iphone::APPLE_VENDOR_ID {
        // iPhones hide the tether interface in a HIGHER configuration:
        // config 1 is PTP-only; with Personal Hotspot on, the ipheth
        // interface (0xFF/0xFD/0x01) appears in the last configuration
        // (Linux/usbmuxd switch configs to reach it). Iterate every
        // configuration, not just the one already in `blob`. Mirrors
        // the xHCI-side lesson from commit 4dd07c2.
        let num_configs = dd[17].max(1);
        println!("[ehci] *** APPLE DEVICE on EHCI — {} configuration(s), searching for ipheth ***",
            num_configs);
        for cfg_idx in 0..num_configs {
            let mut h9 = [0u8; 9];
            match control(ctl_idx, &dev, 0x80, 6, 0x0200 | cfg_idx as u16, 0, Some(&mut h9)) {
                Some(n) if n >= 9 => {}
                other => {
                    println!("[ehci-ipheth] config[{}] header failed ({:?})", cfg_idx, other);
                    continue;
                }
            }
            let c_total = u16::from_le_bytes([h9[2], h9[3]]) as usize;
            let c_value = h9[5];
            // iPhone tether configs (PTP + usbmux + ipheth alts) run
            // larger than typical; 2 KiB covers them comfortably.
            let mut c_blob = [0u8; 2048];
            let c_want = c_total.min(c_blob.len());
            let c_got = match control(ctl_idx, &dev, 0x80, 6, 0x0200 | cfg_idx as u16, 0, Some(&mut c_blob[..c_want])) {
                Some(n) => n,
                None => {
                    println!("[ehci-ipheth] config[{}] full read failed", cfg_idx);
                    continue;
                }
            };
            let c_blob = &c_blob[..c_got];
            if iphone::find_ipheth_interface(c_blob).is_some() {
                println!("[ehci-ipheth] ipheth found in config[{}] (bConfigurationValue={})",
                    cfg_idx, c_value);
                return try_ipheth(ctl_idx, &dev, c_blob, c_value);
            }
            println!("[ehci-ipheth] config[{}] value={} ({} bytes): no ipheth; interfaces:",
                cfg_idx, c_value, c_got);
            iphone::dump_config_interfaces(c_blob);
        }
        println!("[ehci-ipheth] no ipheth in ANY config — hotspot truly off, or iOS needs usbmux pairing first");
        return true; // enumeration itself succeeded
    }

    // Anything else: enumeration itself is the win (this is the path
    // the W540 could never complete through xHCI). Log the interfaces
    // so usbinfo readers can see what's there — verbose only, since a
    // dock full of devices floods the boot log otherwise.
    if VERBOSE_ENUM {
        iphone::dump_config_interfaces(blob);
    }
    true
}

/// Full scan: tear down ipheth, clear the device table + addresses, and
/// re-reset + enumerate every connected root port on every controller.
/// Returns total devices enumerated (hubs count, their children too).
/// Used at boot from `init_and_enumerate` and as the incremental path's
/// conservative fallback.
pub fn enumerate_all_full() -> usize {
    // Port resets invalidate device addresses — the live ipheth RX QH
    // must be unlinked first or the HC keeps polling a stale address.
    ipheth_net_teardown();
    // Re-runs (usbenum hot-plug debugging) reset every port anyway, so
    // start from a clean table + fresh addresses each time.
    unsafe {
        for d in DEVICES.iter_mut() { *d = None; }
        for c in CTLS.iter_mut().flatten() { c.next_addr = 1; }
    }
    let mut total = 0usize;
    for ctl_idx in 0..MAX_CTLS {
        let ctl = match unsafe { CTLS[ctl_idx] } {
            Some(c) => c,
            None => continue,
        };
        // Power up ports first (PPC controllers gate VBUS until told).
        if ctl.ppc {
            for port in 1..=ctl.n_ports {
                let addr = ctl.op_base + op::PORTSC_BASE + ((port - 1) as u64) * 4;
                let sc = unsafe { read_u32(addr) };
                if sc & portsc::PP == 0 {
                    unsafe { write_u32(addr, (sc & !portsc::RW1C) | portsc::PP); }
                }
            }
            sleep_ms(50);
        }
        for port in 1..=ctl.n_ports {
            let addr = ctl.op_base + op::PORTSC_BASE + ((port - 1) as u64) * 4;
            let sc = unsafe { read_u32(addr) };
            // Clear stale change bits so usbinfo output stays readable.
            if sc & portsc::RW1C != 0 {
                unsafe { write_u32(addr, sc); }
            }
            if sc & portsc::CCS == 0 { continue; }
            println!("[ehci] ctl{} root port {}: connected (PORTSC=0x{:08X})", ctl_idx, port, sc);
            if let Some(eps) = reset_root_port(&ctl, port) {
                // Record the root port in `hub_port` (hub_addr stays 0, so
                // split-transaction routing + usbinfo "behind hub" logic
                // are unaffected) — lets the incremental path match a
                // recorded device to its root port.
                if enumerate_device(ctl_idx, eps, 0, port, 0) {
                    total += 1;
                }
            }
        }
    }
    total
}

/// Backwards-compatible alias for the full scan.
pub fn enumerate_all() -> usize {
    enumerate_all_full()
}

// ----------------------------------------------------------------------
// Incremental enumeration (hot-plug). Conservative: any uncertainty
// falls back to a full scan. Correctness over speed.
// ----------------------------------------------------------------------

/// Is the device table empty (nothing enumerated yet)?
fn devices_empty() -> bool {
    unsafe { DEVICES.iter().all(|d| d.is_none()) }
}

/// Does a root-level device exist for this (controller, root port)?
/// Root devices are recorded with `hub_addr == 0` and `hub_port == port`.
fn root_device_present(ctl_idx: usize, port: u8) -> bool {
    unsafe {
        DEVICES.iter().flatten().any(|d|
            d.ctl as usize == ctl_idx && d.hub_addr == 0 && d.hub_port == port)
    }
}

/// True if the live ipheth device lives on this controller.
fn ipheth_on_ctl(ctl_idx: usize) -> bool {
    match unsafe { IPHETH_LINK } {
        Some(l) => l.ctl_idx == ctl_idx,
        None => false,
    }
}

/// Remove the device recorded at (ctl, addr) and, recursively, every
/// device sitting behind it (matching `hub_addr == addr`). If any removed
/// device was the live iPhone, tear down its data path.
fn remove_subtree(ctl_idx: usize, addr: u8) {
    // Collect child hub addresses first (children of `addr`), then recurse.
    let mut child_addrs: [u8; MAX_DEVICES] = [0; MAX_DEVICES];
    let mut n_children = 0usize;
    unsafe {
        for d in DEVICES.iter().flatten() {
            if d.ctl as usize == ctl_idx && d.hub_addr == addr {
                child_addrs[n_children] = d.addr;
                n_children += 1;
            }
        }
    }
    for i in 0..n_children {
        remove_subtree(ctl_idx, child_addrs[i]);
    }
    unsafe {
        for slot in DEVICES.iter_mut() {
            if let Some(d) = slot {
                if d.ctl as usize == ctl_idx && d.addr == addr {
                    if d.is_iphone && ipheth_on_ctl(ctl_idx) {
                        ipheth_net_teardown();
                    }
                    *slot = None;
                }
            }
        }
    }
}

/// Snapshot the recorded hub devices (class 0x09) into a fixed array so we
/// can iterate them without holding a borrow on DEVICES across the control
/// transfers that re-scan each hub's ports.
fn collect_hubs() -> ([Option<EhciDevice>; MAX_DEVICES], usize) {
    let mut out: [Option<EhciDevice>; MAX_DEVICES] = [None; MAX_DEVICES];
    let mut n = 0usize;
    unsafe {
        for d in DEVICES.iter().flatten() {
            if d.class == 0x09 {
                out[n] = Some(*d);
                n += 1;
            }
        }
    }
    (out, n)
}

/// Incremental hot-plug enumeration: re-scan only what changed since the
/// last enumeration, leaving untouched devices (and the live ipheth data
/// path) alone. Falls back to a full scan whenever the picture is unclear.
/// Returns the number of devices newly enumerated this pass.
pub fn enumerate_incremental() -> usize {
    // Nothing recorded yet (cold boot, or a prior teardown wiped it) →
    // there's nothing to do incrementally; do the full bring-up.
    if devices_empty() {
        return enumerate_all_full();
    }

    let mut total = 0usize;

    // -- Root ports -----------------------------------------------------
    for ctl_idx in 0..MAX_CTLS {
        let ctl = match unsafe { CTLS[ctl_idx] } {
            Some(c) => c,
            None => continue,
        };
        for port in 1..=ctl.n_ports {
            let addr = ctl.op_base + op::PORTSC_BASE + ((port - 1) as u64) * 4;
            let sc = unsafe { read_u32(addr) };
            let csc = sc & portsc::CSC != 0;
            let connected = sc & portsc::CCS != 0;
            let known = root_device_present(ctl_idx, port);
            // Re-enumerate when the connect-change latched, or something is
            // plugged in that we have no record of on this root port.
            if csc || (connected && !known) {
                // Clear the latched change bits (W1C) so we don't loop on
                // a stale CSC next pass.
                if sc & portsc::RW1C != 0 {
                    unsafe { write_u32(addr, sc); }
                }
                if !connected {
                    // Disconnect on the root port: drop whatever lived here.
                    // (Root devices carry hub_addr==0/hub_port==port.)
                    let victim = unsafe {
                        DEVICES.iter().flatten()
                            .find(|d| d.ctl as usize == ctl_idx
                                && d.hub_addr == 0 && d.hub_port == port)
                            .map(|d| d.addr)
                    };
                    if let Some(a) = victim {
                        println!("[ehci] ctl{} root port {}: disconnected — removing addr={}", ctl_idx, port, a);
                        remove_subtree(ctl_idx, a);
                    }
                    continue;
                }
                println!("[ehci] ctl{} root port {}: change/new (PORTSC=0x{:08X}) — re-enumerating", ctl_idx, port, sc);
                // A port reset invalidates addresses on this controller's
                // bus — if the ipheth device lives here, tear it down first.
                if ipheth_on_ctl(ctl_idx) {
                    ipheth_net_teardown();
                }
                // Drop any stale record for this root port before re-adding.
                let stale = unsafe {
                    DEVICES.iter().flatten()
                        .find(|d| d.ctl as usize == ctl_idx
                            && d.hub_addr == 0 && d.hub_port == port)
                        .map(|d| d.addr)
                };
                if let Some(a) = stale {
                    remove_subtree(ctl_idx, a);
                }
                if let Some(eps) = reset_root_port(&ctl, port) {
                    if enumerate_device(ctl_idx, eps, 0, port, 0) {
                        total += 1;
                    }
                }
            }
        }
    }

    // -- Recorded hubs --------------------------------------------------
    // Re-scan each known hub's downstream ports for connect-change.
    let (hubs, n_hubs) = collect_hubs();
    for hub_dev in hubs.iter().flatten().take(n_hubs) {
        let ctl_idx = hub_dev.ctl as usize;
        let hub = DevCtx {
            addr: hub_dev.addr,
            mps0: hub_dev.mps0,
            eps: hub_dev.eps,
            hub_addr: hub_dev.hub_addr,
            hub_port: hub_dev.hub_port,
        };
        // Re-read the hub descriptor for its port count (one cheap control
        // transfer). If it fails the hub may be gone — leave it for a
        // future full scan rather than guess.
        let mut desc = [0u8; 16];
        let n_ports = match control(ctl_idx, &hub, 0xA0, 6 /*GET_DESCRIPTOR*/, 0x2900, 0, Some(&mut desc)) {
            Some(n) if n >= 5 => desc[2],
            _ => {
                println!("[ehci] hub addr={}: descriptor re-read failed in incremental scan", hub.addr);
                continue;
            }
        };
        // The hub's depth = its own depth. Children enumerate at that depth
        // (bring_up_hub_port / enumerate_device → bring_up_hub adds +1).
        // We don't track per-device depth, so derive it conservatively:
        // root-attached hub (hub_addr==0) is depth 1, else depth 2. The
        // descent guard (>3) still protects against runaway recursion.
        let hub_depth: u8 = if hub.hub_addr == 0 { 1 } else { 2 };
        for port in 1..=n_ports {
            let (status, change) = match hub_get_port_status(ctl_idx, &hub, port) {
                Some(s) => s,
                None => continue,
            };
            let changed = change & 0x0001 != 0; // C_PORT_CONNECTION
            let now_connected = status & 0x0001 != 0;
            // Find any recorded child on this (hub, port).
            let existing = unsafe {
                DEVICES.iter().flatten()
                    .find(|d| d.ctl as usize == ctl_idx
                        && d.hub_addr == hub.addr && d.hub_port == port)
                    .map(|d| d.addr)
            };
            // React to a latched connect-change, OR to a connected port
            // with no recorded device — the latter recovers devices whose
            // earlier enumeration failed after the change bit was already
            // consumed (same rule the root-port scan applies).
            if !changed && !(now_connected && existing.is_none()) {
                continue;
            }
            if changed {
                let _ = hub_clear_port_feature(ctl_idx, &hub, port, hub_feature::C_PORT_CONNECTION);
            }
            if now_connected {
                println!("[ehci] hub addr={} port {}: connect-change/new, now connected — enumerating", hub.addr, port);
                // Replace any stale record first.
                if let Some(a) = existing {
                    remove_subtree(ctl_idx, a);
                }
                if bring_up_hub_port(ctl_idx, &hub, port, hub_depth) {
                    total += 1;
                }
            } else if let Some(a) = existing {
                println!("[ehci] hub addr={} port {}: disconnected — removing addr={}", hub.addr, port, a);
                remove_subtree(ctl_idx, a);
            }
        }
    }

    total
}

/// One-shot entry point: bring up controllers + enumerate everything.
pub fn init_and_enumerate() -> usize {
    if init() == 0 {
        return 0;
    }
    let n = enumerate_all();
    println!("[ehci] enumeration complete: {} device tree(s)", n);
    n
}

// ============================================================================
// ipheth data path — persistent bulk-IN RX + one-shot bulk-OUT TX
// ============================================================================
//
// The RX QH lives permanently in the async schedule (head → rx → head;
// one-shot transfers insert themselves between head and rx without
// disturbing it). One receive is outstanding at a time: a single qTD
// with a 2 KiB buffer; the HC retires it on the frame's short packet,
// `ipheth_recv_frame` harvests + re-arms. Data toggles are tracked in
// software (DTC=1), same scheme as `bulk()`.
//
// Legacy ipheth framing: each retired transfer is one Ethernet frame
// prefixed with 2 alignment bytes (IPHETH_IP_ALIGN). We never send
// ENABLE_NCM, so the iPhone stays in legacy framing.

#[repr(C, align(4096))]
struct IphethPage {
    rx_qh: Qh,
    rx_qtd: Qtd,
    rx_buf: [u8; 2048],
}
static mut IPHETH_PAGE: IphethPage = IphethPage {
    rx_qh: Qh([0; 32]),
    rx_qtd: Qtd([0; 16]),
    rx_buf: [0; 2048],
};

#[derive(Copy, Clone)]
struct IphethLink {
    ctl_idx: usize,
    dev: DevCtx,
    in_ep: u8,
    out_ep: u8,
    in_mps: u16,
    out_mps: u16,
    rx_qh_phys: u32,
    rx_qtd_phys: u32,
    rx_buf_phys: u32,
}
static mut IPHETH_LINK: Option<IphethLink> = None;
static mut IPHETH_RX_TOGGLE: bool = false;
static mut IPHETH_TX_TOGGLE: bool = false;
/// Rate-limit RX error logging (a noisy phone would flood serial).
static mut IPHETH_RX_ERRS: u32 = 0;

/// Arm (or re-arm) the single outstanding bulk-IN receive.
fn ipheth_arm_rx() {
    let l = match unsafe { IPHETH_LINK } { Some(l) => l, None => return };
    unsafe {
        let q = &raw mut IPHETH_PAGE.rx_qtd.0;
        (*q)[0] = LINK_T;
        (*q)[1] = LINK_T;
        let dt = if IPHETH_RX_TOGGLE { token::DT } else { 0 };
        (*q)[2] = token::ACTIVE | token::CERR_3 | token::PID_IN | token::IOC | dt
            | ((2048u32) << token::TOTAL_SHIFT);
        (*q)[3] = l.rx_buf_phys;
        (*q)[4] = (l.rx_buf_phys & !0xFFF).wrapping_add(0x1000);
        (*q)[5] = 0; (*q)[6] = 0; (*q)[7] = 0;
        fence(Ordering::SeqCst);
        // Hand it to the (idle) RX QH via the overlay: next-qTD pointer
        // first, then a zeroed token (Active=0, Halted=0) which makes
        // the HC advance to and fetch our qTD.
        let h = &raw mut IPHETH_PAGE.rx_qh.0;
        write_volatile(&raw mut (*h)[4], l.rx_qtd_phys);
        write_volatile(&raw mut (*h)[5], LINK_T);
        fence(Ordering::SeqCst);
        write_volatile(&raw mut (*h)[6], 0);
        fence(Ordering::SeqCst);
    }
}

/// Called from `try_ipheth` after GET_MACADDR succeeds: build the RX QH,
/// splice it into this controller's async schedule, and arm the first
/// receive. Idempotent via teardown-first.
fn ipheth_net_setup(ctl_idx: usize, dev: &DevCtx, in_ep: u8, out_ep: u8, in_mps: u16, out_mps: u16) -> bool {
    ipheth_net_teardown();
    let ctl = match unsafe { CTLS[ctl_idx] } { Some(c) => c, None => return false };
    let rx_qh_phys = match phys32_of(unsafe { &raw const IPHETH_PAGE.rx_qh } as u64) { Some(p) => p, None => return false };
    let rx_qtd_phys = match phys32_of(unsafe { &raw const IPHETH_PAGE.rx_qtd } as u64) { Some(p) => p, None => return false };
    let rx_buf_phys = match phys32_of(unsafe { &raw const IPHETH_PAGE.rx_buf } as u64) { Some(p) => p, None => return false };

    unsafe {
        let h = &raw mut IPHETH_PAGE.rx_qh.0;
        for dw in (*h).iter_mut() { *dw = 0; }
        (*h)[0] = ctl.head_phys | LINK_TYP_QH; // close ring back to head
        (*h)[1] = qh_dw1(dev, in_ep & 0x0F, in_mps);
        (*h)[2] = qh_dw2(dev);
        (*h)[4] = LINK_T;
        (*h)[5] = LINK_T;
        (*h)[6] = token::HALTED; // idle until armed
        fence(Ordering::SeqCst);
        // Splice in: head → rx (rx already points back at head).
        write_volatile(&raw mut ASYNCPS[ctl.page].head.0[0], rx_qh_phys | LINK_TYP_QH);
        fence(Ordering::SeqCst);

        IPHETH_LINK = Some(IphethLink {
            ctl_idx, dev: *dev, in_ep, out_ep, in_mps, out_mps,
            rx_qh_phys, rx_qtd_phys, rx_buf_phys,
        });
        IPHETH_RX_TOGGLE = false;
        IPHETH_TX_TOGGLE = false;
        IPHETH_RX_ERRS = 0;
    }
    ipheth_arm_rx();
    println!("[ehci-ipheth] RX armed: IN=0x{:02X} OUT=0x{:02X} (bulk data path live)", in_ep, out_ep);
    true
}

/// Unlink the RX QH and forget the device. Must run before any
/// re-enumeration (port resets change addresses, invalidating the QH).
pub fn ipheth_net_teardown() {
    let l = match unsafe { IPHETH_LINK } { Some(l) => l, None => return };
    if let Some(ctl) = unsafe { CTLS[l.ctl_idx] } {
        unsafe {
            // Restore head's self-link, then IAA so the HC drops any
            // cached pointer to the RX QH before we forget it.
            write_volatile(&raw mut ASYNCPS[ctl.page].head.0[0], ctl.head_phys | LINK_TYP_QH);
            fence(Ordering::SeqCst);
            let cmd = read_u32(ctl.op_base + op::USBCMD);
            write_u32(ctl.op_base + op::USBCMD, cmd | usbcmd::IAAD);
            for _ in 0..1_000_000u32 {
                let sts = read_u32(ctl.op_base + op::USBSTS);
                if sts & usbsts::IAA != 0 {
                    write_u32(ctl.op_base + op::USBSTS, usbsts::IAA);
                    break;
                }
                core::hint::spin_loop();
            }
        }
    }
    unsafe { IPHETH_LINK = None; }
}

/// True once the data path is set up (NetDevice link_up signal).
pub fn ipheth_active() -> bool {
    unsafe { IPHETH_LINK.is_some() }
}

/// Non-consuming check: has the outstanding receive retired?
pub fn ipheth_has_frame() -> bool {
    if unsafe { IPHETH_LINK.is_none() } { return false; }
    let tok = unsafe { read_volatile(&raw const IPHETH_PAGE.rx_qtd.0[2]) };
    tok & token::ACTIVE == 0
}

/// Harvest a received Ethernet frame (alignment padding stripped) into
/// `buf`. Returns 0 if nothing has arrived. Always re-arms.
pub fn ipheth_recv_frame(buf: &mut [u8]) -> usize {
    if unsafe { IPHETH_LINK.is_none() } { return 0; }
    let tok = unsafe { read_volatile(&raw const IPHETH_PAGE.rx_qtd.0[2]) };
    if tok & token::ACTIVE != 0 {
        return 0;
    }
    let mut got = 0usize;
    if tok & token::ERR_MASK == 0 {
        let residual = ((tok & token::TOTAL_MASK) >> token::TOTAL_SHIFT) as usize;
        let n = 2048usize.saturating_sub(residual);
        if n > iphone::IPHETH_IP_ALIGN {
            let payload = n - iphone::IPHETH_IP_ALIGN;
            let copy = payload.min(buf.len());
            unsafe {
                buf[..copy].copy_from_slice(
                    &IPHETH_PAGE.rx_buf[iphone::IPHETH_IP_ALIGN..iphone::IPHETH_IP_ALIGN + copy]);
            }
            got = copy;
            iphone::mark_paired(); // first frame = pairing confirmed
        }
    } else {
        unsafe {
            IPHETH_RX_ERRS += 1;
            if IPHETH_RX_ERRS <= 3 || IPHETH_RX_ERRS.is_power_of_two() {
                println!("[ehci-ipheth] RX error #{} tok=0x{:08X} ({})",
                    IPHETH_RX_ERRS, tok, err_token_name(tok));
            }
        }
    }
    unsafe { IPHETH_RX_TOGGLE = tok & token::DT != 0; }
    ipheth_arm_rx();
    got
}

/// Transmit one Ethernet frame (legacy framing: raw frame, no padding).
pub fn ipheth_send_frame(frame: &[u8]) -> bool {
    let l = match unsafe { IPHETH_LINK } { Some(l) => l, None => return false };
    if frame.is_empty() || frame.len() > 1600 { return false; }
    let mut tmp = [0u8; 1600];
    tmp[..frame.len()].copy_from_slice(frame);
    let mut toggle = unsafe { IPHETH_TX_TOGGLE };
    let ok = bulk(
        l.ctl_idx, &l.dev, l.out_ep & 0x0F, l.out_mps,
        false, &mut toggle, &mut tmp, frame.len(),
    ).is_some();
    unsafe { IPHETH_TX_TOGGLE = toggle; }
    ok
}

/// usbinfo support: print every device enumerated through EHCI.
pub fn print_devices() {
    let mut any = false;
    println!("usbinfo: EHCI-enumerated devices:");
    unsafe {
        for d in DEVICES.iter().flatten() {
            any = true;
            let speed_name = match d.eps { EPS_HS => "HS", EPS_LS => "LS", _ => "FS" };
            let class_name = match d.class {
                0x09 => "hub",
                0x08 => "MSC",
                0x03 => "HID",
                0x00 => "per-interface",
                _ => "other",
            };
            if d.hub_addr != 0 {
                println!("  ctl{} addr={} vendor=0x{:04X} product=0x{:04X} class={} speed={} — behind hub addr={} port={}{}",
                    d.ctl, d.addr, d.vendor, d.product, class_name, speed_name,
                    d.hub_addr, d.hub_port,
                    if d.is_iphone { "  [APPLE]" } else { "" });
            } else {
                println!("  ctl{} addr={} vendor=0x{:04X} product=0x{:04X} class={} speed={} — root port{}",
                    d.ctl, d.addr, d.vendor, d.product, class_name, speed_name,
                    if d.is_iphone { "  [APPLE]" } else { "" });
            }
        }
    }
    if !any {
        println!("  (none)");
    }
}
