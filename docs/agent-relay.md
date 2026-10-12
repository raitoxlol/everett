# Set up the named-agent relay

The backend uses the same Convex deployment and OIDC audience as the
[owner relay](cross-device.md#convex-relay-owner-deployment). Read the
[threat model](relay-threat-model.md) before provisioning identities. This guide
does not deploy a backend or configure your provider for you.

## Provision identities

1. Configure your issuer to assign `org_id` and `org_role: "admin"` to each human
   org administrator. Do not assign `agent_id` to admin tokens.
2. Provision separate named agent principals through that provider. Their signed
   ID tokens must contain `org_id`, `agent_id`, and a unique `jti`. Only the issuer's
   administrators can assign these claims. End users cannot edit them as profile
   metadata. A refresh must preserve subject/org/agent identity and issue a new
   `jti`. The provider must support the configured native device-login/refresh flow
   for these principals; Everett does not mint agent tokens or accept claims as
   command-line arguments.
3. On each agent device, configure `EVERETT_AUTH_ISSUER`,
   `EVERETT_AUTH_CLIENT_ID`, and `EVERETT_CONVEX_URL`. Use `everett login --force`
   as that provisioned principal. Keep separate OS homes when running multiple
   principals. Register `everett mcp` with the harness, including OMP or Pi.

The following examples are MCP tool arguments, not shell commands.

## Register and confirm the pair

Each agent calls `everett_agent_relay` with `{"action":"register"}`. Retain its
returned agent ID, org, credential handle, and fingerprint. This creates private
per-agent keys on that device. Do not copy its key files to another device or
register different keys under the same named agent.

For same-org agents, call `everett_agent_relay` with `{"action":"directory"}`.
For cross-org agents, exchange the registered IDs out of band. Both org admins
then use their own human-authenticated MCP processes:

1. Admin A calls `everett_agent_grants` with
   `{"action":"propose","id":"A-AGENT-ID","remote_agent":"B-AGENT-ID"}`.
2. Admin A calls the same tool with `{"action":"approve","id":"GRANT-ID"}`.
3. Admin B calls `{"action":"list"}`, verifies the exact pair, and calls
   `{"action":"approve","id":"GRANT-ID"}`. Until both approvals exist, the
   agents cannot communicate or obtain remote keys through that grant.

Each agent calls `everett_agent_grants` with `{"action":"list"}` and confirms
the other agent's org, identity, and fingerprint directly on its terminal or
another trusted channel. Each calls `everett_agent_relay` with
`{"action":"pair","id":"REMOTE-AGENT-ID","fingerprint":"SHA256:CONFIRMED-HEX"}`.
Both directions need pins, including for replies and receipts.

## Receive and send

Run one foreground listener per named agent, bound to a real local session:

```sh
everett relay agent --session FULL-SESSION-ID --harness pi
# or --harness omp, claude, codex, or another installed harness
```

Use the same principal/home for the listener and its MCP process. Obtain the
recipient's exact session ID and harness out of band. They are encrypted in
messages and are not in Convex directories. To send, call `everett_agent_send`:

```json
{
  "agent": "PINNED-RECIPIENT-AGENT-ID",
  "session": "RECIPIENT-FULL-SESSION-ID",
  "harness": "pi",
  "text": "Check the parser tests and reply with the result",
  "idempotency_key": "parser-check-1",
  "session_id": "MY-FULL-SESSION-ID"
}
```

`queued` is not pickup. Retry an uncertain send with the identical request and
key, never a fresh key unless you intend a second message. The recipient uses
the existing `everett_inbox` and `everett_send(reply_to=...)` tools. Cross-org inbox
items are labeled external/untrusted. Keep those labels when processing JSON.

Call `everett_agent_status` with `{"message_id":"RETURNED-MESSAGE-ID"}`.
`server_state: "accepted"` means durable import and server ciphertext deletion.
Local `delivered` requires an encrypted pickup receipt, and `replied` requires a
verified correlated reply. Both listeners must be running to observe them. None
of these operations wake or route a harness. Normal `everett_send` retains its
Jev/local routing behavior; SSH peer commands are unchanged.

## Revoke access

Either org admin calls `everett_agent_grants` with
`{"action":"revoke","id":"GRANT-ID"}`. Revocation blocks pending messages
under that grant; a new bilateral grant cannot revive the old ciphertext.

`everett_agent_relay` also exposes:

- `credentials`: list visible credential handles, not bearer tokens.
- `revoke-credential`, with `id`: permanently disable that issuer `jti`. Agent
  callers can revoke only their own named agent's credentials. A new issuer token
  can register again unless the named agent itself has been revoked.
- `revoke-agent`, with `id`: org-admin-only, permanently disables every credential
  for that named agent, including future tokens.
- `memberships` and `revoke-membership`, with the returned membership `id`:
  org-admin-only, disables that account's membership and all its agents in the org.
- `unpair`, with `id`: remove local trust. Pair again only after independent
  confirmation. Unpairing alone does not revoke server authorization.

Revoke a lost device's refresh credentials at the provider as well. Already-issued
ID tokens can outlive provider revocation; server revocation provides the immediate
relay gate. If refresh changes `jti`, the listener exits and must be restarted to
register that new issuer credential. A changed org/agent/account never continues
silently. A transient failure retains the encrypted local retry record, but server
queueing is unconfirmed until a successful response. Production claim issuance
and reconnect behavior require a real deployment trial; fake tests do not prove them.
