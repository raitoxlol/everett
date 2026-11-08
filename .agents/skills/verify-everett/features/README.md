# Everett feature map

| Feature | Evidence |
| --- | --- |
| [Install and MCP discovery](install.md) | Version; first-run onboarding; thirteen advertised and executed tools |
| [Routing and cards](routing.md) | Both CLI and MCP select the synthetic upload session; cards exist |
| [Messages and status](messaging.md) | Requests and replies cross sessions; consumption and event subscriptions work |
| [Shared memory](memory.md) | Facts queue, merge without a model, and appear in global/project core |

The release journeys use `everett-rs/tests/release_journey.rs`; offline diagnostics also use `everett-rs/tests/relay_diagnostics.rs`. See the parent skill for invocation and isolation.
