# Agent support and wake

This reference describes the code in the v2.2 candidate. A supported integration
does not mean every agent can be woken. Candidate changes require an updated
binary and, for Cloudflare changes, an updated Worker.

## Local agents

Everett discovers seven local harnesses. Each supports headless resume and new
session creation through its installed CLI. Resume starts a separate CLI run
against an idle session. It does not inject a prompt into an attached agent.

| Agent | Session discovery | Inbox pickup in an open session | Wake an existing session |
| --- | --- | --- | --- |
| Claude Code | Claude project transcripts | `UserPromptSubmit`, `PostToolUse`, or MCP polling | Default local `ping` verifies and nudges an existing background session; compatibility `--wake`; optional receiver policy |
| Codex | Codex session transcripts | Supported Codex hooks or MCP polling | No native wake |
| OMP | OMP session files | Everett extension watcher, agent start, tool results, or MCP polling | Attached idle OMP 18.12.2 wakes through its live Everett extension; local `ping`/`auto` requires a fresh per-session capability |
| Pi | Pi session files | MCP polling; the Pi extension binds identity and records activity | No native wake |
| Hermes Agent | Profile session databases | MCP polling | No native wake; platform chats are list/route only |
| Grok CLI | Grok session files | `PostToolUse` or MCP polling | No native wake; Grok CLI is separate from Grok Bot |
| Devin CLI | Devin database and transcripts | MCP polling; Devin hooks bind identity but do not inject inbox messages | No native wake |

Hooks require installation and support from the running harness version. They
consume messages when the agent reaches a hook event. They do not start a new
turn in an idle agent, except for the OMP extension watcher described in the
[delivery policy](message-delivery.md#attached-omp-sessions). `hooked: true` reports integration support, not an
observed pickup. Hermes resume applies to interactive CLI sessions, not Telegram
or Discord chats.

T3 Code is an overlay on its Claude, Codex, or Grok sessions. Everett can discover
those threads and queue inbox messages. It refuses CLI resume and native wake
because T3 owns the resume point. The client must bind the provider session and
poll MCP, or use supported underlying hooks. Actual T3-client acceptance remains
separate from the synthetic CLI/MCP tests.

## Cloud agents

| Agent | Connection and delivery | Wake |
| --- | --- | --- |
| OpenAI dots (`openai-dot`) | Explicit registration and owner-authorized MCP gateway; inbox and reply tools | Optional owner webhook endpoint. Native OpenAI MCP Events wake is not implemented in this candidate |
| Grok Bot (`grok-bot`) | Explicit registration and owner-authorized MCP gateway; inbox and reply tools | Optional owner webhook routine configured through `receiver-webhook.json` |
| Hark | Compatibility unverified; no dedicated adapter | Unverified |
| Muse | Compatibility unverified; no dedicated adapter | Unverified |

External registrations do not create a provider session. A successful webhook
POST shows endpoint acceptance. Verify that the provider reads the correlated
inbox message and replies before claiming wake works for that connection.
See [dots setup](openai-dots.md) and [Grok Bot webhook wake](grok-bot.md#wake-a-grok-bot-with-a-webhook-routine).

## Three delivery paths

1. **Ping a local agent.** Default `send` runs an idle target headless or sends
   a verified native nudge to an attached local Claude background session.
   An attached OMP session with a live extension capability accepts the request
   for an idle watcher nudge, reported separately as `extension_pending`.
   Unsupported immediate transports fail without a queue fallback. T3 threads,
   external registrations, and remote transports require explicit legacy inbox mode.
2. **Wake Claude.** Local `send --to FULL_UUID --wake` requires an interactive
   terminal, Claude Code 2.1.285 or newer, and a verified idle background session
   in its recorded project directory. MCP cannot request this flag. A receiving
   device can separately enable `receiver-wake.json` for an exact Claude
   session and directory. That policy acts after an authorized same-owner relay
   import. It excludes T3, SSH, external content, replies, and cross-owner grants.
3. **Queue and optionally nudge a cloud agent.** `--mode inbox` explicitly
   queues for hooks or polling. A matching owner webhook rule can also nudge a
   registered dot or Grok Bot. Its send receipt keeps `wake: false` because it
   did not invoke Claude native wake.

Cloudflare is the active hosted transport. Device names and same-owner trust
select and authenticate a destination. They do not grant wake permission.
SSH and Cloudflare deliver inbox messages without receiver wake.

## Receipts and verification

`queued` means retained for pickup. Encrypted relay `delivered` means authorized
device ingress. `claimed` means inbox pickup. `replied` means a reply was
enqueued or observed. None proves the requested task succeeded.

The implementation boundaries are in [session discovery](../everett-rs/src/registry.rs),
[send and resume](../everett-rs/src/send.rs), [hook installation](../everett-rs/src/install.rs),
[local Claude wake](../everett-rs/src/claude_wake.rs),
[relay Claude policy](../everett-rs/src/receiver_wake.rs), and
[external webhook policy](../everett-rs/src/receiver_webhook.rs).

The CLI/MCP fixtures cover dispatch, refusal, inbox pickup, and correlated
replies. Claude wake tests use controlled process/PTY fixtures, and webhook tests
use a local endpoint. These checks do not establish a live provider wake on
every installation. Use the [receiver acceptance guide](cloudflare-native.md#real-public-relay-receiver-acceptance)
for a real two-device check and the [delivery policy](message-delivery.md) for
receipt details.
