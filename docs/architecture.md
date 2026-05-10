# Architecture

A short module-level map for someone (human or AI) reading the codebase
for the first time. The top-level [README](../README.md) covers the
*why*; this covers the *what is where*.

## Crates

```
kernel-core      — platform-independent. The semantic-object policy lives here.
kernel-x86_64    — x86_64 platform crate. Boots, brings up hardware, implements
                   the Platform trait that kernel-core calls back into.
x86_64-runner    — Windows host tool: takes the kernel ELF and produces a
                   bootable BIOS+UEFI disk image via the bootloader-0.11 crate.
user-programs/*  — Real Rust no_std crates compiled to ELF, embedded in the
                   ramfs at kernel build time and run in Ring 3.
```

`kernel-core` and `kernel-x86_64` are **separate** Cargo crates linked at
compile time. They both opt out of the parent workspace
(`[workspace]` block in each `Cargo.toml`) so they can have independent
target configurations.

## The Platform trait

`kernel-core` is platform-independent: it must not know about IDTs,
page tables, I/O ports, etc. It calls into the platform via a single
trait:

```rust
// kernel-core/src/platform.rs
pub trait Platform: Send + Sync + 'static {
    fn serial_write(&self, s: &str);
    fn ticks(&self) -> u64;
    fn halt(&self);
    fn schedule(&self);                                              // SYS_YIELD
    fn alloc_frame(&self, tier: u8) -> Option<u64>;
    fn free_frame(&self, addr: u64) -> bool;
    fn create_address_space(&self, max_tier: u8) -> Option<u64>;
    fn destroy_address_space(&self, space: u64);
    fn map_elf_segment(&self, space, virt, data, memsz, exec, writ) -> bool;
    fn map_user_stack(&self, space, top, size) -> Option<u64>;
    fn spawn_user_task(&self, name, user_rip, user_rsp, cr3, max_tier) -> Option<usize>;
    fn reap_slot(&self, slot: usize);                                // free per-slot resources
}
```

x86_64 implements this in `kernel-x86_64/src/platform_impl.rs`.
`kernel-core` registers a single global instance at boot via
`set_platform(...)`.

## Boot flow (kernel-x86_64/src/main.rs::kernel_main)

1. `serial::init` — COM1 16550 UART for `println!`
2. `kernel_core::set_platform(&PLATFORM)` — wire the Platform trait
3. Print boot banner
4. `gdt::init` — GDT + TSS, RSP0 placeholder
5. `enable_sse` — clear `CR0.EM`, set `CR4.OSFXSR | CR4.OSXMMEXCPT`
6. `interrupts::init` — IDT with all 32 architectural handlers + timer/keyboard
7. `paging::init(boot_info)` — record `BOOT_CR3` and `PHYS_MEM_OFFSET`
8. `memory::init` — set up the four security-tier memory pools
9. `apic::init` — Local APIC timer (vector 32, periodic, /16, ~62 Hz)
10. `pci::print_bus_0` — log the visible PCI devices
11. `virtio::block::init` — VirtIO Legacy block device handshake + virtqueue
12. `virtio::block::register_with_kernel_core` — register as `"virtio0"`
13. `kernel_core::*::init` — scheduler, process table, ramfs, semantic
    registry, vector index, search engine, LLM service, crypto pools
14. `framebuffer::init` from `boot_info.framebuffer` — 1280×720 BGR console
15. `syscall::init` — write `IA32_LSTAR` to `syscall_entry`, configure STAR
16. `interrupts::test_breakpoint` — single `int3` to verify the IDT works
17. Spawn boot tasks: `task_a`, `task_b`, `task_iso`, `user_task`,
    `init_loader`
18. `loop { hlt }` — kernel idle; timer interrupts drive the scheduler

`init_loader_task` (slot 5) then runs the demos by spawning the embedded
ELFs and printing the kernel-side semantic-object demos.

## Syscall path

```
Ring 3:
  asm!("syscall", in("rax") num, in("rdi") a0, ...)
       │
       ▼
SYSCALL instruction (CPU)
  - saves user RIP -> RCX, RFLAGS -> R11
  - jumps to IA32_LSTAR = syscall_entry (kernel)
       │
       ▼
kernel-x86_64/src/syscall.rs::syscall_entry  (naked)
  - mov rsp, [KERNEL_RSP]                # switch to kernel stack
  - push r15..r11..rcx + callee-saved    # save user state
  - push rdi/rsi/rdx/r10/r8/r9           # save user arg regs (Linux ABI)
  - reorder regs into dispatch's calling convention
  - sti
  - call dispatch(num, a0, a1, a2, a3)
       │
       ▼
kernel-core/src/syscall/mod.rs::dispatch
  - match num { SYS_X => handle_x(...), ... }
       │
       ▼
handler runs (in kernel mode, with caller's CR3)
       │
       ▼
back up the stack: cli; pop user arg regs; pop callee-saved + r11/rcx;
pop rsp; sysretq -> user RIP=RCX, RFLAGS=R11
```

