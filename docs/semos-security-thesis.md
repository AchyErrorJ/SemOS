# Semantic OS — Security Thesis (Path A: From-Scratch)

**Date:** June 2026 (revised 2026-06-11)
**Status:** Working architectural document. Defines the security claim under the from-scratch / no-compatibility commitment.

**Revision note (2026-06-11):** updated for the Phase-15 tether landing (ipheth over EHCI, 2026-06-10), the promotion of bare-metal WiFi from background work to a main track, the collapse of the Expo bridge phases into a single native Swift track, and the resulting need to state the project's position on device firmware and on pairing-as-authentication. The core thesis is unchanged.

**Revision note (2026-06-15) — the re-headline.** On-device compilation now works
(rustc compiled + ran `/hello.rs` on the T540p), which reframes what this whole
document is *for*. The project's headline is now **"an agent-native, self-extending,
sovereign OS"** — an LLM agent that writes its own modules, compiles them on the
machine, and loads them into the running system. **The security tiers stop being
"security for its own sake" and become the load-bearing answer to the obvious
objection: an OS that lets an LLM rewrite itself, *safely*.** The capability fence
on agent-authored code is the only context where the tier model is genuinely
interesting — and it already exists: `current_task_max_tier()` gates the LLM/
semantic/process/namespace syscalls pervasively, and `spawn_tier = max_tier.min(
caller_tier)` means a child can never exceed its spawner's clearance, so spawning
agent-written modules at tier 0 sandboxes them automatically. No separate `CapSet`
is needed for v1; a finer per-syscall capability set is a later refinement. The
from-scratch / smallness / auditability claims below are unchanged and now do
double duty as *what makes agent self-modification tractable*. See
`MASTER_ROADMAP.md` for the combined picture.

---

## The commitment

Semantic OS commits to building its entire userspace from scratch. No POSIX libc. No vendored Linux software. No compatibility shims. No "we'll port ffmpeg eventually." Every program that runs on Semantic OS is written for Semantic OS, against the syscall surface Semantic OS provides.

This is a ten-year project. The pace reflects the commitment. The tradeoff is explicit: dramatically slower progress in exchange for architectural coherence and the security properties that follow from it.

---

## The security thesis

> Semantic OS is a small, from-scratch system. The LLM agent runs at ring-0 because the entire system is small enough to be auditable by one person. We don't import compatibility layers because compatibility layers carry attack surface we didn't write and don't understand. Smallness is the security property.

That sentence is the load-bearing claim. Three parts:

1. **Smallness.** The kernel exposes a deliberately narrow syscall surface. Tens of operations, not hundreds. Every operation is one a maintainer can hold in their head and reason about.
2. **From-scratch.** Every line of privileged code, and every line of unprivileged code that talks to it, was written by the project's maintainers. There is no "vendored module from an external ecosystem" running with privileges. No attack surface inherited from elsewhere — on the host CPU. Device firmware running on peripheral processors is outside this claim and handled as declared trust (commitment 6).
3. **Auditability.** Because the system is small and self-written, one person can reason about the whole privileged surface. The security claim is verifiable by inspection, not by trust in a long supply chain.

The ring-0 LLM agent is *enabled* by these properties, not in tension with them. A small, fully-audited, from-scratch system is exactly the kind of place where an LLM at ring-0 is tractable. It is the opposite of running an LLM at ring-0 on Linux, which would be insane.

---

## What this commits to

The thesis only holds if the project keeps its commitments. The commitments are:

### 1. No compatibility imports

Ever. The syscall surface is not pulled toward POSIX shape. Programs are written *to* the Semantic OS surface, not the other way around. When a new program needs a capability the kernel doesn't have, the question is *"what's the right Semantic-OS-shaped syscall for this?"* — not *"what does Linux call this?"*

This forecloses certain kinds of progress. The web browser is yours, not a port of Firefox. The package manager is yours, not Cargo running on a libc emulator. The video decoder is yours, not ffmpeg behind a shim. Each of those is a real project. None of them are quick.

### 2. Every syscall is a decision

Adding a syscall is an architectural event. Each new operation is documented, justified, audited, and added to the surface inventory. The discipline is not "do we need this to ship feature X" — it is "is this the right primitive given the security thesis, and what's its blast radius if misused."

There's no shortcut. The surface stays small because additions are rare and considered.

### 3. The surface inventory is maintained

