//! M14-F — guarded Haswell/eDP native modeset research path.
//!
//! The T540p currently boots through UEFI GOP. Before replacing GOP ownership,
//! this module models the panel's EDID-native 1920x1080@60.007 timing and
//! exposes shell-controlled read/verify/poke commands. Nothing here runs at
//! boot. The only write operation is `modeset poke-60`, which is deliberately
//! narrow: it writes timing registers only, and does **not** disable/enable the
//! pipe, program DPLL/DDI, relink eDP, or touch the framebuffer base.

use crate::{igpu, println};
use crate::display::mmio::MmioReg;

// Haswell/Gen7 display register offsets in BAR0 for CPU transcoder/pipe A.
// These are the internal-panel path candidates observed/needed for T540p M14.
const TRANS_HTOTAL_A: u64 = 0x60000;
const TRANS_HBLANK_A: u64 = 0x60004;
const TRANS_HSYNC_A:  u64 = 0x60008;
const TRANS_VTOTAL_A: u64 = 0x6000C;
const TRANS_VBLANK_A: u64 = 0x60010;
const TRANS_VSYNC_A:  u64 = 0x60014;
const PIPEASRC:       u64 = 0x6001C;
const TRANS_DDI_FUNC_CTL_A: u64 = 0x60400;
const PIPE_DSL_A:     u64 = 0x70000; // current display scanline, read-only pacing source
const PIPECONF_A:     u64 = 0x70008;
const DSPCNTR_A:      u64 = 0x70180;
const DSPSTRIDE_A:    u64 = 0x70188;
const DSPSURF_A:      u64 = 0x7019C;

#[derive(Clone, Copy)]
pub enum ModeOp {
    Status,
    Plan,
    Verify60,
    Poke60Timings,
    WaitVblank,
}

#[derive(Clone, Copy)]
struct Timing1080p60 {
    hactive: u32,
    hblank: u32,
    hsync_offset: u32,
    hsync_width: u32,
    vactive: u32,
    vblank: u32,
    vsync_offset: u32,
    vsync_width: u32,
    pixel_clock_khz: u32,
}

const T540P_EDP_1080P60: Timing1080p60 = Timing1080p60 {
    // docs/hardware/igpu-2026-07-08/edid_card1-eDP-1_summary.txt
    hactive: 1920,
    hblank: 300,
    hsync_offset: 90,
    hsync_width: 60,
    vactive: 1080,
    vblank: 58,
    vsync_offset: 6,
    vsync_width: 9,
    pixel_clock_khz: 151_600,
};

struct RegPlan {
    htotal: u32,
    hblank: u32,
    hsync: u32,
    vtotal: u32,
    vblank: u32,
    vsync: u32,
    pipeasrc: u32,
}

impl Timing1080p60 {
    fn htotal(&self) -> u32 { self.hactive + self.hblank }
    fn vtotal(&self) -> u32 { self.vactive + self.vblank }

    fn plan(&self) -> RegPlan {
        let hsync_start = self.hactive + self.hsync_offset;
        let hsync_end = hsync_start + self.hsync_width;
        let vsync_start = self.vactive + self.vsync_offset;
        let vsync_end = vsync_start + self.vsync_width;
        RegPlan {
            htotal: pack_pair(self.htotal() - 1, self.hactive - 1),
            hblank: pack_pair(self.htotal() - 1, self.hactive - 1),
            hsync: pack_pair(hsync_end - 1, hsync_start - 1),
            vtotal: pack_pair(self.vtotal() - 1, self.vactive - 1),
            vblank: pack_pair(self.vtotal() - 1, self.vactive - 1),
            vsync: pack_pair(vsync_end - 1, vsync_start - 1),
            pipeasrc: pack_pair(self.hactive - 1, self.vactive - 1),
        }
    }
}

#[inline]
fn pack_pair(high: u32, low: u32) -> u32 { (high << 16) | (low & 0xFFFF) }

