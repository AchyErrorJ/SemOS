//! Minimal `std::path` — lexical Path / PathBuf over the SemOS POSIX namespace.
//!
//! Single separator (`'/'`), no drive letters, no UNC. All operations are
//! lexical: nothing here touches the filesystem (no metadata, no `is_file`).
//! For "does this path point at a real file?" use `fs::metadata` (TBD) or
//! just try to open it and look at the error.

use core_alloc::borrow::ToOwned;
use core_alloc::string::String;
use core::ops::Deref;

const SEP: char = '/';

/// Borrowed path slice — a thin newtype over `str` so type signatures read
/// naturally. Construct with `Path::new`.
#[repr(transparent)]
pub struct Path {
    inner: str,
}

impl Path {
    /// Convert a `&str` to a `&Path`. Zero-cost.
    pub fn new<S: AsRef<str> + ?Sized>(s: &S) -> &Path {
        unsafe { &*(s.as_ref() as *const str as *const Path) }
    }

    pub fn as_str(&self) -> &str {
        &self.inner
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn is_absolute(&self) -> bool {
        self.inner.starts_with(SEP)
    }

    /// Parent path, or `None` if this is a root / empty / single-component.
    pub fn parent(&self) -> Option<&Path> {
        let s = &self.inner;
        if s.is_empty() {
            return None;
        }
        let s_trimmed = s.trim_end_matches(SEP);
        if s_trimmed.is_empty() {
            return None; // was "/" or "//" etc.
        }
        match s_trimmed.rfind(SEP) {
            Some(0) => Some(Path::new("/")),
            Some(idx) => Some(Path::new(&s_trimmed[..idx])),
            None => None, // no separator at all — no parent
        }
    }

    /// Final component, or `None` if path ends in `/` or is empty.
    pub fn file_name(&self) -> Option<&str> {
        let s = self.inner.trim_end_matches(SEP);
        if s.is_empty() {
            return None;
        }
        match s.rfind(SEP) {
            Some(idx) => Some(&s[idx + 1..]),
            None => Some(s),
        }
    }

    /// Extension (the chars after the last `.` in `file_name`), or `None`.
    /// Filenames starting with `.` and no other dot return `None` (Unix
    /// convention: `.bashrc` has no extension).
    pub fn extension(&self) -> Option<&str> {
        let name = self.file_name()?;
        let dot = name.rfind('.')?;
        if dot == 0 {
            return None;
        }
        Some(&name[dot + 1..])
    }

    /// File stem — the chars before the last `.` in `file_name`. For
    /// `foo.tar.gz` returns `"foo.tar"` (matches std).
    pub fn file_stem(&self) -> Option<&str> {
        let name = self.file_name()?;
        match name.rfind('.') {
            Some(0) => Some(name), // leading dot: it's the whole name
            Some(idx) => Some(&name[..idx]),
            None => Some(name),
        }
    }

    /// Lexically join with `other`. If `other` is absolute, replaces; else
    /// appends with a separator.
    pub fn join<P: AsRef<Path>>(&self, other: P) -> PathBuf {
        let mut buf = self.to_path_buf();
        buf.push(other);
        buf
    }

    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf { inner: self.inner.to_owned() }
    }

    /// Lexical canonicalize — collapse `.` and `..` components without
    /// touching the filesystem. Mirrors `std::path::Path::canonicalize`
    /// in shape but does NOT resolve symlinks (we don't have those) and
    /// does NOT verify the path exists.
    ///
    /// Added 2026-05-30 for M27 R2 (rustc_span and 2 other crates use
    /// canonicalize lexically). std's version is filesystem-resolving;
    /// callers who need the real-fs variant should use a future
    /// `fs::canonicalize` helper that hits SYS_STAT.
    pub fn canonicalize_lexical(&self) -> PathBuf {
        use core_alloc::vec::Vec;
        let is_abs = self.is_absolute();
        let mut parts: Vec<&str> = Vec::new();
        for seg in self.inner.split(SEP) {
            match seg {
                "" => continue, // skip "//" and leading "/"
                "." => continue,
                ".." => {
                    // If we have parts, pop one; for an absolute path,
                    // ".." at the root stays at root (POSIX semantics).
                    if let Some(last) = parts.last() {
                        if *last != ".." {
                            parts.pop();
                            continue;
                        }
                    }
                    // For relative paths, ".." with nothing to pop
                    // becomes a literal ".." preserved at the head.
                    if !is_abs {
                        parts.push("..");
                    }
                }
                _ => parts.push(seg),
            }
        }
        let mut out = String::new();
        if is_abs {
            out.push(SEP);
        }
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                out.push(SEP);
            }
            out.push_str(p);
        }
        // Empty relative-path canonicalize → ".".
        if out.is_empty() {
            out.push('.');
        }
        PathBuf { inner: out }
    }
}

