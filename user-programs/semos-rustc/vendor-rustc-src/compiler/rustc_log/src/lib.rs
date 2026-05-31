//! This crate allows tools to enable rust logging without having to magically
//! match rustc's tracing crate version.
//!
//! For example if someone is working on rustc_ast and wants to write some
//! minimal code against it to run in a debugger, with access to the `debug!`
//! logs emitted by rustc_ast, that can be done by writing:
//!
//! ```toml
//! [dependencies]
//! rustc_ast = { path = "../rust/compiler/rustc_ast" }
//! rustc_log = { path = "../rust/compiler/rustc_log" }
//! ```
//!
//! ```ignore
//! fn main() {
//!     rustc_log::init_logger(rustc_log::LoggerConfig::from_env("LOG")).unwrap();
//!     /* ... */
//! }
//! ```
//!
//! Now `LOG=debug cargo +nightly run` will run your minimal main.rs and show
//! rustc's debug logging. In a workflow like this, one might also add
//! `std::env::set_var("LOG", "debug")` to the top of main so that `cargo
//! +nightly run` by itself is sufficient to get logs.
//!
//! The reason rustc_log is a tiny separate crate, as opposed to exposing the
//! same things in rustc_driver only, is to enable the above workflow. If you
//! had to depend on rustc_driver in order to turn on rustc's debug logs, that's
//! an enormously bigger dependency tree; every change you make to rustc_ast (or
//! whichever piece of the compiler you are interested in) would involve
//! rebuilding all the rest of rustc up to rustc_driver in order to run your
//! main.rs. Whereas by depending only on rustc_log and the few crates you are
//! debugging, you can make changes inside those crates and quickly run main.rs
//! to read the debug logs.

#![no_std]

#[macro_use]
extern crate alloc;

// M27 R3: tracing port pending
// ------------------------------------------------------------------
// The full upstream rustc_log is essentially a tracing-subscriber
// configuration shim. Per the assigned-crates note: "tracing is an
// external R3 flagged as PATCH. Mark tracing-using sites `// M27 R3:
// tracing port pending` — the crate may end up a stub."
//
// This file is the stub. Public API surface (LoggerConfig,
// init_logger, init_logger_with_additional_layer, Error,
// stdout_isatty, stderr_isatty, BuildSubscriberRet) is preserved so
// downstream callers (rustc_driver_impl in particular) keep type-
// checking. Bodies that touch tracing-subscriber on the SemOS target
// are replaced with no-op returns. Bodies that read env vars are
// preserved (env::var has a semos-std implementation tracked in R2's
// top-5 list at priority 3).
//
// Resolution when the externals queue catches up:
//   - tracing + tracing-core: no_std patch, mostly mechanical.
//   - tracing-subscriber: harder — pulls thread-local and ANSI bits.
//     R2 lists thread_local as priority 2 for semos-std.
//   - tracing-tree: depends on tracing-subscriber.
// After those land, replace the cfg(target_os = "none") stub branches
// with the real upstream bodies (kept inline below behind the not-none
// cfg for host-target builds + side-by-side reference).
// ------------------------------------------------------------------

use alloc::format;
use alloc::string::String;

// M27 R3: tracing port pending — host-target use sites kept intact so
// the crate still works on `cargo check` against std (useful for the
// build-deps that use rustc_log indirectly during the meta-crate
// codegen phase).
#[cfg(not(target_os = "none"))]
use std::env::{self, VarError};
#[cfg(not(target_os = "none"))]
use std::fmt::{self, Display};
#[cfg(not(target_os = "none"))]
use std::io::{self, IsTerminal};

// M27 R3: tracing port pending — SemOS target uses semos_std::env.
// env::var is on the R2 top-5 semos-std additions list (priority 3).
#[cfg(target_os = "none")]
use semos_std::env::{self, VarError};
#[cfg(target_os = "none")]
use core::fmt::{self, Display};

#[cfg(not(target_os = "none"))]
use tracing::dispatcher::SetGlobalDefaultError;
#[cfg(not(target_os = "none"))]
use tracing::{Event, Subscriber};
#[cfg(not(target_os = "none"))]
use tracing_subscriber::Registry;
#[cfg(not(target_os = "none"))]
use tracing_subscriber::filter::{Directive, EnvFilter, LevelFilter};
#[cfg(not(target_os = "none"))]
use tracing_subscriber::fmt::FmtContext;
#[cfg(not(target_os = "none"))]
use tracing_subscriber::fmt::format::{self, FormatEvent, FormatFields};
#[cfg(not(target_os = "none"))]
use tracing_subscriber::layer::SubscriberExt;
// Re-export tracing
// M27 R3: tracing port pending — re-exports only available on host
// targets until the tracing tree is vendored + patched for no_std.
#[cfg(not(target_os = "none"))]
pub use {tracing, tracing_core, tracing_subscriber};

