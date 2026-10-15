use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{EverettError, Result};
use crate::relay::crypto::Device;

fn bad(message: impl std::fmt::Display) -> EverettError {
    EverettError::new(2, format!("device registry: {message}"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryDevice {
    #[serde(flatten)]
    device: Device,
    name: String,
    ssh_address: Option<String>,
    metadata_signature: Option<String>,
    #[serde(default, deserialize_with = "optional_timestamp")]
    revoked_at: Option<i64>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    pub name: String,
    pub device_id: String,
    #[serde(default, deserialize_with = "optional_timestamp")]
    pub last_seen_at: Option<i64>,
    pub ssh_address: Option<String>,
    #[serde(flatten)]
    pub flags: serde_json::Map<String, Value>,
}

fn optional_timestamp<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<i64>, D::Error> {
    Option::<f64>::deserialize(deserializer)?
        .map(|at| {
            if at.is_finite() && at.fract() == 0.0 && at.abs() <= 9_007_199_254_740_991.0 {
                Ok(at as i64)
            } else {
                Err(serde::de::Error::custom(
                    "timestamp must be a safe whole number of milliseconds",
                ))
            }
        })
        .transpose()
}

pub(crate) fn metadata_bytes(
    owner: &str,
    device: &str,
    name: &str,
    address: Option<&str>,
) -> Vec<u8> {
    serde_json::to_vec(&json!([
        "everett-registry-v2",
        owner,
        device,
        name,
        address
    ]))
    .unwrap()
}

pub(crate) fn declared_address(address: Option<&str>) -> Result<Option<String>> {
    let value = address
        .map(str::to_string)
        .unwrap_or_else(|| crate::config::get("peer.address", None, ""));
    if value.is_empty() {
        return Ok(None);
    }
    crate::peer::parse_registry_address(&value)?;
    Ok(Some(value))
}

struct Client {
    url: String,
    token: String,
    owner: String,
    agent: ureq::Agent,
}

impl Client {
    fn connect() -> Result<Self> {
        let (stored, owner) = crate::relay::account()?;
        let endpoint = crate::config::get("relay.url", Some("EVERETT_CONVEX_URL"), "");
        let url =
            url::Url::parse(&endpoint).map_err(|_| bad("set EVERETT_CONVEX_URL or [relay] url"))?;
        let loopback = url
            .host_str()
            .is_some_and(|host| host == "127.0.0.1" || host == "[::1]");
        if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(bad(
                "Convex deployment must be an HTTPS origin (HTTP is allowed only on loopback)",
            ));
        }
        let client = Self {
            url: endpoint.trim_end_matches('/').into(),
            token: stored.id_token,
            owner,
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(20)))
                .max_redirects(0)
                .build()
                .into(),
        };
        let identity = client.call("relay:whoami", json!({}))?;
        if identity["owner"] != client.owner {
            return Err(bad(
                "verified backend identity differs from local token claims",
            ));
        }
        let claims = crate::relay::claims(&client.token)?;
        if identity["deviceId"] != claims["everett_agent"] {
            return Err(bad(
                "verified backend agent differs from local token claims",
            ));
        }
        Ok(client)
    }

    fn call(&self, path: &str, args: Value) -> Result<Value> {
        let mut response = self
            .agent
            .post(format!("{}/api/query", self.url))
            .header("Authorization", &format!("Bearer {}", self.token))
            .send_json(json!({"path":path,"args":args,"format":"json"}))
            .map_err(|_| bad("Convex query failed"))?;
        let response: Value = response
            .body_mut()
            .with_config()
            .limit(1024 * 1024)
            .read_json()
            .map_err(|_| bad("invalid Convex response"))?;
        match response["status"].as_str() {
            Some("success") => response
                .get("value")
                .cloned()
                .ok_or_else(|| bad("missing Convex result")),
            Some("error") => Err(bad(response["errorMessage"]
                .as_str()
                .unwrap_or("Convex denied the query"))),
            _ => Err(bad("invalid Convex response status")),
        }
    }
}

pub fn devices() -> Result<Vec<DeviceSummary>> {
    let client = Client::connect()?;
    let mut rows = Vec::new();
    let mut cursor = Value::Null;
    let mut cursors = HashSet::new();
    loop {
        let page = client.call("registry:devices", json!({"cursor":cursor}))?;
        let devices = page["devices"]
            .as_array()
            .ok_or_else(|| bad("invalid registry page"))?;
        for row in devices {
            let mut summary: DeviceSummary = serde_json::from_value(row.clone()).map_err(bad)?;
            summary.flags.retain(|key, _| key == "revokedAt");
            rows.push(summary);
        }
        if page["isDone"] == true {
            break;
        }
        let next = page["continueCursor"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("invalid registry cursor"))?;
        if !cursors.insert(next.to_string()) {
            return Err(bad("registry cursor did not advance"));
        }
        cursor = json!(next);
    }
    Ok(rows)
}

pub fn resolve(name: &str) -> Result<Option<String>> {
    let client = Client::connect()?;
    let value = client.call("registry:resolve", json!({"name":name}))?;
    if value.is_null() {
        return Ok(None);
    }
    let row: RegistryDevice = serde_json::from_value(value).map_err(bad)?;
    if row.name != name || row.device.owner != client.owner {
        return Err(bad("registry returned a different name or owner"));
    }
    if row.revoked_at.is_some() {
        return Err(bad("device is revoked"));
    }
    row.device.verify()?;
    if let Some(address) = &row.ssh_address {
        crate::relay::crypto::verify(
            &row.device.signing_key,
            row.metadata_signature
                .as_deref()
                .ok_or_else(|| bad("unsigned SSH address; register the device again"))?,
            &metadata_bytes(
                &client.owner,
                &row.device.device_id,
                &row.name,
                Some(address),
            ),
        )?;
        crate::peer::parse_registry_address(address)?;
    }
    Ok(row.ssh_address)
}
