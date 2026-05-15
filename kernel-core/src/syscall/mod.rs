//! Syscall Interface - Platform Independent
//!
//! Defines syscall numbers, the dispatch table, and handler implementations.
//! Platform crates provide the trap entry point (ARM64: SVC, x86_64: SYSCALL).
//!
//! # Syscall Numbers
//!
//! | Range  | Category        | Examples                    |
//! |--------|-----------------|-----------------------------|
//! | 0-9    | Core            | write, read, exit, yield    |
//! | 10-19  | File operations | open, close, seek, stat     |
//! | 20-29  | Semantic objects| create, query, link, search |
//! | 30-39  | Memory          | alloc, free, pool_info      |
//! | 40-49  | Process         | spawn, wait, getpid         |
//! | 50-59  | LLM services    | context, redact, summarize  |
//! | 60-69  | Crypto/Storage  | encrypt, decrypt, persist   |
//! | 70-79  | System          | time, uptime, reboot        |

/// Syscall number constants
pub mod numbers {
    // Core (0-9)
    pub const SYS_WRITE: u64 = 0;
    pub const SYS_READ: u64 = 1;
    pub const SYS_EXIT: u64 = 2;
    pub const SYS_YIELD: u64 = 3;
    pub const SYS_GETPID: u64 = 4;
    pub const SYS_SLEEP: u64 = 5;
    pub const SYS_INFO: u64 = 6;

    // File operations (10-19)
    pub const SYS_OPEN: u64 = 10;
    pub const SYS_CLOSE: u64 = 11;
    pub const SYS_FREAD: u64 = 12;
    pub const SYS_FWRITE: u64 = 13;
    pub const SYS_SEEK: u64 = 14;
    pub const SYS_STAT: u64 = 15;
    pub const SYS_MKDIR: u64 = 16;
    pub const SYS_UNLINK: u64 = 17;
    pub const SYS_READDIR: u64 = 18;

    // Semantic objects (20-29)
    pub const SYS_SEM_CREATE: u64 = 20;
    pub const SYS_SEM_READ: u64 = 21;
    pub const SYS_SEM_WRITE: u64 = 22;
    pub const SYS_SEM_DELETE: u64 = 23;
    pub const SYS_SEM_LINK: u64 = 24;
    pub const SYS_SEM_QUERY: u64 = 25;
    pub const SYS_SEM_SEARCH: u64 = 26;
    pub const SYS_SEM_META: u64 = 27;

    // Memory (30-39)
    pub const SYS_ALLOC: u64 = 30;
    pub const SYS_FREE: u64 = 31;
    pub const SYS_POOL_INFO: u64 = 32;
    pub const SYS_BRK: u64 = 33;

    // Process (40-49)
    pub const SYS_SPAWN: u64 = 40;
    pub const SYS_WAIT: u64 = 41;
    pub const SYS_KILL: u64 = 42;
    pub const SYS_EXEC: u64 = 43;
    pub const SYS_DUP: u64 = 44;
    pub const SYS_DUP2: u64 = 45;
    pub const SYS_PIPE: u64 = 46;

    // LLM services (50-59)
    pub const SYS_LLM_QUERY: u64 = 50;
    pub const SYS_LLM_CONTEXT: u64 = 51;
    pub const SYS_LLM_REDACT: u64 = 52;
    pub const SYS_LLM_SUMMARIZE: u64 = 53;
    pub const SYS_LLM_ACCESS: u64 = 54;
    pub const SYS_LLM_STREAM_START: u64 = 55;
    pub const SYS_LLM_STREAM_READ: u64 = 56;
    pub const SYS_LLM_SET_POLICY: u64 = 57;
    pub const SYS_LLM_GET_POLICY: u64 = 58;

    // Crypto/Storage (60-69)
    pub const SYS_ENCRYPT: u64 = 60;
    pub const SYS_DECRYPT: u64 = 61;
    pub const SYS_HASH: u64 = 62;
    pub const SYS_PERSIST: u64 = 63;
    pub const SYS_RESTORE: u64 = 64;

    // System (70-79)
    pub const SYS_TIME: u64 = 70;
    pub const SYS_UPTIME: u64 = 71;
    pub const SYS_REBOOT: u64 = 72;
    pub const SYS_SYSINFO: u64 = 73;
}

/// Syscall dispatch — called by platform trap handler with
/// (syscall_number, arg0, arg1, arg2, arg3).
/// Returns the result value.
pub fn dispatch(num: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    use numbers::*;

    match num {
        // Core (0-9)
        SYS_WRITE => handle_write(arg0, arg1),
        SYS_EXIT => handle_exit(arg0),
        SYS_YIELD => { handle_yield(); 0 },
        SYS_GETPID => handle_getpid(),
        SYS_SLEEP => handle_sleep(arg0),
        SYS_INFO => handle_info(),

        // File I/O (10-19)
        SYS_OPEN => handle_open(arg0, arg1),
        SYS_CLOSE => handle_close(arg0),
        SYS_FREAD => handle_fread(arg0, arg1, arg2),
        SYS_FWRITE => handle_fwrite(arg0, arg1, arg2),
        SYS_SEEK => handle_seek(arg0, arg1),
        SYS_STAT => handle_stat(arg0, arg1),

        // Semantic objects (20-29)
        SYS_SEM_CREATE => handle_sem_create(arg0, arg1, arg2, arg3),
        SYS_SEM_READ => handle_sem_read(arg0, arg1, arg2),
        SYS_SEM_WRITE => handle_sem_write(arg0, arg1, arg2, arg3),
        SYS_SEM_DELETE => handle_sem_delete(arg0, arg1),
        SYS_SEM_LINK => handle_sem_link(arg0, arg1, arg2, arg3),
        SYS_SEM_QUERY => handle_sem_query(arg0, arg1, arg2),
        SYS_SEM_SEARCH => handle_sem_search(arg0, arg1, arg2, arg3),
        SYS_SEM_META => handle_sem_meta(arg0, arg1, arg2),

        // Memory (30-39)
        SYS_ALLOC => handle_alloc(arg0, arg1),
        SYS_FREE => handle_free(arg0, arg1),
        SYS_POOL_INFO => handle_pool_info(arg0),

        // Process (40-49)
        SYS_SPAWN => handle_spawn(arg0, arg1, arg2),
        SYS_WAIT => handle_wait(arg0),
        SYS_KILL => handle_kill(arg0),
        SYS_DUP => handle_dup(arg0),
        SYS_DUP2 => handle_dup2(arg0, arg1),
        SYS_PIPE => handle_pipe(arg0),

        // LLM services (50-59)
        SYS_LLM_QUERY => handle_llm_query(arg0, arg1, arg2),
        SYS_LLM_CONTEXT => handle_llm_context(arg0, arg1, arg2),
        SYS_LLM_REDACT => handle_llm_redact(arg0, arg1, arg2),
        SYS_LLM_SUMMARIZE => handle_llm_summarize(arg0, arg1, arg2),
        SYS_LLM_ACCESS => handle_llm_access(arg0, arg1, arg2, arg3),
        SYS_LLM_STREAM_START => handle_llm_stream_start(arg0, arg1, arg2),
        SYS_LLM_STREAM_READ => handle_llm_stream_read(arg0, arg1, arg2),
        SYS_LLM_SET_POLICY => handle_llm_set_policy(arg0, arg1, arg2, arg3),
        SYS_LLM_GET_POLICY => handle_llm_get_policy(arg0, arg1, arg2, arg3),

        // Crypto/Storage (60-69)
        SYS_ENCRYPT => handle_encrypt(arg0, arg1, arg2, arg3),
        SYS_DECRYPT => handle_decrypt(arg0, arg1, arg2, arg3),
        SYS_HASH => handle_hash(arg0, arg1, arg2),

        // System (70-79)
        SYS_TIME => crate::platform::ticks(),

        _ => {
            crate::platform::log("[syscall] Unknown syscall: ");
            crate::platform::log_num(num);
            crate::platform::log("\n");
            u64::MAX // Error
        }
    }
}

