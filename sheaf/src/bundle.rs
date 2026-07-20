use crate::agent::AgentProfile;
use crate::manifest::{default_leaf_for_path, default_mime, default_role_for_path, BundleManifest, Facet, LeafKind, Role};
use crate::{Result, SheafError, Suid};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct BundleInfo {
    pub path: PathBuf,
    pub manifest: BundleManifest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LintLevel { Warn, Error }

#[derive(Clone, Debug)]
pub struct LintIssue {
    pub level: LintLevel,
    pub message: String,
}

impl LintIssue {
    pub fn error(message: impl Into<String>) -> Self {
        Self { level: LintLevel::Error, message: message.into() }
    }
    pub fn warn(message: impl Into<String>) -> Self {
        Self { level: LintLevel::Warn, message: message.into() }
    }
}

pub fn load_bundle(path: &Path) -> Result<BundleInfo> {
    if !crate::is_bundle_dir(path) {
        return Err(SheafError::NotBundle(path.to_path_buf()));
    }
    let manifest = BundleManifest::load(&crate::bundle_manifest_path(path))?;
    Ok(BundleInfo { path: path.to_path_buf(), manifest })
}

pub fn new_document(path: &Path, title: Option<&str>) -> Result<BundleInfo> {
    if path.exists() {
        return Err(SheafError::Invalid(format!("path exists: {}", path.display())));
    }
    std::fs::create_dir_all(path)?;
    let now = crate::iso_now();
    let title = title.unwrap_or_else(|| path.file_name().and_then(|s| s.to_str()).unwrap_or("Untitled"));
    let mut manifest = BundleManifest::new_doc(title, Suid::mint()?, &now);
    std::fs::write(path.join("content.md"), format!("# {title}\n\n").as_bytes())?;
    std::fs::write(path.join("provenance.toml"), crate::provenance::initial(manifest.suid, None))?;
    sync_hashes(path, &mut manifest)?;
    manifest.save(&crate::bundle_manifest_path(path))?;
    Ok(BundleInfo { path: path.to_path_buf(), manifest })
}

pub fn add_facet(bundle: &Path, src: &Path, dest: &str, role: Option<Role>, tier: u8) -> Result<()> {
    let mut info = load_bundle(bundle)?;
    validate_rel_path(dest)?;
    let dst = bundle.join(dest);
    if let Some(parent) = dst.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::copy(src, &dst)?;
    let leaf = default_leaf_for_path(dest);
    let role = role.unwrap_or_else(|| default_role_for_path(dest));
    let mime = default_mime(dest, &leaf, &role);
    info.manifest.facets.insert(dest.into(), Facet {
        path: dest.into(), leaf, role, tier, mime, sha256: None,
    });
    if matches!(info.manifest.facets[dest].leaf, LeafKind::Blob) {
        let sidecar = sidecar_name(dest);
        let sidecar_path = bundle.join(&sidecar);
        if !sidecar_path.exists() {
            let bytes = std::fs::metadata(&dst)?.len();
            let hash = crate::sha256::file_hex(&dst)?;
            std::fs::write(sidecar_path, format!(
                "schema = 1\nmime = \"{}\"\nsha256 = \"{}\"\nbytes = {}\ntitle = \"{}\"\n",
                info.manifest.facets[dest].mime,
                hash,
                bytes,
                dest,
            ))?;
        }
    }
    info.manifest.modified = crate::iso_now();
    sync_hashes(bundle, &mut info.manifest)?;
    info.manifest.save(&crate::bundle_manifest_path(bundle))
}

pub fn remove_facet(bundle: &Path, facet: &str) -> Result<()> {
    let mut info = load_bundle(bundle)?;
    if facet == "bundle.toml" {
        return Err(SheafError::Invalid("cannot remove bundle.toml".into()));
    }
    validate_rel_path(facet)?;
    info.manifest.facets.remove(facet);
    let p = bundle.join(facet);
    if p.exists() { std::fs::remove_file(p)?; }
    let sidecar = bundle.join(sidecar_name(facet));
    if sidecar.exists() { let _ = std::fs::remove_file(sidecar); }
    info.manifest.modified = crate::iso_now();
    sync_hashes(bundle, &mut info.manifest)?;
    info.manifest.save(&crate::bundle_manifest_path(bundle))
}

pub fn lint_bundle(path: &Path) -> Result<Vec<LintIssue>> {
    let info = load_bundle(path)?;
    let mut issues = Vec::new();
    let m = &info.manifest;
    if m.schema != 1 { issues.push(LintIssue::error(format!("unsupported schema {}", m.schema))); }
    if !m.facets.contains_key(&m.default_facet) {
        issues.push(LintIssue::error(format!("default_facet missing: {}", m.default_facet)));
    } else if m.facets[&m.default_facet].role != Role::Payload {
        issues.push(LintIssue::error("default_facet must have role=payload"));
    }
    for (name, facet) in &m.facets {
        if validate_rel_path(name).is_err() {
            issues.push(LintIssue::error(format!("facet path escapes bundle: {name}")));
        }
        if facet.tier > m.tier {
            issues.push(LintIssue::error(format!("{name}: facet tier {} > bundle tier {}", facet.tier, m.tier)));
        }
        let full = path.join(name);
        if !full.is_file() {
            issues.push(LintIssue::error(format!("{name}: missing facet file")));
            continue;
        }
        if let Some(expected) = &facet.sha256 {
            match crate::sha256::file_hex(&full) {
                Ok(actual) if &actual == expected => {}
                Ok(actual) => issues.push(LintIssue::error(format!("{name}: sha256 mismatch expected {expected} got {actual}"))),
                Err(e) => issues.push(LintIssue::error(format!("{name}: hash failed: {e}"))),
            }
        } else if facet.leaf == LeafKind::Blob {
            issues.push(LintIssue::error(format!("{name}: blob facet missing sha256")));
        }
        match facet.leaf {
            LeafKind::Blob => {
                if !path.join(sidecar_name(name)).is_file() {
                    issues.push(LintIssue::error(format!("{name}: blob without sidecar")));
                }
            }
            LeafKind::Text => {
                if std::str::from_utf8(&std::fs::read(&full).unwrap_or_default()).is_err() {
                    issues.push(LintIssue::error(format!("{name}: text facet is not UTF-8")));
                }
                if name.ends_with(".agent") {
                    if facet.role != Role::Agent {
                        issues.push(LintIssue::error(format!("{name}: .agent must have role=agent")));
                    }
                    lint_agent(path, name, facet, &mut issues);
                }
            }
        }
    }

    // Loose blob files in bundle root without sidecar/manifest entry.
    let declared: BTreeSet<_> = m.facets.keys().cloned().collect();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) else { continue };
        if name == "bundle.toml" || name.ends_with(".tmp") { continue; }
        if entry.path().is_file() && !declared.contains(&name) && is_probable_blob(&name) && !name.ends_with(".toml") {
            issues.push(LintIssue::error(format!("{name}: loose blob without manifest facet")));
        }
    }

    Ok(issues)
}

