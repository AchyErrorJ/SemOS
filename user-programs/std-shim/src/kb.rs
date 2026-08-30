//! Raw keyboard events for fullscreen apps (SYS_KB_POLL).
//!
//! The cooked TTY read path is line-buffered, press-only, and ASCII-only —
//! useless for games. This module drains the kernel's raw key-event ring:
//! press AND release, PS/2 set-1 normalized (USB HID is mapped to the same
//! space kernel-side), delivered as plain u32 records.
//!
//! Event record layout (matches kernel-x86_64/src/keyevents.rs):
//!   bit 31    : 1 = pressed, 0 = released
//!   bit 7     : extended (PS/2 0xE0-prefix) flag
//!   bits 6:0  : PS/2 set-1 scancode

use crate::arch::{syscall2, SYS_KB_POLL};

pub const PRESSED: u32 = 1 << 31;
pub const EXT: u32 = 0x80;

/// Scancode constants (PS/2 set-1; combine arrows with [`EXT`]).
pub mod key {
    pub const ESC: u32 = 0x01;
    pub const ENTER: u32 = 0x1C;
    pub const CTRL: u32 = 0x1D;
    pub const SPACE: u32 = 0x39;
    pub const W: u32 = 0x11;
    pub const A: u32 = 0x1E;
    pub const S: u32 = 0x1F;
    pub const D: u32 = 0x20;
    pub const C: u32 = 0x2E;
    /// Arrow keys arrive with the [`super::EXT`] flag set.
    pub const UP: u32 = super::EXT | 0x48;
    pub const DOWN: u32 = super::EXT | 0x50;
    pub const RIGHT: u32 = super::EXT | 0x4D;
    pub const LEFT: u32 = super::EXT | 0x4B;
}

/// Drain up to `buf.len()` pending events. Returns the count (0 = none).
/// Non-blocking. While a fullscreen app owns the screen (see
/// [`crate::fb::claim`]) the kernel also pumps the USB keyboard on this
/// call, so one poll covers both keyboards.
pub fn poll(buf: &mut [u32]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    let r = unsafe { syscall2(SYS_KB_POLL, buf.as_mut_ptr() as u64, (buf.len() * 4) as u64) };
    if r == u64::MAX {
        return 0;
    }
    r as usize
}

/// True if this event is a key press (false = release).
pub fn pressed(ev: u32) -> bool {
    ev & PRESSED != 0
}

/// The scancode with the EXT flag kept but the PRESSED bit cleared —
/// compare directly against the [`key`] constants (e.g. `code(ev) == key::UP`).
pub fn code(ev: u32) -> u32 {
    ev & (PRESSED - 1)
}