// --- User pointer validation ---

/// Maximum user-space virtual address (lower half of canonical form).
/// Any pointer above this is in kernel space and must be rejected.
const USER_ADDR_LIMIT: u64 = 0x0000_8000_0000_0000;

/// Validate that a user pointer + length is in user-space and doesn't overflow.
fn validate_user_ptr(ptr: u64, len: u64) -> bool {
    if ptr == 0 || len == 0 { return false; }
    if ptr >= USER_ADDR_LIMIT { return false; }
    // Check for overflow
    match ptr.checked_add(len) {
        Some(end) if end <= USER_ADDR_LIMIT => true,
        _ => false,
    }
}

/// Read a byte slice from user space. Returns None if the pointer is invalid.
///
/// # Safety
/// The caller must ensure the memory at ptr..ptr+len is actually mapped.
/// We validate address range only (not page table presence).
unsafe fn read_user_slice(ptr: u64, len: u64) -> Option<&'static [u8]> {
    if !validate_user_ptr(ptr, len) {
        return None;
    }
    Some(core::slice::from_raw_parts(ptr as *const u8, len as usize))
}

/// Read a string from user space. Returns None if invalid pointer or not UTF-8.
unsafe fn read_user_str(ptr: u64, len: u64) -> Option<&'static str> {
    let slice = read_user_slice(ptr, len)?;
    core::str::from_utf8(slice).ok()
}

/// Write bytes to user space. Returns false if the pointer is invalid.
unsafe fn write_to_user(ptr: u64, data: &[u8]) -> bool {
    if !validate_user_ptr(ptr, data.len() as u64) {
        return false;
    }
    let dest = core::slice::from_raw_parts_mut(ptr as *mut u8, data.len());
    dest.copy_from_slice(data);
    true
}

// --- Handler implementations ---

fn handle_write(buf_ptr: u64, buf_len: u64) -> u64 {
    let len = buf_len as usize;
    if len > 4096 { return u64::MAX; }
    unsafe {
        // For kernel-mode callers, skip user validation (ptr may be in kernel space)
        let slice = core::slice::from_raw_parts(buf_ptr as *const u8, len);
        if let Ok(s) = core::str::from_utf8(slice) {
            crate::platform::log(s);
        }
    }
    buf_len
}

fn handle_exit(code: u64) -> u64 {
    let _ = code; // silenced "[syscall] Process exit with code N" — noisy in demos
    // Mark task as exited so pick_next will skip it forever.
    let idx = crate::scheduler::current_task_index();
    unsafe {
        let tasks = &raw mut crate::scheduler::TASKS;
        (*tasks)[idx].state = crate::scheduler::TaskState::Exited;
    }
    // Returns 0; the syscall asm will SYSRET back to Ring 3, the user RIP
    // points at whatever follows the `syscall` instruction (usually
    // padding/zeros) and the task will page-fault on its next instruction
    // fetch. The page-fault handler kills the (already-exited) task and
    // the next timer tick picks something else.
    // TODO: cleaner shutdown — modify the iret frame to point to a
    // kernel-mode trampoline that calls schedule directly.
    0
}

fn handle_yield() {
    // Yield the rest of this time slice. The platform's schedule() will
    // pick the next ready task and context-switch. When the caller is
    // eventually re-scheduled, this returns and SYSRET sends control
    // back to the user's instruction after `syscall`.
    crate::platform::schedule();
}

fn handle_getpid() -> u64 {
    crate::scheduler::current_task_index() as u64
}

fn handle_info() -> u64 {
    crate::platform::log("Semantic OS Kernel Core v0.1.0\n");
    0
}

fn handle_alloc(tier: u64, _size: u64) -> u64 {
    // Verify tier access
    let max_tier = crate::scheduler::current_task_max_tier();
    if tier as u8 > max_tier {
        crate::platform::log("[syscall] alloc: tier access denied\n");
        return 0;
    }
    // Allocate a physical frame from the requested tier's pool
    match crate::platform::get().alloc_frame(tier as u8) {
        Some(addr) => addr,
        None => {
            crate::platform::log("[syscall] alloc: pool exhausted for tier ");
            crate::platform::log_num(tier);
            crate::platform::log("\n");
            0
        }
    }
}

fn handle_free(ptr: u64, _size: u64) -> u64 {
    if ptr == 0 { return u64::MAX; }
    if crate::platform::get().free_frame(ptr) {
        0 // success
    } else {
        u64::MAX // error: address not recognized
    }
}

fn handle_pool_info(tier: u64) -> u64 {
    crate::platform::log("[syscall] pool_info for tier ");
    crate::platform::log_num(tier);
    crate::platform::log("\n");
    0
}

// --- File I/O handlers ---

/// SYS_OPEN(path_ptr, path_len) → fd or u64::MAX on error
fn handle_open(path_ptr: u64, path_len: u64) -> u64 {
    let name = unsafe {
        // Allow both user and kernel pointers for now
        let slice = core::slice::from_raw_parts(path_ptr as *const u8, path_len as usize);
        match core::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return u64::MAX,
        }
    };

    let fs = match crate::fs::ramfs::get_fs() {
        Some(fs) => fs,
        None => return u64::MAX,
    };
    let fd_table = match crate::fs::ramfs::get_fd_table_mut() {
        Some(t) => t,
        None => return u64::MAX,
    };

    match fd_table.open(fs, name) {
        Some(fd) => fd as u64,
        None => {
            crate::platform::log("[syscall] open: file not found: ");
            crate::platform::log(name);
            crate::platform::log("\n");
            u64::MAX
        }
    }
}

// --- Pipe FD tracking ---
//
// Maps file descriptor numbers to pipe endpoints.
// Separate from ramfs FdTable so file and pipe FDs can coexist.

/// Maximum pipe-backed file descriptors system-wide.
const MAX_PIPE_FDS: usize = 64;

