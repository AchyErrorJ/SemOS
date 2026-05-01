//! Driver Registry - Device Discovery and Management
//!
//! The registry provides a central place to register and lookup devices
//! by type. This enables code to work with "any block device" without
//! knowing whether it's VirtIO, NVMe, or a RAM disk.
//!
//! # Usage
//!
//! ```ignore
//! // Register a device during driver initialization
//! registry::register_block("virtio0", &VIRTIO_BLK);
//!
//! // Later, get any block device
//! if let Some(blk) = registry::get_block("virtio0") {
//!     blk.read_blocks(0, &mut buf)?;
//! }
//!
//! // Or iterate all block devices
//! for (name, dev) in registry::block_devices() {
//!     println!("{}: {} blocks", name, dev.block_count());
//! }
//! ```

use super::traits::*;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Maximum number of devices per category
const MAX_DEVICES: usize = 8;

/// Device entry with name
struct DeviceEntry<T: ?Sized + 'static> {
    name: &'static str,
    device: Option<&'static T>,
}

impl<T: ?Sized + 'static> DeviceEntry<T> {
    const fn empty() -> Self {
        Self {
            name: "",
            device: None,
        }
    }
}

/// Block device registry
static mut BLOCK_DEVICES: [DeviceEntry<dyn BlockDevice>; MAX_DEVICES] = [
    DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(),
    DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(),
];
static BLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Character device registry
static mut CHAR_DEVICES: [DeviceEntry<dyn CharDevice>; MAX_DEVICES] = [
    DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(),
    DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(),
];
static CHAR_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Network device registry
static mut NET_DEVICES: [DeviceEntry<dyn NetDevice>; MAX_DEVICES] = [
    DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(),
    DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(),
];
static NET_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Input device registry
static mut INPUT_DEVICES: [DeviceEntry<dyn InputDevice>; MAX_DEVICES] = [
    DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(),
    DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(),
];
static INPUT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Display device registry
static mut DISPLAY_DEVICES: [DeviceEntry<dyn DisplayDevice>; MAX_DEVICES] = [
    DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(),
    DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(), DeviceEntry::empty(),
];
static DISPLAY_COUNT: AtomicUsize = AtomicUsize::new(0);

// ============================================================================
// Block Device Registration
// ============================================================================

/// Register a block device.
pub fn register_block(name: &'static str, device: &'static dyn BlockDevice) -> bool {
    let count = BLOCK_COUNT.load(Ordering::Acquire);
    if count >= MAX_DEVICES {
        return false;
    }

    unsafe {
        BLOCK_DEVICES[count] = DeviceEntry {
            name,
            device: Some(device),
        };
    }
    BLOCK_COUNT.store(count + 1, Ordering::Release);

    crate::platform::log("  [registry] Registered block device: ");
    crate::platform::log(name);
    crate::platform::log("\n");

    true
}

/// Get a block device by name.
pub fn get_block(name: &str) -> Option<&'static dyn BlockDevice> {
    let count = BLOCK_COUNT.load(Ordering::Acquire);
    for i in 0..count {
        unsafe {
            if BLOCK_DEVICES[i].name == name {
                return BLOCK_DEVICES[i].device;
            }
        }
    }
    None
}

/// Get the first available block device.
pub fn get_first_block() -> Option<&'static dyn BlockDevice> {
    if BLOCK_COUNT.load(Ordering::Acquire) > 0 {
        unsafe { BLOCK_DEVICES[0].device }
    } else {
        None
    }
}

/// Get block device count.
pub fn block_count() -> usize {
    BLOCK_COUNT.load(Ordering::Acquire)
}

/// Iterate over all block devices.
pub fn block_devices() -> impl Iterator<Item = (&'static str, &'static dyn BlockDevice)> {
    let count = BLOCK_COUNT.load(Ordering::Acquire);
    (0..count).filter_map(|i| unsafe {
        BLOCK_DEVICES[i].device.map(|d| (BLOCK_DEVICES[i].name, d))
    })
}

// ============================================================================
// Character Device Registration
// ============================================================================

/// Register a character device.
pub fn register_char(name: &'static str, device: &'static dyn CharDevice) -> bool {
    let count = CHAR_COUNT.load(Ordering::Acquire);
    if count >= MAX_DEVICES {
        return false;
    }

    unsafe {
        CHAR_DEVICES[count] = DeviceEntry {
            name,
            device: Some(device),
        };
    }
    CHAR_COUNT.store(count + 1, Ordering::Release);

    crate::platform::log("  [registry] Registered char device: ");
    crate::platform::log(name);
    crate::platform::log("\n");

    true
}

/// Get a character device by name.
pub fn get_char(name: &str) -> Option<&'static dyn CharDevice> {
    let count = CHAR_COUNT.load(Ordering::Acquire);
    for i in 0..count {
        unsafe {
            if CHAR_DEVICES[i].name == name {
                return CHAR_DEVICES[i].device;
            }
        }
    }
    None
}

/// Get the first available character device (usually the console).
pub fn get_console() -> Option<&'static dyn CharDevice> {
    // Try to find "console" first, then "serial0", then any
    get_char("console")
        .or_else(|| get_char("serial0"))
        .or_else(|| {
            if CHAR_COUNT.load(Ordering::Acquire) > 0 {
                unsafe { CHAR_DEVICES[0].device }
            } else {
                None
            }
        })
}

/// Get character device count.
pub fn char_count() -> usize {
    CHAR_COUNT.load(Ordering::Acquire)
}

// ============================================================================
// Network Device Registration
// ============================================================================

