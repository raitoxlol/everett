# Test Mac-to-NUC messaging on the public issuer

Use the existing Cloudflare device credentials on the Mac and NUC. This procedure does not sign in, enroll a device, or revoke a device. The driver captures only JSON metadata returned by the Everett CLI. It does not open credential files or print command output that could contain private data.

The driver stages a unique message, retries it with the same idempotency key while the NUC listener is stopped, and checks that the NUC reads one copy and sends one reply after reconnect. Device revocation remains a separate owner action that needs explicit approval in the browser.

## Check the two existing devices

Use an Everett 2.0 or newer binary on both computers. You can point the driver at a v2 binary with `--everett /absolute/path/to/everett-v2` and `--remote-everett /absolute/path/to/everett-v2`. This avoids replacing the installed binary. Confirm that each device already uses native Cloudflare sign-in whose identity issuer is `https://everett.raitox.tech` (the relay URL may be `https://relay.raitox.tech`). Keep the Mac and NUC on the same Everett account.

From the repository root, inspect the plan. This command does not run Everett or make a network request:

```sh
python3 scripts/accept_cloudflare_public.py plan
```

Run the read-only account check with the NUC's SSH alias (here `nuc`):

```sh
python3 scripts/accept_cloudflare_public.py preflight \
  --ssh-host nuc \
  --everett /absolute/path/to/everett-v2 \
  --remote-everett /absolute/path/to/everett-v2
```

The driver checks the issuer, account match, and distinct device identities. It does not display credentials.

## Add temporary peer pins

Choose a test run ID with 12 lowercase hexadecimal characters, or let the driver generate one. The `prepare` command checks both existing identities and verifies the generated alias is unused on both devices before registering their current public relay keys and adding one peer pin on each device. It rejects repeated device IDs or fingerprints. After both pins are added, it saves a mode-restricted local ownership record containing only the run ID, SSH alias, device IDs, and public fingerprints. It never calls `login`.

```sh
python3 scripts/accept_cloudflare_public.py prepare \
  --ssh-host nuc \
  --everett /absolute/path/to/everett-v2 \
  --remote-everett /absolute/path/to/everett-v2
```

Type the NUC SSH alias when prompted. Record the run ID printed by the command. It is a test label, not a credential.

## Queue one message while the NUC is offline

Stop the NUC's `everett relay` listener for the test. Do not stop an unrelated service. Run `queue` with the recorded run ID. Before sending, the driver checks the ownership record and confirms both aliases still pin the recorded device IDs and fingerprints:

```sh
python3 scripts/accept_cloudflare_public.py queue \
  --ssh-host nuc \
  --run-id 012345abcdef \
  --everett /absolute/path/to/everett-v2 \
  --remote-everett /absolute/path/to/everett-v2
```

Type `QUEUE 012345abcdef` when prompted. The driver sends the same test message twice with one idempotency key. It passes this stage only if both sends return the same message ID and the server reports `queued`.

## Reconnect and reply

Start `everett relay` on the Mac and NUC in terminals you control. Leave those processes running while the driver checks delivery. Run:

```sh
python3 scripts/accept_cloudflare_public.py deliver \
  --ssh-host nuc \
  --run-id 012345abcdef \
  --everett /absolute/path/to/everett-v2 \
  --remote-everett /absolute/path/to/everett-v2
```

Type `DELIVER 012345abcdef` when prompted. The driver waits for one matching NUC inbox message, marks it read, checks that no duplicate remains, sends a reply, and waits for one reply in the Mac's run-specific inbox and a signed `replied` state.

## Remove temporary peer pins

After saving the test output, remove only the peer pins created for this run:

```sh
python3 scripts/accept_cloudflare_public.py cleanup \
  --ssh-host nucbox \
  --run-id 012345abcdef \
  --everett /absolute/path/to/everett-v2 \
  --remote-everett /absolute/path/to/everett-v2
```

Type `REMOVE public-012345abcdef` when prompted. Cleanup requires the local ownership record and verifies every remaining peer pin against it before removing anything. If either alias points to a different device or fingerprint, it stops without removing a pin. It then removes the ownership record and leaves device credentials, the run-specific inbox, and relay records intact. The inbox and server records expire under the existing retention rules.

## Device revocation remains a separate check

This driver does not revoke a device. Run this check only after explicit approval for a disposable test device. Before opening the owner page, confirm both devices are signed in to the same public issuer and ready for immediate checks. Match the exact NUC device name and full fingerprint shown on the owner page against the intended test device. Revoke only that named device in the browser. Keep the Mac relay running. Confirm that the NUC is denied and the Mac remains signed in and can still use the relay. Do not select a device by a partial name or partial fingerprint.

## Existing local coverage

`scripts/verify-cloudflare-native.py` covers the native client against a local Worker fixture with synthetic GitHub approval. It already checks isolated send, read, reply, reconnect, and revocation. This public driver uses the already approved Mac and NUC credentials and the public issuer. A successful local fixture run does not prove public messaging.
