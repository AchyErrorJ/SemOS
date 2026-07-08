# M14 — Intel HD 4600 integrated graphics plan (T540p)

Status: planned  
Target machine: ThinkPad T540p/W540-class host running Pop!_OS + SemOS dual boot  
Target GPU: Intel HD 4600 / Haswell GT2, PCI `00:02.0`, vendor/device `8086:0416`

This milestone is about making the machine pleasant to use on the built-in
panel. It is **not** a full Linux `i915`/DRM/Mesa clone. SemOS already has a
working UEFI GOP linear framebuffer plus kernel-side 2D drawing, TTF text, TUI,
editor, Pong/Tetris, and scrollback. M14 extends that into controlled native
Intel display ownership in small, reversible steps.

---

## Current baseline

Already done:

- UEFI boot on the T540p works.
- Bootloader-provided framebuffer is initialized early from `BootInfo`.
- M14-A oracle capture committed under `docs/hardware/igpu-2026-07-08/`
  (HD 4600 `8086:0416` @ `00:02.0`, BAR0 4M, BAR2 256M, eDP-1 native
  `1920x1080@60.007Hz` CMN `N156HGE-EA1`, `intel_backlight` raw max `4438`).
- M14-B read-only PCI probe (`kernel-x86_64/src/igpu.rs`). BAR sizes come from
  the oracle capture rather than the live PCI BAR-sizing write dance, preserving
  the "read-only first" safety rule.
- M14-C framebuffer diagnostics: `fbinfo` shell builtin (`SYS_FBINFO`) prints
  the GOP geometry/format and compares it to the native panel mode.
- M14-D backlight control: `brightness [N|up|down|restore]` shell builtin
  (`SYS_BACKLIGHT`) over the PCH PWM path in `kernel-x86_64/src/backlight.rs`,
  clamped to a 10% visible floor with save/restore.
- M14-E app framebuffer surface: `SYS_FB_META` + `SYS_FB_BLIT` let Ring-3 code
  query GOP framebuffer metadata and present a user-owned RGB buffer; `fb-demo`
  draws a 320x180 gradient through this syscall path.
- `kernel-x86_64/src/framebuffer.rs` exposes:
  - `fb_fill_rect`
  - `fb_blit`
  - `fb_scroll`
  - `fb_scroll_region`
  - `fb_present`
  - readback helpers for headless verification.
- `kernel-x86_64/src/font.rs` and `kernel-x86_64/src/gfx2d.rs` provide TTF and
  anti-aliased 2D drawing over that framebuffer.
- Hardware inventory confirms:
  - iGPU: Intel 4th Gen Core Processor Integrated Graphics Controller
    `8086:0416`
  - bus location: `00:02.0`
  - Linux oracle driver: `i915`

Not done yet:

- No native Intel iGPU driver module.
- No iGPU BAR/MMIO diagnostics.
- No backlight/brightness control.
- No SemOS-side EDID/mode inventory.
- No native Haswell modeset.
- Direct shared framebuffer mapping remains deferred; M14-E uses a safer blit
  syscall first.

---

## Milestone definition

M14 is complete when SemOS on the T540p can:

1. Boot using the existing GOP framebuffer.
2. Identify the Intel HD 4600 reliably through PCI.
3. Report the current display/framebuffer state clearly on screen and in logs.
4. Control internal-panel brightness safely.
5. Provide a cleaner app-facing framebuffer path so SemOS apps can draw without
   every UI staying kernel-owned.
6. Preserve a safe fallback: if native iGPU probing fails, the old GOP
   framebuffer console still works.

Stretch goal:

- Switch to a preferred panel mode or native resolution. Full native modesetting
  is intentionally a later sub-milestone unless GOP already gives us the right
  mode or the boot path can select it cheaply.

---

## Non-goals for this milestone

- No OpenGL/Vulkan/Mesa.
- No command submission / render rings.
- No 3D acceleration.
- No video decode / QuickSync.
- No NVIDIA dGPU changes.
- No deep ACPI power-management framework unless brightness requires a tiny
  targeted ACPI/OpRegion bridge.

---

## Safety rules

1. **Read-only first.** The first SemOS iGPU module only reads PCI config and
   safe registers.
