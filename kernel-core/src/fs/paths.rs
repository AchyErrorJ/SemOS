//! Hierarchical-path namespace over SUID-addressed semantic objects.
//!
//! Phase 9 Stage 1. Lets apps and the kernel address persistent state
//! the way users think about it — `/notes/2026/meeting.md` — without
//! abandoning the SUID-first storage model that Phase 4-7 was built
//! around.
//!
//! # Architecture
//!
//! ```text
//!   "/notes/2026/meeting.md"
//!         │
//!         ▼  walk components, look each one up as a dir entry
//!   Namespace::resolve()
//!         │
//!         ▼  yields SUID at end of walk
//!   SemanticObject (regular file content OR another directory)
//! ```
//!
//! - **Files** are ordinary SemanticObjects whose `content` holds the
//!   user bytes (Inline ≤ 256 B today; Allocated for larger objects
//!   when the memory pool can satisfy it).
//! - **Directories** are SemanticObjects too, with a packed
//!   table of `(name, SUID)` pairs in their content. See
//!   [`DirEntries`] for the on-disk format. There's no separate
//!   "inode" / "dentry" split — the directory IS its semantic object.
//! - **Root** is a well-known SUID ([`ROOT_SUID`]) created by
//!   [`Namespace::init`]; absolute paths start there.
//!
//! This is **Stage 1** — in-memory only. Persistence (snapshot writeback
//! per Phase 6) lands in Stage 2; syscalls in Stage 3.
//!
//! # Not implemented
//!
//! - Permissions beyond what SecurityTier already enforces on the
//!   underlying object. There's no per-user owner bits / mode bits.
//! - Symlinks. The directory entries point to SUIDs directly; cycles
//!   would form a graph, not a tree. Future: a Relationship-typed
//!   ObjectLink could encode "soft" links.
//! - Hardlinks (multiple names → one SUID). Easy to add — the
//!   namespace already stores names alongside SUIDs, but we'd need
//!   reference counting on unlink. Out of scope for Stage 1.
//! - Rename. Two-step today: lookup + create + unlink.
//! - Mount points / VFS-style stacking. Single global namespace.
//!
//! # Tests
//!
//! kernel-core can't run `cargo test` (no_std, no panic handler).
//! Boot-time validation lives in `kernel-x86_64/src/main.rs` as
//! `paths_namespace_test()` (DEMO 17) — see project memory for the
//! convention.

use crate::semantic::object::{ContentType, ObjectContent, SecurityTier, SemanticObject};
use crate::semantic::registry::global_registry;
use crate::semantic::suid::SUID;

// ============================================================================
// Constants
// ============================================================================

/// Maximum length of a single path component (one segment between
/// slashes). Chosen to fit comfortably into a few directory entries
/// per 256-byte inline content block.
pub const MAX_COMPONENT_LEN: usize = 31;

/// Maximum total path length we'll parse. Generous enough for nested
/// project trees; small enough to keep stack frames bounded.
pub const MAX_PATH_LEN: usize = 256;

/// Maximum depth of nested directories in a single `resolve` call.
/// Guards against pathological inputs (`/a/a/a/a/...`) consuming the
/// kernel stack via recursion (we iterate, but a cycle check still
/// needs a bound).
pub const MAX_PATH_DEPTH: usize = 32;

/// Maximum entries a single directory holds. Directory content is now stored
/// via `from_bytes` (heap-backed, like files) rather than 256-byte inline, so
/// the limit is the serialization buffer (`DIR_CONTENT_MAX`) not inline size.
/// 64 entries is right for a dedicated work OS — a handful of apps, system
/// tools, and documents per directory — without being unbounded.
pub const MAX_DIR_ENTRIES: usize = 64;

/// Serialization-buffer size for a directory's content: `[count: u8]` + up to
/// `MAX_DIR_ENTRIES` entries of `[len:1][name:≤31][suid:16]` = ≤ 48 B each.
/// 1 + 64·48 = 3073 → round up to 4096.
pub const DIR_CONTENT_MAX: usize = 4096;

/// Well-known SUID for the root directory. System type (TYPE_SYSTEM=15)
/// so it can't be confused with content-addressed or random user SUIDs.
/// Low half is a memorable bit pattern (`0xF005...`) for ad-hoc
/// identifiability in hex dumps.
pub const ROOT_SUID: SUID = SUID::new(
    0xF000_0000_0000_0001,
    0xF005_F11E_5300_BA5E,
);

// ============================================================================
// Errors
// ============================================================================

/// Failure modes the namespace can return. Mapped to LlmError-shaped
/// codes by the syscall layer (Stage 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    /// Path didn't start with `/` (we don't support cwd-relative).
    NotAbsolute,
    /// Path or one of its components was empty (e.g. `//`).
    EmptyComponent,
    /// A component was longer than [`MAX_COMPONENT_LEN`].
    ComponentTooLong,
    /// Path was longer than [`MAX_PATH_LEN`].
    PathTooLong,
    /// Path nested deeper than [`MAX_PATH_DEPTH`].
    TooDeep,
    /// One of the intermediate components doesn't exist.
    NotFound,
    /// Tried to walk through a non-directory (e.g. `/foo.txt/bar`).
    NotADirectory,
    /// Tried to create/list an entry whose target isn't a directory.
    IsADirectory,
    /// Name already exists in the parent directory.
    AlreadyExists,
    /// Directory is at capacity (Stage 1: [`MAX_DIR_ENTRIES`]).
    DirectoryFull,
    /// ObjectRegistry rejected the insert (probably full).
    RegistryFull,
    /// Tried to write content larger than the object format allows.
    ContentTooLarge,
    /// Internal invariant violation (e.g. directory content malformed).
    Corrupt,
}

