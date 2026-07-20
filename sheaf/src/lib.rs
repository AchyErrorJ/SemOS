//! Sheaf — Phase 0 userland prototype.
//!
//! This crate intentionally has **zero external dependencies** so it can build
//! in the SemOS repo's restricted/offline environment. The implementation is a
//! pragmatic Phase-0 prototype, not a general TOML/tar library: parsers accept
//! exactly the small schema Sheaf writes, plus enough tolerance for hand edits.

pub mod agent;
pub mod bundle;
pub mod export;
pub mod manifest;
pub mod provenance;
pub mod sha256;
pub mod suid;
pub mod toml;
pub mod traverse;

pub use bundle::{BundleInfo, LintIssue, LintLevel};
pub use manifest::{BundleManifest, Facet, LeafKind, Role};
pub use suid::Suid;

use std::fmt;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, SheafError>;

#[derive(Debug)]
pub enum SheafError {
    Io(std::io::Error),
    Parse(String),
    Invalid(String),
    NotBundle(PathBuf),
    Missing(String),
}

impl fmt::Display for SheafError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Parse(s) => write!(f, "parse: {s}"),
            Self::Invalid(s) => write!(f, "invalid: {s}"),
            Self::NotBundle(p) => write!(f, "not a bundle: {}", p.display()),
            Self::Missing(s) => write!(f, "missing: {s}"),
        }
    }
}

impl std::error::Error for SheafError {}

impl From<std::io::Error> for SheafError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn bundle_manifest_path(bundle: &Path) -> PathBuf {
    bundle.join("bundle.toml")
}

pub fn is_bundle_dir(path: &Path) -> bool {
    path.is_dir() && bundle_manifest_path(path).is_file()
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn iso_now() -> String {
    // Phase 0 avoids chrono/time deps. Keep timestamps machine-sortable and
    // explicit; exact RFC3339 formatting can land once a time crate is allowed.
    format!("{}Z", unix_now())
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{ext}.tmp"),
        None => String::from("tmp"),
    });
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
