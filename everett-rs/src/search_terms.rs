use std::sync::OnceLock;

use regex::Regex;

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "how", "i", "in", "is", "it",
    "of", "on", "or", "our", "the", "this", "to", "we", "with",
];

fn token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[^\W_]+").unwrap())
}

pub(crate) fn words(text: &str) -> Vec<String> {
    token_re().find_iter(&text.to_lowercase()).map(|m| m.as_str().to_string()).collect()
}

pub(crate) fn tokens(text: &str) -> Vec<String> {
    words(text)
        .into_iter()
        .filter(|t| !STOP_WORDS.contains(&t.as_str()) && (t.chars().count() > 1 || !t.is_ascii()))
        .collect()
}
