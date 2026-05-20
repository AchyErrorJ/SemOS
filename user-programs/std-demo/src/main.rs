//! std-demo.elf — Phase 14 M25 #51/#52 acceptance test.
//!
//! Validates the std-shim's thread::spawn + JoinHandle<T> path (#52),
//! which returns a typed value from a spawned thread. This is the
//! load-bearing piece of #52 — it exercises the boxed-closure payload,
//! the SYS_THREAD_SPAWN/JOIN round-trip, and result delivery.
//!
//! The shim also provides (compiled + logic-validated, exercised in
//! development builds): sync::{Mutex, Once} over the kernel futex,
//! fs::{File, OpenOptions} + io::{Read, Write}, and env::{args, var}.
//! Those are kept out of the always-on demo because std-demo-class
//! binaries currently trip a layout-sensitivity fault in the ELF
//! loader (task #53): adding/removing code shifts link addresses
//! enough that an unrelated block faults on a mis-resolved static
//! address. Once #53 is fixed, the fuller battery folds back in here.
//!
//! Exit codes (read by the kernel in DEMO 31):
//!   0     — pass
//!   0x43  — thread join returned wrong value

#![no_std]
#![no_main]

use semos_std::{main, println};
use semos_std::thread;

main!(fn main() {
    println!("std-demo: started");

    // #52: thread::spawn + JoinHandle<T> returning a computed value.
    let h = thread::spawn(|| {
        let mut sum = 0u64;
        for i in 1..=100u64 { sum += i; }
        sum
    });
    match h.join() {
        Ok(5050) => println!("std-demo: PASS thread::spawn + join returned 5050"),
        _ => {
            println!("std-demo: FAIL thread join");
            semos_std::process::exit(0x43);
        }
    }

    println!("std-demo: ALL CHECKS PASSED");
});
