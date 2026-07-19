//! Phase 0 acceptance tests mapped to docs/SHEAF_PLAN.md §10.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use sheaf::bundle;
use sheaf::manifest::{BundleManifest, LeafKind, Role};
use sheaf::LintLevel;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn tmp(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("sheaf-test-{}-{}-{}", std::process::id(), n, name));
    let _ = std::fs::remove_dir_all(&p);
    p
}

fn has_error(issues: &[sheaf::LintIssue], needle: &str) -> bool {
    issues.iter().any(|i| i.level == LintLevel::Error && i.message.contains(needle))
}

#[test]
fn test1_new_bundle_lints_clean() {
    let dir = tmp("new");
    let info = bundle::new_document(&dir, Some("Doc")).unwrap();
    assert_eq!(info.manifest.default_facet, "content.md");
    assert!(dir.join("content.md").is_file());
    let issues = bundle::lint_bundle(&dir).unwrap();
    assert!(issues.is_empty(), "unexpected: {issues:?}");
}

#[test]
fn test2_rename_keeps_suid() {
    let dir = tmp("rename-a");
    let info = bundle::new_document(&dir, Some("Doc")).unwrap();
    let suid = info.manifest.suid;
    let dir2 = tmp("rename-b");
    std::fs::rename(&dir, &dir2).unwrap();
    let reloaded = bundle::load_bundle(&dir2).unwrap();
    assert_eq!(reloaded.manifest.suid, suid);
    assert!(bundle::lint_bundle(&dir2).unwrap().is_empty());
}

#[test]
fn test3_export_import_copies_with_new_suid() {
    let dir = tmp("src");
    let info = bundle::new_document(&dir, Some("Doc")).unwrap();
    let orig = info.manifest.suid;

    let archive = std::env::temp_dir().join(format!("sheaf-test-{}-arc.sheaf", std::process::id()));
    let _ = std::fs::remove_file(&archive);
    sheaf::export::export_sheaf(&dir, &archive).unwrap();

    let dest = tmp("dest");
    sheaf::export::import_sheaf(&archive, &dest).unwrap();
    let imported = bundle::load_bundle(&dest).unwrap();
    assert_ne!(imported.manifest.suid, orig, "import must mint a new suid");
    assert_eq!(imported.manifest.derived_from, Some(orig));
    assert!(bundle::lint_bundle(&dest).unwrap().is_empty(), "imported bundle should lint clean");
}

#[test]
fn test4_blob_without_sidecar_fails() {
    let dir = tmp("blob");
    bundle::new_document(&dir, Some("Doc")).unwrap();
    let src = dir.join("content.md"); // any file as fake blob source
    bundle::add_facet(&dir, &src, "image.png", Some(Role::Payload), 0).unwrap();
    assert!(bundle::lint_bundle(&dir).unwrap().is_empty(), "sidecar should exist after add");
    std::fs::remove_file(dir.join("image.png.toml")).unwrap();
    let issues = bundle::lint_bundle(&dir).unwrap();
    assert!(has_error(&issues, "blob without sidecar"), "got {issues:?}");
}

#[test]
fn test5_facet_tier_above_ceiling_fails() {
    let dir = tmp("tier");
    bundle::new_document(&dir, Some("Doc")).unwrap();
    let mut m = BundleManifest::load(&sheaf::bundle_manifest_path(&dir)).unwrap();
    m.facets.get_mut("content.md").unwrap().tier = 3; // bundle tier is 0
    m.save(&sheaf::bundle_manifest_path(&dir)).unwrap();
    let issues = bundle::lint_bundle(&dir).unwrap();
    assert!(has_error(&issues, "facet tier"), "got {issues:?}");
}

#[test]
fn test_agent_leaf_rules() {
    let dir = tmp("agent");
    let mut info = bundle::new_document(&dir, Some("Doc")).unwrap();
    info.manifest.tier = 1;
    info.manifest.save(&sheaf::bundle_manifest_path(&dir)).unwrap();

    // name mismatch + max_tier over facet tier.
    std::fs::write(dir.join("reviewer.agent"),
        b"schema = 1\nname = \"other\"\nmax_tier = 3\ntools = [\"read_facet\"]\ninputs = [\"content.md\"]\noutputs = [\"review.md\"]\n").unwrap();
    let mut m = BundleManifest::load(&sheaf::bundle_manifest_path(&dir)).unwrap();
    m.facets.insert("reviewer.agent".into(), sheaf::manifest::Facet {
        path: "reviewer.agent".into(),
        leaf: LeafKind::Text,
        role: Role::Agent,
        tier: 1,
        mime: "application/sheaf-agent+toml".into(),
        sha256: None,
    });
    bundle::sync_hashes(&dir, &mut m).unwrap();
    m.save(&sheaf::bundle_manifest_path(&dir)).unwrap();

    let issues = bundle::lint_bundle(&dir).unwrap();
    assert!(has_error(&issues, "name must match filename stem"), "got {issues:?}");
    assert!(has_error(&issues, "max_tier"), "got {issues:?}");
}

#[test]
fn test9_plain_folder_is_not_a_bundle() {
    let dir = tmp("plain");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("note.md"), b"hi").unwrap();
    assert!(!sheaf::is_bundle_dir(&dir));
    assert!(bundle::load_bundle(&dir).is_err());
}
