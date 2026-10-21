# Connect two computers through Cloudflare

The development native adapter uses the configured Cloudflare issuer and the existing
custom device approval protocol. There is no hosted default. Real hosted GitHub approval
and physical cross-device acceptance remain pending. Local acceptance uses the production
Worker and native CLI, with only outbound GitHub mocked.

## Approve and connect each computer

Use your configured Everett deployment on both computers. Set `EVERETT_AUTH_PROVIDER=cloudflare`
and `EVERETT_AUTH_ISSUER` to that deployment's HTTPS origin, or the equivalent `[auth]`
provider and issuer settings. These are public configuration values. Do not copy a credential
file, relay keys or an existing device identity to another computer.

1. Install Everett on each computer. On the other computer, run `everett login --device second-computer`.
   Open the printed approval URL on your phone or any browser. Compare the full code and device
   name, then approve the same Everett account. The terminal polls outbound HTTPS and verifies
   the server identity. It needs no incoming port or localhost browser callback.
2. Run `everett relay register` on both computers. Each prints its public device ID and fingerprint.
   Compare fingerprints through a trusted channel. On each computer, run
   `everett peer add-relay other --device OTHER_DEVICE_ID --fingerprint OTHER_FINGERPRINT`.
   Each device has independent Ed25519 and X25519 keys. A changed fingerprint requires explicit removal and repinning.
3. Keep `everett relay` running on both computers. Cloudflare polls every five seconds while idle.
   Temporary failures retain encrypted staging and retry with backoff up to 30 seconds.
   The CLI does not install a background service or wake agent sessions.
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
with Ctrl-C. This proves the local Worker/native contract. Real GitHub, public-network
and physical-device journeys need separate acceptance.
