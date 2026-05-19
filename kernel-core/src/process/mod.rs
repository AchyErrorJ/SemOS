//! Process Management Module
//!
//! Provides full process abstraction with:
//! - Process Control Block (PCB)
//! - Process lifecycle management (spawn, exit, wait)
//! - Parent-child relationships
//! - Per-process file descriptors
//! - Integration with scheduler tasks
//!
//! # Architecture
//!
//! ```text
//! Process (PCB)
//!   ├── pid: ProcessId
//!   ├── parent: Option<ProcessId>
//!   ├── children: Vec<ProcessId>
//!   ├── task_id: TaskId (scheduler task)
//!   ├── capabilities: CapabilitySet
//!   ├── file_descriptors: FdTable
//!   ├── address_space: AddressSpace
//!   └── state: ProcessState
//! ```

pub mod capability;
pub mod elf;

use core::sync::atomic::{AtomicU32, Ordering};
use crate::memory::SecurityTier;

/// Task ID type alias (matches scheduler)
pub type TaskId = usize;

pub use capability::{Capability, CapabilitySet};

/// Process ID type
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessId(pub u32);

impl ProcessId {
    pub const KERNEL: ProcessId = ProcessId(0);
    pub const INIT: ProcessId = ProcessId(1);

    pub fn as_u32(self) -> u32 {
        self.0
    }
}

/// Process state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Process is being created
    Creating,
    /// Process is ready/running
    Running,
    /// Process is waiting for a child
    Waiting,
    /// Process is stopped (e.g., by signal)
    Stopped,
    /// Process has exited but not yet reaped (zombie)
    Zombie,
    /// Process has been fully cleaned up
    Dead,
}

/// Exit status of a process
#[derive(Debug, Clone, Copy)]
pub struct ExitStatus {
    /// Exit code (0 = success)
    pub code: i32,
    /// Was the process killed by a signal?
    pub signaled: bool,
    /// Signal number if signaled
    pub signal: u8,
}

impl ExitStatus {
    pub const fn success() -> Self {
        Self { code: 0, signaled: false, signal: 0 }
    }

    pub const fn failure(code: i32) -> Self {
        Self { code, signaled: false, signal: 0 }
    }

    pub const fn killed(signal: u8) -> Self {
        Self { code: 128 + signal as i32, signaled: true, signal }
    }
}

/// File descriptor entry
#[derive(Debug, Clone, Copy)]
pub enum FdEntry {
    /// Empty slot
    Empty,
    /// Serial console (stdin/stdout/stderr)
    Console,
    /// File in the filesystem
    File {
        /// Path or inode reference
        inode: u32,
        /// Current position
        position: usize,
        /// Flags (read, write, append)
        flags: u32,
    },
    /// Pipe endpoint
    Pipe {
        /// Pipe ID
        pipe_id: u32,
        /// Read or write end
        is_read_end: bool,
    },
}

/// File descriptor flags
pub mod fd_flags {
    pub const O_RDONLY: u32 = 0;
    pub const O_WRONLY: u32 = 1;
    pub const O_RDWR: u32 = 2;
    pub const O_APPEND: u32 = 0x400;
    pub const O_CREAT: u32 = 0x40;
    pub const O_TRUNC: u32 = 0x200;
}

/// Maximum file descriptors per process
pub const MAX_FDS: usize = 64;

/// File descriptor table
pub struct FdTable {
    entries: [FdEntry; MAX_FDS],
    next_fd: usize,
}

impl FdTable {
    pub const fn new() -> Self {
        const EMPTY: FdEntry = FdEntry::Empty;
        Self {
            entries: [EMPTY; MAX_FDS],
            next_fd: 0,
        }
    }

    /// Create a standard FD table with stdin/stdout/stderr
    pub fn with_stdio() -> Self {
        let mut table = Self::new();
        table.entries[0] = FdEntry::Console; // stdin
        table.entries[1] = FdEntry::Console; // stdout
        table.entries[2] = FdEntry::Console; // stderr
        table.next_fd = 3;
        table
    }

    /// Allocate a new file descriptor
    pub fn alloc(&mut self, entry: FdEntry) -> Option<i32> {
        for i in self.next_fd..MAX_FDS {
            if matches!(self.entries[i], FdEntry::Empty) {
                self.entries[i] = entry;
                self.next_fd = i + 1;
                return Some(i as i32);
            }
        }
        // Try from beginning
        for i in 0..self.next_fd {
            if matches!(self.entries[i], FdEntry::Empty) {
                self.entries[i] = entry;
                return Some(i as i32);
            }
        }
        None
    }

