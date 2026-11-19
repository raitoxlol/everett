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
