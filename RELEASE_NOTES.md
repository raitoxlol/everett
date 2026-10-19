# Everett 1.6.0

Everett now reaches your agents on your other machines. Two independent transports ship in this
release, manual SSH and an end-to-end encrypted Convex relay. Neither one wakes or resumes a remote
session.

## Install or update

```bash
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh
everett install-hooks --apply && everett install-mcp --apply
everett doctor
```

`install.sh` installs or updates `everett` in `~/.local/bin` (override with `EVERETT_INSTALL_DIR`,
pin with `EVERETT_VERSION=v1.6.0`) and verifies the archive's SHA-256 before installing.
To build from source: `cargo install --git https://github.com/raitoxlol/everett everett`.

## What changed

- **Cross-device peers over SSH.** `everett peer add`, `serve`, `ls`, `card`, `status`, `identity`,
  `list`, `remove` and `revoke`, plus `ls --peer` and explicit remote inbox sends. Pairing verifies
  host and device fingerprints, pins host keys, and installs a tagged restricted forced command in
  the receiver's real `~/.ssh/authorized_keys`. Replies return through status polling, with
  idempotent send/reply recovery, observed queued/delivered/replied states, a three-hop end-to-end
  limit, 64 KiB request frames and 20-second deadlines. An offline peer fails fast with no local
  retry queue. SSH needs no registry, sign-in, Convex relay or Everett listener.
- **Encrypted relay v2.** Same-owner/account or same-org agent chat over live Convex WebSocket
  subscriptions, behind NAT, with no SSH or Tailscale dependency. Message and reply bodies use
  independent X25519 + HKDF-SHA256 + ChaCha20Poly1305 keys; envelopes and bundles are signed with
  independent relay Ed25519 keys. Cross-org chat needs bilateral org-admin grants through
  `relay grant invite / accept / reconfirm / revoke / get / list`, and a changed key blocks traffic
  until both admins reconfirm and peers re-pin.
- **Breaking: relay v2 is incompatible with v1 envelopes, pins and schema.** The v2 protocol requires
  verified OIDC ID tokens with issuer-controlled agent, org and role claims, authoritative active
  membership, and a distinct per-agent revocable credential on every public function. An existing v1
  deployment needs a reviewed migration that preserves revocation and replay records, not deletion
  and re-enrollment. Operator provisioning is required; there is no public enrollment API.
- **Host-bound MCP caller identity for delivery and replies.** Conflicting model session IDs are
  rejected instead of replacing provenance or hop lineage. Harnesses without native detection must
  bind `EVERETT_SESSION_ID` in their MCP launch environment. Claude Code, Grok CLI 1.0.4+, Codex
  (through authoritative per-call `_meta.threadId`), and `everett gateway` bind automatically.
  Pi, Devin, OMP, Hermes, and T3 Code need `EVERETT_SESSION_ID` set per session; restart or rebind
  their MCP server when the session changes. A model-supplied `session_id` cannot establish identity.
- **Bounded state and explicit repair.** Rejected-message diagnostics are bounded, `peer remove NAME
  --transport ssh|convex` repairs ambiguous legacy aliases, and ambiguous routing or removal now
  fails closed. Signed delivery deletes the live ciphertext row, and undelivered messages expire
  after one hour with bounded cron cleanup.

See `CHANGELOG.md` for the full list.

## Verification

GitHub Actions runs clippy (`-D warnings`), `cargo test` and the release journey (all 10 MCP tools
over isolated stores) on macOS and Linux, plus the Convex backend typecheck and tests, before
building the release targets. A live Convex deployment, live OIDC, real sshd enforcement on two
machines, real T3 hook pickup and live dots/Grok connections remain owner checks, not implied by the
offline coverage.
