# Everett 2.2.1

This maintenance release adds offline delivery diagnostics, reduces quiet
relay polling, and makes send previews and cached delivery status more accurate.

## Changes

- Run `everett relay diagnose` or MCP `everett_agent_relay(action="diagnose")`
  for receiver, registration, webhook, and bounded cached-queue summaries.
  The handler makes no network calls, reads no webhook bearer files, and omits
  credential values, identifiers, URLs, and message text.
- Idle inbox polling backs off to 60 seconds. Active work and local receipt,
  outgoing, wake, and webhook maintenance retain their five-second cadence.
  Due webhook retries reauthorize before dispatch. Logout, provider changes,
  credential replacement, and backend revocation stop unauthorized retries.
- T3, OMP extension, and attached Claude dry-run previews describe their nudge
  transports without invoking resume or queueing a message. Delivery status
  preserves authenticated replies and durable local acceptance across stale
  observations and request expiry.
- Tagged publication waits for Worker tests, typechecking, and public bundle
  packaging. Build runners test the actual optimized CLI/MCP binary when their
  architecture matches the artifact. The T3 fixture supports a temporary home
  on a different filesystem from Cargo's target directory.

The candidate also includes the post-2.2.0 issuer/relay hostname repair and
request-budget fix. With both the Worker and adaptive receiver updated, the
calculated quiet budget is 2,880 Durable Object requests per receiver per day.
This excludes active traffic and retries. Production savings are not measured.

## Compatibility and rollout

Update the Worker before receiver binaries using the rollout procedure. A quiet recipient can
wait up to 60 seconds plus processing time before its next inbox poll. Dispatch,
inbox pickup, a correlated reply, and task completion remain separate evidence.

See [the release procedure](RELEASING.md), [rollout and rollback](docs/cloudflare-rollout.md),
and [messaging diagnosis](docs/messaging-troubleshooting.md).
