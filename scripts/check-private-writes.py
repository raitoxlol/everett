#!/usr/bin/env python3
"""Prove the private-write ratchet catches the mistakes it exists for (#236, #246, #260).

Copies tests/storage_discipline.rs and its baseline into the commit before each
fix and requires the test to fail there, naming the file the fix changed.
It must pass on the tree under test.
"""
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FILES = ["everett-rs/tests/storage_discipline.rs", "everett-rs/tests/storage_discipline.baseline"]
KNOWN_BAD = [
    ("051b05c^", "#236 inbox compaction files", ["src/inbox.rs"]),
    ("db19c1f^", "#246 live record writes", ["src/inbox.rs", "src/omp_wake.rs"]),
    ("e7e1bae^", "#260 codex config rewrite", ["src/install.rs"]),
]


def run(crate: Path, target: Path) -> tuple[int, str]:
    done = subprocess.run(
        ["cargo", "test", "--locked", "--test", "storage_discipline"],
        cwd=crate, env=dict(os.environ, CARGO_TARGET_DIR=str(target)),
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    return done.returncode, done.stdout


def main() -> int:
    scratch = ROOT / "everett-rs" / "target"
    scratch.mkdir(exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", scratch))
    failed = False
    code, out = run(ROOT / "everett-rs", target)
    print(("PASS" if code == 0 else "FAIL") + ": tree under test is within the baseline")
    if code:
        print(out[-2000:])
        failed = True
    for rev, name, expected in KNOWN_BAD:
        with tempfile.TemporaryDirectory(dir=scratch) as tmp:
            tree = Path(tmp) / "old"
            subprocess.run(["git", "worktree", "add", "--detach", str(tree), rev], cwd=ROOT, check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                for rel in FILES:
                    shutil.copy(ROOT / rel, tree / rel)
                code, out = run(tree / "everett-rs", target)
            finally:
                subprocess.run(["git", "worktree", "remove", "--force", str(tree)], cwd=ROOT, check=True)
        named = all(f"{path}:" in out for path in expected)
        ok = code != 0 and "test result: FAILED" in out and named
        print(("PASS" if ok else "FAIL") + f": ratchet fails at {rev} ({name}) naming {', '.join(expected)}")
        if not ok:
            print(out[-2000:])
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
