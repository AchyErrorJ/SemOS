//! Minimal TCP client wrapper over smoltcp's `tcp::Socket`.
//!
//! v1 supports one in-flight TCP connection at a time — that matches
//! the LLM-API use case (the kernel makes a single HTTPS call per chat
//! turn). Per the brief, rx buffer is 16 KiB (max TLS record) and tx
//! buffer is 8 KiB (well over our HTTP request size).
//!
//! # Usage
//!
//! ```ignore
//! let mut stream = TcpStream::connect(Ipv4Address::new(10,0,2,2), 1234)?;
//! while stream.state() == State::SynSent { net::poll(); }
//! match stream.state() {
//!     State::Established => { /* read/write */ }
//!     State::Closed      => { /* peer reset */ }
//!     _ => { /* still negotiating */ }
//! }
//! stream.close();
//! ```
//!
//! # What this does NOT do (yet)
//!
//! - No `embedded_io::Read + Write` impl. Direct API only for v1; the
//!   trait impls land when we vendor embedded-tls (which needs them).
//! - No automatic background polling. Callers must drive
//!   [`super::poll`] between operations until the desired state is
//!   reached. A scheduler-tick-driven poller is a follow-up.
//! - No connection reuse. Once `close()` is called the buffers are
//!   freed back to the static pool; a fresh `connect()` is required
//!   for the next request.

use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint, Ipv4Address};

// ============================================================================
// Static per-socket storage
// ============================================================================

/// TCP receive buffer. Sized for a full TLS record (16 KiB) plus headroom.
const TCP_RX_SIZE: usize = 16 * 1024;

/// TCP transmit buffer. The Phase 8 HTTP request is ≤ 4 KiB; doubled
/// for headroom against handshake message bursts.
const TCP_TX_SIZE: usize = 8 * 1024;

static mut TCP_RX_BUF: [u8; TCP_RX_SIZE] = [0; TCP_RX_SIZE];
static mut TCP_TX_BUF: [u8; TCP_TX_SIZE] = [0; TCP_TX_SIZE];

/// Base of the ephemeral local-port range. RFC 6335 reserves 49152-65535
/// for ephemeral use.
const LOCAL_PORT_BASE: u16 = 49152;
const LOCAL_PORT_TOP: u16 = 65000;

/// Rotating ephemeral local port. We only have one outbound connection in
/// flight at a time, but we must NOT reuse the same local port across
/// back-to-back connections: the SLIRP host NAT (and the peer) keep the prior
/// 4-tuple in TIME_WAIT, so a fresh SYN from an identical (local,remote) tuple
/// is dropped and the connect never establishes — it hangs in
/// `poll_to_terminal` until the timeout. Handing out a new local port each
/// connect gives every connection a distinct 4-tuple. (This is what real
/// ephemeral-port allocation does, and it's why the 3rd reconnect in a boot
/// — DEMO 16 → 48 → 49 — was the one that hung.)
static mut NEXT_LOCAL_PORT: u16 = LOCAL_PORT_BASE;

/// Hand out the next ephemeral local port, wrapping within the range.
unsafe fn next_local_port() -> u16 {
    let p = NEXT_LOCAL_PORT;
    NEXT_LOCAL_PORT = if NEXT_LOCAL_PORT >= LOCAL_PORT_TOP {
        LOCAL_PORT_BASE
    } else {
        NEXT_LOCAL_PORT + 1
    };
    p
}

/// Single-socket guard. `connect` sets this; `close` clears it.
static mut SOCKET_IN_USE: bool = false;

// ============================================================================
// Errors
// ============================================================================

/// Failure modes for the TCP layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpError {
    /// `net::init` hasn't run, or its prerequisites failed.
    NotInitialized,
    /// A TcpStream already exists; close it before opening another.
    SocketBusy,
    /// smoltcp rejected the connect call (bad address/port or socket in wrong state).
    ConnectFailed,
    /// Socket isn't in a state that allows the requested I/O.
    NotConnected,
    /// Peer half-closed; no more data to read.
    Eof,
}

// ============================================================================
// TcpStream
// ============================================================================

/// Handle to a TCP socket living in the kernel's smoltcp SocketSet.
///
/// Owns the static rx/tx buffer reservation for the lifetime of the
/// connection — only one TcpStream can exist at a time. Drop / explicit
/// close releases the reservation.
pub struct TcpStream {
    handle: SocketHandle,
}

