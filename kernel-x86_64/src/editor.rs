//! M21 — modal (vi-style) text editor.
//!
//! Kernel-side v1, launched by sem-sh's `edit <file>` builtin (SYS_EDIT), the
//! same way the agent TUI is launched. It reuses the M7 TTF renderer + the
//! framebuffer + raw USB HID — modal editing needs individual keystrokes
//! (`h`/`j`/`i`/`Esc`), not the cooked line discipline, so we read `poll_hid`
//! directly. The Path-B "real app" version is a Ring-3 editor, which needs a
//! user-space framebuffer surface that doesn't exist yet (open M6 follow-up).
//!
//! The edit *logic* (`handle_key`, `load`, `save`) is pure and IO-light so the
//! verify DEMO can script an edit headlessly without a real keyboard.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::framebuffer as fb;
use crate::{font, usb};
use kernel_core::syscall::{dispatch, numbers::*, open_flags};

// Colors (0x00RRGGBB) — a VS Code-ish Rust palette.
const BG: u32 = 0x0010_1418;
const FG: u32 = 0x00D4_D4D4;
const KEYWORD: u32 = 0x0056_9CD6;
const STRING_C: u32 = 0x00CE_9178;
const COMMENT: u32 = 0x006A_9955;
const NUMBER: u32 = 0x00B5_CEA8;
const STATUS_BG: u32 = 0x0026_4F78;
const STATUS_FG: u32 = 0x00FF_FFFF;
const CURSOR: u32 = 0x0030_D030;

const PX: f32 = 18.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
}

/// A translated key event (mode-independent).
#[derive(Clone, Copy)]
pub enum Key {
    Char(u8),
    Enter,
    Backspace,
    Tab,
    Esc,
    Up,
    Down,
    Left,
    Right,
}

pub struct Editor {
    pub lines: Vec<Vec<u8>>, // one byte-vec per line (ASCII-oriented; UTF-8 lossy on render)
    pub cx: usize,           // cursor column (byte index within the line)
    pub cy: usize,           // cursor row (line index)
    top: usize,              // first visible line (vertical scroll)
    pub mode: Mode,
    pub path: String,
    pub dirty: bool,
    cmd: String,        // the ':' / '/' line being typed
    searching: bool,    // command line is a '/' search, not a ':' command
    last_search: String,
    msg: String,        // transient status message
    pending: u8,        // pending operator/prefix key (e.g. first 'd' of 'dd')
    pub quit: bool,
}