/// A pipe FD entry: maps an FD number to a pipe ID + direction.
#[derive(Clone, Copy)]
struct PipeFdEntry {
    fd: usize,
    pipe_id: usize,
    is_read_end: bool,
    active: bool,
}

impl PipeFdEntry {
    const fn empty() -> Self {
        Self { fd: 0, pipe_id: 0, is_read_end: false, active: false }
    }
}

static mut PIPE_FDS: [PipeFdEntry; MAX_PIPE_FDS] = [PipeFdEntry::empty(); MAX_PIPE_FDS];

/// Register a pipe FD.
fn register_pipe_fd(fd: usize, pipe_id: usize, is_read_end: bool) {
    unsafe {
        let fds = &raw mut PIPE_FDS;
        for entry in (*fds).iter_mut() {
            if !entry.active {
                *entry = PipeFdEntry { fd, pipe_id, is_read_end, active: true };
                return;
            }
        }
    }
}

/// Look up a pipe FD. Returns (pipe_id, is_read_end) if found.
fn lookup_pipe_fd(fd: usize) -> Option<(usize, bool)> {
    unsafe {
        let fds = &raw const PIPE_FDS;
        for entry in (*fds).iter() {
            if entry.active && entry.fd == fd {
                return Some((entry.pipe_id, entry.is_read_end));
            }
        }
    }
    None
}

/// Unregister a pipe FD.
fn unregister_pipe_fd(fd: usize) -> Option<(usize, bool)> {
    unsafe {
        let fds = &raw mut PIPE_FDS;
        for entry in (*fds).iter_mut() {
            if entry.active && entry.fd == fd {
                let result = (entry.pipe_id, entry.is_read_end);
                entry.active = false;
                return Some(result);
            }
        }
    }
    None
}

/// Allocate the next free FD number (starting from 3, skipping stdin/out/err).
fn alloc_fd_number() -> Option<usize> {
    // Check which FD numbers are in use by pipes
    let mut used = [false; 64];
    unsafe {
        let fds = &raw const PIPE_FDS;
        for entry in (*fds).iter() {
            if entry.active && entry.fd < 64 {
                used[entry.fd] = true;
            }
        }
    }
    // Also check ramfs FDs
    if let Some(fd_table) = crate::fs::ramfs::get_fd_table_mut() {
        // FDs 0-2 are reserved, ramfs uses 3+
        // We'll start pipe FDs from a higher range to avoid collisions
    }
    // Find first free FD from 3 upward
    for fd in 3..64 {
        if !used[fd] {
            return Some(fd);
        }
    }
    None
}

/// SYS_CLOSE(fd) → 0 on success, u64::MAX on error
fn handle_close(fd: u64) -> u64 {
    let fd_num = fd as usize;

    // Check if it's a pipe FD first
    if let Some((pipe_id, is_read_end)) = unregister_pipe_fd(fd_num) {
        if is_read_end {
            crate::ipc::close_read_end(pipe_id);
        } else {
            crate::ipc::close_write_end(pipe_id);
        }
        return 0;
    }

    // Otherwise try ramfs
    let fd_table = match crate::fs::ramfs::get_fd_table_mut() {
        Some(t) => t,
        None => return u64::MAX,
    };
    if fd_table.close(fd_num) { 0 } else { u64::MAX }
}

/// SYS_FREAD(fd, buf_ptr, buf_len) → bytes read, 0 = EOF, u64::MAX = error
fn handle_fread(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    let fd_num = fd as usize;
    let len = buf_len as usize;
    if len == 0 || len > 4096 { return u64::MAX; }

    // Check if it's a pipe FD
    if let Some((pipe_id, true)) = lookup_pipe_fd(fd_num) {
        let mut tmp = [0u8; 4096];
        let read_buf = &mut tmp[..len];
        match crate::ipc::pipe_read(pipe_id, read_buf) {
            Some(n) => {
                if n > 0 {
                    unsafe {
                        let dest = core::slice::from_raw_parts_mut(buf_ptr as *mut u8, n);
                        dest.copy_from_slice(&read_buf[..n]);
                    }
                }
                n as u64
            }
            None => {
                // Block: pipe empty, write end still open
                let idx = crate::scheduler::current_task_index();
                unsafe {
                    let tasks = &raw mut crate::scheduler::TASKS;
                    (*tasks)[idx].state = crate::scheduler::TaskState::Blocked;
                    (*tasks)[idx].block_reason =
                        crate::scheduler::BlockReason::PipeRead(pipe_id);
                }
                // Return a sentinel; the task will retry after unblock.
                // In practice the syscall will be re-entered after the task is scheduled.
                0
            }
        }
    } else {
        // Ramfs file read
        let fs = match crate::fs::ramfs::get_fs() {
            Some(fs) => fs,
            None => return u64::MAX,
        };
        let fd_table = match crate::fs::ramfs::get_fd_table_mut() {
            Some(t) => t,
            None => return u64::MAX,
        };

        let mut tmp = [0u8; 4096];
        let read_buf = &mut tmp[..len];

        match fd_table.read(fs, fd_num, read_buf) {
            Some(n) => {
                if n > 0 {
                    unsafe {
                        let dest = core::slice::from_raw_parts_mut(buf_ptr as *mut u8, n);
                        dest.copy_from_slice(&read_buf[..n]);
                    }
                }
                n as u64
            }
            None => u64::MAX,
        }
    }
}

/// SYS_FWRITE(fd, buf_ptr, buf_len) → bytes written or u64::MAX
fn handle_fwrite(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    let fd_num = fd as usize;

    // Console output
    if fd_num == 1 || fd_num == 2 {
        return handle_write(buf_ptr, buf_len);
    }

    // Check if it's a pipe FD (write end)
    if let Some((pipe_id, false)) = lookup_pipe_fd(fd_num) {
        let len = (buf_len as usize).min(4096);
        let data = unsafe {
            core::slice::from_raw_parts(buf_ptr as *const u8, len)
        };
        match crate::ipc::pipe_write(pipe_id, data) {
            Some(0) => {
                // Broken pipe (read end closed)
                crate::platform::log("[syscall] fwrite: broken pipe\n");
                u64::MAX
            }
            Some(n) => n as u64,
            None => {
                // Block: pipe full, read end still open
                let idx = crate::scheduler::current_task_index();
                unsafe {
                    let tasks = &raw mut crate::scheduler::TASKS;
                    (*tasks)[idx].state = crate::scheduler::TaskState::Blocked;
                    (*tasks)[idx].block_reason =
                        crate::scheduler::BlockReason::PipeWrite(pipe_id);
                }
                0
            }
        }
    } else {
        // ramfs is read-only
        crate::platform::log("[syscall] fwrite: read-only filesystem\n");
        u64::MAX
    }
}

/// SYS_SEEK(fd, position) → 0 on success, u64::MAX on error
fn handle_seek(fd: u64, position: u64) -> u64 {
    let fd_table = match crate::fs::ramfs::get_fd_table_mut() {
        Some(t) => t,
        None => return u64::MAX,
    };
    if fd_table.seek(fd as usize, position as usize) { 0 } else { u64::MAX }
}

