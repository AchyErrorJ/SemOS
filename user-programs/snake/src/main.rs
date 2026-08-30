//! snake — first real Ring-3 game on SemOS (userland game kit).
//!
//! Claims the screen + keyboard (SYS_FB_CLAIM), steers with arrows/WASD via
//! raw key events (SYS_KB_POLL — press AND release, PS/2 + USB normalized),
//! and blits only dirty cells (SYS_FB_BLIT is per-pixel volatile — a
//! fullscreen pass is ~200 ms, so the whole performance design is
//! 16×16 dirty rects). Paced by SYS_FB_WAIT_VBLANK with a tick-sleep
//! fallback off-HD4600. ESC / Ctrl+C / q quits.
//!
//! Grid is a power-of-two torus (64×32 cells of 16 px = 1024×512 playfield):
//! wrap is a mask, cell index is `y<<6|x`, occupancy is one u64 per row —
//! no division, no modulo, no slice-index panics. That keeps the code shape
//! within reach of the on-device semos-rustc for a future selfdev demo.

#![no_std]
#![no_main]

use semos_std::arch::{syscall0, SYS_TIME};
use semos_std::time::Instant;
use semos_std::{fb, kb, main, println, thread};

const COLS: usize = 64; // power of 2
const ROWS: usize = 32; // power of 2
const CELL: usize = 16; // px, power of 2
const PLAY_W: usize = COLS * CELL; // 1024
const PLAY_H: usize = ROWS * CELL; // 512

const COLOR_BG: u32 = 0x0000_0000;
const COLOR_BODY: u32 = 0x0030_D030;
const COLOR_HEAD: u32 = 0x0060_FF60;
const COLOR_FOOD: u32 = 0x00D0_3030;
const COLOR_BORDER: u32 = 0x0040_4040;

const START_LEN: usize = 4;
const START_STEP_TICKS: u64 = 6; // ~10 cells/s at the ~62 Hz tick
const MIN_STEP_TICKS: u64 = 2; // speed cap

// Direction encoding: 0=right 1=left 2=down 3=up. Opposite = dir ^ 1.
const DIR_RIGHT: u8 = 0;
const DIR_LEFT: u8 = 1;
const DIR_DOWN: u8 = 2;
const DIR_UP: u8 = 3;

// --- game state (statics; no allocator needed) ------------------------------

/// Occupancy: one u64 per row, bit x = cell (x, row) taken by the body.
static mut OCC: [u64; ROWS] = [0; ROWS];
/// Body cells as packed `y<<6|x`, ring over head_pos/tail_pos.
static mut BODY: [u16; (COLS * ROWS) as usize] = [0; COLS * ROWS];
static mut HEAD_POS: usize = 0; // ring index of the head cell
static mut TAIL_POS: usize = 0; // ring index of the tail cell
static mut LEN: usize = 0;
static mut FOOD: u16 = 0xFFFF; // packed cell, 0xFFFF = none on board
static mut RNG: u32 = 1;

// --- pixel buffers (statics; reused by every blit) ---------------------------

const CELL_BUF_LEN: usize = CELL * CELL;
/// Max border piece: top/bottom run (PLAY_W + 4) × 2 = 2056 px.
const BORDER_BUF_LEN: usize = (PLAY_W + 4) * 2;

static mut CELL_BUF: [u32; CELL_BUF_LEN] = [0; CELL_BUF_LEN];
static mut BORDER_BUF: [u32; BORDER_BUF_LEN] = [0; BORDER_BUF_LEN];

fn rng_next() -> u32 {
    unsafe {
        // xorshift32
        RNG ^= RNG << 13;
        RNG ^= RNG >> 17;
        RNG ^= RNG << 5;
        RNG
    }
}

fn cell_index(x: usize, y: usize) -> u16 {
    ((y << 6) | x) as u16
}

fn occ_get(x: usize, y: usize) -> bool {
    unsafe { (OCC[y] >> x) & 1 != 0 }
}

fn occ_set(x: usize, y: usize, on: bool) {
    unsafe {
        if on {
            OCC[y] |= 1u64 << x;
        } else {
            OCC[y] &= !(1u64 << x);
        }
    }
}

