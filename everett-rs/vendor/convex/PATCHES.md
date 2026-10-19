# Convex 0.10.4 transport patch

Source: crates.io `convex` 0.10.4, Apache-2.0. All protocol, authentication,
subscription, retry, and value code is unchanged.

- `ConvexClientBuilder::with_connector` carries a per-client TLS connector into
  every WebSocket connection, including reconnects.
- `WebSocketState::TlsError` reports TLS handshake failures through the existing
  state channel. The error retains its source instead of becoming only a log.

Everett supplies a rustls connector with WebPKI roots and optional explicit PEM
roots. There is no permissive verifier or change to process-wide TLS settings.
Remove this vendored copy when the upstream SDK provides both hooks.
