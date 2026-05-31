// M27 §1.8 + R4 B5: this file used to import std::backtrace::Backtrace,
// std::path::{Path, PathBuf}, std::process::ExitStatus, std::error::Error,
// std::ffi::CString, std::num::ParseIntError, and std::io::Error directly.
// On the SemOS target most of these are either in core/alloc, in
// semos_std, or absent. We cfg-split per RECIPE §1.3.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use core::num::ParseIntError;

// `Backtrace` is std-only. Mirror the rustc_errors approach: a local
// shim type on the SemOS target that always reports "unsupported".
#[cfg(not(target_os = "none"))]
use std::backtrace::Backtrace;

#[cfg(target_os = "none")]
struct Backtrace;
#[cfg(target_os = "none")]
impl fmt::Display for Backtrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(no backtrace available on SemOS)")
    }
}

// `Path`/`PathBuf` cfg-split per RECIPE §1.3.
#[cfg(not(target_os = "none"))]
use std::path::{Path, PathBuf};
#[cfg(target_os = "none")]
use semos_std::path::{Path, PathBuf};

// `ExitStatus` is std::process::* on the host; SemOS doesn't model the
// std ExitStatus, so we declare a local unit type that's structurally a
// stand-in. None of the SemOS-target call paths construct or read this.
#[cfg(not(target_os = "none"))]
use std::process::ExitStatus;

#[cfg(target_os = "none")]
struct ExitStatus;
#[cfg(target_os = "none")]
impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("exit status (semos)")
    }
}

// `std::io::Error` cfg-split.
#[cfg(not(target_os = "none"))]
use std::io;
#[cfg(target_os = "none")]
use semos_std::io;

// `CString` lives in `alloc::ffi::CString` since 1.64 stable.
use alloc::ffi::CString;

use rustc_ast as ast;
use rustc_ast_pretty::pprust;
use rustc_span::edition::Edition;

use crate::{DiagArgValue, IntoDiagArg, __IntoDiagArgPathBuf};

pub struct DiagArgFromDisplay<'a>(pub &'a dyn fmt::Display);

impl IntoDiagArg for DiagArgFromDisplay<'_> {
    fn into_diag_arg(self, path: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        self.0.to_string().into_diag_arg(path)
    }
}

impl<'a> From<&'a dyn fmt::Display> for DiagArgFromDisplay<'a> {
    fn from(t: &'a dyn fmt::Display) -> Self {
        DiagArgFromDisplay(t)
    }
}

impl<'a, T: fmt::Display> From<&'a T> for DiagArgFromDisplay<'a> {
    fn from(t: &'a T) -> Self {
        DiagArgFromDisplay(t)
    }
}

impl<'a, T: Clone + IntoDiagArg> IntoDiagArg for &'a T {
    fn into_diag_arg(self, path: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        self.clone().into_diag_arg(path)
    }
}

#[macro_export]
macro_rules! into_diag_arg_using_display {
    ($( $ty:ty ),+ $(,)?) => {
        $(
            impl $crate::IntoDiagArg for $ty {
                fn into_diag_arg(
                    self,
                    path: &mut Option<$crate::__IntoDiagArgPathBuf>,
                ) -> $crate::DiagArgValue {
                    self.to_string().into_diag_arg(path)
                }
            }
        )+
    }
}

macro_rules! into_diag_arg_for_number {
    ($( $ty:ty ),+ $(,)?) => {
        $(
            impl $crate::IntoDiagArg for $ty {
                fn into_diag_arg(
                    self,
                    path: &mut Option<$crate::__IntoDiagArgPathBuf>,
                ) -> $crate::DiagArgValue {
                    // Convert to a string if it won't fit into `Number`.
                    #[allow(irrefutable_let_patterns)]
                    if let Ok(n) = TryInto::<i32>::try_into(self) {
                        $crate::DiagArgValue::Number(n)
                    } else {
                        self.to_string().into_diag_arg(path)
                    }
                }
            }
        )+
    }
}

into_diag_arg_using_display!(
    ast::ParamKindOrd,
    io::Error,
    Box<dyn core::error::Error>,
    core::num::NonZero<u32>,
    Edition,
    rustc_span::Ident,
    rustc_span::MacroRulesNormalizedIdent,
    ParseIntError,
    ExitStatus,
);

into_diag_arg_for_number!(i8, u8, i16, u16, i32, u32, i64, u64, i128, u128, isize, usize);

impl IntoDiagArg for bool {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        if self {
            DiagArgValue::Str(Cow::Borrowed("true"))
        } else {
            DiagArgValue::Str(Cow::Borrowed("false"))
        }
    }
}

impl IntoDiagArg for char {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::Str(Cow::Owned(format!("{self:?}")))
    }
}

impl IntoDiagArg for Vec<char> {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::StrListSepByAnd(
            self.into_iter().map(|c| Cow::Owned(format!("{c:?}"))).collect(),
        )
    }
}

impl IntoDiagArg for rustc_span::Symbol {
    fn into_diag_arg(self, path: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        self.to_ident_string().into_diag_arg(path)
    }
}

impl<'a> IntoDiagArg for &'a str {
    fn into_diag_arg(self, path: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        self.to_string().into_diag_arg(path)
    }
}

impl IntoDiagArg for String {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::Str(Cow::Owned(self))
    }
}

impl<'a> IntoDiagArg for Cow<'a, str> {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::Str(Cow::Owned(self.into_owned()))
    }
}

impl<'a> IntoDiagArg for &'a Path {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        // M27 R4 B5: semos_std::path::Path lacks `.display()` returning a
        // `Display` wrapper on every API revision. Both host (std::path
        // since 1.0) and SemOS (added in Phase 4.5, commit de8aff3) now
        // expose `.display()` returning a Display-impl wrapper.
        DiagArgValue::Str(Cow::Owned(self.display().to_string()))
    }
}

impl IntoDiagArg for PathBuf {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::Str(Cow::Owned(self.display().to_string()))
    }
}

impl IntoDiagArg for ast::Expr {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::Str(Cow::Owned(pprust::expr_to_string(&self)))
    }
}

impl IntoDiagArg for ast::Path {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::Str(Cow::Owned(pprust::path_to_string(&self)))
    }
}

impl IntoDiagArg for ast::token::Token {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::Str(pprust::token_to_string(&self))
    }
}

impl IntoDiagArg for ast::token::TokenKind {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::Str(pprust::token_kind_to_string(&self))
    }
}

impl IntoDiagArg for CString {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::Str(Cow::Owned(self.to_string_lossy().into_owned()))
    }
}

impl IntoDiagArg for rustc_data_structures::small_c_str::SmallCStr {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::Str(Cow::Owned(self.to_string_lossy().into_owned()))
    }
}

impl IntoDiagArg for ast::Visibility {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        let s = pprust::vis_to_string(&self);
        let s = s.trim_end().to_string();
        DiagArgValue::Str(Cow::Owned(s))
    }
}

impl IntoDiagArg for Backtrace {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::Str(Cow::from(self.to_string()))
    }
}

impl IntoDiagArg for ast::util::parser::ExprPrecedence {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::Number(self as i32)
    }
}

impl IntoDiagArg for ast::FloatTy {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        DiagArgValue::Str(Cow::Borrowed(self.name_str()))
    }
}
