//! `std::process`-shaped: exit, abort, and `Command`/`Child`/`ExitStatus`.
//!
//! `Command` (M25 Tier 2) wraps SYS_SPAWN + SYS_WAIT. A Ring-3 parent can
//! now spawn a `/bin/<name>` child and block for its exit code: SYS_WAIT
//! resolves the child's scheduler slot and joins on it (the kernel's
//! PROCESS_TABLE Zombie path doesn't fire for Ring-3 children — see the
//! SYS_WAIT note in `kernel-core/src/syscall/mod.rs`).
//!
//! Not yet: piped stdio (`Stdio`, captured `Output.stdout/stderr`) — that
//! needs the TTY/pipe work in Phase 13. `output()` is therefore omitted;
//! use `status()` / `spawn()` + `Child::wait()`.

use crate::arch::{SYS_EXIT, SYS_SPAWN, SYS_WAIT, syscall1, syscall4};
use crate::io;
use core_alloc::string::String;
use core_alloc::vec::Vec;

/// Terminate the current process with the given exit code. The kernel
/// transitions the task to Exited and yields to the scheduler.
pub fn exit(code: i32) -> ! {
    unsafe {
        let _ = syscall1(SYS_EXIT, code as u64);
    }
    // SYS_EXIT shouldn't return — but if it does, halt defensively.
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// Abnormal termination — wraps `exit(101)` to match std's convention.
/// Used by the panic handler.
pub fn abort() -> ! {
    exit(101)
}

/// Mirrors the kernel's `#[repr(C)] SpawnArgs` (syscall/mod.rs). Passed by
/// pointer in SYS_SPAWN's 4th argument; the kernel reads argv/envp blobs
/// from the pointed-to layout while the caller's CR3 is still active.
#[repr(C)]
struct SpawnArgs {
    argv_blob_ptr: u64,
    argv_blob_len: u32,
    envp_blob_ptr: u64,
    envp_blob_len: u32,
}

/// Build the kernel's argv blob: `[count: u32][len: u32][bytes]...`.
/// Items are NOT null-terminated; the kernel adds terminators when it
/// writes the SysV layout onto the child's stack.
fn build_blob(items: &[&str]) -> Vec<u8> {
    let mut blob = Vec::new();
    blob.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for it in items {
        blob.extend_from_slice(&(it.len() as u32).to_le_bytes());
        blob.extend_from_slice(it.as_bytes());
    }
    blob
}

/// The exit status of a finished child. Wraps the raw exit code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExitStatus {
    code: i32,
}

impl ExitStatus {
    /// True iff the child exited with code 0.
    pub fn success(&self) -> bool {
        self.code == 0
    }
    /// The exit code. Always `Some` here — we don't model signals.
    pub fn code(&self) -> Option<i32> {
        Some(self.code)
    }
}

/// A spawned child process. `wait()` blocks for its exit status.
pub struct Child {
    pid: u64,
}

impl Child {
    /// The child's PID (as returned by SYS_SPAWN).
    pub fn id(&self) -> u64 {
        self.pid
    }

    /// Block until the child exits; return its status. Idempotent-ish:
    /// once the child has exited, repeated calls return the same code.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        let r = unsafe { syscall1(SYS_WAIT, self.pid) };
        if r == u64::MAX {
            return Err(io::Error::other());
        }
        Ok(ExitStatus { code: r as i32 })
    }
}

/// Builder mirroring `std::process::Command`. `program` is a spawnable
/// path the kernel understands today — i.e. `/bin/<name>` for a ramfs
/// ELF (see the `/bin` table in `handle_spawn`).
pub struct Command {
    program: String,
    args: Vec<String>,
}

impl Command {
    pub fn new(program: &str) -> Self {
        Self {
            program: String::from(program),
            args: Vec::new(),
        }
    }

    /// Append a single argument.
    pub fn arg(&mut self, arg: &str) -> &mut Self {
        self.args.push(String::from(arg));
        self
    }

    /// Append several arguments.
    pub fn args<'a, I: IntoIterator<Item = &'a str>>(&mut self, args: I) -> &mut Self {
        for a in args {
            self.args.push(String::from(a));
        }
        self
    }

    /// Spawn the child without waiting. argv[0] is the program path,
    /// matching std/SysV convention.
    pub fn spawn(&mut self) -> io::Result<Child> {
        let path = self.program.as_bytes();

        // No args → take the simple, proven no-blob path (arg3 = 0).
        if self.args.is_empty() {
            let pid = unsafe {
                syscall4(SYS_SPAWN, path.as_ptr() as u64, path.len() as u64, 3, 0)
            };
            return if pid == u64::MAX {
                Err(io::Error::other())
            } else {
                Ok(Child { pid })
            };
        }

        // With args: build argv = [program, args...] and pass a SpawnArgs.
        let mut items: Vec<&str> = Vec::with_capacity(self.args.len() + 1);
        items.push(self.program.as_str());
        for a in &self.args {
            items.push(a.as_str());
        }
        let blob = build_blob(&items);
        let spawn_args = SpawnArgs {
            argv_blob_ptr: blob.as_ptr() as u64,
            argv_blob_len: blob.len() as u32,
            envp_blob_ptr: 0,
            envp_blob_len: 0,
        };
        let pid = unsafe {
            syscall4(
                SYS_SPAWN,
                path.as_ptr() as u64,
                path.len() as u64,
                3, // request max tier; kernel clamps to the caller's
                &spawn_args as *const SpawnArgs as u64,
            )
        };
        // `blob` and `spawn_args` stay alive until here — the kernel reads
        // them synchronously inside the SYS_SPAWN call above.
        if pid == u64::MAX {
            Err(io::Error::other())
        } else {
            Ok(Child { pid })
        }
    }

    /// Spawn and wait, returning the exit status. The common case.
    pub fn status(&mut self) -> io::Result<ExitStatus> {
        let mut child = self.spawn()?;
        child.wait()
    }
}
