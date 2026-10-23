# Everett 2.1.0 — release candidate

This release completes account-free local setup and adds opt-in delivery to idle Claude background sessions. The v2 relay protocol stays compatible with 2.0.0.

## Install or update

```bash
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh
everett doctor
```

The installer verifies archive checksums. After publication, set `EVERETT_VERSION=v2.1.0` to pin this version. This candidate is not yet published.

## What changed

- Local setup works without an Everett account. Remote enrollment is an explicit choice after local agent setup.
- `send --wake` can wake an idle Claude background session. Automatic receiver wake requires a local policy allowing the exact session, directory and paired sender device. It is disabled by default.
- Replies retain their request correlation, signed remote provenance and Claude harness identity. Targeted inbox reads preserve unrelated pending messages. A delivered prompt is distinct from a model reply.
- Hosted multi-account enrollment is implemented behind a deployment flag. Each GitHub account owns a separate relay namespace; cross-account messaging remains denied. Broad enrollment remains gated on live second-account acceptance.

## Verified behavior

The real-model acceptance is recorded in [receiver wake PR56](https://github.com/raitoxlol/everett/pull/56).

On 2026-10-24, a physical Linux NUC sent through the public HTTPS relay to a dedicated idle Claude session on a Mac. Claude read the exact request and returned a correlated reply to the NUC. Native terminal observations confirmed the same Claude process and unchanged unsent composer draft. Idempotent replay produced no duplicate reply; a denied receiver policy imported the message without waking the agent.

The Mac reused its existing Claude provider login. Everett GitHub enrollment identifies devices; it does not authenticate an agent provider or transfer provider credentials between machines.

## Scope and remaining gates

Public second-account enrollment, live ChatGPT OAuth and deployed capacity are not yet verified. T3 discovery and MCP behavior have synthetic coverage; automatic T3 wake is not claimed. Native Everett credentials expire after 30 days and require reenrollment.

The service stores account and device metadata, signed routing/status metadata and queued encrypted message bodies. Plaintext transcripts and shared local memory remain on devices. It is inaccurate to describe this relay as storing only authentication information.

Final tagged artifact checks, public-enrollment acceptance and deployment, and the Homebrew update must complete before this candidate is announced as GA.
