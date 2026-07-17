//! Minimal polled Intel e1000e Ethernet driver for SemOS.
//!
//! Targets the Lenovo T540p on-board Intel I217-LM (PCI 00:19.0,
//! vendor/device 0x8086/0x153a). Uses MMIO BAR0 and polled RX/TX; no
//! interrupts, no MSI/MSI-X, no checksum offloads, no VLAN handling.
//!
//! The register layout follows the standard Intel e1000/e1000e legacy
//! PCI-Express MAC. Register offsets and reset sequence are derived from
//! the Linux `e1000e` driver and the oracle capture in
//! `docs/hardware/e1000e-2026-07-08/`.
//!
//! # Architecture
//!
//! - Probe PCI for class 0x02/0x00/0x00 or vendor/device 0x8086/0x153a.
//! - Map BAR0 MMIO via `pci::mmio_bar64` + `paging::phys_to_virt`.
//! - Reset the MAC, read MAC address from RAL0/RAH0.
//! - Set up one legacy RX descriptor ring and one legacy TX descriptor ring.
//! - Implement `kernel_core::drivers::traits::NetDevice` and register as
//!   `"e1000e0"` so the existing smoltcp glue in `main.rs` can bring up
//!   the IP stack.
//!
//! # Limitations
//!
//! - Polled only; no interrupts means CPU spins on TX completion.
//! - No PHY setup beyond link-up polling. The BIOS/Linux e1000e leaves the
//!   PHY negotiated; this driver relies on that for the first metal boot.
//! - No EEPROM/NVM access after init; MAC is taken from the filter regs.
//! - No checksum/TSS/VLAN offloads.
//! - Single RX/TX queue.

use core::sync::atomic::{fence, Ordering};
use crate::pci;
use crate::paging;
use kernel_core::drivers::traits::{NetDevice, DriverError, DriverResult};

// ============================================================================
// PCI identification
// ============================================================================

const INTEL_VENDOR_ID: u16 = 0x8086;
const I217LM_DEVICE_ID: u16 = 0x153a;

// ============================================================================
// Register offsets (legacy e1000e MMIO layout)
// ============================================================================

mod reg {
    pub const CTRL:   u32 = 0x00000; // Device control
    pub const STATUS: u32 = 0x00008; // Device status
    pub const RCTL:   u32 = 0x00100; // Receive control
    pub const TCTL:   u32 = 0x00400; // Transmit control

    pub const RDBAL:  u32 = 0x02800; // RX desc base low
    pub const RDBAH:  u32 = 0x02804; // RX desc base high
    pub const RDLEN:  u32 = 0x02808; // RX desc length
    pub const RDH:    u32 = 0x02810; // RX desc head
    pub const RDT:    u32 = 0x02818; // RX desc tail
    pub const RDTR:   u32 = 0x02820; // RX delay timer

    pub const TDBAL:  u32 = 0x03800; // TX desc base low
    pub const TDBAH:  u32 = 0x03804; // TX desc base high
    pub const TDLEN:  u32 = 0x03808; // TX desc length
    pub const TDH:    u32 = 0x03810; // TX desc head
    pub const TDT:    u32 = 0x03818; // TX desc tail
    pub const TIDV:   u32 = 0x03820; // TX interrupt delay value

    pub const RAL0:   u32 = 0x05400; // Receive address low
    pub const RAH0:   u32 = 0x05404; // Receive address high

    pub const IMS:    u32 = 0x000D0; // Interrupt mask set
    pub const IMC:    u32 = 0x000D8; // Interrupt mask clear
}

// ============================================================================
// CTRL bits
// ============================================================================

mod ctrl {
    pub const RST:      u32 = 1 << 26; // Device reset
    pub const SLU:      u32 = 1 << 6;  // Set link up
    pub const FD:       u32 = 1 << 0;  // Full duplex
    pub const ADVD3WUC: u32 = 1 << 22;
    pub const SPEED_1G: u32 = 0b10 << 8; // SPEED field bits 9:8
}

// ============================================================================
// STATUS bits
// ============================================================================