// ============================================================================
// Path parsing
// ============================================================================

/// Split a `/`-separated absolute path into components. Validates as
/// it goes; on success calls `visit(component)` for each one.
///
/// Iterator-shaped APIs are nicer but allocating a Vec isn't on the
/// table (no_alloc) and a custom Iterator type adds machinery we
/// don't need at this layer. Callback works.
pub fn for_each_component<F>(path: &str, mut visit: F) -> Result<(), FsError>
where
    F: FnMut(&str) -> Result<(), FsError>,
{
    if path.len() > MAX_PATH_LEN { return Err(FsError::PathTooLong); }
    if !path.starts_with('/') { return Err(FsError::NotAbsolute); }

    // Skip the leading '/'. Splitting after it yields the components,
    // with an empty trailing element for "/" itself.
    let trimmed = &path[1..];
    if trimmed.is_empty() {
        // Root path "/" — no components to visit.
        return Ok(());
    }

    let mut depth = 0usize;
    for comp in trimmed.split('/') {
        if comp.is_empty() { return Err(FsError::EmptyComponent); } // catches "//"
        if comp.len() > MAX_COMPONENT_LEN { return Err(FsError::ComponentTooLong); }
        depth += 1;
        if depth > MAX_PATH_DEPTH { return Err(FsError::TooDeep); }
        visit(comp)?;
    }
    Ok(())
}

/// Split a path into `(parent_path, last_component)` so callers can
/// resolve the parent and then operate on the named child. For "/foo"
/// this returns `("/", "foo")`; for "/a/b/c" → `("/a/b", "c")`; for "/"
/// returns `Err(NotFound)` because the root has no parent.
pub fn split_parent(path: &str) -> Result<(&str, &str), FsError> {
    if path.len() > MAX_PATH_LEN { return Err(FsError::PathTooLong); }
    if !path.starts_with('/') { return Err(FsError::NotAbsolute); }
    if path == "/" { return Err(FsError::NotFound); } // no parent

    // Find the last '/'; everything before (or "/") is parent, after is name.
    let last_slash = path.rfind('/').unwrap(); // we know path starts with '/'
    let parent = if last_slash == 0 { "/" } else { &path[..last_slash] };
    let name = &path[last_slash + 1..];
    if name.is_empty() { return Err(FsError::EmptyComponent); } // e.g. "/foo/"
    if name.len() > MAX_COMPONENT_LEN { return Err(FsError::ComponentTooLong); }
    Ok((parent, name))
}

// ============================================================================
// Directory entry format
// ============================================================================
//
// A directory's content bytes look like:
//
//   [count: u8]  (number of entries, 0..=MAX_DIR_ENTRIES)
//   then `count` repetitions of:
//     [name_len: u8]  (1..=MAX_COMPONENT_LEN)
//     [name: name_len bytes, UTF-8]
//     [suid: 16 bytes, high then low, big-endian]
//
// The format is variable-length — short names use less space, so a
// typical 8-char-name directory fits more entries in the same 256-byte
// inline budget than a fixed-size layout. The kernel never trusts the
// content; every parse validates its bounds.

/// Total bytes a directory entry occupies on disk: 1 (name_len) +
/// name_len + 16 (SUID).
fn entry_byte_len(name_len: usize) -> usize { 1 + name_len + 16 }

/// Helper struct for walking a directory's packed bytes.
pub struct DirEntries<'a> {
    /// Slice of all entry bytes (excluding the leading count).
    bytes: &'a [u8],
    /// Declared number of entries; the iterator returns this many or
    /// errors if the bytes run out early.
    count: usize,
    /// Cursor into `bytes` for the iterator.
    cursor: usize,
    /// How many entries have been yielded so far.
    yielded: usize,
}

impl<'a> DirEntries<'a> {
    /// Parse the packed directory content. Validates the count byte
    /// fits within the slice and bounds. Doesn't validate per-entry
    /// fields until iteration — those errors get returned per-step.
    pub fn parse(content: &'a [u8]) -> Result<Self, FsError> {
        if content.is_empty() { return Err(FsError::Corrupt); }
        let count = content[0] as usize;
        if count > MAX_DIR_ENTRIES { return Err(FsError::Corrupt); }
        Ok(Self { bytes: &content[1..], count, cursor: 0, yielded: 0 })
    }
}

impl<'a> Iterator for DirEntries<'a> {
    type Item = Result<(&'a str, SUID), FsError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.yielded >= self.count { return None; }
        let cursor = self.cursor;
        if cursor >= self.bytes.len() { return Some(Err(FsError::Corrupt)); }

        let name_len = self.bytes[cursor] as usize;
        if name_len == 0 || name_len > MAX_COMPONENT_LEN {
            return Some(Err(FsError::Corrupt));
        }
        let name_start = cursor + 1;
        let suid_start = name_start + name_len;
        let suid_end = suid_start + 16;
        if suid_end > self.bytes.len() {
            return Some(Err(FsError::Corrupt));
        }

        // Validate UTF-8 and yield. Names are stored as bytes; we
        // re-validate as a defence against corrupted directory content.
        let name = match core::str::from_utf8(&self.bytes[name_start..suid_start]) {
            Ok(s) => s,
            Err(_) => return Some(Err(FsError::Corrupt)),
        };
        let suid_bytes = &self.bytes[suid_start..suid_end];
        let high = u64::from_be_bytes(suid_bytes[0..8].try_into().unwrap());
        let low = u64::from_be_bytes(suid_bytes[8..16].try_into().unwrap());
        let suid = SUID::new(high, low);

        self.cursor = suid_end;
        self.yielded += 1;
        Some(Ok((name, suid)))
    }
}

