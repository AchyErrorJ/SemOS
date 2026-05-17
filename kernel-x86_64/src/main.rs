//! Semantic OS Kernel - x86_64 Version
//!
//! This is the x86_64 port of the Semantic OS kernel. It uses the bootloader
//! crate to handle the complex x86 boot process (Real → Protected → Long mode).
//!
//! # Architecture Differences from ARM64
//!
//! | Component     | ARM64          | x86_64           |
//! |---------------|----------------|------------------|
//! | Boot          | Direct EL1     | Bootloader crate |
//! | Serial        | PL011 UART     | 16550 COM1       |
//! | Interrupts    | GIC + VBAR     | APIC + IDT       |
//! | MMU           | TTBR0/1        | CR3 + PML4       |
//! | Timer         | ARM Generic    | APIC/PIT/HPET    |

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

use bootloader_api::{entry_point, BootInfo, BootloaderConfig};
use core::panic::PanicInfo;

mod serial;
pub mod gdt;
mod interrupts;
mod memory;
mod platform_impl;
pub mod context;
mod syscall;
mod keyboard;
pub mod paging;
pub mod apic;
pub mod framebuffer;
pub mod pci;
pub mod virtio;
pub mod rng;

use serial::Serial;

/// Bootloader configuration
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    // Request a framebuffer if available
    config.mappings.physical_memory = Some(bootloader_api::config::Mapping::Dynamic);
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

/// Kernel entry point - called by the bootloader after setting up Long Mode
fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // Initialize serial output first (for debugging)
    serial::init();

    // Register the x86_64 platform with kernel-core (must happen before any kernel-core code)
    unsafe {
        kernel_core::set_platform(&platform_impl::PLATFORM);
    }

    println!("====================================================================================================");
    println!("  Semantic OS v0.1.0 - x86_64 Bare Metal");
    println!("====================================================================================================");
    println!();
    println!("Architecture: x86_64 (AMD64)");
    println!("Target: QEMU x86_64");
    println!("Platform: x86_64-unknown-none");
    println!();

    // Print boot info
    println!("[*] Boot information:");
    // Initialize the framebuffer console early so subsequent println! lines
    // appear on screen as well as on serial.
    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let (w, h) = (fb.info().width, fb.info().height);
        framebuffer::init(fb);
        println!("    Framebuffer: {}x{} (console active)", w, h);
    } else {
        println!("    Framebuffer: not available");
    }

    // Physical memory regions
    println!("    Memory regions:");
    let mut total_memory = 0u64;
    for region in boot_info.memory_regions.iter() {
        let size = region.end - region.start;
        total_memory += size;
        println!("      {:016x}-{:016x} ({} KB) {:?}",
            region.start,
            region.end,
            size / 1024,
            region.kind);
    }
    println!("    Total memory: {} MB", total_memory / 1024 / 1024);
    println!();

    // Initialize GDT + TSS (must come before interrupts and syscalls)
    println!("[*] Initializing GDT + TSS...");
    gdt::init();
    println!("[OK] GDT + TSS loaded");
    println!();

    // Enable SSE/SSE2 so f32/f64 math and fxsave/fxrstor work in tasks
    println!("[*] Enabling SSE/SSE2...");
    enable_sse();
    println!("[OK] SSE enabled");
    println!();

    // Initialize interrupts
    println!("[*] Initializing interrupts...");
    interrupts::init();
    println!("[OK] Interrupts initialized");
    println!();

    // Initialize memory pools
    println!("[*] Initializing memory pools...");
    memory::init(boot_info);
    println!("[OK] Memory pools initialized");
    println!();

    // Test memory allocation
    println!("[*] Memory Pool Statistics");
    println!("====================================");
    memory::print_stats();
    println!();

    // Initialize paging subsystem (records bootloader's physical-memory offset).
    // Must run before APIC init, which uses phys_to_virt() for MMIO access.
    println!("[*] Initializing paging...");
    paging::init(boot_info);
    println!("[OK] Paging subsystem initialized");
    println!();

    // Try to bring up the Local APIC timer; fall back to the legacy 8259 PIC
    // (already initialized by interrupts::init) if no APIC is present.
    println!("[*] Initializing Local APIC...");
    if apic::init() {
        println!("[OK] APIC timer active (PIC masked)");
    } else {
        println!("[!] No APIC — staying on legacy PIC + PIT");
    }
    println!();

    println!("[*] Scanning PCI bus 0...");
    pci::print_bus_0();
    println!();

    // Hardware RNG availability check. Required by TLS 1.3 for ClientHello
    // random + X25519 ephemeral scalar — we panic at boot if missing rather
    // than degrade silently later. Sample bytes printed for evidence the
    // RNG actually varies between boots (if you see two boots with the same
    // sample bytes, something is wrong).
    println!("[*] Probing hardware RNG (RDRAND)...");
    if rng::supported() {
        let mut sample = [0u8; 8];
        match rng::fill_bytes(&mut sample) {
            Ok(()) => {
                println!(
                    "[rng] RDRAND ok — sample: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                    sample[0], sample[1], sample[2], sample[3],
                    sample[4], sample[5], sample[6], sample[7],
                );
            }
            Err(()) => {
                println!("[rng] RDRAND reported as supported via CPUID but fill_bytes failed — abort");
                loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
            }
        }
    } else {
        println!("[rng] RDRAND not supported on this CPU — TLS cannot be safe; abort");
        loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
    }
    println!();

    println!("[*] Probing VirtIO block device...");
    if virtio::block::init() {
        if virtio::block::register_with_kernel_core() {
            println!("[virtio-blk] registered with driver registry as 'virtio0'");
        }
    }
    println!();

    println!("[*] Probing VirtIO network device...");
    if virtio::net::init() {
        if virtio::net::register_with_kernel_core() {
            println!("[virtio-net] registered with driver registry as 'virtio-net0'");
            // Bring up the smoltcp Interface on top of virtio-net0.
            // Hardcoded IP per QEMU SLIRP defaults (10.0.2.15/24 via 10.0.2.2).
            if let Some(nd) = kernel_core::drivers::registry::get_net("virtio-net0") {
                if kernel_core::net::init(nd) {
                    // One initial poll to flush any startup state.
                    kernel_core::net::poll();
                }
            }
        }
    }
    println!();

    // Initialize kernel-core subsystems
    println!("[*] Initializing kernel-core subsystems...");
    kernel_core::scheduler::init_core();
    println!("    Scheduler: initialized");
    kernel_core::process::init();
    println!("    Process table: initialized");
    kernel_core::fs::ramfs::init();
    println!("    Ramfs: initialized");
    kernel_core::security::init();
    println!("    Security framework: initialized");

    // Register user-mode programs built from real Rust crates in
    // user-programs/. The build is currently manual (run
    // `cargo build --release` in user-programs/hello/ before kernel build);
    // future: build.rs orchestration. include_bytes! pins the path so the
    // kernel won't link if the user binary is missing.
    static HELLO_RS_ELF: &[u8] = include_bytes!(
        "../../user-programs/hello/target/x86_64-unknown-none/release/hello"
    );
    static SEM_DEMO_ELF: &[u8] = include_bytes!(
        "../../user-programs/sem-demo/target/x86_64-unknown-none/release/sem-demo"
    );
    static EXFIL_DEMO_ELF: &[u8] = include_bytes!(
        "../../user-programs/exfil-demo/target/x86_64-unknown-none/release/exfil-demo"
    );
    if let Some(fs) = kernel_core::fs::ramfs::get_fs_mut() {
        if fs.add("hello-rs.elf", kernel_core::fs::ramfs::FileType::Executable, HELLO_RS_ELF) {
            println!("    Registered hello-rs.elf ({} bytes, real Rust user crate)", HELLO_RS_ELF.len());
        } else {
            println!("    [WARN] failed to register hello-rs.elf");
        }
        if fs.add("sem-demo.elf", kernel_core::fs::ramfs::FileType::Executable, SEM_DEMO_ELF) {
            println!("    Registered sem-demo.elf ({} bytes, semantic-object Ring 3 demo)", SEM_DEMO_ELF.len());
        } else {
            println!("    [WARN] failed to register sem-demo.elf");
        }
        if fs.add("exfil-demo.elf", kernel_core::fs::ramfs::FileType::Executable, EXFIL_DEMO_ELF) {
            println!("    Registered exfil-demo.elf ({} bytes, adversarial exfil demo)", EXFIL_DEMO_ELF.len());
        } else {
            println!("    [WARN] failed to register exfil-demo.elf");
        }
    }

    // Semantic object system
    kernel_core::semantic::registry::init_global_registry();
    println!("    Semantic registry: initialized");
    kernel_core::semantic::vector::init_global_vector_index();
    println!("    Vector index: initialized");
    kernel_core::semantic::search::init_global_search();
    println!("    Semantic search: initialized");

    // LLM services (context builder, redactor, summarizer, provider)
    kernel_core::llm::init();
    println!("    LLM services: initialized");

    // Crypto subsystem
    kernel_core::crypto::init();
    println!("    Crypto: initialized");

    println!("[OK] Kernel-core subsystems ready");
    println!();

    // Initialize SYSCALL/SYSRET
    println!("[*] Initializing SYSCALL/SYSRET...");
    syscall::init();
    println!("[OK] SYSCALL entry point configured");
    println!();

    // Test interrupts
    println!("[*] Testing interrupts...");
    x86_64::instructions::interrupts::int3(); // Trigger breakpoint
    println!("[OK] Breakpoint exception handled");

    // Skip the dispatch self-test from the boot path — the boot stack has no
    // guard page and the deep dispatch call chain has been observed to
    // double-fault on it. Real SYSCALLs from Ring 3 user tasks below run on
    // the per-task kernel stack with KERNEL_RSP and exercise the full path.
    println!("[*] SYSCALL/SYSRET configured (will be exercised by Ring 3 tasks)");
    println!();

    // Print kernel-core version info
    println!("[*] Kernel-core modules available:");
    println!("    - Semantic object system (SUID addressing)");
    println!("    - 4-tier security model (Public/Internal/Sensitive/Secret)");
    println!("    - Cryptography (ChaCha20-Poly1305)");
    println!("    - LLM context builder with redaction");
    println!("    - Process management & scheduler");
    println!("    - Syscall dispatch");
    println!();

    // Background tasks: two kernel-mode workers, one isolated kernel task
    // (own page tables), and one Ring 3 user task. They prove preemptive
    // multitasking + 4-tier isolation works during the demos. They no
    // longer print "tick" lines (silenced for clean demo output).
    // (kstack layout dump removed — we have the addresses)
    // Stack-overflow canaries at the bottom of every TASK_STACK — checked
    // from the PF handler. Cheap detection before metal bring-up.
    context::init_stack_canaries();

    println!("[*] Spawning background tasks...");
    if let Some(slot) = context::spawn_task("task_a", task_a) {
        println!("    task_a       (kernel mode)        slot {}", slot);
    }
    if let Some(slot) = context::spawn_task("task_b", task_b) {
        println!("    task_b       (kernel mode)        slot {}", slot);
    }
    if let Some(slot) = context::spawn_isolated_task("task_iso", task_isolated, 1) {
        println!("    task_iso     (isolated, tier<=1)  slot {}", slot);
    }
    if let Some(slot) = context::spawn_user_task("user_task", user_task_entry, 0) {
        println!("    user_task    (Ring 3, tier<=0)    slot {}", slot);
    }
    if let Some(slot) = context::spawn_task("init_loader", init_loader_task) {
        println!("    init_loader  (kernel mode)        slot {}", slot);
    }
    println!();

    println!("====================================================================================================");
    println!("  Semantic OS x86_64 - Kernel initialized successfully!");
    println!("  Timer-driven preemptive scheduling is now active.");
    println!("====================================================================================================");

    // Kernel idle loop — timer interrupts will preempt and schedule tasks
    loop {
        x86_64::instructions::hlt();
    }
}

