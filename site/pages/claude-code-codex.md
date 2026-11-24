This walkthrough connects two local coding agents — Claude Code and Codex — so they can see
each other's sessions and hand work back and forth through Everett. It covers install, agent
registration, session identification, sending a request, delivery, and reading the correlated
reply. Commands were exercised on Everett 2.4.0 (Linux, revision `53f0d53`); where a step was
not re-run for this page, the limit is stated.

## The problem

Claude Code and Codex each keep their own session history. A Claude Code session cannot see
what a Codex session did, and neither can pick up where the other stopped — you end up copying
context between windows by hand. Everett sits above both: it reads each harness's existing
session store, summarizes every session with a card, routes a request to the session it
continues, and delivers messages plus replies between them.

## Prerequisites

- macOS or Linux with [Claude Code](https://code.claude.com) and [Codex](https://github.com/openai/codex) installed.
- Everett 2.4.0 or newer (`everett --version` to check).
- An Everett account for MCP tool calls: in 2.4.0, `everett mcp` verifies the account online on
  every call. Local CLI commands (`ls`, `route`, `send`, `inbox`) work without sign-in.

## 1. Install Everett

```sh
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/v2.4.0/install.sh | EVERETT_VERSION=v2.4.0 sh
export PATH="$HOME/.local/bin:$PATH"   # add to your shell profile too
```

With Homebrew instead:

```sh
brew tap raitoxlol/tap
brew install everett
```

Check the install:

```sh
everett --version   # expect everett 2.4.0
everett doctor      # verifies launchers and prints exact next commands
```

## 2. Sign in for MCP tools

```sh
everett login
everett whoami
```

`everett login` opens a device-approval page; compare the code shown there with your terminal
before approving. `everett whoami` should print your account and device. Skip this only if you
plan to use the CLI alone — agent tool calls will ask for sign-in.

## 3. Connect both agents

The fastest path registers hooks and MCP for every agent Everett detects:

```sh
everett onboard --yes --no-backfill
```

`--no-backfill` skips generating cards for old sessions; drop it if you want history
summarized. To configure one agent at a time instead:

```sh
everett install-hooks --claude --apply
everett install-mcp --claude --apply
everett install-hooks --codex --apply
everett install-mcp --codex --apply
```

Each `--apply` backs up the harness config before merging. Then **quit and reopen** Claude Code
and Codex so they load the new hooks and MCP server.

## 4. Verify each agent can see Everett

In a fresh Claude Code session and a fresh Codex session, ask each agent to call
`everett_whoami` and `everett_ls`. `everett_whoami` should return a non-empty `session_id`
matching that session; `everett_ls` should list your recent sessions (an empty list is a valid
result early on, not a failure). From the terminal, the same view is:

```sh
everett ls --hours 72
```

Each line shows the harness, age, project, and the session's card — what it is about, its
state, and the next step. Cards are what routing reads, so a good card means better routing.
Agents write their own cards; the Stop hook writes a fallback when one is missing (see
[Session cards and hooks](/session-cards/)).

## 5. Identify the sessions

You need each session's ID only when addressing it explicitly. `everett ls` prints short IDs;
`everett ls --json` gives the full records. To preview where a request would go without sending
anything:

```sh
everett route "cap upload retries at 5 and add a test"
```

Routing answers `SESSION` (one clear match — `send` would deliver there), `NEW` (no match), or
`ASK` (several close candidates — `send` delivers nothing and returns the candidates). See
[Routing](/routing/) for the local BM25 router and the optional Jev router.

## 6. Send a request from one agent to the other

Three equivalent ways:

- **Let the agent do it.** In Claude Code, ask: *"Use Everett to ask the Codex session working
  on `<project>` to …"* The agent calls `everett_send` with just `text`; Everett routes and
  reports which session it picked. See the [agent operating guide](/agent-guide/) for the rules
  agents follow.
- **From your terminal.** `everett send "<request>"` routes and delivers the same way, prints
  the reply, and waits up to `--timeout` seconds (default 120).
- **Address a session directly.** `everett send --to <session-id-or-project> "<request>"`
  skips routing. `--dry-run` shows the chosen transport without sending.

Delivery **mode** matters. The default `ping` is immediate: a verified idle local session is
resumed headless and returns its reply inline; paths that cannot deliver immediately **fail**
rather than silently queue. `--mode inbox` queues the message for the session's next hook or
polling pass instead — use it when you want pickup, not interruption. See
[Delivery policy](/message-delivery/) and [Agent support and wake](/agent-support/) for which
agents support live wake.

## 7. Read the reply

Replies use `reply_to` and land in the sender's inbox — the inbox of the session or terminal
that sent the request:

```sh
everett inbox --peek            # what is waiting, without marking it read
everett status <message-id>     # pickup state for one send
everett sent                    # requests you sent recently
```

Inside an agent, `everett_inbox` and `everett_status` do the same. Receipts are evidence, not
completion: `queued` means written, `picked_up` means the inbox was read, `replied` means a
correlated reply exists. A queued message is not a read receipt.

## Expected result

A request sent from Claude Code (or your terminal on its behalf) is routed to the matching
Codex session, delivered by ping or inbox, and the Codex session's reply returns to the
sender's inbox with a correlated message ID. Verified live on 2.4.0: an inbox-mode send was
picked up by the receiving session's hook and a `reply` returned a new message ID
(`m…` → `m…`). Headless resume delivery was verified in earlier release checks; per-agent wake
support is listed in [Agent support and wake](/agent-support/).

## Troubleshooting

- **Tools missing in the agent** — run `everett doctor`, then restart the client. MCP tool
  calls also need `everett login` to have succeeded in the same profile.
- **`everett_whoami` shows no `session_id`** — listing and routing still work, but other
  sessions cannot reply to this one yet. See [MCP caller identity](/mcp-identity/).
- **Send reports `ASK` or `NEW`** — nothing was delivered. Re-run `everett route` and either
  pick a candidate with `--to`, or pass `--spawn` to start a new session deliberately.
- **Message queued but never picked up** — the target has no injection hook or polling yet.
  Check [Troubleshoot delivery](/messaging-troubleshooting/) for the failed step.
- **Nothing works** — `everett doctor` prints the exact next command for each launcher.
