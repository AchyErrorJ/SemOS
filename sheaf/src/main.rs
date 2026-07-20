use sheaf::traverse::{self, EntryKind};
use sheaf::manifest::Role;
use sheaf::{Result, LintLevel};
use std::path::Path;

fn main() {
    if let Err(e) = run() {
        eprintln!("sheaf: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args[0] == "help" || args[0] == "--help" {
        usage();
        return Ok(());
    }
    let cmd = args.remove(0);
    match cmd.as_str() {
        "new" => {
            let path = required(&args, 0, "path")?;
            let title = args.get(1).map(String::as_str);
            let info = sheaf::bundle::new_document(Path::new(path), title)?;
            println!("created {} suid={}", info.path.display(), info.manifest.suid);
        }
        "info" => {
            let path = required(&args, 0, "bundle")?;
            let info = sheaf::bundle::load_bundle(Path::new(path))?;
            print_info(&info);
        }
        "lint" => {
            let path = required(&args, 0, "bundle")?;
            let issues = sheaf::bundle::lint_bundle(Path::new(path))?;
            print_issues(&issues);
            if issues.iter().any(|i| i.level == LintLevel::Error) {
                std::process::exit(2);
            }
        }
        "verify" => {
            let path = required(&args, 0, "bundle")?;
            let issues = sheaf::bundle::verify_bundle(Path::new(path))?;
            print_issues(&issues);
            if issues.iter().any(|i| i.level == LintLevel::Error) {
                std::process::exit(2);
            } else {
                println!("verify: OK");
            }
        }
        "open" => {
            let path = required(&args, 0, "bundle")?;
            let info = sheaf::bundle::load_bundle(Path::new(path))?;
            let bytes = std::fs::read_to_string(Path::new(path).join(&info.manifest.default_facet))?;
            print!("{bytes}");
        }
        "edit" => {
            let path = required(&args, 0, "bundle")?;
            let info = sheaf::bundle::load_bundle(Path::new(path))?;
            for f in info.manifest.facets.values() {
                println!("{}\t{}\t{}\ttier={}\t{}", f.path, f.leaf, f.role, f.tier, f.mime);
            }
        }
        "add" => {
            let bundle = required(&args, 0, "bundle")?;
            let src = required(&args, 1, "src")?;
            let dest = required(&args, 2, "dest")?;
            let role = args.get(3).map(|s| s.parse()).transpose()?;
            sheaf::bundle::add_facet(Path::new(bundle), Path::new(src), dest, role, 0)?;
        }
        "rm" => {
            let bundle = required(&args, 0, "bundle")?;
            let facet = required(&args, 1, "facet")?;
            sheaf::bundle::remove_facet(Path::new(bundle), facet)?;
        }
        "export" => {
            let bundle = required(&args, 0, "bundle")?;
            let fmt = required(&args, 1, "md|sheaf")?;
            let out = required(&args, 2, "out")?;
            match fmt {
                "md" => sheaf::export::export_md(Path::new(bundle), Path::new(out))?,
                "sheaf" => sheaf::export::export_sheaf(Path::new(bundle), Path::new(out))?,
                _ => return Err(sheaf::SheafError::Invalid(format!("unknown export format {fmt}"))),
            }
            println!("exported {out}");
        }
        "import" => {
            let archive = required(&args, 0, "archive.sheaf")?;
            let dest = required(&args, 1, "dest")?;
            sheaf::export::import_sheaf(Path::new(archive), Path::new(dest))?;
            println!("imported {dest}");
        }
        "agent" => {
            let bundle = required(&args, 0, "bundle")?;
            let name = required(&args, 1, "name")?;
            show_agent(Path::new(bundle), name)?;
        }
        "pack" => {
            let path = required(&args, 0, "folder")?;
            let title = args.get(1).map(String::as_str);
            let info = sheaf::bundle::pack_folder(Path::new(path), title)?;
            println!("packed {} suid={}", info.path.display(), info.manifest.suid);
            println!("default facet: {}", info.manifest.default_facet);
        }
        "repair" => {
            let path = required(&args, 0, "bundle")?;
            let changes = sheaf::bundle::repair_bundle(Path::new(path))?;
            if changes.is_empty() {
                println!("repair: nothing to do");
            } else {
                for c in &changes {
                    println!("repair: {c}");
                }
            }
        }
        "find" => {
            let root = required(&args, 0, "root")?;
            let contents = args.iter().any(|a| a == "--contents");
            for e in traverse::find(Path::new(root), contents)? {
                let tag = match e.kind {
                    EntryKind::Bundle => "bundle",
                    EntryKind::Dir => "dir",
                    EntryKind::File => "file",
                };
                println!("{}\t{}", tag, e.path.display());
            }
        }
        _ => {
            usage();
            return Err(sheaf::SheafError::Invalid(format!("unknown command {cmd}")));
        }
    }
    Ok(())
}

fn usage() {
    eprintln!("usage:");
    eprintln!("  sheaf new <bundle-dir> [title]");
    eprintln!("  sheaf info|lint|verify|open|edit <bundle-dir>");
    eprintln!("  sheaf add <bundle-dir> <src-file> <facet-path> [role]");
    eprintln!("  sheaf rm <bundle-dir> <facet-path>");
    eprintln!("  sheaf export <bundle-dir> md|sheaf <out>");
    eprintln!("  sheaf import <archive.sheaf> <dest-dir>");
    eprintln!("  sheaf agent <bundle-dir> <name>");
    eprintln!("  sheaf pack <folder> [title]");
    eprintln!("  sheaf repair <bundle-dir>");
    eprintln!("  sheaf find <root> [--contents]");
}

fn required<'a>(args: &'a [String], idx: usize, name: &str) -> Result<&'a str> {
    args.get(idx)
        .map(String::as_str)
        .ok_or_else(|| sheaf::SheafError::Missing(name.into()))
}

