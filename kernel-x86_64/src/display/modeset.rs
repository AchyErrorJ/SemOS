//! M14-F — guarded Haswell/eDP native modeset research path.
//!
//! The T540p currently boots through UEFI GOP. Before replacing GOP ownership,
//! this module models the panel's EDID-native 1920x1080@60.007 timing and
//! exposes shell-controlled read/verify/poke commands. Nothing here runs at
//! boot. Write operations stay narrow and reversible: `poke-60` writes timing
//! registers only (no pipe/DPLL/DDI changes, no eDP relink, no framebuffer
//! move), `restore-gop` writes back a `modeset snapshot` taken earlier in the
//! same boot, and `wells` does one bounded force-wake acquire/release. The
//! full DPLL/DDI/pipe takeover (native-60) remains gated on the refreshed
//! Pop!_OS i915 oracle capture.

use crate::{igpu, println};
use crate::display::mmio::MmioReg;

// Haswell/Gen7 display register offsets in BAR0 for CPU transcoder/pipe A.
// These are the internal-panel path candidates observed/needed for T540p M14.

// Power wells / force-wake
const PWR_WELL_CTL:       u64 = 0x45400;
const PWR_WELL_CTL2:      u64 = 0x45404;
// Haswell force-wake (i915 FORCEWAKE_MT path — the gen6-era split
// render/media registers at 0xA254/0xA258 do not exist on HSW; writes there
// land nowhere and reads return 0). These are masked-write registers: the
// upper 16 bits are the write-enable mask, so acquire is a plain
// _MASKED_BIT_ENABLE-style constant write, not a read-modify-write.
const FORCEWAKE_MT:      u64 = 0xA188;  // multi-threaded force-wake request
const FORCEWAKE_MT_ACK:  u64 = 0x130040; // multi-threaded force-wake acknowledge
const FORCEWAKE_KERNEL:  u32 = 0x1;      // kernel-driver hold bit

// DPLL 0 (display PLL for the eDP path)
const DPLL_CTRL1:         u64 = 0x6C058;
const DPLL_CFGCR1:        u64 = 0x6C080;
const DPLL_CFGCR2:        u64 = 0x6C084;

// DDI A (internal eDP panel)
const DDI_BUF_CTL_A:      u64 = 0x64000;
const DP_TP_CTL_A:        u64 = 0x64040;
const DP_TP_STATUS_A:     u64 = 0x64044;
const DDI_BUF_TRANS_A:    u64 = 0x64E00; // base of 9-entry translation table

// Transcoder A timing
const TRANS_HTOTAL_A: u64 = 0x60000;
const TRANS_HBLANK_A: u64 = 0x60004;
const TRANS_HSYNC_A:  u64 = 0x60008;
const TRANS_VTOTAL_A: u64 = 0x6000C;
const TRANS_VBLANK_A: u64 = 0x60010;
const TRANS_VSYNC_A:  u64 = 0x60014;
const PIPEASRC:       u64 = 0x6001C;
const TRANS_DDI_FUNC_CTL_A: u64 = 0x60400;

// Pipe A
const PIPE_DSL_A:     u64 = 0x70000; // current display scanline, read-only pacing source
const PIPECONF_A:     u64 = 0x70008;

// Primary plane A
const DSPCNTR_A:      u64 = 0x70180;
const DSPSTRIDE_A:    u64 = 0x70188;
const DSPSURF_A:      u64 = 0x7019C;
const DSPTILEOFF_A:   u64 = 0x701A0;
const DSPPOS_A:       u64 = 0x7018C;
const DSPSIZE_A:      u64 = 0x70190;

#[derive(Clone, Copy)]
pub enum ModeOp {
    Status,
    Plan,
    Verify60,
    Poke60Timings,
    WaitVblank,
    Snapshot,
    Native60,
    RestoreGop,
    Wells,
}

/// The saved display state `restore-gop` writes back. Filled by
/// `modeset snapshot`; one-shot — cleared by a successful restore.
static SNAP: spin::Mutex<Option<DisplaySnapshot>> = spin::Mutex::new(None);

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

/// Snapshot of the display engine state for save/restore and oracle comparison.
/// All offsets are for Haswell Pipe/Transcoder/DDI/Plane A and DPLL 0.
#[derive(Clone, Copy, Default)]
struct DisplaySnapshot {
    // Power / force-wake
    pwr_well_ctl: u32,
    pwr_well_ctl2: u32,
    forcewake_mt: u32,
    forcewake_mt_ack: u32,

    // DPLL 0
    dpll_ctrl1: u32,
    dpll_cfgcr1: u32,
    dpll_cfgcr2: u32,

