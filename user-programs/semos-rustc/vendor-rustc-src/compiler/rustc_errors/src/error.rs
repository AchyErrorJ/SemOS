// M27 §1.8: i18n / fluent dropped. With no fluent backend, translation
// errors cannot occur — every DiagMessage is either a Str (returned as-is)
// or a FluentIdentifier (returned as the identifier string itself). This
// shim keeps the `TranslateError` type for API compatibility with
// downstream callers in this crate, but its variants are never constructed.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::error::Error;
use core::fmt;

use rustc_error_messages::{FluentArgs, FluentError};

#[derive(Debug)]
pub enum TranslateError<'args> {
    One {
        id: &'args Cow<'args, str>,
        args: &'args FluentArgs<'args>,
        kind: TranslateErrorKind<'args>,
    },
    Two {
        primary: Box<TranslateError<'args>>,
        fallback: Box<TranslateError<'args>>,
    },
}

impl<'args> TranslateError<'args> {
    pub fn message(id: &'args Cow<'args, str>, args: &'args FluentArgs<'args>) -> Self {
        Self::One { id, args, kind: TranslateErrorKind::MessageMissing }
    }

    pub fn primary(id: &'args Cow<'args, str>, args: &'args FluentArgs<'args>) -> Self {
        Self::One { id, args, kind: TranslateErrorKind::PrimaryBundleMissing }
    }

    pub fn attribute(
        id: &'args Cow<'args, str>,
        args: &'args FluentArgs<'args>,
        attr: &'args str,
    ) -> Self {
        Self::One { id, args, kind: TranslateErrorKind::AttributeMissing { attr } }
    }

    pub fn value(id: &'args Cow<'args, str>, args: &'args FluentArgs<'args>) -> Self {
        Self::One { id, args, kind: TranslateErrorKind::ValueMissing }
    }

    pub fn fluent(
        id: &'args Cow<'args, str>,
        args: &'args FluentArgs<'args>,
        errs: Vec<FluentError>,
    ) -> Self {
        Self::One { id, args, kind: TranslateErrorKind::Fluent { errs } }
    }

    pub fn and(self, fallback: TranslateError<'args>) -> TranslateError<'args> {
        Self::Two { primary: Box::new(self), fallback: Box::new(fallback) }
    }
}

#[derive(Debug)]
pub enum TranslateErrorKind<'args> {
    MessageMissing,
    PrimaryBundleMissing,
    AttributeMissing { attr: &'args str },
    ValueMissing,
    Fluent { errs: Vec<FluentError> },
}

impl fmt::Display for TranslateError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // M27 §1.8: simplified display — the rich fluent-error introspection
        // (argument reference suggestions, etc.) is dropped along with i18n.
        match self {
            Self::One { id, kind, .. } => match kind {
                TranslateErrorKind::MessageMissing => {
                    write!(f, "fluent message `{id}` was missing")
                }
                TranslateErrorKind::PrimaryBundleMissing => {
                    write!(f, "the primary fluent bundle was missing")
                }
                TranslateErrorKind::AttributeMissing { attr } => {
                    write!(f, "fluent message `{id}` was missing attribute `{attr}`")
                }
                TranslateErrorKind::ValueMissing => {
                    write!(f, "fluent message `{id}` was missing its value")
                }
                TranslateErrorKind::Fluent { .. } => {
                    write!(f, "fluent formatting of `{id}` failed")
                }
            },
            Self::Two { primary, fallback } => {
                write!(f, "translation error: primary={primary}; fallback={fallback}")
            }
        }
    }
}

impl Error for TranslateError<'_> {}
