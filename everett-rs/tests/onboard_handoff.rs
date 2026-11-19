mod common;

use common::fixture;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::process::{Command, ExitStatus, Stdio};

const CODEX_VECTOR: &str =
    "eyJ2ZXJzaW9uIjoxLCJhZ2VudCI6ImNvZGV4IiwiaG9va3MiOnRydWUsIm1jcCI6dHJ1ZX0";
const CODEX_MCP_ONLY_VECTOR: &str =
    "eyJ2ZXJzaW9uIjoxLCJhZ2VudCI6ImNvZGV4IiwiaG9va3MiOmZhbHNlLCJtY3AiOnRydWV9";
const PI_VECTOR: &str = "eyJ2ZXJzaW9uIjoxLCJhZ2VudCI6InBpIiwiaG9va3MiOnRydWUsIm1jcCI6ZmFsc2V9";

fn sentinel(fx: &common::Fixture) -> std::path::PathBuf {
    fx.write(".everett/config.toml", "theme = \"untouched-test-value\"\n")
}

fn assert_no_setup_writes(fx: &common::Fixture) {
    assert!(!fx.home().join(".codex").exists());
    assert!(!fx.home().join(".everett/onboarding-complete").exists());
    assert!(!fx.home().join(".everett/cards").exists());
    assert!(!fx.home().join(".everett/core").exists());
}

#[test]
fn preview_shows_validated_plan_and_is_read_only() {
    let fx = fixture();
    let sentinel_path = sentinel(&fx);
    let sentinel_bytes = std::fs::read(&sentinel_path).unwrap();
    let out = fx.run(&["onboard", "--handoff", CODEX_VECTOR, "--handoff-preview"]);
    let stdout = fx.stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("Agent: Codex"), "{stdout}");
    assert!(stdout.contains("Install Codex session hooks"), "{stdout}");
    assert!(
        stdout.contains("Register Everett tools with Codex"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&fx.home().join(".codex/hooks.json").display().to_string()),
        "{stdout}"
    );
    assert!(
        stdout.contains(&fx.home().join(".codex/config.toml").display().to_string()),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            &fx.home()
                .join(".everett/onboarding-complete")
                .display()
                .to_string()
        ),
        "{stdout}"
    );
    assert!(stdout.contains("completion metadata"), "{stdout}");
    assert!(
        stdout.contains("Preview only; no files were changed."),
        "{stdout}"
    );
    assert_eq!(std::fs::read(sentinel_path).unwrap(), sentinel_bytes);
    assert_no_setup_writes(&fx);
}

#[test]
fn invalid_input_and_conflicting_options_do_not_echo_or_write() {
    let fx = fixture();
    let sentinel_path = sentinel(&fx);
    let bad = "not-a-handoff-token";
    let out = fx.run(&["onboard", "--handoff", bad, "--handoff-preview"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr.contains("handoff token encoding is invalid"),
        "{stderr}"
    );
    assert!(!stderr.contains(bad), "{stderr}");
    assert_no_setup_writes(&fx);

    let out = fx.run(&["onboard", "--handoff", PI_VECTOR, "--handoff-preview"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr.contains("handoff data is invalid"), "{stderr}");
    assert!(!stderr.contains(PI_VECTOR), "{stderr}");
    assert_no_setup_writes(&fx);

    for extra in [
        &["--yes"][..],
        &["--no-mcp"][..],
        &["--no-backfill"][..],
        &["--jev-key-env", "HANDOFF_TEST_KEY"][..],
        &["--schedule-merge"][..],
        &["--span-days", "3"][..],
    ] {
        let mut args = vec!["onboard", "--handoff", CODEX_VECTOR];
        args.extend_from_slice(extra);
        let out = fx.run(&args);
        assert_eq!(out.status.code(), Some(2), "{}", fx.stdout(&out));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("cannot be combined"), "{stderr}");
        assert_no_setup_writes(&fx);
    }

    let out = fx.run(&["onboard", "--handoff-preview"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--handoff-preview requires --handoff"));
    assert_no_setup_writes(&fx);
    assert_eq!(
        std::fs::read_to_string(sentinel_path).unwrap(),
        "theme = \"untouched-test-value\"\n"
    );
}

#[test]
fn noninteractive_apply_is_rejected_without_writes() {
    let fx = fixture();
    sentinel(&fx);
    let out = fx.run(&["onboard", "--handoff", CODEX_VECTOR]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("requires interactive stdin and stdout"));
    assert_no_setup_writes(&fx);
}

#[cfg(unix)]
fn run_in_pty(fx: &common::Fixture, input: &str, token: &str) -> (ExitStatus, String, String) {
    use std::os::fd::RawFd;

    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    let mut window = libc::winsize {
        ws_row: 24,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let window_ptr = &mut window;
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            window_ptr,
        )
    };
    assert_eq!(opened, 0, "could not open test pty");
    let mut master = unsafe { std::fs::File::from_raw_fd(master) };
    let slave_in = unsafe { std::fs::File::from_raw_fd(slave) };
    let slave_out = slave_in.try_clone().unwrap();
    let home = fx.home();
    let mut command = Command::new(env!("CARGO_BIN_EXE_everett"));
    command
        .args(["onboard", "--handoff", token])
        .env_clear()
        .env("HOME", &home)
        .env("EVERETT_HOME", &home)
        .env("EVERETT_NOTIFY", "none")
        .env("PATH", common::HERMETIC_PATH)
        .env("LANG", "C.UTF-8")
        .env("TERM", "xterm")
        .stdin(Stdio::from(slave_in))
        .stdout(Stdio::from(slave_out))
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    drop(command);
    let prompt = b"Type yes to apply these changes [no]:";
    let mut transcript = Vec::new();
    let mut buf = [0u8; 256];
    while !transcript
        .windows(prompt.len())
        .any(|window| window == prompt)
    {
        let read = master.read(&mut buf).unwrap();
        assert!(read > 0, "child exited before confirmation prompt");
        transcript.extend_from_slice(&buf[..read]);
    }
    master.write_all(input.as_bytes()).unwrap();
    master.flush().unwrap();
    let status = child.wait().unwrap();
    loop {
        match master.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => transcript.extend_from_slice(&buf[..read]),
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) => panic!("failed reading pty output: {error}"),
        }
    }
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    (
        status,
        String::from_utf8_lossy(&transcript).to_string(),
        stderr,
    )
}

