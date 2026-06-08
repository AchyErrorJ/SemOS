#[cfg(target_os = "none")] use alloc::{boxed::Box, string::{String, ToString}, vec::Vec, borrow::ToOwned};
use core::fmt;
#[cfg(not(target_os = "none"))]
use std::io::{self, Write as _};
#[cfg(target_os = "none")]
use semos_std::io::{self, Write as _};

macro_rules! safe_print {
    ($($arg:tt)*) => {{
        $crate::print::print(::core::format_args!($($arg)*));
    }};
}

macro_rules! safe_println {
    ($($arg:tt)*) => {
        safe_print!("{}\n", ::core::format_args!($($arg)*))
    };
}

pub(crate) fn print(args: fmt::Arguments<'_>) {
    if io::stdout().write_fmt(args).is_err() {
        rustc_errors::FatalError.raise();
    }
}