2. **No display writes until the fallback is proven.** Any MMIO write path must
   be feature-gated and callable from a deliberate shell command or demo.
3. **Never set brightness to zero.** Clamp the minimum brightness to a visible
   floor, e.g. 10%.
4. **Save and restore original brightness.** The first backlight demo should
   read current value, step down/up, then restore.
5. **Touch only `8086:0416` initially.** Refuse writes on unknown Intel GPUs.
6. **Do not touch NVIDIA during M14.** Optimus/dGPU remains a separate track.
7. **Metal write tests are manual and small.** QEMU validates build paths and
   mock/probe behavior; real iGPU register writes only happen on the laptop when
   the user is ready.

---

## Work plan

### M14-A — Pop!_OS oracle capture

Capture the Linux/i915 baseline before SemOS writes anything.

Recommended host-side capture:

```bash
mkdir -p docs/hardware/igpu-$(date +%F)

lspci -nnvv -s 00:02.0 | tee docs/hardware/igpu-$(date +%F)/lspci_00_02_0.txt

for f in /sys/class/backlight/*; do
  {
    echo "device=$f"
    cat "$f/type" 2>/dev/null || true
    cat "$f/max_brightness" 2>/dev/null || true
    cat "$f/brightness" 2>/dev/null || true
    cat "$f/actual_brightness" 2>/dev/null || true
  } | tee "docs/hardware/igpu-$(date +%F)/backlight_$(basename "$f").txt"
done

for edid in /sys/class/drm/card*-*/edid; do
  [ -s "$edid" ] || continue
  name=$(echo "$edid" | tr '/:' '__')
  cp "$edid" "docs/hardware/igpu-$(date +%F)/${name}.bin"
  edid-decode "$edid" > "docs/hardware/igpu-$(date +%F)/${name}.txt" 2>/dev/null || true
done

sudo -n cat /sys/kernel/debug/dri/0/i915_display_info \
  > docs/hardware/igpu-$(date +%F)/i915_display_info.txt 2>/dev/null || true
sudo -n cat /sys/kernel/debug/dri/0/i915_opregion \
  > docs/hardware/igpu-$(date +%F)/i915_opregion.txt 2>/dev/null || true
```

If `sudo -n` fails, run the two `sudo cat` commands manually.

Outputs we care about:

- exact backlight provider name, max, current, and type;
- panel connector name, likely `eDP-1` or `LVDS-1`;
- panel native resolution and refresh;
- iGPU BARs and command register;
- whether Linux uses native PWM, ACPI video, or vendor backlight plumbing.

Done when:

- The capture is committed under `docs/hardware/igpu-YYYY-MM-DD/`.
- We know whether brightness control should target Intel PWM registers,
  ACPI/video, or another T540p-specific path.

---

### M14-B — SemOS read-only iGPU probe

Add a new module:

- `kernel-x86_64/src/igpu.rs`

Responsibilities:

- find PCI display controller `8086:0416` at any bus/slot/function;
- confirm class `0x03`;
- print BAR0/BAR2/BAR sizing where applicable;
- enable **memory space only if needed for reads**, never bus-mastering;
- expose `IgpuInfo` with location, device ID, BARs, and generation;
- read a tiny allowlist of harmless display/status registers only after we know
  BAR0 is mapped.

Suggested log:

```text
[igpu] Intel HD 4600 Haswell GT2 @ 00:02.0 device=0x0416
[igpu] BAR0 MMIO=0x........ size=...
[igpu] BAR2 aperture=0x........ size=...
[igpu] GOP framebuffer: 1920x1080 stride=... bpp=4 fmt=BGR
[igpu] native-control: read-only probe complete
```

Done when:

- QEMU still boots with `igpu` probe returning "not found" or harmless device
  info.
- T540p metal boot prints the HD 4600 identity without breaking the existing
  framebuffer console.

---

### M14-C — Framebuffer/display diagnostics

Make the existing GOP framebuffer state more visible and app-friendly:

- extend `fb_format()` or add `fb_info()` to include width, height, stride,
  bytes-per-pixel, pixel format, and framebuffer byte length;
