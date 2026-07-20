# sheaf — Phase 0 prototype

Userland prototype of the Sheaf bundle filesystem (see `docs/SHEAF_PLAN.md`).
Dependency-free std Rust so it builds offline in this repo's sandbox: TOML
(subset), SHA-256, tar, and SUID minting are all hand-rolled in `src/`.

## Build / test

```
cd sheaf
cargo build
cargo test
```

## CLI

```
sheaf new <bundle-dir> [title]           # mint a bundle (content.md default facet)
sheaf info|lint|verify|open|edit <dir>   # inspect / validate / render default facet
sheaf add <dir> <src> <facet> [role]     # add a facet (auto blob sidecar + hash)
sheaf rm  <dir> <facet>                  # remove a facet
sheaf export <dir> md|sheaf <out>        # flat-md or .sheaf (uncompressed ustar) projection
sheaf import <archive.sheaf> <dest-dir>  # copy-import: new SUID + derived_from
sheaf agent <dir> <name>                 # show an .agent profile's requested grants (dry-run)
sheaf pack <folder> [title]              # convert a loose folder into a bundle in place
sheaf repair <dir>                       # resync manifest to reality (hashes, loose files, sidecars)
sheaf find <root> [--contents]           # traversal with the bundle "readdir lie"
```

`sheaf find` reports a bundle once as a single `bundle` entry and does not
descend into it unless `--contents` is passed — the CLI twin of macOS's
package bit (see `docs/SHEAF_PLAN.md` §13).

## Scope / deviations (Phase 0)

- `.agent` is an optional **text leaf** with `role = "agent"` (not a third leaf
  kind); `sheaf agent` only prints requested tools/tier — no LLM execution.
- `.sheaf` export is an **uncompressed** POSIX ustar archive (still lossless);
  gzip is deferred until a compression dep is permitted.
- Timestamps are stored as `"<unix-seconds>Z"` pending a real RFC3339 formatter.
- PDF export is out of scope for Phase 0.

Acceptance tests in `tests/acceptance.rs` map to `docs/SHEAF_PLAN.md` §10.
