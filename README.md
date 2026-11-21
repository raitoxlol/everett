# Everett

Route requests, messages, and shared memory across your coding-agent sessions—from a CLI or MCP client.

**Works with:** Claude Code · Codex · OMP · Pi · Hermes Agent · Grok CLI · Devin CLI.
T3 Code threads are recognized through their underlying harness sessions.
OpenAI dots and Grok Bot connect through registered external MCP inboxes.
Hark and Muse compatibility is unverified.

The [Everett hosted plugin](plugins/everett/README.md) builds one versioned package
around one existing owner app, with no desktop interaction during builds.
Everett Local packages the installed runtime for local plugin clients. ChatGPT
custom MCP connections use the gateway over HTTPS or Secure MCP Tunnel. See
[ChatGPT setup, supported modes, and verification](docs/chatgpt.md).

**Wake support:** Claude Code has native local wake and an optional relay receiver policy.
Dots and Grok Bot can use owner-configured webhook nudges. Other harnesses use
headless resume or inbox pickup. See [agent support and wake](docs/agent-support.md).

The maintainer waived the v2.2.0 live proof bar; live same-org, cross-org, and
external-binding evidence is not recorded. See the [proof-bar status](docs/v2.2-proof-bar.md).
See also [Grok Bot 0.77 reachability](docs/grok-bot.md#grok-bot-077-reachability).

## Quickstart

Everett is one native binary for macOS and Linux. Have at least one harness CLI on your PATH.
Local CLI sessions, routing, inboxes, hooks, MCP registration, and SSH peers need no account.
Calling Everett MCP tools requires `everett login` in the same Everett profile.
The hosted relay is `https://relay.raitox.tech`. Account identity stays
`https://everett.raitox.tech`. Hosted enrollment follows the deployed service's
account policy. The repository's public deployment profile enables separate
GitHub account namespaces; self-hosted deployments can restrict enrollment to one owner.

To install and configure local agents without browser sign-in:

```bash
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh   # ~/.local/bin
export PATH="$HOME/.local/bin:$PATH"
everett onboard --yes --no-backfill
everett doctor
```

Or via Homebrew or Cargo:

```bash
brew tap raitoxlol/tap && brew install everett
cargo install --git https://github.com/raitoxlol/everett everett   # from source
```

`onboard --yes --no-backfill` backs up and applies the default hook and MCP registrations
for detected agents without an interactive review. It skips older session cards and does
not sign in, require a routing key, or schedule nightly merges. To inspect registration
examples without changing files, run `everett install-hooks` and `everett install-mcp`.
Those commands can include agents that onboarding has not detected; they are not an
exact preview of its selected changes.

Onboarding detects installed CLIs even before their first session. It backs up and registers
hooks and MCP for detected Claude Code, Codex, OMP, and Grok installations. It also installs
the Pi extension and registers Devin MCP when those clients are detected. Run
`everett install-hooks --devin --apply` to bind Devin MCP calls to their sessions.
When Hermes is detected, the wizard prints instructions to install and enable the native
[Hermes plugin](plugins/everett-hermes/README.md); it does not install the plugin itself.
`everett install-mcp --hermes` prints a separate manual stdio entry (it does not edit Hermes
configuration and does not support `--apply`). The printed entry has no session-ID override, so it
relies on process detection, which can bind one unambiguous interactive Hermes CLI session in the
server's directory. An explicit `EVERETT_SESSION_ID` or `HERMES_SESSION_ID` in the MCP server's
environment binds that one session instead; it does not provide the native plugin's per-session
gateway binding. Prefer the plugin for native Hermes features, and do not configure both because
they expose duplicate tools.
Restart configured clients and run `everett ls`. Doctor checks registered launchers and lists next steps.
Run `everett login`, restart each MCP client, and ask it to call `everett_ls`. A returned list,
even empty, verifies the tool.
See [MCP caller identity](docs/mcp-identity.md).
No router key or model call is needed for this setup. Codex may ask you to enable/trust its hooks.

`install.sh` verifies the release checksum; `EVERETT_VERSION=vX.Y.Z` pins a release and
`EVERETT_INSTALL_DIR` changes the target directory. Rerun it to update.

From source (needs a Rust toolchain): `cargo install --git https://github.com/raitoxlol/everett everett`,
or `cargo install --path everett-rs` from a checkout.

**Upgrading from the retired Python package (pipx).** Install the binary, then run
`everett install-hooks --apply` and `everett install-mcp --apply`. They back up each file and replace
the Python hook commands, `-m everett mcp` launchers (and their `PYTHONPATH`), and the OMP extension with
the native binary. Then `pipx uninstall everett-sessions`.

### Setup options

Run `everett` in a terminal to start onboarding on first use. Successful setup records that the
wizard was applied, not that an agent has loaded its tools. Cancellation or an apply error leaves
first-run setup pending. Later bare runs show help. Run `everett onboard` to repeat setup. Explicit
commands and noninteractive bare runs do not prompt.

The welcome says "Local setup needs no Everett account. Sign in to use Everett MCP tools."
Its primary action is "Choose setup path"; the next screen is "Choose how to connect".
Press `l` to continue locally. When the wizard offers sign-in, Enter starts device approval.
The wizard then guides you through agent selection, per-agent setup, review and apply, and a
copyable instruction prompt. For sign-in, the CLI opens the approval page locally, prints a
URL and code usable on another device, waits for
approval, saves its private credential, and verifies its identity with the server before continuing.
SSH sessions keep the URL/code path. Fresh interactive setup defaults to
`https://relay.raitox.tech` for the network endpoint, with account identity
`https://everett.raitox.tech`. That service accepts GitHub accounts into separate relay namespaces.
Explicit account configuration is preserved. After a failed or cancelled sign-in, press
`l` in the terminal wizard to continue locally. To skip browser sign-in entirely, use the
account-free quickstart above. See [account setup](docs/auth.md).

Local choices use detected sessions and installed CLIs. Cloud and VM tabs include Dots, Grok Bot,
Hark, Muse and another computer. Each selected local agent gets a guide for its actual settings,
restart and tool verification. Cloud/VM choices remain pending until their own connection is
completed. Everett account approval, local hooks/MCP and external-client OAuth are separate.
Muse and Hark compatibility is unverified.

Space selects a local agent's supported integrations together. Press `d` for individual controls and
file paths. At review, `d` shows exact changes and `v` opens Advanced. Routing keys, nightly merges,
and older session cards live there. Cards and nightly merges start off in the interactive wizard.
`--schedule-merge` explicitly enables the schedule. Plain prompts and `--yes` retain their defaults.

The result distinguishes apply errors from settings saved. Loading and using Everett tools in the
chosen agent still needs verification. The final prompt asks each agent to read its appropriate
instruction Markdown and show a scoped addition before applying it. The wizard never edits those
instruction files. The prompt is printed after leaving the full-screen UI so it can be selected and
copied. Results, guides and the prompt scroll with `j` and `k`.

`q`, Escape or Ctrl-C cancels before apply; completed account approval is retained. `d` on welcome
opens the optional explanation, with Overview, Local, Cloud and VM tabs on keys `1`-`4`; the chooser
uses `1`-`3`. A usable terminal starts at 54 columns and 14 rows. Smaller terminals ask you to resize.
The large balloon layout needs 76 columns and 22 rows; the compact balloon appears from 60 columns.
Without a usable terminal, Everett uses plain prompts. While entering a key, Escape or Ctrl-C cancels.

The welcome has an animated balloon and multiple colors. Set `EVERETT_NO_ANIM=1` to disable motion
or `NO_COLOR=1` to disable color. Input stays available during animation. To review with synthetic
sessions in a disposable home, build the debug binary and run `sh scripts/demo-onboarding.sh`.

[Captured terminal journey](docs/assets/onboarding/onboarding-motion.gif) uses actual PTY cells
from synthetic sessions, rendered with Menlo on a dark canvas. It is not a Termius screenshot.
This historical capture predates the current welcome and account wording. The binary
`onboarding-motion.gif` and `onboarding-summary.png` under `docs/assets/onboarding/`
need recapture: the summary now directs users to sign in if needed and call `everett_ls`,
not just list tools. The text above describes the current flow.

`--yes` explicitly applies defaults without prompting:

```bash
everett onboard --yes                       # apply every default, non-interactively
everett onboard --yes --span-days 7         # backfill cards for the last 7 days (default 3, 1-30)
everett onboard --yes --no-backfill         # skip card backfill
everett onboard --yes --no-mcp              # skip MCP registration
everett onboard --yes --jev-key-env MY_KEY  # save a Jev key from an env var (validated first)
everett onboard --yes --schedule-merge      # macOS/Linux: also schedule nightly `everett trunk merge`
```

**Smarter routing (optional).** Jev ([typesafe.ai](https://typesafe.ai)) picks the right session
when many are running; without it, routing uses local BM25 matching without network calls.
Harness resumes and LLM memory merges use their model providers. The onboarding step offers
"paste a key" or "skip" -- if a key is already found
(`TYPESAFE_API_KEY`, `~/.everett/config.toml`, or `~/.hermes/.env`), it shows "Jev key found ✓
(source)" and defaults to skip. A pasted key is validated with one test route call (5 s timeout);
on failure you can keep it anyway or skip. The key is masked while typing and never echoed or
logged, and is saved as `typesafe_api_key` in `~/.everett/config.toml` (created `chmod 600`,
preserving every other key). The same step has a "merge shared memory nightly" toggle -- see
[Nightly auto-merge](#nightly-auto-merge) below.

Everett reads the session stores the harnesses already write: `~/.claude/projects`,
`~/.codex/sessions`, `~/.omp/agent/sessions`, `~/.pi/agent/sessions`, `~/.grok/sessions`,
Hermes' `state.db` files, the Devin CLI's `sessions.db` (plus `transcripts/*.json`),
and T3 Code's `~/.t3/userdata/statev2.sqlite` (legacy `state.sqlite` fallback). Every database is opened read-only.

Cloud agents use explicit local registrations and the native restricted MCP gateway, not fabricated
provider transcripts or API-model substitutes. See [OpenAI dots setup](docs/openai-dots.md),
[Grok Bot registration, polling, and webhook wake](docs/grok-bot.md), and [gateway permissions](docs/gateway.md).
Owner-side connector setup is required; queued messages do not prove pickup. A matching owner
webhook is the supported Grok Bot wake path: [Wake a Grok Bot](#wake-a-grok-bot).
Manage local bindings with `everett external add`, `list`, and `remove`.
Keep an OpenAI tunnel client forwarding with `everett tunnel watch`; a ready
health check does not show a stuck control-plane poll. See the dots doc.

## Demo

### Local dashboard (Rust binary)

```bash
cargo install --path everett-rs
everett dashboard
everett --hours 168 dashboard --port 7347 --no-open
```

The Mac-first dashboard opens automatically on macOS and prints its loopback URL
on other platforms. It reads the same provider stores and Everett cards as
`everett ls`, showing up to 80 recent non-automated sessions. Search by project,
card, or session ID; filter by harness or state; expand a row for the full card
body, provider/source details, and copyable inbox-send, local-route, and events
commands. Refresh rescans local data. CLI listings retain their compact card
summaries.

It is read-only: copying a command does not execute it, deliver a message, or
wake an agent. “Active” is Everett's existing process/recency heuristic, not a
verified agent heartbeat. The server binds only to `127.0.0.1`, rejects foreign
Host headers and write methods, and sends no session data to a hosted service.
Stop it with Ctrl-C.

**Future direction:** a hosted dashboard could expose cross-device local activity.
Optional account auth and inbox-only SSH/Cloudflare peer transports exist, but dashboard
hosting/session synchronization (including a custom-domain frontend on Vercel) do not.
A Rust local-data service would still be required; a static website cannot read another
device's session stores.

This is example output (paths and ids are shortened):
CLI ages in `everett ls` and `everett inbox` use minutes below one hour, hours
below 48 hours, and days from 48 hours onward (for example, `2d`).

```text
$ everett ls
● claude  2m  ~/src/api        API: adding retry/backoff to upload client; next: integration test
○ codex  40m  ~/src/web        Web: dark-mode toggle in settings; state: styles done, wiring store
○ omp     3h  ~/src/infra      Infra: Terraform module for the staging bucket

$ everett route "the upload retries should cap at 5"
SESSION  (choice=s0, confidence=0.84, router=local)
  → [claude] ~/src/api — API: adding retry/backoff to upload client
  cd ~/src/api && claude --resume 3f1c… 'the upload retries should cap at 5'

$ everett send "the upload retries should cap at 5"
SENT  [claude] ~/src/api — API: adding retry/backoff to upload client
Done. MAX_RETRIES is now 5 and the backoff test covers the cap.
```

`●` means running (its id is in a live process, or its file changed in the last 10 minutes).

## Commands

| Command | What it does |
|---|---|
| `everett control read --session ID [--messages N]` | Reads recent messages from a local session by ID or unambiguous prefix. Uses its T3 thread when mapped, otherwise its supported harness transcript. |
| `everett gateway --config FILE [--transport stdio\|http] [--port 8788]` | Runs the owner-bound gateway; see [gateway setup](docs/gateway.md). |
| `everett gateway-permissions --file FILE [--all\|--allow TOOL,...\|--deny-all]` | Inspects or updates an owner connection's tool grant. |
| `everett external add / list / remove` | Manages registered external inboxes; see [gateway setup](docs/gateway.md). |
| `everett tunnel watch --env-file FILE` | Maintains the configured Secure MCP Tunnel; see [ChatGPT setup](docs/chatgpt.md). |
| `everett identity diagnose [--json]` | Explains account and session identity provenance with read-only account verification. |
| `everett relay diagnose` | Prints redacted local relay diagnostics without online account verification. |
| `everett control read THREAD [--messages N]` | Reads recent T3 thread messages as JSON for an owner-local caller. Defaults to 10 messages and clamps the count to 1–50. |
| `everett control threads [--project P]` | Lists T3 Code threads as JSON through the local T3 CLI. Accepts an optional project filter and requires an owner-local caller. |
| `everett ls [--json] [--all] [--harness H]` | Recent sessions from every harness, one line each. `--all` includes scripted runs. |
| `everett ls --peer <name> [--json]` | Sessions on one paired SSH device, with `peer/harness/session-id` IDs. |
| `everett devices [--json]` | List active Cloudflare device bundles. |
| `everett peer add <name> --ssh user@host [--port N] --binary /absolute/everett --host-key SHA256:... --device-key SHA256:...` | Pair at an explicit SSH address; require both confirmed fingerprints. |
| `everett peer serve --client ID` | Restricted sshd endpoint; no hosted address publication. |
| `everett peer status <name> <message-id>` | SSH polls/imports replies; relay queries queued/delivered/claimed/replied/expired state from signed observations (not task completion). |
| `everett relay` | Receive encrypted messages through Cloudflare push notifications and authenticated HTTPS inbox reads. Claude session wake and external webhook posts follow the local owner policy. |
| `everett relay register / devices / whoami` | Register this agent's keys and display name, list active key bundles (maximum 256), or verify account/agent authorization. |
| `everett relay devices --names` | Hosted Cloudflare enrollment labels beside device IDs and fingerprints. Advisory only; the default JSON bundle list is unchanged. |
| `everett relay expose SESSION [--harness H] [--remove]` | Opt in to sharing this exact local session's card, or withdraw it. The backend derives its authenticated author and agent. |
| `everett relay authors` | List exposed authenticated author/session cards across the account, without device pairing. |
| `everett relay rename NAME` | Update this Cloudflare device's hosted label. Device ID, keys, and pins stay the same. |
| `everett relay revoke-agent ID / revoke-member SUBJECT / rotate` | Revoke this or an admin-selected agent, remove org membership (admin), or rotate this agent's relay keys using its old signing key. |
| `everett relay register-inbox / grant-inbox / revoke-inbox-grant / resolve-inbox` | Claim a `LOCAL@SCOPE` cross-org inbox, allow an exact sender owner/device to reach it, revoke a grant, or resolve an address to its verified recipient bundle. `--to LOCAL@SCOPE` with `--mode inbox` sends to it. |
| `everett access [show \| set \| allow \| deny \| remove \| reset]` | Inspect or edit local cross-org permission settings in `~/.everett/access.json` (`mode`, `external-send`, `max-hops`, local allow/block lists; `--sync` pushes `grant-inbox` to the relay; alias `everett permissions`). |
| `everett peer add-relay <name> --device ID --fingerprint HEX [--grant GRANT_ID]` | Pin a device on the selected relay after out-of-band bundle confirmation; fingerprint is raw 64-character lowercase hex, not the SSH `SHA256:` format. |
| `everett peer identity / list / remove <name> / revoke <client-id>` | Inspect device keys/peers; remove a remote authorization or revoke a local tagged client key. |
| `everett dashboard [--port 7347] [--no-open]` | Native Rust binary only: read-only local browser dashboard with session cards, filters, activity state, and copyable CLI actions. |
| `everett route "<text>" [--router local\|jev] [--json]` | Picks the session a request continues: `SESSION`, `NEW`, or `ASK`, with a confidence and the resume command. It never sends. |
| `everett send "<text>" [--dry-run] [--timeout S] [--router …]` | Routes and pings immediately. Idle local sessions run headless; verified local Claude background sessions receive a native inbox nudge. Unsupported transports fail without queue fallback. `NEW` and `ASK` send nothing unless explicitly resolved. See [Live delivery](#live-delivery). |
| `everett send … [--mode ping\|auto\|resume\|inbox] [--wait S]` | Default `ping` dispatches immediately or fails. Explicit `inbox` is legacy queue delivery for hooks, polling, or remote transports. `auto` and `resume` retain compatibility behavior. `--wait S` applies to explicit inbox delivery. |
| `everett status MESSAGE_ID [--json]` | Check a local request you sent. The original message ID shows pickup and any reply, even after the inbox has been read. |
| `everett sent [--to ID] [--json]` | Recover up to 50 recent local request receipts for the current sender, newest first. `--to` filters by exact recipient session ID. |
| `everett retry MESSAGE_ID [--json]` | Nudge the original T3 thread for a still-queued ping you sent. Reuses the inbox request and message ID. Check pickup with `everett status MESSAGE_ID`. |
| `everett send --to <full-Claude-session-UUID> --wake [--wait S] "<request>"` | Queues the request, then explicitly wakes an existing local Claude Code background session through a PTY and waits for a reply linked to that inbox message. Requires an interactive local terminal and Claude Code 2.1.285 or newer. The sender cannot request this native wake through `auto`, MCP, gateway, remote, or T3 Code sends. A separately enabled receiver policy can wake Claude after a relay import. |
| `everett reply <message-id> "<text>"` | Answers an Everett message; the reply goes to the sender's inbox. |
| `everett proof wake --to <session> [--timeout S] [--json] [--dry-run]` | Checks one local ping path with a random nonce canary and a correlated inbox reply. Default timeout is 120 seconds. Dry-run resolves the target and transport without sending. See [wake proofs](docs/message-delivery.md#wake-proofs). |
| `everett inbox [--session S] [--peek] [--message ID] [--reply-to ID] [--json]` | Read and mark matching inbox messages. Use `--message` for an inbox item's own ID or `--reply-to` for replies to the original request ID. |
| `everett event done\|blocked\|needs-input\|info "<msg>" [--project P]` | Records what this session is doing (see [Events](#events)). |
| `everett events [--since 24h] [--session S] [--json] [--check]` | Recent events across sessions. `--check` also runs the blocked-too-long escalation. |
| `everett subscribe <session\|project> [--as S] [--remove]` | Delivers another session's (or a project's) events to your inbox. |
| `everett send --to <id-prefix\|name> "<text>"` | Skips routing and delivers to one session. The target is an exact session ID or T3 thread ID, a unique session ID prefix, an exact title (case-insensitive), the card's name (`Atlas: …` → `atlas`), or the project name (repository name for T3/Claude worktrees, otherwise the working folder's name). A lone card named only after its project does not hide other sessions in that project. If more than one session matches at a lookup step, Everett lists them and sends nothing. |
| `everett send --spawn [--dir D] [--harness H] "<text>"` | When routing says `NEW` with confidence ≥ 0.6, starts a new headless session and prints its reply and new session id. The directory defaults to the folder of the best-matching session, else the current one. |
| `everett cards [--regenerate-auto] [--dry-run] [--hours N]` | Card coverage per harness; optionally refresh existing automatic cards. |
| `everett skills [QUERY] [--match TASK] [--get NAME] [--source S] [--shortlist] [--json]` | The skill hub over every harness's `SKILL.md` playbooks. `--match` prints the one skill that fits a task (exit 1 when none does); `--get` prints a named skill from the best source that has it; `--shortlist` shows what sessions in this project are told up front; a bare query lists. |
| `everett onboard [--yes] [--span-days N] [--no-backfill] [--no-mcp]` | Human setup TUI (welcome, account approval, agents, guides, review/apply and instruction prompt; optional settings under Advanced); `--yes` runs it non-interactively for scripts. |
| `everett install-hooks [--claude] [--codex] [--omp] [--grok] [--pi] [--devin] [--apply]` | Prints the hook registrations. `--apply` backs up the file, then merges idempotently. |
| `everett mcp [--harness H]` | Runs the stdio MCP server (harnesses start it; see below). Tool calls require `everett login`. |
| `everett install-mcp [--claude] [--codex] [--omp] [--grok] [--devin] [--pi] [--hermes] [--repair] [--apply]` | Prints or registers MCP. Pi uses its extension; `--hermes` prints a manual stdio config only and cannot apply it. Use the native Hermes plugin for native features. `--repair --apply` refreshes stale registrations. |
| `everett doctor` | Installed CLIs, stores, hooks, live stdio/MCP registration checks, cards, router, and account diagnostics; SSH revocation remains explicit. |
| `everett learn "<fact>" [--project P] [--scope global\|project]` | Queues a fact for shared core. Add `--ttl-seconds N` (1 to 31536000) to keep it only in local memory search until expiry. Secrets are rejected. |
| `everett trunk merge [--llm claude\|codex\|none] [--dry-run]` | Distills the inbox into the shared core. |
| `everett core [show\|edit-path\|history] [--project P]` | Shows the core a new session in this folder receives, the file to edit, or past merges. |
| `everett core search "<query>" [--project P] [--json]` | Prints up to five matching learned facts from global memory and the current or named project. Includes unexpired temporary facts. Use `--json` for fact scope and expiry. |
| `everett trunk [view] [--dry-run]` | Writes the session list as a Markdown note into your notes vault (optional). |
| `everett trunk schedule [--at HH:MM] [--remove] [--llm claude\|codex\|none] [--apply]` | Prints (default) or, with `--apply`, installs/removes a macOS LaunchAgent or Linux systemd user timer that runs `everett trunk merge` nightly (default 04:00, `--llm claude`). See [Nightly auto-merge](#nightly-auto-merge). |
| `everett login [--device NAME] [--open] [--force] [--json]` | Native Rust binary only: signs this device in to an Everett account with Cloudflare native device approval (see [Accounts](#accounts)). |
| `everett whoami [--json]` | Verifies the saved Cloudflare credential online. |
| `everett logout [--revoke-device] [--json]` | Deletes the local Cloudflare credential; optionally confirms hosted device revocation first. |
| `everett devices revoke <device-id>` | Permanently revokes self or a same-owner device as admin, and removes matching local peer configuration and pins. SSH authorizations must be revoked separately. |

Global option: `--hours N` sets the look-back window (default 72).
It also applies to `control read --session`; increase it for older sessions.
Preview automatic-card regeneration for the last week with `everett cards --regenerate-auto --dry-run --hours 168`; omit `--dry-run` to write updates. Regeneration updates only existing AUTO-marked cards for supported Claude Code, Codex, and Grok CLI sessions, not agent-written or missing cards.
MCP `everett_control` resolves session IDs within a fixed 168-hour window.
Transcript reads stream JSONL histories and return a bounded tail. Devin ATIF JSON
transcripts support recent user/assistant reads; Devin SQLite sessions do not yet support reads.

### How delivery works

| Harness | Resume (send) | New session (--spawn) |
|---|---|---|
| Claude Code | `claude --resume <id> --print <text>` | `claude --session-id <new uuid> --print <text>` |
| Codex | `codex exec resume <id> <text>` | `codex exec --skip-git-repo-check -o <file> <text>` |
| OMP | `omp -r <id> -p <text>` | `omp -p <text>` |
| Pi | `pi --session <file> -p <text>` | `pi -p <text>` |
| Hermes Agent | `hermes -p <profile> chat --resume <id> -Q -q <text>` | `hermes chat -Q -q <text>` |
| Grok CLI | `grok --resume <id> -p <text>` | `grok --session-id <new uuid> -p <text>` |
| Devin CLI | `devin --resume <id> --print <text>` | `devin -p <text>` |
| T3 Code | Inbox; hooks where supported, otherwise explicit MCP polling; no CLI resume | not supported |
| OpenAI dots / Grok Bot | Registered external inbox via owner-authorized MCP; no provider resume. Optional owner webhook nudge: [Wake a Grok Bot](#wake-a-grok-bot). | not supported |

The harness appends the request and the reply to that session's history. Hermes sessions that belong to a chat platform (Telegram, Discord, and so on) are listed and routable, but `send` refuses them, because a CLI resume would not reach that chat. Scripted Hermes runs (cron, oneshot, webhook) are hidden like other automated runs. Grok sessions are read from `~/.grok/sessions/<url-encoded cwd>/<id>/` (`summary.json` for id, folder, and title; `prompt_history.jsonl` for the typed requests). A Grok session counts as running while `~/.grok/active_sessions.json` names it with a live process id. Headless `grok -p` runs are hidden like other scripted runs. Devin CLI sessions are read from `sessions.db` under `$DEVIN_HOME`, else the platform data dir (`~/Library/Application Support/devin/cli` on macOS, `~/.local/share/devin/cli` on Linux): the `sessions` table gives id, folder, title, and activity; `message_nodes` gives the user asks when the CLI build has it, otherwise `transcripts/<id>.json` carries the listing. Hidden sessions are skipped. Devin has session-binding hooks but no inbox-injection hooks. Idle sessions use headless `devin --resume <id>`; open sessions require explicit MCP inbox polling.

T3 Code is a desktop GUI that runs Codex, Claude Code, and Grok underneath. Everett marks the
matching session with source `t3code` (`[t3code]` in `ls`) and uses the thread title when needed.
Active projections with a real provider ID also supply sessions when the local provider
transcript is absent. Current v2 and legacy stores are supported; see the
[T3 discovery and polling guide](docs/t3code.md).
Owner-local T3 `ping` and `auto` send a fixed inbox nudge through `t3 thread send`.
Explicit `--mode inbox` (MCP `mode: "inbox"`) queues without that nudge.
Hooks can inject a queued message only if T3's provider runtime
actually loads and runs them; queuing alone does not prove pickup. Otherwise the receiving
thread must have `EVERETT_SESSION_ID=<underlying-provider-session-id>` in that thread's MCP
launch environment, then calls `everett_inbox()` and answers with
`everett_send(reply_to="<message-id>", text="…")`. A model-supplied `session_id` cannot create
that binding.
Verify the Everett tools are available inside that T3 thread, and use `everett_core` for shared
memory. CLI resume is refused because it would fork T3's own resume point.

Sessions that Everett spawns are recorded in `~/.everett/spawned.jsonl`, so they show in `ls` even though they ran headless.

Safety rails for agents that call `send`:
- **Hop count.** Each delivery sets `EVERETT_HOPS` in the harness environment. The hop limit is off by default; turn it on with `hop_limit = "on"` in `~/.everett/config.toml` to refuse sends past 3 hops (exit 7). See [docs/hop-limit.md](docs/hop-limit.md).
- **No self-send.** When the caller's session id is known (`EVERETT_SESSION_ID`, or the harness's own session variable), Everett refuses to send to that session.
- **Spawning is explicit.** `NEW` starts a session only with `--spawn`.

Exit codes: 2 bad input, 3 no Jev key with `--router jev`, 4 session stayed busy or automatic delivery has no safe ping/wake path, 5 harness did not start or timed out, 6 harness failed, 7 hop limit reached.

## Wake a Grok Bot

Everett queues a registered Grok Bot inbox. It does not resume the bot. Explicit
`--mode inbox` writes the text. A matching `receiver-webhook.json` rule then POSTs
a nudge that omits the inbox text. HTTP 200 means the routine accepted a run.
`wake` stays false. Copy-paste commands, the policy file, and cross-device relay
are in [Grok Bot webhook wake](docs/grok-bot.md#wake-a-grok-bot-with-a-webhook-routine).
The same file can name `openai-dot`.

On the computer that holds the inbox:

1. Sign in and register the inbox with `everett external add --id ext-grok-acceptance --harness grok-bot --title 'Grok Bot inbox'`.
2. Create a Grok Bot webhook routine and store its URL and Sender key in separate private files with mode `0600`.
3. Run `everett relay webhook ext-grok-acceptance --harness grok-bot --url-file /absolute/webhook.url --bearer-file /absolute/webhook.key`. Everett derives the owner and binds the rule to this device's inbox.
4. Bind the routine's MCP to that inbox through a reachable Remote HTTPS [`everett gateway`](docs/gateway.md). Tell it to call `everett_inbox`, then `everett_send` with `reply_to`.
5. Send `everett send 'check inbox' --to ext-grok-acceptance --mode inbox --json`. Expect `"wake":false` and webhook `"state":"accepted"` with HTTP 200, then verify the routine calls `everett_inbox`.

## Cloudflare native relay

With `[auth] provider = "cloudflare"`, native account approval and encrypted relay use
the same separately approved device identity. Register both computers and run
`everett relay` on both. Devices on the same owner account trust active signed bundles
automatically. Use `everett relay devices --names`, then send to a unique name such as
`--to 'Max NUC/codex/SESSION_ID'`. No fingerprint exchange or `peer add-relay` is required
for same-owner Cloudflare messaging.
Those remote sends now require explicit `--mode inbox`; there is no synchronous remote
ping receipt. The listener uses an outbound hibernating WebSocket for fixed notifications, then reads
the encrypted inbox over authenticated HTTPS. Healthy quiet connections make no scheduled billable
inbox requests. The listener stages ciphertext durably, reauthorizes before inbox
publication, and retries temporary failures. Deploy the matching Worker before updating receivers.
An older Worker fails explicitly; `EVERETT_RELAY_TRANSPORT=poll` enables compatibility polling
with a recurring request cost. See [request costs](docs/cloudflare-usage.md). Receiver-side Claude wake is disabled by
default. An owner can opt in through the provider-local `receiver-wake.json` policy described
in [the native relay guide](docs/cloudflare-native.md). A registered Grok Bot or OpenAI dot
on that computer uses `receiver-webhook.json` in the same directory; see
[Wake a Grok Bot](#wake-a-grok-bot). Read, reply and revocation need their
own verification.

The actual native CLI passed local production-Worker acceptance and a physical Mac/NUC
send/read/reply/reconnect/revoke journey, with synthetic GitHub and private SSH forwarding
to the local Worker. The [2.0.0 release notes](https://github.com/raitoxlol/everett/releases/tag/v2.0.0) record successful hosted
first-run GitHub approval. Real public Mac/NUC delivery, idle Claude wake/reply and second-account isolation/revocation passed for 2.1.0.
Choosing sign-in during fresh interactive onboarding opens `https://relay.raitox.tech` for account approval.
Identity keys remain `https://everett.raitox.tech`.
That service supports public enrollment; messaging stays within each account. Existing account configuration
is preserved. Cloudflare's device protocol is custom, not RFC8628.
Cross-owner delivery uses named inboxes: `everett relay register-inbox LOCAL@SCOPE --harness H --session S`
claims the address (the first owner's SCOPE zone is never transferred), `relay grant-inbox` authorizes an
exact sender owner/device, and `everett send --to LOCAL@SCOPE --mode inbox` sends a hop-1 cross-org root
message whose replies correlate to the same grant. SSH pairing uses an explicit address; cloud-agent OAuth is separate.

Local harnesses that launch Everett with the same `HOME` and `EVERETT_HOME` use that
home's account configuration and device credential. They do not each need a separate
GitHub sign-in. `everett doctor` reports the selected provider's stored account without
verifying it online. With Cloudflare selected, `everett whoami --json` checks the account
with the provider.

## Human-readable relay device names

On the hosted Cloudflare provider, `everett relay devices --names` prints enrollment
labels beside device IDs and verified fingerprints. `everett relay devices` and
`everett relay devices --json` keep the signed JSON bundle array.

Labels come from the hosted account metadata already written during browser
approval. They are **not covered by the relay bundle signature** and do not
establish ownership. Duplicate names remain separate rows with separate IDs;
same-owner Cloudflare sends resolve a unique name to an active device ID and verify
its signed bundle. Duplicate or incomplete name directories fail closed. Use
`id:DEVICE_ID/HARNESS/SESSION` to select an exact device without name metadata.
Revocation and explicit pairing continue to use IDs. Different owners still require
explicit bilateral grants and fingerprint confirmation. No label lookup writes a pin.

`everett relay rename 'Office NUC'` updates this device's hosted label. Device
IDs, peer aliases, keys, pins, and signed bundles stay as they are. When the
backend has no label metadata, `--names` prints `(unnamed)` and says names are
unavailable.

## Cross-device peers (SSH or Cloudflare)

Cloudflare is the only hosted relay. Sign in on each device, run `everett relay`,
then send to an exact authenticated target. Same-account device bundles are pinned
on first use and key changes fail closed. Messages use X25519 + HKDF-SHA256 +
ChaCha20Poly1305 encryption and Ed25519 signatures; opted-in session cards are
available through `everett relay authors`. See [Cloudflare setup](docs/cloudflare-native.md)
and the [metadata and crypto limits](docs/relay-threat-model.md).

For **SSH**, pair once using the receiving machine's host/device fingerprints and absolute Everett path.
Existing noninteractive SSH login access is required for setup. The dedicated peer key can
only run Everett RPC: no shell, PTY, or forwarding. Host-key changes fail closed.

```sh
everett ls --peer linux-box --json
everett send --to linux-box/claude/<full-session-id> --idempotency-key task-42 --json 'Check the failing test'
everett peer status linux-box <returned-message-id>
```

SSH delivery is inbox-only; it never resumes/spawns a harness or silently queues on your
device. The receiving agent replies normally, but Everett retains that reply on the receiver
until the sender polls `peer status`. No reverse SSH pairing is needed for replies.
`queued` means inbox write, `delivered` means inbox pickup (`.done`), and `replied` means a
correlated reply exists. A timeout can leave remote delivery uncertain: retry the same explicit
idempotency key. Requests have a 64 KiB cap and 20-second deadline; hops follow `hop_limit` (docs/hop-limit.md).

SSH needs no registry, account sign-in, hosted relay, or Everett listener. See
[cross-device setup, protocol, revocation, and real-machine checks](docs/cross-device.md).

## Live delivery

Default sends use `ping`: an immediate headless run for an idle local session or a
verified same-session native Claude background nudge. Unsupported immediate transports
fail explicitly rather than silently queueing. A native nudge uses a durable inbox item;
if attachment fails afterward, the error identifies that retained item. Dispatch is not
inbox consumption or task completion.
See [agent support and wake](docs/agent-support.md) and the
[delivery policy and explicit queue exceptions](docs/message-delivery.md).

1. **Which path.** `send --mode ping` is the default in CLI and MCP. Native Claude dispatch requires a full UUID, supported version, and verified background state. Attached OMP 18.12.2 sessions accept local `ping`/`auto` through the running Everett extension when a fresh per-session capability proves support. The receipt reports `transport: "omp-extension"` and `extension_pending`; pickup and reply remain unconfirmed. T3 pings nudge the owning thread through `t3 thread send`; confirmed dispatch is not pickup. External agents, attached sessions without a supported wake transport, and remote transports require explicit legacy `--mode inbox`. `auto` and `resume` are compatibility options, not defaults. Remote provenance never permits a local provider wake or resume. See the [OMP behavior and trust boundary](docs/message-delivery.md#attached-omp-sessions).
2. **Explicit Claude wake.** `send --to <full-session-UUID> --wake` is a separate local CLI opt-in. Everett verifies the Claude version and exact background-session state, queues the original request, then attaches to that same session in a PTY with a fixed nudge to read and answer the queued inbox item. It reports the native prompt receipt separately from a real Everett reply. `--wake` rejects prefixes, routed destinations, remote sessions, T3 Code, relay-derived callers, and non-Claude sessions. The request remains recorded if the attach or reply wait fails, but Claude may already have read it. A timeout does not mean the request is still pending. MCP, gateway, and ordinary `auto` sends cannot request wake.
3. **Inbox.** `~/.everett/inbox/<session-id>.jsonl`, one message per line: `id`, `from` (the sender's session id, or `human` from a terminal), `from_harness`, `from_card`, `text`, `ts`, `reply_to`, `hops`, `kind` (`message`, `reply`, or `event`). Delivered ids go to `<session-id>.done`. Undelivered messages expire after 7 days.
4. **Injection.** The hook adds at most 5 messages (2,000 characters each, 6,000 in total) as context, marked as coming from Everett with the sender's session and card, and marks exactly those delivered. The rest arrive at the next hook. The hook reads local files only, takes about 40 ms, and prints nothing on any error.
5. **Replies.** The receiving agent answers with `everett_send(reply_to="<id>", text=…)` or `everett reply <id> "<text>"`. The reply lands in the sender's inbox and is injected there the same way. A sender can instead block on it: `everett send --wait 120 …` or `everett_send(wait=120)`. `everett inbox` / `everett_inbox` read an inbox directly.
6. **Safety.** With the hop limit on, a thread can go back and forth at most 3 hops (exit 7); it is off by default (docs/hop-limit.md). The calling session is never a target, and headless `everett send` runs (`EVERETT_SEND=1`) never consume inbox messages.

| Harness | Next turn | Mid-task | How |
|---|---|---|---|
| Claude Code | `UserPromptSubmit` | `PostToolUse` | `hookSpecificOutput.additionalContext` |
| Codex (0.155+) | `UserPromptSubmit` | `PostToolUse` | the same contract (checked against the hook schemas embedded in the 0.155.1 binary) |
| Grok CLI | no | `PostToolUse` | Grok discards an allowing `UserPromptSubmit` hook's context, so messages wait for the next tool call. A Grok session that runs no tool before stopping reads them with `everett_inbox`, or at its next tool call. |
| OMP | `before_agent_start`; idle inbox watcher | `tool_result` (steered in with `pi.sendMessage(…, {deliverAs: "steer"})`) | the Everett extension; idle nudges use fixed ID-only text and `triggerTurn: true` |
| Pi, Devin | MCP polling | MCP polling | messages wait in the inbox; the agent reads them with `everett_inbox` |
| Hermes Agent (native plugin) | `pre_llm_call`, `pre_verify` | `transform_tool_result` | Default hooks surface notices with validated message IDs; call `everett_inbox` to read contents. Opt-in `live_wake` sends ID-only local inbox nudges. |
| Hermes Agent (manual or portable MCP) | MCP polling | MCP polling | The agent reads waiting messages with `everett_inbox`; no native Hermes hooks or live wake. |

Grok also runs the hooks in `~/.claude/settings.json`. Everett's Claude delivery hook recognizes Grok's camelCase input and leaves a Grok session's messages alone on `UserPromptSubmit`, so nothing is marked delivered that Grok would drop.

Claude Code has its own cross-session messages (`<cross-session-message>` over sockets in `/tmp/cc-socks/`). Everett does not use that private sender protocol. Native Claude ping uses a verified same-session PTY attachment and a fixed inbox nudge instead.

## Events

Everett knows what each session is doing, not just what it was about.

- **Kinds.** `done`, `blocked`, `needs-input`, `info`. An agent reports one with `everett_event(kind, message)`, or anyone with `everett event blocked "waiting on staging key"`. Events are appended to `~/.everett/events.jsonl` and set the session's state in `~/.everett/state/<session-id>.json`, with `since` (when that state began). Messages are capped at 300 characters and secret-filtered.
- **Automatic.** The Stop hooks (Claude Code, Codex, Grok) read the turn's last assistant message (from the hook input, else the transcript) and classify it deterministically, with no LLM:
  - `blocked`: a closing line states "blocked", "stuck", "waiting on" / "waiting for", "can't proceed" / "cannot continue", or "unable to proceed". Negated ("not blocked", "no longer blocked", "unblocked") and questioned ("is it blocked?") mentions do not count.
  - `needs-input`: a closing line ends with a question mark, or asks the user to act ("need you to", "please confirm", "should I", "do you want", "would you like", "let me know if" …).
  - `done`: anything else, a clean finish.
  Only the last four prose lines count; code blocks, quotes, and tables are ignored. An automatic event that repeats the session's current state within 10 minutes, or with the same text, is dropped, so a chatty session does not spam.
- **Seeing them.** `everett ls` prefixes sessions that are blocked or need input (`⚠ blocked 32h: waiting on staging key · <card>`). `everett_ls` returns `state` and `state_kind` for every session. `everett events --since 24h` lists them.
- **Subscriptions.** `everett subscribe <session|project>` (or `everett_subscribe`) puts another session's or a whole project's events into your inbox, so they are injected at your next turn. Stored in `~/.everett/subscriptions.json`. A session never receives its own events.
- **The human.** `blocked` and `needs-input` also go to you. By default that is a macOS notification (`osascript`). Set `notify_command` to run your own command as well, for example one that pushes a message to your phone. It runs under `sh -c` with the one-line summary as `$1` and `EVERETT_EVENT_KIND`, `EVERETT_EVENT_TEXT`, `EVERETT_EVENT_SESSION`, `EVERETT_EVENT_PROJECT`, `EVERETT_EVENT_REASON`, and `EVERETT_EVENT_JSON` in its environment. Notifiers run detached, so hooks never wait on them.
- **Escalation.** A session still `blocked` or `needs-input` after `escalate_minutes` (default 30) triggers one more notification ("blocked for 45 min: …"). The check runs at most once a minute from any session's hooks, and on demand with `everett events --check` (for example from cron).

## Routing

Everett has two routers with the same output:

- **local** (default without a key): BM25 lexical scoring of the request against each session's card, title, first and last request, and project name, weighted by recency. It needs no network. A clear lead gives `SESSION`, a tie gives `ASK`, and no overlap gives `NEW`.
- **jev** (default when a key is set): the [Jev](https://typesafe.ai) System One model makes a typed choice among the sessions. A confidence below 0.6 gives `ASK`.

Session project names use the repository behind T3 and Claude worktrees, not the
worktree folder's name; other working folders use their own name. Home directories
and Codex app chat folders are not projects. The local router boosts a project when
the request contains its name's words consecutively and in order, ignoring case
and punctuation: `web-mesh` matches `web mesh`, not `mesh web`.

The Jev key is read from, in order: `TYPESAFE_API_KEY`, `typesafe_api_key` in `~/.everett/config.toml`, then `TYPESAFE_API_KEY` in `~/.hermes/.env`. Force a router with `--router local|jev`, or set `router = "local"` in the config.

## Session cards

A card is a note of 50 words or fewer, kept at `~/.everett/cards/<session-id>.md`, that says what a session is about, its state, and the next step. Routing and `ls` prefer a card over the raw transcript text while it is less than 24 hours old by file modification time. At 24 hours, it is stale and the extracted transcript text takes precedence; session activity does not refresh the card's age. Staleness affects which summary Everett uses, not whether the card file is kept.

- **Agent cards**: the SessionStart hook tells each new session where its card lives and asks the agent to write it in its first working reply.
- **Automatic cards**: when the agent has not written one, the Stop hook writes a deterministic fallback from the transcript (no LLM, no network). Automatic cards start with `<!-- everett:auto -->`, and hooks never overwrite an agent-written card.

Automatic card project labels use the repository name for T3 and Claude worktrees,
rather than the worktree folder's name, and the working folder's name otherwise.

Hooks stay silent for `everett send` resumes (`EVERETT_SEND=1`) and for scripted Claude runs.

### Installing the hooks

```bash
everett install-hooks            # print the snippets for all harnesses
everett install-hooks --apply    # back up, then merge into the real config files
```

- **Claude Code**: adds `SessionStart`, `Stop`, `UserPromptSubmit`, and `PostToolUse` entries to `~/.claude/settings.json`. The last two deliver [live messages](#live-delivery); `Stop` also records [events](#events).
- **Codex** (0.155+): adds the same four entries to `~/.codex/hooks.json`. Codex needs `hooks = true` in `~/.codex/config.toml` and asks you to trust new hooks the first time they run.
- **Grok CLI**: writes `~/.grok/hooks/everett.json` with a `Stop` hook for automatic cards and events, and a `PostToolUse` hook for live delivery. Grok ignores `SessionStart` output, so Grok sessions get no card instruction or shared core at start. Grok also runs the hooks in `~/.claude/settings.json`; Everett's Claude hooks do nothing there: they need `session_id` and `transcript_path`, and Grok's documented hook input uses camelCase `sessionId` with no transcript path. `--grok` is included by default when `~/.grok` exists.
- **OMP**: writes `~/.omp/agent/extensions/everett.ts`, which re-exports Everett's extension. OMP loads that folder at startup. You can also pass it per launch: `omp --hook=<path printed by install-hooks>`.

`--apply` writes a backup (`<file>.everett-bak-<timestamp>`) before any change. It never removes other hooks and never adds a second Everett entry. The hooks run `everett hook <name>` through the binary you ran it from. Hooks registered by the retired Python package are replaced in place.

## Shared core: one mind, many sessions

Parallel sessions each build their own context and drift apart. The shared core is the part they all have in common. Their context and actions differ, but the core is the same.

1. **Push.** Any session (or you) runs `everett learn "Atlas deploys from main; staging is auto"`. The fact is checked by a secret filter (key and token patterns plus an entropy check), capped at 500 characters, and appended to `~/.everett/core/inbox.jsonl` with the time, session id, harness, folder, and project.
2. **Merge (slow path).** `everett trunk merge` distills the inbox into `~/.everett/core/core.md` (global) and `~/.everett/core/projects/<project>.md`, at most 300 words each. With `--llm claude` or `--llm codex`, a headless harness run is prompted to deduplicate and resolve contradictions; invalid or secret-bearing output changes nothing. `--llm none` appends and removes normalized duplicates with no LLM. Previous core files and the processed inbox are archived under `~/.everett/core/history/<time>/`. Facts learned during a merge stay pending for the next one; a second simultaneous merge is refused. A configured vault gets a generated `Core.md` mirror.
3. **Pull.** The SessionStart hooks (Claude Code, Codex, OMP) add the global core plus the current project's core (at most 350 words in total) to each new session's context, with one line telling the agent to run `everett learn` when it finds something other sessions should know. The hooks read local files only, take about 40 ms, and stay silent on any error.

During an active session, call `everett_core` again to pull the latest merged facts. Every call reads the local files. Pending learnings still need a merge.

For facts beyond the compact core, call `everett_core(action="search", query="compiler cache", project="atlas")`. Local BM25 search ranks original learnings in `~/.everett/core/facts.sqlite3`. The store keeps accepted facts beyond the 300-word injection cap, with a durable ID, timestamp, scope, project, and source session/harness/folder. Search defaults to global plus current/named project. Use `scope="global"` or `scope="project"` to narrow it. Queries accept up to 500 characters and use at most 32 literal words. Results default to five, with `limit` from 1 to 20. Search does not execute FTS operators supplied in the query.

The searchable store starts with new learnings accepted by this version. It never imports history backups. Existing core files remain readable through the default action. New learnings become searchable immediately, while compact-core injection still needs a merge. Relearning the same normalized fact in the same scope keeps its ID and updates its source metadata.

For temporary memory, call `everett_learn(fact="Preview uses port 4200", ttl_seconds=3600)`. The lifetime is a whole number of seconds from 1 to 31536000 (365 days). Its receipt returns `stored=true`, `pending=false`, and an absolute `expires_at` in Unix seconds. Temporary facts are searchable before expiry and never enter the pending inbox, distillation, or injected core. After expiry, search excludes them; their stored rows remain until forgotten. This does not erase history backups or independently written paraphrases.

Relearning a temporary fact with a TTL refreshes its expiry and keeps its ID. Learning it without a TTL explicitly makes it permanent and queues it for merging. An existing permanent exact fact in the same scope must be forgotten before a temporary copy can be learned. Corrections preserve temporary expiry, including an elapsed deadline, and stay out of the compact core. A correction that would change an existing replacement fact's lifetime is rejected before memory changes.

Local MCP sessions can update memory immediately with `everett_core(action="forget", fact="Atlas deploys on Fridays")` or `everett_core(action="correct", fact="Atlas deploys on Fridays", replacement="Atlas deploys on Mondays")`. Copy the whole fact without its bullet marker, including existing merged bullets longer than 500 characters. Matching ignores case, punctuation, and repeated whitespace. Both actions remove matching active bullets and pending learnings so a later merge cannot restore those pending copies. Replacement facts use the same 500-character limit and secret filter as `everett_learn`, and stay within the 300-word core cap. `changed=false` means no match was found.

Changes default to global scope. Pass `project="atlas"` for project scope, or `scope="project"` for the current project. Forget/correct also update exact matches in the searchable store, including facts dropped from the compact core. They do not match semantic paraphrases. A running merge makes either action fail with a retry message. File and search-store writes are separate: a storage error can leave a partial change; retry the same action after resolving the error to finish it. Generated vault mirrors update after a successful change. History backups stay intact and are never merge inputs. Gateway connections remain read-only for compact shared core and reject local search or mutation arguments.

The project is the enclosing git repository's folder name, else the folder name. Your home folder is not a project. Default merge engine: `merge_llm` in the config, else `claude`.

For storage boundaries, mid-session reads, and the relationship to Power Proxy and Basic Memory, see [Shared memory and other memory tools](docs/memory.md).

### Nightly auto-merge

`everett trunk schedule` previews a native nightly schedule. Apply it explicitly to run the shared-core merge each day (default 04:00 in the host's local time, `--llm claude`):

```bash
everett trunk schedule                       # print the files and activation commands; change nothing
everett trunk schedule --apply               # write and activate this user's schedule
everett trunk schedule --at 02:30 --llm codex --apply
everett trunk schedule --remove --apply      # disable and remove only Everett's schedule
```

| Platform | Files and activation | Inspect runs |
| --- | --- | --- |
| macOS | `~/Library/LaunchAgents/dev.everett.core-merge.plist`; `launchctl bootstrap gui/<uid> <plist>` | `~/.everett/logs/merge.log` |
| Linux | `$XDG_CONFIG_HOME/systemd/user/dev.everett.core-merge.service` and `.timer` (default `~/.config/systemd/user/`); `systemctl --user daemon-reload`, then `systemctl --user enable --now dev.everett.core-merge.timer` | `journalctl --user -u dev.everett.core-merge.service` |

The job uses the current Everett executable, selected `EVERETT_HOME`, and absolute entries from `PATH`. It runs `everett trunk merge --llm <x>` and skips cleanly when the inbox is empty. `--llm none` needs no harness CLI. Linux units contain only `HOME`, `EVERETT_HOME`, and `PATH` from the installation environment; they do not copy API keys. A configured harness can still read its own account files or environment supplied by the user manager.

Linux requires systemd and a reachable user manager. If activation fails, Everett exits nonzero with the failed command and leaves the unit files for inspection/retry. The timer uses [`Persistent=true`](https://www.freedesktop.org/software/systemd/man/latest/systemd.timer.html) to catch a missed run when the user manager starts again. Running while logged out depends on the host's existing user-manager/linger configuration; Everett does not change it.

`everett doctor` checks that the Linux timer is enabled and active; this confirms scheduling, not a completed merge. Check the journal for the actual run result. On macOS it checks the plist and saved executable path. `everett onboard` offers the same "merge shared memory nightly" toggle, or use `--schedule-merge` for an unattended opt-in. Preview and removal do not alter the core or its pending facts.

## For agents: MCP

Run `everett login` in the client's Everett profile before calling tools. Registration and
`tools/list` can succeed while signed out; verify by calling `everett_ls` after restarting
the client. Even `everett_whoami` and `everett_agent_relay(action="diagnose")` require
account verification. For offline diagnostics, use terminal `everett relay diagnose`.

Hermes Agent users should install the [native Hermes plugin](plugins/everett-hermes/README.md)
for per-session tool binding and native Hermes commands and hooks. It uses the installed binary
and the same account check; the manual MCP entry is a more limited alternative.

Everett is mostly used by agents. `everett mcp` is a native Rust stdio MCP server (JSON-RPC 2.0, protocol versions 2025-06-18, 2025-03-26, and 2024-11-05), so MCP-capable harnesses get Everett as native tools:

**Jev first.** Call `everett_send` with just `text`; do not call `everett_ls` and hand-pick a target session. Without `to`, Everett routes the request itself (Jev when a key is configured, else the local matcher) before delivering it, and the response reports the decision -- router, chosen session, confidence -- so you can tell the user e.g. "jev picked \[claude\] ~/src/api". Pass `to` only when the user named a specific session or you are replying to one; it skips routing. On an ambiguous `ASK` decision nothing is sent -- the response carries `candidates` (close sessions) to disambiguate, then send again with `to`. `everett_ls` is for a quick overview of what sessions are doing, not for picking a send target.

| Tool | Arguments | Returns |
|---|---|---|
| `everett_ls` | `hours?`, `harness?`, `all?` | Set `all: true` to include automation sessions. Your own session is marked `you`. Like local `everett ls --json`, this includes `t3_thread_id` alongside the native session `id` when a T3 thread mapping is available. Overview only -- not for picking a send target. |
| `everett_route` | `text`, `router?`, `hours?` | `SESSION` / `NEW` / `ASK`, confidence, router used, candidates. Never sends; excludes the caller when known. |
| `everett_send` | `text`, `to?`, `hours?`, `spawn?`, `dir?`, `harness?`, `timeout?`, `mode?`, `wait?`, `reply_to?` | Routes then pings by default. Dispatch is separate from inbox pickup. Unsupported immediate paths fail explicitly; `mode: "inbox"` opts into legacy hooks/polling/remote queues. `reply_to` queues a correlated answer. |
| `everett_learn` | `fact`, `project?`, `scope?`, `ttl_seconds?` | Stores a secret-filtered fact. Durable facts queue for shared core; temporary facts stay in local search until expiry. |
| `everett_core` | `action?`, `project?`, `query?`, `limit?`, `fact?`, `replacement?`, `scope?` | Reads the latest core by default. Local `search` ranks original learnings beyond the core cap. `forget` and `correct` update exact facts in active, pending, and searchable memory. |
| `everett_card` | `what`, `state`, `next` | Writes the calling session's card. |
| `everett_skill` | `action?`, `task?`, `name?`, `query?`, `source?` | The skill hub. `match` returns the one skill that fits `task`, or null; `get` returns a named skill's instructions, sibling files, and path; `list` (default) browses. Every skill is also an MCP prompt (`prompts/list`, `prompts/get`) for clients that support them. |
| `everett_whoami` | none | The caller's session id, harness, hop count, and how they were detected. |
| `everett_inbox` | `peek?`, `message_id?`, `reply_to?` | Read and mark matching messages in your local inbox. Use `reply_to` to read only pending replies to an original request ID. |
| `everett_sent` | `to` (optional) | Local stdio MCP only. Recover recent request IDs for the current sender, newest first. Filter by exact recipient ID with `to`. |
| `everett_status` | `message_id` | Local stdio MCP only. Check pickup and recover a retained reply from the session that sent the request. Use the original ID from `everett_send`. |
| `everett_retry` | `message_id` | Local stdio MCP only. Nudge the original T3 thread for a still-queued ping from your session using the existing inbox request. |
| `everett_event` | `kind`, `message`, `project?` | Records done / blocked / needs-input / info for your session. |
| `everett_subscribe` | `target`, `unsubscribe?` | Follows a session's or project's events in your inbox. |
| `everett_control` | `action`, `session?`, `thread?`, `text?`, `harness?`, `project?`, `cwd?`, `title?`, `provider?`, `model?`, `worktree?`, `branch?`, `wait?`, `messages?`, `timeout?` | Owner-local control. `threads`, `start`, `send`, `wake` and `read` drive T3 Code threads through the `t3` CLI. `start` also opens headless Claude, Codex, OMP, Pi, Hermes, Grok or Devin sessions. T3 `provider` requires `model`. `send`/`wake` ping other sessions through `everett_send`; `read` returns supported transcript messages (Devin ATIF, not SQLite). `stop` explains why Everett cannot stop a harness. Remote-derived callers are refused. |
| `everett_agent_relay` | `action`, `id?`, `name?`, `fingerprint?` | Verified identity, registration, directory/pins and scoped revocation controls. |
| `everett_agent_send` | `agent`, `session`, `harness`, `text`, `idempotency_key` | Device-side encrypted send to an explicit named agent. No sender-requested wake or implicit routing; receiver policies are separate. |
| `everett_agent_status` | `message_id` | Sender-only status from signed delivery/pickup/reply observations. |

The `everett_send` MCP tool bounds `timeout` and `wait` to 1,800 seconds (30 minutes); values above that are capped. `wait` applies only to explicit `mode: "inbox"` and does not ping or wake the target. These MCP tool bounds do not add a 30-minute cap to the CLI `everett send` options.

See [agent-facing relay MCP](docs/agent-relay.md), [cross-device deployment](docs/cross-device.md)
and [relay threat model](docs/relay-threat-model.md). Cloudflare binds account/agent identity;
cross-owner delivery is unavailable. External inbox text
is labeled untrusted. Existing Jev routing, SSH, OMP, and Pi integration is unchanged.

Listing, routing and sending default to a 72-hour window. Pass the same larger `hours` value
to these MCP tools when continuing an older session. Resume/spawn children have closed stdin;
the MCP connection remains available for the client's protocol messages.

**Skill hub.** Everett finds every `SKILL.md` on the machine and hands an agent the one it needs, so no client has to carry a catalog in context. A skill reaches the agent three ways:

- **Up front.** The MCP instructions name up to eight skills for the current project: the ones used there most, then the project's own, then your extra folders.
- **On your prompt**, where hooks run (Claude Code, Codex). Naming a skill (`/prove`, "use the prove skill", "full send") adds its full instructions. A skill that merely fits is suggested in one line, each skill once per session. A skill the harness loads itself is left to it.
- **On request.** `everett_skill` with `action: "match"` and the task returns one skill or null.

Matching runs locally on names and descriptions; no model is called. Only folders you keep yourself are suggested or shortlisted unasked: `~/.claude/skills`, `~/.codex/skills`, `~/.agents/skills`, `~/.config/devin/skills`, `~/.devin/skills`, project skill folders (`.claude|.codex|.agents|.devin/skills` and `skills/` in the server cwd's ancestors), and extra roots from `EVERETT_SKILL_DIRS` or `[skills] dirs = "~/a:~/b"` in `~/.everett/config.toml`. Skills in plugin caches (`~/.claude/plugins`, the Devin CLI `plugins_cache`) and `~/.hermes/hermes-agent/optional-skills` are returned only when named. Identical copies count as one skill; when versions differ, `get` uses the best source (project, extra, `.agents`, Claude, Codex, Devin, then plugins) and lists the rest in `also_in`. Use per project is counted in `~/.everett/skills/usage.json`.

Register it:

```bash
everett install-mcp            # print the registration for Claude Code, Codex, OMP, and Grok
everett install-mcp --apply    # back up, then register (idempotent)
```

- **Claude Code**: `claude mcp add --scope user everett -- <everett> mcp`, or `--apply` adds `mcpServers.everett` to `~/.claude.json`.
- **Codex**: an `[mcp_servers.everett]` block in `~/.codex/config.toml`.
- **OMP**: `mcpServers.everett` in `~/.omp/agent/mcp.json`. OMP also imports Claude Code's servers.
- **Grok CLI**: `grok mcp add everett <everett> -- mcp`, or `--apply` appends an `[mcp_servers.everett]` block to `~/.grok/config.toml`. Grok also imports Claude Code's servers by default. Included by default when `~/.grok` exists.
- **Devin CLI (v3000.3+)**: `everett install-mcp --devin --apply` merges `mcpServers.everett` (launched as `everett mcp --harness devin`) into `~/.config/devin/mcp_config.json` (Windows: `%APPDATA%/devin/mcp_config.json`). This is separate from the session store configured by `DEVIN_HOME`. Existing servers and unrelated environment values are preserved; use `--repair --apply` to back up and refresh a stale registration. `everett install-hooks --devin --apply` adds `SessionStart`/`UserPromptSubmit` hooks to `~/.config/devin/config.json` that record which session the Devin process holds, so MCP calls bind to it.

The registration launches the executable you ran it from with `["mcp"]`. A launcher left by the
retired Python package (`["-m", "everett", "mcp"]`) is reported stale and replaced by `--apply`.
For another MCP client, copy the launcher printed by a dry run. Restart clients after changing their registration.

**Caller identity.** Claude Code supplies `CLAUDE_CODE_SESSION_ID` when it starts a stdio MCP server; Grok CLI 1.0.4+ supplies `GROK_SESSION_ID`; Codex supplies the current thread on every call as `_meta.threadId` (Unreleased; on `main` after 1.6.0); and `everett gateway` supplies an immutable `EVERETT_SESSION_ID`. Pi, OMP, Devin, and Hermes use session-specific binding (Unreleased): Pi's extension starts its MCP bridge with the session id; the OMP extension and Devin hooks record which session their host processes hold; OMP and Devin MCP servers use `--harness <name>` to bind to their host process. The native Hermes plugin starts a per-session MCP bridge with `EVERETT_SESSION_ID` set. Manual `everett mcp --harness hermes` accepts an explicit `EVERETT_SESSION_ID` or `HERMES_SESSION_ID`; without one, process detection can bind only one unambiguous interactive Hermes CLI session. The printed `install-mcp --hermes` entry omits an explicit ID, and a static ID does not provide the native plugin's per-session gateway binding. The portable Agent Plugins v1 manifest only registers that MCP server; it does not provide the native session bridge or Hermes hooks. When binding is ambiguous or missing, identity-requiring tools return a readable tool error (`isError: true`) that says how to fix it; Everett never guesses. T3 Code threads still need `EVERETT_SESSION_ID` in their MCP launch environment. A model-supplied `session_id` cannot establish or replace a binding; the argument is deprecated, no longer advertised in tool schemas, and a conflicting value is a tool error. Details per harness: [docs/mcp-identity.md](docs/mcp-identity.md).

**Safety.**
- The server refuses self-send. If `everett_whoami` cannot identify you, have the owner bind the MCP launch environment before sending. Do not guess another session's identity from its folder or recency.
- `EVERETT_HOPS` travels through every delivery. With `hop_limit` on, a request that has already been forwarded 3 times is refused, so two agents cannot ping-pong (off by default). A live session also inherits the hop count of messages delivered to it within the last 15 minutes (older deliveries no longer count against it).
- A new session starts only with `spawn: true`.
- `everett_learn` runs the secret filter.
- Calls are logged to `~/.everett/mcp.log` with tool name, argument names, caller id, result status and duration. Argument values and replies are not logged. Existing log entries from older versions are preserved.

Tool failures come back as `isError: true` with a message written for the agent. Malformed requests get standard JSON-RPC error codes.

## Configuration

Everything is optional. `~/.everett/config.toml`:

```toml
vault = "~/Notes"             # enables `everett trunk`
vault_dir = "Everett"         # folder inside the vault (default "Everett")
default_harness = "claude"    # NEW suggestions: claude | codex | omp | pi | hermes | grok
router = "local"              # local | jev
typesafe_api_key = "…"        # Jev key ([jev] api_key also works)
merge_llm = "claude"          # trunk merge engine: claude | codex | none
notify = "osascript"          # blocked/needs-input: osascript | command | both | none
notify_command = "…"          # e.g. a push-notification command; gets the summary as $1 (sets notify default to both)
escalate_minutes = 30         # re-notify once when a session stays blocked this long (0 = never)

[auth]
provider = "cloudflare"                 # EVERETT_AUTH_PROVIDER; only supported provider
issuer = "https://everett.raitox.tech"  # EVERETT_AUTH_ISSUER; identity, not the connect URL
base_url = "https://relay.raitox.tech"  # EVERETT_AUTH_BASE_URL; optional relay URL
# relay_aliases = "https://everett.raitox.tech,https://relay.raitox.tech"  # EVERETT_AUTH_RELAY_ALIASES; default shown

```

Environment overrides: `EVERETT_VAULT`, `EVERETT_VAULT_DIR`, `EVERETT_HARNESS`, `EVERETT_ROUTER`, `TYPESAFE_API_KEY`, `EVERETT_NOTIFY`, `EVERETT_NOTIFY_COMMAND`, `EVERETT_ESCALATE_MINUTES`, `EVERETT_AUTH_ISSUER`, `EVERETT_AUTH_BASE_URL`, `EVERETT_AUTH_RELAY_ALIASES`, and `EVERETT_HOME` (the root used in place of `~`). Existing configs that set only `issuer` keep using that origin as the connect URL. GitHub sign-in stays on the issuer. Pins treat every origin in `relay_aliases` as the same relay; an explicit list replaces the public pair above.

## Accounts

Local MCP tool calls require one `everett login` per Everett profile. Each harness
session keeps its own binding and inbox. Everett verifies the account on each tool
call, so running MCP processes pick up sign-in, sign-out and revocation on their
next call. Local terminal commands such as `everett ls` work offline. Hosted gateway
clients use their configured gateway authentication.

Every native `tools/call` verifies the saved Cloudflare credential online before dispatch;
there are no tool exemptions. Initialization, `ping`, and `tools/list` do not verify it.
`EVERETT_GATEWAY_*` settings do not exempt ordinary `tools/call`: the gateway uses a
separate scoped-worker request path with its own owner/grant authorization.

The optional [private mesh console](web-mesh/README.md) is separate from the local
dashboard. It is read-only, disabled by default, and needs an owner-approved, expiring
Cloudflare read grant; GitHub console sign-in alone does not approve access.

`everett login` uses Cloudflare's native
device approval protocol (not RFC 8628). Approve the named device at the printed
browser link. Credentials live in `~/.everett/cloudflare-auth.json` (0600), and
`whoami` verifies them online. `logout` removes the local credential;
`logout --revoke-device` additionally requests server revocation first.
No incoming CLI port or browser callback is needed. Relay keys and pins stay under
`~/.everett/relay-cloudflare/`; retired state is not migrated or reused.
See [account setup and revocation](docs/auth.md). Sessions and shared core stay local.

## Privacy

Session indexing and state are local. Everett reads harness stores and writes its own state under
`~/.everett/`, plus configured vault notes and explicitly applied harness registrations. Headless
resumes let the harness append its own history. Local routing needs no network; optional Jev
sends routing context to its API. `send`, `--spawn`, and LLM merges call your installed harness,
which may contact its provider. `trunk merge --llm none` is local. Desktop notifications use macOS
`osascript`; a custom `notify_command` can send events wherever you configure it.

Transcript adapters generally inspect bounded slices; Grok prompt-history scans and automatic
card backfills may read more. SQLite stores are opened read-only. No harness session data is copied
to a cloud service for plain `ls`, local routing, or inbox delivery.

Explicit Cloudflare sends share encrypted bodies and **visible** org/device/grant IDs,
public bundles/fingerprints, parent/hop lineage, message/idempotency IDs, timing/size,
signatures and observation state. Account identity and connection metadata
are also visible to the service; replay/revocation records remain after
ciphertext deletion. E2EE is not anonymity, secure backup erasure, endpoint protection
or forward secrecy against later recipient static-key compromise. Full inventory and
limitations: [relay threat model](docs/relay-threat-model.md).

## Troubleshooting

For delivery diagnosis, compatibility checks, migration, and a correlated reply check,
see [troubleshoot message delivery](docs/messaging-troubleshooting.md).

| Symptom | Next step |
| --- | --- |
| `everett: command not found` | Add the install directory (default `~/.local/bin`; `~/.cargo/bin` for cargo) to `PATH`, open a new terminal, then `everett --version`. |
| `No module named everett` / hooks call `python3` | A registration still points at the retired Python package. Run `everett install-hooks --apply` and `everett install-mcp --apply`. |
| MCP client shows zero tools | Run `everett doctor`. Check that it reports a successful stdio tool listing. For a stale/disabled launcher, run `everett install-mcp --claude --repair --apply` (replace `--claude` with `--codex`, `--omp`, `--grok` or `--devin`), then restart/enable the server in the client. Fix malformed config before repair. |
| OMP or Devin has an old `everett mcp` registration | Run `everett install-mcp --omp --devin --repair --apply` to back up and restore the harness-specific arguments. Restart the affected MCP clients, then run `everett doctor`. |
| Hermes points to a missing Everett binary | Put `everett` on the PATH visible to Hermes or set `plugins.entries.everett-hermes.settings.binary` in `~/.hermes/config.yaml`, then restart Hermes. For the manual MCP setup, `everett install-mcp --hermes` prints a config entry but does not edit the file. |
| Session not found / no live reply | Start a harness session or run `everett --hours 168 ls` for older ones; send to its exact id or title. In MCP, pass `hours: 168` to listing and sending. Hooks need to be enabled; the native Hermes plugin surfaces message-ID notices at hook events, so call `everett_inbox` to read contents. Opt-in local `live_wake` also sends ID-only nudges, while manual/portable MCP setups poll `everett_inbox`. A queued message is not a read receipt. |
| Codex reports `Interrupted system call (os error 4)` | Delivery is unconfirmed. Check the target session before retrying; for an open session with Everett hooks, use `--mode inbox` (MCP: `mode: "inbox"`). Failed resumes are not retried automatically. |
| Shared fact missing / scheduling unavailable | Run `everett trunk merge --llm none`, then `everett core`. Run `everett doctor` to inspect the macOS LaunchAgent or Linux systemd user timer. On Linux, inspect `systemctl --user status dev.everett.core-merge.timer` and the service journal; a missing user manager prevents automatic scheduling. |

## Development

The code lives in `everett-rs/`.

```bash
cd everett-rs
cargo build --release            # binary at target/release/everett
cargo test                       # isolated suite: temp HOME, never touches real stores
```

`tests/release_journey.rs` drives the CLI and every MCP tool through routing, messages, replies,
status subscriptions and global/project memory. Point it at a packaged binary with
`EVERETT_VERIFY_BINARY=/path/to/everett cargo test --test release_journey`.
See the [verification skill](.agents/skills/verify-everett/SKILL.md).
GitHub Actions runs `cargo test` and the release journey against a release build on macOS and Linux.

Release tags publish prebuilt binaries for macOS (arm64, x86_64) and Linux (static musl,
x86_64, aarch64), each with a `.sha256`, plus a rendered Homebrew formula (`everett.rb`).

The gateway uses the official Rust MCP SDK for private stdio and authenticated loopback HTTP.
See the [owner-bound gateway setup](docs/gateway.md).

## License

MIT. See [LICENSE](LICENSE).
