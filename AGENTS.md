# Agent Instructions

## Repository Purpose

This repository owns Baixada's Truco HTTP API, hosted match/session lifecycle,
abuse limits, bot orchestration, and notation/exploration transport adapters.
It consumes public engine, bots, and executable-spec contracts.

## Boundaries

- `crates/truco-server` owns HTTP routes, in-memory hosted sessions, quotas,
  environment parsing, bot hosting, and transport-level errors.
- `baixada-cards/truco-engine` owns rules and game-state semantics.
- `baixada-cards/truco-bots` owns bot behavior, provider transports, and TPB1
  policy loading.
- `baixada-cards/truco-spec` owns notation fixtures and contracts.
- Product UI/BFF code, CFR training, policy artifacts, deployment manifests,
  live cloud state, credentials, and licensed media do not belong here.

## Workflow

- Run `make check` before wrapping up a change.
- Use `sfw` for public-registry dependency fetches.
- Keep Cargo and contract locks exact and never use moving Git branches.
- Sign commits.
- Keep dev-only routes disabled in production and document any change to
  session ownership, expiry, quota, provider-key handling, or policy mounting.
- Never commit provider credentials, cookie/session secrets, `.env` files,
  policy bundles, resource identifiers, or live inventories.

## Verification

- Full-SHA GitHub Actions and exact contract locks.
- Rust formatting, Clippy with warnings denied, and all targets/tests.
- Materialized `truco-spec` manifest and notation fixtures.
- Provider tests use local mocks; no billable or credentialed calls.
- Session expiry/quota/dev-route changes require direct route tests.