fn print_info(info: &sheaf::BundleInfo) {
    println!("bundle: {}", info.path.display());
    println!("suid: {}", info.manifest.suid);
    if let Some(parent) = info.manifest.derived_from {
        println!("derived_from: {parent}");
    }
    println!("kind: {}", info.manifest.kind);
    println!("title: {}", info.manifest.title);
    println!("tier: {}", info.manifest.tier);
    println!("default: {}", info.manifest.default_facet);
    println!("facets:");
    for f in info.manifest.facets.values() {
        println!("  - {} [{} {} tier={}]", f.path, f.leaf, f.role, f.tier);
    }
}

fn print_issues(issues: &[sheaf::LintIssue]) {
    if issues.is_empty() {
        println!("lint: OK");
    } else {
        for i in issues {
            let level = match i.level { LintLevel::Warn => "WARN", LintLevel::Error => "ERROR" };
            println!("{level}: {}", i.message);
        }
    }
}

fn show_agent(bundle: &Path, name: &str) -> Result<()> {
    let info = sheaf::bundle::load_bundle(bundle)?;
    let facet_name = format!("{name}.agent");
    let facet = info.manifest.facets.get(&facet_name)
        .ok_or_else(|| sheaf::SheafError::Missing(facet_name.clone()))?;
    if facet.role != Role::Agent {
        return Err(sheaf::SheafError::Invalid(format!("{facet_name} is not role=agent")));
    }
    let text = std::fs::read_to_string(bundle.join(&facet_name))?;
    let profile = sheaf::agent::AgentProfile::parse(&text)?;
    let effective = info.manifest.tier.min(facet.tier).min(profile.max_tier);
    println!("agent: {}", profile.name);
    println!("purpose: {}", profile.purpose);
    println!("model: {}", profile.model);
    println!("requested max_tier: {}", profile.max_tier);
    println!("effective tier ceiling (without caller): {}", effective);
    println!("tools: {}", profile.tools.join(", "));
    println!("inputs: {}", profile.inputs.join(", "));
    println!("outputs: {}", profile.outputs.join(", "));
    println!("requires_human_confirm: {}", profile.requires_human_confirm);
    println!("network: {}", profile.network);
    println!("dry-run only: no LLM/backend execution in Phase 0");
    Ok(())
}
