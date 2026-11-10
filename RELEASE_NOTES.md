# Everett 2.2.1

This maintenance release adds offline delivery diagnostics, removes recurring billable
idle inbox requests on healthy push connections, and makes send previews and cached delivery status more accurate.

## Changes

- Run `everett relay diagnose` or MCP `everett_agent_relay(action="diagnose")`
  for receiver, registration, webhook, and bounded cached-queue summaries.
  The handler makes no network calls, reads no webhook bearer files, and omits
  credential values, identifiers, URLs, and message text.
- Hibernating WebSocket notifications replace idle inbox polling by default.
  Reconnects scan the durable encrypted queue, including work queued offline.
  Local receipt, outgoing, wake, and webhook maintenance retain their five-second cadence.
  Due webhook retries reauthorize before dispatch. Logout, provider changes,
  credential replacement, and backend revocation stop unauthorized retries.
- T3, OMP extension, and attached Claude dry-run previews describe their nudge
  transports without invoking resume or queueing a message. Delivery status
  preserves authenticated replies and durable local acceptance across stale
  observations and request expiry.
- Hosted auth accepts ChatGPT's optional OAuth locale hint and public-client
  authentication advertised alongside a different default in its client
  metadata. PKCE, resource, redirect, and client allowlists are unchanged. This
  repairs consent compatibility; a fresh ChatGPT callback and tool call remain
  unverified. The Everett Local plugin source is in the repository but is not
  published with this release, and ChatGPT app loading is not claimed.
- Tagged publication waits for Worker tests, typechecking, and public bundle
  packaging. Build runners test the actual optimized CLI/MCP binary when their
  architecture matches the artifact. T3 and reply fixtures support a temporary home
  on a different filesystem from Cargo's target directory and retry Linux ETXTBSY.

The release also includes the post-2.2.0 issuer/relay hostname repair and
request-budget fix. The push receiver has no recurring
idle request schedule on a healthy connection. Startup, reconnects, active traffic,
and retries still incur requests. Production savings are not measured.

## Compatibility and rollout

Update the Worker before receiver binaries using the rollout procedure. No new
Durable Object migration or key rotation is required. Older receivers continue
to work. A push receiver fails clearly against an older Worker; explicit
`EVERETT_RELAY_TRANSPORT=poll` enables the old adaptive polling at a recurring cost. Dispatch,
inbox pickup, a correlated reply, and task completion remain separate evidence.

For a Worker deployment that changes Worker/Durable Object RPC methods, use an
immediate Durable Object code update for that deployment; see the rollout guide.

See [the release procedure](RELEASING.md), [rollout and rollback](docs/cloudflare-rollout.md),
and [messaging diagnosis](docs/messaging-troubleshooting.md).
