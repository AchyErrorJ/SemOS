//! Kernel Pipe Implementation
//!
//! Unidirectional byte stream between two endpoints (read end, write end).
//! Uses a fixed-size ring buffer. Blocking semantics:
//!
//! - **Read from empty pipe**: blocks the caller until data arrives or
//!   the write end is closed (EOF).
//! - **Write to full pipe**: blocks the caller until space is available
//!   or the read end is closed (broken pipe).
//!
//! Blocking is cooperative: the syscall handler sets the task to
//! `TaskState::Blocked` with a `BlockReason`, and `pick_next()` in the
//! scheduler checks whether the condition has cleared.

/// Pipe identifier (index into the global pipe table).
pub type PipeId = usize;

/// Ring buffer capacity per pipe.
const PIPE_BUF_SIZE: usize = 4096;

/// Maximum number of simultaneous pipes.
const MAX_PIPES: usize = 32;

/// A single kernel pipe.
struct Pipe {
    buf: [u8; PIPE_BUF_SIZE],
    /// Next position to read from.
    read_pos: usize,
    /// Next position to write to.
    write_pos: usize,
    /// Number of bytes currently in the buffer.
    count: usize,
    /// Open file descriptors referencing the read end (FDs are duplicated by
    /// dup/dup2 and by FD-table inheritance on spawn). The read end is "closed"
    /// when this hits 0.
    readers: u32,
    /// Open file descriptors referencing the write end.
    writers: u32,
    /// Is this pipe slot allocated?
    active: bool,
}

impl Pipe {
    const fn empty() -> Self {
        Self {
            buf: [0; PIPE_BUF_SIZE],
            read_pos: 0,
            write_pos: 0,
            count: 0,
            readers: 0,
            writers: 0,
            active: false,
        }
    }

    /// Bytes available to read.
    fn available(&self) -> usize {
        self.count
    }

    /// Free space available for writing.
    fn free_space(&self) -> usize {
        PIPE_BUF_SIZE - self.count
    }

    /// Read up to `dst.len()` bytes. Returns number of bytes read.
    fn read(&mut self, dst: &mut [u8]) -> usize {
        let n = dst.len().min(self.count);
        for i in 0..n {
            dst[i] = self.buf[self.read_pos];
            self.read_pos = (self.read_pos + 1) % PIPE_BUF_SIZE;
        }
        self.count -= n;
        n
    }

    /// Write up to `src.len()` bytes. Returns number of bytes written.
    fn write(&mut self, src: &[u8]) -> usize {
        let n = src.len().min(self.free_space());
        for i in 0..n {
            self.buf[self.write_pos] = src[i];
            self.write_pos = (self.write_pos + 1) % PIPE_BUF_SIZE;
        }
        self.count += n;
        n
    }
}

/// Global pipe table. Single-core so we use static mut (same pattern as scheduler).
static mut PIPES: [Pipe; MAX_PIPES] = {
    const EMPTY: Pipe = Pipe::empty();
    [EMPTY; MAX_PIPES]
};

/// Create a new pipe. Returns the pipe ID.
pub fn create_pipe() -> Option<PipeId> {
    unsafe {
        let pipes = &raw mut PIPES;
        for i in 0..MAX_PIPES {
            if !(*pipes)[i].active {
                (*pipes)[i] = Pipe {
                    buf: [0; PIPE_BUF_SIZE],
                    read_pos: 0,
                    write_pos: 0,
                    count: 0,
                    readers: 1,
                    writers: 1,
                    active: true,
                };
                return Some(i);
            }
        }
    }
    None
}

/// Read from a pipe. Returns the number of bytes read, or 0 for EOF.
///
/// Returns `None` if the pipe should block (empty buffer, write end still open).
/// The caller should set the task to Blocked and retry later.
pub fn pipe_read(id: PipeId, dst: &mut [u8]) -> Option<usize> {
    unsafe {
        let pipes = &raw mut PIPES;
        if id >= MAX_PIPES || !(*pipes)[id].active {
            return Some(0);
        }
        let pipe = &mut (*pipes)[id];

        if pipe.count > 0 {
            Some(pipe.read(dst))
        } else if pipe.writers == 0 {
            Some(0) // EOF — no writers left
        } else {
            None // Should block
        }
    }
}

/// Write to a pipe. Returns the number of bytes written.
///
/// Returns `None` if the pipe should block (buffer full, read end still open).
/// Returns `Some(0)` if the read end is closed (broken pipe).
pub fn pipe_write(id: PipeId, src: &[u8]) -> Option<usize> {
    unsafe {
        let pipes = &raw mut PIPES;
        if id >= MAX_PIPES || !(*pipes)[id].active {
            return Some(0);
        }
        let pipe = &mut (*pipes)[id];

        if pipe.readers == 0 {
            return Some(0); // Broken pipe — no readers left
        }

        if pipe.free_space() > 0 {
            Some(pipe.write(src))
        } else {
            None // Should block
        }
    }
}

/// Increment the reader count (a read-end FD was duplicated).
pub fn dup_read_end(id: PipeId) {
    unsafe {
        let pipes = &raw mut PIPES;
        if id < MAX_PIPES && (*pipes)[id].active {
            (*pipes)[id].readers += 1;
        }
    }
}

/// Increment the writer count (a write-end FD was duplicated).
pub fn dup_write_end(id: PipeId) {
    unsafe {
        let pipes = &raw mut PIPES;
        if id < MAX_PIPES && (*pipes)[id].active {
            (*pipes)[id].writers += 1;
        }
    }
}

/// Close one reference to the read end. The end is "closed" (EOF to writers)
/// only when the last reader FD goes away; the slot is freed when both ends
/// have no references.
pub fn close_read_end(id: PipeId) {
    unsafe {
        let pipes = &raw mut PIPES;
        if id < MAX_PIPES && (*pipes)[id].active {
            let p = &mut (*pipes)[id];
            p.readers = p.readers.saturating_sub(1);
            if p.readers == 0 && p.writers == 0 {
                p.active = false;
            }
        }
    }
}

/// Close one reference to the write end (last writer → EOF to readers).
pub fn close_write_end(id: PipeId) {
    unsafe {
        let pipes = &raw mut PIPES;
        if id < MAX_PIPES && (*pipes)[id].active {
            let p = &mut (*pipes)[id];
            p.writers = p.writers.saturating_sub(1);
            if p.readers == 0 && p.writers == 0 {
                p.active = false;
            }
        }
    }
}

/// Check if a pipe has data available to read (used by scheduler to unblock).
pub fn has_data(id: PipeId) -> bool {
    unsafe {
        let pipes = &raw const PIPES;
        if id < MAX_PIPES && (*pipes)[id].active {
            (*pipes)[id].count > 0 || (*pipes)[id].writers == 0
        } else {
            true // Pipe gone → unblock so reader gets EOF/error
        }
    }
}

/// Check if a pipe has space available to write (used by scheduler to unblock).
pub fn has_space(id: PipeId) -> bool {
    unsafe {
        let pipes = &raw const PIPES;
        if id < MAX_PIPES && (*pipes)[id].active {
            (*pipes)[id].free_space() > 0 || (*pipes)[id].readers == 0
        } else {
            true // Pipe gone → unblock so writer gets broken-pipe
        }
    }
}
