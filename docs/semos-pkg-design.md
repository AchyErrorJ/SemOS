# semos-pkg (M43/M44) v1 — design

Status: implemented, QEMU-verified (DEMO 89/90). 2026-09-05.

Roadmap: M43 package manager (`semos install`), M44 local registry mirror /
cache with offline mode. DEMO 89: install resolves a dependency DAG, compiles
on-device, installs to /apps. DEMO 90: install with the mirror detached works
from the local cache.

## 0. Scope decisions (v1)

The roadmap text says `semos install ripgrep`. v1 deliberately narrows that:

- **semos-rustc compiles no_std, single-file, cg_clif guests** with five sys
  stubs (sys_write/sys_exit/sys_open/sys_fread/sys_close). std crates with
  deep dependency trees (ripgrep: ~40 deps, SIMD, mmap) cannot compile
  on-device today. v1 packages are therefore **SemOS-format packages**: tiny
  no_std tools in our own registry format. The *machinery* — index, DAG
  resolver, cache, fetch/install/remove, approval-gated install — is the
  real deliverable; the package ABI can widen later without changing it.
- **The mirror is host-seeded, not network-fetched.** The tree has TCP/DNS/
  TLS + an HTTP chunked decoder (LLM work), and virtio-net probes in QEMU,
  but in-guest HTTPS is unproven and would make the demo hostage to sandbox
  egress. v1's mirror is a disk. v1.1 can add HTTPS fetch behind the same
  "resolve → obtain bytes → compile" seam.

## 1. Mirror and registry format

The mirror lives on the **same virtio-blk disk as the SemFS journal**, in
the legacy whole-tree snapshot region the journal deliberately skipped: a
raw, length-prefixed blob at **LBA 16** (journal superblocks start at LBA
8192 — the regions never overlap). Blob layout:

```
[8B magic "SEMREG01"][u64 total payload len][payload bytes][zero pad to 512]
```

This beats the alternatives on code we don't have to write: no second
virtio-blk instance (the driver is single-instance static state), no FAT32
writer on the host, no subdirectory support in the in-kernel FAT reader
(root-dir single-file only). "Detaching the mirror" for DEMO 90 = the host
zeroes LBA 0..8192 between boots; the journal above 8192 is untouched.

The payload IS the whole registry — index and package tarballs in one
archive, format below.

Registry payload format (line-oriented header, raw byte payloads — trivially
parseable in no_std without a tar/gzip stack):

```
SEMOS-REGISTRY 1\n
pkg <name> <version> kind=<lib|bin> deps=<csv|-> bytes=<N> expect=<M>\n
<N bytes of guest Rust source><M bytes of expected selftest stdout>
...more pkg blocks...
end\n
```

- `kind=lib` packages provide functions for dependents: their source is
  concatenated BEFORE the dependent's source at build time. A lib source has
  no `_start` and no `#[panic_handler]` (the bin provides both). **Package
  sources never declare `extern "C"` stubs**: the builder prepends a prelude
  with the crate attributes AND one shared stub block (two extern blocks
  naming the same symbol are E0428 — learned the hard way; the in-guest
  rustc error goes to the guest's stderr, which serial never sees, surfacing
  only as "rustc fatal error"). Rust items are order-independent within a
  module, so the prelude's stubs are in scope for every concatenated source.
- `deps` names other packages in the same archive (same-version, no version
  solver in v1 — the DAG resolver orders them and detects cycles).
- `expect` is the byte-exact stdout the compiled package must print when run
  with no args — the same isolation-test discipline as DEMO 87/93.

v1 ships three packages, chosen so the resolver has real work:

- `fortune 1.0.0` kind=lib, no deps — provides `fn write_fortune()`.
- `cowsay 1.0.0` kind=bin, no deps — standalone.
- `motd 1.0.0` kind=bin, deps=fortune — calls `write_fortune()`; unbuildable
  without the dep, which is the point.

## 2. Local clone + cache (M44)

Two-level local state, both in the SemFS-journaled namespace (so both
survive hard kills for free):

- `/var/lib/semos-pkg/registry.sem` — the local registry **index clone**:
  the full archive copied from the mirror on first use (`semos update` or
  any command that needs the index while the mirror is attached).
- `/var/cache/crates/<name>-<version>.rs` + `.expect` — the **tarball
  cache**: extracted package payloads.

Once cloned, the mirror region can be wiped entirely: every command works
against the local clone, and `install` needs the mirror only for packages
never fetched before. That IS offline mode (DEMO 90).

## 3. Command surface

sem-sh builtin `semos` → `SYS_SEMOSPKG` (142) → kernel flow
(`run_semos_pkg` in kernel-x86_64/src/main.rs):

- `semos update` — (re)clone the registry archive from virtio1 into
  /var/lib/semos-pkg/ and extract all payloads into the cache.
- `semos list` — print the cloned index: name, version, kind, deps,
  installed state.
- `semos fetch <name>` — resolve the DAG, ensure every needed payload is in
  the cache (requires the mirror or a prior clone). No compile, no install.
- `semos install <name>` — resolve the DAG (topological order, cycle =
  error); for each package in order: source from cache (or clone);
  concatenate dep lib sources + bin source to /tmp/semos-pkg/build.rs;
  compile with semos-rustc; run in isolation and byte-compare against the
  packaged `expect` bytes. For the top-level bin only: the fail-fast human
  approval gate (`Install /apps/<name>? [y/N]`), then the atomic
  /apps/.staging rename install + post-install bare-name smoke. Deps are
  build-time inputs, not installed binaries.
- `semos remove <name>` — unlink /apps/<name> (journaled).

Security posture mirrors the selfdev demos: SYS_SEMOSPKG mutations are
interactive-console-only (is_vouch_authority), packages install as Public
tier objects, installs run through the human gate, and everything the
package does at run time happens fenced at tier 0.

## 4. Failure modes

| failure | behaviour |
|---|---|
| mirror absent, no local clone | `list`/`fetch`/`install` say "no registry" |
| mirror absent, package cached | install proceeds from cache (DEMO 90) |
| dependency cycle | resolve errors, names the cycle |
| compile failure | install aborts before the gate; /apps untouched |
| selftest mismatch | install aborts before the gate; /apps untouched |
| denied/timeout at gate | fail-fast deny; /apps untouched |
| power loss mid-install | journal replays: either the staging rename happened atomically or it didn't; no half-installed entry |

## 5. QEMU verification (DEMO 89/90)

Harness `run-semos-pkg-qemu.sh`, feature `pkg-test` (feeder task types the
script; serial answers the gates):

- boot 1 (mirror present at LBA 16): `semos update` clones the registry;
  `semos install motd` → resolver pulls `fortune` first, builds
  fortune+motd, selftest byte-exact, gate 'y', /apps/motd installed →
  `[DEMO 89] PASS`. Then `semos fetch cowsay` warms the cache.
  Hard kill.
- boot 2 (host zeroes LBA 0..8192 — mirror gone, journal intact): replay
  restores the clone + cache + /apps/motd; `semos install cowsay` runs
  entirely from cache → `[DEMO 90] PASS: offline install from local cache`.
  A `motd` smoke re-run proves the journaled install persisted.

## 6. What v1 deliberately does NOT build

- No version solver (single version per package per archive; `update`
  replaces the clone wholesale).
- No HTTPS fetch (v1.1, behind the same obtain-bytes seam).
- No dependency *binary* installs (deps are build-time libs).
- No signature verification on packages — the approval gate + tier fence is
  the v1 control; hash-pinning the index is the natural hardening (same
  lesson as the P-3 hash-bound approval gate in THESIS.md).
