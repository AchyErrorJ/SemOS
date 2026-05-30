# Threat model + package model

Sketch of how SemOS keeps "lefpad-class" supply-chain attacks out of
the system, and what's worth designing now even though it's not on the
critical path until untrusted input arrives. Written 2026-05-30 during
M27 D.2 work, in response to the question: "do we need a new
programming language?" Short answer: no — language choice doesn't
solve dependency vulnerabilities. Package model does.

Tied to the 4-tier security model already wired in the kernel
(Public / Internal / Sensitive / Secret); see also the agent shell
sandbox precedent (`feedback`/`project` memory entries) where the LLM
runs Ring-3 at tier 0 so it cannot read or write higher-tier data.

## Three questions you have to keep separate

People conflate these because they all touch "what runs on my OS." They
take different answers.

1. **What's the host language?** Already settled: Rust, for the kernel,
   for `semos-std`, for user programs. Memory safety without a runtime,
   zero-cost abstractions, no GC pauses. Not negotiable for a kernel.

2. **What scripting / dynamic environment do users get?** Open question.
   The realistic shapes are (a) MicroPython, (b) a small Rust-DSL
   compiled by our own Cranelift toolchain (the M27 endgame would make
   this feasible), (c) some Lua-class embedded scripting language. Pick
   later — the kernel doesn't care.

3. **How do programs get installed and what stops a compromised one?**
   This is the security question, and **changing the answer to question
   1 or 2 does not change the answer to question 3.** A compromised
   Python package and a compromised Rust crate hit you the same way if
   the install path is the same.

Most "we need a new language" arguments are actually unhappy with
question 3 and misdiagnosing it.

## Why a new programming language is the wrong axis

Languages take a decade-plus to mature in any production sense. The
toolchain, the standard library, the editor support, the documentation,
the community that finds the sharp edges — all of that compounds slowly.
Inventing a language to dodge supply-chain attacks is paying language
costs to fix a package-model problem.

What we *can* do cheaply: have our **own** small DSL on top of Rust+
Cranelift for user-facing scripting once M27 lands. That's a few hundred
lines of parser + the Cranelift codegen we're porting now. It doesn't
replace Rust; it's a runtime-compiled mini-language that's tier-aware by
construction. The eventual D.3 work in `SELF_HOSTING_PLAN.md` is the
foundation for this if we want it.

## Python on SemOS — the realistic path

CPython is hopeless: written in C, depends on libc, ~30 MB of binary
artifacts, pulls a giant transitive dep tree the moment anyone uses
pip. Trying to port CPython is paying for things that hurt us.

**MicroPython** is the right answer if Python matters to you:
- Designed for embedded / bare-metal targets from day one
- Single-binary, a few hundred KB
- Subset of CPython 3 + a stdlib-equivalent (`upip`, `uos`, `usocket` …)
- Already runs on Cortex-M, ESP, RP2040 — porting to SemOS is "implement
  a few syscalls + an allocator + a frozen-module loader." Not trivial,
  but on the order of a few sessions, not a year.

What MicroPython does NOT give you: pip. **And that's a feature.** No
pip means no automatic transitive trust. Users who want a library install
it explicitly and visibly. Anything that ships with SemOS goes through
the package model (next section).

(Other options: Lua / Janet / Wren are all defensible. Pick once the
need is concrete.)

## The package model — what actually defends against supply chain

This is the thing worth designing now even if the implementation waits.
The same shape that fixes Python's supply chain also fixes anyone else's.

### The principle: tier-aware install, no transitive default trust

The 4-tier model already gates *execution* — a tier-0 caller cannot
SYS_SPAWN a tier-2 binary. Extend the same principle to *installation*
and to *imports*:

- **Tier 0 (Public):** "anyone's code." Sandboxed by execution; can't
  read Internal/Sensitive/Secret files; can't spawn higher-tier binaries.
  Install path: anything in a public registry that satisfies a basic
  hash check is fine.
- **Tier 1 (Internal):** code you've adopted. Install path: signed by a
  maintainer key you've explicitly vouched for, OR the source has been
  read by you and is committed to your machine's local registry.
- **Tier 2 (Sensitive):** code that touches PII / credentials / your
  agent's tools. Install path: signed by *your* key (or a small set
  you control) and source-vetted.
- **Tier 3 (Secret):** code you wrote yourself, locally compiled,
  never copied from anywhere external.

The install syscall (when we have one) takes a target tier as input and
*denies* the install if the source doesn't meet that tier's evidence
bar. No "drift upward": a tier-1 library cannot be promoted to tier-2
use by another program importing it.

### The hard rule: no transitive trust by default