/// The values of all the environment variables that matter for configuring a logger.
/// Errors are explicitly preserved so that we can share error handling.
pub struct LoggerConfig {
    pub filter: Result<String, VarError>,
    pub color_logs: Result<String, VarError>,
    pub verbose_entry_exit: Result<String, VarError>,
    pub verbose_thread_ids: Result<String, VarError>,
    pub backtrace: Result<String, VarError>,
    pub wraptree: Result<String, VarError>,
    pub lines: Result<String, VarError>,
}

impl LoggerConfig {
    pub fn from_env(env: &str) -> Self {
        LoggerConfig {
            filter: env::var(env),
            color_logs: env::var(format!("{env}_COLOR")),
            verbose_entry_exit: env::var(format!("{env}_ENTRY_EXIT")),
            verbose_thread_ids: env::var(format!("{env}_THREAD_IDS")),
            backtrace: env::var(format!("{env}_BACKTRACE")),
            wraptree: env::var(format!("{env}_WRAPTREE")),
            lines: env::var(format!("{env}_LINES")),
        }
    }
}

/// Initialize the logger with the given values for the filter, coloring, and other options env variables.
pub fn init_logger(cfg: LoggerConfig) -> Result<(), Error> {
    // M27 R3: tracing port pending — on host targets this calls
    // init_logger_with_additional_layer(cfg, Registry::default); on the
    // SemOS target it's a silent no-op so callers (rustc_driver_impl)
    // proceed cleanly without any log subscriber active.
    #[cfg(not(target_os = "none"))]
    {
        init_logger_with_additional_layer(cfg, Registry::default)
    }
    #[cfg(target_os = "none")]
    {
        let _ = cfg;
        Ok(())
    }
}

/// Trait alias for the complex return type of `build_subscriber` in
/// [init_logger_with_additional_layer]. A [Registry] with any composition of [tracing::Subscriber]s
/// (e.g. `Registry::default().with(custom_layer)`) should be compatible with this type.
/// Having an alias is also useful so rustc_driver_impl does not need to explicitly depend on
/// `tracing_subscriber`.
// M27 R3: tracing port pending — trait alias surface preserved on host
// builds; on SemOS target a marker trait stands in so callers' generic
// bounds (`F: FnOnce() -> T where T: BuildSubscriberRet`) still resolve.
#[cfg(not(target_os = "none"))]
pub trait BuildSubscriberRet:
    tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span> + Send + Sync
{
}

#[cfg(not(target_os = "none"))]
impl<
    T: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span> + Send + Sync,
> BuildSubscriberRet for T
{
}

#[cfg(target_os = "none")]
pub trait BuildSubscriberRet {}
#[cfg(target_os = "none")]
impl<T> BuildSubscriberRet for T {}

/// Initialize the logger with the given values for the filter, coloring, and other options env variables.
/// Additionally add a custom layer to collect logging and tracing events via `build_subscriber`,
/// for example: `|| Registry::default().with(custom_layer)`.
pub fn init_logger_with_additional_layer<F, T>(
    cfg: LoggerConfig,
    build_subscriber: F,
) -> Result<(), Error>
where
    F: FnOnce() -> T,
    T: BuildSubscriberRet,
{
    // M27 R3: tracing port pending — host body runs the real
    // tracing-subscriber configuration. SemOS target body discards the
    // builder (calling it so any user side-effects still happen) and
    // returns Ok.
    #[cfg(not(target_os = "none"))]
    {
        let filter = match cfg.filter {
            Ok(env) => EnvFilter::new(env),
            _ => EnvFilter::default().add_directive(Directive::from(LevelFilter::WARN)),
        };

        let color_logs = match cfg.color_logs {
            Ok(value) => match value.as_ref() {
                "always" => true,
                "never" => false,
                "auto" => stderr_isatty(),
                _ => return Err(Error::InvalidColorValue(value)),
            },
            Err(VarError::NotPresent) => stderr_isatty(),
            Err(VarError::NotUnicode(_value)) => return Err(Error::NonUnicodeColorValue),
        };

        let verbose_entry_exit = match cfg.verbose_entry_exit {
            Ok(v) => &v != "0",
            Err(_) => false,
        };

        let verbose_thread_ids = match cfg.verbose_thread_ids {
            Ok(v) => &v == "1",
            Err(_) => false,
        };

        let lines = match cfg.lines {
            Ok(v) => &v == "1",
            Err(_) => false,
        };

        let mut layer = tracing_tree::HierarchicalLayer::default()
            .with_writer(io::stderr)
            .with_ansi(color_logs)
            .with_targets(true)
            .with_verbose_exit(verbose_entry_exit)
            .with_verbose_entry(verbose_entry_exit)
            .with_indent_amount(2)
            .with_indent_lines(lines)
            .with_thread_ids(verbose_thread_ids)
            .with_thread_names(verbose_thread_ids);

        if let Ok(v) = cfg.wraptree {
            match v.parse::<usize>() {
                Ok(v) => layer = layer.with_wraparound(v),
                Err(_) => return Err(Error::InvalidWraptree(v)),
            }
        }

        let subscriber = build_subscriber();
        // NOTE: It is important to make sure that the filter is applied on the last layer
        match cfg.backtrace {
            Ok(backtrace_target) => {
                let fmt_layer = tracing_subscriber::fmt::layer()
                    .with_writer(io::stderr)
                    .without_time()
                    .event_format(BacktraceFormatter { backtrace_target });
                let subscriber = subscriber.with(layer).with(fmt_layer).with(filter);
                tracing::subscriber::set_global_default(subscriber)?;
            }
            Err(_) => {
                tracing::subscriber::set_global_default(subscriber.with(layer).with(filter))?;
            }
        };

        Ok(())
    }
    #[cfg(target_os = "none")]
    {
        // M27 R3: tracing port pending — discard the builder + cfg and exit clean.
        let _ = build_subscriber();
        let _ = cfg;
        Ok(())
    }
}

