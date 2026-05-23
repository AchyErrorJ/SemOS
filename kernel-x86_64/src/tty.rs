//! M7/M8 TTY console — TrueType / anti-aliased text console over the M6 fb.
//!
//! This is the "wire M7/M8 into a console output path" piece: a cursor-managed
//! text console that renders glyphs through the real font stack instead of the
//! 8x8 bitmap. It owns a pixel *region* of the framebuffer, tracks a pen, wraps
//! at the right edge, scrolls the region up a line when the cursor falls off the
//! bottom, and renders in one of two modes:
//!
//!   - `Aa::Sharp`  — M7 1-bit scanline fill (`font::FaceCtx`, one parse per
//!                    `write`, drawn glyph-by-glyph so it can wrap mid-line).
//!   - `Aa::Smooth` — M8 tiny-skia anti-aliased text (`gfx2d::aa_draw_text`,
//!                    one pixmap per line; no mid-line wrap, so keep AA lines
//!                    inside the region width).
//!
//! It is deliberately NOT the kernel's default `print!` sink: the boot console
//! stays on the fast bitmap font (serial is the source of truth, and the M7
//! glyph rasterizer's ~16 KiB stack frame must not run on the interrupt/syscall
//! print path — see the #41/#55 stack-sensitivity notes). DEMO 39 drives this
//! console and verifies it headlessly via pixel readback.

use crate::framebuffer::{self as fb, Color};
use crate::{font, gfx2d};
use spin::Mutex;

/// Glyph rendering mode for a `write`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Aa {
    /// M7: crisp 1-bit scanline fill.
    Sharp,
    /// M8: tiny-skia anti-aliased fill.
    Smooth,
}

/// A TrueType text console bound to a framebuffer region. Cursor state is in
/// pixels; `line_top` is the top of the current text line, `pen_x` the next
/// glyph's left edge.
pub struct TtyConsole {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    px: f32,
    line_h: usize,
    col_w: usize, // nominal cell width (space advance) for ANSI cursor moves
    fg: Color,
    bg: Color,
    pen_x: usize,
    line_top: usize,
    // Line-oriented scrollback: every logical line written is recorded so it
    // can be re-rendered after it scrolls off the region (see show_scrollback).
    sb: [[u8; SB_W]; SB_N],
    sb_len: [usize; SB_N],
    sb_count: usize, // total lines ever pushed (ring index = sb_count % SB_N)
    cur: [u8; SB_W], // line currently being accumulated (until '\n')
    cur_n: usize,
}

/// Scrollback ring: lines kept × max bytes recorded per line.
const SB_N: usize = 64;
const SB_W: usize = 96;

impl TtyConsole {
    /// Create a console over region `(x0, y0, w, h)` with em height `px`,
    /// foreground `fg` on background `bg`. Clears the region to `bg`.
    pub fn new(x0: usize, y0: usize, w: usize, h: usize, px: f32, fg: Color, bg: Color) -> Self {
        let line_h = font::line_height(px).max(px as usize + 2);
        let col_w = font::with_face(px, |f| f.advance(' ') as usize)
            .unwrap_or((px / 2.0) as usize)
            .max(1);
        fb::fb_fill_rect(x0, y0, w, h, bg);
        Self {
            x0, y0, w, h, px, line_h, col_w, fg, bg, pen_x: x0, line_top: y0,
            sb: [[0; SB_W]; SB_N],
            sb_len: [0; SB_N],
            sb_count: 0,
            cur: [0; SB_W],
            cur_n: 0,
        }
    }

    /// Record `text` into the scrollback line buffer (newlines flush a line).
    /// Called by `write` so scrollback captures logical lines regardless of
    /// render mode or wrapping.
    fn record(&mut self, text: &str) {
        for &b in text.as_bytes() {
            if b == b'\n' {
                self.push_line();
            } else if self.cur_n < SB_W {
                self.cur[self.cur_n] = b;
                self.cur_n += 1;
            }
        }
    }

    fn push_line(&mut self) {
        let idx = self.sb_count % SB_N;
        let len = self.cur_n;
        self.sb[idx][..len].copy_from_slice(&self.cur[..len]);
        self.sb_len[idx] = len;
        self.sb_count += 1;
        self.cur_n = 0;
    }

    /// Number of text rows that fit in the region.
    fn visible_rows(&self) -> usize {
        (self.h / self.line_h).max(1)
    }

    /// Oldest scrollback line index still retained in the ring.
    pub fn scrollback_oldest(&self) -> usize {
        self.sb_count.saturating_sub(SB_N)
    }