    /// Get a file descriptor entry
    pub fn get(&self, fd: i32) -> Option<&FdEntry> {
        if fd >= 0 && (fd as usize) < MAX_FDS {
            let entry = &self.entries[fd as usize];
            if !matches!(entry, FdEntry::Empty) {
                return Some(entry);
            }
        }
        None
    }

    /// Get a mutable file descriptor entry
    pub fn get_mut(&mut self, fd: i32) -> Option<&mut FdEntry> {
        if fd >= 0 && (fd as usize) < MAX_FDS {
            let entry = &mut self.entries[fd as usize];
            if !matches!(entry, FdEntry::Empty) {
                return Some(entry);
            }
        }
        None
    }

    /// Close a file descriptor
    pub fn close(&mut self, fd: i32) -> bool {
        if fd >= 0 && (fd as usize) < MAX_FDS {
            if !matches!(self.entries[fd as usize], FdEntry::Empty) {
                self.entries[fd as usize] = FdEntry::Empty;
                if (fd as usize) < self.next_fd {
                    self.next_fd = fd as usize;
                }
                return true;
            }
        }
        false
    }

    /// Duplicate a file descriptor
    pub fn dup(&mut self, old_fd: i32) -> Option<i32> {
        if let Some(entry) = self.get(old_fd) {
            let entry = *entry;
            self.alloc(entry)
        } else {
            None
        }
    }

    /// Duplicate a file descriptor to a specific number
    pub fn dup2(&mut self, old_fd: i32, new_fd: i32) -> Option<i32> {
        if new_fd < 0 || (new_fd as usize) >= MAX_FDS {
            return None;
        }
        if let Some(entry) = self.get(old_fd) {
            let entry = *entry;
            self.entries[new_fd as usize] = entry;
            Some(new_fd)
        } else {
            None
        }
    }
}

/// Maximum children per process
pub const MAX_CHILDREN: usize = 32;

/// Maximum total bytes per process environment block. Holds packed
/// `KEY=VALUE\0KEY=VALUE\0…` entries — see [`Process::env`].
///
/// **Capped at 512 B** (= 32 KiB total BSS across MAX_PROCESSES=64).
/// Task #36's TASK_STACK_SIZE fix (16 KiB → 64 KiB) unblocked USB but
/// the BSS budget still has a tight ceiling somewhere between 512 B
/// and 1024 B × MAX_PROCESSES (16-bit BIOS bootloader region appears
/// to overlap kernel BSS past a certain point — hangs at IDT init
/// when crossed). The underlying bootloader memory-layout issue is
/// a separate follow-up; for now 512 B is enough for typical
/// command-line workflows (~8-10 env vars at ~50 bytes each).
pub const ENV_BLOCK_SIZE: usize = 512;

/// Process Control Block
pub struct Process {
    /// Process ID
    pub pid: ProcessId,
    /// Parent process ID (None for init)
    pub parent: Option<ProcessId>,
    /// Child process IDs
    pub children: [Option<ProcessId>; MAX_CHILDREN],
    /// Number of children
    pub child_count: usize,
    /// Associated scheduler task
    pub task_id: Option<TaskId>,
    /// Process state
    pub state: ProcessState,
    /// Exit status (valid when Zombie)
    pub exit_status: ExitStatus,
    /// Process capabilities
    pub capabilities: CapabilitySet,
    // Note: max security tier is not stored on Process. It lives on the
    // associated scheduler task (`TaskInfo.max_tier`) and is read via
    // `scheduler::current_task_max_tier()`. Storing it on both would mean
    // two sources of truth — see also the user_id note above.
    /// File descriptor table
    pub fds: FdTable,
    /// Process name (for debugging)
    pub name: [u8; 32],
    /// Name length
    pub name_len: usize,
    /// Working directory path. Always absolute (starts with `/`).
    /// Inherited by child processes on spawn. Default: `/`.
    pub cwd: [u8; 128],
    /// CWD length
    pub cwd_len: usize,
    /// Process environment block. Packed `KEY=VALUE\0KEY=VALUE\0…`
    /// (the trailing `\0` separates entries, no second `\0` for end).
    /// Inherited by child processes on spawn. Phase 14 prereq #3 —
    /// std::env::var / std::env::vars reach in here via SYS_GET_ENV / SYS_SET_ENV.
    pub env: [u8; ENV_BLOCK_SIZE],
    /// Bytes used in `env`.
    pub env_len: usize,
    // Note: user/group identity is not stored on Process. The authoritative
    // identity lives on the associated scheduler task (`TaskInfo.user_id`)
    // and is read via `scheduler::current_user_id()`. Two parallel fields
    // would only mean two things to keep in sync; the user registry lives
    // in `security::users`.
    /// Entry point address (for exec)
    pub entry_point: usize,
    /// Stack pointer
    pub stack_ptr: usize,
    /// Heap break address
    pub brk: usize,
}