// M27 R3: tracing port pending — BacktraceFormatter only meaningful with
// tracing-subscriber + std::backtrace. Gated to host builds only.
#[cfg(not(target_os = "none"))]
struct BacktraceFormatter {
    backtrace_target: String,
}

#[cfg(not(target_os = "none"))]
impl<S, N> FormatEvent<S, N> for BacktraceFormatter
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: format::Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let target = event.metadata().target();
        if !target.contains(&self.backtrace_target) {
            return Ok(());
        }
        // Use Backtrace::force_capture because we don't want to depend on the
        // RUST_BACKTRACE environment variable being set.
        let backtrace = std::backtrace::Backtrace::force_capture();
        writeln!(writer, "stack backtrace: \n{backtrace:?}")
    }
}

pub fn stdout_isatty() -> bool {
    // M27 R3: tracing port pending — host uses std::io::IsTerminal;
    // SemOS target has no tty concept yet (stdout is always the
    // kernel-routed serial fd), so report false.
    #[cfg(not(target_os = "none"))]
    {
        io::stdout().is_terminal()
    }
    #[cfg(target_os = "none")]
    {
        false
    }
}

pub fn stderr_isatty() -> bool {
    // M27 R3: tracing port pending — same as stdout_isatty.
    #[cfg(not(target_os = "none"))]
    {
        io::stderr().is_terminal()
    }
    #[cfg(target_os = "none")]
    {
        false
    }
}

#[derive(Debug)]
pub enum Error {
    InvalidColorValue(String),
    NonUnicodeColorValue,
    InvalidWraptree(String),
    // M27 R3: tracing port pending — the AlreadyInit variant wraps
    // tracing::dispatcher::SetGlobalDefaultError. On the SemOS target
    // we keep the variant for API stability but use a unit payload
    // (the subscriber is never installed, so the variant is dead).
    #[cfg(not(target_os = "none"))]
    AlreadyInit(SetGlobalDefaultError),
    #[cfg(target_os = "none")]
    AlreadyInit,
}

// M27 R3: tracing port pending — std::error::Error requires std on
// stable. core::error::Error is stable since Rust 1.81; use it.
impl core::error::Error for Error {}

impl Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidColorValue(value) => write!(
                formatter,
                "invalid log color value '{value}': expected one of always, never, or auto",
            ),
            Error::NonUnicodeColorValue => write!(
                formatter,
                "non-Unicode log color value: expected one of always, never, or auto",
            ),
            Error::InvalidWraptree(value) => write!(
                formatter,
                "invalid log WRAPTREE value '{value}': expected a non-negative integer",
            ),
            #[cfg(not(target_os = "none"))]
            Error::AlreadyInit(tracing_error) => Display::fmt(tracing_error, formatter),
            #[cfg(target_os = "none")]
            Error::AlreadyInit => write!(formatter, "logger already initialized"),
        }
    }
}

#[cfg(not(target_os = "none"))]
impl From<SetGlobalDefaultError> for Error {
    fn from(tracing_error: SetGlobalDefaultError) -> Self {
        Error::AlreadyInit(tracing_error)
    }
}
