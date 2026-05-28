//! Intel HD Audio (M15) — minimum-viable controller + codec + PCM output.
//!
//! Brings up an Intel HDA controller on the PCI bus (class 0x04 / subclass
//! 0x03 / prog-if 0x00), sets up CORB/RIRB so codec verbs can flow, walks
//! the first codec to find a DAC + output pin, programs an output stream
//! (48 kHz, 16-bit stereo), and plays a sine tone from a cyclic 4 KiB PCM
//! buffer for ~1 second. QEMU's `-device intel-hda -device hda-output` is
//! enough to test the whole path in-tree.
//!
//! Scope v1: one codec, one DAC + one output pin (chosen by widget type),
//! one stream (the first output stream descriptor at MMIO offset
//! `0x80 + 0x20*ISS`), one BDL entry pointing at a page-aligned PCM buffer.
//! Polled (no MSI-X). Validation is "the stream advanced LPIB while RUN was
//! set" — proves DMA happened, which is all we can check without hearing it.
//!
//! Verb encoding (the 20-bit "Command" field in a CORB DWORD):
//!   - 4-bit verbs (top nibble) + 16-bit payload: 0x2 = Set Converter Format,
//!     0x3 = Set Amp Gain/Mute.
//!   - 12-bit verbs + 8-bit payload: 0xF00 = Get Parameter, 0x705 = Set Power,
//!     0x706 = Set Converter Stream/Channel, 0x707 = Set Pin Widget Control,
//!     0x70C = Set EAPD/BTL Enable.

use core::ptr::{read_volatile, write_volatile};
use crate::pci;
use crate::paging;
use crate::println;

// --- MMIO register offsets (controller-global) ---
mod reg {
    pub const GCAP:     u64 = 0x00;
    pub const GCTL:     u64 = 0x08;
    pub const STATESTS: u64 = 0x0E;
    pub const CORBLBASE: u64 = 0x40;
    pub const CORBUBASE: u64 = 0x44;
    pub const CORBWP:    u64 = 0x48;
    pub const CORBRP:    u64 = 0x4A;
    pub const CORBCTL:   u64 = 0x4C;
    pub const CORBSIZE:  u64 = 0x4E;
    pub const RIRBLBASE: u64 = 0x50;
    pub const RIRBUBASE: u64 = 0x54;
    pub const RIRBWP:    u64 = 0x58;
    pub const RINTCNT:   u64 = 0x5A;
    pub const RIRBCTL:   u64 = 0x5C;
    pub const RIRBSIZE:  u64 = 0x5E;
}

// Per-stream descriptor (starts at 0x80, 0x20 bytes apart).
mod sd {
    pub const CTL:     u64 = 0x00; // 3 bytes, RUN bit 1
    pub const STS:     u64 = 0x03;
    pub const LPIB:    u64 = 0x04;
    pub const CBL:     u64 = 0x08;
    pub const LVI:     u64 = 0x0C;
    pub const FMT:     u64 = 0x12;
    pub const BDPL:    u64 = 0x18;
    pub const BDPU:    u64 = 0x1C;
}

// Codec parameter IDs (passed as payload to Get Parameter, verb 0xF00).
const PARAM_NODE_COUNT:    u8 = 0x04;
const PARAM_FN_GROUP_TYPE: u8 = 0x05;
const PARAM_WIDGET_CAPS:   u8 = 0x09;

const FNGRP_AFG: u32 = 1; // Audio Function Group

const WIDGET_DAC: u32 = 0; // Audio Output (DAC)
const WIDGET_PIN: u32 = 4; // Pin Complex

const QSIZE: usize = 256; // CORB/RIRB entries

// PCM buffer: one page → ~21 ms at 48 kHz stereo 16-bit; replayed cyclically.
const PCM_BYTES: usize = 4096;
const SAMPLE_RATE: u32 = 48_000;

// --- DMA-visible static storage (page-aligned BSS = contiguous phys pages) ---
#[repr(C, align(128))]
struct Corb([u32; QSIZE]); // 1 KiB, must be 128-byte aligned
#[repr(C, align(128))]
struct Rirb([u64; QSIZE]); // 2 KiB
#[repr(C, align(128))]
struct Bdl([u64; 8]); // 2 entries used (addr_lo+hi, len+ioc) per BDL slot — 16B each
#[repr(C, align(4096))]
struct PcmBuf([u8; PCM_BYTES]);

