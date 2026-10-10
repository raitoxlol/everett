---
name: verify-everett
description: Verify Everett releases, CLI setup, and all ten stdio MCP tools using isolated session stores. Use after routing, messaging, status, memory, or packaging changes.
---
# Verify Everett

## Launch

Everett is one native binary built from `everett-rs/` (macOS/Linux). Build it with `cargo build --release --locked --manifest-path everett-rs/Cargo.toml`. The public surface is the CLI and `everett mcp`. No long-running background server is needed: the journey test starts its own MCP process and closes it afterwards.

## Drive

```sh
cd everett-rs
cargo test --locked --test release_journey                                    # the built test binary
EVERETT_VERIFY_BINARY="$PWD/target/release/everett" cargo test --locked --test release_journey   # a packaged binary
```

Read [the feature map](features/README.md). `tests/release_journey.rs` runs all four mapped journeys with the real CLI and JSON-RPC pipes and checks every advertised tool. It uses no model credentials or paid calls. Synthetic Claude/Codex transcripts stand in for external storage. The Claude stub is detection-only and exits 88 if invoked. A shell Codex stub simulates resume/spawn and reads stdin; it must receive EOF while the MCP connection stays open.

## Evidence

The test asserts doctor's ten-tool probe, onboarding before any session store, cards, inbox consumption, replies, status, core files, older targets, child stdin, a ping after delivery, and that all ten tools were called. This is CLI/protocol evidence; GUI-client trust prompts and actual model resumes need separate checks.

## Cleanup

The test removes its own temporary home and only kills its own MCP subprocess. Never operate on real harness stores. Run the rest of the suite with `cargo test --locked`.