fn lint_agent(bundle: &Path, name: &str, facet: &Facet, issues: &mut Vec<LintIssue>) {
    let text = match std::fs::read_to_string(bundle.join(name)) {
        Ok(s) => s,
        Err(e) => {
            issues.push(LintIssue::error(format!("{name}: cannot read agent profile: {e}")));
            return;
        }
    };
    let profile = match AgentProfile::parse(&text) {
        Ok(p) => p,
        Err(e) => {
            issues.push(LintIssue::error(format!("{name}: invalid agent profile: {e}")));
            return;
        }
    };
    let stem = name.strip_suffix(".agent").unwrap_or(name);
    if profile.name != stem {
        issues.push(LintIssue::error(format!("{name}: name must match filename stem {stem:?}")));
    }
    if profile.max_tier > facet.tier {
        issues.push(LintIssue::error(format!("{name}: max_tier {} > facet tier {}", profile.max_tier, facet.tier)));
    }
    for msg in profile.lint_paths() {
        issues.push(LintIssue::error(format!("{name}: {msg}")));
    }
}

pub fn sync_hashes(bundle: &Path, manifest: &mut BundleManifest) -> Result<()> {
    for facet in manifest.facets.values_mut() {
        let full = bundle.join(&facet.path);
        if full.is_file() {
            facet.sha256 = Some(crate::sha256::file_hex(&full)?);
        }
    }
    Ok(())
}