mod status {
    pub const LU:   u32 = 1 << 1; // Link up
    pub const SPD:  u32 = 0b11 << 6; // Speed bits 7:6
    pub const FD:   u32 = 1 << 0; // Full duplex
}

// ============================================================================
// RCTL bits
// ============================================================================

mod rctl {
    pub const EN:     u32 = 1 << 1;  // Receiver enable
    pub const SBP:    u32 = 1 << 2;  // Store bad packets
    pub const UPE:    u32 = 1 << 3;  // Unicast promiscuous
    pub const MPE:    u32 = 1 << 4;  // Multicast promiscuous
    pub const LPE:    u32 = 1 << 5;  // Long packet enable
    pub const BAM:    u32 = 1 << 15; // Broadcast accept mode
    pub const SECRC:  u32 = 1 << 26; // Strip Ethernet CRC
    pub const BSIZE_2048: u32 = 0b00 << 16; // Buffer size 2048
}

// ============================================================================
// TCTL bits
// ============================================================================

mod tctl {
    pub const EN:     u32 = 1 << 1;  // Transmitter enable
    pub const PSP:    u32 = 1 << 3;  // Pad short packets
    pub const CT:     u32 = 0x0F << 4; // Collision threshold
    pub const COLD:   u32 = 0x3F << 12; // Collision distance
    pub const RTLC:   u32 = 1 << 24; // Re-transmit on late collision
}

// ============================================================================
// Descriptor format (legacy, 16 bytes each)
// ============================================================================

#[repr(C)]
#[derive(Copy, Clone)]
struct RxDesc {
    addr:  u64,
    len:   u16,
    cksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct TxDesc {
    addr:  u64,
    len:   u16,
    cso:   u8,
    cmd:   u8,
    status: u8,
    css:   u8,
    special: u16,
}

mod desc {
    pub const RX_DD: u8 = 1 << 0; // Descriptor done
    pub const RX_EOP: u8 = 1 << 1; // End of packet

