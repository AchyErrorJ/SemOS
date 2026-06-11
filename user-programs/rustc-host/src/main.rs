// M27 DEMO 80 option B — feasibility probe.
// Step 1: confirm the foundational vendored crate builds for the HOST target
// (the cfg(not(target_os="none")) paths, which have only ever been exercised
// implicitly). If this links, we expand the deps toward rustc_driver_impl and
// turn this into a real host rustc driver.
fn main() {
    let mut m: rustc_data_structures::fx::FxHashMap<u32, u32> = Default::default();
    m.insert(1, 2);
    println!("rustc_data_structures host build OK ({} entries)", m.len());
}
