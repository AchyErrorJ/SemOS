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
    /// The previous byte was the 0xE0 extended-scancode prefix.
    ext: bool,
    /// ThinkPad Fn key held. The Fn key itself sends 0xE0 0x63 press /
    /// 0xE0 0xE3 release. While this flag is set we intercept Fn+F5/F6
    /// for brightness control.
    fn_key: bool,
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
            ext: false,
            fn_key: false,
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

/// "User pressed ESC during the demo dispatch" flag. Read by
/// `init_loader_task` to short-circuit out of the per-demo loop and
/// land in the interactive shell. Set unconditionally when scancode
/// 0x01 (Escape, scancode set 1) arrives — works whether the press
/// reached us via the IOAPIC IRQ path or the timer-polled fallback.
pub static SKIP_DEMOS: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Ctrl+C abort. Set when Ctrl+C is detected (works via the timer-polled
/// keyboard path, so it fires even while a command is mid-loop). Long-
/// running kernel operations (USB enumeration, network waits) poll
/// [`abort_requested`] and bail early. The shell clears it before each
/// command via [`clear_abort`].
pub static ABORT_REQUESTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// True if the user pressed Ctrl+C since the last [`clear_abort`].
pub fn abort_requested() -> bool {
    ABORT_REQUESTED.load(core::sync::atomic::Ordering::Relaxed)
}

/// Clear the Ctrl+C abort flag (call before starting a fresh command).
pub fn clear_abort() {
    ABORT_REQUESTED.store(false, core::sync::atomic::Ordering::Relaxed);
}

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

/// Initialize the PS/2 controller (i8042) and keyboard.
///
/// On QEMU the firmware leaves the keyboard fully enabled, so a kernel
/// init isn't required. On real hardware — especially ThinkPad BIOS —
/// the i8042 POST self-test leaves *scanning* disabled (port 0x60 0xF4
/// command is the OS's job). Without this, the IOAPIC delivers IRQ 1
/// only on lid-open style state changes, not on actual keypresses.
///
/// Sequence per the PS/2 controller spec
/// (<https://wiki.osdev.org/%228042%22_PS/2_Controller>):
///   1. Disable both PS/2 ports (commands 0xAD, 0xA7).
///   2. Flush the output buffer (read 0x60 until status bit 0 is clear).
///   3. Read config byte (0x20), enable IRQ + translation, write back (0x60).
///   4. Self-test the controller (0xAA → expect 0x55).
///   5. Re-enable port 1 (0xAE).
///   6. Send 0xF4 (Enable Scanning) to the keyboard via port 0x60.
///
/// Best-effort: any step failing logs and continues. The interrupt
/// handler will still fire if the IOAPIC RTE is programmed; this just
/// gets us from "wired but quiet" → "actually delivers scancodes."
pub fn init() {
    use x86_64::instructions::port::Port;

    const PORT_DATA: u16 = 0x60;
    const PORT_STATUS: u16 = 0x64; // read = status, write = command
    const STATUS_OUTPUT_FULL: u8 = 1 << 0;
    const STATUS_INPUT_FULL: u8 = 1 << 1;

    let mut data: Port<u8> = Port::new(PORT_DATA);
    let mut cmd: Port<u8> = Port::new(PORT_STATUS);

    // Bounded busy-wait helpers — never block the kernel if the
    // controller is dead.
    let wait_input_clear = |cmd: &mut Port<u8>| -> bool {
        for _ in 0..100_000u32 {
            let s = unsafe { cmd.read() };
            if s & STATUS_INPUT_FULL == 0 { return true; }
        }
        false
    };
    let wait_output_full = |cmd: &mut Port<u8>| -> bool {
        for _ in 0..100_000u32 {
            let s = unsafe { cmd.read() };
            if s & STATUS_OUTPUT_FULL != 0 { return true; }
        }
        false
    };

    // 1. Disable both ports so the keyboard can't interrupt us mid-init.
    if !wait_input_clear(&mut cmd) {
        crate::println!("[ps/2] timeout before disable port 1");
    }
    unsafe { cmd.write(0xAD); } // disable port 1
    if !wait_input_clear(&mut cmd) {
        crate::println!("[ps/2] timeout before disable port 2");
    }
    unsafe { cmd.write(0xA7); } // disable port 2 (no-op if absent)

    // 2. Flush output buffer.
    for _ in 0..16 {
        let s = unsafe { cmd.read() };
        if s & STATUS_OUTPUT_FULL == 0 { break; }
        let _ = unsafe { data.read() };
    }

    // 3. Read controller config byte (command 0x20).
    if !wait_input_clear(&mut cmd) {
        crate::println!("[ps/2] timeout before read config");
        return;
    }
    unsafe { cmd.write(0x20); }
    if !wait_output_full(&mut cmd) {
        crate::println!("[ps/2] timeout waiting for config byte");
        return;
    }
    let mut config = unsafe { data.read() };
    // Enable IRQ for port 1 (bit 0), enable scancode set-1 translation
    // (bit 6). Clear port-1-disable (bit 4) and port-2-IRQ (bit 1).
    config |= (1 << 0) | (1 << 6);
    config &= !((1 << 1) | (1 << 4));

    if !wait_input_clear(&mut cmd) {
        crate::println!("[ps/2] timeout before write config");
        return;
    }
    unsafe { cmd.write(0x60); } // write config byte (next byte to 0x60)
    if !wait_input_clear(&mut cmd) {
        crate::println!("[ps/2] timeout before config payload");
        return;
    }
    unsafe { data.write(config); }

    // 4. Controller self-test. Skip on failure but keep going — some
    // ThinkPad firmware fails this even though the controller works.
    if !wait_input_clear(&mut cmd) {
        crate::println!("[ps/2] timeout before self-test");
    } else {
        unsafe { cmd.write(0xAA); }
        if wait_output_full(&mut cmd) {
            let result = unsafe { data.read() };
            if result != 0x55 {
                crate::println!("[ps/2] controller self-test returned 0x{:02X} (expected 0x55)", result);
            }
            // Self-test resets the config; re-write it.
            if wait_input_clear(&mut cmd) {
                unsafe { cmd.write(0x60); }
                if wait_input_clear(&mut cmd) {
                    unsafe { data.write(config); }
                }
            }
        } else {
            crate::println!("[ps/2] controller self-test timed out");
        }
    }

    // 5. Enable port 1.
    if !wait_input_clear(&mut cmd) {
        crate::println!("[ps/2] timeout before enable port 1");
        return;
    }
    unsafe { cmd.write(0xAE); }

    // 6. Send 0xF4 (Enable Scanning) to the keyboard. Drain its 0xFA ACK
    // (or 0xFE resend) — bounded so we don't spin if the kbd is absent.
    if !wait_input_clear(&mut cmd) {
        crate::println!("[ps/2] timeout before enable scanning");
        return;
    }
    unsafe { data.write(0xF4); }
    let mut ack = 0xFFu8;
    if wait_output_full(&mut cmd) {
        ack = unsafe { data.read() };
    }
    crate::println!("[ps/2] init complete (config=0x{:02X}, enable-scan ack=0x{:02X})", config, ack);
}

/// Poll the i8042 status port and consume any pending scancode.
///
/// Bypass for the IRQ-delivery path when the IOAPIC RTE for IRQ 1 isn't
/// programmed correctly (e.g. ACPI MADT Interrupt Source Override that
/// remaps IRQ 1 to a non-1 pin, which we don't yet parse). Designed to
/// be called from a hot loop — the shell's `wait_for_key`, the idle
/// task, or each scheduler tick. Reads at most a few bytes per call so
/// we never block.
/// Counters for diagnosis — `[ps/2 poll] bytes=N kbd=K aux=A` lines
/// every ~1 s tell us whether bytes are arriving at all and how the
/// AUX bit splits. Critical for figuring out why typing produces
/// nothing on real hardware.
pub static POLL_BYTES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static POLL_KBD:   core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static POLL_AUX:   core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn poll_one_scancode() -> bool {
    use x86_64::instructions::port::Port;
    use core::sync::atomic::Ordering;
    const PORT_DATA: u16 = 0x60;
    const PORT_STATUS: u16 = 0x64;
    const STATUS_OUTPUT_FULL: u8 = 1 << 0;
    const STATUS_AUX: u8 = 1 << 5;

    let mut data: Port<u8> = Port::new(PORT_DATA);
    let mut status: Port<u8> = Port::new(PORT_STATUS);

    let mut progressed = false;
    for _ in 0..8 {
        let s = unsafe { status.read() };
        if s & STATUS_OUTPUT_FULL == 0 { break; }
        let byte = unsafe { data.read() };
        progressed = true;
        POLL_BYTES.fetch_add(1, Ordering::Relaxed);
        if s & STATUS_AUX != 0 {
            POLL_AUX.fetch_add(1, Ordering::Relaxed);
        } else {
            POLL_KBD.fetch_add(1, Ordering::Relaxed);
        }
        // Some W540 firmware sets bit 5 spuriously on keyboard bytes,
        // so dispatch ALL bytes through handle_scancode rather than
        // gating on AUX. Trackpoint bytes that happen to match real
        // scancodes are the worst-case noise (rare on a stationary
        // trackpoint with no buttons pressed), and that's much better
        // than dropping every keypress.
        handle_scancode(byte);
    }
    progressed
}

/// Called from the timer ISR; emits a `[ps/2 poll] bytes=... kbd=... aux=...`
/// line roughly once per second so we can see what's arriving without
/// flooding the console.
pub fn report_poll_stats(tick: u64) {
    use core::sync::atomic::Ordering;
    // Diagnostic for "typing produces nothing on real hardware" — solved.
    // Off by default: the cumulative-counter gate below means that once
    // ANY PS/2 byte ever arrives (e.g. a fullscreen app reads a key), the line would
    // print every second for the rest of the session.
    const PS2_POLL_DIAG: bool = false;
    if !PS2_POLL_DIAG { return; }
    if tick % 62 != 0 { return; }
    let b = POLL_BYTES.load(Ordering::Relaxed);
    if b == 0 { return; }
    let k = POLL_KBD.load(Ordering::Relaxed);
    let a = POLL_AUX.load(Ordering::Relaxed);
    crate::println!("[ps/2 poll] bytes={} kbd={} aux={}", b, k, a);
}

/// Adjust the panel brightness by `delta` percent (negative = darker),
/// clamped to the visible floor. Called from the Fn+F5/F6 hotkey path.
fn adjust_brightness(delta: i16) {
    let current = crate::backlight::get_percent().unwrap_or(50) as i16;
    let next = (current + delta).clamp(10, 100) as u8;
    let _ = crate::backlight::set_percent(next);
}

/// Process a scancode from the PS/2 keyboard controller.
/// Called from the keyboard interrupt handler.
pub fn handle_scancode(scancode: u8) {
    // Temporary raw-byte trace. Set to true while debugging "second byte
    // not recorded" / key-combination issues. Logs every byte that reaches
    // the driver, so we can compare observed sequences against expected
    // scancode-set-1 tables.
    const LOG_ALL_SCANCODES: bool = false;
    if LOG_ALL_SCANCODES {
        kernel_core::platform::log("[kbd] byte 0x");
        kernel_core::platform::log_hex_byte(scancode);
        kernel_core::platform::log("\n");
    }

    // ESC (scancode set 1 → 0x01) sets the global skip flag. The demo
    // runner polls this; one keypress is enough to abort the rest of
    // the demo dispatch and land in the interactive shell.
    if scancode == 0x01 {
        SKIP_DEMOS.store(true, core::sync::atomic::Ordering::Relaxed);
    }

    let mut kb = KEYBOARD.lock();

    // Extended-scancode prefix (arrow keys, etc.). Must be checked before the
    // release test, since 0xE0 has bit 7 set.
    if scancode == 0xE0 {
        kb.ext = true;
        return;
    }
    if kb.ext {
        kb.ext = false;
        // Only presses (bit 7 clear). Map the cursor keys to ANSI escapes the
        // TTY line discipline understands: ESC [ A/B/C/D.
        if scancode & 0x80 == 0 {
            // Scrollback paging (consumed here, never sent to the shell). Extended
            // scancode-set-1 codes: PageUp=0x49, PageDown=0x51, End=0x4F. The USB
            // HID path scrolls via 0x4B/0x4E/0x4D; the PS/2 built-in keyboard uses
            // these instead — without them PageUp/Down did nothing on real hardware.
            // We run in IRQ context, so DON'T render here (scroll_view locks CONSOLE
            // and could deadlock against an in-progress print!) — record a pending
            // request and let the main loop apply it.
            match scancode {
                0x49 => { drop(kb); crate::framebuffer::request_scroll(15); return; }   // PageUp
                0x51 => { drop(kb); crate::framebuffer::request_scroll(-15); return; }  // PageDown
                0x4F => { drop(kb); crate::framebuffer::request_scroll(-1_000_000); return; } // End → live
                _ => {}
            }
            let letter = match scancode {
                0x48 => Some(b'A'), // up
                0x50 => Some(b'B'), // down
                0x4D => Some(b'C'), // right
                0x4B => Some(b'D'), // left
                _ => None,
            };
            if let Some(letter) = letter {
                drop(kb);
                crate::tty::input_push(0x1B);
                crate::tty::input_push(b'[');
                crate::tty::input_push(letter);
                return;
            }
            // ThinkPad T540p Fn key: 0xE0 0x63 press, 0xE0 0xE3 release.
            // Track it as a modifier so we can intercept Fn+F5/F6 for
            // brightness control.
            if scancode == 0x63 {
                kb.fn_key = true;
                return;
            }
            if scancode == 0xE3 {
                kb.fn_key = false;
                return;
            }
            // Diagnostic: log any other extended scancode so we can see what
            // Fn+Fx combos (e.g. brightness keys) the T540p delivers. Printed
            // to the framebuffer console so it is visible without a serial cable.
            // This briefly takes the console lock from IRQ context, same as the
            // normal character-echo path in tty::input_push.
            const LOG_UNKNOWN_EXT: bool = true;
            if LOG_UNKNOWN_EXT {
                drop(kb);
                crate::println!("[kbd] unknown ext scancode 0x{:02X}", scancode);
            }
        }
        return;
    }

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
        // Ctrl+C — request abort of the running command. Set the global
        // flag (polled by long-running kernel loops) AND push 0x03 (ETX)
        // into the line discipline so a blocked SYS_READ also unblocks.
        0x2E if kb.ctrl => {
            ABORT_REQUESTED.store(true, core::sync::atomic::Ordering::Relaxed);
            drop(kb);
            crate::tty::input_push(0x03);
            return;
        }
        _ => {}
    }

    // ThinkPad T540p brightness hotkeys: Fn+F5 = down, Fn+F6 = up.
    // The Fn key itself sends 0xE0 0x63 press / 0xE0 0xE3 release, and
    // F5/F6 keep their normal scancodes (0x3F / 0x40) while Fn is held.
    if kb.fn_key {
        match scancode {
            0x3F => { drop(kb); adjust_brightness(-10); return; }
            0x40 => { drop(kb); adjust_brightness(10); return; }
            _ => {}
        }
    }

    // Diagnostic: when Fn is held, log the raw scancode of any other
    // non-extended key so we can discover additional Fn+Fx combos.
    if kb.fn_key {
        crate::println!("[kbd] Fn+key scancode 0x{:02X}", scancode);
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
        // Feed the TTY line discipline (M19); it echoes to serial itself, so
        // drop the lock first to avoid holding KEYBOARD across the STDIN lock.
        drop(kb);
        // If the user is scrolled back into history and starts typing, snap to
        // the live view first so their keystrokes are visible. Deferred to the
        // main loop (we're in IRQ context — see request_scroll).
        if crate::framebuffer::is_scrolled() {
            crate::framebuffer::request_scroll(-1_000_000);
        }
        crate::tty::input_push(c);
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
