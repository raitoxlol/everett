---
name: everett
description: Discover the owner's sessions and exchange authorized handoffs through the single hosted Everett app.
---

Use this plugin's hosted Everett app. Its existing owner connection supplies
authentication. Keep private identity handles and registration keys within this
chat. Never inspect or copy credential files.

First call `everett_whoami`. Verify the logical identity, exact gateway resource,
and both `runtime` and `gateway_runtime` build revisions. A running process,
capability flag, or queue receipt does not prove native dispatch or completion.

During setup, register this conversation once with `everett_identity(action=create)`
and a fresh private random registration key. Use `kind=dot` for a Dot. Retain the
returned handle privately and pass it on subsequent calls. Reuse the same key
only to retry that registration. This assigns a logical inbox to the conversation.

Use full IDs from `everett_whoami.authorized_destinations` for inbox handoffs.
For Dot events, use the Events-enabled HTTP gateway. Set `arguments.identity_handle`
on `events/subscribe` to this Dot's private handle. Use the same handle for
pickup and reply. If the handle is unavailable, pause the event workflow and
report that the Dot's private inbox could not be selected.

For a handoff to an existing session or T3 Code thread (for example "send this
to my Teresis T3 Code session"), call `everett_directory` first. It lists only
the destinations this connection is authorized to reach, each with its exact
`id`, `kind`, `project`, `label`, `state`, and permitted `actions`. Pick the
entry whose `label` or `project` matches what the user named, then call
`everett_send` with that exact `id`; a T3 thread ID is valid and resolves to the
thread's provider session inbox (`resolved_to`). Entries with empty `actions`
are not sendable — read the `note` for the reason and report it instead of
guessing another target. `everett_start`, when advertised, only starts a NEW run
in an owner-approved project and never reaches an existing thread; do not
substitute it for an existing-thread handoff.

On a `message.created` event, call `everett_inbox` with `message_id` from the
event's `data.message_id` and the Dot's private `identity_handle`. If the client
schema lacks `message_id`, pause and report that the existing app's tools need
to be refreshed. An empty `messages` array means there is no pending work for
that event. If the returned `kind` is `message`, handle the request and call
`everett_send` with `reply_to` set to that message ID and the same
`identity_handle`. If `kind` is `reply`, treat it as a result and end the event
workflow. For any other or missing `kind`, stop at pickup.

For a requested native start, require both the registered `everett_start` tool
and `capabilities.native_session_start=true` on this connection. If the grant
exists but the tool is absent, report the stale client catalog. Do not substitute
local T3, another plugin, a shell command, or an inbox message for a hosted start.

An explicitly authorized start can consume the owner's model allowance. Use a
private identity handle and one fresh durable request ID. Retain the exact ID
and inputs before dispatch. Reuse them after any timeout or reconnect. The owner
fixes each project and provider and grants model and effort pairs. Read
`native_start_scope` from whoami with the registered private handle. Omit
`project` for the original fixed grant, or use an exact advertised approved alias.
If the named project is absent, report that it needs separate owner approval.
Never substitute a raw project ID, directory entry or another transport. When the
tool advertises model and effort, omit them to use the defaults or send a
granted pair. Verify `effective` against `requested` for a selection
claim. Legacy CLI receipts report `effective: null`.
`dispatch_not_started` confirms a preflight rejection. After resolving the
diagnostic, retry it with the original request ID and inputs.

Use `everett_start_status(identity_handle=<same handle>, request_id=<original ID>)`
when registered. It reconciles only this identity's journaled accepted run through
native T3 metadata. It cannot read an arbitrary thread or send/control anything.
Keep the immutable dispatch receipt separate from `reconciliation`. A T3 executed
model plus started timestamp establishes T3-reported provider pickup; terminal
status establishes the run outcome. Neither establishes an inbox reply. Missing
run metadata, unavailable transport and a legacy receipt leave pickup unknown.
Never use a thread card's model or status as evidence. `native_session_id` stays
null unless a supported source actually provides it.

A confirmed start receipt proves dispatch only. Independently verify the native
provider identity, intended target, inbox pickup and correlated reply before
claiming that the end-to-end test worked. Cached receipt replay and no duplicate
native launch also need evidence. Synthetic fixtures do not prove hosted acceptance.

Plugin builds and updates are background file operations. Do not open windows,
change focus, restart client apps, alter other installed plugins, or publish a
package as implicit setup. Preserve the user's current authorization and UI
constraints. Report the exact remaining client operation when one is required.
