use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::config;
use crate::error::{EverettError, Result};
use crate::proc::shlex_quote;
use crate::session::{home, now, Session};

pub const JEV_URL: &str = "https://api.typesafe.ai/v1/systemone";
pub const JEV_MODEL: &str = "jev-1.13.0";
pub const MIN_CONFIDENCE: f64 = 0.6;
pub const INSTRUCTIONS: &str =
    "An instruction arrived for a developer who runs many parallel coding-agent sessions. \
     Pick the one session whose ongoing work this instruction continues. \
     Choose \"new\" if it is clearly a new, separate task. \
     Choose \"none\" if it is ambiguous between sessions or too vague to place.";

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "how", "i", "in", "is", "it",
    "of", "on", "or", "our", "the", "this", "to", "we", "with",
];

/// Jev key sources, in order: TYPESAFE_API_KEY, ~/.everett/config.toml, ~/.hermes/.env.
pub fn find_api_key() -> String {
    let key = config::get("typesafe_api_key", Some("TYPESAFE_API_KEY"), "");
    if !key.is_empty() {
        return key;
    }
    if let Ok(text) = fs::read_to_string(home().join(".hermes").join(".env")) {
        for line in text.lines() {
            if let Some((name, value)) = line.split_once('=') {
                if name.trim() == "TYPESAFE_API_KEY" {
                    return value.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
                }
            }
        }
    }
    String::new()
}

