//! Bootloader Runner
//!
//! This program creates a bootable disk image from the kernel binary.
//!
//! Usage: cargo run --release

use std::path::PathBuf;

fn main() {
    // Get the kernel binary path from command line or default
    let kernel_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let manifest_dir = env!("CARGO_MANIFEST_DIR");
            PathBuf::from(manifest_dir)
                .parent()
                .unwrap()
                .join("kernel-x86_64")
                .join("target")
                .join("x86_64-unknown-none")
                .join("release")
                .join("semantic-os-x86_64")
        });

    println!("Kernel path: {}", kernel_path.display());

    if !kernel_path.exists() {
        eprintln!("Error: Kernel binary not found at {}", kernel_path.display());
        eprintln!("Build the kernel first with: cd kernel-x86_64 && cargo build --release");
        std::process::exit(1);
    }

    // Create UEFI and BIOS disk images
    let uefi_path = kernel_path.with_extension("img");
    let bios_path = kernel_path.with_file_name("semantic-os-x86_64-bios.img");

    println!("Creating UEFI disk image: {}", uefi_path.display());
    bootloader::UefiBoot::new(&kernel_path)
        .create_disk_image(&uefi_path)
        .expect("Failed to create UEFI disk image");
    println!("UEFI image created: {}", uefi_path.display());

    println!("Creating BIOS disk image: {}", bios_path.display());
    bootloader::BiosBoot::new(&kernel_path)
        .create_disk_image(&bios_path)
        .expect("Failed to create BIOS disk image");
    println!("BIOS image created: {}", bios_path.display());

    println!();
    println!("Run with QEMU:");
    println!("  BIOS: qemu-system-x86_64 -drive format=raw,file={} -serial stdio", bios_path.display());
}
