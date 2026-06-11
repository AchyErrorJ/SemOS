//! The `rustc_ast_passes` crate contains passes which validate the AST in `syntax`
//! parsed by `rustc_parse` and then lowered, after the passes in this crate,
//! by `rustc_ast_lowering`.

// M27 Phase 3 C2: rustc_ast_passes runs against semos_std (no full std)
// on the SemOS-host build. `#![no_std]` MUST be the first inner attribute,
// before the `#![feature(...)]` block (per A2-followup / B1 lib.rs lesson).
#![cfg_attr(target_os = "none", no_std)]
// tidy-alphabetical-start
#![feature(box_patterns)]
#![feature(if_let_guard)]
#![feature(iter_is_partitioned)]
// tidy-alphabetical-end

// M27 Phase 3 C2: alloc prelude — provides Vec/String/Box/format!/vec!
// crate-wide. The `#[macro_use]` reaches vec!/format! into submodules
// without per-file imports.
#[macro_use]
extern crate alloc;

pub mod ast_validation;
mod errors;
pub mod feature_gate;

rustc_fluent_macro::fluent_messages! { "../messages.ftl" }
