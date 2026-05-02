//! RAM Filesystem Implementation
//!
//! Simple in-memory filesystem with static file entries.
//! Supports read-only access to embedded files.

use core::str;

/// Maximum number of files in the filesystem
pub const MAX_FILES: usize = 32;

/// Maximum filename length
pub const MAX_FILENAME: usize = 32;

/// File type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    /// Regular file with content
    Regular,
    /// Directory (not fully implemented)
    Directory,
    /// Executable program
    Executable,
}

/// A file entry in the ramfs
#[derive(Clone, Copy)]
pub struct RamfsFile {
    /// Filename (null-terminated within the array)
    name: [u8; MAX_FILENAME],
    /// Length of the name
    name_len: usize,
    /// File type
    file_type: FileType,
    /// Pointer to file data
    data: *const u8,
    /// Size of file data
    size: usize,
}

impl RamfsFile {
    /// Create an empty file entry
    pub const fn empty() -> Self {
        Self {
            name: [0; MAX_FILENAME],
            name_len: 0,
            file_type: FileType::Regular,
            data: core::ptr::null(),
            size: 0,
        }
    }

    /// Create a new file entry from static data
    ///
    /// # Safety
    /// The data pointer must remain valid for the lifetime of the filesystem.
    pub const fn new(name: &'static str, file_type: FileType, data: &'static [u8]) -> Self {
        let name_bytes = name.as_bytes();
        let mut name_arr = [0u8; MAX_FILENAME];
        let mut i = 0;
        while i < name_bytes.len() && i < MAX_FILENAME {
            name_arr[i] = name_bytes[i];
            i += 1;
        }
        Self {
            name: name_arr,
            name_len: name_bytes.len(),
            file_type,
            data: data.as_ptr(),
            size: data.len(),
        }
    }

    /// Check if this entry is valid (has a name)
    pub fn is_valid(&self) -> bool {
        self.name_len > 0
    }

    /// Get the filename as a string slice
    pub fn name(&self) -> &str {
        // Safety: name is always valid UTF-8 (ASCII subset)
        unsafe { str::from_utf8_unchecked(&self.name[..self.name_len]) }
    }

    /// Get the file type
    pub fn file_type(&self) -> FileType {
        self.file_type
    }

    /// Get the file size
    pub fn size(&self) -> usize {
        self.size
    }

    /// Get the file data as a byte slice
    pub fn data(&self) -> &[u8] {
        if self.data.is_null() || self.size == 0 {
            &[]
        } else {
            // Safety: data pointer is valid for 'static lifetime
            unsafe { core::slice::from_raw_parts(self.data, self.size) }
        }
    }

    /// Check if name matches
    pub fn matches(&self, name: &str) -> bool {
        self.name() == name
    }
}

/// The RAM filesystem
pub struct Ramfs {
    /// File entries
    files: [RamfsFile; MAX_FILES],
    /// Number of files
    count: usize,
}

impl Ramfs {
    /// Create a new empty filesystem
    pub const fn new() -> Self {
        Self {
            files: [RamfsFile::empty(); MAX_FILES],
            count: 0,
        }
    }

    /// Add a file to the filesystem
    ///
    /// Returns true on success, false if filesystem is full.
    pub fn add_file(&mut self, file: RamfsFile) -> bool {
        if self.count >= MAX_FILES {
            return false;
        }
        self.files[self.count] = file;
        self.count += 1;
        true
    }

    /// Add a file from static data
    pub fn add(&mut self, name: &'static str, file_type: FileType, data: &'static [u8]) -> bool {
        self.add_file(RamfsFile::new(name, file_type, data))
    }

    /// Find a file by name
    pub fn find(&self, name: &str) -> Option<&RamfsFile> {
        for i in 0..self.count {
            if self.files[i].matches(name) {
                return Some(&self.files[i]);
            }
        }
        None
    }

    /// Get file at index
    pub fn get(&self, index: usize) -> Option<&RamfsFile> {
        if index < self.count {
            Some(&self.files[index])
        } else {
            None
        }
    }

    /// Get number of files
    pub fn len(&self) -> usize {
        self.count
    }

    /// Check if filesystem is empty
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Iterate over all files
    pub fn iter(&self) -> RamfsIter {
        RamfsIter {
            fs: self,
            index: 0,
        }
    }

    /// Print filesystem contents
    pub fn print_listing(&self) {

        crate::platform::log("=== Ramfs Contents ===\n");
        crate::platform::log("  Files: ");
        crate::platform::log_num(self.count as u64);
        crate::platform::log("\n\n");

        for file in self.iter() {
            crate::platform::log("  ");
            match file.file_type() {
                FileType::Regular => crate::platform::log("[F] "),
                FileType::Directory => crate::platform::log("[D] "),
                FileType::Executable => crate::platform::log("[X] "),
            }
            crate::platform::log(file.name());
            crate::platform::log("  (");
            crate::platform::log_num(file.size() as u64);
            crate::platform::log(" bytes)\n");
        }
        crate::platform::log("======================\n");
    }
}

/// Iterator over ramfs files
pub struct RamfsIter<'a> {
    fs: &'a Ramfs,
    index: usize,
}

impl<'a> Iterator for RamfsIter<'a> {
    type Item = &'a RamfsFile;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.fs.count {
            let file = &self.fs.files[self.index];
            self.index += 1;
            Some(file)
        } else {
            None
        }
    }
}

// ============================================================================
// File Descriptor Management
// ============================================================================

/// Maximum number of open file descriptors per process
pub const MAX_FDS: usize = 16;

/// An open file descriptor
#[derive(Clone, Copy)]
pub struct FileDescriptor {
    /// Index into ramfs files array (or special value)
    file_index: usize,
    /// Current read position
    position: usize,
    /// Is this descriptor in use?
    in_use: bool,
}