impl TcpStream {
    /// Open a TCP connection to `(remote_ip, remote_port)`. Returns
    /// immediately after queueing the SYN; the connection is not yet
    /// established. Caller drives [`super::poll`] until [`Self::state`]
    /// reaches `Established` (success) or `Closed` (rejection or RST).
    pub fn connect(remote_ip: Ipv4Address, remote_port: u16) -> Result<Self, TcpError> {
        unsafe {
            if SOCKET_IN_USE {
                return Err(TcpError::SocketBusy);
            }
            if !super::state::is_initialized() {
                return Err(TcpError::NotInitialized);
            }

            // Build a TCP socket from the static buffers.
            // SocketBuffer::new takes a `&'static mut [u8]` (via the
            // ManagedSlice::Borrowed path — no alloc needed).
            let rx_storage: &'static mut [u8] = &mut *core::ptr::addr_of_mut!(TCP_RX_BUF);
            let tx_storage: &'static mut [u8] = &mut *core::ptr::addr_of_mut!(TCP_TX_BUF);
            let rx_buf = tcp::SocketBuffer::new(rx_storage);
            let tx_buf = tcp::SocketBuffer::new(tx_storage);
            let socket = tcp::Socket::new(rx_buf, tx_buf);

            // Add to the SocketSet, get a handle.
            let sockets = match super::state::sockets_mut() {
                Some(s) => s,
                None => return Err(TcpError::NotInitialized),
            };
            let handle = sockets.add(socket);

            // Issue the connect. Needs the Interface's Context, which is
            // distinct from the SocketSet so disjoint borrows are fine.
            let iface = match super::state::iface_mut() {
                Some(i) => i,
                None => return Err(TcpError::NotInitialized),
            };
            let cx = iface.context();
            let socket = sockets.get_mut::<tcp::Socket>(handle);
            let remote_endpoint = IpEndpoint::new(IpAddress::Ipv4(remote_ip), remote_port);
            let local_endpoint = IpListenEndpoint { addr: None, port: next_local_port() };
            if socket.connect(cx, remote_endpoint, local_endpoint).is_err() {
                // Roll back: remove the socket so we don't leak it.
                sockets.remove(handle);
                return Err(TcpError::ConnectFailed);
            }

            SOCKET_IN_USE = true;
            Ok(TcpStream { handle })
        }
    }

    /// Current TCP state. Cheap query — useful in poll loops to detect
    /// when SYN-SENT advances to ESTABLISHED or back to CLOSED.
    pub fn state(&self) -> tcp::State {
        unsafe {
            let sockets = match super::state::sockets_mut() {
                Some(s) => s,
                None => return tcp::State::Closed,
            };
            sockets.get_mut::<tcp::Socket>(self.handle).state()
        }
    }

    /// Has the connection completed the handshake?
    pub fn is_established(&self) -> bool { self.state() == tcp::State::Established }

    /// Has the connection torn down (either side)?
    pub fn is_closed(&self) -> bool { self.state() == tcp::State::Closed }

    /// Pull up to `buf.len()` bytes from the rx ring. Returns `Ok(n)`
    /// where `n` may be 0 if nothing's ready right now (caller polls and
    /// retries). Returns `Err(Eof)` if the peer closed cleanly.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, TcpError> {
        unsafe {
            let sockets = match super::state::sockets_mut() {
                Some(s) => s,
                None => return Err(TcpError::NotInitialized),
            };
            let socket = sockets.get_mut::<tcp::Socket>(self.handle);
            if !socket.may_recv() {
                // may_recv is false after the peer FINs us.
                return Err(TcpError::Eof);
            }
            if !socket.can_recv() {
                return Ok(0);
            }
            socket.recv_slice(buf).map_err(|_| TcpError::NotConnected)
        }
    }

    /// Push up to `buf.len()` bytes into the tx ring. Returns `Ok(n)`
    /// — `n` may be less than `buf.len()` if the ring is filling.
    /// Caller polls and retries with the remainder.
    pub fn write(&mut self, buf: &[u8]) -> Result<usize, TcpError> {
        unsafe {
            let sockets = match super::state::sockets_mut() {
                Some(s) => s,
                None => return Err(TcpError::NotInitialized),
            };
            let socket = sockets.get_mut::<tcp::Socket>(self.handle);
            if !socket.may_send() {
                return Err(TcpError::NotConnected);
            }
            if !socket.can_send() {
                return Ok(0);
            }
            socket.send_slice(buf).map_err(|_| TcpError::NotConnected)
        }
    }

    /// Initiate clean shutdown (sends FIN). Caller still needs to poll
    /// until the socket fully drains and returns to CLOSED, then drop
    /// the handle (or let it go out of scope).
    pub fn close(&mut self) {
        unsafe {
            if let Some(sockets) = super::state::sockets_mut() {
                sockets.get_mut::<tcp::Socket>(self.handle).close();
            }
        }
    }

    /// Forcibly tear down the socket and release the buffer reservation.
    /// Use when you've reached a terminal state (CLOSED, the peer RST'd,
    /// etc.) and want the slot free for the next connect. The actual
    /// removal happens in `Drop` when `self` falls out of scope here, so
    /// the socket is never removed twice.
    pub fn release(self) {
        // Drop does the work.
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        // Fully remove the smoltcp socket from the SocketSet (not just the
        // SOCKET_IN_USE reservation). Leaving it behind leaked a socket bound
        // to the fixed LOCAL_PORT, so a *second* connect (e.g. the agent
        // reconnecting after DEMO 16) added a colliding socket and never
        // established — the connect hung. Removing it gives the next connect a
        // clean slate. Single-socket model: exactly one TcpStream owns `handle`,
        // so this removes it exactly once.
        unsafe {
            if let Some(sockets) = super::state::sockets_mut() {
                sockets.remove(self.handle);
            }
            SOCKET_IN_USE = false;
        }
    }
}