pub fn verify_key(key: &str) -> bool {
    let crit = json!({
        "new": "start a new session",
        "none": "ambiguous or too vague to place",
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    call_jev("ping", &crit, key).is_ok()
}

/// cwd (or git repo) basename, casefolded — how a request names a project.
pub fn project_name(session: &Session) -> String {
    session.cwd.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_lowercase()
}

const COORDINATOR_TERMS: &[&str] = &[
    "everett_send",
    "everett send",
    "everett_route",
    "everett route",
    "everett_ls",
    "everett ls",
    "coordinator",
    "coordinating",
    "coordinate sessions",
    "asking the",
    "ask the",
    "list sessions",
    "listing sessions",
    "dispatch to",
    "delegate to",
    "delegating to",
];

/// True for a session whose activity is mostly talking about/steering other sessions.
pub fn is_coordinator(session: &Session) -> bool {
    let text = format!("{} {} {} {}", session.card, session.title, session.first_user, session.last_user)
        .to_lowercase();
    COORDINATOR_TERMS.iter().any(|term| text.contains(term))
}

pub fn describe(session: &Session) -> String {
    let project = {
        let p = project_name(session);
        if p.is_empty() { "no-project".to_string() } else { p }
    };
    let role = if is_coordinator(session) { " role=coordinator" } else { "" };
    let prefix = format!("[{}] project={}{} cwd={}", session.harness, project, role, session.cwd);
    if !session.card.is_empty() {
        return format!("{} — {}", prefix, session.card);
    }
    let headline = if !session.title.is_empty() {
        session.title.clone()
    } else {
        session.first_user.chars().take(100).collect()
    };
    let last: String = session.last_user.chars().take(160).collect();
    format!("{} — {} — last: {}", prefix, headline, last)
}

/// Candidate sessions for routing: never the caller's own session; coordinators are dropped
/// unless the request explicitly names that coordinator's own project.
pub fn relevant_sessions(text: &str, sessions: &[Session], caller_id: Option<&str>) -> Vec<Session> {
    let query_tokens: HashSet<String> = tokens(text).into_iter().collect();
    let without_caller: Vec<Session> = sessions
        .iter()
        .filter(|s| !(caller_id.is_some() && s.id == caller_id.unwrap()))
        .cloned()
        .collect();
    let mut filtered: Vec<Session> = without_caller
        .iter()
        .filter(|s| {
            let proj = project_name(s);
            let named = !proj.is_empty() && query_tokens.contains(&proj);
            !is_coordinator(s) || named
        })
        .cloned()
        .collect();
    let mut pool = if filtered.is_empty() { without_caller } else { std::mem::take(&mut filtered) };
    pool.sort_by_key(|s| (!query_tokens.contains(&project_name(s)), is_coordinator(s)));
    pool
}

/// (criteria for Jev, option key -> session)
pub fn criteria(sessions: &[Session]) -> (Map<String, Value>, HashMap<String, Session>) {
    let mut options = HashMap::new();
    let mut crit = Map::new();
    for (i, session) in sessions.iter().enumerate() {
        let key = format!("s{}", i);
        crit.insert(key.clone(), json!(describe(session)));
        options.insert(key, session.clone());
    }
    crit.insert("new".into(), json!("Belongs to no existing session; start a new session."));
    crit.insert("none".into(), json!("Ambiguous between sessions or too vague to place."));
    (crit, options)
}

pub fn resume_command(session: &Session, text: &str) -> String {
    if crate::adapters::external::is_external(&session.harness) {
        return format!(
            "everett send {} --to {} --mode inbox   # queues only; the external agent must poll Everett through MCP",
            shlex_quote(text),
            shlex_quote(&session.id)
        );
    }
    if session.source == "t3code" {
        return format!(
            "# Continue in T3 Code; poll everett_inbox(session_id={}). Do not resume this thread with the provider CLI.",
            json!(session.id)
        );
    }
    let cwd = if session.cwd.is_empty() { "." } else { &session.cwd };
    let cd = format!("cd {}", shlex_quote(cwd));
    match session.harness.as_str() {
        "claude" => format!(
            "{} && claude --resume {} {}",
            cd,
            shlex_quote(&session.id),
            shlex_quote(text)
        ),
        "codex" => format!(
            "{} && codex resume {} {}",
            cd,
            shlex_quote(&session.id),
            shlex_quote(text)
        ),
        "pi" => format!(
            "{} && pi --session {}   # then paste the text",
            cd,
            shlex_quote(&session.path)
        ),
        "hermes" => format!(
            "{} && hermes -p {} chat --resume {}   # then paste the text",
            cd,
            shlex_quote(if session.profile.is_empty() { "default" } else { &session.profile }),
            shlex_quote(&session.id)
        ),
        "grok" => format!(
            "{} && grok --resume {}   # then paste the text",
            cd,
            shlex_quote(&session.id)
        ),
        "devin" => format!(
            "{} && devin --resume {}   # then paste the text",
            cd,
            shlex_quote(&session.id)
        ),
        _ => format!("{} && omp -r {}   # then paste the text", cd, shlex_quote(&session.id)),
    }
}

pub fn default_harness() -> String {
    config::get("default_harness", Some("EVERETT_HARNESS"), "claude")
}

/// Manual command for starting a fresh interactive session with this request.
pub fn new_command(text: &str) -> String {
    format!("{} {}", shlex_quote(&default_harness()), shlex_quote(text))
}

pub fn call_jev(text: &str, crit: &Map<String, Value>, key: &str) -> Result<Map<String, Value>> {
    let payload = json!({
        "model": JEV_MODEL,
        "state": serde_json::to_string(&json!({"incoming_instruction": text})).unwrap(),
        "questions": {
            "route": {
                "type": "choice",
                "instructions": INSTRUCTIONS,
                "criteria": crit,
            }
        }
    });
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build();
    let agent: ureq::Agent = config.into();
    let mut resp = agent
        .post(JEV_URL)
        .header("Authorization", &format!("Bearer {}", key))
        .header("Content-Type", "application/json")
        .send_json(&payload)
        .map_err(|e| {
            EverettError::new(2, format!("Jev unavailable or returned an invalid response: {}", e))
        })?;
    let result: Value = resp.body_mut().read_json().map_err(|e| {
        EverettError::new(2, format!("Jev unavailable or returned an invalid response: {}", e))
    })?;
    result
        .get("answers")
        .and_then(|a| a.get("route"))
        .and_then(|r| r.as_object().cloned())
        .ok_or_else(|| {
            EverettError::new(2, "Jev unavailable or returned an invalid response: missing answers.route".to_string())
        })
}

fn token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[^\W_]+").unwrap())
}

pub fn tokens(text: &str) -> Vec<String> {
    let lowered = text.to_lowercase();
    token_re()
        .find_iter(&lowered)
        .map(|m| m.as_str().to_string())
        .filter(|t| !STOP_WORDS.contains(&t.as_str()) && (t.chars().count() > 1 || !t.is_ascii()))
        .collect()
}

fn document(session: &Session) -> String {
    let project = session.cwd.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let headline = if !session.title.is_empty() { &session.title } else { &session.first_user };
    format!("{} {} {} {}", session.card, headline, session.last_user, project)
}

fn bm25(
    query: &[String],
    doc: &[String],
    df: &HashMap<String, usize>,
    count: usize,
    average_length: f64,
) -> f64 {
    if query.is_empty() || doc.is_empty() {
        return 0.0;
    }
    let k1 = 1.2;
    let b = 0.75;
    let mut score = 0.0;
    let unique: HashSet<&String> = query.iter().collect();
    for token in unique {
        let frequency = doc.iter().filter(|t| *t == token).count() as f64;
        if frequency == 0.0 {
            continue;
        }
        let df = *df.get(token).unwrap_or(&0) as f64;
        let inverse = (1.0 + (count as f64 - df + 0.5) / (df + 0.5)).ln();
        score += inverse * frequency * (k1 + 1.0)
            / (frequency + k1 * (1.0 - b + b * doc.len() as f64 / average_length));
    }
    score
}

const PROJECT_BOOST: f64 = 3.0;

/// [(score, option key, coverage)] best first; empty when the request has no terms.
pub fn rank(text: &str, sessions: &[Session], at: Option<f64>) -> Vec<(f64, String, f64)> {
    let query = tokens(text);
    if query.is_empty() || sessions.is_empty() {
        return Vec::new();
    }
    let query_set: HashSet<String> = query.iter().cloned().collect();
    let docs: Vec<(String, Vec<String>)> = sessions
        .iter()
        .enumerate()
        .map(|(i, s)| (format!("s{}", i), tokens(&document(s))))
        .collect();
    let mut df: HashMap<String, usize> = HashMap::new();
    for token in &query_set {
        df.insert(
            token.clone(),
            docs.iter().filter(|(_, doc)| doc.contains(token)).count(),
        );
    }
    let average_length = (docs.iter().map(|(_, d)| d.len()).sum::<usize>() as f64 / docs.len() as f64).max(1.0);
    let current = at.unwrap_or_else(now);
    let mut ranked: Vec<(f64, String, f64)> = Vec::new();
    for (key, doc) in &docs {
        let idx: usize = key[1..].parse().unwrap_or(0);
        let session = &sessions[idx];
        let mut raw_score = bm25(&query, doc, &df, docs.len(), average_length);
        let age_hours = ((current - session.last_active) / 3600.0).max(0.0);
        let recency = 1.0 / (1.0 + age_hours / 72.0);
        let mut coverage = query_set.iter().filter(|t| doc.contains(*t)).count() as f64
            / query_set.len() as f64;
        let project = project_name(session);
        if !project.is_empty() && query_set.contains(&project) {
            raw_score = raw_score * PROJECT_BOOST + PROJECT_BOOST;
            coverage = coverage.max(1.0 / query_set.len() as f64);
        }
        ranked.push((raw_score * recency, key.clone(), coverage));
    }
    ranked.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1[1..].parse::<usize>().unwrap_or(0).cmp(&b.1[1..].parse::<usize>().unwrap_or(0)))
    });
    ranked
}

