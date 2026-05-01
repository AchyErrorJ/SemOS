//! PS/2 Keyboard Driver
//!
//! Handles scancode set 1 (default for IBM PC compatible keyboards).
//! Supports lowercase, uppercase (shift/caps lock), and special keys.
//! Provides a ring buffer for asynchronous key consumption.
//!
//! # Scancode Set 1 Key Ranges
//!
//! | Scancode | Key                    |
//! |----------|------------------------|
//! | 0x01     | Escape                 |
//! | 0x02-0x0D| 1-9, 0, -, =           |
//! | 0x0E     | Backspace              |
//! | 0x0F     | Tab                    |
//! | 0x10-0x1C| q-p, [, ], Enter       |
//! | 0x1D     | Left Ctrl              |
//! | 0x1E-0x28| a-l, ;, '             |
//! | 0x29     | Backtick               |
//! | 0x2A     | Left Shift             |
//! | 0x2B     | Backslash              |
//! | 0x2C-0x35| z-m, comma, ., /       |
//! | 0x36     | Right Shift            |
//! | 0x38     | Left Alt               |
//! | 0x39     | Space                  |
//! | 0x3A     | Caps Lock              |
//! | 0x80+    | Key release (scancode + 0x80) |

use spin::Mutex;

/// Ring buffer size (must be power of 2)
const KEY_BUF_SIZE: usize = 64;

/// Keyboard state
struct KeyboardState {
    /// Key ring buffer
    buffer: [u8; KEY_BUF_SIZE],
    /// Read position
    read_pos: usize,
    /// Write position
    write_pos: usize,
    /// Shift held
    shift: bool,
    /// Caps lock active
    caps_lock: bool,
    /// Ctrl held
    ctrl: bool,
}

impl KeyboardState {
    const fn new() -> Self {
        Self {
            buffer: [0; KEY_BUF_SIZE],
            read_pos: 0,
            write_pos: 0,
            shift: false,
            caps_lock: false,
            ctrl: false,
        }
    }

    fn push(&mut self, key: u8) {
        let next = (self.write_pos + 1) & (KEY_BUF_SIZE - 1);
        if next != self.read_pos {
            self.buffer[self.write_pos] = key;
            self.write_pos = next;
        }
        // Drop key if buffer full
    }

    fn pop(&mut self) -> Option<u8> {
        if self.read_pos == self.write_pos {
            None
        } else {
            let key = self.buffer[self.read_pos];
            self.read_pos = (self.read_pos + 1) & (KEY_BUF_SIZE - 1);
            Some(key)
        }
    }
}

static KEYBOARD: Mutex<KeyboardState> = Mutex::new(KeyboardState::new());

/// Lowercase scancode-to-ASCII table (scancode set 1, index 0x00-0x39)
static SCANCODE_TABLE: [u8; 58] = [
    0,    // 0x00: (none)
    0x1B, // 0x01: Escape
    b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', // 0x02-0x0B
    b'-', b'=',       // 0x0C-0x0D
    0x08,             // 0x0E: Backspace
    b'\t',            // 0x0F: Tab
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', b'o', b'p', // 0x10-0x19
    b'[', b']',       // 0x1A-0x1B
    b'\n',            // 0x1C: Enter
    0,                // 0x1D: Left Ctrl (modifier)
    b'a', b's', b'd', b'f', b'g', b'h', b'j', b'k', b'l',       // 0x1E-0x26
    b';', b'\'',      // 0x27-0x28
    b'`',             // 0x29: Backtick
    0,                // 0x2A: Left Shift (modifier)
    b'\\',            // 0x2B: Backslash
    b'z', b'x', b'c', b'v', b'b', b'n', b'm',                   // 0x2C-0x32
    b',', b'.', b'/', // 0x33-0x35
    0,                // 0x36: Right Shift (modifier)
    b'*',             // 0x37: Keypad *
    0,                // 0x38: Left Alt (modifier)
    b' ',             // 0x39: Space
];

/// Shifted scancode-to-ASCII table
static SCANCODE_TABLE_SHIFT: [u8; 58] = [
    0,    // 0x00
    0x1B, // 0x01: Escape
    b'!', b'@', b'#', b'$', b'%', b'^', b'&', b'*', b'(', b')', // 0x02-0x0B
    b'_', b'+',       // 0x0C-0x0D
    0x08,             // 0x0E: Backspace
    b'\t',            // 0x0F: Tab
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', b'O', b'P', // 0x10-0x19
    b'{', b'}',       // 0x1A-0x1B
    b'\n',            // 0x1C: Enter
    0,                // 0x1D: Left Ctrl
    b'A', b'S', b'D', b'F', b'G', b'H', b'J', b'K', b'L',       // 0x1E-0x26
    b':', b'"',       // 0x27-0x28
    b'~',             // 0x29
    0,                // 0x2A: Left Shift
    b'|',             // 0x2B
    b'Z', b'X', b'C', b'V', b'B', b'N', b'M',                   // 0x2C-0x32
    b'<', b'>', b'?', // 0x33-0x35
    0,                // 0x36: Right Shift
    b'*',             // 0x37
    0,                // 0x38: Left Alt
    b' ',             // 0x39: Space
];

/// Process a scancode from the PS/2 keyboard controller.
/// Called from the keyboard interrupt handler.
pub fn handle_scancode(scancode: u8) {
    let mut kb = KEYBOARD.lock();

    // Key release (bit 7 set)
    if scancode & 0x80 != 0 {
        let released = scancode & 0x7F;
        match released {
            0x2A | 0x36 => kb.shift = false,   // Shift released
            0x1D => kb.ctrl = false,            // Ctrl released
            _ => {}
        }
        return;
    }

    // Key press
    match scancode {
        0x2A | 0x36 => { kb.shift = true; return; }   // Shift pressed
        0x1D => { kb.ctrl = true; return; }            // Ctrl pressed
        0x3A => { kb.caps_lock = !kb.caps_lock; return; } // Caps Lock toggle
        _ => {}
    }

    if (scancode as usize) >= SCANCODE_TABLE.len() {
        return;
    }

    let shifted = kb.shift;
    let caps = kb.caps_lock;

    let mut c = if shifted {
        SCANCODE_TABLE_SHIFT[scancode as usize]
    } else {
        SCANCODE_TABLE[scancode as usize]
    };

    // Caps lock only affects letters
    if caps && c != 0 {
        if c >= b'a' && c <= b'z' {
            c -= 32; // to uppercase
        } else if c >= b'A' && c <= b'Z' {
            c += 32; // to lowercase (caps + shift = lowercase)
        }
    }

    if c != 0 {
        kb.push(c);
        // Echo to serial for visibility
        crate::serial::Serial::put_char(c as char);
    }
}

/// Read a key from the buffer (non-blocking).
pub fn read_key() -> Option<u8> {
    KEYBOARD.lock().pop()
}

/// Check if there are keys available.
#[allow(dead_code)]
pub fn has_key() -> bool {
    let kb = KEYBOARD.lock();
    kb.read_pos != kb.write_pos
}
