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
pub mod dns;
pub mod http;
pub mod state;
pub mod tcp;

pub use http::{decode_chunked, is_chunked, ChunkedError};

pub use state::{init, poll, is_initialized};
pub use tcp::{TcpStream, TcpError};
pub use dns::resolve;

// Re-export smoltcp address / state types the user-facing demos need,
// so platform crates (e.g. kernel-x86_64) don't have to pull smoltcp
// as their own dependency just to spell out a constant.
pub use smoltcp::wire::Ipv4Address;
pub use smoltcp::socket::tcp::State as TcpState;