/// SYS_STAT(path_ptr, path_len) → file size, or u64::MAX if not found
fn handle_stat(path_ptr: u64, path_len: u64) -> u64 {
    let name = unsafe {
        let slice = core::slice::from_raw_parts(path_ptr as *const u8, path_len as usize);
        match core::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return u64::MAX,
        }
    };

    let fs = match crate::fs::ramfs::get_fs() {
        Some(fs) => fs,
        None => return u64::MAX,
    };

    match fs.find(name) {
        Some(file) => file.size() as u64,
        None => u64::MAX,
    }
}

// --- Sleep handler ---

/// SYS_SLEEP(ticks) → 0
/// Blocks the current task for the given number of timer ticks.
fn handle_sleep(ticks: u64) -> u64 {
    if ticks == 0 { return 0; }
    let wake_at = crate::platform::ticks() + ticks;
    let idx = crate::scheduler::current_task_index();
    unsafe {
        let tasks = &raw mut crate::scheduler::TASKS;
        (*tasks)[idx].state = crate::scheduler::TaskState::Blocked;
        (*tasks)[idx].wake_at = wake_at;
        (*tasks)[idx].block_reason = crate::scheduler::BlockReason::Sleep;
    }
    0
}

// --- Pipe / FD management handlers ---

/// SYS_PIPE(out_ptr) → 0 on success, u64::MAX on error
///
/// Creates a pipe and writes [read_fd, write_fd] (two u64s) to `out_ptr`.
fn handle_pipe(out_ptr: u64) -> u64 {
    let pipe_id = match crate::ipc::create_pipe() {
        Some(id) => id,
        None => {
            crate::platform::log("[syscall] pipe: no free pipe slots\n");
            return u64::MAX;
        }
    };

    // Allocate two FD numbers
    let read_fd = match alloc_fd_number() {
        Some(fd) => fd,
        None => return u64::MAX,
    };
    register_pipe_fd(read_fd, pipe_id, true);

    let write_fd = match alloc_fd_number() {
        Some(fd) => fd,
        None => {
            unregister_pipe_fd(read_fd);
            crate::ipc::close_read_end(pipe_id);
            crate::ipc::close_write_end(pipe_id);
            return u64::MAX;
        }
    };
    register_pipe_fd(write_fd, pipe_id, false);

    // Write the two FDs to user space
    unsafe {
        let out = out_ptr as *mut u64;
        *out = read_fd as u64;
        *out.add(1) = write_fd as u64;
    }

    0
}

/// SYS_DUP(old_fd) → new_fd or u64::MAX
fn handle_dup(old_fd: u64) -> u64 {
    let old = old_fd as usize;

    // If it's a pipe FD, duplicate the pipe binding
    if let Some((pipe_id, is_read_end)) = lookup_pipe_fd(old) {
        let new_fd = match alloc_fd_number() {
            Some(fd) => fd,
            None => return u64::MAX,
        };
        register_pipe_fd(new_fd, pipe_id, is_read_end);
        return new_fd as u64;
    }

    // Otherwise try ramfs FD duplication (not currently supported — return error)
    u64::MAX
}

/// SYS_DUP2(old_fd, new_fd) → new_fd or u64::MAX
fn handle_dup2(old_fd: u64, new_fd: u64) -> u64 {
    let old = old_fd as usize;
    let new = new_fd as usize;
    if new >= 64 { return u64::MAX; }

    // Close the target FD if it's open
    if lookup_pipe_fd(new).is_some() {
        // Close existing pipe binding on new_fd
        if let Some((pipe_id, is_read_end)) = unregister_pipe_fd(new) {
            if is_read_end {
                crate::ipc::close_read_end(pipe_id);
            } else {
                crate::ipc::close_write_end(pipe_id);
            }
        }
    }

    // Copy the old FD to the new slot
    if let Some((pipe_id, is_read_end)) = lookup_pipe_fd(old) {
        register_pipe_fd(new, pipe_id, is_read_end);
        return new as u64;
    }

    u64::MAX
}

// --- Process management handlers ---

/// SYS_SPAWN(path_ptr, path_len, max_tier) → PID or u64::MAX on error
///
/// Loads an ELF binary from ramfs and spawns it as a Ring 3 process.
fn handle_spawn(path_ptr: u64, path_len: u64, max_tier: u64) -> u64 {
    // Validate tier access — can't spawn at a higher tier than yourself
    let caller_tier = crate::scheduler::current_task_max_tier();
    let spawn_tier = (max_tier as u8).min(caller_tier);

    let name = unsafe {
        let slice = core::slice::from_raw_parts(path_ptr as *const u8, path_len as usize);
        match core::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return u64::MAX,
        }
    };

    // Look up the ELF binary in ramfs
    let fs = match crate::fs::ramfs::get_fs() {
        Some(fs) => fs,
        None => return u64::MAX,
    };

    let file = match fs.find(name) {
        Some(f) => f,
        None => {
            crate::platform::log("[syscall] spawn: file not found: ");
            crate::platform::log(name);
            crate::platform::log("\n");
            return u64::MAX;
        }
    };

    let elf_data = file.data();

    // We need a &'static str for the process name — use a fixed set
    // (scheduler requires 'static lifetime names)
    let static_name: &'static str = match name {
        "init" => "init",
        "shell" => "shell",
        "test" => "test",
        _ => "user",
    };

    match crate::process::spawn_from_elf(static_name, elf_data, spawn_tier) {
        Some(pid) => pid.0 as u64,
        None => {
            crate::platform::log("[syscall] spawn: failed to load ELF\n");
            u64::MAX
        }
    }
}

/// SYS_WAIT(pid) → exit code or u64::MAX on error
///
/// Waits for a child process to exit and returns its exit code.
/// If pid == 0, waits for any child.
fn handle_wait(pid: u64) -> u64 {
    if pid == 0 {
        // Wait for any child
        match crate::process::waitpid_any() {
            Some((_child_pid, status)) => status.code as u64,
            None => u64::MAX,
        }
    } else {
        let child_pid = crate::process::ProcessId(pid as u32);
        match crate::process::wait(child_pid) {
            Some(status) => status.code as u64,
            None => u64::MAX,
        }
    }
}

/// SYS_KILL(pid) → 0 on success, u64::MAX on error
///
/// Terminates a process. The caller must be the parent or have Secret tier.
fn handle_kill(pid: u64) -> u64 {
    let target_pid = crate::process::ProcessId(pid as u32);
    let caller_pid = crate::process::current_pid();
    let caller_tier = crate::scheduler::current_task_max_tier();

    // Check permission: must be parent or have Secret tier
    let allowed = if let Some(target) = crate::process::get(target_pid) {
        target.parent == Some(caller_pid) || caller_tier >= 3
    } else {
        false
    };

    if !allowed {
        crate::platform::log("[syscall] kill: permission denied\n");
        return u64::MAX;
    }

    // Mark the process as zombie
    unsafe {
        if let Some(proc) = crate::process::get_mut(target_pid) {
            proc.exit(crate::process::ExitStatus::killed(9));
            // Also mark the scheduler task as exited
            if let Some(task_id) = proc.task_id {
                let tasks = &raw mut crate::scheduler::TASKS;
                (*tasks)[task_id].state = crate::scheduler::TaskState::Exited;
            }
            0
        } else {
            u64::MAX
        }
    }
}

