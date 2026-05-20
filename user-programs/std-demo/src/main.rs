//! std-demo.elf — Phase 14 M25 #51/#52 acceptance test.
//!
//! Full end-to-end validation of the std-shim: thread::spawn +
//! JoinHandle<T>, futex-backed sync::{Mutex, Once}, and fs::File +
//! io::{Read, Write} round-trips. Built at opt-level=0 (see task #54):
//! any optimization miscompiles the std-shim syscall path — at -Oz the
//! File/Mutex/Once paths corrupt, at -O1 those recover but SYS_FREAD
//! still returns garbage; only -O0 passes the whole surface.
//!
//! Avoids `{}`/`{:?}` number formatting in output (a separate codegen
//! sensitivity); checks compare internally and signal via exit codes.
//!
//! Exit codes (read by the kernel in DEMO 31):
//!   0     — full pass
//!   0x41  — fs round-trip mismatch
//!   0x43  — thread join returned the wrong value
//!   0x44  — Mutex counter wrong
//!   0x45  — Once ran more than once

#![no_std]
#![no_main]

use semos_std::{main, println};
use semos_std::sync::{Mutex, Once};
use semos_std::thread;
use semos_std::io::{Read, Write};
use semos_std::fs::{self, File};
use semos_std::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

main!(fn main() {
    println!("std-demo: started");

    // #52: thread::spawn + JoinHandle<T> returning a computed value.
    {
        let h = thread::spawn(|| {
            let mut sum = 0u64;
            for i in 1..=100u64 { sum += i; }
            sum
        });
        match h.join() {
            Ok(5050) => println!("std-demo: PASS thread::spawn + join returned 5050"),
            _ => { println!("std-demo: FAIL thread join"); semos_std::process::exit(0x43); }
        }
    }

    // #52: static Mutex shared across spawner + worker thread (futex).
    {
        static COUNTER: Mutex<u64> = Mutex::new(0);
        let h = thread::spawn(|| { for _ in 0..1000 { *COUNTER.lock() += 1; } });
        for _ in 0..1000 { *COUNTER.lock() += 1; }
        h.join().expect("join");
        if *COUNTER.lock() != 2000 {
            println!("std-demo: FAIL Mutex counter wrong");
            semos_std::process::exit(0x44);
        }
        println!("std-demo: PASS static Mutex<u64> shared count = 2000");
    }

    // #52: Once runs exactly once (futex).
    {
        static INIT: Once = Once::new();
        static RUNS: AtomicU32 = AtomicU32::new(0);
        for _ in 0..5 {
            INIT.call_once(|| { RUNS.fetch_add(1, Ordering::SeqCst); });
        }
        if RUNS.load(Ordering::SeqCst) != 1 {
            println!("std-demo: FAIL Once ran more than once");
            semos_std::process::exit(0x45);
        }
        println!("std-demo: PASS Once::call_once ran exactly once");
    }

    // #51: fs::File create + write_all, reopen + read_to_end round-trip.
    {
        let path = "/std-demo-scratch.txt";
        let payload = b"semos-std file I/O round-trip via Read/Write traits\n";
        {
            let mut f = File::create(path).expect("create");
            f.write_all(payload).expect("write_all");
        }
        let mut back = Vec::new();
        {
            let mut f = File::open(path).expect("open");
            f.read_to_end(&mut back).expect("read_to_end");
        }
        if back != payload {
            println!("std-demo: FAIL fs round-trip");
            semos_std::process::exit(0x41);
        }
        fs::remove_file(path).expect("remove");
        println!("std-demo: PASS fs::File write_all + read_to_end round-trip");
    }

    println!("std-demo: ALL CHECKS PASSED");
});
