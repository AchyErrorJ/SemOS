//! Driver Traits - Generic Interfaces for Device Abstraction
//!
//! These traits define the standard interfaces that all drivers implement,
//! enabling code to work with any device without knowing the specific hardware.
//!
//! # Design Philosophy
//!
//! - Traits are minimal and focused
//! - Implementations are responsible for hardware-specific details
//! - Errors are specific to each device class
//! - All traits work in no_std environment

/// Driver error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverError {
    // Generic errors
    NotReady,
    NotSupported,
    InvalidParameter,
    Timeout,

    // Block device errors
    ReadOnly,
    OutOfBounds,
    IoError,

    // Network errors
    LinkDown,
    BufferTooSmall,
    NoBuffers,

    // Character device errors
    WouldBlock,
    Closed,
}

pub type DriverResult<T> = Result<T, DriverError>;

// ============================================================================
// Block Device Trait
// ============================================================================

/// A block device provides random access to fixed-size blocks of data.
///
/// Examples: Hard drives, SSDs, SD cards, RAM disks, disk images
///
/// # Block Size
///
/// The standard block size is 512 bytes (traditional sector size).
/// Some devices may use larger blocks (4096 for modern SSDs).
pub trait BlockDevice: Send + Sync {
    /// Read blocks from the device.
    ///
    /// # Arguments
    /// * `block` - Starting block number
    /// * `buf` - Buffer to read into (must be multiple of block_size)
    fn read_blocks(&self, block: u64, buf: &mut [u8]) -> DriverResult<()>;

    /// Write blocks to the device.
    ///
    /// # Arguments
    /// * `block` - Starting block number
    /// * `buf` - Buffer to write from (must be multiple of block_size)
    fn write_blocks(&self, block: u64, buf: &[u8]) -> DriverResult<()>;

    /// Flush any cached writes to the device.
    fn flush(&self) -> DriverResult<()> {
        Ok(()) // Default: no-op for devices without caching
    }

    /// Get the block size in bytes.
    fn block_size(&self) -> usize;

    /// Get the total number of blocks.
    fn block_count(&self) -> u64;

    /// Check if the device is read-only.
    fn is_read_only(&self) -> bool {
        false
    }

    /// Get device name/identifier.
    fn name(&self) -> &'static str;

    // === Convenience methods (with default implementations) ===

    /// Get total device size in bytes.
    fn size_bytes(&self) -> u64 {
        self.block_count() * self.block_size() as u64
    }

    /// Read a single block.
    fn read_block(&self, block: u64, buf: &mut [u8]) -> DriverResult<()> {
        if buf.len() < self.block_size() {
            return Err(DriverError::BufferTooSmall);
        }
        self.read_blocks(block, &mut buf[..self.block_size()])
    }

    /// Write a single block.
    fn write_block(&self, block: u64, buf: &[u8]) -> DriverResult<()> {
        if buf.len() < self.block_size() {
            return Err(DriverError::BufferTooSmall);
        }
        self.write_blocks(block, &buf[..self.block_size()])
    }
}

// ============================================================================
// Network Device Trait
// ============================================================================

/// A network device provides packet-based communication.
///
/// Examples: Ethernet adapters, WiFi, VirtIO-net
pub trait NetDevice: Send + Sync {
    /// Send a packet.
    ///
    /// The packet should include the full Ethernet frame (starting from dst MAC).
    fn send(&self, packet: &[u8]) -> DriverResult<()>;

    /// Receive a packet.
    ///
    /// Returns the number of bytes received, or WouldBlock if no packet available.
    fn recv(&self, buf: &mut [u8]) -> DriverResult<usize>;

    /// Check if a packet is available to receive.
    fn poll(&self) -> bool;

    /// Get the MAC address.
    fn mac_address(&self) -> [u8; 6];

    /// Get the MTU (Maximum Transmission Unit).
    fn mtu(&self) -> usize {
        1500 // Standard Ethernet MTU
    }

    /// Check if the link is up.
    fn link_up(&self) -> bool {
        true
    }

    /// Get device name/identifier.
    fn name(&self) -> &'static str;
}

