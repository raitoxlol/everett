use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::{EverettError, Result};

/// Environment for a headless harness run: card hooks stay quiet, hop count carries over.
pub fn child_env(extra: &HashMap<String, String>) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("EVERETT_SEND".into(), "1".into());
    for (k, v) in extra {
        env.insert(k.clone(), v.clone());
    }
    env
}

/// Headless command that starts a NEW session with this request.
pub fn spawn_command(harness: &str, text: &str, session_id: &str, out_file: &str) -> Result<Vec<String>> {
    if crate::harness::EXTERNAL.contains(&harness) {
        return Err(EverettError::new(
            2,
            "External agents must be registered explicitly and are inbox-only; Everett cannot spawn them.",
        ));
    }
    if text.trim().is_empty() {
        return Err(EverettError::new(2, "The request must not be empty."));
    }
    match harness {
        "claude" => Ok(if session_id.is_empty() {
            vec!["claude".into(), "--print".into(), text.into()]
        } else {
            vec!["claude".into(), "--session-id".into(), session_id.into(), "--print".into(), text.into()]
        }),
        "codex" => {
            let mut cmd = vec!["codex".into(), "exec".into(), "--skip-git-repo-check".into()];
            if !out_file.is_empty() {
                cmd.push("-o".into());
                cmd.push(out_file.into());
            }
            cmd.push(text.into());
            Ok(cmd)
        }
        "omp" => Ok(vec!["omp".into(), "-p".into(), text.into()]),
        "pi" => Ok(vec!["pi".into(), "-p".into(), text.into()]),
        "hermes" => Ok(vec!["hermes".into(), "chat".into(), "-Q".into(), "-q".into(), text.into()]),
        "grok" => Ok(if session_id.is_empty() {
            vec!["grok".into(), "-p".into(), text.into()]
        } else {
            vec!["grok".into(), "--session-id".into(), session_id.into(), "-p".into(), text.into()]
        }),
        "devin" => Ok(vec!["devin".into(), "-p".into(), text.into()]),
        other => Err(EverettError::new(
            2,
            format!("Cannot spawn harness {:?}; choose one of {}.", other, crate::harness::SPAWNABLE.join(", ")),
        )),
    }
}

pub(crate) struct HeadlessCommand {
    pub argv: Vec<String>,
    output: Option<PathBuf>,
}

impl HeadlessCommand {
    pub fn new(harness: &str, text: &str, session_id: &str, out_file: &str) -> Result<Self> {
        let out_file = if harness == "codex" { out_file } else { "" };
        Ok(Self {
            argv: spawn_command(harness, text, session_id, out_file)?,
            output: (!out_file.is_empty()).then(|| PathBuf::from(out_file)),
        })
    }

    pub fn output(&self) -> Option<String> {
        self.output.as_ref().and_then(|path| std::fs::read_to_string(path).ok()).filter(|text| !text.is_empty())
    }
}

impl Drop for HeadlessCommand {
    fn drop(&mut self) {
        if let Some(path) = &self.output {
            let _ = std::fs::remove_file(path);
        }
    }
}
