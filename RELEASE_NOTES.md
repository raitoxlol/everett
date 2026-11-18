# Everett 2.4.0

- Check whether an agent picked up a request, find its ID again, and recover its reply through the CLI or MCP.
- Send to a specific T3 thread, retry a retained request, and reuse a confirmed dispatch receipt. Everett also finds T3 sessions in a custom data directory.
- Claim inbox work before handing it to a reader. Hosted Dots can pick up the exact message named by an event and leave other requests pending.
- Read recent T3, Codex, and Devin transcript messages from the CLI, and search saved facts with structured output.

Use the pinned installer after the release is published. It chooses the archive for your Mac or Linux machine and verifies its checksum.

```sh
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/v2.4.0/install.sh -o /tmp/everett-install.sh
EVERETT_VERSION=v2.4.0 sh /tmp/everett-install.sh
everett --version
```

Hosted Events and exact pickup require an updated gateway and a current tool catalog. Verify a real Dot event, exact pickup, and reply on your connection before enabling the workflow for daily use.

See [CHANGELOG.md](CHANGELOG.md) for the detailed changes.