A single document tracks every privileged operation the kernel exposes. Each entry: operation name, parameters, capability requirements, intended use, audit history. The inventory is the artifact that backs the auditability claim. Without it, "small attack surface" is rhetoric; with it, it's checkable.

### 4. From-scratch extends to dependencies

External Rust crates are minimized in the privileged path. Any crate that's part of the kernel build is treated like project code: audited, understood, and ideally written by the project. The ena vendoring in the rustc port is the model — when an external dependency is needed, it's vendored, patched, audited, and owned by the project from that point forward.

This rules out unaudited deep dependency trees. It does not rule out *any* external code. It means external code crosses the ring-0 boundary only after the project has accepted maintenance responsibility for it.

### 5. LLM agent inputs are scoped, even with ring-0 privileges

The LLM running at ring-0 has, in principle, full kernel access. In practice, each agent invocation should be scoped: this session can read these files, write to these paths, call these tools. The scoping is enforced inside the kernel — not by moving the agent to userspace, but by capability tokens granted per-session.

This is a real piece of architecture to design and build. It's compatible with ring-0 placement; it's actually easier to implement at ring-0 than across a kernel/userspace boundary, because the kernel has full visibility into what the agent is doing.

### 6. Device firmware is declared trust, not silent trust

The from-scratch claim is about code the *host CPU* executes. Some hardware requires opaque vendor firmware running on the device's own processor — the iwlwifi NIC is the first significant case, and its promotion from background work to a main track makes this current rather than hypothetical. That firmware cannot be audited, rewritten, or owned. Pretending otherwise would make the thesis false; ignoring it would make the thesis vague.

The precise claim, restated: **every line of code the host executes is from-scratch and auditable; device firmware is a declared, inventoried trust boundary.** This is the same posture seL4 deployments take — formal verification of the kernel, declared trust in NIC and storage controller firmware. It is defensible because it is stated.

The discipline that follows:

- Every firmware blob the kernel loads gets an entry in the surface inventory: device, blob identity (hash + version), what bus access the device has (notably whether it is behind the IOMMU), and what the blast radius is if the firmware is malicious or compromised.
- Where the platform allows, the device is constrained by the IOMMU so firmware compromise is bounded to the device's DMA windows, not arbitrary physical memory.
- Firmware blobs are vendored like crates: pinned, hashed, stored in-tree, never fetched live at build time.

---

## What the security thesis does *not* claim

Honest scope:

- **Not "we solved AI safety at the kernel level."** This is a small-systems argument, not a foundational claim about LLM safety. An LLM with full ring-0 privileges in Semantic OS is auditable *because the system is small*, not because of any special LLM mediation technology.
- **Not "we eliminated all bugs."** Memory safety from Rust eliminates a class of bugs. Logic bugs remain. The auditability claim is about reasoning about the surface, not about formal correctness of every line.
- **Not "this is more secure than seL4."** seL4 has formal verification. Semantic OS does not (yet). The claim is comparable in *direction* — small surface, careful design — but seL4 has more formal weight behind it.
- **Not "ring-0 LLMs are universally safe."** They're tractable in this system because of the from-scratch commitment. The same architectural choice on a Linux-shaped system would be reckless.

The thesis is narrow and defensible. Overclaiming it weakens it.

---

## Practical disciplines (adopt now)

### Discipline 1: Surface inventory exists

A `KERNEL_SURFACE.md` document in the repository, kept current with every syscall addition. Format: operation name, signature, capability required, audit notes, change log.

Cost: a weekend to write the first version covering the existing surface. Ongoing cost: one entry per new syscall.

### Discipline 2: Roadmap entries answer the surface question

Every new milestone in the roadmap, before it's started, answers:

- Does this require a new syscall? If yes, which one(s)?
- What's the smallest possible shape for the new syscall(s)?
- What capability check guards it?
- What's the blast radius if the LLM misuses it?

This is process, not technology. It costs nothing except the discipline of asking the question.

### Discipline 3: Vendoring is intentional

When external Rust crates enter the privileged build, they're vendored and patched into the project, not pulled live from crates.io as a build dependency. The vendoring is the moment the project accepts ownership.

The ena 0.14.4 vendoring in the rustc port set the pattern. The same applies to anything that ends up running in the privileged path.

### Discipline 4: Capability scoping for agent sessions

Design and build a per-session capability system for the LLM agent. Agent invocations carry tokens that describe what they're allowed to do; the kernel enforces. This is a real engineering project but it's bounded — probably a few weeks of work once the core kernel is stable.

This is the mitigation that does the most for security per unit of effort.

