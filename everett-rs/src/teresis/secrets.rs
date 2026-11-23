//! Deterministic secret detection and redaction.
//! Port of teresis/secrets.py — same rules, same order, same replacements.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::{Captures, Regex};

/// Published documentation/example values that match real key shapes.
const ALLOWLIST: &[&str] = &[
    "AKIAIOSFODNN7EXAMPLE",                     // AWS docs access key
    "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", // AWS docs secret key
];

/// Words that mark a value as a placeholder rather than a live credential.
const PLACEHOLDER_WORDS: &[&str] = &[
    "your", "youre", "placeholder", "example", "sample", "dummy", "fake", "test", "testing",
    "demo", "redacted", "change", "changeme", "insert", "replace", "here", "todo", "none", "null",
    "na", "notset", "n/a", "secret", "token", "key", "apikey", "password", "passwd", "pwd",
    "credential", "credentials", "auth", "user", "username", "admin", "root", "real", "actual",
    "some", "my", "the", "this", "put",
];

const TRAIL: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\''];

fn placeholder_parts(v: &str) -> HashSet<String> {
    static SPLIT: OnceLock<Regex> = OnceLock::new();
    let re = SPLIT.get_or_init(|| Regex::new(r"[^a-z0-9]+").unwrap());
    re.split(&v.to_lowercase())
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect()
}

fn has_placeholder_word(v: &str) -> bool {
    placeholder_parts(v)
        .iter()
        .any(|p| PLACEHOLDER_WORDS.contains(&p.as_str()))
}

fn looks_masked(v: &str) -> bool {
    static MASK: OnceLock<Regex> = OnceLock::new();
    let re = MASK.get_or_init(|| Regex::new(r"^[x*._\-]+$").unwrap());
    re.is_match(v)
}

/// True when `v` looks like a doc placeholder/reference rather than a real credential.
fn is_placeholder(v: &str) -> bool {
    let v = v
        .trim()
        .trim_end_matches(TRAIL)
        .trim_matches(|c| c == '\'' || c == '"');
    if v.chars().count() < 16 || ALLOWLIST.contains(&v) {
        return true;
    }
    let first = v.chars().next().unwrap();
    if "$<`{[(".contains(first) || v.chars().any(|c| "([{<$".contains(c)) {
        return true;
    }
    if looks_masked(v) {
        return true;
    }
    has_placeholder_word(v)
}

type Check = fn(&Captures) -> bool;
type Repl = fn(&Captures, &str) -> String;

struct Rule {
    kind: &'static str,
    pattern: &'static Regex,
    check: Option<Check>,
    repl: Option<Repl>,
}

fn env_check(m: &Captures) -> bool {
    !is_placeholder(&m["v"])
}

fn env_repl(m: &Captures, marker: &str) -> String {
    let v_full = &m["v"];
    let v = v_full.trim_end_matches(TRAIL);
    let keep = format!("{}{}{}{}", &m["n"], &m["qc"], &m["s"], &m["q"]);
    format!("{keep}{marker}{}", &v_full[v.len()..])
}

fn uri_check(m: &Captures) -> bool {
    let v = &m[1];
    if v.chars().count() < 4 || ALLOWLIST.contains(&v) || "$<`{[(".contains(v.chars().next().unwrap()) {
        return false;
    }
    if looks_masked(v) {
        return false;
    }
    !has_placeholder_word(v)
}

fn uri_repl(m: &Captures, marker: &str) -> String {
    let whole = &m[0];
    let v_len = m[1].len();
    format!("{}{marker}@", &whole[..whole.len() - v_len - 1])
}

fn bearer_repl(m: &Captures, marker: &str) -> String {
    match m.get(1) {
        None => marker.to_string(),
        Some(g) => {
            let whole = &m[0];
            format!("{}{marker}", &whole[..whole.len() - g.as_str().len()])
        }
    }
}

fn aws_check(m: &Captures) -> bool {
    !ALLOWLIST.contains(&&m[0])
}

