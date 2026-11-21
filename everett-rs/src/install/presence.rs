use crate::adapters::devin as devin_adapter;
use crate::session::home;

pub(crate) fn stores() -> &'static [(&'static str, &'static str)] {
    // t3code's database annotates sessions of the other harnesses; devin's root moves with DEVIN_HOME.
    &[
        ("claude", ".claude/projects"),
        ("codex", ".codex/sessions"),
        ("omp", ".omp/agent/sessions"),
        ("pi", ".pi/agent/sessions"),
        ("hermes", ".hermes"),
        ("grok", ".grok/sessions"),
        ("devin", devin_adapter::DEFAULT_REL),
        ("t3code", ".t3/userdata/state.sqlite"),
    ]
}

pub(crate) fn store_path(harness: &str, relative: &str) -> std::path::PathBuf {
    if harness == "pi" {
        crate::adapters::pi::agent_dir().join("sessions")
    } else if harness == "hermes" {
        crate::adapters::hermes::profile_home(&home())
    } else {
        home().join(relative)
    }
}

pub(crate) fn which(name: &str) -> Option<String> {
    crate::proc::which(name, &std::env::var("PATH").unwrap_or_default())
}

pub(crate) fn detected_harnesses() -> Vec<String> {
    stores()
        .iter()
        .filter(|(h, _)| *h != "t3code")
        .filter(|(h, rel)| {
            which(h).is_some()
                || store_path(h, rel).exists()
                || (*h == "devin" && devin_adapter::data_dirs().iter().any(|d| d.is_dir()))
                || (*h == "devin" && super::mcp_path(h).exists())
        })
        .map(|(h, _)| h.to_string())
        .collect()
}
