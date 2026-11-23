//! Text normalization, tokenization, and FTS5 query building.
//! Port of teresis/text.py — behavior must match Python exactly.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;
use sha2::{Digest, Sha256};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// Python `re` `\w` under `re.UNICODE` = `str.isalnum()` or `_`; combining marks do
/// not count (Rust regex `\w` would include them, so this is a hand scan, not a regex).
fn is_word(c: char) -> bool {
    (c.is_alphanumeric() || c == '_') && !is_combining_mark(c)
}

fn nfkc_fold(text: &str) -> String {
    caseless::default_case_fold_str(&text.nfkc().collect::<String>())
}

/// `re.findall(r"[\w][\w'’.-]*[\w]|[\w]")` — greedy run of word/`'’.-` chars
/// starting at a word char, backtracked so it ends on a word char.
fn word_tokens(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if is_word(chars[i]) {
            let mut end = i + 1;
            let mut j = i + 1;
            while j < n && (is_word(chars[j]) || matches!(chars[j], '\'' | '’' | '.' | '-')) {
                if is_word(chars[j]) {
                    end = j + 1;
                }
                j += 1;
            }
            out.push(chars[i..end].iter().collect());
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

const CONTRACTIONS: &[&str] = &[
    "aren't", "can't", "couldn't", "didn't", "doesn't", "don't", "hadn't", "hasn't", "haven't",
    "he'd", "he'll", "he's", "i'd", "i'll", "i'm", "i've", "isn't", "it's", "let's", "mightn't",
    "mustn't", "shan't", "she'd", "she'll", "she's", "shouldn't", "that's", "there's", "they'd",
    "they'll", "they're", "they've", "wasn't", "we'd", "we'll", "we're", "we've", "weren't",
    "what's", "where's", "who's", "won't", "wouldn't", "you'd", "you'll", "you're", "you've",
];

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "been", "but", "by", "can", "could", "did", "do",
    "does", "doing", "for", "from", "had", "has", "have", "having", "he", "her", "here", "hers",
    "him", "his", "how", "i", "if", "in", "into", "is", "it", "its", "itself", "just", "me", "my",
    "of", "on", "or", "our", "ours", "she", "should", "so", "some", "such", "than", "that", "the",
    "their", "them", "then", "there", "these", "they", "this", "those", "to", "too", "us", "was",
    "we", "were", "what", "when", "where", "which", "while", "who", "whom", "why", "will", "with",
    "would", "you", "your", "yours", "about", "after", "again", "all", "also", "am", "any",
    "because", "before", "being", "below", "between", "both", "down", "during", "each", "few",
    "further", "more", "most", "no", "nor", "not", "now", "off", "once", "only", "other", "out",
    "over", "own", "same", "under", "until", "up", "very",
];

fn is_stopword(w: &str) -> bool {
    STOPWORDS.contains(&w)
}

fn is_contraction(w: &str) -> bool {
    CONTRACTIONS.contains(&w)
}

/// Canonical form used for exact-duplicate detection.
pub fn normalize(text: &str) -> String {
    let folded = nfkc_fold(text);
    let mut cleaned = String::with_capacity(folded.len());
    for c in folded.chars() {
        if is_word(c) || c.is_whitespace() {
            cleaned.push(c);
        } else {
            cleaned.push(' ');
        }
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn content_hash(text: &str) -> String {
    let digest = Sha256::digest(normalize(text).as_bytes());
    hex::encode(digest)[..32].to_string()
}

pub fn tokens(text: &str, keep_stopwords: bool) -> Vec<String> {
    let mut out = Vec::new();
    for raw in word_tokens(&nfkc_fold(text)) {
        let mut tok = raw.replace('’', "'").trim_matches(|c| c == '\'' || c == '.' || c == '-').to_string();
        if tok.is_empty() {
            continue;
        }
        if !keep_stopwords && is_contraction(&tok) {
            continue;
        }
        if tok.ends_with("'s") && tok.chars().count() > 2 {
            tok.truncate(tok.len() - 2);
        }
        if tok.is_empty() || (!keep_stopwords && is_stopword(&tok)) {
            continue;
        }
        out.push(tok);
    }
    out
}

/// Light suffix stripping for similarity scoring only (FTS uses Porter).
fn stem(tok: &str) -> String {
    for suf in ["ingly", "edly", "ing", "ed", "es", "ly", "s"] {
        if tok.chars().count() > suf.len() + 2 && tok.ends_with(suf) {
            return tok[..tok.len() - suf.len()].to_string();
        }
    }
    tok.to_string()
}

pub fn term_set(text: &str) -> BTreeSet<String> {
    tokens(text, false).iter().map(|t| stem(t)).collect()
}

pub fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    a.intersection(b).count() as f64 / a.union(b).count() as f64
}

/// Fraction of the smaller set contained in the larger.
pub fn containment(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    a.intersection(b).count() as f64 / a.len().min(b.len()) as f64
}

fn quote(tok: &str) -> String {
    format!("\"{}\"", tok.replace('"', "\"\""))
}

/// OR-query over meaningful terms; BM25 rewards documents matching more of them.
pub fn fts_query(text: &str) -> Option<String> {
    let mut toks = tokens(text, false);
    if toks.is_empty() {
        toks = tokens(text, true);
    }
    let mut deduped = Vec::new();
    for t in toks {
        if !deduped.contains(&t) {
            deduped.push(t);
        }
    }
    if deduped.is_empty() {
        return None;
    }
    Some(deduped.iter().map(|t| quote(t)).collect::<Vec<_>>().join(" OR "))
}

/// OR over character trigrams so misspellings and partial words still match.
pub fn trigram_query(text: &str) -> Option<String> {
    trigram_query_n(text, 48)
}

pub fn trigram_query_n(text: &str, max_grams: usize) -> Option<String> {
    let mut grams: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for tok in tokens(text, false) {
        if !seen.contains(&tok) {
            seen.push(tok);
        }
    }
    for tok in seen {
        let chars: Vec<char> = tok.chars().collect();
        if chars.len() < 4 {
            continue;
        }
        for i in 0..chars.len() - 2 {
            let gram: String = chars[i..i + 3].iter().collect();
            if !grams.contains(&gram) {
                grams.push(gram);
            }
        }
    }
    grams.truncate(max_grams);
    if grams.is_empty() {
        return None;
    }
    Some(grams.iter().map(|g| quote(g)).collect::<Vec<_>>().join(" OR "))
}

/// Python `len` counts code points.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4).max(1)
}

fn between_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bbetween\s+(.+?)\s+and\s+(.+?)(?:\?|$)").unwrap())
}
fn first_or_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(?:first|earlier|before|longer|more recently)[,:]\s+(.+?)\s+or\s+(.+?)(?:\?|$)").unwrap()
    })
}
fn since_when_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bsince\s+(.+?)\s+when\s+(.+?)(?:\?|$)").unwrap())
}
fn compare_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bcompare\s+(.+?)\s+(?:and|with|to|vs\.?)\s+(.+?)(?:\?|$)").unwrap()
    })
}
fn quoted_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"['"]([^'"]{2,60})['"]"#).unwrap())
}

/// Split comparison / multi-entity queries into clauses so neither side drowns the other.
pub fn sub_queries(query: &str) -> Vec<String> {
    let q = query.trim();
    for pat in [between_re(), first_or_re(), since_when_re(), compare_re()] {
        if let Some(m) = pat.captures(q) {
            let a = m[1].trim_matches(|c| c == ' ' || c == ',' || c == '.' || c == '?' || c == '!').to_string();
            let b = m[2].trim_matches(|c| c == ' ' || c == ',' || c == '.' || c == '?' || c == '!').to_string();
            if !tokens(&a, false).is_empty() && !tokens(&b, false).is_empty() {
                return vec![a, b];
            }
        }
    }
    let mut quoted: Vec<String> = Vec::new();
    for m in quoted_re().captures_iter(q) {
        let inner = m[1].trim().to_string();
        if !tokens(&inner, false).is_empty() && !quoted.contains(&inner) {
            quoted.push(inner);
        }
    }
    if quoted.len() >= 2 && quoted.len() <= 3 {
        return quoted;
    }
    Vec::new()
}

/// Python `round(x, 6)` — correctly rounded via decimal formatting.
pub fn round6(x: f64) -> f64 {
    format!("{x:.6}").parse().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combining_marks_are_not_word_chars() {
        // İ casefolds to i + combining dot; Python \w drops the mark, splitting the token.
        assert_eq!(normalize("İstanbul"), "i stanbul");
    }
}