// ============================================================================
// Character Device Trait
// ============================================================================

/// A character device provides sequential byte-stream I/O.
///
/// Examples: Serial ports, consoles, keyboards, printers
pub trait CharDevice: Send + Sync {
    /// Read bytes from the device.
    ///
    /// Returns the number of bytes read (may be less than buf.len()).
    /// Returns WouldBlock if no data available and non-blocking.
    fn read(&self, buf: &mut [u8]) -> DriverResult<usize>;

    /// Write bytes to the device.
    ///
    /// Returns the number of bytes written (may be less than buf.len()).
    fn write(&self, buf: &[u8]) -> DriverResult<usize>;

    /// Check if data is available to read.
    fn can_read(&self) -> bool {
        true
    }

    /// Check if the device can accept more data.
    fn can_write(&self) -> bool {
        true
    }

    /// Flush output buffers.
    fn flush(&self) -> DriverResult<()> {
        Ok(())
    }

    /// Get device name/identifier.
    fn name(&self) -> &'static str;

    // === Convenience methods ===

    /// Read a single byte.
    fn read_byte(&self) -> DriverResult<u8> {
        let mut buf = [0u8; 1];
        self.read(&mut buf)?;
        Ok(buf[0])
    }

    /// Write a single byte.
    fn write_byte(&self, byte: u8) -> DriverResult<()> {
        self.write(&[byte])?;
        Ok(())
    }

    /// Write a string.
    fn write_str(&self, s: &str) -> DriverResult<()> {
        self.write(s.as_bytes())?;
        Ok(())
    }
}

// ============================================================================
// Input Device Trait
// ============================================================================

/// Input event types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEventType {
    KeyPress,
    KeyRelease,
    MouseMove,
    MouseButton,
    TouchStart,
    TouchMove,
    TouchEnd,
    Accelerometer,
    Gyroscope,
}

/// An input event
#[derive(Debug, Clone, Copy)]
pub struct InputEvent {
    /// Event type
    pub event_type: InputEventType,
    /// Key code (for keyboard events)
    pub code: u16,
    /// Value (button state, axis value, etc.)
    pub value: i32,
    /// X coordinate (for pointer/touch events)
    pub x: i32,
    /// Y coordinate (for pointer/touch events)
    pub y: i32,
    /// Timestamp (in microseconds)
    pub timestamp: u64,
}

impl InputEvent {
    pub const fn key(code: u16, pressed: bool) -> Self {
        Self {
            event_type: if pressed { InputEventType::KeyPress } else { InputEventType::KeyRelease },
            code,
            value: if pressed { 1 } else { 0 },
            x: 0,
            y: 0,
            timestamp: 0,
        }
    }

    pub const fn mouse_move(x: i32, y: i32) -> Self {
        Self {
            event_type: InputEventType::MouseMove,
            code: 0,
            value: 0,
            x,
            y,
            timestamp: 0,
        }
    }
}

/// An input device provides user input events.
///
/// Examples: Keyboards, mice, touchscreens, game controllers, VR sensors
pub trait InputDevice: Send + Sync {
    /// Poll for the next input event.
    ///
    /// Returns None if no event is available.
    fn poll_event(&self) -> Option<InputEvent>;

    /// Check if events are available.
    fn has_events(&self) -> bool;

    /// Get device name/identifier.
    fn name(&self) -> &'static str;

    /// Get device capabilities (bitmask of InputEventType).
    fn capabilities(&self) -> u32 {
        0xFFFFFFFF // All capabilities by default
    }
}

// ============================================================================
// Display Device Trait
// ============================================================================

/// Pixel format for framebuffers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    RGB888,
    RGBA8888,
    BGR888,
    BGRA8888,
    RGB565,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(&self) -> usize {
        match self {
            PixelFormat::RGB888 | PixelFormat::BGR888 => 3,
            PixelFormat::RGBA8888 | PixelFormat::BGRA8888 => 4,
            PixelFormat::RGB565 => 2,
        }
    }
}

/// Display device info
#[derive(Debug, Clone, Copy)]
pub struct DisplayInfo {
    pub width: u32,
    pub height: u32,
    pub pitch: u32, // Bytes per row
    pub format: PixelFormat,
}