    pub const TX_CMD_EOP: u8 = 1 << 0; // End of packet
    pub const TX_CMD_IFCS: u8 = 1 << 1; // Insert FCS
    pub const TX_CMD_RS: u8 = 1 << 3; // Report status
    pub const TX_DD: u8 = 1 << 0; // Descriptor done
}

// ============================================================================
// Ring sizing
// ============================================================================

const RING_SIZE: usize = 16;
const RX_BUFFER_SIZE: usize = 2048;
const TX_BUFFER_SIZE: usize = 2048;

// ============================================================================
// Static state
// ============================================================================

#[repr(C, align(4096))]
struct RxDescRing([RxDesc; RING_SIZE]);

#[repr(C, align(4096))]
struct TxDescRing([TxDesc; RING_SIZE]);

#[repr(C, align(4096))]
struct RxBuffers([[u8; RX_BUFFER_SIZE]; RING_SIZE]);

#[repr(C, align(4096))]
struct TxBuffer([u8; TX_BUFFER_SIZE]);

static mut RX_RING: RxDescRing = RxDescRing([RxDesc {
    addr: 0, len: 0, cksum: 0, status: 0, errors: 0, special: 0,
}; RING_SIZE]);
static mut TX_RING: TxDescRing = TxDescRing([TxDesc {
    addr: 0, len: 0, cso: 0, cmd: 0, status: 0, css: 0, special: 0,
}; RING_SIZE]);

static mut RX_BUFFERS: RxBuffers = RxBuffers([[0; RX_BUFFER_SIZE]; RING_SIZE]);
static mut TX_BUFFER: TxBuffer = TxBuffer([0; TX_BUFFER_SIZE]);

static mut RX_BUFFER_PHYS: [u64; RING_SIZE] = [0; RING_SIZE];
static mut TX_BUFFER_PHYS: u64 = 0;
static mut RX_RING_PHYS: u64 = 0;
static mut TX_RING_PHYS: u64 = 0;

static mut MMIO_BASE: u64 = 0;
static mut MAC: [u8; 6] = [0; 6];

// Per-ring indices. Software owns these; hardware owns the registers.
// For RX: hardware owns descriptors in [RDH, RDT] (modulo). We consume at
// RX_CLEAN and recycle at RX_FREE by advancing RDT.
static mut RX_CLEAN: usize = 0; // Next descriptor to check for received packet.
static mut RX_FREE: usize = 0;  // Last descriptor given back to hardware (write to RDT).

// For TX: we submit at TX_SUBMIT (write to TDT) and reclaim completed
// descriptors at TX_RECLAIM by checking descriptor status bits.
static mut TX_SUBMIT: usize = 0;   // Next free slot to submit into.
static mut TX_RECLAIM: usize = 0;  // Next descriptor to reclaim after TX done.

// ============================================================================
// MMIO helpers
// ============================================================================

#[inline]
unsafe fn rd32(offset: u32) -> u32 {
    core::ptr::read_volatile((MMIO_BASE + offset as u64) as *const u32)
}

#[inline]
unsafe fn wr32(offset: u32, value: u32) {
    core::ptr::write_volatile((MMIO_BASE + offset as u64) as *mut u32, value);
}

#[inline]
unsafe fn rd64(offset: u32) -> u64 {
    let low = rd32(offset);
    let high = rd32(offset + 4);
    ((high as u64) << 32) | (low as u64)
}

#[inline]
unsafe fn wr64(offset: u32, value: u64) {
    wr32(offset, value as u32);
    wr32(offset + 4, (value >> 32) as u32);
}

// ============================================================================
// Initialization
// ============================================================================

/// Probe for the Intel I217-LM and bring it up. Returns `true` if found and
/// initialized.
pub fn init() -> bool {
    // Prefer class scan so other e1000e-class NICs have a chance later.
    let loc = match pci::find_by_class(0x02, 0x00, 0x00) {
        Some(l) => {
            let (c, sc, pi) = pci::class_triple(l);
            let v = l.vendor_id();
            let d = l.device_id();
            if v != INTEL_VENDOR_ID || d != I217LM_DEVICE_ID {
                crate::println!(
                    "[e1000e] PCI class 02/00/00 device {:04X}:{:04X} is not I217-LM; skipping",
                    v, d
                );
                return false;
            }
            l
        }
        None => {
            // Fallback to explicit vendor/device scan.
            match pci::find_first(INTEL_VENDOR_ID, I217LM_DEVICE_ID) {
                Some(l) => l,
                None => {
                    crate::println!("[e1000e] no Intel I217-LM on PCI bus 0");
                    return false;
                }
            }
        }
    };

    loc.enable_io_and_bus_master();

    let phys_base = match pci::mmio_bar64(loc) {
        Some(b) => b,
        None => {
            crate::println!("[e1000e] BAR0 is I/O space; MMIO required — abort");
            return false;
        }
    };

    unsafe { MMIO_BASE = paging::phys_to_virt(phys_base); }

    crate::println!(
        "[e1000e] PCI 00:{:02X}.{}  MMIO=0x{:016X}  ven=0x{:04X} dev=0x{:04X}",
        loc.slot, loc.func, phys_base, loc.vendor_id(), loc.device_id()
    );

    if !reset() {
        crate::println!("[e1000e] MAC reset failed");
        return false;
    }

    read_mac();
    let mac = mac_address();
    crate::println!(
        "[e1000e] MAC {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    if !init_rings() {
        crate::println!("[e1000e] descriptor ring setup failed");
        return false;
    }

    enable_rx_tx();

    crate::println!("[e1000e] link {}  status=0x{:08X}",
        if link_up() { "UP" } else { "DOWN" },
        unsafe { rd32(reg::STATUS) }
    );

    true
}

/// Sleep at least `ms` milliseconds using the scheduler tick (62 Hz, so
/// resolution is ~16 ms; always rounds up by one tick). Ctrl+C cuts the
/// wait short so a real-hardware hang doesn't deadlock the whole boot
/// with no way out — same pattern as the EHCI/xHCI reset waits.
fn sleep_ms(ms: u64) {
    let ticks_needed = (ms * kernel_core::scheduler::SCHEDULER_TICK_HZ).div_ceil(1000) + 1;
    let end = kernel_core::platform::ticks() + ticks_needed;
    while kernel_core::platform::ticks() < end {
        if crate::keyboard::abort_requested() { return; }
        core::hint::spin_loop();
    }
}

/// Reset the MAC. Per the e1000e spec, write CTRL.RST and wait for it to
/// clear, then set SLU to take the PHY out of low-power state.
///
/// Real PCH-integrated silicon (unlike QEMU's instant-response emulated
/// e1000e) can leave the MMIO register interface unresponsive for a few
/// milliseconds while the reset pulse is in flight. A blocking MMIO load
/// issued into that window doesn't time out — the load instruction never
/// retires — so we must NOT re-read any register immediately after
/// writing RST. Sleep first (matches the Linux e1000e ich8lan/lpt reset
/// sequence), then poll with real per-iteration delays.
fn reset() -> bool {
    unsafe {
        // Disable RX/TX before reset.
        wr32(reg::RCTL, 0);
        wr32(reg::TCTL, 0);

        // Mask all interrupts.
        wr32(reg::IMC, 0xFFFF_FFFF);

        // Request reset.
        let mut ctrl = rd32(reg::CTRL);
        ctrl |= ctrl::RST;
        wr32(reg::CTRL, ctrl);

        // Let the reset pulse land before touching MMIO again — do NOT
        // read back CTRL immediately (see doc comment above).
        sleep_ms(20);

        // Now poll for RST to clear, with a bounded real-time deadline and
        // a sleep between reads rather than a tight spin.
        let deadline = kernel_core::platform::ticks() + kernel_core::scheduler::SCHEDULER_TICK_HZ; // ~1 s
        loop {
            if rd32(reg::CTRL) & ctrl::RST == 0 { break; }
            if kernel_core::platform::ticks() >= deadline {
                crate::println!("[e1000e] reset did not clear within 1s");
                return false;
            }
            if crate::keyboard::abort_requested() { return false; }
            sleep_ms(2);
        }

        // Additional settle delay for PHY/serdes.
        sleep_ms(5);

        // Bring the link up.
        ctrl = rd32(reg::CTRL);
        ctrl |= ctrl::SLU | ctrl::FD;
        // Force 1G selection bits; actual speed is PHY-negotiated.
        ctrl &= !(0b11 << 8);
        ctrl |= ctrl::SPEED_1G;
        wr32(reg::CTRL, ctrl);

        true
    }
}

/// Read the station address from receive-address register 0.
fn read_mac() {
    unsafe {
        let low = rd32(reg::RAL0);
        let high = rd32(reg::RAH0);
        MAC[0] = (low & 0xFF) as u8;
        MAC[1] = ((low >> 8) & 0xFF) as u8;
        MAC[2] = ((low >> 16) & 0xFF) as u8;
        MAC[3] = ((low >> 24) & 0xFF) as u8;
        MAC[4] = (high & 0xFF) as u8;
        MAC[5] = ((high >> 8) & 0xFF) as u8;
    }
}

/// Set up descriptor rings and their physical addresses.
fn init_rings() -> bool {
    let resolve = |virt: u64| -> Option<u64> { paging::walk_active_pml4(virt) };

    unsafe {
        // RX ring.
        let rx_ring_virt = (&raw const RX_RING) as u64;
        RX_RING_PHYS = match resolve(rx_ring_virt) {
            Some(p) if p & 0xFFF == 0 => p,
            _ => { crate::println!("[e1000e] RX ring phys translation failed"); return false; }
        };
        wr32(reg::RDBAL, RX_RING_PHYS as u32);
        wr32(reg::RDBAH, (RX_RING_PHYS >> 32) as u32);
        wr32(reg::RDLEN, (core::mem::size_of::<RxDescRing>()) as u32);
        wr32(reg::RDH, 0);
        wr32(reg::RDT, 0);

        // TX ring.
        let tx_ring_virt = (&raw const TX_RING) as u64;
        TX_RING_PHYS = match resolve(tx_ring_virt) {
            Some(p) if p & 0xFFF == 0 => p,
            _ => { crate::println!("[e1000e] TX ring phys translation failed"); return false; }
        };
        wr32(reg::TDBAL, TX_RING_PHYS as u32);
        wr32(reg::TDBAH, (TX_RING_PHYS >> 32) as u32);
        wr32(reg::TDLEN, (core::mem::size_of::<TxDescRing>()) as u32);
        wr32(reg::TDH, 0);
        wr32(reg::TDT, 0);

        // Resolve per-buffer physical addresses.
        let rx_base = (&raw const RX_BUFFERS) as u64;
        for i in 0..RING_SIZE {
            let v = rx_base + (i * RX_BUFFER_SIZE) as u64;
            let page_virt = v & !0xFFF;
            let page_off = v & 0xFFF;
            let phys = match resolve(page_virt) {
                Some(p) => p + page_off,
                None => { crate::println!("[e1000e] RX buf {} phys translation failed", i); return false; }
            };
            RX_BUFFER_PHYS[i] = phys;
            RX_RING.0[i].addr = phys;
            RX_RING.0[i].len = RX_BUFFER_SIZE as u16;
            RX_RING.0[i].status = 0;
        }

        let tx_v = (&raw const TX_BUFFER) as u64;
        TX_BUFFER_PHYS = match resolve(tx_v & !0xFFF) {
            Some(p) => p + (tx_v & 0xFFF),
            None => { crate::println!("[e1000e] TX buf phys translation failed"); return false; }
        };

        RX_CLEAN = 0;
        RX_FREE = RING_SIZE - 1;
        TX_SUBMIT = 0;
        TX_RECLAIM = 0;
    }

    true
}

/// Enable receiver and transmitter with conservative settings.
fn enable_rx_tx() {
    unsafe {
        // Set RDT to one less than RING_SIZE so hardware owns all descriptors.
        wr32(reg::RDH, 0);
        wr32(reg::RDT, (RING_SIZE - 1) as u32);

        let rctl = rctl::EN
                 | rctl::BAM
                 | rctl::BSIZE_2048
                 | rctl::SECRC;
        wr32(reg::RCTL, rctl);

        let tctl = tctl::EN
                 | tctl::PSP
                 | tctl::CT
                 | tctl::COLD
                 | tctl::RTLC;
        wr32(reg::TCTL, tctl);
    }
}

// ============================================================================
// Send / receive
// ============================================================================

/// Send one Ethernet frame. Returns `true` on success.
pub fn send_frame(frame: &[u8]) -> bool {
    if frame.len() > TX_BUFFER_SIZE { return false; }
    if unsafe { MMIO_BASE == 0 } { return false; }

    unsafe {
        // Reclaim any completed descriptors so TX_SUBMIT can advance.
        while TX_RECLAIM != TX_SUBMIT {
            let desc = &TX_RING.0[TX_RECLAIM];
            if desc.status & desc::TX_DD == 0 {
                break;
            }
            TX_RECLAIM = (TX_RECLAIM + 1) % RING_SIZE;
        }

        // If the ring is full, spin-wait for the oldest descriptor to complete.
        let next = (TX_SUBMIT + 1) % RING_SIZE;
        if next == TX_RECLAIM {
            let mut spins = 0u32;
            while TX_RING.0[TX_RECLAIM].status & desc::TX_DD == 0 {
                spins = spins.wrapping_add(1);
                if spins > 10_000_000 {
                    crate::println!("[e1000e] TX ring full, timeout waiting for completion");
                    return false;
                }
                core::hint::spin_loop();
            }
            TX_RECLAIM = (TX_RECLAIM + 1) % RING_SIZE;
        }

        // Copy frame into the contiguous TX buffer.
        let tx = (&raw mut TX_BUFFER) as *mut u8;
        core::ptr::copy_nonoverlapping(frame.as_ptr(), tx, frame.len());

        // Build legacy TX descriptor.
        let desc = &mut TX_RING.0[TX_SUBMIT];
        desc.addr = TX_BUFFER_PHYS;
        desc.len = frame.len() as u16;
        desc.cso = 0;
        desc.cmd = desc::TX_CMD_EOP | desc::TX_CMD_IFCS | desc::TX_CMD_RS;
        desc.status = 0;
        desc.css = 0;
        desc.special = 0;

        fence(Ordering::Release);

        // Advance tail to hand descriptor to hardware.
        TX_SUBMIT = (TX_SUBMIT + 1) % RING_SIZE;
        wr32(reg::TDT, TX_SUBMIT as u32);

        // Poll for completion of the descriptor we just submitted.
        let idx = (TX_SUBMIT + RING_SIZE - 1) % RING_SIZE;
        let mut spins = 0u32;
        while TX_RING.0[idx].status & desc::TX_DD == 0 {
            spins = spins.wrapping_add(1);
            if spins > 10_000_000 {
                crate::println!("[e1000e] TX timeout");
                return false;
            }
            core::hint::spin_loop();
        }

        fence(Ordering::Acquire);
    }

    true
}

/// Receive one Ethernet frame into `out`. Returns `Ok(len)` or `WouldBlock`.
pub fn recv_frame(out: &mut [u8]) -> DriverResult<usize> {
    if unsafe { MMIO_BASE == 0 } { return Err(DriverError::NotReady); }

    unsafe {
        // The descriptor at RX_CLEAN is the next one hardware may have filled.
        // If it equals RX_FREE, the ring is empty (hardware owns no buffers).
        if RX_CLEAN == RX_FREE {
            return Err(DriverError::WouldBlock);
        }

        let desc = &mut RX_RING.0[RX_CLEAN];
        if desc.status & desc::RX_DD == 0 {
            return Err(DriverError::WouldBlock);
        }

        let len = desc.len as usize;
        let copy_len = len.min(out.len());
        let src = (&raw const RX_BUFFERS) as *const u8;
        let buf_base = src.add(RX_CLEAN * RX_BUFFER_SIZE);
        core::ptr::copy_nonoverlapping(buf_base, out.as_mut_ptr(), copy_len);

        // Reclaim descriptor: clear status and advance clean pointer.
        desc.status = 0;
        RX_CLEAN = (RX_CLEAN + 1) % RING_SIZE;

        // Recycle descriptor: advance free pointer and update RDT.
        RX_FREE = (RX_FREE + 1) % RING_SIZE;
        wr32(reg::RDT, RX_FREE as u32);

        fence(Ordering::Release);

        Ok(copy_len)
    }
}

/// True if a received frame is waiting.
pub fn rx_has_data() -> bool {
    if unsafe { MMIO_BASE == 0 } { return false; }
    unsafe {
        if RX_CLEAN == RX_FREE { return false; }
        RX_RING.0[RX_CLEAN].status & desc::RX_DD != 0
    }
}

/// Return the cached MAC address.
pub fn mac_address() -> [u8; 6] {
    unsafe { MAC }
}

/// Poll the STATUS register for link up.
pub fn link_up() -> bool {
    if unsafe { MMIO_BASE == 0 } { return false; }
    unsafe { (rd32(reg::STATUS) & status::LU) != 0 }
}

// ============================================================================
// NetDevice trait impl + registration
// ============================================================================

pub struct E1000eNet;

impl NetDevice for E1000eNet {
    fn send(&self, packet: &[u8]) -> DriverResult<()> {
        if send_frame(packet) { Ok(()) } else { Err(DriverError::IoError) }
    }

    fn recv(&self, buf: &mut [u8]) -> DriverResult<usize> {
        recv_frame(buf)
    }

    fn poll(&self) -> bool {
        rx_has_data()
    }

    fn mac_address(&self) -> [u8; 6] {
        mac_address()
    }

    fn mtu(&self) -> usize {
        1500
    }

    fn link_up(&self) -> bool {
        link_up()
    }

    fn name(&self) -> &'static str {
        "e1000e0"
    }
}

pub static E1000E_NET: E1000eNet = E1000eNet;

/// Register with kernel-core's driver registry. Must be called after `init()`.
pub fn register_with_kernel_core() -> bool {
    kernel_core::drivers::registry::register_net("e1000e0", &E1000E_NET)
}