    // DDI A (eDP)
    ddi_buf_ctl_a: u32,
    dp_tp_ctl_a: u32,
    dp_tp_status_a: u32,

    // Transcoder A timing
    trans_htotal_a: u32,
    trans_hblank_a: u32,
    trans_hsync_a: u32,
    trans_vtotal_a: u32,
    trans_vblank_a: u32,
    trans_vsync_a: u32,
    pipeasrc: u32,
    trans_ddi_func_ctl_a: u32,

    // Pipe A
    pipeconf_a: u32,

    // Plane A
    dspcntr_a: u32,
    dspstride_a: u32,
    dspsurf_a: u32,
    dsptileoff_a: u32,
    dsppos_a: u32,
    dspsize_a: u32,
}

impl DisplaySnapshot {
    fn read(mmio: &MmioReg) -> Self {
        Self {
            pwr_well_ctl: mmio.read32(PWR_WELL_CTL),
            pwr_well_ctl2: mmio.read32(PWR_WELL_CTL2),
            forcewake_mt: mmio.read32(FORCEWAKE_MT),
            forcewake_mt_ack: mmio.read32(FORCEWAKE_MT_ACK),

            dpll_ctrl1: mmio.read32(DPLL_CTRL1),
            dpll_cfgcr1: mmio.read32(DPLL_CFGCR1),
            dpll_cfgcr2: mmio.read32(DPLL_CFGCR2),

            ddi_buf_ctl_a: mmio.read32(DDI_BUF_CTL_A),
            dp_tp_ctl_a: mmio.read32(DP_TP_CTL_A),
            dp_tp_status_a: mmio.read32(DP_TP_STATUS_A),

            trans_htotal_a: mmio.read32(TRANS_HTOTAL_A),
            trans_hblank_a: mmio.read32(TRANS_HBLANK_A),
            trans_hsync_a: mmio.read32(TRANS_HSYNC_A),
            trans_vtotal_a: mmio.read32(TRANS_VTOTAL_A),
            trans_vblank_a: mmio.read32(TRANS_VBLANK_A),
            trans_vsync_a: mmio.read32(TRANS_VSYNC_A),
            pipeasrc: mmio.read32(PIPEASRC),
            trans_ddi_func_ctl_a: mmio.read32(TRANS_DDI_FUNC_CTL_A),

            pipeconf_a: mmio.read32(PIPECONF_A),

            dspcntr_a: mmio.read32(DSPCNTR_A),
            dspstride_a: mmio.read32(DSPSTRIDE_A),
            dspsurf_a: mmio.read32(DSPSURF_A),
            dsptileoff_a: mmio.read32(DSPTILEOFF_A),
            dsppos_a: mmio.read32(DSPPOS_A),
            dspsize_a: mmio.read32(DSPSIZE_A),
        }
    }

