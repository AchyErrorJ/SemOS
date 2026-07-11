//! Semantic OS — aarch64 HAL with kernel-core scheduler integration.
//!
//! Bare-metal aarch64 kernel for QEMU `-M virt`. Boots, turns on the MMU,
//! brings up GICv2 + the ARM Generic Timer, registers an `Aarch64Platform`
//! with `kernel-core`, and runs preemptive kernel-mode tasks through the
//! architecture-independent scheduler.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::arch::global_asm;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};

mod context;
mod fdt;
mod memory;
mod mmu;
mod platform_impl;
mod serial;

// ---- Global allocator (backs all `alloc` usage in kernel-core) -------------
struct KernelGlobalAlloc;

unsafe impl GlobalAlloc for KernelGlobalAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        kernel_core::memory::heap::allocate(layout.size(), layout.align())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        kernel_core::memory::heap::deallocate(ptr, layout.size(), layout.align());
    }
}

#[global_allocator]
static KERNEL_ALLOCATOR: KernelGlobalAlloc = KernelGlobalAlloc;

// Boot entry. Linked first (`.text._start`) at 0x4008_0000 where QEMU's -kernel
// loader places us. Set the stack, zero BSS, branch into Rust. `ldr =sym` uses a
// literal pool the assembler emits — fine for our fixed link address.
global_asm!(
    r#"
.section .text._start
.global _start
_start:
    mov     x19, x0                 // preserve DTB pointer (x0) from QEMU/m1n1
    ldr     x0, =_stack_top
    mov     sp, x0
    ldr     x0, =_bss_start
    ldr     x1, =_bss_end
0:  cmp     x0, x1
    b.hs    1f
    str     xzr, [x0], #8
    b       0b
1:  mov     x0, x19                 // pass DTB pointer as kmain's first arg
    bl      kmain
2:  wfe
    b       2b

// ---- Exception vector table (AArch64, EL1) -----------------------------------
// 16 entries × 0x80 bytes, table aligned to 0x800 (VBAR_EL1 requirement). Each
// entry passes its index + the syndrome regs (ESR/ELR/FAR) to the Rust handler,
// then halts. The 16 are 4 groups of Sync/IRQ/FIQ/SError: Current-EL-SP0 (0-3),
// Current-EL-SPx (4-7), Lower-EL-AArch64 (8-11), Lower-EL-AArch32 (12-15). A BRK
// at EL1 (which uses SPx) lands in entry 4.
.macro VEC idx
.balign 0x80
    mov     x0, #\idx
    mrs     x1, esr_el1
    mrs     x2, elr_el1
    mrs     x3, far_el1
    bl      exc_handler
0:  wfe
    b       0b
.endm

// Each vector-table slot is only 0x80 bytes — far too small for a full context
// save/restore. So the IRQ slots hold just a branch to `irq_entry` (below the
// table), which does the real work. (The VEC macro's 7 instructions fit fine.)
.macro IRQ_VEC
.balign 0x80
    b       irq_entry
.endm

.section .vectors, "ax"
.balign 0x800
.global _vectors
_vectors:
    VEC 0          // Current EL, SP0, Synchronous
    IRQ_VEC        // Current EL, SP0, IRQ
    VEC 2          // Current EL, SP0, FIQ
    VEC 3          // Current EL, SP0, SError
    VEC 4          // Current EL, SPx, Synchronous
    IRQ_VEC        // Current EL, SPx, IRQ  <- timer IRQs land here at EL1
    VEC 6          // Current EL, SPx, FIQ
    VEC 7          // Current EL, SPx, SError
    VEC 8
    VEC 9
    VEC 10
    VEC 11
    VEC 12
    VEC 13
    VEC 14
    VEC 15

// IRQ trampoline (outside the 0x80 slots): save full integer context + ELR/SPSR,
// call the Rust handler, restore, and ERET. Async interrupts can fire mid-stream,
// so every caller-saved register must be preserved.
.section .text
.global irq_entry
irq_entry:
    sub     sp, sp, #272
    stp     x0,  x1,  [sp, #16*0]
    stp     x2,  x3,  [sp, #16*1]
    stp     x4,  x5,  [sp, #16*2]
    stp     x6,  x7,  [sp, #16*3]
    stp     x8,  x9,  [sp, #16*4]
    stp     x10, x11, [sp, #16*5]
    stp     x12, x13, [sp, #16*6]
    stp     x14, x15, [sp, #16*7]
    stp     x16, x17, [sp, #16*8]
    stp     x18, x19, [sp, #16*9]
    stp     x20, x21, [sp, #16*10]
    stp     x22, x23, [sp, #16*11]
    stp     x24, x25, [sp, #16*12]
    stp     x26, x27, [sp, #16*13]
    stp     x28, x29, [sp, #16*14]
    mrs     x0, elr_el1
    mrs     x1, spsr_el1
    stp     x30, x0,  [sp, #16*15]
    str     x1,       [sp, #16*16]
    mov     x0, sp                  // pass the saved-context SP to the handler
    bl      irq_handler             // returns the NEXT task's saved-context SP
    mov     sp, x0                  // ...switch to it (the context switch)
    ldr     x1,       [sp, #16*16]
    ldp     x30, x0,  [sp, #16*15]
    msr     elr_el1,  x0
    msr     spsr_el1, x1
    ldp     x0,  x1,  [sp, #16*0]
    ldp     x2,  x3,  [sp, #16*1]
    ldp     x4,  x5,  [sp, #16*2]
    ldp     x6,  x7,  [sp, #16*3]
    ldp     x8,  x9,  [sp, #16*4]
    ldp     x10, x11, [sp, #16*5]
    ldp     x12, x13, [sp, #16*6]
    ldp     x14, x15, [sp, #16*7]
    ldp     x16, x17, [sp, #16*8]
    ldp     x18, x19, [sp, #16*9]
    ldp     x20, x21, [sp, #16*10]
    ldp     x22, x23, [sp, #16*11]
    ldp     x24, x25, [sp, #16*12]
    ldp     x26, x27, [sp, #16*13]
    ldp     x28, x29, [sp, #16*14]
    add     sp, sp, #272
    eret
"#
);

// ---- GICv2 + generic timer (interrupts) -------------------------------------
// QEMU `-M virt` with GICv2: distributor @0x0800_0000, CPU interface @0x0801_0000.
// The non-secure EL1 physical timer (CNTP) fires PPI INTID 30.
const GICD_BASE: u64 = 0x0800_0000;
const GICC_BASE: u64 = 0x0801_0000;
const TIMER_INTID: u32 = 30;

static mut TIMER_INTERVAL: u64 = 0;
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Return the monotonic tick count (called by `kernel_core::Platform::ticks`).
pub fn get_ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

#[inline]
unsafe fn mmio_w32(addr: u64, v: u32) {
    core::ptr::write_volatile(addr as *mut u32, v);
}
#[inline]
unsafe fn mmio_r32(addr: u64) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}
#[inline]
unsafe fn mmio_w8(addr: u64, v: u8) {
    core::ptr::write_volatile(addr as *mut u8, v);
}

/// Bring up GICv2: enable the distributor + CPU interface, route INTID 30.
unsafe fn gic_init() {
    // Distributor: enable, set-enable INTID 30, give it a priority.
    mmio_w32(GICD_BASE + 0x000, 1); // GICD_CTLR.EnableGrp0
    mmio_w32(GICD_BASE + 0x100, 1 << TIMER_INTID); // GICD_ISENABLER0 bit 30
    mmio_w8(GICD_BASE + 0x400 + TIMER_INTID as u64, 0xA0); // GICD_IPRIORITYR[30]
    // CPU interface: allow all priorities, enable.
    mmio_w32(GICC_BASE + 0x004, 0xFF); // GICC_PMR
    mmio_w32(GICC_BASE + 0x000, 1); // GICC_CTLR.Enable
}

/// Program the EL1 physical timer to fire at SCHEDULER_TICK_HZ and enable it.
unsafe fn timer_init() {
    let freq: u64;
    core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq);
    TIMER_INTERVAL = freq / kernel_core::scheduler::SCHEDULER_TICK_HZ;
    core::arch::asm!("msr cntp_tval_el0, {}", in(reg) TIMER_INTERVAL);
    core::arch::asm!("msr cntp_ctl_el0, {}", in(reg) 1u64); // ENABLE=1, IMASK=0
    serial::uart_str("  CNTFRQ_EL0 = ");
    uart_hex(freq);
    serial::uart_str(" Hz, tick interval set for ");
    uart_hex(kernel_core::scheduler::SCHEDULER_TICK_HZ);
    serial::uart_str(" Hz.\n");
}

#[inline]
unsafe fn rearm_timer() {
    core::arch::asm!("msr cntp_tval_el0, {}", in(reg) TIMER_INTERVAL);
}

// ---- Exception handlers -----------------------------------------------------

/// Timer IRQ handler. `cur_sp` is the running task's saved-frame SP. Services
/// the timer and delegates the scheduling decision to `context::timer_schedule`,
/// which returns the next task's frame SP for `irq_entry` to restore.
#[no_mangle]
extern "C" fn irq_handler(cur_sp: u64) -> u64 {
    unsafe {
        let iar = mmio_r32(GICC_BASE + 0x00C); // GICC_IAR
        let intid = iar & 0x3FF;
        if intid == TIMER_INTID {
            TICKS.fetch_add(1, Ordering::Relaxed);
            rearm_timer();
        }
        mmio_w32(GICC_BASE + 0x010, iar); // GICC_EOIR
        context::timer_schedule(cur_sp)
    }
}

/// Common synchronous exception handler. Reports and halts.
#[no_mangle]
extern "C" fn exc_handler(index: u64, esr: u64, elr: u64, far: u64) {
    serial::uart_str("\n[aarch64] *** EXCEPTION *** vector=");
    uart_hex(index);
    serial::uart_str("\n  ESR_EL1=");
    uart_hex(esr);
    let ec = (esr >> 26) & 0x3F;
    serial::uart_str(" (EC=");
    uart_hex(ec);
    serial::uart_str(match ec {
        0x3C => " BRK",
        0x24 | 0x25 => " data-abort",
        0x20 | 0x21 => " instr-abort",
        0x15 => " SVC",
        _ => " other",
    });
    serial::uart_str(")\n  ELR_EL1=");
    uart_hex(elr);
    serial::uart_str(" FAR_EL1=");
    uart_hex(far);
    serial::uart_str("\n  halting.\n");
}

// ---- Helper formatting routines ---------------------------------------------

/// Print a byte as two hex chars.
fn uart_byte_hex(b: u8) {
    for nib in [b >> 4, b & 0xF] {
        serial::uart_put(if nib < 10 { b'0' + nib } else { b'a' + (nib - 10) });
    }
}

/// Print a u64 as hex.
fn uart_hex(mut v: u64) {
    serial::uart_str("0x");
    for i in (0..16).rev() {
        let nib = ((v >> (i * 4)) & 0xF) as u8;
        serial::uart_put(if nib < 10 { b'0' + nib } else { b'a' + (nib - 10) });
    }
    let _ = &mut v;
}

// ---- Demo kernel tasks ------------------------------------------------------

fn demo_a() -> ! {
    loop {
        kernel_core::platform::log("A");
        for _ in 0..6_000_000 {
            core::hint::spin_loop();
        }
    }
}

fn demo_b() -> ! {
    loop {
        kernel_core::platform::log("B");
        for _ in 0..6_000_000 {
            core::hint::spin_loop();
        }
    }
}

// ---- MMU self-test ----------------------------------------------------------

fn mmu_self_test() {
    kernel_core::platform::log("  [mmu] self-test begin\n");

    let as_handle = kernel_core::platform::get().create_address_space(3)
        .expect("create_address_space failed");
    kernel_core::platform::log("  [mmu] created AS, TTBR0=");
    kernel_core::platform::log_num(as_handle);
    kernel_core::platform::log("\n");

    let ok = kernel_core::platform::get().map_user_region(as_handle, 0x1000, 0x1000)
        && kernel_core::platform::get().map_user_region(as_handle, 0x2000, 0x1000)
        && kernel_core::platform::get().map_user_region(as_handle, 0x3000, 0x1000);
    kernel_core::platform::log("  [mmu] map_user_region 3 pages: ");
    kernel_core::platform::log(if ok { "PASS\n" } else { "FAIL\n" });

    let stack_top = kernel_core::platform::get().map_user_stack(as_handle, 0x10000, 0x4000)
        .expect("map_user_stack failed");
    kernel_core::platform::log("  [mmu] user stack mapped at top=");
    kernel_core::platform::log_num(stack_top);
    kernel_core::platform::log("\n");

    kernel_core::platform::get().destroy_address_space(as_handle);
    kernel_core::platform::log("  [mmu] destroyed AS\n");

    let (total, used, free) = crate::memory::stats();
    kernel_core::platform::log("  [memory] frames: total=");
    kernel_core::platform::log_num(total as u64);
    kernel_core::platform::log(" used=");
    kernel_core::platform::log_num(used as u64);
    kernel_core::platform::log(" free=");
    kernel_core::platform::log_num(free as u64);
    kernel_core::platform::log("\n");

    if used == 0 {
        kernel_core::platform::log("  [mmu] self-test PASS — no leaks\n");
    } else {
        kernel_core::platform::log("  [mmu] self-test FAIL — leak detected\n");
    }
}

// ---- Physical memory discovery (M4/M5) --------------------------------------

/// The RAM banks the tree describes. Needed before the MMU comes up, because
/// the boot map is built from them.
///
/// With no tree — or a tree with no `/memory` — fall back to what QEMU `virt`
/// hands out, so a broken DTB degrades to the old fixed behavior rather than to
/// a machine with no RAM at all.
fn discover_banks(fdt: Option<fdt::Fdt>) -> ([(u64, u64); 8], usize) {
    let mut banks: [(u64, u64); 8] = [(0, 0); 8];
    let mut n = 0usize;
    if let Some(f) = fdt {
        n = f.memory_banks(&mut banks);
    }
    if n == 0 {
        banks[0] = (0x4000_0000, 0x0800_0000);
        n = 1;
        serial::uart_str("  [memory] no /memory in FDT — assuming QEMU virt 128 MiB\n");
    }
    (banks, n)
}

/// Build the physical frame pool from the device tree.
///
/// Everything the loader left live in RAM has to be reserved before the pool
/// opens, or we will hand it out and overwrite it:
///
/// * **the kernel image and its stack** — we are executing out of them;
/// * **the DTB itself** — QEMU parks it at `0x4400_0000`, which is inside RAM
///   and *above* the old `_stack_top` pool floor, so the previous allocator was
///   free to hand the device tree out as scratch. It survived only because
///   nothing had claimed enough frames to reach it yet;
/// * **the header's reservation block and `/reserved-memory`** — on Apple these
///   cover firmware regions and the framebuffer m1n1 is still scanning out of.
///
/// Reservations are rounded outward to whole frames: over-reserving costs a
/// page, under-reserving corrupts something that is still in use.
fn init_memory(fdt: Option<fdt::Fdt>, dtb: u64, banks: &[(u64, u64)]) {
    extern "C" {
        static _kernel_start: u8;
        static _stack_top: u8;
    }
    let kernel_start = core::ptr::addr_of!(_kernel_start) as u64;
    let stack_top = core::ptr::addr_of!(_stack_top) as u64;

    unsafe {
        if let Some(f) = fdt {
            // The DTB is read-only to us, but it lives in RAM: reserve it or the
            // allocator will hand it out from under the parser.
            crate::memory::reserve(dtb, f.totalsize() as u64);

            let mut resv: [(u64, u64); 16] = [(0, 0); 16];
            let n = f.mem_reservations(&mut resv);
            for &(base, size) in &resv[..n] {
                crate::memory::reserve(base, size);
            }
            let n = f.reserved_memory(&mut resv);
            for &(base, size) in &resv[..n] {
                crate::memory::reserve(base, size);
            }
            if n > 0 {
                kernel_core::platform::log("  [memory] honored ");
                kernel_core::platform::log_num(n as u64);
                kernel_core::platform::log(" /reserved-memory region(s)\n");
            }
        }

        crate::memory::reserve(kernel_start, stack_top - kernel_start);

        // Anything the boot map could not cover has no virtual address, and
        // mmu.rs zeroes every frame it allocates — an unmapped frame is a data
        // abort, not a bad pointer to debug later. After M8 this should be empty
        // for any machine whose RAM fits the 39-bit VA and the static L2 pool;
        // if it ever isn't, the kernel withholds that RAM and says so rather
        // than handing out memory it cannot reach.
        let mut mapped: [(u64, u64); 16] = [(0, 0); 16];
        let nmapped = crate::mmu::mapped_ram(&mut mapped);

        for &(base, size) in banks {
            crate::memory::add_bank(base, size);
            kernel_core::platform::log("  [memory] bank base=");
            kernel_core::platform::log_num(base);
            kernel_core::platform::log(" size=");
            kernel_core::platform::log_num(size / 1024 / 1024);
            kernel_core::platform::log(" MiB\n");

            let end = base.saturating_add(size);
            let mut cursor = base;
            while cursor < end {
                // Is `cursor` inside a mapped range? If so, skip to its end.
                let mut covered_to = None;
                for &(mb, me) in &mapped[..nmapped] {
                    if cursor >= mb && cursor < me {
                        covered_to = Some(me.min(end));
                        break;
                    }
                }
                if let Some(to) = covered_to {
                    cursor = to;
                    continue;
                }
                // Not mapped. Reserve up to the start of the next mapped range.
                let mut next = end;
                for &(mb, _) in &mapped[..nmapped] {
                    if mb > cursor && mb < next {
                        next = mb;
                    }
                }
                crate::memory::reserve(cursor, next - cursor);
                kernel_core::platform::log("  [memory] NOTE: ");
                kernel_core::platform::log_num((next - cursor) / 1024 / 1024);
                kernel_core::platform::log(" MiB unmapped by the boot map — withheld\n");
                cursor = next;
            }
        }

        if !crate::memory::finalize() {
            kernel_core::platform::log("  [memory] FATAL: no usable RAM\n");
            loop {
                core::arch::asm!("wfe");
            }
        }
    }

    let (bm_addr, bm_bytes) = crate::memory::bitmap_info();
    kernel_core::platform::log("  [memory] bitmap ");
    kernel_core::platform::log_num(bm_bytes / 1024);
    kernel_core::platform::log(" KiB @");
    kernel_core::platform::log_num(bm_addr);
    kernel_core::platform::log("\n");

    let (total, _used, _free) = crate::memory::stats();
    kernel_core::platform::log("  [memory] frame pool: ");
    kernel_core::platform::log_num(total as u64);
    kernel_core::platform::log(" frames (");
    kernel_core::platform::log_num((total as u64) * 4096 / 1024 / 1024);
    kernel_core::platform::log(" MiB) allocatable\n");
}

// ---- Kernel entry -----------------------------------------------------------

#[no_mangle]
pub extern "C" fn kmain(dtb: u64) -> ! {
    serial::uart_str("\nSemOS aarch64 — ARM HAL + kernel-core scheduler\n");

    // These two must precede any nontrivial Rust: at -O the compiler emits NEON
    // in plain integer code (the FDT walker's array init does), which faults with
    // EC=0x07 while FPEN is clear — and with VBAR_EL1 still zero that fault
    // vectors into nothing and spins forever, with no output.
    unsafe {
        core::arch::asm!("msr cpacr_el1, {}", in(reg) (3u64 << 20));
        core::arch::asm!("isb");

        extern "C" {
            static _vectors: u8;
        }
        let v = &_vectors as *const u8 as u64;
        core::arch::asm!("msr vbar_el1, {}", in(reg) v);
        core::arch::asm!("isb");
        serial::uart_str("  FP/SIMD enabled, VBAR_EL1 set = ");
        uart_hex(v);
        serial::uart_str("\n");
    }

    // M1 verify: parse the device tree m1n1/QEMU handed us in x0. The MMU is
    // still off here, so the physical DTB pointer is directly readable.
    serial::uart_str("  DTB ptr (x0) = ");
    uart_hex(dtb);
    serial::uart_str("\n");
    let fdt = unsafe { fdt::Fdt::from_ptr(dtb as *const u8) };
    match fdt {
        Some(f) => {
            serial::uart_str("  [fdt] magic OK, totalsize=");
            uart_hex(f.totalsize() as u64);
            serial::uart_str("\n");

            // M2: retarget the console at whatever UART the tree names.
            // Everything from here on — including the panic handler — talks
            // to the discovered device, not the compiled-in guess.
            match serial::init_from_fdt(&f) {
                Some((kind, base)) => {
                    serial::uart_str("  [uart] console from FDT: ");
                    serial::uart_str(kind.name());
                    serial::uart_str(" @");
                    uart_hex(base);
                    serial::uart_str("\n");
                }
                None => {
                    let (kind, base) = serial::current();
                    serial::uart_str("  [uart] no console node in FDT — keeping ");
                    serial::uart_str(kind.name());
                    serial::uart_str(" @");
                    uart_hex(base);
                    serial::uart_str("\n");
                }
            }

            if f.find_compatible("arm,armv8-timer").is_some() {
                serial::uart_str("  [fdt] found arm,armv8-timer node\n");
            }
        }
        None => serial::uart_str("  [fdt] parse FAILED (bad magic?)\n"),
    }

    // Which exception level did QEMU drop us at?
    let el: u64;
    unsafe { core::arch::asm!("mrs {}, CurrentEL", out(reg) el) };
    serial::uart_str("  CurrentEL = EL");
    serial::uart_put(b'0' + (((el >> 2) & 0x3) as u8));
    serial::uart_str("\n");

    let midr: u64;
    unsafe { core::arch::asm!("mrs {}, MIDR_EL1", out(reg) midr) };
    serial::uart_str("  MIDR_EL1  = ");
    uart_hex(midr);
    serial::uart_str("\n");

    // Register the platform before any other kernel-core code runs.
    unsafe {
        kernel_core::set_platform(&platform_impl::PLATFORM);
    }
    kernel_core::platform::log("  [platform] Aarch64Platform registered\n");

    // M8: the boot map is built from the RAM the tree described, so the banks
    // have to be discovered before the MMU comes up — not after. (Reading the
    // FDT here is safe: translation is still off, so the physical pointer works.)
    let (banks, nbanks) = discover_banks(fdt);
    let (_, console_mmio) = serial::current();
    serial::uart_str("  enabling MMU (identity map from FDT, IPS=");
    uart_hex(crate::mmu::ips());
    serial::uart_str(")...\n");
    unsafe { crate::mmu::enable_identity_mmu(&banks[..nbanks], console_mmio) };
    serial::uart_str("  MMU ON — translation active.\n");

    // M4/M5: build the frame pool from the RAM the device tree describes.
    init_memory(fdt, dtb, &banks[..nbanks]);

    // Initialize kernel-core subsystems in the same order as the x86_64 backend.
    kernel_core::scheduler::init_core();
    kernel_core::process::init();
    kernel_core::fs::ramfs::init();
    kernel_core::security::init();
    kernel_core::memory::heap::init();
    let (used, free, blocks) = kernel_core::memory::heap::stats();
    kernel_core::platform::log("  [heap] initialized: ");
    kernel_core::platform::log_num(((used + free) / 1024) as u64);
    kernel_core::platform::log(" KiB arena (");
    kernel_core::platform::log_num(used as u64);
    kernel_core::platform::log(" used, ");
    kernel_core::platform::log_num(free as u64);
    kernel_core::platform::log(" free, ");
    kernel_core::platform::log_num(blocks as u64);
    kernel_core::platform::log(" blocks)\n");

    // Prove the PORTABLE CORE runs on aarch64.
    {
        let h = kernel_core::crypto::sha256::hash(b"SemOS on aarch64");
        let expect: [u8; 32] = [
            0x80, 0x9f, 0x1c, 0x64, 0x8e, 0x7c, 0xa2, 0x84, 0x77, 0xc5, 0xe4, 0x25, 0x64, 0x13,
            0x0c, 0xef, 0xe8, 0x7c, 0x82, 0xeb, 0x60, 0xa9, 0x75, 0x95, 0x4c, 0x12, 0x9b, 0x28,
            0x9e, 0x94, 0x59, 0x8a,
        ];
        kernel_core::platform::log("  kernel-core::sha256 on aarch64 = ");
        for b in &h[..6] {
            kernel_core::platform::log_hex_byte(*b);
        }
        kernel_core::platform::log("... ");
        kernel_core::platform::log(if h == expect {
            "PASS — the portable OS core RUNS on ARM.\n"
        } else {
            "FAIL — digest mismatch.\n"
        });
    }

    // Exercise the new allocator + page tables before enabling preemption.
    mmu_self_test();

    // Spawn preemptible kernel-mode demo tasks.
    context::spawn_task("task_a", demo_a);
    context::spawn_task("task_b", demo_b);
    kernel_core::platform::log("  [tasks] demo tasks A and B spawned\n");

    // Interrupts: GIC + timer, then unmask IRQs.
    unsafe {
        gic_init();
        timer_init();
        core::arch::asm!("msr daifclr, #2"); // clear PSTATE.I — enable IRQs
    }
    kernel_core::platform::log("  [irq] IRQs on — scheduler running\n");

    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    serial::uart_str("\n[aarch64] PANIC — halting.\n");
    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}
