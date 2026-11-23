#!/usr/bin/env python3
"""Prove integration-test fixtures ignore the developer's harness stores (#230).

Seeds a synthetic Devin sessions.db, exports DEVIN_HOME at it, and runs the
existing `ls_empty_home` test. At the tree under test it must pass; at the
known-bad commit (before e19e93c fixed it) it must fail.

  scripts/check-fixture-isolation.py                    # HEAD must pass
  scripts/check-fixture-isolation.py --known-bad ebb13b2  # that rev must fail
"""
import argparse
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TEST = ["--test", "journey", "ls_empty_home", "--", "--exact"]


def seed(devin_home: Path) -> None:
    devin_home.mkdir(parents=True)
    now = time.time()
    con = sqlite3.connect(devin_home / "sessions.db")
    con.executescript(
        """
        CREATE TABLE sessions (id TEXT PRIMARY KEY, working_directory TEXT, title TEXT, model TEXT,
          created_at REAL, last_activity_at REAL, hidden INTEGER DEFAULT 0);
        CREATE TABLE message_nodes (node_id INTEGER PRIMARY KEY, session_id TEXT NOT NULL,
          created_at REAL, chat_message TEXT);
        """
    )
    con.execute(
        "INSERT INTO sessions VALUES ('host-leak', '/work/host', 'Host session', 'm', ?, ?, 0)",
        (now - 600, now - 60),
    )
    con.execute(
        "INSERT INTO message_nodes VALUES (1, 'host-leak', ?, ?)",
        (now - 590, '{"role":"user","content":"host work","metadata":{"is_user_input":true}}'),
    )
    con.commit()
    con.close()


def run_test(tree: Path, devin_home: Path, target_dir: Path) -> tuple[int, str]:
    env = dict(os.environ, DEVIN_HOME=str(devin_home), CARGO_TARGET_DIR=str(target_dir))
    done = subprocess.run(
        ["cargo", "test", "--locked", *TEST], cwd=tree / "everett-rs", env=env,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    print(done.stdout)
    return done.returncode, done.stdout


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--known-bad", metavar="REV", help="revision where the test must fail")
    args = ap.parse_args()
    scratch = ROOT / "everett-rs" / "target"
    scratch.mkdir(exist_ok=True)  # a stray /tmp/Cargo.toml would break workspace discovery under /tmp
    with tempfile.TemporaryDirectory(dir=scratch) as tmp:
        tmp = Path(tmp)
        devin_home = tmp / "devin"
        seed(devin_home)
        target_dir = Path(os.environ.get("CARGO_TARGET_DIR", tmp / "target"))
        if not args.known_bad:
            code, _ = run_test(ROOT, devin_home, target_dir)
            print("PASS: fixture ignored the exported DEVIN_HOME" if code == 0
                  else "FAIL: fixture leaked the exported DEVIN_HOME")
            return code
        tree = tmp / "known-bad"
        subprocess.run(["git", "worktree", "add", "--detach", str(tree), args.known_bad], cwd=ROOT, check=True)
        try:
            code, out = run_test(tree, devin_home, target_dir)
        finally:
            subprocess.run(["git", "worktree", "remove", "--force", str(tree)], cwd=ROOT, check=True)
        if "test ls_empty_home ... FAILED" not in out:
            print(f"FAIL: {args.known_bad} did not fail inside ls_empty_home (build or setup error, or no leak)")
            return 1
        print(f"PASS: {args.known_bad} leaks as expected, so the check catches #230")
        return 0


if __name__ == "__main__":
    sys.exit(main())
