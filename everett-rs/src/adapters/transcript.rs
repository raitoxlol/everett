use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use serde_json::{json, Value};

use crate::error::{EverettError, Result};
use crate::session::Session;

use super::{devin, grok, hermes};

pub(crate) fn read(session: &Session, messages: usize) -> Result<Vec<Value>> {
    if session.harness == "devin" && Path::new(&session.path).extension().and_then(|e| e.to_str()) != Some("json") {
        return Err(EverettError::new(4, "read is not supported for devin SQLite sessions yet."));
    }
    let unreadable = |e: String| EverettError::new(4, format!("Cannot read the {} transcript: {e}", session.harness));
    let messages = if session.harness == "devin" {
        devin::transcript_messages(Path::new(&session.path), messages).map_err(unreadable)?
    } else if session.harness == "hermes" {
        hermes::messages(&std::path::PathBuf::from(&session.path), &session.id, messages)
            .map_err(unreadable)?
            .into_iter()
            .map(|(role, text)| json!({"role": role, "text": text.chars().take(4000).collect::<String>()}))
            .collect()
    } else {
        let file = std::fs::File::open(jsonl_path(Path::new(&session.path)))
            .map_err(|e| unreadable(e.to_string()))?;
        read_transcript_tail(BufReader::new(file), messages).map_err(|e| unreadable(e.to_string()))?
    };
    Ok(messages)
}

// Grok indexes directories because changes to its files determine session activity.
pub(crate) fn jsonl_path(path: &Path) -> Cow<'_, Path> {
    if path.is_dir() {
        Cow::Owned(path.join(grok::HISTORY_FILE))
    } else {
        Cow::Borrowed(path)
    }
}

// Stream through the file while keeping just the most recent text messages.
fn read_transcript_tail(mut reader: impl BufRead, limit: usize) -> io::Result<Vec<Value>> {
    let mut out = VecDeque::new();
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let Ok(row) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some((role, body)) = find_message(&row, 0) {
            if !body.trim().is_empty() {
                let body: String = body.chars().take(4000).collect();
                if out.len() == limit {
                    out.pop_front();
                }
                out.push_back(json!({"role": role, "text": body}));
            }
        }
    }
    Ok(out.into())
}

fn find_message(v: &Value, depth: usize) -> Option<(String, String)> {
    let obj = v.as_object()?;
    if obj.contains_key("synthetic_reason") {
        return None;
    }
    // Grok rows carry the speaker in `type` instead of `role`.
    let role = obj.get("role").or_else(|| obj.get("type")).and_then(Value::as_str);
    if let (Some(role), Some(content)) = (role, obj.get("content")) {
        if matches!(role, "user" | "assistant") {
            return Some((role.to_string(), content_text(content)));
        }
    }
    if depth >= 3 {
        return None;
    }
    obj.values().find_map(|child| find_message(child, depth + 1))
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter()
            .filter(|p| matches!(p.get("type").and_then(Value::as_str), Some("text" | "input_text" | "output_text")))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_reads_claude_codex_and_plain_rows() {
        let text = [
            r#"{"type":"user","message":{"role":"user","content":"hi claude"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello"},{"type":"tool_use","name":"x"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"codex ask"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"hidden"}]}}"#,
            r#"{"role":"assistant","content":"plain"}"#,
            "not json",
        ].join("\n");
        let all = read_transcript_tail(text.as_bytes(), 10).unwrap();
        let texts: Vec<_> = all.iter().map(|m| m["text"].as_str().unwrap()).collect();
        assert_eq!(texts, ["hi claude", "hello", "codex ask", "plain"]);
        let last = read_transcript_tail(text.as_bytes(), 2).unwrap();
        assert_eq!(last.len(), 2);
        assert_eq!(last[1]["text"], "plain");
    }
}
