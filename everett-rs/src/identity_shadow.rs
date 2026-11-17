//! Offline candidate model. Deliberately not imported by the Everett binary.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AccountPrincipal {
    pub issuer: String,
    pub subject: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientDevice {
    pub id: String,
    pub account: AccountPrincipal,
    pub revoked: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    HostAttested,
    OwnerRegistered,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Registration {
    Registered,
    Unavailable,
    Revoked,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reachability {
    Unknown,
    Online,
    Stale,
    Offline,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionIdentity {
    pub client_id: String,
    pub session_id: String,
    pub name: String,
    pub kind: String,
    pub provenance: Provenance,
    pub registration: Registration,
    pub reachability: Reachability,
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl SessionIdentity {
    pub fn from_native(client_id: &str, record: &everett::session::Session) -> Self {
        Self {
            client_id: client_id.into(),
            session_id: record.id.clone(),
            name: record.title.clone(),
            kind: record.harness.clone(),
            provenance: Provenance::HostAttested,
            registration: Registration::Registered,
            reachability: Reachability::Unknown,
            aliases: vec![],
        }
    }

    pub fn from_registered(client_id: &str, record: &serde_json::Value) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct Record {
            session_id: String,
            name: String,
            kind: String,
            state: String,
        }
        let record: Record = serde_json::from_value(record.clone()).map_err(|e| e.to_string())?;
        let registration = match record.state.as_str() {
            "registered" => Registration::Registered,
            "registration_unavailable" => Registration::Unavailable,
            "revoked" => Registration::Revoked,
            _ => return Err("unsupported registration state".into()),
        };
        Ok(Self {
            client_id: client_id.into(),
            session_id: record.session_id,
            name: record.name,
            kind: record.kind,
            provenance: Provenance::OwnerRegistered,
            registration,
            reachability: Reachability::Unknown,
            aliases: vec![],
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub clients: Vec<ClientDevice>,
    pub sessions: Vec<SessionIdentity>,
}

#[derive(Debug, Serialize)]
pub struct Discovered<'a> {
    pub identity: &'a SessionIdentity,
    pub label: String,
    pub client_revoked: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Message,
    Wake,
    Execute,
    TranscriptRead,
    SharedMemory,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Request {
    pub account: AccountPrincipal,
    pub from: String,
    pub to: String,
    pub capability: Capability,
    pub current: Decision,
}

#[derive(Debug, Serialize)]
pub struct Comparison {
    pub current: Decision,
    pub proposed: Decision,
    pub changed: bool,
    pub canonical_recipient: Option<String>,
    pub reason: &'static str,
    pub shadow_only: bool,
}

impl Snapshot {
    pub fn validate(&self) -> Result<(), String> {
        let mut clients = BTreeSet::new();
        for c in &self.clients {
            if c.id.is_empty()
                || c.account.issuer.is_empty()
                || c.account.subject.is_empty()
                || !clients.insert(&c.id)
            {
                return Err("invalid or duplicate client mapping".into());
            }
        }
        let mut ids = BTreeSet::new();
        for s in &self.sessions {
            if !clients.contains(&s.client_id) || s.session_id.is_empty() {
                return Err("missing client mapping or session ID".into());
            }
            for id in std::iter::once(&s.session_id).chain(s.aliases.iter()) {
                if id.is_empty() || !ids.insert(id) {
                    return Err("ambiguous session ID or alias; explicit migration required".into());
                }
            }
        }
        Ok(())
    }

    pub fn discover(&self, account: &AccountPrincipal) -> Result<Vec<Discovered<'_>>, String> {
        self.validate()?;
        Ok(self
            .sessions
            .iter()
            .filter_map(|s| {
                let client = self
                    .clients
                    .iter()
                    .find(|c| c.id == s.client_id && &c.account == account)?;
                Some(Discovered {
                    identity: s,
                    label: format!("{} [{}]", s.name, s.session_id),
                    client_revoked: client.revoked,
                })
            })
            .collect())
    }

    fn resolve(&self, account: &AccountPrincipal, id: &str) -> Option<&SessionIdentity> {
        self.sessions.iter().find(|s| {
            (s.session_id == id || s.aliases.iter().any(|alias| alias == id))
                && s.registration == Registration::Registered
                && self
                    .clients
                    .iter()
                    .any(|c| c.id == s.client_id && &c.account == account && !c.revoked)
        })
    }

    pub fn compare(&self, request: Request) -> Result<Comparison, String> {
        self.validate()?;
        let sender = self.resolve(&request.account, &request.from);
        let recipient = self.resolve(&request.account, &request.to);
        let (proposed, reason) = if sender.is_none() || recipient.is_none() {
            (
                Decision::Deny,
                "unknown, foreign, unavailable or revoked identity",
            )
        } else if request.capability != Capability::Message {
            (
                Decision::Deny,
                "same-owner messaging conveys no other capability",
            )
        } else {
            (
                Decision::Allow,
                "same-owner registered identities; reachability is separate",
            )
        };
        Ok(Comparison {
            changed: request.current != proposed,
            current: request.current,
            proposed,
            canonical_recipient: recipient.map(|s| s.session_id.clone()),
            reason,
            shadow_only: true,
        })
    }
}
