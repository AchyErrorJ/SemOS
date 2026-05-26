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

    fn stdin_read(&self, buf: &mut [u8]) -> usize {
        crate::tty::drain(buf)
    }

    fn schedule(&self) {
        crate::context::schedule();
    }

    fn random_bytes(&self, buf: &mut [u8]) -> Result<(), ()> {
        crate::rng::fill_bytes(buf)
    }

    fn wall_clock(&self) -> Option<u64> {
        crate::rtc::unix_time()
    }

    fn setup_user_argv(
        &self,
        space: u64,
        stack_top: u64,
        argv: &[&[u8]],
        envp: &[&[u8]],
    ) -> Option<u64> {
        // NOTE: we no longer early-return for empty argv/envp. Always
        // emit at least a minimal SysV layout (argc=0, argv NULL, envp
        // NULL) so the user runtime can unconditionally read argc at
        // [rsp] without first having to prove a layout exists. The
        // empty layout is 24 bytes (argc + argv-NULL + envp-NULL), well
        // inside the top stack page. M25 #51 (argv parsing) depends on
        // this guarantee.
        // Cap individual lengths so a runaway request can't trash the
        // whole page. The whole layout has to fit comfortably under the
        // 4 KiB top stack page; bound by sane-defaults rather than try
        // to be precise.
        const MAX_ITEMS: usize = 32;
        const MAX_STR_BYTES: usize = 256;
        if argv.len() > MAX_ITEMS || envp.len() > MAX_ITEMS { return None; }
        for s in argv.iter().chain(envp.iter()) {
            if s.len() > MAX_STR_BYTES { return None; }
        }

        // Step 1: compute total string-data size (each string +
        // 1-byte null terminator).
        let mut str_bytes: usize = 0;
        for s in argv.iter().chain(envp.iter()) { str_bytes += s.len() + 1; }

        // Step 2: compute pointer-array size.
        //   argc(8) + argv ptrs(8 * (argc+1, +1 for NULL)) +
        //   envp ptrs(8 * (envc+1))
        let ptr_bytes = 8 + 8 * (argv.len() + 1) + 8 * (envp.len() + 1);

        // Total layout, with 16-byte alignment slop.
        let total = str_bytes + ptr_bytes + 16;
        if total > 4096 { return None; }

        // Step 3: assemble the layout in a kernel-side scratch buffer,
        // then page-translate the top of the new user stack and copy
        // it across. Doing it in two halves (build + copy) keeps the
        // logic clear and lets us bail before any writes.
        let mut scratch = [0u8; 4096];
        // Pack into scratch starting from the END (the bytes we want
        // at the highest addresses come first in our buffer-from-end
        // perspective).
        //
        // Layout in scratch (low→high address) BEFORE we shift to user
        // space: we mirror what'll be on the user stack.
        //
        //   scratch[0..ptr_bytes]                  pointer area
        //     [argc][argv[0]..argv[N-1]][NULL][envp[0]..envp[M-1]][NULL]
        //   scratch[ptr_bytes..ptr_bytes+str_bytes] string data
        //
        // Then we copy this to (user_top - total), and the user RSP
        // points to scratch[0]'s equivalent on the user stack.

        // Where on the user stack does our layout START (low end)?
        let layout_base_user_virt = (stack_top - total as u64) & !0xF;
        // Where do strings start within scratch (low end)?
        let strings_base_user_virt = layout_base_user_virt + ptr_bytes as u64;

        // Fill pointer area in scratch.
        let mut cursor = 0usize;
        // argc
        scratch[cursor..cursor+8].copy_from_slice(&(argv.len() as u64).to_le_bytes());
        cursor += 8;
        // argv ptrs (each points into the string-data region)
        let mut str_cursor = strings_base_user_virt;
        for s in argv.iter() {
            scratch[cursor..cursor+8].copy_from_slice(&str_cursor.to_le_bytes());
            cursor += 8;
            str_cursor += (s.len() + 1) as u64;
        }
        // argv terminator
        scratch[cursor..cursor+8].copy_from_slice(&0u64.to_le_bytes());
        cursor += 8;
        // envp ptrs
        for s in envp.iter() {
            scratch[cursor..cursor+8].copy_from_slice(&str_cursor.to_le_bytes());
            cursor += 8;
            str_cursor += (s.len() + 1) as u64;
        }
        // envp terminator
        scratch[cursor..cursor+8].copy_from_slice(&0u64.to_le_bytes());
        cursor += 8;

        // String data
        for s in argv.iter().chain(envp.iter()) {
            scratch[cursor..cursor+s.len()].copy_from_slice(s);
            cursor += s.len();
            scratch[cursor] = 0; // null terminator
            cursor += 1;
        }

        // Step 4: copy scratch[0..cursor] to user-space at layout_base_user_virt.
        // The user stack page is mapped into `space` (the new process's cr3);
        // we translate page-by-page via walk_pml4_for and write through the
        // kernel's identity-map window.
        let bytes_to_write = cursor;
        let mut written = 0usize;
        while written < bytes_to_write {
            let user_virt = layout_base_user_virt + written as u64;
            let phys = crate::paging::walk_pml4_for(space, user_virt)?;
            let page_off = (user_virt & 0xFFF) as usize;
            let chunk = (4096 - page_off).min(bytes_to_write - written);
            unsafe {
                let dst = crate::paging::phys_to_virt(phys & !0xFFF) as *mut u8;
                core::ptr::copy_nonoverlapping(
                    scratch.as_ptr().add(written),
                    dst.add(page_off),
                    chunk,
                );
            }
            written += chunk;
        }

        Some(layout_base_user_virt)
    }

    fn reap_slot(&self, slot: usize) {
        // Free the slot's AddressSpace (PML4 + subtables) and zero its
        // saved cr3. Called from alloc_task_slot at the moment of
        // reusing an Exited slot.
        //
        // CRITICAL (M25 #45/#52): a Ring-3 *thread* shares its parent's
        // cr3. If we blindly destroy this slot's cr3 we'd free the page
        // tables out from under the still-running parent (or sibling
        // threads). So only destroy the address space when NO other
        // live slot still uses this cr3 — i.e. this is the last task in
        // the address space. Otherwise just detach this slot's cr3.
        unsafe {
            let contexts = &raw mut crate::context::CONTEXTS;
            if slot >= crate::context::CONTEXTS.len() {
                return;
            }
            let dying_cr3 = (*contexts)[slot].cr3;
            if dying_cr3 == 0 {
                return;
            }
            // Scan for any OTHER slot that is alive and shares this cr3.
            let tasks = &raw const kernel_core::scheduler::TASKS;
            let mut shared = false;
            for i in 0..kernel_core::scheduler::MAX_TASKS {
                if i == slot {
                    continue;
                }
                let st = (*tasks)[i].state;
                let alive = !matches!(
                    st,
                    kernel_core::scheduler::TaskState::Empty
                        | kernel_core::scheduler::TaskState::Exited
                );
                if alive && (*contexts)[i].cr3 == dying_cr3 {
                    shared = true;
                    break;
                }
            }
            (*contexts)[slot].cr3 = 0; // detach regardless
            if !shared {
                crate::context::destroy_address_space(dying_cr3);
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

    fn reclaim_address_spaces(&self) -> usize {
        crate::context::reclaim_dead_address_spaces()
    }

    fn enable_interrupts(&self) {
        x86_64::instructions::interrupts::enable();
    }

    fn llm_ask(&self, prompt: &[u8], out: &mut [u8]) -> usize {
        // The agent's network path is wall-clock bounded (TLS idle timeout),
        // which needs the timer to advance — and a multi-second call shouldn't
        // freeze the scheduler. Enable interrupts for the duration; iretq
        // restores the caller's (Ring-3, IF=1) flags on return.
        x86_64::instructions::interrupts::enable();
        let p = core::str::from_utf8(prompt).unwrap_or("");
        crate::agent::ask(p, out)
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
            // Two segments can share a virtual page when the linker packs
            // a small .data/.rodata right after a larger one (e.g. vec-demo
            // has rodata ending at 0x403D1A and the next RW segment starting
            // at 0x403D20 — same page 0x403000). If we allocate a fresh
            // zeroed frame for the second segment we'd wipe the first's
            // content. So: reuse the existing frame if the page is already
            // mapped, and copy our bytes INTO it without zeroing.
            let already = crate::paging::walk_pml4_for(space, page);
            let (frame, zero_first) = match already {
                Some(phys) => (phys & !0xFFF, false), // reuse; don't clobber
                None => {
                    let f = match crate::paging::alloc_pt_frame() {
                        Some(f) => f,
                        None => return false,
                    };
                    (f, true)
                }
            };

            let frame_virt = crate::paging::phys_to_virt(frame);
            unsafe {
                if zero_first {
                    // Fresh frame — zero it (covers BSS + partial pages).
                    core::ptr::write_bytes(frame_virt as *mut u8, 0, page_size as usize);
                }
                // Copy file data that falls within this page.
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

            // Only (re)map if this page wasn't already mapped. Re-mapping
            // an existing page would need an unmap first and could change
            // permissions for the shared page — we keep the first
            // segment's perms (acceptable: shared pages are rare and the
            // looser of R / RW is what the data needs anyway).
            if already.is_none() {
                if !crate::context::map_page_in_space(space, page, frame, perm) {
                    return false;
                }
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

    fn current_cr3(&self) -> u64 {
        // The active CR3 register reflects whichever task last ran.
        // Same-AS thread spawn (task #45) calls this from a syscall
        // handler running in the parent's AS, so this is what we want.
        crate::paging::read_cr3()
    }

    fn map_user_region(&self, cr3: u64, addr: u64, size: u64) -> bool {
        use kernel_core::memory::SecurityTier;
        let page_size = crate::paging::PAGE_SIZE_4K;
        let start = addr & !(page_size - 1);
        let end = (addr + size + page_size - 1) & !(page_size - 1);
        let mut page = start;
        while page < end {
            // Data frames come from the big tiered pool (13.5K frames
            // each), NOT the 512-frame PT pool — a multi-MiB user heap
            // would exhaust the PT pool instantly. Page-table interior
            // nodes still draw from the PT pool inside map_page_in_space,
            // but those are ~1 per 2 MiB.
            let frame = match crate::memory::alloc(SecurityTier::Public) {
                Some(f) => f,
                None => {
                    let free = crate::memory::free_frames(SecurityTier::Public);
                    crate::serial::_print(format_args!(
                        "[map_user_region] Public pool OOM at page 0x{:x} (free={})\n",
                        page, free));
                    return false; // pool OOM
                }
            };
            // Zero the frame before exposing it to user space (no info leak).
            unsafe {
                let virt = crate::paging::phys_to_virt(frame);
                core::ptr::write_bytes(virt as *mut u8, 0, page_size as usize);
            }
            if !crate::context::map_page_in_space(
                cr3, page, frame,
                crate::paging::PagePermission::ReadWrite,
            ) {
                return false;
            }
            page += page_size;
        }
        true
    }

    fn spawn_thread(
        &self,
        name: &'static str,
        cr3: u64,
        entry_va: u64,
        arg: u64,
        max_tier: u8,
    ) -> Option<usize> {
        // Kernel-mode (cr3==0 or matches boot_cr3): entry signature is
        // `fn()` and runs in Ring 0. `arg` is unused; the kernel-side
        // DEMO 27 uses module-level globals for cross-thread comm.
        if cr3 == 0 || cr3 == crate::paging::boot_cr3() {
            let _ = arg;
            // SAFETY: caller asserts entry_va is a function pointer of
            // the right type. Same trust the existing Ring 0 helpers use.
            let entry: fn() = unsafe { core::mem::transmute(entry_va as usize) };
            let _ = max_tier; // kernel-mode tasks always max_tier=3
            return crate::context::spawn_task(name, entry);
        }
        // Ring 3 same-AS branch — pick a fresh user-stack region in the
        // parent's CR3, map it, build a Ring 3 context that delivers
        // `arg` in rdi. Closes the SYS_THREAD_SPAWN end-to-end story
        // (Phase 14 Tier 3 #45 finish).
        crate::context::spawn_ring3_thread_in_cr3(name, cr3, entry_va, arg, max_tier)
    }
}

/// Global platform instance (lives for the kernel's lifetime)
pub static PLATFORM: X86Platform = X86Platform;
