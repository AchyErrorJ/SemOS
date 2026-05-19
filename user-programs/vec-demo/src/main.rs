//! vec-demo.elf — Phase 14 M25 Tier 2 acceptance test for the
//! GlobalAlloc → SYS_HEAP_ALLOC path.
//!
//! Exercises `Vec`, `String`, `Box`, and `format!` — all of which
//! depend on the semos-std crate's `#[global_allocator]` being wired
//! up correctly. If allocation works end-to-end, this prints a few
//! lines and exits 0; on failure (OOM, double-free, etc) the panic
//! handler installs exit code 1.
//!
//! Exit codes (read by the kernel in DEMO 30):
//!   0           — full pass
//!   1           — panic (allocator failure surfaces as panic_handler)
//!   0x30 + step — value-check failure at step N

#![no_std]
#![no_main]

use semos_std::{main, println};
use semos_std::vec::Vec;
use semos_std::string::{String, ToString};
use semos_std::boxed::Box;
use semos_std::format;

main!(fn main() {
    println!("vec-demo: started");

    // Step 1: Vec::with_capacity + push to validate growth + reallocation.
    let mut v: Vec<u32> = Vec::with_capacity(4);
    for i in 0..10u32 {
        v.push(i * i);
    }
    if v.len() != 10 || v[3] != 9 || v[9] != 81 {
        println!("vec-demo: FAIL Vec growth (len={}, [3]={}, [9]={})", v.len(), v[3], v[9]);
        semos_std::process::exit(0x31);
    }
    println!("vec-demo: PASS Vec<u32> push×10 (cap grew, indexing works)");

    // Step 2: String concatenation via format! (heap-allocates a buffer
    // and writes via fmt::Write — exercises the alloc layer + fmt).
    let name = "semos-std";
    let s: String = format!("Hello from {} v{}!", name, env!("CARGO_PKG_VERSION"));
    if !s.starts_with("Hello from semos-std") {
        println!("vec-demo: FAIL format! produced {:?}", s);
        semos_std::process::exit(0x32);
    }
    println!("vec-demo: PASS format!() produced \"{}\"", s);

    // Step 3: Box<T> — single allocation + Drop frees it on scope exit.
    {
        let boxed: Box<[u32; 4]> = Box::new([0xAAAA, 0xBBBB, 0xCCCC, 0xDDDD]);
        if boxed[0] != 0xAAAA || boxed[3] != 0xDDDD {
            println!("vec-demo: FAIL Box content");
            semos_std::process::exit(0x33);
        }
        // NOTE: decimal {} here, not {:#X}. The hex/alternate-form fmt
        // path currently faults under our ELF loader (RIP=0) — tracked
        // separately. Decimal exercises the same Box deref + fmt → Stdout
        // path and is sufficient to validate the allocator.
        println!("vec-demo: PASS Box<[u32; 4]> ({}..{})", boxed[0], boxed[3]);
    }

    // Step 4: realloc stress — push past initial capacity many times,
    // forcing multiple growth rounds. If realloc-by-alloc-and-copy
    // leaks anything the kernel heap would exhaust quickly; this is
    // implicitly tested by the process not panicking.
    let mut big: Vec<u8> = Vec::new();
    for i in 0..4096u32 {
        big.push((i & 0xFF) as u8);
    }
    if big.len() != 4096 || big[1000] != (1000 & 0xFF) as u8 {
        println!("vec-demo: FAIL big-Vec content");
        semos_std::process::exit(0x34);
    }
    println!("vec-demo: PASS Vec<u8>::push × 4096 (multi-grow realloc OK)");

    // Step 5: Strings own their buffer; explicit Drop on shadowing
    // releases it. If dealloc were broken (or aliased) the next
    // round would fault.
    for i in 0..32 {
        let _temp: String = i.to_string();
    }
    println!("vec-demo: PASS 32 transient Strings allocated + dropped");

    println!("vec-demo: ALL CHECKS PASSED");
});