### Discipline 5: Formal verification as a long-term option

The kernel stays small enough that formal verification of critical paths is achievable. Not now; not in year three. But by year ten, with a stable surface, this is a project that's reachable. seL4 took years and a team; Semantic OS won't replicate that scale, but bounded verification of specific subsystems (the syscall dispatch, the capability check, the agent token validation) is realistic.

This is the long arc that makes the security claim formally checkable rather than rhetorical.

---

## Where the thesis is weakest

Honesty about the soft spots:

### The LLM's inputs are still untrusted

Even with ring-0 placement and small kernel surface, the LLM consumes prompts from many sources — user input, file contents, web content (when the from-scratch browser ships), agent tool outputs. A prompt injection that convinces the LLM to do something harmful can cause harm bounded by what the syscall surface allows. With ~50 syscalls, that's bounded but not zero.

The mitigation is discipline 4 (capability scoping). The LLM having ring-0 access is fine; the *specific invocation* should have scoped capabilities for the work it's doing.

### Bugs in the project's own code

Rust eliminates memory-unsafety bugs. It doesn't eliminate logic bugs. A missing bounds check in the filesystem driver, an integer overflow in the network stack, an off-by-one in the syscall dispatch — these are still possible. The auditability claim helps because bugs in small audited code are more findable than bugs in large unaudited code. It doesn't make the code bug-free.

The mitigation is rigorous testing (already underway — 150+ boot-time DEMOs), fuzzing where applicable, and eventually formal verification of critical paths.

### Agent actions persist and accumulate

The LLM writes to disk, modifies system state, executes things. If the LLM is wrong — hallucinates, follows a bad prompt, has bugs in its loop logic — the consequences accumulate and persist.

The mitigation is transactional, reversible, audited agent actions. The kernel knows what the agent did and can roll back. This is a feature to build, not an automatic property. It's a kernel-level feature, compatible with ring-0 LLM placement; in fact easier to implement at ring-0 because the kernel has full visibility.

### Cryptographic and network code

The TLS stack is from-scratch (DEMO 8+). That's consistent with the thesis. It also means the TLS stack hasn't been reviewed by anyone who does TLS for a living. Cryptographic code is famously hard to get right. The current implementation may be subtly wrong in ways only an expert would catch.

Mitigation, when the project is mature enough: invite external cryptographic review of the TLS path specifically. The from-scratch commitment doesn't preclude external review; it precludes external code in the privileged path. Different things.

With WiFi promoted, this soft spot widens: the WPA2/WPA3 handshake is more from-scratch cryptographic code (built on the existing HMAC/SHA-256/HKDF primitives from Phase 8, but new protocol logic), and it authenticates the link the whole network stack rides on. The same mitigation applies — the eventual external review should cover the 802.11 authentication path alongside TLS.

### Wireless firmware is the largest unauditable component

Commitment 6 declares the trust; this section names it as a weakness. The iwlwifi firmware is megabytes of opaque code running on a bus-attached processor, far larger than any single audited subsystem in the project. A vulnerability in it — and WiFi firmware has a public history of them — is invisible to the audit-by-inspection claim. IOMMU containment and blob pinning bound the damage; they do not eliminate it. The honest statement is that the moment WiFi lands, the smallest-attack-surface claim applies to the *host* surface, and the system's total trusted base grows by one component nobody on the project can read.

### Pairing-as-authentication concentrates identity in the phone

The locked architectural decision — no passwords, no login screen, the paired phone *is* the user account — is elegant and consistent with phone-as-peripheral. It also means the phone is a single point of identity failure: a lost, stolen, or compromised phone is the account, and the unattended laptop's security is exactly the security of the pairing protocol and its session state. This is not worse than passwords in any obvious way (phones have Secure Enclave-backed keys; passwords have humans), but it is a *different* failure model and the thesis should not present it as a free win.

What this demands: the pairing protocol design (M55) is security-critical work, not app plumbing. Unpair/revocation must be possible from the OS side without the phone (the recovery story for a lost phone). Session lifetime, re-authentication triggers, and what an attacker with brief physical access to an unlocked paired session can do all need answers in the protocol document before the protocol ships.

---

## What this means for the roadmap

The Phase 14 self-hosting milestone (rustc on Semantic OS) is the right shape. The rustc port has been done correctly: `target_os = "none"`, against `semos-std`, not against a libc. This is the from-scratch pattern working in practice. As of 2026-06-11, M27 is at the metadata wall with the C3 de-risk step landed; the disk-resident sysroot (read-only, no general filesystem — the scope guard in the M27 design is itself discipline 2 in action) is the remaining plumbing.

