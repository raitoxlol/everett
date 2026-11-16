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

The November 16 Linux audit installed 2.2.1 through install.sh. Public enrollment’s
20 synthetic tests passed; live outside-account approval remains pending.
Do not infer hosted delivery from health checks or MCP tool discovery.

No Worker deployment, production enrollment change, merge, release tag, or
publication is authorized by this candidate. See issue #156 for acceptance work.