Today's pip / npm / cargo all have an implicit rule: "if you trust
`foo`, you trust everything `foo` transitively depends on." That's how
event-stream and colors.js worked. A library you trusted decided to
trust someone else, and you inherited that decision.

The SemOS rule should be the inverse: **transitive deps inherit the
caller's tier ceiling, not the called crate's.** A tier-2 program that
imports `prettytext` only gets `prettytext` at tier 0 — its writes
can't touch Sensitive data. If `prettytext` actually needs to manipulate
Sensitive bytes, the *caller* has to explicitly grant it tier-2 at
install time, which forces a human-readable consent step.

This is what the agent shell sandbox already does at runtime: the LLM
runs tier 0, so even if it `bash`-spawns sem-sh, that shell can't read
Secret files. Apply the same pattern at install time and you've killed
the lefpad attack class.

### Source-available vetting at the boundary

For anything above tier 0, install requires source. The user (or a
designated agent) reads the diff. This is the same hygiene as code review
on a PR — applied to packages.

For tier 1, you can vouch by *signature* — "I trust this maintainer's
key for code under tier 1." Compromise of the maintainer's key still
bounds blast radius to tier-1 access, never higher.

For tier 2+, every install is hand-reviewed. Yes, that's expensive. The
point is that you only do it for the small set of code that's allowed
to touch Sensitive data. The vast majority of useful libraries are
fine at tier 0.

### Registry shape

When we have one, the registry is:
- **Content-addressed.** Identifier is `sha256(source-tarball)`, not
  `name@version`. Kills typosquatting and dependency-confusion attacks
  (no way to "name-collide" with a hash).
- **Append-only.** Past versions are immutable. Cuts the
  install-then-tamper attack.
- **Optionally federated.** You can pin a registry URL per-tier;
  Sensitive code might be pinned to a registry only you serve.

This shape is well-understood — Nix and Sigstore are close in spirit.
We don't have to invent it.

## What's safe today vs what gets dangerous when

**Today** (current SemOS state, M27 in progress): the threat model is
"is your build environment trustworthy?" That's your laptop's threat
model. SemOS doesn't load anything at runtime that wasn't compiled into
its boot image. There's no install syscall a hacker can hit. There's
no network registry. The agent shell sandbox already handles the one
case of "untrusted code at runtime" (the LLM), and handles it correctly.

You can ship a defensible SemOS without a single line of new package
machinery. Don't spend cycles worrying about it.

**Dangerous transition #1:** when SemOS gains an *install* syscall that
takes a file path or network resource and registers it as a spawnable
binary. The moment that exists, the registry-and-tier story needs to be
real — not just sketched. Otherwise you have a tier-3 user (the human)
installing some random binary at tier-3 by default, and you're back to
1990s-Windows-double-click semantics.

**Dangerous transition #2:** when SemOS exposes a network API the
outside world can hit. Even a "harmless" read-only API gives an
attacker a probe surface. The 4-tier model on the kernel side already
handles this correctly — the API should run at tier 0 by construction
— but you'll want to audit before opening any port.

**Dangerous transition #3:** when SemOS runs a third-party agent in
production. The LLM is one example, but anyone else's agent has the
same shape: untrusted code controlling syscalls. The existing
`feedback_agent_shell_sandbox` pattern (tier-0 by default) is exactly
how this should be handled and is the precedent for everything else.

## What to do now

- **Don't invent a language.** Question 1 is settled (Rust); question 2
  can wait; question 3 is what matters.
- **Pin the principles.** Write the rules down once so future agents
  (and future you) don't drift. This document is the start.
- **Defer the implementation.** No install syscall, no registry code,
  no MicroPython port is needed until the threat-model transitions
  above are imminent. M27 / M28 (rustc-on-SemOS, self-bootstrap) are
  higher-leverage; package model lands when there's user demand to
  install anything.
- **Reuse the tier model.** The 4-tier security primitive is already
  doing all the heavy lifting. Anything new — installs, imports,
  registry, network APIs — should plug into it, not invent a parallel
  trust system.

## Open questions for later

- Concrete syscall shape for `SYS_INSTALL(path, tier, signature)`. Probably
  has a `--review-path` flag that opens a TUI diff before committing.
- How tier-1 maintainer keys are bootstrapped (ed25519 + a small set of
  hardcoded root keys? Or pure manual on first run?)
- Whether to support unsigned tier-0 installs (yes, almost certainly —
  it's the path that makes the OS feel usable) and how to mark them as
  such in `ps`/`ls` so you can see at a glance which processes are
  running untrusted code.
- Whether MicroPython on SemOS gets first-class treatment or is just
  "another Ring-3 program that happens to interpret a language." The
  latter is simpler.
