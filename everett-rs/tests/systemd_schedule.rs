mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{lib_home, LibHome};
use everett::trunk_schedule::{render_systemd, Scheduler};

const TIMER: &str = "dev.everett.core-merge.timer";
const SERVICE: &str = "dev.everett.core-merge.service";

fn isolated() -> LibHome {
    let home = lib_home();
    std::env::set_var("XDG_CONFIG_HOME", home.path().join(".config"));
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let stub = bin.join("systemctl");
    std::fs::write(
        &stub,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/systemctl.log"
if [ -e "$HOME/fail-$2" ]; then
    echo 'Synthetic user manager failure' >&2
    exit 1
fi
case "$2" in
    enable|disable)
        if [ ! -f "$XDG_CONFIG_HOME/systemd/user/dev.everett.core-merge.timer" ]; then
            echo 'Unit dev.everett.core-merge.timer does not exist' >&2
            exit 1
        fi
        ;;
esac
case "$2" in
    enable)
        touch "$HOME/enabled" "$HOME/active"
        ;;
    disable)
        rm -f "$HOME/enabled" "$HOME/active"
        ;;
    is-enabled)
        test -e "$HOME/enabled"
        ;;
    is-active)
        test -e "$HOME/active"
        ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    home
}

fn unit_dir(home: &LibHome) -> PathBuf {
    home.path().join(".config/systemd/user")
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn systemd_preview_validates_without_files_and_preserves_only_selected_environment() {
    let home = isolated();
    let store = home.path().join("shared store %h $literal");
    std::env::set_var("EVERETT_HOME", &store);
    std::env::set_var("UNRELATED_API_KEY", "synthetic-secret-do-not-capture");
    std::env::set_var(
        "PATH",
        format!(
            "relative::{}:/usr/bin:/bin",
            home.path().join("bin").display()
        ),
    );
    let units = render_systemd("06:30", "codex").unwrap();
    assert!(units
        .service
        .contains(&format!("Environment=\"HOME={}\"", home.path().display())));
    assert!(
        units.service.contains("shared store %%h $literal"),
        "{}",
        units.service
    );
    assert!(!units.service.contains("relative"));
    assert!(!units.service.contains("synthetic-secret-do-not-capture"));
    assert!(units.timer.contains("OnCalendar=*-*-* 06:30:00"));
    let preview = Scheduler::Systemd.preview("06:30", "codex").unwrap();
    assert!(preview.contains(SERVICE) && preview.contains(TIMER));
    assert_eq!(Scheduler::Systemd.path(), unit_dir(&home).join(TIMER));
    assert!(Scheduler::Systemd.install("25:00", "none").is_err());
    assert!(Scheduler::Systemd.install("04:00", "invalid").is_err());
    assert!(!unit_dir(&home).exists());
    assert!(!home.path().join("systemctl.log").exists());
    std::env::remove_var("UNRELATED_API_KEY");
}

#[test]
fn systemd_relative_selected_home_is_saved_as_an_absolute_path() {
    let home = isolated();
    let selected = PathBuf::from("./store %h $literal");
    std::env::set_var("EVERETT_HOME", &selected);
    let absolute = std::env::current_dir().unwrap().join(&selected);
    let expected = format!(
        "Environment=\"EVERETT_HOME={}\"",
        absolute.to_string_lossy().replace('%', "%%")
    );
    let units = render_systemd("04:00", "none").unwrap();
    assert!(units.service.contains(&expected), "{}", units.service);
    Scheduler::Systemd.install("04:00", "none").unwrap();
    assert!(std::fs::read_to_string(unit_dir(&home).join(SERVICE))
        .unwrap()
        .contains(&expected));
}

#[test]
fn systemd_install_reapply_status_and_remove_are_user_scoped_and_idempotent() {
    let home = isolated();
    assert_eq!(Scheduler::Systemd.remove().unwrap()["removed"], false);
    let installed = Scheduler::Systemd.install("04:00", "none").unwrap();
    assert_eq!(installed["systemctl_ok"], true);
    assert_eq!(mode(&unit_dir(&home).join(TIMER)), 0o600);
    assert_eq!(mode(&unit_dir(&home).join(SERVICE)), 0o600);
    assert_eq!(Scheduler::Systemd.status().0, Some(true));
    std::fs::set_permissions(unit_dir(&home), std::fs::Permissions::from_mode(0o750)).unwrap();
    Scheduler::Systemd.install("05:30", "codex").unwrap();
    assert_eq!(mode(&unit_dir(&home)), 0o750);
    assert!(std::fs::read_to_string(unit_dir(&home).join(TIMER))
        .unwrap()
        .contains("OnCalendar=*-*-* 05:30:00"));
    assert!(std::fs::read_to_string(unit_dir(&home).join(SERVICE))
        .unwrap()
        .contains("trunk merge --llm codex"));
    let unrelated = unit_dir(&home).join("another.service");
    std::fs::write(&unrelated, "keep this unit").unwrap();
    assert_eq!(Scheduler::Systemd.remove().unwrap()["removed"], true);
    assert_eq!(Scheduler::Systemd.remove().unwrap()["removed"], false);
    assert_eq!(
        std::fs::read_to_string(unrelated).unwrap(),
        "keep this unit"
    );
    assert!(!unit_dir(&home).join(TIMER).exists());
    assert!(!unit_dir(&home).join(SERVICE).exists());
    assert_eq!(std::fs::read_to_string(home.path().join("systemctl.log")).unwrap(), format!(
        "--user daemon-reload\n--user enable --now {TIMER}\n--user is-enabled {TIMER}\n--user is-active {TIMER}\n--user daemon-reload\n--user enable --now {TIMER}\n--user disable --now {TIMER}\n--user daemon-reload\n",
    ));
}

#[test]
fn systemd_manager_failures_never_claim_activation_or_successful_removal() {
    let home = isolated();
    let failure = home.path().join("fail-daemon-reload");
    std::fs::write(&failure, "fail").unwrap();
    let error = Scheduler::Systemd.install("04:00", "none").unwrap_err();
    assert_eq!(error.code, 5);
    assert!(error
        .message
        .contains("systemctl --user daemon-reload failed"));
    assert!(error.message.contains("user manager"));
    assert!(unit_dir(&home).join(TIMER).exists());
    assert_eq!(Scheduler::Systemd.status().0, Some(false));
    std::fs::remove_file(failure).unwrap();
    Scheduler::Systemd.install("04:00", "none").unwrap();
    std::fs::write(home.path().join("fail-disable"), "fail").unwrap();
    assert_eq!(Scheduler::Systemd.remove().unwrap_err().code, 5);
    assert!(unit_dir(&home).join(TIMER).exists());
    assert!(home.path().join("active").exists());
    std::fs::remove_file(home.path().join("fail-disable")).unwrap();
    Scheduler::Systemd.remove().unwrap();
    assert_eq!(Scheduler::Systemd.status().0, None);
}

#[test]
fn systemd_incomplete_installs_can_be_removed_without_masking_manager_failures() {
    let home = isolated();
    std::fs::create_dir_all(unit_dir(&home).join(TIMER)).unwrap();
    assert!(Scheduler::Systemd.install("04:00", "none").is_err());
    assert!(unit_dir(&home).join(SERVICE).is_file());
    assert!(!home.path().join("systemctl.log").exists());
    std::fs::remove_dir(unit_dir(&home).join(TIMER)).unwrap();
    let unrelated = unit_dir(&home).join("another.service");
    std::fs::write(&unrelated, "keep this unit").unwrap();
    assert_eq!(Scheduler::Systemd.status().0, Some(false));
    let removed = Scheduler::Systemd.remove().unwrap();
    assert_eq!(removed["removed"], true);
    assert_eq!(
        removed["path"],
        unit_dir(&home).join(SERVICE).to_str().unwrap()
    );
    assert!(!unit_dir(&home).join(SERVICE).exists());
    assert_eq!(
        std::fs::read_to_string(home.path().join("systemctl.log")).unwrap(),
        "--user daemon-reload\n"
    );

    let units = render_systemd("04:00", "none").unwrap();
    std::fs::write(unit_dir(&home).join(TIMER), units.timer).unwrap();
    assert_eq!(Scheduler::Systemd.status().0, Some(false));
    std::fs::write(home.path().join("fail-disable"), "fail").unwrap();
    assert_eq!(Scheduler::Systemd.remove().unwrap_err().code, 5);
    assert!(unit_dir(&home).join(TIMER).is_file());
    std::fs::remove_file(home.path().join("fail-disable")).unwrap();
    assert_eq!(Scheduler::Systemd.remove().unwrap()["removed"], true);
    assert_eq!(Scheduler::Systemd.remove().unwrap()["removed"], false);
    assert!(!unit_dir(&home).join(TIMER).exists());
    assert_eq!(
        std::fs::read_to_string(unrelated).unwrap(),
        "keep this unit"
    );
    assert_eq!(std::fs::read_to_string(home.path().join("systemctl.log")).unwrap(), format!(
        "--user daemon-reload\n--user disable --now {TIMER}\n--user disable --now {TIMER}\n--user daemon-reload\n",
    ));
}

#[cfg(target_os = "linux")]
fn cli(home: &LibHome, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(args)
        .env_clear()
        .env("HOME", home.path())
        .env("EVERETT_HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", home.path().join("bin").display()),
        )
        .env("EVERETT_NOTIFY", "none")
        .output()
        .unwrap()
}

