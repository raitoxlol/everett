---
name: everett-hermes
description: Inspect local coding-agent sessions and Everett's detected Hermes caller identity.
---

# Everett in Hermes

Use `everett_whoami` to check the identity Everett actually detects for this caller. Use
`everett_ls` for a read-only overview of recent local sessions; pass `harness: "hermes"` to
filter to Hermes. Do not choose a handoff target from the overview; use Everett's routing tools
when routing work.

Everett requires an account in the same local profile as its MCP process. Never invent a session
ID or claim a session is bound when `everett_whoami` reports that it could not detect one.

A shared Hermes gateway can expose the plugin tools to several conversations. When
`everett_whoami` reports an unbound caller, use Hermes's local terminal tool with its
normal approval settings. Check the session ID Hermes supplied to that command with
`printenv HERMES_SESSION_ID`. Stop if it is empty. Keep the commands in this Hermes turn.

Prefix every Everett command below with
`env EVERETT_HARNESS_NAME=hermes EVERETT_SESSION_ID="${HERMES_SESSION_ID:?missing Hermes caller}"`
in that terminal invocation. This selects the current Hermes turn even when another agent launched it.

- Send to the chosen peer with `everett send --to <full-target-id> --mode inbox --json "request"` and save the returned message ID.
- In the receiving session, run `everett inbox --json` and answer with `everett reply <message-id> "reply"`.
- On the sender, run `everett inbox --reply-to <message-id> --json` and `everett status <message-id> --json`. Check the reply author and pickup status.

If the terminal tool or its session binding is unavailable, report that limitation and use
the plugin for discovery. A queued request requires the target to check its inbox.