static mut CORB: Corb = Corb([0; QSIZE]);
static mut RIRB: Rirb = Rirb([0; QSIZE]);
static mut BDL: Bdl = Bdl([0; 8]);
static mut PCM: PcmBuf = PcmBuf([0; PCM_BYTES]);

// --- driver state ---
static mut MMIO: u64 = 0;
static mut CORB_WP: u16 = 0; // next slot to write a verb into
static mut RIRB_RP: u16 = 0; // next slot to read a response from
static mut ISS: u8 = 0; // input streams (used to find first output stream)
static mut OUT_SD_OFF: u64 = 0; // MMIO offset of the chosen output stream descriptor

// Chosen codec + widget IDs (filled by walk_codec).
static mut CODEC_ADDR: u8 = 0xFF;
static mut DAC_NID: u8 = 0;
static mut PIN_NID: u8 = 0;

#[inline] unsafe fn r8(o: u64) -> u8  { read_volatile((MMIO + o) as *const u8) }
#[inline] unsafe fn r16(o: u64) -> u16 { read_volatile((MMIO + o) as *const u16) }
#[inline] unsafe fn r32(o: u64) -> u32 { read_volatile((MMIO + o) as *const u32) }
#[inline] unsafe fn w8(o: u64, v: u8)  { write_volatile((MMIO + o) as *mut u8, v) }
#[inline] unsafe fn w16(o: u64, v: u16) { write_volatile((MMIO + o) as *mut u16, v) }
#[inline] unsafe fn w32(o: u64, v: u32) { write_volatile((MMIO + o) as *mut u32, v) }

fn make_verb_4(cad: u8, nid: u8, verb4: u32, payload16: u16) -> u32 {
    ((cad as u32) << 28) | ((nid as u32) << 20) | (verb4 << 16) | (payload16 as u32)
}
fn make_verb_12(cad: u8, nid: u8, verb12: u32, payload8: u8) -> u32 {
    ((cad as u32) << 28) | ((nid as u32) << 20) | (verb12 << 8) | (payload8 as u32)
}

/// Send a verb via the Immediate Command Interface (ICI) — direct registers
/// at 0x60 (ICO write), 0x64 (IRI read), 0x68 (IRS status). Simpler than the
/// CORB/RIRB DMA path and well-supported by QEMU. CORB/RIRB are still set up
/// in `init` (some real codecs prefer them) but we use ICI for v1.
///
/// Sequence: clear IRV (write bit 1 of IRS), write verb to ICO, set ICB (bit 0
/// of IRS) to kick, poll IRS until IRV becomes 1 (or ICB drops), read IRI.
unsafe fn verb(packet: u32) -> u32 {
    const ICO: u64 = 0x60;
    const IRI: u64 = 0x64;
    const IRS: u64 = 0x68;
    const ICB: u16 = 1 << 0;
    const IRV: u16 = 1 << 1;

    // Wait for any prior command to drain.
    let mut spins: u64 = 0;
    while r16(IRS) & ICB != 0 {
        spins += 1;
        if spins > 200_000_000 {
            println!("[hda] ICI: stuck ICB before verb 0x{:08X}", packet);
            return 0xFFFF_FFFF;
        }
        core::hint::spin_loop();
    }
    // Clear IRV (write 1 to clear), write verb, kick by setting ICB.
    w16(IRS, IRV);
    w32(ICO, packet);
    w16(IRS, ICB);
    // Poll until IRV is set (response ready).
    spins = 0;
    loop {
        let s = r16(IRS);
        if s & IRV != 0 {
            return r32(IRI);
        }
        spins += 1;
        if spins > 200_000_000 {
            println!("[hda] ICI timeout (verb 0x{:08X}, IRS=0x{:04X})", packet, s);
            return 0xFFFF_FFFF;
        }
        core::hint::spin_loop();
    }
}