#[cfg(target_os = "linux")]
#[test]
fn linux_cli_selects_systemd_previews_applies_and_removes_with_explicit_consent() {
    let home = isolated();
    let preview = cli(&home, &["trunk", "schedule", "--llm", "none"]);
    assert!(preview.status.success());
    let text = String::from_utf8_lossy(&preview.stdout);
    assert!(
        text.contains("systemctl --user") && text.contains(SERVICE) && text.contains(TIMER),
        "{text}"
    );
    assert!(!unit_dir(&home).exists());
    assert_eq!(
        cli(&home, &["trunk", "schedule", "--apply", "--at", "invalid"])
            .status
            .code(),
        Some(2)
    );
    assert!(!unit_dir(&home).exists());
    let applied = cli(&home, &["trunk", "schedule", "--apply", "--llm", "none"]);
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert!(String::from_utf8_lossy(&applied.stdout).contains("systemd user timer activation: ok"));
    let doctor = cli(&home, &["doctor"]);
    assert!(String::from_utf8_lossy(&doctor.stdout).contains("active systemd user timer"));
    assert!(cli(&home, &["trunk", "schedule", "--remove", "--apply"])
        .status
        .success());
    assert!(!unit_dir(&home).join(TIMER).exists());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_onboarding_uses_systemd_and_activation_failures_exit_nonzero() {
    let home = isolated();
    std::fs::write(home.path().join("fail-enable"), "fail").unwrap();
    let failed = cli(&home, &["trunk", "schedule", "--apply", "--llm", "none"]);
    assert_eq!(failed.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&failed.stderr).contains("systemctl --user enable --now"));
    std::fs::remove_file(home.path().join("fail-enable")).unwrap();
    let onboard = cli(
        &home,
        &[
            "onboard",
            "--yes",
            "--no-backfill",
            "--no-mcp",
            "--schedule-merge",
        ],
    );
    assert!(
        onboard.status.success(),
        "{}",
        String::from_utf8_lossy(&onboard.stdout)
    );
    assert!(unit_dir(&home).join(TIMER).exists());
    assert!(String::from_utf8_lossy(&onboard.stdout).contains("nightly at 04:00"));
}