/// Append a `(name, suid)` entry to a directory's content. Returns
/// the new content bytes via the caller-provided buffer + length.
/// Doesn't mutate the registry — that's the caller's job.
fn insert_dir_entry(
    existing: &[u8],
    name: &str,
    suid: SUID,
    out: &mut [u8],
) -> Result<usize, FsError> {
    // Re-validate the input we're appending.
    if name.is_empty() { return Err(FsError::EmptyComponent); }
    if name.len() > MAX_COMPONENT_LEN { return Err(FsError::ComponentTooLong); }

    // Parse existing to make sure we're not silently appending after
    // junk + to reject duplicates.
    let existing_count = if existing.is_empty() { 0 } else { existing[0] as usize };
    if existing_count >= MAX_DIR_ENTRIES { return Err(FsError::DirectoryFull); }

    if !existing.is_empty() {
        for entry in DirEntries::parse(existing)? {
            let (existing_name, _) = entry?;
            if existing_name == name { return Err(FsError::AlreadyExists); }
        }
    }

    // Compute the new size and bounds-check the destination buffer.
    let existing_payload_len = if existing.is_empty() { 0 } else { existing.len() - 1 };
    let new_entry_len = entry_byte_len(name.len());
    let new_total = 1 + existing_payload_len + new_entry_len;
    if new_total > out.len() { return Err(FsError::DirectoryFull); }

    // Emit: [new_count][existing_entries][new_entry].
    out[0] = (existing_count + 1) as u8;
    if existing_payload_len > 0 {
        out[1..1 + existing_payload_len].copy_from_slice(&existing[1..]);
    }
    let entry_off = 1 + existing_payload_len;
    out[entry_off] = name.len() as u8;
    out[entry_off + 1..entry_off + 1 + name.len()].copy_from_slice(name.as_bytes());
    let suid_off = entry_off + 1 + name.len();
    out[suid_off..suid_off + 8].copy_from_slice(&suid.high.to_be_bytes());
    out[suid_off + 8..suid_off + 16].copy_from_slice(&suid.low.to_be_bytes());

    Ok(new_total)
}

/// Remove the entry with the given name. Returns the new packed
/// content length, or `Err(NotFound)` if `name` wasn't present.
fn remove_dir_entry(
    existing: &[u8],
    name: &str,
    out: &mut [u8],
) -> Result<(usize, SUID), FsError> {
    if existing.is_empty() { return Err(FsError::NotFound); }
    let existing_count = existing[0] as usize;
    if existing_count == 0 { return Err(FsError::NotFound); }

    // First scan finds the target entry and computes its byte range.
    let mut target_start = None;
    let mut target_len = 0usize;
    let mut target_suid = SUID::NULL;
    let mut byte_off = 1usize; // skip count
    for _ in 0..existing_count {
        if byte_off >= existing.len() { return Err(FsError::Corrupt); }
        let nl = existing[byte_off] as usize;
        if nl == 0 || nl > MAX_COMPONENT_LEN { return Err(FsError::Corrupt); }
        let name_start = byte_off + 1;
        let suid_start = name_start + nl;
        let next = suid_start + 16;
        if next > existing.len() { return Err(FsError::Corrupt); }

        let this_name = match core::str::from_utf8(&existing[name_start..suid_start]) {
            Ok(s) => s,
            Err(_) => return Err(FsError::Corrupt),
        };
        if this_name == name {
            target_start = Some(byte_off);
            target_len = next - byte_off;
            let high = u64::from_be_bytes(existing[suid_start..suid_start+8].try_into().unwrap());
            let low = u64::from_be_bytes(existing[suid_start+8..next].try_into().unwrap());
            target_suid = SUID::new(high, low);
            break;
        }
        byte_off = next;
    }

    let target_start = target_start.ok_or(FsError::NotFound)?;

    // Build the output: new count + everything except the target slice.
    let new_total = existing.len() - target_len;
    if new_total > out.len() { return Err(FsError::Corrupt); } // shouldn't happen
    out[0] = (existing_count - 1) as u8;
    if target_start > 1 {
        out[1..target_start].copy_from_slice(&existing[1..target_start]);
    }
    let tail_src_start = target_start + target_len;
    let tail_len = existing.len() - tail_src_start;
    if tail_len > 0 {
        out[target_start..target_start + tail_len]
            .copy_from_slice(&existing[tail_src_start..]);
    }
    Ok((new_total, target_suid))
}

// ============================================================================
// Namespace — top-level API
// ============================================================================

/// Singleton orchestrator. Holds the root SUID; methods translate
/// path-based requests into ObjectRegistry operations.
///
/// Stateless aside from the root SUID — recreating an instance is
/// cheap because all state lives in `global_registry()`.
pub struct Namespace;

