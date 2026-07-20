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
/// USB 3.0 SuperSpeed hub descriptor (USB 3.2 spec § 10.15). Type
/// byte = 0x2A. Layout is similar to the USB-2 hub descriptor — same
/// first 6 bytes — so we can parse it into the same struct.
pub const HUB_DESCRIPTOR_TYPE_SS: u16 = 0x2A;

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
    /// true when the hub itself is SuperSpeed; SS hub ports only ever see
    /// SS devices, so xhci_speed() returns 4 without inspecting status bits.
    pub hub_is_ss: bool,
}

impl PortStatus {
    pub fn connected(&self) -> bool { (self.status & 0x0001) != 0 }
    pub fn enabled(&self) -> bool { (self.status & 0x0002) != 0 }
    pub fn powered(&self) -> bool { (self.status & 0x0100) != 0 }
    pub fn low_speed(&self) -> bool { !self.hub_is_ss && (self.status & 0x0200) != 0 }
    pub fn high_speed(&self) -> bool { !self.hub_is_ss && (self.status & 0x0400) != 0 }
    /// PORT_LINK_STATE for USB 3.0 hub ports (bits 7:4 of wPortStatus).
    /// U0 = 3, U1 = 4, U2 = 5, U3 = 6, etc.
    pub fn link_state(&self) -> u8 { ((self.status >> 4) & 0xF) as u8 }
    /// Convert to the xHCI-style speed enum used by the rest of the driver
    /// (1=FS, 2=LS, 3=HS, 4=SS).
    pub fn xhci_speed(&self) -> u8 {
        if self.hub_is_ss { 4 }
        else if self.low_speed() { 2 }
        else if self.high_speed() { 3 }
        else { 1 }
    }
}