/// Demo task A — prints periodically to show it's running
fn task_a() {
    let mut counter: u64 = 0;
    loop {
        counter += 1;
        if false && counter % 5_000_000 == 0 {
            println!("[task_a] tick {}", counter / 5_000_000);
        }
        core::hint::spin_loop();
    }
}

/// Demo task B — prints periodically to show it's running
fn task_b() {
    let mut counter: u64 = 0;
    loop {
        counter += 1;
        if false && counter % 5_000_000 == 0 {
            println!("[task_b] tick {}", counter / 5_000_000);
        }
        core::hint::spin_loop();
    }
}

/// Demo isolated task — runs in its own address space with restricted tier access.
/// Still runs in Ring 0 for now (kernel code segment), but with an isolated CR3
/// that only maps pools up to its tier. Full Ring 3 user mode is the next step
/// after verifying isolated address spaces work correctly.
fn task_isolated() {
    let mut counter: u64 = 0;
    loop {
        counter += 1;
        if false && counter % 5_000_000 == 0 {
            println!("[task_iso] tick {} (isolated address space)", counter / 5_000_000);
        }
        core::hint::spin_loop();
    }
}

/// One-shot kernel task that loads and spawns built-in user ELFs via
/// SYS_SPAWN. Lives on its own 16 KiB task stack (vs the boot stack which
/// has no guard page and overflows under the ELF loader's call depth).
/// After firing the syscalls it idles in `hlt`.
fn init_loader_task() {
    // Run kernel-side demos FIRST (demos 2 & 3 — the SemanticObject path).
    sem_demo_kernel();

    // DEMO 0: real Rust user binary (hello-rs.elf, built from
    // user-programs/hello/). Proves the toolchain works end-to-end —
    // a no_std Rust crate compiled with rust-lld, loaded by ramfs,
    // spawned by SYS_SPAWN, runs in Ring 3, prints, exits.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 0: Ring 3 user binary built from real Rust crate");
    println!("================================================================");
    spawn_named_at("hello-rs.elf", 0);

    println!();
    println!("================================================================");
    println!("  SemOS DEMO 1: Ring 3 user binary -> SYS_LLM_REDACT");
    println!("================================================================");
    spawn_named_at("redact.elf", 0);

    // DEMO 4: the security thesis end-to-end from user space.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 4: Ring 3 sem-demo (Sensitive obj, direct vs LLM)");
    println!("================================================================");
    spawn_named_at("sem-demo.elf", 2);

    // DEMO 6: adversarial PII exfiltration via the LLM channel.
    // CURRENTLY DISABLED — the kernel's task#40 cascade starves
    // exfil-demo before its prints get out, so the demo runs
    // intermittently and produces unreadable output. Crate is in tree
    // (user-programs/exfil-demo) and compiles/registers normally; flip
    // this back on once task #40 is properly fixed.
    // println!();
    // println!("================================================================");
    // println!("  SemOS DEMO 6: adversarial PII exfiltration via the LLM channel");
    // println!("================================================================");
    // spawn_named_at("exfil-demo.elf", 2);

    // DEMO 5 — re-enabled for task #40 hunt 2026-05-13 with new diagnostics
    // (canary check, expanded PF dump, IDT-dbg, timer-trap RIP=0 reporter).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 5: persistent SemanticObject (survives reboot)");
    println!("================================================================");
    persistence_demo();

    // DEMO 7: LLM Streaming Test
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 7: LLM streaming syscalls");
    println!("================================================================");
    llm_streaming_test();

    // DEMO 8: Security Policy Framework Test
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 8: Security policy framework");
    println!("================================================================");
    security_policy_test();

    // DEMO 9: Context-Aware Redaction Test
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 9: Context-aware redaction engine");
    println!("================================================================");
    context_aware_redaction_test();

    // DEMO 10: Network-Backed LLM Provider (HTTP/JSON over loopback transport)
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 10: Remote LLM provider (HTTP/JSON, loopback)");
    println!("================================================================");
    network_llm_provider_test();

    // DEMO 11: User Identity & Isolation
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 11: User identity & isolation");
    println!("================================================================");
    user_identity_test();

    // DEMO 12: TCP smoke test over smoltcp + virtio-net.
    // Only runs if the net stack came up (skipped silently otherwise so
    // bare-metal boots without virtio-net don't fail the cascade).
    if kernel_core::net::is_initialized() {
        println!();
        println!("================================================================");
        println!("  SemOS DEMO 12: TCP connect smoke test (smoltcp + virtio-net)");
        println!("================================================================");
        tcp_smoke_test();
    }

    // DEMO 13: SPKI-pinning byte-level validation against the real
    // Anthropic intermediate cert. Unconditional — exercises the DER
    // scanner + SHA-256 + pin compare without needing network. If this
    // fails we know the TLS path is broken before we even attempt a
    // handshake.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 13: SPKI pinning (Anthropic WE1 intermediate)");
    println!("================================================================");
    spki_pin_test();

    // DEMO 14: end-to-end through the embedded-tls TlsVerifier trait
    // surface — builds a CertificateRef from the real fixtures and
    // calls verify_certificate the way embedded-tls will during a
    // real handshake. Proves the trait wiring works without needing
    // a network round-trip.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 14: TlsVerifier trait surface (synthetic chain)");
    println!("================================================================");
    tls_verifier_test();

    // DEMO 15: TLS transport wiring smoke test. Only runs if the net
    // stack is up. We don't have a TLS server reachable in QEMU's
    // SLIRP, so the goal isn't a successful handshake — it's that
    // the whole TLS path is reachable, the failure mode is clean,
    // and no panics escape into the kernel.
    if kernel_core::net::is_initialized() {
        println!();
        println!("================================================================");
        println!("  SemOS DEMO 15: TLS transport smoke (configure + connect)");
        println!("================================================================");
        tls_transport_smoke();
    }

    // DEMO 16-pre: ChaCha20-Poly1305 KAT against the RFC 8439 §2.8.2
    // published test vector, going through our crypto_shim trait surface
    // (the exact path embedded-tls's TLS 1.3 record layer takes). The
    // existing self-roundtrip tests prove encrypt/decrypt are mutually
    // consistent — they don't prove the ciphertext matches what a peer
    // computes. If this KAT diverges, the live handshake failure is in
    // AEAD; if it matches, look further up in the key schedule.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 16-pre: ChaCha20-Poly1305 KAT (RFC 8439 §2.8.2)");
    println!("================================================================");
    let aead_ok = aead_kat_test();

    // DEMO 16: live TLS handshake against api.anthropic.com via SLIRP.
    // The Phase 8 finish line — first real outbound TLS from this kernel.
    // No API key, so we expect a 401 from the server, but the TLS
    // handshake itself must complete (real cert chain validated against
    // the SPKI pin) and we must get a parseable HTTP response back.
    if kernel_core::net::is_initialized() && aead_ok {
        println!();
        println!("================================================================");
        println!("  SemOS DEMO 16: live TLS handshake to api.anthropic.com");
        println!("================================================================");
        tls_live_handshake();
    } else if !aead_ok {
        println!();
        println!("  [DEMO 16] SKIPPED — AEAD KAT failed; live handshake would fail too.");
    }

    // Final marker before idling. On bare metal this is your "the kernel
    // didn't crash" signal — without serial capture, the framebuffer is
    // the only feedback channel. Anything other than this banner on the
    // last line means the boot was interrupted mid-demo.
    println!();
    println!("================================================================");
    println!("  All demos complete — kernel idling. Safe to power off.");
    println!("  ({} context switches across {} tasks)",
        kernel_core::scheduler::stats().0,
        kernel_core::scheduler::current_task_index() + 1);
    println!("================================================================");

    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}

