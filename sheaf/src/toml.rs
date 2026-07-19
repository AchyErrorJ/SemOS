//! Very small TOML subset for Sheaf Phase 0.
//!
//! Supports:
//! - `key = "string"`, integers, bools, and inline string arrays;
//! - section headers `[facets."name"]`;
//! - comments outside quoted strings.

use crate::{Result, SheafError};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    Array(Vec<String>),
}

pub type Table = BTreeMap<String, BTreeMap<String, Value>>;

pub fn parse(input: &str) -> Result<Table> {
    let mut t: Table = BTreeMap::new();
    let mut section = String::new();
    t.entry(section.clone()).or_default();

    for (line_no, raw) in input.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() { continue; }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len()-1].trim().to_string();
            t.entry(section.clone()).or_default();
            continue;
        }
        let (k, v) = line.split_once('=')
            .ok_or_else(|| SheafError::Parse(format!("line {}: expected key = value", line_no + 1)))?;
        let key = k.trim().to_string();
        let value = parse_value(v.trim())
            .map_err(|e| SheafError::Parse(format!("line {}: {e}", line_no + 1)))?;
        t.entry(section.clone()).or_default().insert(key, value);
    }
    Ok(t)
}

fn strip_comment(s: &str) -> &str {
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in s.char_indices() {
        if in_str {
            if esc { esc = false; }
            else if c == '\\' { esc = true; }
            else if c == '"' { in_str = false; }
        } else if c == '"' {
            in_str = true;
        } else if c == '#' {
            return &s[..i];
        }
    }
    s
}

fn parse_value(s: &str) -> std::result::Result<Value, String> {
    if s.starts_with('"') {
        return Ok(Value::Str(parse_str(s)?));
    }
    if s.starts_with('[') {
        if !s.ends_with(']') { return Err("unterminated array".into()); }
        let inner = &s[1..s.len()-1];
        let mut out = Vec::new();
        for part in split_array(inner) {
            let p = part.trim();
            if p.is_empty() { continue; }
            out.push(parse_str(p)?);
        }
        return Ok(Value::Array(out));
    }
    match s {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        _ => {}
    }
    if let Ok(i) = s.parse::<i64>() {
        return Ok(Value::Int(i));
    }
    // Bare timestamp-ish values are stored as strings for this prototype.
    if s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | ':' | 'T' | 'Z' | '.')) {
        return Ok(Value::Str(s.to_string()));
    }
    Err(format!("unsupported TOML value {s:?}"))
}

fn parse_str(s: &str) -> std::result::Result<String, String> {
    if !s.starts_with('"') || !s.ends_with('"') {
        return Err(format!("expected quoted string, got {s:?}"));
    }
    let mut out = String::new();
    let mut it = s[1..s.len()-1].chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next().ok_or("dangling escape")? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                other => return Err(format!("unsupported escape \\{other}")),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

fn split_array(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in s.char_indices() {
        if in_str {
            if esc { esc = false; }
            else if c == '\\' { esc = true; }
            else if c == '"' { in_str = false; }
        } else if c == '"' {
            in_str = true;
        } else if c == ',' {
            out.push(&s[start..i]);
            start = i + 1;
        }
    }
    out.push(&s[start..]);
    out
}

pub fn get_str<'a>(t: &'a Table, section: &str, key: &str) -> Result<&'a str> {
    match t.get(section).and_then(|s| s.get(key)) {
        Some(Value::Str(s)) => Ok(s),
        _ => Err(SheafError::Missing(format!("{section}.{key}"))),
    }
}

pub fn get_int(t: &Table, section: &str, key: &str) -> Result<i64> {
    match t.get(section).and_then(|s| s.get(key)) {
        Some(Value::Int(i)) => Ok(*i),
        _ => Err(SheafError::Missing(format!("{section}.{key}"))),
    }
}

pub fn get_array(t: &Table, section: &str, key: &str) -> Vec<String> {
    match t.get(section).and_then(|s| s.get(key)) {
        Some(Value::Array(v)) => v.clone(),
        _ => Vec::new(),
    }
}

pub fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn string_array(items: &[String]) -> String {
    let mut out = String::from("[");
    for (i, item) in items.iter().enumerate() {
        if i > 0 { out.push_str(", "); }
        out.push_str(&quote(item));
    }
    out.push(']');
    out
}