pub fn run(op: ModeOp) -> u64 {
    let info = match igpu::find() {
        Some(i) if i.device_id == igpu::HASWELL_GT2_MOBILE_HD4600 => i,
        Some(i) => {
            println!("modeset: Intel GPU 0x{:04X} is not the M14 HD4600 target", i.device_id);
            return u64::MAX;
        }
        None => {
            println!("modeset: no Intel display controller found");
            return u64::MAX;
        }
    };
    let mmio = match MmioReg::new() {
        Some(m) => m,
        None => {
            println!("modeset: display MMIO unavailable");
            return u64::MAX;
        }
    };

    match op {
        ModeOp::Status => status(&mmio, info.loc),
        ModeOp::Plan => plan(),
        ModeOp::Verify60 => verify_60(&mmio),
        ModeOp::Poke60Timings => poke_60_timings(&mmio),
        ModeOp::WaitVblank => wait_vblank_cmd(&mmio),
    }
}

fn status(mmio: &MmioReg, loc: crate::pci::Location) -> u64 {
    println!("modeset: HD4600 @ {:02X}:{:02X}.{} BAR0 mapped, mode writes are shell-gated", loc.bus, loc.slot, loc.func);
    dump_reg(mmio, "PIPE_DSL_A", PIPE_DSL_A);
    dump_reg(mmio, "PIPECONF_A", PIPECONF_A);
    dump_reg(mmio, "TRANS_DDI_FUNC_CTL_A", TRANS_DDI_FUNC_CTL_A);
    dump_reg(mmio, "TRANS_HTOTAL_A", TRANS_HTOTAL_A);
    dump_reg(mmio, "TRANS_HBLANK_A", TRANS_HBLANK_A);
    dump_reg(mmio, "TRANS_HSYNC_A", TRANS_HSYNC_A);
    dump_reg(mmio, "TRANS_VTOTAL_A", TRANS_VTOTAL_A);
    dump_reg(mmio, "TRANS_VBLANK_A", TRANS_VBLANK_A);
    dump_reg(mmio, "TRANS_VSYNC_A", TRANS_VSYNC_A);
    dump_reg(mmio, "PIPEASRC", PIPEASRC);
    dump_reg(mmio, "DSPCNTR_A", DSPCNTR_A);
    dump_reg(mmio, "DSPSTRIDE_A", DSPSTRIDE_A);
    dump_reg(mmio, "DSPSURF_A", DSPSURF_A);
    0
}

fn plan() -> u64 {
    let t = T540P_EDP_1080P60;
    let p = t.plan();
    println!("modeset plan: CMN N156HGE-EA1 eDP native {}x{} @ ~60.007 Hz", t.hactive, t.vactive);
    println!("  pixel_clock={} kHz htotal={} hblank={} hsync={}+{} vtotal={} vblank={} vsync={}+{}",
        t.pixel_clock_khz, t.htotal(), t.hblank, t.hsync_offset, t.hsync_width,
        t.vtotal(), t.vblank, t.vsync_offset, t.vsync_width);
    print_plan_regs(&p);
    println!("  next safe step: modeset verify-60; full DPLL/DDI/pipe takeover remains gated");
    0
}

fn verify_60(mmio: &MmioReg) -> u64 {
    let p = T540P_EDP_1080P60.plan();
    let mut bad = 0u64;
    bad += check_reg(mmio, "TRANS_HTOTAL_A", TRANS_HTOTAL_A, p.htotal);
    bad += check_reg(mmio, "TRANS_HBLANK_A", TRANS_HBLANK_A, p.hblank);
    bad += check_reg(mmio, "TRANS_HSYNC_A", TRANS_HSYNC_A, p.hsync);
    bad += check_reg(mmio, "TRANS_VTOTAL_A", TRANS_VTOTAL_A, p.vtotal);
    bad += check_reg(mmio, "TRANS_VBLANK_A", TRANS_VBLANK_A, p.vblank);
    bad += check_reg(mmio, "TRANS_VSYNC_A", TRANS_VSYNC_A, p.vsync);
    bad += check_reg(mmio, "PIPEASRC", PIPEASRC, p.pipeasrc);
    if bad == 0 {
        println!("modeset verify-60: live timing regs match the 1920x1080@60.007 oracle");
        0
    } else {
        println!("modeset verify-60: {} timing register(s) differ", bad);
        bad
    }
}

