use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::session::home;

pub fn config_path() -> PathBuf {
    home().join(".everett").join("config.toml")
}

fn alias(name: &str) -> &str {
    match name {
        "everett.vault" => "vault",
        "jev.api_key" => "typesafe_api_key",
        "everett.vault_dir" => "vault_dir",
        "everett.default_harness" => "default_harness",
        "everett.router" => "router",
        _ => name,
    }
}

fn parse(text: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(|c| c == '[' || c == ']').trim().to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let mut value = value.trim().to_string();
        let first = value.chars().next().unwrap_or('\0');
        if first == '"' || first == '\'' {
            let inner: String = value.chars().skip(1).collect();
            value = match inner.find(first) {
                Some(end) => inner[..end].to_string(),
                None => inner,
            };
        } else {
            value = value.split('#').next().unwrap_or("").trim().to_string();
        }
        let name = if section.is_empty() {
            key.trim().to_string()
        } else {
            format!("{}.{}", section, key.trim())
        };
        values.insert(alias(&name).to_string(), value);
    }
    values
}

pub fn values() -> Result<HashMap<String, String>, String> {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(text) => Ok(parse(&text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(format!("cannot read {}: {}", path.display(), e)),
    }
}

/// Environment variable first, then the config file, then the default. Never fails.
pub fn get(key: &str, env: Option<&str>, default: &str) -> String {
    if let Some(env) = env {
        if let Ok(v) = std::env::var(env) {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    match values() {
        Ok(v) => v.get(key).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| default.to_string()),
        Err(_) => default.to_string(),
    }
}

/// Set a top-level (no-section) key in ~/.everett/config.toml, preserving everything else.
pub fn set_value(key: &str, value: &str) -> io::Result<PathBuf> {
    let path = config_path();
    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    let quoted = format!("{} = \"{}\"", key, value);
    let mut in_section = false;
    let mut replaced = false;
    let mut insert_at = lines.len();
    for (i, raw) in lines.iter_mut().enumerate() {
        let stripped = raw.trim();
        if stripped.starts_with('[') && stripped.ends_with(']') {
            if !in_section {
                insert_at = i;
            }
            in_section = true;
            continue;
        }
        if in_section {
            continue;
        }
        let name = stripped.split('=').next().unwrap_or("").trim();
        if name == key {
            *raw = quoted.clone();
            replaced = true;
            break;
        }
    }
    if !replaced {
        lines.insert(insert_at, quoted);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(".{}.tmp", path.file_name().unwrap().to_string_lossy()));
    fs::write(&tmp, lines.join("\n") + "\n")?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    fs::rename(&tmp, &path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(path)
}
