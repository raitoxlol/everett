use std::ffi::OsStr;
use std::process::Command;

pub(crate) fn isolated(name: &str) -> bool {
    let receipt = format!("everett isolated test: {name}");
    if std::env::var_os("EVERETT_TEST_ISOLATED").as_deref() == Some(OsStr::new(name)) {
        eprintln!("{receipt}");
        return true;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("EVERETT_TEST_ISOLATED", name)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated test {name} failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(&receipt),
        "isolated test {name} did not execute"
    );
    false
}
