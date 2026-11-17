#[path = "../src/identity_shadow.rs"]
mod identity_shadow;
use identity_shadow::{AccountPrincipal, Request, SessionIdentity, Snapshot};
use serde::Deserialize;
use std::io::{self, Read};

#[derive(Deserialize)]
struct Input {
    snapshot: Snapshot,
    account: AccountPrincipal,
    requests: Vec<Request>,
    #[serde(default)]
    native: Vec<NativeInput>,
    #[serde(default)]
    registered: Vec<RegisteredInput>,
}

#[derive(Deserialize)]
struct NativeInput {
    client_id: String,
    record: everett::session::Session,
}
#[derive(Deserialize)]
struct RegisteredInput {
    client_id: String,
    record: serde_json::Value,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let mut input: Input = serde_json::from_str(&input)?;
    for source in input.native {
        input.snapshot.sessions.push(SessionIdentity::from_native(
            &source.client_id,
            &source.record,
        ));
    }
    for source in input.registered {
        input
            .snapshot
            .sessions
            .push(SessionIdentity::from_registered(
                &source.client_id,
                &source.record,
            )?);
    }
    let discovery = input.snapshot.discover(&input.account)?;
    let comparisons = input
        .requests
        .into_iter()
        .map(|r| input.snapshot.compare(r))
        .collect::<Result<Vec<_>, _>>()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "shadow_only": true, "discovery": discovery, "comparisons": comparisons,
            "delivery_observations": "not_observed"
        }))?
    );
    Ok(())
}