impl Process {
    /// Create a new process
    pub fn new(pid: ProcessId, parent: Option<ProcessId>, name: &str) -> Self {
        let mut proc = Self {
            pid,
            parent,
            children: [None; MAX_CHILDREN],
            child_count: 0,
            task_id: None,
            state: ProcessState::Creating,
            exit_status: ExitStatus::success(),
            capabilities: CapabilitySet::empty(),
            fds: FdTable::with_stdio(),
            name: [0u8; 32],
            name_len: 0,
            cwd: [0u8; 128],
            cwd_len: 1,
            env: [0u8; ENV_BLOCK_SIZE],
            env_len: 0,
            entry_point: 0,
            stack_ptr: 0,
            brk: 0,
        };

        // Set name
        let name_bytes = name.as_bytes();
        let copy_len = name_bytes.len().min(31);
        proc.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
        proc.name_len = copy_len;

        // Set default CWD to "/"
        proc.cwd[0] = b'/';

        proc
    }

    /// Create the kernel process (PID 0).
    ///
    /// Tier isn't set here — the kernel's effective max_tier lives on
    /// scheduler task slot 0 (`TaskInfo.max_tier = Secret`), established
    /// in `scheduler::init_core`. The Process PCB carries only the bits
    /// the scheduler doesn't already know about.
    pub fn kernel() -> Self {
        let mut proc = Self::new(ProcessId::KERNEL, None, "kernel");
        proc.capabilities = CapabilitySet::all();
        proc.state = ProcessState::Running;
        proc
    }

    /// Create the init process (PID 1).
    ///
    /// Init is a placeholder PCB today — it has no associated scheduler
    /// task, so its tier would only ever be a write-only field. When init
    /// gets a real task slot, set the tier on the TaskInfo at spawn time
    /// rather than here.
    pub fn init() -> Self {
        let mut proc = Self::new(ProcessId::INIT, Some(ProcessId::KERNEL), "init");
        proc.capabilities = CapabilitySet::default_user();
        proc
    }