// ============================================================================
// Semantic Object Syscalls (20-29)
// ============================================================================

/// Helper: convert u8 tier to SecurityTier
fn tier_from_u8(t: u8) -> crate::memory::SecurityTier {
    match t {
        0 => crate::memory::SecurityTier::Public,
        1 => crate::memory::SecurityTier::Internal,
        2 => crate::memory::SecurityTier::Sensitive,
        _ => crate::memory::SecurityTier::Secret,
    }
}

/// SYS_SEM_CREATE(suid_high, suid_low, tier, content_ptr | content_len<<32)
/// Creates a semantic object. Returns 0 on success, u64::MAX on error.
fn handle_sem_create(suid_high: u64, suid_low: u64, tier: u64, content_info: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let obj_tier = (tier as u8).min(max_tier);

    let suid = crate::semantic::SUID::new(suid_high, suid_low);
    let security_tier = tier_from_u8(obj_tier);
    let owner = crate::scheduler::current_task_index() as u8;

    let content_ptr = content_info & 0xFFFF_FFFF;
    let content_len = (content_info >> 32) as usize;

    let obj = if content_ptr != 0 && content_len > 0 && content_len <= 1024 {
        let data = unsafe {
            core::slice::from_raw_parts(content_ptr as *const u8, content_len)
        };
        match crate::semantic::SemanticObject::with_content(suid, security_tier, owner, data) {
            Some(o) => o,
            None => return u64::MAX,
        }
    } else {
        crate::semantic::SemanticObject::new(suid, security_tier, owner)
    };

    unsafe {
        let registry = crate::semantic::registry::global_registry();
        if registry.insert(obj) { 0 } else { u64::MAX }
    }
}

/// SYS_SEM_READ(suid_high, suid_low, out_ptr) → content length or u64::MAX
/// Reads the content of a semantic object into the buffer at out_ptr (max 1024 bytes).
fn handle_sem_read(suid_high: u64, suid_low: u64, out_ptr: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let suid = crate::semantic::SUID::new(suid_high, suid_low);

    unsafe {
        let registry = crate::semantic::registry::global_registry();
        match registry.get(&suid) {
            Some(obj) => {
                if (obj.tier as u8) > max_tier {
                    crate::platform::log("[syscall] sem_read: tier access denied\n");
                    return u64::MAX;
                }
                match obj.content.as_bytes() {
                    Some(data) => {
                        if out_ptr != 0 {
                            let dest = core::slice::from_raw_parts_mut(
                                out_ptr as *mut u8, data.len(),
                            );
                            dest.copy_from_slice(data);
                        }
                        data.len() as u64
                    }
                    None => 0, // Empty content
                }
            }
            None => u64::MAX,
        }
    }
}

/// SYS_SEM_WRITE(suid_high, suid_low, data_ptr, data_len) → 0 or u64::MAX
/// Updates the content of an existing semantic object.
fn handle_sem_write(suid_high: u64, suid_low: u64, data_ptr: u64, data_len: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let suid = crate::semantic::SUID::new(suid_high, suid_low);
    let len = data_len as usize;
    if len > 1024 { return u64::MAX; }

    unsafe {
        let registry = crate::semantic::registry::global_registry();
        match registry.get_mut(&suid) {
            Some(obj) => {
                if (obj.tier as u8) > max_tier {
                    return u64::MAX;
                }
                if obj.flags.is_immutable() {
                    return u64::MAX;
                }
                let data = core::slice::from_raw_parts(data_ptr as *const u8, len);
                match crate::semantic::ObjectContent::from_inline(data) {
                    Some(content) => {
                        obj.content = content;
                        0
                    }
                    None => u64::MAX,
                }
            }
            None => u64::MAX,
        }
    }
}

/// SYS_SEM_DELETE(suid_high, suid_low) → 0 or u64::MAX
fn handle_sem_delete(suid_high: u64, suid_low: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let suid = crate::semantic::SUID::new(suid_high, suid_low);

    unsafe {
        let registry = crate::semantic::registry::global_registry();
        // Check tier before removing
        if let Some(obj) = registry.get(&suid) {
            if (obj.tier as u8) > max_tier {
                return u64::MAX;
            }
        } else {
            return u64::MAX;
        }
        match registry.remove(&suid) {
            Some(_) => 0,
            None => u64::MAX,
        }
    }
}

/// SYS_SEM_LINK(src_high, src_low, dst_high, dst_low) → 0 or u64::MAX
/// Links two semantic objects (src → dst with References relation).
fn handle_sem_link(src_high: u64, src_low: u64, dst_high: u64, dst_low: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let src_suid = crate::semantic::SUID::new(src_high, src_low);
    let dst_suid = crate::semantic::SUID::new(dst_high, dst_low);

    unsafe {
        let registry = crate::semantic::registry::global_registry();
        match registry.get_mut(&src_suid) {
            Some(obj) => {
                if (obj.tier as u8) > max_tier {
                    return u64::MAX;
                }
                if obj.add_link(dst_suid, crate::semantic::object::RelationType::References) {
                    0
                } else {
                    u64::MAX
                }
            }
            None => u64::MAX,
        }
    }
}

/// SYS_SEM_QUERY(tier_filter, out_ptr, max_results) → count or u64::MAX
/// Lists semantic objects at or below the given tier. Writes SUID pairs to out_ptr.
fn handle_sem_query(tier_filter: u64, out_ptr: u64, max_results: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let filter = (tier_filter as u8).min(max_tier);
    let limit = (max_results as usize).min(64);

    unsafe {
        let registry = crate::semantic::registry::global_registry();
        let mut count = 0usize;
        let out = out_ptr as *mut u64;

        for obj in registry.iter() {
            if (obj.tier as u8) <= filter {
                if count < limit {
                    *out.add(count * 2) = obj.suid.high;
                    *out.add(count * 2 + 1) = obj.suid.low;
                }
                count += 1;
            }
        }
        count as u64
    }
}