/// Register a network device.
pub fn register_net(name: &'static str, device: &'static dyn NetDevice) -> bool {
    let count = NET_COUNT.load(Ordering::Acquire);
    if count >= MAX_DEVICES {
        return false;
    }

    unsafe {
        NET_DEVICES[count] = DeviceEntry {
            name,
            device: Some(device),
        };
    }
    NET_COUNT.store(count + 1, Ordering::Release);

    crate::platform::log("  [registry] Registered net device: ");
    crate::platform::log(name);
    crate::platform::log("\n");

    true
}

/// Get a network device by name.
pub fn get_net(name: &str) -> Option<&'static dyn NetDevice> {
    let count = NET_COUNT.load(Ordering::Acquire);
    for i in 0..count {
        unsafe {
            if NET_DEVICES[i].name == name {
                return NET_DEVICES[i].device;
            }
        }
    }
    None
}

/// Get the first available network device.
pub fn get_first_net() -> Option<&'static dyn NetDevice> {
    if NET_COUNT.load(Ordering::Acquire) > 0 {
        unsafe { NET_DEVICES[0].device }
    } else {
        None
    }
}

/// Get network device count.
pub fn net_count() -> usize {
    NET_COUNT.load(Ordering::Acquire)
}

// ============================================================================
// Input Device Registration
// ============================================================================

/// Register an input device.
pub fn register_input(name: &'static str, device: &'static dyn InputDevice) -> bool {
    let count = INPUT_COUNT.load(Ordering::Acquire);
    if count >= MAX_DEVICES {
        return false;
    }

    unsafe {
        INPUT_DEVICES[count] = DeviceEntry {
            name,
            device: Some(device),
        };
    }
    INPUT_COUNT.store(count + 1, Ordering::Release);

    crate::platform::log("  [registry] Registered input device: ");
    crate::platform::log(name);
    crate::platform::log("\n");

    true
}

/// Get an input device by name.
pub fn get_input(name: &str) -> Option<&'static dyn InputDevice> {
    let count = INPUT_COUNT.load(Ordering::Acquire);
    for i in 0..count {
        unsafe {
            if INPUT_DEVICES[i].name == name {
                return INPUT_DEVICES[i].device;
            }
        }
    }
    None
}

/// Get input device count.
pub fn input_count() -> usize {
    INPUT_COUNT.load(Ordering::Acquire)
}

/// Poll all input devices for events.
pub fn poll_all_input() -> Option<InputEvent> {
    let count = INPUT_COUNT.load(Ordering::Acquire);
    for i in 0..count {
        unsafe {
            if let Some(dev) = INPUT_DEVICES[i].device {
                if let Some(event) = dev.poll_event() {
                    return Some(event);
                }
            }
        }
    }
    None
}

// ============================================================================
// Display Device Registration
// ============================================================================

/// Register a display device.
pub fn register_display(name: &'static str, device: &'static dyn DisplayDevice) -> bool {
    let count = DISPLAY_COUNT.load(Ordering::Acquire);
    if count >= MAX_DEVICES {
        return false;
    }

    unsafe {
        DISPLAY_DEVICES[count] = DeviceEntry {
            name,
            device: Some(device),
        };
    }
    DISPLAY_COUNT.store(count + 1, Ordering::Release);

    crate::platform::log("  [registry] Registered display device: ");
    crate::platform::log(name);
    crate::platform::log("\n");

    true
}

/// Get a display device by name.
pub fn get_display(name: &str) -> Option<&'static dyn DisplayDevice> {
    let count = DISPLAY_COUNT.load(Ordering::Acquire);
    for i in 0..count {
        unsafe {
            if DISPLAY_DEVICES[i].name == name {
                return DISPLAY_DEVICES[i].device;
            }
        }
    }
    None
}

/// Get the first available display device.
pub fn get_first_display() -> Option<&'static dyn DisplayDevice> {
    if DISPLAY_COUNT.load(Ordering::Acquire) > 0 {
        unsafe { DISPLAY_DEVICES[0].device }
    } else {
        None
    }
}

/// Get display device count.
pub fn display_count() -> usize {
    DISPLAY_COUNT.load(Ordering::Acquire)
}

// ============================================================================
// Registry Info
// ============================================================================

/// Print registry status.
pub fn print_status() {
    crate::platform::log("\n  Driver Registry:\n");

    crate::platform::log("    Block devices:   ");
    crate::platform::log_num(block_count() as u64);
    crate::platform::log("\n");

    crate::platform::log("    Char devices:    ");
    crate::platform::log_num(char_count() as u64);
    crate::platform::log("\n");

    crate::platform::log("    Net devices:     ");
    crate::platform::log_num(net_count() as u64);
    crate::platform::log("\n");

    crate::platform::log("    Input devices:   ");
    crate::platform::log_num(input_count() as u64);
    crate::platform::log("\n");

    crate::platform::log("    Display devices: ");
    crate::platform::log_num(display_count() as u64);
    crate::platform::log("\n");

    // List block devices with details
    if block_count() > 0 {
        crate::platform::log("\n    Block device details:\n");
        for (name, dev) in block_devices() {
            crate::platform::log("      ");
            crate::platform::log(name);
            crate::platform::log(": ");
            crate::platform::log_num(dev.block_count());
            crate::platform::log(" blocks (");
            crate::platform::log_num(dev.size_bytes() / 1024 / 1024);
            crate::platform::log(" MB)");
            if dev.is_read_only() {
                crate::platform::log(" [RO]");
            }
            crate::platform::log("\n");
        }
    }
}