impl Namespace {
    /// Boot-time setup: install the root directory in the registry as
    /// an empty directory object. Idempotent — re-running is a no-op
    /// if root already exists.
    pub fn init() -> Result<(), FsError> {
        let registry = unsafe { global_registry() };
        if registry.get(&ROOT_SUID).is_some() {
            return Ok(()); // already initialised
        }
        let mut root = SemanticObject::new(ROOT_SUID, SecurityTier::Public, 0);
        root.content_type = ContentType::Structured;
        // Empty directory: just the count byte = 0.
        let empty = [0u8; 1];
        root.content = ObjectContent::from_inline(&empty).ok_or(FsError::Corrupt)?;
        if !registry.insert(root) {
            return Err(FsError::RegistryFull);
        }
        Ok(())
    }

    /// Walk a path from root and return the SUID of the named object.
    /// Each intermediate component must be a directory.
    pub fn resolve(path: &str) -> Result<SUID, FsError> {
        let mut current = ROOT_SUID;
        for_each_component(path, |component| {
            let next = lookup_in_dir(current, component)?;
            current = next;
            Ok(())
        })?;
        Ok(current)
    }

    /// Look up `name` within the directory at `parent` (no path
    /// walking — single level). Useful when you've already resolved
    /// the parent.
    pub fn lookup_in(parent: SUID, name: &str) -> Result<SUID, FsError> {
        lookup_in_dir(parent, name)
    }

    /// Create a new (empty) directory at `path`. The parent must
    /// already exist and be a directory; the basename must not.
    pub fn mkdir(path: &str) -> Result<SUID, FsError> {
        let (parent_path, name) = split_parent(path)?;
        let parent = Self::resolve(parent_path)?;
        let suid = mint_suid();
        let now = crate::platform::wall_clock().unwrap_or(0);
        let mut dir = SemanticObject::new(suid, SecurityTier::Public, 0);
        dir.content_type = ContentType::Structured;
        dir.created_at = now;
        dir.modified_at = now;
        let empty = [0u8; 1];
        dir.content = ObjectContent::from_inline(&empty).ok_or(FsError::Corrupt)?;
        let registry = unsafe { global_registry() };
        if !registry.insert(dir) { return Err(FsError::RegistryFull); }
        add_child(parent, name, suid)?;
        Ok(suid)
    }

    /// Create a regular file at `path` with the given initial content
    /// and security tier. Returns the new file's SUID.
    pub fn create_file(
        path: &str,
        tier: SecurityTier,
        content: &[u8],
    ) -> Result<SUID, FsError> {
        let (parent_path, name) = split_parent(path)?;
        let parent = Self::resolve(parent_path)?;
        let suid = mint_suid();
        let now = crate::platform::wall_clock().unwrap_or(0);
        let mut file = SemanticObject::new(suid, tier, 0);
        file.content_type = ContentType::Binary;
        file.created_at = now;
        file.modified_at = now;
        file.content = ObjectContent::from_bytes(content).ok_or(FsError::ContentTooLarge)?;
        let registry = unsafe { global_registry() };
        if !registry.insert(file) { return Err(FsError::RegistryFull); }
        add_child(parent, name, suid)?;
        Ok(suid)
    }

    /// Replace a file's content. The object at `path` must already
    /// exist and be a regular file. New content must fit the inline
    /// storage limit (Stage 1).
    pub fn write_file(path: &str, content: &[u8]) -> Result<(), FsError> {
        let suid = Self::resolve(path)?;
        let now = crate::platform::wall_clock().unwrap_or(0);
        let registry = unsafe { global_registry() };
        let obj = registry.get_mut(&suid).ok_or(FsError::NotFound)?;
        if obj.content_type == ContentType::Structured {
            return Err(FsError::IsADirectory);
        }
        obj.content = ObjectContent::from_bytes(content).ok_or(FsError::ContentTooLarge)?;
        obj.modified_at = now;
        Ok(())
    }

