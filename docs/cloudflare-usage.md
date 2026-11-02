# Cloudflare relay request budget

The free Durable Objects request allowance is 100,000 requests per account per UTC day. Each method called on a Durable Object namespace stub starts a billable RPC session. Worker requests, Durable Object requests, SQL rows and compute duration are separate meters.

## Investigation, 2026-11-02

Cloudflare's notification identified Durable Object **requests**, not storage, as the exhausted meter. The November 1 notification reported exhaustion at 15:12 UTC. The GraphQL adaptive analytics dataset for November 1 UTC returned 70,468 requests for `OwnerRelay` and 37,204 for `HostedAuth`, totaling 107,672. Worker analytics returned 45,249 invocations. These are analytics counts, not an exact billing reconciliation or endpoint-level attribution. Invocation success does not establish HTTP success.

The receiver's idle loop previously registered the same device before every pending-inbox poll. Both HTTP requests called `HostedAuth.verifyAgent`, which called `OwnerRelay.credentialActive`, followed by the actual `OwnerRelay.relay` operation. Each operation already checks current membership and credential state inside its owning object. The loop therefore spent six Durable Object requests every five seconds even with no messages.

The receiver now registers until the first successful response, then polls without repeating registration. Transient registration failures retry before polling; a restarted receiver registers again. Durable registration survives server eviction and network interruption. Native key rotation already requires stopping the receiver through its daemon lock.

`HostedAuth.authenticateRelayToken` validates the native token, expiry and account binding. It does not authorize an operation. `OwnerRelay.relay` authorizes every operation; metadata operations authorize through `activeCredentialIds` before reading or changing labels. Hosted credential/membership denials still return HTTP 401 with `invalid_token`. No token or authorization cache is introduced.

## Steady idle budget

These are calculated ceilings assuming one cycle every five seconds, excluding HTTP latency, startup, retries, message traffic, login and alarms. They are not measured post-deployment savings.

| Per receiver | Before | After both changes |
| --- | ---: | ---: |
| HTTP requests per cycle | 2 | 1 |
| Durable Object requests per cycle | 6 | 2 |
| HTTP requests per day | 34,560 | 17,280 |
| Durable Object requests per day | 103,680 | 34,560 |

That is a 66.7% reduction in steady idle Durable Object calls. Two receivers use approximately 69,120/day before other traffic; three still exceed the free allowance. Deploying only the Worker reduces the old receiver's cycle from six calls to four. Updating only the receiver reduces it from six to three. Both are required for the two-call cycle. Neither change was deployed during this investigation.

## Verification

The actual CLI against an isolated HTTP fixture produced `3 registrations, 3 polls` before the fix and `1 registrations, 3 polls` afterward, including a temporary pending failure. A separate test covers failed registration and process restart. Existing checks cover provider/credential replacement and authoritative revocation.

The production Worker under workerd made three redundant `credentialActive` calls for three requests before the fix and zero afterward. Regression tests independently revoke credential and membership rows while the native token remains valid; relay and metadata requests must immediately fail. The full Worker suite also exercises signed registration, encrypted delivery/reply, independent revocation, OAuth and public-account isolation.

```sh
cargo test --locked --manifest-path everett-rs/Cargo.toml --test cloudflare_relay --test relay_native --test release_journey
npm --prefix cloudflare test
npm --prefix cloudflare run typecheck
npm --prefix cloudflare run build:public
```

For live analytics, query `durableObjectsInvocationsAdaptiveGroups` by UTC date and namespace ID, and resolve IDs through `GET /accounts/{account_id}/workers/durable_objects/namespaces`. Query `workersInvocationsAdaptive` separately. Observability is disabled in the public profile; this investigation did not enable request-body logging or read bearer credentials.

Further cuts need separate evidence: terminal receipt reconciliation can retry remote denials, and more receivers increase the fixed polling baseline. Push delivery could reduce idle traffic further, but requires reconnect, authorization and missed-message recovery tests. Slower polling changes message latency. Skipping per-operation authorization would weaken revocation and is not an acceptable saving.

Sources: [Durable Objects pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/), [metrics and analytics](https://developers.cloudflare.com/durable-objects/observability/metrics-and-analytics/).
