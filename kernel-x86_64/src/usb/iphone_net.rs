//! iPhone tether (ipheth) as a `NetDevice` exposed to kernel-core.
//!
//! Adapter struct that implements the kernel's `NetDevice` trait by
//! forwarding to the EHCI ipheth bulk engine in `usb::ehci`
//! (`ipheth_send_frame` / `ipheth_recv_frame` / `ipheth_has_frame`).
//! Registered with the driver registry as `"ipheth0"` so the smoltcp
//! glue in `kernel-x86_64/src/main.rs` can bring up an IP stack on top
//! (same pattern as `cdc-ecm0` / `virtio-net0`).
//!
//! Mirrors `cdc_ecm_net.rs` exactly, but over the EHCI ipheth API rather
//! than the xHCI CDC-ECM one. We only register if `ehci::ipheth_active()`
//! returns true — i.e. an iPhone with Personal Hotspot on was enumerated
//! and its bulk data path is live.

use kernel_core::drivers::traits::{NetDevice, DriverError, DriverResult};

pub struct IphethNet;

impl NetDevice for IphethNet {
    fn send(&self, packet: &[u8]) -> DriverResult<()> {
        if crate::usb::ehci::ipheth_send_frame(packet) {
            Ok(())
        } else {
            Err(DriverError::IoError)
        }
    }

    fn recv(&self, buf: &mut [u8]) -> DriverResult<usize> {
        let n = crate::usb::ehci::ipheth_recv_frame(buf);
        if n == 0 {
            Err(DriverError::WouldBlock)
        } else {
            Ok(n)
        }
    }

    fn poll(&self) -> bool {
        crate::usb::ehci::ipheth_has_frame()
    }

    fn mac_address(&self) -> [u8; 6] {
        crate::usb::iphone::iphone_device()
            .map(|d| d.mac)
            .unwrap_or([0; 6])
    }

    fn mtu(&self) -> usize {
        // Legacy ipheth framing carries a standard 1500-byte Ethernet
        // payload; the iPhone never advertises a larger segment.
        1500
    }

    fn link_up(&self) -> bool {
        // "Bulk data path live" is our link-up signal — set once the RX
        // QH is armed in ehci::ipheth_net_setup.
        crate::usb::ehci::ipheth_active()
    }

    fn name(&self) -> &'static str { "ipheth0" }
}

pub static IPHETH_NET: IphethNet = IphethNet;

/// Register with kernel-core's driver registry. Returns `false` if no
/// live ipheth data path exists (no iPhone enumerated / hotspot off).
pub fn register_with_kernel_core() -> bool {
    if !crate::usb::ehci::ipheth_active() {
        return false;
    }
    kernel_core::drivers::registry::register_net("ipheth0", &IPHETH_NET)
}
