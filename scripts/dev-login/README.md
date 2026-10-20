# Test Everett login and encrypted relay locally

Run an Everett binary built from this branch against a local OIDC issuer and a local Convex backend. No Docker, Java, paid account, cloud deployment, or system CA installation is needed. The old installed 1.6.0 binary lacks the new CA option; its historical failures are retained in [RESULTS.md](RESULTS.md).

The stack generates a private development CA and a separate localhost server certificate under `.state/`. Both isolated devices explicitly trust that CA through `EVERETT_RELAY_CA_FILE` / `[relay] ca_file`. Default public WebPKI roots, certificate verification, and HTTP rejection remain unchanged. The measured smoke passes, including encrypted A → B → A delivery and signed `replied` status.

## Start the stack

Run these commands from the Everett checkout root. Use Rust, Node 24 or later, npm, `unzip`, and `openssl`. macOS already provides `unzip` and `openssl`; Homebrew OpenSSL also works. The pinned Linux backend requires glibc 2.39 and recent libstdc++ (Ubuntu 24.04 or equivalent); Ubuntu 22.04 needs an isolated runtime workaround as noted in RESULTS.md. Allow space for Rust build artifacts as well as the backend.

```sh
cargo build --manifest-path everett-rs/Cargo.toml --locked
./scripts/dev-login/up.sh
```

Leave this terminal open. Wait for `PASS both operator identities provisioned`. Every start runs `npm ci` for both lockfiles. npm can reuse its download cache, and later starts reuse the pinned native Convex binary from GitHub. All auth, provisioning, database, and messaging requests stay on loopback. Backend beacons and Sentry are disabled.

In another terminal at the checkout root, run the real binary smoke. Allow about 90 seconds once the stack is ready.

```sh
EVERETT_VERIFY_BINARY="$PWD/everett-rs/target/debug/everett" ./scripts/dev-login/smoke.sh
```

Read the summary even when the command exits 1. Full command output is private under `scripts/dev-login/.state/runs/`. The latest summary is `scripts/dev-login/.state/last-smoke.txt`. No tokens are printed. Run the smoke only while the stack is running.

## Owner-only hosted smoke

The stack starts in generic OIDC mode: `convex/auth.config.ts` no longer reads `EVERETT_OWNER_GITHUB_ID`, so `convex dev` and `convex deploy` succeed with it unset, and hosted requests fail closed until it is set. With the stack running (run `smoke.sh` first so the generic OIDC caller has a fresh token), the hosted smoke switches the local backend to owner mode (GitHub ID `424242`, login `dev-owner`, a local Convex Auth signing key), runs its checks, then removes `EVERETT_OWNER_GITHUB_ID` again.

```sh
EVERETT_VERIFY_BINARY="$PWD/everett-rs/target/debug/everett" node scripts/dev-login/hosted-smoke.mjs
```

