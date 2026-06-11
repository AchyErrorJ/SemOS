//! Static net-stack state and the init/poll entry points.
//!
//! Holds the smoltcp [`Interface`], the [`SocketSet`] (and its backing
//! storage), and our [`NetDeviceAdapter`]. Everything is in `static mut`
//! because:
//!   - no `alloc` in kernel-core, so we can't `Box<Interface>`
//!   - smoltcp's `Interface` / `SocketSet` are not `Send` so a `Mutex`
//!     wouldn't help anyway in our single-core kernel
//!   - the existing pattern (virtio-block's `VRING`, ChaCha key tables)
//!     is exactly this shape
//!
//! Access discipline is the same one the rest of the kernel uses:
//! `unsafe { &mut *core::ptr::addr_of_mut!(STATIC) }` at every call
//! site, and don't hold the resulting `&mut` across calls that might
//! re-enter.
//!
//! # v1 scope
//! Just the Interface + an empty SocketSet. That's enough for smoltcp
//! to respond to ARP requests for our IP and discard non-IP frames
//! cleanly. TCP, DHCP, DNS sockets land in follow-up commits.

use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, Ipv4Address, Ipv4Cidr};

use super::adapter::NetDeviceAdapter;
use super::clock;

use crate::drivers::traits::NetDevice;

// ============================================================================
// Static state
// ============================================================================

/// Number of socket slots the kernel reserves. Pick small for v1; can
/// expand when concurrent sockets become useful. (+1 slot reserved for
/// the DHCP client socket when `start_dhcp` runs.)
const SOCKET_SLOTS: usize = 6;

