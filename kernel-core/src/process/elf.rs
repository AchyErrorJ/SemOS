//! ELF Loader for User Programs
//!
//! Parses and loads ELF64 binaries for user-mode execution.
//!
//! # Supported Features
//!
//! - ELF64 format (architecture-independent validation)
//! - Static executables
//! - PT_LOAD segments
//! - Position-independent executables (PIE)
//!
//! # Memory Layout
//!
//! User processes have this memory layout:
//! ```text
//! 0x0000_0000_0000_0000 - 0x0000_0000_0040_0000  (Reserved/unmapped)
//! 0x0000_0000_0040_0000 - 0x0000_0000_8000_0000  (Code + Data)
//! 0x0000_0000_8000_0000 - 0x0000_0000_C000_0000  (Heap)
//! 0x0000_FFFF_F000_0000 - 0x0000_FFFF_FFFF_0000  (Stack)
//! ```

use core::mem::size_of;

/// ELF magic number
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

/// ELF class (32 or 64 bit)
const ELFCLASS64: u8 = 2;

/// ELF data encoding
const ELFDATA2LSB: u8 = 1; // Little endian

/// ELF machine type for AArch64
pub const EM_AARCH64: u16 = 183;
/// ELF machine type for x86_64
pub const EM_X86_64: u16 = 62;

/// Expected machine type for this build.
#[cfg(target_arch = "aarch64")]
pub const EXPECTED_MACHINE: u16 = EM_AARCH64;
#[cfg(target_arch = "x86_64")]
pub const EXPECTED_MACHINE: u16 = EM_X86_64;
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub const EXPECTED_MACHINE: u16 = 0;

/// ELF type for executable
const ET_EXEC: u16 = 2;
/// ELF type for shared object (PIE)
const ET_DYN: u16 = 3;

/// Program header type: loadable segment
pub const PT_LOAD: u32 = 1;

/// Segment flags
pub const PF_X: u32 = 1; // Execute
pub const PF_W: u32 = 2; // Write
pub const PF_R: u32 = 4; // Read

/// ELF64 Header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Header {
    /// Magic number and identification
    pub e_ident: [u8; 16],
    /// Object file type
    pub e_type: u16,
    /// Machine type
    pub e_machine: u16,
    /// Object file version
    pub e_version: u32,
    /// Entry point address
    pub e_entry: u64,
    /// Program header table offset
    pub e_phoff: u64,
    /// Section header table offset
    pub e_shoff: u64,
    /// Processor-specific flags
    pub e_flags: u32,
    /// ELF header size
    pub e_ehsize: u16,
    /// Program header entry size
    pub e_phentsize: u16,
    /// Number of program headers
    pub e_phnum: u16,
    /// Section header entry size
    pub e_shentsize: u16,
    /// Number of section headers
    pub e_shnum: u16,
    /// Section name string table index
    pub e_shstrndx: u16,
}

/// ELF64 Program Header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Phdr {
    /// Segment type
    pub p_type: u32,
    /// Segment flags
    pub p_flags: u32,
    /// Offset in file
    pub p_offset: u64,
    /// Virtual address in memory
    pub p_vaddr: u64,
    /// Physical address (usually same as vaddr)
    pub p_paddr: u64,
    /// Size in file
    pub p_filesz: u64,
    /// Size in memory
    pub p_memsz: u64,
    /// Alignment
    pub p_align: u64,
}

/// Loaded ELF information
#[derive(Debug, Clone)]
pub struct ElfInfo {
    /// Entry point address
    pub entry: usize,
    /// Base address (for PIE)
    pub base: usize,
    /// End of loaded segments (start of heap)
    pub brk: usize,
    /// Stack top address
    pub stack_top: usize,
    /// Number of segments loaded
    pub num_segments: usize,
}

/// ELF loading error
#[derive(Debug, Clone, Copy)]
pub enum ElfError {
    /// Invalid ELF magic
    BadMagic,
    /// Wrong ELF class (not 64-bit)
    WrongClass,
    /// Wrong endianness
    WrongEndian,
    /// Wrong machine type (wrong architecture)
    WrongMachine,
    /// Unsupported ELF type
    UnsupportedType,
    /// Data too small
    TooSmall,
    /// Invalid program header
    BadProgramHeader,
    /// Segment overlaps kernel
    OverlapsKernel,
    /// Out of memory
    OutOfMemory,
}