/// Blit one 16×16 cell in `color` at playfield cell (cx, cy).
fn blit_cell(cx: usize, cy: usize, color: u32, ox: usize, oy: usize) {
    unsafe {
        let mut i = 0;
        while i < CELL_BUF_LEN {
            CELL_BUF[i] = color;
            i += 1;
        }
        let buf = core::slice::from_raw_parts(core::ptr::addr_of!(CELL_BUF) as *const u32, CELL_BUF_LEN);
        let _ = fb::blit(buf, ox + (cx << 4), oy + (cy << 4), CELL, CELL);
    }
}

fn draw_border(ox: usize, oy: usize) {
    unsafe {
        let mut i = 0;
        while i < BORDER_BUF_LEN {
            BORDER_BUF[i] = COLOR_BORDER;
            i += 1;
        }
        let buf = core::slice::from_raw_parts(core::ptr::addr_of!(BORDER_BUF) as *const u32, BORDER_BUF_LEN);
        let bx = ox.saturating_sub(2);
        let by = oy.saturating_sub(2);
        // top + bottom (PLAY_W+4 × 2)
        let _ = fb::blit(buf, bx, by, PLAY_W + 4, 2);
        let _ = fb::blit(buf, bx, oy + PLAY_H, PLAY_W + 4, 2);
        // left + right (2 × PLAY_H+4; buffer is big enough, tightly packed)
        let _ = fb::blit(buf, bx, by, 2, PLAY_H + 4);
        let _ = fb::blit(buf, ox + PLAY_W, by, 2, PLAY_H + 4);
    }
}

/// Place food on a free cell: rejection-sample, then fall back to a linear
/// scan. Returns false when the board is full (that is a win).
fn spawn_food() -> bool {
    let mut tries = 0;
    while tries < 64 {
        let c = (rng_next() & 0x7FF) as usize; // 2048 cells
        let (x, y) = (c & 63, c >> 6);
        if !occ_get(x, y) {
            unsafe { FOOD = c as u16 };
            return true;
        }
        tries += 1;
    }
    let mut c = 0usize;
    while c < COLS * ROWS {
        if !occ_get(c & 63, c >> 6) {
            unsafe { FOOD = c as u16 };
            return true;
        }
        c += 1;
    }
    false
}

