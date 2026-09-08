//! SYS_LOGFILE: append the kernel log ring to LOG.TXT on the SEMOS_LOG
//! FAT32 partition, so Pop!_OS (or any OS) can read the kernel's last words
//! after a reboot — no network, no serial cable, no framebuffer photo.
//! Console-only: the dispatcher gates this on the vouch authority (the
//! interactive sem-sh), so the agent and arbitrary Ring-3 tasks can't reach
//! it. Shell side: the `log flush` builtin.
//!
//! LOG.TXT is **preallocated at a fixed size** by tools/setup-log-partition.sh
//! and only ever overwritten in place — SemOS never allocates clusters and
//! never edits FAT or directory entries, so a bug here can corrupt the log
//! file but not the filesystem.
//!
//! LOG.TXT layout:
//! ```text
//!   [0..512)   header: "SEMOSLOG1\nnext=<u64 decimal>\n", zero-padded
//!   [512..)    JSON-Lines records, one per kernel log line:
//!              {"n":<ring byte index>,"line":"<escaped>"}
//! ```
//! `next` is the append pointer (absolute file offset of the next record).
//! It lives in the file itself so appends survive reboots; a freshly
//! preallocated (all-zero) file starts appending at 512.
//!
//! Records are JSON-escaped per the syscall ABI doc: `"`, `\`, and every
//! byte outside 0x20..=0x7E become `\u00XX`, so no payload byte can break
//! out of the quoted string when the file is later handed to an LLM as
//! untrusted data. `n` is the record's absolute ring byte index — monotonic
//! within a boot — so the reader can dedupe overlapping flushes and spot
//! the gap records emitted when the 64 KiB ring wrapped between flushes.

use core::fmt::Write as _;
use core::ptr::read_volatile;
use core::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use kernel_core::drivers::registry;
use kernel_core::fs::fat::Fat32;

const RING_BYTES: u64 = 64 * 1024;
const HEADER_BYTES: u64 = 512;
const MAGIC: &[u8] = b"SEMOSLOG1\nnext=";
/// One JSONL line buffer; longer kernel lines are split into continuation
/// records rather than dropped.
const LINE_MAX: usize = 1024;

/// Ring head at the previous flush. Statics reset each boot, so the first
/// flush of a boot captures the whole current ring — exactly what you want
/// when pulling boot logs off the disk from another OS.
static LAST_HEAD: AtomicU64 = AtomicU64::new(0);

pub fn run() -> u64 {
    for name in ["sata0", "nvme0", "virtio0"] {
        let Some(dev) = registry::get_block(name) else {
            continue;
        };
        if dev.block_size() != 512 {
            continue;
        }
        let Some(mut fat) = Fat32::mount_labeled(dev, b"SEMOS_LOG  ") else {
            continue;
        };
        let Some((cluster, size)) = fat.find_file(b"LOG     TXT", "log.txt") else {
            continue;
        };
        return flush_to(&mut fat, name, cluster, size);
    }
    crate::println!("log-flush: no SEMOS_LOG partition with LOG.TXT found (run tools/setup-log-partition.sh)");
    u64::MAX
}