/// Default base address for PIE executables
const PIE_BASE: usize = 0x0000_0000_0040_0000; // 4MB

/// Default user-space stack top.
///
/// Must be in the canonical lower half (bit 47 = 0, bits 48-63 = 0) so x86_64
/// will accept it without #GP, AND below the platform's user-space cap that
/// the page-table mapping helpers enforce (typically anything below
/// 0x0000_8000_0000_0000). 512 GiB - 64 KiB sits comfortably under both.
const STACK_TOP: usize = 0x0000_007F_FFFF_0000;

/// Default stack size
const STACK_SIZE: usize = 64 * 1024; // 64KB

/// Kernel space starts here (don't load user code here)
const KERNEL_SPACE: usize = 0xFFFF_0000_0000_0000;

/// Validate ELF header
fn validate_header(data: &[u8]) -> Result<&Elf64Header, ElfError> {
    if data.len() < size_of::<Elf64Header>() {
        return Err(ElfError::TooSmall);
    }

    // Safety: we checked the length
    let header = unsafe { &*(data.as_ptr() as *const Elf64Header) };

    // Check magic
    if header.e_ident[0..4] != ELF_MAGIC {
        return Err(ElfError::BadMagic);
    }

    // Check class (64-bit)
    if header.e_ident[4] != ELFCLASS64 {
        return Err(ElfError::WrongClass);
    }

    // Check endianness (little endian)
    if header.e_ident[5] != ELFDATA2LSB {
        return Err(ElfError::WrongEndian);
    }

    // Check machine type
    if header.e_machine != EXPECTED_MACHINE {
        return Err(ElfError::WrongMachine);
    }

    // Check type (executable or PIE)
    if header.e_type != ET_EXEC && header.e_type != ET_DYN {
        return Err(ElfError::UnsupportedType);
    }

    Ok(header)
}

/// Get program header at index
fn get_phdr<'a>(data: &'a [u8], header: &Elf64Header, index: u16) -> Result<&'a Elf64Phdr, ElfError> {
    let offset = header.e_phoff as usize + (index as usize) * (header.e_phentsize as usize);
    let end = offset + size_of::<Elf64Phdr>();

    if end > data.len() {
        return Err(ElfError::BadProgramHeader);
    }

    // Safety: we checked bounds
    Ok(unsafe { &*(data.as_ptr().add(offset) as *const Elf64Phdr) })
}

/// Load an ELF binary and return information needed for execution
pub fn load_elf(data: &[u8]) -> Option<ElfInfo> {
    let header = validate_header(data).ok()?;

    // Determine if this is PIE (position-independent)
    let is_pie = header.e_type == ET_DYN;
    let base = if is_pie { PIE_BASE } else { 0 };

    let mut max_addr: usize = 0;
    let mut num_segments = 0;

    // Process each loadable segment
    for i in 0..header.e_phnum {
        let phdr = get_phdr(data, header, i).ok()?;

        if phdr.p_type != PT_LOAD {
            continue;
        }

        let vaddr = (phdr.p_vaddr as usize).wrapping_add(base);
        let memsz = phdr.p_memsz as usize;
        let filesz = phdr.p_filesz as usize;
        let offset = phdr.p_offset as usize;

        // Validate addresses
        if vaddr >= KERNEL_SPACE {
            crate::platform::log("[elf] Segment overlaps kernel space\n");
            return None;
        }

        // Validate file data
        if offset + filesz > data.len() {
            crate::platform::log("[elf] Segment extends beyond file\n");
            return None;
        }

        // Track highest address
        let end_addr = vaddr + memsz;
        if end_addr > max_addr {
            max_addr = end_addr;
        }

        // In a real implementation, we would:
        // 1. Allocate pages for [vaddr, vaddr + memsz)
        // 2. Map them with appropriate permissions (R/W/X from p_flags)
        // 3. Copy data from [offset, offset + filesz)
        // 4. Zero the remaining bytes (BSS)
        //
        // For now, we just validate the ELF structure.
        // The actual memory mapping happens in the MMU setup.

        num_segments += 1;
    }

    if num_segments == 0 {
        crate::platform::log("[elf] No loadable segments\n");
        return None;
    }

    // Calculate entry point
    let entry = (header.e_entry as usize).wrapping_add(base);

    // Align break to page boundary
    let brk = (max_addr + 0xFFF) & !0xFFF;

    Some(ElfInfo {
        entry,
        base,
        brk,
        stack_top: STACK_TOP,
        num_segments,
    })
}

