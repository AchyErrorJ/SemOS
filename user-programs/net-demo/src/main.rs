//! net-demo.elf — Phase 14 M25 `std::net` acceptance test.
//!
//! A Ring-3 program that uses `semos_std::net` to resolve a hostname,
//! open a TCP connection, send a plaintext HTTP/1.1 request, and read the
//! response — all over the kernel's smoltcp stack via the SYS_DNS_RESOLVE
//! + SYS_TCP_{CONNECT,READ,WRITE,CLOSE} syscalls. First time a Ring-3
//! program drives the network directly.
//!
//! Built at opt-level=0 (task #54). Avoids numeric `{}` formatting in
//! output (a separate codegen sensitivity); signals via exit codes.
//!
//! Needs `-netdev user` (SLIRP) to reach the internet; the kernel's
//! DEMO 36 harness skips this when the net stack isn't up.
//!
//! Exit codes (read by the kernel in DEMO 36):
//!   0     — full pass (resolved + connected + sent + got an HTTP response)
//!   0x61  — connect failed (resolve or handshake)
//!   0x62  — write failed
//!   0x63  — response didn't start with "HTTP/"

#![no_std]
#![no_main]

use semos_std::{main, println};
use semos_std::net::TcpStream;
use semos_std::io::{Read, Write};
use semos_std::vec::Vec;

main!(fn main() {
    println!("net-demo: started");

    // Resolve + connect in one step (host:port).
    let mut stream = match TcpStream::connect("example.com", 80) {
        Ok(s) => s,
        Err(_) => {
            println!("net-demo: FAIL connect (resolve or handshake)");
            semos_std::process::exit(0x61);
        }
    };
    println!("net-demo: connected to example.com:80");

    // Minimal HTTP/1.1 GET. `Connection: close` makes the server send the
    // response then FIN, so our read loop sees a clean EOF.
    let req = b"GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n";
    if stream.write_all(req).is_err() {
        println!("net-demo: FAIL write");
        semos_std::process::exit(0x62);
    }
    println!("net-demo: sent HTTP GET");

    // Read until the peer closes (bounded so a misbehaving server can't
    // wedge us). `read` returns 0 on clean EOF.
    let mut body = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                body.extend_from_slice(&chunk[..n]);
                if body.len() > 4096 { break; }
            }
            Err(_) => break,
        }
    }

    if body.starts_with(b"HTTP/") {
        println!("net-demo: PASS received HTTP response over Ring-3 TcpStream");
    } else {
        println!("net-demo: FAIL no HTTP response");
        semos_std::process::exit(0x63);
    }

    println!("net-demo: ALL CHECKS PASSED");
});
