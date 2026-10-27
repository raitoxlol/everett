# Security review, October 2026

Historical defensive evidence for the local/MCP and SSH paths, not certification.
Retired backend findings and their deployment instructions have been removed.
The observations below describe the revisions tested then, not a fresh review.

### M1: unbound stdio MCP accepted a model-supplied `session_id` (High)

**Prerequisite.** `everett mcp` starts without a host-detected caller (no `EVERETT_SESSION_ID` and no native harness variable). The gateway is not affected.

The impact is access between logical sessions under the same local OS account, not an OS privilege escalation. The local account and MCP launcher remain trusted.

**Repro.** Seed `~/.everett/inbox/victim.jsonl`. On the unbound server, call `everett_inbox {"session_id":"victim","peek":true}`, then `everett_inbox {"session_id":"victim"}`, then `everett_card {"session_id":"victim", ...}`.

**Observed.** The caller read the victim's message, marked it delivered in `victim.done`, and wrote `cards/victim.md`. README and `docs/grok-bot.md` say a model-supplied `session_id` cannot establish identity.

**Fix.** `caller_session` in `everett-rs/src/mcp.rs` returns JSON-RPC `-32602` when `session_id` is supplied without a host binding. A matching `session_id` on a bound server still works. Codex's host-written per-call `_meta.threadId` is accepted as its binding because Codex overwrites that value at the MCP dispatch boundary.

**Regression tests.** `mcp_stdio::unbound_mcp_cannot_read_consume_or_overwrite_another_sessions_state`, `mcp_stdio::codex_call_metadata_binds_identity_without_process_environment`, plus updated `self_send_refused`, `unbound_mcp_cannot_deliver_or_reply_with_model_provenance`, and `audit::oversized_first_inbox_message_delivers_truncated` (now binds `EVERETT_SESSION_ID`).

**Behavior change.** Claude Code, Grok CLI 1.0.4+, Codex, and `everett gateway` bind automatically. Pi, Devin, OMP, Hermes, T3 Code, and other clients without supported host metadata must set `EVERETT_SESSION_ID` per session in the MCP launch environment. A client can no longer establish identity by passing a tool argument.

