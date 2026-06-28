//! Platform Abstraction Layer
//!
//! Defines the trait that platform crates (kernel-arm64, kernel-x86_64, etc.)
//! must implement. Provides global accessors so kernel-core code can do I/O
//! and get timestamps without knowing which hardware it's running on.

use core::sync::atomic::{AtomicBool, Ordering};

/// Platform trait — implemented once per architecture.
///
/// Registered at boot via [`set_platform`]. All methods are safe because
/// the platform crate is responsible for ensuring hardware access is valid.
pub trait Platform: Send + Sync + 'static {
    /// Write a string to the debug serial console.
    fn serial_write(&self, s: &str);

    /// Get monotonic tick count (platform-defined resolution).
    fn ticks(&self) -> u64;

    /// Halt the CPU (wait for interrupt / low-power idle).
    fn halt(&self);

    /// Read available cooked-mode stdin bytes into `buf` (non-blocking).
    /// Returns the number of bytes written (0 if no complete input is ready).
    /// The platform's TTY line discipline owns the buffering + echo; this is
    /// the drain the SYS_READ(fd=0) path calls. Default: no input source.
    fn stdin_read(&self, _buf: &mut [u8]) -> usize { 0 }

    /// Allocate a 4KB physical frame from the given security tier's pool.
    /// Returns the physical address, or None if the pool is exhausted.
    fn alloc_frame(&self, _tier: u8) -> Option<u64> { None }

    /// Free a 4KB physical frame back to its pool.
    /// Returns true if the frame was successfully freed.
    fn free_frame(&self, _addr: u64) -> bool { false }

    // --- Process address space management ---

    /// Create a new user address space restricted to the given security tier.
    /// Returns an opaque handle (CR3 on x86_64, TTBR0 on ARM64).
    /// The address space inherits kernel higher-half mappings.
    fn create_address_space(&self, _max_tier: u8) -> Option<u64> { None }

    /// Reclaim page-table frames of exited-but-unreaped processes (those whose
    /// address space has no live task). The spawn path calls this when
    /// `create_address_space` fails so a long-lived session that spawns many
    /// short-lived children (e.g. the shell / agent `bash` tool) doesn't run
    /// the PT-frame pool dry. Returns the number of address spaces freed.
    fn reclaim_address_spaces(&self) -> usize { 0 }

    /// Ensure maskable interrupts are enabled. Called at the top of a blocking
    /// network read/write spin so the timer IRQ keeps firing — without it,
    /// `ticks()` freezes and the wall-clock idle-timeout can never fire, so a
    /// slow/stalled peer hangs the kernel forever (the recv-stall bug). A
    /// spinning task-level wait must always allow the timer + preemption.
    /// No-op by default; arch impl sets IF.
    fn enable_interrupts(&self) {}

    /// Run a one-shot LLM query (the shell `ask` builtin / `SYS_ASK`): send
    /// `prompt` to the configured model over the network and write the plain
    /// text answer into `out`, returning its length. Runs synchronously in the
    /// caller's syscall context, so the impl enables interrupts (the network
    /// path's wall-clock timeouts need the timer). Default: unavailable.
    fn llm_ask(&self, _prompt: &[u8], _out: &mut [u8]) -> usize { 0 }

    /// SYS_AGENT: run the interactive split-pane agent terminal (the shell's
    /// `agent` builtin). Blocks in the caller's syscall context, driving a
    /// framebuffer TUI chat loop off the real keyboard until the user exits;
    /// returns 0 on a clean exit, non-zero if it couldn't run (e.g. headless).
    /// Default: unavailable.
    fn run_agent_tui(&self, _flags: u64) -> u64 { 0 }

    /// SYS_EDIT: run the modal text editor over the file named by the user
    /// pointer/len. Blocks in the caller's syscall context driving a framebuffer
    /// editor off the real keyboard until the user quits (`:q`). Returns 0 on a
    /// clean exit, non-zero if it couldn't run. Default: unavailable.
    fn run_editor(&self, _path_ptr: u64, _path_len: u64) -> u64 { 0 }

    /// SYS_USBINFO: print a multi-line summary of every USB port (PORTSC,
    /// PLS, speed, CCS, PED) and every enumerated slot (vendor/product,
    /// class, MUX/MSC/CDC-ECM/HUB state) directly to the current TTY.
    /// Read at the shell prompt to debug enumeration problems without
    /// trying to catch boot scroll. Returns 0. Default: unavailable.
    fn run_usbinfo(&self) -> u64 { 0 }

    /// SYS_USBENUM: re-run xHCI port enumeration. The kernel boot path
    /// only enumerates once during init, so devices plugged in AFTER
    /// boot (the iPhone use case) are not detected. Calling this from
    /// the shell after plug-in retries enumeration with the boot-path's
    /// PR + WPR + descriptor read for every CCS=1 port. Returns the
    /// number of devices that completed enumeration. Default: 0.
    fn run_usbenum(&self) -> u64 { 0 }

    /// SYS_PONG: run the fullscreen pong game until the user hits Esc. Blocks
    /// in the caller's syscall context driving the framebuffer + raw HID
    /// keyboard at ~60 FPS. Returns 0 on a clean exit, 1 if headless.
    /// Default: unavailable.
    fn run_pong(&self) -> u64 { 0 }

    /// SYS_TETRIS: run the fullscreen tetris game until the user hits Esc/Q.
    /// Same shape as `run_pong`. Default: unavailable.
    fn run_tetris(&self) -> u64 { 0 }

    /// SYS_WIFI_SCAN: scan for WiFi networks and print a de-duplicated numbered
    /// list to the TTY (`wifi` shell command). Blocks for the scan duration.
    /// Returns the number of unique networks found. Default: unavailable.
    fn run_wifi_scan(&self) -> u64 { 0 }

    /// SYS_WIFI_CONNECT: connect to scan-list network `idx` using the password
    /// at the user pointer/len (`wifi connect <idx> <password>`). Derives the
    /// WPA2 PMK and runs the association engine. Returns 1 on success, 0 on
    /// failure. Default: unavailable.
    fn run_wifi_connect(&self, _idx: u64, _pass_ptr: u64, _pass_len: u64) -> u64 { 0 }

    /// SYS_TTY_SUPPRESS: set/clear the cooked-mode line-discipline's
    /// input-suppression flag. `on=true` drops keystrokes from feeding
    /// the pend buffer (they're still mirrored to serial). sem-sh sets
    /// this around external commands so keys typed during a child run
    /// don't buffer into the next prompt. Returns 0. Default: no-op.
    fn tty_suppress(&self, _on: bool) -> u64 { 0 }

    /// Reset any console/TTY state that a user process may have left set
    /// (e.g. input suppression or fullscreen flags). Called from SYS_EXIT
    /// so a crashing or exiting process cannot permanently silence the shell.
    /// Default: no-op.
    fn reset_tty_flags(&self) {}

    /// Map a segment of an ELF binary into a user address space.
    ///
    /// - `space`: the address space handle from `create_address_space`
    /// - `virt_addr`: virtual address to map at (page-aligned)
    /// - `data`: segment content to copy
    /// - `memsz`: total memory size (may be > data.len() for BSS)
    /// - `executable`: whether the segment should be executable
    /// - `writable`: whether the segment should be writable
    ///
    /// Returns true on success.
    fn map_elf_segment(
        &self,
        _space: u64,
        _virt_addr: u64,
        _data: &[u8],
        _memsz: usize,
        _executable: bool,
        _writable: bool,
    ) -> bool { false }

    /// Map a user stack in the given address space.
    /// Returns the stack top (highest address), or None on failure.
    /// The stack is mapped as read-write, no-execute.
    fn map_user_stack(&self, _space: u64, _stack_top: u64, _stack_size: u64) -> Option<u64> { None }

    /// Spawn a Ring 3 user-mode task with the given address space.
    /// - `name`: task name for scheduler
    /// - `user_rip`: user-mode entry point (virtual address)
    /// - `user_rsp`: user-mode stack pointer
    /// - `cr3`: address space handle
    /// - `max_tier`: security tier
    ///
    /// Returns the scheduler task slot index.
    fn spawn_user_task(
        &self,
        _name: &'static str,
        _user_rip: u64,
        _user_rsp: u64,
        _cr3: u64,
        _max_tier: u8,
    ) -> Option<usize> { None }

    /// Destroy an address space, freeing all page table frames.
    fn destroy_address_space(&self, _space: u64) {}

    /// Voluntarily yield the CPU to the scheduler.
    /// Used by `SYS_YIELD` so a task can give up its time slice without
    /// waiting for a timer tick. Default is a no-op (busy-wait).
    fn schedule(&self) {}

    /// Reclaim per-task platform resources for an Exited slot before
    /// it is reused. Called by `alloc_task_slot` just before overwriting
    /// the slot's TaskInfo. Default: no-op (platform may not need this).
    /// On x86_64 this destroys the slot's AddressSpace (frees PML4 +
    /// subtable frames) so they don't leak as more demos are spawned.
    fn reap_slot(&self, _slot: usize) {}

    /// Fill `buf` with cryptographically-strong random bytes from the
    /// hardware RNG (RDRAND on x86_64, equivalent elsewhere).
    ///
    /// Returns `Err(())` if no hardware RNG is available — caller must
    /// treat that as fatal for any security-sensitive use. The default
    /// impl fails so accidentally using `NullPlatform` for crypto can't
    /// silently weaken things.
    ///
    /// Used at minimum for: TLS 1.3 ClientHello.random (32 bytes per
    /// connection), X25519 ephemeral scalar (32 bytes per handshake),
    /// smoltcp's TCP ISN / DNS txid seeds.
    fn random_bytes(&self, _buf: &mut [u8]) -> Result<(), ()> { Err(()) }

    /// Write argv + envp onto a newly-mapped user stack at the SysV
    /// AMD64 ABI positions, returning the adjusted initial RSP.
    ///
    /// Layout written, starting at `stack_top` and growing down:
    /// ```text
    ///   [string data: argv strings + envp strings, null-terminated]
    ///   [NULL]              (envp terminator)
    ///   [envp[n-1] ptr]
    ///   ...
    ///   [envp[0] ptr]
    ///   [NULL]              (argv terminator)
    ///   [argv[argc-1] ptr]
    ///   ...
    ///   [argv[0] ptr]
    ///   [argc]              ← new RSP, returned
    /// ```
    ///
    /// Each pointer is a u64 in the user process's virtual address
    /// space, pointing into the string-data region just above. The
    /// new RSP is 16-byte aligned per ABI.
    ///
    /// Default impl returns `Some(stack_top)` so platforms without
    /// argv-write support behave as if argv was empty (existing
    /// `spawn_from_elf` callers that pass empty argv/envp still work).
    ///
    /// Phase 14 prereq #2 — `std::env::args()` and the
    /// cargo→rustc handoff depend on this.
    fn setup_user_argv(
        &self,
        _space: u64,
        stack_top: u64,
        _argv: &[&[u8]],
        _envp: &[&[u8]],
    ) -> Option<u64> { Some(stack_top) }

    /// Read the active CR3 (page-table root) of the currently-running
    /// task. Used by SYS_THREAD_SPAWN to identify the parent's
    /// address space — the new thread is mapped into the same one.
    /// Returns 0 to mean "kernel boot page tables" (kernel-mode tasks).
    fn current_cr3(&self) -> u64 { 0 }

    /// Map `size` bytes (rounded up to pages) of fresh, zeroed,
    /// USER-accessible memory into address space `cr3` at virtual
    /// `addr`. Returns true on success.
    ///
    /// Backs SYS_MMAP_ANON → the semos-std user-space heap allocator
    /// (M25 Tier 2 #50). Pages are ReadWrite with the user bit set.
    /// Default returns false so platforms without an MMU reject it.
    fn map_user_region(&self, _cr3: u64, _addr: u64, _size: u64) -> bool { false }

    /// Phase 14 Tier 3 (#45) — Spawn a Ring-3 sibling task in an
    /// EXISTING address space.
    ///
    /// Unlike `spawn_user_task`, which assumes the caller already
    /// allocated `cr3` and mapped the user code + stack, `spawn_thread`
    /// owns the user-stack mapping: it picks a fresh virtual stack
    /// region inside `cr3`, maps it from the new scheduler slot's
    /// physical backing, builds a Ring-3 context that starts at
    /// `entry_va` with `arg` in rdi, and marks the slot ready.
    ///
    /// `entry_va` must already be mapped executable in `cr3` (typically
    /// it's a function in the parent's user image — both threads share
    /// the same AS, so the parent's text is visible).
    ///
    /// Returns the scheduler slot index, or None on OOM / mapping
    /// failure / no free slots.
    ///
    /// Default returns None — platforms without same-AS thread support
    /// reject SYS_THREAD_SPAWN cleanly.
    fn spawn_thread(
        &self,
        _name: &'static str,
        _cr3: u64,
        _entry_va: u64,
        _arg: u64,
        _max_tier: u8,
    ) -> Option<usize> { None }

    /// Read absolute wall-clock time as seconds since the Unix epoch
    /// (1970-01-01 00:00:00 UTC). `None` if the platform has no
    /// real-time clock, or if the RTC read failed.
    ///
    /// Unlike `ticks()` (monotonic since boot, platform-defined
    /// resolution), this returns absolute UTC time. Use it for:
    /// - TLS `notAfter` validation (Phase 9 follow-up)
    /// - File timestamps on Semantic Objects (`created_at`, `modified_at`)
    /// - User-facing date/time displays in the Marée / Brise utilities
    ///
    /// Default returns `None` so a platform without an RTC (or a buggy
    /// one) can't silently feed garbage timestamps into security-
    /// sensitive code paths.
    fn wall_clock(&self) -> Option<u64> { None }
}

