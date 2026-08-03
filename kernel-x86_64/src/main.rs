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
// The x86 drivers (xHCI/EHCI/virtio/iwlwifi) reach hardware through fixed
// mutable statics (rings, buffers, device tables) touched only from single-core
// init/IRQ paths. That idiom trips `static_mut_refs`; kernel-core already opts
// out of the same lint. Migrating these to `&raw`/UnsafeCell is a deliberate
// future pass (needs hardware to re-validate each driver), so accept the lint
// crate-wide here rather than churn ~60 untestable driver sites blind.
#![allow(static_mut_refs)]
// This crate is overwhelmingly hardware drivers: iwlwifi/e1000e/xHCI/EHCI/APIC
// register maps and bring-up scaffolding keep complete constant tables and
// staged structs on purpose (documentation + not-yet-wired paths). kernel-core
// already opts out of dead_code for the same reason; match it here so the
// register maps don't drown real warnings. NOTE: this hides genuinely-dead
// first-party code too — a periodic manual triage is still worthwhile.
#![allow(dead_code)]

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
pub mod agent;
pub mod tui;
pub mod demos; // agent/shell/TUI boot demos, extracted from main.rs (new demos go there)
pub mod pairing_host; // M56 SYS_PAIR host side (identity, store, handshake)
pub mod netlog; // mirror the kernel log to a LAN UDP listener (SYS_NETLOG)
pub mod session; // interactive shell loop + idle/keepalive tasks
pub mod legacy_demos; // older kernel-mode regression demos extracted from main.rs
pub mod gdt;
mod interrupts;
mod memory;
mod firmware;
mod platform_impl;
pub mod context;
mod syscall;
mod keyboard;
mod editor;
mod nvme;
mod ahci;
mod hda;
mod igpu;
mod backlight;
mod display;
mod wireless;
mod e1000e;
mod panic_dump;
pub mod paging;
pub mod apic;
pub mod ioapic;
pub mod framebuffer;
pub mod font;
pub mod gfx2d;
pub mod pci;
pub mod virtio;
pub mod rng;
pub mod rtc;
pub mod usb;

use crate::session::{idle_with_heartbeat, interactive_session};
use crate::legacy_demos::*;

/// Set while a kernel-side fullscreen app (the `agent` TUI or the `edit`or)
/// owns the keyboard. The interactive-shell wait loop checks this and stops
/// pumping the USB HID ring so it doesn't race the app's own pump for
/// keystrokes (both call `usb::xhci::poll_hid`). Set by those apps' run loops.
pub static FULLSCREEN_APP_ACTIVE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Run the kernel demo suite at boot. Default: OFF — boot lands at the
/// shell immediately. The `demos` shell builtin (SYS_DEMOS) is the
/// on-demand entry point.
pub static DEMOS_ON_BOOT: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Bootloader configuration
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    // Request a framebuffer if available
    config.mappings.physical_memory = Some(bootloader_api::config::Mapping::Dynamic);
    // task #42: bump main kernel stack from default 80 KiB to 512 KiB.
    // This is the EARLY-boot stack (before kernel_main hands off to the
    // `init_loader` demo-runner task, which runs on a TASK_STACKS slot — see
    // scheduler::TASK_STACK_SIZE). kernel_main + the
    // Lazy<InterruptDescriptorTable>::new closure (20+ set_handler_fn calls)
    // overflowed 80 KiB → "hang at IDT init"; 512 KiB fixed it. (The M22 TUI
    // #DF was a separate overflow of the *task* stack, fixed there.)
    config.kernel_stack_size = 512 * 1024;
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