pub fn verify_bundle(path: &Path) -> Result<Vec<LintIssue>> {
    lint_bundle(path)
}

pub fn validate_rel_path(path: &str) -> Result<()> {
    if path.is_empty() || path.starts_with('/') || path.contains("..") || path.contains('\\') {
        return Err(SheafError::Invalid(format!("bad relative path {path:?}")));
    }
    Ok(())
}

pub fn sidecar_name(path: &str) -> String {
    format!("{path}.toml")
}

fn is_probable_blob(name: &str) -> bool {
    !matches!(name.rsplit('.').next().unwrap_or(""), "md" | "toml" | "css" | "agent" | "txt")
}

/// Collect regular files under `dir`, as bundle-root-relative slash paths.
pub fn collect_rel_files(root: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    collect_rel(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_rel(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rel(root, &path, out)?;
        } else if path.is_file() {
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            out.push(rel);
        }
    }
    Ok(())
}

/// Is `name` the auto-sidecar of a blob file that is also present in `files`?
/// (e.g. `image.png.toml` next to `image.png`.)
fn is_blob_sidecar(name: &str, files: &BTreeSet<String>) -> bool {
    match name.strip_suffix(".toml") {
        Some(base) if !base.is_empty() && files.contains(base) => {
            default_leaf_for_path(base) == LeafKind::Blob
        }
        _ => false,
    }
}

fn ensure_blob_sidecar(bundle: &Path, facet: &Facet) -> Result<()> {
    let sidecar = bundle.join(sidecar_name(&facet.path));
    if sidecar.exists() {
        return Ok(());
    }
    let full = bundle.join(&facet.path);
    let bytes = std::fs::metadata(&full)?.len();
    let hash = crate::sha256::file_hex(&full)?;
    if let Some(parent) = sidecar.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        sidecar,
        format!(
            "schema = 1\nmime = \"{}\"\nsha256 = \"{}\"\nbytes = {}\ntitle = \"{}\"\n",
            facet.mime, hash, bytes, facet.path,
        ),
    )?;
    Ok(())
}

/// Pick the default (payload) facet for a packed/repaired bundle.
fn pick_default_facet(facets: &BTreeMap<String, Facet>) -> Option<String> {
    if facets.contains_key("content.md") {
        return Some("content.md".into());
    }
    // First existing payload; else first .md; else first facet.
    if let Some((name, _)) = facets.iter().find(|(_, f)| f.role == Role::Payload) {
        return Some(name.clone());
    }
    if let Some((name, _)) = facets.iter().find(|(n, _)| n.ends_with(".md")) {
        return Some(name.clone());
    }
    facets.keys().next().cloned()
}

/// Convert a plain folder of loose files into a bundle (in place).
pub fn pack_folder(path: &Path, title: Option<&str>) -> Result<BundleInfo> {
    if !path.is_dir() {
        return Err(SheafError::Invalid(format!("not a directory: {}", path.display())));
    }
    if crate::is_bundle_dir(path) {
        return Err(SheafError::Invalid(format!("already a bundle: {}", path.display())));
    }

    let files = collect_rel_files(path)?;
    let file_set: BTreeSet<String> = files.iter().cloned().collect();

    let mut facets: BTreeMap<String, Facet> = BTreeMap::new();
    for rel in &files {
        if rel == "bundle.toml" || rel == "provenance.toml" || rel.ends_with(".tmp") {
            continue;
        }
        if is_blob_sidecar(rel, &file_set) {
            continue; // it's a sidecar for a present blob, not its own facet
        }
        let leaf = default_leaf_for_path(rel);
        let role = default_role_for_path(rel);
        let mime = default_mime(rel, &leaf, &role);
        facets.insert(rel.clone(), Facet { path: rel.clone(), leaf, role, tier: 0, mime, sha256: None });
    }
    if facets.is_empty() {
        return Err(SheafError::Invalid("no packable files found".into()));
    }

    let default_facet = pick_default_facet(&facets)
        .ok_or_else(|| SheafError::Invalid("could not determine default facet".into()))?;
    // The default facet must be a payload.
    if let Some(f) = facets.get_mut(&default_facet) {
        f.role = Role::Payload;
    }

    let now = crate::iso_now();
    let title = title
        .map(str::to_string)
        .unwrap_or_else(|| path.file_name().and_then(|s| s.to_str()).unwrap_or("Untitled").to_string());
    let mut manifest = BundleManifest {
        schema: 1,
        suid: Suid::mint()?,
        kind: "document".into(),
        title,
        created: now.clone(),
        modified: now,
        default_facet,
        tier: 0,
        derived_from: None,
        facets,
    };

    let blob_facets: Vec<Facet> = manifest.facets.values().filter(|f| f.leaf == LeafKind::Blob).cloned().collect();
    for f in &blob_facets {
        ensure_blob_sidecar(path, f)?;
    }
    std::fs::write(path.join("provenance.toml"), crate::provenance::initial(manifest.suid, None))?;
    sync_hashes(path, &mut manifest)?;
    manifest.save(&crate::bundle_manifest_path(path))?;
    Ok(BundleInfo { path: path.to_path_buf(), manifest })
}