    /// Total logical lines pushed to scrollback.
    pub fn scrollback_total(&self) -> usize {
        self.sb_count
    }

    /// Re-render the region showing scrollback starting at logical line
    /// `top` (clamped to retained range). Used to scroll back through output
    /// that has scrolled off the live view. Does not disturb the live cursor.
    pub fn show_scrollback(&mut self, top: usize) {
        let rows = self.visible_rows();
        let oldest = self.scrollback_oldest();
        let top = top.max(oldest).min(self.sb_count);
        fb::fb_fill_rect(self.x0, self.y0, self.w, self.h, self.bg);
        for r in 0..rows {
            let line_idx = top + r;
            if line_idx >= self.sb_count {
                break;
            }
            let phys = line_idx % SB_N;
            let len = self.sb_len[phys];
            let baseline = (self.y0 + r * self.line_h + (self.line_h * 4) / 5) as f32;
            let x0 = self.x0 as f32;
            let fg = self.fg;
            if let Ok(s) = core::str::from_utf8(&self.sb[phys][..len]) {
                font::with_face(self.px, |f| {
                    let mut x = x0;
                    for ch in s.chars() {
                        x += f.draw_char(x, baseline, ch, fg);
                    }
                });
            }
        }
    }

    /// Set the foreground color used for subsequent glyphs (ANSI SGR).
    pub fn set_fg(&mut self, fg: Color) {
        self.fg = fg;
    }

    /// Clear the whole region to background and home the cursor (ANSI 2J + H).
    pub fn clear(&mut self) {
        fb::fb_fill_rect(self.x0, self.y0, self.w, self.h, self.bg);
        self.pen_x = self.x0;
        self.line_top = self.y0;
    }

    /// Clear from the cursor to the end of the current line (ANSI K).
    pub fn clear_eol(&mut self) {
        let x = self.pen_x.min(self.x0 + self.w);
        fb::fb_fill_rect(x, self.line_top, (self.x0 + self.w).saturating_sub(x), self.line_h, self.bg);
    }

    /// Move the cursor to 1-based `(row, col)` in nominal cells (ANSI H/f),
    /// clamped to the region.
    pub fn move_cursor(&mut self, row: usize, col: usize) {
        let r = row.saturating_sub(1);
        let c = col.saturating_sub(1);
        let max_top = self.y0 + self.h.saturating_sub(self.line_h);
        self.line_top = (self.y0 + r * self.line_h).min(max_top);
        self.pen_x = (self.x0 + c * self.col_w).min(self.x0 + self.w);
    }

    /// Current line's baseline (≈80% down the line box — leaves room for
    /// descenders). Matches what `draw_char` / `aa_draw_text` expect.
    #[inline]
    fn baseline(&self) -> usize {
        self.line_top + (self.line_h * 4) / 5
    }

    /// Advance to the start of the next line, scrolling the region up by one
    /// line if the next line wouldn't fit.
    fn newline(&mut self) {
        self.pen_x = self.x0;
        if self.line_top + 2 * self.line_h <= self.y0 + self.h {
            self.line_top += self.line_h;
        } else {
            fb::fb_scroll_region(self.x0, self.y0, self.w, self.h, self.line_h, self.bg);
            // line_top stays pinned to the last line.
        }
    }

    /// Pixel rows occupied (current baseline relative to the region top) —
    /// used by tests to confirm the cursor advanced/scrolled.
    pub fn cursor_baseline(&self) -> usize {
        self.baseline()
    }

    /// Write `text` in `mode`, handling `\n`, right-edge wrap (Sharp), and
    /// bottom scroll.
    pub fn write(&mut self, mode: Aa, text: &str) {
        self.record(text); // capture logical lines for scrollback
        match mode {
            Aa::Sharp => {
                // One parse for the whole write; draw glyph-by-glyph so we can
                // wrap at the region's right edge.
                font::with_face(self.px, |face| {
                    for ch in text.chars() {
                        if ch == '\n' {
                            self.newline();
                            continue;
                        }
                        let adv = face.advance(ch);
                        if self.pen_x as f32 + adv > (self.x0 + self.w) as f32 {
                            self.newline();
                        }
                        let nx = face.draw_char(self.pen_x as f32, self.baseline() as f32, ch, self.fg);
                        self.pen_x += nx as usize;
                    }
                });
            }
            Aa::Smooth => {
                // tiny-skia rasterizes a whole run per pixmap, so render one
                // line segment at a time (no mid-line wrap in this mode).
                let mut first = true;
                for seg in text.split('\n') {
                    if !first {
                        self.newline();
                    }
                    first = false;
                    if !seg.is_empty() {
                        let end = gfx2d::aa_draw_text(self.pen_x, self.baseline(), seg, self.px, self.fg);
                        self.pen_x = end;
                    }
                }
            }
        }
    }
}

