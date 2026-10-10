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
            let chars: Vec<char> = value.chars().skip(1).collect();
            let mut end = chars.len();
            let mut i = 0;
            while i < chars.len() {
                if chars[i] == '\\' && first == '"' {
                    i += 2;
                    continue;
                }
                if chars[i] == first {
                    end = i;
                    break;
                }
                i += 1;
            }
            let inner: String = chars[..end].iter().collect();
            value = if first == '"' { toml_unescape(&inner) } else { inner };
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
/// Inverse of toml_escape for the escapes Everett writes.
fn toml_unescape(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            let next = chars[i + 1];
            let simple = match next {
                'n' => Some('\n'),
                'r' => Some('\r'),
                't' => Some('\t'),
                '"' => Some('"'),
                '\\' => Some('\\'),
                _ => None,
            };
            if let Some(c) = simple {
                out.push(c);
                i += 2;
                continue;
            }
            if next == 'u' && i + 5 < chars.len() {
                let hex: String = chars[i + 2..i + 6].iter().collect();
                if let Ok(n) = u32::from_str_radix(&hex, 16) {
                    if let Some(c) = char::from_u32(n) {
                        out.push(c);
                        i += 6;
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Escape a value for a TOML basic string.
fn toml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

pub fn set_value(key: &str, value: &str) -> io::Result<PathBuf> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Serialize read-modify-write across processes; only a missing file is empty.
    let lock_target = path.with_file_name(".config.lock");
    let lock = fs::OpenOptions::new().create(true).write(true).truncate(true).open(&lock_target)?;
    use fs2::FileExt;
    lock.lock_exclusive()?;
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    let quoted = format!("{} = \"{}\"", key, toml_escape(value));
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
    let tmp = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    fs::write(&tmp, lines.join("\n") + "\n")?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    fs::rename(&tmp, &path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_value_escapes_and_roundtrips_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("EVERETT_HOME", dir.path());
        set_value("vault", "a\"b\\c\nline").unwrap();
        let text = fs::read_to_string(config_path()).unwrap();
        assert!(text.contains(r#"vault = "a\"b\\c\nline""#), "{text}");
        assert_eq!(get("vault", None, ""), "a\"b\\c\nline");
    }
}
