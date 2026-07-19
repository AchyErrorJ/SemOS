use crate::agent::AgentProfile;
use crate::manifest::{default_leaf_for_path, default_mime, default_role_for_path, BundleManifest, Facet, LeafKind, Role};
use crate::{Result, SheafError, Suid};
use std::collections::BTreeSet;
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

