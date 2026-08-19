//! UDP send for netlog — the kernel-core half.
//!
//! kernel-x86_64's `netlog` ring/parsing can't reach the network stack:
//! `state::sockets_mut()` is `pub(super)` and `smoltcp` isn't a direct dep of
//! the x86_64 crate. So the actual datagram send lives here, next to the DNS
//! resolver whose socket shape it mirrors. The x86_64 side drains its log ring
//! and calls `send_udp(dest, port, bytes)` in ordinary syscall context.
//!
//! UDP is fire-and-forget (no handshake, loss-tolerant) — right for a debug
//! log: it can't stall the caller the way a TCP connect would if the listener
//! isn't up. Best-effort delivery is the accepted trade.

use smoltcp::socket::udp;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

/// Ephemeral source port for netlog datagrams.
const LOCAL_PORT: u16 = 49154;

/// Max bytes per datagram: stay under the Ethernet payload (1500 − 20 IP − 8
/// UDP = 1472) so a datagram never fragments.
const MTU: usize = 1400;

static mut RX_META: [udp::PacketMetadata; 2] = [udp::PacketMetadata::EMPTY; 2];
static mut TX_META: [udp::PacketMetadata; 16] = [udp::PacketMetadata::EMPTY; 16];
static mut RX_PAYLOAD: [u8; 1600] = [0; 1600];
// TX payload must hold SEVERAL full datagrams: with only ~1.14 packets of
// headroom (1600 B vs 1400 B MTU) every second chunk hits smoltcp's ring-wrap
// padding path, which panics in `enqueue_with` ("range end index 1400 out of
// range for slice of length 400") when a cold ARP leaves the first datagram
// queued. 16 KiB holds the whole 8 KiB log ring with room to spare.
static mut TX_PAYLOAD: [u8; 16384] = [0; 16384];

/// Send `bytes` to `dest:port` as a sequence of MTU-sized UDP datagrams.
/// Returns the number of bytes handed to the socket (0 if the stack is down or
/// the socket couldn't be opened). Best-effort: a datagram that fails to queue
/// is skipped, not retried — this is a log, not a protocol.
pub fn send_udp(dest: Ipv4Address, port: u16, bytes: &[u8]) -> usize {
    if !super::state::is_initialized() || bytes.is_empty() {
        return 0;
    }
    let remote = IpEndpoint::new(IpAddress::Ipv4(dest), port);
    let mut sent = 0usize;
    unsafe {
        let sockets = match super::state::sockets_mut() {
            Some(s) => s,
            None => return 0,
        };
        let rx_meta: &'static mut [udp::PacketMetadata] = &mut *core::ptr::addr_of_mut!(RX_META);
        let tx_meta: &'static mut [udp::PacketMetadata] = &mut *core::ptr::addr_of_mut!(TX_META);
        let rx_payload: &'static mut [u8] = &mut *core::ptr::addr_of_mut!(RX_PAYLOAD);
        let tx_payload: &'static mut [u8] = &mut *core::ptr::addr_of_mut!(TX_PAYLOAD);
        let rx_buf = udp::PacketBuffer::new(rx_meta, rx_payload);
        let tx_buf = udp::PacketBuffer::new(tx_meta, tx_payload);
        let mut socket = udp::Socket::new(rx_buf, tx_buf);
        if socket.bind(LOCAL_PORT).is_err() {
            return 0;
        }
        let handle = sockets.add(socket);

        let mut off = 0usize;
        while off < bytes.len() {
            let end = (off + MTU).min(bytes.len());
            let pkt = &bytes[off..end];
            if sockets
                .get_mut::<udp::Socket>(handle)
                .send_slice(pkt, remote)
                .is_ok()
            {
                // Poll so the datagram actually goes out (and ARP resolves on a
                // cold send). A few polls is plenty on a LAN.
                for _ in 0..64 {
                    super::state::poll();
                }
                sent += pkt.len();
            }
            off = end;
        }

        sockets.get_mut::<udp::Socket>(handle).close();
        sockets.remove(handle);
    }
    sent
}
