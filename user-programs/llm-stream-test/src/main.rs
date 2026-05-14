#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

// Syscall numbers - from kernel-core/src/syscall/mod.rs
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 60;
const SYS_LLM_STREAM_START: u64 = 55;
const SYS_LLM_STREAM_READ: u64 = 56;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Print test banner
    let banner = b"LLM Streaming Test: Starting request...\n";
    syscall3(SYS_WRITE, 1, banner.as_ptr() as u64, banner.len() as u64);

    // Start streaming LLM request
    let prompt = b"explain semantic operating systems";
    let request_id = syscall3(SYS_LLM_STREAM_START,
        prompt.as_ptr() as u64,
        prompt.len() as u64,
        0); // No context for this test

    if request_id == u64::MAX {
        let error = b"ERROR: Failed to start LLM stream\n";
        syscall3(SYS_WRITE, 1, error.as_ptr() as u64, error.len() as u64);
        syscall1(SYS_EXIT, 1);
    }

    // Print request ID
    let msg = b"Stream request started, polling for response...\n";
    syscall3(SYS_WRITE, 1, msg.as_ptr() as u64, msg.len() as u64);

    // Poll for response
    let mut buffer = [0u8; 512];
    let mut poll_count = 0;

    loop {
        let result = syscall3(SYS_LLM_STREAM_READ,
            request_id,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64);

        match result {
            u64::MAX => {
                // Error
                let error = b"ERROR: Stream read failed\n";
                syscall3(SYS_WRITE, 1, error.as_ptr() as u64, error.len() as u64);
                break;
            },
            val if val == u64::MAX - 1 => {
                // Still processing
                poll_count += 1;
                if poll_count % 1000000 == 0 {
                    let waiting = b"[waiting for LLM response...]\n";
                    syscall3(SYS_WRITE, 1, waiting.as_ptr() as u64, waiting.len() as u64);
                }
                continue;
            },
            val if val == u64::MAX - 2 => {
                // Cancelled
                let cancelled = b"Stream was cancelled\n";
                syscall3(SYS_WRITE, 1, cancelled.as_ptr() as u64, cancelled.len() as u64);
                break;
            },
            0 => {
                // Complete, no more data
                let complete = b"Stream complete!\n";
                syscall3(SYS_WRITE, 1, complete.as_ptr() as u64, complete.len() as u64);
                break;
            },
            bytes_read => {
                // Got data
                let header = b"LLM Response: ";
                syscall3(SYS_WRITE, 1, header.as_ptr() as u64, header.len() as u64);
                syscall3(SYS_WRITE, 1, buffer.as_ptr() as u64, bytes_read);
                let newline = b"\n";
                syscall3(SYS_WRITE, 1, newline.as_ptr() as u64, newline.len() as u64);
                break; // For now, just read one chunk
            }
        }
    }

    let done = b"LLM streaming test complete!\n";
    syscall3(SYS_WRITE, 1, done.as_ptr() as u64, done.len() as u64);
    syscall1(SYS_EXIT, 0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    syscall1(SYS_EXIT, 2);
}

#[inline]
fn syscall1(n: u64, a1: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "syscall",
            in("rax") n,
            in("rdi") a1,
            out("rcx") _,
            out("r11") _,
            lateout("rax") ret,
            options(nostack, preserves_flags),
        );
    }
    ret
}

#[inline]
fn syscall3(n: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "syscall",
            in("rax") n,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            out("rcx") _,
            out("r11") _,
            lateout("rax") ret,
            options(nostack, preserves_flags),
        );
    }
    ret
}