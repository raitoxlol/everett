use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{EverettError, Result};
use crate::session::Session;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthorCard {
    pub owner: String,
    pub author: String,
    pub agent: String,
    pub session: String,
    pub harness: String,
    pub card: String,
    pub updated_at: f64,
    pub target: String,
}

pub fn expose_id(id: &str, harness: Option<&str>, remove: bool) -> Result<Value> {
    if remove {
        return update_card(id, harness.unwrap_or("unknown"), "", true);
    }
    let sessions = crate::registry::scan(24.0 * 365.0, true, None, harness.unwrap_or(""));
    let matches: Vec<_> = sessions.iter().filter(|session| session.id == id).collect();
    if matches.len() != 1 {
        return Err(EverettError::new(
            2,
            "Exposure requires one exact local session ID and harness",
        ));
    }
    expose(matches[0], remove)
}

pub fn expose(session: &Session, remove: bool) -> Result<Value> {
    let card = crate::cards::read_card_body(&session.id)
        .map(|(body, _)| body)
        .unwrap_or_else(|| session.card.clone());
    update_card(&session.id, &session.harness, &card, remove)
}

fn update_card(session: &str, harness: &str, card: &str, remove: bool) -> Result<Value> {
    let mut units = 0;
    let card: String = card
        .chars()
        .take_while(|character| {
            units += character.len_utf16();
            units <= 640
        })
        .collect();
    crate::relay::admin_call(
        "relay:exposeSession",
        json!({"session":session,"harness":harness,"card":card,"remove":remove}),
        false,
    )?;
    Ok(json!({"session":session,"exposed":!remove}))
}

pub fn authors() -> Result<Vec<AuthorCard>> {
    authors_with(|cursor| {
        crate::relay::admin_call("relay:sessionDirectory", json!({"cursor":cursor}), true)
    })
}

fn authors_with(mut query: impl FnMut(Value) -> Result<Value>) -> Result<Vec<AuthorCard>> {
    let mut cards = Vec::new();
    let mut cursor = Value::Null;
    let mut seen = HashSet::new();
    loop {
        let page = query(cursor)?;
        let rows: Vec<AuthorCard> = serde_json::from_value(page["sessions"].clone())
            .map_err(|error| EverettError::new(2, format!("invalid author directory: {error}")))?;
        cards.extend(rows);
        if page["isDone"] == true {
            return Ok(cards);
        }
        let next = page["continueCursor"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| EverettError::new(2, "author directory cursor is missing"))?;
        if !seen.insert(next.to_string()) || seen.len() > 256 {
            return Err(EverettError::new(
                2,
                "author directory cursor did not advance",
            ));
        }
        cursor = json!(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authors_follows_empty_pages_and_rejects_repeated_cursors() {
        let mut calls = 0;
        let cards = authors_with(|cursor| {
            calls += 1;
            assert_eq!(
                cursor,
                if calls == 1 {
                    Value::Null
                } else {
                    json!("next")
                }
            );
            Ok(json!({"sessions":[],"isDone":calls == 2,"continueCursor":"next"}))
        })
        .unwrap();
        assert!(cards.is_empty());
        assert_eq!(calls, 2);
        assert!(authors_with(|_| Ok(
            json!({"sessions":[],"isDone":false,"continueCursor":"same"})
        ))
        .is_err());
    }
}
