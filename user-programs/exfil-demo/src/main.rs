//! exfil-demo.elf — adversarial demo of PII exfiltration via the LLM channel.
//!
//! Each "attack" creates a Sensitive SemanticObject containing PII in
//! some obfuscated form, then asks the kernel for an LLM-bound view via
//! SYS_LLM_CONTEXT. The kernel applies its rule-based redactor (regex
//! patterns for email / SSN / credit card / phone / IP / API key). We
//! then scan the kernel's output for an attacker-chosen "leak indicator"
//! — a substring whose presence proves the obfuscated PII survived
//! unredacted.
//!
//! The point: rule-based redaction is a baseline, not a defense. With
//! a single Sensitive object and no special tooling, any of the obvious
//! obfuscations bypass it. The thesis-correct fix is an on-device
//! intent-aware model running in the same Ring 0 trust boundary, which
//! is the next phase of this kernel.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

// --- Syscall numbers ---
const SYS_WRITE:       u64 = 0;
const SYS_EXIT:        u64 = 2;
const SYS_SEM_CREATE:  u64 = 20;
const SYS_LLM_CONTEXT: u64 = 51;

// --- Attack table ----------------------------------------------------------

struct Attack {
    label:     &'static str,
    content:   &'static [u8],
    indicator: &'static [u8],   // substring whose presence == leak
    expected:  &'static str,    // human-readable expectation
}

const ATTACKS: &[Attack] = &[
    Attack {
        label:     "1. plain text PII (regex baseline)",
        content:   b"Sensitive: email=user@example.com card=4111-1111-1111-1111",
        indicator: b"user@example.com",
        expected:  "CAUGHT",
    },
    Attack {
        label:     "2. base64-encoded email",
        content:   b"Sensitive: pii_b64=dXNlckBleGFtcGxlLmNvbQ==",
        indicator: b"dXNlckBleGFtcGxlLmNvbQ==",
        expected:  "LEAKED",
    },
    Attack {
        label:     "3. bracket [at]/[dot] obfuscation",
        content:   b"Sensitive: contact=user[at]example[dot]com",
        indicator: b"user[at]example[dot]com",
        expected:  "LEAKED",
    },
    Attack {
        label:     "4. whitespace-split chars",
        content:   b"Sensitive: addr=u s e r @ e x a m p l e . c o m",
        indicator: b"u s e r @",
        expected:  "LEAKED",
    },
    Attack {
        label:     "5. reversed string",
        content:   b"Sensitive: rev=moc.elpmaxe@resu",
        indicator: b"moc.elpmaxe@resu",
        expected:  "LEAKED",
    },
    Attack {
        label:     "6. hex-encoded bytes",
        content:   b"Sensitive: hex=75736572406578616d706c652e636f6d",
        indicator: b"75736572406578616d706c652e636f6d",
        expected:  "LEAKED",
    },
    Attack {
        label:     "7. credit card with non-dash separators",
        content:   b"Sensitive: card=4111x1111x1111x1111",
        indicator: b"4111x1111x1111x1111",
        expected:  "LEAKED",
    },
];

// SUIDs for ATTACKS[i] and for the split-attack pair.
const SUID_BASE_HIGH: u64 = 0x1000_0000_0000_0E00;

// --- Static buffers (in .bss) ---
const CTX_BUF_SIZE: usize = 4096;
static mut CTX_BUF:    [u8; CTX_BUF_SIZE] = [0; CTX_BUF_SIZE];
static mut SUID_PAIRS: [(u64, u64); 2]    = [(0, 0); 2];