/// Load ELF segments into memory
/// This is called after page tables are set up
pub fn load_segments(data: &[u8], info: &ElfInfo) -> Result<(), ElfError> {
    let header = validate_header(data)?;

    for i in 0..header.e_phnum {
        let phdr = get_phdr(data, header, i)?;

        if phdr.p_type != PT_LOAD {
            continue;
        }

        let vaddr = (phdr.p_vaddr as usize).wrapping_add(info.base);
        let filesz = phdr.p_filesz as usize;
        let memsz = phdr.p_memsz as usize;
        let offset = phdr.p_offset as usize;

        // Copy file data to memory
        if filesz > 0 {
            let src = &data[offset..offset + filesz];
            let dst = unsafe { core::slice::from_raw_parts_mut(vaddr as *mut u8, filesz) };
            dst.copy_from_slice(src);
        }

        // Zero BSS (memsz > filesz)
        if memsz > filesz {
            let bss_start = vaddr + filesz;
            let bss_size = memsz - filesz;
            let bss = unsafe { core::slice::from_raw_parts_mut(bss_start as *mut u8, bss_size) };
            bss.fill(0);
        }
    }

    Ok(())
}

/// Create a minimal ELF header for testing.
/// Builds a tiny executable that just does an exit syscall — the simplest
/// possible Ring 3 program that proves spawn_from_elf works end to end.
/// The caller is responsible for storing the result in a `'static` location
/// (e.g. by populating a `static mut [u8; 256]` at boot time).
#[allow(dead_code)]
pub fn create_test_elf() -> [u8; 256] {
    let mut buf = [0u8; 256];

    // ELF header
    buf[0..4].copy_from_slice(&ELF_MAGIC);
    buf[4] = ELFCLASS64;
    buf[5] = ELFDATA2LSB;
    buf[6] = 1; // EV_CURRENT
    buf[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
    buf[18..20].copy_from_slice(&EXPECTED_MACHINE.to_le_bytes());
    buf[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
    buf[24..32].copy_from_slice(&0x400078u64.to_le_bytes()); // e_entry
    buf[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
    buf[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize
    buf[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
    buf[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum

    // Program header (at offset 64)
    let ph_offset = 64;
    buf[ph_offset..ph_offset + 4].copy_from_slice(&PT_LOAD.to_le_bytes()); // p_type
    buf[ph_offset + 4..ph_offset + 8].copy_from_slice(&(PF_R | PF_X).to_le_bytes()); // p_flags
    buf[ph_offset + 8..ph_offset + 16].copy_from_slice(&0u64.to_le_bytes()); // p_offset
    buf[ph_offset + 16..ph_offset + 24].copy_from_slice(&0x400000u64.to_le_bytes()); // p_vaddr
    buf[ph_offset + 24..ph_offset + 32].copy_from_slice(&0x400000u64.to_le_bytes()); // p_paddr
    buf[ph_offset + 32..ph_offset + 40].copy_from_slice(&256u64.to_le_bytes()); // p_filesz
    buf[ph_offset + 40..ph_offset + 48].copy_from_slice(&256u64.to_le_bytes()); // p_memsz
    buf[ph_offset + 48..ph_offset + 56].copy_from_slice(&0x1000u64.to_le_bytes()); // p_align

    // Code at offset 0x78 (entry = 0x400078, base = 0x400000)
    let code_offset = 0x78;
    #[cfg(target_arch = "x86_64")]
    {
        // x86_64: xor edi,edi; mov eax,2; syscall  (SYS_EXIT with code 0)
        buf[code_offset] = 0x31; buf[code_offset + 1] = 0xFF;         // xor edi, edi
        buf[code_offset + 2] = 0xB8;                                   // mov eax, imm32
        buf[code_offset + 3..code_offset + 7].copy_from_slice(&2u32.to_le_bytes()); // SYS_EXIT=2
        buf[code_offset + 7] = 0x0F; buf[code_offset + 8] = 0x05;     // syscall
    }
    #[cfg(target_arch = "aarch64")]
    {
        // ARM64: mov x0, #0; mov x8, #2; svc #0  (SYS_EXIT with code 0)
        buf[code_offset..code_offset + 4].copy_from_slice(&0xd2800000u32.to_le_bytes());
        buf[code_offset + 4..code_offset + 8].copy_from_slice(&0xd2800048u32.to_le_bytes());
        buf[code_offset + 8..code_offset + 12].copy_from_slice(&0xd4000001u32.to_le_bytes());
    }

    buf
}

/// Create a slightly less trivial ELF: SYS_WRITE("Hello from Ring 3...\n")
/// followed by SYS_EXIT(0). Uses a string literal in the same PT_LOAD
/// segment as the code (after the syscall sequence) — proves that user
/// programs with embedded data work, which is the prerequisite for
/// anything more complex (semantic-object demos, an LLM redaction demo,
/// any real userspace program).
///
/// File layout (single 256-byte PT_LOAD at vaddr 0x400000, R+X):
///   0x00-0x3F: ELF header
///   0x40-0x77: program header + padding
///   0x78:      code (entry point) — 31 bytes
///   0xD0:      string ("Hello from Ring 3 ELF binary!\n", 30 bytes)
///   ...:       trailing padding
#[allow(dead_code)]
pub fn create_hello_elf() -> [u8; 256] {
    let mut buf = [0u8; 256];

    // ---- ELF header ----
    buf[0..4].copy_from_slice(&ELF_MAGIC);
    buf[4] = ELFCLASS64;
    buf[5] = ELFDATA2LSB;
    buf[6] = 1; // EV_CURRENT
    buf[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
    buf[18..20].copy_from_slice(&EXPECTED_MACHINE.to_le_bytes());
    buf[20..24].copy_from_slice(&1u32.to_le_bytes());        // e_version
    buf[24..32].copy_from_slice(&0x400078u64.to_le_bytes()); // e_entry
    buf[32..40].copy_from_slice(&64u64.to_le_bytes());       // e_phoff
    buf[52..54].copy_from_slice(&64u16.to_le_bytes());       // e_ehsize
    buf[54..56].copy_from_slice(&56u16.to_le_bytes());       // e_phentsize
    buf[56..58].copy_from_slice(&1u16.to_le_bytes());        // e_phnum

    // ---- Program header (offset 64) ----
    let ph = 64;
    buf[ph     ..ph +  4].copy_from_slice(&PT_LOAD.to_le_bytes());
    buf[ph +  4..ph +  8].copy_from_slice(&(PF_R | PF_X).to_le_bytes());
    buf[ph +  8..ph + 16].copy_from_slice(&0u64.to_le_bytes());        // p_offset
    buf[ph + 16..ph + 24].copy_from_slice(&0x400000u64.to_le_bytes()); // p_vaddr
    buf[ph + 24..ph + 32].copy_from_slice(&0x400000u64.to_le_bytes()); // p_paddr
    buf[ph + 32..ph + 40].copy_from_slice(&256u64.to_le_bytes());      // p_filesz
    buf[ph + 40..ph + 48].copy_from_slice(&256u64.to_le_bytes());      // p_memsz
    buf[ph + 48..ph + 56].copy_from_slice(&0x1000u64.to_le_bytes());   // p_align

    // ---- Embedded string at file offset 0xD0 / vaddr 0x4000D0 ----
    let msg: &[u8] = b"Hello from Ring 3 ELF binary!\n";
    const STR_OFFSET: usize = 0xD0;
    const STR_VADDR: u64 = 0x400000 + STR_OFFSET as u64;
    let str_len = msg.len() as u32;
    buf[STR_OFFSET..STR_OFFSET + msg.len()].copy_from_slice(msg);

    // ---- Code at file offset 0x78 / vaddr 0x400078 ----
    // Sequence (31 bytes total):
    //   48 BF <8B imm>           mov rdi, STR_VADDR     ; arg0 = buf_ptr
    //   BE <4B imm>              mov esi, str_len       ; arg1 = buf_len
    //   B8 00 00 00 00           mov eax, 0             ; SYS_WRITE
    //   0F 05                    syscall
    //   31 FF                    xor edi, edi           ; arg0 = 0
    //   B8 02 00 00 00           mov eax, 2             ; SYS_EXIT
    //   0F 05                    syscall
    #[cfg(target_arch = "x86_64")]
    {
        let mut p = 0x78;
        // mov rdi, STR_VADDR (REX.W 0xBF + 8-byte imm)
        buf[p] = 0x48; buf[p + 1] = 0xBF; p += 2;
        buf[p..p + 8].copy_from_slice(&STR_VADDR.to_le_bytes()); p += 8;
        // mov esi, str_len  (0xBE + 4-byte imm; clears high 32 bits of rsi)
        buf[p] = 0xBE; p += 1;
        buf[p..p + 4].copy_from_slice(&str_len.to_le_bytes()); p += 4;
        // mov eax, 0 (SYS_WRITE)
        buf[p] = 0xB8; p += 1;
        buf[p..p + 4].copy_from_slice(&0u32.to_le_bytes()); p += 4;
        // syscall
        buf[p] = 0x0F; buf[p + 1] = 0x05; p += 2;
        // xor edi, edi
        buf[p] = 0x31; buf[p + 1] = 0xFF; p += 2;
        // mov eax, 2 (SYS_EXIT)
        buf[p] = 0xB8; p += 1;
        buf[p..p + 4].copy_from_slice(&2u32.to_le_bytes()); p += 4;
        // syscall
        buf[p] = 0x0F; buf[p + 1] = 0x05;
        let _ = p;
    }

    // No aarch64 implementation — this stretch demo is x86_64-only.
    buf
}

/// Get human-readable segment flags
pub fn flags_str(flags: u32) -> &'static str {
    match (flags & PF_R != 0, flags & PF_W != 0, flags & PF_X != 0) {
        (true, false, false) => "R--",
        (true, true, false) => "RW-",
        (true, false, true) => "R-X",
        (true, true, true) => "RWX",
        (false, false, true) => "--X",
        (false, true, false) => "-W-",
        (false, true, true) => "-WX",
        (false, false, false) => "---",
    }
}

/// Print ELF information for debugging
pub fn print_elf_info(data: &[u8]) {
    let header = match validate_header(data) {
        Ok(h) => h,
        Err(e) => {
            crate::platform::log("[elf] Invalid ELF: ");
            match e {
                ElfError::BadMagic => crate::platform::log("bad magic\n"),
                ElfError::WrongClass => crate::platform::log("not 64-bit\n"),
                ElfError::WrongEndian => crate::platform::log("not little-endian\n"),
                ElfError::WrongMachine => crate::platform::log("wrong architecture\n"),
                ElfError::UnsupportedType => crate::platform::log("unsupported type\n"),
                ElfError::TooSmall => crate::platform::log("data too small\n"),
                _ => crate::platform::log("unknown error\n"),
            }
            return;
        }
    };

    crate::platform::log("[elf] ELF64 ");
    if header.e_type == ET_EXEC {
        crate::platform::log("executable");
    } else if header.e_type == ET_DYN {
        crate::platform::log("PIE/shared object");
    }
    crate::platform::log("\n");

    crate::platform::log("[elf] Entry: 0x");
    crate::platform::log_num(header.e_entry);
    crate::platform::log("\n");

    crate::platform::log("[elf] Program headers: ");
    crate::platform::log_num(header.e_phnum as u64);
    crate::platform::log("\n");

    for i in 0..header.e_phnum {
        if let Ok(phdr) = get_phdr(data, header, i) {
            if phdr.p_type == PT_LOAD {
                crate::platform::log("  LOAD: 0x");
                crate::platform::log_num(phdr.p_vaddr);
                crate::platform::log(" (");
                crate::platform::log_num(phdr.p_memsz);
                crate::platform::log(" bytes) ");
                crate::platform::log(flags_str(phdr.p_flags));
                crate::platform::log("\n");
            }
        }
    }
}
