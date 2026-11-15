use std::process::Command;
fn main() {
    println!("cargo:rerun-if-env-changed=EVERETT_BUILD_REVISION");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../.git");
    for name in ["HEAD", "index"] {
        track_git_path(name);
    }
    if let Ok(out) = Command::new("git").args(["symbolic-ref", "-q", "HEAD"]).output() {
        if out.status.success() { track_git_path(String::from_utf8_lossy(&out.stdout).trim()); }
    }
    let revision = std::env::var("EVERETT_BUILD_REVISION").ok().or_else(|| {
        let out = Command::new("git").args(["rev-parse", "HEAD"]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }).filter(|s| !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))).unwrap_or_else(|| "unknown".into());
    let dirty = Command::new("git").args(["diff", "--quiet", "HEAD", "--", "."]).status().map(|s| !s.success()).unwrap_or(true);
    println!("cargo:rustc-env=EVERETT_BUILD_REVISION={revision}");
    println!("cargo:rustc-env=EVERETT_BUILD_DIRTY={dirty}");
}

fn track_git_path(name: &str) {
    if let Ok(out) = Command::new("git").args(["rev-parse", "--git-path", name]).output() {
        if out.status.success() { println!("cargo:rerun-if-changed={}", String::from_utf8_lossy(&out.stdout).trim()); }
    }
}
