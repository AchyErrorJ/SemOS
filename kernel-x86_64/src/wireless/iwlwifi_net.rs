//! iwlwifi NetDevice stub — M11 stage 3.
//!
//! Implements the kernel's `NetDevice` trait so smoltcp can be wired to
//! the iwlwifi TX/RX queues once the hardware layer lands.  Until then
//! `link_up` is always `false`, so smoltcp gracefully falls through to
//! `virtio-net0` or `cdc-ecm0`.

use kernel_core::drivers::traits::{NetDevice, DriverError, DriverResult};

pub struct IwlNet;

impl NetDevice for IwlNet {
    fn send(&self, _packet: &[u8]) -> DriverResult<()> {
        // TODO(M11): push frame to iwlwifi TX ring via iwlwifi_device::tx_frame.
        Err(DriverError::NotReady)
    }

    fn recv(&self, _buf: &mut [u8]) -> DriverResult<usize> {
        // TODO(M11): pull frame from iwlwifi RX ring via iwlwifi_device::rx_frame.
        Err(DriverError::WouldBlock)
    }

    fn poll(&self) -> bool {
        // TODO(M11): check RX queue head/tail for available frames.
        false
    }

    fn mac_address(&self) -> [u8; 6] {
        // TODO(M11): read MAC from EEPROM/NVM during PHY init.
        [0x02, 0x00, 0x00, 0x00, 0x00, 0x00]
    }

    fn mtu(&self) -> usize {
        // 802.11 with CCMP: 2304 byte MSDU - 8 byte CCMP header = 2296.
        // We report 1500 here so the IP stack sees a familiar Ethernet-like
        // MTU; the 802.11 MAC will fragment larger frames as needed.
        1500
    }

    fn link_up(&self) -> bool {
        // Only true after firmware upload + ALIVE + PHY init + association.
        super::iwlwifi_device::device()
            .map(|d| d.is_associated())
            .unwrap_or(false)
    }

    fn name(&self) -> &'static str { "iwlwifi0" }
}

pub static IWL_NET: IwlNet = IwlNet;

/// Register with kernel-core's driver registry.  Returns `false` if no
/// iwlwifi NIC was found during PCI probe.
pub fn register_with_kernel_core() -> bool {
    if !super::iwlwifi_device::init() {
        return false;
    }
    kernel_core::drivers::registry::register_net("iwlwifi0", &IWL_NET)
}
