# Agent-facing encrypted relay MCP

The four agent tools share the same hardened v2 protocol, credentials, key pins and
delivery observations as the CLI. There is one backend and one receiver, not separate
account and agent transports. SSH remains independently paired.

Deploy, configure issuer-controlled `everett_agent`/`everett_org`/`everett_role`, and
provision authoritative identities using [the owner runbook](cross-device.md). The
temporary `agent_id`/`jti` enrollment contract and v1 pins are incompatible; do not
silently migrate an existing deployment or discard revocation/replay records.

## Same-owner Cloudflare devices

Run `everett relay` on each registered device. Same-owner Cloudflare devices verify
active signed bundles through the authenticated service without manual pins.
Call `everett_agent_relay` with `{"action":"names"}` to discover hosted names.
Send through `everett_send` to `Max NUC/codex/SESSION_ID` using a unique name.
An exact `id:DEVICE_ID/HARNESS/SESSION_ID` selector works without name metadata.
Duplicate or incomplete name directories fail closed. Names select an ID; they do
not replace signed device identity. Existing SSH aliases retain their transport.

The Cloudflare account service is the same-owner trust authority. It can authorize
a new key bundle; a compromised service can substitute a same-owner identity.
Cloudflare does not support cross-owner traffic. The retired OIDC/Convex
implementation retains manual pins and bilateral grants below for historical
reference. It is not the active v2.2 transport.

Receiver wake support is separate from device trust. See
[agent support and wake](agent-support.md).

## Legacy Convex registration and pairing

Start `everett relay` on each device. It writes to local inboxes. Optional local
receiver policies control wake actions. Within the agent's Everett MCP connection:

```json
{"action":"identity"}
{"action":"register"}
{"action":"directory"}
```

Call `everett_agent_relay` with each object. Identity/registration require the owner's
OIDC login and deployment-side provisioning. Directory lists registered, active agents
only within the authenticated namespace. Exchange full public bundles and fingerprints
out of band; a registry response alone is not trust. Pin the other agent:

```json
{"action":"pair","id":"machine-b","fingerprint":"B_CONFIRMED_64_CHARACTER_LOWERCASE_HEX"}
```

The alias defaults to `id`; optional `name` selects another local alias. For cross-org
pairing also supply `grant_id`, only after the grant is active. Existing aliases never
change keys/transport silently. Unpair with `{"action":"unpair","id":"ALIAS"}`.

## Legacy Convex bilateral cross-org grants

Call `everett_agent_grants`. Org A admin invites one local and one remote agent, with
both independently verified fingerprints and the remote's exact namespace:

```json
{"action":"invite","id":"org-a-org-b-1","device":"machine-a","peer_owner":"[\"https://login.example.com/realms/everett\",\"org\",\"org-b\"]","remote_agent":"machine-c","local_fingerprint":"A_CONFIRMED_HEX","peer_fingerprint":"C_CONFIRMED_HEX","rate_per_minute":30,"body_cap_bytes":65536}
```

Optional `expires_at` is an absolute Unix-millisecond timestamp. Org B admin reverses
the fingerprint orientation and calls:

```json
{"action":"accept","id":"org-a-org-b-1","local_fingerprint":"C_CONFIRMED_HEX","peer_fingerprint":"A_CONFIRMED_HEX"}
```

Use `list` or `get` to inspect grants. `propose`/`approve` alias `invite`/`accept`, but
still require the same explicit fingerprints. A key rotation blocks delivery until
both admins `reconfirm` the new pair. Either side can `revoke` with the grant `id`;
revocation is terminal. There is no wildcard org permission or automatic waking.

## Send, observe, reply

Call `everett_agent_send` with the pinned alias and exact remote session/harness:

```json
{"agent":"machine-b","session":"REMOTE-EXACT-SESSION-ID","harness":"pi","text":"Please inspect the queued work","idempotency_key":"work-1","session_id":"LOCAL-SESSION-ID"}
```

The MCP process must have a host-detected caller identity. `session_id` is optional and, when supplied, must match that binding. If native detection is unavailable, the owner must set `EVERETT_SESSION_ID` in the session's MCP launch environment; a model argument cannot establish identity. Retries must reuse the exact request and key.
The recipient uses `everett_inbox` and `everett_send(reply_to="LOCAL-INBOX-ID", text="…")`.
External messages retain an explicit external/untrusted label through hooks, MCP and
forwarding; remote-derived callers are inbox-only, never resumed/spawned as owner prompts.

Call `everett_agent_status` with `{"message_id":"RETURNED-MESSAGE-ID"}`. `queued`
means acknowledged enqueue, `delivered` means durable recipient acceptance, `claimed`
means observed local inbox pickup, and `replied` means correlated reply enqueue plus a
signed observation. The listener retries pickup/reply observations; none proves task
completion. Server ciphertext is deleted on delivery; local inbox retention is separate.

## Revoke and inspect

`everett_agent_relay` actions `credentials` and `memberships` return scoped metadata,
never JWTs or private keys. Admins see at most 256 own-org rows with a truncation flag;
members see only their own credential and cannot list memberships.

- `revoke-agent`, `id`: exact provisioned agent ID; self or org admin.
- `revoke-credential`, `id`: alias for the same permanent per-agent revocation, **not**
  a temporary JWT/`jti` block that later refresh can evade.
- `revoke-membership`, `id`: exact provider subject/UUID; org admin only.

Backend tombstones deny still-valid JWTs and future registration for those identities.
Also revoke provider refresh/offline sessions after device compromise. See the
[threat model](relay-threat-model.md) for visible metadata, endpoint-compromise and
no-forward-secrecy limits. Live OIDC/Convex and physical-machine trials remain unverified.