#[cfg(unix)]
#[test]
fn typed_yes_applies_only_the_selected_setup_and_preserves_other_settings() {
    let fx = fixture();
    let sentinel_path = sentinel(&fx);
    let codex_config = fx.write(".codex/config.toml", "model = \"sentinel-model\"\n");
    let (status, stdout, stderr) = run_in_pty(&fx, "yes\n", CODEX_VECTOR);
    assert!(status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(
        stdout.contains("Local integration setup applied"),
        "{stdout}"
    );
    assert!(fx.home().join(".codex/hooks.json").is_file());
    let updated = std::fs::read_to_string(&codex_config).unwrap();
    assert!(updated.contains("model = \"sentinel-model\""), "{updated}");
    assert!(updated.contains("[mcp_servers.everett]"), "{updated}");
    assert!(fx.home().join(".everett/onboarding-complete").is_file());
    assert_eq!(
        std::fs::read_to_string(sentinel_path).unwrap(),
        "theme = \"untouched-test-value\"\n"
    );
    assert!(!fx.home().join(".everett/cards").exists());
    assert!(!fx.home().join(".everett/core").exists());
    assert!(std::fs::read_dir(fx.home().join(".codex"))
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry
            .file_name()
            .to_string_lossy()
            .contains(".everett-bak-")));
}

#[cfg(unix)]
#[test]
fn typed_yes_applies_mcp_only_without_installing_hooks() {
    let fx = fixture();
    let sentinel_path = sentinel(&fx);
    let codex_config = fx.write(".codex/config.toml", "model = \"sentinel-model\"\n");
    let (status, stdout, stderr) = run_in_pty(&fx, "yes\n", CODEX_MCP_ONLY_VECTOR);
    assert!(status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(
        stdout.contains("Register Everett tools with Codex"),
        "{stdout}"
    );
    assert!(!stdout.contains("Install Codex session hooks"), "{stdout}");
    assert!(!fx.home().join(".codex/hooks.json").exists());
    let updated = std::fs::read_to_string(&codex_config).unwrap();
    assert!(updated.contains("model = \"sentinel-model\""), "{updated}");
    assert!(updated.contains("[mcp_servers.everett]"), "{updated}");
    assert!(fx.home().join(".everett/onboarding-complete").is_file());
    assert_eq!(
        std::fs::read_to_string(sentinel_path).unwrap(),
        "theme = \"untouched-test-value\"\n"
    );
    assert!(!fx.home().join(".everett/cards").exists());
    assert!(!fx.home().join(".everett/core").exists());
}

#[cfg(unix)]
#[test]
fn enter_no_and_eof_cancel_without_writes() {
    for input in ["\n", "no\n", "\x04"] {
        let fx = fixture();
        sentinel(&fx);
        let (status, stdout, stderr) = run_in_pty(&fx, input, CODEX_VECTOR);
        assert!(status.success(), "stdout={stdout}\nstderr={stderr}");
        assert!(
            stdout.contains("Canceled. No files were changed."),
            "{stdout}"
        );
        assert_no_setup_writes(&fx);
    }
}
