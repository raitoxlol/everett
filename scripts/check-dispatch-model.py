#!/usr/bin/env python3
"""Prove the dispatch_once model catches the bugs it exists for (#205).

Runs the model tests on the real source, then on copies mutated back to each
historical mistake. The unmutated copy must pass; every mutant must fail inside
a test, not at compile time. Your working tree is never edited.
"""
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = "src/inbox/t3_dispatch.rs"
MUTANTS = {
    "confirmed receipt ignored, retry dispatches again": (
        "Some(Some(receipt)) => return Ok((receipt, true)),",
        "Some(Some(_)) => {}",
    ),
    "uncertain failure releases the reservation": (
        "Err(failure) if failure.retryable => {",
        "Err(failure) => {",
    ),
    "reservation never committed before dispatch": (
        "reserve.commit().map_err(journal_error)?;",
        "drop(reserve);",
    ),
    "unsaved confirmation reported as retryable": (
        'confirmation: {error}")),\n        retryable: false,',
        'confirmation: {error}")),\n        retryable: true,',
    ),
}


def model(crate: Path, target: Path) -> tuple[int, str]:
    done = subprocess.run(
        ["cargo", "test", "--locked", "--lib", "t3_dispatch::model"],
        cwd=crate, env=dict(os.environ, CARGO_TARGET_DIR=str(target)),
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    return done.returncode, done.stdout


def main() -> int:
    scratch = ROOT / "everett-rs" / "target"
    scratch.mkdir(exist_ok=True)
    failed = False
    with tempfile.TemporaryDirectory(dir=scratch) as tmp:
        tmp = Path(tmp)
        target = Path(os.environ.get("CARGO_TARGET_DIR", scratch))
        for name, edit in [("unmutated source", None), *MUTANTS.items()]:
            crate = tmp / "everett-rs"
            shutil.rmtree(crate, ignore_errors=True)
            shutil.copytree(ROOT / "everett-rs", crate, ignore=shutil.ignore_patterns("target"))
            if edit:
                path = crate / SOURCE
                text = path.read_text()
                assert text.count(edit[0]) == 1, f"mutation site moved: {edit[0]!r}"
                path.write_text(text.replace(*edit))
            code, out = model(crate, target)
            if edit is None:
                ok = code == 0
                print(("PASS" if ok else "FAIL") + f": {name}")
            else:
                ok = code != 0 and "test result: FAILED" in out
                print(("PASS" if ok else "FAIL") + f": model catches mutant '{name}'")
            if not ok:
                print(out[-3000:])
                failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