    /// Read a file's content. Returns a slice into the underlying
    /// SemanticObject's inline buffer; valid until the next mutation.
    pub fn read_file(path: &str) -> Result<&'static [u8], FsError> {
        let suid = Self::resolve(path)?;
        let registry = unsafe { global_registry() };
        let obj = registry.get(&suid).ok_or(FsError::NotFound)?;
        if obj.content_type == ContentType::Structured {
            return Err(FsError::IsADirectory);
        }
        // `as_bytes` returns Option<&[u8]> with the object's lifetime.
        // The registry is 'static so this slice lives until the object
        // is mutated or removed.
        let bytes = obj.content.as_bytes().ok_or(FsError::Corrupt)?;
        // Extend the lifetime — sound because the registry is &'static mut.
        Ok(unsafe { core::mem::transmute::<&[u8], &'static [u8]>(bytes) })
    }

    /// Remove the entry named `path`. If it's a directory, the
    /// directory must be empty (no recursive rmdir in Stage 1). The
    /// underlying SemanticObject is also removed from the registry.
    pub fn unlink(path: &str) -> Result<(), FsError> {
        let (parent_path, name) = split_parent(path)?;
        let parent = Self::resolve(parent_path)?;
        let suid = lookup_in_dir(parent, name)?;
        // If target is a non-empty directory, refuse.
        {
            let registry = unsafe { global_registry() };
            let obj = registry.get(&suid).ok_or(FsError::NotFound)?;
            if obj.content_type == ContentType::Structured {
                let bytes = obj.content.as_bytes().unwrap_or(&[]);
                if !bytes.is_empty() && bytes[0] > 0 {
                    return Err(FsError::DirectoryFull); // reused: "non-empty"
                }
            }
        }
        remove_child(parent, name)?;
        // Drop the object itself.
        let registry = unsafe { global_registry() };
        registry.remove(&suid);
        Ok(())
    }

    /// Run `visit(name, suid)` for each entry in the directory at
    /// `path`. Use this instead of returning an iterator because the
    /// directory's content slice is borrowed from the registry — we
    /// can't easily express that lifetime through an iterator type
    /// without `GenericAssociatedType`s + lifetime gymnastics.
    pub fn readdir<F>(path: &str, mut visit: F) -> Result<(), FsError>
    where
        F: FnMut(&str, SUID),
    {
        let suid = Self::resolve(path)?;
        let registry = unsafe { global_registry() };
        let obj = registry.get(&suid).ok_or(FsError::NotFound)?;
        if obj.content_type != ContentType::Structured {
            return Err(FsError::NotADirectory);
        }
        let bytes = obj.content.as_bytes().unwrap_or(&[]);
        if bytes.is_empty() { return Ok(()); }
        for entry in DirEntries::parse(bytes)? {
            let (name, child) = entry?;
            visit(name, child);
        }
        Ok(())
    }

    // ========================================================================
    // Phase 14 Tier 2 — rename + truncate
    // ========================================================================

    /// Atomically rename `old_path` → `new_path`. The underlying SUID
    /// stays the same — we just swap the parent-dir entries. Both
    /// parents must exist; the new basename must not.
    ///
    /// Cross-directory move is supported (`/a/foo` → `/b/foo`).
    pub fn rename(old_path: &str, new_path: &str) -> Result<(), FsError> {
        let (old_parent_path, old_name) = split_parent(old_path)?;
        let (new_parent_path, new_name) = split_parent(new_path)?;

        let old_parent = Self::resolve(old_parent_path)?;
        let new_parent = Self::resolve(new_parent_path)?;

        // Reject if the target name already exists.
        if lookup_in_dir(new_parent, new_name).is_ok() {
            return Err(FsError::AlreadyExists);
        }

        // Look up the SUID we're moving.
        let suid = lookup_in_dir(old_parent, old_name)?;

        // Add to new parent first — if that fails, old state is intact.
        add_child(new_parent, new_name, suid)?;

        // Then remove from old parent. If THIS fails, we've left a
        // duplicate entry in new_parent; recover by removing it.
        if let Err(e) = remove_child(old_parent, old_name).map(|_| ()) {
            let _ = remove_child(new_parent, new_name);
            return Err(e);
        }

        // Bump mtime on the moved object so std::fs::Metadata sees the rename.
        let now = crate::platform::wall_clock().unwrap_or(0);
        let registry = unsafe { global_registry() };
        if let Some(obj) = registry.get_mut(&suid) {
            obj.modified_at = now;
        }
        Ok(())
    }

    /// Set the file's content length to `new_size`. Shrinks (drops
    /// the tail) or extends with zero bytes. Errors with
    /// `ContentTooLarge` if `new_size > MAX_FILE_CONTENT`.
    ///
    /// Sub-256 sizes land in the `Inline` storage variant; larger
    /// sizes go through the heap `Allocated` path (task #44).
    pub fn truncate(path: &str, new_size: usize) -> Result<(), FsError> {
        use crate::semantic::object::MAX_FILE_CONTENT;

        let suid = Self::resolve(path)?;
        let registry = unsafe { global_registry() };
        let obj = registry.get_mut(&suid).ok_or(FsError::NotFound)?;
        if obj.content_type == ContentType::Structured {
            return Err(FsError::IsADirectory);
        }
        if new_size > MAX_FILE_CONTENT { return Err(FsError::ContentTooLarge); }

        if new_size <= 256 {
            // Small path: stack buf + Inline.
            let mut buf = [0u8; 256];
            let existing = obj.content.as_bytes().unwrap_or(&[]);
            let keep = existing.len().min(new_size);
            buf[..keep].copy_from_slice(&existing[..keep]);
            // Tail past `keep` is already zero from the buf init.
            obj.content = ObjectContent::from_inline(&buf[..new_size])
                .ok_or(FsError::ContentTooLarge)?;
        } else {
            // Large path: allocate the final heap block directly and
            // hand it to the Allocated variant. Doing it in one shot
            // (rather than allocate-temp + from_bytes-which-allocates-again)
            // halves the heap pressure for this op.
            let buf = crate::memory::heap::allocate(new_size, 8);
            if buf.is_null() { return Err(FsError::ContentTooLarge); }
            // Copy existing bytes + zero-pad the tail, while the
            // existing borrow is alive — its lifetime ends with the
            // final use below (NLL), before we overwrite `obj.content`.
            {
                let existing = obj.content.as_bytes().unwrap_or(&[]);
                let keep = existing.len().min(new_size);
                // SAFETY: `buf` is a fresh, unaliased `new_size`-byte
                // block; `existing.as_ptr()` is valid for `keep` reads.
                unsafe {
                    core::ptr::copy_nonoverlapping(existing.as_ptr(), buf, keep);
                    if new_size > keep {
                        core::ptr::write_bytes(buf.add(keep), 0, new_size - keep);
                    }
                }
            }
            // Assignment drops the previous content (frees its heap
            // block if it was Allocated) and installs the new one.
            obj.content = ObjectContent::Allocated {
                ptr: buf as usize,
                len: new_size,
                capacity: new_size,
            };
        }
        obj.modified_at = crate::platform::wall_clock().unwrap_or(0);
        Ok(())
    }

    // ========================================================================
    // Stage 2 — persistence
    // ========================================================================

    /// Walk the namespace from [`ROOT_SUID`] and write a packed
    /// serialization into `buf`. Returns the number of bytes written.
    ///
    /// Format (see [`serial`] for details):
    /// ```text
    ///   [magic 4]["FSNS"][version u32][count u32]
    ///   for each object reachable from ROOT_SUID (BFS order):
    ///     [suid 16][tier u8][content_type u8][reserved 2][created_at u64]
    ///     [modified_at u64][content_len u16][content content_len]
    /// ```
    ///
    /// BFS, not DFS, so the root + its immediate children land first
    /// and a partial truncated load can still see a usable subtree.
    pub fn serialize(buf: &mut [u8]) -> Result<usize, FsError> {
        serial::serialize_namespace(buf)
    }

    /// Reverse of [`serialize`]. Walks the packed bytes, inserts each
    /// object into the registry, then verifies the root SUID came back.
    /// Existing namespace state is NOT cleared — callers should reset
    /// the registry first if they want a clean reload.
    pub fn deserialize(buf: &[u8]) -> Result<usize, FsError> {
        serial::deserialize_namespace(buf)
    }

    /// Snapshot the path namespace through the storage layer. Wraps
    /// `crate::storage::snapshot::save_snapshot` with our serializer.
    /// Caller passes a `BlockDevice` — typically `"virtio0"`.
    pub fn save(dev: &dyn crate::drivers::traits::BlockDevice) -> Result<usize, FsError> {
        // Heap-backed scratch (MAX_SNAPSHOT_BYTES is now 1 MiB — far too big for
        // the stack). Freed on every path.
        let cap = crate::storage::snapshot::MAX_SNAPSHOT_BYTES;
        let ptr = crate::memory::heap::allocate(cap, 8);
        if ptr.is_null() {
            return Err(FsError::Corrupt);
        }
        let result = {
            let scratch = unsafe { core::slice::from_raw_parts_mut(ptr, cap) };
            match Self::serialize(scratch) {
                Ok(n) => crate::storage::snapshot::save_snapshot(dev, &scratch[..n])
                    .map(|_| n)
                    .map_err(|_| FsError::Corrupt),
                Err(e) => Err(e),
            }
        };
        crate::memory::heap::deallocate(ptr, cap, 8);
        result
    }

    /// Read a previously-saved snapshot and reconstruct the namespace
    /// in `global_registry()`. Returns the number of bytes consumed.
    pub fn load(dev: &dyn crate::drivers::traits::BlockDevice) -> Result<usize, FsError> {
        let cap = crate::storage::snapshot::MAX_SNAPSHOT_BYTES;
        let ptr = crate::memory::heap::allocate(cap, 8);
        if ptr.is_null() {
            return Err(FsError::Corrupt);
        }
        let result = {
            let scratch = unsafe { core::slice::from_raw_parts_mut(ptr, cap) };
            match crate::storage::snapshot::load_snapshot(dev, scratch) {
                Ok(n) => Self::deserialize(&scratch[..n]).map(|_| n),
                Err(_) => Err(FsError::Corrupt),
            }
        };
        crate::memory::heap::deallocate(ptr, cap, 8);
        result
    }
}

// ============================================================================
// Stage 2 — packed serializer (BFS from root)
// ============================================================================

mod serial {
    use super::*;

