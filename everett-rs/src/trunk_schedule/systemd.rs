use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::error::{EverettError, Result};
use crate::proc::{run_capture, RunOutput};

use super::{everett_command, home, parse_at, validate_llm, LABEL};

pub struct SystemdUnits {
    pub service: String,
    pub timer: String,
}

fn unit_dir() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    config
        .unwrap_or_else(|| user_home().join(".config"))
        .join("systemd/user")
}

fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(home)
}

pub(super) fn timer_path() -> PathBuf {
    unit_dir().join(format!("{LABEL}.timer"))
}

pub(super) fn service_path() -> PathBuf {
    unit_dir().join(format!("{LABEL}.service"))
}

fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '%' => out.push_str("%%"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_ascii_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn render(at: &str, llm: &str) -> Result<SystemdUnits> {
    let selected_home = home();
    let selected_home = if selected_home.is_absolute() {
        selected_home
    } else {
        std::env::current_dir()
            .map_err(|e| EverettError::new(2, format!("cannot resolve Everett home for systemd: {e}")))?
            .join(selected_home)
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<_> = std::env::split_paths(&path)
        .filter(|p| p.is_absolute())
        .collect();
    let path = std::env::join_paths(dirs)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
    render_with(
        at,
        llm,
        &everett_command()[0],
        &[
            ("HOME", user_home().to_string_lossy().into_owned()),
            ("EVERETT_HOME", selected_home.to_string_lossy().into_owned()),
            ("PATH", path.to_string_lossy().into_owned()),
        ],
    )
}

fn render_with(
    at: &str,
    llm: &str,
    executable: &str,
    environment: &[(&str, String)],
) -> Result<SystemdUnits> {
    let (hour, minute) = parse_at(at)?;
    validate_llm(llm)?;
    if !std::path::Path::new(executable).is_absolute()
        || executable
            .chars()
            .any(|c| c.is_ascii_control() || ['\'', '"', '\\'].contains(&c))
    {
        return Err(EverettError::new(
            2,
            "systemd requires an absolute Everett executable path without quotes, backslashes, or control characters; move the binary and retry.",
        ));
    }
    let env = environment
        .iter()
        .map(|(key, value)| format!("Environment={}\n", quote(&format!("{key}={value}"))))
        .collect::<String>();
    let executable = quote(executable);
    Ok(SystemdUnits {
        service: format!(
            "[Unit]\nDescription=Everett shared memory merge\n\n[Service]\nType=oneshot\n{env}ExecStart=:{executable} trunk merge --llm {llm}\n",
        ),
        timer: format!(
            "[Unit]\nDescription=Everett nightly shared memory merge\n\n[Timer]\nOnCalendar=*-*-* {hour:02}:{minute:02}:00\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n",
        ),
    })
}

fn manager_env() -> HashMap<String, String> {
    [
        "HOME",
        "PATH",
        "XDG_CONFIG_HOME",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
    ]
    .iter()
    .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
    .collect()
}

fn systemctl(args: &[&str]) -> std::result::Result<RunOutput, String> {
    let mut command = vec!["systemctl".to_string(), "--user".to_string()];
    command.extend(args.iter().map(|s| s.to_string()));
    run_capture(&command, None, &manager_env(), 15.0)
}

fn require_systemctl(args: &[&str]) -> Result<()> {
    let output = systemctl(args);
    let detail = match output {
        Ok(r) if r.code == 0 && !r.timed_out => return Ok(()),
        Ok(r) if r.timed_out => "timed out".to_string(),
        Ok(r) => format!("exit {}: {}", r.code, r.stderr.trim()),
        Err(e) => e,
    };
    Err(EverettError::new(5, format!(
        "systemctl --user {} failed: {}. Ensure systemd and your user manager are available, then retry. Inspect `systemctl --user status {LABEL}.timer`. Everett unit files are in {}.",
        args.join(" "), detail, unit_dir().display(),
    )))
}

pub(super) fn install(at: &str, llm: &str) -> Result<Map<String, Value>> {
    let units = render(at, llm)?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(unit_dir())
        .map_err(|e| {
            EverettError::new(5, format!("cannot create {}: {e}", unit_dir().display()))
        })?;
    crate::peer::save(&service_path(), units.service.as_bytes())?;
    crate::peer::save(&timer_path(), units.timer.as_bytes())?;
    require_systemctl(&["daemon-reload"])?;
    require_systemctl(&["enable", "--now", &format!("{LABEL}.timer")])?;
    Ok(serde_json::from_value(json!({
        "path":timer_path(), "service_path":service_path(), "systemctl_ok":true,
    }))
    .unwrap())
}

pub(super) fn remove() -> Result<Map<String, Value>> {
    let timer = timer_path();
    let service = service_path();
    let has_timer = timer.exists();
    let existed = has_timer || service.exists();
    if existed {
        if has_timer {
            require_systemctl(&["disable", "--now", &format!("{LABEL}.timer")])?;
        }
        for path in [&timer, &service] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(EverettError::new(
                        5,
                        format!("cannot remove {}: {e}", path.display()),
                    ))
                }
            }
        }
        require_systemctl(&["daemon-reload"])?;
    }
    let path = if existed && !has_timer {
        service
    } else {
        timer
    };
    Ok(serde_json::from_value(json!({"path":path, "removed":existed})).unwrap())
}