/// Null platform used before a real one is registered.
struct NullPlatform;

impl Platform for NullPlatform {
    fn serial_write(&self, _s: &str) {}
    fn ticks(&self) -> u64 { 0 }
    fn halt(&self) {}
}

static NULL_PLATFORM: NullPlatform = NullPlatform;

/// Global platform reference. Points to NullPlatform until set_platform() is called.
static mut PLATFORM: &dyn Platform = &NULL_PLATFORM;
static PLATFORM_SET: AtomicBool = AtomicBool::new(false);

/// Register the platform implementation. Call once at boot.
///
/// # Safety
/// Must be called before any other kernel-core code runs, and only once.
/// The reference must be valid for the lifetime of the kernel ('static).
pub unsafe fn set_platform(p: &'static dyn Platform) {
    PLATFORM = p;
    PLATFORM_SET.store(true, Ordering::Release);
}

/// Get a reference to the current platform.
#[inline]
pub fn get() -> &'static dyn Platform {
    // Safety: PLATFORM is set once at boot and never changes after.
    unsafe { PLATFORM }
}

/// Write a log message to the platform's serial console.
#[inline]
pub fn log(s: &str) {
    get().serial_write(s);
}

/// Get the current tick count from the platform timer.
#[inline]
pub fn ticks() -> u64 {
    get().ticks()
}