impl Editor {
    /// Load `path` from the namespace into a line buffer. A missing file opens
    /// as a single empty line (a new file).
    pub fn load(path: &str) -> Editor {
        let mut lines: Vec<Vec<u8>> = Vec::new();
        let p = path.as_bytes();
        let fd = dispatch(SYS_OPEN, p.as_ptr() as u64, p.len() as u64, 0, 0);
        if fd != u64::MAX {
            let mut data: Vec<u8> = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = dispatch(SYS_FREAD, fd, buf.as_mut_ptr() as u64, buf.len() as u64, 0);
                if n == u64::MAX || n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n as usize]);
                if data.len() > 4 * 1024 * 1024 {
                    break;
                }
            }
            dispatch(SYS_CLOSE, fd, 0, 0, 0);
            for line in data.split(|&b| b == b'\n') {
                lines.push(line.to_vec());
            }
        }
        if lines.is_empty() {
            lines.push(Vec::new());
        }
        Editor {
            lines,
            cx: 0,
            cy: 0,
            top: 0,
            mode: Mode::Normal,
            path: String::from(path),
            dirty: false,
            cmd: String::new(),
            searching: false,
            last_search: String::new(),
            msg: String::new(),
            pending: 0,
            quit: false,
        }
    }

    /// Write the buffer back to `path`: truncate then write the full content.
    /// Returns true on success and clears the dirty flag.
    pub fn save(&mut self) -> bool {
        let mut out: Vec<u8> = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            out.extend_from_slice(line);
            if i + 1 < self.lines.len() {
                out.push(b'\n');
            }
        }
        let p = self.path.as_bytes();
        // Ensure the object exists, then truncate to 0 so a shrink doesn't leave
        // a stale tail, then write from offset 0.
        let fd = dispatch(SYS_OPEN, p.as_ptr() as u64, p.len() as u64, open_flags::CREATE, 0);
        if fd == u64::MAX {
            return false;
        }
        dispatch(SYS_CLOSE, fd, 0, 0, 0);
        dispatch(SYS_TRUNCATE, p.as_ptr() as u64, p.len() as u64, 0, 0);
        let fd = dispatch(SYS_OPEN, p.as_ptr() as u64, p.len() as u64, 0, 0);
        if fd == u64::MAX {
            return false;
        }
        let n = dispatch(SYS_FWRITE, fd, out.as_ptr() as u64, out.len() as u64, 0);
        dispatch(SYS_CLOSE, fd, 0, 0, 0);
        let ok = n != u64::MAX && n as usize == out.len();
        if ok {
            self.dirty = false;
        }
        ok
    }

    // --- pure edit logic -----------------------------------------------------

    pub fn handle_key(&mut self, key: Key) {
        self.msg.clear();
        match self.mode {
            Mode::Normal => self.normal_key(key),
            Mode::Insert => self.insert_key(key),
            Mode::Command => self.command_key(key),
        }
    }

    fn cur_len(&self) -> usize {
        self.lines[self.cy].len()
    }

    fn clamp_cx(&mut self) {
        let ll = self.cur_len();
        if self.cx > ll {
            self.cx = ll;
        }
    }

    fn normal_key(&mut self, key: Key) {
        let pend = self.pending;
        self.pending = 0;
        match key {
            Key::Char(b'h') | Key::Left => {
                if self.cx > 0 {
                    self.cx -= 1;
                }
            }
            Key::Char(b'l') | Key::Right => {
                if self.cx < self.cur_len() {
                    self.cx += 1;
                }
            }
            Key::Char(b'k') | Key::Up => {
                if self.cy > 0 {
                    self.cy -= 1;
                    self.clamp_cx();
                }
            }
            Key::Char(b'j') | Key::Down => {
                if self.cy + 1 < self.lines.len() {
                    self.cy += 1;
                    self.clamp_cx();
                }
            }
            Key::Char(b'0') => self.cx = 0,
            Key::Char(b'$') => self.cx = self.cur_len(),
            Key::Char(b'i') => self.mode = Mode::Insert,
            Key::Char(b'a') => {
                if self.cur_len() > 0 {
                    self.cx = (self.cx + 1).min(self.cur_len());
                }
                self.mode = Mode::Insert;
            }
            Key::Char(b'A') => {
                self.cx = self.cur_len();
                self.mode = Mode::Insert;
            }
            Key::Char(b'o') => {
                self.lines.insert(self.cy + 1, Vec::new());
                self.cy += 1;
                self.cx = 0;
                self.dirty = true;
                self.mode = Mode::Insert;
            }
            Key::Char(b'O') => {
                self.lines.insert(self.cy, Vec::new());
                self.cx = 0;
                self.dirty = true;
                self.mode = Mode::Insert;
            }
            Key::Char(b'x') => {
                let ll = self.cur_len();
                if self.cx < ll {
                    self.lines[self.cy].remove(self.cx);
                    let nl = self.cur_len();
                    if self.cx >= nl && self.cx > 0 {
                        self.cx -= 1;
                    }
                    self.dirty = true;
                }
            }
            Key::Char(b'd') => {
                if pend == b'd' {
                    self.delete_line();
                } else {
                    self.pending = b'd';
                }
            }
            Key::Char(b'g') => {
                if pend == b'g' {
                    self.cy = 0;
                    self.cx = 0;
                } else {
                    self.pending = b'g';
                }
            }
            Key::Char(b'G') => {
                self.cy = self.lines.len().saturating_sub(1);
                self.cx = 0;
            }
            Key::Char(b':') => {
                self.mode = Mode::Command;
                self.searching = false;
                self.cmd.clear();
            }
            Key::Char(b'/') => {
                self.mode = Mode::Command;
                self.searching = true;
                self.cmd.clear();
            }
            Key::Char(b'n') => self.search_next(),
            _ => {}
        }
    }

    fn insert_key(&mut self, key: Key) {
        match key {
            Key::Esc => {
                self.mode = Mode::Normal;
                if self.cx > 0 {
                    self.cx -= 1;
                }
            }
            Key::Left => {
                if self.cx > 0 {
                    self.cx -= 1;
                }
            }
            Key::Right => {
                if self.cx < self.cur_len() {
                    self.cx += 1;
                }
            }
            Key::Up => {
                if self.cy > 0 {
                    self.cy -= 1;
                    self.clamp_cx();
                }
            }
            Key::Down => {
                if self.cy + 1 < self.lines.len() {
                    self.cy += 1;
                    self.clamp_cx();
                }
            }
            Key::Enter => {
                let cx = self.cx.min(self.cur_len());
                let tail = self.lines[self.cy].split_off(cx);
                self.lines.insert(self.cy + 1, tail);
                self.cy += 1;
                self.cx = 0;
                self.dirty = true;
            }
            Key::Backspace => {
                if self.cx > 0 {
                    self.cx -= 1;
                    self.lines[self.cy].remove(self.cx);
                    self.dirty = true;
                } else if self.cy > 0 {
                    let cur = self.lines.remove(self.cy);
                    self.cy -= 1;
                    self.cx = self.cur_len();
                    self.lines[self.cy].extend_from_slice(&cur);
                    self.dirty = true;
                }
            }
            Key::Tab => {
                for _ in 0..4 {
                    self.insert_byte(b' ');
                }
            }
            Key::Char(c) => self.insert_byte(c),
        }
    }

    fn insert_byte(&mut self, b: u8) {
        let cx = self.cx.min(self.cur_len());
        self.lines[self.cy].insert(cx, b);
        self.cx = cx + 1;
        self.dirty = true;
    }

    fn delete_line(&mut self) {
        if self.lines.len() <= 1 {
            self.lines[0].clear();
        } else {
            self.lines.remove(self.cy);
            if self.cy >= self.lines.len() {
                self.cy = self.lines.len() - 1;
            }
        }
        self.cx = 0;
        self.dirty = true;
    }

    fn command_key(&mut self, key: Key) {
        match key {
            Key::Esc => {
                self.mode = Mode::Normal;
                self.cmd.clear();
            }
            Key::Backspace => {
                if self.cmd.pop().is_none() {
                    self.mode = Mode::Normal;
                }
            }
            Key::Enter => self.exec_command(),
            Key::Char(c) => self.cmd.push(c as char),
            _ => {}
        }
    }

    fn exec_command(&mut self) {
        if self.searching {
            self.last_search = self.cmd.clone();
            self.mode = Mode::Normal;
            self.cmd.clear();
            self.search_next();
            return;
        }
        let cmd = self.cmd.trim();
        match cmd {
            "w" => {
                self.msg = if self.save() {
                    format!("\"{}\" written", self.path)
                } else {
                    String::from("write failed")
                };
            }
            "q" => {
                if self.dirty {
                    self.msg = String::from("unsaved changes — :w to save, :q! to discard");
                } else {
                    self.quit = true;
                }
            }
            "q!" => self.quit = true,
            "wq" | "x" => {
                if self.save() {
                    self.quit = true;
                } else {
                    self.msg = String::from("write failed");
                }
            }
            other => self.msg = format!("unknown command: {}", other),
        }
        self.mode = Mode::Normal;
        self.cmd.clear();
    }

    fn search_next(&mut self) {
        if self.last_search.is_empty() {
            return;
        }
        let needle = self.last_search.as_bytes();
        let n = self.lines.len();
        for off in 0..=n {
            let li = (self.cy + off) % n;
            let start = if off == 0 { self.cx + 1 } else { 0 };
            if let Some(pos) = find_sub(&self.lines[li], needle, start) {
                self.cy = li;
                self.cx = pos;
                return;
            }
        }
        self.msg = format!("not found: {}", self.last_search);
    }

    // --- rendering -----------------------------------------------------------

    fn render(&mut self) {
        let (w, h) = fb::fb_dimensions();
        if w == 0 || h == 0 {
            return;
        }
        let lh = font::line_height(PX).max(20);
        let margin = 8usize;
        let status_h = lh + 6;
        let avail_h = h.saturating_sub(status_h + margin);
        let visible_rows = (avail_h / lh).max(1);

        // Keep the cursor on screen.
        if self.cy < self.top {
            self.top = self.cy;
        }
        if self.cy >= self.top + visible_rows {
            self.top = self.cy + 1 - visible_rows;
        }

        fb::fb_fill_rect(0, 0, w, h, BG);

        let ascent = (PX * 0.78) as usize;
        font::with_face(PX, |face| {
            let mut row = 0;
            while row < visible_rows {
                let li = self.top + row;
                if li >= self.lines.len() {
                    break;
                }
                let line = &self.lines[li];
                let colors = colorize(line);
                let baseline = (margin + row * lh + ascent) as f32;
                let mut pen = margin as f32;
                for (idx, &b) in line.iter().enumerate() {
                    let color = colors.get(idx).copied().unwrap_or(FG);
                    pen += face.draw_char(pen, baseline, b as char, color);
                }
                row += 1;
            }
        });

        self.draw_cursor(lh, margin, visible_rows);
        self.draw_status(w, h, status_h, ascent);
    }

    fn draw_cursor(&self, lh: usize, margin: usize, visible_rows: usize) {
        if self.cy < self.top || self.cy >= self.top + visible_rows {
            return;
        }
        let row = self.cy - self.top;
        let line = &self.lines[self.cy];
        let cur_x = font::with_face(PX, |face| {
            let mut pen = margin as f32;
            for &b in line.iter().take(self.cx) {
                pen += face.advance(b as char);
            }
            pen
        })
        .unwrap_or(margin as f32) as usize;
        let y = margin + row * lh;
        if self.mode == Mode::Insert {
            fb::fb_fill_rect(cur_x, y, 2, lh, CURSOR);
        } else {
            // Block cursor: a filled cell with the underlying glyph redrawn in BG.
            let ch = line.get(self.cx).map(|&b| b as char).unwrap_or(' ');
            let cw = font::with_face(PX, |face| {
                let a = face.advance(ch);
                if a > 1.0 {
                    a
                } else {
                    face.advance('m')
                }
            })
            .unwrap_or(9.0) as usize;
            fb::fb_fill_rect(cur_x, y, cw.max(4), lh, CURSOR);
            if self.cx < line.len() {
                font::with_face(PX, |face| {
                    face.draw_char(cur_x as f32, (y + (PX * 0.78) as usize) as f32, ch, BG);
                });
            }
        }
    }

    fn draw_status(&self, w: usize, h: usize, status_h: usize, ascent: usize) {
        let y = h - status_h;
        fb::fb_fill_rect(0, y, w, status_h, STATUS_BG);
        let baseline = y + 3 + ascent;
        let text = if self.mode == Mode::Command {
            let prompt = if self.searching { "/" } else { ":" };
            format!("{}{}", prompt, self.cmd)
        } else {
            let m = match self.mode {
                Mode::Normal => "NORMAL",
                Mode::Insert => "INSERT",
                Mode::Command => "",
            };
            let flag = if self.dirty { " [+]" } else { "" };
            let tail = if self.msg.is_empty() {
                format!("Ln {}, Col {}", self.cy + 1, self.cx + 1)
            } else {
                self.msg.clone()
            };
            format!("-- {} --  {}{}    {}", m, self.path, flag, tail)
        };
        font::fb_draw_text(8, baseline, &text, PX, STATUS_FG);
    }
}