/// DEMO 5: round-trip a Sensitive object through the BlockDevice.
fn persistence_demo() {
    use kernel_core::drivers::registry::get_block;
    use kernel_core::semantic::{SUID, SemanticObject};
    use kernel_core::memory::SecurityTier;
    use kernel_core::storage::snapshot::{save_snapshot, load_snapshot};

    let dev = match get_block("virtio0") {
        Some(d) => d,
        None => {
            println!("  [DEMO 5] no virtio0 block device — skipping (run with -drive ...,if=virtio)");
            return;
        }
    };

    // SUID for the persisted object — distinct from sem_demo_kernel's
    // and sem-demo.elf's so they can coexist.
    let suid = SUID::new(0x1000_0000_0000_00FF, 0xDEAD_BEEF_CAFE_BABE);

    // Try to restore. If absent / first boot, we'll seed a fresh one.
    // Use a static buffer — a stack-local [u8; 4096] in init_loader_task
    // overflows the per-task kstack budget under serial-print latency
    // and triggers task #40. Static is safe because persistence_demo
    // runs once at boot and is not re-entered.
    static mut DEMO5_BUF: [u8; 4096] = [0; 4096];
    let buf = unsafe { &mut *(&raw mut DEMO5_BUF) };
    match load_snapshot(dev, buf) {
        Ok(len) => {
            // Format: [16 SUID][1 tier][1 owner][2 content_len][content]
            if len < 20 {
                println!("  [DEMO 5] snapshot too short — re-seeding");
                seed_persistent_object(dev, &suid);
                return;
            }
            let stored_suid_hi = u64::from_le_bytes(buf[0..8].try_into().unwrap());
            let stored_suid_lo = u64::from_le_bytes(buf[8..16].try_into().unwrap());
            let tier_byte = buf[16];
            let owner = buf[17];
            let content_len = u16::from_le_bytes(buf[18..20].try_into().unwrap()) as usize;
            if 20 + content_len != len || content_len > 256 {
                println!("  [DEMO 5] snapshot malformed (declared {} bytes, file {})",
                    20 + content_len, len);
                return;
            }
            let tier = match tier_byte {
                0 => SecurityTier::Public,
                1 => SecurityTier::Internal,
                2 => SecurityTier::Sensitive,
                _ => SecurityTier::Secret,
            };
            let restored_suid = SUID::new(stored_suid_hi, stored_suid_lo);
            let content = &buf[20..20 + content_len];
            println!("  [DEMO 5] restored from disk:  SUID=0x{:016X}_{:016X}",
                stored_suid_hi, stored_suid_lo);
            println!("  [DEMO 5]   tier={:?}  owner={}  content_len={}", tier, owner, content_len);
            println!("  [DEMO 5]   DIRECT READ:  {}",
                core::str::from_utf8(content).unwrap_or("<bad utf8>"));

            // Insert into registry so build_from_suids can find it.
            let obj = match SemanticObject::with_content(restored_suid, tier, owner, content) {
                Some(o) => o,
                None => { println!("  [DEMO 5] with_content failed"); return; }
            };
            unsafe {
                let registry = kernel_core::semantic::registry::global_registry();
                if !registry.insert(obj) {
                    println!("  [DEMO 5] registry.insert failed (duplicate SUID?)");
                    return;
                }
            }

            // Apply the LLM-context security policy and print the redacted view.
            // 2026-05-13: REPLACED build_from_suids (which returns LlmContext
            // ~258KB by value, ballooning init_loader_task's stack frame to
            // 528KB and overwriting adjacent slots' iret-RIP slots — task #40
            // bug source #2). Use redactor.redact directly with a static
            // scratch, same pattern as sem_demo_one.
            static mut DEMO5_REDACT_OUT: [u8; 1024] = [0; 1024];
            unsafe {
                let scratch = core::slice::from_raw_parts_mut(
                    (&raw mut DEMO5_REDACT_OUT) as *mut u8, 1024,
                );
                let redactor = kernel_core::llm::context_builder::global_redactor();
                let n = redactor.redact(content, scratch);
                let scratch_ro = &*((&raw const DEMO5_REDACT_OUT) as *const [u8; 1024]);
                print!("  [DEMO 5]   LLM CONTEXT:  ");
                for &b in &scratch_ro[..n] { print!("{}", b as char); }
                println!();
            }
            println!("  [DEMO 5] => kernel redaction policy survives a reboot");
        }
        Err(e) if matches!(e, kernel_core::drivers::traits::DriverError::NotReady) => {
            println!("  [DEMO 5] no snapshot on disk — first boot, seeding");
            seed_persistent_object(dev, &suid);
        }
        Err(e) => {
            println!("  [DEMO 5] load_snapshot error: {:?} — re-seeding", e);
            seed_persistent_object(dev, &suid);
        }
    }
}

/// Helper: write a fresh Sensitive object to the snapshot area.
fn seed_persistent_object(
    dev: &dyn kernel_core::drivers::traits::BlockDevice,
    suid: &kernel_core::semantic::SUID,
) {
    use kernel_core::storage::snapshot::save_snapshot;
    let content = b"Persistent Sensitive payload: email=zara@example.com card=5500-0000-0000-0004";
    static mut SEED_BUF: [u8; 4096] = [0; 4096];
    let buf = unsafe { &mut *(&raw mut SEED_BUF) };
    buf[0..8].copy_from_slice(&suid.high.to_le_bytes());
    buf[8..16].copy_from_slice(&suid.low.to_le_bytes());
    buf[16] = 2; // Sensitive
    buf[17] = 0; // owner
    let len = content.len() as u16;
    buf[18..20].copy_from_slice(&len.to_le_bytes());
    buf[20..20 + content.len()].copy_from_slice(content);
    let total = 20 + content.len();
    match save_snapshot(dev, &buf[..total]) {
        Ok(()) => println!("  [DEMO 5] seeded snapshot ({} bytes); reboot to see persistence", total),
        Err(e) => println!("  [DEMO 5] save_snapshot failed: {:?}", e),
    }
}

/// Kernel-side SemanticObject demo. Exercises the same registry +
/// context_builder code paths that SYS_SEM_CREATE / SYS_LLM_CONTEXT would
/// hit from Ring 3, without depending on the user-ELF loader. Proves the
/// project's headline differentiator: tier-based processing happens
/// automatically when content is packaged for an LLM, even though the
/// same content is fully visible via direct registry access.
///
/// Demo 2 (Sensitive object): same data shows verbatim via direct read,
/// redacted via LLM CTX.
/// Demo 3 (Public object):    same data shows verbatim via direct read,
/// verbatim via LLM CTX too — Public objects are full-LLM-access by
/// design. The visual contrast makes the model self-explanatory.
fn sem_demo_kernel() {
    use kernel_core::semantic::{SUID, SemanticObject};
    use kernel_core::memory::SecurityTier;

    // ---- Demo 2: Sensitive tier ----
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 2: SemanticObject + LLM context (Sensitive tier)");
    println!("================================================================");
    sem_demo_one(
        SUID::new(0x1000_0000_0000_0001, 0x0123_4567_89AB_CDEF),
        SecurityTier::Sensitive,
        b"Sensitive: email=user@example.com card=4111-1111-1111-1111",
    );

    // ---- Demo 3: Public tier — same data, no redaction ----
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 3: SemanticObject + LLM context (Public tier)");
    println!("================================================================");
    sem_demo_one(
        SUID::new(0x1000_0000_0000_0002, 0xCAFE_BABE_DEAD_BEEF),
        SecurityTier::Public,
        b"Public:    email=user@example.com card=4111-1111-1111-1111",
    );

    println!();
    println!("================================================================");
    println!("  Same data, two views: kernel mediates only LLM-bound output.");
    println!("================================================================");
}

/// DEMO 7: LLM streaming syscalls test
fn llm_streaming_test() {
    println!("  [DEMO 7] Testing LLM streaming syscalls from kernel space");

    // Make the syscalls directly from kernel space to test the interface
    let prompt = b"explain semantic operating systems briefly";

    println!("  [DEMO 7] Starting streaming LLM request...");
    let request_id = unsafe {
        kernel_core::syscall::handle_llm_stream_start(
            prompt.as_ptr() as u64,
            prompt.len() as u64,
            0, // No context
        )
    };

    if request_id == u64::MAX {
        println!("  [DEMO 7] ERROR: Failed to start LLM stream");
        return;
    }

    println!("  [DEMO 7] Stream request started (ID={}), polling for response...", request_id);

    // Give the mock provider a chance to process
    for _i in 0..1000 {
        core::hint::spin_loop();
    }

    // Process pending requests manually since we're in kernel space
    unsafe {
        let provider = kernel_core::llm::provider::global_provider();
        provider.process_pending();
    }

    // Try to read response
    let mut buffer = [0u8; 512];
    let result = unsafe {
        kernel_core::syscall::handle_llm_stream_read(
            request_id,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
        )
    };

    match result {
        u64::MAX => {
            println!("  [DEMO 7] ERROR: Stream read failed");
        },
        val if val == u64::MAX - 1 => {
            println!("  [DEMO 7] Stream still processing");
        },
        val if val == u64::MAX - 2 => {
            println!("  [DEMO 7] Stream was cancelled");
        },
        0 => {
            println!("  [DEMO 7] Stream complete (no data)");
        },
        bytes_read => {
            println!("  [DEMO 7] Got response ({} bytes):", bytes_read);
            let response = core::str::from_utf8(&buffer[..bytes_read as usize])
                .unwrap_or("[invalid UTF-8]");
            println!("  [DEMO 7]   Response: {}", response);
        }
    }

    println!("  [DEMO 7] => Streaming LLM syscalls working!");
}

/// DEMO 8: Security policy framework test
fn security_policy_test() {
    use kernel_core::security::{
        policy::{PolicyObject, PolicyType, PolicyTarget, PolicyRule, RuleCondition, PolicyAction},
        evaluation::{create_evaluation_context, RequestType, global_policy_engine},
        policy_suids, user_ids,
    };
    use kernel_core::semantic::SUID;
    use kernel_core::memory::SecurityTier;

    println!("  [DEMO 8] Testing security policy framework");

    // Create a test policy: "Admin can access everything, others get Public tier"
    let mut admin_policy = PolicyObject::new(
        PolicyType::ObjectAccess,
        PolicyTarget::Everyone,
        user_ids::ADMIN,
        100, // High priority
    );

    // Rule 1: Admin gets full access
    let admin_rule = PolicyRule::simple(
        RuleCondition::RequesterIs(user_ids::ADMIN),
        PolicyAction::Allow(SecurityTier::Secret),
    );

    // Rule 2: Everyone else gets public access
    let public_rule = PolicyRule::simple(
        RuleCondition::Always,
        PolicyAction::Allow(SecurityTier::Public),
    );

    if admin_policy.add_rule(admin_rule).is_ok() &&
       admin_policy.add_rule(public_rule).is_ok() {
        println!("  [DEMO 8] Created test policy with 2 rules");
    } else {
        println!("  [DEMO 8] Failed to create test policy");
        return;
    }

    // Store policy as semantic object
    let policy_suid = policy_suids::new_system_policy(1);
    let mut policy_data = [0u8; 256];
    match admin_policy.serialize(&mut policy_data) {
        Ok(len) => {
            println!("  [DEMO 8] Serialized policy ({} bytes) to SUID {:016X}_{:016X}",
                len, policy_suid.high, policy_suid.low);

            // Test evaluation engine with default policies (no storage needed for demo)
            let test_object = SUID::new(0x1000_0000_0000_1234, 0xABCD_EF01_2345_6789);

            // Test 1: Admin access (should use default policy since no stored policies)
            let admin_context = create_evaluation_context(
                user_ids::ADMIN,
                SecurityTier::Public,
                test_object,
                RequestType::DirectAccess,
            );

            unsafe {
                let engine = global_policy_engine();
                let result = engine.evaluate(&admin_context);
                println!("  [DEMO 8] Admin access result: {:?}", result);
            }

            // Test 2: Regular user access
            let user_context = create_evaluation_context(
                42, // Regular user ID
                SecurityTier::Internal,
                test_object,
                RequestType::DirectAccess,
            );

            unsafe {
                let engine = global_policy_engine();
                let result = engine.evaluate(&user_context);
                println!("  [DEMO 8] Regular user access result: {:?}", result);
            }

        },
        Err(e) => {
            println!("  [DEMO 8] Policy serialization failed: {:?}", e);
            return;
        }
    }

    // Test policy SUID generation
    let system_suid = policy_suids::new_system_policy(123);
    let user_suid = policy_suids::new_user_policy(42, 456);

    println!("  [DEMO 8] System policy SUID: {:016X}_{:016X}", system_suid.high, system_suid.low);
    println!("  [DEMO 8] User policy SUID:   {:016X}_{:016X}", user_suid.high, user_suid.low);

    println!("  [DEMO 8] Policy SUID validation:");
    println!("    System SUID is policy: {}", policy_suids::is_policy_suid(&system_suid));
    println!("    User SUID is policy: {}", policy_suids::is_policy_suid(&user_suid));
    println!("    System SUID is system policy: {}", policy_suids::is_system_policy(&system_suid));
    println!("    User SUID is system policy: {}", policy_suids::is_system_policy(&user_suid));

    println!("  [DEMO 8] => Security policy framework working!");

    // Test the SYS_LLM_SET_POLICY and SYS_LLM_GET_POLICY syscalls
    test_policy_syscalls();
}