    /// Get process name as string
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("???")
    }

    /// Get CWD as string
    pub fn cwd(&self) -> &str {
        core::str::from_utf8(&self.cwd[..self.cwd_len]).unwrap_or("/")
    }

    /// Add a child process
    pub fn add_child(&mut self, child_pid: ProcessId) -> bool {
        for slot in &mut self.children {
            if slot.is_none() {
                *slot = Some(child_pid);
                self.child_count += 1;
                return true;
            }
        }
        false
    }

    /// Remove a child process
    pub fn remove_child(&mut self, child_pid: ProcessId) -> bool {
        for slot in &mut self.children {
            if *slot == Some(child_pid) {
                *slot = None;
                self.child_count -= 1;
                return true;
            }
        }
        false
    }

    /// Check if this process has a specific capability
    pub fn has_capability(&self, cap: Capability) -> bool {
        self.capabilities.has(cap)
    }

    // Note: removed `can_access_tier(tier)`. The scheduler-task tier
    // (`current_task_max_tier()`) is the authoritative check; using a
    // PCB-side mirror was the only reason `Process` still tracked tier.

    // --- Env + CWD accessors (Phase 14 prereq #3) ---

    /// Current CWD as a string slice. Always valid UTF-8 (we only
    /// write valid paths in via [`set_cwd`]).
    pub fn cwd_str(&self) -> &str {
        // Safety: cwd is only written via set_cwd which validates UTF-8.
        unsafe { core::str::from_utf8_unchecked(&self.cwd[..self.cwd_len]) }
    }

    /// Replace CWD. Validates that `path` is absolute (starts with `/`)
    /// and fits the fixed 128-byte buffer. Returns true on success.
    pub fn set_cwd(&mut self, path: &str) -> bool {
        let bytes = path.as_bytes();
        if !path.starts_with('/') || bytes.len() > self.cwd.len() {
            return false;
        }
        self.cwd[..bytes.len()].copy_from_slice(bytes);
        self.cwd_len = bytes.len();
        true
    }

    /// Look up a single env var by key. Returns the value bytes, or None
    /// if the key isn't present. Walks the packed `KEY=VALUE\0…` block.
    pub fn env_get(&self, key: &str) -> Option<&[u8]> {
        let key_bytes = key.as_bytes();
        let mut start = 0usize;
        while start < self.env_len {
            // Find the end of this entry (next \0 or end of block).
            let mut end = start;
            while end < self.env_len && self.env[end] != 0 { end += 1; }
            let entry = &self.env[start..end];
            // Find the '=' within the entry.
            if let Some(eq) = entry.iter().position(|&b| b == b'=') {
                if &entry[..eq] == key_bytes {
                    return Some(&entry[eq + 1..]);
                }
            }
            start = end + 1;
        }
        None
    }

    /// Set or update an env var. If the key exists, replaces the value
    /// (entire entry rewritten in place is too brittle — we delete the
    /// old entry and append the new one). Returns true on success,
    /// false if the env block would overflow.
    pub fn env_set(&mut self, key: &str, value: &str) -> bool {
        // Step 1: remove any existing entry with this key.
        let key_bytes = key.as_bytes();
        let mut start = 0usize;
        while start < self.env_len {
            let mut end = start;
            while end < self.env_len && self.env[end] != 0 { end += 1; }
            let entry = &self.env[start..end];
            let matches = entry.iter().position(|&b| b == b'=')
                .map(|eq| &entry[..eq] == key_bytes)
                .unwrap_or(false);
            if matches {
                // Shift the tail down by (end - start + 1) bytes
                // (+1 to also skip the entry's trailing \0, if any).
                let consumed = if end < self.env_len { end - start + 1 } else { end - start };
                let tail_src_start = start + consumed;
                let tail_len = self.env_len.saturating_sub(tail_src_start);
                self.env.copy_within(tail_src_start..tail_src_start + tail_len, start);
                self.env_len -= consumed;
                break;
            }
            start = end + 1;
        }

        // Step 2: append the new entry.
        let needed = key_bytes.len() + 1 + value.len() + 1; // KEY=VALUE\0
        if self.env_len + needed > self.env.len() { return false; }
        let mut p = self.env_len;
        self.env[p..p + key_bytes.len()].copy_from_slice(key_bytes);
        p += key_bytes.len();
        self.env[p] = b'=';
        p += 1;
        self.env[p..p + value.len()].copy_from_slice(value.as_bytes());
        p += value.len();
        self.env[p] = 0;
        p += 1;
        self.env_len = p;
        true
    }

    /// Inherit env + CWD from another process (used on spawn). Copies
    /// the raw bytes — no validation, parent is assumed to have valid
    /// state.
    pub fn inherit_env_cwd_from(&mut self, parent: &Process) {
        self.cwd[..parent.cwd_len].copy_from_slice(&parent.cwd[..parent.cwd_len]);
        self.cwd_len = parent.cwd_len;
        self.env[..parent.env_len].copy_from_slice(&parent.env[..parent.env_len]);
        self.env_len = parent.env_len;
    }

    /// Mark process as zombie with exit status
    pub fn exit(&mut self, status: ExitStatus) {
        self.exit_status = status;
        self.state = ProcessState::Zombie;
    }

    /// Reap a zombie process (called by parent)
    pub fn reap(&mut self) -> ExitStatus {
        let status = self.exit_status;
        self.state = ProcessState::Dead;
        status
    }
}

/// Maximum number of processes
pub const MAX_PROCESSES: usize = 64;

/// Process table
pub struct ProcessTable {
    processes: [Option<Process>; MAX_PROCESSES],
    count: usize,
}

impl ProcessTable {
    pub const fn new() -> Self {
        const NONE: Option<Process> = None;
        Self {
            processes: [NONE; MAX_PROCESSES],
            count: 0,
        }
    }