    /// On-disk magic so a stale or wrong snapshot can't be loaded as
    /// a namespace. ASCII "FSNS".
    const MAGIC: [u8; 4] = *b"FSNS";
    /// Bump when the field layout changes. v2: content length widened u16→u32
    /// so files over 64 KiB persist (up to MAX_FILE_CONTENT). v1 snapshots are
    /// rejected on load (deserialize checks VERSION) → treated as a fresh disk.
    const VERSION: u32 = 2;
    /// Per-object header size (everything except the trailing content).
    const OBJ_HEADER: usize = 16 + 1 + 1 + 2 + 8 + 8 + 4;
    //                       suid + tier + type + rsvd + ctime + mtime + clen(u32)

    /// Maximum reachable objects we'll serialize in one snapshot. The
    /// registry caps at MAX_OBJECTS (1024) but the path namespace
    /// will be much smaller in practice; bounding here keeps the
    /// BFS queue static.
    const MAX_QUEUE: usize = 256;

    pub fn serialize_namespace(buf: &mut [u8]) -> Result<usize, FsError> {
        if buf.len() < 12 { return Err(FsError::Corrupt); }
        // Header: magic | version | count (count filled at the end).
        buf[0..4].copy_from_slice(&MAGIC);
        buf[4..8].copy_from_slice(&VERSION.to_le_bytes());
        // count placeholder — we know the final value only after BFS
        // walks the tree, so write 0 here and patch at the end.
        buf[8..12].copy_from_slice(&0u32.to_le_bytes());
        let mut cursor = 12usize;
        let mut count = 0u32;

        let mut queue = [SUID::NULL; MAX_QUEUE];
        let mut visited = [SUID::NULL; MAX_QUEUE];
        queue[0] = ROOT_SUID;
        let mut head = 0usize;
        let mut tail = 1usize;

        let registry = unsafe { global_registry() };
        while head < tail {
            let suid = queue[head];
            head += 1;

            // Guard against cycles. The path namespace is a tree by
            // construction (no hardlinks yet) so this should never
            // trigger, but a corrupted directory could send us in
            // circles — bound the work either way.
            if visited[..count as usize].iter().any(|&v| v == suid) { continue; }

            let obj = registry.get(&suid).ok_or(FsError::NotFound)?;
            let content_bytes = obj.content.as_bytes().unwrap_or(&[]);
            let content_len = content_bytes.len();
            if content_len > crate::semantic::object::MAX_FILE_CONTENT {
                return Err(FsError::ContentTooLarge);
            }

            // Bounds-check before writing the object record.
            let record_len = OBJ_HEADER + content_len;
            if cursor + record_len > buf.len() {
                return Err(FsError::ContentTooLarge);
            }

            // suid
            buf[cursor..cursor + 8].copy_from_slice(&suid.high.to_be_bytes());
            buf[cursor + 8..cursor + 16].copy_from_slice(&suid.low.to_be_bytes());
            // tier, content_type
            buf[cursor + 16] = obj.tier as u8;
            buf[cursor + 17] = obj.content_type as u8;
            // reserved
            buf[cursor + 18] = 0;
            buf[cursor + 19] = 0;
            // timestamps
            buf[cursor + 20..cursor + 28].copy_from_slice(&obj.created_at.to_le_bytes());
            buf[cursor + 28..cursor + 36].copy_from_slice(&obj.modified_at.to_le_bytes());
            // content length + body
            buf[cursor + 36..cursor + 40].copy_from_slice(&(content_len as u32).to_le_bytes());
            buf[cursor + 40..cursor + 40 + content_len].copy_from_slice(content_bytes);
            cursor += record_len;

            visited[count as usize] = suid;
            count += 1;

            // BFS: if this is a directory, enqueue its children.
            if obj.content_type == ContentType::Structured && !content_bytes.is_empty() {
                for entry in DirEntries::parse(content_bytes)? {
                    let (_, child) = entry?;
                    if tail >= queue.len() { return Err(FsError::DirectoryFull); }
                    queue[tail] = child;
                    tail += 1;
                }
            }
        }

        // Patch the count field at offset 8.
        buf[8..12].copy_from_slice(&count.to_le_bytes());
        Ok(cursor)
    }