/// Issue GET_DESCRIPTOR(Hub) against `slot_id`. Returns Some(desc) on
/// success. The hub descriptor is class-type, so bmRequestType is
/// 0xA0 (device→host, class, recipient=device).
pub fn read_hub_descriptor(slot_id: u8, speed: u8) -> Option<HubDescriptor> {
    let mut buf = [0u8; 12];
    // SuperSpeed hubs (speed=4) use descriptor type 0x2A, USB-2 hubs use
    // 0x29. Sending the wrong type causes the device to STALL the EP0
    // setup stage — exactly the cc=6 we saw on the Pro Dock 40A1.
    let dtype = if speed == 4 { HUB_DESCRIPTOR_TYPE_SS } else { HUB_DESCRIPTOR_TYPE };
    let ok = crate::usb::xhci::control_in(
        slot_id,
        0xA0,
        hub_request::GET_DESCRIPTOR,
        dtype << 8,
        0,
        buf.len() as u16,
        buf.as_mut_ptr(),
        buf.len(),
    );
    if !ok {
        crate::usb::usbdbg!("[xhci-hub] slot={} GET_DESCRIPTOR(Hub, type=0x{:02X}, speed={}) failed",
            slot_id, dtype, speed);
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

/// SET_HUB_DEPTH — USB 3.0 hubs require this before they can route packets
/// to downstream ports using the route_string. `depth` is the hub's tier
/// depth (0 = directly on root hub, 1 = behind one hub, etc.).
pub fn set_hub_depth(slot_id: u8, depth: u8) -> bool {
    crate::usb::xhci::control_out(
        slot_id,
        0x20, // host→device, class, recipient=device
        0x0C, // SET_HUB_DEPTH
        depth as u16,
        0,
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
/// `hub_speed` is the speed of the hub itself (4 = SuperSpeed); needed so
/// `PortStatus::xhci_speed()` knows whether to trust the LS/HS bits.
pub fn get_port_status(slot_id: u8, port_num: u8, hub_speed: u8) -> Option<PortStatus> {
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
        hub_is_ss: hub_speed == 4,
    })
}

/// Sleep at least `ms` milliseconds using the scheduler tick (62 Hz, ~16 ms
/// resolution; rounds up). Tick-based so a 100 ms power-good wait is actually
/// ~100 ms instead of a multi-billion-cycle guess that ran for seconds. Ctrl+C
/// cuts the wait short so a runaway enumeration can be aborted.
fn sleep_ms(ms: u64) {
    let ticks_needed = (ms * 62).div_ceil(1000) + 1;
    let end = kernel_core::platform::ticks() + ticks_needed;
    while kernel_core::platform::ticks() < end {
        if crate::keyboard::abort_requested() { return; }
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
/// `speed` is the speed at which the hub was enumerated. SuperSpeed hubs
/// (speed=4) require descriptor type 0x2A instead of 0x29 — sending the
/// wrong type yields STALL (cc=6).
///
/// Returns the number of downstream ports successfully enumerated.
pub fn bring_up_hub(slot_id: u8, root_hub_port: u8, speed: u8) -> u8 {
    crate::usb::usbdbg!("[xhci-hub] slot={} bringing up hub", slot_id);

    let desc = match read_hub_descriptor(slot_id, speed) {
        Some(d) => d,
        None => return 0,
    };
    crate::usb::usbdbg!("[xhci-hub] slot={} ports={} pwr2good={}ms hub-chars=0x{:04X}",
        slot_id, desc.b_nbr_ports,
        desc.b_pwr_on_2_pwr_good as u32 * 2,
        desc.w_hub_characteristics);

    // ---- Power up every downstream port ----
    for port in 1..=desc.b_nbr_ports {
        if !set_port_power(slot_id, port) {
            crate::usb::usbdbg!("[xhci-hub] slot={} port={} SET_FEATURE(PORT_POWER) failed",
                slot_id, port);
        }
    }
    // Wait the spec-mandated power-on-to-power-good time. The hub guarantees
    // PortPower=1 status reflects reality after this delay. We multiply by
    // a generous spin scale since we don't have a real ms timer here.
    sleep_ms(desc.b_pwr_on_2_pwr_good.max(1) as u64 * 2); // bPwrOn2PwrGood is in 2 ms units

    // ---- Per-port: status check, reset if connected ----
    let mut connected = 0u8;
    let hub_is_ss = speed == 4;
    for port in 1..=desc.b_nbr_ports {
        let st = match get_port_status(slot_id, port, speed) {
            Some(s) => s,
            None => {
                crate::usb::usbdbg!("[xhci-hub] slot={} port={} GET_PORT_STATUS failed",
                    slot_id, port);
                continue;
            }
        };
        if !st.connected() {
            continue;
        }
        crate::usb::usbdbg!("[xhci-hub] slot={} port={} CONNECTED status=0x{:04X} change=0x{:04X} powered={} enabled={}",
            slot_id, port, st.status, st.change, st.powered(), st.enabled());

        // USB 3.0 hub ports train their link automatically. We must wait
        // for PORT_LINK_STATE == U0 (3) before the device will respond to
        // requests. Checking only PORT_ENABLED is not enough — enabled=1
        // just means the port is not disabled; the link could still be in
        // Rx.Detect (0) or Polling (1).
        let mut child_speed = st.xhci_speed();
        if hub_is_ss {
            // Poll for U0 using the scheduler tick (62 Hz) instead of a fixed
            // iteration count so the wait scales with CPU speed. ~64 ms window.
            let mut link_ok = st.link_state() == 3;
            let link_deadline = kernel_core::platform::ticks() + 4;
            while !link_ok && kernel_core::platform::ticks() < link_deadline {
                if let Some(st2) = get_port_status(slot_id, port, speed) {
                    if st2.link_state() == 3 {
                        link_ok = true;
                        break;
                    }
                } else {
                    break;
                }
                core::hint::spin_loop();
            }
            if !link_ok {
                crate::usb::usbdbg!("[xhci-hub] slot={} port={} SS link did not reach U0 (link_state={}) — falling back to reset path",
                    slot_id, port, st.link_state());
                // fall through to the USB-2-style reset path below
            } else {
                crate::usb::usbdbg!("[xhci-hub] slot={} port={} SS link in U0 — skipping reset", slot_id, port);
                let _ = clear_port_feature(slot_id, port, port_feature::C_PORT_CONNECTION);
                sleep_ms(1); // ~16-32 ms tick-based pause before child enum
                if crate::usb::xhci::enumerate_child_of_hub(
                    slot_id, port, root_hub_port, child_speed,
                ) {
                    crate::usb::usbdbg!("[xhci-hub] slot={} port={} child enumerated", slot_id, port);
                    connected += 1;
                } else {
                    crate::usb::usbdbg!("[xhci-hub] slot={} port={} child enumeration FAILED", slot_id, port);
                }
                continue;
            }
        }

        // ---- USB 2.0 / fallback path: power + reset ----
        if !st.powered() {
            crate::usb::usbdbg!("[xhci-hub] slot={} port={} not powered — retrying PORT_POWER", slot_id, port);
            if !set_port_power(slot_id, port) {
                crate::usb::usbdbg!("[xhci-hub] slot={} port={} SET_FEATURE(PORT_POWER) failed, skipping",
                    slot_id, port);
                continue;
            }
            sleep_ms(desc.b_pwr_on_2_pwr_good.max(1) as u64 * 2); // bPwrOn2PwrGood is in 2 ms units
        }

        // Small delay before the next class request so the hub isn't back-to-back.
        sleep_ms(1);

        // Clear the C_PORT_CONNECTION change bit before resetting.
        let _ = clear_port_feature(slot_id, port, port_feature::C_PORT_CONNECTION);
        sleep_ms(1);

        if !set_port_reset(slot_id, port) {
            crate::usb::usbdbg!("[xhci-hub] slot={} port={} SET_FEATURE(PORT_RESET) failed",
                slot_id, port);
            continue;
        }

        // Poll up to ~200 ms for C_PORT_RESET, using the scheduler tick so the
        // timeout is wall-clock based rather than a cycle-count guess.
        let mut reset_ok = false;
        let reset_deadline = kernel_core::platform::ticks() + 13; // ~210 ms @ 62 Hz
        while kernel_core::platform::ticks() < reset_deadline {
            if let Some(st2) = get_port_status(slot_id, port, speed) {
                if (st2.change & (1 << 4)) != 0 /* C_PORT_RESET */ {
                    crate::usb::usbdbg!("[xhci-hub] slot={} port={} reset complete status=0x{:04X} change=0x{:04X} speed={}",
                        slot_id, port, st2.status, st2.change, st2.xhci_speed());
                    let _ = clear_port_feature(slot_id, port, port_feature::C_PORT_RESET);
                    reset_ok = true;
                    child_speed = st2.xhci_speed();
                    break;
                }
            }
            core::hint::spin_loop();
        }
        if !reset_ok {
            crate::usb::usbdbg!("[xhci-hub] slot={} port={} reset did not complete within ~200 ms",
                slot_id, port);
            continue;
        }

        // USB 2.0 spec §9.1.1.5: host must wait ≥10 ms after reset before
        // the device will respond to data transfers. Give a generous margin.
        sleep_ms(12); // USB 2.0 §9.1.1.5: ≥10 ms after reset before data

        // Now actually enumerate the child: EnableSlot, build input ctx
        // with route string + TT info, AddressDevice, dispatch to class.
        if crate::usb::xhci::enumerate_child_of_hub(
            slot_id, port, root_hub_port, child_speed,
        ) {
            crate::usb::usbdbg!("[xhci-hub] slot={} port={} child enumerated", slot_id, port);
            connected += 1;
        } else {
            crate::usb::usbdbg!("[xhci-hub] slot={} port={} child enumeration FAILED", slot_id, port);
        }
    }

    if connected > 0 {
        crate::usb::usbdbg!("[xhci-hub] slot={} {} downstream device(s) enumerated", slot_id, connected);
    } else {
        crate::usb::usbdbg!("[xhci-hub] slot={} no downstream devices enumerated", slot_id);
    }
    connected
}