Numbered syscalls are listed in `kernel-core/src/syscall/mod.rs::numbers`.
Categories: Core (0–9), File I/O (10–19), Semantic objects (20–29),
Memory (30–39), Process (40–49), LLM services (50–59), Crypto/Storage
(60–69), System (70–79).

## Context-switch / scheduling

```
Timer (vector 32) -> timer_interrupt_handler   (IDT entry, x86-interrupt fn)
                       └─> increments TIMER_TICKS, sends APIC EOI
                       └─> calls crate::context::schedule()
                                            │
                                            ▼
                                kernel-x86_64/src/context.rs::schedule()
                                  - cli (re-entrancy guard for SYS_YIELD)
                                  - kernel_core::scheduler::pick_next() -> (cur, next)
                                  - fxsave(CONTEXTS[cur])
                                  - write_cr3(next.cr3) if different
                                  - gdt::set_kernel_stack(top of next's per-task kstack)
                                  - fxrstor(next)
                                  - record (cur, next, next.rip) in CTX_LOG ring buffer
                                    (task#40 diagnostic)
                                  - context_switch(old=&CONTEXTS[cur], new=&CONTEXTS[next])
                                            │
                                            ▼
                            kernel-x86_64/src/context.rs::context_switch  (naked)
                                  - save callee-saved regs to *old
                                  - pop retaddr; save as old.rip; save rsp as old.rsp
                                  - load callee-saved from *new
                                  - mov rsp, new.rsp
                                  - popfq with new.rflags
                                  - jmp [new.rip]              # resume next task
```

Per-task storage layout (all sized by `MAX_TASKS = 16`):

| Array | Per-slot size | Where | Purpose |
|-------|---------------|-------|---------|
| `kernel_core::scheduler::TASKS`   | 64 B  | BSS | Platform-independent task metadata |
| `kernel-x86_64::CONTEXTS`         | 72 B  | BSS | Saved registers + cr3 + rsp + rip |
| `kernel-x86_64::FXSAVE_AREAS`     | 512 B | BSS | FPU/SSE state |
| `kernel-x86_64::TASK_STACKS`      | 16 KB | BSS | Primary kernel-mode stack |
| `kernel-x86_64::PER_TASK_KERNEL_STACKS` | 8 KB | BSS | Stack used on Ring 3 → Ring 0 transitions (TSS.RSP0) |
| `kernel-x86_64::ADDRESS_SPACES`   | ~64 B | BSS | Tracked per-process page tables for cleanup |

## Address-space lifecycle

A user task gets a fresh `AddressSpace` (PML4 + subtables) at
spawn time. The PML4 starts as a shallow copy of the boot PML4
(kernel mappings shared via PDPT pointers); the user-space
subtables are fresh.

When the task exits and its slot is later **reused** by
`alloc_task_slot`, the platform's `reap_slot` is called: it walks
`ADDRESS_SPACES` for the dying task's CR3 and frees the PML4 + the
tracked subtables back to the page-table frame pool. **Don't try to
destroy in `kill_current_task`** — empirically this introduces a
race that kills hello.elf reliability. Defer to slot-reuse time.

## Where to look

- The headline security policy: `kernel-core/src/llm/context_builder.rs::build_from_suids` —
  per-tier dispatch into the redactor or summarizer.
- The user-side demo of that policy: `user-programs/sem-demo/src/main.rs`.
- The kernel-side demo: `kernel-x86_64/src/main.rs::sem_demo_one`.
- ELF loading: `kernel-core/src/process/{mod.rs,elf.rs}` +
  `kernel-x86_64/src/platform_impl.rs::{create_address_space, map_elf_segment, map_user_stack, spawn_user_task}`.
- VirtIO disk: `kernel-x86_64/src/virtio/block.rs` (init + virtqueue +
  read/write); `kernel-core/src/storage/snapshot.rs` (the
  block-device-agnostic snapshot wrapper).
