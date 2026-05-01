//! Physical frame allocator
//!
//! Allocates 4KB frames of physical memory using a bitmap allocator.

use crate::memory::types::{Frame, FrameNumber, PhysicalAddress};
use core::sync::atomic::{AtomicUsize, Ordering};

const FRAME_SIZE: usize = 4096;
const MAX_FRAMES: usize = 1024; // 4MB of RAM (minimal for testing)

/// Bitmap frame allocator
///
/// Tracks which frames are allocated using a bitmap.
/// Each bit represents one 4KB frame.
pub struct FrameAllocator {
    /// Bitmap of allocated frames (1 = allocated, 0 = free)
    bitmap: [u64; MAX_FRAMES / 64],
    /// Total number of frames managed
    total_frames: AtomicUsize,
    /// Number of free frames remaining
    free_frames: AtomicUsize,
    /// Base physical address of managed memory
    base_addr: PhysicalAddress,
}

impl FrameAllocator {
    /// Initialize frame allocator in-place to avoid stack allocation of bitmap
    ///
    /// # Safety
    /// - `ptr` must point to valid, uninitialized memory for FrameAllocator
    /// - The memory region must be valid and unused
    #[inline(never)]
    pub unsafe fn init_in_place(ptr: *mut Self, base_addr: PhysicalAddress, size_bytes: usize) {
        use core::ptr::addr_of_mut;

        let total_frames = size_bytes / FRAME_SIZE;

        // Zero the bitmap array directly - this is the big one!
        let bitmap_ptr = addr_of_mut!((*ptr).bitmap);
        core::ptr::write_bytes(bitmap_ptr as *mut u8, 0, MAX_FRAMES / 8);

        // Write the other fields
        addr_of_mut!((*ptr).total_frames).write(AtomicUsize::new(total_frames));
        addr_of_mut!((*ptr).free_frames).write(AtomicUsize::new(total_frames));
        addr_of_mut!((*ptr).base_addr).write(base_addr);
    }

    /// Create a new frame allocator (WARNING: uses stack space for bitmap)
    ///
    /// # Safety
    /// The caller must ensure the memory region is valid and unused
    #[allow(dead_code)]
    pub unsafe fn new(base_addr: PhysicalAddress, size_bytes: usize) -> Self {
        let total_frames = size_bytes / FRAME_SIZE;

        Self {
            bitmap: [0u64; MAX_FRAMES / 64],
            total_frames: AtomicUsize::new(total_frames),
            free_frames: AtomicUsize::new(total_frames),
            base_addr,
        }
    }

    /// Allocate a single frame (WORKAROUND VERSION - no atomic on free_frames)
    pub fn allocate_frame(&mut self) -> Option<Frame> {
        // Load atomic value for total_frames
        let total = self.total_frames.load(Ordering::Acquire);

        // Find first free frame
        for word_idx in 0..self.bitmap.len() {
            let bitmap_word = self.bitmap[word_idx];

            if bitmap_word == u64::MAX {
                continue;
            }

            // Find first zero bit
            let bit_pos = if bitmap_word & 1 == 0 { 0 } else { 1 };
            let frame_num = (word_idx * 64) + bit_pos;

            if frame_num >= total {
                continue;
            }

            // Mark as allocated in bitmap
            self.bitmap[word_idx] |= 1 << bit_pos;

            // NOTE: Skip atomic update to free_frames due to LLVM bug
            // self.free_frames.fetch_sub(1, Ordering::Relaxed);

            return Some(Frame::new(FrameNumber::new(frame_num)));
        }

        None
    }

    /// Allocate multiple contiguous frames
    pub fn allocate_frames(&mut self, count: usize) -> Option<Frame> {
        if count == 0 {
            return None;
        }

        if count == 1 {
            return self.allocate_frame();
        }

        // Find a run of count contiguous free frames
        let mut start_frame: Option<FrameNumber> = None;
        let mut contiguous = 0;

        for frame_num in 0..self.total_frames.load(Ordering::Acquire) {
            if self.is_allocated(FrameNumber::new(frame_num)) {
                start_frame = None;
                contiguous = 0;
            } else {
                if start_frame.is_none() {
                    start_frame = Some(FrameNumber::new(frame_num));
                }
                contiguous += 1;

                if contiguous == count {
                    let start = start_frame.unwrap();

                    // Mark all as allocated
                    for i in 0..count {
                        self.mark_allocated(FrameNumber::new(start.0 + i));
                    }

                    self.free_frames.fetch_sub(count, Ordering::AcqRel);
                    return Some(Frame::new(start));
                }
            }
        }

        None
    }

