//! Phase 15 M50 follow-up — USB 2.0 hub class bring-up.
//!
//! Scope (this slice):
//!  * Read the hub descriptor (type 0x29 — USB 2.0 high-speed hub).
//!  * Apply SET_FEATURE(PORT_POWER) to every downstream port.
//!  * Wait `bPwrOn2PwrGood × 2 ms` for power to stabilize.
//!  * Read each port's status; for any port reporting Connection, issue
//!    SET_FEATURE(PORT_RESET) and poll for C_PORT_RESET.
//!  * Log the per-port verdict (speed, connected, enabled).
//!
//! Out of scope (next session): downstream `EnableSlot`/`AddressDevice`
//! on a child slot, route-string construction, transaction-translator
//! info for full/low-speed children behind a high-speed hub. Those
//! require a multi-slot refactor of the EP0 transfer ring statics in
//! `xhci.rs` (currently SLOT1-only).
//!
//! USB 3 SuperSpeed hubs (descriptor type 0x2A) use a slightly different
//! port-status layout; this module handles USB 2.0 today. SS support is
//! a follow-up — the 40A1 dock exposes both USB-2 and USB-3 hub paths.

use crate::println;

/// USB 2.0 hub class. See USB 2.0 spec § 11.24.
pub mod hub_request {
    /// Standard GET_DESCRIPTOR repurposed via class type byte.
    pub const GET_DESCRIPTOR: u8 = 0x06;
    /// Hub-class GET_PORT_STATUS.
    pub const GET_PORT_STATUS: u8 = 0x00;
    /// Hub-class SET_FEATURE.
    pub const SET_FEATURE: u8 = 0x03;
    /// Hub-class CLEAR_FEATURE.
    pub const CLEAR_FEATURE: u8 = 0x01;
}

/// USB 2.0 hub port features (§ 11.24.2 Table 11-17).
pub mod port_feature {
    pub const PORT_CONNECTION: u16 = 0;
    pub const PORT_ENABLE: u16 = 1;
    pub const PORT_RESET: u16 = 4;
    pub const PORT_POWER: u16 = 8;
    pub const C_PORT_CONNECTION: u16 = 16;
    pub const C_PORT_RESET: u16 = 20;
}

/// USB 2.0 hub descriptor (§ 11.23.2.1 Table 11-13). Type byte = 0x29.
pub const HUB_DESCRIPTOR_TYPE: u16 = 0x29;

#[derive(Copy, Clone, Default, Debug)]
pub struct HubDescriptor {
    pub b_desc_length: u8,
    pub b_descriptor_type: u8,
    pub b_nbr_ports: u8,
    pub w_hub_characteristics: u16,
    /// Time from PORT_POWER until the port reports good power, in 2 ms units.
    pub b_pwr_on_2_pwr_good: u8,
    pub b_hub_contr_current: u8,
}

#[derive(Copy, Clone, Default)]
pub struct PortStatus {
    pub status: u16,
    pub change: u16,
}

impl PortStatus {
    pub fn connected(&self) -> bool { (self.status & 0x0001) != 0 }
    pub fn enabled(&self) -> bool { (self.status & 0x0002) != 0 }
    pub fn powered(&self) -> bool { (self.status & 0x0100) != 0 }
    pub fn low_speed(&self) -> bool { (self.status & 0x0200) != 0 }
    pub fn high_speed(&self) -> bool { (self.status & 0x0400) != 0 }
    /// Convert to the xHCI-style speed enum used by the rest of the driver
    /// (1=FS, 2=LS, 3=HS, 4=SS). Default to FS if uncertain.
    pub fn xhci_speed(&self) -> u8 {
        if self.low_speed() { 2 }
        else if self.high_speed() { 3 }
        else { 1 }
    }
}

