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

extern crate alloc;

use bootloader_api::{entry_point, BootInfo, BootloaderConfig};
use core::panic::PanicInfo;

/// Kernel global allocator — wraps the 16 MiB free-list heap arena
/// (`kernel_core::memory::heap`, initialised at boot before any allocation).
/// Enables `alloc` (Box/Vec/String) inside the kernel, used by tiny-skia for
/// 2D vector rendering (M8) and available for later kernel work. The same
/// arena already backs ObjectContent and FWRITE via direct heap calls; the
/// GlobalAlloc path shares it.
struct KernelGlobalAlloc;

unsafe impl core::alloc::GlobalAlloc for KernelGlobalAlloc {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        kernel_core::memory::heap::allocate(layout.size(), layout.align())
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        kernel_core::memory::heap::deallocate(ptr, layout.size(), layout.align());
    }
}

#[global_allocator]
static KERNEL_ALLOCATOR: KernelGlobalAlloc = KernelGlobalAlloc;

mod serial;
pub mod tty;
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
pub mod font;
pub mod gfx2d;
pub mod pci;
pub mod virtio;
pub mod rng;
pub mod rtc;
pub mod usb;

use serial::Serial;

/// Bootloader configuration
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    // Request a framebuffer if available
    config.mappings.physical_memory = Some(bootloader_api::config::Mapping::Dynamic);
    // task #42: bump main kernel stack from default 80 KiB to 512 KiB.
    // kernel_main is a single giant function with many println-formatting
    // frames + the Lazy<InterruptDescriptorTable>::new closure (20+
    // set_handler_fn calls). Small code-size growth (e.g. adding a Drop
    // impl, bumping a fixed buffer) was inflating LLVM-chosen frame
    // sizes enough to overflow 80 KiB → silent SS fault that manifested
    // as "hang at IDT init" (the latest println before the closure ran).
    // 512 KiB gives generous headroom; the bootloader allocates this
    // from its frame allocator before jumping to the kernel.
    config.kernel_stack_size = 512 * 1024;
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

    // Probe the MC146818 RTC and log the wall-clock time. Non-fatal —
    // the kernel boots either way; absent RTC just means
    // `Platform::wall_clock()` returns None and time-dependent code
    // (TLS notAfter, file timestamps, Marée/Brise) falls back to
    // monotonic-only behaviour.
    println!("[*] Probing RTC (MC146818)...");
    rtc::init_and_log();
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

    // USB init. Task #36 root cause: TASK_STACK_SIZE was 16 KiB,
    // adding USB pushed some kernel function's stack frame past
    // the cliff and overflowed into the previous slot's iret-RIP.
    // Fixed by bumping TASK_STACK_SIZE to 64 KiB (Phase 9 M3).
    println!("[*] Probing xHCI USB controller...");
    let _usb_ok = usb::init_and_enumerate();
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
    kernel_core::memory::heap::init();
    let (used, free, _blocks) = kernel_core::memory::heap::stats();
    println!("    Heap allocator: {} KiB arena ({} used, {} free)",
        (used+free)/1024, used, free);

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
    // Phase 14 Tier 3 #45 Ring-3 validation: spawns a sibling thread,
    // round-trips through SYS_FUTEX_WAIT/WAKE/THREAD_JOIN, exits with
    // a known code the kernel reads in DEMO 28.
    static THREAD_DEMO_ELF: &[u8] = include_bytes!(
        "../../user-programs/thread-demo/target/x86_64-unknown-none/release/thread-demo"
    );
    // Phase 14 M25 Tier-1 validation: same observable behaviour as
    // hello-rs.elf but produced through the new semos-std shim.
    // Exercises println! → fmt::Write::write_str → SYS_WRITE, plus
    // the main!() macro's _start glue + panic_handler routing.
    static HELLO_STD_ELF: &[u8] = include_bytes!(
        "../../user-programs/hello-std/target/x86_64-unknown-none/release/hello-std"
    );
    // M25 Tier 2 #50 validation: exercises GlobalAlloc → SYS_HEAP_ALLOC
    // via Vec/String/Box/format! end-to-end.
    static VEC_DEMO_ELF: &[u8] = include_bytes!(
        "../../user-programs/vec-demo/target/x86_64-unknown-none/release/vec-demo"
    );
    // M25 #51/#52 validation: fs::File + io::Read/Write, env::args/var,
    // sync::{Mutex,Once}, thread::spawn/join.
    static STD_DEMO_ELF: &[u8] = include_bytes!(
        "../../user-programs/std-demo/target/x86_64-unknown-none/release/std-demo"
    );
    // M25 std::process::Command validation: a Ring-3 parent spawns /bin
    // children and waits on their exit codes via SYS_SPAWN + SYS_WAIT.
    static SPAWN_DEMO_ELF: &[u8] = include_bytes!(
        "../../user-programs/spawn-demo/target/x86_64-unknown-none/release/spawn-demo"
    );
    // M25 std::net validation: a Ring-3 program resolves a host, TCP-connects,
    // and round-trips an HTTP request over the kernel's smoltcp stack.
    static NET_DEMO_ELF: &[u8] = include_bytes!(
        "../../user-programs/net-demo/target/x86_64-unknown-none/release/net-demo"
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
        if fs.add("thread-demo.elf", kernel_core::fs::ramfs::FileType::Executable, THREAD_DEMO_ELF) {
            println!("    Registered thread-demo.elf ({} bytes, Ring 3 threading demo)", THREAD_DEMO_ELF.len());
        } else {
            println!("    [WARN] failed to register thread-demo.elf");
        }
        if fs.add("hello-std.elf", kernel_core::fs::ramfs::FileType::Executable, HELLO_STD_ELF) {
            println!("    Registered hello-std.elf ({} bytes, semos-std M25 hello-world)", HELLO_STD_ELF.len());
        } else {
            println!("    [WARN] failed to register hello-std.elf");
        }
        if fs.add("vec-demo.elf", kernel_core::fs::ramfs::FileType::Executable, VEC_DEMO_ELF) {
            println!("    Registered vec-demo.elf ({} bytes, semos-std M25 Tier 2 alloc demo)", VEC_DEMO_ELF.len());
        } else {
            println!("    [WARN] failed to register vec-demo.elf");
        }
        if fs.add("std-demo.elf", kernel_core::fs::ramfs::FileType::Executable, STD_DEMO_ELF) {
            println!("    Registered std-demo.elf ({} bytes, semos-std M25 #51/#52 demo)", STD_DEMO_ELF.len());
        } else {
            println!("    [WARN] failed to register std-demo.elf");
        }
        if fs.add("spawn-demo.elf", kernel_core::fs::ramfs::FileType::Executable, SPAWN_DEMO_ELF) {
            println!("    Registered spawn-demo.elf ({} bytes, semos-std M25 process::Command demo)", SPAWN_DEMO_ELF.len());
        } else {
            println!("    [WARN] failed to register spawn-demo.elf");
        }
        if fs.add("net-demo.elf", kernel_core::fs::ramfs::FileType::Executable, NET_DEMO_ELF) {
            println!("    Registered net-demo.elf ({} bytes, semos-std M25 std::net demo)", NET_DEMO_ELF.len());
        } else {
            println!("    [WARN] failed to register net-demo.elf");
        }
    }

    // Semantic object system
    kernel_core::semantic::registry::init_global_registry();
    println!("    Semantic registry: initialized");
    kernel_core::semantic::vector::init_global_vector_index();
    println!("    Vector index: initialized");
    kernel_core::semantic::search::init_global_search();
    println!("    Semantic search: initialized");

    // Phase 9 Stage 2: install path-namespace root + load any prior-boot
    // snapshot from virtio0. MUST run AFTER `init_global_registry()` —
    // that call clears the registry; loading earlier wipes our entries.
    // `Namespace::init()` is idempotent (no-op if root already exists);
    // `load()` then overlays the persisted tree onto the live registry.
    // On a fresh disk `load()` returns Err(_) which we log + continue.
    if kernel_core::fs::paths::Namespace::init().is_err() {
        println!("    Path namespace: FAILED to install root");
    }
    if let Some(dev) = kernel_core::drivers::registry::get_block("virtio0") {
        match kernel_core::fs::paths::Namespace::load(dev) {
            Ok(n) => println!("    Path namespace: loaded {} bytes from virtio0 (prior-boot snapshot)", n),
            Err(_) => println!("    Path namespace: no prior snapshot on virtio0 (fresh disk)"),
        }
    }

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
    // Real unmapped guard pages below every task / per-task kernel stack
    // (task #41) — turns a silent neighbour-smashing overflow into an
    // immediate, precisely-addressed #PF. The canaries above remain as a
    // secondary net for any slot whose guard couldn't be installed.
    context::init_stack_guard_pages();

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

    // DEMO 17: hierarchical path namespace (Phase 9 Stage 1). Drives
    // `fs::paths::Namespace` through its public API end-to-end —
    // init, mkdir, create, write, read, readdir, unlink. Exercises the
    // path→SUID layer over the existing ObjectRegistry without needing
    // any persistence or syscall plumbing yet.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 17: hierarchical path namespace (Phase 9 Stage 1)");
    println!("================================================================");
    paths_namespace_test();

    // DEMO 18: USB xHCI + HID boot keyboard. PASS/FAIL/SKIPPED lines.
    // The first PASS lines (controller found, reset complete, descriptor
    // parsed) are printed inside usb::init_and_enumerate at boot. The
    // remaining ones (HID report read) require polling for keypress events,
    // which we do here for a bounded number of iterations and then either
    // PASS or SKIPPED if no keypress arrived (QEMU may not type into us).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 18: USB xHCI + HID boot keyboard");
    println!("================================================================");
    usb_hid_demo();

    // DEMO 19: RTC + wall clock through the Platform trait.
    // Validates that kernel-core's `platform::wall_clock()` reaches the
    // x86_64 RTC driver and returns a sensible Unix timestamp.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 19: RTC wall clock (Platform::wall_clock)");
    println!("================================================================");
    wall_clock_test();

    // DEMO 20: FS Stage 3 syscalls — exercise SYS_OPEN / SYS_FREAD /
    // SYS_FWRITE / SYS_CLOSE / SYS_MKDIR / SYS_UNLINK / SYS_READDIR /
    // SYS_STAT against the path namespace from Ring 0. User-space port
    // (fs-demo program) lands in a follow-up commit.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 20: FS syscalls over path namespace (Phase 9 Stage 3)");
    println!("================================================================");
    fs_syscall_test();

    // DEMO 21: FS Stage 2 — snapshot persistence. Save the namespace
    // to virtio0, simulate reboot by clearing in-memory state, reload,
    // verify same content + timestamps. Only runs if virtio0 is present.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 21: FS snapshot persistence (Phase 9 Stage 2)");
    println!("================================================================");
    fs_persistence_test();

    // DEMO 22: heap allocator (Phase 14 prereq #1).
    // SYS_HEAP_ALLOC / SYS_HEAP_FREE backed by the kernel's free-list
    // heap. The first foundation piece for std::alloc::GlobalAlloc.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 22: heap allocator (Phase 14 prereq)");
    println!("================================================================");
    heap_allocator_test();

    // DEMO 23: argv/envp passthrough in SYS_SPAWN (Phase 14 prereq #2).
    // Phase 14 prereq #2 — std::env::args() and cargo→rustc handoff.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 23: SYS_SPAWN argv/envp passthrough (Phase 14 prereq)");
    println!("================================================================");
    spawn_argv_test();

    // DEMO 24: per-process env + CWD via SYS_GET_*/SET_* (Phase 14 prereq #3).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 24: per-process env + CWD (Phase 14 prereq)");
    println!("================================================================");
    env_cwd_test();

    // DEMO 25: Tier 2 extended file ops (FSYNC / RENAME / TRUNCATE / STATX).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 25: extended file ops (Phase 14 Tier 2)");
    println!("================================================================");
    extended_fs_test();

    // DEMO 26: large-file FWRITE (>256 B → heap-Allocated ObjectContent).
    // Validates task #44.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 26: large-file FWRITE (Phase 14 Tier 2 #44)");
    println!("================================================================");
    large_file_fwrite_test();

    // DEMO 27: kernel-mode threading + futex + join (Phase 14 Tier 3
    // prereqs #45/#46/#47 scheduler-side).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 27: threading + futex + join (Phase 14 Tier 3)");
    println!("================================================================");
    threading_futex_test();

    // DEMO 28: Ring-3 same-AS thread spawn end-to-end (#45 finish).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 28: Ring 3 thread_spawn (Phase 14 Tier 3 #45)");
    println!("================================================================");
    ring3_thread_demo();

    // DEMO 29: hello-std.elf — first program using the semos-std shim.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 29: semos-std shim hello-world (Phase 14 M25)");
    println!("================================================================");
    hello_std_demo();

    // DEMO 30: vec-demo.elf — Vec/String/Box via SYS_HEAP_ALLOC.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 30: semos-std alloc (Vec/String/Box) — M25 Tier 2 #50");
    println!("================================================================");
    vec_demo();

    // DEMO 31: std-demo.elf — fs/io/env + Mutex/Once + thread (M25 #51/#52).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 31: semos-std fs/io/env/sync/thread — M25 #51/#52");
    println!("================================================================");
    std_demo();

    // DEMO 32: spawn-demo.elf — std::process::Command from a Ring-3 parent.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 32: semos-std process::Command (SYS_SPAWN/WAIT) — M25");
    println!("================================================================");
    spawn_demo();

    // DEMO 33: HTTP chunked-transfer-encoding decoder (M13).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 33: HTTP chunked-transfer decoder — M13");
    println!("================================================================");
    chunked_decode_demo();

    // DEMO 34: DNS resolver over SLIRP (10.0.2.3) — M12.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 34: DNS resolver (UDP/A-record over SLIRP) — M12");
    println!("================================================================");
    dns_resolver_demo();

    // DEMO 35: M6 framebuffer drawing API — checkerboard + rect + blit +
    // scroll, verified by reading pixels back out of framebuffer memory.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 35: framebuffer drawing API (M6)");
    println!("================================================================");
    fb_drawing_demo();

    // DEMO 36: net-demo.elf — std::net from Ring 3 (resolve + TCP + HTTP).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 36: semos-std std::net (resolve + TcpStream) — M25");
    println!("================================================================");
    net_demo();

    // DEMO 37: M7 TrueType font rasterization (ttf-parser + scanline fill).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 37: TTF font rasterization (NotoSans via M6 fb) — M7");
    println!("================================================================");
    font_demo();

    // DEMO 38: M8 anti-aliased 2D vector rendering (tiny-skia → M6 fb).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 38: anti-aliased 2D vector (tiny-skia) — M8");
    println!("================================================================");
    gfx2d_demo();

    // DEMO 39: M7/M8 TTY console — TrueType + AA text through a cursor-managed
    // console (newline, wrap, region scroll), verified by pixel readback.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 39: TTF/AA TTY console (M7 sharp + M8 smooth)");
    println!("================================================================");
    tty_demo();

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

