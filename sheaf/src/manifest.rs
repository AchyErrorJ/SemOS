use crate::{Result, SheafError, Suid};
use crate::toml::{self, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeafKind { Text, Blob }

impl fmt::Display for LeafKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self { Self::Text => "text", Self::Blob => "blob" })
    }
}

impl FromStr for LeafKind {
    type Err = SheafError;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "text" => Ok(Self::Text),
            "blob" => Ok(Self::Blob),
            _ => Err(SheafError::Parse(format!("unknown leaf kind {s:?}"))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role { Payload, Render, Preview, Meta, Data, Agent }

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Payload => "payload",
            Self::Render => "render",
            Self::Preview => "preview",
            Self::Meta => "meta",
            Self::Data => "data",
            Self::Agent => "agent",
        })
    }
}

impl FromStr for Role {
    type Err = SheafError;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "payload" => Ok(Self::Payload),
            "render" => Ok(Self::Render),
            "preview" => Ok(Self::Preview),
            "meta" => Ok(Self::Meta),
            "data" => Ok(Self::Data),
            "agent" => Ok(Self::Agent),
            _ => Err(SheafError::Parse(format!("unknown role {s:?}"))),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Facet {
    pub path: String,
    pub leaf: LeafKind,
    pub role: Role,
    pub tier: u8,
    pub mime: String,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BundleManifest {
    pub schema: u32,
    pub suid: Suid,
    pub kind: String,
    pub title: String,
    pub created: String,
    pub modified: String,
    pub default_facet: String,
    pub tier: u8,
    pub derived_from: Option<Suid>,
    pub facets: BTreeMap<String, Facet>,
}

impl BundleManifest {
    pub fn new_doc(title: &str, suid: Suid, now: &str) -> Self {
        let mut facets = BTreeMap::new();
        facets.insert("content.md".into(), Facet {
            path: "content.md".into(),
            leaf: LeafKind::Text,
            role: Role::Payload,
            tier: 0,
            mime: "text/markdown".into(),
            sha256: None,
        });
        Self {
            schema: 1,
            suid,
            kind: "document".into(),
            title: title.into(),
            created: now.into(),
            modified: now.into(),
            default_facet: "content.md".into(),
            tier: 0,
            derived_from: None,
            facets,
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        let t = toml::parse(s)?;
        let schema = toml::get_int(&t, "", "schema")? as u32;
        let suid = toml::get_str(&t, "", "suid")?.parse()?;
        let kind = toml::get_str(&t, "", "kind")?.to_string();
        let title = toml::get_str(&t, "", "title")?.to_string();
        let created = toml::get_str(&t, "", "created")?.to_string();
        let modified = toml::get_str(&t, "", "modified")?.to_string();
        let default_facet = toml::get_str(&t, "", "default_facet")?.to_string();
        let tier = toml::get_int(&t, "", "tier")? as u8;
        let derived_from = t.get("").and_then(|root| root.get("derived_from")).and_then(|v| {
            if let Value::Str(s) = v { s.parse().ok() } else { None }
        });

        let mut facets = BTreeMap::new();
        for (sec, vals) in &t {
            let Some(path) = parse_facet_section(sec) else { continue; };
            let leaf = match vals.get("leaf") {
                Some(Value::Str(s)) => s.parse()?,
                _ => default_leaf_for_path(&path),
            };
            let role = match vals.get("role") {
                Some(Value::Str(s)) => s.parse()?,
                _ => Role::Data,
            };
            let tier = match vals.get("tier") {
                Some(Value::Int(i)) => *i as u8,
                _ => 0,
            };
            let mime = match vals.get("mime") {
                Some(Value::Str(s)) => s.clone(),
                _ => default_mime(&path, &leaf, &role),
            };
            let sha256 = match vals.get("sha256") {
                Some(Value::Str(s)) => Some(s.clone()),
                _ => None,
            };
            facets.insert(path.clone(), Facet { path, leaf, role, tier, mime, sha256 });
        }
        Ok(Self { schema, suid, kind, title, created, modified, default_facet, tier, derived_from, facets })
    }

    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("schema = {}\n", self.schema));
        out.push_str(&format!("suid = {}\n", toml::quote(&self.suid.to_string())));
        if let Some(parent) = self.derived_from {
            out.push_str(&format!("derived_from = {}\n", toml::quote(&parent.to_string())));
        }
        out.push_str(&format!("kind = {}\n", toml::quote(&self.kind)));
        out.push_str(&format!("title = {}\n", toml::quote(&self.title)));
        out.push_str(&format!("created = {}\n", toml::quote(&self.created)));
        out.push_str(&format!("modified = {}\n", toml::quote(&self.modified)));
        out.push_str(&format!("default_facet = {}\n", toml::quote(&self.default_facet)));
        out.push_str(&format!("tier = {}\n\n", self.tier));
        for facet in self.facets.values() {
            out.push_str(&format!("[facets.{}]\n", toml::quote(&facet.path)));
            out.push_str(&format!("leaf = {}\n", toml::quote(&facet.leaf.to_string())));
            out.push_str(&format!("role = {}\n", toml::quote(&facet.role.to_string())));
            out.push_str(&format!("tier = {}\n", facet.tier));
            out.push_str(&format!("mime = {}\n", toml::quote(&facet.mime)));
            if let Some(hash) = &facet.sha256 {
                out.push_str(&format!("sha256 = {}\n", toml::quote(hash)));
            }
            out.push('\n');
        }
        out
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&std::fs::read_to_string(path)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        crate::atomic_write(path, self.to_toml().as_bytes())
    }
}

fn parse_facet_section(sec: &str) -> Option<String> {
    let rest = sec.strip_prefix("facets.")?;
    if rest.starts_with('"') && rest.ends_with('"') {
        Some(rest[1..rest.len()-1].replace("\\\"", "\"").replace("\\\\", "\\"))
    } else {
        Some(rest.to_string())
    }
}

pub fn default_leaf_for_path(path: &str) -> LeafKind {
    match path.rsplit('.').next().unwrap_or("") {
        "md" | "toml" | "css" | "agent" | "txt" => LeafKind::Text,
        _ => LeafKind::Blob,
    }
}

pub fn default_role_for_path(path: &str) -> Role {
    match path.rsplit('.').next().unwrap_or("") {
        "agent" => Role::Agent,
        "css" => Role::Render,
        "md" | "txt" => Role::Payload,
        "toml" => Role::Meta,
        _ => Role::Data,
    }
}

pub fn default_mime(path: &str, leaf: &LeafKind, role: &Role) -> String {
    if *role == Role::Agent { return "application/sheaf-agent+toml".into(); }
    match path.rsplit('.').next().unwrap_or("") {
        "md" => "text/markdown",
        "toml" => "text/toml",
        "css" => "text/css",
        "txt" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ if *leaf == LeafKind::Text => "text/plain",
        _ => "application/octet-stream",
    }.into()
}

