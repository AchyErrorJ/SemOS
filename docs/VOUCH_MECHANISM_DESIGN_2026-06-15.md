# Vouch Mechanism — design (2026-06-15)

How a tier-0-by-default agent-authored tool earns the right to run with privilege.
Pairs with the **deny-by-default fence** already in the kernel (`spawn_namespace_elf`
caps any namespace executable to tier 0 unless `ObjectFlags::VOUCHED_EXEC` is set).
This doc designs what sets that flag, and on what basis.

User decision (2026-06-15): *"if a privileged shell creates a tool that can then
do bad things, that would be bad. So any created tool drops to tier 0 unless the
tool can be shown to be safe."*

---

## 1. Threat model (what vouching must defend against)

The agent can **write source and compile tools** on the machine. So:

1. **Self-elevation.** The agent must NOT be able to vouch its own code. If the
   vouch authority is reachable from the agent's syscall surface, the fence is
   theater. → **The vouch authority lives outside the agent.**
2. **Bait-and-switch.** A tool vouched as `greet` must not be swappable for a
   different ELF afterward. → **Vouch binds to the exact bytes** (content hash),
   and a vouched object is immutable.
3. **Privilege laundering.** A tier-0 tool must not spawn a "more trusted" child
   to do its dirty work. → Already covered: `spawn_tier = max_tier.min(caller_tier)`
   means a child never exceeds its parent; a tier-0 tool's children are tier 0.
4. **Over-grant.** Vouching "safe" should not mean "all-powerful." → Grant the
   *least* privilege that makes the tool useful (v2: per-capability, not blanket
   tier).

---

## 2. The trust anchor: the human, via the interactive shell

The only actor allowed to vouch is the **human at the keyboard**, expressed
through the **interactive `sem-sh`** (built with `--features interactive`). That
shell runs at the human's clearance and only acts on keystrokes the human typed.

- The **LLM agent has NO vouch syscall.** `SYS_VOUCH` is rejected unless the
  caller is the interactive shell session (or a future dedicated reviewer task)
  AND the caller's `current_task_max_tier()` ≥ the tier being granted (you can't
  grant clearance you don't hold).
- This makes vouching a deliberate human act: *you* looked at the tool and said
  "this one's fine, give it Internal." The agent can ask, but cannot grant.

> The whole security story in one line: **the agent writes tools; the human
> decides which tools are trusted; the kernel enforces that decision.**

---

## 3. v1 — human-review vouch (the stopgap to build first)

Smallest thing that is actually safe.

### Object fields (add to `SemanticObject`)
- `vouch_tier: u8` — max tier this tool may run at (0 = unvouched, the default).
- `vouch_hash: [u8; 32]` — SHA-256 of the ELF content the vouch was granted for.
- (existing) `ObjectFlags::VOUCHED_EXEC` — set when `vouch_tier > 0`.
- On vouch, also set `ObjectFlags::IMMUTABLE` so the bytes can't be edited after.

### `SYS_VOUCH(path_ptr, path_len, grant_tier)` → ok / err
Gate (all must hold, else `EPERM`):
1. Caller is the interactive shell session (a `FROM_INTERACTIVE` task marker, or
   a dedicated `reviewer` capability — NOT any agent task).
2. `current_task_max_tier() >= grant_tier`.
3. The path resolves to a namespace object with ELF content.

Effect: compute `SHA-256(elf)`, store it in `vouch_hash`, set `vouch_tier =
grant_tier`, set `VOUCHED_EXEC | IMMUTABLE`. Log `vouched <path> @ tier N`.

### Enforcement at spawn (`spawn_namespace_elf`, extends today's fence)
```
exec_cap = if obj.flags.is_vouched_exec()
              && sha256(elf) == obj.vouch_hash      // bytes unchanged since vouch
           { obj.vouch_tier }                        // run up to the granted tier
           else { 0 };                               // unvouched / tampered → tier 0
spawn_tier = spawn_tier.min(exec_cap);
```
The hash recheck closes bait-and-switch even if IMMUTABLE were ever bypassed.

### Shell UX
- `vouch <path> [tier]` builtin (interactive only) → `SYS_VOUCH`. Default tier =
  Internal(1) (LLM-capable, but still can't read Secret credentials).
- `vouches` builtin → list vouched objects + their granted tier (audit).
- `unvouch <path>` → clear the flag (revocation).

v1 "shown to be safe" = **a human read it and typed `vouch`.** Honest, doesn't
scale, but correct. Good enough until there are many tools.

---

## 4. v2 — capability manifest + consent (the target, Genode-shaped)

Replace the blunt "grant a tier" with "grant exactly the capabilities declared."

### The manifest
A tool declares what it needs, as a small record the kernel can read **without
trusting the tool** — best as a dedicated ELF section (`.semos.caps`) or a sidecar
namespace object keyed to the ELF hash. Example:
```
caps: { net: false, fs_write: ["/tmp"], llm: true, raw_dev: false, spawn: false }
```

### Vouch = approve the declared set
`vouch <path>` shows the human the declared capabilities and records the approved
subset into the object (`granted_caps`). The grant can be *narrower* than the
request (human says "net? no — the rest yes").

### Enforcement = the per-syscall CapSet
Each task carries a `CapSet` (set at spawn from the object's `granted_caps`, or
empty for unvouched). The syscall dispatcher checks it before the sensitive
classes (net / llm / fs-write-outside-scope / raw device / spawn):
```
if !current_task().caps.allows(syscall_no, args) { return EPERM; }
```
This is the finer fence sketched in the loader notes — deferred until v1 lands and
there's a real tool to scope. Tier and CapSet coexist: tier governs *data
confidentiality* (what objects it can read), CapSet governs *actions* (what it can
do). A tool can be Internal-tier (reads internal docs) yet have net = false.

### Provenance shortcut (optional)
Tools built from a trusted source tree, or signed by a trusted key, could be
auto-vouched at a low tier without per-tool human review — a scaling path once the
manifest + signing story exists. Not v1.

---

## 5. Build order

1. **v1 fields + `SYS_VOUCH` + spawn hash-check + `vouch`/`vouches`/`unvouch`
   builtins.** Offline-doable now (SHA-256 is already in the crypto stack). The
   default-deny fence is already shipped; this adds the earn-trust path.
2. **Mark the interactive shell as the vouch authority** (the `FROM_INTERACTIVE`
   task marker) so the agent provably can't call `SYS_VOUCH`.
3. **v2 manifest + CapSet** once there are enough tools that per-tool human review
   is the bottleneck.

## 6. What this is NOT

- Not a sandbox escape audit of the ELF loader itself (separate hardening).
- Not signing/PKI (that's an optional v2+ provenance path).
- Not a claim that a vouched tool is *correct* — only that a human deliberately
  accepted its declared behavior at a chosen privilege. Smallness + human review,
  consistent with the security thesis.
