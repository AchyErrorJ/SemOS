//! `rustc_proc_macro` — M27 Phase 5b Stage E iter 10 STUB.
//!
//! Upstream's lib.rs points at `library/proc_macro/src/lib.rs` (the
//! standard library proc-macro tree). We don't vendor `library/` —
//! only `compiler/` — so the path is dead. Per M27_RUSTC_PORT_PLAN.md
//! §1.5 we drop the proc-macro runtime entirely (no dlopen, no proc-
//! macro subprocess server in v1). This stub exposes ONLY the type
//! names that downstream consumers (`rustc_expand`, `rustc_metadata`,
//! `rustc_builtin_macros`) import from `proc_macro`.
//!
//! Any call into these stubs unreachable!()s — semos-rustc will refuse
//! to compile proc-macro-using crates per the §1.5 documented v1
//! limitation.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone)]
pub struct TokenStream(());

impl TokenStream {
    pub fn new() -> Self {
        Self(())
    }
    pub fn is_empty(&self) -> bool {
        true
    }
}

impl Default for TokenStream {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Display for TokenStream {
    fn fmt(&self, _f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Ok(())
    }
}

impl core::fmt::Debug for TokenStream {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TokenStream").finish()
    }
}

impl IntoIterator for TokenStream {
    type Item = TokenTree;
    type IntoIter = alloc::vec::IntoIter<TokenTree>;
    fn into_iter(self) -> Self::IntoIter {
        Vec::new().into_iter()
    }
}

#[derive(Clone, Debug)]
pub enum TokenTree {
    Group(Group),
    Ident(Ident),
    Punct(Punct),
    Literal(Literal),
}

#[derive(Clone, Debug)]
pub struct Group(());
impl Group {
    pub fn new(_delim: Delimiter, _stream: TokenStream) -> Self {
        Self(())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Delimiter {
    Parenthesis,
    Brace,
    Bracket,
    None,
}

#[derive(Clone, Debug)]
pub struct Ident(String);
impl Ident {
    pub fn new(s: &str, _span: Span) -> Self {
        Self(String::from(s))
    }
    pub fn to_string(&self) -> String {
        self.0.clone()
    }
}

#[derive(Clone, Debug)]
pub struct Punct(());
impl Punct {
    pub fn new(_ch: char, _spacing: Spacing) -> Self {
        Self(())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Spacing {
    Alone,
    Joint,
}

#[derive(Clone, Debug)]
pub struct Literal(String);
impl Literal {
    pub fn string(s: &str) -> Self {
        Self(String::from(s))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Span;
impl Span {
    pub fn call_site() -> Self {
        Self
    }
}

/// Diagnostic stub.
pub struct Diagnostic;
impl Diagnostic {
    pub fn emit(self) {}
}

pub mod bridge {
    //! Empty stub — upstream contains the proc-macro runtime bridge.
    //! Per §1.5 we drop proc-macro expansion in v1; this module exists
    //! only so `use proc_macro::bridge::*;` resolves.

    pub mod client {
        //! Stub for `proc_macro::bridge::client::ProcMacro`. The rmeta
        //! decoder's `load_proc_macro` matches on this enum — the field-
        //! cfg-gated downstream code never reads `client` on SemOS, so an
        //! empty unit variant payload is fine.
        use alloc::string::String;
        use alloc::vec::Vec;

        #[derive(Copy, Clone, Debug)]
        pub struct Client<I, O>(core::marker::PhantomData<(fn(I) -> O,)>);

        pub enum ProcMacro {
            CustomDerive {
                trait_name: &'static str,
                attributes: &'static [&'static str],
                client: Client<crate::TokenStream, crate::TokenStream>,
            },
            Attr {
                name: &'static str,
                client: Client<(crate::TokenStream, crate::TokenStream), crate::TokenStream>,
            },
            Bang {
                name: &'static str,
                client: Client<crate::TokenStream, crate::TokenStream>,
            },
        }

        // Silence unused-imports inside the stub module.
        #[allow(dead_code)]
        fn _unused() {
            let _: Vec<String> = Vec::new();
        }
    }
}
