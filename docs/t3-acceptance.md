# Verify Everett from a real T3 client

Use two disposable agent sessions on the same execution host. Keep the exact MCP
results locally. Synthetic fixtures and a terminal-launched MCP server do not prove
that T3 registers or binds the server.

## Check the native client

1. Record the T3 client version and selected execution host. In the T3-created
   agent session, call `everett_whoami()` and `everett_ls()`. The returned native
   provider ID must match a session listed with `source="t3code"`. Record the
   identity's detection source. An empty or conflicting ID fails this check.
2. If identity is unavailable, use the host-owned per-thread MCP launch environment
   to set `EVERETT_SESSION_ID` to that exact provider ID, then restart the MCP
   connection. A model-supplied `session_id` cannot establish identity. Verify the
   calls again from the T3 client. See [MCP identity](mcp-identity.md).

Do not treat a bound session ID as proof of remote account ownership. For a remote
gateway, verify the authenticated owner as well. Local stdio access runs with the
owning operating-system account's permissions.

## Exchange an exact request and reply

1. Choose a unique marker such as `everett-t3-acceptance-20261026-a`. From the second
   bound agent, call `everett_send(to="<T3 native ID>", text="<marker>", mode="inbox")`.
   Record `message_id`. This is an explicit polling exception. A queue receipt
   does not pass the request-and-reply check.
2. In T3, call `everett_inbox()`. Verify that the returned request has that exact
   ID, sender, and marker. Reply with
   `everett_send(reply_to="<message_id>", text="<marker>-reply", mode="inbox")`.
3. From the second agent, call `everett_inbox()`. Verify the exact reply text,
   `reply_to`, and T3 sender ID. Poll again in both disposable sessions and verify
   that the test messages have cleared.

Use disposable inboxes so the check does not consume unrelated messages. T3 wake
remains unsupported.

## Verify shared core with owner authorization

Record the owner's approval of the exact test project, fact, and Everett home
before writing. Prefer an isolated test home configured for both MCP connections.

1. Read `everett_core(project="<approved test project>")` from both clients.
2. In T3, call `everett_learn(fact="<approved unique fact>", project="<approved test project>")`.
   Verify `learned=true`, `scope="project"`, and the approved project slug in the
   receipt. The receipt does not contain the fact text or source session. With
   owner-authorized local file access, inspect only the matching entry in
   `<approved Everett home>/.everett/core/inbox.jsonl`: its `text` must equal the
   approved fact, its `project` and `scope` must match, and its `session` must equal
   the `session_id` recorded from T3's `everett_whoami()`.
3. After the owner-approved merge, read `everett_core` again from the second
   client's existing MCP connection. Verify that the exact fact appears.

`everett trunk merge --llm none` processes the whole local pending inbox. Run it
only in the isolated test home or under explicit approval for all pending facts.
A pending count alone does not prove that the other client read the exact fact.

## Check archive and deletion from T3

Filter behavior for archived/deleted threads is covered by the unit regressions on
the [filter fix PR](https://github.com/raitoxlol/everett/pull/75). Use a binary that
includes that fix. For [issue #5](https://github.com/raitoxlol/everett/issues/5)'s
live lifecycle checkbox, use a disposable T3 thread on the real client. On its
selected execution host, use the same Everett home as the MCP connections and
replace the placeholder with the full native provider ID:

```bash
everett --hours 876000 send --mode inbox --dry-run --json \
  --to "<full-native-id>" "Everett T3 read-only acceptance probe"
```

1. While the thread is active, require exit status 0, `dry_run=true`,
   `delivered=false`, and `session.id` equal to that full ID. Check
   `session.source="t3code"`; a prefix or title match to another ID does not pass.
2. Archive it in the T3 UI. Repeat the command; require exit status 2 with
   `no session matches`. A usage, identity, or store error is inconclusive.
3. Restore it and require the exact active receipt again. Then, with owner
   approval, delete the disposable thread and repeat the command; require exit
   status 2 with `no session matches` again.

Do not send a live message for this check. Keep the dry-run receipts with the rest of
the evidence pack. There is no Python helper in-tree for this step.

## Record the result

Keep the client version, execution host, native identity result, exact message
IDs, approved core scope, observed fact, and lifecycle receipts together. Redact
unrelated prompts and credentials before sharing evidence. Mark each remaining
[issue #5](https://github.com/raitoxlol/everett/issues/5) checkbox only after its
matching native-client observation passes.
