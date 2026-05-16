# smoltcp Vendoring Brief

**Date:** 2026-05-16
**Status:** scoping document; integration not yet started
**Companion to:** `docs/PHASE_8_ROADMAP.md` (sections "Track B" and "Suggested first concrete step")

Source: agent brief produced 2026-05-16 as part of Phase 8 planning. Saved
here because the agent task output files are session-scoped and would
otherwise be lost.

---

## 1. smoltcp at a glance

- **Crate**: `smoltcp` on crates.io. Repo: `https://github.com/smoltcp-rs/smoltcp`. Originally written by whitequark for redox-os/firmware use, now community-maintained.
- **Version to target**: `0.11.x` (released 2024) is the most widely deployed line; `0.12.x` exists in the repo with API churn around the `Interface` constructor. Recommend pinning to **0.11.0**; confirm latest patch version at vendor time.
- **License**: 0BSD (zero-clause BSD). Compatible with anything; no attribution required.
- **Architecture**: a *sans-IO* stack. Core types: `phy::Device`, `iface::Interface`, `iface::SocketSet`, per-socket types (`tcp::Socket`, `udp::Socket`, `dhcpv4::Socket`, `dns::Socket`, ...).
- **What it gives us**: link-layer Ethernet framing, ARP, IPv4 (frag/reass optional), ICMPv4, TCP (with congestion control), UDP, DHCPv4 client, DNS client.
- **What it does not give us**: TLS, HTTP, async runtime, threads, allocator.

## 2. no_std and alloc story

- smoltcp is `#![no_std]` by default. Use `default-features = false`.
- **`socket-tcp` does NOT require alloc.** Buffers are `tcp::SocketBuffer<'a>` wrapping `ManagedSlice<'a, u8>`. Build from `&mut [u8]` (the `Borrowed` variant).
- Pattern: `static mut RX: [u8; N] = [0; N];` and pass `&mut RX[..]`.
- `socket-dhcpv4`, `socket-dns`, `SocketSet` all follow the same borrowed-buffer pattern.
- **Conclusion**: full target feature set is buildable with `alloc` disabled.

## 3. Dependency tree (minimum useful set)

At features `["medium-ethernet", "proto-ipv4", "socket-tcp"]`:

- `managed` (≈ 0.8) — the `ManagedSlice<'a, T>` and `ManagedMap` types; core-only. ~400 LOC. Load-bearing for no-alloc.
- `byteorder` (1.x) — header parsing.
- `log` (0.4) — only if `log` feature is on; facade only.
- `bitflags` (≈ 2.x) — protocol header bit-fields.
- `cfg-if` — trivial.

Total transitive crate count at our minimum set: **≈5 crates** beyond `core`. Pure-Rust.

## 4. Integration with our `NetDevice` trait

### smoltcp's `phy::Device`

```rust
pub trait Device {
    type RxToken<'a>: RxToken where Self: 'a;
    type TxToken<'a>: TxToken where Self: 'a;
    fn receive(&mut self, timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)>;
    fn transmit(&mut self, timestamp: Instant) -> Option<Self::TxToken<'_>>;
    fn capabilities(&self) -> DeviceCapabilities;
}
pub trait RxToken { fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R; }
pub trait TxToken { fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R; }
```

### NetDevice → phy::Device adapter sketch

A ~80 LOC adapter wraps our `kernel-core::drivers::traits::NetDevice` to satisfy `phy::Device`. Two scratch buffers (rx + tx), `RxToken` and `TxToken` are thin shims that defer to `NetDevice::send`/`recv`. Two awkwardnesses: (a) trait methods take `&self` (interior mutability — fine), (b) borrow checker on returning two `&mut` from one struct — fix with `UnsafeCell` and disjoint indices (~10 extra LOC).

### Polling model

`iface.poll(now, &mut device, &mut sockets) -> bool`. Recommended loop runs the poll on a scheduler tick (≤10 ms granularity) and honours `poll_delay` for sleeping.

## 5. Memory model

Buffers are **per-socket**. For one TCP connection (TLS to api.anthropic.com):

| Item | Size |
|---|---|
| TCP rx buffer | 16 KiB (fits a max TLS record) |
| TCP tx buffer | 8 KiB |
| `tcp::Socket` metadata | ~400 B |
| `SocketSet` storage (4 slots) | ~128 B |
| Interface state (neighbour cache, routing, ARP/IP config) | ~1 KiB |
| DNS socket | ~2 KiB |
| DHCP socket | ~1 KiB |

**Total: ≈30 KiB BSS**, all `'static`. Same `unsafe { &mut STATIC }` pattern as `VRING` in `virtio/block.rs`.

## 6. What we keep, what we replace

Recommended feature flags:

```toml
[dependencies.smoltcp]
version = "=0.11.0"   # pin exactly
default-features = false
features = [
  "medium-ethernet",
  "proto-ipv4",
  "proto-dhcpv4",     # only if QEMU/lab has DHCP; else drop and hardcode
  "proto-dns",
  "socket-tcp",
  "socket-dhcpv4",
  "socket-dns",
]
```

Explicitly off: `proto-ipv6`, `proto-igmp`, `proto-ipv4-fragmentation`, `socket-raw`, `socket-icmp`, `socket-udp` (DNS brings its own).

If DHCP undesirable, drop those two features and call `Interface::update_ip_addrs` with hardcoded `IpCidr` + `routes_mut().add_default_ipv4_route(gateway)`.

## 7. TLS layering

smoltcp's `tcp::Socket` exposes `send_slice`, `recv_slice`, `can_send`, `can_recv`. It does **not** implement `embedded_io::Read + Write` natively in 0.11.

embedded-tls wants `embedded_io::Read + Write` (sync) or `embedded_io_async::Read + Write` (async). The adapter is a thin **TcpStream wrapper** (~30 LOC) that loops on `iface.poll` and calls `send_slice`/`recv_slice`.

## 8. Recommended path

1. **Vendor** `smoltcp` at the pinned tag `v0.11.0` into `F:\Software\ArmKernel3\third_party\smoltcp\`. Also vendor `managed`, `byteorder`, `bitflags`, `cfg-if`, `log`.
2. **Workspace patch**: `[patch.crates-io] smoltcp = { path = "third_party/smoltcp" }` in root `Cargo.toml`.
3. **New module `kernel-core/src/net/`** with:
   - `adapter.rs` — `NetDeviceAdapter` (≈80 LOC).
   - `clock.rs` — `instant_now()` (≈20 LOC).
   - `state.rs` — `static mut NET_STATE: NetState`, `init()`, `poll()` (≈150 LOC).
   - `tcp.rs` — `tcp_connect(host_ip, port)` + `TcpStream` `embedded_io` impl (≈150 LOC).
   - `dns.rs` — `resolve(name)` (≈80 LOC).
4. **Boot wiring** (in `kernel-x86_64/src/main.rs`): after virtio-net registers, call `kernel_core::net::init(registry::get_net("virtio-net0").unwrap())`. Spawn a tick that calls `net::poll()` every scheduler quantum.
5. **Smoke test**: `tcp_connect(192.0.2.1, 80)` against QEMU user-mode netdev (`-netdev user,hostfwd=...`) — first milestone is SYN out / SYN-ACK in.

**Total integration LOC estimate: ~500 LOC of glue.**

## 9. Pitfalls

- **Poll cadence**: ≤10 ms or honour `poll_delay`. 100 ms means slow TCP retransmit.
- **`Instant` source**: monotonic only; never RTC. Cast our `TimerDevice::now_ns` to `i64` ms.
- **Socket state races**: don't hold `&mut tcp::Socket` across a `poll` call.
- **DHCP socket lifetime**: apply `Event::Configured` to `Interface` via `update_ip_addrs` + `add_default_ipv4_route`.
- **MTU and `max_burst_size`**: pin to `Some(1)` since we're copy-based.
- **Checksum offload**: disable (`ChecksumCapabilities::default()` = software).
- **Endianness**: smoltcp uses `byteorder::NetworkEndian` consistently.
- **`SocketHandle`**: just a `usize`. Don't leak across `SocketSet` rebuilds.

## 10. Open questions

1. Version pin: `0.11.0` vs latest 0.11 patch; review `CHANGELOG.md`.
2. `embedded_io` v0.5 vs v0.6 trait shapes — coordinate with embedded-tls.
3. Does smoltcp 0.11 expose `embedded_io::{Read,Write}` on `tcp::Socket` behind a feature flag?
4. Sensible defaults for `set_nagle_enabled`, `set_keep_alive` for TLS-over-WAN.
5. MSS: virtio-net MTU 1500 → IPv4 1480 → TCP MSS 1460. Confirm.
6. DHCP behaviour under QEMU SLIRP (fine) vs bridged-tap (may not have one).
7. Does our `NetDevice::recv` ever return partial Ethernet frames? (No — virtqueue delivers whole frames.)
8. Thread safety: smoltcp types are `!Send`. Wrap `NET_STATE` in `spin::Mutex` if future async moves it.
9. Logging volume: `net_trace!` is per-packet chatty; default `log::set_max_level` to `Info`.
10. Binary size: rough estimate +120–180 KiB `.text`, +35 KiB BSS. Measure after first integration build.

---

**Bottom line**: smoltcp v0.11.0, no_std, no_alloc, ~500 LOC of glue, ~35 KiB static buffers, 0BSD. Genuine integration risk is poll-cadence/timing rather than API mismatch.