/// SYS_SEM_SEARCH(query_ptr, query_dims, max_results, out_ptr) → count
/// Vector similarity search. query_ptr points to f32 array, results written to out_ptr.
fn handle_sem_search(query_ptr: u64, query_dims: u64, max_results: u64, out_ptr: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let dims = query_dims as usize;
    let limit = (max_results as usize).min(16);

    if dims == 0 || dims > 384 { return u64::MAX; }

    let query = unsafe {
        core::slice::from_raw_parts(query_ptr as *const f32, dims)
    };

    unsafe {
        let search = crate::semantic::search::global_search();
        let mut results = [crate::semantic::SearchResult::new(0, 0, 0.0); 16];
        match search.find_similar(query, max_tier, limit, &mut results[..limit]) {
            Ok(count) => {
                // Write results: each result is (suid_high, suid_low, score_bits)
                let out = out_ptr as *mut u64;
                for i in 0..count {
                    *out.add(i * 3) = results[i].suid_high;
                    *out.add(i * 3 + 1) = results[i].suid_low;
                    *out.add(i * 3 + 2) = (results[i].score.to_bits()) as u64;
                }
                count as u64
            }
            Err(_) => 0,
        }
    }
}

/// SYS_SEM_META(suid_high, suid_low, out_ptr) → 0 or u64::MAX
/// Reads metadata about a semantic object: [tier, owner, content_len, link_count, flags].
fn handle_sem_meta(suid_high: u64, suid_low: u64, out_ptr: u64) -> u64 {
    let max_tier = crate::scheduler::current_task_max_tier();
    let suid = crate::semantic::SUID::new(suid_high, suid_low);

    unsafe {
        let registry = crate::semantic::registry::global_registry();
        match registry.get(&suid) {
            Some(obj) => {
                if (obj.tier as u8) > max_tier {
                    return u64::MAX;
                }
                let out = out_ptr as *mut u64;
                *out.add(0) = obj.tier as u64;
                *out.add(1) = obj.owner as u64;
                *out.add(2) = obj.content.len() as u64;
                let link_count = obj.get_links().iter().filter(|l| l.is_some()).count();
                *out.add(3) = link_count as u64;
                *out.add(4) = obj.flags.as_u32() as u64;
                0
            }
            None => u64::MAX,
        }
    }
}

// ============================================================================
// LLM Service Syscalls (50-59)
// ============================================================================

/// SYS_LLM_QUERY(prompt_ptr, prompt_len, out_ptr) → response length or u64::MAX
/// Submits a prompt to the LLM provider. Writes response to out_ptr (max 4096 bytes).
fn handle_llm_query(prompt_ptr: u64, prompt_len: u64, out_ptr: u64) -> u64 {
    let len = prompt_len as usize;
    if len == 0 || len > 1024 { return u64::MAX; }
    let tier = crate::scheduler::current_task_max_tier();
    let task_id = crate::scheduler::current_task_index() as u8;

    let prompt = unsafe {
        core::slice::from_raw_parts(prompt_ptr as *const u8, len)
    };

    unsafe {
        let provider = crate::llm::provider::global_provider();
        let mut request = crate::llm::provider::LlmRequest::new(task_id, tier, prompt);

        match provider.submit(request) {
            Ok(request_id) => {
                // Process immediately (mock provider)
                provider.process_pending();
                // Try to get the response
                match provider.get_response(request_id) {
                    Some(response) => {
                        if response.is_success() {
                            let content = response.content();
                            if out_ptr != 0 && !content.is_empty() {
                                let dest = core::slice::from_raw_parts_mut(
                                    out_ptr as *mut u8, content.len(),
                                );
                                dest.copy_from_slice(content);
                            }
                            content.len() as u64
                        } else {
                            u64::MAX
                        }
                    }
                    None => u64::MAX,
                }
            }
            Err(_) => u64::MAX,
        }
    }
}

/// SYS_LLM_CONTEXT(suid_pairs_ptr, count, out_ptr) → context size or u64::MAX
/// Builds an LLM context from a list of SUID pairs. Writes serialized context to out_ptr.
///
/// FIXED 2026-05-14: avoid 258KB LlmContext stack allocation (task #40 root cause).
/// Process entries one-by-one using static scratch buffer instead of build_from_suids.
fn handle_llm_context(suid_pairs_ptr: u64, count: u64, out_ptr: u64) -> u64 {
    let n = (count as usize).min(32);
    let tier = crate::scheduler::current_task_max_tier();

    let suids = unsafe {
        core::slice::from_raw_parts(suid_pairs_ptr as *const (u64, u64), n)
    };

    // Static scratch buffer for processing one entry at a time.
    // Safe because syscalls are serialized (no concurrent access).
    static mut CONTEXT_SCRATCH: [u8; 4096] = [0; 4096];

    unsafe {
        let registry = crate::semantic::registry::global_registry();
        let redactor = crate::llm::context_builder::global_redactor();
        let scratch = core::slice::from_raw_parts_mut(
            (&raw mut CONTEXT_SCRATCH) as *mut u8, 4096
        );

        let mut total_size = 0usize;
        let mut offset = 0usize;
        let out = if out_ptr != 0 { out_ptr as *mut u8 } else { core::ptr::null_mut() };

        for (suid_high, suid_low) in suids {
            let suid = crate::semantic::SUID::new(*suid_high, *suid_low);

            if let Some(object) = registry.get(&suid) {
                let obj_tier = object.tier as u8;
                if obj_tier > tier {
                    continue; // Can't access this tier
                }

                // Apply tier-based processing (same logic as build_from_suids)
                let obj_content_bytes = match object.content.as_bytes() {
                    Some(bytes) => bytes,
                    None => continue, // Skip objects with no content
                };

                let content = match obj_tier {
                    0 => obj_content_bytes, // Tier 0: verbatim
                    1 => {
                        // Tier 1: summarize (placeholder - use redactor for now)
                        let n = redactor.redact(obj_content_bytes, scratch);
                        &scratch[..n]
                    }
                    2 => {
                        // Tier 2: redact
                        let n = redactor.redact(obj_content_bytes, scratch);
                        &scratch[..n]
                    }
                    _ => continue, // Tier 3+: exclude
                };

                let entry_len = content.len();
                total_size += entry_len + 8; // +8 for length prefix

                // Write to output buffer if provided
                if !out.is_null() && offset + entry_len + 8 <= 32768 {
                    // Write length prefix
                    let len_bytes = (entry_len as u64).to_le_bytes();
                    core::ptr::copy_nonoverlapping(
                        len_bytes.as_ptr(), out.add(offset), 8
                    );
                    offset += 8;

                    // Write content
                    core::ptr::copy_nonoverlapping(
                        content.as_ptr(), out.add(offset), entry_len
                    );
                    offset += entry_len;
                }
            }
        }

        if out.is_null() {
            total_size as u64 // Size query
        } else {
            offset as u64 // Bytes written
        }
    }
}

