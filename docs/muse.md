# Muse agent setup

Muse is Meta's consumer VM agent: it runs on its own computer inside Meta's Muse
VM. Everett treats it as an **external inbox agent** — the same connection shape
as an OpenAI dot or Grok Bot, registered explicitly and addressed by id.

Muse is distinct from Muse Code, the `muse` terminal CLI. Muse Code is a local
harness Everett discovers from `~/.local/share/muse`; the Muse VM agent connects
through registration and relay instead.

## 1. Register the agent

On the computer that owns your Everett sessions:

```bash
everett external add --id muse --harness muse-agent --title "Muse VM agent"
```

The registration creates a durable `ext`-style inbox. Registration alone does not
connect anything — the agent still needs a way to reach it.

## 2. Run Everett inside the Muse VM

The Muse VM can run the Everett binary itself, like any other computer:

```bash
# inside the Muse VM
everett login --provider cloudflare
```

Approve the same account from the device-flow URL. See
[account setup](auth.md) and [Cloudflare native relay](cloudflare-native.md) for
the exact flow.

## 3. Pair the two devices over relay

```bash
# on both devices
everett relay register
```

Compare fingerprints, then `peer add-relay`, and start `everett relay` on both.
Device names and same-owner trust select and authenticate the destination; they
do not grant wake permission.

## 4. Bind the registration to the relay peer

```bash
everett external add --id muse --harness muse-agent --replace \
  --device PEER/muse-agent/SESSION
```

Sends to `muse` then ride the encrypted relay and stay queued until the Muse VM's
Everett picks them up — no local inbox file is written for bound registrations.

## 5. Verify

Send one message and confirm pickup before relying on the connection:

```bash
everett send "status check" --to muse --mode inbox
```

`queued` means retained for pickup, `claimed` means inbox pickup, `replied` means
a reply was observed. None proves the Muse agent completed the task. Muse has no
native wake: delivery is inbox or relay-bound only.
