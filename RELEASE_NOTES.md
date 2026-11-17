# Everett 2.3.1 maintenance update

- Use matching, fresh tunnel-client operator health to retain healthy connections rather than interrupting requests during scheduled renewal. Stale or unavailable health preserves bounded renewal. Configure the health endpoint as described in docs/gateway.md; no credentials are moved or changed.
- Accept actual T3 thread/run receipt IDs and expose live destination grants without changing the send tool schema for each grant update.
- Validate a downloaded executable before atomically replacing a working installation. Failed validation or activation preserves the previous binary.
- Build and exercise extracted release archives on native Linux and macOS runners for both architectures. Candidate workflow runs do not publish.

This maintenance update does not establish public GA acceptance. Hosted tool
registration, owner-scoped native start, receiver pickup and a correlated reply
are separate checks. A transport disconnect can strand an upstream request even
after local recovery. Reconcile sends and starts before retrying; never infer
completion from a healthy connection. Older clients may need one normal tool
refresh to adopt the stable destination schema. A new ChatGPT-mode GUI
conversation and outside-account acceptance require their own live evidence.

See CHANGELOG.md and the client-specific setup and verification documentation.
