//! `kernel_core::Platform` implementation for aarch64.
//!
//! This is the HAL seam: everything architecture-specific that kernel-core
//! needs (serial output, ticks, halt, interrupt control, memory management)
//! is routed here.

use kernel_core::Platform;

/// aarch64 platform singleton.
pub struct Aarch64Platform;

impl Platform for Aarch64Platform {
    fn serial_write(&self, s: &str) {
        crate::serial::uart_str(s);
    }

    fn ticks(&self) -> u64 {
        crate::get_ticks()
    }

    fn halt(&self) {
        unsafe { core::arch::asm!("wfe") };
    }

    fn enable_interrupts(&self) {
        unsafe { core::arch::asm!("msr daifclr, #2") };
    }

    fn schedule(&self) {
        crate::context::schedule();
    }

    // ---- Physical frames ----------------------------------------------------

    fn alloc_frame(&self, _tier: u8) -> Option<u64> {
        unsafe { crate::memory::alloc() }
    }

    fn free_frame(&self, addr: u64) -> bool {
        unsafe { crate::memory::free(addr) }
    }

    // ---- Address spaces -----------------------------------------------------

    fn create_address_space(&self, max_tier: u8) -> Option<u64> {
        unsafe {
            let space = crate::mmu::new_address_space(max_tier)?;
            let ttbr0 = space.ttbr0;
            crate::mmu::store_address_space(space);
            Some(ttbr0)
        }
    }

    fn reclaim_address_spaces(&self) -> usize {
        crate::mmu::reclaim_dead_address_spaces()
    }

    fn destroy_address_space(&self, space: u64) {
        unsafe { crate::mmu::destroy_address_space(space) }
    }

    fn map_elf_segment(
        &self,
        space: u64,
        virt_addr: u64,
        data: &[u8],
        memsz: usize,
        executable: bool,
        writable: bool,
    ) -> bool {
        unsafe { crate::mmu::map_elf_segment(space, virt_addr, data, memsz, executable, writable) }
    }

    fn map_user_stack(&self, space: u64, stack_top: u64, stack_size: u64) -> Option<u64> {
        unsafe { crate::mmu::map_user_stack(space, stack_top, stack_size) }
    }

    fn map_user_region(&self, cr3: u64, addr: u64, size: u64) -> bool {
        unsafe { crate::mmu::map_user_region(cr3, addr, size) }
    }

    fn current_cr3(&self) -> u64 {
        unsafe { crate::mmu::read_ttbr0() }
    }

    // ---- Crypto / RNG -------------------------------------------------------

    /// No hardware RNG available on the QEMU `virt` target we target first.
    /// Returning `Err` forces crypto callers to fail closed.
    fn random_bytes(&self, _buf: &mut [u8]) -> Result<(), ()> {
        Err(())
    }
}

/// Static platform reference registered with `kernel_core::set_platform`.
pub static PLATFORM: Aarch64Platform = Aarch64Platform;
