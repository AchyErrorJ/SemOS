//! std-demo.elf — Phase 14 M25 #52 acceptance test.
//!
//! Validates the load-bearing piece of the std-shim's threading: a
//! `thread::spawn` whose closure computes and *returns* a value, then
//! `JoinHandle<T>::join` recovers it. This exercises the boxed-closure
//! payload on the bump-allocator heap (SYS_MMAP_ANON), the
//! SYS_THREAD_SPAWN/JOIN round-trip, and typed result delivery.
//!
//! The shim also provides (compiled + dev-build-validated) sync::
//! {Mutex, Once} over the kernel futex, fs::{File, OpenOptions} +
//! io::{Read, Write}, env::{args, var}. Those, plus number formatting,
//! are held out of the always-on demo: std-shim binaries past a
//! certain size trip a Ring-3 stack/loader memory-corruption bug
//! (task #53, same structural family as #41) that shifts with link
//! layout. The single-thread path is robust across every layout.
//!
//! Exit codes (read by the kernel in DEMO 31):
//!   0     — pass
//!   0x43  — thread join returned the wrong value

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
        Ok(5050) => println!("std-demo: PASS thread::spawn + JoinHandle<u64>::join returned 5050"),
        _ => {
            println!("std-demo: FAIL thread join");
            semos_std::process::exit(0x43);
        }
    }

    println!("std-demo: ALL CHECKS PASSED");
});