fn env_pattern() -> String {
    const ENV_NAME: &str = concat!(
        r"(?:[a-z][a-z0-9]*[_-])+",
        r"(?:key|secret|token|password|passwd|pwd|credentials?|auth|apikey)",
        r"|api[_-]?key|access[_-]?key|secret[_-]?key|private[_-]?key|client[_-]?secret",
        r"|(?:auth|access|refresh|session|bearer|id|api)[_-]?token|apikey",
        r"|key|secret|token|password|passwd|pwd|credentials?"
    );
    format!(
        r#"(?i)\b(?P<n>{ENV_NAME})(?P<qc>['"]?)(?P<s>[\t ]*[:=][\t ]*)(?P<q>['"]?)(?P<v>[^\s'";,#]+)"#
    )
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        vec![
            Rule {
                kind: "private_key",
                pattern: Box::leak(Box::new(
                    Regex::new(
                        r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----(?:.*?-----END [A-Z0-9 ]*PRIVATE KEY-----|.*)",
                    )
                    .unwrap(),
                )),
                check: None,
                repl: None,
            },
            Rule {
                kind: "anthropic_key",
                pattern: Box::leak(Box::new(Regex::new(r"\bsk-ant-[A-Za-z0-9_-]{20,}").unwrap())),
                check: None,
                repl: None,
            },
            Rule {
                kind: "openai_key",
                pattern: Box::leak(Box::new(Regex::new(r"\bsk-[A-Za-z0-9_-]{20,}").unwrap())),
                check: None,
                repl: None,
            },
            Rule {
                kind: "stripe_key",
                pattern: Box::leak(Box::new(
                    Regex::new(r"\b(?:[sr]k_(?:live|test)_|whsec_)[A-Za-z0-9]{16,}").unwrap(),
                )),
                check: None,
                repl: None,
            },
            Rule {
                kind: "resend_key",
                pattern: Box::leak(Box::new(Regex::new(r"\bre_[A-Za-z0-9_]{20,}").unwrap())),
                check: None,
                repl: None,
            },
            Rule {
                kind: "github_token",
                pattern: Box::leak(Box::new(
                    Regex::new(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})").unwrap(),
                )),
                check: None,
                repl: None,
            },
            Rule {
                kind: "slack_token",
                pattern: Box::leak(Box::new(Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{10,}").unwrap())),
                check: None,
                repl: None,
            },
            Rule {
                kind: "huggingface_token",
                pattern: Box::leak(Box::new(Regex::new(r"\bhf_[A-Za-z0-9]{20,}").unwrap())),
                check: None,
                repl: None,
            },
            Rule {
                kind: "aws_access_key",
                pattern: Box::leak(Box::new(Regex::new(r"\bAKIA[0-9A-Z]{16}").unwrap())),
                check: Some(aws_check),
                repl: None,
            },
            Rule {
                kind: "bearer_token",
                pattern: Box::leak(Box::new(
                    Regex::new(
                        r"(?i)\bbearer\s+([A-Za-z0-9._~+/=-]{20,})|\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
                    )
                    .unwrap(),
                )),
                check: None,
                repl: Some(bearer_repl),
            },
            Rule {
                kind: "uri_credential",
                pattern: Box::leak(Box::new(
                    Regex::new(r"\b[a-z][a-z0-9+.-]*://[^\s/:@]+:([^\s/@]+)@").unwrap(),
                )),
                check: Some(uri_check),
                repl: Some(uri_repl),
            },
            Rule {
                kind: "env_secret",
                pattern: Box::leak(Box::new(Regex::new(&env_pattern()).unwrap())),
                check: Some(env_check),
                repl: Some(env_repl),
            },
        ]
    })
}

fn ok(rule: &Rule, m: &Captures) -> bool {
    rule.check.map(|c| c(m)).unwrap_or(true)
}

/// Kind of the first credential-looking span in `text`, else None.
pub fn contains_secret(text: &str) -> Option<&'static str> {
    for rule in rules() {
        for m in rule.pattern.captures_iter(text) {
            if ok(rule, &m) {
                return Some(rule.kind);
            }
        }
    }
    None
}

/// Replace every credential-looking span with `[REDACTED:<kind>]`.
/// Returns `(scrubbed_text, kinds)` — each matched kind once, in rule order.
pub fn redact_secrets(text: &str) -> (String, Vec<String>) {
    let mut text = text.to_string();
    let mut kinds: Vec<&'static str> = Vec::new();
    for rule in rules() {
        let kinds_ref = &mut kinds;
        let replaced = rule
            .pattern
            .replace_all(&text, |m: &Captures| {
                if !ok(rule, m) {
                    return m[0].to_string();
                }
                if !kinds_ref.contains(&rule.kind) {
                    kinds_ref.push(rule.kind);
                }
                let marker = format!("[REDACTED:{}]", rule.kind);
                match rule.repl {
                    Some(repl) => repl(m, &marker),
                    None => marker,
                }
            })
            .into_owned();
        text = replaced;
    }
    (text, kinds.into_iter().map(|k| k.to_string()).collect())
}