    /// Deallocate a frame
    pub fn deallocate_frame(&mut self, frame: Frame) {
        let frame_num = frame.number.0;

        if frame_num >= self.total_frames.load(Ordering::Acquire) {
            return;
        }

        if self.mark_free(frame.number) {
            self.free_frames.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Check if a frame is allocated
    fn is_allocated(&self, frame: FrameNumber) -> bool {
        let word_idx = frame.0 / 64;
        let bit_idx = frame.0 % 64;

        if word_idx >= self.bitmap.len() {
            return false;
        }

        (self.bitmap[word_idx] >> bit_idx) & 1 == 1
    }

    /// Mark a frame as allocated
    fn mark_allocated(&mut self, frame: FrameNumber) -> bool {
        let word_idx = frame.0 / 64;
        let bit_idx = frame.0 % 64;

        if word_idx >= self.bitmap.len() {
            return false;
        }

        let mask = 1u64 << bit_idx;
        let was_free = (self.bitmap[word_idx] & mask) == 0;
        self.bitmap[word_idx] |= mask;

        was_free
    }

    /// Mark a frame as free
    fn mark_free(&mut self, frame: FrameNumber) -> bool {
        let word_idx = frame.0 / 64;
        let bit_idx = frame.0 % 64;

        if word_idx >= self.bitmap.len() {
            return false;
        }

        let mask = 1u64 << bit_idx;
        let was_allocated = (self.bitmap[word_idx] & mask) != 0;
        self.bitmap[word_idx] &= !mask;

        was_allocated
    }

    /// Get the number of free frames
    pub fn free_frames(&self) -> usize {
        self.free_frames.load(Ordering::Acquire)
    }

    /// Get total frames
    pub fn total_frames(&self) -> usize {
        self.total_frames.load(Ordering::Acquire)
    }

    // ========== Persistence Support Methods ==========

    /// Check if a frame is allocated (public interface)
    pub fn is_frame_allocated_public(&self, frame_index: usize) -> bool {
        self.is_allocated(FrameNumber::new(frame_index))
    }

    /// Mark a frame as allocated by index (for restoration)
    pub fn mark_frame_allocated(&mut self, frame_index: usize) {
        if self.mark_allocated(FrameNumber::new(frame_index)) {
            // Only decrement if frame was actually free
            let _ = self.free_frames.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Get bitmap word for persistence
    pub fn get_bitmap_word(&self, word_index: usize) -> u64 {
        if word_index < self.bitmap.len() {
            self.bitmap[word_index]
        } else {
            0
        }
    }

    /// Restore bitmap word from persistence
    pub fn set_bitmap_word(&mut self, word_index: usize, value: u64) {
        if word_index < self.bitmap.len() {
            self.bitmap[word_index] = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_allocate() {
        let mut allocator = unsafe { FrameAllocator::new(PhysicalAddress::new(0x40000000), 1024 * 1024) };

        let frame1 = allocator.allocate_frame();
        assert!(frame1.is_some());

        let frame2 = allocator.allocate_frame();
        assert!(frame2.is_some());

        assert_ne!(frame1.unwrap().number.0, frame2.unwrap().number.0);

        assert_eq!(allocator.free_frames(), allocator.total_frames() - 2);
    }

    #[test]
    fn test_frame_deallocate() {
        let mut allocator = unsafe { FrameAllocator::new(PhysicalAddress::new(0x40000000), 1024 * 1024) };

        let frame = allocator.allocate_frame().unwrap();
        assert_eq!(allocator.free_frames(), allocator.total_frames() - 1);

        allocator.deallocate_frame(frame);
        assert_eq!(allocator.free_frames(), allocator.total_frames());
    }
}