// ============================================================================
// embedded_io::{Read, Write} blocking impls.
//
// embedded-tls drives the transport through these traits, so we have to
// satisfy the blocking semantics: "if no bytes are currently available
// to read, this function blocks until at least one byte is available."
// Our inherent read/write are non-blocking (return Ok(0) when not ready);
// the trait impls wrap them in a poll loop until the socket is ready or
// the connection terminates.
// ============================================================================

impl embedded_io::Error for TcpError {
    fn kind(&self) -> embedded_io::ErrorKind {
        use embedded_io::ErrorKind;
        match self {
            TcpError::NotInitialized => ErrorKind::NotConnected,
            TcpError::SocketBusy     => ErrorKind::AlreadyExists,
            TcpError::ConnectFailed  => ErrorKind::ConnectionRefused,
            TcpError::NotConnected   => ErrorKind::NotConnected,
            // Eof is mapped to ConnectionAborted because embedded_io has
            // no dedicated "graceful close" variant; the read-returns-0
            // convention of std::io::Read covers it, which we honour by
            // returning Ok(0) at EOF (per the trait's "read of length 0
            // means EOF" contract).
            TcpError::Eof            => ErrorKind::ConnectionAborted,
        }
    }
}

// embedded_io 0.7 added a `core::error::Error` supertrait on
// `embedded_io::Error`. That needs Display in turn. We have nothing
// interesting to say beyond the Debug repr, so the Display impl
// just defers to it.
impl core::fmt::Display for TcpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl core::error::Error for TcpError {}

impl embedded_io::ErrorType for TcpStream {
    type Error = TcpError;
}

/// Max wall-clock ticks an embedded-io read/write will spin with no progress
/// before giving up. 100 Hz APIC → 3000 ticks = 30 s. A live peer makes
/// progress within an RTT, so this only fires when the peer is silent — e.g. a
/// non-TLS service that accepted the TCP connect but never speaks (the
/// SLIRP-port-1 case in DEMO 15). Without it the TLS handshake's blocking read
/// of ServerHello spins forever and hangs the boot. On timeout we report EOF
/// (read) / NotConnected (write) so the handshake fails *cleanly*.
///
/// 30 s (not 10) because an LLM endpoint's time-to-first-byte is legitimately
/// long: when the agent asks Claude to *generate* a reply (e.g. summarize after
/// a tool call), the server may think for >10 s before the first token. A 10 s
/// cap reported a premature EOF mid-conversation and failed the turn. 30 s
/// still bounds the dead-peer case at a tolerable worst case.
const IO_IDLE_TIMEOUT_TICKS: u64 = 3000;

impl embedded_io::Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TcpError> {
        if buf.is_empty() { return Ok(0); }
        // A spinning network wait MUST let the timer IRQ fire, or `ticks()`
        // freezes and the idle-timeout below can never trip — the recv-stall
        // hang. Ensure interrupts are enabled before we start spinning.
        crate::platform::get().enable_interrupts();
        let start = crate::platform::ticks();
        loop {
            super::state::poll();
            match TcpStream::read(self, buf) {
                Ok(n) if n > 0 => return Ok(n),
                Ok(_) => {
                    // No data ready yet. Check whether we should give up:
                    // peer FIN (Eof) is the clean half-close case; if the
                    // socket has reached Closed without seeing FIN it's
                    // an error from our side.
                    if self.is_closed() {
                        // embedded_io's convention: read of 0 == EOF.
                        return Ok(0);
                    }
                    // Idle-timeout guard: a silent peer must not hang us. This
                    // is wall-clock (tick) based, which only works because the
                    // `enable_interrupts()` above keeps the timer IRQ firing
                    // while we spin (a disabled-IF spin froze ticks → the
                    // recv-stall hang, root-caused 2026-05-25).
                    if crate::platform::ticks().wrapping_sub(start) >= IO_IDLE_TIMEOUT_TICKS {
                        return Ok(0);
                    }
                    core::hint::spin_loop();
                }
                Err(TcpError::Eof) => return Ok(0),
                Err(e) => return Err(e),
            }
        }
    }
}

impl embedded_io::Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> Result<usize, TcpError> {
        if buf.is_empty() { return Ok(0); }
        crate::platform::get().enable_interrupts(); // keep the timer alive while spinning
        let start = crate::platform::ticks();
        loop {
            super::state::poll();
            match TcpStream::write(self, buf) {
                Ok(n) if n > 0 => return Ok(n),
                Ok(_) => {
                    if self.is_closed() {
                        return Err(TcpError::NotConnected);
                    }
                    if crate::platform::ticks().wrapping_sub(start) >= IO_IDLE_TIMEOUT_TICKS {
                        return Err(TcpError::NotConnected);
                    }
                    core::hint::spin_loop();
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn flush(&mut self) -> Result<(), TcpError> {
        // smoltcp flushes whatever is in the tx ring on each poll. We
        // poll a few times so that anything just written has a chance
        // to actually leave the device.
        for _ in 0..4 { super::state::poll(); }
        Ok(())
    }
}
