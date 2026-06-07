//! Pong — a kernel-side fullscreen game, launched by sem-sh's `pong` builtin
//! (SYS_PONG). Mirrors the editor / agent TUI shape: holds FULLSCREEN_APP_ACTIVE
//! for the duration, drives the framebuffer directly, and reads raw USB HID
//! polls so each keystroke (W / S / Up / Down / Esc) is a per-press event,
//! not a cooked line.
//!
//! Default mode is 1P-vs-CPU: you steer the left paddle with W/S (or
//! Up/Down — both work), the right paddle is a speed-capped tracker that
//! can still be beaten by a well-angled smash. Press T to flip into 2P
//! local (left = W/S, right = Up/Down). Space pauses / restarts after a
//! win, Esc quits. First to 7 wins.

use crate::framebuffer as fb;
use crate::{font, usb};
use kernel_core::syscall::{dispatch, numbers::SYS_SLEEP};

// Palette — neon-on-black for visibility on real hardware.
const BG: u32 = 0x0000_0000;
const NET: u32 = 0x0044_4444;
const LEFT: u32 = 0x0044_C8FF; // cyan
const RIGHT: u32 = 0x00FF_8844; // amber
const BALL: u32 = 0x00FF_FFFF;
const SCORE: u32 = 0x00B0_B0B0;

const WIN_SCORE: u32 = 7;
// Fixed simulation step. ~62 Hz scheduler ticks → SLEEP 1 yields ~16 ms,
// close enough to a 60 FPS feel for a paddle game.
const STEP_TICKS: u64 = 1;

// HID Usage IDs (Keyboard/Keypad page).
const HID_W: u8 = 0x1A;
const HID_S: u8 = 0x16;
const HID_UP: u8 = 0x52;
const HID_DOWN: u8 = 0x51;
const HID_ESC: u8 = 0x29;
const HID_SPACE: u8 = 0x2C;
const HID_T: u8 = 0x17;

#[derive(Default, Clone, Copy)]
struct Paddle {
    /// Top of the paddle, in pixels.
    y: i32,
}

#[derive(Clone, Copy)]
struct Ball {
    x: i32,
    y: i32,
    /// Velocity in fixed-point: pixels per tick × 256.
    vx: i32,
    vy: i32,
}

struct Game {
    w: i32,
    h: i32,
    paddle_w: i32,
    paddle_h: i32,
    ball_size: i32,
    left: Paddle,
    right: Paddle,
    ball: Ball,
    score_l: u32,
    score_r: u32,
    quit: bool,
    paused: bool,
    /// Frames remaining on the "celebrate a point" flash. 0 = idle.
    flash: u32,
    /// PRNG state for serving direction. xorshift64.
    rng: u64,
    /// If true, the right paddle is CPU-controlled (1P mode); if false,
    /// it's the second human player (2P mode). Toggled with T.
    ai_right: bool,
}

impl Game {
    fn new(w: i32, h: i32) -> Self {
        // Sizes scale with the screen so 800×600 and 1920×1080 both look right.
        let paddle_h = (h / 8).max(40);
        let paddle_w = (w / 80).max(8);
        let ball_size = (w / 100).max(8);
        let mut g = Self {
            w,
            h,
            paddle_w,
            paddle_h,
            ball_size,
            left: Paddle { y: (h - paddle_h) / 2 },
            right: Paddle { y: (h - paddle_h) / 2 },
            ball: Ball { x: 0, y: 0, vx: 0, vy: 0 },
            score_l: 0,
            score_r: 0,
            quit: false,
            paused: false,
            flash: 0,
            rng: 0xDEAD_BEEF_CAFE_F00D ^ kernel_core::platform::ticks().wrapping_mul(0x9E37_79B9_7F4A_7C15),
            ai_right: true,
        };
        g.serve(true);
        g
    }