main!(fn main() {
    let info = match fb::fbinfo() {
        Some(i) => i,
        None => {
            println!("snake: no framebuffer on this machine");
            return;
        }
    };
    if (info.width as usize) < PLAY_W || (info.height as usize) < PLAY_H {
        println!(
            "snake: framebuffer {}x{} too small (need {}x{})",
            info.width, info.height, PLAY_W, PLAY_H
        );
        return;
    }
    let ox = ((info.width as usize) - PLAY_W) / 2;
    let oy = ((info.height as usize) - PLAY_H) / 2;

    unsafe {
        // Seed the RNG off the kernel tick count; |1 so xorshift never
        // parks at zero.
        RNG = (syscall0(SYS_TIME) as u32) | 1;

        // Initial snake: 4 cells horizontal, mid-board, moving right.
        let y = ROWS / 2;
        let mut i = 0;
        while i < START_LEN {
            let x = COLS / 2 - START_LEN + i + 1;
            BODY[i] = cell_index(x, y);
            occ_set(x, y, true);
            i += 1;
        }
        HEAD_POS = START_LEN - 1;
        TAIL_POS = 0;
        LEN = START_LEN;
    }

    if !fb::claim(true) {
        println!("snake: screen claim unavailable on this kernel");
        return;
    }

    draw_border(ox, oy);
    unsafe {
        let mut i = TAIL_POS;
        loop {
            let c = BODY[i] as usize;
            let color = if i == HEAD_POS { COLOR_HEAD } else { COLOR_BODY };
            blit_cell(c & 63, c >> 6, color, ox, oy);
            if i == HEAD_POS {
                break;
            }
            i = (i + 1) & (COLS * ROWS - 1);
        }
    }
    if !spawn_food() {
        fb::claim(false);
        println!("snake: you filled the board. score {}", unsafe { LEN } - START_LEN);
        return;
    }
    unsafe {
        let f = FOOD as usize;
        blit_cell(f & 63, f >> 6, COLOR_FOOD, ox, oy);
    }

    let mut dir: u8 = DIR_RIGHT;
    let mut pending: u8 = DIR_RIGHT;
    let mut ctrl_held = false;
    let mut quit = false;
    let mut dead = false;
    let mut evbuf = [0u32; 32];
    let mut last_step = Instant::now();

    while !quit && !dead {
        // --- input ---------------------------------------------------------
        let n = kb::poll(&mut evbuf);
        let mut i = 0;
        while i < n {
            let ev = evbuf[i];
            let code = kb::code(ev);
            let pressed = kb::pressed(ev);
            if code & !kb::EXT == kb::key::CTRL {
                ctrl_held = pressed;
            } else if pressed {
                let want = match code {
                    kb::key::UP | kb::key::W => Some(DIR_UP),
                    kb::key::DOWN | kb::key::S => Some(DIR_DOWN),
                    kb::key::LEFT | kb::key::A => Some(DIR_LEFT),
                    kb::key::RIGHT | kb::key::D => Some(DIR_RIGHT),
                    kb::key::ESC => {
                        quit = true;
                        None
                    }
                    kb::key::C if ctrl_held => {
                        quit = true;
                        None
                    }
                    0x10 => { // q
                        quit = true;
                        None
                    }
                    _ => None,
                };
                // Reject 180° turns (opposite = dir ^ 1).
                if let Some(w) = want {
                    if w != (dir ^ 1) {
                        pending = w;
                    }
                }
            }
            i += 1;
        }

        // --- step ----------------------------------------------------------
        let score = unsafe { LEN } - START_LEN;
        let step_ticks = START_STEP_TICKS - ((score as u64) >> 2).min(START_STEP_TICKS - MIN_STEP_TICKS);
        let now = Instant::now();
        if now.duration_since(last_step).as_ticks() >= step_ticks {
            last_step = now;
            dir = pending;

            let (hx, hy, eating) = unsafe {
                let head = BODY[HEAD_POS] as usize;
                let (mut hx, mut hy) = (head & 63, head >> 6);
                match dir {
                    DIR_RIGHT => hx = (hx + 1) & (COLS - 1),
                    DIR_LEFT => hx = (hx + COLS - 1) & (COLS - 1),
                    DIR_DOWN => hy = (hy + 1) & (ROWS - 1),
                    _ => hy = (hy + ROWS - 1) & (ROWS - 1),
                }
                (hx, hy, cell_index(hx, hy) == FOOD)
            };
            let tail = unsafe { BODY[TAIL_POS] as usize };
            let tail_xy = (tail & 63, tail >> 6);

            // Moving into the vacating tail cell is legal when not eating.
            if occ_get(hx, hy) && (eating || (hx, hy) != tail_xy) {
                dead = true;
            } else {
                unsafe {
                    if !eating {
                        // Vacate the tail.
                        occ_set(tail_xy.0, tail_xy.1, false);
                        blit_cell(tail_xy.0, tail_xy.1, COLOR_BG, ox, oy);
                        TAIL_POS = (TAIL_POS + 1) & (COLS * ROWS - 1);
                    } else {
                        LEN += 1;
                        if spawn_food() {
                            let f = FOOD as usize;
                            blit_cell(f & 63, f >> 6, COLOR_FOOD, ox, oy);
                        } else {
                            FOOD = 0xFFFF; // board full after this bite
                        }
                    }
                    // Old head becomes body color; new head goes on top.
                    let old = BODY[HEAD_POS] as usize;
                    blit_cell(old & 63, old >> 6, COLOR_BODY, ox, oy);
                    occ_set(hx, hy, true);
                    HEAD_POS = (HEAD_POS + 1) & (COLS * ROWS - 1);
                    BODY[HEAD_POS] = cell_index(hx, hy);
                    blit_cell(hx, hy, COLOR_HEAD, ox, oy);
                }
            }
        }

        // --- pacing ---------------------------------------------------------
        if !fb::wait_vblank() {
            thread::sleep_ticks(1);
        }
    }

    if dead {
        // Blink the head, then out.
        let head = unsafe { BODY[HEAD_POS] as usize };
        let (hx, hy) = (head & 63, head >> 6);
        let mut b = 0;
        while b < 3 {
            blit_cell(hx, hy, COLOR_BG, ox, oy);
            thread::sleep_ticks(4);
            blit_cell(hx, hy, COLOR_HEAD, ox, oy);
            thread::sleep_ticks(4);
            b += 1;
        }
    }

    fb::claim(false);
    let score = unsafe { LEN } - START_LEN;
    if dead {
        println!("snake: crashed at length {} — score {}", unsafe { LEN }, score);
    } else {
        println!("snake: score {}", score);
    }
});