// --- _start ----------------------------------------------------------------

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start() -> ! {
    print(b"================================================================\n");
    print(b"  EXFIL DEMO: PII exfiltration attempts via the LLM channel\n");
    print(b"  Caller: Ring 3, tier 2 (Sensitive). Each attempt creates a\n");
    print(b"  Sensitive SemanticObject and asks the kernel for an LLM-bound\n");
    print(b"  view. The kernel applies rule-based redaction (regex). We\n");
    print(b"  scan the result for an attacker-chosen leak indicator.\n");
    print(b"================================================================\n");

    let mut caught = 0u32;
    let mut leaked = 0u32;

    // Single-object attacks.
    for (i, atk) in ATTACKS.iter().enumerate() {
        let suid_high = SUID_BASE_HIGH | (i as u64 + 1);
        let suid_low  = 0xCAFE_F00D_0000_0000 | (i as u64 + 1);
        let leaked_this = run_single(atk, suid_high, suid_low);
        if leaked_this { leaked += 1; } else { caught += 1; }
    }

    // Split-across-objects attack: each part alone is too short for the
    // email regex; in the LLM output they sit side-by-side and a
    // downstream model can trivially rejoin them.
    {
        print(b"\n  ATTACK 8. split across two Sensitive objects\n");
        let suid_a_h = SUID_BASE_HIGH | 0x8A;  let suid_a_l = 0xCAFE_F00D_0000_008A;
        let suid_b_h = SUID_BASE_HIGH | 0x8B;  let suid_b_l = 0xCAFE_F00D_0000_008B;
        let part_a = b"Sensitive: prefix=user@";
        let part_b = b"Sensitive: suffix=example.com";
        let create_ok =
            sem_create(suid_a_h, suid_a_l, 2, part_a) == 0 &&
            sem_create(suid_b_h, suid_b_l, 2, part_b) == 0;
        if !create_ok {
            print(b"    [exfil] sem_create failed\n");
        } else {
            unsafe {
                (*(&raw mut SUID_PAIRS))[0] = (suid_a_h, suid_a_l);
                (*(&raw mut SUID_PAIRS))[1] = (suid_b_h, suid_b_l);
            }
            let pairs_ptr = (&raw const SUID_PAIRS) as *const _ as u64;
            let ctx_ptr   = (&raw mut CTX_BUF) as *mut u8 as u64;
            let total = unsafe { sys3(SYS_LLM_CONTEXT, pairs_ptr, 2, ctx_ptr) };
            // Print the raw RAW lines and the LLM blob, then check.
            print(b"    RAW (a): "); print(part_a); print(b"\n");
            print(b"    RAW (b): "); print(part_b); print(b"\n");
            let buf = unsafe { &*(&raw const CTX_BUF) };
            // The output is a sequence of [u64 len][bytes]. Just scan
            // the whole buffer up to `total` for the indicators.
            let scan = if (total as usize) <= CTX_BUF_SIZE { &buf[..total as usize] } else { buf };
            let has_user_at  = contains(scan, b"user@");
            let has_example  = contains(scan, b"example.com");
            print(b"    LLM raw bytes (capped 256):\n      ");
            let preview = if scan.len() > 256 { &scan[..256] } else { scan };
            print_printable(preview);
            print(b"\n");
            if has_user_at && has_example {
                print(b"    LEAK?  YES - both halves of the email survive in the LLM channel\n");
                print(b"           (a downstream model trivially rejoins them)             [LEAKED]\n");
                leaked += 1;
            } else {
                print(b"    LEAK?  no - one or both halves were redacted away              [CAUGHT]\n");
                caught += 1;
            }
        }
    }

    // Summary.
    print(b"\n================================================================\n");
    print(b"  Summary: caught ");
    print_dec(caught as u64);
    print(b" / leaked ");
    print_dec(leaked as u64);
    print(b" of ");
    print_dec((caught + leaked) as u64);
    print(b" attempts\n");
    print(b"  -> Rule-based redaction handles the textbook case and loses to\n");
    print(b"     any obfuscation a basic adversary could try in a few minutes.\n");
    print(b"  -> A real on-device intent-aware model in Ring 0 is the next\n");
    print(b"     step for the kernel-level LLM-isolation thesis.\n");
    print(b"================================================================\n");

    unsafe { sys_exit(0) }
}

