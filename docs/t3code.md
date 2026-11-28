# T3 Code sessions and messaging

Everett reads T3's SQLite projections without modifying the database. It prefers
`~/.t3/userdata/statev2.sqlite` when present and otherwise reads legacy
`~/.t3/userdata/state.sqlite`. It does not combine the stores: a migrated legacy
copy must not revive old threads.

The v2 reader follows [T3 commit 43f8a8d](https://github.com/pingdotgg/t3code/tree/43f8a8de17a7ac1baa7a3cf36d681856de2d8add),
specifically migrations 005 and 055/Foundation and the `orchestrationV2.ts`
contracts. It follows the application's active provider thread to
`nativeThreadRef.nativeId`, using the provider **driver**, not the configured
provider instance, to select Codex, Claude, or Grok. Devin threads use the ACP registry
driver with provider instance `devin`. Other drivers are not supported.
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
T3 CLI must support thread commands. Everett selects `EVERETT_T3_BIN` first.
It then uses the active, completely installed runtime in the Everett profile's
`.t3/runtime` directory, followed by `t3` on PATH and `.t3/bin/t3`. An executable shim on PATH may still fail if
its runtime is unavailable. Check that exact executable's `thread send --help`
and `thread start --help`; no installer or runtime configuration is changed by
Everett. Plain help output or a malformed JSON response cannot confirm dispatch.

Set `T3CODE_HOME` in Everett's environment when T3 uses a separate data folder.
Everett reads T3's session database and managed runtime from that folder. The
Everett inbox still lives in the Everett profile.
Do not blindly retry an unconfirmed send or start: the CLI might have acted before
its response failed.

Dispatch confirmation requires the CLI's command-specific JSON receipt, not just
parseable JSON or exit zero. Non-waited sends require a matching `threadId`,
`messageId`, `runId`, `status`, and `delivery`. Starts require `threadId`,
`projectId`, and a nullable `runId`. Waited results require the matching thread,
a run ID, `status="completed"` at exit zero or `status="waiting"` at exit two,
plus `reply` and a null `error`. A start with no run retains its launch receipt
even with `--wait`. Negative acknowledgements, empty objects, arrays,
wrong-target receipts, or incomplete shapes leave dispatch unconfirmed. An empty
array remains a valid thread listing. Failed ping confirmation retains the durable
inbox message and does not establish receiver pickup.

These shapes were checked in the T3 source checkout at snapshot `8d14716`:
`apps/server/src/cli/thread.ts` (`ThreadLaunchResult` and `finishRun`) and
`packages/contracts/src/orchestratorMcp.ts` (`OrchestratorMcpThreadSendResult`).
The same shapes were inspected in the locally installed T3 0.1.9 runtime.
The snapshot is local; no public source URL for it was verified. Source inspection
and the subprocess fixtures do not establish live thread dispatch or completion.

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
home. Each sending or replying provider session needs its own host-bound MCP caller identity.
Use the binding check below before configuring an explicit launch environment.
A `session_id` argument cannot establish that binding. Installing Everett in a standalone provider does not prove it is available
inside a T3-created provider session.

### Have the agent verify its binding first

Ask the agent already running in the receiving T3 thread to drive these checks.
Use the same Everett profile for the terminal and the MCP server. Native MCP
calls require an existing sign-in in that profile; local CLI commands do not.
If sign-in is missing, stop and obtain authorization before running `everett login`.

1. The agent runs `everett ls --json` and identifies its exact underlying provider
   session ID. A T3 application thread ID is a destination alias, not a caller ID.
2. Inside that thread, the agent calls `everett_whoami()` and compares `session_id`
   with that provider ID. If they match, keep the working binding. Current Codex
   integration can use per-call `_meta.threadId`; it does not need a static override.
3. If identity is missing, configure a **dedicated** MCP launch for this one
   provider session. Merge the following environment into that launch's existing
   Everett server entry, preserving its absolute command, arguments, profile
   environment, and other servers:

   ```json
   {
     "command": "/absolute/path/to/everett",
     "args": ["mcp"],
     "env": {"EVERETT_SESSION_ID": "EXACT_PROVIDER_SESSION_ID"}
   }
   ```

   For a launcher that accepts command and arguments instead of an `env` object,
   the equivalent launch is `/usr/bin/env EVERETT_SESSION_ID=EXACT_PROVIDER_SESSION_ID /absolute/path/to/everett mcp`.
   The variable must reach the MCP subprocess. Exporting it in an unrelated shell
   or passing a `session_id` tool argument does not configure a running MCP server.
   Never put one thread's static ID in a shared registration used by other threads.
4. Restart or rebind that thread's MCP process and call `everett_whoami()` again.
   Stop if the tool is absent, sign-in fails, the ID is empty or mismatched, or the
   host cannot provide a dedicated launch. Do not send or consume an inbox to
   work around a missing binding.

The launch-environment contract is covered by `everett-rs/tests/host_identity.rs`
and the isolated `release_journey` test. The JSON above describes a subprocess
registration, not a verified T3 settings screen or CLI flag. A T3-specific way
to install a dedicated per-thread registration remains **unverified**. Standalone
provider configuration alone does not establish that T3 loaded it. The in-thread
`everett_whoami` check is required before treating setup as successful.

### One CLI request, recipient pickup, and correlated reply

This is an agent-driven manual acceptance procedure, **not a recorded live T3
result**. Use one existing, authorized receiving thread. Do not start another
model session. First complete the binding check above. Explicit inbox mode does
not wake the receiver; arrange for the existing agent to poll on its next turn.

1. In the sending agent's terminal, choose an explicit sender identity for this
   check and the exact recipient ID from `everett ls --json`:

   ```sh
   sender_id=local-cli-acceptance
   recipient_id=EXACT_PROVIDER_SESSION_ID
   EVERETT_SESSION_ID="$sender_id" everett send "Reply with ACK only" --to "$recipient_id" --mode inbox --json
   ```

   Save the returned `message_id` as `request_id`. Confirm `reply_inbox` equals
   `local-cli-acceptance`. `queued=true` proves enqueue only. Send once; do not
   retry automatically after an uncertain result.
2. In the receiving T3 thread, have the agent call
   `everett_inbox(message_id="REQUEST_ID")`, substituting that request ID. Confirm
   the returned message's `id` matches. This consumes the matching request.
   Then call `everett_send(reply_to="REQUEST_ID", text="ACK")`.
3. Back in the original terminal, the sending agent sets the saved ID and checks
   the original receipt and the matching reply:

   ```sh
   request_id=RETURNED_MESSAGE_ID
   EVERETT_SESSION_ID="$sender_id" everett status "$request_id" --json
   everett inbox --session "$sender_id" --reply-to "$request_id" --json
   ```

   Require `picked_up=true`, state `replied`, and reply text `ACK` in status.
   Require the inbox reply's `reply_to` to equal the original request ID and
   `from` to equal the recipient's provider ID. An empty inbox after prior
   consumption is not a lost reply: status retains the correlated response.

The isolated release journey covers CLI send, MCP pickup/reply, and retained
correlation using synthetic sessions. It does not verify this host's T3 MCP
registration, automatic hooks, wake, provider access, or task completion.

## Identity and shared memory

Bind the MCP caller using its **native provider session ID**. The exact T3
application thread ID is an alias for send destinations. Caller identity still
uses the provider session. If `everett_whoami` has no identity, configure that
session's MCP launch environment with its exact `EVERETT_SESSION_ID` before
delivery or replies. Rebind or restart the MCP process after switching provider
sessions.

Once Everett's MCP tools are available, `everett_core(project=<workspace>)` reads the
shared core and `everett_learn(fact=..., project=<workspace>)` queues a learning.
Queued learnings enter the shared core after the normal Everett merge workflow;
enqueue alone does not immediately update every agent's injected context.

Synthetic Rust tests cover provider-ID discovery, read-only access,
deduplication, route guidance, CLI nudge receipts, explicit inbox pickup, replies,
self-send rejection with known identity, and learning/merge/peer shared-core reads. They do not establish
real T3 SDK MCP registration, hook execution, automatic wakeup, or cross-device delivery.