    /// Get a process by PID
    pub fn get(&self, pid: ProcessId) -> Option<&Process> {
        let idx = pid.0 as usize;
        if idx < MAX_PROCESSES {
            self.processes[idx].as_ref()
        } else {
            None
        }
    }

    /// Get a mutable process by PID
    pub fn get_mut(&mut self, pid: ProcessId) -> Option<&mut Process> {
        let idx = pid.0 as usize;
        if idx < MAX_PROCESSES {
            self.processes[idx].as_mut()
        } else {
            None
        }
    }

    /// Insert a process at a specific PID
    pub fn insert(&mut self, proc: Process) -> bool {
        let idx = proc.pid.0 as usize;
        if idx < MAX_PROCESSES && self.processes[idx].is_none() {
            self.processes[idx] = Some(proc);
            self.count += 1;
            true
        } else {
            false
        }
    }

    /// Remove a process
    pub fn remove(&mut self, pid: ProcessId) -> Option<Process> {
        let idx = pid.0 as usize;
        if idx < MAX_PROCESSES {
            if let Some(proc) = self.processes[idx].take() {
                self.count -= 1;
                return Some(proc);
            }
        }
        None
    }

    /// Allocate a new PID
    pub fn alloc_pid(&self) -> Option<ProcessId> {
        // Start from PID 2 (0=kernel, 1=init)
        for i in 2..MAX_PROCESSES {
            if self.processes[i].is_none() {
                return Some(ProcessId(i as u32));
            }
        }
        None
    }

    /// Get number of active processes
    pub fn count(&self) -> usize {
        self.count
    }

    /// Iterate over all processes
    pub fn iter(&self) -> impl Iterator<Item = &Process> {
        self.processes.iter().filter_map(|p| p.as_ref())
    }

    /// Iterate mutably over all processes
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Process> {
        self.processes.iter_mut().filter_map(|p| p.as_mut())
    }

    /// Find the process whose `task_id` matches `slot`. Used by
    /// syscall handlers that know the scheduler slot (via
    /// `current_task_index()`) but can't rely on `current_pid()`
    /// (which doesn't update on context switch). Returns the PID.
    pub fn find_pid_by_task(&self, slot: usize) -> Option<ProcessId> {
        for p in self.iter() {
            if p.task_id == Some(slot) {
                return Some(p.pid);
            }
        }
        None
    }
}

/// Walk the process table looking up the PID owning scheduler slot `slot`.
/// Bridges `scheduler::current_task_index()` to a ProcessId without
/// going through the unreliable `current_pid()` global.
pub fn pid_for_slot(slot: usize) -> Option<ProcessId> {
    unsafe { PROCESS_TABLE.find_pid_by_task(slot) }
}

/// Global process table
static mut PROCESS_TABLE: ProcessTable = ProcessTable::new();

/// Next PID counter
static NEXT_PID: AtomicU32 = AtomicU32::new(2);

/// Current process PID (per-CPU, but we're single CPU for now)
static CURRENT_PID: AtomicU32 = AtomicU32::new(0);

/// Initialize the process subsystem
pub fn init() {
    unsafe {
        // Create kernel process (PID 0)
        let kernel_proc = Process::kernel();
        PROCESS_TABLE.insert(kernel_proc);

        // Create init process (PID 1)
        let init_proc = Process::init();
        PROCESS_TABLE.insert(init_proc);

        CURRENT_PID.store(0, Ordering::Release);
    }

    crate::platform::log("  [process] Process management initialized\n");
}

/// Get current process ID
pub fn current_pid() -> ProcessId {
    ProcessId(CURRENT_PID.load(Ordering::Acquire))
}

/// Set current process ID
pub fn set_current_pid(pid: ProcessId) {
    CURRENT_PID.store(pid.0, Ordering::Release);
}

/// Get current process
pub fn current() -> Option<&'static Process> {
    unsafe { PROCESS_TABLE.get(current_pid()) }
}

/// Get current process mutably
pub fn current_mut() -> Option<&'static mut Process> {
    unsafe { PROCESS_TABLE.get_mut(current_pid()) }
}

/// Get a process by PID
pub fn get(pid: ProcessId) -> Option<&'static Process> {
    unsafe { PROCESS_TABLE.get(pid) }
}