/// Resync a bundle's manifest to reality: add loose files, drop missing
/// facets, regenerate missing blob sidecars, and refresh hashes. Keeps SUID.
pub fn repair_bundle(path: &Path) -> Result<Vec<String>> {
    let mut info = load_bundle(path)?;
    let mut changes = Vec::new();

    let files = collect_rel_files(path)?;
    let file_set: BTreeSet<String> = files.iter().cloned().collect();

    // Add loose files missing from the manifest.
    for rel in &files {
        if rel == "bundle.toml" || rel == "provenance.toml" || rel.ends_with(".tmp") {
            continue;
        }
        if is_blob_sidecar(rel, &file_set) {
            continue;
        }
        if !info.manifest.facets.contains_key(rel) {
            let leaf = default_leaf_for_path(rel);
            let role = default_role_for_path(rel);
            let mime = default_mime(rel, &leaf, &role);
            info.manifest.facets.insert(rel.clone(), Facet {
                path: rel.clone(), leaf, role, tier: 0, mime, sha256: None,
            });
            changes.push(format!("added facet {rel}"));
        }
    }

    // Drop facets whose files vanished.
    let missing: Vec<String> = info.manifest.facets.keys()
        .filter(|k| !path.join(k).is_file())
        .cloned()
        .collect();
    for k in missing {
        info.manifest.facets.remove(&k);
        changes.push(format!("removed facet {k} (file gone)"));
    }

    // Regenerate missing blob sidecars.
    let blob_facets: Vec<Facet> = info.manifest.facets.values()
        .filter(|f| f.leaf == LeafKind::Blob)
        .cloned()
        .collect();
    for f in &blob_facets {
        let sidecar = path.join(sidecar_name(&f.path));
        if !sidecar.exists() {
            ensure_blob_sidecar(path, f)?;
            changes.push(format!("regenerated sidecar {}", sidecar_name(&f.path)));
        }
    }

    // Fix a dangling default facet.
    if !info.manifest.facets.contains_key(&info.manifest.default_facet) {
        if let Some(def) = pick_default_facet(&info.manifest.facets) {
            if let Some(f) = info.manifest.facets.get_mut(&def) {
                f.role = Role::Payload;
            }
            changes.push(format!("default_facet {} -> {}", info.manifest.default_facet, def));
            info.manifest.default_facet = def;
        }
    }

    // Refresh hashes (this is also the "dirty manifest" repair for test 6).
    let before: BTreeMap<String, Option<String>> =
        info.manifest.facets.iter().map(|(k, f)| (k.clone(), f.sha256.clone())).collect();
    sync_hashes(path, &mut info.manifest)?;
    for (k, f) in &info.manifest.facets {
        if before.get(k).and_then(|o| o.clone()) != f.sha256 {
            changes.push(format!("refreshed hash {k}"));
        }
    }

    info.manifest.modified = crate::iso_now();
    info.manifest.save(&crate::bundle_manifest_path(path))?;
    Ok(changes)
}