It mints real Convex Auth sessions for the owner and a non-owner GitHub account (only GitHub's OAuth exchange is replaced, via `auth:store` as Convex Auth records it), enrolls two agents through real `everett login`, checks that non-owner, agent, generic OIDC and anonymous callers cannot approve, inspect, list or revoke, revokes one agent and checks its still-valid cached JWT fails on the next relay call while the other keeps working, and scans every log and transcript for the tokens, JWTs, refresh tokens and signing key it saw. Output goes to `.state/hosted-runs/`; the summary is `.state/last-hosted-smoke.txt`.

## Log in interactively

For device A, run the following in a terminal at the checkout root.

```sh
source ./scripts/dev-login/.state/alice-a.env
./everett-rs/target/debug/everett login --device alice-a --force --open
./everett-rs/target/debug/everett whoami --json
./everett-rs/target/debug/everett relay whoami
./everett-rs/target/debug/everett relay register
./everett-rs/target/debug/everett relay devices
```

On the browser page, enter the code printed in the terminal, choose **alice-a**, and click **Approve**. `whoami` must show `sub` and `device_name` equal to `alice-a`.

For device B, open another terminal at the checkout root.

```sh
source ./scripts/dev-login/.state/alice-b.env
./everett-rs/target/debug/everett login --device alice-b --force --open
./everett-rs/target/debug/everett whoami --json
./everett-rs/target/debug/everett relay whoami
./everett-rs/target/debug/everett relay register
./everett-rs/target/debug/everett relay devices
```

Approve as **alice-b**. The two identities share org `local-alice`, with A as `admin` and B as `member`. Their subjects and agent IDs differ. `--device` sets a display name; the issuer's fixed account registry supplies the trusted claims.

To avoid opening a browser automatically, omit `--open` or use `--open=false` and visit the printed URL yourself. This branch accepts both `--open` and `--open=true` to open it; bare `--open` keeps its old behavior. The smoke uses the explicit `--open=false` spelling.

For an automated approval by hand, leave `login` waiting in one terminal. In another terminal, substitute its printed user code and the intended identity.

```sh
curl --fail --silent --show-error http://127.0.0.1:4444/approve \
  --data-urlencode 'user_code=CODE-FROM-TERMINAL' \
  --data-urlencode 'account=alice-a'
```

The approval endpoint is intentionally unauthenticated for this local development issuer. It accepts only `alice-a` or `alice-b`, never client-supplied org, role, or agent claims. Do not expose it beyond loopback or use real accounts with it.

## Stop and restart

Press Ctrl-C in the stack terminal, or run the following from the checkout root.

```sh
./scripts/dev-login/down.sh
```

Shutdown stops only this stack's child processes. It preserves keys, homes, provisioning, database records, and logs in the gitignored `.state/` directory. `up.sh` refuses occupied ports and a running stack instead of killing unrelated processes. An issuer restart loses its in-memory grants and refresh tokens; log in again with `--force`. Existing relay key files remain distinct per device.

For dependency updates, edit the exact version and regenerate `package-lock.json` deliberately. `up.sh` installs the lockfiles with `npm ci`; it does not upgrade packages. `oidc-provider` 9.12.2 was published on 2026-09-05. The native backend is release `precompiled-2026-10-06-a3538c6`; archive SHA-256 values are pinned in `stack.mjs`. The repo's existing Convex CLI dependency is 1.46.0.

## Local services and storage

All services bind **127.0.0.1**. The issuer uses HTTP, which both Everett login and this local Convex backend accept.

| Port | Service |
| --- | --- |
| 4444 | OIDC discovery, JWKS, RFC 8628 authorization, token refresh, userinfo, revocation, and device approval |
| 3210 | Native Convex backend HTTP API and WebSocket sync |
| 3211 | Convex HTTP actions |
| 4445 | HTTPS and WebSocket proxy with a localhost certificate signed by the generated local CA |

The supervisor replaces only `.state/operator/convex/` with the current `convex/` functions, sets `OIDC_ISSUER` and `OIDC_CLIENT_ID` with `convex env set`, pushes them with `convex dev --once`, and calls the internal operator-only `relay:provisionIdentity` for both fixed accounts. It uses the pinned repo CLI through its installed Node entrypoint, equivalent to `npx --no-install convex` in that directory. Typechecking is enabled. Operator credentials stay in mode-0600 `.state/operator/.env.local`; they are never included in device environments.

The anonymous `convex dev` path was tested and does not need cloud login. Its launcher binds `0.0.0.0`, so the delivered supervisor runs that native backend directly with `--interface 127.0.0.1` and targets it through Convex's supported self-hosted environment variables. There is no hosted service involved. Do not run a separate anonymous launcher on these ports. If a future CLI unexpectedly requests cloud authentication, startup stops and points to `.state/convex.log`; do not select a cloud deployment for this loopback issuer.

`EVERETT_HOME` points at a **home directory**, not at `.everett` itself. Each device has `.state/homes/alice-a/.everett/` or `.state/homes/alice-b/.everett/`. This follows `src/session.rs` and the native integration-test isolation. The real macOS home, Keychain, installed binary, and real `~/.everett/` are untouched. The smoke also isolates child `HOME` and clears inherited Everett and harness identity overrides.

`up.sh` prints all non-secret `EVERETT_*` values and writes sourceable `alice-a.env` and `alice-b.env` files. Each isolated home also gets the following `config.toml`.

```toml
[auth]
issuer = "http://127.0.0.1:4444"
client_id = "everett-relay"
[relay]
url = "https://127.0.0.1:4445"
ca_file = "/absolute/checkout/path/scripts/dev-login/.state/ca.pem"
```

The account registry in `settings.mjs` controls `everett_agent`, `everett_org`, and `everett_role`. Signed RS256 ID tokens retain `iss`, `sub`, `aud`, and `exp`, including on refresh. Generated signing keys, CA/TLS private keys, tokens, and operator keys remain in `.state/`, which is ignored by git. Private CA/TLS keys have mode 0600. Startup validates CA/leaf expiry, server names, signatures, and matching keys; it replaces expired, broken, or legacy self-signed TLS files without changing system trust. The issuer's OAuth state is held in memory.

## Smoke coverage and limits

The smoke uses `EVERETT_VERIFY_BINARY` (or `everett` on PATH) and records every command's stdout, stderr, and exit code. Before checking stack or relay readiness, it seeds dated Codex transcripts in both isolated homes and asserts that `ls --json` returns their exact session IDs. It attempts both identities' device login, `whoami --json`, `relay whoami`, `relay register`, and `relay devices`. It approves each printed code over the local HTTP approval endpoint. It also forces the local access-token expiry timestamp to zero, calls the real CLI's `whoami` refresh path, and checks the refreshed signed ID token through Convex. It never replaces or fabricates the binary's tokens.

Independent backend diagnostics check signature/claim validation, authoritative provisioning, role/namespace, rejection of a tampered token, and credential-file permissions. The native client verifies CA trust through both environment and config-file options, then rejects the untrusted local certificate promptly with its certificate error. HTTP still fails. A Node HTTPS request independently verifies the TLS proxy.

After native relay setup, the smoke follows the [two-device journey](../../docs/cross-device.md#convex-relay-two-machine-trial): it reads both public registration fingerprints directly, pins both peers, sends A to B, verifies the queued signed ciphertext has no plaintext before B runs, starts two `everett relay` listeners, peeks and consumes B's inbox, replies using B's local inbox ID, consumes A's reply, and waits for signed `replied` status. It stops only listeners it started. Pending pagination uses a stable recipient range and filters expired rows after pagination, so advancing time cannot invalidate the next cursor.

This stack does not exercise real NAT, real sshd, real harness/model sessions, browser login security, production enrollment, cross-org grants, revocation of active subscriptions, full message recovery after disconnects, or production endpoints. Rust transport tests cover reconnects with the configured CA, default/unrelated-CA rejection, hostname enforcement, and missing/malformed CA files. No system CA is installed and no client TLS checks are disabled; the extra roots apply only to Everett relay clients explicitly configured to use them.

## Run local regressions

With the root npm dependencies installed, run these checks without starting the stack. They use the selected Everett binary, offline npm fixture packages, and disposable local files. The HTTPS check briefly opens an ephemeral loopback port with explicit Node CA trust.

```sh
EVERETT_VERIFY_BINARY="$PWD/everett-rs/target/debug/everett" node --test scripts/dev-login/regression.mjs
```