// ============================================================================
// stdin — cooked-mode line discipline
// ============================================================================
//
// Keystrokes (from the PS/2 ISR or the USB HID poll) feed `input_push`. In
// cooked mode the line accumulates in `pend`; Backspace edits it and Enter
// commits the whole line + '\n' into the readable ring `q`. `drain` (behind
// the SYS_READ → platform::stdin_read path) pulls committed bytes out. Each
// keystroke is echoed to serial immediately so on-metal typing is visible
// even before a reader consumes it; colored/visual echo is the reader's job.

const PEND_MAX: usize = 256;
const QUEUE_MAX: usize = 1024;
const HIST_N: usize = 8; // remembered command lines
const HIST_W: usize = 128; // max bytes stored per history line

/// Input-escape parser state. Arrow keys arrive as `ESC [ A/B/C/D` (the
/// keyboard layer emits these for the cursor keys); we parse them here for
/// in-line editing + history recall, distinct from `AnsiTty` which parses
/// *output* escapes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InEsc {
    Normal,
    Esc,
    Csi,
}

struct LineState {
    // Pending (cooked) line being edited, with an in-line insertion cursor.
    pend: [u8; PEND_MAX],
    pend_n: usize,
    cursor: usize, // 0..=pend_n
    // Committed-byte ring drained by SYS_READ.
    q: [u8; QUEUE_MAX],
    head: usize,
    tail: usize,
    count: usize,
    // Command history (oldest..newest in 0..hist_count) + navigation index.
    hist: [[u8; HIST_W]; HIST_N],
    hist_len: [usize; HIST_N],
    hist_count: usize,
    hist_nav: usize, // 0..=hist_count; == hist_count means "editing a fresh line"
    esc: InEsc,
}

impl LineState {
    const fn new() -> Self {
        Self {
            pend: [0; PEND_MAX],
            pend_n: 0,
            cursor: 0,
            q: [0; QUEUE_MAX],
            head: 0,
            tail: 0,
            count: 0,
            hist: [[0; HIST_W]; HIST_N],
            hist_len: [0; HIST_N],
            hist_count: 0,
            hist_nav: 0,
            esc: InEsc::Normal,
        }
    }

    fn commit_push(&mut self, b: u8) {
        if self.count == QUEUE_MAX {
            return; // queue full — drop (reader is too slow)
        }
        self.q[self.head] = b;
        self.head = (self.head + 1) % QUEUE_MAX;
        self.count += 1;
    }

    fn commit_pop(&mut self) -> Option<u8> {
        if self.count == 0 {
            return None;
        }
        let b = self.q[self.tail];
        self.tail = (self.tail + 1) % QUEUE_MAX;
        self.count -= 1;
        Some(b)
    }

    /// Insert `b` at the cursor (mid-line inserts shift the tail right).
    fn insert(&mut self, b: u8) {
        if self.pend_n >= PEND_MAX {
            return;
        }
        let c = self.cursor;
        self.pend.copy_within(c..self.pend_n, c + 1);
        self.pend[c] = b;
        self.pend_n += 1;
        self.cursor += 1;
    }

    /// Delete the char before the cursor (Backspace).
    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let c = self.cursor;
        self.pend.copy_within(c..self.pend_n, c - 1);
        self.pend_n -= 1;
        self.cursor -= 1;
    }

    /// Replace the pending line with history entry `hist_nav` (or clear it
    /// when nav is at the "fresh line" slot).
    fn load_from_history(&mut self) {
        if self.hist_nav >= self.hist_count {
            self.pend_n = 0;
            self.cursor = 0;
            return;
        }
        let len = self.hist_len[self.hist_nav];
        self.pend[..len].copy_from_slice(&self.hist[self.hist_nav][..len]);
        self.pend_n = len;
        self.cursor = len;
    }

    /// Store the just-committed line into history (drops the oldest when full).
    fn store_history(&mut self) {
        if self.pend_n == 0 {
            return;
        }
        let len = self.pend_n.min(HIST_W);
        if self.hist_count < HIST_N {
            let i = self.hist_count;
            self.hist[i][..len].copy_from_slice(&self.pend[..len]);
            self.hist_len[i] = len;
            self.hist_count += 1;
        } else {
            // Shift down, drop oldest, append at the end.
            for i in 1..HIST_N {
                let (a, b) = self.hist.split_at_mut(i);
                a[i - 1] = b[0];
                self.hist_len[i - 1] = self.hist_len[i];
            }
            self.hist[HIST_N - 1][..len].copy_from_slice(&self.pend[..len]);
            self.hist_len[HIST_N - 1] = len;
        }
    }
}

