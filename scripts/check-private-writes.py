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
# Each entry checks the commit before the fix actually landed (the SHAs in the
# first version of this check belonged to commits that were rewritten by a
# squash merge). `baseline_overrides` raises a file's limit to the calls that
# remained after the fix, so a missed replacement — not an unrelated older
# call — is what makes the ratchet name the file.
KNOWN_BAD = [
    ("e9d596b^", "#236 inbox compaction files", ["src/inbox.rs"], {}),
    # At 86c464e the fix left two fs::rename calls in src/inbox.rs (the #236
    # compaction bug, fixed later); allowing exactly those two means only the
    # live-record replacements trip the ratchet.
    ("86c464e^", "#246 live record writes", ["src/inbox.rs", "src/omp_wake.rs"], {"src/inbox.rs": 2}),
    ("4b1348a^", "#260 codex config rewrite", ["src/install.rs"], {}),
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
    for rev, name, expected, baseline_overrides in KNOWN_BAD:
        with tempfile.TemporaryDirectory(dir=scratch) as tmp:
            tree = Path(tmp) / "old"
            subprocess.run(["git", "worktree", "add", "--detach", str(tree), rev], cwd=ROOT, check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                for rel in FILES:
                    shutil.copy(ROOT / rel, tree / rel)
                if baseline_overrides:
                    baseline_path = tree / FILES[1]
                    lines = baseline_path.read_text().splitlines()
                    merged = dict(line.split(" ", 1) for line in lines)
                    merged.update(baseline_overrides)
                    baseline_path.write_text("".join(f"{p} {c}\n" for p, c in sorted(merged.items())))
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