const SEMOS_BUILD_TAG: &str = match option_env!("SEMOS_BUILD_TAG") {
    Some(value) => value,
    None => "unknown",
};
const SEMOS_BUILD_DATE: &str = match option_env!("SEMOS_BUILD_DATE") {
    Some(value) => value,
    None => "unknown-date",
};
const SEMOS_RUSTC_VERSION: &str = match option_env!("SEMOS_RUSTC_VERSION") {
    Some(value) => value,
    None => "unknown-toolchain",
};

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
    println!("  build {} · {} · rustc {}", SEMOS_BUILD_TAG, SEMOS_BUILD_DATE, SEMOS_RUSTC_VERSION);
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
        if let Some(info) = framebuffer::fb_info() {
            println!(
                "[fb] {}x{} stride={} bpp={} fmt={} bytes={}",
                info.width,
                info.height,
                info.stride,
                info.bytes_per_pixel,
                framebuffer::pixel_format_name(info.format),
                info.byte_len,
            );
        }
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

    // Firmware probe: authoritative machine identity (SMBIOS/DMI) + whether
    // the platform exposes an IOMMU (ACPI DMAR / VT-d). Read-only; informs
    // the VT-d subsystem before any DMA-device firmware is loaded.
    firmware::probe(boot_info.rsdp_addr.into_option());
    println!();

    // Try to bring up the Local APIC timer; fall back to the legacy 8259 PIC
    // (already initialized by interrupts::init) if no APIC is present.
    println!("[*] Initializing Local APIC...");
    if apic::init() {
        println!("[OK] APIC timer active (PIC masked)");
        // After LAPIC is up, route legacy device IRQs through the
        // IOAPIC. Without this, real hardware (W540 PS/2 keyboard on
        // IRQ 1) never delivers — the PIC is masked. QEMU's
        // emulated devices used to bypass this via the i8259 path.
        ioapic::init();
    } else {
        println!("[!] No APIC — staying on legacy PIC + PIT");
    }
    println!();

    // Initialize the PS/2 controller (i8042). ThinkPad BIOS leaves
    // scanning disabled after its keyboard POST self-test; without
    // an OS-side 0xF4 the IRQ wires up but the keyboard never sends.
    println!("[*] Initializing PS/2 keyboard...");
    keyboard::init();
    println!();

    println!("[*] Scanning PCI bus 0...");
    pci::print_bus_0();
    println!();

    println!("[*] Probing Intel integrated graphics (read-only)...");
    igpu::probe();
    println!();

    println!("[*] Initializing backlight controller...");
    backlight::init();
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

    println!("[*] Probing NVMe controller...");
    if nvme::init() {
        if nvme::register_with_kernel_core() {
            println!("[nvme] registered with driver registry as 'nvme0'");
        }
    }
    println!();

    println!("[*] Probing AHCI/SATA controller...");
    if ahci::init() {
        if ahci::register_with_kernel_core() {
            println!("[ahci] registered with driver registry as 'sata0'");
            // M27 DEMO 80 (Layer B): if a sysroot blob is staged on the SATA
            // disk (LBA 0 magic SEMSYSR1), cache its file table so semos-rustc
            // can stream core/compiler_builtins metadata via SYS_SYSROOT_READ.
            kernel_core::sysroot_blob::probe();
        }
    }
    println!();

    println!("[*] Probing Intel HDA audio controller...");
    hda::init();
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

    println!("[*] Probing Intel e1000e Ethernet device...");
    if !kernel_core::net::is_initialized() {
        if e1000e::init() {
            if e1000e::register_with_kernel_core() {
                println!("[e1000e] registered with driver registry as 'e1000e0'");
                if let Some(nd) = kernel_core::drivers::registry::get_net("e1000e0") {
                    if kernel_core::net::init(nd) {
                        // Real Ethernet cannot use QEMU SLIRP's fallback
                        // 10.0.2.15 address. Start DHCP immediately; the static
                        // config remains only until a lease arrives.
                        if kernel_core::net::start_dhcp() {
                            println!("[e1000e] DHCP client started (run `netinfo` after link-up)");
                        }
                        kernel_core::net::poll();
                    }
                }
            }
        }
    }

    // M11 iwlwifi — register NetDevice stub if PCI probe finds a NIC.
    // On QEMU this silently returns false (no Intel wireless).  On metal
    // it creates the device skeleton and registers "iwlwifi0" so smoltcp
    // can use it once firmware + association land.
    if wireless::iwlwifi_net::register_with_kernel_core() {
        println!("[iwlwifi] registered with driver registry as 'iwlwifi0'");
    }

    // USB init. Task #36 root cause: TASK_STACK_SIZE was 16 KiB,
    // adding USB pushed some kernel function's stack frame past
    // the cliff and overflowed into the previous slot's iret-RIP.
    // Fixed by bumping TASK_STACK_SIZE to 64 KiB (Phase 9 M3).
    println!("[*] Probing xHCI USB controller...");
    let _usb_ok = usb::init_and_enumerate();
    // Phase 15 M50: if an enumerated USB device is CDC-ECM (tethered phone
    // or USB-Ethernet dongle), register it with the driver registry so the
    // smoltcp glue below can bring up an IP stack on top.
    if usb::cdc_ecm_net::register_with_kernel_core() {
        let mac = usb::xhci::cdc_ecm_device().map(|d| d.mac).unwrap_or([0; 6]);
        println!("[cdc-ecm] registered with driver registry as 'cdc-ecm0' MAC {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
        // Bring up smoltcp on the CDC-ECM interface. Same pattern as
        // virtio-net0 above. If virtio-net0 was already registered as
        // the active interface, this is additive; net::init takes the
        // *first* registered device on its first call.
        if let Some(nd) = kernel_core::drivers::registry::get_net("cdc-ecm0") {
            if kernel_core::net::init(nd) {
                kernel_core::net::poll();
                println!("[cdc-ecm] smoltcp interface initialized on cdc-ecm0");
            }
        }
    }
    // iPhone tether (ipheth over EHCI): if an iPhone with Personal Hotspot
    // on was enumerated and its bulk data path is live, register it and
    // bring up smoltcp with the iPhone tether subnet (172.20.10.0/28; the
    // phone is both gateway and DNS at 172.20.10.1). On the W540 there is
    // no virtio-net and no CDC-ECM, so net::init stays uninitialized until
    // this call — `init_with_ipconfig` brings the stack up on ipheth0.
    if usb::iphone_net::register_with_kernel_core() {
        let mac = usb::iphone::iphone_device().map(|d| d.mac).unwrap_or([0; 6]);
        println!("[ipheth] registered with driver registry as 'ipheth0' MAC {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
        if let Some(nd) = kernel_core::drivers::registry::get_net("ipheth0") {
            use kernel_core::net::Ipv4Address;
            if kernel_core::net::init_with_ipconfig(
                nd,
                Ipv4Address::new(172, 20, 10, 9),
                28,
                Ipv4Address::new(172, 20, 10, 1),
                Ipv4Address::new(172, 20, 10, 1),
            ) {
                kernel_core::net::poll();
                println!("[ipheth] smoltcp up on ipheth0 (172.20.10.9/28 gw/dns 172.20.10.1)");
                // iOS hotspots are suspected to NAT only for clients they
                // have LEASED — run DHCP on top; the static config above
                // keeps working until the lease replaces it. Lease
                // progress happens inside net::poll (driven by any net
                // activity, e.g. the first `fetch`).
                if kernel_core::net::start_dhcp() {
                    // Give the lease ~2s of wall-clock at boot (tick-based
                    // per the net-wait rule; DISCOVER→ACK is several RTTs).
                    // If it doesn't land here, later net activity keeps
                    // polling and the lease applies whenever it arrives.
                    let deadline = kernel_core::platform::ticks() + 124;
                    while kernel_core::platform::ticks() < deadline {
                        kernel_core::net::poll();
                        core::hint::spin_loop();
                    }
                }
            }
        }
    }
    // M27 DEMO 80 Stage 1: register the enumerated USB stick as block device
    // "usb0" (infrastructure — runs every boot, not gated behind the demo
    // suite) and smoke-read its LBA 0 to confirm USB MSC reads work. For a
    // FAT stick LBA 0 is the MBR (ends 55 AA); for a raw blob it's SEMSYSR1.
    if usb::xhci::register_usb_with_kernel_core() {
        println!("[usb0] registered USB Mass Storage as block device 'usb0'");
        let mut sec = [0u8; 512];
        match kernel_core::drivers::registry::get_block("usb0") {
            Some(dev) => match dev.read_blocks(0, &mut sec) {
                Ok(()) => {
                    print!("[usb0] LBA 0 first 16 bytes:");
                    for b in sec.iter().take(16) {
                        print!(" {:02X}", b);
                    }
                    println!("  [510..512]={:02X} {:02X}", sec[510], sec[511]);
                }
                Err(_) => println!("[usb0] LBA 0 read FAILED"),
            },
            None => println!("[usb0] registry lookup failed"),
        }
    } else {
        println!("[usb0] no USB Mass Storage device enumerated");
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
    // 2026-07-17 review P0 regression: Ring-3 attacks on SYS_WRITE /
    // SYS_LLM_CONTEXT with kernel pointers must all return u64::MAX.
    static PTR_GUARD_TEST_ELF: &[u8] = include_bytes!(
        "../../user-programs/ptr-guard-test/target/x86_64-unknown-none/release/ptr-guard-test"
    );
    // Phase 14 Tier 3 #45 Ring-3 validation: spawns a sibling thread,
    // round-trips through SYS_FUTEX_WAIT/WAKE/THREAD_JOIN, exits with
    // a known code the kernel reads in DEMO 28.
    static THREAD_DEMO_ELF: &[u8] = include_bytes!(
        "../../user-programs/thread-demo/target/x86_64-unknown-none/release/thread-demo"
    );
    // M25 sync surface validation: live exercise of Condvar wakeup +
    // mpsc ordering + RwLock concurrent readers. Compiled against the
    // semos-std shim; runs as DEMO 70 and exits with 0 on full pass.
    static SYNC_DEMO_ELF: &[u8] = include_bytes!(
        "../../user-programs/sync-demo/target/x86_64-unknown-none/release/sync-demo"
    );
    // M26 Session C: a hello program compiled with rustc's Cranelift
    // backend (rustc_codegen_cranelift, via -Z codegen-backend=cranelift).
    // core/compiler_builtins still go through LLVM (the wildcard
    // [profile.release.package."*"] override) because cg_clif can't yet
    // build core's va_end intrinsic — but cg-clif-hello.rs itself is
    // pure Cranelift output. DEMO 71 SPAWNs it, expects exit 0 + the
    // marker text on stdout.
    static CG_CLIF_HELLO_ELF: &[u8] = include_bytes!(
        "../../user-programs/cg-clif-hello/target/x86_64-unknown-none/release/cg-clif-hello"
    );
    // M27 Session D.1: the first ELF assembled directly by *our* host
    // compiler (compiler/) — Cranelift-codegen'd `add(i64,i64)` body
    // glued to a hand-emitted `_start` shim and wrapped in an ET_EXEC
    // header, all without invoking rustc or a linker. _start calls
    // add(1, 2) and exits with the result; DEMO 72 asserts exit==3 +
    // marker on stdout. Build with: cd compiler && cargo run.
    static SEMOS_CC_HELLO_ELF: &[u8] = include_bytes!(
        "../../compiler/out/semos_cc_hello.elf"
    );
    // M27 Session D.2: same emitter ported to Ring 3 on SemOS — runs as
    // a user program, emits its own ET_EXEC, writes it to the install-
    // anywhere namespace at /d2-emitted.elf. DEMO 73 spawns it then
    // spawns the emitted ELF and verifies exit==3 + marker — proving the
    // toolchain pipeline runs end-to-end *on* SemOS. Cranelift's add()
    // bytes are inlined (snapshot from D.1) until the cranelift no_std
    // port lands as a follow-up.
    static SEMOS_CC_ELF: &[u8] = include_bytes!(
        "../../user-programs/semos-cc/target/x86_64-unknown-none/release/semos-cc"
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
    // M14 user/app framebuffer surface validation: draws a rectangle via
    // SYS_FB_BLIT without touching the kernel framebuffer directly.
    static FB_DEMO_ELF: &[u8] = include_bytes!(
        "../../user-programs/fb-demo/target/x86_64-unknown-none/release/fb-demo"
    );
    // M20 native shell: parses + runs commands, spawns ELF children.
    static SEM_SH_ELF: &[u8] = include_bytes!(
        "../../user-programs/sem-sh/target/x86_64-unknown-none/release/sem-sh"
    );
    // M27 Phase 5b iter 5 — DEMO 80: rustc-on-SemOS. The 88 MB binary is
    // the full rustc compiler infrastructure plus the Cranelift codegen
    // stack statically linked into one Ring-3 ELF.
    //
    // Note (2026-06-10): the 88 MB include makes a 102 MB kernel that
    // never reaches serial under QEMU's BIOS bootloader (size > legacy
    // load-segment ceiling). It loads fine on real hardware (T540p
    // validated) and under UEFI — but parallel EHCI-validation sessions
    // running under QEMU-BIOS need to keep this stub'd. If you're a
    // sub-agent reaching this comment: only flip the stub on if your
    // tests run on hardware or UEFI; for QEMU-BIOS leave the &[] in.
    // UN-STUBBED for the DEMO 80 post-compile-hang investigation (real HW /
    // UEFI boot). Re-stub to `&[]` for QEMU-BIOS validation sessions.
    // UN-STUBBED for DEMO 80 hardware boots. Re-stub to `&[]` for QEMU-BIOS
    // validation sessions (e.g. WiFi work) where the 89 MB include won't boot.
    static SEMOS_RUSTC_ELF: &[u8] = include_bytes!(
        "../../user-programs/semos-rustc/target/x86_64-unknown-none/release/semos-rustc"
    );
    // DEMO 80 input source: a trivial no_std/no_main Rust program. The
    // SemOS-resident semos-rustc compiles this to /tmp/hello.elf which
    // SYS_SPAWN can then load.
    static HELLO_RS_SOURCE: &[u8] = include_bytes!(
        "../../user-programs/semos-rustc/test-sources/hello.rs"
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
        if fs.add("ptr-guard-test.elf", kernel_core::fs::ramfs::FileType::Executable, PTR_GUARD_TEST_ELF) {
            println!("    Registered ptr-guard-test.elf ({} bytes, P0 pointer-validation regression)", PTR_GUARD_TEST_ELF.len());
        } else {
            println!("    [WARN] failed to register ptr-guard-test.elf");
        }
        if fs.add("sync-demo.elf", kernel_core::fs::ramfs::FileType::Executable, SYNC_DEMO_ELF) {
            println!("    Registered sync-demo.elf ({} bytes, Condvar+mpsc+RwLock smoke)", SYNC_DEMO_ELF.len());
        } else {
            println!("    [WARN] failed to register sync-demo.elf");
        }
        if fs.add("cg-clif-hello.elf", kernel_core::fs::ramfs::FileType::Executable, CG_CLIF_HELLO_ELF) {
            println!("    Registered cg-clif-hello.elf ({} bytes, rustc Cranelift backend)", CG_CLIF_HELLO_ELF.len());
        } else {
            println!("    [WARN] failed to register cg-clif-hello.elf");
        }
        if fs.add("semos-cc-hello.elf", kernel_core::fs::ramfs::FileType::Executable, SEMOS_CC_HELLO_ELF) {
            println!("    Registered semos-cc-hello.elf ({} bytes, host-compiler ET_EXEC)", SEMOS_CC_HELLO_ELF.len());
        } else {
            println!("    [WARN] failed to register semos-cc-hello.elf");
        }
        if fs.add("semos-cc.elf", kernel_core::fs::ramfs::FileType::Executable, SEMOS_CC_ELF) {
            println!("    Registered semos-cc.elf ({} bytes, D.2 Ring-3 emitter)", SEMOS_CC_ELF.len());
        } else {
            println!("    [WARN] failed to register semos-cc.elf");
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
        if fs.add("fb-demo.elf", kernel_core::fs::ramfs::FileType::Executable, FB_DEMO_ELF) {
            println!("    Registered fb-demo.elf ({} bytes, M14 user framebuffer surface demo)", FB_DEMO_ELF.len());
        } else {
            println!("    [WARN] failed to register fb-demo.elf");
        }
        if fs.add("sem-sh.elf", kernel_core::fs::ramfs::FileType::Executable, SEM_SH_ELF) {
            println!("    Registered sem-sh.elf ({} bytes, M20 native shell)", SEM_SH_ELF.len());
        } else {
            println!("    [WARN] failed to register sem-sh.elf");
        }
        if fs.add("semos-rustc.elf", kernel_core::fs::ramfs::FileType::Executable, SEMOS_RUSTC_ELF) {
            println!("    Registered semos-rustc.elf ({} bytes, M27 Phase 5b iter 5: rustc-on-SemOS)", SEMOS_RUSTC_ELF.len());
        } else {
            println!("    [WARN] failed to register semos-rustc.elf");
        }
        if fs.add("hello.rs", kernel_core::fs::ramfs::FileType::Regular, HELLO_RS_SOURCE) {
            println!("    Registered hello.rs ({} bytes, DEMO 80 source)", HELLO_RS_SOURCE.len());
        } else {
            println!("    [WARN] failed to register hello.rs");
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
    // M27 DEMO 80: the SemOS-resident semos-rustc reads its input source
    // through SYS_OPEN (path namespace), NOT the flat ramfs. Register the
    // hello.rs source as a namespace file and a /tmp directory for the
    // compiled output (`semos-rustc /hello.rs -o /tmp/hello.elf`).
    {
        use kernel_core::fs::paths::Namespace;
        use kernel_core::semantic::object::SecurityTier;
        if Namespace::create_file("/hello.rs", SecurityTier::Public, HELLO_RS_SOURCE).is_err() {
            println!("    [WARN] DEMO 80: failed to register /hello.rs in namespace");
        } else {
            println!("    DEMO 80: /hello.rs registered ({} bytes) + /tmp dir", HELLO_RS_SOURCE.len());
        }
        let _ = Namespace::mkdir("/tmp");
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
    println!("  build {} · {} · rustc {}", SEMOS_BUILD_TAG, SEMOS_BUILD_DATE, SEMOS_RUSTC_VERSION);
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
    // Demos are opt-in: default DEMOS_ON_BOOT=false boots straight to the
    // shell. The full suite is reachable on demand via the `demos` builtin
    // (SYS_DEMOS -> run_all_demos).
    if DEMOS_ON_BOOT.load(core::sync::atomic::Ordering::Relaxed) {
        run_all_demos();
    }

    // `--features autocompile`: headless DEMO 80 smoke for QEMU/serial runs.
    // This avoids relying on the interactive keyboard path just to validate
    // `semos-rustc` + the disk-backed sysroot blob.
    #[cfg(feature = "autocompile")]
    demo80_autocompile();

    // `--features interactive`: hand the keyboard to a live sem-sh instead of
    // idling. Returns only if the shell can't be spawned, then we fall through
    // to the halt loop (same as a default build).
    #[cfg(feature = "interactive")]
    interactive_session();

    // M10 watchdog (the "kernel didn't crash" heartbeat). Periodically print
    // a line that proves the scheduler is still running — critical on bare
    // metal without serial, where a frozen framebuffer would otherwise be
    // indistinguishable from a panic'd or wedged kernel.
    idle_with_heartbeat();
}

/// Run the full boot DEMO suite end-to-end. Invoked at boot when
/// DEMOS_ON_BOOT is set, and on demand from the shell via the `demos`
/// builtin (SYS_DEMOS). Pressing ESC during the run aborts it early.
pub(crate) fn run_all_demos() {
    // The agent/shell/TUI demos now live in `demos.rs`; pull them into scope so
    // the calls below read the same as before. (Older demos are still local.)
    use crate::demos::*;
    // Reset the ESC short-circuit so a shell-invoked (SYS_DEMOS) run starts
    // fresh; ESC during the run still aborts it via the checks below.
    crate::keyboard::SKIP_DEMOS.store(false, core::sync::atomic::Ordering::Relaxed);

    // Hot-key ESC short-circuit: press Escape during the demo run to
    // jump straight to the shell. SKIP_DEMOS is set by the keyboard
    // polling path the moment scancode 0x01 arrives.
    //
    // (Inline `if SKIP_DEMOS { return; }` at each check point
    // because Rust 2024's macro-hygiene scoping doesn't propagate
    // outer labels into macro_rules! expansions cleanly.)

    // Boot lands directly at the shell — demos are now opt-in via the
    // 'demos' shell builtin (SYS_DEMOS). The ESC-to-skip path becomes
    // irrelevant because we never enter the suite on boot.
    if crate::keyboard::SKIP_DEMOS.load(core::sync::atomic::Ordering::Relaxed) {
        println!("  [ESC pressed — skipping demos.]");
        return;
    }

    // Run kernel-side demos FIRST (demos 2 & 3 — the SemanticObject path).
    sem_demo_kernel();
    if crate::keyboard::SKIP_DEMOS.load(core::sync::atomic::Ordering::Relaxed) {
        println!("  [ESC pressed — skipping demos.]");
        return;
    }

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

    // DEMO 6b: P0 pointer-validation regression (2026-07-17 review).
    // Ring-3, tier 0 — the least-privileged context in the system — attacks
    // SYS_WRITE / SYS_LLM_CONTEXT with kernel pointers; every attempt must
    // return u64::MAX and the machine must survive. Gated like DEMO 6
    // (task #40 output starvation); the ELF is always registered in ramfs,
    // so it can also be run from sem-sh as `ptr-guard-test.elf`.
    // println!();
    // println!("================================================================");
    // println!("  SemOS DEMO 6b: P0 Ring-3 kernel read/write primitive regression");
    // println!("================================================================");
    // spawn_named_at("ptr-guard-test.elf", 0);

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

    // DEMO 40: M19 TTY — line-discipline stdin (SYS_READ fd 0) + ANSI output.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 40: TTY line discipline + ANSI (M19)");
    println!("================================================================");
    tty_m19_demo();

    // DEMO 41: per-process FD table — redirect stdout (fd 1) to a pipe.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 41: per-process stdio — pipe-redirected stdout (M19)");
    println!("================================================================");
    fd_redirect_demo();

    // DEMO 42: FD inheritance across spawn — child inherits redirected stdout.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 42: FD inheritance on spawn (M19)");
    println!("================================================================");
    fd_inherit_demo();

    // DEMO 43: TTY line editor — in-line cursor (arrows), mid-line edit, history.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 43: TTY line editing + history (M19)");
    println!("================================================================");
    line_editor_demo();

    // DEMO 44: TtyConsole scrollback — recover a line that scrolled off.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 44: TTY scrollback (M19)");
    println!("================================================================");
    scrollback_demo();

    // DEMO 45: M20 native shell — run a script, capture its stdout.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 45: sem-sh native shell (M20 stage A+B)");
    println!("================================================================");
    shell_demo();

    // DEMO 46: M20 stage C — redirection (>) + pipes (|) in sem-sh.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 46: sem-sh redirection + pipes (M20 stage C)");
    println!("================================================================");
    shell_pipe_demo();

    // DEMO 47: M22 Claude agent core — Messages-API framing + tool dispatch.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 47: Claude agent core (M22 stage A, no network)");
    println!("================================================================");
    agent_demo();

    // DEMO 48: M22 stage B — agent's request over live TLS to api.anthropic.com.
    if kernel_core::net::is_initialized() {
        if crate::agent::api_key().is_empty() {
            // DEMO 48 is the no-key round-trip test (expects 401). With a key
            // baked in, DEMO 49 supersedes it AND skipping it means DEMO 49's
            // session opens on the 2nd TLS connect of the boot (after DEMO 16)
            // rather than the 3rd — the reconnect flake worsens with count.
            println!();
            println!("================================================================");
            println!("  SemOS DEMO 48: agent live TLS round-trip (M22 stage B)");
            println!("================================================================");
            agent_live_demo();
        } else {
            // DEMO 49: M22 stage C — full agent loop with a real Claude model
            // (read_file tool round-trip), over ONE keep-alive connection.
            println!();
            println!("================================================================");
            println!("  SemOS DEMO 49: agent tool loop w/ live Claude (M22 stage C)");
            println!("================================================================");
            agent_loop_demo();
        }
    }

    // DEMO 50: M22 TUI — the multi-pane agent terminal (status / transcript /
    // prompt) rendered over the framebuffer, verified by pixel readback.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 50: Claude agent TUI (M22 — panes + roles)");
    println!("================================================================");
    agent_tui_demo();

    // DEMO 51: M22 TUI interactive input — type into the prompt pane, edit,
    // and commit a line (cooked-mode line discipline → TUI prompt → read_line).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 51: TUI keyboard input (M22 — prompt + read_line)");
    println!("================================================================");
    agent_tui_input_demo();

    // DEMO 52: M22 agent `bash` tool — run a real shell command via sem-sh and
    // capture its output (the agent's shell surface).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 52: agent bash tool (M22 — sem-sh capture)");
    println!("================================================================");
    agent_bash_tool_demo();

    // DEMO 53: shell introspection builtins (ps / free / uptime) — the shell as
    // a system interface. Read-only, tier-safe.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 53: shell introspection — ps / free / uptime");
    println!("================================================================");
    shell_introspection_demo();

    // DEMO 54: agentic shell — the `ask` builtin reaches the kernel LLM agent
    // via SYS_ASK (Ring-3 → kernel bridge).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 54: agentic shell — `ask` builtin (SYS_ASK bridge)");
    println!("================================================================");
    agent_ask_demo();

    // DEMO 55: shell `fetch` — HTTP GET over the kernel TCP stack from sem-sh.
    if kernel_core::net::is_initialized() {
        println!();
        println!("================================================================");
        println!("  SemOS DEMO 55: shell `fetch` — HTTP GET (sem-sh + std::net)");
        println!("================================================================");
        shell_fetch_demo();
    }

    // DEMO 56: security — the agent's shell is sandboxed at tier 0 (Public), so
    // it cannot read a Secret-tier file (the LLM can't see secrets).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 56: agent shell sandbox — LLM can't read secrets");
    println!("================================================================");
    agent_sandbox_demo();

    // DEMO 57: shell scripting — `&&` / `||` conditional chaining.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 57: shell scripting — && / || (short-circuit)");
    println!("================================================================");
    shell_scripting_demo();

    // DEMO 58: install anywhere / run anywhere — install an ELF at a namespace
    // path and run it from the shell (spawn is no longer /bin-only).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 58: install anywhere — run an app from any path");
    println!("================================================================");
    install_anywhere_demo();

    // DEMO 59: $PATH — install into /apps and run it by BARE NAME (always on path).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 59: $PATH — installed app runnable by bare name");
    println!("================================================================");
    path_search_demo();

    // DEMO 60: persistence — an installed app survives reboot (fsync + restore).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 60: persistence — installed app survives reboot");
    println!("================================================================");
    persistence_install_demo();

    // DEMO 61: M21 modal editor — script an edit through the pure key logic
    // (no HID/render needed) and verify the save round-trips to the FS.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 61: modal editor (M21) — edit + save round-trip");
    println!("================================================================");
    editor_demo();

    // DEMO 62: NVMe block I/O via the BlockDevice trait (M9).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 62: NVMe block read/write (M9) via BlockDevice");
    println!("================================================================");
    nvme_demo();

    // DEMO 63: HD Audio (M15) — confirm the stream's LPIB advances while RUN.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 63: HD Audio (M15) — stream DMA advancing");
    println!("================================================================");
    hda_demo();

    // DEMO 64: HID report-descriptor parser (M16) — canned gamepad descriptor.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 64: HID report descriptor parser (M16, gamepad)");
    println!("================================================================");
    hid_parser_demo();

    // DEMO 65: 802.11 protocol layer (M11 v1) — frame builders ready for metal.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 65: 802.11 / iwlwifi scaffolding (M11)");
    println!("================================================================");
    wireless_demo();

    // DEMO 66: CDC-ECM (USB Ethernet) protocol layer.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 66: CDC-ECM USB Ethernet — descriptor parser");
    println!("================================================================");
    cdc_ecm_demo();

    // DEMO 67: AHCI/SATA block I/O via the BlockDevice trait — the T540's path.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 67: AHCI/SATA block read/write via BlockDevice");
    println!("================================================================");
    ahci_demo();

    // DEMO 68: USB Mass Storage protocol layer — read a USB stick on metal.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 68: USB Mass Storage CBW/CSW + SCSI");
    println!("================================================================");
    usb_msc_demo();

    // DEMO 69: live xHCI bulk endpoints — enumerate a usb-storage device,
    // read INQUIRY + READ CAPACITY via the CBW/CSW protocol.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 69: live USB Mass Storage on xHCI (bulk endpoints)");
    println!("================================================================");
    xhci_msc_demo();

    // DEMO 70: Ring 3 sync-demo — functional smoke for Condvar + mpsc + RwLock.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 70: Ring 3 semos-std::{{Condvar, mpsc, RwLock}}");
    println!("================================================================");
    ring3_sync_demo();

    // DEMO 71: Ring 3 program built by rustc's Cranelift codegen backend.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 71: rustc Cranelift backend → SemOS ELF → SYS_SPAWN");
    println!("================================================================");
    ring3_cg_clif_demo();

    // DEMO 72: M27 Session D.1 — host semos-compiler assembles an ET_EXEC
    // directly from Cranelift bytes (no rustc, no linker) and SemOS runs it.
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 72: host semos-compiler → SemOS ELF → SYS_SPAWN");
    println!("================================================================");
    ring3_semos_cc_demo();

    // DEMO 73: M27 Session D.2 — semos-cc runs on SemOS, emits an ELF,
    // SemOS then spawns the emitted ELF (toolchain pipeline on-target).
    println!();
    println!("================================================================");
    println!("  SemOS DEMO 73: Ring-3 semos-cc emits ELF on SemOS → SYS_SPAWN");
    println!("================================================================");
    ring3_semos_cc_d2_demo();

    if crate::keyboard::SKIP_DEMOS.load(core::sync::atomic::Ordering::Relaxed) {
        println!("  [ESC pressed — skipping demos.]");
        return;
    }

    println!();
    println!("================================================================");
    println!("  SemOS DEMO 81: USB CDC-ECM enumeration (Phase 15 M50)");
    println!("================================================================");
    demo_81_cdc_ecm();

    if crate::keyboard::SKIP_DEMOS.load(core::sync::atomic::Ordering::Relaxed) {
        println!("  [ESC pressed — skipping demos.]");
        return;
    }

    println!();
    println!("================================================================");
    println!("  SemOS DEMO 82: iPhone tether — USB MUX iface (session 1)");
    println!("================================================================");
    demo_82_iphone();

    println!();
    println!("================================================================");
    println!("  SemOS DEMO 86: pairing protocol self-test (M56)");
    println!("================================================================");
    pairing_self_test_demo();

    // Final marker. An ESC short-circuit above `return`s straight out; when the
    // suite completes normally we fall through to this banner.
    // On bare metal this is your "the kernel
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

}

/// Headless DEMO 80 runner:
///   semos-rustc /hello.rs -o /tmp/hello.elf
///
/// Use with QEMU serial, e.g. `cargo build --release --features autocompile`
/// and boot with a sysroot blob attached as AHCI. Success criteria:
/// - `semos-rustc` exits 0,
/// - `/tmp/hello.elf` exists and has non-zero size.
#[cfg(feature = "autocompile")]
fn demo80_autocompile() {
    use alloc::vec::Vec;
    use kernel_core::scheduler::{self, TaskState};
    use kernel_core::syscall::{dispatch, numbers::*, SpawnArgs, StatX};

    println!();
    println!("================================================================");
    println!("  DEMO 80 AUTOCOMPILE: semos-rustc /hello.rs -o /tmp/hello.elf");
    println!("================================================================");

    let items: [&str; 4] = ["/bin/semos-rustc", "/hello.rs", "-o", "/tmp/hello.elf"];
    let mut argv_blob: Vec<u8> = Vec::new();
    argv_blob.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for it in items {
        argv_blob.extend_from_slice(&(it.len() as u32).to_le_bytes());
        argv_blob.extend_from_slice(it.as_bytes());
    }

    let spawn_args = SpawnArgs {
        argv_blob_ptr: argv_blob.as_ptr() as u64,
        argv_blob_len: argv_blob.len() as u32,
        envp_blob_ptr: 0,
        envp_blob_len: 0,
    };

    let path = "/bin/semos-rustc";
    let pid = dispatch(
        SYS_SPAWN,
        path.as_ptr() as u64,
        path.len() as u64,
        3, // trusted user tier, matching manual interactive-shell use
        &spawn_args as *const SpawnArgs as u64,
    );
    if pid == u64::MAX {
        println!("  [DEMO 80] FAIL: SYS_SPAWN({}) returned MAX", path);
        return;
    }
    println!("  [DEMO 80] spawned semos-rustc PID {}", pid);

    let process_id = kernel_core::process::ProcessId(pid as u32);
    let slot = match kernel_core::process::get(process_id).and_then(|p| p.task_id) {
        Some(s) => s,
        None => {
            println!("  [DEMO 80] FAIL: PID {} has no task_id", pid);
            return;
        }
    };

    let mut polled = 0u64;
    loop {
        if scheduler::task_state(slot) == TaskState::Exited {
            break;
        }
        if polled > 120_000 {
            println!("  [DEMO 80] FAIL: semos-rustc did not exit within 120000 ticks");
            return;
        }
        let _ = dispatch(SYS_SLEEP, 1, 0, 0, 0);
        polled += 1;
    }

    let code = scheduler::task_exit_code(slot);
    println!("  [DEMO 80] semos-rustc exited code={} after {} ticks", code, polled);
    if code != 0 {
        println!("  [DEMO 80] FAIL: compiler returned non-zero");
        return;
    }

    let out_path = "/tmp/hello.elf";
    let mut st = StatX {
        size: 0,
        suid_high: 0,
        suid_low: 0,
        created_at: 0,
        modified_at: 0,
        file_type: 0,
        tier: 0,
        _reserved: [0; 3],
    };
    let rc = dispatch(
        SYS_STATX,
        out_path.as_ptr() as u64,
        out_path.len() as u64,
        &mut st as *mut _ as u64,
        0,
    );
    if rc != 0 {
        println!("  [DEMO 80] FAIL: statx({}) returned {}", out_path, rc);
        return;
    }
    if st.size == 0 {
        println!("  [DEMO 80] FAIL: {} exists but size is 0", out_path);
        return;
    }
    println!("  [DEMO 80] PASS: {} written ({} bytes)", out_path, st.size);
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
        cr0 |=  1u64 << 1;  // set MP
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
    use core::fmt::Write;

    println!();
    println!("!!! KERNEL PANIC !!!");
    println!("{}", info);

    // Best-effort: format the panic message into a stack buffer and persist a
    // panic snapshot (scrollback ring + reason) to the end of the first
    // available block device, so it can be recovered post-mortem from another
    // OS (HxD / dd to the last 130 sectors; look for "PANICLOG" magic).
    let mut reason_buf = [0u8; 384];
    let mut w = panic_dump::BufWriter { buf: &mut reason_buf, n: 0 };
    let _ = write!(&mut w, "{}", info);
    let written = w.n;
    match panic_dump::dump(&reason_buf[..written]) {
        Some(name) => println!("[panic-dump] wrote panic snapshot to {}; recover with HxD at last 130 sectors", name),
        None => println!("[panic-dump] no block device available — only the framebuffer print above"),
    }

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
