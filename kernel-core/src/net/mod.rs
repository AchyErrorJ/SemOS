//! Network stack — Track B of Phase 8.
//!
//! Provides a smoltcp-backed Interface that sits on top of any
//! [`NetDevice`](crate::drivers::traits::NetDevice) (currently just the
//! virtio-net driver, but eventually iwlwifi too).
//!
//! This module's responsibility ends at "ARP + IPv4 packets work." TCP
//! sockets, DNS resolution, and the embedded-tls layer on top are
//! planned follow-ups per [`docs/PHASE_8_ROADMAP.md`] and
//! [`docs/SMOLTCP_VENDORING_BRIEF.md`].
//!
//! # Architecture
//!
//! ```text
//!  NetworkLlmProvider::complete()  (existing — uses loopback today)
//!         │
//!         ▼
//!  TlsConnection  (future, embedded-tls)
//!         │
//!         ▼
//!  smoltcp::socket::tcp::Socket  (future, TcpStream wrapper)
//!         │
//!         ▼
//!  smoltcp::iface::Interface     ← lives here, fed by poll() tick
//!         │
//!         ▼
//!  NetDeviceAdapter (phy::Device impl)
//!         │
//!         ▼
//!  kernel_core::drivers::NetDevice  (the trait, generic)
//!         │
//!         ▼
//!  virtio-net0 (or iwlwifi later)
//! ```

pub mod adapter;
pub mod clock;
pub mod state;

pub use state::{init, poll, is_initialized};