/// Get a process mutably by PID
pub fn get_mut(pid: ProcessId) -> Option<&'static mut Process> {
    unsafe { PROCESS_TABLE.get_mut(pid) }
}

/// Spawn a new kernel-mode process
/// Note: name must be static for scheduler compatibility
pub fn spawn(name: &'static str, entry: fn()) -> Option<ProcessId> {
    let parent_pid = current_pid();

    unsafe {
        // Allocate PID
        let pid = PROCESS_TABLE.alloc_pid()?;

        // Create process
        let mut proc = Process::new(pid, Some(parent_pid), name);

        // Inherit capabilities from parent (reduced). The tier is read off
        // the current scheduler task rather than the parent PCB: spawn()
        // runs in the parent's task context, so `current_task_max_tier()`
        // is the same value `parent.max_tier` used to be, and there's no
        // PCB-side mirror to drift.
        if let Some(parent) = PROCESS_TABLE.get(parent_pid) {
            proc.capabilities = parent.capabilities.inherit();
        }
        let inherited_tier = crate::scheduler::current_task_max_tier();

        // Allocate a scheduler task slot
        // NOTE: The platform crate must set up the actual stack and context
        // for this task slot after this function returns.
        let task_id = crate::scheduler::alloc_task_slot(
            name,
            inherited_tier,
            true, // kernel mode
        )?;
        proc.task_id = Some(task_id);
        proc.state = ProcessState::Running;

        // Add to process table
        PROCESS_TABLE.insert(proc);

        // Add to parent's children
        if let Some(parent) = PROCESS_TABLE.get_mut(parent_pid) {
            parent.add_child(pid);
        }

        Some(pid)
    }
}

/// Spawn a user-mode process
/// Note: name must be static for scheduler compatibility
pub fn spawn_user(name: &'static str, entry: fn()) -> Option<ProcessId> {
    let parent_pid = current_pid();

    unsafe {
        // Allocate PID
        let pid = PROCESS_TABLE.alloc_pid()?;

        // Create process
        let mut proc = Process::new(pid, Some(parent_pid), name);

        // User processes get limited capabilities + the lowest tier.
        proc.capabilities = CapabilitySet::minimal();
        let new_tier = SecurityTier::Public;

        // Allocate a scheduler task slot
        // NOTE: The platform crate must set up user-mode stack, context,
        // and address space for this task slot.
        let task_id = crate::scheduler::alloc_task_slot(
            name,
            new_tier as u8,
            false, // user mode
        )?;
        proc.task_id = Some(task_id);
        proc.state = ProcessState::Running;

        // Add to process table
        PROCESS_TABLE.insert(proc);

        // Add to parent's children
        if let Some(parent) = PROCESS_TABLE.get_mut(parent_pid) {
            parent.add_child(pid);
        }

        Some(pid)
    }
}

/// Spawn a user-mode process from ELF data.
///
/// Orchestrates: parse ELF → create address space → map segments → map stack
/// → spawn Ring 3 task. All hardware operations go through the Platform trait.
/// Spawn a Ring 3 task from an ELF binary, no argv/envp. Equivalent
/// to `spawn_from_elf_with_args(name, elf_data, max_tier, &[], &[])`.
/// Existing call sites use this; new ones that want argv should use
/// the `_with_args` variant.
pub fn spawn_from_elf(name: &'static str, elf_data: &[u8], max_tier: u8) -> Option<ProcessId> {
    spawn_from_elf_with_args(name, elf_data, max_tier, &[], &[])
}

