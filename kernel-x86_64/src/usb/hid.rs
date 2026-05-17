//! USB HID boot-protocol keyboard parsing.
//!
//! Once we set the HID interface protocol to "boot" (HID_SET_PROTOCOL,
//! wValue=0), the keyboard sends a fixed 8-byte report on its Interrupt
//! IN endpoint every time the key state changes:
//!
//! | Byte | Meaning                                                |
//! |------|--------------------------------------------------------|
//! | 0    | Modifier bitmap (LCtrl/LShift/.../RGUI)                |
//! | 1    | Reserved (always 0 on host-side; OEM may use)          |
//! | 2..7 | Up to six concurrently-pressed keycodes (HID Usage IDs)|
//!
//! Reference: USB HID 1.11 spec §B (Boot Interface Subclass), and
//! Usage Tables 1.12 §10 (Keyboard/Keypad page 0x07).
//!
//! We deliberately do not parse the full HID report descriptor — boot
//! protocol bypasses it. A full HID stack belongs in a later task.
//!
//! # Mouse hook
//!
//! The same boot-protocol idea applies to mice: 3-byte report (buttons,
//! dx, dy). A future `usb-mouse` task would add a parallel module here
//! and reuse the transfer-ring polling machinery. Not in scope for
//! this task.

/// A single boot-protocol keyboard report (8 bytes).
#[derive(Copy, Clone, Default)]
pub struct KeyboardReport {
    pub modifiers: u8,
    pub _reserved: u8,
    pub keys: [u8; 6],
}

impl KeyboardReport {
    /// Build from the raw 8-byte buffer the device wrote into our DMA region.
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 8 { return None; }
        Some(Self {
            modifiers: buf[0],
            _reserved: buf[1],
            keys: [buf[2], buf[3], buf[4], buf[5], buf[6], buf[7]],
        })
    }

    /// True if any modifier bit is set (LCtrl=1, LShift=2, LAlt=4, LGUI=8,
    /// RCtrl=16, RShift=32, RAlt=64, RGUI=128).
    pub fn shift_held(&self) -> bool {
        (self.modifiers & 0x22) != 0
    }

    /// Iterate the keycodes that are non-zero AND non-error.
    pub fn pressed_keys(&self) -> impl Iterator<Item = u8> + '_ {
        // 0x01..0x03 are ErrorRollOver / POSTFail / ErrorUndefined — drop those.
        self.keys.iter().copied().filter(|&k| k >= 4)
    }
}

/// Map a HID Usage ID (Keyboard/Keypad page, §10 of HID Usage Tables) to
/// a printable ASCII byte. Covers the boot subset: a-z, 0-9, space,
/// return, tab, backspace, plus a handful of common punctuation. Anything
/// else returns None (e.g. F-keys, arrows, modifiers).
pub fn keycode_to_ascii(code: u8, shifted: bool) -> Option<u8> {
    // Tables from HID Usage Tables 1.12 §10.
    match code {
        // 0x04..0x1D = A..Z
        0x04..=0x1D => {
            let base = b'a' + (code - 0x04);
            Some(if shifted { base - 32 } else { base })
        }
        // 0x1E..0x26 = 1..9
        0x1E..=0x26 => {
            let digit = code - 0x1E + 1; // 1..9
            if shifted {
                // Shift+digit shifted symbols on a US layout.
                let sym = b"!@#$%^&*(";
                Some(sym[(digit - 1) as usize])
            } else {
                Some(b'0' + digit)
            }
        }
        0x27 => Some(if shifted { b')' } else { b'0' }),
        0x28 => Some(b'\n'),        // Return
        0x29 => None,               // Escape
        0x2A => Some(0x08),         // Backspace
        0x2B => Some(b'\t'),        // Tab
        0x2C => Some(b' '),         // Space
        0x2D => Some(if shifted { b'_' } else { b'-' }),
        0x2E => Some(if shifted { b'+' } else { b'=' }),
        0x2F => Some(if shifted { b'{' } else { b'[' }),
        0x30 => Some(if shifted { b'}' } else { b']' }),
        0x31 => Some(if shifted { b'|' } else { b'\\' }),
        0x33 => Some(if shifted { b':' } else { b';' }),
        0x34 => Some(if shifted { b'"' } else { b'\'' }),
        0x35 => Some(if shifted { b'~' } else { b'`' }),
        0x36 => Some(if shifted { b'<' } else { b',' }),
        0x37 => Some(if shifted { b'>' } else { b'.' }),
        0x38 => Some(if shifted { b'?' } else { b'/' }),
        _ => None,
    }
}

#[cfg(any())]  // disabled — no_std kernel; tests would belong in a host crate.
#[test]
fn ascii_letter_lowercase() {
    assert_eq!(keycode_to_ascii(0x04, false), Some(b'a'));
    assert_eq!(keycode_to_ascii(0x1D, false), Some(b'z'));
}
