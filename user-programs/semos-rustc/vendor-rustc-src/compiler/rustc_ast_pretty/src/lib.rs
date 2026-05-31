// M27 Phase 3 C2: rustc_ast_pretty runs against semos_std (no full std)
// on the SemOS-host build. `#![no_std]` MUST be the first inner attribute,
// before the `#![feature(...)]` block (per A2-followup / B1 lib.rs lesson).
#![no_std]
// tidy-alphabetical-start
#![feature(box_patterns)]
#![feature(negative_impls)]
// tidy-alphabetical-end

// M27 Phase 3 C2: alloc prelude — provides Vec/String/Box/format!/vec!
// crate-wide. The `#[macro_use]` reaches vec!/format! into submodules
// without per-file imports.
#[macro_use]
extern crate alloc;

mod helpers;
pub mod pp;
pub mod pprust;