Phase 15 (tether) landed 2026-06-10 and validated something the thesis cares about: the first real network link on bare metal runs entirely through from-scratch host code (EHCI driver, ipheth, smoltcp integration) with no firmware blob involved — the phone's radio is the phone's problem. That clean property does *not* survive the next step.

**Bare-metal WiFi is promoted from background work to a main track.** This is the first milestone where commitment 6 (declared firmware trust) is load-bearing rather than theoretical. Before firmware upload work starts, the iwlwifi blob gets its surface-inventory entry and the IOMMU question gets answered. The WPA2/WPA3 work adds to the eventual external-crypto-review scope.

**The phone phases collapse into one native track.** With WiFi handling the network and the sensor work requiring native ARKit, the Expo bridge prototype loses its rationale (it existed to deliver networking without a Mac). The pairing protocol and companion capabilities land as a single native Swift app. Security consequence: the pairing protocol (M55) is now designed once, for production, rather than prototyped and rewritten — which raises the bar on getting the design document right the first time. See the pairing weakness above for what that document must answer.

Phases beyond (browser, agent improvements, package manager, media) keep the same shape. Each phase, before it's started, answers the surface question (discipline 2). Each phase commits to from-scratch implementation, not porting.

This makes the phases longer than the roadmaps currently estimate. A from-scratch web browser is not a six-month project; it's two or three years of focused work to reach "usable for reading documentation." A from-scratch 802.11 stack with WPA is a real subsystem, not evenings. The roadmaps should be honest about what from-scratch costs — not because the destinations are wrong but because the timelines follow from the commitment.

---

## The honest framing

Semantic OS is not "a security-first operating system that figured out how to safely run LLMs at ring-0." That phrasing suggests a technical breakthrough.

Semantic OS is "a from-scratch operating system built slowly enough that one person can hold the whole privileged surface in their head. The LLM at ring-0 works because the system is small. The system is small because we said no to compatibility."

That's the accurate version. It's a project about discipline, not a project about a clever trick.

It's also a more compelling story, because it's true. Most secure systems claim to have a special technique. Semantic OS claims to have a *commitment*. Commitments are harder to falsify than techniques.

The audience for this thesis is patient. It's not investors who want a 24-month payoff. It's the small population of people who appreciate Plan 9, who follow Oberon, who think about Mu, who read the seL4 papers carefully. That's a real audience. It's just not the venture capital audience.

For the venture capital pitch (Baukunst and similar), the OS doesn't appear at all. For the right audience, when the project is mature enough to be shown, the OS is the centerpiece — not as a product but as a demonstration of how the team thinks.

---

## Open questions to resolve

1. **The surface inventory.** Still not written. The right time was already "now"; with WiFi promoted it becomes blocking — the firmware-blob entry format (commitment 6) needs the document to exist. A weekend of work, scheduled before iwlwifi firmware upload starts.
2. **Capability scoping for agent sessions.** Design now or after Phase 14 closes? Probably after — Phase 14 is the priority. But the design can be sketched now.
3. **IOMMU policy for the WiFi NIC.** Does the W540's VT-d cover the iwlwifi device, and does the kernel enable it? This determines whether firmware compromise is bounded to DMA windows or has the run of physical memory. Answer required before the firmware-upload milestone is marked done.
4. **Pairing protocol security review.** The M55 design document now ships once, for production. Decide what "reviewed" means for it — at minimum a written threat model (lost phone, stolen phone, evil-maid on the laptop, MITM during QR exchange) before implementation starts.
5. **External cryptographic review.** When? Probably not until Phase 14 is closed and the TLS stack has been stable in use. Scope now includes the WPA2/WPA3 handshake path. Year three or four.
6. **Formal verification target.** Which subsystem first? Syscall dispatch is the smallest and highest-impact. Realistic candidate for year five or six.

---

## What this document is

This is the security thesis for Semantic OS under the from-scratch commitment. It defines what's being claimed, what the commitments are, what the disciplines are, and where the thesis is weakest.

It's a working document. Update it when commitments shift. The thesis is only as strong as the commitments behind it; documenting both keeps the project honest about which claims it can defend.

If the from-scratch commitment ever softens — if compatibility pressure becomes real and gets accepted — this document needs to be rewritten. The thesis as stated doesn't survive that shift.

For now: the commitment is clear. The thesis holds. The disciplines are the work.