/// Working directory of the best lexical match, for spawning a NEW session nearby.
pub fn best_dir(text: &str, sessions: &[Session]) -> String {
    for (score, key, _) in rank(text, sessions, None) {
        let idx: usize = key[1..].parse().unwrap_or(0);
        let cwd = &sessions[idx].cwd;
        if score > 0.0 && !cwd.is_empty() && cwd != "/" {
            return cwd.clone();
        }
    }
    String::new()
}

fn local_route_result(input: &str, choice: &str, confidence: f64) -> Map<String, Value> {
    let mut r = Map::new();
    r.insert("input".into(), json!(input));
    r.insert("choice".into(), json!(choice));
    r.insert("confidence".into(), json!(confidence));
    r
}

pub fn local_route(text: &str, sessions: &[Session], at: Option<f64>, caller_id: Option<&str>) -> Map<String, Value> {
    let sessions = relevant_sessions(text, sessions, caller_id);
    if sessions.is_empty() {
        let mut r = local_route_result(text, "new", 0.95);
        r.insert("decision".into(), json!("NEW"));
        r.insert("command".into(), json!(new_command(text)));
        return r;
    }
    let ranked = rank(text, &sessions, at);
    if ranked.is_empty() {
        let mut r = local_route_result(text, "none", 0.0);
        r.insert("decision".into(), json!("ASK"));
        r.insert("suggested".into(), Value::Null);
        r.insert(
            "candidates".into(),
            json!(sessions.iter().take(3).map(describe).collect::<Vec<_>>()),
        );
        return r;
    }
    let (best_score, best_key, coverage) = &ranked[0];
    if *best_score <= 0.0 || *coverage < 0.2 {
        let mut r = local_route_result(text, "new", 0.72);
        r.insert("decision".into(), json!("NEW"));
        r.insert("command".into(), json!(new_command(text)));
        return r;
    }
    let second_score = if ranked.len() > 1 { ranked[1].0 } else { 0.0 };
    let margin = if *best_score != 0.0 { (best_score - second_score) / best_score } else { 0.0 };
    let confidence = (0.25 + 0.3 * coverage + 0.45 * margin).min(0.99);
    let best_idx: usize = best_key[1..].parse().unwrap_or(0);
    if confidence < MIN_CONFIDENCE {
        let candidates: Vec<String> = ranked
            .iter()
            .take(3)
            .map(|(_, key, _)| describe(&sessions[key[1..].parse::<usize>().unwrap_or(0)]))
            .collect();
        let mut r = local_route_result(text, "none", (confidence * 1000.0).round() / 1000.0);
        r.insert("decision".into(), json!("ASK"));
        r.insert("suggested".into(), json!(describe(&sessions[best_idx])));
        r.insert("candidates".into(), json!(candidates));
        return r;
    }
    let session = &sessions[best_idx];
    let mut r = local_route_result(text, best_key, (confidence * 1000.0).round() / 1000.0);
    r.insert("decision".into(), json!("SESSION"));
    r.insert("session".into(), session.to_dict());
    r.insert("command".into(), json!(resume_command(session, text)));
    r
}

