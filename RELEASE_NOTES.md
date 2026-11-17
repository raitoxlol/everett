# Everett 2.3.2 maintenance update

- Fix hosted inbox reads that could mark messages delivered before the gateway rejected an oversized response. Budget the complete encoded MCP result before consumption, return bounded batches, and leave unreturned messages queued. Metadata that cannot fit returns an error before consumption.
- Warn that a timed-out or abnormally exited T3 dispatch may already have delivered the turn. Check the thread before retrying.
- Harden the repository's read-only tunnel-health diagnostic on Python 3.9 for fractional RFC3339 timestamps and recursive JSON input. The diagnostic does not establish model completion or a recovery-time guarantee.

Release archives are exercised on native Linux and macOS runners for both
architectures. Candidate workflow runs do not publish a release.

This maintenance update does not establish public GA acceptance. Hosted tool
registration, owner-scoped native start, receiver pickup and a correlated reply
are separate checks. A transport disconnect can strand an upstream request even
after local recovery. Reconcile sends and starts before retrying; never infer
completion from a healthy connection. Older clients may need one normal tool
refresh to adopt the stable destination schema. A new ChatGPT-mode GUI
conversation and outside-account acceptance require their own live evidence.

See CHANGELOG.md and the client-specific setup and verification documentation.