/// Issue GET_DESCRIPTOR(Hub) against `slot_id`. Returns Some(desc) on
/// success. The hub descriptor is class-type, so bmRequestType is
/// 0xA0 (device→host, class, recipient=device).
pub fn read_hub_descriptor(slot_id: u8) -> Option<HubDescriptor> {
    let mut buf = [0u8; 9];
    let ok = crate::usb::xhci::control_in(
        slot_id,
        0xA0,
        hub_request::GET_DESCRIPTOR,
        HUB_DESCRIPTOR_TYPE << 8,
        0,
        buf.len() as u16,
        buf.as_mut_ptr(),
        buf.len(),
    );
    if !ok {
        println!("[xhci-hub] slot={} GET_DESCRIPTOR(Hub) failed", slot_id);
        return None;
    }
    Some(HubDescriptor {
        b_desc_length: buf[0],
        b_descriptor_type: buf[1],
        b_nbr_ports: buf[2],
        w_hub_characteristics: u16::from_le_bytes([buf[3], buf[4]]),
        b_pwr_on_2_pwr_good: buf[5],
        b_hub_contr_current: buf[6],
    })
}

/// SET_FEATURE(PORT_POWER) — turn power on for `port_num` (1-indexed).
pub fn set_port_power(slot_id: u8, port_num: u8) -> bool {
    crate::usb::xhci::control_out(
        slot_id,
        0x23, // host→device, class, recipient=other (per-port)
        hub_request::SET_FEATURE,
        port_feature::PORT_POWER,
        port_num as u16,
        0,
    )
}

/// SET_FEATURE(PORT_RESET) — issue a USB reset on `port_num`. After the
/// reset completes the spec mandates the hub clears its own PORT_RESET
/// and sets C_PORT_RESET. Callers must clear C_PORT_RESET themselves.
pub fn set_port_reset(slot_id: u8, port_num: u8) -> bool {
    crate::usb::xhci::control_out(
        slot_id,
        0x23,
        hub_request::SET_FEATURE,
        port_feature::PORT_RESET,
        port_num as u16,
        0,
    )
}

/// CLEAR_FEATURE on a port (used for C_PORT_RESET / C_PORT_CONNECTION).
pub fn clear_port_feature(slot_id: u8, port_num: u8, feature: u16) -> bool {
    crate::usb::xhci::control_out(
        slot_id,
        0x23,
        hub_request::CLEAR_FEATURE,
        feature,
        port_num as u16,
        0,
    )
}

/// GET_PORT_STATUS — 4-byte response: { wPortStatus, wPortChange }.
pub fn get_port_status(slot_id: u8, port_num: u8) -> Option<PortStatus> {
    let mut buf = [0u8; 4];
    let ok = crate::usb::xhci::control_in(
        slot_id,
        0xA3, // device→host, class, recipient=other (per-port)
        hub_request::GET_PORT_STATUS,
        0,
        port_num as u16,
        buf.len() as u16,
        buf.as_mut_ptr(),
        buf.len(),
    );
    if !ok { return None; }
    Some(PortStatus {
        status: u16::from_le_bytes([buf[0], buf[1]]),
        change: u16::from_le_bytes([buf[2], buf[3]]),
    })
}

/// Spin-wait `cycles` arbitrary CPU cycles. We don't have a precise
/// millisecond timer wired into this code path; the existing xHCI bring-up
/// uses the same pattern (`for _ in 0..N { spin_loop() }`). 200M ≈ a few
/// ms on a modern CPU, fine for `bPwrOn2PwrGood` waits (typical value: 50,
/// i.e. 100 ms).
fn spin_for(cycles: u64) {
    for _ in 0..cycles {
        core::hint::spin_loop();
    }
}

