# Designer OS — Thesis: who Semantic OS is for

**Date:** 2026-07-17
**Status:** Working architectural document. Companion to
`semos-security-thesis.md` — that document is the *how*; this one is the *who*
and the *why*. Related: `provenance-commitment.md`, `SHEAF_PLAN.md`,
`VOUCH_MECHANISM_DESIGN_2026-06-15.md`.

---

## The claim

> Semantic OS is a designer's operating system. It senses the outside world,
> understands it with AI, and reserves judgment — taste — to the human.

The security thesis explains why a ring-0 LLM agent is *tractable* (smallness,
from-scratch, auditability). It does not explain what the agent is *for*. This
document does: the agent exists to understand the world on the designer's
behalf, and the human exists to decide. The security tiers were never really
"security for its own sake" — the 2026-06-15 revision already admitted they are
the capability fence on agent-authored code. This thesis goes one step further:
the fence is the formal expression of a division of labor. **The machine
proposes; the human disposes. The kernel enforces the arrangement.**

A designer's tools today are scattered: a sensor over here, a model over
there, taste exercised in the cracks between applications that own their own
files and their own formats. The designer's OS makes the three acts of design
— sensing, understanding, judging — into system primitives instead of
application accidents.

---

## The triad

Every subsystem of SemOS answers to one of three verbs.

### 1. Sense — input from the outside world

LiDAR capture and as-built extraction (`ls-site`), WiFi scanning against real
silicon (iwlwifi), USB device enumeration, the AR overlay work (LegiView) that
puts design back on top of the built world. Sensors are not peripherals
attached to the OS as an afterthought; **the world is input**, and capture is
a kernel-adjacent concern. From the moment of capture, sensor data is a
semantic object with provenance — the provenance commitment applies at
birth, not at export.

### 2. Understand — AI makes the input legible

The LLM agent, the tier-mediated LLM context (redaction before bytes leave
the kernel), on-device rustc so understanding can extend itself. The agent's
job is to turn capture into constraints, options, and questions. Its output is
always *proposed*, never *final* — see the next verb.

### 3. Judge — taste is human, and the kernel knows it

This is the verb other operating systems don't have. SemOS already encodes it
as a primitive: **SYS_VOUCH is taste as a capability.** Only the human at the
interactive console can elevate agent work; the agent structurally cannot
elevate its own. That mechanism was designed as a security control. It is also
the first kernel-level expression of an aesthetic fact: in a system where a
machine can generate a thousand options, the scarce resource is the person who
can say *this one, not those* — and the OS treats that act as first-class,
auditable, and permanent.

The triad is a loop, not a pipeline: sense → understand → judge → **make** —
and the made thing re-enters the world, where it can be sensed again
(as-built capture against design intent). LegibleStudios is the "make" end
(permit sets are production). LegiView closes the loop. They were always one
system.

---

## Provenance is the taste ledger

`provenance-commitment.md` established that vouch and provenance are two
halves of one claim — trust pointed at the future and the past. The designer
thesis adds what the past-half is *for*: **a record of judgment.**

Every `derived_from`, every export stamp, every `by = "user:…"` /
`by = "agent:…"` line is the system remembering who decided what. A bundle's
provenance chain answers the questions a designer actually gets asked: *why
does it look like this? who chose this option? what was rejected along the
way?* In a future where generation is cheap, the judgment trail is the
valuable artifact — and Sheaf makes it a byproduct of normal work instead of
a discipline the designer has to remember to keep.

Two rules, continuing the commitments of that document:

1. **Agents identify as agents in the provenance chain.** Non-negotiable —
   the taste ledger is only meaningful if human and machine judgment are
   distinguishable in it.
2. **Agent output is never final until judged.** Unvouched, unexported,
   un-stamped-as-final — pick the mechanism per subsystem, but the state
   "proposed by machine, not yet disposed by human" must always be
   representable and visible.

---

## The substrate test

The honest anxiety of this project: *we build before the questions, because we
have to prepare for a future where we don't know where or what we'll work.*

Building substrate ahead of the questions is not the failure mode — every
toolmaker does it, and the commitments that already exist (bundles, tiers,
vouch, provenance) are all substrate. The failure mode is building **answers
disguised as substrate**: load-bearing decisions that foreclose questions not
yet heard. So it is a design rule and not a vibe:

> **Every substrate decision must increase, or at least not decrease, the
> number of question-types the system can ask later.**

Bundles pass: they are agnostic about their contents. Tier-per-facet passes:
it does not presume what will count as sensitive in a project that doesn't
exist yet. Vouch passes spectacularly: it is literally a question — *human, do
you trust this?* — that the system will keep asking about things nobody has
imagined yet.

Where the test bites: anywhere a *design answer* hardens into the kernel — a
fixed vocabulary of room types, a hardcoded workflow, one blessed way to
render. The corrective pattern already exists in LegibleStudios' QBD
structure: the engine is substrate, and **questions live in data** —
manifests, templates, catalogs — swappable per project. That discipline now
applies at the OS level. When a new domain appears, the question must be
*"what data teaches the system to ask about this?"* — never *"what do we
recompile?"*

---

## The questions log

Preparing for an unknown future is a vibe until it becomes an evidence-gathering
process. The method:

1. Every real project the system touches — a residence, a site, a client —
   generates a log of **questions the current system could not answer, or
   could not even express.**
2. The log is the roadmap. Substrate work is justified by pointing at logged
   questions it makes askable; substrate that answers no logged question and
   enables no new question-type is procrastination dressed as progress.
3. Entries are semantic objects with provenance, like everything else. The
   system should be able to ask *its own* questions log what it still can't
   ask.

This converts "we don't know where or what we'll work" from a risk into the
design brief it actually is.

---

## The commitments

1. **Every subsystem answers to the triad.** Sense, understand, or judge — a
   subsystem that answers to none of the three, or that can't say which, is
   scope creep.
2. **Judgment is a first-class operation.** Proposed-vs-disposed is
   representable everywhere; agents identify as agents; the human's decision
   is recorded, not implied.
3. **Substrate passes the question-types test.** Answers live in data. The
   kernel stays question-agnostic.
4. **The world is input.** Capture carries provenance from birth; a sensor
   reading without provenance is a regression the same way an unstamped
   artifact is.
5. **The questions log is maintained** and is the cited source for milestone
   proposals in `MASTER_ROADMAP.md`.

---

## Relation to the security thesis

The security thesis says: a small, from-scratch, auditable system is the kind
of place a ring-0 LLM can run safely. The designer thesis says what that LLM
is doing there: understanding a sensed world so a human can spend their
attention on the one thing that can't be delegated — deciding. Security is
the how. The designer is the who. The questions log is the where-next.
