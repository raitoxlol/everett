# Connect two computers through Cloudflare

The native adapter uses the configured Cloudflare issuer and a custom device approval
protocol. Interactive setup offers account-free local agents or explicit sign-in to
`https://everett.raitox.tech`. Existing custom issuer configuration is preserved.
The public service accepts GitHub accounts into separate relay namespaces. For local
agents without an account, use the [account-free setup](auth.md#set-up-local-agents-without-an-account).

The [2.1.0 release notes](https://github.com/raitoxlol/everett/releases/tag/v2.1.0)
record physical public Mac/NUC delivery, real idle Claude wake/reply, second-account
namespace isolation, foreign native pairing/revocation denial and independent revocation.
Hostile browser request replay remains covered by synthetic-provider runtime tests.

## Approve and connect each computer

Run `everett` normally on each new computer and choose sign-in for hosted delivery; no separate login
command is required. For a custom deployment, set `EVERETT_AUTH_PROVIDER=cloudflare`
and `EVERETT_AUTH_ISSUER` to that deployment's HTTPS origin, or the equivalent `[auth]`
provider and issuer settings. These are public configuration values. Do not copy a credential
file, relay keys or an existing device identity to another computer.

1. Install Everett on each computer and run `everett` to begin automatic onboarding.
   Open the printed approval URL on your phone or any browser. Compare the full code and device
   name, then approve the same Everett account. The terminal polls outbound HTTPS and verifies
   the server identity. It needs no incoming port or localhost browser callback.
2. Run `everett relay register` on both computers. Each prints its public device ID and fingerprint.
   Compare fingerprints through a trusted channel. On each computer, run
   `everett peer add-relay other --device OTHER_DEVICE_ID --fingerprint OTHER_FINGERPRINT`.
   Each device has independent Ed25519 and X25519 keys. A changed fingerprint requires explicit removal and repinning.
3. Keep `everett relay` running on both computers. Cloudflare polls every five seconds while idle.
   Temporary failures retain encrypted staging and retry with backoff up to 30 seconds.
   The CLI does not install a background service. Receiver-side Claude wake is disabled
   unless the receiving owner writes the exact local policy described below.
4. Send with `everett send "test request" --to other/codex/EXACT_SESSION_ID --mode inbox --idempotency-key first-test --json`.
   Read on the recipient with `everett inbox --session EXACT_SESSION_ID --json`, then run
   `everett reply LOCAL_MESSAGE_ID "test reply"`. Check on the sender with
   `everett peer status other REMOTE_MESSAGE_ID`. Queued means retained by the relay;
   claimed means the recipient read its inbox; replied requires a signed reply observation.
5. Revoke that computer from the owner approval page, or self-revoke with
   `everett logout --revoke-device --json` after registration. Verify the revoked listener
   stops and the other computer remains signed in. Local logout alone does not revoke
   server authorization. SSH peer pairing remains an optional advanced transport.

The relay only imports into an existing exact session. It stores ciphertext in transit,
with visible routing metadata. Fingerprint checks, signed observations, idempotency,
private durable staging and the three-hop bound are shared with the existing relay.
Cloudflare keys, pins and retry records live under `.everett/relay-cloudflare` beneath
`EVERETT_HOME` or `HOME`. Existing OIDC/Convex state is separate. Selecting an unsupported
registry or cross-owner operation returns an error rather than using a different account.

## Optional receiver-side Claude wake

Wake is off by default. To allow a narrowly scoped Mac-to-NUC wake, create
`$EVERETT_HOME/.everett/relay-cloudflare/receiver-wake.json` on the receiving NUC
(use `.everett/relay/receiver-wake.json` for Convex). Keep `HOME` and
`CLAUDE_CONFIG_DIR` pointed at the normal Claude account. The policy is local to that
Everett profile and must name one canonical project directory, one full Claude session
UUID, and one same-owner paired sender device ID:

Create the file with owner-only permissions (`chmod 600 receiver-wake.json`).

```json
{
  "version": 1,
  "enabled": true,
  "allow": [
    {
      "session_id": "38cab978-79c0-4f42-9aee-5c30d8aa6e4c",
      "cwd": "/home/nuc/Projects/example",
      "sender_device_id": "DEVICE_ID_FROM_PEER_LIST"
    }
  ]
}
```

Use the actual session UUID and cwd reported by `claude agents --json`, and the pinned
same-owner sender device ID. Wildcards, cross-owner grants, external content, T3 sessions,
replies and SSH inboxes cannot enable wake. Import and durable inbox write happen first.
An active, missing or blocked Claude session remains deferred until message expiry.
After an attach may have sent the nudge, restart does not retry that import automatically.
Prompt receipt still does not prove a model reply. The Mac-to-NUC acceptance boundary is
the correlated reply arriving in the Mac sender session's inbox; the physical acceptance
commands remain a separate device check.

`receiver-wake.json` applies to one Claude session. It does not wake a registered
Grok Bot or OpenAI dot. Those inboxes use `receiver-webhook.json` in this same
relay directory. See [Grok Bot webhook wake](grok-bot.md#wake-a-grok-bot-with-a-webhook-routine).

### Real public-relay receiver acceptance

`scripts/accept_receiver_wake.py` runs the actual Mac-to-NUC acceptance journey against
the public Cloudflare relay. It expects the already released Mac CLI and existing isolated
Mac public profile, the candidate NUC CLI and its isolated public profile, pre-enrolled
same-owner device identities with reciprocal exact fingerprint pins, and two real idle
Claude background sessions on the NUC: one allowed session and one separate session for
the deny-path probe. The caller supplies the full UUIDs, canonical cwd, pinned IDs and
fingerprints, and an existing SSH alias. The script does not enroll devices, install
binaries, or read/copy credentials. A blocked, missing, busy, or unobservable Claude
session is a preflight failure before policy installation or listener startup.
Pass the exact existing Claude CLI path; the runner uses it for `claude agents --json`,
records its resolved path without querying a provider, and prepends its directory to the
owned NUC relay listener's `PATH` while retaining the system path.

Run from the repository on the Mac with a new path under an owner-only directory:

```sh
python3 scripts/accept_receiver_wake.py \
  --nuc-ssh nucbox \
  --mac-binary /ABS/PATH/TO/released-mac-everett \
  --nuc-binary /ABS/PATH/TO/candidate-nuc-everett \
  --nuc-claude-binary /home/nuc/.local/bin/claude \
  --nuc-sha256 EXPECTED_CANDIDATE_SHA256 \
  --source-commit FULL_CANDIDATE_SOURCE_COMMIT \
  --mac-device-id MAC_DEVICE_ID --mac-fingerprint MAC_FINGERPRINT \
  --nuc-device-id NUC_DEVICE_ID --nuc-fingerprint NUC_FINGERPRINT \
  --session-id ALLOWED_CLAUDE_UUID --denied-session-id DENY_PROBE_CLAUDE_UUID \
  --cwd /home/nuc/Projects/example \
  --manifest /path/in/owner-only-directory/receiver-wake-run.json
```

The runner checks profile ownership, both live Claude fixtures, exact peer pins and the
disabled default policy. It then installs one exact temporary rule, starts only its own
relay listeners, sends one unique public inbox request without sender-side wake, retries
the same idempotency key before and after an owned NUC listener restart, and verifies one
unchanged receiver job plus the exact real Claude reply in the Mac `human` inbox. A
separate unique request targets the real deny-probe session and must reach the NUC inbox
without creating a wake job, changing that session, or returning a reply. This second
request is required to prove denial; a missing deny fixture cannot pass acceptance.

The owner-only manifest records phases, exact IDs, message correlation, listener PIDs,
cleanup outcomes, the declared candidate source commit, and SHA-256 hashes of both binaries.
It also compares the NUC project tree before and after the probes and fails if tracked or
untracked project state changes. The candidate NUC hash must match the explicit
`--nuc-sha256` value before any mutation.
Pass explicit binary paths because
candidate build directories can change even when their names do not. The runner stops only
listeners it started and restores the policy only while its installed file identity and
bytes remain unchanged. SIGINT, SIGTERM, and Ctrl-C run cleanup and cannot produce `passed`.
Positive/negative relay checks alone leave the run `incomplete`: full v2 readiness also
requires independently verified evidence that an existing draft in the native Claude
composer survives the wake journey. This runner does not synthesize that evidence or accept
a boolean assertion in its place. A failed or uncertain cleanup also returns `incomplete`;
inspect the manifest before manual recovery. Offline coverage is
`python3 -m unittest -v scripts.test_accept_receiver_wake`; that suite does not exercise
the public relay or replace the real-device acceptance result.

## Reproduce isolated local acceptance

The backend PR provides `cloudflare/test/native-fixture.mjs`. Install its pinned dependencies
and start `node /ABS/BACKEND/cloudflare/test/native-fixture.mjs serve 18791` on a free loopback port.
The fixture runs the production Worker, HostedAuth and OwnerRelay. Only outbound GitHub is
synthetic. It denies other outbound calls, keeps temporary state and adds no public test routes.

Build the debug Rust binary, then run:

```sh
python3 scripts/verify-cloudflare-native.py \
  --binary /ABS/everett/everett-rs/target/debug/everett \
  --fixture /ABS/BACKEND/cloudflare/test/native-fixture.mjs \
  --node /ABS/node \
  --issuer http://127.0.0.1:18791
```

The harness creates and removes isolated temporary homes. It checks separate approvals,
signed registration and pins, an offline encrypted send, idempotent retry, durable inbox
pickup, signed claimed/replied status, encrypted reply, independent owner revocation and
preservation of old OIDC files. It never reads real account files. Stop the local fixture
with Ctrl-C. This harness proves the local Worker/native contract on one machine.
The separately verified physical Mac/NUC journey used independent temporary identities,
interrupted the private fixture connection, verified queued delivery after reconnect and
revoked the NUC while the Mac remained active. Real GitHub and public-issuer delivery
still need separate acceptance.