pub(super) fn status() -> (Option<bool>, String) {
    if !timer_path().exists() && !service_path().exists() {
        return (
            None,
            "not scheduled (optional, systemd user timer; `everett trunk schedule --apply`)".into(),
        );
    }
    if !timer_path().is_file() || !service_path().is_file() {
        return (
            Some(false),
            format!(
                "incomplete systemd units ({}); run `everett trunk schedule --apply`",
                unit_dir().display()
            ),
        );
    }
    for action in ["is-enabled", "is-active"] {
        match systemctl(&[action, &format!("{LABEL}.timer")]) {
            Ok(r) if r.code == 0 && !r.timed_out => {},
            _ => return (Some(false), format!(
                "systemd units configured, timer {action} check failed ({}); inspect `systemctl --user status {LABEL}.timer`",
                timer_path().display(),
            )),
        }
    }
    (Some(true), format!("active systemd user timer ({}); inspect merge runs with `journalctl --user -u {LABEL}.service`", timer_path().display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_escapes_systemd_specifiers_without_a_shell_or_environment_expansion() {
        let units = render_with(
            "23:59",
            "none",
            "/opt/a b/%h/${HOME}/everett",
            &[("EVERETT_HOME", "/home/a b/%h/$value/\"quoted\\dir".into())],
        )
        .unwrap();
        assert!(
            units
                .service
                .contains("ExecStart=:\"/opt/a b/%%h/${HOME}/everett\" trunk merge --llm none\n"),
            "{}",
            units.service
        );
        assert!(
            units
                .service
                .contains("Environment=\"EVERETT_HOME=/home/a b/%%h/$value/\\\"quoted\\\\dir\"\n"),
            "{}",
            units.service
        );
        assert!(units
            .timer
            .contains("OnCalendar=*-*-* 23:59:00\nPersistent=true\n"));
        assert!(render_with("24:01", "none", "/bin/everett", &[]).is_err());
        for executable in [
            "everett",
            "/opt/\"everett",
            "/opt/everett\\bin",
            "/opt/everett\n",
        ] {
            assert!(render_with("04:00", "none", executable, &[]).is_err());
        }
        assert!(render_with(
            "04:00",
            "invalid\nExecStart=/bin/false",
            "/bin/everett",
            &[]
        )
        .is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn systemd_analyze_accepts_units_with_literal_executable_specifiers() {
        let analyzer = std::path::Path::new("/usr/bin/systemd-analyze");
        if !analyzer.is_file() {
            eprintln!("systemd-analyze unavailable; syntax verification runs in Linux CI");
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let executable = home.path().join("everett %h ${HOME}");
        std::os::unix::fs::symlink("/usr/bin/true", &executable).unwrap();
        let units = render_with("04:00", "none", executable.to_str().unwrap(), &[]).unwrap();
        let service = home.path().join(format!("{LABEL}.service"));
        let timer = home.path().join(format!("{LABEL}.timer"));
        fs::write(&service, units.service).unwrap();
        fs::write(&timer, units.timer).unwrap();
        let output = std::process::Command::new(analyzer)
            .args(["--user", "verify"])
            .arg(service)
            .arg(timer)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
