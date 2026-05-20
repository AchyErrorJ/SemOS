// Wires the linker script into rustc with an absolute path so cargo can be
// invoked from anywhere. Also tells cargo to rerun this when the script
// changes.

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let link_script = format!("{manifest_dir}/link.ld");

    println!("cargo:rerun-if-changed={link_script}");
    println!("cargo:rerun-if-changed=build.rs");

    // -T <script> selects our linker script.
    // -no-pie keeps the binary as ET_EXEC at the script-defined load addr,
    // matching the kernel's USER_CODE_BASE (0x400000) and the ELF loader's
    // non-PIE path.
    println!("cargo:rustc-link-arg=-T{link_script}");
    println!("cargo:rustc-link-arg=-no-pie");
}
