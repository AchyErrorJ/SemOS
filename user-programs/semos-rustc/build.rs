// Wires the linker script into rustc with an absolute path so cargo can be
// invoked from anywhere. Mirrors semos-cc / hello-std so the binary lands
// as ET_EXEC at USER_CODE_BASE (0x400000), not PIE.

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let link_script = format!("{manifest_dir}/link.ld");

    println!("cargo:rerun-if-changed={link_script}");
    println!("cargo:rerun-if-changed=build.rs");

    println!("cargo:rustc-link-arg=-T{link_script}");
    println!("cargo:rustc-link-arg=-no-pie");
}
