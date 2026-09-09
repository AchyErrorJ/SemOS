# Self-rebuild (M22 capstone) — design

Status: M22a implemented, QEMU-verified (DEMO 94/95/96 PASS). 2026-09-08.

Implementation notes beyond the original design:
- The drop zone lives in the LAST 128 MiB of the disk (LBA 786432+), not
  between the journal and the slots: placed mid-disk, a 119 MiB payload
  overran into slot A and the corruption surfaced only as a bootloader
  ELF-parse panic in the sabotage trial. Big raw regions stay disjoint.
- The build tag that binds a trial to its kernel is `SEMOS_BUILD_TAG`;
  build.rs now honors it as an explicit override (default stays the git
  build tag), with `cargo:rerun-if-env-changed` so slot builds don't
  silently reuse a cached tag (that silent reuse is how the first trial
  boot self-reverted as "stale").
- The SemFS journal log is now bounded (32768 sectors) so the slot and
  drop-zone regions are never journal traffic.
- Health gate (lightweight v1): journal mounted + namespace readable +
  fenced sem-sh spawn. The DEMO-80-style canned compile from §3 stays
  available as a heavier gate when trials get riskier.

The whole self-extension map points here: an OS that codes, modifies,
rebuilds, and reboots *itself* — safely. The key split from the roadmap:
live userland extension needs no reboot (DONE: M1–M4, headline demo,
semos-pkg); kernel self-replace needs a new image on disk + a reboot, made
to *feel* live by being fast and stateful (phone-OTA model), done **without
bricking the machine**.

## 0. Scope honesty

- **M22a (this doc, next demo): the machinery.** Slot state machine,
  hash-bound human approval, boot-once trial, health gate, auto-revert
  logic, format-versioned state. In QEMU the "boot into the candidate"
  step is performed by the harness choosing which UEFI image to boot —
  playing the role of the future loader.
- **M22b (follow-up): the chainloader.** A tiny SemOS-managed first stage
  on the ESP that reads the slot record and loads kernel A or B, making
  the boot switch physical and identical on QEMU and the T540p. Without
  it the switch needs firmware cooperation (BootNext/BootOrder NVRAM),
  which SemOS cannot write post-ExitBootServices.
- **Not claimed: on-device kernel compile.** semos-rustc builds no_std
  single-file guests; the kernel is a multi-crate tree with a custom
  target JSON. The v1 candidate image is built off-device and delivered
  as a blob (a semos-pkg-style package / raw region). The machinery is
  identical regardless of where the bytes come from.

## 1. Threat model — how we brick the machine