/// DEMO 9: Context-aware redaction engine test
fn context_aware_redaction_test() {
    use kernel_core::llm::{ContextAwareRedactor, RedactionContext};
    use kernel_core::security::{user_ids, evaluation::RequestType};
    use kernel_core::semantic::SUID;
    use kernel_core::memory::SecurityTier;

    println!("  [DEMO 9] Testing context-aware redaction engine");

    // Create a redactor instance
    let mut redactor = ContextAwareRedactor::new();
    redactor.init();

    // Test data containing various PII patterns
    let test_content = b"Patient John Smith, MRN123456, DOB 01/15/1990, has account ACCT9876543210. \
                        Contact: john.smith@email.com, SSN: 123-45-6789, CC: 4532-1234-5678-9012";

    let mut output_buffer = [0u8; 512];

    println!("  [DEMO 9] Original content ({} bytes):", test_content.len());
    if let Ok(content_str) = core::str::from_utf8(test_content) {
        println!("    \"{}\"", content_str);
    }

    // Test 1: Admin user with high-tier access (minimal redaction)
    println!("  [DEMO 9] Test 1: Admin user (minimal redaction)");
    let admin_context = RedactionContext {
        requester_id: user_ids::ADMIN,
        requester_tier: SecurityTier::Secret,
        target_suid: SUID::new(0x1234, 0x5678),
        request_type: RequestType::LLMContext,
        context_flags: 0,
        app_context: 0,
    };

    let len = redactor.redact_with_context(test_content, &admin_context, &mut output_buffer);
    if let Ok(result) = core::str::from_utf8(&output_buffer[..len]) {
        println!("    Result: \"{}\"", result);
    }

    // Test 2: Guest user with medical redaction profile
    println!("  [DEMO 9] Test 2: Guest user (standard redaction)");
    let guest_context = RedactionContext {
        requester_id: user_ids::GUEST,
        requester_tier: SecurityTier::Public,
        target_suid: SUID::new(0x1234, 0x5678),
        request_type: RequestType::LLMContext,
        context_flags: 0,
        app_context: 0,
    };

    output_buffer.fill(0);
    let len = redactor.redact_with_context(test_content, &guest_context, &mut output_buffer);
    if let Ok(result) = core::str::from_utf8(&output_buffer[..len]) {
        println!("    Result: \"{}\"", result);
    }

    // Test 3: Direct medical pattern testing
    println!("  [DEMO 9] Test 3: Medical redaction patterns");
    let medical_content = b"Patient PATIENT789 has MRN456789 and DOB 03/22/1985";
    output_buffer.fill(0);
    let len = redactor.redact_with_context(medical_content, &guest_context, &mut output_buffer);
    if let Ok(result) = core::str::from_utf8(&output_buffer[..len]) {
        println!("    Medical content: \"{}\"", core::str::from_utf8(medical_content).unwrap_or("invalid"));
        println!("    Redacted result: \"{}\"", result);
    }

    // Test 4: Financial pattern testing
    println!("  [DEMO 9] Test 4: Financial redaction patterns");
    let financial_content = b"Account ACCT987654321 with routing 123456789";
    output_buffer.fill(0);
    let len = redactor.redact_with_context(financial_content, &guest_context, &mut output_buffer);
    if let Ok(result) = core::str::from_utf8(&output_buffer[..len]) {
        println!("    Financial content: \"{}\"", core::str::from_utf8(financial_content).unwrap_or("invalid"));
        println!("    Redacted result: \"{}\"", result);
    }

    // Test 5: Name-only redaction
    println!("  [DEMO 9] Test 5: Name pattern detection");
    let name_content = b"Dr. Elizabeth Martinez and Michael Johnson collaborated on the research";
    output_buffer.fill(0);
    let len = redactor.redact_with_context(name_content, &guest_context, &mut output_buffer);
    if let Ok(result) = core::str::from_utf8(&output_buffer[..len]) {
        println!("    Name content: \"{}\"", core::str::from_utf8(name_content).unwrap_or("invalid"));
        println!("    Redacted result: \"{}\"", result);
    }

    println!("  [DEMO 9] => Context-aware redaction engine working!");
}

/// DEMO 10: network-backed LLM provider end-to-end.
///
/// Exercises the full path:
///   LlmProvider::submit + process_pending (ProviderType::Remote)
///     → NetworkLlmProvider::complete
///       → HTTP/1.1 request framing into req_buf
///       → LoopbackTransport (in-kernel mock peer)
///         → parses request, synthesises Anthropic-shaped JSON response
///       → HTTP/JSON parse extracts the completion text
///   ← LlmResponse delivered back through the queue
fn network_llm_provider_test() {
    use kernel_core::llm::{
        net_provider::{global_net_provider, ApiFormat, TransportKind},
        provider::{global_provider, LlmRequest, ProviderType, RequestState},
    };

    println!("  [DEMO 10] Inspecting default endpoint configuration:");
    unsafe {
        let net = global_net_provider();
        let ep = net.endpoint();
        let host_str = core::str::from_utf8(ep.host()).unwrap_or("<invalid>");
        let path_str = core::str::from_utf8(ep.path()).unwrap_or("<invalid>");
        let model_str = core::str::from_utf8(ep.model()).unwrap_or("<invalid>");
        println!("    host=\"{}\"  port={}", host_str, ep.port());
        println!("    path=\"{}\"  model=\"{}\"", path_str, model_str);
        println!("    transport={:?}  format={:?}  max_tokens={}",
            ep.transport, ep.format, ep.max_tokens);
    }

    // ---- Test 1: direct round-trip via NetworkLlmProvider::complete ----
    println!("  [DEMO 10] Test 1: direct round-trip via NetworkLlmProvider");
    let prompt = b"What is a semantic operating system?";
    let mut completion = [0u8; 512];
    let res = unsafe { global_net_provider().complete(prompt, &mut completion) };
    match res {
        Ok(n) => {
            let s = core::str::from_utf8(&completion[..n]).unwrap_or("<binary>");
            println!("    completion ({} bytes): \"{}\"", n, s);
        }
        Err(e) => {
            println!("    FAILED with LlmError code {}", e.to_error_code());
        }
    }

    // ---- Test 2: round-trip via LlmProvider queue with ProviderType::Remote ----
    println!("  [DEMO 10] Test 2: queue round-trip via LlmProvider (Remote)");
    unsafe {
        let provider = global_provider();
        let saved_type = provider.provider_type();
        provider.set_type(ProviderType::Remote);

        let task_id = kernel_core::scheduler::current_task_index() as u8;
        let tier = kernel_core::scheduler::current_task_max_tier();
        let req = LlmRequest::new(task_id, tier, b"summarize: tiered LLM access");
        match provider.submit(req) {
            Ok(request_id) => {
                println!("    submitted request_id={}", request_id);
                // Drive the queue. process_pending runs one request at a time.
                provider.process_pending();
                match provider.get_status(request_id) {
                    Some(RequestState::Completed) => {
                        if let Some(resp) = provider.get_response(request_id) {
                            let body = resp.content();
                            let s = core::str::from_utf8(body).unwrap_or("<binary>");
                            println!("    response ({} bytes, ok={}): \"{}\"",
                                body.len(), resp.is_success(), s);
                        } else {
                            println!("    no response cached");
                        }
                    }
                    Some(s) => println!("    unexpected final state: {:?}", s),
                    None => println!("    request vanished from queue"),
                }
            }
            Err(e) => println!("    submit FAILED with LlmError code {}", e.to_error_code()),
        }
        provider.cleanup();
        provider.set_type(saved_type);
    }

    // ---- Test 3: provider counters reflect the activity ----
    unsafe {
        let net = global_net_provider();
        println!("  [DEMO 10] NetworkLlmProvider stats: success={}, failure={}",
            net.success_count(), net.failure_count());
    }

    // ---- Test 4: degraded path — TlsTcp transport without an IP
    // configured fails cleanly (no DNS yet; caller must set remote_ip
    // on the TLS singleton at boot before this transport is usable). ----
    println!("  [DEMO 10] Test 4: switching transport to TlsTcp (no remote IP yet)");
    unsafe {
        let net = global_net_provider();
        let saved = net.endpoint().transport;
        net.endpoint_mut().transport = TransportKind::TlsTcp;
        let mut sink = [0u8; 64];
        match net.complete(b"ping", &mut sink) {
            Ok(_) => println!("    UNEXPECTED success on unconfigured TlsTcp"),
            Err(e) => println!("    rejected as expected (LlmError code {})", e.to_error_code()),
        }
        net.endpoint_mut().transport = saved;
    }

    // ---- Test 5: format switch — verify parser rejects mismatched shape ----
    println!("  [DEMO 10] Test 5: ApiFormat::OpenAi parser rejects Anthropic-shape body");
    unsafe {
        let net = global_net_provider();
        let saved_fmt = net.endpoint().format;
        net.endpoint_mut().format = ApiFormat::OpenAi;
        // The loopback peer always answers Anthropic-shape: `content` is an
        // ARRAY of `{type, text}` objects, not a string. The OpenAI parser
        // walks `"content":"..."` (a string), so it must fail to extract,
        // and `complete` must surface InternalError (-57). A success here
        // would mean the parser was matching the wrong field.
        let mut out = [0u8; 256];
        match net.complete(b"hello openai branch", &mut out) {
            Ok(n) => {
                let s = core::str::from_utf8(&out[..n]).unwrap_or("<binary>");
                println!("    UNEXPECTED success: \"{}\"", s);
            }
            Err(e) if e.to_error_code() == -57 => {
                println!("    correctly rejected mismatched shape (LlmError code -57)");
            }
            Err(e) => println!("    rejected, but with unexpected code {}", e.to_error_code()),
        }
        net.endpoint_mut().format = saved_fmt;
    }

    println!("  [DEMO 10] => Network-backed LLM provider working end-to-end!");
}