/// Drain available cooked-mode stdin bytes (non-blocking). See
/// [`Platform::stdin_read`]. Returns bytes written into `buf`.
#[inline]
pub fn stdin_read(buf: &mut [u8]) -> usize {
    get().stdin_read(buf)
}

/// Voluntarily yield the CPU to the scheduler.
#[inline]
pub fn schedule() {
    get().schedule();
}

/// Fill `buf` with cryptographically-strong random bytes from the
/// hardware RNG. Returns `Err(())` if no RNG is available — security-
/// sensitive callers must treat this as fatal.
#[inline]
pub fn random_bytes(buf: &mut [u8]) -> Result<(), ()> {
    get().random_bytes(buf)
}

/// Absolute wall-clock time in seconds since Unix epoch. `None` if
/// no RTC is present or the read failed. See [`Platform::wall_clock`].
#[inline]
pub fn wall_clock() -> Option<u64> {
    get().wall_clock()
}

/// Log a number in decimal.
pub fn log_num(n: u64) {
    if n == 0 {
        log("0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    let mut val = n;
    while val > 0 {
        buf[i] = b'0' + (val % 10) as u8;
        val /= 10;
        i += 1;
    }
    // Reverse
    let mut j = 0;
    while j < i / 2 {
        buf.swap(j, i - 1 - j);
        j += 1;
    }
    // Safety: buf contains only ASCII digits
    if let Ok(s) = core::str::from_utf8(&buf[..i]) {
        log(s);
    }
}

/// Log a byte as two hex characters.
pub fn log_hex_byte(b: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let chars = [HEX[(b >> 4) as usize], HEX[(b & 0xF) as usize]];
    if let Ok(s) = core::str::from_utf8(&chars) {
        log(s);
    }
}
