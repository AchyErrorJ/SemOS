//! Tetris — a kernel-side fullscreen game, launched by sem-sh's `tetris` builtin
//! (SYS_TETRIS). Same shape as pong.rs: holds FULLSCREEN_APP_ACTIVE, drives the
//! framebuffer directly, and reads raw USB HID polls (edge + held) plus PS/2
//! ASCII impulses so it plays on both real hardware and QEMU's i8042.
//!
//! NES-style: a 10×20 playfield, the seven tetrominoes in their classic colours,
//! level-based gravity (NES frame table), line-clear scoring (40/100/300/1200 ×
//! (level+1)), and NES-faithful rotation (rotate; revert if it collides — no
//! wall kicks). Controls:
//!   move    Left / Right   (or A / D)        — with DAS auto-shift when held
//!   soft    Down           (or S)
//!   rotate  Up / X (CW)     Z (CCW)
//!   drop    Space (hard drop)
//!   P pause     Esc / Q quit

use crate::framebuffer as fb;
use crate::{font, usb};
use kernel_core::syscall::{dispatch, numbers::SYS_SLEEP};

const COLS: i32 = 10;
const ROWS: i32 = 20;

// Per-piece colours (the recognizable tetromino palette). Index 0 = empty.
const EMPTY: u32 = 0x0000_0000;
const COLORS: [u32; 8] = [
    EMPTY,
    0x0000_F0F0, // I — cyan
    0x00F0_F000, // O — yellow
    0x00A0_00F0, // T — purple
    0x0000_F000, // S — green
    0x00F0_0000, // Z — red
    0x0000_30F0, // J — blue
    0x00F0_A000, // L — orange
];
const BG: u32 = 0x0000_0000;
const GRID: u32 = 0x0010_1018;
const BORDER: u32 = 0x0060_6070;
const TEXT: u32 = 0x00C0_C0C0;
const GHOSTLINE: u32 = 0x0030_3040;

// HID Usage IDs (Keyboard/Keypad page).
const HID_LEFT: u8 = 0x50;
const HID_RIGHT: u8 = 0x4F;
const HID_DOWN: u8 = 0x51;
const HID_UP: u8 = 0x52;
const HID_Z: u8 = 0x1D;
const HID_X: u8 = 0x1B;
const HID_A: u8 = 0x04;
const HID_D: u8 = 0x07;
const HID_S: u8 = 0x16;
const HID_SPACE: u8 = 0x2C;
const HID_P: u8 = 0x13;
const HID_ESC: u8 = 0x29;
const HID_Q: u8 = 0x14;

// DAS (delayed auto-shift), in frames. Classic-ish: a held direction moves once,
// waits DAS_INITIAL, then repeats every DAS_REPEAT.
const DAS_INITIAL: i32 = 16;
const DAS_REPEAT: i32 = 6;

/// The 7 tetrominoes, each as 4 rotation states of 4 (x, y) cells in a 4×4 box.
/// Order: I, O, T, S, Z, J, L  (colour index = piece + 1).
const PIECES: [[[(i8, i8); 4]; 4]; 7] = [
    // I
    [
        [(0, 1), (1, 1), (2, 1), (3, 1)],
        [(2, 0), (2, 1), (2, 2), (2, 3)],
        [(0, 2), (1, 2), (2, 2), (3, 2)],
        [(1, 0), (1, 1), (1, 2), (1, 3)],
    ],
    // O
    [
        [(1, 0), (2, 0), (1, 1), (2, 1)],
        [(1, 0), (2, 0), (1, 1), (2, 1)],
        [(1, 0), (2, 0), (1, 1), (2, 1)],
        [(1, 0), (2, 0), (1, 1), (2, 1)],
    ],
    // T
    [
        [(1, 0), (0, 1), (1, 1), (2, 1)],
        [(1, 0), (1, 1), (2, 1), (1, 2)],
        [(0, 1), (1, 1), (2, 1), (1, 2)],
        [(1, 0), (0, 1), (1, 1), (1, 2)],
    ],
    // S
    [
        [(1, 0), (2, 0), (0, 1), (1, 1)],
        [(1, 0), (1, 1), (2, 1), (2, 2)],
        [(1, 1), (2, 1), (0, 2), (1, 2)],
        [(0, 0), (0, 1), (1, 1), (1, 2)],
    ],
    // Z
    [
        [(0, 0), (1, 0), (1, 1), (2, 1)],
        [(2, 0), (1, 1), (2, 1), (1, 2)],
        [(0, 1), (1, 1), (1, 2), (2, 2)],
        [(1, 0), (0, 1), (1, 1), (0, 2)],
    ],
    // J
    [
        [(0, 0), (0, 1), (1, 1), (2, 1)],
        [(1, 0), (2, 0), (1, 1), (1, 2)],
        [(0, 1), (1, 1), (2, 1), (2, 2)],
        [(1, 0), (1, 1), (0, 2), (1, 2)],
    ],
    // L
    [
        [(2, 0), (0, 1), (1, 1), (2, 1)],
        [(1, 0), (1, 1), (1, 2), (2, 2)],
        [(0, 1), (1, 1), (2, 1), (0, 2)],
        [(0, 0), (1, 0), (1, 1), (1, 2)],
    ],
];