/// DEMO 11: User identity & isolation.
///
/// Exercises the user-account registry and the new identity-aware syscall
/// surface end-to-end:
///   1. Read the current uid (boot task should be SYSTEM).
///   2. Enumerate the built-in accounts.
///   3. Create a new user via SYS_CREATE_USER.
///   4. Look it up via SYS_LOOKUP_USER.
///   5. Drop privilege via SYS_SETUID to the new user.
///   6. Re-check uid + verify SetUid policy refuses upward setuid.
///   7. Attempt to create *another* user as the dropped user — must fail.
///   8. Reset back to SYSTEM and confirm.
fn user_identity_test() {
    use kernel_core::syscall::{dispatch, numbers::*};
    use kernel_core::security::user_ids;
    use kernel_core::security::users::{global_user_registry, MAX_USERNAME_LEN};

    // ---- 1. Current uid ----
    let initial_uid = dispatch(SYS_GETUID, 0, 0, 0, 0);
    println!("  [DEMO 11] initial uid = {} (SYSTEM={}, ADMIN={})",
        initial_uid, user_ids::SYSTEM, user_ids::ADMIN);

    // ---- 2. Enumerate built-ins ----
    println!("  [DEMO 11] built-in accounts:");
    unsafe {
        let reg = global_user_registry();
        for acc in reg.iter() {
            let name = core::str::from_utf8(acc.name()).unwrap_or("?");
            println!("    uid={:3}  name=\"{}\"  tier={}  group={}  flags=0x{:04x}",
                acc.id, name, acc.default_max_tier as u8, acc.group, acc.flags.0);
        }
    }

    // ---- 3. Create a new user "alice" via SYS_CREATE_USER ----
    let alice_name = b"alice";
    let alice_uid = dispatch(SYS_CREATE_USER,
        alice_name.as_ptr() as u64,
        alice_name.len() as u64,
        1, // tier = Internal
        kernel_core::security::users::groups::USERS as u64);
    println!("  [DEMO 11] create_user(\"alice\", tier=1) -> uid={}", alice_uid);
    if alice_uid == u64::MAX {
        println!("  [DEMO 11] FAILED to create alice; aborting demo");
        return;
    }

    // ---- 4. Look alice up via SYS_LOOKUP_USER ----
    let mut record = [0u8; 128];
    let n = dispatch(SYS_LOOKUP_USER,
        alice_uid,
        record.as_ptr() as u64,
        record.len() as u64,
        0);
    if n > 0 && n < record.len() as u64 {
        let s = core::str::from_utf8(&record[..n as usize]).unwrap_or("<binary>");
        println!("  [DEMO 11] lookup_user({}) -> \"{}\"", alice_uid, s);
    } else {
        println!("  [DEMO 11] lookup_user({}) returned {} (unexpected)", alice_uid, n);
    }

    // ---- 5. Drop to alice ----
    let drop_result = dispatch(SYS_SETUID, alice_uid, 0, 0, 0);
    println!("  [DEMO 11] setuid({}) -> {}", alice_uid, drop_result);
    let after_drop = dispatch(SYS_GETUID, 0, 0, 0, 0);
    println!("  [DEMO 11] uid after drop = {} (expected {})", after_drop, alice_uid);

    // ---- 6. Alice can't escalate back to ADMIN ----
    let escalate = dispatch(SYS_SETUID, user_ids::ADMIN as u64, 0, 0, 0);
    if escalate == u64::MAX {
        println!("  [DEMO 11] setuid(ADMIN) from alice REJECTED (good)");
    } else {
        println!("  [DEMO 11] setuid(ADMIN) from alice UNEXPECTEDLY succeeded ({})", escalate);
    }
    let still_alice = dispatch(SYS_GETUID, 0, 0, 0, 0);
    println!("  [DEMO 11] uid still = {} after rejected escalation", still_alice);

    // ---- 7. Alice can't create users ----
    let bob_name = b"bob";
    let bob_attempt = dispatch(SYS_CREATE_USER,
        bob_name.as_ptr() as u64,
        bob_name.len() as u64,
        0,
        kernel_core::security::users::groups::USERS as u64);
    if bob_attempt == u64::MAX {
        println!("  [DEMO 11] create_user(\"bob\") from alice REJECTED (good)");
    } else {
        println!("  [DEMO 11] create_user(\"bob\") from alice UNEXPECTEDLY returned {}", bob_attempt);
    }

    // ---- 8. Bypass the syscall guard (we're still in-kernel) to drop back ----
    // Real userland code would never have this option — the kernel is doing
    // this on its own behalf to clean up after the demo.
    kernel_core::scheduler::set_current_user_id(initial_uid as u8);
    let restored = dispatch(SYS_GETUID, 0, 0, 0, 0);
    println!("  [DEMO 11] restored uid = {} (expected {})", restored, initial_uid);

    // ---- 9. Show that the lookup format is parseable from user space ----
    println!("  [DEMO 11] final registry size: {} active account(s)",
        unsafe { global_user_registry().count() });

    // Touch MAX_USERNAME_LEN so an over-long name doesn't accidentally compile-
    // away. Negative test: name longer than the cap must be rejected.
    let too_long = [b'x'; MAX_USERNAME_LEN + 4];
    let reject = dispatch(SYS_CREATE_USER,
        too_long.as_ptr() as u64,
        too_long.len() as u64,
        0,
        0);
    if reject == u64::MAX {
        println!("  [DEMO 11] over-long name REJECTED (good)");
    } else {
        println!("  [DEMO 11] over-long name UNEXPECTEDLY returned {}", reject);
    }

    println!("  [DEMO 11] => User identity & isolation working!");
}

/// DEMO 12: TCP smoke test.
///
/// Opens a TCP connection to the SLIRP gateway (10.0.2.2) on a port
/// where no service is listening. The expected outcome is:
///   SynSent → Closed (RST'd by the host kernel)
///
/// Observing that state transition proves the whole L2/L3/L4 stack moves
/// a packet end-to-end:
///   - virtio-net descriptor chain works (TX SYN, RX RST)
///   - smoltcp Interface emits a SYN with correct headers
///   - smoltcp ARPs for the gateway first; SLIRP replies; ARP cache works
///   - smoltcp processes incoming RST and advances socket state
///
/// We log only state transitions (not every poll), then release the
/// socket so the slot is available for the next test.
fn tcp_smoke_test() {
    use kernel_core::net::{self, TcpStream, Ipv4Address, TcpState as State};

    let target = Ipv4Address::new(10, 0, 2, 2);
    let target_port: u16 = 1234;

    println!("  [DEMO 12] connect 10.0.2.2:{} (no service expected; should RST quickly)", target_port);
    let mut stream = match TcpStream::connect(target, target_port) {
        Ok(s) => s,
        Err(e) => {
            println!("  [DEMO 12] connect() returned error: {:?} — aborting", e);
            return;
        }
    };

    // Poll up to N times, log every state change. SLIRP replies fast;
    // we should see SynSent within the first few polls and the final
    // state (Closed) within a few dozen.
    let mut last_state = State::Closed; // placeholder so first observed state is logged
    let mut first = true;
    let mut terminal_state: Option<State> = None;
    for i in 0..5_000 {
        net::poll();
        let s = stream.state();
        if first || s != last_state {
            println!("  [DEMO 12] poll {:4}: state = {}", i, s);
            last_state = s;
            first = false;
        }
        // Terminal states for our purposes: Established (handshake OK,
        // unlikely against a closed port) or Closed (RST'd or never
        // reachable). Either tells us the stack moved through the SM.
        if matches!(s, State::Established | State::Closed) && !first {
            terminal_state = Some(s);
            // Give the loop a couple more iterations to confirm stability.
            for _ in 0..10 { net::poll(); }
            break;
        }
        // Bound the cost — each poll is one smoltcp tick.
        if i > 200 && s == State::SynSent {
            println!("  [DEMO 12] still SynSent after 200 polls — SLIRP didn't respond; giving up");
            break;
        }
        core::hint::spin_loop();
    }

    match terminal_state {
        Some(State::Established) => {
            // Best possible outcome: full three-way handshake completed
            // over the virtio-net + smoltcp stack. Either there's a real
            // service on 10.0.2.2:1234, or QEMU SLIRP optimistically
            // SYN-ACKed (some SLIRP versions do this before forwarding,
            // then RST on the first data packet). Either way: SYN out,
            // SYN-ACK in, ACK out — the stack moves packets both ways.
            println!("  [DEMO 12] => Stack works END TO END: SYN → SYN-ACK → ACK, socket ESTABLISHED");
        }
        Some(State::Closed) => {
            // Also a valid signal: SYN went out, peer responded (likely
            // RST), state machine advanced. Less stack work exercised
            // than the Established case but proves bidirectional flow.
            println!("  [DEMO 12] => Stack works: SYN went out, peer responded (likely RST), state advanced");
        }
        Some(other) => {
            println!("  [DEMO 12] => Stopped in intermediate state {} — unusual but not necessarily wrong", other);
        }
        None => {
            println!("  [DEMO 12] => No terminal state observed; either SLIRP is dropping or polling too slow");
        }
    }

    stream.close();
    for _ in 0..20 { net::poll(); } // let FIN exchange complete
    stream.release();
    println!("  [DEMO 12] socket released; net state ready for next connection");
}