    pub fn deserialize_namespace(buf: &[u8]) -> Result<usize, FsError> {
        if buf.len() < 12 { return Err(FsError::Corrupt); }
        if &buf[0..4] != &MAGIC { return Err(FsError::Corrupt); }
        let version = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        if version != VERSION { return Err(FsError::Corrupt); }
        let count = u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize;

        let registry = unsafe { global_registry() };
        let mut cursor = 12usize;
        for _ in 0..count {
            if cursor + OBJ_HEADER > buf.len() { return Err(FsError::Corrupt); }
            let suid_high = u64::from_be_bytes(buf[cursor..cursor + 8].try_into().unwrap());
            let suid_low = u64::from_be_bytes(buf[cursor + 8..cursor + 16].try_into().unwrap());
            let suid = SUID::new(suid_high, suid_low);
            let tier_raw = buf[cursor + 16];
            let ctype_raw = buf[cursor + 17];
            let created_at = u64::from_le_bytes(buf[cursor + 20..cursor + 28].try_into().unwrap());
            let modified_at = u64::from_le_bytes(buf[cursor + 28..cursor + 36].try_into().unwrap());
            let content_len = u32::from_le_bytes(buf[cursor + 36..cursor + 40].try_into().unwrap()) as usize;
            cursor += OBJ_HEADER;
            if cursor + content_len > buf.len() { return Err(FsError::Corrupt); }
            let content_slice = &buf[cursor..cursor + content_len];
            cursor += content_len;

            let tier = match tier_raw {
                0 => SecurityTier::Public,
                1 => SecurityTier::Internal,
                2 => SecurityTier::Sensitive,
                3 => SecurityTier::Secret,
                _ => return Err(FsError::Corrupt),
            };
            let ctype = match ctype_raw {
                0 => ContentType::Binary,
                1 => ContentType::Text,
                2 => ContentType::Vector,
                3 => ContentType::Structured,
                4 => ContentType::Reference,
                _ => return Err(FsError::Corrupt),
            };
            // If an object with this SUID already exists in the registry
            // (e.g. ROOT_SUID was installed by Namespace::init), update
            // it in place instead of failing on duplicate insert.
            if let Some(existing) = registry.get_mut(&suid) {
                existing.tier = tier;
                existing.content_type = ctype;
                existing.created_at = created_at;
                existing.modified_at = modified_at;
                // Heap-backed (from_bytes) so restored content over 256 B —
                // notably installed ELFs and large directories — round-trips.
                existing.content = ObjectContent::from_bytes(content_slice)
                    .ok_or(FsError::ContentTooLarge)?;
            } else {
                let mut obj = SemanticObject::new(suid, tier, 0);
                obj.content_type = ctype;
                obj.created_at = created_at;
                obj.modified_at = modified_at;
                obj.content = ObjectContent::from_bytes(content_slice)
                    .ok_or(FsError::ContentTooLarge)?;
                if !registry.insert(obj) { return Err(FsError::RegistryFull); }
            }
        }
        Ok(cursor)
    }
}

// ----------------------------------------------------------------------------
// Internal helpers
// ----------------------------------------------------------------------------

/// Look up one path component in the directory at `parent_suid`.
/// Errors if the parent isn't a directory or the name isn't present.
fn lookup_in_dir(parent_suid: SUID, name: &str) -> Result<SUID, FsError> {
    let registry = unsafe { global_registry() };
    let parent = registry.get(&parent_suid).ok_or(FsError::NotFound)?;
    if parent.content_type != ContentType::Structured {
        return Err(FsError::NotADirectory);
    }
    let bytes = parent.content.as_bytes().unwrap_or(&[]);
    if bytes.is_empty() { return Err(FsError::NotFound); }
    for entry in DirEntries::parse(bytes)? {
        let (existing_name, child) = entry?;
        if existing_name == name { return Ok(child); }
    }
    Err(FsError::NotFound)
}

/// Append `(name, suid)` to the directory at `parent_suid`. Rewrites
/// the parent's content with the new packed bytes.
fn add_child(parent_suid: SUID, name: &str, suid: SUID) -> Result<(), FsError> {
    // Snapshot existing content into a local buffer so we can reborrow
    // the registry mutably for the rewrite.
    let mut scratch = [0u8; DIR_CONTENT_MAX];
    let existing_len = {
        let registry = unsafe { global_registry() };
        let parent = registry.get(&parent_suid).ok_or(FsError::NotFound)?;
        if parent.content_type != ContentType::Structured {
            return Err(FsError::NotADirectory);
        }
        let bytes = parent.content.as_bytes().unwrap_or(&[]);
        if bytes.len() > scratch.len() { return Err(FsError::Corrupt); }
        scratch[..bytes.len()].copy_from_slice(bytes);
        bytes.len()
    };

    let mut new_buf = [0u8; DIR_CONTENT_MAX];
    let new_len = insert_dir_entry(&scratch[..existing_len], name, suid, &mut new_buf)?;

    let registry = unsafe { global_registry() };
    let parent = registry.get_mut(&parent_suid).ok_or(FsError::NotFound)?;
    // Heap-backed (from_bytes) so a directory isn't capped at 256 B inline.
    parent.content = ObjectContent::from_bytes(&new_buf[..new_len])
        .ok_or(FsError::DirectoryFull)?;
    Ok(())
}

/// Remove the entry named `name` from the directory at `parent_suid`.
/// Returns the SUID of the entry that was removed.
fn remove_child(parent_suid: SUID, name: &str) -> Result<SUID, FsError> {
    let mut scratch = [0u8; DIR_CONTENT_MAX];
    let existing_len = {
        let registry = unsafe { global_registry() };
        let parent = registry.get(&parent_suid).ok_or(FsError::NotFound)?;
        if parent.content_type != ContentType::Structured {
            return Err(FsError::NotADirectory);
        }
        let bytes = parent.content.as_bytes().unwrap_or(&[]);
        if bytes.len() > scratch.len() { return Err(FsError::Corrupt); }
        scratch[..bytes.len()].copy_from_slice(bytes);
        bytes.len()
    };

    let mut new_buf = [0u8; DIR_CONTENT_MAX];
    let (new_len, removed_suid) =
        remove_dir_entry(&scratch[..existing_len], name, &mut new_buf)?;

    let registry = unsafe { global_registry() };
    let parent = registry.get_mut(&parent_suid).ok_or(FsError::NotFound)?;
    parent.content = ObjectContent::from_bytes(&new_buf[..new_len])
        .ok_or(FsError::Corrupt)?;
    Ok(removed_suid)
}

/// Generate a fresh SUID for a new object. RDRAND-backed (Stage 2) so
/// SUIDs persisted across boots don't collide with newly-minted ones.
///
/// If the platform RNG fails (no RDRAND, e.g. `NullPlatform`), fall
/// back to a counter + boot-tick scramble. That's only safe for
/// in-memory-only use — persistent state would risk collision. Worth
/// failing loudly here once we have a way to signal it; today we
/// log and continue with the degraded SUID.
fn mint_suid() -> SUID {
    let mut buf = [0u8; 16];
    if crate::platform::random_bytes(&mut buf).is_ok() {
        let high = u64::from_be_bytes(buf[0..8].try_into().unwrap());
        let low = u64::from_be_bytes(buf[8..16].try_into().unwrap());
        // Stamp the random-type nibble in the top of `high` so it can't
        // collide with content-addressed (type 0) or system (type 15)
        // SUIDs. Bits 60-63 = 0b0001 (TYPE_RANDOM); preserve the rest.
        let high = (high & 0x0FFF_FFFF_FFFF_FFFF) | (1u64 << 60);
        return SUID::new(high, low);
    }
    // Degraded fallback for platforms without RDRAND (testing only).
    crate::platform::log("[fs] mint_suid: RDRAND unavailable, using counter fallback\n");
    use core::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let high = (1u64 << 60) | (n & 0x0FFF_FFFF_FFFF_FFFF);
    let low = n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    SUID::new(high, low)
}
