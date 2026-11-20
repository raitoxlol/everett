# Everett 2.4.0

Everett can route a request to a local coding session, show whether it was picked up, and retrieve the retained reply. When several sessions match, use an address from the route results or an exposed card to choose the recipient (#231, #232).

Pi connects during onboarding, and Hermes users get local setup instructions (#229). The skill hub selects a playbook for a task, and Everett recognizes Devin threads managed by T3 Code (#228).

Local CLI setup and routing work without an Everett account. Run `everett login` once per profile for local MCP tool calls. Grok CLI and Devin headless turns depend on provider access. Outside-hosted inbox and wake integrations remain in preview and require separate acceptance on the configured connection.

Install the runtime on Linux or macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/v2.4.0/install.sh -o everett-install.sh
EVERETT_VERSION=v2.4.0 sh everett-install.sh
everett --version
```

See the [full changelog](https://github.com/raitoxlol/everett/blob/v2.4.0/CHANGELOG.md).