/// Backing storage for the [`SocketSet`]. Each `SocketStorage::EMPTY`
/// is a `None`-shaped slot until a socket is added.
static mut SOCKET_STORAGE: [smoltcp::iface::SocketStorage<'static>; SOCKET_SLOTS] =
    [smoltcp::iface::SocketStorage::EMPTY; SOCKET_SLOTS];

/// The phy::Device adapter. Initialized when `init()` runs — the inner
/// `NetDevice` reference is `&'static dyn`, supplied by the caller.
static mut DEVICE: Option<NetDeviceAdapter> = None;

/// The smoltcp Interface. Owns ARP cache, routing, IP config.
static mut IFACE: Option<Interface> = None;

/// The socket container. Empty for v1; populated when TCP/DNS lands.
static mut SOCKETS: Option<SocketSet<'static>> = None;

/// Are we initialised? Set by `init()`, checked by `poll()` so a tick
/// fired before `init` is a no-op rather than a deref of `None`.
static mut INITIALIZED: bool = false;

/// DHCP client socket handle, when `start_dhcp` has run. Polled inside
/// `poll()`; a lease replaces the interface's static address + default
/// route + DNS server (static config stays as the pre-lease fallback).
static mut DHCP_HANDLE: Option<smoltcp::iface::SocketHandle> = None;

// ============================================================================
// Default IP config — v1 hardcoded, future DHCP via socket-dhcpv4.
// ============================================================================
//
// QEMU SLIRP user-mode networking defaults:
//   guest IP:    10.0.2.15/24
//   gateway:     10.0.2.2
//   DNS server:  10.0.2.3
//
// These are SLIRP's published defaults; they're also fine for any
// developer that runs the kernel with `-netdev user,...`. When DHCP
// is wired up later this constant goes away — the lease provides them.

const GUEST_IP:    Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
const GUEST_CIDR:  u8 = 24;
const GATEWAY:     Ipv4Address = Ipv4Address::new(10, 0, 2, 2);
const SLIRP_DNS:   Ipv4Address = Ipv4Address::new(10, 0, 2, 3);

/// DNS resolver for the active interface. SLIRP default; overridden by
/// `init_with_ipconfig` (e.g. the iPhone tether hands out 172.20.10.1).
static mut DNS_SERVER_IP: Ipv4Address = SLIRP_DNS;

/// The DNS server the resolver (net/dns.rs) should query.
pub fn dns_server() -> Ipv4Address {
    unsafe { DNS_SERVER_IP }
}

// ============================================================================
// Public API
// ============================================================================

/// Bring up the network stack on top of `dev` with the QEMU SLIRP
/// default IP config (10.0.2.15/24, gw 10.0.2.2, dns 10.0.2.3). Thin
/// wrapper over [`init_with_ipconfig`] for the common developer case.
///
/// Idempotent: calling twice silently returns `true` the second time.
pub fn init(dev: &'static dyn NetDevice) -> bool {
    init_with_ipconfig(dev, GUEST_IP, GUEST_CIDR, GATEWAY, SLIRP_DNS)
}

/// Bring up the network stack on top of `dev` with an explicit IP
/// config. The device must already have completed its own bring-up
/// (virtio-net does this in `virtio::net::init()`); we just consume
/// its MAC + send/recv APIs.
///
/// `ip`/`cidr` is the interface address, `gateway` the default route,
/// `dns` the resolver the DNS client (net/dns.rs) should query.
///
/// Idempotent: calling twice silently returns `true` the second time.
pub fn init_with_ipconfig(
    dev: &'static dyn NetDevice,
    ip: Ipv4Address,
    cidr: u8,
    gateway: Ipv4Address,
    dns: Ipv4Address,
) -> bool {
    unsafe {
        if INITIALIZED { return true; }

        // Construct the phy::Device adapter from the supplied NetDevice.
        DEVICE = Some(NetDeviceAdapter::new(dev));
        let device_ref = match DEVICE.as_mut() {
            Some(d) => d,
            None => return false,
        };

        // smoltcp needs to know our hardware (MAC) address up front.
        let mac = dev.mac_address();
        let hwaddr = HardwareAddress::Ethernet(EthernetAddress(mac));
        let mut config = Config::new(hwaddr);
        // Seed smoltcp's internal RNG (used for TCP ISN, DNS txid, etc.).
        // Using ticks is weak — fine for a single-host test environment
        // but will need a real RNG (RDRAND-seeded) when the network goes
        // off-machine. Tracked as an open question in the embedded-tls brief.
        config.random_seed = crate::platform::ticks();

        // Build the Interface. The `now` argument is just the initial
        // timestamp for internal counters.
        let mut iface = Interface::new(config, device_ref, clock::now());

        // Apply the caller's IP + default route.
        let ip_cidr = IpCidr::Ipv4(Ipv4Cidr::new(ip, cidr));
        iface.update_ip_addrs(|addrs| {
            // `push` returns Result, but we know the heapless Vec has
            // room (it's sized for IFACE_MAX_ADDR_COUNT which is ≥ 1).
            let _ = addrs.push(ip_cidr);
        });
        let _ = iface.routes_mut().add_default_ipv4_route(gateway);

        IFACE = Some(iface);

        // Record the DNS server so the resolver queries the right host.
        DNS_SERVER_IP = dns;

        // Empty SocketSet — no sockets yet in v1. `addr_of_mut!` keeps
        // the borrow scoping explicit; we hand smoltcp a `&'static mut`
        // slice into our storage.
        let storage: &'static mut [smoltcp::iface::SocketStorage<'static>] =
            &mut *core::ptr::addr_of_mut!(SOCKET_STORAGE);
        SOCKETS = Some(SocketSet::new(storage));

        INITIALIZED = true;

        crate::platform::log("[net] smoltcp interface up: ");
        log_ipv4(&ip);
        crate::platform::log("/");
        crate::platform::log_num(cidr as u64);
        crate::platform::log(" via ");
        log_ipv4(&gateway);
        crate::platform::log(" on ");
        crate::platform::log(dev.name());
        crate::platform::log("\n");

        true
    }
}