static STDIN: Mutex<LineState> = Mutex::new(LineState::new());

/// Feed one input byte through the line discipline. Call from the keyboard
/// ISR / HID poll. Handles printable insert, Backspace, Enter (commit), and
/// the `ESC [ A/B/C/D` cursor/history escapes the keyboard emits for arrows.
/// `without_interrupts` guards against a keyboard IRQ deadlocking against a
/// concurrent `drain` (both take the STDIN lock).
pub fn input_push(b: u8) {
    x86_64::instructions::interrupts::without_interrupts(|| input_push_locked(b));
}

fn input_push_locked(b: u8) {
    let mut s = STDIN.lock();

    // --- input-escape state machine (arrow keys) ---
    match s.esc {
        InEsc::Esc => {
            s.esc = if b == b'[' { InEsc::Csi } else { InEsc::Normal };
            return;
        }
        InEsc::Csi => {
            match b {
                b'A' => {
                    // Up — older history.
                    if s.hist_nav > 0 {
                        s.hist_nav -= 1;
                        s.load_from_history();
                    }
                }
                b'B' => {
                    // Down — newer history (hist_count == fresh empty line).
                    if s.hist_nav < s.hist_count {
                        s.hist_nav += 1;
                        s.load_from_history();
                    }
                }
                b'C' => {
                    if s.cursor < s.pend_n {
                        s.cursor += 1;
                    }
                }
                b'D' => {
                    if s.cursor > 0 {
                        s.cursor -= 1;
                    }
                }
                _ => {}
            }
            s.esc = InEsc::Normal;
            return;
        }
        InEsc::Normal => {}
    }

    match b {
        0x1B => s.esc = InEsc::Esc,
        b'\n' | b'\r' => {
            for i in 0..s.pend_n {
                let c = s.pend[i];
                s.commit_push(c);
            }
            s.commit_push(b'\n');
            s.store_history();
            s.pend_n = 0;
            s.cursor = 0;
            s.hist_nav = s.hist_count; // reset to fresh-line slot
            crate::serial::Serial::put_char('\n');
        }
        0x08 | 0x7F => {
            if s.cursor > 0 {
                s.backspace();
                // Erase the echoed glyph on a terminal: BS, space, BS.
                crate::serial::Serial::put_char('\u{8}');
                crate::serial::Serial::put_char(' ');
                crate::serial::Serial::put_char('\u{8}');
            }
        }
        0x20..=0x7E | b'\t' => {
            if s.pend_n < PEND_MAX {
                s.insert(b);
                crate::serial::Serial::put_char(b as char);
            }
        }
        _ => {}
    }
}

/// Drain committed stdin bytes into `buf` (non-blocking). Returns the count.
/// This backs `platform::stdin_read` → SYS_READ(fd 0).
pub fn drain(buf: &mut [u8]) -> usize {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut s = STDIN.lock();
        let mut n = 0;
        while n < buf.len() {
            match s.commit_pop() {
                Some(c) => {
                    buf[n] = c;
                    n += 1;
                }
                None => break,
            }
        }
        n
    })
}

// ============================================================================
// AnsiTty — ANSI escape-sequence handler over a TtyConsole
// ============================================================================
//
// Parses the minimal CSI subset a TUI program assumes, in the output stream:
//   - SGR `\x1b[<n>m`     color (30-37, 90-97 bright, 39 default, 0 reset)
//   - `\x1b[2J`           clear screen
//   - `\x1b[K`            clear to end of line
//   - `\x1b[<r>;<c>H`/`f` cursor position (1-based cells; H with no args = home)
// Everything else is rendered as text through the wrapped console.

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnsiState {
    Normal,
    Esc,
    Csi,
}

/// A console with an ANSI escape-sequence front-end.
pub struct AnsiTty {
    con: TtyConsole,
    mode: Aa,
    default_fg: Color,
    state: AnsiState,
    params: [u32; 8],
    nparam: usize,
    have_digit: bool,
}