impl FileDescriptor {
    pub const fn empty() -> Self {
        Self {
            file_index: 0,
            position: 0,
            in_use: false,
        }
    }
}

/// File descriptor table (one per process, but we use global for now)
pub struct FdTable {
    fds: [FileDescriptor; MAX_FDS],
}

impl FdTable {
    pub const fn new() -> Self {
        Self {
            fds: [FileDescriptor::empty(); MAX_FDS],
        }
    }

    /// Open a file, returning a file descriptor
    pub fn open(&mut self, fs: &Ramfs, name: &str) -> Option<usize> {
        // Find the file
        let file_index = fs.iter().position(|f| f.matches(name))?;

        // Find a free fd slot (skip 0, 1, 2 for stdin/stdout/stderr)
        for fd in 3..MAX_FDS {
            if !self.fds[fd].in_use {
                self.fds[fd] = FileDescriptor {
                    file_index,
                    position: 0,
                    in_use: true,
                };
                return Some(fd);
            }
        }
        None
    }

    /// Close a file descriptor
    pub fn close(&mut self, fd: usize) -> bool {
        if fd < MAX_FDS && self.fds[fd].in_use {
            self.fds[fd].in_use = false;
            true
        } else {
            false
        }
    }

    /// Read from a file descriptor
    pub fn read(&mut self, fs: &Ramfs, fd: usize, buf: &mut [u8]) -> Option<usize> {
        if fd >= MAX_FDS || !self.fds[fd].in_use {
            return None;
        }

        let file = fs.get(self.fds[fd].file_index)?;
        let data = file.data();
        let pos = self.fds[fd].position;

        if pos >= data.len() {
            return Some(0); // EOF
        }

        let remaining = data.len() - pos;
        let to_read = buf.len().min(remaining);

        buf[..to_read].copy_from_slice(&data[pos..pos + to_read]);
        self.fds[fd].position += to_read;

        Some(to_read)
    }

    /// Get file size
    pub fn size(&self, fs: &Ramfs, fd: usize) -> Option<usize> {
        if fd >= MAX_FDS || !self.fds[fd].in_use {
            return None;
        }
        fs.get(self.fds[fd].file_index).map(|f| f.size())
    }

    /// Seek to position
    pub fn seek(&mut self, fd: usize, pos: usize) -> bool {
        if fd < MAX_FDS && self.fds[fd].in_use {
            self.fds[fd].position = pos;
            true
        } else {
            false
        }
    }

    /// Get current position
    pub fn tell(&self, fd: usize) -> Option<usize> {
        if fd < MAX_FDS && self.fds[fd].in_use {
            Some(self.fds[fd].position)
        } else {
            None
        }
    }
}

// ============================================================================
// Global Filesystem Instance
// ============================================================================

/// Global ramfs instance
static mut RAMFS: Option<Ramfs> = None;

/// Global file descriptor table
static mut FD_TABLE: Option<FdTable> = None;

/// Backing buffer for the built-in test.elf binary. Filled by [`init`] from
/// `crate::process::elf::create_test_elf()`. Size matches that helper's
/// return type. Static mut is fine here because we only write once at boot
/// and ramfs subsequently exposes it as `&'static [u8]`.
static mut TEST_ELF_BUF: [u8; 256] = [0; 256];

/// Initialize the global filesystem
pub fn init() {

    crate::platform::log("[*] Initializing ramfs...\n");

    unsafe {
        RAMFS = Some(Ramfs::new());
        FD_TABLE = Some(FdTable::new());

        // Populate the test ELF backing buffer once, then register it as a
        // ramfs file. `&*(&raw const TEST_ELF_BUF)` extends the borrow to
        // 'static (sound because we never mutate the buffer again).
        let elf = crate::process::elf::create_test_elf();
        TEST_ELF_BUF = elf;
        let test_elf_static: &'static [u8] = {
            let p = &raw const TEST_ELF_BUF;
            &(*p)
        };

        // Add some built-in files
        if let Some(ref mut fs) = RAMFS {
            // Add a welcome message
            fs.add(
                "welcome.txt",
                FileType::Regular,
                b"Welcome to Semantic OS!\n\nThis is a minimal ARM64 kernel with:\n- Preemptive multitasking\n- Security-tiered memory\n- EL0/EL1 privilege separation\n- Simple shell interface\n\nType 'help' for available commands.\n",
            );

            // Add help text
            fs.add(
                "help.txt",
                FileType::Regular,
                b"Available commands:\n  help    - Show this help\n  ls      - List files\n  cat     - Display file contents\n  run     - Execute a program\n  ps      - Show running tasks\n  mem     - Show memory info\n  clear   - Clear screen\n  exit    - Exit shell\n",
            );

            // Add the built-in test ELF — a 256-byte Ring 3 binary that just
            // calls SYS_EXIT(0). spawn_from_elf("test.elf", ...) loads it.
            fs.add("test.elf", FileType::Executable, test_elf_static);

            crate::platform::log("    Added built-in files\n");
        }
    }

    crate::platform::log("[OK] Ramfs initialized\n");
}

/// Get reference to global filesystem
pub fn get_fs() -> Option<&'static Ramfs> {
    unsafe { RAMFS.as_ref() }
}

/// Get mutable reference to global filesystem
pub fn get_fs_mut() -> Option<&'static mut Ramfs> {
    unsafe { RAMFS.as_mut() }
}

/// Get reference to global fd table
pub fn get_fd_table() -> Option<&'static FdTable> {
    unsafe { FD_TABLE.as_ref() }
}

/// Get mutable reference to global fd table
pub fn get_fd_table_mut() -> Option<&'static mut FdTable> {
    unsafe { FD_TABLE.as_mut() }
}