/// Per-byte color for one line: a tiny Rust tokenizer (keywords, strings, line
/// comments, numbers). Good enough for v1 highlighting.
fn colorize(line: &[u8]) -> Vec<u32> {
    let mut col = vec![FG; line.len()];
    let mut i = 0;
    while i < line.len() {
        let b = line[i];
        // line comment `// ...`
        if b == b'/' && i + 1 < line.len() && line[i + 1] == b'/' {
            for c in col.iter_mut().skip(i) {
                *c = COMMENT;
            }
            break;
        }
        // string literal
        if b == b'"' {
            let start = i;
            i += 1;
            while i < line.len() && line[i] != b'"' {
                if line[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            let end = (i + 1).min(line.len());
            for c in col.iter_mut().take(end).skip(start) {
                *c = STRING_C;
            }
            i = end;
            continue;
        }
        // number
        if b.is_ascii_digit() {
            let start = i;
            while i < line.len()
                && (line[i].is_ascii_alphanumeric() || line[i] == b'.' || line[i] == b'_')
            {
                i += 1;
            }
            for c in col.iter_mut().take(i).skip(start) {
                *c = NUMBER;
            }
            continue;
        }
        // identifier / keyword
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < line.len() && (line[i].is_ascii_alphanumeric() || line[i] == b'_') {
                i += 1;
            }
            if is_keyword(&line[start..i]) {
                for c in col.iter_mut().take(i).skip(start) {
                    *c = KEYWORD;
                }
            }
            continue;
        }
        i += 1;
    }
    col
}

fn is_keyword(w: &[u8]) -> bool {
    matches!(
        w,
        b"as" | b"break" | b"const" | b"continue" | b"crate" | b"dyn" | b"else" | b"enum"
            | b"extern" | b"false" | b"fn" | b"for" | b"if" | b"impl" | b"in" | b"let"
            | b"loop" | b"match" | b"mod" | b"move" | b"mut" | b"pub" | b"ref" | b"return"
            | b"self" | b"Self" | b"static" | b"struct" | b"super" | b"trait" | b"true"
            | b"type" | b"unsafe" | b"use" | b"where" | b"while" | b"async" | b"await"
    )
}

fn find_sub(hay: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || start > hay.len() {
        return None;
    }
    let mut i = start;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Translate a raw HID keycode (+ shift) into an editor `Key`. Returns None for
/// modifiers / keys we don't handle.
fn translate(k: u8, shift: bool) -> Option<Key> {
    match k {
        0x29 => Some(Key::Esc),
        0x4F => Some(Key::Right),
        0x50 => Some(Key::Left),
        0x51 => Some(Key::Down),
        0x52 => Some(Key::Up),
        _ => {
            let c = usb::hid::keycode_to_ascii(k, shift)?;
            Some(match c {
                b'\n' => Key::Enter,
                0x08 => Key::Backspace,
                b'\t' => Key::Tab,
                _ => Key::Char(c),
            })
        }
    }
}

/// SYS_EDIT entry: run the editor over `path` until the user quits. Drives the
/// real keyboard via raw HID polls (modal editing needs per-key events, not the
/// cooked line discipline). Holds FULLSCREEN_APP_ACTIVE so the interactive
/// shell's pump doesn't race ours, and clears the screen on exit.
pub fn run(path: &str) -> u64 {
    use core::sync::atomic::Ordering;

    let (w, h) = fb::fb_dimensions();
    if w == 0 || h == 0 {
        return 1; // headless — nothing to draw
    }

    crate::FULLSCREEN_APP_ACTIVE.store(true, Ordering::Relaxed);
    let mut ed = Editor::load(path);
    ed.render();

    let mut prev = [0u8; 6];
    while !ed.quit {
        let mut changed = false;
        usb::xhci::poll_hid(|rep| {
            let shift = rep.shift_held();
            for &k in rep.keys.iter() {
                if k == 0 || prev.contains(&k) {
                    continue;
                }
                if let Some(key) = translate(k, shift) {
                    ed.handle_key(key);
                    changed = true;
                }
            }
            prev = rep.keys;
        });
        if changed {
            ed.render();
        }
        // Re-pin FD/namespace resolution to us (the slot drifts across sleeps)
        // so an in-loop `:w` save resolves against our task, then yield.
        kernel_core::process::set_kernel_task_id(Some(
            kernel_core::scheduler::current_task_index(),
        ));
        let _ = dispatch(SYS_SLEEP, 1, 0, 0, 0);
    }

    crate::FULLSCREEN_APP_ACTIVE.store(false, Ordering::Relaxed);
    fb::clear();
    0
}