struct Game {
    /// Board cells: 0 = empty, else colour index (piece + 1).
    board: [[u8; COLS as usize]; ROWS as usize],
    piece: usize,
    rot: usize,
    px: i32,
    py: i32,
    next: usize,
    score: u32,
    lines: u32,
    level: u32,
    drop_timer: i32,
    quit: bool,
    paused: bool,
    over: bool,
    rng: u64,
    // input: DAS state for held left/right (USB HID).
    das_dir: i32,
    das_timer: i32,
}

impl Game {
    fn new() -> Self {
        let seed =
            0x1234_5678_9ABC_DEF0 ^ kernel_core::platform::ticks().wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut g = Game {
            board: [[0; COLS as usize]; ROWS as usize],
            piece: 0,
            rot: 0,
            px: 0,
            py: 0,
            next: 0,
            score: 0,
            lines: 0,
            level: 0,
            drop_timer: 0,
            quit: false,
            paused: false,
            over: false,
            rng: seed | 1,
            das_dir: 0,
            das_timer: 0,
        };
        g.next = (g.rand() % 7) as usize;
        g.spawn();
        g.drop_timer = g.gravity();
        g
    }

    fn rand(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    /// NES frames-per-cell gravity table by level.
    fn gravity(&self) -> i32 {
        match self.level {
            0 => 48,
            1 => 43,
            2 => 38,
            3 => 33,
            4 => 28,
            5 => 23,
            6 => 18,
            7 => 13,
            8 => 8,
            9 => 6,
            10..=12 => 5,
            13..=15 => 4,
            16..=18 => 3,
            19..=28 => 2,
            _ => 1,
        }
    }

    /// Does the given piece/rotation at (x, y) collide with a wall, the floor,
    /// or a filled cell?
    fn collides(&self, piece: usize, rot: usize, x: i32, y: i32) -> bool {
        for &(cx, cy) in &PIECES[piece][rot] {
            let bx = x + cx as i32;
            let by = y + cy as i32;
            if bx < 0 || bx >= COLS || by >= ROWS {
                return true;
            }
            if by >= 0 && self.board[by as usize][bx as usize] != 0 {
                return true;
            }
        }
        false
    }

    /// Spawn `self.next` at the top-centre; promote a fresh `next`. Sets
    /// `over` if the spawn position is already blocked.
    fn spawn(&mut self) {
        self.piece = self.next;
        self.next = (self.rand() % 7) as usize;
        self.rot = 0;
        self.px = (COLS - 4) / 2; // 4-wide box, centred → x = 3
        self.py = 0;
        if self.collides(self.piece, self.rot, self.px, self.py) {
            self.over = true;
        }
    }

    fn try_move(&mut self, dx: i32, dy: i32) -> bool {
        if !self.collides(self.piece, self.rot, self.px + dx, self.py + dy) {
            self.px += dx;
            self.py += dy;
            true
        } else {
            false
        }
    }

    /// Rotate by `dir` (+1 = CW, -1 = CCW). NES-style: no wall kicks — if the
    /// new orientation collides, the rotation is rejected.
    fn rotate(&mut self, dir: i32) {
        let nrot = ((self.rot as i32 + dir + 4) % 4) as usize;
        if !self.collides(self.piece, nrot, self.px, self.py) {
            self.rot = nrot;
        }
    }

    /// Lock the current piece into the board, clear full lines, score, and
    /// spawn the next piece.
    fn lock(&mut self) {
        let color = (self.piece + 1) as u8;
        for &(cx, cy) in &PIECES[self.piece][self.rot] {
            let bx = self.px + cx as i32;
            let by = self.py + cy as i32;
            if by >= 0 && by < ROWS && bx >= 0 && bx < COLS {
                self.board[by as usize][bx as usize] = color;
            }
        }
        let cleared = self.clear_lines();
        if cleared > 0 {
            let base = [0u32, 40, 100, 300, 1200][cleared as usize];
            self.score += base * (self.level + 1);
            self.lines += cleared;
            self.level = self.lines / 10;
        }
        self.spawn();
        self.drop_timer = self.gravity();
    }

    /// Remove full rows, dropping everything above down. Returns the count.
    fn clear_lines(&mut self) -> u32 {
        let mut cleared = 0u32;
        let mut row = ROWS - 1;
        while row >= 0 {
            let full = (0..COLS).all(|c| self.board[row as usize][c as usize] != 0);
            if full {
                // Shift everything above `row` down by one.
                for r in (1..=row).rev() {
                    self.board[r as usize] = self.board[(r - 1) as usize];
                }
                self.board[0] = [0; COLS as usize];
                cleared += 1;
                // Re-check the same row index (now holds the shifted-in row).
            } else {
                row -= 1;
            }
        }
        cleared
    }

    /// Lowest y the current piece can occupy (for the ghost / hard drop).
    fn drop_y(&self) -> i32 {
        let mut y = self.py;
        while !self.collides(self.piece, self.rot, self.px, y + 1) {
            y += 1;
        }
        y
    }

    fn hard_drop(&mut self) {
        self.py = self.drop_y();
        self.lock();
    }

    fn render(&self, ox: usize, oy: usize, cell: usize) {
        let (w, h) = fb::fb_dimensions();
        let (w, h) = (w as usize, h as usize);
        fb::fb_fill_rect(0, 0, w, h, BG);

        let pf_w = COLS as usize * cell;
        let pf_h = ROWS as usize * cell;

        // Playfield border.
        fb::fb_fill_rect(ox - 3, oy - 3, pf_w + 6, pf_h + 6, BORDER);
        fb::fb_fill_rect(ox, oy, pf_w, pf_h, GRID);

        // Settled cells.
        for r in 0..ROWS as usize {
            for c in 0..COLS as usize {
                let v = self.board[r][c];
                if v != 0 {
                    draw_cell(ox + c * cell, oy + r * cell, cell, COLORS[v as usize]);
                }
            }
        }

        // Ghost (landing preview) — a faint outline row beneath the piece.
        if !self.over && !self.paused {
            let gy = self.drop_y();
            for &(cx, cy) in &PIECES[self.piece][self.rot] {
                let bx = self.px + cx as i32;
                let by = gy + cy as i32;
                if by >= 0 {
                    fb::fb_fill_rect(
                        ox + bx as usize * cell,
                        oy + by as usize * cell,
                        cell,
                        cell,
                        GHOSTLINE,
                    );
                }
            }
        }

        // Active piece.
        if !self.over {
            let color = COLORS[self.piece + 1];
            for &(cx, cy) in &PIECES[self.piece][self.rot] {
                let bx = self.px + cx as i32;
                let by = self.py + cy as i32;
                if by >= 0 {
                    draw_cell(ox + bx as usize * cell, oy + by as usize * cell, cell, color);
                }
            }
        }

        // ---- Side panel (to the right of the playfield) ----
        let panel_x = ox + pf_w + cell;
        let label_px = (cell as f32 * 0.7).max(14.0);
        let val_px = (cell as f32 * 0.9).max(18.0);
        let mut ty = oy;

        let _ = font::fb_draw_text(panel_x, ty, "SCORE", label_px, TEXT);
        ty += (label_px * 1.2) as usize;
        let mut nbuf = [0u8; 12];
        let n = fmt_u32(&mut nbuf, self.score);
        let _ = font::fb_draw_text(panel_x, ty, core::str::from_utf8(&nbuf[..n]).unwrap_or("?"), val_px, TEXT);
        ty += (val_px * 1.6) as usize;

        let _ = font::fb_draw_text(panel_x, ty, "LEVEL", label_px, TEXT);
        ty += (label_px * 1.2) as usize;
        let n = fmt_u32(&mut nbuf, self.level);
        let _ = font::fb_draw_text(panel_x, ty, core::str::from_utf8(&nbuf[..n]).unwrap_or("?"), val_px, TEXT);
        ty += (val_px * 1.6) as usize;

        let _ = font::fb_draw_text(panel_x, ty, "LINES", label_px, TEXT);
        ty += (label_px * 1.2) as usize;
        let n = fmt_u32(&mut nbuf, self.lines);
        let _ = font::fb_draw_text(panel_x, ty, core::str::from_utf8(&nbuf[..n]).unwrap_or("?"), val_px, TEXT);
        ty += (val_px * 1.8) as usize;

        // Next-piece preview.
        let _ = font::fb_draw_text(panel_x, ty, "NEXT", label_px, TEXT);
        ty += (label_px * 1.3) as usize;
        let ncell = (cell * 3) / 4;
        let ncolor = COLORS[self.next + 1];
        for &(cx, cy) in &PIECES[self.next][0] {
            draw_cell(
                panel_x + cx as usize * ncell,
                ty + cy as usize * ncell,
                ncell,
                ncolor,
            );
        }

        // Status line at the bottom of the playfield.
        let msg = if self.over {
            "GAME OVER  —  P: new game   Esc: quit"
        } else if self.paused {
            "PAUSED  —  P: resume   Esc: quit"
        } else {
            "Move: Arrows/AD   Rotate: Up/X / Z   Soft: Down   Hard: Space   P: pause"
        };
        let _ = font::fb_draw_text(ox, oy + pf_h + 8, msg, (cell as f32 * 0.5).max(12.0), TEXT);

        let _ = fb::fb_present();
    }
}

/// Draw one block cell: filled, with a slightly darker 1-px inset border for
/// the classic gridded look.
fn draw_cell(x: usize, y: usize, cell: usize, color: u32) {
    fb::fb_fill_rect(x, y, cell, cell, color & 0x007F_7F7F); // darker edge
    if cell > 2 {
        fb::fb_fill_rect(x + 1, y + 1, cell - 2, cell - 2, color);
    }
}

/// Format an unsigned into `buf`; returns the byte length.
fn fmt_u32(buf: &mut [u8], mut n: u32) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 10];
    let mut len = 0;
    while n > 0 {
        tmp[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
    }
    for i in 0..len {
        buf[i] = tmp[len - 1 - i];
    }
    len
}

