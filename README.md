# Truco Server

HTTP and hosted-session adapter for Baixada's two-player Brazilian Truco.

The server keeps transport, session lifecycle, abuse limits, provider-backed
opponents, and solved-policy mounting outside the rules engine. It consumes
exact public revisions of:

- [`baixada-cards/truco-engine`](https://github.com/baixada-cards/truco-engine)
  for authoritative rules, views, exploration, and notation operations;
- [`baixada-cards/truco-bots`](https://github.com/baixada-cards/truco-bots)
  for gameplay, provider, and policy-backed bots;
- [`baixada-cards/truco-spec`](https://github.com/baixada-cards/truco-spec)
  for executable notation fixtures.

All revisions and the spec manifest digest are immutable in
[`contracts.lock.json`](contracts.lock.json) and
[`spec.lock.json`](spec.lock.json).

## Development

Prerequisites are stable Rust with Clippy and rustfmt, Python 3.11+ for the
standard-library contract synchronizer, and
[Socket Firewall Free](https://docs.socket.dev/docs/socket-firewall-free).

```sh
make sync
make check
cargo run -p truco-server
```

The service listens on `127.0.0.1:4000` by default. Override it with
`TRUCO_SERVER_BIND`; the old `TRUCO_ENGINE_SERVICE_BIND` name remains a
temporary compatibility fallback. When neither variable is present, Cloud
Run's `PORT` variable is honored automatically on all interfaces.

Route families and runtime settings are documented in
[`crates/truco-server/README.md`](crates/truco-server/README.md).

## Runtime security

This repository contains no deployment credential, provider key, live policy
bundle, cloud inventory, or purchased media. Provider keys can be supplied by
the calling user at runtime; tests use local mocks. Production secrets,
immutable policy bundle checksums, infrastructure state, and deployment
identity live in the private `baixada-ops` boundary.

Dev-only state injection and private-view routes are disabled when
`NODE_ENV=production`. Production deployments should set this explicitly and
configure session TTL and quota variables described in the crate README.

## Container

The checked-in multi-stage image builds the exact locked dependency graph with
Socket Firewall, copies only the release binary and runtime certificates into
the final Debian image, and runs as an unprivileged user. It contains no Cargo
cache, source checkout, credentials, provider keys, policy bundle, or
deployment metadata.

## Versioning

The API is pre-1.0 and follows Semantic Versioning. A release records the exact
engine, bots, and spec revisions it passed against.

## License

MIT. See [`LICENSE`](LICENSE).
