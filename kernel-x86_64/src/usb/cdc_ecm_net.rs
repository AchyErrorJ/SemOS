//! Phase 15 M50 — CDC-ECM as a `NetDevice` exposed to kernel-core.
//!
//! Adapter struct that implements the kernel's `NetDevice` trait by
//! forwarding to `usb::xhci::cdc_ecm_send_frame` / `cdc_ecm_recv_frame`.
//! Registered with the driver registry as `"cdc-ecm0"` so the existing
//! smoltcp glue at `kernel-x86_64/src/main.rs::~280` (the same pattern
//! used for `virtio-net0`) can pick it up and run a TCP/IP stack on top.
//!
//! For v1 we only register if `usb::xhci::cdc_ecm_device()` returns
//! Some — i.e. an actual USB-Ethernet adapter or tethered phone was
//! enumerated during boot.

use kernel_core::drivers::traits::{NetDevice, DriverError, DriverResult};

pub struct CdcEcmNet;

impl NetDevice for CdcEcmNet {
    fn send(&self, packet: &[u8]) -> DriverResult<()> {
        if crate::usb::xhci::cdc_ecm_send_frame(packet) {
            Ok(())
        } else {
            Err(DriverError::IoError)
        }
    }

    fn recv(&self, buf: &mut [u8]) -> DriverResult<usize> {
        let n = crate::usb::xhci::cdc_ecm_recv_frame(buf);
        if n == 0 {
            Err(DriverError::WouldBlock)
        } else {
            Ok(n)
        }
    }

    fn poll(&self) -> bool {
        crate::usb::xhci::cdc_ecm_has_data()
    }

    fn mac_address(&self) -> [u8; 6] {
        crate::usb::xhci::cdc_ecm_device()
            .map(|d| d.mac)
            .unwrap_or([0; 6])
    }

    fn mtu(&self) -> usize {
        // CDC-ECM functional descriptor reports wMaxSegmentSize; most
        // devices return 1514 (standard Ethernet). Fall back to 1500 if
        // the descriptor said 0 / not enumerated yet.
        let m = crate::usb::xhci::cdc_ecm_device()
            .map(|d| d.mtu as usize)
            .unwrap_or(0);
        if m == 0 { 1500 } else { m }
    }

    fn link_up(&self) -> bool {
        // CDC-ECM's NetworkConnection notification on the interrupt
        // endpoint is the canonical "link up/down" signal. We don't
        // poll the interrupt endpoint yet, so for v1 "enumerated"
        // implies "up". Refine later when we add the interrupt EP.
        crate::usb::xhci::cdc_ecm_device().is_some()
    }

    fn name(&self) -> &'static str { "cdc-ecm0" }
}

pub static CDC_ECM_NET: CdcEcmNet = CdcEcmNet;

/// Register with kernel-core's driver registry. Returns `false` if no
/// CDC-ECM device was enumerated by xHCI during boot.
pub fn register_with_kernel_core() -> bool {
    if crate::usb::xhci::cdc_ecm_device().is_none() {
        return false;
    }
    kernel_core::drivers::registry::register_net("cdc-ecm0", &CDC_ECM_NET)
}
