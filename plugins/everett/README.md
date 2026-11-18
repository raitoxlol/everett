# Everett

One version-controlled hosted plugin with one existing Everett app dependency.
The package includes the hosted workflow skill and a nonsecret readiness check.
Its app reference reuses normal owner authentication. The older Everett, private
test and local plugin variants are not dependencies of this bundle.

## Build in the background

Use Python 3.9+ and the id of the owner's existing hosted Everett App SDK app:

```sh
python3 plugins/everett/build.py \
  --app-id YOUR_EXISTING_EVERETT_APP_ID \
  --output /private/output/everett-2.2.1.zip
```

The builder reads an explicit file list, includes exactly one `.app.json`
reference, and derives both supported manifest layouts from
`.codex-plugin/plugin.json`. It records source revision, dirty state and file
hashes in `provenance.json`. Equal source and app inputs produce equal archives.
An existing output is refused. No credential files, tokens, connection handles,
local MCP command or runtime fallback are included.

Building does not install, publish, change grants, restart an app or manipulate
the desktop. Client import/update uses its normal plugin mechanism when
authorized. Existing installed variants are preserved. A bundle build does not
refresh an app's server-side tool catalog or establish client acceptance.

## Check the actual connection

Save the nonsecret `everett_whoami` result and tool catalog for the same hosted
connection. Run this from the repository checkout.

```sh
python3 plugins/everett/check.py \
  --whoami /private/evidence/whoami.json \
  --tools /private/evidence/tools.json \
  --resource https://YOUR_GATEWAY/MCP_PATH
```

The catalog uses MCP's `{"tools":[{"name":"everett_whoami"}, ...]}` shape.
Save the tools registered in your client.
The check rejects resource drift, unverified or mismatched builds, missing base
tools, and an enabled native-start grant whose client tool is missing. Add
`--require-native-start` to check that your connection has native start access.

For a Dot, pass `--require-event-pickup` and save the full tool schemas in
`tools.json`. The check requires the registered `everett_inbox` schema to accept
`message_id` and `identity_handle`. If it reports `event_pickup_client_catalog_stale`,
refresh the existing Everett app's tools in ChatGPT and save the updated catalog.
Verify the Events-enabled HTTP route separately. See the [Dot setup guide](https://github.com/raitoxlol/everett/blob/main/docs/chatgpt.md#receive-everett-events-in-a-dot).

Check native dispatch with a separate live test that verifies creation, target
identity, pickup, reply and duplicate-launch protection.

```sh
python3 -m unittest discover -s plugins/everett/tests -v
```
