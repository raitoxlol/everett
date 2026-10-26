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
   Verify the fact and bound source session in the returned receipt.
3. After the owner-approved merge, read `everett_core` again from the second
   client's existing MCP connection. Verify that the exact fact appears.

`everett trunk merge --llm none` processes the whole local pending inbox. Run it
only in the isolated test home or under explicit approval for all pending facts.
A pending count alone does not prove that the other client read the exact fact.

## Check archive and deletion from T3

Run the read-only helper on the selected execution host. Use an absolute path to
the candidate Everett binary and the full provider ID returned by the native call.

```sh
python3 scripts/check_t3_filter.py \
  --binary /absolute/path/to/everett \
  --home /absolute/path/to/execution-host-home \
  --native-id <full-provider-id> --expect visible
```

1. Check a disposable active thread and save the JSON receipt.
2. Archive it in the actual T3 UI. Run the same command with `--expect hidden`.
   The receipt must show an archived link and `resolved=false`.
3. Restore the thread in T3 and repeat `--expect visible`.
4. With the owner's approval, delete a disposable thread in T3 and repeat
   `--expect hidden`. The receipt must show a deleted link and `resolved=false`.

The helper opens the current v2 SQLite store read-only and calls only
`everett send --mode inbox --dry-run`. It does not send a message, start a provider,
change T3 state, or write core. Its receipt omits prompts and marks native MCP,
live messaging, and shared-core proof as `not_checked`.

An active T3 link to the same native ID keeps that session visible. Choose a unique
disposable provider session for the archive and deletion checks. If the store has
no supported link for an ID, the helper reports an inconclusive check.

## Record the result

Keep the client version, execution host, native identity result, exact message
IDs, approved core scope, observed fact, and lifecycle receipts together. Redact
unrelated prompts and credentials before sharing evidence. Mark each remaining
[issue #5](https://github.com/raitoxlol/everett/issues/5) checkbox only after its
matching native-client observation passes.
