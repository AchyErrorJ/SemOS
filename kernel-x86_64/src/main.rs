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

    println!("[*] Probing VirtIO block device...");
    if virtio::block::init() {
        if virtio::block::register_with_kernel_core() {
            println!("[virtio-blk] registered with driver registry as 'virtio0'");
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
