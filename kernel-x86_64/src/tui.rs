//! M22 TUI — kernel-side multi-pane interface for the Claude agent.
//!
//! A Claude-Code-style terminal UI laid out over the M6 framebuffer using the
//! M7/M8 [`TtyConsole`](crate::tty::TtyConsole) panes. Three stacked regions:
//!
//! ```text
//!  ┌──────────────────────────────────────────────┐  status bar (accent bg):
//!  │ Semantic OS · Claude agent · <model> · <state>│  app/model/agent-state
//!  ├──────────────────────────────────────────────┤  ← divider
//!  │ ▸ you   Read /README and summarize.            │  transcript (scrollback):
//!  │ ● claude I'll read the file.                   │  role-coloured turns —
//!  │ ⚙ read_file {"path":"/README"}                 │  user / assistant /
//!  │ ↳ Semantic OS is a bare-metal kernel…          │  tool_use / tool_result
//!  │ ● claude Semantic OS is a bare-metal x86_64…   │
//!  ├──────────────────────────────────────────────┤  ← divider
//!  │ ›                                              │  prompt / input line
//!  └──────────────────────────────────────────────┘
//! ```
//!
//! It lives in the binary crate next to [`crate::agent`] because the agent
//! loop runs in kernel context (it needs the Phase-8 TLS transport, which is
//! not yet exposed to Ring 3 — the Ring-3 TUI wrapper is a later refactor).
//! The agent drives the panes as a turn unfolds: `set_status` while it thinks /
//! connects / runs a tool, `push_*` to append each turn to the transcript.
//!
//! Rendering uses `Aa::Sharp` (M7 1-bit scanline fill) so glyphs land in a
//! *solid* foreground colour with no blending — which also lets DEMO 50 verify
//! each role rendered in its exact colour by counting matching pixels.

use crate::font;
use crate::framebuffer::{self as fb, Color};
use crate::tty::{Aa, TtyConsole};

// ---- palette ---------------------------------------------------------------
const STATUS_BG: Color = fb::rgb(0x10, 0x18, 0x30); // deep blue chrome
const PROMPT_BG: Color = fb::rgb(0x14, 0x14, 0x1A); // near-black input strip
const TRANS_BG: Color = fb::rgb(0x00, 0x00, 0x00); // black transcript
const ACCENT: Color = fb::rgb(0x50, 0x70, 0xFF); // divider + app name
const FG: Color = fb::rgb(0xE0, 0xE0, 0xE0); // default text

const C_USER: Color = fb::rgb(0x40, 0xD0, 0xE0); // cyan
const C_ASSISTANT: Color = fb::rgb(0x60, 0xE0, 0x70); // green
const C_TOOL: Color = fb::rgb(0xE0, 0xC0, 0x40); // yellow
const C_RESULT: Color = fb::rgb(0xA0, 0xA0, 0xA0); // gray
const C_ERROR: Color = fb::rgb(0xE0, 0x50, 0x50); // red

/// A three-pane agent terminal over the framebuffer.
pub struct Tui {
    status: TtyConsole,
    transcript: TtyConsole,
    prompt: TtyConsole,
    model: &'static str,
    // Geometry kept for chrome redraw + headless readback.
    x0: usize,
    w: usize,
    status_y: usize,
    status_h: usize,
    trans_y: usize,
    trans_h: usize,
    prompt_y: usize,
    prompt_h: usize,
}

impl Tui {
    /// Lay the TUI out over the framebuffer for `model`. Anchored to the lower
    /// portion of the screen so the boot bitmap console (top) doesn't clobber
    /// it mid-run. Returns `None` if there's no framebuffer.
    pub fn new(model: &'static str) -> Option<Self> {
        let (fbw, fbh) = fb::fb_dimensions();
        if fbw == 0 || fbh == 0 {
            return None;
        }

        let px = 18.0f32;
        let lh = font::line_height(px).max(px as usize + 2);
        let margin = 12usize;
        let gap = 8usize;
        let x0 = margin;
        let w = fbw.saturating_sub(2 * margin);

        let status_h = lh + 8;
        let prompt_h = lh + 8;
        let trans_rows = 10usize;
        let trans_h = lh * trans_rows;

        let total = status_h + gap + trans_h + gap + prompt_h;
        // Anchor near the bottom; clamp to the top if the screen is short.
        let top = fbh.saturating_sub(total + 16).max(8);

        let status_y = top;
        let trans_y = status_y + status_h + gap;
        let prompt_y = trans_y + trans_h + gap;

        let status = TtyConsole::new(x0, status_y, w, status_h, px, FG, STATUS_BG);
        let transcript = TtyConsole::new(x0, trans_y, w, trans_h, px, FG, TRANS_BG);
        let prompt = TtyConsole::new(x0, prompt_y, w, prompt_h, px, FG, PROMPT_BG);

        let mut tui = Self {
            status,
            transcript,
            prompt,
            model,
            x0,
            w,
            status_y,
            status_h,
            trans_y,
            trans_h,
            prompt_y,
            prompt_h,
        };
        tui.draw_dividers();
        tui.set_status("ready");
        tui.set_prompt("");
        Some(tui)
    }