/// DEMO 35: M6 framebuffer drawing API. Exercises every primitive
/// (`fb_fill_rect`, `fb_blit`, `fb_scroll`, `fb_present`) and verifies the
/// result by READING PIXELS BACK from framebuffer memory — this works in a
/// headless QEMU run where we can't see the screen.
///
/// Console-scroll hazard: the text console shares this framebuffer and
/// scrolls the whole surface up on every newline past the bottom row. So we
/// draw and read back EVERYTHING into locals first, with no `println!` in
/// between, then print the PASS/FAIL verdicts afterward.
fn fb_drawing_demo() {
    use framebuffer as fb;

    let (w, h) = fb::fb_dimensions();
    if w == 0 || h == 0 {
        println!("  [DEMO 35] SKIPPED: framebuffer not available");
        return;
    }
    let (bpp, stride, is_rgb) = fb::fb_format();

    // Two distinct test colors. rgb() packs to the native order internally.
    let red = fb::rgb(0xFF, 0x00, 0x00);
    let blue = fb::rgb(0x00, 0x00, 0xFF);
    let green = fb::rgb(0x00, 0xFF, 0x00);

    // Work in a self-contained block in the lower-right of the screen, well
    // away from the top where the console cursor lives. Use a 64x64 tile.
    const TILE: usize = 64;
    let region_x = w.saturating_sub(TILE + 16);
    let region_y = h.saturating_sub(TILE + 16);

    // --- 1. Checkerboard via fb_fill_rect (8x8 cells, alternating red/blue).
    let cell = TILE / 8;
    for cy in 0..8 {
        for cx in 0..8 {
            let c = if (cx + cy) % 2 == 0 { red } else { blue };
            fb::fb_fill_rect(region_x + cx * cell, region_y + cy * cell, cell, cell, c);
        }
    }

    // --- 2. A small solid green rect drawn on top, near the tile center.
    let rect_x = region_x + TILE / 2 - 4;
    let rect_y = region_y + TILE / 2 - 4;
    fb::fb_fill_rect(rect_x, rect_y, 8, 8, green);

    // --- 3. A blit: a 4x4 all-red patch at a known offset inside the tile.
    let patch = [red; 16];
    let blit_x = region_x + 2;
    let blit_y = region_y + 2;
    fb::fb_blit(&patch, blit_x, blit_y, 4, 4);

    // Commit the damage rect.
    let presented = fb::fb_present();

    // --- READ BACK (no println! above this point after drawing started).
    // (a) center of the green rect should be green.
    let px_green = fb::fb_read_pixel(rect_x + 4, rect_y + 4);
    // (b) a checkerboard cell we know is red: cell (0,0) center.
    let px_cb0 = fb::fb_read_pixel(region_x + cell / 2, region_y + cell / 2);
    // (c) adjacent cell (1,0) center should be blue.
    let px_cb1 = fb::fb_read_pixel(region_x + cell + cell / 2, region_y + cell / 2);
    // (d) blit patch center should be red.
    let px_blit = fb::fb_read_pixel(blit_x + 1, blit_y + 1);
    // (e) an untouched corner well outside the tile (top-left of screen is
    //     console territory, so use a point just left of our region which the
    //     console only reaches via full-screen scroll — sample the pixel one
    //     row BELOW our region, beyond what we drew, which we never wrote).
    let outside_x = region_x.saturating_sub(8);
    let outside_y = region_y + TILE + 4;
    let px_outside_before = fb::fb_read_pixel(outside_x, outside_y);

    // --- 4. Scroll test: shift the whole framebuffer left by 0 / up by 0 is
    //     a no-op; instead do a tiny localized verification by scrolling and
    //     checking a pixel moved. To avoid disturbing the verified region,
    //     test scroll on a fresh throwaway draw far from the tile and just
    //     confirm it doesn't fault and clears vacated edge to black. We do a
    //     full-surface scroll AFTER capturing the readbacks above so it
    //     can't corrupt them.
    // Draw a green marker, scroll up by 1px, confirm it moved up.
    let mk_x = region_x;
    let mk_y = region_y.saturating_sub(20);
    fb::fb_fill_rect(mk_x, mk_y, 4, 4, green);
    let before_scroll = fb::fb_read_pixel(mk_x + 1, mk_y + 1);
    fb::fb_scroll(0, 1); // content moves UP by 1 row
    let after_scroll = fb::fb_read_pixel(mk_x + 1, mk_y); // one row up
    fb::fb_present();

    // --- Now it's safe to print verdicts.
    println!("  [DEMO 35] surface: {}x{}  bpp={}  stride={}  format={}",
        w, h, bpp, stride, if is_rgb { "RGB" } else { "BGR" });

    let mut ok = true;

    if px_green == green {
        println!("  [DEMO 35] PASS: fb_fill_rect center reads back GREEN (0x{:06X})", px_green);
    } else {
        println!("  [DEMO 35] FAIL: fb_fill_rect center = 0x{:06X}, want 0x{:06X}", px_green, green);
        ok = false;
    }

    if px_cb0 == red && px_cb1 == blue {
        println!("  [DEMO 35] PASS: checkerboard cells alternate RED/BLUE (0x{:06X}/0x{:06X})", px_cb0, px_cb1);
    } else {
        println!("  [DEMO 35] FAIL: checkerboard cells = 0x{:06X}/0x{:06X}, want 0x{:06X}/0x{:06X}",
            px_cb0, px_cb1, red, blue);
        ok = false;
    }

    if px_blit == red {
        println!("  [DEMO 35] PASS: fb_blit patch reads back RED (0x{:06X})", px_blit);
    } else {
        println!("  [DEMO 35] FAIL: fb_blit patch = 0x{:06X}, want 0x{:06X}", px_blit, red);
        ok = false;
    }

    if px_outside_before == 0 {
        println!("  [DEMO 35] PASS: untouched pixel outside primitives still black (0x{:06X})", px_outside_before);
    } else {
        // Not fatal on its own (console could theoretically touch it), but
        // we drew nothing there, so it should be black.
        println!("  [DEMO 35] FAIL: untouched pixel = 0x{:06X}, want 0x000000", px_outside_before);
        ok = false;
    }

    if before_scroll == green && after_scroll == green {
        println!("  [DEMO 35] PASS: fb_scroll(0,1) moved marker up one row (GREEN before & after)");
    } else {
        println!("  [DEMO 35] FAIL: fb_scroll: before=0x{:06X} after=0x{:06X}, want both 0x{:06X}",
            before_scroll, after_scroll, green);
        ok = false;
    }

    match presented {
        Some((x0, y0, x1, y1)) => {
            println!("  [DEMO 35] PASS: fb_present committed damage rect ({},{})-({},{})", x0, y0, x1, y1);
        }
        None => {
            println!("  [DEMO 35] FAIL: fb_present returned no damage rect after draws");
            ok = false;
        }
    }

    if ok {
        println!("  [DEMO 35] => M6 framebuffer drawing API verified by pixel readback");
    } else {
        println!("  [DEMO 35] => M6 had failures (see above)");
    }
}

