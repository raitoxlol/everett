# Dashboard status JSON

Use `everett status <message-id> --json` or native MCP `everett_status` for
a request from the current local sender. For an encrypted relay request, use
`everett peer status <peer> <message-id>` or native MCP `everett_agent_status`.
Lookups remain scoped to the current sender or relay account and key. A missing,
foreign, or unauthorized ID returns the existing error.

Version 1 adds `schema_version`, `delivery`, and `agent` to the existing JSON
object. Read `delivery.state` for a common state across both transports. Older
readers can continue using the top-level `state`, `posted`, `picked_up`, and
`reply` fields where those fields were already available.

| `delivery.state` | Evidence |
| --- | --- |
| `queued` | The request was written to the local inbox or the relay confirmed the queue. |
| `picked_up` | The target inbox acknowledged the request or the relay verified a signed claim. |
| `replied` | There is a correlated reply or a signed relay reply observation. A human reply can precede target pickup. |
| `expired` | The request or its retry window reached expiry without stronger retained evidence. Earlier pickup can still be uncertain. |
| `unknown` | Available evidence cannot establish a listed state. This includes an unconfirmed relay receipt and relay `delivered` (durable device ingress). |

The four `delivery.evidence` fields use Unix milliseconds and are nullable.

- `queued_at_ms` records local enqueue or relay confirmation. The relay uses
  the Worker timestamp when available and otherwise uses the local enqueue
  acknowledgement time. A staged outgoing envelope alone supplies no queue time.
- `picked_up_at_ms` records local inbox acknowledgement or a signed relay claim.
  A legacy acknowledgement can prove pickup with a null timestamp.
- `replied_at_ms` records the local reply, the signed relay observation, or the
  time an authenticated correlated reply was imported. Each is evidence of a
  response. Task success requires the response itself.
- `expires_at_ms` is the deadline. Older or missing receipt fields remain null
  wherever the corresponding timestamp cannot be recovered.

Remote status uses state and timestamps from the same Worker response.
Observed timestamps are also retained with the outgoing receipt so a locally
imported reply can show prior claim and queue evidence.

`agent` is a separate, self-reported card snapshot. An agent can write three
single lines in its card using `What:`, `State:`, and `Next:` labels. Its
`state` is text, `what` and `next` are nullable, and `updated_at_ms` is the
card modification time. The `freshness` field is `current` for usable cards
less than 24 hours old, `stale` for older agent cards, and `unknown` when
the card is unavailable, automatic, future-dated, or lacks a usable state.
Stale and unknown cards return `state: "unknown"` and null descriptions.
Relay-only lookups return unknown freshness. Use the agent and delivery fields
independently.

Fixture examples cover every delivery state, including a stale card with a
retained correlated reply. [dashboard-status-v1.json](fixtures/dashboard-status-v1.json)
