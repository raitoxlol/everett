use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use everett::teresis::{secrets, store, temporal, text};
use everett::teresis::store::{MemoryStore, RecallOpts, RememberOpts};
use serde_json::{json, Value};

const DAY: f64 = 86400.0;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/teresis")
}

fn cases() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture_dir().join("cases.json")).unwrap()).unwrap()
}

fn expected() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture_dir().join("expected.json")).unwrap()).unwrap()
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

fn store_at(path: &Path, clock: Arc<Mutex<f64>>) -> MemoryStore {
    MemoryStore::open_with_clock(path, Box::new(move || *clock.lock().unwrap())).unwrap()
}

#[test]
fn text_matches_python() {
    let cases = cases();
    let expected = expected();
    for (i, s) in cases["text"].as_array().unwrap().iter().enumerate() {
        let s = s.as_str().unwrap();
        let e = &expected["text"][i];
        assert_eq!(text::normalize(s), e["normalize"].as_str().unwrap(), "normalize[{i}]");
        assert_eq!(text::content_hash(s), e["hash"].as_str().unwrap(), "hash[{i}]");
        assert_eq!(json!(text::tokens(s, false)), e["tokens"], "tokens[{i}]");
        assert_eq!(json!(text::tokens(s, true)), e["tokens_all"], "tokens_all[{i}]");
        let terms: Vec<String> = text::term_set(s).into_iter().collect();
        assert_eq!(json!(terms), e["terms"], "terms[{i}]");
        assert_eq!(json!(text::fts_query(s)), e["fts"], "fts[{i}]");
        assert_eq!(json!(text::trigram_query(s)), e["tri"], "tri[{i}]");
        assert_eq!(json!(text::sub_queries(s)), e["subs"], "subs[{i}]");
        assert_eq!(json!(text::estimate_tokens(s)), e["est"], "est[{i}]");
    }
}

#[test]
fn secrets_match_python() {
    let cases = cases();
    let expected = expected();
    for (i, parts) in cases["secrets"].as_array().unwrap().iter().enumerate() {
        let input: String = parts.as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
        let e = &expected["secrets"][i];
        let (out, kinds) = secrets::redact_secrets(&input);
        assert_eq!(out, e["out"].as_str().unwrap(), "out[{i}]");
        assert_eq!(json!(kinds), e["kinds"], "kinds[{i}]");
        assert_eq!(json!(secrets::contains_secret(&input)), e["first"], "first[{i}]");
    }
}

#[test]
fn temporal_matches_python() {
    let cases = cases();
    let expected = expected();
    let t0 = f(&cases["t0"]);
    for (i, pair) in cases["temporal"].as_array().unwrap().iter().enumerate() {
        let q = pair[0].as_str().unwrap();
        let now = t0 + f(&pair[1]) * DAY;
        let w = temporal::parse_time_window(q, now);
        let e = &expected["temporal"][i];
        match (w, e.is_null()) {
            (None, true) => {}
            (Some(w), false) => {
                assert!((w.start - f(&e[0])).abs() < 1e-6, "start[{i}]: {} vs {}", w.start, e[0]);
                assert!((w.end - f(&e[1])).abs() < 1e-6, "end[{i}]: {} vs {}", w.end, e[1]);
                assert!((w.soft_margin - f(&e[2])).abs() < 1e-6, "margin[{i}]");
            }
            (got, _) => panic!("temporal[{i}] {q:?}: got {got:?}, expected {e}"),
        }
    }
}

struct Replay {
    op_ids: Vec<Option<String>>,
    db: PathBuf,
    _dir: tempfile::TempDir,
}

