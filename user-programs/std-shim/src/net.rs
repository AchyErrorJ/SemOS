//! `std::net`-shaped surface: `Ipv4Addr`, `resolve`, and `TcpStream`.
//!
//! M25. Backed by the kernel's smoltcp stack via SYS_DNS_RESOLVE +
//! SYS_TCP_{CONNECT,READ,WRITE,CLOSE}. The kernel has a single TCP socket
//! today, so only one `TcpStream` can be open at a time — `connect`
//! returns an error if one is already live. `read`/`write` are blocking
//! (the kernel polls the stack with a wall-clock budget) and implement
//! `io::{Read, Write}`, so `read_to_end` / `write_all` work unchanged.
//!
//! Not yet: `UdpSocket` (DNS is offered as a one-shot `resolve` instead),
//! IPv6, `TcpListener`, `ToSocketAddrs` genericity.

use crate::arch::{
    SYS_DNS_RESOLVE, SYS_TCP_CONNECT, SYS_TCP_READ, SYS_TCP_WRITE, SYS_TCP_CLOSE,
    SYS_TCP_STATE, SYS_SLEEP, NET_WOULDBLOCK,
    syscall1, syscall2, syscall3,
};
use crate::io::{self, Read, Write};

/// Per-call retry budget. The kernel net syscalls are non-blocking (one
/// poll + one try, #56); we drive the wait here, sleeping ~1 tick between
/// tries. ~600 tries × ~16 ms ≈ ~10 s — enough for a SLIRP→host round-trip
/// while still terminating if the peer never answers.
const NET_RETRY_BUDGET: u32 = 600;

/// Sleep one scheduler tick — yields to other tasks and lets wall-clock
/// (hence the host RTT and the kernel's per-syscall net::poll) advance.
#[inline]
fn yield_tick() {
    unsafe { let _ = syscall1(SYS_SLEEP, 1); }
}

/// A bare IPv4 address. Mirrors `std::net::Ipv4Addr`'s octet view.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ipv4Addr {
    octets: [u8; 4],
}

impl Ipv4Addr {
    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self { octets: [a, b, c, d] }
    }
    pub const fn octets(&self) -> [u8; 4] {
        self.octets
    }
    /// Pack to the kernel's big-endian u32 form (first octet in the MSB).
    fn to_be_u32(self) -> u64 {
        ((self.octets[0] as u64) << 24)
            | ((self.octets[1] as u64) << 16)
            | ((self.octets[2] as u64) << 8)
            | (self.octets[3] as u64)
    }
    fn from_be_u32(v: u64) -> Self {
        Self::new((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8)
    }
}

impl core::fmt::Display for Ipv4Addr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}.{}.{}", self.octets[0], self.octets[1], self.octets[2], self.octets[3])
    }
}

/// Resolve a hostname to an IPv4 address via the kernel DNS resolver.
/// Returns `None` on failure (no reply, NXDOMAIN, net stack down).
pub fn resolve(host: &str) -> Option<Ipv4Addr> {
    let r = unsafe {
        syscall2(SYS_DNS_RESOLVE, host.as_ptr() as u64, host.len() as u64)
    };
    if r == u64::MAX {
        None
    } else {
        Some(Ipv4Addr::from_be_u32(r))
    }
}

/// A TCP connection. Closes its kernel socket on Drop. Implements
/// `io::Read` + `io::Write`.
pub struct TcpStream {
    fd: u64,
}

impl TcpStream {
    /// Connect to `ip:port`. Queues the SYN (SYS_TCP_CONNECT is non-blocking)
    /// then drives the handshake from user space via SYS_TCP_STATE until the
    /// socket is established, refused/closed, or the budget runs out.
    pub fn connect_addr(ip: Ipv4Addr, port: u16) -> io::Result<TcpStream> {
        let fd = unsafe { syscall2(SYS_TCP_CONNECT, ip.to_be_u32(), port as u64) };
        if fd == u64::MAX {
            return Err(io::Error::other());
        }
        // Own the fd now so any early return closes the kernel socket on Drop.
        let stream = TcpStream { fd };
        for _ in 0..NET_RETRY_BUDGET {
            match unsafe { syscall1(SYS_TCP_STATE, fd) } {
                2 => return Ok(stream),           // established
                0 | u64::MAX => return Err(io::Error::other()), // closed / error
                _ => yield_tick(),                // still connecting
            }
        }
        Err(io::Error::other()) // handshake timed out
    }

    /// Resolve `host` then connect to `host:port`. The common case.
    pub fn connect(host: &str, port: u16) -> io::Result<TcpStream> {
        let ip = resolve(host).ok_or_else(io::Error::other)?;
        Self::connect_addr(ip, port)
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        for _ in 0..NET_RETRY_BUDGET {
            let r = unsafe {
                syscall3(SYS_TCP_READ, self.fd, buf.as_mut_ptr() as u64, buf.len() as u64)
            };
            if r == u64::MAX {
                return Err(io::Error::other());
            }
            if r == NET_WOULDBLOCK {
                yield_tick();
                continue;
            }
            return Ok(r as usize); // 0 == peer closed (EOF)
        }
        Ok(0) // no data within budget — surface as EOF
    }
}

impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for _ in 0..NET_RETRY_BUDGET {
            let r = unsafe {
                syscall3(SYS_TCP_WRITE, self.fd, buf.as_ptr() as u64, buf.len() as u64)
            };
            if r == u64::MAX {
                return Err(io::Error::other());
            }
            if r == NET_WOULDBLOCK {
                yield_tick();
                continue;
            }
            return Ok(r as usize);
        }
        Err(io::Error::other()) // tx ring stayed full
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        unsafe {
            let _ = syscall1(SYS_TCP_CLOSE, self.fd);
        }
    }
}
