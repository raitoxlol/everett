# Everett 2.0.0

Everett 2.0 adds a guided first run, native Cloudflare account and relay support, signed device discovery, and an OAuth resource-server mode for the Rust gateway. Local and SSH workflows remain available without an Everett account.

## Install or update

```bash
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh
everett doctor
```

The installer verifies the archive checksum. Set `EVERETT_VERSION=v2.0.0` to pin this release. To build from source, run `cargo install --git https://github.com/raitoxlol/everett everett`.

## What changed

- **Guided setup and native account approval.** First run offers local, cloud, and VM choices. Cloud and VM connections remain pending until configured. The native Cloudflare flow uses GitHub to identify the account owner and approve a device; it does not sign in to an agent provider. Cloudflare credentials and relay state are separate from OIDC/Convex state.
- **Native Cloudflare relay.** Add outbound HTTPS polling, encrypted message delivery, durable ciphertext staging, retry handling, and explicit peer fingerprint pins. A physical Mac/NUC fixture passed. Public cross-device message delivery has not yet been verified.
- **OAuth resource server.** The Rust gateway can authorize allowlisted external clients with PKCE, rotating refresh tokens, introspection, and revocation. Dynamic client registration is not supported. Live ChatGPT OAuth has not yet been verified.
- **Device discovery and revocation.** `everett devices` pages signed registry metadata used by `peer add NAME`. Pairing still requires out-of-band fingerprint checks. Revocations propagate when an offline peer next fetches them; existing SSH connections are not terminated.
- **MCP identity and routing.** Bind calls to host or per-call session identity, return identity failures as tool errors, and fall back to local routing when Jev rejects a request. Track reply depth separately from forward hops and install hooks only for harnesses Everett detects or the user selects.

## Upgrade from 1.6.0

- The 1.6.0 release already uses encrypted relay v2. This CLI version bump does not change that wire protocol. A deployment still using relay v1 needs a reviewed migration that preserves revocation and replay records; do not delete the data and re-enroll.
- Native Cloudflare credentials and state are separate from OIDC/Convex credentials and state; there is no automatic import between them. Existing OIDC configuration remains available.
- Native Cloudflare device authorization is a custom protocol, not RFC 8628. Local and SSH workflows remain usable without account sign-in.

## Verification and release scope

The public first-run GitHub account flow automatically advanced to the agent chooser, and a repeat bare `everett` invocation passed. A physical Mac/NUC fixture passed. Public cross-device delivery and live ChatGPT OAuth remain unverified.

The tagged CLI workflow runs Rust tests and Clippy on macOS and Linux, then builds four platform targets. Publishing the CLI does not deploy the Cloudflare Worker; Worker deployment and the Homebrew tap are separate operations.

See `CHANGELOG.md` for the complete history.
