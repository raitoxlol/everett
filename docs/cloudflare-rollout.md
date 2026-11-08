# Roll out the relay request reductions

Use this procedure after review and explicit authorization to deploy and update receivers. Preparation does not authorize a release, tag, installation, restart, or live mutation.

## Record the candidate and rollback targets

1. Pin the reviewed source SHA and clean patch. The implementation base is main `6faee61b7602a34f1e6ba4e072bfd3ab8f22b7e9`, including quota fix #126 at `6ba52ed14730172f2d843ff9c5c8c2fe18d9340e`. The adaptive receiver must contain both changes. The local candidate reports 2.2.1, but a version string alone does not identify tested bytes. Record the executable hash and verify the exact artifact before rollout. A candidate version does not authorize publication.
2. Inventory every running receiver on every host. Record the actual executable path, SHA-256, source provenance, service command, selected Everett home, process ID, and account/device identity. Count running receivers directly. An owner Durable Object count does not establish a receiver count. Keep credentials out of evidence.
3. Record the production Worker account, name, active version IDs and traffic percentages, namespace IDs, routes, bindings, and migration state. The repository's public profile targets Worker `everett`, account `17bafde311907d7c3d81a3fcbfd31990`, and the `everett.raitox.tech` and `relay.raitox.tech` domains. Confirm those targets at rollout time. Do not use the default development profile.
4. Compare the deployed source and configuration with the candidate. Preserve any later runtime fixes. Stop if the production state cannot be reconciled or a Durable Object migration is pending. This polling patch changes no Worker source, bindings, migrations, keys, or stored data.
5. Retain the prior Worker version and each host's previous compatible binary and service command. Preserve the existing Everett home, encrypted staging, receipts, and outbox. Record a baseline UTC measurement window before changing anything.

From `cloudflare/`, these read-only commands record deployment information with the pinned project Wrangler:

```sh
npm exec -- wrangler deployments list --config wrangler.public.jsonc --json
npm exec -- wrangler versions list --config wrangler.public.jsonc
```

## Verify the exact artifacts

Run these checks from the repository root. Use isolated fixture homes, not real accounts. Keep command logs and artifact hashes with the reviewed SHA.

```sh
cargo test --locked --manifest-path everett-rs/Cargo.toml
cargo clippy --locked --manifest-path everett-rs/Cargo.toml --all-targets -- -D warnings
cargo build --release --locked --manifest-path everett-rs/Cargo.toml
EVERETT_VERIFY_BINARY="$PWD/everett-rs/target/release/everett" cargo test --locked --manifest-path everett-rs/Cargo.toml --test release_journey
npm --prefix cloudflare ci
npm --prefix cloudflare test
npm --prefix cloudflare run typecheck
npm --prefix cloudflare run build:public
```

Verify macOS and Linux before release. The public build script uses `wrangler deploy --dry-run`; its success validates packaging, not a deployed service. A tag push triggers publishing in the Rust workflow, so do not create or push a tag during preparation.

## Deploy the Worker, then canary one receiver

1. After deployment authorization, deploy the reviewed Worker with the public config. Record its new version ID and verify the active deployment. Confirm the existing domains, native sign-in, hosted connector, and an already enrolled receiver still work before updating receiver binaries.
2. Measure a Worker-only window with the old receiver inventory unchanged. The modeled old-receiver idle cycle drops from six to four Durable Object requests. If #126 is already active in the Worker, skip the redundant deployment and record the existing version.
3. After receiver-update authorization, stop exactly one selected receiver through its existing supervisor. Replace only its executable with the reviewed artifact and restart the same service with the same home and identity. Confirm one receiver process and its capability lease. Do not log in again, rotate keys, or alter grants to perform an upgrade.
4. Verify a request and correlated reply between the canary and an existing peer. Observe recipient-visible inbox content and the sender's signed status. Duplicate delivery must not create duplicate inbox entries. Record message IDs and timing without message bodies or bearer credentials.
5. Leave the canary idle for at least five minutes. Confirm remote poll intervals reach 60 seconds. Send a message just after an idle poll and verify pickup within 70 seconds on a healthy connection. Send a short burst and confirm the following polls return to about five seconds. Post a newly created request from the canary CLI while idle and confirm the receiver polls promptly for its reply during the one-minute window measured from envelope creation. Retrying an older envelope does not restart that window. Network and processing delays can exceed those bounds; investigate before expanding.
6. Verify local outgoing and claimed receipts continue to progress within the existing five-second maintenance cadence. Exercise connection loss and recovery on a disposable test receiver. Confirm encrypted staging survives and no duplicate delivery occurs. Use disposable credentials for live revocation tests; do not revoke the owner's working devices. A revoked operation must fail before publication, even while an older response or ciphertext is cached. With a saved webhook retry due during an idle poll gap, verify that logout, provider or credential replacement, and backend revocation stop dispatch. A transient authorization failure must leave its attempt counter and cooldown unchanged.
7. Observe the canary for at least an hour. Expand one receiver at a time only if delivery, retry, authentication, and error rates remain healthy. Record the final running binary hashes and receiver count. Confirm a restart still runs the intended executable.