fn flush_to(fat: &mut Fat32, devname: &str, cluster: u32, size: u32) -> u64 {
    if (size as u64) < HEADER_BYTES + RING_BYTES {
        crate::println!("log-flush: {}: LOG.TXT is only {} bytes — recreate it via setup-log-partition.sh", devname, size);
        return u64::MAX;
    }

    // Append pointer from the header sector (absent/invalid → fresh file).
    let mut append_off = HEADER_BYTES;
    let mut hdr = [0u8; HEADER_BYTES as usize];
    let mut got = false;
    let _ = fat.read_file(cluster, HEADER_BYTES as u32, |off, chunk| {
        if off == 0 {
            let n = chunk.len().min(hdr.len());
            hdr[..n].copy_from_slice(&chunk[..n]);
            got = true;
        }
        false // first chunk is enough
    });
    if got && hdr.starts_with(MAGIC) {
        let mut v = 0u64;
        for &b in &hdr[MAGIC.len()..] {
            if b.is_ascii_digit() {
                v = v * 10 + (b - b'0') as u64;
            } else {
                break;
            }
        }
        if v >= HEADER_BYTES && (v as u64) < size as u64 {
            append_off = v;
        }
    }

    // Delta window: everything since the last flush that is still in the ring.
    let head = crate::framebuffer::scrollback_head();
    let recorded = head.min(RING_BYTES);
    let oldest = head - recorded;
    let last = LAST_HEAD.load(AtomicOrdering::Acquire);
    let from = last.max(oldest).min(head);
    if from == head {
        crate::println!("log-flush: no new records since last flush");
        return 0;
    }
    // Worst case is ~26 output bytes per ring byte (1-byte lines), plus the
    // header rewrite and a gap record. Refuse early if the file can't fit it.
    let delta = head - from;
    let worst = delta * 26 + 1024;
    if append_off + worst > size as u64 {
        crate::println!(
            "log-flush: {}: LOG.TXT full (next={}, need up to {} more of {})",
            devname, append_off, worst, size
        );
        return u64::MAX;
    }

    let mut w = JsonlWriter {
        fat,
        cluster,
        off: append_off,
        buf: [0u8; 4096],
        len: 0,
        total: 0,
    };

    if last != 0 && last < oldest {
        w.record(oldest, b"--- gap: ring wrapped between flushes, lines lost ---");
    }
    w.record_marker(head);

    // Stream ring bytes [from, head), one JSONL record per '\n'-terminated line.
    let buf_ptr = crate::framebuffer::scrollback_buf_ptr();
    let mut line = [0u8; LINE_MAX];
    let mut line_len = 0usize;
    let mut line_start = from;
    let mut i = from;
    while i < head {
        let b = unsafe { read_volatile(buf_ptr.add((i & (RING_BYTES - 1)) as usize)) };
        if b == b'\n' || line_len == LINE_MAX {
            w.record(line_start, &line[..line_len]);
            line_len = 0;
            line_start = i + 1;
        } else {
            line[line_len] = b;
            line_len += 1;
        }
        i += 1;
    }
    if line_len > 0 {
        w.record(line_start, &line[..line_len]); // trailing partial line
    }

    if !w.finish() {
        crate::println!("log-flush: {}: write failed at file offset {}", devname, w.off);
        return u64::MAX;
    }

    // Commit the append pointer LAST, so an interrupted flush leaves the
    // file readable up to the previous record boundary.
    let mut hdr2 = [0u8; HEADER_BYTES as usize];
    {
        let mut bw = crate::panic_dump::BufWriter { buf: &mut hdr2, n: 0 };
        let _ = write!(bw, "SEMOSLOG1\nnext={}\n", w.off);
    }
    if !w.fat.write_at(cluster, 0, &hdr2) {
        crate::println!("log-flush: {}: header rewrite failed", devname);
        return u64::MAX;
    }

    LAST_HEAD.store(head, AtomicOrdering::Release);
    crate::println!(
        "log-flush: {} bytes appended to {}:LOG.TXT (next={})",
        w.total, devname, w.off
    );
    w.total
}

/// Streaming JSONL record writer over `Fat32::write_at`. Flushes 4 KiB
/// chunks as they fill; records may span chunk boundaries (the file is an
/// append-only byte stream — only the header commit makes them visible).
struct JsonlWriter<'a, 'b> {
    fat: &'b mut Fat32<'a>,
    cluster: u32,
    off: u64,
    buf: [u8; 4096],
    len: usize,
    total: u64,
}

impl JsonlWriter<'_, '_> {
    fn push(&mut self, b: u8) {
        if self.len == self.buf.len() && !self.flush_buf() {
            return; // sticky failure surfaces at finish()/header commit
        }
        self.buf[self.len] = b;
        self.len += 1;
    }

    fn push_str(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.push(b);
        }
    }

    fn push_escaped(&mut self, b: u8) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        match b {
            b'"' => self.push_str("\\\""),
            b'\\' => self.push_str("\\\\"),
            0x20..=0x7E => self.push(b),
            _ => {
                self.push_str("\\u00");
                self.push(HEX[(b >> 4) as usize]);
                self.push(HEX[(b & 15) as usize]);
            }
        }
    }

    /// Emit one record: {"n":<n>,"line":"<escaped line>"}\n
    fn record(&mut self, n: u64, line: &[u8]) {
        let mut prefix = [0u8; 32];
        let plen = {
            let mut bw = crate::panic_dump::BufWriter { buf: &mut prefix, n: 0 };
            let _ = write!(bw, "{{\"n\":{},\"line\":\"", n);
            bw.n
        };
        self.push_str(core::str::from_utf8(&prefix[..plen]).unwrap_or("{\"n\":0,\"line\":\""));
        for &b in line {
            self.push_escaped(b);
        }
        self.push_str("\"}\n");
    }

    /// Boot/flush separator so the appended file is navigable.
    fn record_marker(&mut self, n: u64) {
        let mut msg = [0u8; 64];
        let mlen = {
            let mut bw = crate::panic_dump::BufWriter { buf: &mut msg, n: 0 };
            let _ = write!(bw, "--- log flush @ tick {} ---", kernel_core::platform::ticks());
            bw.n
        };
        self.record(n, &msg[..mlen]);
    }

    fn flush_buf(&mut self) -> bool {
        if self.len == 0 {
            return true;
        }
        let ok = self.fat.write_at(self.cluster, self.off, &self.buf[..self.len]);
        if ok {
            self.off += self.len as u64;
            self.total += self.len as u64;
            self.len = 0;
        }
        ok
    }

    fn finish(&mut self) -> bool {
        self.flush_buf()
    }
}
