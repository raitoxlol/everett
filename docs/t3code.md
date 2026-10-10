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

Deleted/archived threads and deleted projects do not supply fallback sessions.
Fallback sessions obey the look-back window and harness filter before the listing
limit. Worktrees take precedence over the project's workspace root. A current v2
database with unsupported/corrupt schema is reported and skipped, not silently
replaced with potentially stale legacy state.

## Polling and replying

T3 owns its provider's resume point. Everett refuses direct provider CLI resume,
and its route output tells you to continue in T3. Automatic delivery selects the
Everett inbox even when no provider process is visible.

Inbox enqueue is **not proof of pickup**. A T3 result reports `pickup=poll`,
`poll_session_id`, and `hooked=false` conservatively; Everett does not verify that
T3's SDK executes native hooks or inherits a user-installed Everett MCP server.
The receiver must have access to Everett's MCP tools and the same local Everett
home. Installing Everett in a standalone provider does not prove it is available
inside a T3-created provider session.

1. List the thread with `everett_ls`. Use the returned provider session `id`.
2. From another agent, call `everett_send(to=<provider id>, text=..., session_id=<sender id>)`.
3. In the receiving T3 session, call `everett_inbox(session_id=<provider id>)`.
   This marks returned messages consumed; use `peek=true` to inspect without consuming.
4. Reply with `everett_send(reply_to=<message id>, text=..., session_id=<provider id>)`.
5. The sender polls `everett_inbox(session_id=<sender id>)` for the reply.

The receiver is not automatically woken by enqueue. Poll again at task/turn boundaries
when native hook pickup has not been independently verified. A wait timeout does not
mean the queued request was lost.

## Identity and shared memory

Use **native provider IDs**, not T3 application thread IDs or internal provider-thread
IDs. T3 aliases and automatic T3 caller detection are not implemented. If
`everett_whoami` has no caller identity, pass `session_id` explicitly. Known native
identity enables existing self-send protection; unknown identity is not a guarantee.

Once Everett's MCP tools are available, `everett_core(project=<workspace>)` reads the
shared core and `everett_learn(fact=..., project=<workspace>)` queues a learning.
Queued learnings enter the shared core after the normal Everett merge workflow;
enqueue alone does not immediately update every agent's injected context.

Synthetic Rust tests cover provider-ID discovery, read-only access,
deduplication, route guidance, explicit inbox pickup, replies, self-send rejection
with known identity, and learning/merge/peer shared-core reads. They do not establish
real T3 SDK MCP registration, hook execution, automatic wakeup, or cross-device delivery.
