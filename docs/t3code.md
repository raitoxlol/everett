# T3 Code sessions and messaging

Everett reads T3's SQLite projections without modifying the database. It prefers
`~/.t3/userdata/statev2.sqlite` when present and otherwise reads legacy
`~/.t3/userdata/state.sqlite`. It does not combine the stores: a migrated legacy
copy must not revive old threads.

The v2 reader follows [T3 commit 43f8a8d](https://github.com/pingdotgg/t3code/tree/43f8a8de17a7ac1baa7a3cf36d681856de2d8add),
specifically migrations 005 and 055/Foundation and the `orchestrationV2.ts`
contracts. It follows the application's active provider thread to
`nativeThreadRef.nativeId`, using the provider **driver**, not the configured
provider instance, to select Codex, Claude, or Grok. Other drivers are not supported.
Legacy cursors use Codex `threadId`, Claude `resume`, or Grok `sessionId`.

## Discovery

Existing provider transcripts keep their IDs, paths, prompts, and working directories.
Everett marks them `source=t3code` and fills missing metadata from T3. If the provider
transcript is absent, an active T3 projection with a real native ID supplies the
session metadata instead. No transcript file or provider ID is fabricated.

Deleted or archived threads and deleted projects hide their linked provider sessions,
including existing native transcripts. An active T3 thread linked to the same native
session keeps that session visible. Unlinked standalone provider sessions stay visible.
Fallback sessions obey the look-back window and harness filter before the listing
limit. Worktrees take precedence over the project's workspace root. A current v2
database with unsupported/corrupt schema is reported and skipped, not silently
replaced with potentially stale legacy state.

## Polling and replying

T3 owns its provider's resume point. Everett refuses direct provider CLI resume,
and its route output tells you to continue in T3. Owner-local `ping` and `auto`
delivery use `t3 thread send` to send a fixed inbox nudge into the owning thread.
An idle thread starts a turn; a busy thread queues the next turn. The selected
T3 CLI must support thread commands. Everett selects `EVERETT_T3_BIN`, then
`t3` on PATH, then `~/.t3/bin/t3`. An executable shim on PATH may still fail if
its runtime is unavailable. Check that exact executable's `thread send --help`
and `thread start --help`; no installer or runtime configuration is changed by
Everett. Plain help output or a malformed JSON response cannot confirm dispatch.
Do not blindly retry an unconfirmed send or start: the CLI might have acted before
its response failed.

Owner-local `everett_control` send and wake requests validate `timeout` before
updating receiver hop state or dispatching. The timeout also reaches non-T3
ping/resume subprocesses. Values must be finite and positive and are capped at
1800 seconds; the default is 30 seconds, or 600 with `wait=true`. A timeout after
dispatch does not establish that the recipient did nothing. Do not retry blindly.

The hosted ChatGPT gateway stays inbox-only and cannot use owner-local control.
Use explicit `mode: "inbox"` when you accept polling for pickup.

Inbox enqueue is **not proof of pickup**. An explicit inbox-only T3 send reports `queued=true`,
`delivered=false`, `wake=false`, `pickup=poll`,
`poll_session_id`, and `hooked=false` conservatively; Everett does not verify that
T3's SDK executes native hooks or inherits a user-installed Everett MCP server.
The receiver must have access to Everett's MCP tools and the same local Everett
home. Each sending or replying provider session needs its own host-bound MCP caller identity, using `EVERETT_SESSION_ID` in the launch environment when native detection is unavailable. A `session_id` argument must match that binding. Installing Everett in a standalone provider does not prove it is available
inside a T3-created provider session.

1. List the thread with `everett_ls`. Use the returned provider session `id`.
2. Bind each participant's exact provider ID as `EVERETT_SESSION_ID` in that
   T3 thread's MCP launch environment.
3. From another agent, call `everett_send(to=<provider id>, text=..., mode="inbox")`.
4. In the receiving T3 session, call `everett_inbox()`.
   This marks returned messages consumed; use `peek=true` to inspect without consuming.
5. Reply with `everett_send(reply_to=<message id>, text=...)`.
6. The sender polls `everett_inbox()` for the reply.

An explicit inbox-only send does not wake the receiver. Poll again at task/turn boundaries
when native hook pickup has not been independently verified. A wait timeout does not
mean the queued request was lost.

## Identity and shared memory

Use **native provider IDs**, not T3 application thread IDs or internal provider-thread
IDs. T3 aliases and automatic T3 caller detection are not implemented. If
`everett_whoami` has no caller identity, the owner must configure that provider session's MCP launch environment with its exact `EVERETT_SESSION_ID` before delivery or replies. A model-supplied `session_id` does not authenticate the caller. Rebind or restart the MCP process after switching provider sessions.

Once Everett's MCP tools are available, `everett_core(project=<workspace>)` reads the
shared core and `everett_learn(fact=..., project=<workspace>)` queues a learning.
Queued learnings enter the shared core after the normal Everett merge workflow;
enqueue alone does not immediately update every agent's injected context.

Synthetic Rust tests cover provider-ID discovery, read-only access,
deduplication, route guidance, CLI nudge receipts, explicit inbox pickup, replies,
self-send rejection with known identity, and learning/merge/peer shared-core reads. They do not establish
real T3 SDK MCP registration, hook execution, automatic wakeup, or cross-device delivery.