/// SYS_LLM_REDACT(input_ptr, input_len, out_ptr) → output length or u64::MAX
/// Redacts sensitive patterns (emails, SSNs, etc.) from text.
fn handle_llm_redact(input_ptr: u64, input_len: u64, out_ptr: u64) -> u64 {
    let len = input_len as usize;
    if len == 0 || len > 4096 { return u64::MAX; }

    // Use a static scratch buffer instead of a stack-allocated 4 KiB array.
    // The per-task kernel stack is 8 KiB, and a 4 KiB local plus the
    // syscall entry, dispatch, and handler frames overflows it silently
    // (no guard page yet). Single-threaded for now → static is safe.
    static mut REDACT_SCRATCH: [u8; 4096] = [0; 4096];

    let input = unsafe {
        core::slice::from_raw_parts(input_ptr as *const u8, len)
    };

    unsafe {
        let redactor = crate::llm::context_builder::global_redactor();
        let scratch_slice: &mut [u8] = core::slice::from_raw_parts_mut(
            (&raw mut REDACT_SCRATCH) as *mut u8,
            4096,
        );
        let out_len = redactor.redact(input, scratch_slice);
        if out_ptr != 0 && out_len > 0 {
            let dest = core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len);
            dest.copy_from_slice(&scratch_slice[..out_len]);
        }
        out_len as u64
    }
}

/// SYS_LLM_SUMMARIZE(input_ptr, input_len, out_ptr) → output length or u64::MAX
/// Summarizes text content.
fn handle_llm_summarize(input_ptr: u64, input_len: u64, out_ptr: u64) -> u64 {
    let len = input_len as usize;
    if len == 0 || len > 4096 { return u64::MAX; }

    let input = unsafe {
        core::slice::from_raw_parts(input_ptr as *const u8, len)
    };

    unsafe {
        let summarizer = crate::llm::context_builder::global_summarizer();
        let summary = summarizer.summarize(input);
        let data = summary.as_bytes();
        if out_ptr != 0 && !data.is_empty() {
            let dest = core::slice::from_raw_parts_mut(out_ptr as *mut u8, data.len());
            dest.copy_from_slice(data);
        }
        data.len() as u64
    }
}

/// SYS_LLM_ACCESS(requester_id, current_tier, requested_tier, justification_ptr | len<<32)
/// Submits a tier escalation request. Returns request ID or u64::MAX.
fn handle_llm_access(requester_id: u64, current_tier: u64, requested_tier: u64, justification_info: u64) -> u64 {
    let just_ptr = justification_info & 0xFFFF_FFFF;
    let just_len = (justification_info >> 32) as usize;

    let justification = if just_ptr != 0 && just_len > 0 && just_len <= 256 {
        unsafe { core::slice::from_raw_parts(just_ptr as *const u8, just_len) }
    } else {
        b"No justification provided"
    };

    unsafe {
        let queue = crate::llm::access_request::global_escalation_queue();
        let request = crate::llm::access_request::AccessRequest::for_tier(
            requester_id as u8,
            current_tier as u8,
            requested_tier as u8,
            justification,
        );
        match queue.submit(request) {
            Ok(id) => id as u64,
            Err(_) => u64::MAX,
        }
    }
}

/// SYS_LLM_STREAM_START(prompt_ptr, prompt_len, context_ptr) → request_id or u64::MAX
/// Start a streaming LLM request. Returns request ID for polling with SYS_LLM_STREAM_READ.
/// context_ptr points to serialized context data (same format as SYS_LLM_CONTEXT output).
pub fn handle_llm_stream_start(prompt_ptr: u64, prompt_len: u64, context_ptr: u64) -> u64 {
    let len = prompt_len as usize;
    if len == 0 || len > 1024 { return u64::MAX; }

    let prompt = unsafe {
        core::slice::from_raw_parts(prompt_ptr as *const u8, len)
    };

    let task_id = crate::scheduler::current_task_index() as u8;
    let tier = crate::scheduler::current_task_max_tier();

    unsafe {
        let provider = crate::llm::provider::global_provider();
        let request = crate::llm::provider::LlmRequest::new(task_id, tier, prompt);

        match provider.submit(request) {
            Ok(request_id) => request_id,
            Err(_) => u64::MAX,
        }
    }
}

/// SYS_LLM_STREAM_READ(request_id, out_ptr, out_len) → bytes_read or error_code
/// Read chunk from streaming LLM response. Returns:
/// - > 0: bytes read (response continues)
/// - 0: response complete
/// - u64::MAX: error or invalid request_id
/// Special values: u64::MAX-1 = still processing, u64::MAX-2 = cancelled
pub fn handle_llm_stream_read(request_id: u64, out_ptr: u64, out_len: u64) -> u64 {
    let max_len = out_len as usize;
    if max_len == 0 || out_ptr == 0 { return u64::MAX; }

    unsafe {
        let provider = crate::llm::provider::global_provider();

        // Check request status
        match provider.get_status(request_id) {
            Some(crate::llm::provider::RequestState::Queued) |
            Some(crate::llm::provider::RequestState::Processing) => {
                // Still processing
                u64::MAX - 1
            },
            Some(crate::llm::provider::RequestState::Completed) => {
                // Get response and copy to output buffer
                if let Some(response) = provider.get_response(request_id) {
                    if response.is_success() {
                        let content = response.content();
                        let copy_len = content.len().min(max_len);
                        let out = core::slice::from_raw_parts_mut(out_ptr as *mut u8, copy_len);
                        out.copy_from_slice(&content[..copy_len]);
                        copy_len as u64
                    } else {
                        response.error_code as u64
                    }
                } else {
                    u64::MAX
                }
            },
            Some(crate::llm::provider::RequestState::Cancelled) => {
                u64::MAX - 2
            },
            Some(crate::llm::provider::RequestState::Failed) => {
                // Try to get error code from response
                if let Some(response) = provider.get_response(request_id) {
                    response.error_code as u64
                } else {
                    u64::MAX
                }
            },
            _ => u64::MAX, // Invalid request ID
        }
    }
}