unsafe fn get_param(nid: u8, param: u8) -> u32 {
    verb(make_verb_12(CODEC_ADDR, nid, 0xF00, param))
}

/// Walk the first responding codec, find an AFG, then find the first DAC +
/// the first output pin under it. Sets CODEC_ADDR / DAC_NID / PIN_NID.
unsafe fn walk_codec() -> bool {
    // STATESTS bit set per responding codec address (SDIN line). Pick the lowest.
    let stss = r16(reg::STATESTS);
    if stss == 0 {
        println!("[hda] no codec responded on STATESTS");
        return false;
    }
    CODEC_ADDR = stss.trailing_zeros() as u8;

    // Root node (NID 0) → subordinate node count.
    let root = get_param(0, PARAM_NODE_COUNT);
    let root_start = ((root >> 16) & 0xFF) as u8;
    let root_count = (root & 0xFF) as u8;
    if root_count == 0 {
        println!("[hda] codec {} has no subnodes", CODEC_ADDR);
        return false;
    }

    // Find the first AFG among the root subnodes.
    let mut afg_nid: u8 = 0;
    for i in 0..root_count {
        let nid = root_start + i;
        let fgt = get_param(nid, PARAM_FN_GROUP_TYPE) & 0x7F;
        if fgt == FNGRP_AFG {
            afg_nid = nid;
            break;
        }
    }
    if afg_nid == 0 {
        println!("[hda] codec {} has no AFG", CODEC_ADDR);
        return false;
    }

    // Bring the AFG out of reset to D0 (some emulated codecs are already on,
    // but this matches the standard sequence and is harmless if already D0).
    verb(make_verb_12(CODEC_ADDR, afg_nid, 0x705, 0));

    // AFG subnodes → find the first DAC + the first pin complex.
    let afg = get_param(afg_nid, PARAM_NODE_COUNT);
    let afg_start = ((afg >> 16) & 0xFF) as u8;
    let afg_count = (afg & 0xFF) as u8;

    for i in 0..afg_count {
        let nid = afg_start + i;
        let caps = get_param(nid, PARAM_WIDGET_CAPS);
        let wtype = (caps >> 20) & 0xF;
        if wtype == WIDGET_DAC && DAC_NID == 0 {
            DAC_NID = nid;
        } else if wtype == WIDGET_PIN && PIN_NID == 0 {
            PIN_NID = nid;
        }
        if DAC_NID != 0 && PIN_NID != 0 {
            break;
        }
    }
    if DAC_NID == 0 || PIN_NID == 0 {
        println!(
            "[hda] codec {} AFG {}: missing DAC or Pin (DAC={}, Pin={})",
            CODEC_ADDR, afg_nid, DAC_NID, PIN_NID,
        );
        return false;
    }
    println!(
        "[hda] codec {} AFG {} → DAC {} + Pin {}",
        CODEC_ADDR, afg_nid, DAC_NID, PIN_NID,
    );
    true
}

/// 48 kHz, 16-bit, stereo PCM format word (the value programmed into the
/// stream's FMT register AND into the DAC's converter-format verb).
const FMT_48K_S16_STEREO: u16 = (1 << 4) | (2 - 1);

/// Fill the PCM buffer with a 440 Hz-ish sine that loops "close enough" over
/// the buffer length (PCM_BYTES / 4 = 1024 frames at 48 kHz = 21.33 ms cycle).
/// Headless testing can't actually hear it; this is for fidelity on metal.
unsafe fn fill_sine() {
    let frames = PCM_BYTES / 4; // 16-bit stereo = 4 B/frame
    // Use a small lookup-free integer sine via a Taylor-style approximation
    // around a 16-step LUT, sufficient for a tone.
    static SINE16: [i16; 16] = [
        0, 12539, 23170, 30273, 32767, 30273, 23170, 12539,
        0, -12539, -23170, -30273, -32767, -30273, -23170, -12539,
    ];
    for f in 0..frames {
        // 440 Hz at 48 kHz: ~109.09 samples / cycle. Index into a 16-step
        // sine = (f * 16 * 440 / 48000) wrap 16. Cheap integer math.
        let idx = ((f as u64 * 16 * 440) / SAMPLE_RATE as u64) as usize % 16;
        let s = SINE16[idx];
        let off = f * 4;
        PCM.0[off..off + 2].copy_from_slice(&s.to_le_bytes()); // left
        PCM.0[off + 2..off + 4].copy_from_slice(&s.to_le_bytes()); // right
    }
}

