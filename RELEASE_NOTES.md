# Everett 2.4.1

An authorized agent can start a new T3 run in the owner's fixed default project or an approved project alias, then check only that accepted run with `everett_start_status` (#282). The Muse VM is a supported external inbox, and Muse Code is documented as a local harness (#277, #278, #283).

This release also includes the Muse Code local harness, four MCP tools (`search`, `peers`, `broadcast`, `tasks`), Teresis-backed `everett recall`, the native Hermes plugin, `everett_directory`, and the other unreleased fixes accumulated since 2.4.0. See the changelog for the full list.

Install the runtime on Linux or macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/v2.4.1/install.sh -o everett-install.sh
EVERETT_VERSION=v2.4.1 sh everett-install.sh
export PATH="$HOME/.local/bin:$PATH"
everett --version
```

See the [full changelog](https://github.com/raitoxlol/everett/blob/v2.4.1/CHANGELOG.md).
