# Messages and status

## Sub-features

Inbox delivery, replies, consumption, subscriptions, status events and headless child input.

## How to get to it (user POV)

`everett send "request" --to SESSION --mode inbox`, `everett reply MESSAGE_ID "answer"`, `everett inbox --session SESSION --json`, and MCP send/inbox/subscribe/event tools.

## Driving it with the release helper

An MCP request receives a CLI reply; a CLI request receives an MCP reply. Verify message ids, reply_to, text and destination. A second inbox read is empty after consumption. A done event must arrive in the subscriber's inbox and appear in session listing and CLI events.

A fake Codex reads stdin during both resume and spawn over MCP. It must receive EOF, and the server must answer a subsequent ping. This catches a harness consuming or waiting on the parent's protocol pipe.

## Gotchas

`cargo test --locked --test relay_diagnostics` exercises `everett relay diagnose` and MCP `everett_agent_relay(action="diagnose")` over isolated homes. It checks live local lease state, matching and mismatched cached identities, redaction, absence of network calls, unchanged relay files, unsafe records, scan bounds, and reply receipts after expiry. Counts describe cached files across all stored accounts, not remote authorization or task completion.

This proves file delivery and subprocess/stdio behavior; actual models and hook injection need separate live checks. The unit suite covers interrupted delivery guidance without retries, hop limits and self-send guards.
