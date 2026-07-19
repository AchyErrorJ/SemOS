use crate::bundle::{load_bundle, sync_hashes};
use crate::manifest::LeafKind;
use crate::{Result, SheafError, Suid};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn export_md(bundle: &Path, out: &Path) -> Result<()> {
    let info = load_bundle(bundle)?;
    let default = bundle.join(&info.manifest.default_facet);
    let title = &info.manifest.title;
    let mut s = String::new();
    s.push_str(&format!(
        "<!-- sheaf://{} · exported {} · from {} -->\n\n",
        info.manifest.suid,
        crate::iso_now(),
        crate::toml::quote(title),
    ));
    s.push_str(&std::fs::read_to_string(&default)?);
    s.push_str("\n\n---\n\n");
    s.push_str("Sheaf facets:\n");
    for f in info.manifest.facets.values() {
        if f.path == info.manifest.default_facet { continue; }
        match f.leaf {
            LeafKind::Text if f.role.to_string() == "agent" => {
                s.push_str(&format!("- `{}` agent profile (not inlined)\n", f.path));
            }
            LeafKind::Text => s.push_str(&format!("- `{}` text facet\n", f.path)),
            LeafKind::Blob => s.push_str(&format!("- `{}` blob facet\n", f.path)),
        }
    }
    crate::atomic_write(out, s.as_bytes())?;
    let hash = crate::sha256::file_hex(out)?;
    crate::provenance::append_export(bundle, "md", &hash, "user")?;
    Ok(())
}

pub fn export_sheaf(bundle: &Path, out: &Path) -> Result<()> {
    let mut info = load_bundle(bundle)?;
    sync_hashes(bundle, &mut info.manifest)?;
    info.manifest.save(&crate::bundle_manifest_path(bundle))?;

    let mut files = Vec::new();
    collect_files(bundle, bundle, &mut files)?;
    let mut w = std::fs::File::create(out)?;
    for rel in files {
        let full = bundle.join(&rel);
        let data = std::fs::read(&full)?;
        write_header(&mut w, &rel.to_string_lossy(), data.len() as u64)?;
        w.write_all(&data)?;
        let pad = (512 - (data.len() % 512)) % 512;
        if pad > 0 { w.write_all(&vec![0u8; pad])?; }
    }
    w.write_all(&[0u8; 1024])?;
    w.flush()?;
    let hash = crate::sha256::file_hex(out)?;
    crate::provenance::append_export(bundle, "sheaf", &hash, "user")?;
    Ok(())
}

pub fn import_sheaf(archive: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        return Err(SheafError::Invalid(format!("destination exists: {}", dest.display())));
    }
    std::fs::create_dir_all(dest)?;
    let bytes = std::fs::read(archive)?;
    let mut i = 0usize;
    while i + 512 <= bytes.len() {
        let hdr = &bytes[i..i+512];
        if hdr.iter().all(|&b| b == 0) { break; }
        let name = read_cstr(&hdr[0..100]);
        let size = parse_octal(&hdr[124..136]) as usize;
        i += 512;
        if name.starts_with('/') || name.contains("..") {
            return Err(SheafError::Invalid(format!("archive path escapes: {name}")));
        }
        let out = dest.join(&name);
        if let Some(parent) = out.parent() { std::fs::create_dir_all(parent)?; }
        if i + size > bytes.len() { return Err(SheafError::Parse("truncated tar".into())); }
        std::fs::write(out, &bytes[i..i+size])?;
        i += size;
        i += (512 - (size % 512)) % 512;
    }

    // Importing a .sheaf creates a copy: mint a new SUID and remember parent.
    let manifest_path = crate::bundle_manifest_path(dest);
    let mut m = crate::manifest::BundleManifest::load(&manifest_path)?;
    let parent = m.suid;
    m.suid = Suid::mint()?;
    m.derived_from = Some(parent);
    m.modified = crate::iso_now();
    sync_hashes(dest, &mut m)?;
    m.save(&manifest_path)?;
    std::fs::write(dest.join("provenance.toml"), crate::provenance::initial(m.suid, Some(parent)))?;
    Ok(())
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if path.is_file() {
            out.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
    out.sort();
    Ok(())
}

fn write_header(w: &mut std::fs::File, name: &str, size: u64) -> Result<()> {
    if name.len() > 100 { return Err(SheafError::Invalid(format!("tar path too long: {name}"))); }
    let mut h = [0u8; 512];
    h[0..name.len()].copy_from_slice(name.as_bytes());
    write_octal(&mut h[100..108], 0o644);
    write_octal(&mut h[108..116], 0);
    write_octal(&mut h[116..124], 0);
    write_octal(&mut h[124..136], size);
    write_octal(&mut h[136..148], crate::unix_now());
    for b in &mut h[148..156] { *b = b' '; }
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum: u32 = h.iter().map(|&b| b as u32).sum();
    let chk = format!("{:06o}\0 ", sum);
    h[148..156].copy_from_slice(chk.as_bytes());
    w.write_all(&h)?;
    Ok(())
}

fn write_octal(dst: &mut [u8], n: u64) {
    for b in dst.iter_mut() { *b = 0; }
    let s = format!("{:0width$o}", n, width = dst.len() - 1);
    let b = s.as_bytes();
    let start = dst.len().saturating_sub(1 + b.len());
    dst[start..start + b.len()].copy_from_slice(b);
}

fn read_cstr(s: &[u8]) -> String {
    let n = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    String::from_utf8_lossy(&s[..n]).to_string()
}

fn parse_octal(s: &[u8]) -> u64 {
    let mut n = 0u64;
    for &b in s {
        if (b'0'..=b'7').contains(&b) {
            n = n * 8 + (b - b'0') as u64;
        }
    }
    n
}

