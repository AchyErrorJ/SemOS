//! Adapter bridging our [`NetDevice`] trait to smoltcp's
//! [`phy::Device`].
//!
//! The bridge is straightforward in shape — `receive` calls into
//! `NetDevice::recv` to copy a frame into an rx scratch buffer and
//! returns an [`RxToken`] over it; `transmit` returns a [`TxToken`]
//! that, when consumed, writes the caller's bytes to a tx scratch
//! buffer and pushes them via `NetDevice::send`.
//!
//! The only subtlety is the borrow checker: smoltcp's `receive`
//! returns *both* an rx token and a tx token from one `&mut self`,
//! and each token holds a borrow for `'_` (the lifetime of `&mut
//! self`). We get away with this by having `rx_buffer` and
//! `tx_buffer` as separate fields — Rust's disjoint-borrow rule
//! permits two `&mut` borrows of distinct fields.

use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

use crate::drivers::traits::NetDevice;

/// Max Ethernet frame we'll send/receive. 1514 = MTU 1500 + 14-byte
/// Ethernet header; rounded up to a comfortable 1536 (the same number
/// smoltcp's StmPhy doc example uses).
const BUFFER_SIZE: usize = 1536;

/// Bridges a [`NetDevice`] to smoltcp's [`phy::Device`].
///
/// One scratch buffer per direction. Single-frame-in-flight model —
/// adequate for the polled, low-throughput network this kernel will
/// drive (it's chatting with an LLM API, not serving traffic).
pub struct NetDeviceAdapter {
    dev: &'static dyn NetDevice,
    rx_buffer: [u8; BUFFER_SIZE],
    tx_buffer: [u8; BUFFER_SIZE],
}

impl NetDeviceAdapter {
    pub const fn new(dev: &'static dyn NetDevice) -> Self {
        Self {
            dev,
            rx_buffer: [0; BUFFER_SIZE],
            tx_buffer: [0; BUFFER_SIZE],
        }
    }

    pub fn device(&self) -> &'static dyn NetDevice { self.dev }
}

impl Device for NetDeviceAdapter {
    type RxToken<'a> = RxToken<'a> where Self: 'a;
    type TxToken<'a> = TxToken<'a> where Self: 'a;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = self.dev.mtu();
        // Single in-flight TX matches our polled NetDevice contract.
        caps.max_burst_size = Some(1);
        caps
    }

    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // Try to drain one frame from the device into rx_buffer.
        // NetDevice::recv returns WouldBlock if nothing's ready — map
        // that to None so smoltcp will retry on the next poll.
        let n = match self.dev.recv(&mut self.rx_buffer) {
            Ok(n) if n > 0 => n,
            _ => return None,
        };
        // Disjoint borrows of self.rx_buffer and self.tx_buffer make
        // both tokens live concurrently. The `dev` field is copied
        // (it's a `&'static dyn` reference, which is `Copy`).
        let dev = self.dev;
        let rx_slice = &mut self.rx_buffer[..n];
        let tx_slice = &mut self.tx_buffer[..];
        Some((RxToken { buf: rx_slice }, TxToken { dev, buf: tx_slice }))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> {
        Some(TxToken { dev: self.dev, buf: &mut self.tx_buffer[..] })
    }
}

/// Rx-side token. Owns a borrow of the adapter's rx scratch buffer for
/// the duration smoltcp inspects the frame.
pub struct RxToken<'a> {
    buf: &'a mut [u8],
}

impl<'a> phy::RxToken for RxToken<'a> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(self.buf)
    }
}

/// Tx-side token. Holds the device reference + tx scratch buffer; on
/// consume, hands the buffer to smoltcp to write `len` bytes into,
/// then pushes those bytes to the device.
pub struct TxToken<'a> {
    dev: &'static dyn NetDevice,
    buf: &'a mut [u8],
}

impl<'a> phy::TxToken for TxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        debug_assert!(len <= self.buf.len(),
            "smoltcp asked for tx buffer {} > capacity {}", len, self.buf.len());
        let r = f(&mut self.buf[..len]);
        // Push the prepared frame to the device. We deliberately swallow
        // send errors here — there's no way to signal a failure back
        // through smoltcp's TxToken contract beyond logging. The
        // underlying driver already logs on TX timeout.
        let _ = self.dev.send(&self.buf[..len]);
        r
    }
}