    /// Accent rule between status/transcript and transcript/prompt.
    fn draw_dividers(&self) {
        let d1 = self.status_y + self.status_h + 2;
        let d2 = self.prompt_y.saturating_sub(4);
        fb::fb_fill_rect(self.x0, d1, self.w, 2, ACCENT);
        fb::fb_fill_rect(self.x0, d2, self.w, 2, ACCENT);
    }

    /// Redraw the status bar: app name (accent) · model · agent state.
    pub fn set_status(&mut self, state: &str) {
        self.status.clear();
        self.status.set_fg(ACCENT);
        self.status.write(Aa::Sharp, "Semantic OS");
        self.status.set_fg(C_RESULT);
        self.status.write(Aa::Sharp, " \u{b7} Claude agent \u{b7} ");
        self.status.set_fg(FG);
        self.status.write(Aa::Sharp, self.model);
        self.status.set_fg(C_RESULT);
        self.status.write(Aa::Sharp, " \u{b7} ");
        self.status.set_fg(C_ASSISTANT);
        self.status.write(Aa::Sharp, state);
    }

    /// Render the input line: `› ` prompt (accent) then the current edit text.
    pub fn set_prompt(&mut self, text: &str) {
        self.prompt.clear();
        self.prompt.set_fg(ACCENT);
        self.prompt.write(Aa::Sharp, "\u{203a} ");
        self.prompt.set_fg(FG);
        self.prompt.write(Aa::Sharp, text);
    }

    /// Append a transcript line: coloured `label` then `text`, newline-terminated.
    fn push(&mut self, color: Color, label: &str, text: &str) {
        self.transcript.set_fg(color);
        self.transcript.write(Aa::Sharp, label);
        self.transcript.set_fg(FG);
        self.transcript.write(Aa::Sharp, text);
        self.transcript.write(Aa::Sharp, "\n");
    }

    /// A user turn.
    pub fn push_user(&mut self, text: &str) {
        self.push(C_USER, "\u{25b8} you  ", text);
    }

    /// An assistant text turn.
    pub fn push_assistant(&mut self, text: &str) {
        self.push(C_ASSISTANT, "\u{25cf} claude  ", text);
    }

    /// A tool invocation (name + raw input JSON), drawn in the tool colour.
    pub fn push_tool_call(&mut self, name: &str, input_json: &str) {
        self.transcript.set_fg(C_TOOL);
        self.transcript.write(Aa::Sharp, "\u{2699} ");
        self.transcript.write(Aa::Sharp, name);
        self.transcript.write(Aa::Sharp, " ");
        self.transcript.write(Aa::Sharp, input_json);
        self.transcript.write(Aa::Sharp, "\n");
    }

    /// The result fed back to the model (truncated for display).
    pub fn push_tool_result(&mut self, text: &str) {
        let shown = if text.len() > 80 { &text[..80] } else { text };
        self.push(C_RESULT, "\u{21b3} result  ", shown);
    }

    /// A system / error notice.
    pub fn push_error(&mut self, text: &str) {
        self.push(C_ERROR, "\u{2715} ", text);
    }

    /// Scroll the transcript back/forward through retained history.
    pub fn scroll_to(&mut self, top: usize) {
        self.transcript.show_scrollback(top);
    }

    /// `(x, y, w, h)` of each pane — for headless pixel-readback validation.
    pub fn status_rect(&self) -> (usize, usize, usize, usize) {
        (self.x0, self.status_y, self.w, self.status_h)
    }
    pub fn transcript_rect(&self) -> (usize, usize, usize, usize) {
        (self.x0, self.trans_y, self.w, self.trans_h)
    }
    pub fn prompt_rect(&self) -> (usize, usize, usize, usize) {
        (self.x0, self.prompt_y, self.w, self.prompt_h)
    }
}

/// Count pixels in region `(x,y,w,h)` that exactly equal `color` (Sharp glyphs
/// fill in solid fg, so a non-zero count proves text rendered in that colour).
pub fn count_color(x: usize, y: usize, w: usize, h: usize, color: Color) -> usize {
    let (fbw, fbh) = fb::fb_dimensions();
    let x1 = (x + w).min(fbw);
    let y1 = (y + h).min(fbh);
    let mut n = 0;
    let mut yy = y;
    while yy < y1 {
        let mut xx = x;
        while xx < x1 {
            if fb::fb_read_pixel(xx, yy) == color {
                n += 1;
            }
            xx += 1;
        }
        yy += 1;
    }
    n
}

/// Count pixels in region `(x,y,w,h)` that differ from `bg` (any rendered ink).
pub fn count_non_bg(x: usize, y: usize, w: usize, h: usize, bg: Color) -> usize {
    let (fbw, fbh) = fb::fb_dimensions();
    let x1 = (x + w).min(fbw);
    let y1 = (y + h).min(fbh);
    let mut n = 0;
    let mut yy = y;
    while yy < y1 {
        let mut xx = x;
        while xx < x1 {
            if fb::fb_read_pixel(xx, yy) != bg {
                n += 1;
            }
            xx += 1;
        }
        yy += 1;
    }
    n
}

/// The role colours, exported so a demo can assert each one rendered.
pub const ROLE_COLORS: [Color; 4] = [C_USER, C_ASSISTANT, C_TOOL, C_RESULT];
pub const TRANSCRIPT_BG: Color = TRANS_BG;
pub const STATUS_BG_C: Color = STATUS_BG;
pub const PROMPT_BG_C: Color = PROMPT_BG;
