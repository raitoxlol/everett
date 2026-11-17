# MCP Events candidate

The authenticated HTTP gateway optionally supports `message.created` using the
[OpenAI MCP Events contract](https://developers.openai.com/plugins/build/mcp-events.md).
Events is disabled by default. The binding's `events` boolean enables the candidate;
this change does not enable it in any live configuration.

The existing Streamable HTTP initialize response advertises `events`. The same
authenticated `/mcp` endpoint accepts `events/list`, `events/subscribe`, and
`events/unsubscribe`. No polling interface replaces webhook delivery.

Subscriptions select the connection inbox or a registered inbox using its private
`identity_handle`. An optional `session_id` must match that selection. Account,
client, and handle boundaries use the current gateway identity implementation.
OAuth credentials and private handles are revalidated before delivery. Temporary
introspection failures defer delivery; revoked credentials or handles remove the
subscription. Existing tool authorization and destination grants are unchanged.

Callbacks require HTTPS, public destination addresses checked at connection time,
no redirects, a fresh signed challenge, a successful HTTP response, and a constant
time challenge match. Standard Webhooks signatures cover exact serialized bytes.
HTTP 2xx acknowledges receipt, not downstream processing. Bursts produce one event
per request; ChatGPT controls task batching. Payloads contain only inbox and message IDs; message text is retrieved separately.
Subscription identity is deterministic. Refresh is idempotent; successful callback
verification is cached for five minutes, and key rotation has a five-minute overlap.
Subscriptions expire after at most one day, capped at the verified OAuth token
expiry. Refresh with a fresh token before that deadline to retain subscription
identity and delivery history. Caller-supplied expiry claims are not used. Delivery retries keep the event ID,
use exponential backoff, and stop after eight attempts. HTTP 410 removes the
subscription; 413 and acknowledged deliveries are never retried.

The private SQLite store lives under `.everett/gateway-events` in the gateway home.
It contains callback signing secrets and OAuth credentials needed for revalidation.
Never publish it or copy it into evidence. The store permits 64 subscriptions and
10,000 retained delivery records per subscription. Oversized individual inbox
records and exhausted delivery capacity block the subscription explicitly; a
refresh then fails until unsubscribe/resubscribe. Historical inbox size alone does
not block delivery.

Verification uses synthetic stores, local HTTP fixtures, and mock webhook delivery.
These tests do not establish current ChatGPT subscription or live delivery success.
