//! Platform trait implementation for x86_64
//!
//! Bridges kernel-core's Platform trait to x86_64 hardware:
//! - serial_write → COM1 16550 UART
//! - ticks → PIT timer tick counter
//! - halt → HLT instruction
//! - address space / ELF loading → paging + context modules

use kernel_core::Platform;

/// x86_64 platform implementation
pub struct X86Platform;

impl Platform for X86Platform {
    fn serial_write(&self, s: &str) {
        crate::serial::_print(format_args!("{}", s));
    }

    fn ticks(&self) -> u64 {
        crate::interrupts::get_ticks()
    }

    fn halt(&self) {
        x86_64::instructions::hlt();
    }

    fn schedule(&self) {
        crate::context::schedule();
    }

    fn reap_slot(&self, slot: usize) {
        // Free the slot's AddressSpace (PML4 + subtables) and zero its
        // saved cr3. Called from alloc_task_slot at the moment of
        // reusing an Exited slot — by this point no kernel code is
        // running on the dying CR3, so we can safely free its frames.
        unsafe {
            let contexts = &raw mut crate::context::CONTEXTS;
            if slot < crate::context::CONTEXTS.len() {
                let dying_cr3 = (*contexts)[slot].cr3;
                if dying_cr3 != 0 {
                    crate::context::destroy_address_space(dying_cr3);
                    (*contexts)[slot].cr3 = 0;
                }
            }
        }
    }

    fn alloc_frame(&self, tier: u8) -> Option<u64> {
        use kernel_core::memory::SecurityTier;
        let tier = match tier {
            0 => SecurityTier::Public,
            1 => SecurityTier::Internal,
            2 => SecurityTier::Sensitive,
            3 => SecurityTier::Secret,
            _ => return None,
        };
        crate::memory::alloc(tier)
    }

    fn free_frame(&self, addr: u64) -> bool {
        crate::memory::free(addr)
    }

    fn create_address_space(&self, max_tier: u8) -> Option<u64> {
        let space = crate::paging::create_process_address_space(max_tier)?;
        let cr3 = space.cr3;
        crate::context::store_address_space(space);
        Some(cr3)
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
        let perm = match (executable, writable) {
            (true, false) => crate::paging::PagePermission::ReadExecute,
            (false, true) => crate::paging::PagePermission::ReadWrite,
            (true, true) => crate::paging::PagePermission::ReadWriteExecute,
            (false, false) => crate::paging::PagePermission::ReadOnly,
        };

        // Map pages covering [virt_addr, virt_addr + memsz), allocating frames
        // and copying data into them.
        let page_size = crate::paging::PAGE_SIZE_4K;
        let start_page = virt_addr & !0xFFF;
        let end = virt_addr + memsz as u64;
        let end_page = (end + page_size - 1) & !0xFFF;

        let mut page = start_page;
        while page < end_page {
            // Allocate a physical frame for this page
            let frame = match crate::paging::alloc_pt_frame() {
                Some(f) => f,
                None => return false,
            };

            // Copy data into the frame (via physical memory mapping)
            let frame_virt = crate::paging::phys_to_virt(frame);
            unsafe {
                // Zero the frame first (handles BSS and partial pages)
                core::ptr::write_bytes(frame_virt as *mut u8, 0, page_size as usize);

                // Copy file data that falls within this page
                let page_offset_in_seg = if page >= virt_addr {
                    (page - virt_addr) as usize
                } else {
                    0
                };
                let copy_start_in_page = if virt_addr > page {
                    (virt_addr - page) as usize
                } else {
                    0
                };

                if page_offset_in_seg < data.len() {
                    let remaining = data.len() - page_offset_in_seg;
                    let copy_len = remaining.min((page_size as usize) - copy_start_in_page);
                    let dst = (frame_virt as *mut u8).add(copy_start_in_page);
                    let src = data.as_ptr().add(page_offset_in_seg);
                    core::ptr::copy_nonoverlapping(src, dst, copy_len);
                }
            }

            // Map the page into the address space
            // We need to find the AddressSpace by CR3 and call map_4k
            if !crate::context::map_page_in_space(space, page, frame, perm) {
                return false;
            }

            page += page_size;
        }
        true
    }

    fn map_user_stack(&self, space: u64, stack_top: u64, stack_size: u64) -> Option<u64> {
        let page_size = crate::paging::PAGE_SIZE_4K;
        let stack_bottom = stack_top - stack_size;
        let pages = (stack_size + page_size - 1) / page_size;

        for i in 0..pages {
            let frame = crate::paging::alloc_pt_frame()?;
            // Zero the stack frame
            unsafe {
                let virt = crate::paging::phys_to_virt(frame);
                core::ptr::write_bytes(virt as *mut u8, 0, page_size as usize);
            }
            let virt = stack_bottom + i * page_size;
            if !crate::context::map_page_in_space(
                space, virt, frame,
                crate::paging::PagePermission::ReadWrite,
            ) {
                return None;
            }
        }

        // Return stack top, 16-byte aligned
        Some(stack_top & !0xF)
    }

    fn spawn_user_task(
        &self,
        name: &'static str,
        user_rip: u64,
        user_rsp: u64,
        cr3: u64,
        max_tier: u8,
    ) -> Option<usize> {
        crate::context::spawn_user_task_with_cr3(name, user_rip, user_rsp, cr3, max_tier)
    }

    fn destroy_address_space(&self, space: u64) {
        crate::context::destroy_address_space(space);
    }
}

/// Global platform instance (lives for the kernel's lifetime)
pub static PLATFORM: X86Platform = X86Platform;