/// A display device provides a framebuffer for graphics output.
///
/// Examples: Framebuffers, VirtIO-GPU, LCD controllers
pub trait DisplayDevice: Send + Sync {
    /// Get display info.
    fn info(&self) -> DisplayInfo;

    /// Get mutable access to the framebuffer.
    ///
    /// # Safety
    /// Caller must ensure proper synchronization.
    unsafe fn framebuffer(&self) -> *mut u8;

    /// Flush a region of the framebuffer to the display.
    fn flush_rect(&self, x: u32, y: u32, width: u32, height: u32) -> DriverResult<()>;

    /// Flush the entire framebuffer.
    fn flush(&self) -> DriverResult<()> {
        let info = self.info();
        self.flush_rect(0, 0, info.width, info.height)
    }

    /// Get device name/identifier.
    fn name(&self) -> &'static str;

    // === Convenience methods ===

    /// Set a pixel (bounds checked).
    fn set_pixel(&self, x: u32, y: u32, r: u8, g: u8, b: u8) -> DriverResult<()> {
        let info = self.info();
        if x >= info.width || y >= info.height {
            return Err(DriverError::OutOfBounds);
        }

        let bpp = info.format.bytes_per_pixel();
        let offset = (y * info.pitch + x * bpp as u32) as usize;

        unsafe {
            let fb = self.framebuffer();
            match info.format {
                PixelFormat::RGB888 => {
                    *fb.add(offset) = r;
                    *fb.add(offset + 1) = g;
                    *fb.add(offset + 2) = b;
                }
                PixelFormat::BGR888 => {
                    *fb.add(offset) = b;
                    *fb.add(offset + 1) = g;
                    *fb.add(offset + 2) = r;
                }
                PixelFormat::RGBA8888 => {
                    *fb.add(offset) = r;
                    *fb.add(offset + 1) = g;
                    *fb.add(offset + 2) = b;
                    *fb.add(offset + 3) = 0xFF;
                }
                PixelFormat::BGRA8888 => {
                    *fb.add(offset) = b;
                    *fb.add(offset + 1) = g;
                    *fb.add(offset + 2) = r;
                    *fb.add(offset + 3) = 0xFF;
                }
                PixelFormat::RGB565 => {
                    let pixel = ((r as u16 & 0xF8) << 8)
                        | ((g as u16 & 0xFC) << 3)
                        | ((b as u16 & 0xF8) >> 3);
                    *(fb.add(offset) as *mut u16) = pixel;
                }
            }
        }
        Ok(())
    }
}

// ============================================================================
// Timer Device Trait
// ============================================================================

/// A timer device provides timing services.
pub trait TimerDevice: Send + Sync {
    /// Get the current time in nanoseconds since boot.
    fn now_ns(&self) -> u64;

    /// Get the timer frequency in Hz.
    fn frequency(&self) -> u64;

    /// Set a one-shot timer callback.
    fn set_timeout(&self, ns: u64, callback: fn()) -> DriverResult<()>;

    /// Get device name/identifier.
    fn name(&self) -> &'static str;

    // === Convenience methods ===

    /// Get the current time in microseconds.
    fn now_us(&self) -> u64 {
        self.now_ns() / 1000
    }

    /// Get the current time in milliseconds.
    fn now_ms(&self) -> u64 {
        self.now_ns() / 1_000_000
    }
}

// ============================================================================
// Entropy Device Trait
// ============================================================================

/// An entropy device provides random data.
///
/// Examples: Hardware RNG, VirtIO-entropy
pub trait EntropyDevice: Send + Sync {
    /// Fill buffer with random bytes.
    fn get_random(&self, buf: &mut [u8]) -> DriverResult<()>;

    /// Get a random u64.
    fn random_u64(&self) -> DriverResult<u64> {
        let mut buf = [0u8; 8];
        self.get_random(&mut buf)?;
        Ok(u64::from_le_bytes(buf))
    }

    /// Get device name/identifier.
    fn name(&self) -> &'static str;
}