/// Spawn a Ring 3 task from an ELF, writing `argv` + `envp` onto the
/// new process's user stack at SysV positions per
/// [`crate::platform::Platform::setup_user_argv`]. Phase 14 prereq #2.
pub fn spawn_from_elf_with_args(
    name: &'static str,
    elf_data: &[u8],
    max_tier: u8,
    argv: &[&[u8]],
    envp: &[&[u8]],
) -> Option<ProcessId> {
    use crate::process::elf::{self, Elf64Phdr, PT_LOAD, PF_X, PF_W};

    // 1. Parse and validate the ELF binary
    let elf_info = elf::load_elf(elf_data)?;
    let platform = crate::platform::get();

    // (silenced — was "[process] Loading ELF: entry=... segments=..." debug log)

    // 2. Create a new address space restricted to max_tier
    let cr3 = platform.create_address_space(max_tier)?;

    // 3. Map each PT_LOAD segment into the address space
    let header = unsafe { &*(elf_data.as_ptr() as *const elf::Elf64Header) };
    for i in 0..header.e_phnum {
        let ph_offset = header.e_phoff as usize + (i as usize) * (header.e_phentsize as usize);
        if ph_offset + core::mem::size_of::<Elf64Phdr>() > elf_data.len() {
            platform.destroy_address_space(cr3);
            return None;
        }
        let phdr = unsafe { &*(elf_data.as_ptr().add(ph_offset) as *const Elf64Phdr) };
        if phdr.p_type != PT_LOAD {
            continue;
        }

        let vaddr = (phdr.p_vaddr as usize).wrapping_add(elf_info.base);
        let filesz = phdr.p_filesz as usize;
        let memsz = phdr.p_memsz as usize;
        let offset = phdr.p_offset as usize;
        let executable = phdr.p_flags & PF_X != 0;
        let writable = phdr.p_flags & PF_W != 0;

        // Extract file data for this segment
        let seg_data = if filesz > 0 && offset + filesz <= elf_data.len() {
            &elf_data[offset..offset + filesz]
        } else {
            &[]
        };

        if !platform.map_elf_segment(cr3, vaddr as u64, seg_data, memsz, executable, writable) {
            crate::platform::log("[process] Failed to map ELF segment\n");
            platform.destroy_address_space(cr3);
            return None;
        }
    }

    // 4. Map a user stack
    let stack_top = elf_info.stack_top as u64;
    // User stack size — must NOT exceed crate::scheduler::TASK_STACK_SIZE
    // on the platform crate, because we reuse the TASK_STACKS slot as
    // physical backing. Currently 64 KiB (was 16 KiB before task #36's
    // fix); larger sizes would alias adjacent slots' TASK_STACKS and
    // corrupt their iret-RIP slot at [top-56] (original task #40 family).
    // Long-term: allocate from a separate pool, not from TASK_STACKS.
    let stack_size = crate::scheduler::TASK_STACK_SIZE as u64;
    let user_rsp = match platform.map_user_stack(cr3, stack_top, stack_size) {
        Some(rsp) => rsp,
        None => {
            crate::platform::log("[process] Failed to map user stack\n");
            platform.destroy_address_space(cr3);
            return None;
        }
    };

    // 4b. Phase 14 prereq #2: if argv or envp are non-empty, write
    // them onto the top of the new user stack at SysV positions.
    // The returned RSP is below the layout, so the user-side _start
    // (when written) can do `pop argc; pop argv; pop envp` style.
    // If argv+envp are empty (legacy callers), this is a no-op and
    // returns stack_top unchanged.
    let user_rsp = match platform.setup_user_argv(cr3, user_rsp, argv, envp) {
        Some(rsp) => rsp,
        None => {
            crate::platform::log("[process] Failed to set up user argv/envp\n");
            platform.destroy_address_space(cr3);
            return None;
        }
    };

    // 5. Spawn the Ring 3 task
    let user_rip = elf_info.entry as u64;
    let task_slot = match platform.spawn_user_task(name, user_rip, user_rsp, cr3, max_tier) {
        Some(slot) => slot,
        None => {
            crate::platform::log("[process] Failed to spawn user task\n");
            platform.destroy_address_space(cr3);
            return None;
        }
    };

    // 6. Create process in the process table
    let parent_pid = current_pid();
    unsafe {
        let pid = PROCESS_TABLE.alloc_pid()?;
        let mut proc = Process::new(pid, Some(parent_pid), name);
        proc.capabilities = CapabilitySet::minimal();
        // Inherit env + CWD from parent (Phase 14 prereq #3). Default
        // is empty env + cwd="/"; if the parent has set anything, the
        // child gets a copy. Standard POSIX semantics.
        if let Some(parent) = PROCESS_TABLE.get(parent_pid) {
            // Clone the parent's slice into a stack snapshot so we don't
            // hold a borrow while we mutate `proc` (same struct, same table).
            let parent_snap = (parent.cwd, parent.cwd_len, parent.env, parent.env_len);
            proc.cwd = parent_snap.0;
            proc.cwd_len = parent_snap.1;
            proc.env = parent_snap.2;
            proc.env_len = parent_snap.3;
        }
        // The tier was already applied to the scheduler task by
        // `platform.spawn_user_task(...max_tier)` above; the PCB used to
        // mirror it, but that was the redundancy we just removed.
        proc.task_id = Some(task_slot);
        proc.state = ProcessState::Running;
        proc.entry_point = elf_info.entry;
        proc.stack_ptr = user_rsp as usize;
        proc.brk = elf_info.brk;

        PROCESS_TABLE.insert(proc);

        if let Some(parent) = PROCESS_TABLE.get_mut(parent_pid) {
            parent.add_child(pid);
        }

        // (silenced — was "[process] Spawned ELF process PID=...")
        let _ = task_slot;

        Some(pid)
    }
}

