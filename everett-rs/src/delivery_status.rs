use serde::Serialize;
use serde_json::{json, Value};

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeliveryState {
    Queued,
    PickedUp,
    Replied,
    Expired,
    Unknown,
}

#[derive(Default, Serialize)]
pub(crate) struct Evidence {
    pub queued_at_ms: Option<i64>,
    pub picked_up_at_ms: Option<i64>,
    pub replied_at_ms: Option<i64>,
    pub expires_at_ms: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CardFreshness {
    Current,
    Stale,
    Unknown,
}

#[derive(Serialize)]
pub(crate) struct AgentStatus {
    pub freshness: CardFreshness,
    pub what: Option<String>,
    pub state: String,
    pub next: Option<String>,
    pub updated_at_ms: Option<i64>,
}

impl AgentStatus {
    pub(crate) fn unknown(updated_at_ms: Option<i64>) -> Self {
        Self {
            freshness: CardFreshness::Unknown,
            what: None,
            state: "unknown".into(),
            next: None,
            updated_at_ms,
        }
    }
}

pub(crate) fn unix_ms(seconds: f64) -> Option<i64> {
    (seconds.is_finite() && seconds > 0.0 && seconds * 1000.0 < i64::MAX as f64)
        .then(|| (seconds * 1000.0).round() as i64)
}

pub(crate) fn with_status(
    mut legacy: Value,
    state: DeliveryState,
    evidence: Evidence,
    agent: AgentStatus,
) -> Value {
    legacy["schema_version"] = json!(1);
    legacy["delivery"] = json!({ "state": state, "evidence": evidence });
    legacy["agent"] = json!(agent);
    legacy
}