/// Drive one tick of the network stack. Call this from the scheduler
/// timer (or any periodic context). Cheap if nothing has changed.
///
/// Returns `true` if the poll did meaningful work (a frame moved, a
/// timer expired). The return is informational — useful for adaptive
/// sleeping later; ignore for now.
pub fn poll() -> bool {
    unsafe {
        if !INITIALIZED { return false; }
        let iface = match IFACE.as_mut() {
            Some(i) => i,
            None => return false,
        };
        let device = match DEVICE.as_mut() {
            Some(d) => d,
            None => return false,
        };
        let sockets = match SOCKETS.as_mut() {
            Some(s) => s,
            None => return false,
        };
        let worked = iface.poll(clock::now(), device, sockets);
        dhcp_process(iface, sockets);
        worked
    }
}

/// Start the DHCP client on the active interface. Idempotent. The
/// current (static) config keeps working until a lease arrives; the
/// lease then replaces address, default route, and DNS server. Needed
/// for networks that only route leased clients (iPhone Personal
/// Hotspot is suspected to be one).
pub fn start_dhcp() -> bool {
    unsafe {
        if !INITIALIZED { return false; }
        if DHCP_HANDLE.is_some() { return true; }
        let sockets = match SOCKETS.as_mut() {
            Some(s) => s,
            None => return false,
        };
        let socket = smoltcp::socket::dhcpv4::Socket::new();
        let handle = sockets.add(socket);
        DHCP_HANDLE = Some(handle);
        crate::platform::log("[net] DHCP client started\n");
        true
    }
}

/// Consume pending DHCP events; apply a lease when one lands.
unsafe fn dhcp_process(iface: &mut Interface, sockets: &mut SocketSet<'static>) {
    use smoltcp::socket::dhcpv4;
    let handle = match DHCP_HANDLE {
        Some(h) => h,
        None => return,
    };
    let event = sockets.get_mut::<dhcpv4::Socket>(handle).poll();
    match event {
        None => {}
        Some(dhcpv4::Event::Configured(config)) => {
            crate::platform::log("[net] DHCP lease: ");
            log_ipv4(&config.address.address());
            crate::platform::log("/");
            crate::platform::log_num(config.address.prefix_len() as u64);
            iface.update_ip_addrs(|addrs| {
                addrs.clear();
                let _ = addrs.push(IpCidr::Ipv4(config.address));
            });
            if let Some(router) = config.router {
                let _ = iface.routes_mut().add_default_ipv4_route(router);
                crate::platform::log(" via ");
                log_ipv4(&router);
            }
            if let Some(dns) = config.dns_servers.first() {
                DNS_SERVER_IP = *dns;
                crate::platform::log(" dns ");
                log_ipv4(dns);
            }
            crate::platform::log("\n");
        }
        Some(dhcpv4::Event::Deconfigured) => {
            // Lease lost — keep the last config (better a stale address
            // than none on a link this simple); the client renegotiates.
            crate::platform::log("[net] DHCP deconfigured (keeping last config)\n");
        }
    }
}

/// Has [`init`] completed successfully?
pub fn is_initialized() -> bool {
    unsafe { INITIALIZED }
}

// ============================================================================
// Crate-internal accessors for the static net state.
//
// Used by `super::tcp` (and future `super::dns`) to add sockets / fetch
// the Interface's Context. Not `pub` outside `super::` — callers should
// go through the high-level wrappers (`TcpStream`, etc.) rather than
// touch the singletons directly.
// ============================================================================

pub(super) unsafe fn sockets_mut() -> Option<&'static mut SocketSet<'static>> {
    SOCKETS.as_mut()
}

pub(super) unsafe fn iface_mut() -> Option<&'static mut Interface> {
    IFACE.as_mut()
}

// ============================================================================
// Helpers
// ============================================================================

/// Log an IPv4 address without pulling in `alloc::format!`.
fn log_ipv4(addr: &Ipv4Address) {
    let bytes = addr.as_bytes();
    crate::platform::log_num(bytes[0] as u64);
    crate::platform::log(".");
    crate::platform::log_num(bytes[1] as u64);
    crate::platform::log(".");
    crate::platform::log_num(bytes[2] as u64);
    crate::platform::log(".");
    crate::platform::log_num(bytes[3] as u64);
}
