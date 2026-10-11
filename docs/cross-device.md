# Cross-device Everett (design)

Status: design for issue #4, the second half. `docs/auth.md` covers sign-in (implemented:
`login` / `whoami` / `logout`). This document covers what sign-in is *for*: seeing the sessions on
your other machines and messaging them. Nothing below is implemented yet.

Target: a Mac and a Linux box, each running its own Claude Code / Codex / OMP sessions. From either
machine, `everett ls` shows both machines' session cards, `everett send` reaches a session on the
other machine, and the reply comes back to the sender's inbox.

## Rules that do not change

- Sessions, inbox messages, cards and shared core stay on the machine that produced them. The
  account backend never stores any of them.
- A queued message is not a read message. Remote delivery reports `queued` (written to the remote
  inbox), `delivered` (the remote session's hook or MCP poll took it) and `replied`. Nothing claims
  more than it observed.
- Adapters stay read-only. Remote reads go through the remote Everett binary, never through the
  remote harness store directly.

## Layers

| Layer | Holds | Where |
|---|---|---|
| Identity | who you are (`sub`, email) | OIDC issuer (`docs/auth.md`) |
| Device registry | per account: device id, name, public key, tailnet/SSH address, created/revoked | Convex (one `devices` table, two functions), self-hostable |
| Transport | authenticated byte stream between two of your devices | OpenSSH over Tailscale (phase 1), optional direct HTTPS later |
| Everett RPC | `ls`, `card`, `send`, `reply` across the link | `everett peer serve` on the remote, `everett peer …` on the caller |

The registry is a directory, not a mailbox. If it is down, already-paired devices keep working
because the SSH host key and address are cached locally.

## Phase 1: SSH over Tailscale, two devices

Why this first: Tailscale gives both machines stable addresses behind NAT (direct when it can,
relayed and still end-to-end encrypted when it cannot) and OpenSSH is already installed, audited and
understood. No listener, TLS or certificate code lands in Everett.

Each device needs a Tailscale client authenticated to the same tailnet using a Tailscale account.
This is separate from `everett login`, so phase 1 does not fulfill issue #4's one-sign-in goal.
Manual pairing does not require an Everett account. A reachable LAN or other VPN can replace
Tailscale, but Everett does not establish that network for you.

Both devices need OpenSSH server access enabled and network rules that allow SSH in both directions.
This design uses OpenSSH over the Tailscale network, not Tailscale SSH, which has a different
authorization model. It assumes sshd reads `~/.ssh/authorized_keys`. If the host uses a different
`AuthorizedKeysFile`, pairing stops with an explanation rather than changing `sshd_config`.
See the [OpenSSH key-file configuration](https://man.openbsd.org/sshd_config.5#AuthorizedKeysFile)
and the [Tailscale account setup](https://tailscale.com/kb/1017/install).

Pairing (`everett peer add`) establishes both directions:

1. Each device generates an Ed25519 keypair under `~/.everett/peer/` at first use. The private key
   has mode 0600 and the directory has mode 0700. Private keys never leave their device.
2. `everett peer add linux-box --ssh wright@linux-box.tailnet.ts.net` prompts for the caller's
   reachable SSH address for the return connection. Before trusting either device, compare its
   SSH host-key and Everett device-key fingerprints with values shown on that device's local
   terminal or another trusted out-of-band channel. An SSH prompt or registry row alone is not
   proof of identity. Pairing pins the confirmed host keys on both devices and rejects mismatches.
3. Through an interactive SSH setup session, pairing installs the caller's public key in the
   remote `~/.ssh/authorized_keys` and the remote public key in the caller's
   `~/.ssh/authorized_keys`. Each entry has a trailing `# everett-peer:<device_id>` marker and a
   forced command using that host's absolute Everett binary path. For example, on Linux:

   ```text
   command="/home/wright/.local/bin/everett peer serve",no-pty,no-port-forwarding,no-agent-forwarding,no-X11-forwarding ssh-ed25519 <base64-public-key> # everett-peer:mac-studio
   ```

   Pairing resolves the installed path separately on each host; it does not rely on a login
   shell's PATH. It backs up existing files, preserves unrelated key entries, and maintains
   directory mode 0700 and file mode 0600. Retrying replaces the matching tagged entry rather than
   adding a duplicate. No key goes into `~/.everett/peer/authorized_keys`; sshd does not read it by
   default. The peer keys permit only Everett RPC, not an unrestricted shell.
4. Pairing checks an RPC connection in each direction before marking the pair complete. A failed
   return connection leaves the pair incomplete, reports the failure, and blocks cross-device
   messaging until pairing succeeds. Retrying completes the same pair without duplicate entries.

Transport: `ssh -i ~/.everett/peer/id_ed25519 <addr>`, using the pinned host key with strict checking
and the forced command above. Each connection carries one newline-delimited JSON request on
stdin/stdout, capped at 64 KiB, with a 20 s timeout. No message text goes in shell arguments.

RPC (served by `everett peer serve`, which refuses anything else):

| Request | Reply | Notes |
|---|---|---|
| `{"op":"ls","hours":72}` | `Session[]` as today, each with `device` set | same look-back and filters as local `ls` |
| `{"op":"card","session":ID}` | `{card, source, updated}` | read-only |
| `{"op":"send","session":ID,"text":…,"from":{device,session,harness,card},"idempotency_key":K,"hops":n}` | `{message_id, state:"queued"}` | writes the remote inbox; duplicate `K` returns the existing id |
| `{"op":"status","message_id":M}` | `{state: queued\|delivered\|replied, reply?}` | `delivered` = present in the remote `done` set |
| `{"op":"reply","message_id":M,"reply_id":R,"text":…}` | `{reply_id}` | callback to the original sender; only accepts the paired device that received `M`; duplicate `R` returns the existing id |

Session IDs gain a device prefix at the boundary only: `linux-box/claude/abc123`. Local code keeps
plain IDs; `send.rs` splits the prefix, routes to the peer if it is not this device, and
`inbox.rs` stores the sender as `mac-studio/claude/def456` so `reply` knows which peer to call back.
`hops` keep counting across devices; the three-hop limit applies end to end.

Each sender keeps an outgoing record mapping the remote message ID to its target device and local
sender session. The return `reply` RPC checks that record before writing the sender's inbox.
Replies need the reverse SSH connection established during pairing. If that connection fails,
the replying device retains the reply for `status` polling and reports the callback failure.
The original sender can fetch it through `status`; importing the same reply twice must not
duplicate its inbox entry. A failed callback is not reported as successful inbox delivery.

Offline peer: `send` fails fast with exit 7 (`peer unreachable`) and leaves nothing queued locally.
A local outbox with retry is scope for later; silently queuing would turn `queued` into a lie.

## Phase 2: registry-driven discovery

- `everett devices` lists the account's devices (name, last seen, address, revoked) from the
  registry; `everett peer add <name>` resolves the address from it.
- Signed registry rows bind metadata to a device key, but do not prove who owns an unfamiliar key.
  A compromised registry cannot impersonate an already-pinned device without its private keys.
  It can publish an attacker's key and address for a new device, or hide devices and revocations.
  First pairing therefore requires the out-of-band fingerprint checks from phase 1, even when
  the registry supplies the address. Registry changes never replace a pin automatically.
- When signed in, devices publish `{device_id, name, public_key, address}` to Convex so a third
  device can discover addresses. Session content never goes to the registry.
- Revoking a device marks it revoked in Convex. Each remaining peer's next `everett doctor`
  removes that device's tagged entry from its own `~/.ssh/authorized_keys`, preserving unrelated
  entries. Offline peers retain access until they fetch the revocation; registry revocation is not
  immediate enforcement. `everett logout` on a lost device remains best effort.

## Phase 3 (optional): direct HTTPS peer endpoint

For devices that cannot run SSH (phones, locked-down hosts), a small authenticated endpoint
`everett peer serve --transport https` with device-key request signatures (RFC 9421). Only if phase
1 proves people want the cross-device flow at all; it brings listener lifecycle, certificates and
rate limiting into Everett.

## Hosted dashboard

`everett dashboard` stays local and read-only. Once phase 1 exists, the local dashboard can show
peer sessions through the same RPC, which is the "use the dashboard across devices" idea without a
hosted data service. A website on a custom domain can only ever show what a running Everett on one
of your devices serves it.

## Decided

The device registry uses Convex. Everett needs one `devices` table and the `upsertDevice` and
`listDevices` functions, both gated on the caller's OIDC identity. The Rust side calls Convex's
HTTP API with a token accepted by the configured identity provider. Registry storage contains
device metadata only.

## Decisions needed from the owner before building

1. **Issuer**: choose an OIDC provider that supports RFC 8628 device flow and tokens accepted by
   Convex (`docs/auth.md`). No hosted issuer is selected yet. Device revocation lives in Convex.
2. **Network**: Tailscale assumed for phase 1, with separate account authentication on each device.
   Plain SSH over LAN/VPN works identically with a manual address.
3. **Device naming**: `--device` from `everett login` doubles as the peer prefix; confirm names are
   per account unique (the registry enforces it).

## Smallest shippable slice

`everett peer add` + `peer serve` + remote `ls`/`card`/`send`/`status`/`reply` over manual SSH
addresses, no registry or Everett sign-in dependency. Tailscale still requires its own account
authentication if used for transport.

Tests use a fake `ssh` command, like the gateway tests' fixture backend, to exercise:

- Two-way pairing, rejection of unconfirmed fingerprints and pinned-key mismatches, and a failed
  reverse connection leaving pairing incomplete.
- Tagged key installation in `~/.ssh/authorized_keys`, absolute forced-command paths, preservation
  of unrelated entries, retry without duplicates, and revocation removing only the matching entry.
- Oversized requests, duplicate idempotency keys, and the three-hop limit across devices.
- `status` reporting `queued` to `delivered` only after the remote inbox's `done` set changes.
- Reply delivery to the original sender, rejection of a callback from the wrong peer, and status
  recovery without duplicate inbox entries when the return connection fails.

An isolated OpenSSH integration test must also prove that the tagged entry invokes only the
absolute forced command, never opens an unrestricted shell, and denies forwarding. A fake SSH test cannot prove
sshd enforces these restrictions.

## Not in scope

Transcript sync, remote shared-core merge, automatic wake-up of a remote session, phones as peers,
multi-user accounts.