    fn print(&self) {
        println!("modeset snapshot:");
        println!("  PWR_WELL_CTL          [0x{:05X}] = 0x{:08X}", PWR_WELL_CTL, self.pwr_well_ctl);
        println!("  PWR_WELL_CTL2         [0x{:05X}] = 0x{:08X}", PWR_WELL_CTL2, self.pwr_well_ctl2);
        println!("  FORCEWAKE_MT          [0x{:05X}] = 0x{:08X}", FORCEWAKE_MT, self.forcewake_mt);
        println!("  FORCEWAKE_MT_ACK      [0x{:05X}] = 0x{:08X}", FORCEWAKE_MT_ACK, self.forcewake_mt_ack);
        println!("  DPLL_CTRL1            [0x{:05X}] = 0x{:08X}", DPLL_CTRL1, self.dpll_ctrl1);
        println!("  DPLL_CFGCR1           [0x{:05X}] = 0x{:08X}", DPLL_CFGCR1, self.dpll_cfgcr1);
        println!("  DPLL_CFGCR2           [0x{:05X}] = 0x{:08X}", DPLL_CFGCR2, self.dpll_cfgcr2);
        println!("  DDI_BUF_CTL_A         [0x{:05X}] = 0x{:08X}", DDI_BUF_CTL_A, self.ddi_buf_ctl_a);
        println!("  DP_TP_CTL_A           [0x{:05X}] = 0x{:08X}", DP_TP_CTL_A, self.dp_tp_ctl_a);
        println!("  DP_TP_STATUS_A        [0x{:05X}] = 0x{:08X}", DP_TP_STATUS_A, self.dp_tp_status_a);
        println!("  TRANS_HTOTAL_A        [0x{:05X}] = 0x{:08X}", TRANS_HTOTAL_A, self.trans_htotal_a);
        println!("  TRANS_HBLANK_A        [0x{:05X}] = 0x{:08X}", TRANS_HBLANK_A, self.trans_hblank_a);
        println!("  TRANS_HSYNC_A         [0x{:05X}] = 0x{:08X}", TRANS_HSYNC_A, self.trans_hsync_a);
        println!("  TRANS_VTOTAL_A        [0x{:05X}] = 0x{:08X}", TRANS_VTOTAL_A, self.trans_vtotal_a);
        println!("  TRANS_VBLANK_A        [0x{:05X}] = 0x{:08X}", TRANS_VBLANK_A, self.trans_vblank_a);
        println!("  TRANS_VSYNC_A         [0x{:05X}] = 0x{:08X}", TRANS_VSYNC_A, self.trans_vsync_a);
        println!("  PIPEASRC              [0x{:05X}] = 0x{:08X}", PIPEASRC, self.pipeasrc);
        println!("  TRANS_DDI_FUNC_CTL_A  [0x{:05X}] = 0x{:08X}", TRANS_DDI_FUNC_CTL_A, self.trans_ddi_func_ctl_a);
        println!("  PIPECONF_A            [0x{:05X}] = 0x{:08X}", PIPECONF_A, self.pipeconf_a);
        println!("  DSPCNTR_A             [0x{:05X}] = 0x{:08X}", DSPCNTR_A, self.dspcntr_a);
        println!("  DSPSTRIDE_A           [0x{:05X}] = 0x{:08X}", DSPSTRIDE_A, self.dspstride_a);
        println!("  DSPSURF_A             [0x{:05X}] = 0x{:08X}", DSPSURF_A, self.dspsurf_a);
        println!("  DSPTILEOFF_A          [0x{:05X}] = 0x{:08X}", DSPTILEOFF_A, self.dsptileoff_a);
        println!("  DSPPOS_A              [0x{:05X}] = 0x{:08X}", DSPPOS_A, self.dsppos_a);
        println!("  DSPSIZE_A             [0x{:05X}] = 0x{:08X}", DSPSIZE_A, self.dspsize_a);
    }
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
        ModeOp::Snapshot => snapshot(&mmio),
        ModeOp::Native60 => native_60(&mmio),
        ModeOp::RestoreGop => restore_gop(&mmio),
        ModeOp::Wells => wells(&mmio),
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

/// Capture a snapshot of every display register M14-I may touch, print it,
/// and KEEP it — this is what `restore-gop` writes back. Save-before-restore
/// is the M14 hard rule, so restore refuses to run until a snapshot exists.
fn snapshot(mmio: &MmioReg) -> u64 {
    let s = DisplaySnapshot::read(mmio);
    s.print();
    let mut slot = SNAP.lock();
    println!(
        "modeset snapshot: {}saved for restore-gop (valid this boot only)",
        if slot.is_some() { "overwrote previous; " } else { "" }
    );
    *slot = Some(s);
    0
}

/// M14-I placeholder: perform a full native Haswell modeset to 1920x1080@60.
/// The implementation is gated until the refreshed `i915_display_info.txt`
/// oracle values are committed and the design doc is written.
fn native_60(_mmio: &MmioReg) -> u64 {
    println!("modeset native-60: M14-I takeover not yet implemented");
    println!("modeset native-60: refresh the Pop!_OS oracle capture first (see docs/DISPLAY_HASWELL_NATIVE_MODESET.md)");
    if SNAP.lock().is_some() {
        println!("modeset native-60: snapshot saved — restore-gop is armed for when the takeover lands");
    } else {
        println!("modeset native-60: no snapshot yet — run `modeset snapshot` so restore-gop has something to restore");
    }
    u64::MAX
}

/// Restore the saved display state (GOP-driven, captured by `modeset
/// snapshot`). Conservative by construction: today the saved values are the
/// live ones, so the writes are the same class poke-60 already does. The
/// point is a PROVEN restore path for the day native-60 scrambles something.
/// Write order keeps scanout consistent: plane first, timing next, then the
/// func/DPLL/DDI enables, power/force-wake last.
fn restore_gop(mmio: &MmioReg) -> u64 {
    let s = match SNAP.lock().take() {
        Some(s) => s,
        None => {
            println!("modeset restore-gop: no snapshot this boot — run `modeset snapshot` first");
            println!("modeset restore-gop: (reboot always returns to GOP)");
            return u64::MAX;
        }
    };
    println!("modeset restore-gop: writing back snapshot...");

    // Plane A first — scanout keeps pointing at a consistent surface.
    write_reg(mmio, "DSPCNTR_A", DSPCNTR_A, s.dspcntr_a);
    write_reg(mmio, "DSPSTRIDE_A", DSPSTRIDE_A, s.dspstride_a);
    write_reg(mmio, "DSPSURF_A", DSPSURF_A, s.dspsurf_a);
    write_reg(mmio, "DSPTILEOFF_A", DSPTILEOFF_A, s.dsptileoff_a);
    write_reg(mmio, "DSPPOS_A", DSPPOS_A, s.dsppos_a);
    write_reg(mmio, "DSPSIZE_A", DSPSIZE_A, s.dspsize_a);

    // Transcoder A timing.
    write_reg(mmio, "TRANS_HTOTAL_A", TRANS_HTOTAL_A, s.trans_htotal_a);
    write_reg(mmio, "TRANS_HBLANK_A", TRANS_HBLANK_A, s.trans_hblank_a);
    write_reg(mmio, "TRANS_HSYNC_A", TRANS_HSYNC_A, s.trans_hsync_a);
    write_reg(mmio, "TRANS_VTOTAL_A", TRANS_VTOTAL_A, s.trans_vtotal_a);
    write_reg(mmio, "TRANS_VBLANK_A", TRANS_VBLANK_A, s.trans_vblank_a);
    write_reg(mmio, "TRANS_VSYNC_A", TRANS_VSYNC_A, s.trans_vsync_a);
    write_reg(mmio, "PIPEASRC", PIPEASRC, s.pipeasrc);
    write_reg(mmio, "TRANS_DDI_FUNC_CTL_A", TRANS_DDI_FUNC_CTL_A, s.trans_ddi_func_ctl_a);

    // Pipe + DPLL 0 + DDI A.
    write_reg(mmio, "PIPECONF_A", PIPECONF_A, s.pipeconf_a);
    write_reg(mmio, "DPLL_CFGCR1", DPLL_CFGCR1, s.dpll_cfgcr1);
    write_reg(mmio, "DPLL_CFGCR2", DPLL_CFGCR2, s.dpll_cfgcr2);
    write_reg(mmio, "DPLL_CTRL1", DPLL_CTRL1, s.dpll_ctrl1);
    write_reg(mmio, "DDI_BUF_CTL_A", DDI_BUF_CTL_A, s.ddi_buf_ctl_a);
    write_reg(mmio, "DP_TP_CTL_A", DP_TP_CTL_A, s.dp_tp_ctl_a);

    // Power wells / force-wake last.
    write_reg(mmio, "PWR_WELL_CTL", PWR_WELL_CTL, s.pwr_well_ctl);
    write_reg(mmio, "PWR_WELL_CTL2", PWR_WELL_CTL2, s.pwr_well_ctl2);
    // Snapshot value written raw: FORCEWAKE_MT is masked-write (upper 16 bits
    // are the enable mask), so restoring a live value of 0 is a harmless
    // no-op — the restore never asserts or drops a hold by accident.
    write_reg(mmio, "FORCEWAKE_MT", FORCEWAKE_MT, s.forcewake_mt);

    // Readback audit: any register that didn't take the write is a finding.
    let now = DisplaySnapshot::read(mmio);
    let mut diffs = 0u64;
    macro_rules! audit {
        ($name:literal, $off:expr, $want:expr, $got:expr) => {
            if $want != $got {
                println!("  DIFF {:<20} want=0x{:08X} got=0x{:08X}", $name, $want, $got);
                diffs += 1;
            }
        };
    }
    audit!("DSPSURF_A", DSPSURF_A, s.dspsurf_a, now.dspsurf_a);
    audit!("TRANS_HTOTAL_A", TRANS_HTOTAL_A, s.trans_htotal_a, now.trans_htotal_a);
    audit!("TRANS_VTOTAL_A", TRANS_VTOTAL_A, s.trans_vtotal_a, now.trans_vtotal_a);
    audit!("PIPEASRC", PIPEASRC, s.pipeasrc, now.pipeasrc);
    audit!("TRANS_DDI_FUNC_CTL_A", TRANS_DDI_FUNC_CTL_A, s.trans_ddi_func_ctl_a, now.trans_ddi_func_ctl_a);
    audit!("PIPECONF_A", PIPECONF_A, s.pipeconf_a, now.pipeconf_a);
    audit!("DPLL_CTRL1", DPLL_CTRL1, s.dpll_ctrl1, now.dpll_ctrl1);
    audit!("DDI_BUF_CTL_A", DDI_BUF_CTL_A, s.ddi_buf_ctl_a, now.ddi_buf_ctl_a);
    audit!("PWR_WELL_CTL", PWR_WELL_CTL, s.pwr_well_ctl, now.pwr_well_ctl);
    if diffs == 0 {
        println!("modeset restore-gop: OK — snapshot written back, screen should be unchanged");
        0
    } else {
        println!("modeset restore-gop: {} register(s) did not take the write — note them and reboot if the screen misbehaves", diffs);
        u64::MAX
    }
}

// ---------------------------------------------------------------------------
// Force-wake / power-well helpers (M14-I runway). Bounded polls everywhere —
// a force-wake or power-well sequence error can hang the GPU, so every wait
// has a timeout and reports instead of spinning forever.
// ---------------------------------------------------------------------------

const FORCEWAKE_SPIN_LIMIT: u64 = 5_000_000;
const POWERWELL_SPIN_LIMIT: u64 = 5_000_000;

/// Request the Haswell kernel force-wake and wait for the ACK. Returns false
/// (with a message) on timeout. No-op true if the ACK is already set.
/// Masked-write register: acquire = mask|KERNEL, no read-modify-write.
fn forcewake_acquire(mmio: &MmioReg) -> bool {
    if mmio.read32(FORCEWAKE_MT_ACK) & FORCEWAKE_KERNEL != 0 {
        return true;
    }
    mmio.write32(FORCEWAKE_MT, (FORCEWAKE_KERNEL << 16) | FORCEWAKE_KERNEL);
    for _ in 0..FORCEWAKE_SPIN_LIMIT {
        core::hint::spin_loop();
        if mmio.read32(FORCEWAKE_MT_ACK) & FORCEWAKE_KERNEL != 0 {
            return true;
        }
    }
    println!("modeset: FORCEWAKE_MT ack timeout (ack=0x{:08X})", mmio.read32(FORCEWAKE_MT_ACK));
    false
}

/// Drop the kernel hold (masked-disable) and wait for the ACK to clear.
/// Returns false (with a message) on timeout — a stale hold keeps the GT
/// awake and wastes power but does not corrupt display state.
fn forcewake_release(mmio: &MmioReg) -> bool {
    mmio.write32(FORCEWAKE_MT, FORCEWAKE_KERNEL << 16);
    for _ in 0..FORCEWAKE_SPIN_LIMIT {
        core::hint::spin_loop();
        if mmio.read32(FORCEWAKE_MT_ACK) & FORCEWAKE_KERNEL == 0 {
            return true;
        }
    }
    println!("modeset: FORCEWAKE_MT release timeout (ack=0x{:08X})", mmio.read32(FORCEWAKE_MT_ACK));
    false
}

/// Enable one power-well field if (and only if) its state bit is off — the
/// BIOS/GOP almost certainly has the eDP wells on already, and re-requesting
/// a live well is the risky direction. i915 encoding: request = mask<<1,
/// state = mask<<0 within the field. Returns true when the well is on.
/// Used by native-60 when the takeover lands (runway helper).
#[allow(dead_code)]
fn power_well_enable_if_off(mmio: &MmioReg, ctl: u64, field_mask: u32) -> bool {
    let val = mmio.read32(ctl);
    if val & field_mask != 0 {
        return true; // state bit already set — nothing to do
    }
    mmio.write32(ctl, val | (field_mask << 1));
    for _ in 0..POWERWELL_SPIN_LIMIT {
        core::hint::spin_loop();
        if mmio.read32(ctl) & field_mask != 0 {
            return true;
        }
    }
    println!("modeset: power-well [0x{:05X}] mask 0x{:08X} enable timeout", ctl, field_mask);
    false
}

/// `modeset wells` (op 8): read-only dump of the power/force-wake state plus
/// one force-wake acquire/release round-trip — a smoke test for the helpers
/// the takeover will depend on.
fn wells(mmio: &MmioReg) -> u64 {
    dump_reg(mmio, "PWR_WELL_CTL", PWR_WELL_CTL);
    dump_reg(mmio, "PWR_WELL_CTL2", PWR_WELL_CTL2);
    dump_reg(mmio, "FORCEWAKE_MT", FORCEWAKE_MT);
    dump_reg(mmio, "FORCEWAKE_MT_ACK", FORCEWAKE_MT_ACK);
    if !forcewake_acquire(mmio) {
        return u64::MAX;
    }
    println!("modeset wells: force-wake acquire OK (ack=0x{:08X})", mmio.read32(FORCEWAKE_MT_ACK));
    if !forcewake_release(mmio) {
        return u64::MAX;
    }
    println!("modeset wells: force-wake released (ack=0x{:08X})", mmio.read32(FORCEWAKE_MT_ACK));
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
