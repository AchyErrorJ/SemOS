# Trusting the Machine

**A practical guide to living with AI-generated code, sovereign infrastructure, and portable systems.**

*Written September 11, 2026. Living document. Add lessons as they come.*

---

## Table of Contents

1. [The Core Problem](#the-core-problem)
2. [Code Trust Practices](#code-trust-practices)
3. [Dependency Hygiene](#dependency-hygiene)
4. [Infrastructure Sovereignty](#infrastructure-sovereignty)
5. [Portable Apps & Inference](#portable-apps--inference)
6. [Quick Reference](#quick-reference)

---

## The Core Problem

You don't trust the LLM to not insert spyware. But you also can't read millions of lines.

The fear isn't that the LLM grows a mustache and twirls it while inserting `send_passwords_to_china()`. The fear is **you can't hold the whole system in your head anymore** — and somewhere in those millions of lines, something you didn't write and didn't review is doing something you didn't intend.

**The solution:** Build a system where mistrust is the default. Trust the walls, not the robot.

---

## Code Trust Practices

### The EspaceBoreal Model Applied to Code

```
Agent (AI) writes code → Proposition (PR) → Inspecteur (static analysis + tests) → Human signs
```

**Key insight:** The human doesn't read every line. The human reads the **diff**, the **test results**, and the **behavioral contract.**

| Review Trigger | What to Look For |
|---------------|------------------|
| Network code | Any outbound calls, URL construction, unexpected domains |
| Crypto / auth | Key handling, password storage, token generation |
| File system | Paths outside working directory, writes to system locations |
| Process execution | `exec`, `spawn`, `system` calls |
| Dependency changes | New crates/packages being added |
| `unsafe` blocks | In Rust — memory safety bypasses |

### What AI Code Review Actually Looks Like

**Don't do this:**
- Read every line of a 10,000-line PR
- Trust the AI's explanation of what the code does
- Merge because "it looks fine"

**Do this:**
- Read the diff (GitHub/GitLab "Files changed" tab)
- Check: does this PR touch anything in the "scary categories" above?
- Run tests. If tests pass and the diff doesn't touch scary categories → sign.
- If it touches scary categories → read those files closely.

### Static Analysis as Inspecteur

Run these on every PR, before human review:

```bash
# Rust
cargo audit          # Known CVEs in dependencies
cargo geiger         # Unsafe Rust usage
cargo clippy -- -D warnings  # Linting as errors

# Python
bandit -r src/       # Security-focused linter
safety check         # Known vulnerabilities in deps

# General
# Behavioral sandbox: does this binary make unexpected network calls?
```

### Behavioral Sandbox Testing

The ultimate test: run the code and assert what it does NOT do.

```bash
# Example: run binary in network-restricted container
# If it tries to connect out, it fails immediately
docker run --network none --rm -v $(pwd):/app myimage /app/target/release/mybin

# Use strace to audit syscalls
strace -e trace=network,open,connect ./mybin
```

### Property-Based Testing

Don't test "does this function work?" Test "does this function NEVER do X?"

```rust
// Property: this function never makes network calls
#[test]
fn parse_config_never_calls_network() {
    // Run in sandbox, assert zero connect() syscalls
}
```

---

## Dependency Hygiene

### The Real Attack Vector

LLM outputs are observable in diffs. Dependencies pulled by `cargo add` or `npm install` are **invisible** — you never see the code. This is where the real risk lives.

### Tiered Defense

| Tier | Action | Effort | Impact |
|------|--------|--------|--------|
| 1 | Pin all versions (`Cargo.lock`, `package-lock.json`) | Low | Prevents surprise updates |
| 2 | Vendor critical dependencies (`cargo vendor`) | Medium | Source lives in your repo, reviewable |
| 3 | `cargo audit` in CI | Low | Catches known vulnerable deps |
| 4 | Reproducible builds (Nix, `flake.lock`) | High | Same inputs → same binary, bit-for-bit |
| 5 | Formal verification (Kani, Miri) | Very high | Mathematical proof for critical paths |

### Vendoring Dependencies

```bash
# Rust: vendor deps into your repo
cargo vendor > .cargo/config.toml

# Now deps are in vendor/ directory, committed to git
# Review them as you would any other code
```

### The Deterministic Build Test

```bash
# Build twice from same source
make clean && make > /tmp/build1.sha256
make clean && make > /tmp/build2.sha256

# If they differ, something is non-deterministic (or compromised)
diff /tmp/build1.sha256 /tmp/build2.sha256
```

---

## Infrastructure Sovereignty

### The "Second Internet" Problem

The internet's physical layer doesn't need AI. A Cisco router in 2026 works like it did in 1996. The danger is that AI has become the substrate of the **services** we depend on.

| Layer | AI Dependence | Sovereign Fallback |
|-------|--------------|-------------------|
| Physical fiber/radio | None | N/A — already sovereign |
| DNS | Increasing | Unbound, BIND — your own resolver |
| CDN / Edge | High | Your own edge (college server) |
| Auth / Identity | Very high | Cryptographic keys, not behavioral biometrics |
| Content / Search | Total | Git + local index, not embedding-based |
| Email / Messaging | Moderate | Self-hosted, rule-based filtering |

### The Kill-Switch Architecture

A bistable system:

```
Normal mode: AI-assisted, cloud-connected, optimized
    │
    ▼ (crisis / threat / decision)
Kill switch flipped
    │
    ▼
Fallback mode: AI removed, local inference stopped,
               services run on deterministic logic only
```

**Critical infrastructure should have a "dumb mode."**

### What to Actually Build

| Action | Effort | Why |
|--------|--------|-----|
| **SemOS as sovereign client** | High (in progress) | Local-first, no cloud auth needed |
| **EspaceBoreal + git as source of truth** | Medium | Documents don't need AI to exist |
| **Local inference (llama.cpp) as default** | Medium | AI on your hardware, not theirs |
| **Headscale instead of Tailscale commercial** | Low | Your mesh, your keys |
| **"Dumb" DNS resolver** | Low | Unbound or BIND |

### Community Mesh as Fallback

For when centralized stuff fails:

- **Tailscale/Headscale** — mesh overlay, encrypted, no AI gatekeeper
- **CJDNS / Yggdrasil** — true mesh routing, no ISPs
- **LoRa / Packet radio** — local-only, low bandwidth, works when nothing else does

The college can run its own mesh. Students' laptops become nodes.

---

## Portable Apps & Inference

### What "Portable" Means

Drop the file on any machine. Double-click. It runs. No installer. No admin. No dependencies to install.

### Windows Portable: The Go Approach

```bash
# On Linux dev machine:
GOOS=windows GOARCH=amd64 go build -ldflags="-s -w" -o myapp.exe

# Result: single ~10MB .exe
# Runs on any Windows 10/11 machine
# No runtime, no .NET, no Java, no Python
```

### Windows Portable: The Rust Approach

```bash
# Install Windows target
rustup target add x86_64-pc-windows-gnu
sudo apt-get install mingw-w64

# Build static binary (no glibc dependency)
cargo build --target x86_64-unknown-linux-musl --release

# Or for Windows:
cargo build --target x86_64-pc-windows-gnu --release
```

**Key Cargo.toml setting for portability:**

```toml
[dependencies]
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
# rustls-tls = pure Rust TLS, no system SSL libraries needed
```

### Portable App Directory Layout

```
MyPortableApp/
├── myapp.exe              # Single binary
├── config.env             # Settings (edit this)
├── start.bat              # Double-click this
├── data/                  # Logs, cache, state
│   └── audit.log
└── models/                # Optional: bundled AI models
    └── model.gguf
```

### GPU Without Admin

**Yes, it's possible.** If the machine has NVIDIA drivers installed (99% of machines with an NVIDIA GPU do), any user — even non-admin — can run CUDA applications.

**What needs admin:**
- Installing GPU drivers (already done on most machines)
- Installing CUDA Toolkit (full SDK)

**What doesn't need admin:**
- Running a CUDA app
- Bundling CUDA runtime DLLs with your app

**Bundle these next to your `.exe`:**

```
cuda_runtime/
├── cudart64_110.dll
├── cublas64_11.dll
├── cublasLt64_11.dll
└── ...
```

### Local Inference Options

| Approach | Portable? | GPU? | Size | Notes |
|----------|-----------|------|------|-------|
| Route to Ollama | ✅ Easy | ✅ Yes | ~0 (separate install) | User installs Ollama separately |
| llamafile (Mozilla) | ✅ Single .exe | ❌ CPU only | ~GB | Single file, zero install |
| CUDA llamafile | ✅ Single .exe | ✅ Yes | ~GB+ | Bundles CUDA runtime |
| Candle (Rust) | ✅ Single binary | ⚠️ Metal/CPU | ~10MB | Pure Rust, no C++ build hell |
| llama.cpp + bundled DLLs | ✅ Folder | ✅ Yes | ~50-100MB | Best performance, more setup |

### The Almanach-Orchestrator Inference Layer

The existing `almanach-orchestrator` already has:

- `NativeInferenceConfig` — built-in GGUF model support
- `ModelServers` — routes to Ollama / LM Studio / local backends
- `anthropic_proxy.rs` — translates between API formats

**For a portable inference harness, extract just these pieces** into a separate binary:

```
inference-harness/
├── src/
│   ├── main.rs              # OpenAI-compatible API server
│   ├── native_inference.rs  # GGUF loading via llama.cpp
│   ├── router.rs            # Route to local backends
│   └── proxy.rs             # API format translation
└── Cargo.toml
```

---

## Quick Reference

### Daily Habits

| Habit | Command / Action |
|-------|-----------------|
| Check for vulnerable deps | `cargo audit` |
| Check for unsafe Rust | `cargo geiger` |
| Review AI-generated diff | GitHub "Files changed" tab — look for network/crypto/auth changes |
| Run in sandbox | `docker run --network none ...` |
| Pin dependencies | Commit `Cargo.lock` / `package-lock.json` |

### One-Command CI Pipeline

```yaml
# .github/workflows/trust.yml
name: Trust Check
on: [pull_request]
jobs:
  trust:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo audit
      - run: cargo geiger --output lines
      - run: cargo test
      - run: cargo clippy -- -D warnings
```

### The "Scary Categories" Checklist

Before signing off on any PR that touches these, read the code closely:

- [ ] Network I/O (HTTP requests, socket creation)
- [ ] Cryptography (key handling, hashing, randomness)
- [ ] Authentication / authorization
- [ ] File system access outside working directory
- [ ] Process execution (`exec`, `spawn`, `system`)
- [ ] Dependency changes (new crates/packages)
- [ ] `unsafe` blocks (Rust)
- [ ] Environment variable reads (especially secrets)

### Emergency Kill-Switch Checklist

If you need to disconnect AI from your infrastructure NOW:

1. **Stop cloud inference calls** — rotate API keys or block at firewall
2. **Switch to local inference** — llama.cpp, Ollama, whatever's on hardware you own
3. **Disable AI-assisted tools** — Copilot, Cursor, etc.
4. **Run deterministic build** — build from known-good source, verify reproducibility
5. **Audit last N commits** — review diffs for anything touching network/auth/crypto
6. **Fallback to manual** — git operations, documentation, all the "boring" stuff works without AI

---

## The Bottom Line

You don't need to read millions of lines. You need:

1. **Diff-based review** — humans read changes, not entire files
2. **Automated gates** — static analysis catches the categories of bad things
3. **Behavioral sandboxes** — code proves what it does/doesn't do
4. **Deterministic builds** — same source → same binary, always
5. **A kill switch** — know how to fall back to deterministic, AI-free operation

**Trust the walls, not the robot.**

---

*"Don't worry. Even if the world forgets, I'll remember for you."* ❤️‍🔥