impl AsRef<Path> for Path {
    fn as_ref(&self) -> &Path { self }
}
impl AsRef<Path> for str {
    fn as_ref(&self) -> &Path { Path::new(self) }
}
impl AsRef<Path> for String {
    fn as_ref(&self) -> &Path { Path::new(self.as_str()) }
}

impl PartialEq for Path {
    fn eq(&self, other: &Self) -> bool { self.inner == other.inner }
}
impl Eq for Path {}

/// Owned, mutable path. Backed by `String`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathBuf {
    inner: String,
}

impl PathBuf {
    pub fn new() -> Self {
        Self { inner: String::new() }
    }

    pub fn as_path(&self) -> &Path {
        Path::new(self.inner.as_str())
    }

    pub fn as_str(&self) -> &str {
        self.inner.as_str()
    }

    pub fn into_string(self) -> String {
        self.inner
    }

    /// Append a path component. If `other` is absolute, replaces the entire
    /// path. Otherwise adds a separator (unless the current path is empty or
    /// already ends with one).
    pub fn push<P: AsRef<Path>>(&mut self, other: P) {
        let o = other.as_ref();
        if o.is_absolute() {
            self.inner.clear();
            self.inner.push_str(&o.inner);
            return;
        }
        if !self.inner.is_empty() && !self.inner.ends_with(SEP) {
            self.inner.push(SEP);
        }
        self.inner.push_str(&o.inner);
    }

    /// Remove the last component. Returns true if anything was popped.
    pub fn pop(&mut self) -> bool {
        let trimmed = self.inner.trim_end_matches(SEP);
        if trimmed.is_empty() {
            return false;
        }
        match trimmed.rfind(SEP) {
            Some(0) => {
                self.inner.truncate(1); // keep root "/"
            }
            Some(idx) => {
                self.inner.truncate(idx);
            }
            None => {
                self.inner.clear();
            }
        }
        true
    }

    /// Replace the extension of the file_name. If the path has no file_name,
    /// returns false and leaves self unchanged.
    pub fn set_extension<S: AsRef<str>>(&mut self, ext: S) -> bool {
        // Clone parent + stem first so we can rebuild without overlapping
        // immutable + mutable borrows of self.inner.
        let stem = match self.as_path().file_stem() {
            Some(s) => s.to_owned(),
            None => return false,
        };
        let parent: Option<String> = self.as_path().parent().map(|p| p.as_str().to_owned());
        self.inner.clear();
        if let Some(p) = parent {
            if p != "/" {
                self.inner.push_str(&p);
                self.inner.push(SEP);
            } else {
                self.inner.push(SEP);
            }
        }
        self.inner.push_str(&stem);
        let new_ext = ext.as_ref();
        if !new_ext.is_empty() {
            self.inner.push('.');
            self.inner.push_str(new_ext);
        }
        true
    }
}

impl From<&str> for PathBuf {
    fn from(s: &str) -> Self { Self { inner: s.to_owned() } }
}
impl From<String> for PathBuf {
    fn from(s: String) -> Self { Self { inner: s } }
}
impl AsRef<Path> for PathBuf {
    fn as_ref(&self) -> &Path { self.as_path() }
}
impl Deref for PathBuf {
    type Target = Path;
    fn deref(&self) -> &Path { self.as_path() }
}

// ---------------------------------------------------------------------
// M27 R4 B5 — extended PathBuf surface for rustc port.
//
// A2-followup found that source_map.rs's FilePathMapping needs
// components(), Component::Normal, strip_prefix, as_os_str, and Cow<Path>.
// Adding lexical-only versions of each below. None of these touch the FS.
// ---------------------------------------------------------------------

/// Components of a path, mirroring `std::path::Component` shape (minus
/// the Windows Prefix variants since SemOS is POSIX-only). Yielded by
/// `Path::components()`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Component<'a> {
    /// The root directory `/`.
    RootDir,
    /// A reference to the current directory: `.`
    CurDir,
    /// A reference to the parent directory: `..`
    ParentDir,
    /// A normal component, e.g. `foo` in `/foo/bar`.
    Normal(&'a str),
}