/// Held flags for continuous movement / soft drop (USB HID press-release diff).
#[derive(Default, Clone, Copy)]
struct Held {
    left: bool,
    right: bool,
    down: bool,
}

impl Held {
    fn apply(&mut self, prev: &[u8; 6], cur: &[u8; 6]) {
        let set = |k: u8| matches!(k, HID_LEFT | HID_RIGHT | HID_DOWN | HID_A | HID_D | HID_S);
        let _ = set;
        for &k in cur.iter() {
            if k == 0 || prev.contains(&k) {
                continue;
            }
            match k {
                HID_LEFT | HID_A => self.left = true,
                HID_RIGHT | HID_D => self.right = true,
                HID_DOWN | HID_S => self.down = true,
                _ => {}
            }
        }
        for &k in prev.iter() {
            if k == 0 || cur.contains(&k) {
                continue;
            }
            match k {
                HID_LEFT | HID_A => self.left = false,
                HID_RIGHT | HID_D => self.right = false,
                HID_DOWN | HID_S => self.down = false,
                _ => {}
            }
        }
    }
}

/// SYS_TETRIS entry. Holds FULLSCREEN_APP_ACTIVE, drives the framebuffer, and
/// reads USB HID (edge + held) + PS/2 (ASCII impulses) until Esc/Q.
pub fn run() -> u64 {
    use core::sync::atomic::Ordering;

    let (w, h) = fb::fb_dimensions();
    if w == 0 || h == 0 {
        return 1;
    }
    let (w, h) = (w as i32, h as i32);

    // Cell size: fit 20 rows into ~90% of the height, leave room for a panel.
    let cell = (((h - 60) / (ROWS + 1)).min((w * 2 / 3) / (COLS + 6))).max(6) as usize;
    let pf_w = COLS as usize * cell;
    let pf_h = ROWS as usize * cell;
    // Centre the playfield+panel block horizontally; panel is ~6 cells wide.
    let total_w = pf_w + cell * 7;
    let ox = (((w as usize).saturating_sub(total_w)) / 2 + 4).max(8);
    let oy = ((h as usize).saturating_sub(pf_h)) / 2;

    crate::FULLSCREEN_APP_ACTIVE.store(true, Ordering::Relaxed);
    crate::tty::SUPPRESS_TTY_INPUT.store(true, Ordering::Relaxed);
    while crate::keyboard::read_key().is_some() {}

    let mut game = Game::new();
    game.render(ox, oy, cell);

    let mut prev_hid: [u8; 6] = [0; 6];
    let mut held = Held::default();
    let mut ps2_rotate_cd: i32 = 0; // PS/2 rotate cooldown (anti-autorepeat-spin)

    loop {
        if game.quit {
            break;
        }

        // ---- USB HID: edge events + held diff ----
        usb::xhci::poll_hid(|rep| {
            for &k in rep.keys.iter() {
                if k == 0 || prev_hid.contains(&k) {
                    continue;
                }
                edge(&mut game, k);
            }
            held.apply(&prev_hid, &rep.keys);
            prev_hid = rep.keys;
        });

        // ---- PS/2: ASCII impulses (QEMU / no-USB). Letters only; arrow keys
        // arrive as escape sequences the TTY swallows. ----
        while let Some(b) = crate::keyboard::read_key() {
            match b {
                b'a' | b'A' => {
                    if !game.over && !game.paused {
                        game.try_move(-1, 0);
                    }
                }
                b'd' | b'D' => {
                    if !game.over && !game.paused {
                        game.try_move(1, 0);
                    }
                }
                b's' | b'S' => {
                    if !game.over && !game.paused {
                        game.try_move(0, 1);
                    }
                }
                b'x' | b'X' | b'k' | b'K' => {
                    if ps2_rotate_cd == 0 && !game.over && !game.paused {
                        game.rotate(1);
                        ps2_rotate_cd = 8;
                    }
                }
                b'z' | b'Z' | b'j' | b'J' => {
                    if ps2_rotate_cd == 0 && !game.over && !game.paused {
                        game.rotate(-1);
                        ps2_rotate_cd = 8;
                    }
                }
                b' ' => {
                    if !game.over && !game.paused {
                        game.hard_drop();
                    }
                }
                b'p' | b'P' => edge(&mut game, HID_P),
                b'q' | b'Q' | 0x1B => game.quit = true,
                _ => {}
            }
        }
        if ps2_rotate_cd > 0 {
            ps2_rotate_cd -= 1;
        }

        // ---- Held movement (USB): DAS auto-shift ----
        if !game.over && !game.paused {
            let dir = match (held.left, held.right) {
                (true, false) => -1,
                (false, true) => 1,
                _ => 0,
            };
            if dir != 0 {
                if dir != game.das_dir {
                    game.das_dir = dir;
                    game.try_move(dir, 0);
                    game.das_timer = DAS_INITIAL;
                } else {
                    game.das_timer -= 1;
                    if game.das_timer <= 0 {
                        game.try_move(dir, 0);
                        game.das_timer = DAS_REPEAT;
                    }
                }
            } else {
                game.das_dir = 0;
                game.das_timer = 0;
            }

            // ---- Gravity (soft drop if Down held) ----
            game.drop_timer -= 1;
            if held.down && game.drop_timer > 2 {
                game.drop_timer = 2;
            }
            if game.drop_timer <= 0 {
                if !game.try_move(0, 1) {
                    game.lock();
                } else {
                    game.drop_timer = game.gravity();
                }
            }
        }

        game.render(ox, oy, cell);

        kernel_core::process::set_kernel_task_id(Some(
            kernel_core::scheduler::current_task_index(),
        ));
        let _ = dispatch(SYS_SLEEP, 1, 0, 0, 0);
    }

    crate::tty::SUPPRESS_TTY_INPUT.store(false, Ordering::Relaxed);
    crate::FULLSCREEN_APP_ACTIVE.store(false, Ordering::Relaxed);
    fb::clear();
    let _ = fb::fb_present();
    0
}

/// Edge-triggered actions (one per key press) shared by USB HID and PS/2.
fn edge(game: &mut Game, code: u8) {
    match code {
        HID_ESC | HID_Q => game.quit = true,
        HID_P => {
            if game.over {
                *game = Game::new();
            } else {
                game.paused = !game.paused;
            }
        }
        _ if game.over || game.paused => {}
        HID_UP | HID_X => game.rotate(1),
        HID_Z => game.rotate(-1),
        HID_SPACE => game.hard_drop(),
        HID_LEFT | HID_A => {
            game.try_move(-1, 0);
        }
        HID_RIGHT | HID_D => {
            game.try_move(1, 0);
        }
        HID_DOWN | HID_S => {
            game.try_move(0, 1);
        }
        _ => {}
    }
}