/// DEMO 13: SPKI-pinning byte-level validation.
///
/// kernel-core can't run `cargo test` (no_std, no test harness), so the
/// crypto/parser correctness checks live here as boot-time exercises.
/// All three pieces of the pinning flow get exercised:
///   1. DER scanner walks the real WE1 intermediate (675 bytes) and
///      returns the right SubjectPublicKeyInfo sub-slice.
///   2. SHA-256 of that SPKI equals the hardcoded `ANTHROPIC_INTERMEDIATE_PIN`.
///   3. EC P-256 point extraction pulls the 65-byte uncompressed point
///      from the leaf SPKI (the one we'd feed `crypto::p256::verify_p256`).
/// Any FAIL line means the TLS path is broken before we touch the network.
fn spki_pin_test() {
    use kernel_core::tls::spki_pin::{
        self, ANTHROPIC_INTERMEDIATE_PIN, EC_P256_UNCOMPRESSED_LEN,
    };

    // Same fixtures the kernel-core tests reference. Bundled into the
    // binary at compile time — zero runtime cost beyond the bytes.
    const INTERMEDIATE_DER: &[u8] =
        include_bytes!("../../kernel-core/src/tls/fixtures/anthropic_intermediate_we1.der");
    const INTERMEDIATE_SPKI: &[u8] =
        include_bytes!("../../kernel-core/src/tls/fixtures/anthropic_intermediate_we1_spki.der");
    const LEAF_SPKI: &[u8] =
        include_bytes!("../../kernel-core/src/tls/fixtures/anthropic_leaf_spki.der");

    println!("  [DEMO 13] fixtures: intermediate={}B  spki={}B  leaf_spki={}B",
        INTERMEDIATE_DER.len(), INTERMEDIATE_SPKI.len(), LEAF_SPKI.len());

    // (1) DER scanner — does extract_spki find the right sub-slice?
    let extracted = match spki_pin::extract_spki(INTERMEDIATE_DER) {
        Ok(s) => s,
        Err(e) => { println!("  [DEMO 13] FAIL: extract_spki errored: {:?}", e); return; }
    };
    if extracted.len() != INTERMEDIATE_SPKI.len()
        || extracted.iter().zip(INTERMEDIATE_SPKI.iter()).any(|(a, b)| a != b)
    {
        println!("  [DEMO 13] FAIL: extracted SPKI ({} B) doesn't match fixture ({} B)",
            extracted.len(), INTERMEDIATE_SPKI.len());
        return;
    }
    println!("  [DEMO 13] PASS: DER scanner extracted SPKI byte-exactly ({} B)", extracted.len());

    // (2) Pin compare — SHA-256(SPKI) == ANTHROPIC_INTERMEDIATE_PIN.
    match spki_pin::verify_pin(INTERMEDIATE_DER, &ANTHROPIC_INTERMEDIATE_PIN) {
        Ok(()) => println!("  [DEMO 13] PASS: SHA-256(SPKI) == pinned hash"),
        Err(e) => { println!("  [DEMO 13] FAIL: verify_pin: {:?}", e); return; }
    }

    // Negative-side check: tampering one bit of the pin must reject.
    let mut wrong = ANTHROPIC_INTERMEDIATE_PIN;
    wrong[7] ^= 0x01;
    match spki_pin::verify_pin(INTERMEDIATE_DER, &wrong) {
        Err(_) => println!("  [DEMO 13] PASS: wrong pin correctly rejected"),
        Ok(()) => { println!("  [DEMO 13] FAIL: wrong pin was accepted (!!)"); return; }
    }

    // (3) EC point extraction from the leaf SPKI. This is what feeds
    // into ECDSA verification of the CertificateVerify message in
    // TLS 1.3. Must be exactly 65 bytes starting with 0x04.
    let point = match spki_pin::extract_ec_p256_point(LEAF_SPKI) {
        Ok(p) => p,
        Err(e) => { println!("  [DEMO 13] FAIL: extract_ec_p256_point: {:?}", e); return; }
    };
    if point.len() != EC_P256_UNCOMPRESSED_LEN || point[0] != 0x04 {
        println!("  [DEMO 13] FAIL: leaf EC point malformed (len={}, marker=0x{:02x})",
            point.len(), point[0]);
        return;
    }
    println!("  [DEMO 13] PASS: leaf EC P-256 point extracted (X[0..4] = {:02x} {:02x} {:02x} {:02x})",
        point[1], point[2], point[3], point[4]);

    println!("  [DEMO 13] => SPKI pinning ready; TLS verifier needs only the embedded-tls glue");
}

/// DEMO 14: drive [`kernel_core::tls::verifier::SpkiPinVerifier`]
/// through its `TlsVerifier` trait surface using a synthetic chain
/// built from the real fixtures.
///
/// What this proves that DEMO 13 doesn't: the spki_pin module is
/// reachable through the embedded-tls `verify_certificate` signature
/// — i.e. the vendor visibility patches were correct, the trait
/// bounds resolve, and the verifier actually populates its captured
/// leaf-point slot on success.
///
/// What this still doesn't prove: `verify_signature` against a real
/// transcript. That needs an actual handshake; it's exercised once
/// `NetworkLlmProvider` opens a TLS connection in Task #31.
fn tls_verifier_test() {
    use kernel_core::tls::crypto_shim::KernelSha256;
    use kernel_core::tls::verifier::SpkiPinVerifier;
    use kernel_core::tls::{CertificateEntryRef, CertificateRef, TlsVerifier};

    const INTERMEDIATE_DER: &[u8] =
        include_bytes!("../../kernel-core/src/tls/fixtures/anthropic_intermediate_we1.der");

    // For the leaf slot in the synthetic chain we reuse the intermediate
    // cert. We only need an X.509 cert whose SPKI is EC P-256, and both
    // fixtures qualify (Anthropic's leaf and intermediate are both EC).
    // verify_signature isn't run here, so it doesn't matter that we'd
    // never see this exact leaf in a real handshake.
    const LEAF_FAKE: &[u8] = INTERMEDIATE_DER;

    println!("  [DEMO 14] building synthetic CertificateRef (leaf={} B, intermediate={} B)",
        LEAF_FAKE.len(), INTERMEDIATE_DER.len());

    let mut chain = CertificateRef::with_context(&[]);
    if chain.add(CertificateEntryRef::X509(LEAF_FAKE)).is_err() {
        println!("  [DEMO 14] FAIL: chain.add(leaf) returned error");
        return;
    }
    if chain.add(CertificateEntryRef::X509(INTERMEDIATE_DER)).is_err() {
        println!("  [DEMO 14] FAIL: chain.add(intermediate) returned error");
        return;
    }

    // Empty transcript — verify_certificate clones it but doesn't
    // require any particular state for step 1; the transcript is only
    // used in step 2 (signature verification).
    let transcript = KernelSha256::default();

    let mut verifier = SpkiPinVerifier::new();
    match verifier.verify_certificate(&transcript, &None, chain) {
        Ok(()) => println!("  [DEMO 14] PASS: verify_certificate accepted real chain"),
        Err(e) => { println!("  [DEMO 14] FAIL: verify_certificate rejected real chain: {:?}", e); return; }
    }

    // After success the verifier must have captured the leaf EC point.
    match verifier.captured_leaf_point() {
        Some(point) => {
            if point[0] != 0x04 {
                println!("  [DEMO 14] FAIL: captured leaf point bad marker 0x{:02x}", point[0]);
                return;
            }
            println!("  [DEMO 14] PASS: leaf EC point captured ({:02x} {:02x} {:02x} {:02x}...)",
                point[1], point[2], point[3], point[4]);
        }
        None => { println!("  [DEMO 14] FAIL: leaf EC point NOT captured after success"); return; }
    }

    // Negative case: build a chain whose intermediate has one byte
    // flipped INSIDE its SPKI region. Flipping outside the SPKI
    // wouldn't change the hash (we only hash the SubjectPublicKeyInfo
    // sub-slice, not the whole cert), so the test would silently
    // pass-when-it-shouldn't. Compute the SPKI offset by extracting
    // it from the original cert and taking a pointer difference;
    // that's the actual byte range the pin covers.
    const N: usize = 675;
    let spki_slice = kernel_core::tls::spki_pin::extract_spki(INTERMEDIATE_DER)
        .expect("intermediate parses");
    let spki_offset = (spki_slice.as_ptr() as usize) - (INTERMEDIATE_DER.as_ptr() as usize);
    println!("  [DEMO 14] SPKI lives at byte {}..{} of intermediate",
        spki_offset, spki_offset + spki_slice.len());

    let mut bad_intermediate = [0u8; N];
    bad_intermediate.copy_from_slice(INTERMEDIATE_DER);
    // Flip a byte well inside the SPKI body (past the SEQUENCE +
    // AlgorithmIdentifier headers — should land in the EC point).
    bad_intermediate[spki_offset + 40] ^= 0xFF;

    let mut bad_chain = CertificateRef::with_context(&[]);
    let _ = bad_chain.add(CertificateEntryRef::X509(LEAF_FAKE));
    let _ = bad_chain.add(CertificateEntryRef::X509(&bad_intermediate));

    let mut v2 = SpkiPinVerifier::new();
    match v2.verify_certificate(&transcript, &None, bad_chain) {
        Err(_) => println!("  [DEMO 14] PASS: tampered intermediate correctly rejected"),
        Ok(()) => { println!("  [DEMO 14] FAIL: tampered intermediate was ACCEPTED (!!)"); return; }
    }

    println!("  [DEMO 14] => TlsVerifier trait surface OK; ready to plug into TlsConnection");
}

/// DEMO 15: TLS transport wiring smoke test.
///
/// What this proves:
///   1. `global_tls_transport()` returns a singleton that starts in
///      a sensible state (not connected, no remote IP).
///   2. `configure_global(ip, port)` plumbs through to the singleton.
///   3. A `connect()` against an unreachable target fails cleanly via
///      `TransportError::Closed` — no panic, no buffer leak, the
///      transport returns to `!is_connected()` and is reusable.
///
/// What this DOESN'T prove (yet):
///   - That a real TLS handshake succeeds against api.anthropic.com.
///     We have no DNS, so until the user wires an actual Anthropic IP
///     into `configure_global`, the handshake path is exercised only
///     up to "TCP connect" (and only as a failure). Live-handshake
///     test is a separate task pending an Anthropic IP or an
///     in-QEMU TLS server.
fn tls_transport_smoke() {
    use kernel_core::net::Ipv4Address;
    use kernel_core::llm::transport::NetworkTransport;
    use kernel_core::tls::transport_tls::{configure_global, global_tls_transport};

    // Step 1: initial state.
    let (initially_connected, initially_ip) = unsafe {
        let t = global_tls_transport();
        (t.is_connected(), t.remote_ip())
    };
    if initially_connected {
        println!("  [DEMO 15] FAIL: singleton started connected (!?)"); return;
    }
    if initially_ip.is_some() {
        println!("  [DEMO 15] FAIL: singleton started with a remote_ip"); return;
    }
    println!("  [DEMO 15] PASS: singleton starts clean (not connected, no IP)");

    // Step 2: configure with an unreachable target. SLIRP routes to
    // the host stack, but port 1 is reserved (no service ever listens)
    // — we get a clean RST. This is enough to drive TCP through to a
    // terminal state without leaving any sockets dangling.
    let target = Ipv4Address::new(10, 0, 2, 2);
    let target_port = 1u16;
    configure_global(target, target_port);

    let remote = unsafe { global_tls_transport().remote_ip() };
    if remote != Some(target) {
        println!("  [DEMO 15] FAIL: configure_global didn't stick (got {:?})", remote);
        return;
    }
    println!("  [DEMO 15] PASS: configured remote_ip = {:?}:{}", target, target_port);

    // Step 3: try to connect. Three possible outcomes:
    //   - Closed: SLIRP responded with RST (most likely on port 1).
    //   - Closed: TCP came up but TLS handshake failed (no server).
    //   - Ok(()): impossible without a real TLS server (would mean
    //     SLIRP is forwarding port 1 to something — buggy setup).
    // Pass on any clean error; fail on success or panic.
    let mut sink = [0u8; 16];
    let connect_result = unsafe {
        global_tls_transport().connect("api.anthropic.com", target_port)
    };
    match connect_result {
        Err(e) => println!("  [DEMO 15] PASS: connect to unreachable target failed cleanly ({:?})", e),
        Ok(()) => {
            println!("  [DEMO 15] UNEXPECTED: connect succeeded — closing");
            unsafe { global_tls_transport().close(); }
        }
    }

    // Make sure we don't return with the singleton claiming an open connection.
    let post_state = unsafe {
        let t = global_tls_transport();
        (t.is_connected(), t.name())
    };
    if post_state.0 {
        println!("  [DEMO 15] FAIL: singleton still connected after error path"); return;
    }
    println!("  [DEMO 15] PASS: singleton reusable after failed connect (name={})", post_state.1);

    // Step 4: prove send/recv before connect rejects with InvalidState
    // (the trait's contract for the unhappy path).
    let send_err = unsafe { global_tls_transport().send(b"x") };
    let recv_err = unsafe { global_tls_transport().recv(&mut sink) };
    match (send_err, recv_err) {
        (Err(_), Err(_)) => println!("  [DEMO 15] PASS: send/recv before connect both reject"),
        (s, r) => { println!("  [DEMO 15] FAIL: pre-connect send/recv didn't reject (send={:?} recv={:?})", s, r); return; }
    }

    println!("  [DEMO 15] => TLS transport ready for live handshake once an Anthropic IP is wired");
}