fn normalize(text: &str, answer: &Map<String, Value>, options: &HashMap<String, Session>) -> Result<Map<String, Value>> {
    let choice = answer.get("choice").and_then(|v| v.as_str()).unwrap_or("");
    let confidence = answer.get("confidence").and_then(|v| v.as_f64());
    let confidence = match confidence {
        Some(c) if c.is_finite() => c,
        _ => return Err(EverettError::new(2, "Jev returned no valid confidence.")),
    };
    let mut base = Map::new();
    base.insert("input".into(), json!(text));
    base.insert(
        "choice".into(),
        answer.get("choice").cloned().unwrap_or(Value::Null),
    );
    base.insert("confidence".into(), answer.get("confidence").cloned().unwrap_or(Value::Null));
    if let Some(session) = options.get(choice) {
        if confidence >= MIN_CONFIDENCE {
            base.insert("decision".into(), json!("SESSION"));
            base.insert("session".into(), session.to_dict());
            base.insert("command".into(), json!(resume_command(session, text)));
            return Ok(base);
        }
    }
    if choice == "new" && confidence >= MIN_CONFIDENCE {
        base.insert("decision".into(), json!("NEW"));
        base.insert("command".into(), json!(new_command(text)));
        return Ok(base);
    }
    let mut candidates: Vec<String> = options.get(choice).map(describe).into_iter().collect();
    if candidates.is_empty() {
        let pool: Vec<Session> = {
            let mut keys: Vec<&String> = options.keys().collect();
            keys.sort_by_key(|k| k[1..].parse::<usize>().unwrap_or(usize::MAX));
            keys.into_iter().map(|k| options[k].clone()).collect()
        };
        candidates = rank(text, &pool, None)
            .into_iter()
            .take(3)
            .map(|(_, key, _)| describe(&pool[key[1..].parse::<usize>().unwrap_or(0)]))
            .collect();
    }
    base.insert("decision".into(), json!("ASK"));
    base.insert(
        "suggested".into(),
        options.get(choice).map(|s| json!(describe(s))).unwrap_or(Value::Null),
    );
    base.insert("candidates".into(), json!(candidates));
    Ok(base)
}

type JevFn = dyn Fn(&str, &Map<String, Value>, &str) -> Result<Map<String, Value>>;

/// Route with Jev when a key exists (or router='jev'), otherwise the local BM25 router.
pub fn route(text: &str, sessions: &[Session], router: Option<&str>, caller_id: Option<&str>) -> Result<Map<String, Value>> {
    route_with(text, sessions, router, None, &call_jev, caller_id)
}

pub fn route_with(
    text: &str,
    sessions: &[Session],
    router: Option<&str>,
    api_key: Option<&str>,
    jev: &JevFn,
    caller_id: Option<&str>,
) -> Result<Map<String, Value>> {
    let key = api_key.map(|k| k.to_string()).unwrap_or_else(find_api_key);
    let selected = router
        .map(|r| r.to_string())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| {
            let cfg = config::get("router", Some("EVERETT_ROUTER"), "");
            if !cfg.is_empty() {
                cfg
            } else if !key.is_empty() {
                "jev".to_string()
            } else {
                "local".to_string()
            }
        });
    if selected == "local" {
        let mut r = local_route(text, sessions, None, caller_id);
        r.insert("router".into(), json!("local"));
        return Ok(r);
    }
    if selected != "jev" {
        return Err(EverettError::new(2, "router must be \"local\" or \"jev\"."));
    }
    if key.is_empty() {
        return Err(EverettError::new(
            3,
            "Jev router requested but no key found: set TYPESAFE_API_KEY, \
             typesafe_api_key in ~/.everett/config.toml, or use --router local.",
        ));
    }
    let (crit, options) = criteria(&relevant_sessions(text, sessions, caller_id));
    let answer = jev(text, &crit, &key)?;
    let mut r = normalize(text, &answer, &options)?;
    r.insert("router".into(), json!("jev"));
    Ok(r)
}