The authorized deployment command, run from `cloudflare/`, is:

```sh
npm exec -- wrangler deploy --config wrangler.public.jsonc
```

No command in this preparation has performed that deployment.

## Measure the actual request reduction

1. Resolve namespace IDs using `GET /accounts/{account_id}/workers/durable_objects/namespaces`. Record the `OwnerRelay` and `HostedAuth` namespaces in production and development, plus any other account traffic.
2. Query `durableObjectsInvocationsAdaptiveGroups` for explicit UTC windows and `sum.requests`, grouped by namespace. Use GraphQL introspection to confirm the current filter and dimension names. Query `workersInvocationsAdaptive` separately. Retain the query, variables, complete results, and any sampling metadata. Check GraphQL errors and pagination before calculating totals.
3. Compare equal-duration windows for baseline, Worker-only, one canary, and all updated receivers. Record receiver count, active-message volume, failures, and downtime alongside each window. Wait for analytics to populate and repeat after the next complete UTC day. Do not count outage time as a saving.
4. Calculate `requests / window_seconds * 86400` as the daily run rate. Compare steady idle periods with the modeled 2,880 Durable Object requests per updated receiver per day. Account totals also include messages, authentication, alarms, metadata, retries, and development traffic. Fewer requests do not establish equivalent savings in duration or storage meters.
5. Compare the complete UTC day with the account's request allowance and the separate billing view. Use 80,000 requests/day as a rollout headroom target under the existing 100,000 free allowance. Investigate residual amplification if the idle slope remains above the model. Do not claim a bill reduction from adaptive analytics alone.

Use this record for each stage:

| Stage | UTC start/end | Worker version | Receiver hashes/count | DO requests by namespace | Worker requests | Traffic/errors | Daily run rate |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | Pending | Pending | Pending | Pending | Pending | Pending | Pending |
| Worker #126 | Pending | Pending | Pending | Pending | Pending | Pending | Pending |
| One adaptive receiver | Pending | Pending | Pending | Pending | Pending | Pending | Pending |
| All adaptive receivers | Pending | Pending | Pending | Pending | Pending | Pending | Pending |

## Roll back a failed stage

1. Stop expansion for duplicate or missing delivery, unauthorized publication, unexplained authentication failures, failed restart, or unhealthy retries. Roll back the receiver canary first if the failure began with its update.
2. Stop only the affected receiver, restore its recorded previous executable, and restart its existing service with the same home and device identity. Preserve staged ciphertext and outbox records. Verify a round trip and signed status again. Expect the older polling request rate to return.
3. If the failure began with the Worker update, restore the recorded previous Worker version. Confirm that no binding or Durable Object lifecycle change blocks rollback. Check the active deployment and existing connectors after the rollback.
4. Record the incident, version IDs, binary hashes, delivery result, and request-rate impact. Worker rollback changes code; it does not restore Durable Object data, deleted resources, or revoked credentials.

From `cloudflare/`, after rollback authorization and substituting the recorded version ID:

```sh
npm exec -- wrangler rollback PREVIOUS_VERSION_ID --config wrangler.public.jsonc
```

Command and rollback behavior are described in Cloudflare's [Wrangler reference](https://developers.cloudflare.com/workers/wrangler/commands/workers/) and [rollback limits](https://developers.cloudflare.com/workers/versions-and-deployments/rollbacks/). Measurement uses the [Durable Object metrics API](https://developers.cloudflare.com/durable-objects/observability/metrics-and-analytics/).
