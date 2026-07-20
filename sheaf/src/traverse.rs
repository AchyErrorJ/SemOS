use crate::Result;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Bundle,
}

#[derive(Clone, Debug)]
pub struct FindEntry {
    pub path: PathBuf,
    pub kind: EntryKind,
}

/// Find-style traversal with the Sheaf "readdir lie":
/// bundle dirs are reported once as Bundle and not traversed unless
/// `contents=true`.
pub fn find(root: &Path, contents: bool) -> Result<Vec<FindEntry>> {
    let mut out = Vec::new();
    walk(root, contents, &mut out)?;
    Ok(out)
}

fn walk(path: &Path, contents: bool, out: &mut Vec<FindEntry>) -> Result<()> {
    if crate::is_bundle_dir(path) && !contents {
        out.push(FindEntry { path: path.to_path_buf(), kind: EntryKind::Bundle });
        return Ok(());
    }
    if path.is_dir() {
        out.push(FindEntry { path: path.to_path_buf(), kind: if crate::is_bundle_dir(path) { EntryKind::Bundle } else { EntryKind::Dir } });
        let mut entries = std::fs::read_dir(path)?.collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| e.path());
        for e in entries {
            walk(&e.path(), contents, out)?;
        }
    } else if path.is_file() {
        out.push(FindEntry { path: path.to_path_buf(), kind: EntryKind::File });
    }
    Ok(())
}

