/// Build provenance, not a statement that this artifact is deployed elsewhere.
pub fn value() -> serde_json::Value {
    serde_json::json!({"version": env!("CARGO_PKG_VERSION"), "revision": env!("EVERETT_BUILD_REVISION"), "dirty": env!("EVERETT_BUILD_DIRTY") == "true"})
}