fn poke_60_timings(mmio: &MmioReg) -> u64 {
    let p = T540P_EDP_1080P60.plan();
    println!("modeset poke-60: EXPERIMENTAL timing-register write only");
    println!("modeset poke-60: not touching DPLL/DDI enable, pipe enable, plane base, or eDP training");
    write_reg(mmio, "TRANS_HTOTAL_A", TRANS_HTOTAL_A, p.htotal);
    write_reg(mmio, "TRANS_HBLANK_A", TRANS_HBLANK_A, p.hblank);
    write_reg(mmio, "TRANS_HSYNC_A", TRANS_HSYNC_A, p.hsync);
    write_reg(mmio, "TRANS_VTOTAL_A", TRANS_VTOTAL_A, p.vtotal);
    write_reg(mmio, "TRANS_VBLANK_A", TRANS_VBLANK_A, p.vblank);
    write_reg(mmio, "TRANS_VSYNC_A", TRANS_VSYNC_A, p.vsync);
    write_reg(mmio, "PIPEASRC", PIPEASRC, p.pipeasrc);
    let _ = verify_60(mmio);
    0
}

/// Public 60 Hz pacing primitive for the present path. Read-only: constructs a
/// BAR0 view for the HD4600 target and waits for one Pipe A scanline wrap.
/// Returns true on a detected frame boundary, false if unavailable/timed out.
pub fn wait_vblank() -> bool {
    match igpu::find() {
        Some(i) if i.device_id == igpu::HASWELL_GT2_MOBILE_HD4600 => {}
        _ => return false,
    }
    let mmio = match MmioReg::new() {
        Some(m) => m,
        None => return false,
    };
    wait_for_scanline_wrap(&mmio).is_some()
}

fn wait_vblank_cmd(mmio: &MmioReg) -> u64 {
    let before = read_scanline(mmio);
    match wait_for_scanline_wrap(mmio) {
        Some((after, spins)) => {
            println!(
                "modeset wait-vblank: scanline {} -> {} at frame boundary ({} spins, read-only)",
                before,
                after,
                spins,
            );
            0
        }
        None => {
            println!(
                "modeset wait-vblank: timeout; PIPE_DSL_A stayed near scanline {} (is pipe A active?)",
                read_scanline(mmio),
            );
            u64::MAX
        }
    }
}

/// Wait for the scanline counter to wrap. This is the safest 60 Hz primitive:
/// it only reads PIPE_DSL_A and uses the live display engine's existing frame
/// cadence. No IRQ enables/acks, no mode writes, no plane changes.
fn wait_for_scanline_wrap(mmio: &MmioReg) -> Option<(u32, u64)> {
    let mut last = read_scanline(mmio);
    for spins in 0..25_000_000u64 {
        core::hint::spin_loop();
        let cur = read_scanline(mmio);
        if cur < last && last < 8192 {
            return Some((cur, spins));
        }
        last = cur;
    }
    None
}

#[inline]
fn read_scanline(mmio: &MmioReg) -> u32 {
    mmio.read32(PIPE_DSL_A) & 0x1FFF
}

fn dump_reg(mmio: &MmioReg, name: &str, off: u64) {
    println!("  {:<22} [0x{:05X}] = 0x{:08X}", name, off, mmio.read32(off));
}

fn print_plan_regs(p: &RegPlan) {
    println!("  TRANS_HTOTAL_A = 0x{:08X}", p.htotal);
    println!("  TRANS_HBLANK_A = 0x{:08X}", p.hblank);
    println!("  TRANS_HSYNC_A  = 0x{:08X}", p.hsync);
    println!("  TRANS_VTOTAL_A = 0x{:08X}", p.vtotal);
    println!("  TRANS_VBLANK_A = 0x{:08X}", p.vblank);
    println!("  TRANS_VSYNC_A  = 0x{:08X}", p.vsync);
    println!("  PIPEASRC       = 0x{:08X}", p.pipeasrc);
}

fn check_reg(mmio: &MmioReg, name: &str, off: u64, expected: u32) -> u64 {
    let got = mmio.read32(off);
    if got == expected {
        println!("  OK   {:<16} = 0x{:08X}", name, got);
        0
    } else {
        println!("  DIFF {:<16} got=0x{:08X} want=0x{:08X}", name, got, expected);
        1
    }
}

fn write_reg(mmio: &MmioReg, name: &str, off: u64, value: u32) {
    println!("  WRITE {:<16} [0x{:05X}] <- 0x{:08X}", name, off, value);
    mmio.write32(off, value);
}