/// Configure the chosen output stream descriptor: program format, CBL, LVI,
/// BDL base, then set RUN. Returns the stream descriptor's MMIO offset.
unsafe fn start_stream(stream_tag: u8) -> u64 {
    // First output stream descriptor: SD index = ISS (input streams come first).
    let sd_off = 0x80 + 0x20 * (ISS as u64);
    OUT_SD_OFF = sd_off;

    // Clear any pending status bits + ensure RUN=0.
    w8(sd_off + sd::CTL, 0);
    w8(sd_off + sd::STS, 0x1C); // clear BCIS|FE|DESE (write-1-to-clear)

    // One BDL entry: addr = PCM phys, length = PCM_BYTES, IOC = 0.
    let pcm_phys = paging::walk_active_pml4((&raw const PCM) as u64).unwrap_or(0);
    BDL.0[0] = pcm_phys;
    BDL.0[1] = ((PCM_BYTES as u64) & 0xFFFF_FFFF) | (0 << 32); // length lo / ioc hi
    let bdl_phys = paging::walk_active_pml4((&raw const BDL) as u64).unwrap_or(0);

    w32(sd_off + sd::CBL, PCM_BYTES as u32);
    w16(sd_off + sd::LVI, 0); // last valid index = 0 (single entry)
    w16(sd_off + sd::FMT, FMT_48K_S16_STEREO);
    w32(sd_off + sd::BDPL, bdl_phys as u32);
    w32(sd_off + sd::BDPU, (bdl_phys >> 32) as u32);

    // Stream tag in CTL bits 23:20; RUN in bit 1.
    let ctl: u32 = ((stream_tag as u32 & 0xF) << 20) | (1 << 1);
    w32(sd_off + sd::CTL, ctl);
    sd_off
}