fn replay_ops() -> Replay {
    let dir = tempfile::tempdir().unwrap();
    let cases = cases();
    let t0 = f(&cases["t0"]);
    let clock = Arc::new(Mutex::new(t0));
    let db = dir.path().join("memory.db");
    let mut store = store_at(&db, clock.clone());
    let mut op_ids: Vec<Option<String>> = Vec::new();
    for op in cases["ops"].as_array().unwrap() {
        *clock.lock().unwrap() = t0 + f(&op["day"]) * DAY;
        if op["op"] == "remember" {
            let content = match op["content"].as_str() {
                Some(c) => c.to_string(),
                None => op["content_parts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| p.as_str().unwrap())
                    .collect(),
            };
            let opts = RememberOpts {
                scope: op["scope"].as_str().unwrap_or("global").to_string(),
                kind: op["kind"].as_str().unwrap_or("fact").to_string(),
                key: op["key"].as_str().map(|s| s.to_string()),
                tags: op["tags"]
                    .as_array()
                    .map(|a| a.iter().map(|t| t.as_str().unwrap().to_string()).collect())
                    .unwrap_or_default(),
                importance: op["importance"].as_f64().unwrap_or(0.5),
                pinned: op["pinned"].as_bool().unwrap_or(false),
                ttl_days: op["ttl_days"].as_f64(),
                source: op["source"].as_str().map(|s| s.to_string()),
                meta: if op["meta"].is_null() { None } else { Some(op["meta"].clone()) },
                replaces: op["replaces_ref"].as_u64().map(|r| op_ids[r as usize].clone().unwrap()),
                scrub_secrets: true,
                fuzzy_dedupe: true,
                update_expiry_on_duplicate: false,
            };
            let res = store.remember(&content, &opts).unwrap();
            op_ids.push(Some(res.id));
        } else {
            let n = if let Some(r) = op["ref"].as_u64() {
                store.forget_id(&op_ids[r as usize].clone().unwrap(), false).unwrap()
            } else {
                store
                    .forget_key(op["scope"].as_str().unwrap(), op["key"].as_str().unwrap(), false)
                    .unwrap()
            };
            op_ids.push(None);
            let _ = n;
        }
    }
    drop(store);
    Replay { op_ids, db, _dir: dir }
}

#[test]
fn replaying_writes_matches_python() {
    let cases = cases();
    let expected = expected();
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(Mutex::new(f(&cases["t0"])));
    let db = dir.path().join("memory.db");
    let mut store = store_at(&db, clock.clone());
    let t0 = f(&cases["t0"]);
    let mut op_ids: Vec<String> = Vec::new();
    for (i, op) in cases["ops"].as_array().unwrap().iter().enumerate() {
        *clock.lock().unwrap() = t0 + f(&op["day"]) * DAY;
        let e = &expected["ops"][i];
        if op["op"] == "remember" {
            let content = match op["content"].as_str() {
                Some(c) => c.to_string(),
                None => op["content_parts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| p.as_str().unwrap())
                    .collect(),
            };
            let opts = RememberOpts {
                scope: op["scope"].as_str().unwrap_or("global").to_string(),
                kind: op["kind"].as_str().unwrap_or("fact").to_string(),
                key: op["key"].as_str().map(|s| s.to_string()),
                tags: op["tags"]
                    .as_array()
                    .map(|a| a.iter().map(|t| t.as_str().unwrap().to_string()).collect())
                    .unwrap_or_default(),
                importance: op["importance"].as_f64().unwrap_or(0.5),
                pinned: op["pinned"].as_bool().unwrap_or(false),
                ttl_days: op["ttl_days"].as_f64(),
                source: op["source"].as_str().map(|s| s.to_string()),
                meta: if op["meta"].is_null() { None } else { Some(op["meta"].clone()) },
                replaces: op["replaces_ref"].as_u64().map(|r| op_ids[r as usize].clone()),
                scrub_secrets: true,
                fuzzy_dedupe: true,
                update_expiry_on_duplicate: false,
            };
            let res = store.remember(&content, &opts).unwrap();
            assert_eq!(res.status, e["status"].as_str().unwrap(), "ops[{i}].status");
            let replaced_idx: Vec<usize> = res
                .replaced
                .iter()
                .map(|id| op_ids.iter().position(|x| x == id).expect("replaced id not in op_ids"))
                .collect();
            assert_eq!(json!(replaced_idx), e["replaced"], "ops[{i}].replaced");
            assert_eq!(json!(res.redacted), e["redacted"], "ops[{i}].redacted");
            let same = op_ids.iter().position(|x| x == &res.id);
            assert_eq!(json!(same), e["same_id_as"], "ops[{i}].same_id_as");
            op_ids.push(res.id);
        } else {
            let n = if let Some(r) = op["ref"].as_u64() {
                store.forget_id(&op_ids[r as usize], false).unwrap()
            } else {
                store
                    .forget_key(op["scope"].as_str().unwrap(), op["key"].as_str().unwrap(), false)
                    .unwrap()
            };
            assert_eq!(json!(n), e["forgot"], "ops[{i}].forgot");
            op_ids.push(String::new());
        }
    }
    drop(store);

    let con = rusqlite::Connection::open(&db).unwrap();
    let mut stmt = con.prepare("SELECT * FROM memories ORDER BY created_at, rowid").unwrap();
    let rows: Vec<serde_json::Map<String, Value>> = stmt
        .query_map([], |r| {
            let mut m = serde_json::Map::new();
            for (idx, name) in r.as_ref().column_names().iter().enumerate() {
                let v: rusqlite::types::Value = r.get(idx)?;
                m.insert(
                    name.to_string(),
                    match v {
                        rusqlite::types::Value::Null => Value::Null,
                        rusqlite::types::Value::Integer(i) => json!(i),
                        rusqlite::types::Value::Real(x) => json!(x),
                        rusqlite::types::Value::Text(s) => json!(s),
                        rusqlite::types::Value::Blob(b) => json!(b),
                    },
                );
            }
            Ok(m)
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let by_id: std::collections::HashMap<String, &serde_json::Map<String, Value>> =
        rows.iter().map(|r| (r["id"].as_str().unwrap().to_string(), r)).collect();
    let exp_state = expected["state"].as_array().unwrap();
    assert_eq!(rows.len(), exp_state.len(), "state row count");
    for (i, e) in exp_state.iter().enumerate() {
        let r = &rows[i];
        for (field, _) in e.as_object().unwrap() {
            if field == "meta" {
                let got: Value = serde_json::from_str(r["meta"].as_str().unwrap()).unwrap();
                assert_eq!(got, e["meta"], "state[{i}].meta");
            } else if field == "superseded_by_created_at" {
                let got = match r["superseded_by"].as_str() {
                    Some(sid) => by_id[sid]["created_at"].clone(),
                    None => Value::Null,
                };
                assert_eq!(&got, &e["superseded_by_created_at"], "state[{i}].superseded_by_created_at");
            } else {
                assert_eq!(&r[field.as_str()], &e[field.as_str()], "state[{i}].{field}");
            }
        }
    }
}

fn recall_opts(case: &Value, t0: f64) -> RecallOpts {
    let scope = match &case["scope"] {
        Value::Null => None,
        Value::String(s) => Some(vec![s.clone()]),
        Value::Array(a) => Some(a.iter().map(|s| s.as_str().unwrap().to_string()).collect()),
        _ => panic!("bad scope"),
    };
    RecallOpts {
        scope,
        inherit: case["inherit"].as_bool().unwrap_or(true),
        kinds: case["kinds"]
            .as_array()
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default(),
        exclude_kinds: case["exclude_kinds"]
            .as_array()
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default(),
        tags: case["tags"]
            .as_array()
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default(),
        k: case["k"].as_u64().unwrap_or(8) as usize,
        budget_tokens: case["budget_tokens"].as_u64().map(|b| b as usize),
        as_of: case["as_of_day"].as_f64().map(|d| t0 + d * DAY),
        touch: false,
        explain: true,
    }
}

fn assert_recall_equal(got: &[store::Memory], exp: &[Value], label: &str) {
    assert_eq!(got.len(), exp.len(), "{label}: hit count");
    for (i, (m, e)) in got.iter().zip(exp.iter()).enumerate() {
        let at = format!("{label}[{i}] id={}", m.id);
        assert_eq!(m.id, e["id"].as_str().unwrap(), "{at} id");
        assert_eq!(m.content, e["content"].as_str().unwrap(), "{at} content");
        match (&m.score, e["score"].is_null()) {
            (None, true) => {}
            (Some(s), false) => {
                assert!((s - f(&e["score"])).abs() < 1e-9, "{at} score {s} vs {}", e["score"])
            }
            (s, _) => panic!("{at} score {s:?} vs {}", e["score"]),
        }
        if let Some(why) = &m.why {
            let ew = &e["why"];
            for key in ["channels", "relevance", "recency", "temporal"] {
                if ew[key].is_null() {
                    continue;
                }
                if key == "channels" {
                    for (ch, ev) in ew["channels"].as_object().unwrap() {
                        let gv = why["channels"][ch].as_f64().unwrap_or(f64::NAN);
                        assert!((gv - f(ev)).abs() < 1e-9, "{at} why.channels.{ch} {gv} vs {ev}");
                    }
                } else {
                    let gv = why[key].as_f64().unwrap_or(f64::NAN);
                    assert!((gv - f(&ew[key])).abs() < 1e-9, "{at} why.{key} {gv} vs {}", ew[key]);
                }
            }
            if !ew["sub_query"].is_null() {
                assert_eq!(why["sub_query"], ew["sub_query"], "{at} why.sub_query");
            }
        }
    }
}

#[test]
fn recall_on_python_db_matches_python() {
    let cases = cases();
    let expected = expected();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("memory.db");
    std::fs::copy(fixture_dir().join("memory.db"), &db).unwrap();
    let clock = Arc::new(Mutex::new(f(&expected["now"])));
    let mut store = store_at(&db, clock);
    let t0 = f(&cases["t0"]);
    for (i, case) in cases["recall"].as_array().unwrap().iter().enumerate() {
        let q = case["q"].as_str().unwrap();
        let hits = store.recall(q, &recall_opts(case, t0)).unwrap();
        assert_recall_equal(&hits, expected["recall"][i].as_array().unwrap(), &format!("recall[{i}] {q:?}"));
    }
}

#[test]
fn newer_schema_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("memory.db");
    {
        let con = rusqlite::Connection::open(&db).unwrap();
        con.pragma_update(None, "user_version", 2).unwrap();
    }
    let before = std::fs::read(&db).unwrap();
    let res = MemoryStore::open(&db);
    assert!(res.is_err());
    let after = std::fs::read(&db).unwrap();
    assert_eq!(before, after);
}

fn teresis_src() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("TERESIS_SRC") {
        let p = PathBuf::from(v);
        if p.join("teresis/store.py").exists() {
            return Some(p);
        }
    }
    let fallback = PathBuf::from("/home/nuc/Projects/teresis/src");
    if fallback.join("teresis/store.py").exists() {
        return Some(fallback);
    }
    None
}

#[test]
fn python_teresis_round_trip() {
    let Some(src) = teresis_src() else {
        eprintln!("skipped: no Teresis checkout (set TERESIS_SRC)");
        return;
    };
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipped: python3 not available");
        return;
    }
    let cases = cases();
    let expected = expected();
    let t0 = f(&cases["t0"]);
    let now = f(&expected["now"]);
    let dir = tempfile::tempdir().unwrap();
    let gen_dir = dir.path().join("gen");
    let gen = std::process::Command::new("python3")
        .arg(fixture_dir().join("gen.py"))
        .arg(&src)
        .arg(&gen_dir)
        .output()
        .unwrap();
    assert!(gen.status.success(), "gen.py failed: {}", String::from_utf8_lossy(&gen.stderr));
    let regenerated: Value =
        serde_json::from_str(&std::fs::read_to_string(gen_dir.join("expected.json")).unwrap()).unwrap();
    // uuid4 ids are random per gen.py run; mask them before the equality check.
    fn mask_ids(v: &mut Value) {
        match v {
            Value::Object(map) => {
                for (k, val) in map.iter_mut() {
                    if k == "id" {
                        *val = json!("<id>");
                    } else {
                        mask_ids(val);
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(mask_ids),
            _ => {}
        }
    }
    let (mut a, mut b) = (regenerated, expected.clone());
    mask_ids(&mut a);
    mask_ids(&mut b);
    assert_eq!(a, b, "committed fixture is stale vs live Python Teresis");

    // Rust-written DB: replay ops, compute recalls, then let Python verify.
    let replay = replay_ops();
    let db = replay.db.clone();
    let mut rust_recalls = Vec::new();
    {
        let clock = Arc::new(Mutex::new(now));
        let mut store = store_at(&db, clock);
        for case in cases["recall"].as_array().unwrap() {
            let q = case["q"].as_str().unwrap();
            rust_recalls.push(store.recall(q, &recall_opts(case, t0)).unwrap());
        }
    }
    let verify = std::process::Command::new("python3")
        .arg(fixture_dir().join("verify.py"))
        .arg(&src)
        .arg(&db)
        .arg(format!("{now}"))
        .output()
        .unwrap();
    assert!(verify.status.success(), "verify.py failed: {}", String::from_utf8_lossy(&verify.stderr));
    let report: Value = serde_json::from_slice(&verify.stdout).unwrap();
    assert_eq!(report["user_version"], 1);
    assert_eq!(report["integrity"], "ok");
    assert_eq!(report["fts_integrity"], "ok");
    assert_eq!(report["journal_mode"], "wal");

    // Same sqlite_master rows (type, name, sql) as the committed Python-made fixture.
    let master = |db: &Path| -> Vec<Vec<Value>> {
        let con = rusqlite::Connection::open(db).unwrap();
        let mut stmt = con
            .prepare("SELECT type, name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| {
            Ok(vec![
                json!(r.get::<_, String>(0)?),
                json!(r.get::<_, String>(1)?),
                json!(r.get::<_, Option<String>>(2)?),
            ])
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    assert_eq!(master(&db), master(&fixture_dir().join("memory.db")), "schema rows differ");

    // Every Python recall equals the Rust recall on the same DB and clock (Rust side
    // was computed before verify.py wrote).
    for (i, hits) in rust_recalls.iter().enumerate() {
        assert_recall_equal(hits, report["recall"][i].as_array().unwrap(), &format!("py-recall[{i}]"));
    }

    assert_eq!(report["python_write"]["duplicate"]["status"], "duplicate");
    assert_eq!(report["python_write"]["duplicate"]["id"], json!(replay.op_ids[11].clone().unwrap()));
    assert_eq!(report["python_write"]["keyed"]["status"], "superseded");
    let zed_id = replay.op_ids[4].clone().unwrap();
    assert!(
        report["python_write"]["keyed"]["replaced"]
            .as_array()
            .unwrap()
            .contains(&json!(zed_id)),
        "keyed write did not replace the Rust Zed row"
    );
    assert_eq!(
        report["python_write"]["editor_history"],
        json!([
            "Editor is Neovim",
            "Editor is Zed since the Neovim config broke",
            "Editor is Helix now"
        ])
    );

    // Reopen in Rust after Python wrote.
    let clock = Arc::new(Mutex::new(now));
    let mut store = store_at(&db, clock);
    let history = store.history_key("global", "user.editor").unwrap();
    assert_eq!(history.len(), 3);
    let hits = store
        .recall("editor", &RecallOpts {
            scope: Some(vec!["global".to_string()]),
            touch: false,
            ..recall_opts(&json!({"q":"editor","scope":"global","k":5}), t0)
        })
        .unwrap();
    assert_eq!(hits[0].content, "Editor is Helix now");
    let res = store
        .remember("Editor is Helix now", &RememberOpts { scope: "global".to_string(), ..Default::default() })
        .unwrap();
    assert_eq!(res.status, "duplicate");
    assert_eq!(res.id, report["python_write"]["keyed"]["id"].as_str().unwrap());
}
