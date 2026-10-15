# Local login verification, 2026-10-15

## Environment

The run used macOS on Apple Silicon, Node v24.16.0, and the existing `/opt/homebrew/bin/everett`, which reported `everett 1.6.0`. The binary was never rebuilt or replaced. The Rust CLI and `convex/` functions were not modified.

The issuer used `oidc-provider` 9.12.2. Convex used the repo's CLI 1.46.0 and native backend `precompiled-2026-10-06-a3538c6`. All 40 issuer packages in the lockfile were checked against npm publication timestamps and predate 2026-10-08.

The initial anonymous `convex dev` experiment succeeded without login. Its default backend listened on `0.0.0.0`; the delivered scripts instead start the same native backend with `--interface 127.0.0.1`. `lsof` confirmed loopback listeners on 3210, 3211, 4444, and 4445. Convex's local self-hosted operator mode required no account or paid service.

## Passed

- `up.sh` started the issuer and backend, set both auth environment variables, pushed the unchanged functions with typechecking enabled, and provisioned both exact identities. Repeated startup/provisioning succeeded. A second concurrent `up.sh` refused the live PID without stopping the first stack.
- The installed binary completed both device-code logins, both `whoami --json` commands, and both native refresh paths. The smoke sets only the stored expiry timestamp to zero to trigger refresh. Convex validated the resulting signed ID tokens and returned the expected per-agent IDs, common org namespace, and distinct admin/member roles. Both altered-signature tokens were rejected. Both credential files had mode 0600.
- Browser approval was executed in T3's collaborative browser. The provider's complete verification URL prefilled the code, clicking **Approve** returned HTTP 200, and the real waiting binary completed sign-in as `alice-a` with device display name `browser`. This used a separate isolated browser-test home.
- Root `npm run typecheck` and `npm test` passed. Vitest reported **2 files, 68 tests passed**. Every new `.mjs` passed `node --check`, and every shell entrypoint passed `bash -n`. Rust tests and a source rebuild were not run because the production code is unchanged and the requested verification target is the installed binary.
- `down.sh` stopped the stack. A direct bind check confirmed that all four ports were free, PID/readiness files were removed, and the database and private device credentials remained. No CA was installed, system trust changed, cloud deployment created, or real harness started.

## Native 1.6.0 blockers

### The requested boolean spelling is invalid

Exact command:

```sh
/opt/homebrew/bin/everett login --device alice-a --open=false
```

Exit code 2, exact stderr:

```text
error: unexpected value 'false' for '--open' found; no more were expected

Usage: everett login --open

For more information, try '--help'.
```

`--open` is a switch, as documented in `docs/auth.md`. The smoke records the failed requested spelling, then continues with the documented non-browser command `login --device alice-a --force --json`. This is an input mismatch, not a device-flow failure.

### HTTP relay URLs are rejected; private TLS roots cannot be supplied

With Alice A signed in, this exact command exits 7:

```sh
EVERETT_CONVEX_URL=http://127.0.0.1:3210 /opt/homebrew/bin/everett relay whoami
```

Exact stderr:

```text
everett: relay: Convex deployment must be an HTTPS origin
```

With `EVERETT_CONVEX_URL=https://127.0.0.1:4445`, all six real `relay whoami`, `relay register`, and `relay devices` calls, across A and B, exit 5 after about 20 seconds. Exact stderr:

```text
everett: Convex request timed out; delivery is unconfirmed. Retry with the same idempotency key.
```

The TLS proxy recorded the client's actual handshake rejection in `.state/tls-errors.log`:

```text
2026-10-15T09:24:10.324Z 805E10F401000000:error:0A000416:SSL routines:ssl3_read_bytes:ssl/tls alert certificate unknown:ssl/record/rec_layer_s3.c:918:SSL alert number 46
```

A Node HTTPS request with explicit CA trust returned HTTP 200 for the same proxy's `/version`. Real authenticated HTTP backend queries also succeeded. This isolates the native failure to certificate trust, rather than provisioning or an absent deployment. `everett-rs/Cargo.toml` selects `rustls-tls-webpki-roots` for the Convex client, and `src/relay/mod.rs` requires HTTPS without exposing custom trust configuration. Keychain trust or a generated `mkcert` root cannot change that compiled trust source.

The product limitation is the lack of a supported loopback HTTP or custom-CA option for local relay testing. The diagnostic bug is that certificate rejection becomes a generic delivery timeout even for `relay whoami`. No CLI or backend patch was made. A public-trusted certificate or hosted service would exceed this task's local-only setup, so neither was used.

## Real smoke output

Final command `./scripts/dev-login/smoke.sh` exited **1**. The run directory is `.state/runs/2026-10-15T09-23-10-933Z/`. Output tail:

```text
PASS alice-b backend diagnostic: verified JWT/claims/enrollment, refresh ID token, tamper rejection, credentials mode 0600
PASS alice-a backend diagnostic: verified JWT/claims/enrollment, refresh ID token, tamper rejection, credentials mode 0600
FAIL alice-a relay whoami
  everett: Convex request timed out; delivery is unconfirmed. Retry with the same idempotency key.
FAIL alice-b relay whoami
  everett: Convex request timed out; delivery is unconfirmed. Retry with the same idempotency key.
FAIL alice-a relay register
  everett: Convex request timed out; delivery is unconfirmed. Retry with the same idempotency key.
FAIL alice-b relay register
  everett: Convex request timed out; delivery is unconfirmed. Retry with the same idempotency key.
FAIL alice-b relay devices
  everett: Convex request timed out; delivery is unconfirmed. Retry with the same idempotency key.
FAIL alice-a relay devices
  everett: Convex request timed out; delivery is unconfirmed. Retry with the same idempotency key.
PASS HTTP rejection reproduced: everett: relay: Convex deployment must be an HTTPS origin
PASS TLS proxy /version with explicit Node CA trust (diagnostic only; Everett trust is unchanged)
SKIP message exchange: installed binary could not complete relay identity/registration/devices
SUMMARY FAIL: 7 failed checks; real output in /Users/wrightwilliams/.t3/worktrees/everett/t3-dcb3322b/scripts/dev-login/.state/runs/2026-10-15T09-23-10-933Z
```

The count is one rejected requested flag spelling plus six native relay commands. A fully passing native relay smoke and the A-to-B encrypted exchange were **not achieved**. The exchange branch is implemented following `docs/cross-device.md` and explicitly skipped when native setup fails; it remains unverified against a live native relay. Real NAT, sshd, active-subscription revocation, and real model sessions were not exercised.

An earlier development run hit a full-disk error, including `can't create temp file for here document: no space left on device` and a backend SQLite `unable to open database file` shutdown. Only this task's duplicate downloaded Convex cache was removed. The final run used the healthy restarted backend, and `up.sh` now checks for at least 500 MB free before startup. This was an environment failure, not a CLI auth bug.