/// One-shot hub bring-up entry point. Called from `enumerate_device` when
/// the device descriptor reports class 0x09. Reads the hub descriptor,
/// powers every port, resets connected ports, and enumerates each child
/// via `xhci::enumerate_child_of_hub` (a fresh xHCI slot per child).
///
/// `root_hub_port` is the root-hub port that this hub branch hangs off —
/// child slot contexts need this even though they sit one tier below.
///
/// Returns the number of downstream ports successfully enumerated.
pub fn bring_up_hub(slot_id: u8, root_hub_port: u8) -> u8 {
    println!("[xhci-hub] slot={} bringing up hub", slot_id);

    let desc = match read_hub_descriptor(slot_id) {
        Some(d) => d,
        None => return 0,
    };
    println!("[xhci-hub] slot={} ports={} pwr2good={}ms hub-chars=0x{:04X}",
        slot_id, desc.b_nbr_ports,
        desc.b_pwr_on_2_pwr_good as u32 * 2,
        desc.w_hub_characteristics);

    // ---- Power up every downstream port ----
    for port in 1..=desc.b_nbr_ports {
        if !set_port_power(slot_id, port) {
            println!("[xhci-hub] slot={} port={} SET_FEATURE(PORT_POWER) failed",
                slot_id, port);
        }
    }
    // Wait the spec-mandated power-on-to-power-good time. The hub guarantees
    // PortPower=1 status reflects reality after this delay. We multiply by
    // a generous spin scale since we don't have a real ms timer here.
    spin_for(50_000_000u64 * desc.b_pwr_on_2_pwr_good.max(1) as u64);

    // ---- Per-port: status check, reset if connected ----
    let mut connected = 0u8;
    for port in 1..=desc.b_nbr_ports {
        let st = match get_port_status(slot_id, port) {
            Some(s) => s,
            None => {
                println!("[xhci-hub] slot={} port={} GET_PORT_STATUS failed",
                    slot_id, port);
                continue;
            }
        };
        if !st.connected() {
            // Empty port — log only at high verbosity. Skip silently for
            // now; an empty 7-port dock would otherwise spam the serial.
            continue;
        }
        println!("[xhci-hub] slot={} port={} CONNECTED before-reset status=0x{:04X} change=0x{:04X}",
            slot_id, port, st.status, st.change);

        // Clear the C_PORT_CONNECTION change bit before resetting.
        let _ = clear_port_feature(slot_id, port, port_feature::C_PORT_CONNECTION);

        if !set_port_reset(slot_id, port) {
            println!("[xhci-hub] slot={} port={} SET_FEATURE(PORT_RESET) failed",
                slot_id, port);
            continue;
        }

        // Poll up to ~200 ms for C_PORT_RESET. USB 2.0 spec requires the
        // hub to report reset complete within 20 ms; we give a wider window.
        let mut reset_ok = false;
        let mut child_speed = 0u8;
        for _ in 0..50 {
            spin_for(2_000_000);
            if let Some(st2) = get_port_status(slot_id, port) {
                if (st2.change & (1 << 4)) != 0 /* C_PORT_RESET */ {
                    println!("[xhci-hub] slot={} port={} reset complete status=0x{:04X} change=0x{:04X} speed={}",
                        slot_id, port, st2.status, st2.change, st2.xhci_speed());
                    let _ = clear_port_feature(slot_id, port, port_feature::C_PORT_RESET);
                    reset_ok = true;
                    child_speed = st2.xhci_speed();
                    break;
                }
            }
        }
        if !reset_ok {
            println!("[xhci-hub] slot={} port={} reset did not complete within ~200 ms",
                slot_id, port);
            continue;
        }

        // Now actually enumerate the child: EnableSlot, build input ctx
        // with route string + TT info, AddressDevice, dispatch to class.
        if crate::usb::xhci::enumerate_child_of_hub(
            slot_id, port, root_hub_port, child_speed,
        ) {
            println!("[xhci-hub] slot={} port={} child enumerated", slot_id, port);
            connected += 1;
        } else {
            println!("[xhci-hub] slot={} port={} child enumeration FAILED", slot_id, port);
        }
    }

    if connected > 0 {
        println!("[xhci-hub] slot={} {} downstream device(s) enumerated", slot_id, connected);
    } else {
        println!("[xhci-hub] slot={} no downstream devices enumerated", slot_id);
    }
    connected
}