/// Exit the current process
pub fn exit(code: i32) -> ! {
    let pid = current_pid();

    unsafe {
        if let Some(proc) = PROCESS_TABLE.get_mut(pid) {
            proc.exit(ExitStatus::failure(code));

            // Reparent children to init
            for child_pid in proc.children.iter().filter_map(|c| *c) {
                if let Some(child) = PROCESS_TABLE.get_mut(child_pid) {
                    child.parent = Some(ProcessId::INIT);
                }
                if let Some(init) = PROCESS_TABLE.get_mut(ProcessId::INIT) {
                    init.add_child(child_pid);
                }
            }

            // Wake parent if waiting
            if let Some(parent_pid) = proc.parent {
                if let Some(parent) = PROCESS_TABLE.get_mut(parent_pid) {
                    if parent.state == ProcessState::Waiting {
                        parent.state = ProcessState::Running;
                    }
                }
            }
        }
    }

    // Yield to scheduler (which will skip this zombie task)
    loop {
        crate::scheduler::pick_next();
        core::hint::spin_loop();
    }
}

/// Wait for a child process to exit
pub fn wait(child_pid: ProcessId) -> Option<ExitStatus> {
    let current = current_pid();

    unsafe {
        // Verify it's our child
        let proc = PROCESS_TABLE.get(current)?;
        if !proc.children.contains(&Some(child_pid)) {
            return None;
        }

        // Wait for child to become zombie
        loop {
            if let Some(child) = PROCESS_TABLE.get(child_pid) {
                if child.state == ProcessState::Zombie {
                    break;
                }
            } else {
                return None; // Child doesn't exist
            }

            // Set ourselves as waiting
            if let Some(proc) = PROCESS_TABLE.get_mut(current) {
                proc.state = ProcessState::Waiting;
            }

            crate::scheduler::pick_next();
        }

        // Reap the child
        let status = if let Some(child) = PROCESS_TABLE.get_mut(child_pid) {
            child.reap()
        } else {
            return None;
        };

        // Remove from our children
        if let Some(proc) = PROCESS_TABLE.get_mut(current) {
            proc.remove_child(child_pid);
            proc.state = ProcessState::Running;
        }

        // Remove from process table
        PROCESS_TABLE.remove(child_pid);

        Some(status)
    }
}

/// Wait for any child process to exit
pub fn waitpid_any() -> Option<(ProcessId, ExitStatus)> {
    let current = current_pid();

    unsafe {
        loop {
            // Check all children for zombies
            if let Some(proc) = PROCESS_TABLE.get(current) {
                for &child_pid in proc.children.iter().filter_map(|c| c.as_ref()) {
                    if let Some(child) = PROCESS_TABLE.get(child_pid) {
                        if child.state == ProcessState::Zombie {
                            // Found a zombie, reap it
                            let status = wait(child_pid)?;
                            return Some((child_pid, status));
                        }
                    }
                }

                // No zombies, check if we have any children at all
                if proc.child_count == 0 {
                    return None;
                }
            } else {
                return None;
            }

            // Wait
            if let Some(proc) = PROCESS_TABLE.get_mut(current) {
                proc.state = ProcessState::Waiting;
            }

            crate::scheduler::pick_next();
        }
    }
}

/// Get process count
pub fn process_count() -> usize {
    unsafe { PROCESS_TABLE.count() }
}

/// List all processes (for debugging/ps command)
pub fn list_processes<F: FnMut(&Process)>(mut f: F) {
    unsafe {
        for proc in PROCESS_TABLE.iter() {
            f(proc);
        }
    }
}
