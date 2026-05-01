//! Memory types for physical memory management

/// Physical address (no virtual memory in Semantic OS)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysicalAddress(pub usize);

impl PhysicalAddress {
    /// Null/zero address
    pub const NULL: Self = Self(0);

    /// Create from usize
    pub const fn new(addr: usize) -> Self {
        Self(addr)
    }

    /// Get the raw value
    pub const fn as_usize(self) -> usize {
        self.0
    }

    /// Align to page boundary
    pub const fn align_up_to_page(self) -> Self {
        Self((self.0 + 4095) & !4095)
    }

    /// Align down to page boundary
    pub const fn align_down_to_page(self) -> Self {
        Self(self.0 & !4095)
    }

    /// Check if aligned to page boundary
    pub const fn is_page_aligned(self) -> bool {
        self.0 & 0xFFF == 0
    }
}

/// Frame number (4KB page index)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameNumber(pub usize);

impl FrameNumber {
    /// Create from usize
    pub const fn new(n: usize) -> Self {
        Self(n)
    }

    /// Get the physical address of this frame
    pub const fn to_address(self) -> PhysicalAddress {
        PhysicalAddress(self.0 * 4096)
    }

    /// Create from physical address
    pub const fn from_address(addr: PhysicalAddress) -> Self {
        Self(addr.0 / 4096)
    }
}

/// A 4KB frame of physical memory
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub number: FrameNumber,
}

impl Frame {
    /// Create a new frame
    pub const fn new(number: FrameNumber) -> Self {
        Self { number }
    }

    /// Get the physical address
    pub const fn address(&self) -> PhysicalAddress {
        self.number.to_address()
    }

    /// Get frame containing an address
    pub fn containing(address: PhysicalAddress) -> Self {
        Self {
            number: FrameNumber::from_address(address),
        }
    }
}

/// Size in bytes
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bytes(pub usize);

impl Bytes {
    /// Create from usize
    pub const fn new(n: usize) -> Self {
        Self(n)
    }

    /// Round up to page alignment
    pub const fn align_up_to_page(self) -> Bytes {
        Bytes((self.0 + 4095) & !4095)
    }

    /// Number of frames needed
    pub const fn as_frames(self) -> usize {
        (self.0 + 4095) / 4096
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_physical_address_alignment() {
        let addr = PhysicalAddress::new(0x1234);
        assert_eq!(addr.align_up_to_page().0, 0x2000);
        assert_eq!(addr.align_down_to_page().0, 0x1000);
    }

    #[test]
    fn test_frame_conversion() {
        let frame = FrameNumber::new(42);
        let addr = frame.to_address();
        assert_eq!(addr.0, 42 * 4096);
        assert_eq!(FrameNumber::from_address(addr).0, 42);
    }
}