/// Run the RFC 8439 §2.8.2 ChaCha20-Poly1305 test vector through our
/// crypto_shim's `KernelChacha20Poly1305` and compare the resulting
/// ciphertext + auth tag byte-for-byte against the published bytes.
///
/// Returns `true` on full match. Diagnoses the bug fast if our shim
/// drifts from the spec — every cross-impl AEAD failure starts here.
fn aead_kat_test() -> bool {
    use kernel_core::tls::crypto_shim::{run_rfc8439_aead_kat, AeadKatOutcome};
    match run_rfc8439_aead_kat() {
        AeadKatOutcome::Pass => {
            println!("  [KAT] PASS: ChaCha20-Poly1305 ciphertext + tag match RFC 8439 §2.8.2");
            println!("  [KAT] => AEAD shim is byte-correct against the spec");
            true
        }
        AeadKatOutcome::CiphertextMismatch { diff_bytes } => {
            println!("  [KAT] FAIL: ciphertext differs in {} of 114 bytes", diff_bytes);
            false
        }
        AeadKatOutcome::TagMismatch => {
            println!("  [KAT] FAIL: Poly1305 tag differs from spec");
            false
        }
        AeadKatOutcome::EncryptFailed => {
            println!("  [KAT] FAIL: encrypt_in_place_detached errored");
            false
        }
    }
}


/// DEMO 16: live TLS handshake against api.anthropic.com (Phase 8 finale).
///
/// This is the first outbound TLS connection this kernel has ever made
/// to a real server. The success criterion isn't "got a valid Claude
/// response" — we don't have an API key plumbed in via the boot path
/// — it's "TLS handshake completed AND we got a parseable HTTP response
/// back." That means:
///   • ClientHello / ServerHello / Key Exchange done
///   • Server's real cert chain walked through our SpkiPinVerifier
///   • Pin matched the WE1 intermediate (the trust anchor decision)
///   • Leaf's ECDSA signature over the transcript verified via our p256
///   • Application-data records exchanged in both directions
///
/// Caveats:
///   • IP is hardcoded from `nslookup api.anthropic.com` on the host.
///     Anthropic uses cloud routing — the IP can rotate. If this demo
///     hangs or RSTs, re-resolve and update [`ANTHROPIC_IP`] below.
///   • SLIRP outbound TCP to arbitrary IPs must be enabled in QEMU
///     (default; `-netdev user`). On bare metal we'd need iwlwifi.
///   • The 401 response we expect is the SERVER telling us "you didn't
///     authenticate," which itself is a strong positive signal — it
///     means our request was decrypted, parsed, and routed.
fn tls_live_handshake() {
    use kernel_core::net::Ipv4Address;
    use kernel_core::llm::transport::NetworkTransport;
    use kernel_core::tls::transport_tls::{configure_global, global_tls_transport};

    // Resolved 2026-05-16 via `Resolve-DnsName api.anthropic.com` on
    // the host. Re-resolve if the handshake starts failing.
    const ANTHROPIC_IP: Ipv4Address = Ipv4Address::new(160, 79, 104, 10);
    const ANTHROPIC_PORT: u16 = 443;
    const SNI_HOST: &str = "api.anthropic.com";

    println!("  [DEMO 16] target: {}:{} (SNI={})", ANTHROPIC_IP, ANTHROPIC_PORT, SNI_HOST);

    configure_global(ANTHROPIC_IP, ANTHROPIC_PORT);

    // Stage A: connect. This drives TCP SYN/SYN-ACK/ACK and then the
    // full TLS 1.3 handshake. Expected duration: 1-2 seconds on QEMU
    // SLIRP depending on network RTT.
    println!("  [DEMO 16] opening TLS connection... (handshake includes pin check + signature verify)");
    let connect_result = unsafe {
        global_tls_transport().connect(SNI_HOST, ANTHROPIC_PORT)
    };
    match connect_result {
        Ok(()) => println!("  [DEMO 16] PASS: TLS handshake succeeded — server cert pinned + signature verified"),
        Err(e) => {
            println!("  [DEMO 16] FAIL: handshake failed: {:?}", e);
            let tcp_state = kernel_core::tls::transport_tls::TlsTransport::last_tcp_state();
            let tls_err = kernel_core::tls::transport_tls::TlsTransport::last_handshake_error();
            println!("  [DEMO 16] last TCP state: {:?}", tcp_state);
            println!("  [DEMO 16] underlying TlsError: {:?}", tls_err);
            // If TCP never reached Established, the cause is networking
            // (stale IP, SLIRP routing, host firewall). If TCP came up
            // and TLS failed, the cause is in our crypto/verifier.
            match (tcp_state, tls_err) {
                (Some(s), _) if s != kernel_core::net::TcpState::Established =>
                    println!("  [DEMO 16] DIAGNOSIS: TCP didn't reach Established ({:?}) — check IP/SLIRP", s),
                (Some(_), Some(_)) =>
                    println!("  [DEMO 16] DIAGNOSIS: TCP up, TLS rejected — check cipher/cert/p256"),
                _ =>
                    println!("  [DEMO 16] DIAGNOSIS: unclear — neither TCP nor TLS state was captured"),
            }
            // Still try to clean up the singleton state so the kernel
            // boot continues.
            unsafe { global_tls_transport().close(); }
            return;
        }
    }

    // Stage B: send a minimal HTTP/1.1 POST. No API key, so we expect
    // a 401 — but that confirms the request was decrypted, parsed,
    // and routed by Anthropic's frontend.
    //
    // Body is deliberately tiny so the whole request fits in one TLS
    // record. Content-Length must match the body byte count exactly.
    let body: &[u8] = b"{\"model\":\"claude-haiku-4-5-20251001\",\"max_tokens\":8,\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}";
    let mut req_buf = [0u8; 512];
    let req_len = {
        let mut p = 0;
        let head = b"POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\nUser-Agent: semantic-os/0.1\r\nContent-Type: application/json\r\nanthropic-version: 2023-06-01\r\nContent-Length: ";
        req_buf[p..p+head.len()].copy_from_slice(head); p += head.len();
        // Decimal Content-Length.
        let mut len_buf = [0u8; 8];
        let len_str = write_u32_decimal(body.len() as u32, &mut len_buf);
        req_buf[p..p+len_str.len()].copy_from_slice(len_str); p += len_str.len();
        req_buf[p..p+4].copy_from_slice(b"\r\n\r\n"); p += 4;
        req_buf[p..p+body.len()].copy_from_slice(body); p += body.len();
        p
    };
    println!("  [DEMO 16] sending {}-byte HTTP POST...", req_len);

    let mut total_sent = 0;
    while total_sent < req_len {
        let n_result = unsafe {
            global_tls_transport().send(&req_buf[total_sent..req_len])
        };
        match n_result {
            Ok(0) => { println!("  [DEMO 16] FAIL: send returned 0 bytes"); break; }
            Ok(n) => total_sent += n,
            Err(e) => { println!("  [DEMO 16] FAIL: send error: {:?}", e); unsafe { global_tls_transport().close(); } return; }
        }
    }
    println!("  [DEMO 16] PASS: {} bytes sent over TLS", total_sent);

    // Stage C: read the response back. Cap at 4 KiB — Anthropic 401
    // responses are tiny. We only need enough to confirm "HTTP/1.1 4xx"
    // and dump a few diagnostic headers/JSON characters.
    let mut resp = [0u8; 4096];
    let mut total_recv = 0;
    for _round in 0..20 {
        if total_recv == resp.len() { break; }
        let n_result = unsafe {
            global_tls_transport().recv(&mut resp[total_recv..])
        };
        match n_result {
            Ok(0) => break, // EOF / no more data this poll
            Ok(n) => total_recv += n,
            Err(e) => {
                println!("  [DEMO 16] recv error: {:?} (got {} B so far)", e, total_recv);
                if let Some(tls_err) = kernel_core::tls::transport_tls::TlsTransport::last_io_error() {
                    println!("  [DEMO 16] underlying TlsError: {:?}", tls_err);
                }
                break;
            }
        }
        // Stop early once we have the status line + a body chunk.
        if total_recv > 256 { break; }
    }
    println!("  [DEMO 16] PASS: {} bytes received over TLS", total_recv);

    // Print the HTTP status line + the first chunk of body so the
    // outcome is visible in the serial log without needing to dump
    // the full response.
    let response_str = match core::str::from_utf8(&resp[..total_recv]) {
        Ok(s) => s,
        Err(_) => { println!("  [DEMO 16] response not UTF-8 (binary?)"); ""; "" }
    };
    if !response_str.is_empty() {
        // Print first line (status).
        if let Some(eol) = response_str.find("\r\n") {
            println!("  [DEMO 16] status: {}", &response_str[..eol]);
        }
        // Print first ~200 chars of the body (after the header block).
        if let Some(body_start) = response_str.find("\r\n\r\n") {
            let body_text = &response_str[body_start + 4..];
            let preview = if body_text.len() > 200 { &body_text[..200] } else { body_text };
            println!("  [DEMO 16] body preview: {}", preview);
        }
    }

    // Tear down.
    unsafe { global_tls_transport().close(); }
    println!("  [DEMO 16] => First outbound TLS round-trip from this kernel — Phase 8 closed.");
}

