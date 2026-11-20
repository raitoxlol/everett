use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::Deserialize;

const MAX_TOKEN_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Agent {
    Claude,
    Codex,
    Omp,
    Grok,
    Devin,
}

impl Agent {
    fn harness(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Omp => "omp",
            Self::Grok => "grok",
            Self::Devin => "devin",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Omp => "OMP",
            Self::Grok => "Grok CLI",
            Self::Devin => "Devin CLI",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoffV1 {
    version: u8,
    agent: Agent,
    hooks: bool,
    mcp: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Plan {
    agent: Agent,
    hooks: bool,
    mcp: bool,
}

fn parse_token(token: &str) -> Result<Plan, &'static str> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err("handoff token is too long");
    }
    if token.is_empty() {
        return Err("handoff token is empty");
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| "handoff token encoding is invalid")?;
    let handoff: HandoffV1 =
        serde_json::from_slice(&bytes).map_err(|_| "handoff data is invalid")?;
    if handoff.version != 1 {
        return Err("unsupported handoff version");
    }
    if !handoff.hooks && !handoff.mcp {
        return Err("handoff plan selects no actions");
    }
    Ok(Plan {
        agent: handoff.agent,
        hooks: handoff.hooks,
        mcp: handoff.mcp,
    })
}

fn hook_path(agent: Agent) -> PathBuf {
    match agent {
        Agent::Omp => crate::install::omp_extension_path(),
        other => crate::install::settings_path(other.harness()),
    }
}

fn destinations(plan: Plan) -> Vec<(&'static str, PathBuf)> {
    let mut paths = Vec::new();
    if plan.hooks {
        let action = match plan.agent {
            Agent::Omp => "OMP session extension",
            _ => "session hooks",
        };
        paths.push((action, hook_path(plan.agent)));
    }
    if plan.mcp {
        paths.push((
            "MCP registration",
            crate::install::mcp_path(plan.agent.harness()),
        ));
    }
    paths
}

fn review(plan: Plan) -> String {
    let mut lines = vec![
        "Read-only setup review".to_string(),
        format!("Agent: {}", plan.agent.label()),
        "Selected actions:".to_string(),
    ];
    if plan.hooks {
        lines.push(format!(
            "  - {}",
            match plan.agent {
                Agent::Omp => "Install the OMP session extension".to_string(),
                _ => format!("Install {} session hooks", plan.agent.label()),
            }
        ));
    }
    if plan.mcp {
        lines.push(format!(
            "  - Register Everett tools with {}",
            plan.agent.label()
        ));
    }
    lines.push("Exact destinations:".to_string());
    for (action, path) in destinations(plan) {
        let state = if path.exists() {
            "existing file; a timestamped Everett backup is attempted before changes"
        } else {
            "new file"
        };
        lines.push(format!("  - {}: {} ({})", action, path.display(), state));
    }
    lines.push(format!(
        "  - completion metadata: {} (written after successful apply)",
        crate::onboard::completion_marker_path().display()
    ));
    lines.extend([
        "Existing files are preserved through timestamped .everett-bak-* backups when the installer can create them.".to_string(),
        "This does not sign in to Everett or an agent provider, and does not configure endpoints, credentials, or models.".to_string(),
    ]);
    lines.join("\n")
}

fn has_conflicting_options(args: &crate::cli::Args) -> bool {
    args.onboard_yes
        || args.no_mcp
        || args.no_backfill
        || args.jev_key_env.is_some()
        || args.schedule_merge
        || args.onboard_span_days_explicit
}

fn apply(plan: Plan) -> Vec<(&'static str, Result<String, String>)> {
    let mut results = Vec::new();
    if plan.hooks {
        let harness = plan.agent.harness();
        let result = match plan.agent {
            Agent::Omp => crate::install::apply(harness, None),
            _ => {
                let events = crate::install::HOOKS
                    .iter()
                    .find(|(name, _)| *name == harness)
                    .map(|(_, hooks)| {
                        hooks
                            .iter()
                            .map(|(event, _, _)| (*event).to_string())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if events.is_empty() {
                    Err(format!(
                        "no Everett hooks are available for {}",
                        plan.agent.label()
                    ))
                } else {
                    crate::install::apply(harness, Some(&events))
                }
            }
        };
        results.push(("hooks/extension", result));
    }
    if plan.mcp {
        results.push((
            "MCP registration",
            crate::install::apply_mcp(plan.agent.harness(), false),
        ));
    }
    results
}

pub fn run(args: &crate::cli::Args) -> i32 {
    let Some(token) = args.onboard_handoff.as_deref() else {
        eprintln!("everett onboard: --handoff-preview requires --handoff");
        return 2;
    };
    if has_conflicting_options(args) {
        eprintln!("everett onboard: --handoff cannot be combined with existing onboarding options");
        return 2;
    }
    let plan = match parse_token(token) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("everett onboard: {error}");
            return 2;
        }
    };
    let summary = review(plan);
    if args.onboard_handoff_preview {
        println!("{summary}");
        println!("Preview only; no files were changed.");
        return 0;
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("everett onboard: --handoff requires interactive stdin and stdout; use --handoff-preview to inspect without applying");
        return 2;
    }
    println!("{summary}");
    println!("Only the local integration choices above will be applied.");
    print!("Type yes to apply these changes [no]: ");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    match io::stdin().read_line(&mut answer) {
        Ok(0) | Err(_) => {
            println!("Canceled. No files were changed.");
            return 0;
        }
        _ => {}
    }
    if answer.trim_end_matches(['\r', '\n']) != "yes" {
        println!("Canceled. No files were changed.");
        return 0;
    }

    let results = apply(plan);
    let mut failed = false;
    for (action, result) in results {
        match result {
            Ok(message) => println!("{action}: {message}"),
            Err(error) => {
                failed = true;
                eprintln!("{action}: {error}");
            }
        }
    }
    if failed {
        eprintln!("Setup was partially applied; some local files may have changed. No completion marker was written. Review the reported errors; this operation does not roll back partial writes.");
        return 1;
    }
    if crate::onboard::finish() != 0 {
        eprintln!("Integration changes were applied, but setup completion could not be recorded.");
        return 1;
    }
    println!("Local integration setup applied. Restart the selected agent and verify its tools; installation alone does not prove a client connection.");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHARED_CODEX_VECTOR: &str =
        "eyJ2ZXJzaW9uIjoxLCJhZ2VudCI6ImNvZGV4IiwiaG9va3MiOnRydWUsIm1jcCI6dHJ1ZX0";

    fn token(json: &str) -> String {
        URL_SAFE_NO_PAD.encode(json.as_bytes())
    }

    #[test]
    fn accepts_shared_codex_vector() {
        assert_eq!(
            parse_token(SHARED_CODEX_VECTOR),
            Ok(Plan {
                agent: Agent::Codex,
                hooks: true,
                mcp: true,
            })
        );
    }

    #[test]
    fn rejects_bad_schemas_without_echoing_input() {
        for raw in [
            r#"{"version":1,"agent":"claude","hooks":true,"mcp":true,"extra":false}"#,
            r#"{"version":1,"agent":"claude","agent":"codex","hooks":true,"mcp":true}"#,
            r#"{"version":1,"agent":"claude","hooks":"true","mcp":true}"#,
            r#"{"version":1,"agent":"unknown","hooks":true,"mcp":true}"#,
            r#"{"version":1,"agent":"pi","hooks":true,"mcp":false}"#,
            r#"{"version":2,"agent":"claude","hooks":true,"mcp":true}"#,
            r#"{"version":1,"agent":"claude","hooks":false,"mcp":false}"#,
        ] {
            let bad = token(raw);
            let error = parse_token(&bad).unwrap_err();
            assert!(!error.contains(bad.as_str()));
            assert!(!error.contains(raw));
        }
    }

    #[test]
    fn rejects_missing_fields_and_oversized_or_invalid_encoding() {
        assert!(parse_token(&token(r#"{"version":1,"agent":"claude","hooks":true}"#)).is_err());
        assert_eq!(parse_token("!"), Err("handoff token encoding is invalid"));
        assert_eq!(
            parse_token("x".repeat(MAX_TOKEN_BYTES + 1).as_str()),
            Err("handoff token is too long")
        );
        assert_eq!(parse_token(""), Err("handoff token is empty"));
    }
}