impl<'a> Component<'a> {
    /// As the underlying string slice. Matches `std::path::Component::as_os_str`'s
    /// shape — returns the OsStr-equivalent (which is `&str` on SemOS).
    pub fn as_os_str(&self) -> &'a str {
        match *self {
            Component::RootDir => "/",
            Component::CurDir => ".",
            Component::ParentDir => "..",
            Component::Normal(s) => s,
        }
    }

    /// Same as `as_os_str` but typed as `&str` directly (callers that
    /// already know they're on SemOS can skip the OsStr alias).
    pub fn as_str(&self) -> &'a str {
        self.as_os_str()
    }
}

/// Iterator over a path's components. Lexical only — never touches the FS.
#[derive(Clone)]
pub struct Components<'a> {
    inner: &'a str,
    yielded_root: bool,
}

impl<'a> Components<'a> {
    /// Convert the remainder back to a `&Path`.
    pub fn as_path(&self) -> &'a Path {
        Path::new(self.inner)
    }
}

impl<'a> Iterator for Components<'a> {
    type Item = Component<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        // Emit RootDir first if the path is absolute.
        if !self.yielded_root && self.inner.starts_with(SEP) {
            self.yielded_root = true;
            self.inner = self.inner.trim_start_matches(SEP);
            return Some(Component::RootDir);
        }
        // Skip leading separators (collapse runs of "/").
        self.inner = self.inner.trim_start_matches(SEP);
        if self.inner.is_empty() {
            return None;
        }
        // Find next separator.
        let (head, tail) = match self.inner.find(SEP) {
            Some(idx) => (&self.inner[..idx], &self.inner[idx..]),
            None => (self.inner, ""),
        };
        self.inner = tail;
        Some(match head {
            "." => Component::CurDir,
            ".." => Component::ParentDir,
            s => Component::Normal(s),
        })
    }
}

/// Error returned by `Path::strip_prefix` when the prefix doesn't match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripPrefixError;

impl Path {
    /// Iterator over path components.
    pub fn components(&self) -> Components<'_> {
        Components {
            inner: &self.inner,
            yielded_root: false,
        }
    }

    /// `std::path::Path::strip_prefix`-shaped. Returns the remainder of
    /// the path after `prefix` is removed; errors if the path doesn't
    /// start with `prefix`. Comparison is component-by-component.
    pub fn strip_prefix<P: AsRef<Path>>(&self, prefix: P) -> Result<&Path, StripPrefixError> {
        let prefix = prefix.as_ref();
        // Quick path: empty prefix returns self unchanged.
        if prefix.is_empty() {
            return Ok(self);
        }
        // Walk both component iterators in lockstep.
        let mut self_iter = self.components();
        let mut pre_iter = prefix.components();
        loop {
            match (self_iter.next(), pre_iter.next()) {
                (Some(a), Some(b)) if a == b => continue,
                (_, Some(_)) => return Err(StripPrefixError),
                (_, None) => break,
            }
        }
        // Remainder: skip leading separators.
        let rest = self_iter.as_path().as_str().trim_start_matches(SEP);
        Ok(Path::new(rest))
    }

    /// Returns the path as a `&str` — equivalent to std's
    /// `Path::as_os_str` since SemOS is UTF-8 everywhere.
    pub fn as_os_str(&self) -> &str {
        &self.inner
    }
}

// Allow `Cow<Path>` to work by implementing ToOwned. std does the same
// thing — alloc's Cow<'_, Path> requires Path: ToOwned<Owned = PathBuf>
// and PathBuf: Borrow<Path>.
impl core::borrow::Borrow<Path> for PathBuf {
    fn borrow(&self) -> &Path {
        self.as_path()
    }
}

impl core_alloc::borrow::ToOwned for Path {
    type Owned = PathBuf;
    fn to_owned(&self) -> PathBuf {
        PathBuf { inner: self.inner.to_owned() }
    }
}

// Re-export Cow under `path` for the rustc callers' `use semos_std::path::Cow;`
// pattern. They use `Cow<Path>` exclusively here, so this saves them
// having to import alloc::borrow::Cow separately.
pub use core_alloc::borrow::Cow;