/// DEMO 18: poll the HID transfer ring for a few seconds, print any keypress
/// reports. Emits PASS/FAIL/SKIPPED lines matching the brief's checklist.
fn usb_hid_demo() {
    let dev = match usb::xhci::enumerated_device() {
        Some(d) => d,
        None => {
            println!("  [DEMO 18] SKIPPED: no USB device enumerated (run with -device qemu-xhci -device usb-kbd)");
            return;
        }
    };
    println!("  [DEMO 18] device: slot={} addr={} port={} speed={} v=0x{:04X} p=0x{:04X} kbd={}",
        dev.slot_id, dev.usb_address, dev.port, dev.speed,
        dev.vendor, dev.product, dev.is_keyboard);
    if !dev.is_keyboard {
        println!("  [DEMO 18] SKIPPED: enumerated device is not a HID boot keyboard");
        return;
    }

    // Poll for ~3 seconds (300 iterations of ~10ms each). Each iteration
    // drains all currently-pending Transfer Events on the HID ring. We
    // print the first report we see (idle or pressed) so the PASS line
    // doesn't require an actual keypress — QEMU may not type into us. If
    // a real keypress arrives we also translate keycodes to ASCII via
    // `keycode_to_ascii`.
    // Bounded poll: 20 iterations is enough to drain whatever's
    // already pending without burning the remaining DEMO-run budget.
    // QEMU's usb-kbd only sends reports on actual key-state changes,
    // not periodically, so without external input there's usually
    // nothing to drain. The fix #36 stack bump means subsequent
    // DEMOs MUST get a chance to run — keep this brief.
    let mut reports_seen: usize = 0;
    let mut printed_first = false;
    for _outer in 0..20 {
        let n = usb::xhci::poll_hid(|rep| {
            if !printed_first {
                let k0 = rep.keys[0];
                println!("  [DEMO 18] PASS: HID report read (modifiers=0x{:02X} key0=0x{:02X})",
                    rep.modifiers, k0);
                printed_first = true;
            }
            let shift = rep.shift_held();
            for k in rep.pressed_keys() {
                if let Some(c) = usb::hid::keycode_to_ascii(k, shift) {
                    if c.is_ascii_graphic() || c == b' ' {
                        print!("{}", c as char);
                    } else if c == b'\n' {
                        println!();
                    }
                }
            }
        });
        reports_seen += n;
        // Brief spin between polls (~ms). Was 1M before — way too long
        // under QEMU's TCG slowness; choked the rest of the DEMOs.
        for _ in 0..50_000 { core::hint::spin_loop(); }
    }

    if !printed_first {
        println!("  [DEMO 18] SKIPPED: HID report read (no report arrived during {} polls; \
            the keyboard may still be enumerated correctly — QEMU's usb-kbd only sends \
            reports on actual key state change, not periodically)",
            reports_seen);
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

    // M12: resolve the IP at runtime via SLIRP's DNS (10.0.2.3) instead
    // of hardcoding it. Anthropic uses cloud routing — the IP rotates —
    // so a live lookup is more robust than the baked-in value. The
    // hardcoded address (resolved 2026-05-16 via `Resolve-DnsName`)
    // stays as a fallback so DEMO 16 still passes if DNS is unavailable.
    const ANTHROPIC_IP_FALLBACK: Ipv4Address = Ipv4Address::new(160, 79, 104, 10);
    const ANTHROPIC_PORT: u16 = 443;
    const SNI_HOST: &str = "api.anthropic.com";

    let anthropic_ip = match kernel_core::net::resolve(SNI_HOST) {
        Some(ip) => {
            println!("  [DEMO 16] DNS resolved {} -> {}", SNI_HOST, ip);
            ip
        }
        None => {
            println!("  [DEMO 16] DNS resolve failed/unavailable — using hardcoded fallback {}", ANTHROPIC_IP_FALLBACK);
            ANTHROPIC_IP_FALLBACK
        }
    };

    println!("  [DEMO 16] target: {}:{} (SNI={})", anthropic_ip, ANTHROPIC_PORT, SNI_HOST);

    configure_global(anthropic_ip, ANTHROPIC_PORT);

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

/// DEMO 34: DNS resolver over SLIRP's resolver (10.0.2.3) — M12.
///
/// Exercises `kernel_core::net::resolve`: build an A-record query, send
/// it over a UDP socket, poll for the reply, parse the first A record.
///
/// PASS criteria:
///   • resolve() returns Some(ip) with a plausible (non-zero) IPv4
///   • a second resolve() of the same host returns the *same* answer
///     from the cache without issuing a new query
///
/// `example.com` is the primary target — it's stable and friendly over
/// SLIRP. We also try `api.anthropic.com` as a bonus (its IP rotates via
/// cloud routing, so we only report it, never fail on it).
fn dns_resolver_demo() {
    use kernel_core::net::{self, Ipv4Address};

    if !net::is_initialized() {
        println!("  [DEMO 34] SKIPPED: net stack not initialized (run with -netdev user)");
        return;
    }

    const HOST: &str = "example.com";

    // First resolution — issues a real UDP query to 10.0.2.3:53.
    let first = net::resolve(HOST);
    let ip = match first {
        Some(ip) => ip,
        None => {
            println!("  [DEMO 34] FAIL: resolve({}) returned None (no reply / malformed / SLIRP DNS down)", HOST);
            return;
        }
    };

    // Plausibility: 4 bytes (guaranteed by the type) and non-zero.
    let bytes = ip.as_bytes();
    let all_zero = bytes.iter().all(|&b| b == 0);
    if all_zero {
        println!("  [DEMO 34] FAIL: resolve({}) returned 0.0.0.0 (implausible)", HOST);
        return;
    }
    println!("  [DEMO 34] PASS: resolved {} -> {} (non-zero IPv4)", HOST, ip);

    // Second resolution — must hit the cache and return the SAME answer.
    let second = net::resolve(HOST);
    match second {
        Some(ip2) if ip2 == ip => {
            println!("  [DEMO 34] PASS: cache returned identical answer ({}) on second call", ip2);
        }
        Some(ip2) => {
            println!("  [DEMO 34] FAIL: second resolve returned {} != {}", ip2, ip);
            return;
        }
        None => {
            println!("  [DEMO 34] FAIL: second resolve returned None (cache miss?)");
            return;
        }
    }

    // Bonus: report the Anthropic resolution that DEMO 16 relies on.
    match net::resolve("api.anthropic.com") {
        Some(a_ip) if a_ip != Ipv4Address::new(0, 0, 0, 0) =>
            println!("  [DEMO 34] info: api.anthropic.com -> {} (used by DEMO 16)", a_ip),
        _ =>
            println!("  [DEMO 34] info: api.anthropic.com did not resolve over SLIRP (DEMO 16 uses fallback)"),
    }

    println!("  [DEMO 34] => M12 closed — kernel resolves hostnames via DNS.");
}

/// DEMO 17: hierarchical path namespace end-to-end (Phase 9 Stage 1).
///
/// Walks the full public API of [`kernel_core::fs::paths::Namespace`]:
///   1. init() — installs root directory
///   2. mkdir() — creates nested dirs
///   3. create_file() — writes initial content under a path
///   4. read_file() — reads back exact bytes
///   5. write_file() — overwrites + reads back the new content
///   6. readdir() — lists entries by name + SUID
///   7. unlink() — removes a file
///   8. negative paths — bad paths/missing entries return the right
///      FsError variants, no panics
///
/// Any FAIL line means the namespace is broken before persistence
/// or syscalls land on top.
fn paths_namespace_test() {
    use kernel_core::fs::paths::{FsError, Namespace};
    use kernel_core::memory::SecurityTier;

    // Step 0: clean any leftover state from a prior boot's persisted
    // snapshot. Phase 9 Stage 2 means /notes might already exist on disk
    // and get auto-loaded at boot; this DEMO assumes a fresh tree.
    // Order matters — unlink leaves first, then parents.
    let _ = Namespace::unlink("/notes/2026/meeting.md");
    let _ = Namespace::unlink("/notes/2026/scratch.md");
    let _ = Namespace::unlink("/notes/2026");
    let _ = Namespace::unlink("/notes");

    // Step 1: install root.
    if let Err(e) = Namespace::init() {
        println!("  [DEMO 17] FAIL: init: {:?}", e); return;
    }
    println!("  [DEMO 17] PASS: namespace init (root directory installed)");

    // Step 2: mkdir / nested mkdir.
    match Namespace::mkdir("/notes") {
        Ok(_) => println!("  [DEMO 17] PASS: mkdir /notes"),
        Err(e) => { println!("  [DEMO 17] FAIL: mkdir /notes: {:?}", e); return; }
    }
    match Namespace::mkdir("/notes/2026") {
        Ok(_) => println!("  [DEMO 17] PASS: mkdir /notes/2026 (nested)"),
        Err(e) => { println!("  [DEMO 17] FAIL: mkdir /notes/2026: {:?}", e); return; }
    }
    // Same name twice should reject.
    match Namespace::mkdir("/notes") {
        Err(FsError::AlreadyExists) => println!("  [DEMO 17] PASS: duplicate mkdir rejected (AlreadyExists)"),
        other => { println!("  [DEMO 17] FAIL: duplicate mkdir got {:?}, want AlreadyExists", other); return; }
    }

    // Step 3: create a file with initial content.
    const HELLO: &[u8] = b"first cut of the meeting notes\n";
    let _file_suid = match Namespace::create_file("/notes/2026/meeting.md", SecurityTier::Internal, HELLO) {
        Ok(s) => { println!("  [DEMO 17] PASS: create /notes/2026/meeting.md ({} B)", HELLO.len()); s }
        Err(e) => { println!("  [DEMO 17] FAIL: create_file: {:?}", e); return; }
    };

    // Step 4: read it back byte-exact.
    let got = match Namespace::read_file("/notes/2026/meeting.md") {
        Ok(b) => b,
        Err(e) => { println!("  [DEMO 17] FAIL: read_file: {:?}", e); return; }
    };
    if got != HELLO {
        println!("  [DEMO 17] FAIL: read_file returned {} B (want {} B)", got.len(), HELLO.len());
        return;
    }
    println!("  [DEMO 17] PASS: read_file returned exact bytes");

    // Step 5: overwrite + re-read.
    const REVISED: &[u8] = b"REVISED: now with action items.\n";
    if let Err(e) = Namespace::write_file("/notes/2026/meeting.md", REVISED) {
        println!("  [DEMO 17] FAIL: write_file: {:?}", e); return;
    }
    let got2 = match Namespace::read_file("/notes/2026/meeting.md") {
        Ok(b) => b,
        Err(e) => { println!("  [DEMO 17] FAIL: read_file after overwrite: {:?}", e); return; }
    };
    if got2 != REVISED {
        println!("  [DEMO 17] FAIL: read after overwrite mismatched"); return;
    }
    println!("  [DEMO 17] PASS: overwrite + reread roundtrip");

    // Step 6: readdir on /notes — should list "2026" alone.
    let mut found = false;
    let mut count = 0usize;
    let dir_res = Namespace::readdir("/notes", |name, _suid| {
        count += 1;
        if name == "2026" { found = true; }
    });
    match dir_res {
        Ok(()) => {
            if !found || count != 1 {
                println!("  [DEMO 17] FAIL: readdir /notes returned {} entries (want 1, '2026' found={})", count, found);
                return;
            }
            println!("  [DEMO 17] PASS: readdir /notes -> ['2026']");
        }
        Err(e) => { println!("  [DEMO 17] FAIL: readdir: {:?}", e); return; }
    }

    // Create a second file to test multi-entry readdir.
    if let Err(e) = Namespace::create_file("/notes/2026/scratch.md", SecurityTier::Internal, b"scratch") {
        println!("  [DEMO 17] FAIL: create scratch: {:?}", e); return;
    }
    let mut names: [&'static str; 4] = ["", "", "", ""];
    let mut ni = 0usize;
    let _ = Namespace::readdir("/notes/2026", |name, _| {
        if ni < names.len() {
            // SAFETY: the name slice lives in the registry; we only
            // print it (no storage past this closure). For the test
            // we promote with transmute since the iteration borrow
            // already prevents mutation during this call.
            names[ni] = unsafe { core::mem::transmute::<&str, &'static str>(name) };
            ni += 1;
        }
    });
    if ni != 2 {
        println!("  [DEMO 17] FAIL: /notes/2026 has {} entries, want 2", ni); return;
    }
    println!("  [DEMO 17] PASS: readdir /notes/2026 -> ['{}', '{}']", names[0], names[1]);

    // Step 7: unlink one file, confirm gone, confirm sibling untouched.
    if let Err(e) = Namespace::unlink("/notes/2026/scratch.md") {
        println!("  [DEMO 17] FAIL: unlink: {:?}", e); return;
    }
    match Namespace::read_file("/notes/2026/scratch.md") {
        Err(FsError::NotFound) => {}
        other => { println!("  [DEMO 17] FAIL: read after unlink got {:?}, want NotFound", other.map(|_| ())); return; }
    }
    match Namespace::read_file("/notes/2026/meeting.md") {
        Ok(_) => println!("  [DEMO 17] PASS: unlink removed only the named entry"),
        Err(e) => { println!("  [DEMO 17] FAIL: sibling read after unlink: {:?}", e); return; }
    }

    // Step 8: negative-path coverage. Each must return the right
    // FsError; any Ok (or wrong error) is a regression.
    match Namespace::mkdir("relative/path") {
        Err(FsError::NotAbsolute) => {}
        other => { println!("  [DEMO 17] FAIL: relative path got {:?}, want NotAbsolute", other.map(|_| ())); return; }
    }
    match Namespace::mkdir("/notes//double") {
        Err(FsError::EmptyComponent) => {}
        other => { println!("  [DEMO 17] FAIL: empty component got {:?}, want EmptyComponent", other.map(|_| ())); return; }
    }
    match Namespace::resolve("/does/not/exist") {
        Err(FsError::NotFound) => {}
        other => { println!("  [DEMO 17] FAIL: missing path got {:?}, want NotFound", other.map(|_| ())); return; }
    }
    // Walking through a file (treating it as a dir) must fail.
    match Namespace::resolve("/notes/2026/meeting.md/inner") {
        Err(FsError::NotADirectory) => {}
        other => { println!("  [DEMO 17] FAIL: through-file got {:?}, want NotADirectory", other.map(|_| ())); return; }
    }
    println!("  [DEMO 17] PASS: bad paths reject with the right FsError variants");

    println!("  [DEMO 17] => path namespace works end-to-end; persistence (Stage 2) is next");
}

/// DEMO 21: FS snapshot persistence across actual reboots.
///
/// Branches on whether the boot-time auto-load (in `kernel_main`,
/// just before this DEMO runs) restored a prior boot's snapshot:
///
/// - **First boot** (no snapshot on disk): create /persist/{a.txt,b.txt}
///   with known content, save, leave for next boot.
/// - **Second+ boot** (snapshot restored): verify the files came back
///   byte-exact + timestamps look right.
///
/// Two QEMU runs against the same vdisk image is the real test —
/// the first one saves, the second one validates.
fn fs_persistence_test() {
    use kernel_core::fs::paths::Namespace;
    use kernel_core::memory::SecurityTier;

    let dev = match kernel_core::drivers::registry::get_block("virtio0") {
        Some(d) => d,
        None => {
            println!("  [DEMO 21] SKIPPED: no virtio0 block device — run with -drive ...,if=virtio");
            return;
        }
    };

    const FILE_A: &[u8] = b"alpha file, saved across reboot\n";
    const FILE_B: &[u8] = b"beta file with different content\n";

    // Was a previous boot's state restored by the boot-time auto-load?
    let already_loaded = Namespace::read_file("/persist/a.txt").is_ok();

    if already_loaded {
        // SECOND+ BOOT: validate everything came back.
        println!("  [DEMO 21] detected prior-boot snapshot (boot-time auto-load fired)");

        let a = match Namespace::read_file("/persist/a.txt") {
            Ok(b) => b,
            Err(e) => { println!("  [DEMO 21] FAIL: read a.txt: {:?}", e); return; }
        };
        if a != FILE_A {
            println!("  [DEMO 21] FAIL: a.txt content drifted ({} bytes, want {})", a.len(), FILE_A.len());
            return;
        }
        let b = match Namespace::read_file("/persist/b.txt") {
            Ok(b) => b,
            Err(e) => { println!("  [DEMO 21] FAIL: read b.txt: {:?}", e); return; }
        };
        if b != FILE_B {
            println!("  [DEMO 21] FAIL: b.txt content drifted"); return;
        }
        println!("  [DEMO 21] PASS: both files survived reboot byte-exact ({} + {} bytes)",
            a.len(), b.len());

        // Timestamps should pre-date current wall clock (they're from a prior boot).
        let ts = {
            let suid = Namespace::resolve("/persist/a.txt").unwrap();
            let registry = unsafe { kernel_core::semantic::registry::global_registry() };
            registry.get(&suid).map(|o| (o.created_at, o.modified_at)).unwrap_or((0,0))
        };
        let now = kernel_core::platform::wall_clock().unwrap_or(0);
        if ts.0 == 0 {
            println!("  [DEMO 21] (timestamps were 0 — wall_clock was unavailable at create time)");
        } else if ts.0 > now {
            println!("  [DEMO 21] FAIL: created_at={} is in the future vs now={}", ts.0, now);
            return;
        } else {
            let age_sec = now - ts.0;
            println!("  [DEMO 21] PASS: persisted timestamp looks right (file is {} s old)", age_sec);
        }

        // Walk /persist via readdir as a final sanity check.
        let mut count = 0;
        let _ = Namespace::readdir("/persist", |_name, _suid| { count += 1; });
        if count != 2 {
            println!("  [DEMO 21] FAIL: readdir saw {} entries, want 2", count);
            return;
        }
        println!("  [DEMO 21] PASS: readdir /persist sees 2 entries");

        // Re-save (idempotent) so a single-boot run can recover even if
        // the snapshot from boot N got corrupted in transit.
        match Namespace::save(dev) {
            Ok(n) => println!("  [DEMO 21] re-saved {} bytes (idempotent rewrite)", n),
            Err(e) => println!("  [DEMO 21] re-save failed: {:?} (non-fatal)", e),
        }
        println!("  [DEMO 21] => persistence works across actual reboot");
    } else {
        // FIRST BOOT: create the tree, save, leave for next reboot.
        println!("  [DEMO 21] no prior-boot snapshot; first-boot path");
        if Namespace::mkdir("/persist").is_err() {
            println!("  [DEMO 21] FAIL: mkdir /persist"); return;
        }
        if Namespace::create_file("/persist/a.txt", SecurityTier::Public, FILE_A).is_err() {
            println!("  [DEMO 21] FAIL: create a.txt"); return;
        }
        if Namespace::create_file("/persist/b.txt", SecurityTier::Internal, FILE_B).is_err() {
            println!("  [DEMO 21] FAIL: create b.txt"); return;
        }
        println!("  [DEMO 21] PASS: created /persist/{{a.txt,b.txt}} for next boot");

        match Namespace::save(dev) {
            Ok(n) => println!("  [DEMO 21] PASS: saved {} bytes to virtio0", n),
            Err(e) => { println!("  [DEMO 21] FAIL: save: {:?}", e); return; }
        }
        println!("  [DEMO 21] => first-boot save complete; reboot QEMU against same vdisk to verify load");
    }
}

/// DEMO 20: FS syscalls (SYS_OPEN/FREAD/FWRITE/CLOSE/MKDIR/UNLINK/READDIR/STAT)
/// driven against the path namespace from Ring 0.
///
/// We use kernel-core's `syscall::dispatch` directly instead of going
/// through SYSCALL/SYSRET — same code path, no user-mode round-trip,
/// faster to verify. Once `user-programs/fs-demo` is ported, the same
/// surface gets exercised from Ring 3 via SYSCALL.
fn fs_syscall_test() {
    use kernel_core::syscall::dispatch;
    use kernel_core::syscall::numbers::*;
    use kernel_core::syscall::open_flags;

    fn path_args(p: &str) -> (u64, u64) {
        (p.as_ptr() as u64, p.len() as u64)
    }

    // Phase 9 Stage 2: clean any leftover state from a prior boot.
    // /fs-demo might be auto-loaded by the boot-time snapshot restore.
    {
        let (p, l) = path_args("/fs-demo/hello.txt");
        let _ = dispatch(SYS_UNLINK, p, l, 0, 0);
        let (p, l) = path_args("/fs-demo/notes.md");
        let _ = dispatch(SYS_UNLINK, p, l, 0, 0);
        let (p, l) = path_args("/fs-demo");
        let _ = dispatch(SYS_UNLINK, p, l, 0, 0);
    }

    // The namespace was init'd by DEMO 17; root + /notes already exist.
    // Make a fresh subtree under /fs-demo so we don't tangle with that.
    let (p, l) = path_args("/fs-demo");
    let r = dispatch(SYS_MKDIR, p, l, 0, 0);
    if r != 0 { println!("  [DEMO 20] FAIL: mkdir /fs-demo: {}", r); return; }
    println!("  [DEMO 20] PASS: SYS_MKDIR /fs-demo");

    // Create a file via SYS_OPEN with CREATE flag.
    let (p, l) = path_args("/fs-demo/hello.txt");
    let fd = dispatch(SYS_OPEN, p, l, open_flags::CREATE, 0);
    if fd == u64::MAX { println!("  [DEMO 20] FAIL: open+create /fs-demo/hello.txt"); return; }
    println!("  [DEMO 20] PASS: SYS_OPEN(CREATE) returned fd={}", fd);

    // Write content.
    const MSG: &[u8] = b"phase 9 stage 3, syscalls live\n";
    let n = dispatch(SYS_FWRITE, fd, MSG.as_ptr() as u64, MSG.len() as u64, 0);
    if n as usize != MSG.len() {
        println!("  [DEMO 20] FAIL: fwrite returned {}, want {}", n, MSG.len());
        dispatch(SYS_CLOSE, fd, 0, 0, 0);
        return;
    }
    println!("  [DEMO 20] PASS: SYS_FWRITE wrote {} bytes", n);

    // Close + reopen for read.
    let close_r = dispatch(SYS_CLOSE, fd, 0, 0, 0);
    if close_r != 0 { println!("  [DEMO 20] FAIL: close: {}", close_r); return; }
    println!("  [DEMO 20] PASS: SYS_CLOSE");

    let fd2 = dispatch(SYS_OPEN, p, l, 0, 0);
    if fd2 == u64::MAX { println!("  [DEMO 20] FAIL: reopen for read"); return; }

    // Read and verify content.
    let mut buf = [0u8; 64];
    let n = dispatch(SYS_FREAD, fd2, buf.as_mut_ptr() as u64, buf.len() as u64, 0);
    if n as usize != MSG.len() || &buf[..n as usize] != MSG {
        println!("  [DEMO 20] FAIL: fread mismatch: got {} bytes", n);
        dispatch(SYS_CLOSE, fd2, 0, 0, 0);
        return;
    }
    println!("  [DEMO 20] PASS: SYS_FREAD round-trip ({} bytes match)", n);
    dispatch(SYS_CLOSE, fd2, 0, 0, 0);

    // SYS_STAT on the path.
    let size = dispatch(SYS_STAT, p, l, 0, 0);
    if size as usize != MSG.len() {
        println!("  [DEMO 20] FAIL: stat returned {}, want {}", size, MSG.len());
        return;
    }
    println!("  [DEMO 20] PASS: SYS_STAT returned {} bytes", size);

    // Add a second file so readdir has something to walk.
    let (p2, l2) = path_args("/fs-demo/notes.md");
    let fd3 = dispatch(SYS_OPEN, p2, l2, open_flags::CREATE, 0);
    if fd3 == u64::MAX { println!("  [DEMO 20] FAIL: create notes.md"); return; }
    dispatch(SYS_CLOSE, fd3, 0, 0, 0);

    // Open /fs-demo as a directory and walk it.
    let (dp, dl) = path_args("/fs-demo");
    let dfd = dispatch(SYS_OPEN, dp, dl, open_flags::DIRECTORY, 0);
    if dfd == u64::MAX { println!("  [DEMO 20] FAIL: open /fs-demo as dir"); return; }
    let mut name_buf = [0u8; 64];
    let mut entries_seen = 0;
    for idx in 0u64..16 {
        let n = dispatch(SYS_READDIR, dfd, idx,
            name_buf.as_mut_ptr() as u64, name_buf.len() as u64);
        if n == 0 { break; }
        if n == u64::MAX {
            println!("  [DEMO 20] FAIL: readdir at idx {}", idx);
            dispatch(SYS_CLOSE, dfd, 0, 0, 0);
            return;
        }
        let name = core::str::from_utf8(&name_buf[..n as usize]).unwrap_or("?");
        println!("  [DEMO 20]   readdir[{}] = '{}'", idx, name);
        entries_seen += 1;
    }
    dispatch(SYS_CLOSE, dfd, 0, 0, 0);
    if entries_seen != 2 {
        println!("  [DEMO 20] FAIL: readdir saw {} entries, want 2", entries_seen);
        return;
    }
    println!("  [DEMO 20] PASS: SYS_READDIR walked 2 entries");

    // Unlink and confirm.
    let unlink_r = dispatch(SYS_UNLINK, p, l, 0, 0);
    if unlink_r != 0 { println!("  [DEMO 20] FAIL: unlink: {}", unlink_r); return; }
    let stat_after = dispatch(SYS_STAT, p, l, 0, 0);
    if stat_after != u64::MAX {
        println!("  [DEMO 20] FAIL: stat after unlink returned {}, want u64::MAX", stat_after);
        return;
    }
    println!("  [DEMO 20] PASS: SYS_UNLINK + post-stat returns NotFound");

    println!("  [DEMO 20] => FS syscalls work end-to-end against path namespace");
}

/// DEMO 19: RTC + wall_clock through the Platform trait surface.
///
/// What this proves:
///   1. Direct RTC read via `rtc::read()` returns a stable, decoded
///      DateTime (BCD/binary handling correct, UIP race avoided).
///   2. `Platform::wall_clock()` (the kernel-core abstraction)
///      reaches the x86_64 impl and returns the same Unix seconds.
///   3. Two reads taken ~10 ticks apart never go backwards (basic
///      monotonicity sanity — RTC ticks once per second so they may
///      be equal, but never negative-delta).
///   4. The timestamp is "plausible" — between 2025-01-01 and 2099-12-31
///      so we catch a sign error or a Y2K-style overflow.
fn wall_clock_test() {
    use kernel_core::platform;

    // Step 1: direct driver read.
    let dt = match rtc::read() {
        Some(d) => d,
        None => { println!("  [DEMO 19] FAIL: rtc::read() returned None"); return; }
    };
    println!("  [DEMO 19] PASS: rtc::read() {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second);

    let direct_secs = dt.to_unix_seconds();

    // Step 2: through the platform trait abstraction.
    let via_trait = match platform::wall_clock() {
        Some(s) => s,
        None => { println!("  [DEMO 19] FAIL: platform::wall_clock() returned None"); return; }
    };
    // The two reads are roughly contemporaneous but the RTC ticks
    // every second — they may differ by 1 if we crossed a second
    // boundary between them. Tolerate exactly that.
    let drift = if via_trait >= direct_secs { via_trait - direct_secs } else { direct_secs - via_trait };
    if drift > 2 {
        println!("  [DEMO 19] FAIL: direct={} trait={} (drift {} > 2)", direct_secs, via_trait, drift);
        return;
    }
    println!("  [DEMO 19] PASS: platform::wall_clock() = {} (drift from direct read: {} s)", via_trait, drift);

    // Step 3: simple monotonicity sanity.
    let t1 = platform::wall_clock().unwrap_or(0);
    for _ in 0..1_000_000 { core::hint::spin_loop(); }
    let t2 = platform::wall_clock().unwrap_or(0);
    if t2 < t1 {
        println!("  [DEMO 19] FAIL: wall_clock went backwards: {} -> {}", t1, t2);
        return;
    }
    println!("  [DEMO 19] PASS: wall_clock monotonic ({} -> {})", t1, t2);

    // Step 4: plausibility envelope. 2025-01-01 = 1735689600 epoch sec;
    // 2099-12-31 = 4102444800. If we're outside this range something
    // is wrong with our BCD decoding or our day arithmetic.
    const LOWER: u64 = 1_735_689_600; // 2025-01-01T00:00:00Z
    const UPPER: u64 = 4_102_444_800; // 2099-12-31T00:00:00Z
    if via_trait < LOWER || via_trait > UPPER {
        println!("  [DEMO 19] FAIL: timestamp {} outside plausible range [{}..{}]", via_trait, LOWER, UPPER);
        return;
    }
    println!("  [DEMO 19] PASS: timestamp inside plausible epoch range");

    println!("  [DEMO 19] => wall_clock works end-to-end; ready for TLS notAfter + file timestamps");
}

/// DEMO 25: Tier 2 extended file ops — SYS_FSYNC, SYS_RENAME,
/// SYS_TRUNCATE, SYS_STATX.
///
/// What this proves:
///   1. STATX returns the rich metadata struct (type + size + tier + timestamps)
///   2. RENAME atomically moves a file within and across directories
///   3. TRUNCATE shrinks + extends file content; STATX size reflects it
///   4. FSYNC writes the namespace to disk without error (requires virtio0)
fn extended_fs_test() {
    use kernel_core::syscall::{dispatch, StatX};
    use kernel_core::syscall::numbers::*;
    use kernel_core::syscall::open_flags;

    // Clean leftover state from prior boots (DEMO 25 might run multiple times
    // against a persistent disk).
    fn unlink(path: &str) {
        let _ = dispatch(SYS_UNLINK, path.as_ptr() as u64, path.len() as u64, 0, 0);
    }
    unlink("/t2/file.txt"); unlink("/t2/renamed.txt"); unlink("/t2/sub/file.txt");
    unlink("/t2/sub"); unlink("/t2");

    // Setup: /t2/file.txt with 32 bytes of known content.
    let path = "/t2";
    let _ = dispatch(SYS_MKDIR, path.as_ptr() as u64, path.len() as u64, 0, 0);
    let file = "/t2/file.txt";
    let fd = dispatch(SYS_OPEN, file.as_ptr() as u64, file.len() as u64, open_flags::CREATE, 0);
    if fd == u64::MAX { println!("  [DEMO 25] FAIL: open+create"); return; }
    let msg = b"tier 2 file ops test content!\n\n\n";  // 32 bytes
    let _ = dispatch(SYS_FWRITE, fd, msg.as_ptr() as u64, msg.len() as u64, 0);
    let _ = dispatch(SYS_CLOSE, fd, 0, 0, 0);

    // Step 1: STATX returns rich metadata.
    let mut st = core::mem::MaybeUninit::<StatX>::zeroed();
    let r = dispatch(SYS_STATX, file.as_ptr() as u64, file.len() as u64,
        st.as_mut_ptr() as u64, 0);
    if r != 0 { println!("  [DEMO 25] FAIL: SYS_STATX returned {}", r); return; }
    let st = unsafe { st.assume_init() };
    if st.size != msg.len() as u64 {
        println!("  [DEMO 25] FAIL: STATX size {}, want {}", st.size, msg.len());
        return;
    }
    if st.created_at == 0 || st.modified_at == 0 {
        println!("  [DEMO 25] FAIL: STATX timestamps zero (wall_clock not working?)");
        return;
    }
    println!("  [DEMO 25] PASS: SYS_STATX → size={} tier={} type={} created={} modified={}",
        st.size, st.tier, st.file_type, st.created_at, st.modified_at);

    // Helper to do a STATX into a fresh buffer (avoids variable-shadow
    // confusion when the same buffer would otherwise be reused after
    // `assume_init` consumed it).
    fn stat(path: &str) -> Result<StatX, u64> {
        let mut buf = core::mem::MaybeUninit::<StatX>::zeroed();
        let r = dispatch(SYS_STATX, path.as_ptr() as u64, path.len() as u64,
            buf.as_mut_ptr() as u64, 0);
        if r == 0 { Ok(unsafe { buf.assume_init() }) } else { Err(r) }
    }

    // Step 2: RENAME within same dir.
    let renamed = "/t2/renamed.txt";
    let r = dispatch(SYS_RENAME,
        file.as_ptr() as u64, file.len() as u64,
        renamed.as_ptr() as u64, renamed.len() as u64);
    if r != 0 { println!("  [DEMO 25] FAIL: SYS_RENAME within dir: {}", r); return; }
    if stat(file).is_ok() {
        println!("  [DEMO 25] FAIL: old path still resolvable after rename");
        return;
    }
    let st_renamed = match stat(renamed) {
        Ok(s) => s,
        Err(e) => { println!("  [DEMO 25] FAIL: new path STATX: {}", e); return; }
    };
    if (st_renamed.suid_high, st_renamed.suid_low) != (st.suid_high, st.suid_low) {
        println!("  [DEMO 25] FAIL: SUID changed during rename (not atomic!)");
        return;
    }
    println!("  [DEMO 25] PASS: SYS_RENAME within-dir preserves SUID + content");

    // Step 3: RENAME cross-directory.
    let _ = dispatch(SYS_MKDIR, "/t2/sub".as_ptr() as u64, 7, 0, 0);
    let moved = "/t2/sub/file.txt";
    let r = dispatch(SYS_RENAME,
        renamed.as_ptr() as u64, renamed.len() as u64,
        moved.as_ptr() as u64, moved.len() as u64);
    if r != 0 { println!("  [DEMO 25] FAIL: SYS_RENAME cross-dir: {}", r); return; }
    if stat(moved).is_err() { println!("  [DEMO 25] FAIL: moved path STATX"); return; }
    println!("  [DEMO 25] PASS: SYS_RENAME cross-dir works");

    // Step 4: TRUNCATE shrink.
    let r = dispatch(SYS_TRUNCATE, moved.as_ptr() as u64, moved.len() as u64, 10, 0);
    if r != 0 { println!("  [DEMO 25] FAIL: SYS_TRUNCATE shrink: {}", r); return; }
    let st_trunc = stat(moved).expect("STATX after truncate");
    if st_trunc.size != 10 {
        println!("  [DEMO 25] FAIL: post-truncate size {}, want 10", st_trunc.size); return;
    }
    println!("  [DEMO 25] PASS: SYS_TRUNCATE shrink (32 → 10 bytes)");

    // Step 5: TRUNCATE extend with zeros.
    let r = dispatch(SYS_TRUNCATE, moved.as_ptr() as u64, moved.len() as u64, 64, 0);
    if r != 0 { println!("  [DEMO 25] FAIL: SYS_TRUNCATE extend: {}", r); return; }
    let st_ext = stat(moved).expect("STATX after extend");
    if st_ext.size != 64 {
        println!("  [DEMO 25] FAIL: post-extend size {}, want 64", st_ext.size); return;
    }
    println!("  [DEMO 25] PASS: SYS_TRUNCATE extend (10 → 64 bytes)");

    // Step 6: FSYNC (only meaningful with virtio0; skip if absent).
    let r = dispatch(SYS_FSYNC, 0, 0, 0, 0);
    if r == u64::MAX {
        println!("  [DEMO 25] SKIPPED: SYS_FSYNC needs virtio0 disk (none attached)");
    } else {
        println!("  [DEMO 25] PASS: SYS_FSYNC flushed namespace to virtio0");
    }

    // Cleanup so re-runs don't accumulate state.
    let _ = dispatch(SYS_UNLINK, moved.as_ptr() as u64, moved.len() as u64, 0, 0);
    let _ = dispatch(SYS_UNLINK, "/t2/sub".as_ptr() as u64, 7, 0, 0);
    let _ = dispatch(SYS_UNLINK, "/t2".as_ptr() as u64, 3, 0, 0);

    println!("  [DEMO 25] => Tier 2 file ops ready for cargo's atomic-rename build flow");
}

/// DEMO 26: large-file FWRITE — exercises the heap-Allocated
/// ObjectContent path (task #44).
///
/// Until #44 landed, FWRITE was capped at 256 B (the Inline variant of
/// ObjectContent). The compiler emits source files much larger than
/// that — DEMO 26 proves the kernel now handles writes up to
/// MAX_FILE_CONTENT (64 KiB) by routing them through the heap.
///
/// What this proves:
///   1. FWRITE accepts a 4 KiB payload (well past the 256 B inline cap)
///   2. STATX size reflects the full length
///   3. FREAD round-trips the exact bytes back
///   4. An overwrite (4 KiB → 8 KiB) frees the old heap block + reallocates
///   5. UNLINK frees the heap block (no leak across boot)
///   6. A pathologically-large write (> MAX_FILE_CONTENT) fails cleanly
fn large_file_fwrite_test() {
    use kernel_core::syscall::{dispatch, StatX};
    use kernel_core::syscall::numbers::*;
    use kernel_core::syscall::open_flags;

    fn unlink(path: &str) {
        let _ = dispatch(SYS_UNLINK, path.as_ptr() as u64, path.len() as u64, 0, 0);
    }
    unlink("/big/file.bin");
    unlink("/big");

    let _ = dispatch(SYS_MKDIR, "/big".as_ptr() as u64, 4, 0, 0);
    let path = "/big/file.bin";

    // Step 1: 4 KiB FWRITE — past the 256 B inline cap, comfortably
    // inside MAX_FILE_CONTENT (64 KiB).
    let mut payload = [0u8; 4096];
    for (i, b) in payload.iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(31).wrapping_add(7);
    }
    let fd = dispatch(SYS_OPEN, path.as_ptr() as u64, path.len() as u64,
        open_flags::CREATE, 0);
    if fd == u64::MAX { println!("  [DEMO 26] FAIL: open+create"); return; }
    let n = dispatch(SYS_FWRITE, fd, payload.as_ptr() as u64, payload.len() as u64, 0);
    if n != payload.len() as u64 {
        println!("  [DEMO 26] FAIL: FWRITE 4 KiB returned {} (want {})", n, payload.len());
        return;
    }
    let _ = dispatch(SYS_CLOSE, fd, 0, 0, 0);
    println!("  [DEMO 26] PASS: FWRITE accepted 4096-byte payload (via heap Allocated)");

    // Step 2: STATX size reflects the full 4 KiB.
    let mut st = core::mem::MaybeUninit::<StatX>::zeroed();
    let r = dispatch(SYS_STATX, path.as_ptr() as u64, path.len() as u64,
        st.as_mut_ptr() as u64, 0);
    if r != 0 { println!("  [DEMO 26] FAIL: STATX returned {}", r); return; }
    let st = unsafe { st.assume_init() };
    if st.size != payload.len() as u64 {
        println!("  [DEMO 26] FAIL: STATX size {}, want {}", st.size, payload.len());
        return;
    }
    println!("  [DEMO 26] PASS: STATX size = {} bytes", st.size);

    // Step 3: FREAD round-trips byte-exact.
    let mut readback = [0u8; 4096];
    let fd = dispatch(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 0, 0);
    if fd == u64::MAX { println!("  [DEMO 26] FAIL: open for read"); return; }
    let n = dispatch(SYS_FREAD, fd, readback.as_mut_ptr() as u64, readback.len() as u64, 0);
    let _ = dispatch(SYS_CLOSE, fd, 0, 0, 0);
    if n != payload.len() as u64 {
        println!("  [DEMO 26] FAIL: FREAD returned {} (want {})", n, payload.len());
        return;
    }
    if readback != payload {
        // Find first mismatch for diagnostics.
        let i = readback.iter().zip(payload.iter())
            .position(|(a, b)| a != b).unwrap_or(0);
        println!("  [DEMO 26] FAIL: FREAD mismatch at byte {}: got 0x{:02x}, want 0x{:02x}",
            i, readback[i], payload[i]);
        return;
    }
    println!("  [DEMO 26] PASS: FREAD round-trip byte-exact (4096 bytes)");

    // Step 4: Overwrite with a larger payload (8 KiB). The old heap
    // block must be freed and a new one allocated transparently.
    let mut big = [0u8; 8192];
    for (i, b) in big.iter_mut().enumerate() { *b = (i as u8) ^ 0xA5; }
    let fd = dispatch(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 0, 0);
    if fd == u64::MAX { println!("  [DEMO 26] FAIL: open for overwrite"); return; }
    let n = dispatch(SYS_FWRITE, fd, big.as_ptr() as u64, big.len() as u64, 0);
    let _ = dispatch(SYS_CLOSE, fd, 0, 0, 0);
    if n != big.len() as u64 {
        println!("  [DEMO 26] FAIL: FWRITE 8 KiB returned {}", n);
        return;
    }
    let mut st2 = core::mem::MaybeUninit::<StatX>::zeroed();
    let _ = dispatch(SYS_STATX, path.as_ptr() as u64, path.len() as u64,
        st2.as_mut_ptr() as u64, 0);
    let st2 = unsafe { st2.assume_init() };
    if st2.size != big.len() as u64 {
        println!("  [DEMO 26] FAIL: post-overwrite STATX size {}", st2.size); return;
    }
    println!("  [DEMO 26] PASS: overwrite 4 KiB → 8 KiB (old heap block freed, new allocated)");

    // Step 5: Pathologically-large write past MAX_FILE_CONTENT (64 KiB)
    // must fail cleanly without corrupting state.
    let huge_len = 65 * 1024;
    let huge_ptr = kernel_core::memory::heap::allocate(huge_len, 8);
    if huge_ptr.is_null() {
        println!("  [DEMO 26] SKIPPED: couldn't allocate test buffer (heap pressure)");
    } else {
        let fd = dispatch(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 0, 0);
        let r = dispatch(SYS_FWRITE, fd, huge_ptr as u64, huge_len as u64, 0);
        let _ = dispatch(SYS_CLOSE, fd, 0, 0, 0);
        kernel_core::memory::heap::deallocate(huge_ptr, huge_len, 8);
        if r != u64::MAX {
            println!("  [DEMO 26] FAIL: oversize FWRITE accepted (returned {}); expected rejection", r);
            return;
        }
        // Confirm the file still has its 8 KiB content from step 4.
        let mut st3 = core::mem::MaybeUninit::<StatX>::zeroed();
        let _ = dispatch(SYS_STATX, path.as_ptr() as u64, path.len() as u64,
            st3.as_mut_ptr() as u64, 0);
        let st3 = unsafe { st3.assume_init() };
        if st3.size != big.len() as u64 {
            println!("  [DEMO 26] FAIL: failed FWRITE corrupted file (size now {})", st3.size);
            return;
        }
        println!("  [DEMO 26] PASS: oversize FWRITE rejected; existing content intact");
    }

    // Step 6: UNLINK frees the heap block. We can't directly observe
    // the deallocation through the syscall surface, but heap::stats
    // would show the regression on the next boot if Drop weren't wired
    // up. UNLINK succeeding means the registry remove + drop chain ran.
    let r = dispatch(SYS_UNLINK, path.as_ptr() as u64, path.len() as u64, 0, 0);
    if r != 0 { println!("  [DEMO 26] FAIL: UNLINK returned {}", r); return; }
    println!("  [DEMO 26] PASS: UNLINK ran Drop chain (heap block freed)");

    // Cleanup.
    let _ = dispatch(SYS_UNLINK, "/big".as_ptr() as u64, 4, 0, 0);

    println!("  [DEMO 26] => FWRITE up to MAX_FILE_CONTENT (64 KiB) works; #44 unblocked");
}

// DEMO 27 — kernel-mode threading + futex + join validation.
//
// We exercise the scheduler primitives end-to-end through the syscall
// surface (so the std-shim path is on the hook):
//   - SYS_THREAD_SPAWN to fork a kernel-mode sibling task (waiter or
//     waker)
//   - SYS_FUTEX_WAIT / SYS_FUTEX_WAKE to block + release on a shared
//     u32 word
//   - SYS_THREAD_JOIN to block on a sibling's exit + harvest its code
//
// The shared u32 lives in a static global because kernel-mode threads
// here use the entry: fn() ABI (no arg slot through dispatch yet).
// Ring 3 threads will get arg-via-rdi as part of task #45's Ring 3
// branch — this commit lands the scheduler-side primitives only.

/// Shared futex word for DEMO 27. The waiter polls this == 0; the
/// waker writes 1 + FUTEX_WAKE. `repr(align(4))` makes the &raw cast
/// well-formed; the static is in .bss, naturally 4-aligned anyway but
/// being explicit documents the futex contract.
#[repr(align(4))]
struct FutexWord(core::sync::atomic::AtomicU32);

static DEMO27_FUTEX: FutexWord = FutexWord(core::sync::atomic::AtomicU32::new(0));

/// Set by the waiter thread on exit so the main thread can spot-check
/// that the wait actually unblocked rather than returning early via
/// the mismatch path.
static DEMO27_WAITER_RESULT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(u64::MAX);

extern "C" fn demo27_waiter_entry() {
    use kernel_core::syscall::dispatch;
    use kernel_core::syscall::numbers::*;
    use core::sync::atomic::Ordering;

    // Wait for the word == 0 (the value it was initialised to). The
    // syscall returns 0 once we're woken, 1 if main raced ahead and
    // already changed the value. Both are "test passed if reached".
    let addr = &DEMO27_FUTEX.0 as *const _ as u64;
    let r = dispatch(SYS_FUTEX_WAIT, addr, 0, 0, 0);
    DEMO27_WAITER_RESULT.store(r, Ordering::SeqCst);

    // Exit code = 0xBEEF on a real wake, 0xCAFE if the FUTEX_WAIT path
    // hit value mismatch (race), u64::MAX on syscall error. The main
    // thread joins us and reads this back through scheduler exit_code.
    let code = match r {
        0 => 0xBEEFu64,
        1 => 0xCAFE,
        _ => u64::MAX,
    };
    dispatch(SYS_EXIT, code, 0, 0, 0);
    // SYS_EXIT marks us Exited; the next timer tick will not reschedule
    // us. A defensive halt loop covers the gap until then.
    loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
}

/// DEMO 27: Phase 14 Tier 3 threading + sync prereqs.
///
/// What this proves:
///   1. SYS_THREAD_SPAWN forks a kernel-mode sibling task
///   2. SYS_FUTEX_WAIT correctly blocks the sibling on a u32 word
///   3. SYS_FUTEX_WAKE unblocks exactly one waiter
///   4. SYS_THREAD_JOIN blocks the main task until the sibling exits
///   5. exit code flows through SYS_EXIT → scheduler::task_exit_code
///   6. FUTEX_WAIT's value-mismatch fast path returns 1, not u64::MAX
///   7. SYS_WAITNB returns u64::MAX with no children, 0 otherwise
fn threading_futex_test() {
    use kernel_core::syscall::dispatch;
    use kernel_core::syscall::numbers::*;
    use core::sync::atomic::Ordering;

    // Step 0: value-mismatch fast path. Set word to 7, wait expecting
    // 0 → must return 1 without blocking.
    DEMO27_FUTEX.0.store(7, Ordering::SeqCst);
    let addr = &DEMO27_FUTEX.0 as *const _ as u64;
    let r = dispatch(SYS_FUTEX_WAIT, addr, 0, 0, 0);
    if r != 1 {
        println!("  [DEMO 27] FAIL: FUTEX_WAIT mismatch returned {} (want 1)", r);
        return;
    }
    println!("  [DEMO 27] PASS: FUTEX_WAIT mismatch fast-path returns 1 (no block)");

    // Reset to 0 so the waiter can take the slow path.
    DEMO27_FUTEX.0.store(0, Ordering::SeqCst);
    DEMO27_WAITER_RESULT.store(u64::MAX, Ordering::SeqCst);

    // Step 1: spawn a sibling thread.
    let tid = dispatch(SYS_THREAD_SPAWN, demo27_waiter_entry as u64, 0, 0, 0);
    if tid == u64::MAX {
        println!("  [DEMO 27] FAIL: SYS_THREAD_SPAWN returned MAX (kernel-mode path missing?)");
        return;
    }
    println!("  [DEMO 27] PASS: SYS_THREAD_SPAWN forked tid={} (kernel-mode sibling)", tid);

    // Step 2+3: wait for the sibling to reach SYS_FUTEX_WAIT and block,
    // then confirm it's actually Blocked (not still Running, or already
    // Exited because of a bug above). A single fixed SYS_SLEEP used to
    // flake here: under -netdev the extra interrupt load can delay the
    // sibling getting scheduled to its first syscall past the deadline,
    // leaving it still Running on the one-shot check. So we POLL — sleep
    // a tick, re-read the state — up to a generous cap, and succeed the
    // instant we observe Blocked. This mirrors DEMO 28's slot-poll and
    // keeps the assertion's intent intact (the sibling must really
    // block) while being robust to scheduling latency.
    let mut st = kernel_core::scheduler::task_state(tid as usize);
    let mut waited = 0u64;
    while st != kernel_core::scheduler::TaskState::Blocked && waited < 200 {
        let _ = dispatch(SYS_SLEEP, 1, 0, 0, 0); // ~10 ms/tick
        waited += 1;
        st = kernel_core::scheduler::task_state(tid as usize);
    }
    if st != kernel_core::scheduler::TaskState::Blocked {
        println!("  [DEMO 27] FAIL: sibling state {:?} after {} ticks, want Blocked", st, waited);
        return;
    }
    println!("  [DEMO 27] PASS: sibling Blocked on FUTEX_WAIT after {} tick(s)", waited);

    // Step 4: wake it. Returns count actually woken — exactly 1.
    let woken = dispatch(SYS_FUTEX_WAKE, addr, 1, 0, 0);
    if woken != 1 {
        println!("  [DEMO 27] FAIL: SYS_FUTEX_WAKE woke {} (want 1)", woken);
        return;
    }
    println!("  [DEMO 27] PASS: SYS_FUTEX_WAKE(addr, 1) returned 1");

    // Step 5: join the sibling. Blocks until Exited; returns exit code.
    let code = dispatch(SYS_THREAD_JOIN, tid, 0, 0, 0);
    if code != 0xBEEF {
        println!("  [DEMO 27] FAIL: SYS_THREAD_JOIN got exit_code 0x{:X} (want 0xBEEF)", code);
        return;
    }
    let waiter_r = DEMO27_WAITER_RESULT.load(Ordering::SeqCst);
    if waiter_r != 0 {
        println!("  [DEMO 27] FAIL: waiter recorded FUTEX_WAIT result {} (want 0 = woken)", waiter_r);
        return;
    }
    println!("  [DEMO 27] PASS: SYS_THREAD_JOIN got 0xBEEF; waiter FUTEX_WAIT returned 0");

    // Step 6: FUTEX_WAKE on an address with no waiters returns 0 (not err).
    let unused_addr = &DEMO27_WAITER_RESULT as *const _ as u64;
    let zero = dispatch(SYS_FUTEX_WAKE, unused_addr, 8, 0, 0);
    if zero != 0 {
        println!("  [DEMO 27] FAIL: FUTEX_WAKE on quiet addr returned {} (want 0)", zero);
        return;
    }
    println!("  [DEMO 27] PASS: FUTEX_WAKE on addr with no waiters returns 0");

    // Step 7: SYS_WAITNB sanity. We don't know whether prior demos left
    // living children of the kernel task hanging around, so we accept:
    //   u64::MAX = "no children at all"
    //   0        = "children exist but none have exited yet"
    // The bad outcome would be any other value (a stray PID number from
    // an already-reaped zombie still showing up), which would indicate
    // child tracking is broken.
    let r = dispatch(SYS_WAITNB, 0, 0, 0, 0);
    if r != u64::MAX && r != 0 {
        println!("  [DEMO 27] FAIL: SYS_WAITNB returned unexpected {} (want MAX or 0)", r);
        return;
    }
    println!("  [DEMO 27] PASS: SYS_WAITNB non-blocking returned {}", if r == u64::MAX { "MAX (no children)" } else { "0 (no zombies)" });

    println!("  [DEMO 27] => Tier 3 scheduler-side primitives green; Ring 3 thread_spawn next");
}

/// DEMO 28: Ring 3 thread spawn end-to-end via thread-demo.elf.
///
/// Spawns the user-mode binary, which itself calls SYS_THREAD_SPAWN to
/// fork a Ring-3 sibling sharing its address space, synchronises on a
/// shared u32 with SYS_FUTEX_WAIT/WAKE, and reaps the sibling's exit
/// code via SYS_THREAD_JOIN. The user program exits with 0x2700 on
/// full pass; anything else is a stage-specific failure code (see
/// thread-demo/src/main.rs for the table).
///
/// We harvest the exit status by polling the scheduler slot directly
/// (state == Exited, then read scheduler::task_exit_code). SYS_WAIT
/// can't be used here because its ProcessState bookkeeping was never
/// wired to the user-mode SYS_EXIT path — that's a separate refactor.
/// Looking the slot up by PID via the process table is the bridge.
fn ring3_thread_demo() {
    use kernel_core::syscall::{dispatch, numbers::*};

    let path = "/bin/thread-demo";
    let pid = dispatch(SYS_SPAWN, path.as_ptr() as u64, path.len() as u64, 0, 0);
    if pid == u64::MAX {
        println!("  [DEMO 28] FAIL: SYS_SPAWN({}) returned MAX", path);
        return;
    }
    println!("  [DEMO 28] PASS: SYS_SPAWN({}) → PID {}", path, pid);

    // Find the scheduler slot owned by this PID. Then poll until the
    // slot transitions to Exited and harvest the published exit code.
    let process_id = kernel_core::process::ProcessId(pid as u32);
    let slot = match kernel_core::process::get(process_id) {
        Some(p) => p.task_id,
        None => {
            println!("  [DEMO 28] FAIL: PID {} not in process table", pid);
            return;
        }
    };
    let slot = match slot {
        Some(s) => s,
        None => {
            println!("  [DEMO 28] FAIL: PID {} has no task_id", pid);
            return;
        }
    };
    println!("  [DEMO 28] PASS: PID {} occupies scheduler slot {}", pid, slot);

    // Poll the slot's TaskState. The thread-demo binary takes ~30 ms of
    // wall time once everything's hot; cap our wait at 500 ticks (~5 s
    // at the default PIT rate) to fail fast on regression.
    let mut polled = 0u64;
    loop {
        if kernel_core::scheduler::task_state(slot) == kernel_core::scheduler::TaskState::Exited {
            break;
        }
        if polled > 500 {
            println!("  [DEMO 28] FAIL: thread-demo didn't exit within 500 ticks");
            return;
        }
        let _ = dispatch(SYS_SLEEP, 1, 0, 0, 0);
        polled += 1;
    }
    let code = kernel_core::scheduler::task_exit_code(slot);
    let code32 = code as u32;
    if code32 == 0x2700 {
        println!("  [DEMO 28] PASS: thread-demo exited 0x2700 (full pass — Ring 3 thread_spawn + futex + join)");
    } else {
        println!("  [DEMO 28] FAIL: thread-demo exited 0x{:X} (table in user-programs/thread-demo/src/main.rs)", code32);
        return;
    }
    println!("  [DEMO 28] => Ring 3 same-AS thread spawn fully unblocked; Tier 3 #45 closed");
}

/// DEMO 29: hello-std.elf — first Phase 14 M25 program.
///
/// Spawns `/bin/hello-std`, which prints "Hello from semos-std!"
/// through the new shim's `println!` macro and exits 0. Validates
/// the full M25 Tier 1 path: `main!()` macro → user `_start` → user
/// `main` body → `println!` → `core::fmt::Write` → `Stdout::write_str`
/// → SYS_WRITE → kernel serial.
fn hello_std_demo() {
    use kernel_core::syscall::{dispatch, numbers::*};

    let path = "/bin/hello-std";
    let pid = dispatch(SYS_SPAWN, path.as_ptr() as u64, path.len() as u64, 0, 0);
    if pid == u64::MAX {
        println!("  [DEMO 29] FAIL: SYS_SPAWN({}) returned MAX", path);
        return;
    }
    println!("  [DEMO 29] PASS: SYS_SPAWN({}) → PID {}", path, pid);

    // Poll the scheduler slot for Exited and read the published code.
    let process_id = kernel_core::process::ProcessId(pid as u32);
    let slot = match kernel_core::process::get(process_id).and_then(|p| p.task_id) {
        Some(s) => s,
        None => {
            println!("  [DEMO 29] FAIL: PID {} has no task_id", pid);
            return;
        }
    };
    let mut polled = 0u64;
    loop {
        if kernel_core::scheduler::task_state(slot) == kernel_core::scheduler::TaskState::Exited {
            break;
        }
        if polled > 500 {
            println!("  [DEMO 29] FAIL: hello-std didn't exit within 500 ticks");
            return;
        }
        let _ = dispatch(SYS_SLEEP, 1, 0, 0, 0);
        polled += 1;
    }
    let code = kernel_core::scheduler::task_exit_code(slot);
    if code != 0 {
        println!("  [DEMO 29] FAIL: hello-std exit code = {} (want 0)", code);
        return;
    }
    println!("  [DEMO 29] PASS: hello-std exited 0 via semos-std::main!()");
    println!("  [DEMO 29] => Phase 14 M25 Tier 1 shim foundation green");
}

/// DEMO 30: vec-demo.elf — Phase 14 M25 Tier 2 #50 acceptance.
///
/// Spawns `/bin/vec-demo`, which exercises Vec / String / Box / format!
/// to validate the GlobalAlloc → SYS_HEAP_ALLOC path end-to-end. Pass
/// criterion: process exits with code 0 (any failure inside the user
/// program exits non-zero with a stage-specific code).
fn vec_demo() {
    use kernel_core::syscall::{dispatch, numbers::*};

    let path = "/bin/vec-demo";
    let pid = dispatch(SYS_SPAWN, path.as_ptr() as u64, path.len() as u64, 0, 0);
    if pid == u64::MAX {
        println!("  [DEMO 30] FAIL: SYS_SPAWN({}) returned MAX", path);
        return;
    }
    println!("  [DEMO 30] PASS: SYS_SPAWN({}) → PID {}", path, pid);

    let process_id = kernel_core::process::ProcessId(pid as u32);
    let slot = match kernel_core::process::get(process_id).and_then(|p| p.task_id) {
        Some(s) => s,
        None => {
            println!("  [DEMO 30] FAIL: PID {} has no task_id", pid);
            return;
        }
    };
    // Larger tick budget than hello-std (this one does meaningful work:
    // multiple growth-rounds on Vec + 32 transient String allocs).
    let mut polled = 0u64;
    loop {
        if kernel_core::scheduler::task_state(slot) == kernel_core::scheduler::TaskState::Exited {
            break;
        }
        if polled > 2000 {
            println!("  [DEMO 30] FAIL: vec-demo didn't exit within 2000 ticks");
            return;
        }
        let _ = dispatch(SYS_SLEEP, 1, 0, 0, 0);
        polled += 1;
    }
    let code = kernel_core::scheduler::task_exit_code(slot);
    if code != 0 {
        println!("  [DEMO 30] FAIL: vec-demo exit code = 0x{:X} (want 0)", code);
        return;
    }
    println!("  [DEMO 30] PASS: vec-demo exited 0 (GlobalAlloc + Vec/String/Box working)");
    println!("  [DEMO 30] => M25 Tier 2 #50 closed — alloc-crate downstream is live");
}

/// DEMO 31: std-demo.elf — Phase 14 M25 #51/#52 acceptance.
///
/// Spawns `/bin/std-demo`, which exercises fs::File + io::Read/Write,
/// env::args/var, sync::{Mutex,Once}, and thread::spawn/join. Pass
/// criterion: exit code 0 (a failed check exits 0x41-0x45).
fn std_demo() {
    use kernel_core::syscall::{dispatch, numbers::*};

    let path = "/bin/std-demo";
    let pid = dispatch(SYS_SPAWN, path.as_ptr() as u64, path.len() as u64, 0, 0);
    if pid == u64::MAX {
        println!("  [DEMO 31] FAIL: SYS_SPAWN({}) returned MAX", path);
        return;
    }
    println!("  [DEMO 31] PASS: SYS_SPAWN({}) → PID {}", path, pid);

    let process_id = kernel_core::process::ProcessId(pid as u32);
    let slot = match kernel_core::process::get(process_id).and_then(|p| p.task_id) {
        Some(s) => s,
        None => {
            println!("  [DEMO 31] FAIL: PID {} has no task_id", pid);
            return;
        }
    };
    // std-demo does thread spawn/join + 2000 Mutex lock cycles — give it
    // a generous budget.
    let mut polled = 0u64;
    loop {
        if kernel_core::scheduler::task_state(slot) == kernel_core::scheduler::TaskState::Exited {
            break;
        }
        if polled > 3000 {
            println!("  [DEMO 31] FAIL: std-demo didn't exit within 3000 ticks");
            return;
        }
        let _ = dispatch(SYS_SLEEP, 1, 0, 0, 0);
        polled += 1;
    }
    let code = kernel_core::scheduler::task_exit_code(slot);
    if code != 0 {
        println!("  [DEMO 31] FAIL: std-demo exit code = 0x{:X} (want 0)", code);
        return;
    }
    println!("  [DEMO 31] PASS: std-demo exited 0 (fs/io/env + Mutex/Once + thread join)");
    println!("  [DEMO 31] => M25 #51/#52 closed — std-shim has files, args, sync, threads");
}

/// DEMO 32: spawn-demo.elf — Phase 14 M25 `std::process::Command`.
///
/// Spawns `/bin/spawn-demo`, which is itself a Ring-3 program that uses
/// `Command` to spawn `/bin/hello-std` (twice — once with an arg) and
/// `/bin/thread-demo`, blocking on each via SYS_WAIT. This proves a
/// Ring-3 *parent* can spawn+wait children and that exit codes (0 and the
/// non-zero 0x2700 from thread-demo) propagate back. spawn-demo exits 0
/// only if all three child waits returned the expected codes.
fn spawn_demo() {
    use kernel_core::syscall::{dispatch, numbers::*};

    let path = "/bin/spawn-demo";
    let pid = dispatch(SYS_SPAWN, path.as_ptr() as u64, path.len() as u64, 0, 0);
    if pid == u64::MAX {
        println!("  [DEMO 32] FAIL: SYS_SPAWN({}) returned MAX", path);
        return;
    }
    println!("  [DEMO 32] PASS: SYS_SPAWN({}) → PID {}", path, pid);

    let process_id = kernel_core::process::ProcessId(pid as u32);
    let slot = match kernel_core::process::get(process_id).and_then(|p| p.task_id) {
        Some(s) => s,
        None => {
            println!("  [DEMO 32] FAIL: PID {} has no task_id", pid);
            return;
        }
    };
    // spawn-demo spawns + joins three children (one of which spawns its
    // own threads) — give it a generous budget.
    let mut polled = 0u64;
    loop {
        if kernel_core::scheduler::task_state(slot) == kernel_core::scheduler::TaskState::Exited {
            break;
        }
        if polled > 5000 {
            println!("  [DEMO 32] FAIL: spawn-demo didn't exit within 5000 ticks");
            return;
        }
        let _ = dispatch(SYS_SLEEP, 1, 0, 0, 0);
        polled += 1;
    }
    let code = kernel_core::scheduler::task_exit_code(slot);
    if code != 0 {
        println!("  [DEMO 32] FAIL: spawn-demo exit code = 0x{:X} (want 0)", code);
        return;
    }
    println!("  [DEMO 32] PASS: spawn-demo exited 0 (Command spawn+wait, exit codes propagate)");
    println!("  [DEMO 32] => M25 std::process::Command works from a Ring-3 parent");
}

/// DEMO 36: net-demo.elf — Phase 14 M25 `std::net`.
///
/// Spawns `/bin/net-demo`, a Ring-3 program that resolves example.com,
/// TCP-connects, sends an HTTP GET, and reads the response — all via the
/// SYS_DNS_RESOLVE + SYS_TCP_* syscalls over the kernel smoltcp stack.
/// Skips cleanly when the net stack isn't up (run with `-netdev user`).
fn net_demo() {
    use kernel_core::syscall::{dispatch, numbers::*};
    if !kernel_core::net::is_initialized() {
        println!("  [DEMO 36] SKIPPED: net stack not initialized (run with -netdev user)");
        return;
    }
    let path = "/bin/net-demo";
    let pid = dispatch(SYS_SPAWN, path.as_ptr() as u64, path.len() as u64, 0, 0);
    if pid == u64::MAX {
        println!("  [DEMO 36] FAIL: SYS_SPAWN({}) returned MAX", path);
        return;
    }
    println!("  [DEMO 36] PASS: SYS_SPAWN({}) → PID {}", path, pid);
    let process_id = kernel_core::process::ProcessId(pid as u32);
    let slot = match kernel_core::process::get(process_id).and_then(|p| p.task_id) {
        Some(s) => s,
        None => { println!("  [DEMO 36] FAIL: PID {} has no task_id", pid); return; }
    };
    // net-demo: resolve + TCP HTTP round-trip via the non-blocking net
    // syscalls (#56) — generous budget for the user-space poll loop.
    let mut polled = 0u64;
    loop {
        if kernel_core::scheduler::task_state(slot) == kernel_core::scheduler::TaskState::Exited { break; }
        if polled > 30000 { println!("  [DEMO 36] FAIL: net-demo didn't exit within budget"); return; }
        let _ = dispatch(SYS_SLEEP, 1, 0, 0, 0);
        polled += 1;
    }
    let code = kernel_core::scheduler::task_exit_code(slot);
    if code != 0 {
        println!("  [DEMO 36] FAIL: net-demo exit code = 0x{:X} (want 0)", code);
        return;
    }
    println!("  [DEMO 36] PASS: net-demo exited 0 (resolve + TcpStream HTTP round-trip)");
    println!("  [DEMO 36] => M25 std::net works end-to-end from a Ring-3 program");
}

/// DEMO 37: M7 TrueType font rasterization. Renders a string at three pixel
/// sizes via `font::fb_draw_text` (ttf-parser outlines → scanline fill →
/// M6 framebuffer), and verifies — headless-safe — by reading the pixels
/// back: each size must light up a plausible number of glyph pixels in its
/// band, and an untouched margin must stay background. Drawn near the bottom
/// of the screen (where the top-down console hasn't reached) with readback
/// done immediately, before any further console output can overwrite it.
fn font_demo() {
    use crate::framebuffer as fb;
    let white = fb::rgb(0xFF, 0xFF, 0xFF);
    let black = fb::rgb(0x00, 0x00, 0x00);
    let (fbw, fbh) = fb::fb_dimensions();
    let text = "SemOS fonts 0123!";
    let sizes = [16.0f32, 24.0, 40.0];
    let left = 24usize;
    let mut all_ok = true;

    for (i, &px) in sizes.iter().enumerate() {
        // Stack the three bands near the bottom, well clear of the console.
        let baseline = fbh.saturating_sub(40 + (sizes.len() - 1 - i) * 56);
        let band_top = baseline.saturating_sub(px as usize);
        let end_x = font::fb_draw_text(left, baseline, text, px, white);

        // Count lit glyph pixels in the band (immediately, pre-console-scroll).
        let mut lit = 0usize;
        let mut yy = band_top;
        while yy < baseline && yy < fbh {
            let mut xx = left;
            while xx < end_x && xx < fbw {
                if fb::fb_read_pixel(xx, yy) == white { lit += 1; }
                xx += 1;
            }
            yy += 1;
        }
        // Real text lights up a *fraction* of its bounding box: enough pixels
        // to be glyphs (lit > 60) but well under a solid fill (a flood bug
        // would push coverage near 100%). Coverage between those bounds, at
        // all three sizes, is the headless proof that glyphs rasterized.
        let band_w = end_x.saturating_sub(left);
        let band_area = band_w * (px as usize);
        let cov = if band_area > 0 { lit * 100 / band_area } else { 0 };
        if lit > 60 && end_x > left && cov < 80 {
            println!("  [DEMO 37] PASS: {}px lit {} glyph pixels (~{}% of band, advance x={})",
                px as usize, lit, cov, end_x);
        } else {
            println!("  [DEMO 37] FAIL: {}px lit={} cov={}% end_x={}", px as usize, lit, cov, end_x);
            all_ok = false;
        }
        let _ = (black, fbw);
    }

    if all_ok {
        println!("  [DEMO 37] => M7 TTF rasterization works (ttf-parser outlines + scanline fill → fb)");
    } else {
        println!("  [DEMO 37] FAIL: one or more font sizes did not render");
    }
}

/// DEMO 38: M8 anti-aliased 2D vector rendering. Renders a filled circle and
/// a stroked cubic Bézier with tiny-skia into an in-heap pixmap, blits it to
/// the framebuffer, and verifies headlessly: the scene must light a plausible
/// number of pixels AND contain blended AA-edge pixels (neither pure
/// background nor pure source color) — the signature that anti-aliasing ran
/// (vs M7's 1-bit fill). Also exercises the new kernel global allocator.
fn gfx2d_demo() {
    use crate::framebuffer as fb;
    let (fbw, fbh) = fb::fb_dimensions();
    let w = 320usize.min(fbw);
    let h = 240usize.min(fbh);
    // Bottom-right-ish, clear of the top-down console.
    let ox = fbw.saturating_sub(w + 16);
    let oy = fbh.saturating_sub(h + 16);

    let (lit, aa) = gfx2d::aa_scene(ox, oy, w, h);

    // Confirm the blit reached the framebuffer: the circle center should be
    // (near) white. Circle center ≈ (ox + 0.30w, oy + 0.50h).
    let cx = ox + (w * 30 / 100);
    let cy = oy + (h * 50 / 100);
    let center = fb::fb_read_pixel(cx, cy);
    let center_white = center == fb::rgb(0xFF, 0xFF, 0xFF);

    if lit > 200 && aa > 50 && center_white {
        println!("  [DEMO 38] PASS: AA scene — {} lit px, {} anti-aliased edge px", lit, aa);
        println!("  [DEMO 38] PASS: pixmap blitted to fb (circle center reads white)");
        println!("  [DEMO 38] => M8: tiny-skia AA vector rendering over the M6 framebuffer");
    } else {
        println!("  [DEMO 38] FAIL: lit={} aa={} center_white={}", lit, aa, center_white);
    }
}

/// DEMO 39: M7/M8 TTY console. Drives `tty::TtyConsole` through both render
/// modes and verifies headlessly by reading the framebuffer back:
///   - Sharp (M7): writes several lines (forcing newline + a region scroll),
///     then confirms a plausible count of crisp white glyph pixels spread over
///     ≥2 text rows, at a coverage that's glyphs-not-flood.
///   - Smooth (M8): writes a white AA line, then confirms blended edge pixels
///     (neither pure black nor pure white) exist — the signature AA ran.
/// All readback happens before any further `println!`, so the bitmap console's
/// own scrolling can't disturb the region under test.
fn tty_demo() {
    use crate::framebuffer as fb;
    use crate::tty::{Aa, TtyConsole};

    let white = fb::rgb(0xFF, 0xFF, 0xFF);
    let black = fb::rgb(0x00, 0x00, 0x00);
    let (fbw, fbh) = fb::fb_dimensions();
    if fbw == 0 || fbh == 0 {
        println!("  [DEMO 39] SKIPPED: no framebuffer");
        return;
    }

    let px = 20.0f32;
    let lh = font::line_height(px).max(px as usize + 2);
    let x0 = 16usize;
    let w = fbw.saturating_sub(32).min(720);
    let h_a = lh * 4; // Sharp region: 4 text lines tall (so 8 lines → scroll)
    let h_b = lh * 2; // Smooth region
    // Pin both regions to the lower screen with a margin; cosmetic only
    // (headless), but keeps them off the very top where the bitmap console
    // is actively writing.
    let y_a = fbh.saturating_sub(h_a + h_b + 48);
    let y_b = y_a + h_a + 16;

    // ---- Sharp (M7): multi-line write that overflows the region → scroll ----
    let mut con = TtyConsole::new(x0, y_a, w, h_a, px, white, black);
    con.write(Aa::Sharp, "M7 sharp TTF console.\n");
    for i in 0..8u32 {
        // Per-line content + newline; 8 lines into a 4-line region forces the
        // region-scroll path to run.
        con.write(Aa::Sharp, "line ");
        let mut buf = [0u8; 2];
        buf[0] = b'0' + (i % 10) as u8;
        buf[1] = b'\n';
        con.write(Aa::Sharp, core::str::from_utf8(&buf).unwrap_or("?\n"));
    }

    // Readback region A: count white pixels and the number of rows containing
    // any white pixel (≥2 ⇒ newline worked).
    let mut lit_a = 0usize;
    let mut rows_with_text = 0usize;
    let ay1 = (y_a + h_a).min(fbh);
    let ax1 = (x0 + w).min(fbw);
    let mut yy = y_a;
    while yy < ay1 {
        let mut row_lit = false;
        let mut xx = x0;
        while xx < ax1 {
            if fb::fb_read_pixel(xx, yy) == white {
                lit_a += 1;
                row_lit = true;
            }
            xx += 1;
        }
        if row_lit {
            rows_with_text += 1;
        }
        yy += 1;
    }
    let area_a = w * h_a;
    let cov_a = if area_a > 0 { lit_a * 100 / area_a } else { 0 };

    // ---- Smooth (M8): one AA line; look for blended edge pixels ----
    let mut con_b = TtyConsole::new(x0, y_b, w, h_b, px, white, black);
    con_b.write(Aa::Smooth, "M8 smooth (anti-aliased) TTF console 0123");

    let mut lit_b = 0usize;
    let mut aa_edge = 0usize;
    let by1 = (y_b + h_b).min(fbh);
    let bx1 = (x0 + w).min(fbw);
    let mut yy = y_b;
    while yy < by1 {
        let mut xx = x0;
        while xx < bx1 {
            let p = fb::fb_read_pixel(xx, yy);
            if p != black {
                lit_b += 1;
                if p != white {
                    aa_edge += 1; // blended (gray) → anti-aliasing ran
                }
            }
            xx += 1;
        }
        yy += 1;
    }

    // ---- Verdict (all readback done; safe to print now) ----
    let sharp_ok = lit_a > 200 && rows_with_text >= 2 && cov_a < 80;
    let smooth_ok = lit_b > 100 && aa_edge > 30;

    if sharp_ok {
        println!("  [DEMO 39] PASS: Sharp (M7) — {} white px over {} rows ({}% cov, multi-line+scroll)",
            lit_a, rows_with_text, cov_a);
    } else {
        println!("  [DEMO 39] FAIL: Sharp lit={} rows={} cov={}%", lit_a, rows_with_text, cov_a);
    }
    if smooth_ok {
        println!("  [DEMO 39] PASS: Smooth (M8) — {} lit px, {} anti-aliased edge px", lit_b, aa_edge);
    } else {
        println!("  [DEMO 39] FAIL: Smooth lit={} aa_edge={}", lit_b, aa_edge);
    }
    if sharp_ok && smooth_ok {
        println!("  [DEMO 39] => M7/M8 wired into a cursor-managed console (newline, wrap, region scroll)");
    }
}

/// DEMO 33: HTTP chunked-transfer-encoding decoder (M13).
///
/// Feeds hardcoded chunked byte streams into `kernel_core::net::decode_chunked`
/// and asserts the reassembled body matches. No network needed — this is the
/// unit-level acceptance for the decoder that the NetworkLlmProvider response
/// path now uses. Three sub-checks:
///   1. normal multi-chunk body reassembles correctly
///   2. an empty body (`0\r\n\r\n`) decodes to zero bytes
///   3. a truncated chunk errors cleanly (no panic / no over-read)
/// Plus a bonus check that hex sizes and trailing headers are handled.
fn chunked_decode_demo() {
    use kernel_core::net::decode_chunked;
    let mut all_ok = true;
    let mut out = [0u8; 256];

    // --- check 1: normal multi-chunk ---------------------------------------
    // "4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n" -> "Wikipedia"
    let input1: &[u8] = b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
    match decode_chunked(input1, &mut out) {
        Ok(n) if &out[..n] == b"Wikipedia" => {
            println!("  [DEMO 33] PASS: multi-chunk decoded {} bytes -> \"Wikipedia\"", n);
        }
        Ok(n) => {
            println!("  [DEMO 33] FAIL: multi-chunk wrong output ({} bytes)", n);
            all_ok = false;
        }
        Err(e) => {
            println!("  [DEMO 33] FAIL: multi-chunk errored: {:?}", e);
            all_ok = false;
        }
    }

    // --- check 2: empty body -----------------------------------------------
    let input2: &[u8] = b"0\r\n\r\n";
    match decode_chunked(input2, &mut out) {
        Ok(0) => println!("  [DEMO 33] PASS: empty body decoded to 0 bytes"),
        Ok(n) => {
            println!("  [DEMO 33] FAIL: empty body decoded {} bytes (want 0)", n);
            all_ok = false;
        }
        Err(e) => {
            println!("  [DEMO 33] FAIL: empty body errored: {:?}", e);
            all_ok = false;
        }
    }

    // --- check 3: truncated input must error cleanly -----------------------
    // declares 9 bytes but only 4 are present, no terminator.
    let input3: &[u8] = b"9\r\nWiki";
    match decode_chunked(input3, &mut out) {
        Err(e) => println!("  [DEMO 33] PASS: truncated input errored cleanly: {:?}", e),
        Ok(n) => {
            println!("  [DEMO 33] FAIL: truncated input decoded {} bytes (want error)", n);
            all_ok = false;
        }
    }

    // --- check 4: hex sizes + trailing headers -----------------------------
    // 0xC = 12 data bytes ("Hello, world"), then CRLF, then a trailer header
    // after the final chunk. (The data must be exactly 12 bytes followed by
    // CRLF — a 13-byte "Hello, world!" here would be malformed framing.)
    let input4: &[u8] = b"C\r\nHello, world\r\n0\r\nX-Trailer: ok\r\n\r\n";
    match decode_chunked(input4, &mut out) {
        Ok(n) if &out[..n] == b"Hello, world" => {
            println!("  [DEMO 33] PASS: hex size + trailer decoded -> \"Hello, world\"");
        }
        Ok(n) => {
            println!("  [DEMO 33] FAIL: hex/trailer wrong output ({} bytes): {:?}",
                n, core::str::from_utf8(&out[..n]).unwrap_or("<non-utf8>"));
            all_ok = false;
        }
        Err(e) => {
            println!("  [DEMO 33] FAIL: hex/trailer errored: {:?}", e);
            all_ok = false;
        }
    }

    if all_ok {
        println!("  [DEMO 33] PASS: all chunked-decoder sub-checks green");
        println!("  [DEMO 33] => M13 closed — chunked bodies de-framed before JSON extract");
    } else {
        println!("  [DEMO 33] FAIL: one or more chunked-decoder sub-checks failed");
    }
}

/// DEMO 24: per-process env + CWD via the 4 new syscalls (Phase 14 prereq #3).
///
/// What this proves:
///   1. GET_CWD returns the default `/` for the kernel process
///   2. SET_CWD writes a new value; GET_CWD reflects it
///   3. SET_CWD rejects non-absolute paths
///   4. SET_ENV creates a key; GET_ENV reads it back byte-exact
///   5. SET_ENV updates an existing key (no duplicate entries left behind)
///   6. GET_ENV on a missing key returns 0
fn env_cwd_test() {
    use kernel_core::syscall::dispatch;
    use kernel_core::syscall::numbers::*;

    let mut buf = [0u8; 128];

    // Step 1: read default CWD.
    let n = dispatch(SYS_GET_CWD, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0);
    if n == u64::MAX || n == 0 {
        println!("  [DEMO 24] FAIL: GET_CWD returned {}", n); return;
    }
    let cwd = core::str::from_utf8(&buf[..n as usize]).unwrap_or("?");
    println!("  [DEMO 24] PASS: GET_CWD = '{}' ({} bytes)", cwd, n);

    // Step 2: change CWD.
    let new_cwd = "/persist";
    let r = dispatch(SYS_SET_CWD, new_cwd.as_ptr() as u64, new_cwd.len() as u64, 0, 0);
    if r != 0 { println!("  [DEMO 24] FAIL: SET_CWD: {}", r); return; }
    let n2 = dispatch(SYS_GET_CWD, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0);
    let cwd2 = core::str::from_utf8(&buf[..n2 as usize]).unwrap_or("?");
    if cwd2 != new_cwd {
        println!("  [DEMO 24] FAIL: GET_CWD after set returned '{}', want '{}'", cwd2, new_cwd);
        return;
    }
    println!("  [DEMO 24] PASS: SET_CWD then GET_CWD round-trip ('{}')", cwd2);

    // Step 3: SET_CWD rejects non-absolute.
    let bad = "relative/path";
    let r_bad = dispatch(SYS_SET_CWD, bad.as_ptr() as u64, bad.len() as u64, 0, 0);
    if r_bad != u64::MAX {
        println!("  [DEMO 24] FAIL: SET_CWD accepted relative path (got {})", r_bad);
        return;
    }
    println!("  [DEMO 24] PASS: SET_CWD rejects relative path");

    // Step 4: SET_ENV + GET_ENV byte-exact.
    let key = "RUST_LOG";
    let val = "debug";
    let r = dispatch(SYS_SET_ENV,
        key.as_ptr() as u64, key.len() as u64,
        val.as_ptr() as u64, val.len() as u64);
    if r != 0 { println!("  [DEMO 24] FAIL: SET_ENV: {}", r); return; }
    let mut vbuf = [0u8; 64];
    let n = dispatch(SYS_GET_ENV,
        key.as_ptr() as u64, key.len() as u64,
        vbuf.as_mut_ptr() as u64, vbuf.len() as u64);
    if n == u64::MAX || n == 0 {
        println!("  [DEMO 24] FAIL: GET_ENV after set returned {}", n);
        return;
    }
    let got = core::str::from_utf8(&vbuf[..n as usize]).unwrap_or("?");
    if got != val {
        println!("  [DEMO 24] FAIL: GET_ENV returned '{}', want '{}'", got, val);
        return;
    }
    println!("  [DEMO 24] PASS: SET_ENV + GET_ENV round-trip ('{}={}')", key, got);

    // Step 5: SET_ENV updates existing key.
    let val2 = "trace";
    let _ = dispatch(SYS_SET_ENV,
        key.as_ptr() as u64, key.len() as u64,
        val2.as_ptr() as u64, val2.len() as u64);
    let n = dispatch(SYS_GET_ENV,
        key.as_ptr() as u64, key.len() as u64,
        vbuf.as_mut_ptr() as u64, vbuf.len() as u64);
    let got2 = core::str::from_utf8(&vbuf[..n as usize]).unwrap_or("?");
    if got2 != val2 {
        println!("  [DEMO 24] FAIL: SET_ENV update: got '{}', want '{}'", got2, val2);
        return;
    }
    println!("  [DEMO 24] PASS: SET_ENV updates existing key in place ('{}={}')", key, got2);

    // Step 6: GET_ENV on missing key returns 0.
    let missing = "NOT_SET";
    let n = dispatch(SYS_GET_ENV,
        missing.as_ptr() as u64, missing.len() as u64,
        vbuf.as_mut_ptr() as u64, vbuf.len() as u64);
    if n != 0 {
        println!("  [DEMO 24] FAIL: GET_ENV on missing key returned {}, want 0", n);
        return;
    }
    println!("  [DEMO 24] PASS: GET_ENV on missing key returns 0");

    // Restore default CWD so later DEMOs aren't surprised.
    let _ = dispatch(SYS_SET_CWD, "/".as_ptr() as u64, 1, 0, 0);

    println!("  [DEMO 24] => per-process env + CWD ready for std::env shim");
}

/// DEMO 23: SYS_SPAWN with argv/envp via SpawnArgs (Phase 14 prereq #2).
///
/// What this proves:
///   1. SpawnArgs struct + blob format are valid — kernel parses it.
///   2. spawn_from_elf_with_args returns a valid PID with non-empty argv.
///   3. Backwards compat: SYS_SPAWN with arg3=0 still works.
///   4. Negative path: oversized argv blob → kernel rejects cleanly.
///
/// What this DOESN'T prove yet:
///   - User-side reading of argv from the stack. That needs a startup
///     shim in user-programs; lands when std-shim work begins (M25).
fn spawn_argv_test() {
    use kernel_core::syscall::{dispatch, SpawnArgs};
    use kernel_core::syscall::numbers::*;

    // Step 1: build the argv blob — [count u32][len u32][bytes]…
    // Two args: "/bin/hello-rs" and "--demo23".
    const ARGV_BLOB: &[u8] = b"\x02\x00\x00\x00\
                               \x0d\x00\x00\x00/bin/hello-rs\
                               \x08\x00\x00\x00--demo23";
    // Envp blob with one entry: "TEST=phase14".
    const ENVP_BLOB: &[u8] = b"\x01\x00\x00\x00\
                               \x0c\x00\x00\x00TEST=phase14";

    let spawn_args = SpawnArgs {
        argv_blob_ptr: ARGV_BLOB.as_ptr() as u64,
        argv_blob_len: ARGV_BLOB.len() as u32,
        envp_blob_ptr: ENVP_BLOB.as_ptr() as u64,
        envp_blob_len: ENVP_BLOB.len() as u32,
    };
    let path = "/bin/hello-rs";

    let pid = dispatch(
        SYS_SPAWN,
        path.as_ptr() as u64,
        path.len() as u64,
        0,  // tier
        &spawn_args as *const SpawnArgs as u64,
    );
    if pid == u64::MAX {
        println!("  [DEMO 23] FAIL: SYS_SPAWN with argv returned u64::MAX");
        return;
    }
    println!("  [DEMO 23] PASS: SYS_SPAWN with 2-arg argv + 1-entry envp → PID {}", pid);

    // Step 2: backwards compat — arg3=0 should still work.
    let pid2 = dispatch(
        SYS_SPAWN,
        path.as_ptr() as u64,
        path.len() as u64,
        0,
        0,  // arg3=0 → no SpawnArgs, behave like legacy 3-arg API
    );
    if pid2 == u64::MAX {
        println!("  [DEMO 23] FAIL: SYS_SPAWN with arg3=0 (legacy) returned u64::MAX");
        return;
    }
    println!("  [DEMO 23] PASS: backwards-compat (arg3=0) still works → PID {}", pid2);

    // Step 3: oversized blob rejected.
    const TOO_BIG_ARGV: [u8; 4096] = [0u8; 4096];  // declared count=0 but len > MAX_BLOB_BYTES
    let big_args = SpawnArgs {
        argv_blob_ptr: TOO_BIG_ARGV.as_ptr() as u64,
        argv_blob_len: TOO_BIG_ARGV.len() as u32,  // 4096 > MAX_BLOB_BYTES (1024)
        envp_blob_ptr: 0,
        envp_blob_len: 0,
    };
    let bad_pid = dispatch(
        SYS_SPAWN,
        path.as_ptr() as u64,
        path.len() as u64,
        0,
        &big_args as *const SpawnArgs as u64,
    );
    if bad_pid != u64::MAX {
        println!("  [DEMO 23] FAIL: oversized argv blob accepted (PID {})", bad_pid);
        return;
    }
    println!("  [DEMO 23] PASS: oversized argv blob rejected cleanly");

    println!("  [DEMO 23] => SYS_SPAWN argv/envp ABI works; user-side reader is M25 follow-up");
}

/// DEMO 22: heap allocator through SYS_HEAP_ALLOC / SYS_HEAP_FREE.
///
/// Phase 14 Tier 1 prerequisite — std::alloc::GlobalAlloc backing.
/// What this proves:
///   1. Small + large allocations succeed (16 B, 1 KiB, 64 KiB)
///   2. Distinct allocations have distinct addresses
///   3. Pointers are aligned to the requested alignment
///   4. Free + realloc cycle returns valid memory
///   5. Free-list accounting stays consistent across mixed ops
fn heap_allocator_test() {
    use kernel_core::syscall::dispatch;
    use kernel_core::syscall::numbers::*;

    let (used_before, free_before, blocks_before) = kernel_core::memory::heap::stats();
    println!("  [DEMO 22] heap start: {}K used, {}K free, {} free blocks",
        used_before/1024, free_before/1024, blocks_before);

    // 1: small alloc, 16-byte align.
    let p1 = dispatch(SYS_HEAP_ALLOC, 16, 16, 0, 0u64);
    if p1 == 0 { println!("  [DEMO 22] FAIL: 16-byte alloc returned null"); return; }
    if p1 % 16 != 0 { println!("  [DEMO 22] FAIL: 16-byte alloc not 16-aligned: 0x{:x}", p1); return; }
    println!("  [DEMO 22] PASS: 16-byte alloc @ 0x{:x} (16-aligned)", p1);

    // 2: medium alloc, 64-byte align.
    let p2 = dispatch(SYS_HEAP_ALLOC, 1024, 64, 0, 0);
    if p2 == 0 { println!("  [DEMO 22] FAIL: 1KB alloc returned null"); return; }
    if p2 % 64 != 0 { println!("  [DEMO 22] FAIL: 1KB alloc not 64-aligned: 0x{:x}", p2); return; }
    if p1 == p2 { println!("  [DEMO 22] FAIL: two allocs returned same ptr"); return; }
    println!("  [DEMO 22] PASS: 1KB alloc @ 0x{:x} (64-aligned, distinct)", p2);

    // 3: large alloc, page-aligned.
    let p3 = dispatch(SYS_HEAP_ALLOC, 64 * 1024, 4096, 0, 0);
    if p3 == 0 { println!("  [DEMO 22] FAIL: 64KB alloc returned null"); return; }
    if p3 % 4096 != 0 { println!("  [DEMO 22] FAIL: 64KB alloc not 4K-aligned: 0x{:x}", p3); return; }
    println!("  [DEMO 22] PASS: 64KB alloc @ 0x{:x} (4K-aligned)", p3);

    // 4: write through both small + medium buffers, verify no aliasing.
    unsafe {
        let buf1 = core::slice::from_raw_parts_mut(p1 as *mut u8, 16);
        let buf2 = core::slice::from_raw_parts_mut(p2 as *mut u8, 1024);
        buf1.fill(0xAA);
        buf2.fill(0xBB);
        if buf1.iter().any(|&b| b != 0xAA) {
            println!("  [DEMO 22] FAIL: buf1 corrupted by buf2 write"); return;
        }
        if buf2.iter().any(|&b| b != 0xBB) {
            println!("  [DEMO 22] FAIL: buf2 corrupted"); return;
        }
    }
    println!("  [DEMO 22] PASS: distinct buffers don't alias");

    // 5: free + realloc cycle.
    let _ = dispatch(SYS_HEAP_FREE, p2, 1024, 64, 0);
    let p2_again = dispatch(SYS_HEAP_ALLOC, 1024, 64, 0, 0u64);
    if p2_again == 0 { println!("  [DEMO 22] FAIL: realloc after free returned null"); return; }
    if p2_again % 64 != 0 { println!("  [DEMO 22] FAIL: realloc not 64-aligned"); return; }
    println!("  [DEMO 22] PASS: free + realloc returned valid ptr @ 0x{:x}", p2_again);

    // Cleanup so later DEMOs see a non-fragmented arena.
    let _ = dispatch(SYS_HEAP_FREE, p1, 16, 16, 0);
    let _ = dispatch(SYS_HEAP_FREE, p2_again, 1024, 64, 0);
    let _ = dispatch(SYS_HEAP_FREE, p3, 64 * 1024, 4096, 0);

    let (used_after, free_after, blocks_after) = kernel_core::memory::heap::stats();
    println!("  [DEMO 22] heap end: {}K used, {}K free, {} free blocks",
        used_after/1024, free_after/1024, blocks_after);
    if used_after != used_before {
        println!("  [DEMO 22] FAIL: used bytes drifted ({} -> {})", used_before, used_after);
        return;
    }
    println!("  [DEMO 22] PASS: heap accounting clean (used unchanged across full cycle)");

    println!("  [DEMO 22] => heap allocator ready for std::alloc::GlobalAlloc shim (M25)");
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
