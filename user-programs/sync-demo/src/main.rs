//! sync-demo.elf — functional smoke for semos_std::sync::Condvar +
//! semos_std::mpsc + semos_std::sync::RwLock.
//!
//! Run as DEMO 70: kernel SYS_SPAWNs this binary, waits, reads the exit
//! code via SYS_WAIT. Compile-validation (the types build cleanly) is
//! one signal; this is the live one (wakeups actually fire, ordering
//! holds, RwLock allows multiple readers).
//!
//! opt-level=0 only — task #54.
//!
//! Exit codes (read by DEMO 70):
//!   0     — all four sub-tests passed
//!   0x71  — Condvar wakeup didn't fire / state wrong on resume
//!   0x72  — mpsc value mismatch (wrong sum from 1..=5)
//!   0x73  — mpsc disconnect (RecvError) NOT delivered after sender dropped
//!   0x74  — RwLock couldn't hold two concurrent read guards / writer failed

#![no_std]
#![no_main]

use semos_std::mpsc::{self, RecvError};
use semos_std::sync::{Condvar, Mutex, RwLock};
use semos_std::thread;
use semos_std::{main, println};

main!(fn main() {
    println!("sync-demo: started");

    // ---- Test 1: Condvar wakeup ---------------------------------------
    // Main parks on CV.wait(); spawned thread sleeps briefly so main is
    // genuinely parked, then sets state + notifies. If wait returns and
    // state == 42 the futex-seq-counter path works.
    {
        static STATE: Mutex<u32> = Mutex::new(0);
        static CV: Condvar = Condvar::new();

        let h = thread::spawn(|| {
            // ~0.3 s @ 62 Hz — enough for main to park first.
            thread::sleep_ticks(20);
            let mut s = STATE.lock();
            *s = 42;
            CV.notify_one();
        });
        let mut s = STATE.lock();
        while *s == 0 {
            s = CV.wait(s);
        }
        let v = *s;
        drop(s);
        let _ = h.join();
        if v != 42 {
            println!("sync-demo: FAIL Condvar wakeup");
            semos_std::process::exit(0x71);
        }
        println!("sync-demo: PASS Condvar wakeup (state=42)");
    }

    // ---- Test 2: mpsc ordering + disconnect ---------------------------
    // Producer sends 1..=5 then drops its sender. Main also drops its
    // sender. After 5 successful recv()s, the next recv() MUST be
    // RecvError (all senders gone, queue drained).
    {
        let (tx, rx) = mpsc::channel::<u32>();
        let tx2 = tx.clone();
        let h = thread::spawn(move || {
            for i in 1..=5u32 {
                let _ = tx2.send(i);
            }
            drop(tx2);
        });
        drop(tx); // main's own sender goes away too
        let mut sum = 0u32;
        for _ in 0..5 {
            match rx.recv() {
                Ok(v) => sum = sum.saturating_add(v),
                Err(_) => {
                    println!("sync-demo: FAIL mpsc early disconnect");
                    semos_std::process::exit(0x72);
                }
            }
        }
        let _ = h.join();
        if sum != 15 {
            println!("sync-demo: FAIL mpsc sum");
            semos_std::process::exit(0x72);
        }
        match rx.recv() {
            Err(RecvError) => println!("sync-demo: PASS mpsc 1..=5 ordering + disconnect"),
            Ok(_) => {
                println!("sync-demo: FAIL extra value after disconnect");
                semos_std::process::exit(0x73);
            }
        }
    }

    // ---- Test 3: RwLock — two concurrent readers + a writer ----------
    // Holding two read guards simultaneously must not deadlock (would
    // mean the RwLock secretly only allows one reader). After they drop,
    // try_write must succeed; the new value must round-trip via read.
    {
        static LOCK: RwLock<u32> = RwLock::new(7);
        let r1 = LOCK.read();
        let r2 = LOCK.read();
        if *r1 != 7 || *r2 != 7 {
            println!("sync-demo: FAIL RwLock readers see wrong value");
            semos_std::process::exit(0x74);
        }
        drop(r1);
        drop(r2);
        match LOCK.try_write() {
            Some(mut w) => *w = 8,
            None => {
                println!("sync-demo: FAIL RwLock try_write blocked");
                semos_std::process::exit(0x74);
            }
        }
        if *LOCK.read() != 8 {
            println!("sync-demo: FAIL RwLock writer didn't persist");
            semos_std::process::exit(0x74);
        }
        println!("sync-demo: PASS RwLock 2 readers + writer");
    }

    println!("sync-demo: ALL PASS");
    semos_std::process::exit(0);
});
