// Each integration binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub struct Fixture {
    pub dir: tempfile::TempDir,
}

impl Fixture {
    pub fn home(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    /// Run the everett binary with HOME+EVERETT_HOME pointed at the fixture.
    pub fn run(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_everett"));
        cmd.args(args)
            .env("EVERETT_HOME", self.home())
            .env("HOME", self.home())
            .env("EVERETT_NOTIFY", "none")
            .env_remove("TYPESAFE_API_KEY")
            .env_remove("EVERETT_SEND");
        cmd.output().unwrap()
    }

    /// Same, with stdin attached and extra env.
    pub fn run_stdin(&self, args: &[&str], input: &str, extra_env: &[(&str, &str)]) -> Output {
        use std::io::Write;
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_everett"));
        cmd.args(args)
            .env("EVERETT_HOME", self.home())
            .env("HOME", self.home())
            .env("EVERETT_NOTIFY", "none")
            .env_remove("TYPESAFE_API_KEY")
            .env_remove("EVERETT_SEND")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    }

    pub fn stdout(&self, out: &Output) -> String {
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    pub fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.home().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    /// `everett ls --json` parsed.
    pub fn ls_json(&self) -> Vec<serde_json::Value> {
        let out = self.run(&["ls", "--json"]);
        assert_eq!(out.status.code(), Some(0), "ls --json failed: {}", self.stdout(&out));
        serde_json::from_str(&self.stdout(&out)).unwrap()
    }
}

pub fn fixture() -> Fixture {
    Fixture { dir: tempfile::tempdir().unwrap() }
}

/// One JSON object per line.
pub fn jsonl(rows: &[serde_json::Value]) -> String {
    rows.iter().map(|r| r.to_string() + "\n").collect()
}

/// Copy a fixture transcript into a harness store path under the fixture home.
pub fn plant(fixture: &Fixture, name: &str, rel: &str) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    fixture.write(rel, &std::fs::read_to_string(src).unwrap())
}
