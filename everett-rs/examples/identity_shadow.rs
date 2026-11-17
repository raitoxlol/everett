#[path = "../src/identity_shadow.rs"]
mod identity_shadow;
use identity_shadow::{AccountPrincipal, Request, Snapshot};
use serde::Deserialize;
use std::io::{self, Read};

#[derive(Deserialize)]
struct Input {
    snapshot: Snapshot,
    account: AccountPrincipal,
    requests: Vec<Request>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let input: Input = serde_json::from_str(&input)?;
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
