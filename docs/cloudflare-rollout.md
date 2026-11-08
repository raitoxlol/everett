# Roll out the relay without idle polling

Use this procedure after review and explicit authorization for the named Worker and receivers. The candidate reports 2.2.1; identify exact bytes by source and executable hashes. The previously installed adaptive 2.2.1 binary does not contain push notifications.

## Record the candidate and rollback targets

1. Pin the reviewed source SHA, patch, and executable hash. Verify macOS and Linux with the project checks. Keep all account credentials out of evidence.
2. Inventory running receivers directly. Record each executable path, hash, supervisor command, Everett home, PID, and device identity. An owner object count does not establish the receiver count.
3. Authenticate Wrangler privately. Record the active Worker versions, traffic percentages, namespace IDs, routes, bindings, and migrations. Confirm the public target is Worker `everett` in account `17bafde311907d7c3d81a3fcbfd31990`, serving `everett.raitox.tech` and `relay.raitox.tech`. Reconcile the deployed source with the candidate so later hosted-auth repairs are preserved.
4. Retain the prior Worker version and each canary's previous binary and supervisor command. Preserve existing homes, keys, pins, encrypted queues, receipts, and outboxes. Push uses the existing `OwnerRelay` and `HostedAuth` namespaces. It requires no new migration, binding, secret, login, or key rotation.
5. Record a baseline UTC analytics window. Stop if production source/configuration cannot be reconciled or a migration is pending.

Run these read-only commands from `cloudflare/` with the pinned Wrangler:

```sh
npm exec -- wrangler deployments list --config wrangler.public.jsonc --json
npm exec -- wrangler versions list --config wrangler.public.jsonc
```

## Verify the artifacts

Run from the repository root with isolated fixtures. Preserve command logs and hashes.

```sh
cargo test --locked --manifest-path everett-rs/Cargo.toml
cargo clippy --locked --manifest-path everett-rs/Cargo.toml --all-targets -- -D warnings
cargo build --release --locked --manifest-path everett-rs/Cargo.toml
EVERETT_VERIFY_BINARY="$PWD/everett-rs/target/release/everett" cargo test --locked --manifest-path everett-rs/Cargo.toml --test release_journey --test relay_diagnostics --test relay_push
npm --prefix cloudflare ci
npm --prefix cloudflare test
npm --prefix cloudflare run typecheck
npm --prefix cloudflare run build:public
```

`build:public` performs a dry run. A passing local bundle does not establish a deployment. The Rust tag workflow publishes artifacts; do not create a tag during preparation.

## Deploy the Worker before one receiver

1. After authorization, deploy the reviewed Worker using the public config. Record the new version ID and verify the active deployment. Check existing native sign-in, hosted connectors, and an enrolled polling receiver before updating a client. Existing polling operations remain available.
2. Stop exactly one authorized receiver through its existing supervisor. Replace only its executable and restart with the same home and identity. Confirm one receiver process, the intended executable hash, a valid capability lease, and successful push connection. Do not re-enroll or rotate keys.
3. Verify a request and correlated reply between the canary and an existing peer. Confirm recipient inbox content and signed sender status. Duplicate sends with the same idempotency key must not duplicate inbox delivery. Record IDs and timing without bodies or credentials.
4. Observe at least five quiet minutes. Healthy push connections must create no scheduled inbox requests. Protocol Ping/Pong is expected and does not count as an application heartbeat. Send queued work and verify notification-driven pickup without a 60-second polling wait. Local claims and outgoing work must still progress on the five-second maintenance cycle.
5. On a disposable canary, interrupt the connection, queue encrypted work offline, then reconnect. Confirm recovery without duplicate publication. Verify logout, credential expiry/replacement, member/device revocation, and signed key rotation stop invalid subscriptions. Never revoke an owner's working device for a test. Due webhook retries must still authorize before dispatch; transient uncertainty must preserve retry state.

After a healthy hour and analytics inspection, expand one receiver at a time. Record final hashes and counts. Do not count downtime as a saving.

The authorized deployment command, from `cloudflare/`, is:

```sh
npm exec -- wrangler deploy --config wrangler.public.jsonc
```

A new push receiver fails explicitly against an older Worker. If compatibility polling is required, explicitly set `EVERETT_RELAY_TRANSPORT=poll` or `[relay] transport = "poll"`. That mode resumes the recurring request cost and can wait up to 60 seconds for quiet inbox pickup.

## Measure the account request budget

1. Resolve namespace IDs using `GET /accounts/{account_id}/workers/durable_objects/namespaces`. Include production/development `OwnerRelay`, `HostedAuth`, and other account workloads.
2. Query `durableObjectsInvocationsAdaptiveGroups` for explicit UTC windows and `sum.requests`, grouped by namespace. Query `workersInvocationsAdaptive` separately. Confirm schema/filter names with introspection. Preserve complete results, sampling metadata, pagination, and GraphQL errors.
3. Compare equal windows before and after the canary. Record updated receiver counts, connection/reconnect frequency, message volume, failures, and downtime. Separate startup, active work, retries, and idle periods. Wait for analytics to populate and check the next complete UTC day.
4. Calculate `requests / window_seconds * 86400` as an observed run rate only when the window represents the intended workload. A quiet push connection has no recurring idle request schedule. Reconnect storms, receipt retries, messages, login, and other account traffic can still exhaust the 100,000 daily allowance. Use 80,000/day as an operational headroom target, not a promised capacity.
5. Check duration, SQL rows, storage, and the billing view separately. Hibernation avoids idle socket duration charges; active handlers and storage remain metered. Analytics request counts alone do not establish a dollar reduction.

| Stage | UTC window | Worker version | Receiver hashes/count | DO requests by namespace | Worker requests | Reconnects/messages/errors |
| --- | --- | --- | --- | --- | --- | --- |
| Baseline | Pending | Pending | Pending | Pending | Pending | Pending |
| Worker with old receivers | Pending | Pending | Pending | Pending | Pending | Pending |
| One push canary | Pending | Pending | Pending | Pending | Pending | Pending |
| Updated receivers | Pending | Pending | Pending | Pending | Pending | Pending |

## Roll back a failed stage

1. Stop expansion for missing/duplicate delivery, unauthorized publication, authentication failure, restart failure, or unhealthy reconnects.
2. Restore the affected receiver's saved binary through its supervisor first. Preserve its home, identity, queues, and outbox. Verify a round trip and signed status. Expect polling costs to return.
3. If the Worker caused the fault, restore its recorded prior version after authorization. Rolling the Worker back first can disconnect push clients and leave them without a supported endpoint. Check bindings/migrations and all existing connectors afterward.
4. Record version IDs, executable hashes, delivery outcome, and request-rate impact. Worker rollback restores code, not revoked credentials or modified object data.

From `cloudflare/`, substitute the recorded version:

```sh
npm exec -- wrangler rollback PREVIOUS_VERSION_ID --config wrangler.public.jsonc
```

References: [WebSocket hibernation](https://developers.cloudflare.com/durable-objects/best-practices/websockets/), [request and duration pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/), [analytics](https://developers.cloudflare.com/durable-objects/observability/metrics-and-analytics/), and [rollback limits](https://developers.cloudflare.com/workers/versions-and-deployments/rollbacks/).
