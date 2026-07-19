use crate::{Result, SheafError};

#[derive(Clone, Debug)]
pub struct AgentProfile {
    pub schema: u32,
    pub name: String,
    pub purpose: String,
    pub model: String,
    pub max_tier: u8,
    pub tools: Vec<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub requires_human_confirm: bool,
    pub network: bool,
}

impl AgentProfile {
    pub fn parse(s: &str) -> Result<Self> {
        let t = crate::toml::parse(s)?;
        let schema = crate::toml::get_int(&t, "", "schema")? as u32;
        let name = crate::toml::get_str(&t, "", "name")?.to_string();
        let purpose = crate::toml::get_str(&t, "", "purpose").unwrap_or("").to_string();
        let model = crate::toml::get_str(&t, "", "model").unwrap_or("default-local").to_string();
        let max_tier = crate::toml::get_int(&t, "", "max_tier").unwrap_or(0) as u8;
        let tools = crate::toml::get_array(&t, "", "tools");
        let inputs = crate::toml::get_array(&t, "", "inputs");
        let outputs = crate::toml::get_array(&t, "", "outputs");
        let requires_human_confirm = bool_key(&t, "requires_human_confirm").unwrap_or(true);
        let network = bool_key(&t, "network").unwrap_or(false);
        Ok(Self { schema, name, purpose, model, max_tier, tools, inputs, outputs, requires_human_confirm, network })
    }

    pub fn lint_paths(&self) -> Vec<String> {
        let mut issues = Vec::new();
        for p in self.inputs.iter().chain(self.outputs.iter()) {
            if p.starts_with('/') || p.contains("..") || p.contains("://") {
                issues.push(format!("agent path escapes bundle root: {p}"));
            }
        }
        issues
    }
}

fn bool_key(t: &crate::toml::Table, key: &str) -> Result<bool> {
    match t.get("").and_then(|s| s.get(key)) {
        Some(crate::toml::Value::Bool(b)) => Ok(*b),
        Some(_) => Err(SheafError::Parse(format!("{key} must be bool"))),
        None => Err(SheafError::Missing(key.into())),
    }
}