Published host contracts were checked before keeping the manual-binding list. [Pi's session variables](https://pi.dev/docs/latest/environment-variables) are injected into shell tools, not MCP servers. Devin puts `session_id` in [lifecycle-hook payloads](https://docs.devin.ai/cli/extensibility/hooks/lifecycle-hooks), while its [MCP configuration](https://docs.devin.ai/cli/extensibility/mcp/configuration) offers static `env` values and no per-call session field. OMP's [MCP configuration](https://omp.sh/docs/mcp) likewise expands process/static configuration; its session ID is available to Everett's OMP extension through `sessionManager`, not on MCP calls. No automatic Pi, Devin, or OMP binding is inferred from their shell/hook APIs.

### M2: `grok-bot` and `openai-dot` messages were not labeled untrusted (Medium)

**Prerequisite.** A registered external harness sends or replies through bound `everett mcp` or `everett gateway`.

**Repro.** Register `ext-grok-bot` with `everett external add --harness grok-bot`. From that binding, reply to an owner handoff. Read the owner's inbox.

**Observed.** The owner received raw text with no `external`, `untrusted`, `trust`, or `instruction_source` field and no `[EXTERNAL / UNTRUSTED ...]` wrapper. Relay cross-org messages were already labeled. Only the logical external harnesses were missed.

**Fix.** `inbox::post` stamps the labels when `from_harness` is an external harness. `inbox::external` also derives provenance from `from_harness`, so messages queued before this fix are labeled on read. A reply derives provenance from the exact original message. Merely peeking at or consuming a logical Grok Bot/OpenAI dot message does not write durable relay context or taint unrelated later resume/spawn work; durable session context remains limited to messages with relay lineage.

**Regression tests.** `external::previously_queued_external_messages_without_trust_flags_are_untrusted_on_read`, `external::cloud_origin_provenance_survives_reply_without_tainting_unrelated_forward`, updated `external::stdio_mcp_queues_polls_and_replies_for_both_external_harnesses`, the `gateway_native` stdio journey, and `relay_native` same-org and cross-org consumption through real `everett mcp` and `everett gateway` processes.

### M3: JSON-RPC ids `true`, `{}`, and `[]` were dispatched (Low)

**Repro.** Send `{"jsonrpc":"2.0","id":true,"method":"tools/call", ...}` to `everett mcp`.

**Observed.** The tool ran. JSON-RPC 2.0 allows only string, number, or null ids.

**Fix.** `mcp::handle` returns `-32600` with `id: null` and does not dispatch. The gateway already dropped these frames without dispatch, before and after the fix.

**Regression test.** `mcp_stdio::malformed_jsonrpc_ids_never_dispatch_tools`.

### M4: concurrent MCP card updates shared one temporary filename (Low)

**Repro.** Run `cargo test --locked --test mcp_stdio concurrent_card_updates` in `everett-rs/`. The regression starts eight independent stdio MCP processes bound to the same synthetic session and synchronizes 16 rounds of ordinary `everett_card` updates.

**Observed.** The old `.SESSION.md.tmp` path let one process rename another process's temporary file. A legitimate update failed with `No such file or directory`. This was reproduced on unchanged main in the independent review and during final verification. One successful baseline run does not disprove this concurrency-sensitive race.

**Fix.** `tool_card` uses the existing `peer::save` atomic writer: unique temporary file, private permissions, sync, then persist. Last complete write wins. No optimistic concurrency or card history is promised.

**Regression test.** `mcp_stdio::concurrent_card_updates_are_atomic_and_do_not_share_temporary_files` checks all 128 updates succeed, the final card is a complete last-round body, and no temporary files remain. The test and fix from #27 are retained in #26.

### S1: a held-open SSH channel can submit one frame after revocation (Low, not fixed)

**Prerequisite.** The attacker holds the paired client key and opens the forced-command channel before the receiver runs `everett peer revoke`.

**Repro.** On two Docker hosts (`alice@A`, `bob@B`), A opens `ssh bob@B` with the paired key and sends nothing. B runs `everett peer revoke <client>`; the `authorized_keys` tag is gone. A then writes one `send` frame within the 20-second `peer serve` timeout.

**Observed.** B queues the message with `peer_client` set to the revoked ID. A new login with the same key fails with `Permission denied (publickey)`. `main` and PR #24 behave the same.

**Status.** Not fixed. `docs/cross-device.md` says revocation takes effect on new SSH connections. Re-checking `authorized(client)` after `read_frame()` in `peer::serve` would shrink the window but not close it.

### SSH hosts

Two Ubuntu 24.04 containers had separate network namespaces, filesystems, users (`alice` and `bob`), sshd processes, host keys, Everett keys, `HOME`, and `EVERETT_HOME`. No owner SSH agent or real key directory was mounted. Pairing used independently read host/device fingerprints and a stable absolute binary path.

The [SSH reviewer](https://app.devin.ai/sessions/1b27133bdca34fa2bf44b8dbec05065a) exercised pairing, A→B inbox delivery, take/reply, sender status polling, receiver revocation, rejection on a fresh SSH connection, and explicit re-pairing on main and #24. Eighteen of nineteen security/journey checks passed on each binary; the nineteenth exposed S1. A separate [integration run](https://app.devin.ai/sessions/db4f9ff95ab44c339115f2e2c4de8728) also completed normal two-host pairing/send/reply/revoke.

These are real OpenSSH connections, not the fake-SSH fixture used by some Rust tests. Hosted device revocation and explicit SSH revocation are separate.

### Grok Bot and OpenAI dots

The local portions of `docs/grok-bot.md`, `docs/openai-dots.md`, and `docs/gateway.md` were exercised with explicit `grok-bot` and `openai-dot` registrations. Real `everett mcp` and `everett gateway` subprocess clients covered bound identity, queue-only delivery, inbox peek/take/reply, and the gateway's exact-destination and inbox-only restrictions. Native HTTP tests cover the authenticated loopback transport, Host/Origin checks, frame bounds, and tool restrictions. Shared core is optional; with it disabled the gateway exposes four tools.

M1 and M2 were discrepancies with the documented identity/data boundaries on main and are fixed here. The docs correctly distinguish a logical owner binding from vendor authentication, automatic cloud wake-up, and provider consumption. No live Grok Bot or existing OpenAI dot was connected. The Secure MCP Tunnel, provider permissions, credential UI, and tool selection remain owner-side verification, not inferred successes from local MCP tests. No claim is made that an LLM obeys the untrusted warning.

## Recovered negative-test observations

The original review reported the following observations from synthetic fixtures and local services. These are bounded checks, not proof of exhaustive attack resistance. Consolidation reran the committed suites, not the separate one-off attack harnesses or live stacks.

- **SSH.** Two isolated Docker hosts with separate users, IPs, sshd, host keys, `HOME`, and `EVERETT_HOME` ran 19 checks on `main` and PR #24. Wrong host or device pins, forced-command escapes (`; id`, `$(...)`, newline, `peer serve --client attacker --setup`), unknown ops, `client` override fields, oversized frames, and pinned `known_hosts` replacement were all refused. Pairing, send, reply via `peer status`, and re-pairing worked. Only S1 failed.
- **MCP, Grok Bot, and dots.** Registration, queue-only delivery, refused resume and spawn, peek, take, and reply worked through `everett mcp` and `everett gateway` for both harnesses. A bound `session_id` spoof was rejected. The gateway exposed only `everett_whoami`, `everett_inbox`, `everett_card`, and `everett_send`, and rejected foreign destinations, foreign `reply_to`, ambiguous prefixes, and spawn. Registration IDs `../escaped`, `x/y`, `..`, `human`, `live`, `a.b`, `%2e%2e`, non-ASCII, and 101 characters were rejected without writing outside `~/.everett/external`.
