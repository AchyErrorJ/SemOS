use crate::{Result, Suid};
use std::path::Path;

pub fn initial(suid: Suid, derived_from: Option<Suid>) -> String {
    let mut s = String::new();
    s.push_str("schema = 1\n");
    s.push_str(&format!("suid = \"{}\"\n", suid));
    if let Some(parent) = derived_from {
        s.push_str(&format!("derived_from = \"{}\"\n", parent));
    }
    s
}

pub fn append_export(bundle: &Path, format: &str, sha256: &str, by: &str) -> Result<()> {
    let path = bundle.join("provenance.toml");
    let mut s = std::fs::read_to_string(&path).unwrap_or_else(|_| String::from("schema = 1\n"));
    s.push_str("\n[[exports]]\n");
    s.push_str(&format!("at = \"{}\"\n", crate::iso_now()));
    s.push_str(&format!("format = \"{}\"\n", format));
    s.push_str(&format!("sha256 = \"{}\"\n", sha256));
    s.push_str(&format!("by = \"{}\"\n", by));
    crate::atomic_write(&path, s.as_bytes())
}

pub fn append_edit(bundle: &Path, by: &str, note: &str) -> Result<()> {
    let path = bundle.join("provenance.toml");
    let mut s = std::fs::read_to_string(&path).unwrap_or_else(|_| String::from("schema = 1\n"));
    s.push_str("\n[[edits]]\n");
    s.push_str(&format!("at = \"{}\"\n", crate::iso_now()));
    s.push_str(&format!("by = \"{}\"\n", by));
    s.push_str(&format!("note = {}\n", crate::toml::quote(note)));
    crate::atomic_write(&path, s.as_bytes())
}

