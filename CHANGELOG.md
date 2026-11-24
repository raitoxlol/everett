# Changelog

## Unreleased

### Changed

- Skip HostedAuth expiry sweeps on OAuth discovery and introspection while keeping expiry checks in authorization paths; use bounded indexed sweeps on other requests and prune stale `auth_rates` buckets once per minute window (#252).

### Added

- Add Muse Code as an eighth local harness (#268): `everett ls` and routing read Muse's `session-index.db` (read-only, under `$MUSE_DATA_HOME`, `$XDG_DATA_HOME/muse`, or `~/.local/share/muse`) plus each session's `session.jsonl`; `everett send` resumes with `muse exec --session-id <id> --allow-workspace-switch` and `--spawn` uses `muse exec`. `install-hooks --muse` adds `SessionStart`, `Stop`, `UserPromptSubmit`, and `PostToolUse` hooks and `install-mcp --muse` registers `mcp_servers.everett` in `~/.config/muse/settings.json`, both writing `schema_version: 1` and a backup; MCP calls bind through `MUSE_SESSION_ID`, and `doctor` and onboarding check Muse. Transcript reads (`control read`) are not supported for Muse yet.
- Add four MCP tools (#273): `everett_search` ranks recent sessions across harnesses by title, card, first/last request, and working directory; `everett_peers` lists paired SSH and relay devices or one SSH peer's recent sessions; `everett_broadcast` sends one request to up to 16 explicit `everett_send` targets with per-target results; `everett_tasks` keeps a shared per-project task board at `<project>/.everett/tasks.json` (`list`, `add`, `claim`, `done`, `remove`). Gateway bindings deny them by default, and the Hermes plugin's fallback catalog and manifest list them.
- Add durable Teresis memory: every `everett learn` is also written to a shared `memory.db`, and `everett recall` plus the `everett_recall` MCP tool search all learned facts with word, stem, and typo-tolerant matching. `everett learn --key` names a fact so a new value replaces the old one in memory search, and `EVERETT_MEMORY_DB=off` disables the store.
- Add private cross-account `LOCAL@SCOPE` inboxes with grants scoped to one sender owner/device and one recipient device/session; encrypt message bodies end-to-end and support grant revocation (#244).
- Add the native Hermes plugin with per-session tools, inbox hooks, `/everett`, and opt-in local live wake (#247). Its portable Agent Plugins v1 fallback registers MCP only.
- Add Hermes `/everett card` and `/everett sent`, reset/delete session cleanup, and active-profile plugin diagnostics with native install guidance (#253).
- Add `everett_directory`, a read-only gateway and MCP tool that labels each authorized destination ID with its kind (session, T3 Code thread, registered conversation, or bare inbox), project, title, availability, and permitted actions, so a remote client resolves a named existing session or T3 Code thread to the exact `to` for `everett_send`. Gateway `everett_send` now accepts an exact T3 thread ID and resolves it to the thread's provider session inbox, reporting `resolved_from`/`resolved_to`; destinations outside the owner allowlist, agent-supplied `ids` arguments, and name/title targets remain rejected, and blocked sends name `everett_directory` as the recovery path (#255).
- Everett Local 0.3.2: add `check.py`, a read-only install diagnostic that verifies the package layout, portable-manifest absence, `.mcp.json` launch command, configured runtime version, and the generated `marketplace.json`, each with a remedy; the skill now distinguishes a missing local server, an answer from a different Everett MCP server (`mcp__everett__*`), and a runtime/account failure instead of letting the gap pass silently (#262; diagnoses #132, which stays open).
- Everett Local `check.py` also requires the Codex registration in `~/.codex/config.toml` (the `everett-direct-local` marketplace pointing at this install and an enabled `everett-direct@everett-direct-local` plugin), so an install that skipped `codex plugin marketplace add`/`codex plugin add` no longer reports ready. It requires `.mcp.json` to run an executable Python with this plugin's own `launch.py`, reports wrong-typed JSON as a failed check with a remedy instead of a traceback, and warns about a cached root `plugin.json`. Remedies for a missing generated marketplace say to rerun `install.py` (with the install's `--destination`) before the `codex plugin` commands. `install.py` now prints both `codex plugin` commands and a runnable `python3 /absolute/path/check.py`.

### Fixed

- Keep Unix inbox-compaction replacements and live-session records owner-only, with a private live directory and `0600` locks (#236, #246).
- Keep Unix event-state files (`~/.everett/state/<session-id>.json` and `~/.everett/subscriptions.json`) owner-only (`0600`) when Everett replaces them (#271).
- Treat session cards as fresh for 24 hours from their last modification, independent of session activity; stale cards no longer replace extracted session text (#241).
- Bound `everett_send` MCP timeouts and explicit inbox waits to 1,800 seconds; this does not change the CLI `everett send` options (#241).
- Write dotted config keys to their TOML section, including sections with trailing comments, and keep Codex/Grok `install-mcp --repair` entries in their native TOML form (#241).
- Show sub-minute event ages in seconds, preserve fractional seconds when parsing ISO timestamps, format duration and timeout values to six significant digits, and keep every row in the generated `Trunk.md` session table on one line (#241).
- Return one JSON-RPC error object for invalid empty batches, retain complete session-history records at the head-window boundary, and preserve Unicode question-based `needs-input` classification (#241).
- Decode property-list-escaped Trunk schedule paths before staleness checks and keep subprocess deadlines effective while input is supplied (#241).
- Resolve inferred project-scoped memory to the enclosing Git repository from deeply nested directories; outside a repository, keep the current-directory-name fallback (#238).
- Fix regeneration of Grok CLI AUTO cards from their session transcripts (#240).
- Fix docs-site search results so heading links target the rendered section anchors (#240).
- Cap CLI `everett send` subprocess waits, including `everett send --spawn`, at one year (#260).
- Strip `Next steps:` as well as `Next step:` when deriving a card's next step (#260).
- Name the Devin config file in the `install-hooks --devin` snippet (#260).
- Keep the existing file mode when `install-mcp --codex --apply` rewrites `~/.codex/config.toml` or `install-mcp --grok --apply` rewrites `~/.grok/config.toml` (#260).
- Read `SKILL.md` name, description, and body correctly when the file starts with a UTF-8 BOM (#260).
- Match coordinator routing terms only at the start of a word, so words that merely contain a term no longer mark a session as a coordinator (#260).
- Show the latest non-empty user message for Hermes sessions, skipping empty user rows (#260).

## 2.4.0

- Bind local handoffs to the active harness session (#231). Ambiguous route results and exposed cards include an address callers can use (#232).
- Connect Pi during onboarding and guide Hermes users through local setup (#229). The skill hub selects a relevant playbook and links Devin threads managed by T3 (#228).

- Add the skill hub: `everett skills`, the `everett_skill` MCP tool, and MCP prompts expose every harness's `SKILL.md` playbooks to any client. Extra roots come from `EVERETT_SKILL_DIRS` or `[skills] dirs` in `~/.everett/config.toml`.
- The skill hub hands over one skill instead of a catalog: `everett_skill` `match` and `everett skills --match` pick the skill that fits a task, MCP instructions name a short per-project list, and the Claude Code and Codex prompt hooks add a skill the user names or suggest one that fits. Only skill folders the user keeps are suggested unasked.
- Recognize Devin threads started from T3 Code as `[t3code]` sessions with their T3 thread ID.
- Route, `send --to` and automatic cards name a T3 or Claude worktree session by its repository. A request reaches a project whose name has several words, such as `web-mesh`.
- Keep comparisons such as `a < b and c > d` in session summaries and automatic cards.
- Parse date-only timestamps, and stop dropping a timestamp whose fraction rounds up to the next second.
- Show sessions older than two days in days in `everett ls` and `everett inbox`.
- Require Cloudflare native account verification on every local MCP tool call; local CLI setup and SSH peers remain account-free. Clarify onboarding paths and client-side tool verification (#211, #216).
- Add a private, opt-in read-only mesh console with owner-approved expiring read grants, and a documentation site built from the repository Markdown (#214, #221).
- Add a Hermes-supported portable plugin and repair Pi tool registration, session working directories, cancellation and reconnection (#220, #217).
- Add owner MCP tool grants and CLI management, isolate remote conversation inboxes, and require private inbox handles in strict hosted mode (#210, #213, #219).
- Restore opt-in authenticated MCP Events, advertise them to MCP 2.0 clients, and support exact event-message pickup for ChatGPT Dots (#169, #207, #208).
- Add local request status, retained reply lookup, sent-request recovery and recipient filters in CLI/MCP. Return the retained request ID when T3 dispatch fails after writing the inbox item, and reuse confirmed dispatches on retry (#174, #177, #178, #179, #190, #193, #196, #198, #199, #200, #202, #205).
- List and address exact T3 thread IDs, discover managed runtimes and honor `T3CODE_HOME`; add CLI thread listing and recent transcript reads, including Devin ATIF. Honor the CLI look-back window and stream long JSONL histories (#180, #181, #183, #184, #194, #195, #197, #201, #203, #212).
- Add delivery evidence and verified T3 model selection; distinguish pending pings from confirmed dispatch and show follow-up commands (#176, #215).
- Add learned-fact expiry and CLI search with JSON scope/expiry metadata (#189, #191, #192).
- Explain identity provenance, support automation-session MCP listings and gateway conversation-kind filters, and show actual remote preview transports (#168, #173, #186, #187).
- Claim inbox messages before returning them and acknowledge hook pickup before returning work; recover valid status despite unrelated unreadable relay receipts (#175, #185, #188).
- Improve Everett Local runtime configuration errors, narrow-terminal resize guidance, and Devin listings with a single activity timestamp (#172, #182, #204).

## 2.3.2 — maintenance candidate

- Budget the complete encoded MCP inbox result before marking messages delivered. Large queues return bounded batches, and messages outside the returned batch remain queued (#165).
- Warn that timed-out or abnormally exited T3 dispatches may already have delivered the turn; reconcile the thread before retrying (#163).
- Handle fractional RFC3339 timestamps and recursive JSON input in the read-only Python 3.9 tunnel-health diagnostic (#162).

## 2.3.1 — maintenance candidate

- Preserve a tunnel connection with fresh, matching local poll health instead of interrupting it on every scheduled renewal. Stale or unavailable health still renews the client; disconnects can leave upstream requests unresolved.
- Accept actual opaque T3 thread/run receipt IDs without relaxing destination grants (#158).
- Discover current destination grants through whoami instead of freezing them in cached tool schemas (#159).
- Validate downloaded executables before atomically replacing an installation; failures preserve the previous binary (#160).
- Qualify all four release archives on native architecture runners, including extracted-binary MCP, gateway and tunnel tests. Manual candidate builds do not publish a release.

## 2.3.0 — 2026-11-16

- Require structured, command-specific T3 dispatch receipts before reporting delivery, and validate control timeouts and chat discovery records (#149, #152).
- Authorize gateway plugins separately from their registered chat identities, so chat and Codex sessions retain distinct identities (#150).
- Give shared session, delivery, and gateway behavior clear domain owners (#151).
- Recover tunnel connections and gate native starts on verified readiness (#153).
- Restrict local gateway access to the configured owner account and use supported tunnel route-pinning flags (#154).
- Package one versioned hosted Everett plugin around an existing owner app (#155).

- Hop limit is now off by default; turn it on with `hop_limit = "on"` (docs/hop-limit.md). Cross-device relay/peer messages follow the same setting (wire bound 64; hosted relay redeployed to accept it).
- Every delivered Everett batch now carries a short context note (agent requests, read-only by default, how to reply).

- Everett Local 0.3.1: ChatGPT desktop ignored the plugin's local server when the 0.3.0 root `plugin.json`/`mcp.json` were present. The installer now writes the Codex/ChatGPT layout by default, removes stale root manifests, and keeps them behind `--portable`. Verified in ChatGPT desktop Codex mode with `mcp__everett_local__everett_whoami` and `everett_ls`.
- Add MCP `everett_control` for owner-local session control: list, start, send to, wake and read T3 Code threads through the `t3` CLI; start headless sessions in other harnesses; read the latest messages from any harness transcript. `stop` reports per harness why it is unsupported instead of killing anything. Remote-derived callers are refused. Not exposed through the gateway.

## 2.2.1 — 2026-11-10

- Add `everett relay diagnose` and MCP `everett_agent_relay(action="diagnose")` for offline receiver, registration, webhook, and bounded cached-queue summaries. Diagnostics omit message bodies, account identifiers, URLs, and credential values. They do not verify remote connectivity or authorization.
- Use hibernating WebSocket notifications by default so healthy quiet receivers make no recurring billable inbox requests. Preserve encrypted queue recovery on reconnect, per-operation authorization, and five-second local maintenance. Legacy adaptive polling requires explicit `EVERETT_RELAY_TRANSPORT=poll`. Update the Worker first.
- Remove repeated idle device registration and redundant Worker membership RPCs while retaining per-operation authorization. Startup, reconnects, messages, and retries still incur requests. Production savings remain unmeasured.
- Correct T3, OMP, and attached Claude send previews without queueing or invoking resume. Preserve authenticated replies and durable delivery receipts when older status observations or request expiry arrive later.
- Copy test executables into T3 and reply fixtures so tests work when Cargo's target and temporary home use different filesystems, and retry fixture spawns that hit Linux ETXTBSY.
- Gate tagged publication on Worker tests, typechecking, and public bundle packaging. Run the optimized CLI/MCP journey on build runners with a matching architecture before uploading their archives.
- Keep GitHub sign-in on the identity issuer when a browser opens another public relay host, so the GitHub OAuth app needs only the issuer callback. Accept existing peer pins across an explicit relay alias list; the public issuer and `https://relay.raitox.tech` are the default list.
- Accept ChatGPT's optional OAuth locale hint and validated support for public-client authentication in client metadata. Keep PKCE and configured client, redirect and resource restrictions. Preserve both existing connector resources in the public deployment profile. A fresh ChatGPT callback and tool call remain unverified.
- Document keeping Worker and Durable Object code in step: use immediate Durable Object code updates per deployment only when RPC methods change.

## 2.2.0 — 2026-10-27

- Separate the Cloudflare relay base URL from issuer identity. Account keys stay `https://everett.raitox.tech`; the CLI defaults to `https://relay.raitox.tech` for connect. Worker auth accepts an allowlist of public hosts. Existing configs that set only `issuer` keep working.

- Remove Convex completely: Cloudflare is the only hosted relay. Remove the legacy OIDC relay login, registry/address publication, backend package, vendored SDK, CI, and unsupported cross-owner grant CLI/MCP surfaces. Local/offline messaging, explicit SSH pairing, and signed v2 envelopes remain. This breaks use of the Convex relay.

- Make `ping` the CLI and MCP send default and add opt-in authenticated author/session cards on Cloudflare (RAI-70). Idle local sessions run headless; verified local Claude background sessions receive an immediate native inbox nudge. Unsupported immediate transports fail instead of silently queueing. `inbox`, `auto`, and `resume` remain explicit compatibility modes. `relay expose SESSION`, `relay authors`, and MCP relay `expose`/`withdraw`/`authors` use credential-derived authors and agents, without device pairing for discovery. MCP `directory` retains device bundles. Revoked credentials and memberships disappear from directory results; unexpected lookup failures surface as errors.
- Keep validated native Claude ping requests durable even when preflight or attach fails, returning the retained message ID without claiming successful dispatch. Stubbed CLI/MCP tests cover native receipts, failure retention, and non-interactive stdin; legacy `--wake` remains interactive-only.
- Show authenticated peer cards and separate agent/session tags in the local dashboard. Cards are shared plaintext metadata only after explicit exposure; message encryption, remote provenance, and read-only provider stores are unchanged. A Cloudflare Worker deployment is required for the directory APIs.

- Route relay-bound delivery to registered external agents, including `grok-bot` and `openai-dot` (#97).
- Keep delivery running when a capability-status write fails (#109), and skip incomplete inbox rows and unreadable outgoing receipts instead of terminating the receiver (#110).
- Add `relay devices --names` to show hosted enrollment names beside device IDs and verified fingerprints, and `relay rename NAME` to update only this device's advisory label. Original JSON bundle output and pairing/pin semantics are unchanged. Hosted metadata requires a backend update; missing labels remain explicitly unnamed.

- Add an owner `receiver-webhook.json` policy. When a rule matches a registered `grok-bot` or `openai-dot` inbox message, Everett POSTs a wake nudge to the owner's HTTPS URL. The POST omits message text. HTTP 200 means the endpoint accepted a routine run, not that the bot consumed the inbox. A missing or disabled policy leaves send behavior unchanged.

## 2.1.0 — 2026-10-24

- Make local setup account-free, with remote enrollment as an explicit choice. Open browser sign-in only after explicit confirmation.
- Add explicit `send --wake` for idle Claude background sessions and opt-in automatic relay receiver wake. Local policy restricts the exact session, working directory and paired sender device; remote content cannot enable wake.
- Preserve correlated replies, durable at-most-once wake attempts, signed provenance and harness identity. Targeted inbox reads avoid consuming unrelated backlog.
- Enable public multi-account enrollment after live GitHub account isolation and independent revocation acceptance. Each verified GitHub account has an isolated relay namespace; cross-account messaging remains denied.
- Add public cross-device acceptance tooling and repair test isolation for concurrent credential writes and send context.


## 2.0.0 — 2026-10-22

### Onboarding and accounts

- Add a compact interactive setup flow with a local, cloud and VM chooser. Cloud and VM selections remain pending until their own connections are configured.
- Add native Cloudflare device approval to first-run setup. GitHub identifies the account owner; approving a device does not sign in to an agent provider.
- Add encrypted native Cloudflare relay messaging with outbound polling, durable ciphertext staging, temporary-failure retries and explicit peer fingerprint pins.
- Add an OAuth resource-server mode to the Rust gateway for allowlisted external clients. It uses PKCE, rotating refresh tokens, introspection and revocation. Dynamic client registration is not supported.
- Keep local and SSH workflows usable without an Everett account. Native Cloudflare device authorization is a custom protocol, not RFC 8628.

### Device discovery and caller identity

- Add paged `everett devices` and signed registry metadata for `peer add NAME`. Pairing still requires out-of-band fingerprint checks.
- Propagate permanent device revocations through the registry and clean matching tagged SSH authorizations. Offline peers receive revocations after their next successful fetch; existing SSH connections are not terminated.
- Bind MCP calls to host or per-call session identity, return identity failures as tool errors, and fall back to local routing when Jev rejects a request.
- Track reply depth separately from forward hops. Install only the hooks and extensions for harnesses that Everett detects or the user selects.

### Detailed changes included in 2.0.0

- Document the OpenAI dot tunnel that was checked on 1.6.0, including the
  gateway file, exact session ids, `CONTROL_PLANE_API_KEY`, and ChatGPT's
  no-authentication plugin path. Add `everett tunnel watch` plus systemd and
  launchd examples so a live `tunnel-client` that stops forwarding after
  repeated poll timeouts is restarted without putting keys in the unit file.
- Codex MCP calls bind from the per-call `_meta.threadId` (first included in
  2.0.0).
- Pi, OMP, Devin, and Hermes MCP callers bind to their session without a
  hand-set `EVERETT_SESSION_ID`: `everett mcp --harness <name>` finds the host
  process and the session its Everett hook recorded (Pi extension, OMP
  extension, new Devin hooks via `install-hooks --devin`), or the single new
  session of that harness in its directory. Ambiguity fails closed.
- Identity failures are tool results with `isError: true` and a fix, not
  JSON-RPC `-32602`. `session_id` is removed from tool schemas (still accepted
  when it matches the binding; never establishes one).
- A Jev failure (rejected key, timeout, bad response) falls back to local
  routing with a warning in CLI stderr and MCP `warning`/`fallback_from`.
  `onboard --yes` no longer saves a rejected key and names any stale saved key;
  `doctor` now makes one Jev call to check the key.
- Local replies keep the hop count of the message they answer and track
  `reply_depth` (limit 4), so ask -> answer -> acknowledgement fits; forwards
  and remote-derived replies still spend hops.
- `install-hooks` writes OMP/Pi/Devin files only when that harness is installed
  or asked for. `onboard --yes` warns when no harness was configured.
- Reading or peeking at a local Grok Bot or OpenAI dot message labels it
  untrusted without tainting unrelated later resume/spawn work; an explicit
  reply retains the external provenance.
- Propagate permanent registry device revocations through `everett devices revoke`, best
  effort `logout --revoke-device`, and doctor cleanup of matching tagged SSH authorizations.
  Preserve unrelated keys and existing SSH UUIDs; registration binds the SSH device ID and
  key with a signature by that SSH key (set once, unique per owner), so revocation prunes
  exactly that key even when the SSH ID differs from the relay agent ID. Doctor pages all
  tombstones; `revokedAt` is optional so existing rows need no backfill. Offline peers
  retain access until their next successful revocation fetch.
- Add paged `everett devices [--json]` and registry address resolution for `peer add NAME`.
  Publish signed display-name/address metadata on `relay register` and signed-in SSH serve
  startup with a declared `[peer] address` or `--address`. Explicit address conflicts fail
  closed; first pairing still requires both out-of-band fingerprints and never refreshes pins.

## 1.6.0 — 2026-10-15

- Require host-bound MCP caller identity for delivery and replies; reject conflicting model
  session IDs instead of allowing them to replace provenance or hop lineage. Harnesses
  without native detection must bind `EVERETT_SESSION_ID` in their MCP launch environment.
- Reconcile the account/device and agent relay implementations into one v2 encrypted
  transport. Preserve agent MCP identity/pairing, grants, send and status tools with
  authoritative per-agent revocation and explicit bilateral fingerprint confirmation.

- Add bilateral admin-only named-agent cross-org grants: `relay grant invite`, `accept`,
  `reconfirm`, `revoke`, `get`, `list`; pin both bundles/fingerprints, optional expiry,
  either-side revocation, per-grant shared rate and serialized-body caps. Key changes
  block until independent bilateral reconfirmation and explicit local re-pins.
- Encrypt message/reply bodies with independent X25519 + HKDF-SHA256 +
  ChaCha20Poly1305 keys and sign envelopes/bundles with independent relay Ed25519 keys,
  not SSH identity keys. Signed visible routing binds owners, grant, fingerprints and
  parent/hop lineage. Preserve external/untrusted agent-data labels in local inbox,
  CLI/hooks/MCP, replies and forwarding; refuse provenance-less external SSH forwarding.
- Persist ID tokens at login/refresh; keep local/SSH commands usable without sign-in.
  Add encrypted exact-ciphertext retry/staging, permanent replay/idempotency metadata,
  observed queued/delivered/claimed/replied states, three-hop end-to-end lineage,
  one-hour undelivered client TTL and bounded cron cleanup. Signed delivery deletes the
  live ciphertext row; state observations are signed metadata, not encrypted receipts.
- Add SSH-only cross-device peers: `peer add`, `serve`, `ls`, `card`, `status`, `identity`,
  `list`, `remove`, and `revoke`, plus `ls --peer` and explicit remote inbox sends.
  Pairing verifies host/device fingerprints, pins host keys, and installs a tagged restricted
  forced command in the receiver's real `~/.ssh/authorized_keys`.
- For SSH, return cross-device replies only through status polling, with idempotent send/reply recovery,
  observed queued/delivered/replied states, a three-hop end-to-end limit, 64 KiB request frames,
  and 20-second deadlines. Offline peers fail without a local retry queue.
## 1.5.0 — 2026-10-10

- **Breaking:** remove the Python package (`everett/`, `pyproject.toml`, `bin/everett`); `pipx install`
  no longer works. Install with `install.sh`, Homebrew, or `cargo install`. `install-hooks --apply` and
  `install-mcp --apply` back up and replace Python hook commands, `-m everett mcp` launchers (dropping
  their `PYTHONPATH`) and the Python OMP extension with the native binary.
- Port the Python test suite's behavior to `everett-rs/tests/`; `tests/release_journey.rs` replaces
  `scripts/verify_release.py`, and CI runs it against a release build.
- Fix the native stop hook comparing only the sub-second part of file mtimes, which made stale
  auto-card refresh effectively random. `trunk schedule` now runs `launchctl` with `PATH`/`HOME`.
- Add native `everett external add`, `list`, and `remove` for durable cloud-agent registrations,
  with cloud harness filters in `everett ls`.
- Register OpenAI dots and Grok Bot as durable external sessions, with inbox-only delivery,
  explicit polling, correlated replies, and no local provider resume or spawn.
- Add native `everett gateway` using the official Rust MCP SDK, replacing the Python gateway
  and its optional SDK dependency. The gateway has fixed identity, destination and project
  permissions, private stdio or authenticated loopback HTTP, and exact-ID-only backend delivery.
  Live account connections, automatic wake-up, and cross-device transport are not included.

### Local dashboard

- Add `everett dashboard` to the Rust binary: a Mac-first, loopback-only HTML
  dashboard with recent sessions, Everett cards, harness/state/search filters,
  inline details, and copyable CLI actions.
- Use a restrained orange dispatch-desk identity with responsive desktop and
  narrow-window layouts, keyboard focus, and reduced-motion support.
- Keep provider stores read-only and reject write methods and foreign Host
  headers. Account-backed hosted/cross-device access remains future scope.

- **Current T3 discovery** — read active v2 projections or the legacy store, map driver/native provider IDs, and fill missing-transcript sessions without fabricating transcripts. Filter deleted/archived projects and threads, deduplicate provider sessions, and apply the look-back/harness filters before listing limits.
- **T3 continuation** — keep sends inbox-only, report explicit polling instead of claiming hook pickup, and guide routes back to T3 rather than provider CLI resume. Synthetic Python/Rust journeys cover polling, replies, and shared memory after merge; real T3 MCP inheritance remains unverified.
- **Verification accuracy** — exercise the selected CLI's MCP launcher in the release journey; freeze the synthetic Devin ISO fixture's clock so its 72-hour window cannot expire.
- **Devin MCP registration** — `install-mcp --devin` supports the dedicated CLI MCP config on Linux/macOS and Windows, with existing dry-run, backup, repair, and idempotency behavior. Doctor and onboarding recognize the registration without adding native hooks or assuming a caller-id variable.
- **Stable Devin timestamp regression** — freeze the synthetic ISO timestamp fixture's clock so the default 72-hour look-back does not expire it.

### Audit hardening

- Detect stale hook and MCP registrations (moved interpreters, binaries, and
  script paths): `doctor` reports them with repair commands and
  `install-hooks --apply` replaces Everett's dead entries while preserving
  foreign hooks.
- Record the highest hop count delivered to a live session so fresh sends are
  still refused at the hop limit.
- Validate session ids inside `card_path` so transcript ids cannot escape the
  cards directory.
- Compare full modification times for auto-card freshness instead of the
  nanosecond field alone.
- Propagate inbox, event-log, and card-write failures to the CLI and MCP
  instead of reporting success.
- Advance the escalation-throttle stamp each scan so hooks check at most once
  per minute.
- Order the Devin sessions query by newest activity before the row limit.
- Deliver a single oversized inbox message truncated instead of dropping it.
- Keep user text that merely starts with `<` visible by allowlisting the known
  injected tag names.
- `launchctl bootout` before `bootstrap` when re-applying the merge schedule.

### Audit hardening (2)

- Lock inbox reads and writes per session and compact the inbox, `.done`, and
  stale live/state records on take; live records now expire after 24 h so a
  recycled PID cannot keep a dead session attached.
- Lock and atomically replace `subscriptions.json`; lock and atomically write
  `config.toml` with proper TOML escaping and non-NotFound read errors surfaced.
- Embed the absolute binary path in the generated OMP extension (basename
  fallback), bound its card-context spawn with the same 3 s timeout, and guard
  `markSession` so malformed hook context cannot break OMP.
- Contain MCP tool panics behind a JSON-RPC internal error; hooks write through
  a fallible writer, run under `catch_unwind`, and treat only a non-empty
  `EVERETT_SEND` as set.
- Parse naive ISO timestamps as local time like Python, escape `|` in the trunk
  cwd column, share the harness-binary check between `ls` and `send`, reject
  Grok pids ≤ 1, keep tail-window records that start on a line boundary, list
  Hermes databases that lack a `messages` table, kill the process group on send
  timeout, wrap spawn `mkstemp` failures, refuse a Pi resume without a session
  file, extract the resumable UUID from Codex rollout filenames, show the real
  `--session-id` shape in Grok dry-run output, and honor an empty OMP hook-event
  selection before the OMP early return.
- Honor `everett cards --hours` without `--regenerate-auto`.

## 1.4.0 — 2026-10-05

### Rust binary releases

- Tags `vX.Y.Z` build the Rust `everett` for macOS arm64/x86_64 and static Linux musl
  x86_64/aarch64, attach `everett-<target>.tar.gz` + `.sha256` to the GitHub release, and render
  a binary Homebrew formula (`everett.rb`). The tag must match `everett-rs/Cargo.toml`'s version.
- `install.sh` installs or updates the binary into `~/.local/bin` with checksum verification;
  `cargo install --git https://github.com/raitoxlol/everett everett` builds from source.
- CI runs `cargo test` on macOS and Linux. The crate now builds on Linux (`pid_alive` used the
  macOS-only `libc::__error`), and `doctor`'s MCP probe inherits the caller's environment like
  Python's, so it no longer writes `.everett/mcp.log` into the working directory.

### Devin CLI adapter

- `everett ls` and routing now include Devin CLI sessions, read from `sessions.db` under
  `$DEVIN_HOME` or the platform data dir (`~/Library/Application Support/devin/cli` on macOS,
  `~/.local/share/devin/cli` on Linux; both candidates are scanned). The database opens
  read-only like every other store.
- Sessions come from the `sessions` table (id, `working_directory`, title, created/last-activity;
  hidden and archived rows skipped). User asks come from `message_nodes` when the CLI build has
  it; otherwise `transcripts/<id>.json` (ATIF) carries the listing and enriches database rows.
  Epoch seconds, milliseconds, and ISO 8601 timestamps are all accepted.
- `send` resumes a Devin session headless with `devin --resume <id> --print <text>`; `--spawn`
  starts one with `devin -p <text>`. Devin has no Everett hooks, so there is no inbox delivery
  yet; `everett_ls`'s `harness` filter accepts `devin`, and `doctor` reports the store.
- The Devin CLI sets no caller session env var for MCP servers; callers pass `session_id` or set
  `EVERETT_SESSION_ID`, same as OMP.

## 1.3.0 — 2026-10-04

### First install and diagnosis

- Detect installed harness CLIs before a session store exists. Onboarding includes Grok MCP setup, cancels on EOF, and rejects macOS scheduling on Linux before writing configuration.
- `everett doctor` probes real stdio initialization and all 10 tools, checks registered launchers, detects disabled/stale/broken MCP entries, and prints concrete setup/repair commands.
- `everett install-mcp --repair --apply` refreshes stale Everett registrations with backups while preserving unrelated servers. Malformed configuration is reported rather than overwritten.
- Missing harnesses fail before idle waits with an installation hint. Failed Codex spawns clean their temporary output files.
- Python 3.10 installs the small conditional `tomli` parser; Python 3.11+ still has no third-party runtime dependencies.

### MCP, messages and memory

- Declare caller identity in the route schema and enforce advertised argument types, enums and ranges across all tools. Restore stdout after an in-process server exits.
- Close resume/spawn child stdin so harnesses cannot consume the MCP protocol stream. Codex interruption errors report unconfirmed delivery and advise checking the target before retrying; the cause of reported interruptions remains unconfirmed.
- Allow a larger `hours` window in MCP routing/sending and resolve exact session titles case-insensitively. Duplicate titles still require an id. T3 Code's resume guard explains the supported inbox path.
- Log tool/argument names and status instead of argument values or error bodies. Existing older log entries remain untouched.
- Keep facts learned during a merge pending for the next merge, prevent simultaneous mergers, and preserve queued facts on failed or invalid model output. Deterministic merges remain available with `--llm none`.
- Reap detached notification subprocesses and preserve the harness PATH in generated macOS merge schedules.

### Verification and documentation

- Add routing, two-way inbox/reply, status subscription and global/project memory verification through real CLI commands and all 10 stdio tools, with disposable HOME directories and saved JSON evidence.
- Add regression coverage for first-run setup, MCP repair/validation, child stdin, older/title-based targets and concurrent memory operations. GitHub Actions runs unit tests and fresh wheel installs on macOS/Linux and Python 3.10/3.14, plus Linux 3.12.
- Package tests, fixtures, the release verification helper and release documents in the source distribution. Verify installed metadata matches console/module versions.
- Replace the README opening with a three-command pipx quickstart, demo placeholder, harness list and troubleshooting. Document manual inbox polling, hook trust, local/provider boundaries and macOS-only scheduling honestly.
- Clarify local routing versus provider calls in onboarding. Verify declining its final confirmation on both macOS and Linux without assuming a fixed number of prompts.

## 1.2.0 — 2026-09-25

Install: `brew install raitoxlol/tap/everett` or `pipx install git+https://github.com/raitoxlol/everett`

### Onboarding: Jev key + nightly merge
- `everett onboard` adds a "Smarter routing (optional)" step (curses TUI, plain fallback, and `--yes`), between MCP and backfill: two lines explain that Jev (typesafe.ai) picks the right session when many are running, and that without it Everett uses local matching.
- Paste a key or skip. If a key is already found (`TYPESAFE_API_KEY`, `~/.everett/config.toml`, or `~/.hermes/.env`), the screen shows "Jev key found ✓ (source)" and defaults to skip.
- A pasted key is validated with one test route call (5 s timeout); on success it's saved, on failure you're offered "keep it anyway" or "skip". The key is masked while typing (curses `*`, plain `getpass`) and is never echoed or logged — only the saved/validated status appears in the summary.
- The key is written as `typesafe_api_key` in `~/.everett/config.toml` (created `chmod 600`; every other key and `[section]` is preserved). `--yes --jev-key-env VAR` reads the key from an env var for scripted setup.
- The same step carries an optional "merge shared memory nightly" toggle (see below); `--yes --schedule-merge` opts in non-interactively. The onboarding step count and progress rail updated (6 → 7 steps).

### Nightly auto-merge for the shared core
- `everett trunk schedule [--at HH:MM] [--remove] [--llm claude|codex|none]` prints (default) or, with `--apply`, installs/removes a macOS LaunchAgent (`~/Library/LaunchAgents/dev.everett.core-merge.plist`) that runs `everett trunk merge --llm <x>` nightly (default 04:00, default `claude`). `everett trunk merge` already skips cleanly when the inbox is empty. Logs go to `~/.everett/logs/merge.log`.
- `--apply` writes the plist and runs `launchctl bootstrap`; `--remove --apply` runs `launchctl bootout` and deletes the plist. Every path and the launchctl call are injectable (`trunk_schedule.install/remove(path=, runner=)`), so tests never touch the real `~/Library` or `launchctl`.
- `everett doctor` reports whether nightly merge is scheduled.

## 1.1.0 — 2026-09-25

First public release. It includes the 1.0.0 work prepared on 2026-09-24, which was never published on its own.

### Sessions
- Lists Claude Code, Codex, OMP, Pi, Hermes Agent, and Grok CLI sessions from the stores those harnesses already write. Everett never writes to them. It reads only the first and last 64 KB of transcript files and opens SQLite stores with `mode=ro`.
- Hermes: `~/.hermes/state.db` and `~/.hermes/profiles/*/state.db`. The profile name is kept. Chat-platform sessions (Telegram, Discord, and so on) are list/route only. Cron, oneshot, and webhook runs are hidden as automated.
- Pi: `~/.pi/agent/sessions/<cwd-slug>/*.jsonl`, in the same format as OMP.
- Grok CLI: `~/.grok/sessions/<url-encoded cwd>/<id>/`. Everett reads `summary.json` for the id, folder, title, and `session_kind`, and `prompt_history.jsonl` for the typed requests, falling back to `<user_query>` blocks in `chat_history.jsonl`. A session is running while `~/.grok/active_sessions.json` names it with a live pid. Headless runs are hidden as automated.
- T3 Code: each thread runs Codex, Claude Code, or Grok underneath, and `~/.t3/userdata/state.sqlite` names the underlying session. Those sessions are marked source `t3code` (`[t3code]` in `ls`) and take the thread title when they have none, so nothing is listed twice. Codex sessions started by T3 are also recognized by their `t3code_desktop` originator.
- `everett ls [--json] [--all] [--harness H]`. `--harness` filters before the 40-session cap.

### Jev first
- `everett_send` (and `everett send`) route by default: omit `to` and Everett picks the session (jev when a key is configured, else the local matcher) instead of an agent eyeballing `everett_ls` and sending straight `to` an id. `to` is now documented as for a named session or a reply only. The MCP server instructions and the `everett_ls` / `everett_route` / `everett_send` tool descriptions say so explicitly, and `everett_ls`'s says it is for overview, not for picking a send target.
- The `everett_send` response and the `everett send` CLI report the routing decision (router `jev`/`local`, chosen session, confidence): the CLI prints `routed by jev → [claude] ~/src/api — ... (0.87)` before delivering. On an `ASK` decision nothing is sent; the response now carries `candidates` (up to 3 close sessions) instead of a single `suggested` guess.

### Routing and delivery
- Local router (default without a Jev key): BM25 over each session's card, title, first and last requests, and project folder, weighted by recency. The Jev router is used when a key is set. Both return `SESSION` / `NEW` / `ASK` with a confidence.
- `everett send` waits for the target to go idle (up to 2 minutes), then resumes it headless and prints the reply. `--to <id-prefix|name>` skips routing. `--spawn` starts a new headless session on a confident `NEW` and records it in `~/.everett/spawned.jsonl`.
- Resume and spawn commands: `claude --resume` / `--session-id`, `codex exec resume` / `codex exec`, `omp -r`, `pi --session`, `hermes -p <profile> chat --resume`, and `grok --resume <id> -p` / `grok --session-id <uuid> -p`. `send` refuses T3 Code sessions, because T3 keeps its own resume point and a CLI resume would fork the thread.
- Safety: a hop guard (`EVERETT_HOPS`, max 3, exit 7), refusal to send to the calling session, and spawning only on request.

### Live delivery
- Running sessions are reachable. `send --mode auto` (the default) puts a request for a session with a live harness process into `~/.everett/inbox/<session-id>.jsonl` instead of refusing it or forking it with a headless resume; idle sessions are still resumed headless. `--mode resume|inbox` forces a path. T3 Code sessions now take inbox delivery instead of being refused.
- The session's own hooks inject pending messages (at most 5 per hook, 6,000 characters) marked as coming from Everett with the sender's session and card: Claude Code and Codex at `UserPromptSubmit` and `PostToolUse` (`hookSpecificOutput.additionalContext`; the Codex contract checked against the 0.155.1 binary's embedded schemas), Grok at `PostToolUse` only (it discards `UserPromptSubmit` context), and OMP through its extension (`before_agent_start`, plus `tool_result` steering). About 40 ms per hook, silent on every error.
- Replies: `everett reply <id> "<text>"` or `everett_send(reply_to=…)` deliver to the sender's inbox. `send --wait S` / `everett_send(wait=S)` wait for the reply. `everett inbox` and the `everett_inbox` MCP tool read an inbox directly (for Pi, Hermes, and humans). Reply chains stop at 3 hops.
- `install-hooks` adds the new hooks (backup first, idempotent). `install-mcp --grok` registers the MCP server in `~/.grok/config.toml`. `everett onboard` offers the new hooks.
- Not used: Claude Code's own `<cross-session-message>` socket channel, which has no public way to send.

### Events
- `everett event done|blocked|needs-input|info "<msg>" [--project P]` and `everett_event`. Stored in `~/.everett/events.jsonl`, with each session's current state (and since when) in `~/.everett/state/`. `everett ls` flags blocked and waiting sessions (`⚠ blocked 32h: waiting on staging key`), `everett_ls` returns `state` for each session, and `everett events [--since 24h]` lists them.
- Stop hooks detect events deterministically from the turn's last assistant message: a closing question or an ask ("need you to", "should I", "please confirm") → `needs-input`; "blocked" / "stuck" / "waiting on" / "can't proceed" (not negated, not asked) → `blocked`; otherwise `done`. Repeats of the current state within 10 minutes are dropped.
- `everett subscribe <session|project>` / `everett_subscribe` route events into the subscriber's inbox. `blocked` and `needs-input` notify the human: a macOS notification by default, plus `notify_command` (config or `EVERETT_NOTIFY_COMMAND`) for your own channel, and one escalation after `escalate_minutes` (default 30). `everett events --check` runs the escalation from cron.

### Cards and shared core
- Session cards in `~/.everett/cards/`: the SessionStart hook asks the agent to write one, and the Stop hook writes a deterministic fallback. `everett install-hooks [--claude] [--codex] [--omp] [--grok] [--apply]` backs up the config first and merges idempotently. Grok gets the Stop hook only (`~/.grok/hooks/everett.json`), because Grok ignores `SessionStart` output.
- Shared core: `everett learn` (secret-filtered, capped at 500 characters), `everett trunk merge [--llm claude|codex|none]` (capped at 300 words, with history snapshots), and `everett core`. SessionStart hooks for Claude Code, Codex, and OMP inject the global and project core, at most 350 words.

### For agents
- `everett mcp`: a stdio MCP server built on the standard library, with the tools `everett_ls`, `everett_route`, `everett_send`, `everett_learn`, `everett_core`, `everett_card`, and `everett_whoami`. Calls are logged to `~/.everett/mcp.log`. `everett install-mcp` registers it for Claude Code, Codex, and OMP.
- Caller identity comes from `CLAUDE_CODE_SESSION_ID`, `CODEX_THREAD_ID`, `HERMES_SESSION_ID`, `PI_SESSION_FILE`, or `GROK_SESSION_ID`, and `EVERETT_SESSION_ID` overrides them all.

### Setup
- `everett onboard`: a friendly first-time-setup TUI (curses, with a plain-prompt fallback when not a TTY) -- welcome, detected harnesses, per-hook toggles with the exact files that will change, MCP registration toggles, and an optional card-backfill step (deterministic AUTO cards, no LLM calls) with a progress bar. Nothing is written before the final confirm. `--yes [--span-days N] [--no-backfill] [--no-mcp]` runs it non-interactively.
- `~/.everett/config.toml` (`vault`, `vault_dir`, `default_harness`, `router`, `merge_llm`, Jev key), with environment overrides. No vault path is hardcoded.
- `everett doctor` shows the Python version, session stores (including Grok and T3 Code), hooks, MCP registration, card coverage, router, and vault.
- Install with `pipx install git+https://github.com/raitoxlol/everett` (the package is not on PyPI), `python -m everett`, and `everett --version`. No runtime dependencies. MIT license.

### Verified
Live round trips on throwaway sessions (2026-09-24):
- Claude Code `send` replied in 10.3 s. Codex `send` replied in 43.8 s, including the idle wait; an earlier attempt was correctly refused as busy. OMP `send` replied in 60.5 s.
- `spawn('claude', ...)` replied in 5.6 s, and the new session appeared in `everett ls`.
- Not live-checked: the Hermes, Pi, and Grok resume and spawn commands, which come from each CLI's `--help` and docs. Pi was not installed on the test machine. The Grok and T3 Code listings were checked read-only against real stores.

## 0.x — 2026-09-23

- Session registry for Claude Code, Codex, and OMP; Jev router; `send` with idle wait; session cards with SessionStart and Stop hooks; automatic fallback cards; `everett cards`; vault trunk note.