    /// Decide which direction the AI paddle should move this tick.
    ///
    /// Strategy: when the ball is moving toward the AI, chase its centre Y
    /// with a small dead-zone so the paddle doesn't jitter. When the ball
    /// is moving away, drift back to centre so we're not stuck at one
    /// edge if the player lobs a high-angle return next. The paddle's max
    /// speed is capped slightly under the human's so a well-placed angled
    /// hit can outrun it — keeps the game beatable.
    fn ai_step(&self) -> i32 {
        let paddle_centre = self.right.y + self.paddle_h / 2;
        let target = if self.ball.vx > 0 {
            // Ball heading toward us — track the ball.
            self.ball.y + self.ball_size / 2
        } else {
            // Ball heading away — drift home.
            self.h / 2
        };
        // Dead-zone scales with paddle size so it's not jittery on tall
        // displays, not sluggish on short ones.
        let deadzone = self.paddle_h / 8;
        if target > paddle_centre + deadzone {
            1
        } else if target < paddle_centre - deadzone {
            -1
        } else {
            0
        }
    }

    fn rand(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    fn serve(&mut self, toward_right: bool) {
        self.ball.x = (self.w - self.ball_size) / 2;
        self.ball.y = (self.h - self.ball_size) / 2;
        // Base speed ~ width / 110 pixels per tick → in fixed-point that's
        // (w / 110) * 256 = w * 256 / 110. At 1920×1080, 62 FPS, that's
        // ~17 px/tick → ~1080 px/s, so the ball crosses the screen in
        // under two seconds.
        let base = self.w * 256 / 110;
        let dir_x = if toward_right { 1 } else { -1 };
        // Random vertical bias: ±(0..1/4 of base) so serves aren't too steep.
        let bias = ((self.rand() % (base as u64 / 2)) as i32) - (base / 4);
        self.ball.vx = base * dir_x;
        self.ball.vy = bias;
    }

    fn update(&mut self, l_dir: i32, r_dir: i32) {
        if self.paused || self.flash > 0 {
            if self.flash > 0 {
                self.flash -= 1;
            }
            return;
        }
        // Paddles move at a fixed speed in pixels per tick. The AI runs
        // a hair slower than a human so a well-angled hit can beat it.
        let paddle_speed = (self.h / 36).max(8);
        let ai_speed = (paddle_speed * 75) / 100;
        self.left.y = (self.left.y + l_dir * paddle_speed).clamp(0, self.h - self.paddle_h);
        let r_speed = if self.ai_right { ai_speed } else { paddle_speed };
        self.right.y = (self.right.y + r_dir * r_speed).clamp(0, self.h - self.paddle_h);

        // Advance ball in fixed-point.
        self.ball.x += self.ball.vx / 256;
        self.ball.y += self.ball.vy / 256;

        // Top/bottom walls.
        if self.ball.y < 0 {
            self.ball.y = 0;
            self.ball.vy = -self.ball.vy;
        } else if self.ball.y + self.ball_size > self.h {
            self.ball.y = self.h - self.ball_size;
            self.ball.vy = -self.ball.vy;
        }

        // Paddle collisions: the left paddle lives at x ∈ [0, paddle_w),
        // the right paddle at x ∈ [w - paddle_w, w).
        let bsz = self.ball_size;
        let pw = self.paddle_w;
        let ph = self.paddle_h;
        let speed_up = 16; // accelerate slightly each hit (1/16 = 6.25%)
        // Left paddle.
        if self.ball.vx < 0
            && self.ball.x <= pw
            && self.ball.x + bsz > 0
            && self.ball.y + bsz >= self.left.y
            && self.ball.y <= self.left.y + ph
        {
            self.ball.x = pw;
            self.ball.vx = -self.ball.vx;
            self.ball.vx += self.ball.vx / speed_up;
            // Add english based on where on the paddle it hit.
            let offset = (self.ball.y + bsz / 2) - (self.left.y + ph / 2);
            self.ball.vy = self.ball.vy + offset * 4;
        }
        // Right paddle.
        if self.ball.vx > 0
            && self.ball.x + bsz >= self.w - pw
            && self.ball.x < self.w
            && self.ball.y + bsz >= self.right.y
            && self.ball.y <= self.right.y + ph
        {
            self.ball.x = self.w - pw - bsz;
            self.ball.vx = -self.ball.vx;
            self.ball.vx += self.ball.vx / speed_up;
            let offset = (self.ball.y + bsz / 2) - (self.right.y + ph / 2);
            self.ball.vy = self.ball.vy + offset * 4;
        }

        // Scoring — the ball passed a goal line entirely.
        if self.ball.x + bsz < 0 {
            self.score_r += 1;
            self.flash = 30; // ~30 frames pause
            if self.score_r >= WIN_SCORE {
                self.paused = true;
            } else {
                self.serve(false);
            }
        } else if self.ball.x > self.w {
            self.score_l += 1;
            self.flash = 30;
            if self.score_l >= WIN_SCORE {
                self.paused = true;
            } else {
                self.serve(true);
            }
        }
    }

    fn render(&self) {
        let w = self.w as usize;
        let h = self.h as usize;

        // Clear background.
        fb::fb_fill_rect(0, 0, w, h, BG);

        // Dashed centre net: short vertical segments.
        let dash_w = 4usize;
        let dash_h = 16usize;
        let dash_gap = 12usize;
        let mid = (w - dash_w) / 2;
        let mut y = 8usize;
        while y + dash_h < h {
            fb::fb_fill_rect(mid, y, dash_w, dash_h, NET);
            y += dash_h + dash_gap;
        }

        // Paddles.
        fb::fb_fill_rect(0, self.left.y as usize, self.paddle_w as usize, self.paddle_h as usize, LEFT);
        fb::fb_fill_rect(
            (self.w - self.paddle_w) as usize,
            self.right.y as usize,
            self.paddle_w as usize,
            self.paddle_h as usize,
            RIGHT,
        );

        // Ball.
        fb::fb_fill_rect(
            self.ball.x as usize,
            self.ball.y as usize,
            self.ball_size as usize,
            self.ball_size as usize,
            BALL,
        );

        // Scores: big digits at top quarters.
        // Format the scores as two decimal characters each so we don't need
        // alloc::string here. WIN_SCORE is 7, but pad to 2 just in case.
        let mut left_buf = [b' '; 2];
        let mut right_buf = [b' '; 2];
        write_two_digit(&mut left_buf, self.score_l);
        write_two_digit(&mut right_buf, self.score_r);
        let score_px = (h as f32 / 8.0).max(24.0);
        let baseline_y = h / 5;
        let left_str = core::str::from_utf8(&left_buf).unwrap_or("?");
        let right_str = core::str::from_utf8(&right_buf).unwrap_or("?");
        let _ = font::fb_draw_text(w / 4 - 24, baseline_y, left_str, score_px, SCORE);
        let _ = font::fb_draw_text(3 * w / 4 - 24, baseline_y, right_str, score_px, SCORE);

        // Status line at the bottom.
        let won_l = self.score_l >= WIN_SCORE;
        let won_r = self.score_r >= WIN_SCORE;
        let status = if won_l {
            if self.ai_right { "YOU WIN  —  Space to restart, Esc to quit" }
            else { "LEFT WINS  —  Space to restart, Esc to quit" }
        } else if won_r {
            if self.ai_right { "CPU WINS  —  Space to restart, Esc to quit" }
            else { "RIGHT WINS  —  Space to restart, Esc to quit" }
        } else if self.paused {
            "Paused — Space to resume, Esc to quit"
        } else if self.ai_right {
            "1P vs CPU   W/S or Up/Dn   T: 2P mode   Space: pause   Esc: quit"
        } else {
            "2P local   Left: W/S   Right: Up/Dn   T: 1P mode   Space: pause   Esc: quit"
        };
        let _ = font::fb_draw_text(16, h - 12, status, 14.0, SCORE);

        let _ = fb::fb_present();
    }
}

fn write_two_digit(buf: &mut [u8; 2], n: u32) {
    let n = n.min(99);
    if n >= 10 {
        buf[0] = b'0' + (n / 10) as u8;
        buf[1] = b'0' + (n % 10) as u8;
    } else {
        buf[0] = b' ';
        buf[1] = b'0' + n as u8;
    }
}

/// Per-key "is this key currently held down?" tracking. Survives across
/// frames because USB boot keyboards only emit a report on state change
/// (and PS/2 only emits ASCII byte impulses) — so reading "the keys in
/// this frame" misses everything in steady state.
#[derive(Default, Clone, Copy)]
struct Held {
    w: bool,
    s: bool,
    up: bool,
    down: bool,
}

impl Held {
    /// Diff a new USB HID boot report against the previous one and update
    /// our held flags. `prev` and `cur` are the 6-key rosters (HID Usage
    /// IDs; 0 in a slot = empty).
    fn apply_hid(&mut self, prev: &[u8; 6], cur: &[u8; 6]) {
        // Newly pressed = in cur but not in prev. Newly released = vice versa.
        for &k in cur.iter() {
            if k == 0 || prev.contains(&k) { continue; }
            match k {
                HID_W => self.w = true,
                HID_S => self.s = true,
                HID_UP => self.up = true,
                HID_DOWN => self.down = true,
                _ => {}
            }
        }
        for &k in prev.iter() {
            if k == 0 || cur.contains(&k) { continue; }
            match k {
                HID_W => self.w = false,
                HID_S => self.s = false,
                HID_UP => self.up = false,
                HID_DOWN => self.down = false,
                _ => {}
            }
        }
    }
}

/// SYS_PONG entry: run the game until the user hits Esc. Holds
/// FULLSCREEN_APP_ACTIVE so the interactive shell's keyboard pump doesn't race
/// ours, and clears the framebuffer on exit.
///
/// Input is dual-sourced so the game works on both USB HID (real hardware,
/// QEMU `-device usb-kbd`) AND PS/2 (QEMU's default — i8042). USB HID drives
/// proper held-key continuous motion via a diff against the previous report;
/// PS/2 only exposes ASCII byte impulses (with autorepeat at ~25 Hz), so each
/// PS/2 byte becomes one paddle-tick of motion.
pub fn run() -> u64 {
    use core::sync::atomic::Ordering;

    let (w, h) = fb::fb_dimensions();
    if w == 0 || h == 0 {
        return 1; // headless — no framebuffer to draw on
    }

    crate::FULLSCREEN_APP_ACTIVE.store(true, Ordering::Relaxed);
    // Stop the TTY from echoing typed keys onto the framebuffer (or
    // accumulating them in the cooked-mode pend buffer for the shell to
    // see when we exit).
    crate::tty::SUPPRESS_TTY_INPUT.store(true, Ordering::Relaxed);

    // Drain any ASCII bytes already buffered by the PS/2 ISR before we set
    // the suppress flag — typically the trailing newline from "pong\n".
    while crate::keyboard::read_key().is_some() {}

    let mut game = Game::new(w as i32, h as i32);
    game.render();

    let mut prev_hid: [u8; 6] = [0; 6];
    let mut held = Held::default();
    // Tiny saturating counters for PS/2 impulse motion: each ASCII byte
    // adds 2 ticks of motion in the chosen direction. Drained 1 tick/frame.
    let mut ps2_up: u8 = 0;
    let mut ps2_down: u8 = 0;

    loop {
        if game.quit {
            break;
        }

        // 1. USB HID path: drain transfer events, diff against prev report.
        usb::xhci::poll_hid(|rep| {
            // Edge events for Esc / Space / T.
            for &k in rep.keys.iter() {
                if k == 0 || prev_hid.contains(&k) { continue; }
                handle_edge(&mut game, k);
            }
            // Then update held state from press/release diff.
            held.apply_hid(&prev_hid, &rep.keys);
            prev_hid = rep.keys;
        });

        // 2. PS/2 ASCII path: each byte is an impulse. Arrow keys go into TTY
        // as ESC[A/B sequences so we won't see them here, but W/S/Esc/Space
        // arrive as plain ASCII.
        while let Some(b) = crate::keyboard::read_key() {
            match b {
                // Each PS/2 byte buys ~4 frames of motion. PS/2 autorepeats
                // at ~25 Hz vs our ~62 FPS frame rate, so a byte arrives
                // every ~2.5 frames — 4 frames of credit per byte keeps the
                // paddle moving continuously while you hold the key.
                b'w' | b'W' => ps2_up = ps2_up.saturating_add(4),
                b's' | b'S' => ps2_down = ps2_down.saturating_add(4),
                b'q' | b'Q' | 0x1B => game.quit = true,
                b' ' => handle_edge(&mut game, HID_SPACE),
                b't' | b'T' => handle_edge(&mut game, HID_T),
                _ => {}
            }
        }

        // 3. Compose paddle direction from both sources.
        let mut l_dir = 0i32;
        let mut r_dir = 0i32;
        if game.ai_right {
            // 1P: W/S/Up/Down all drive the human (left) paddle.
            if held.w || held.up || ps2_up > 0 { l_dir -= 1; }
            if held.s || held.down || ps2_down > 0 { l_dir += 1; }
            r_dir = game.ai_step();
        } else {
            // 2P: W/S = left, Up/Down = right (USB only — PS/2 arrows
            // don't reach us here, so 2P mode is best on USB HID setups).
            if held.w || ps2_up > 0 { l_dir -= 1; }
            if held.s || ps2_down > 0 { l_dir += 1; }
            if held.up { r_dir -= 1; }
            if held.down { r_dir += 1; }
        }

        // Drain one tick of impulse credit per frame.
        ps2_up = ps2_up.saturating_sub(1);
        ps2_down = ps2_down.saturating_sub(1);

        game.update(l_dir.clamp(-1, 1), r_dir.clamp(-1, 1));
        game.render();

        // Re-pin FD/namespace resolution to us (slot drifts across sleeps),
        // mirroring the editor's pattern.
        kernel_core::process::set_kernel_task_id(Some(
            kernel_core::scheduler::current_task_index(),
        ));
        let _ = dispatch(SYS_SLEEP, STEP_TICKS, 0, 0, 0);
    }

    crate::tty::SUPPRESS_TTY_INPUT.store(false, Ordering::Relaxed);
    crate::FULLSCREEN_APP_ACTIVE.store(false, Ordering::Relaxed);
    fb::clear();
    let _ = fb::fb_present();
    0
}

/// Edge-triggered handler shared between the USB HID and PS/2 input paths.
/// `code` is a HID Usage ID (for the USB path) or one of our synthetic
/// HID_SPACE / HID_T values (for the PS/2 path, which passes the matching
/// HID code so the dispatch table is the same).
fn handle_edge(game: &mut Game, code: u8) {
    match code {
        HID_ESC => game.quit = true,
        HID_T => {
            let new_ai = !game.ai_right;
            let nw = game.w;
            let nh = game.h;
            *game = Game::new(nw, nh);
            game.ai_right = new_ai;
        }
        HID_SPACE => {
            if game.score_l >= WIN_SCORE || game.score_r >= WIN_SCORE {
                let nw = game.w;
                let nh = game.h;
                let was_ai = game.ai_right;
                *game = Game::new(nw, nh);
                game.ai_right = was_ai;
            } else {
                game.paused = !game.paused;
            }
        }
        _ => {}
    }
}
