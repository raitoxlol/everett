#!/usr/bin/env python3
"""Build the Teresis compatibility fixture from the real Python Teresis.

    python3 gen.py <teresis-src-dir> <out-dir>

Replays cases.json through teresis.store.MemoryStore with a fixed clock and no
embedder, then writes <out-dir>/memory.db (one checkpointed file) and
<out-dir>/expected.json. The Rust port must reproduce expected.json: the pure
text/secret/temporal functions exactly, the outcome of every write, the final
table state, and the ids and scores of every recall against memory.db.
"""
import json
import os
import sqlite3
import sys
from pathlib import Path

src, out = Path(sys.argv[1]), Path(sys.argv[2])
sys.path.insert(0, str(src))
from teresis import secrets, temporal, text as T  # noqa: E402
from teresis.store import MemoryStore  # noqa: E402

DAY = 86400.0
here = Path(__file__).resolve().parent
cases = json.loads((here / "cases.json").read_text(encoding="utf-8"))
t0 = cases["t0"]
out.mkdir(parents=True, exist_ok=True)
db = out / "memory.db"
for suffix in ("", "-wal", "-shm"):
    try:
        os.remove(str(db) + suffix)
    except FileNotFoundError:
        pass

clock = [t0]
store = MemoryStore(db, embedder=None, reranker=None, clock=lambda: clock[0])


def state(path):
    con = sqlite3.connect(str(path))
    con.row_factory = sqlite3.Row
    rows = con.execute("SELECT * FROM memories ORDER BY created_at, rowid").fetchall()
    by_id = {r["id"]: r for r in rows}
    result = []
    for r in rows:
        sup = by_id.get(r["superseded_by"]) if r["superseded_by"] else None
        result.append({
            "scope": r["scope"], "kind": r["kind"], "key": r["key"], "content": r["content"],
            "tags": r["tags"], "meta": json.loads(r["meta"]), "source": r["source"],
            "importance": r["importance"], "pinned": r["pinned"], "content_hash": r["content_hash"],
            "created_at": r["created_at"], "updated_at": r["updated_at"],
            "access_count": r["access_count"], "expires_at": r["expires_at"],
            "superseded_by_created_at": sup["created_at"] if sup else None,
            "superseded_at": r["superseded_at"], "deleted_at": r["deleted_at"],
        })
    con.close()
    return result


op_ids, op_results = [], []
for op in cases["ops"]:
    clock[0] = t0 + op["day"] * DAY
    if op["op"] == "remember":
        content = op.get("content") or "".join(op["content_parts"])
        kwargs = {k: op[k] for k in ("scope", "kind", "key", "tags", "importance", "pinned", "ttl_days", "source", "meta") if k in op}
        if "replaces_ref" in op:
            kwargs["replaces"] = op_ids[op["replaces_ref"]]
        res = store.remember(content, **kwargs)
        op_ids.append(res.id)
        op_results.append({"status": res.status, "replaced": [op_ids.index(i) for i in res.replaced], "redacted": res.redacted,
                           "same_id_as": op_ids.index(res.id) if op_ids.index(res.id) != len(op_ids) - 1 else None})
    else:
        if "ref" in op:
            n = store.forget(op_ids[op["ref"]])
        else:
            n = store.forget(scope=op["scope"], key=op["key"])
        op_ids.append(None)
        op_results.append({"forgot": n})

now = t0 + cases["recall_now_days"] * DAY
clock[0] = now
recalls = []
for case in cases["recall"]:
    kwargs = {k: case[k] for k in ("scope", "k", "inherit", "kinds", "exclude_kinds", "tags", "budget_tokens") if k in case}
    if "as_of_day" in case:
        kwargs["as_of"] = t0 + case["as_of_day"] * DAY
    hits = store.recall(case["q"], touch=False, explain=True, **kwargs)
    recalls.append([{"id": m.id, "score": m.score, "content": m.content, "why": m.why} for m in hits])

store._conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
store.close()
con = sqlite3.connect(str(db))
con.execute("PRAGMA journal_mode=DELETE")
con.close()

texts = []
for s in cases["text"]:
    texts.append({
        "in": s, "normalize": T.normalize(s), "hash": T.content_hash(s),
        "tokens": T.tokens(s), "tokens_all": T.tokens(s, keep_stopwords=True),
        "terms": sorted(T.term_set(s)), "fts": T.fts_query(s), "tri": T.trigram_query(s),
        "subs": T.sub_queries(s), "est": T.estimate_tokens(s),
    })
secret_rows = []
for parts in cases["secrets"]:
    scrubbed, kinds = secrets.redact_secrets("".join(parts))
    secret_rows.append({"out": scrubbed, "kinds": kinds, "first": secrets.contains_secret("".join(parts))})
windows = []
for query, day in cases["temporal"]:
    w = temporal.parse_time_window(query, t0 + day * DAY)
    windows.append(None if w is None else [w.start, w.end, w.soft_margin])

expected = {"now": now, "ops": op_results, "state": state(db), "recall": recalls,
            "text": texts, "secrets": secret_rows, "temporal": windows}
(out / "expected.json").write_text(json.dumps(expected, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
print(f"wrote {db} and expected.json: {len(op_results)} ops, {len(recalls)} recalls")
