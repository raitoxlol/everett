# Shared memory and other memory tools

Everett core holds short facts that agents need across sessions. Basic Memory holds longer notes and their relationships. Power Proxy discovers and calls tools supplied by either service. Each has its own storage and permissions.

| Component | Owns | Does not imply |
| --- | --- | --- |
| Everett core | Pending learnings, distilled global and project facts, merge history | A complete notes archive or memory synchronization between devices |
| Basic Memory | Markdown notes and an index for searching notes and following their relationships | That an Everett learning has been imported or corrected there |
| Power Proxy | Tool discovery, schema lookup, call policy, and dispatch to configured sources | A memory store, a shared login, or synchronization between those sources |

Basic Memory uses Markdown as the source of truth and a database as a secondary index. Its [technical reference](https://docs.basicmemory.com/reference/technical-information) describes that model and its MCP tools. It is an optional, separately installed service. Everett remains MIT and does not embed Basic Memory or copy its implementation.

Power Proxy exposes `search`, `describe`, and `call` for configured upstream MCP sources. Its [source configuration and call policy](https://github.com/raitoxlol/power-proxy#configure-sources) apply independently of Everett. Provider authentication stays with the provider or its owning client.

## Pull the current core during a session

Call `everett_core` again when you need the current global and project core. Each call reads the files again, including changes merged by another session after the MCP connection started. The read does not run a merge or consume the pending learning inbox.

From the CLI, run `everett core`. To include pending facts first, run `everett trunk merge --llm none`, then read the core. The deterministic merge stays local. An explicitly selected `--llm claude` or `--llm codex` merge invokes that harness and may contact its provider.

The short injected context and distilled files retain their word caps. Increasing searchable memory is tracked separately in [shared-memory issue #68](https://github.com/raitoxlol/everett/issues/68).

## Use the tools together

Keep detailed reasoning and evidence in your canonical notes. Put only the durable fact that another agent needs in Everett, with a note path or link when useful. Do not put credentials in either store.

If Basic Memory indexes the vault configured for Everett, it can index the generated `Core.md` mirror as a note. That is a generated view. Edit the underlying Everett facts through Everett, then regenerate the mirror with a merge. Editing or deleting the mirror alone does not change Everett's local core or pending inbox.

When using Power Proxy, search for the specific task, inspect the source's availability and permission result, describe the selected capability, then call it. A source listed as unavailable has no callable tools. A successful Everett call does not establish Basic Memory access. This documentation does not configure either source or claim a live round trip between them.

There is no automatic import, bidirectional reconciliation, or shared deletion between Everett and Basic Memory. A fact copied into a canonical note must be corrected there too. Merge-history backups can retain older fact text. They are audit records and are not searched or restored automatically by Everett's core read.

## Local storage and encrypted synchronization

Everett stores the active core, pending learnings, project files, and merge history under `~/.everett/core/`. `EVERETT_HOME` selects the home containing that directory. A configured vault receives a generated mirror.

The current encrypted device relay carries messages and replies. It does not synchronize core files, the memory inbox, or the vault. Pairing devices or signing into the same account does not copy their memory. The existing [cross-device trust and revocation rules](cross-device.md) remain in effect.

Encrypted memory synchronization is still tracked in #68. It needs a distinct authenticated payload, approved device pins, replay protection, and versioned changes that carry corrections, deletions, and expiry. A stale device must not restore a forgotten fact. Project names, fact bodies, source paths, and memory indexes must remain inside the encrypted payload. The hub may retain ciphertext and routing metadata, with account ownership derived from authenticated identity.

Do not use plaintext hub storage or a blind file overwrite as a shortcut. Until synchronization is implemented and accepted between two real devices, treat each device's core as local.
