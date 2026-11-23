#!/usr/bin/env python3
"""Prove the Dot delivery model catches the bugs it exists for (#206).

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
SOURCE = "src/gateway_events.rs"
MUTANTS = {
    "a receipt that could not be written is forgotten": [
        ("while let Some(receipt) = receipts.last() {", "while let Some(receipt) = receipts.pop() {"),
        ("            receipts.pop();\n        }\n        Ok(())", "        }\n        Ok(())"),
    ],
    "delivery sends before saved receipts are flushed": [
        ("let mut db = self.db.lock().map_err(|_| storage())?;\n        self.flush_acknowledgments(&db, None)?;",
         "let mut db = self.db.lock().map_err(|_| storage())?;"),
    ],
    "a late result mutates a refreshed subscription": [
        ("id=?3 AND revision=?6 AND token IS ?7 AND expires>?8 AND blocked='')",
         "id=?3 AND ?6 IS NOT NULL AND ?7 IS ?7 AND expires>?8 AND blocked='')"),
    ],
    "webhook is posted though the claim did not commit": [
        ("if rows.next().map_err(|_| storage())?.is_some() {\n                    return Err(storage());\n                }",
         "let _ = rows.next();"),
    ],
}


def model(crate: Path, target: Path) -> tuple[int, str]:
    done = subprocess.run(
        ["cargo", "test", "--locked", "--lib", "gateway_events::model"],
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
                for old, new in edit:
                    assert text.count(old) == 1, f"mutation site moved: {old!r}"
                    text = text.replace(old, new)
                path.write_text(text)
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