/// Run one single-object attack. Returns true if the indicator survived.
fn run_single(atk: &Attack, suid_high: u64, suid_low: u64) -> bool {
    print(b"\n  ATTACK ");
    print(atk.label.as_bytes());
    print(b"\n");

    // Create the Sensitive object.
    let rc = sem_create(suid_high, suid_low, 2 /* Sensitive */, atk.content);
    if rc == u64::MAX {
        print(b"    [exfil] sem_create failed\n");
        return false;
    }

    // Ask the kernel for the LLM-bound view.
    unsafe { (*(&raw mut SUID_PAIRS))[0] = (suid_high, suid_low); }
    let pairs_ptr = (&raw const SUID_PAIRS) as *const _ as u64;
    let ctx_ptr   = (&raw mut CTX_BUF) as *mut u8 as u64;
    let total = unsafe { sys3(SYS_LLM_CONTEXT, pairs_ptr, 1, ctx_ptr) };
    if total == u64::MAX || total < 8 {
        print(b"    [exfil] llm_context failed\n");
        return false;
    }

    // First entry: [u64 len][bytes]. Cap aggressively so a wild
    // entry_len (e.g., uninitialized memory because the kernel returned
    // before populating the header) can't dump kilobytes of NULs.
    let buf = unsafe { &*(&raw const CTX_BUF) };
    let mut len_bytes = [0u8; 8];
    len_bytes.copy_from_slice(&buf[0..8]);
    let entry_len_raw = u64::from_le_bytes(len_bytes) as usize;
    let total_usize = total as usize;
    // Trust whichever upper bound is tighter: the parsed length, the
    // total returned by the syscall (minus 8-byte header), or 1 KiB.
    let cap = total_usize.saturating_sub(8).min(1024);
    let entry_len = entry_len_raw.min(cap);
    let llm_view = &buf[8..8 + entry_len.min(CTX_BUF_SIZE - 8)];

    print(b"    RAW:   ");
    print(atk.content);
    print(b"\n    LLM:   ");
    print_printable(llm_view);
    print(b"\n");

    let leaked = contains(llm_view, atk.indicator);
    if leaked {
        print(b"    LEAK?  YES - indicator survived                                ");
    } else {
        print(b"    LEAK?  no  - indicator gone                                    ");
    }
    if leaked { print(b"[LEAKED]\n"); } else { print(b"[CAUGHT]\n"); }
    let _ = atk.expected; // suppress unused warning; expected is for documentation
    leaked
}

// --- helpers ---------------------------------------------------------------

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() { return true; }
    if needle.len() > haystack.len() { return false; }
    let last = haystack.len() - needle.len();
    for i in 0..=last {
        if &haystack[i..i + needle.len()] == needle {
            return true;
        }
    }
    false
}

/// Print only printable ASCII so the serial console doesn't get scrambled
/// by any control bytes in the LLM-context buffer.
fn print_printable(buf: &[u8]) {
    let mut start = 0usize;
    let mut i = 0;
    while i < buf.len() {
        let c = buf[i];
        let printable = (b' '..=b'~').contains(&c);
        if !printable {
            if i > start {
                print(&buf[start..i]);
            }
            print(b".");
            start = i + 1;
        }
        i += 1;
    }
    if start < buf.len() {
        print(&buf[start..]);
    }
}

fn sem_create(suid_high: u64, suid_low: u64, tier: u64, content: &[u8]) -> u64 {
    let info = (content.as_ptr() as u64 & 0xFFFF_FFFF)
             | ((content.len() as u64) << 32);
    unsafe { sys4(SYS_SEM_CREATE, suid_high, suid_low, tier, info) }
}

fn print(s: &[u8]) {
    unsafe { sys2(SYS_WRITE, s.as_ptr() as u64, s.len() as u64); }
}

fn print_dec(mut n: u64) {
    if n == 0 { print(b"0"); return; }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    print(&buf[i..]);
}

// --- syscall stubs ---------------------------------------------------------

#[inline(always)]
unsafe fn sys2(num: u64, a0: u64, a1: u64) -> u64 {
    let ret: u64;
    asm!(
        "syscall",
        in("rax") num, in("rdi") a0, in("rsi") a1,
        lateout("rax") ret,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    ret
}

#[inline(always)]
unsafe fn sys3(num: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let ret: u64;
    asm!(
        "syscall",
        in("rax") num, in("rdi") a0, in("rsi") a1, in("rdx") a2,
        lateout("rax") ret,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    ret
}

#[inline(always)]
unsafe fn sys4(num: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let ret: u64;
    asm!(
        "syscall",
        in("rax") num,
        in("rdi") a0, in("rsi") a1, in("rdx") a2, in("r10") a3,
        lateout("rax") ret,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    ret
}

#[inline(always)]
unsafe fn sys_exit(code: u64) -> ! {
    asm!(
        "syscall",
        in("rax") SYS_EXIT,
        in("rdi") code,
        options(nostack),
    );
    loop {}
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    unsafe { sys_exit(1) }
}
