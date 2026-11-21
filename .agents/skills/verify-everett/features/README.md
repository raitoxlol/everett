# Everett feature map

| Feature | Evidence |
| --- | --- |
| [Install and MCP discovery](install.md) | Version; first-run onboarding; all advertised tools executed |
| [Routing and cards](routing.md) | Both CLI and MCP select the synthetic upload session; cards exist |
| [Messages and status](messaging.md) | Requests and replies cross sessions; consumption and event subscriptions work |
| [Skill hub](skills.md) | `cargo test --test skills`: list, get, match and shortlist across harness, extra and project folders; scored cases end with `failed=0` |
| [Shared memory](memory.md) | Facts queue, merge without a model, and appear in global/project core |
| Owner-authorized native gateway start | `cargo test --test gateway_native`: default denial, exact OAuth account/client and private identity, fixed T3 project/provider/model, stdin prompts, validated dispatch, concurrent retries, reconnect/crash reservations, private journal and unchanged inbox-only send grants. Synthetic issuer/CLI only; no hosted or model acceptance. |

The release journeys use `everett-rs/tests/release_journey.rs`; offline diagnostics also use `everett-rs/tests/relay_diagnostics.rs`. See the parent skill for invocation and isolation.

Gateway and tunnel subprocess journeys also accept `EVERETT_VERIFY_BINARY` to
drive an actual packaged binary. Run `cargo test --test gateway_native --test tunnel`
with that absolute path set. They retain isolated stores and synthetic issuer/T3/
tunnel subprocesses; no model or production connection is invoked.
