#!/usr/bin/env python3
"""Check a memory.db written by the Rust port with the real Python Teresis.

    python3 verify.py <teresis-src-dir> <memory.db> <now>

Opens the file with teresis.store.MemoryStore (which re-runs its schema script),
checks SQLite and FTS integrity, runs every recall from cases.json, then writes
through Python: an exact repeat of a Rust-written memory must be a duplicate and
a keyed write must supersede the Rust-written value. Prints one JSON object.
"""
import json
import sqlite3
import sys
from pathlib import Path

src, db, now = Path(sys.argv[1]), sys.argv[2], float(sys.argv[3])
sys.path.insert(0, str(src))
from teresis.store import SCHEMA_VERSION, MemoryStore  # noqa: E402

cases = json.loads((Path(__file__).resolve().parent / "cases.json").read_text(encoding="utf-8"))
t0 = cases["t0"]
store = MemoryStore(db, embedder=None, reranker=None, clock=lambda: now)
con = store._conn
report = {
    "user_version": con.execute("PRAGMA user_version").fetchone()[0],
    "schema_version": SCHEMA_VERSION,
    "journal_mode": con.execute("PRAGMA journal_mode").fetchone()[0],
    "integrity": con.execute("PRAGMA integrity_check").fetchone()[0],
    "schema": [list(r) for r in con.execute("SELECT type, name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name")],
}
con.execute("INSERT INTO memories_fts(memories_fts) VALUES ('integrity-check')")
con.execute("INSERT INTO memories_tri(memories_tri) VALUES ('integrity-check')")
report["fts_integrity"] = "ok"

recalls = []
for case in cases["recall"]:
    kwargs = {k: case[k] for k in ("scope", "k", "inherit", "kinds", "exclude_kinds", "tags", "budget_tokens") if k in case}
    if "as_of_day" in case:
        kwargs["as_of"] = t0 + case["as_of_day"] * 86400.0
    hits = store.recall(case["q"], touch=False, **kwargs)
    recalls.append([{"id": m.id, "score": m.score, "content": m.content} for m in hits])
report["recall"] = recalls

dup = store.remember("teresis stores AGENT memory in one sqlite file with wal mode", scope="project/teresis")
keyed = store.remember("Editor is Helix now", scope="global", key="user.editor")
report["python_write"] = {
    "duplicate": dup.to_dict(),
    "keyed": keyed.to_dict(),
    "editor_history": [m.content for m in store.history(scope="global", key="user.editor")],
}
store.close()
print(json.dumps(report, ensure_ascii=False))