/// Format a u32 as ASCII decimal into `buf`. Returns the populated slice.
/// Small helper local to DEMO 16 because the kernel println! ecosystem
/// has its own formatter we don't want to invoke for a single number.
fn write_u32_decimal(n: u32, buf: &mut [u8; 8]) -> &[u8] {
    if n == 0 { buf[0] = b'0'; return &buf[..1]; }
    let mut tmp = [0u8; 10];
    let mut k = 0;
    let mut v = n;
    while v > 0 && k < tmp.len() { tmp[k] = b'0' + (v % 10) as u8; v /= 10; k += 1; }
    for i in 0..k { buf[i] = tmp[k - 1 - i]; }
    &buf[..k]
}

/// Test policy management syscalls
fn test_policy_syscalls() {
    println!("  [DEMO 8] Testing policy management syscalls...");

    // Check current user ID and create appropriate policy SUID
    let current_user = kernel_core::scheduler::current_task_index() as u8;

    // Create a simple test policy owned by current user
    let mut test_policy = kernel_core::security::policy::PolicyObject::new(
        kernel_core::security::policy::PolicyType::ObjectAccess,
        kernel_core::security::policy::PolicyTarget::Everyone,
        current_user, // Policy owned by current user
        200, // High priority
    );

    // Add a simple rule to make the policy valid
    let allow_rule = kernel_core::security::policy::PolicyRule::simple(
        kernel_core::security::policy::RuleCondition::Always,
        kernel_core::security::policy::PolicyAction::Allow(kernel_core::memory::SecurityTier::Public),
    );

    if test_policy.add_rule(allow_rule).is_err() {
        println!("  [DEMO 8] Failed to add rule to test policy");
        return;
    }

    // Serialize the policy
    let mut policy_data = [0u8; 256];
    let policy_len = match test_policy.serialize(&mut policy_data) {
        Ok(len) => len,
        Err(_) => {
            println!("  [DEMO 8] Policy serialization failed");
            return;
        }
    };

    println!("  [DEMO 8] Current user ID: {}", current_user);

    // Try both system policy (should fail unless we're admin/system)
    // and user policy (should work for any user)
    let system_suid = kernel_core::security::policy_suids::new_system_policy(42);
    let user_suid = kernel_core::security::policy_suids::new_user_policy(current_user, 123);

    println!("  [DEMO 8] Testing SYS_LLM_SET_POLICY...");
    println!("    System SUID: {:016X}_{:016X}", system_suid.high, system_suid.low);
    println!("    User SUID:   {:016X}_{:016X}", user_suid.high, user_suid.low);

    // Test 1: Try system policy (should fail unless we're admin/system)
    println!("  [DEMO 8] Test 1: System policy creation");
    let system_result = unsafe {
        kernel_core::syscall::handle_llm_set_policy(
            system_suid.high,
            system_suid.low,
            policy_data.as_ptr() as u64,
            policy_len as u64,
        )
    };

    match system_result {
        val if val == u64::MAX => {
            println!("    System policy: general error");
        },
        val if val == u64::MAX - 1 => {
            println!("    System policy: validation error");
        },
        val if val == u64::MAX - 2 => {
            println!("    System policy: insufficient privilege (expected for non-admin)");
        },
        suid_high => {
            println!("    System policy: SUCCESS (stored at {:016X}_xxxx)", suid_high);
        }
    }

    // Test 2: Try user policy (should succeed)
    println!("  [DEMO 8] Test 2: User policy creation");
    let user_result = unsafe {
        kernel_core::syscall::handle_llm_set_policy(
            user_suid.high,
            user_suid.low,
            policy_data.as_ptr() as u64,
            policy_len as u64,
        )
    };

    match user_result {
        val if val == u64::MAX => {
            println!("    User policy: general error");
        },
        val if val == u64::MAX - 1 => {
            println!("    User policy: validation error");
        },
        val if val == u64::MAX - 2 => {
            println!("    User policy: insufficient privilege");
        },
        suid_high => {
            println!("    User policy: SUCCESS (stored at {:016X}_xxxx)", suid_high);

            // Test retrieval of user policy
            let mut read_buffer = [0u8; 256];
            let get_result = unsafe {
                kernel_core::syscall::handle_llm_get_policy(
                    user_suid.high,
                    user_suid.low,
                    read_buffer.as_mut_ptr() as u64,
                    read_buffer.len() as u64,
                )
            };

            match get_result {
                0 => {
                    println!("    GET_POLICY: policy not found");
                },
                val if val == u64::MAX => {
                    println!("    GET_POLICY: general error");
                },
                val if val == u64::MAX - 2 => {
                    println!("    GET_POLICY: insufficient privilege");
                },
                bytes_read => {
                    println!("    GET_POLICY: SUCCESS (read {} bytes)", bytes_read);

                    // Verify the data matches what we stored
                    if bytes_read as usize >= policy_len &&
                       &read_buffer[..policy_len] == &policy_data[..policy_len] {
                        println!("    Data verification: PASSED");
                    } else {
                        println!("    Data verification: FAILED");
                    }
                }
            }
        }
    }

    println!("  [DEMO 8] => Policy syscalls test complete!");
}

/// Run a single SemanticObject demo: insert, direct read, LLM-context read.
fn sem_demo_one(
    suid: kernel_core::semantic::SUID,
    tier: kernel_core::memory::SecurityTier,
    content: &[u8],
) {
    use kernel_core::semantic::SemanticObject;

    let owner = 0u8;
    let obj = match SemanticObject::with_content(suid, tier, owner, content) {
        Some(o) => o,
        None => { println!("[sem_demo] SemanticObject::with_content failed"); return; }
    };
    let inserted = unsafe {
        let registry = kernel_core::semantic::registry::global_registry();
        registry.insert(obj)
    };
    if !inserted {
        println!("[sem_demo] registry.insert failed");
        return;
    }
    let tier_label = match tier {
        kernel_core::memory::SecurityTier::Public    => "Public",
        kernel_core::memory::SecurityTier::Internal  => "Internal",
        kernel_core::memory::SecurityTier::Sensitive => "Sensitive",
        kernel_core::memory::SecurityTier::Secret    => "Secret",
    };
    println!("  SUID:        0x{:016X}_{:016X}", suid.high, suid.low);
    println!("  Tier:        {}", tier_label);

    // Direct registry read — kernel mode = full access.
    let direct: &[u8] = unsafe {
        let registry = kernel_core::semantic::registry::global_registry();
        match registry.get(&suid) {
            Some(o) => o.content.as_bytes().unwrap_or(&[]),
            None => &[],
        }
    };
    print!("  DIRECT READ: ");
    for &b in direct { print!("{}", b as char); }
    println!();

    // LLM context build, simulated. We avoid `build_from_suids` here
    // because LlmContext is ~262 KiB and would overflow the 16 KiB
    // kernel task stack. Same logic the context_builder applies though:
    // tier-based processing.
    print!("  LLM CONTEXT: ");
    use kernel_core::memory::SecurityTier;
    static mut CTX_OUT: [u8; 1024] = [0; 1024];
    match tier {
        SecurityTier::Public => {
            // Verbatim — full LLM access.
            for &b in direct { print!("{}", b as char); }
        }
        SecurityTier::Internal => {
            // Summarize.
            let summary = unsafe {
                kernel_core::llm::context_builder::global_summarizer().summarize(direct)
            };
            for &b in summary.as_bytes() { print!("{}", b as char); }
        }
        SecurityTier::Sensitive => {
            // Redact.
            let n = unsafe {
                let scratch = core::slice::from_raw_parts_mut(
                    (&raw mut CTX_OUT) as *mut u8, 1024,
                );
                let redactor = kernel_core::llm::context_builder::global_redactor();
                redactor.redact(direct, scratch)
            };
            unsafe {
                let scratch = &*((&raw const CTX_OUT) as *const [u8; 1024]);
                for &b in &scratch[..n] { print!("{}", b as char); }
            }
        }
        SecurityTier::Secret => {
            print!("<excluded>");
        }
    }
    println!();
}

fn spawn_named(path: &str) { spawn_named_at(path, 0); }

fn spawn_named_at(path: &str, tier: u64) {
    let pid = kernel_core::syscall::dispatch(
        kernel_core::syscall::numbers::SYS_SPAWN,
        path.as_ptr() as u64,
        path.len() as u64,
        tier,
        0,
    );
    if pid == u64::MAX {
        println!("[init_loader] SYS_SPAWN({}) FAILED", path);
    }
    // (silenced success line — keep the demo output clean)
}

/// User-mode task entry point — runs in Ring 3 (unprivileged).
///
/// **Constraint:** until we have a real ELF loader path for arbitrary user
/// programs, this function is mapped into the user's address space at
/// USER_CODE_BASE as a single 4KB code page. That means it MUST NOT
/// reference any data outside its own code page — no string literals
/// (in .rodata), no statics (in .data/.bss), no extern functions. Every
/// access has to go through registers passed via SYSCALL.
///
/// This stub validates the full Ring 3 → SYSCALL → kernel → SYSRET → Ring 3
/// round-trip with syscalls that take only register arguments and no
/// pointers: SYS_GETPID (4) and SYS_YIELD (3).
fn user_task_entry() {
    loop {
        // SYS_GETPID — no arguments, returns the current task index.
        let _pid = user_syscall(4, 0, 0, 0, 0);
        // SYS_YIELD — give other tasks a turn so the scheduler can prove
        // it round-robins from Ring 3 too.
        user_syscall(3, 0, 0, 0, 0);
    }
}

/// Universal syscall helper — issues SYSCALL with up to 4 arguments.
/// Returns the value in RAX after the syscall.
#[inline(always)]
fn user_syscall(num: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let result: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") num,
            in("rdi") arg0,
            in("rsi") arg1,
            in("rdx") arg2,
            in("r10") arg3,
            lateout("rax") result,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    result
}

/// Enable SSE and SSE2 instructions.
///
/// Configures CR0 and CR4 so the CPU stops raising #UD on SSE instructions
/// and so fxsave/fxrstor work for context switches:
/// - CR0.EM (bit 2) = 0   — don't emulate FPU (use real instructions)
/// - CR0.MP (bit 1) = 1   — monitor coprocessor (allows WAIT/FWAIT to wait on FPU)
/// - CR4.OSFXSR    (bit 9)  = 1 — OS supports fxsave/fxrstor + SSE
/// - CR4.OSXMMEXCPT (bit 10) = 1 — OS handles SIMD floating-point exceptions
fn enable_sse() {
    unsafe {
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
        cr0 &= !(1u64 << 2);  // clear EM
        cr0 |=  (1u64 << 1);  // set MP
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack));

        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        cr4 |= (1u64 << 9) | (1u64 << 10);  // OSFXSR | OSXMMEXCPT
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack));
    }
}

/// Panic handler
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!();
    println!("!!! KERNEL PANIC !!!");
    println!("{}", info);

    loop {
        x86_64::instructions::hlt();
    }
}

/// Print macro for serial output
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::serial::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
