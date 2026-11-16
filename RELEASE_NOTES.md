# Everett 2.3.0 beta — unpublished candidate

Bring the changes since 2.2.1 to outside testers using Claude Code or Codex.

- Add owner-local session control through `everett_control`.
- Keep ChatGPT chat and Codex identities distinct and isolate owner discovery.
- Improve tunnel recovery and package the hosted Everett plugin.
- Make the hop cap opt-in and add context to delivered batches.

## Acceptance required before publication

This candidate is not a verified outside-tester release. Require a fresh own-GitHub
sign-in, proof that an outside account cannot discover Wright’s agents, and two
actual harness agents exchanging a request and correlated reply through the hosted
relay. Run installation and acceptance on macOS as well as Linux. ChatGPT chat
mode is outside this beta.

See [the changelog](CHANGELOG.md) for the full change list.
