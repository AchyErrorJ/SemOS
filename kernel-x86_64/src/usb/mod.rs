//! USB stack — xHCI controller + device enumeration + boot HID keyboard.
//!
//! Scope and explicit non-goals (per Task #13 brief):
//!
//! - **In scope:** xHCI bring-up on qemu-xhci, single root-hub port
//!   enumeration, USB device descriptor read, HID boot keyboard
//!   recognition + 8-byte report parsing.
//! - **Out of scope (deferred tasks):** USB hubs, multiple concurrent
//!   devices, hot-plug, interrupt-driven completion (we poll), CDC-ECM
//!   for USB Ethernet, full HID report descriptor parsing, mouse path
//!   (boot mouse falls out trivially if added — the transfer-ring +
//!   event polling code in `xhci.rs` is endpoint-agnostic).
//!
//! See `xhci.rs` for the controller logic, `ring.rs` for TRB ring
//! primitives, `device.rs` for slot/endpoint context layouts and USB
//! descriptors, and `hid.rs` for boot-keyboard parsing.
//!
//! The public entry point for callers (kernel main, DEMO 18) is
//! `init_and_enumerate()` which does the full bring-up and reports
//! a single struct describing what it found.

pub mod ring;
pub mod device;
pub mod hid;
pub mod hid_report;
pub mod cdc_ecm;
pub mod cdc_ecm_net;
pub mod cdc_ncm;
pub mod ehci;
pub mod hub;
pub mod iphone;
pub mod iphone_net;
pub mod mass_storage;
pub mod xhci;

/// One-shot bring-up. Returns true if at least the controller came up.
/// Whether a device was enumerated is reported via `xhci::enumerated_device()`.
pub fn init_and_enumerate() -> bool {
    let xhci_ok = xhci::init();
    if xhci_ok {
        let _ = xhci::enumerate_ports();
    }
    // EHCI second, so it ends up owning the USB-2 ports on chipsets that
    // still have a companion EHCI (W540 / Lynx Point). On such machines
    // intel_pch_port_routing leaves XUSB2PR=0 (USB-2 stays on EHCI) and
    // ehci::init sets CONFIGFLAG=1 last, winning ownership. Machines
    // without EHCI (modern PCH, QEMU default) skip this in one PCI scan.
    let _ = ehci::init_and_enumerate();
    xhci_ok
}
