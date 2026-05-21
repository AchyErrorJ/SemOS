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
    syscall1, syscall2, syscall3,
};
use crate::io::{self, Read, Write};

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
    /// Connect to `ip:port`. Blocks until the handshake completes.
    pub fn connect_addr(ip: Ipv4Addr, port: u16) -> io::Result<TcpStream> {
        let fd = unsafe { syscall2(SYS_TCP_CONNECT, ip.to_be_u32(), port as u64) };
        if fd == u64::MAX {
            Err(io::Error::other())
        } else {
            Ok(TcpStream { fd })
        }
    }

    /// Resolve `host` then connect to `host:port`. The common case.
    pub fn connect(host: &str, port: u16) -> io::Result<TcpStream> {
        let ip = resolve(host).ok_or_else(io::Error::other)?;
        Self::connect_addr(ip, port)
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let r = unsafe {
            syscall3(SYS_TCP_READ, self.fd, buf.as_mut_ptr() as u64, buf.len() as u64)
        };
        if r == u64::MAX {
            Err(io::Error::other())
        } else {
            Ok(r as usize) // 0 == peer closed (EOF)
        }
    }
}

impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let r = unsafe {
            syscall3(SYS_TCP_WRITE, self.fd, buf.as_ptr() as u64, buf.len() as u64)
        };
        if r == u64::MAX {
            Err(io::Error::other())
        } else {
            Ok(r as usize)
        }
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        unsafe {
            let _ = syscall1(SYS_TCP_CLOSE, self.fd);
        }
    }
}
