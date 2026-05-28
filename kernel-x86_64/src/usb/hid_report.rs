//! HID report descriptor parser (M16 — the "real" one, not boot-protocol).
//!
//! Walks a HID 1.11 report descriptor and emits a flat table of *Input* fields
//! with their bit offsets, sizes, and logical ranges. Apply that table to a raw
//! input report to extract gamepad axes + buttons.
//!
//! The parser is pure (no IO), so DEMO 64 validates it against a canned
//! generic-gamepad descriptor + a synthetic report — important because QEMU
//! has no gamepad device to test the live xHCI path against, so v1 ships the
//! parser hardware-ready and wires it to a real device on metal later.

/// Common usage pages we care about.
pub const USAGE_PAGE_GENERIC_DESKTOP: u16 = 0x01;
pub const USAGE_PAGE_BUTTON: u16 = 0x09;

/// Generic Desktop usages (subset).
pub mod gd {
    pub const JOYSTICK: u16 = 0x04;
    pub const GAME_PAD: u16 = 0x05;
    pub const X: u16 = 0x30;
    pub const Y: u16 = 0x31;
    pub const Z: u16 = 0x32;
    pub const RX: u16 = 0x33;
    pub const RY: u16 = 0x34;
    pub const RZ: u16 = 0x35;
    pub const HAT: u16 = 0x39;
}

const MAX_FIELDS: usize = 64;
const MAX_USAGES: usize = 16;

/// One Input field extracted from the descriptor.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub usage_page: u16,
    pub usage: u16,
    pub bit_offset: u16,
    pub bit_size: u8,
    pub signed: bool,
    pub logical_min: i32,
    pub logical_max: i32,
}

/// Parsed layout — a fixed-size array (no_std, no alloc dependency).
pub struct ReportLayout {
    fields: [Option<Field>; MAX_FIELDS],
    count: usize,
}

