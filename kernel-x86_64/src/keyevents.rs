//! Raw key-event ring for Ring-3 fullscreen apps (SYS_KB_POLL).
//!
//! The cooked TTY line discipline (tty.rs) is press-only, ASCII-only, and
//! Enter-gated — useless for games. This module tees a parallel raw stream:
//! PS/2 scancodes (press AND release) are pushed from `keyboard::handle_scancode`
//! in IRQ context, and USB HID reports are pumped in by the SYS_KB_POLL
//! handler itself (only while a fullscreen app owns input — see
//! `platform_impl::kb_poll`; the shell's session pump owns USB otherwise).
//!
//! Event record = one u32 (LE):
//!   bit 31    : 1 = pressed, 0 = released
//!   bit 7     : extended (PS/2 0xE0-prefix) flag
//!   bits 6:0  : normalized PS/2 set-1 scancode
//!
//! USB HID usage IDs are normalized to (ext, set-1) pairs at the tap point
//! (`hid_to_set1`) so Ring-3 sees exactly one key space.
//!
//! IRQ discipline: `push` runs inside the keyboard/timer ISR while the
//! caller holds the KEYBOARD lock. Everything here is a short spin-Mutex
//! critical section — no allocation, no printing. Lock order is strictly
//! KEYBOARD → EVENTS; nothing in this module ever takes KEYBOARD.

use spin::Mutex;

const RING_CAP: usize = 64; // power of 2

struct Ring {
    buf: [u32; RING_CAP],
    head: usize, // next write
    tail: usize, // next read
}

impl Ring {
    const fn new() -> Self {
        Ring { buf: [0; RING_CAP], head: 0, tail: 0 }
    }

    /// Drop-oldest on full: a burst of missed events must never block the
    /// ISR, and stale input is worse than lost input for a game.
    fn push(&mut self, ev: u32) {
        self.buf[self.head] = ev;
        self.head = (self.head + 1) & (RING_CAP - 1);
        if self.head == self.tail {
            self.tail = (self.tail + 1) & (RING_CAP - 1);
        }
    }

    fn pop(&mut self) -> Option<u32> {
        if self.tail == self.head {
            return None;
        }
        let ev = self.buf[self.tail];
        self.tail = (self.tail + 1) & (RING_CAP - 1);
        Some(ev)
    }
}

static EVENTS: Mutex<Ring> = Mutex::new(Ring::new());

/// USB edge-detection state: previous report's pressed-key set + modifier
/// byte. Reset on fb_claim so a key held across the handoff doesn't replay.
static USB_PREV: Mutex<([u8; 6], u8)> = Mutex::new(([0u8; 6], 0u8));

const PRESSED: u32 = 1 << 31;
const EXT: u32 = 0x80;

/// Record one event. Called from IRQ context (PS/2 tap) and from the
/// SYS_KB_POLL handler's USB pump. Never blocks, never prints.
pub fn push(pressed: bool, ext: bool, code: u8) {
    let ev = (if pressed { PRESSED } else { 0 })
        | (if ext { EXT } else { 0 })
        | (code & 0x7F) as u32;
    x86_64::instructions::interrupts::without_interrupts(|| {
        EVENTS.lock().push(ev);
    });
}

/// Normalize a USB HID usage ID (Keyboard/Keypad page) to
/// (extended, PS/2 set-1 scancode). Unmapped keys are dropped — the game
/// kit only promises the boot-protocol subset.
fn hid_to_set1(usage: u8) -> Option<(bool, u8)> {
    // HID order a..z from usage 0x04.
    const LETTERS: [u8; 26] = [
        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18,
        0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
    ];
    let m = match usage {
        0x04..=0x1D => (false, LETTERS[(usage - 0x04) as usize]),
        0x1E..=0x26 => (false, usage - 0x1E + 0x02), // 1-9 → set-1 0x02..0x0A
        0x27 => (false, 0x0B),                       // 0
        0x28 => (false, 0x1C),                       // Enter
        0x29 => (false, 0x01),                       // Esc
        0x2A => (false, 0x0E),                       // Backspace
        0x2B => (false, 0x0F),                       // Tab
        0x2C => (false, 0x39),                       // Space
        0x4F => (true, 0x4D),                        // Right
        0x50 => (true, 0x4B),                        // Left
        0x51 => (true, 0x50),                        // Down
        0x52 => (true, 0x48),                        // Up
        _ => return None,
    };
    Some(m)
}

/// Pump pending USB HID reports into the ring, computing press/release
/// edges against USB_PREV. Called by the SYS_KB_POLL handler ONLY while a
/// fullscreen app owns input (FULLSCREEN_APP_ACTIVE) — otherwise the
/// shell's session pump owns the USB keyboard and we must not consume
/// its reports.
pub fn pump_usb_hid() {
    crate::usb::xhci::poll_hid(|rep| {
        let mut prev = USB_PREV.lock();
        // Key edges: in the new set but not the old = press; in the old but
        // not the new = release.
        for k in rep.pressed_keys() {
            if !prev.0.contains(&k) {
                if let Some((ext, code)) = hid_to_set1(k) {
                    push(true, ext, code);
                }
            }
        }
        for &k in prev.0.iter() {
            if k != 0 && !rep.keys.contains(&k) {
                if let Some((ext, code)) = hid_to_set1(k) {
                    push(false, ext, code);
                }
            }
        }
        // Modifier edges (LCtrl/LShift/RCtrl/RShift only — Alt/GUI skipped).
        const MODS: [(u8, bool, u8); 4] = [
            (0x01, false, 0x1D), // LCtrl
            (0x02, false, 0x2A), // LShift
            (0x10, true, 0x1D),  // RCtrl (extended)
            (0x20, false, 0x36), // RShift
        ];
        for (bit, ext, code) in MODS {
            let was = prev.1 & bit != 0;
            let is = rep.modifiers & bit != 0;
            if is != was {
                push(is, ext, code);
            }
        }
        *prev = (rep.keys, rep.modifiers);
    });
}

/// Drain the ring + reset USB edge state. Called on fb_claim(1) so stale
/// pre-claim input can't leak into the game.
pub fn reset() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut r = EVENTS.lock();
        r.head = 0;
        r.tail = 0;
    });
    *USB_PREV.lock() = ([0u8; 6], 0u8);
}

/// Pop up to `cap` events into the user buffer. Returns the count copied.
/// Called from the SYS_KB_POLL handler (syscall context, user pointer
/// already validated by the platform layer).
pub fn drain_into(out: *mut u32, cap: usize) -> usize {
    let mut n = 0;
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut r = EVENTS.lock();
        while n < cap {
            match r.pop() {
                Some(ev) => {
                    unsafe { out.add(n).write_volatile(ev) };
                    n += 1;
                }
                None => break,
            }
        }
    });
    n
}