impl AnsiTty {
    /// Wrap `con`, rendering glyphs in `mode`; `default_fg` is restored by
    /// SGR reset (`0`) and default-foreground (`39`).
    pub fn new(con: TtyConsole, mode: Aa, default_fg: Color) -> Self {
        Self {
            con,
            mode,
            default_fg,
            state: AnsiState::Normal,
            params: [0; 8],
            nparam: 0,
            have_digit: false,
        }
    }

    /// Map an SGR foreground code to a logical 0x00RRGGBB color.
    fn sgr_color(&self, code: u32) -> Option<Color> {
        Some(match code {
            0 | 39 => self.default_fg,
            30 => fb::rgb(0x00, 0x00, 0x00),
            31 => fb::rgb(0xCC, 0x00, 0x00),
            32 => fb::rgb(0x00, 0xCC, 0x00),
            33 => fb::rgb(0xCC, 0xCC, 0x00),
            34 => fb::rgb(0x40, 0x60, 0xFF),
            35 => fb::rgb(0xCC, 0x00, 0xCC),
            36 => fb::rgb(0x00, 0xCC, 0xCC),
            37 => fb::rgb(0xC0, 0xC0, 0xC0),
            90 => fb::rgb(0x80, 0x80, 0x80),
            91 => fb::rgb(0xFF, 0x40, 0x40),
            92 => fb::rgb(0x40, 0xFF, 0x40),
            93 => fb::rgb(0xFF, 0xFF, 0x40),
            94 => fb::rgb(0x60, 0x90, 0xFF),
            95 => fb::rgb(0xFF, 0x40, 0xFF),
            96 => fb::rgb(0x40, 0xFF, 0xFF),
            97 => fb::rgb(0xFF, 0xFF, 0xFF),
            _ => return None,
        })
    }

    /// Dispatch a completed CSI sequence whose final byte is `final_b`.
    fn csi(&mut self, final_b: u8) {
        let count = self.nparam + if self.have_digit { 1 } else { 0 };
        let p0 = self.params[0];
        let p1 = self.params[1];
        match final_b {
            b'm' => {
                if count == 0 {
                    self.con.set_fg(self.default_fg); // bare ESC[m == reset
                } else {
                    for i in 0..count {
                        if let Some(c) = self.sgr_color(self.params[i]) {
                            self.con.set_fg(c);
                        }
                    }
                }
            }
            b'J' => {
                if p0 == 2 {
                    self.con.clear();
                }
            }
            b'K' => self.con.clear_eol(),
            b'H' | b'f' => {
                let row = if count >= 1 { (p0.max(1)) as usize } else { 1 };
                let col = if count >= 2 { (p1.max(1)) as usize } else { 1 };
                self.con.move_cursor(row, col);
            }
            _ => {}
        }
    }

    fn flush_run(&mut self, run: &[u8]) {
        if !run.is_empty() {
            if let Ok(s) = core::str::from_utf8(run) {
                self.con.write(self.mode, s);
            }
        }
    }

    /// Write `text`, interpreting ANSI escape sequences.
    pub fn write_str(&mut self, text: &str) {
        let mut run = [0u8; 128];
        let mut rn = 0usize;
        for &b in text.as_bytes() {
            match self.state {
                AnsiState::Normal => {
                    if b == 0x1B {
                        self.flush_run(&run[..rn]);
                        rn = 0;
                        self.state = AnsiState::Esc;
                    } else {
                        run[rn] = b;
                        rn += 1;
                        if rn == run.len() {
                            self.flush_run(&run[..rn]);
                            rn = 0;
                        }
                    }
                }
                AnsiState::Esc => {
                    if b == b'[' {
                        self.state = AnsiState::Csi;
                        self.params = [0; 8];
                        self.nparam = 0;
                        self.have_digit = false;
                    } else {
                        // Not a CSI we handle — drop the escape, resume text.
                        self.state = AnsiState::Normal;
                    }
                }
                AnsiState::Csi => {
                    if b.is_ascii_digit() {
                        if self.nparam < self.params.len() {
                            self.params[self.nparam] =
                                self.params[self.nparam].saturating_mul(10) + (b - b'0') as u32;
                        }
                        self.have_digit = true;
                    } else if b == b';' {
                        if self.nparam < self.params.len() - 1 {
                            self.nparam += 1;
                        }
                        self.have_digit = false;
                    } else {
                        self.csi(b);
                        self.state = AnsiState::Normal;
                    }
                }
            }
        }
        self.flush_run(&run[..rn]);
    }
}