impl ReportLayout {
    pub const fn new() -> Self {
        const NONE: Option<Field> = None;
        Self {
            fields: [NONE; MAX_FIELDS],
            count: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn fields(&self) -> &[Option<Field>] {
        &self.fields[..self.count]
    }

    fn push(&mut self, f: Field) {
        if self.count < MAX_FIELDS {
            self.fields[self.count] = Some(f);
            self.count += 1;
        }
    }

    /// First field matching `(usage_page, usage)`, or None.
    pub fn find(&self, usage_page: u16, usage: u16) -> Option<Field> {
        for f in self.fields[..self.count].iter().flatten() {
            if f.usage_page == usage_page && f.usage == usage {
                return Some(*f);
            }
        }
        None
    }

    /// Extract one field's value from a raw report. Signed fields are
    /// sign-extended from `bit_size` to i32. Bits past the report bound read
    /// as 0 (defensive — malformed reports shouldn't fault the kernel).
    pub fn read(&self, f: Field, report: &[u8]) -> i32 {
        let mut acc: u32 = 0;
        let mut bo = f.bit_offset as usize;
        let total_bits = report.len() * 8;
        for bit in 0..f.bit_size as usize {
            if bo >= total_bits {
                break;
            }
            let v = (report[bo / 8] >> (bo % 8)) & 1;
            acc |= (v as u32) << bit;
            bo += 1;
        }
        if f.signed && f.bit_size > 0 && f.bit_size < 32 {
            let sign_bit = 1u32 << (f.bit_size - 1);
            if acc & sign_bit != 0 {
                let mask = !0u32 << f.bit_size;
                return (acc | mask) as i32;
            }
        }
        acc as i32
    }
}

// --- internal parser state ---

#[derive(Default, Clone, Copy)]
struct Globals {
    usage_page: u16,
    logical_min: i32,
    logical_max: i32,
    report_size: u32,
    report_count: u32,
    _report_id: u8,
}

#[derive(Default)]
struct Locals {
    usage: [u16; MAX_USAGES],
    usage_n: usize,
    usage_min: Option<u16>,
    usage_max: Option<u16>,
}

impl Locals {
    fn reset(&mut self) {
        self.usage_n = 0;
        self.usage_min = None;
        self.usage_max = None;
    }
}

/// Parse a HID report descriptor into an Input-field table. Returns `None` on
/// malformed input (truncated item, etc.). Long items (rare) are skipped.
pub fn parse(desc: &[u8]) -> Option<ReportLayout> {
    let mut out = ReportLayout::new();
    let mut g = Globals::default();
    let mut l = Locals::default();
    let mut bit_offset: u32 = 0;

    let mut i = 0;
    while i < desc.len() {
        let item = desc[i];
        // Long item: tag 0xFE — has a 2-byte size header. Skip cleanly.
        if item == 0xFE {
            if i + 2 >= desc.len() {
                return None;
            }
            let long_size = desc[i + 1] as usize;
            i += 3 + long_size;
            if i > desc.len() {
                return None;
            }
            continue;
        }
        let size_code = item & 0x03;
        let item_type = (item >> 2) & 0x03;
        let item_tag = (item >> 4) & 0x0F;
        let size = match size_code {
            0 => 0,
            1 => 1,
            2 => 2,
            3 => 4,
            _ => 0,
        };
        i += 1;
        if i + size > desc.len() {
            return None;
        }
        let data_u32: u32 = match size {
            0 => 0,
            1 => desc[i] as u32,
            2 => (desc[i] as u32) | ((desc[i + 1] as u32) << 8),
            4 => {
                (desc[i] as u32)
                    | ((desc[i + 1] as u32) << 8)
                    | ((desc[i + 2] as u32) << 16)
                    | ((desc[i + 3] as u32) << 24)
            }
            _ => 0,
        };
        // Signed interpretation for items whose value is naturally signed
        // (Logical Min/Max etc.). Sign-extend per the read size.
        let data_i32: i32 = match size {
            1 => desc[i] as i8 as i32,
            2 => ((desc[i] as i16) | ((desc[i + 1] as i16) << 8)) as i32,
            4 => data_u32 as i32,
            _ => 0,
        };
        i += size;

        match item_type {
            // Main items.
            0 => {
                if item_tag == 0x8 {
                    // Input — emit `report_count` fields of `report_size` bits.
                    let signed = g.logical_min < 0;
                    for k in 0..g.report_count {
                        let usage = if let (Some(min), Some(_max)) = (l.usage_min, l.usage_max) {
                            min.wrapping_add(k as u16)
                        } else if l.usage_n > 0 {
                            let idx = (k as usize).min(l.usage_n - 1);
                            l.usage[idx]
                        } else {
                            0
                        };
                        out.push(Field {
                            usage_page: g.usage_page,
                            usage,
                            bit_offset: bit_offset as u16,
                            bit_size: g.report_size as u8,
                            signed,
                            logical_min: g.logical_min,
                            logical_max: g.logical_max,
                        });
                        bit_offset += g.report_size;
                    }
                } else if item_tag == 0x9 || item_tag == 0xB {
                    // Output / Feature — advance bit_offset (so subsequent Inputs
                    // line up if mixed in one report), but don't record them.
                    bit_offset += g.report_size * g.report_count;
                }
                // Locals reset on EVERY Main item (Collection/EndCollection too).
                l.reset();
            }
            // Global items.
            1 => match item_tag {
                0x0 => g.usage_page = data_u32 as u16,
                0x1 => g.logical_min = data_i32,
                0x2 => g.logical_max = data_i32,
                0x7 => g.report_size = data_u32,
                0x8 => g._report_id = data_u32 as u8,
                0x9 => g.report_count = data_u32,
                _ => {}
            },
            // Local items.
            2 => match item_tag {
                0x0 => {
                    if l.usage_n < MAX_USAGES {
                        l.usage[l.usage_n] = data_u32 as u16;
                        l.usage_n += 1;
                    }
                }
                0x1 => l.usage_min = Some(data_u32 as u16),
                0x2 => l.usage_max = Some(data_u32 as u16),
                _ => {}
            },
            _ => {}
        }
    }
    Some(out)
}

/// Decoded gamepad snapshot — standard axes + button bitmask (bit N-1 = btn N).
#[derive(Default, Clone, Copy, Debug)]
pub struct GamepadState {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub rx: i32,
    pub ry: i32,
    pub rz: i32,
    pub hat: i32,
    pub buttons: u32,
}

/// Apply a parsed layout to a raw input report and pull standard gamepad axes
/// + the first 32 buttons out.
pub fn decode_gamepad(layout: &ReportLayout, report: &[u8]) -> GamepadState {
    let mut s = GamepadState::default();
    let g = USAGE_PAGE_GENERIC_DESKTOP;
    if let Some(f) = layout.find(g, gd::X) { s.x = layout.read(f, report); }
    if let Some(f) = layout.find(g, gd::Y) { s.y = layout.read(f, report); }
    if let Some(f) = layout.find(g, gd::Z) { s.z = layout.read(f, report); }
    if let Some(f) = layout.find(g, gd::RX) { s.rx = layout.read(f, report); }
    if let Some(f) = layout.find(g, gd::RY) { s.ry = layout.read(f, report); }
    if let Some(f) = layout.find(g, gd::RZ) { s.rz = layout.read(f, report); }
    if let Some(f) = layout.find(g, gd::HAT) { s.hat = layout.read(f, report); }
    for f in layout.fields().iter().flatten() {
        if f.usage_page == USAGE_PAGE_BUTTON && f.usage >= 1 && f.usage <= 32 {
            let v = layout.read(*f, report);
            if v != 0 {
                s.buttons |= 1u32 << (f.usage - 1);
            }
        }
    }
    s
}