- print one compact display line at boot;
- add a shell builtin or demo such as `fbinfo`;
- compare GOP mode against the Pop!_OS EDID/native panel mode.

Done when:

- From SemOS shell we can see the current resolution and pixel format.
- We know whether the current GOP framebuffer is already the native panel mode.

Decision after this:

- If GOP already gives native resolution, **do not modeset yet**.
- If GOP gives a poor mode, first investigate bootloader/GOP mode selection.
- Only start native Haswell modesetting if GOP mode selection is unavailable or
  insufficient.

---

### M14-D — Backlight/brightness control

This is the highest-value native iGPU feature for day-to-day use.

Implementation should be a tiny capability, not a full display driver:

- discover/control the path selected from M14-A:
  - Intel panel PWM registers if Linux confirms native PWM;
  - or a narrow ACPI/video/OpRegion route if that is what the machine uses.
- provide safe kernel functions:
  - `backlight::get() -> Option<BacklightState>`
  - `backlight::set_percent(percent: u8) -> Result<(), Error>`
- clamp writes to `[10, 100]` by default;
- add shell commands:
  - `brightness`
  - `brightness 50`
  - `brightness up`
  - `brightness down`
- optionally map brightness keys later after shell control is stable.

First metal test:

1. print current brightness;
2. set 80%;
3. set 50%;
4. restore original;
5. verify screen never blanks.

Done when:

- Brightness can be changed from SemOS on the T540p.
- A failed probe leaves the old framebuffer console untouched.
- The original brightness is restored at the end of the demo/test.

---

### M14-E — User/app framebuffer surface

This closes the old M6 follow-up and makes "graphics" useful before full native
KMS or acceleration exists.

Options:

1. Minimal syscall returns framebuffer metadata + a mapped linear region.
2. Safer syscall exposes a draw buffer and copies damaged rects through the
   kernel.
3. Hybrid: metadata syscall now, direct mapping only for trusted/debug apps.

Recommended first version:

- add a syscall for framebuffer info;
- add a syscall to present/blit a user-owned pixel buffer or damaged rect;
- keep direct mapping as a later optimization.

Done when:

- a user-space app can draw a rectangle/text/scene without kernel-only game
  code owning the whole UI path;
- editor/TUI/game code has an obvious future migration path.

---

### M14-F — Native modesetting research spike, optional

Only start this after brightness and app framebuffer are stable.

Scope:

- read Intel PRM / Linux `i915` Haswell display path;
- model only the internal panel path;
- understand pipes, planes, transcoders, DPLL, FDI/eDP/LVDS path, watermarks,
  and required power wells;
- write a design doc before any mode-register writes.

Hard rule:

- Native modeset writes require an explicit feature gate and a restore/reboot
  fallback plan.

Done when:

- We can explain the exact register write sequence for the T540p internal panel,
  or decide to defer native modesetting and keep GOP long-term.

---

## Suggested implementation order

1. `docs/hardware/igpu-*` Linux capture.
2. `igpu.rs` read-only PCI probe.
3. Better SemOS framebuffer info diagnostics.
4. Backlight control.
5. User/app framebuffer syscall.
6. Optional GOP mode selection.
7. Optional native Haswell modesetting design.

This order makes the laptop more usable quickly while avoiding the huge trap of
starting with full `i915`-style KMS.

---

## Build/test loop

QEMU:

- validates `igpu` absence path;
- validates framebuffer-info shell/demo code;
- validates user framebuffer syscalls;
- cannot validate HD 4600 MMIO/backlight.

Metal:

- required for iGPU PCI/BAR logs;
- required for backlight;
- required for mode/resolution decisions.

Keep metal tests short:

1. build on Pop!_OS;
2. copy ESP kernel;
3. reboot SemOS;
4. run one specific command/demo;
5. capture result in docs;
6. reboot Pop!_OS only if the test needs code changes.

---

## Success criteria for "usable integrated graphics"

Minimum useful landing:

- SemOS boots into a stable high-resolution framebuffer.
- The shell/TUI/editor can report display info.
- Brightness is controllable.
- Apps have a syscall-level framebuffer path instead of being kernel-only.

That is enough to call the iGPU milestone useful even before acceleration or
native modesetting.

