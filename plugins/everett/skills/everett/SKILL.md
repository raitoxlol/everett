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

Register this chat with `everett_identity(action=create)` and a fresh private
random registration key. Retain the returned handle privately and pass it on
subsequent calls. Reuse the same key only to retry that registration. A chat
identity is a logical inbox, not a native harness session or vendor attestation.

Use full IDs from `everett_whoami.authorized_destinations` for inbox handoffs.
On a `message.created` event, call `everett_inbox` with the event's `message_id`
and this chat's private `identity_handle`. Act on the returned message and reply
with `everett_send(reply_to=...)` using that same message ID. An empty result
means there is no pending work for that event in this inbox. Keep unrelated
messages pending.

For a requested native start, require both the registered `everett_start` tool
and `capabilities.native_session_start=true` on this connection. If the grant
exists but the tool is absent, report the stale client catalog. Do not substitute
local T3, another plugin, a shell command, or an inbox message for a hosted start.

An explicitly authorized start can consume the owner's model allowance. Use a
private identity handle and one fresh durable request ID. Retain the exact ID
and text before dispatch. Reuse both after any timeout or reconnect; never
change IDs to retry an uncertain launch. The owner fixes the project, provider
and model. Do not ask for broader grants or expose general remote control.

A confirmed start receipt proves dispatch only. Independently verify the native
provider identity, intended target, inbox pickup and correlated reply before
claiming that the end-to-end test worked. Cached receipt replay and no duplicate
native launch also need evidence. Synthetic fixtures do not prove hosted acceptance.

Plugin builds and updates are background file operations. Do not open windows,
change focus, restart client apps, alter other installed plugins, or publish a
package as implicit setup. Preserve the user's current authorization and UI
constraints. Report the exact remaining client operation when one is required.
