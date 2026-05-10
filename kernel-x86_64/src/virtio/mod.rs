//! VirtIO device drivers.
//!
//! Currently implements only the legacy (PCI 0.9.5 / VirtIO 1.0 §4.1.4)
//! block device on x86_64. Modern (1.1+) MMIO/PCI capabilities not
//! supported. Sufficient for QEMU's `-drive ...,if=virtio` default,
//! which advertises the legacy interface at `0x1AF4:0x1001`.

pub mod block;
