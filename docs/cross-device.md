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

Pairing (`everett peer add`):

1. Each device generates an Ed25519 keypair under `~/.everett/peer/` (0600) at first use.
2. `everett peer add linux-box --ssh wright@linux-box.tailnet.ts.net` records the address, pins the
   SSH host key on first connect (printed for confirmation, like `ssh`), and installs this device's
   public key in the remote `~/.everett/peer/authorized_keys` through one interactive SSH session.
3. The remote key line is restricted: `command="everett peer serve",no-pty,no-port-forwarding,
   no-agent-forwarding,no-X11-forwarding`. The caller can run nothing except the Everett RPC.
4. When signed in, both devices also publish `{device_id, name, public_key, address}` to the
   registry so a third device can list and pair without typing addresses. Without sign-in, pairing
   is manual and still works.

Transport: `ssh -i ~/.everett/peer/id_ed25519 <addr> everett peer serve`, one request per
connection, newline-delimited JSON on stdin/stdout, request bodies capped (64 KiB), 20 s timeout.
No message text in shell arguments.

RPC (served by `everett peer serve`, which refuses anything else):

| Request | Reply | Notes |
|---|---|---|
| `{"op":"ls","hours":72}` | `Session[]` as today, each with `device` set | same look-back and filters as local `ls` |
| `{"op":"card","session":ID}` | `{card, source, updated}` | read-only |
| `{"op":"send","session":ID,"text":…,"from":{device,session,harness,card},"idempotency_key":K,"hops":n}` | `{message_id, state:"queued"}` | writes the remote inbox; duplicate `K` returns the existing id |
| `{"op":"status","message_id":M}` | `{state: queued\|delivered\|replied, reply?}` | `delivered` = present in the remote `done` set |
| `{"op":"reply","message_id":M,"text":…}` | `{reply_id}` | only for a message addressed to the remote device |

Session IDs gain a device prefix at the boundary only: `linux-box/claude/abc123`. Local code keeps
plain IDs; `send.rs` splits the prefix, routes to the peer if it is not this device, and
`inbox.rs` stores the sender as `mac-studio/claude/def456` so `reply` knows which peer to call back.
`hops` keep counting across devices; the three-hop limit applies end to end.

Offline peer: `send` fails fast with exit 7 (`peer unreachable`) and leaves nothing queued locally.
A local outbox with retry is scope for later; silently queuing would turn `queued` into a lie.

## Phase 2: registry-driven discovery

- `everett devices` lists the account's devices (name, last seen, address, revoked) from the
  registry; `everett peer add <name>` resolves the address from it.
- Registry rows are signed by the device key, so a compromised registry can hide devices but cannot
  impersonate one: the caller still verifies the SSH host key it pinned (or the published one).
- Revoking a device = mark revoked in the registry **and** drop its key from every peer's
  `authorized_keys` on next `everett doctor` (which pulls the revoked list). `everett logout` on the
  lost device is best effort, as today.

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

## Decisions needed from the owner before building

1. **Issuer**: Auth0, Zitadel or Supabase Auth for the hosted tenant (any RFC 8628 issuer works;
   `docs/auth.md`). This also decides where "revoke a lost device" lives.
2. **Registry backend**: Convex (decided 2026-10-10). Everett needs one `devices` table and two
   functions, `upsertDevice` and `listDevices`, both gated on the caller's OIDC identity; the Rust
   side calls them over Convex's HTTP API with the access token from `auth.json`.
3. **Network**: Tailscale assumed for phase 1. Plain SSH over LAN/VPN works identically with a
   manual address.
4. **Device naming**: `--device` from `everett login` doubles as the peer prefix; confirm names are
   per account unique (the registry enforces it).

## Smallest shippable slice

`everett peer add` + `peer serve` + remote `ls`/`send`/`status` over manual SSH addresses, no
registry, no sign-in dependency. Tests: an in-process fake `ssh` command (like the gateway tests'
fixture backend) exercising pairing, pinned-key mismatch, oversized body, duplicate idempotency key,
hop-limit across devices, and `status` reporting `queued` → `delivered` only after the remote
inbox's `done` set changes.

## Not in scope

Transcript sync, remote shared-core merge, automatic wake-up of a remote session, phones as peers,
multi-user accounts.