pub fn init() -> bool {
    let loc = match pci::find_by_class(0x04, 0x03, 0x00) {
        Some(l) => l,
        None => {
            println!("[hda] no Intel HDA controller on PCI bus 0");
            return false;
        }
    };
    loc.enable_io_and_bus_master();
    let phys_base = match pci::mmio_bar64(loc) {
        Some(b) => b,
        None => {
            println!("[hda] BAR0 is I/O space; HDA must be MMIO — abort");
            return false;
        }
    };
    unsafe { MMIO = paging::phys_to_virt(phys_base); }

    unsafe {
        let gcap = r16(reg::GCAP);
        let oss = ((gcap >> 12) & 0xF) as u8;
        let iss = ((gcap >> 8) & 0xF) as u8;
        ISS = iss;
        println!("[hda] PCI 00:{:02X}.0  MMIO=0x{:016X}  GCAP=0x{:04X}  ISS={} OSS={}",
            loc.slot, phys_base, gcap, iss, oss);
        if oss == 0 {
            println!("[hda] no output streams — abort");
            return false;
        }

        // Controller reset: drop GCTL.CRST, then assert, wait until it reads back.
        w32(reg::GCTL, 0);
        let mut spins = 0u64;
        while r32(reg::GCTL) & 1 != 0 {
            spins += 1;
            if spins > 200_000_000 { println!("[hda] reset-low timeout"); return false; }
            core::hint::spin_loop();
        }
        w32(reg::GCTL, 1);
        spins = 0;
        while r32(reg::GCTL) & 1 == 0 {
            spins += 1;
            if spins > 200_000_000 { println!("[hda] reset-high timeout"); return false; }
            core::hint::spin_loop();
        }
        // STATESTS is populated ~512 us after reset — short busy wait.
        for _ in 0..1_000_000 { core::hint::spin_loop(); }

        // Set up CORB: stop DMA, point at our ring, set size = 256, reset RP.
        w8(reg::CORBCTL, 0);
        // Wait CORB DMA to actually stop.
        spins = 0;
        while r8(reg::CORBCTL) & 0x02 != 0 {
            spins += 1; if spins > 200_000_000 { return false; } core::hint::spin_loop();
        }
        let corb_phys = paging::walk_active_pml4((&raw const CORB) as u64).unwrap_or(0);
        if corb_phys == 0 { println!("[hda] CORB phys translate failed"); return false; }
        w32(reg::CORBLBASE, corb_phys as u32);
        w32(reg::CORBUBASE, (corb_phys >> 32) as u32);
        // CORBSIZE: 0x02 = 256 entries.
        w8(reg::CORBSIZE, 0x02);
        // Reset CORB RP: write bit 15 (CORBRPRST), wait for it to read back, then clear it.
        w16(reg::CORBRP, 1 << 15);
        spins = 0;
        while r16(reg::CORBRP) & (1 << 15) == 0 {
            spins += 1; if spins > 200_000_000 { println!("[hda] CORB RP reset stuck"); break; } core::hint::spin_loop();
        }
        w16(reg::CORBRP, 0);
        // CORB WP = 0 (and our shadow).
        CORB_WP = 0;
        w16(reg::CORBWP, 0);
        // Run CORB DMA.
        w8(reg::CORBCTL, 0x02);

        // RIRB: same dance. Size = 256 entries.
        w8(reg::RIRBCTL, 0);
        let rirb_phys = paging::walk_active_pml4((&raw const RIRB) as u64).unwrap_or(0);
        if rirb_phys == 0 { println!("[hda] RIRB phys translate failed"); return false; }
        w32(reg::RIRBLBASE, rirb_phys as u32);
        w32(reg::RIRBUBASE, (rirb_phys >> 32) as u32);
        w8(reg::RIRBSIZE, 0x02);
        // Reset RIRB WP: write bit 15.
        w16(reg::RIRBWP, 1 << 15);
        RIRB_RP = 0;
        // Set RIRB response interrupt count (don't care for polling, but spec
        // wants non-zero; pick 1 so the controller writes promptly).
        w16(reg::RINTCNT, 1);
        // Run RIRB DMA.
        w8(reg::RIRBCTL, 0x02);

        if !walk_codec() {
            return false;
        }

        // Configure the DAC for our format + stream tag 1, channel 0.
        let stream_tag: u8 = 1;
        verb(make_verb_4(CODEC_ADDR, DAC_NID, 0x2, FMT_48K_S16_STEREO));
        verb(make_verb_12(CODEC_ADDR, DAC_NID, 0x706, (stream_tag << 4) | 0));
        // Unmute output amp (output side, both channels, gain ~ max).
        verb(make_verb_4(CODEC_ADDR, DAC_NID, 0x3, 0xB07F));

        // Pin: power up, enable output drive, also enable EAPD (real codecs).
        verb(make_verb_12(CODEC_ADDR, PIN_NID, 0x705, 0));
        verb(make_verb_12(CODEC_ADDR, PIN_NID, 0x707, 0x40)); // OUT_EN
        verb(make_verb_12(CODEC_ADDR, PIN_NID, 0x70C, 0x02)); // EAPD

        // Fill the PCM page with a sine, then arm + start the stream.
        fill_sine();
        let sd_off = start_stream(stream_tag);
        println!("[hda] stream tag {} armed at SD offset 0x{:X}, FMT=0x{:04X}, CBL={} B",
            stream_tag, sd_off, FMT_48K_S16_STEREO, PCM_BYTES);
    }
    true
}

/// Sample LPIB (link position in buffer) for the active output stream. Returns
/// 0 if not initialized. Used by the DEMO to confirm DMA is advancing.
pub fn lpib() -> u32 {
    unsafe {
        if OUT_SD_OFF == 0 || MMIO == 0 {
            return 0;
        }
        r32(OUT_SD_OFF + sd::LPIB)
    }
}

/// Stop the stream (clear RUN). Safe to call multiple times.
pub fn stop() {
    unsafe {
        if OUT_SD_OFF == 0 || MMIO == 0 {
            return;
        }
        w32(OUT_SD_OFF + sd::CTL, 0);
    }
}