/// SYS_LLM_SET_POLICY(suid_high, suid_low, policy_data_ptr, policy_data_len) → result
/// Create or update a security policy. Returns:
/// - SUID.high: success, policy stored
/// - u64::MAX: error (permission denied, invalid data, etc.)
/// - u64::MAX-1: policy validation failed
/// - u64::MAX-2: insufficient privilege
pub fn handle_llm_set_policy(suid_high: u64, suid_low: u64, policy_data_ptr: u64, policy_data_len: u64) -> u64 {
    let data_len = policy_data_len as usize;
    if data_len == 0 || data_len > crate::security::policy::MAX_POLICY_SIZE || policy_data_ptr == 0 {
        return u64::MAX - 1; // Invalid parameters
    }

    let policy_suid = crate::semantic::SUID::new(suid_high, suid_low);

    // Validate this is a policy SUID
    if !crate::security::policy_suids::is_policy_suid(&policy_suid) {
        return u64::MAX - 1; // Not a valid policy SUID
    }

    let requester_id = crate::scheduler::current_task_index() as u8;
    let requester_tier = crate::scheduler::current_task_max_tier();

    // Read policy data from user space
    let policy_data = unsafe {
        core::slice::from_raw_parts(policy_data_ptr as *const u8, data_len)
    };

    // Deserialize policy object
    let policy = match crate::security::policy::PolicyObject::deserialize(policy_data) {
        Ok(p) => p,
        Err(_) => return u64::MAX - 1, // Invalid policy data
    };

    // Check permissions
    unsafe {
        let registry = crate::semantic::registry::global_registry();

        // If policy already exists, check if requester can modify it
        if let Some(existing_obj) = registry.get(&policy_suid) {
            if let Some(existing_content) = existing_obj.content.as_bytes() {
                if let Ok(existing_policy) = crate::security::policy::PolicyObject::deserialize(existing_content) {
                    if !existing_policy.can_modify(requester_id) {
                        return u64::MAX - 2; // Insufficient privilege
                    }
                }
            }
        } else {
            // Creating new policy - check if user has permission to create policies
            // System policies can only be created by admin/system
            if crate::security::policy_suids::is_system_policy(&policy_suid) {
                if requester_id != crate::security::user_ids::ADMIN &&
                   requester_id != crate::security::user_ids::SYSTEM {
                    return u64::MAX - 2; // Insufficient privilege
                }
            }
        }

        // Validate policy structure
        if !policy.is_active() && policy.rule_count == 0 {
            return u64::MAX - 1; // Empty/invalid policy
        }

        // Create semantic object for the policy
        // Policies are stored at Secret tier by default for security
        let policy_obj = match crate::semantic::SemanticObject::with_content(
            policy_suid,
            crate::memory::SecurityTier::Secret,
            policy.owner,
            policy_data,
        ) {
            Some(obj) => obj,
            None => return u64::MAX, // Failed to create object
        };

        // Store in registry
        if registry.insert(policy_obj) {
            // Policy stored successfully
            suid_high
        } else {
            u64::MAX // Registry insertion failed
        }
    }
}

/// SYS_LLM_GET_POLICY(suid_high, suid_low, out_ptr, out_len) → bytes_read or error
/// Retrieve a security policy by SUID. Returns:
/// - >0: bytes written to output buffer
/// - 0: policy not found
/// - u64::MAX: error (permission denied, buffer too small)
/// - u64::MAX-2: insufficient privilege
pub fn handle_llm_get_policy(suid_high: u64, suid_low: u64, out_ptr: u64, out_len: u64) -> u64 {
    let buffer_len = out_len as usize;
    if buffer_len == 0 || out_ptr == 0 {
        return u64::MAX;
    }

    let policy_suid = crate::semantic::SUID::new(suid_high, suid_low);

    // Validate this is a policy SUID
    if !crate::security::policy_suids::is_policy_suid(&policy_suid) {
        return u64::MAX;
    }

    let requester_id = crate::scheduler::current_task_index() as u8;

    unsafe {
        let registry = crate::semantic::registry::global_registry();

        if let Some(policy_obj) = registry.get(&policy_suid) {
            if let Some(policy_content) = policy_obj.content.as_bytes() {
                // Parse policy to check read permissions
                if let Ok(policy) = crate::security::policy::PolicyObject::deserialize(policy_content) {
                    // Check if requester can read this policy
                    // System policies are readable by admin/system only
                    // User policies are readable by owner + admin
                    let can_read = if crate::security::policy_suids::is_system_policy(&policy_suid) {
                        requester_id == crate::security::user_ids::ADMIN ||
                        requester_id == crate::security::user_ids::SYSTEM
                    } else {
                        requester_id == policy.owner ||
                        requester_id == crate::security::user_ids::ADMIN
                    };

                    if !can_read {
                        return u64::MAX - 2; // Insufficient privilege
                    }

                    // Copy policy data to output buffer
                    let copy_len = policy_content.len().min(buffer_len);
                    let out_buffer = core::slice::from_raw_parts_mut(out_ptr as *mut u8, copy_len);
                    out_buffer.copy_from_slice(&policy_content[..copy_len]);

                    copy_len as u64
                } else {
                    u64::MAX // Policy deserialization failed
                }
            } else {
                u64::MAX // Policy has no content
            }
        } else {
            0 // Policy not found
        }
    }
}

// ============================================================================
// Crypto Syscalls (60-69)
// ============================================================================

/// SYS_ENCRYPT(key_ptr, nonce_ptr, data_ptr, data_len) → 0 or u64::MAX
/// In-place ChaCha20 encryption. key=32 bytes, nonce=12 bytes.
fn handle_encrypt(key_ptr: u64, nonce_ptr: u64, data_ptr: u64, data_len: u64) -> u64 {
    let len = data_len as usize;
    if len == 0 || len > 4096 { return u64::MAX; }

    unsafe {
        let key_bytes = core::slice::from_raw_parts(key_ptr as *const u8, 32);
        let nonce_bytes = core::slice::from_raw_parts(nonce_ptr as *const u8, 12);
        let data = core::slice::from_raw_parts_mut(data_ptr as *mut u8, len);

        let mut key_arr = [0u8; 32];
        key_arr.copy_from_slice(key_bytes);
        let mut nonce_arr = [0u8; 12];
        nonce_arr.copy_from_slice(nonce_bytes);

        let key = crate::crypto::CryptoKey::from_bytes(key_arr);
        let nonce = crate::crypto::Nonce::from_bytes(nonce_arr);

        crate::crypto::chacha20::encrypt(&key, &nonce, data);
        0
    }
}

/// SYS_DECRYPT(key_ptr, nonce_ptr, data_ptr, data_len) → 0 or u64::MAX
/// In-place ChaCha20 decryption (same as encrypt — XOR cipher).
fn handle_decrypt(key_ptr: u64, nonce_ptr: u64, data_ptr: u64, data_len: u64) -> u64 {
    // ChaCha20 is a stream cipher — encrypt and decrypt are the same operation
    handle_encrypt(key_ptr, nonce_ptr, data_ptr, data_len)
}

/// SYS_HASH(data_ptr, data_len, out_ptr) → 0 or u64::MAX
/// SHA-256 hash. Writes 32-byte digest to out_ptr.
fn handle_hash(data_ptr: u64, data_len: u64, out_ptr: u64) -> u64 {
    let len = data_len as usize;
    if len == 0 || len > 65536 { return u64::MAX; }
    if out_ptr == 0 { return u64::MAX; }

    let data = unsafe {
        core::slice::from_raw_parts(data_ptr as *const u8, len)
    };

    let digest = crate::crypto::sha256::hash(data);
    unsafe {
        let dest = core::slice::from_raw_parts_mut(out_ptr as *mut u8, 32);
        dest.copy_from_slice(&digest);
    }
    0
}

/// Convenience function for userspace: write a string
pub fn write(s: &str) {
    dispatch(numbers::SYS_WRITE, s.as_ptr() as u64, s.len() as u64, 0, 0);
}

/// Convenience function for userspace: exit
pub fn exit(code: i32) -> ! {
    dispatch(numbers::SYS_EXIT, code as u64, 0, 0, 0);
    loop { core::hint::spin_loop(); }
}

/// Convenience function for userspace: yield
pub fn yield_now() {
    dispatch(numbers::SYS_YIELD, 0, 0, 0, 0);
}