On the T540p there is no OS underneath SemOS. "Bricked" = the machine no
longer boots SemOS and needs external media to recover. (The firmware
itself is unreachable from our code — worst case is always "USB stick and
five minutes," never "new motherboard.")

| # | vector | consequence |
|---|---|---|
| 1 | overwriting the only bootable image with a broken one | dead boot; external reflash |
| 2 | power loss mid-flash → half-written image | dead boot; the journal can't help — the boot image lives outside SemFS |
| 3 | new kernel boots but can't parse state (journal/ABI drift) | boot → crash loop; data intact but unreachable |
| 4 | writing the wrong sectors (bad LBA math, wrong disk) | ESP/partition table/journal destroyed — brick + data loss (near-miss shape already fixed once in SYS_FLASH_SYSROOT) |
| 5 | no watchdog/rollback: a wedged kernel is never un-selected | machine sits dead until a human reflashes |
| 6 | agent reaches the flash path without a human in the loop | vector 1 with no gate |
| 7 | disk-full / OOM during image write | vector 2 without the power cut |

## 2. Slot layout and the slot record

Two image regions at **fixed, compile-time LBAs** (no write-target
arithmetic anywhere — vector 4):

```
LBA 8190/8191   slot record, copies A/B (SemFS superblock pattern:
                [magic "SRBL"][u32 format_version=1][u64 generation]
                [u8 state][u8 active_slot][u64 image_len]
                [u8;32 image_sha256][u32 header_crc32])
LBA 8192+       SemFS journal (unchanged)
image regions   inactive-slot bytes live OUTSIDE the journal, at fixed
                SLOT_A_BASE / SLOT_B_BASE constants, capacity-checked
                before the first sector is written (vector 7)
```

Why a raw, tiny, format-stable record and not a journaled namespace
object: the slot record is control-plane metadata that the *M22b
chainloader* must parse before any filesystem exists. Two copies, higher
generation wins — the exact pattern SemFS already proved. A kernel that
reads `format_version` it doesn't know treats the record as read-only and
NEVER "repairs" it (vector 3).

## 3. Promotion state machine

```
                 stage                 vouch                reboot
  EMPTY/IDLE ──────────► STAGED ───────────────► PENDING ───────► TRIAL
                            ▲  (sha256 bound        (generation+1,  (candidate
                            │   to the prompt)       state=TRIAL)    boots)
                            │                                       │
                 torn/bad CRC rejected before STAGED                ▼
                                                          health gate passes?
                                                          ┌────────┴────────┐
                                                          yes               no/timeout
                                                          ▼                 ▼
                                                       HEALTHY ◄──────── REVERT
                                                          │            (next boot finds
                                             human `rebuild keep`         TRIAL still set:
                                                          │                loader/harness
                                                          ▼                boots A again)
                                                       PROMOTED
```

- **stage**: stream the candidate into the INACTIVE region only (never
  the running slot — vector 1), capacity check first (vector 7), then
  sha256 + length into the slot record with a CRC'd header, generation+1.
  A torn stage is detected by CRC/hash and never leaves EMPTY (vector 2).
- **vouch**: the approval gate (demo_approval_prompt) extended P-3-style:
  the prompt PRINTS the candidate's sha256 prefix; the human's 'y' is
  bound to those bytes, not to a path or a claim (vector 6).
- **trial**: exactly one boot of the candidate with state=TRIAL. The
  candidate kernel reads the record at boot, learns it is on trial, and
  must run the health gate and then mark HEALTHY *from within the trial*.
- **health gate** (all must pass): SemFS journal replays (state readable),
  sem-sh spawns and runs a command, a canned selftest (DEMO 80-style
  compile loop) succeeds. Only then state=HEALTHY.
- **keep/revert**: HEALTHY still isn't promotion — the human keeps it
  (`rebuild keep`, journaled audit line) or it reverts on the NEXT boot
  boundary. If the trial kernel never sets HEALTHY (panic, wedge, crash
  loop — vector 5), the next boot sees a stale TRIAL and selects the
  other slot. In M22a the harness performs that selection; M22b's
  chainloader performs it physically.
- **auto-revert is the default direction**: every failure path —
  timeout, panic, human denial, torn state — points back at the
  last-known-good slot. Promotion is the only transition that requires a
  human.

## 4. Command surface

`rebuild` sem-sh builtin (new syscall, console-gated like SYS_SEMOSPKG
mutations; `rebuild status` read-only):

- `rebuild stage <path>` — image at <path> (delivered via semos-pkg or
  written into the namespace) → inactive slot + STAGED.
- `rebuild status` — slot record dump: state, generation, hash, which
  slot is running, which is trial.
- `rebuild boot-next` — the vouch step: hash-bound approval → PENDING.
  The following reboot trials the candidate.
- `rebuild keep` — from inside a HEALTHY trial: PROMOTED.
- `rebuild revert` — human-triggered revert (any time before keep).
- Health gate + trial detection run automatically at boot when
  state=TRIAL; no command needed.

## 5. Failure-mode matrix

| failure | detection | outcome |
|---|---|---|
| torn stage write (power loss) | slot-record CRC / image sha256 mismatch | stays EMPTY; re-stage |
| candidate panics at boot | TRIAL never → HEALTHY | next boot reverts (vector 5 closed) |
| candidate boots, health gate fails | gate verdict | auto-revert, audit line |
| candidate can't parse journal (format drift) | replay refuses unknown FORMAT_VERSION (read-only, never format) | health gate fails → revert (vector 3) |
| stage would exceed slot capacity | pre-write capacity check | refuse before sector 1 (vector 7) |
| write-target corruption | no arithmetic: fixed LBA constants only (vector 4) | — |
| agent calls rebuild from non-console | dispatcher is_vouch_authority gate | DENIED + log (vector 6) |
| human denies at vouch | fail-fast gate | stays STAGED, nothing booted |
| power loss during TRIAL | generation/state survive (write-through) | next boot sees TRIAL, reverts unless HEALTHY was already set |

## 6. QEMU test plan (M22a)

Feature `rebuild-test`; harness `run-rebuild-qemu.sh`. Two UEFI images of
the same kernel tree: A = normal build, B = a build with a visible marker
(different boot banner string → provably different bytes and behavior).

- **DEMO 94 (stage → trial → keep)**: boot A; harness delivers B as a
  blob; feeder runs `rebuild stage`, `rebuild boot-next` (serial 'y',
  hash shown in the log); harness reboots INTO B (playing the M22b
  loader); B's trial boot runs the health gate, sets HEALTHY; feeder runs
  `rebuild keep` → `[DEMO 94] PASS`.
- **DEMO 95 (boot-loop candidate auto-reverts)**: same flow but B's
  health gate is deliberately sabotaged (build-flag) — trial never goes
  HEALTHY; harness reboots into A per the record and A logs
  `[DEMO 95] PASS: stale TRIAL reverted to last-known-good`.
- **DEMO 96 (torn candidate rejected)**: host truncates the staged
  region between stage and vouch; `rebuild boot-next` refuses on hash
  mismatch → `[DEMO 96] PASS`.

## 7. What M22a deliberately does NOT build

- The chainloader (M22b) — the boot switch is harness-performed in QEMU.
- On-device kernel compile (see §0).
- NVRAM writes / BootNext (firmware-owned; chainloader supersedes).
- Signed images — the hash-bound human vouch is the control; signatures
  would harden the no-human path, which this design deliberately keeps
  impossible (auto-revert needs no trust; promotion always does).
- Rollback *history* (one previous slot only, like phone A/B).
