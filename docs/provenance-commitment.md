# Commitment: Provenance — artifacts that outlive a session are self-identifying

**Date:** 2026-06-11
**Status:** Locked architectural commitment. Drop-in section for both
`semos-security-thesis.md` (as commitment 7) and the post-Phase-14 roadmap
(as a cross-cutting commitment). Milestone zero — the kernel build tag — is the
fifteen-minute first instance; the follow-ons land whenever each subsystem is
next open.

---

## The commitment

Every artifact Semantic OS emits that outlives the session that produced it
carries the provenance of what produced it. A persisted artifact can answer, by
inspection, *which kernel build, in what trust state, made me.*

This is not "the kernel knows its own version." That is one instance. The
commitment is uniform: the boot banner, the panic dump, the snapshot
filesystem, and eventually every vouched binary all carry a provenance field. An
artifact that leaves the session and cannot name its origin is a regression
against this commitment, the same way a syscall added without answering the four
surface questions is a regression against the from-scratch commitment.

The principle behind it is the one the project already runs on: **images are
disposable because they are a pure function of the source.** That only holds if
the artifact records *which source*. Provenance is the function recording its own
inputs. Without it, an artifact is not disposable-because-reproducible; it is
disposable-because-lost — discardable but not recoverable, because the
commit-plus-working-tree that would regenerate it is unknown. Provenance is what
keeps "I can always rebuild it" true in practice and not merely in theory.

---

## Provenance and vouch are two halves of one claim

The security thesis is built on knowing what is trusted. Trust has two
directions, and the system now needs both:

- **vouch** answers the forward question: *what authority does this artifact run
  with?* A freshly compiled binary is untrusted output until explicitly vouched,
  and even vouched it is capped below the caller's tier, until reboot. (DEMO 80.)
- **provenance** answers the backward question: *what produced this artifact?*

They are the same commitment — *nothing in this system is trusted without
knowing where it came from* — pointed at the future and the past. An
auditability claim is only as strong as the evidence it can produce on demand;
vouch governs what may run, provenance is the evidence of what made it. A
from-scratch system whose whole security argument is "one person can hold the
trusted surface in their head" needs the surface to be able to *name itself*.

Note the structural rhyme: a provenance field is **write-at-creation,
read-only after** — the same write-once pattern as the integrity lock and the
irrevocable tier. Provenance can't be edited by the thing it describes, for the
same reason the lock has no unlock syscall: the operation that would let an
artifact lie about its origin simply does not exist.

---

## The design rule

So it is a design rule and not a vibe:

1. **Any artifact with a persistent header gets a provenance field.**
2. The field records, at minimum, the **build tag and dirty state** of the
   producing kernel; **timestamp and toolchain version** where cheap.
3. The field is **written at creation, immutable after** — the write-once
   pattern.
4. **A reader that encounters a provenance mismatch surfaces it rather than
   silently proceeding.**

Clause 4 is what makes this a safety property and not decoration. Provenance you
record but never check is a label; provenance a reader acts on is enforcement. A
kernel reading a snapshot written by a *different* build should notice and decide
what that means, not assume the on-disk format still matches.

The graceful-degradation rule from the build script applies throughout: where
provenance is unavailable (a tarball build with no `.git`, a legacy artifact
predating the field), the value degrades to `unknown` rather than failing. A
wrong or missing stamp is never worse than the no-provenance state the project
has today.

---

## Surface discipline (the four questions)

- **New syscall?** No. Provenance is data in artifact headers the kernel already
  writes; the build tag is a compile-time constant.
- **Smallest shape?** A fixed-size field: short git hash, a dirty bit, and where
  present a timestamp and toolchain string. No variable structure, no parser.
- **Capability guard?** None needed — the field is write-at-creation, read-only
  after. There is no operation to guard because there is no mutation to permit.
- **Blast radius if misused?** Nil. A wrong provenance stamp degrades to the same
  `unknown` the system tolerates today; it cannot grant authority, leak data, or
  alter trust. Failures are conservative by construction.

It is the rare commitment that costs almost nothing and strengthens the central
claim.

---

## Milestones

**Milestone zero — kernel build tag (the fifteen-minute first instance).**
A `build.rs` in `kernel-x86_64` stamps the real git short-hash and a `-dirty`
flag (via `git rev-parse` and `git status --porcelain`) into an env var the
kernel reads with `env!`, replacing the hardcoded `BUILD-TAG abc123` placeholder.
`rerun-if-changed` on `.git/HEAD` and `.git/index` re-stamps on commit/stage.
The boot banner gains one line: `build <hash> · <date> · rustc <ver>`. The
`-dirty` half is the load-bearing half — during active development most boots
are dirty, and a clean hash without the dirty bit is a lie about what is actually
running.

**Follow-on — panic dump provenance.** The disk-resident panic dump gains a
provenance header. A recovered crash log that names its own kernel build (and
dirty state) is actionable — it says exactly what to check out and rebuild to
reproduce. A panic dump that can't identify its kernel is a clue with the
timestamp torn off, and panics are precisely the moment no one was watching.

**Follow-on — snapshot FS provenance.** The snapshot namespace header records
the build that wrote it. A booting kernel reading state from a *different* build
surfaces the mismatch (clause 4) rather than assuming the on-disk format still
matches — the bug that otherwise eats an afternoon when the from-scratch FS
format drifts between builds.

**Follow-on — vouched binary provenance (closes the loop).** A binary
`semos-rustc` produces carries which compiler build produced it. A vouched tool's
authority then chains back to a kernel build, which chains back to a commit. This
is the self-bootstrap (M28) story made auditable: when the OS compiles its own
next compiler, provenance is what stops that from being a trust black hole —
"which compiler compiled the compiler that's running" becomes answerable, the
exact question self-hosting otherwise makes unanswerable.

---

## Scheduling

Not a phase. A cross-cutting property every future milestone inherits, the way
they already inherit the four surface questions. Milestone zero is a standalone
fifteen-minute session, best taken before boots start being saved as evidence in
earnest — retroactive provenance is the one thing this commitment cannot grant,
so the boots already photographed stay unidentifiable, and that is a sunk cost.
Everything from the next build forward does not have to be. The follow-ons land
opportunistically: each stamps its header the next time its subsystem is open,
no dedicated phase required.
