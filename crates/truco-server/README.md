# truco-server

Thin HTTP adapter over `truco-engine`.

## Route Families

- Stateful routes keep `MatchState` in an in-memory store keyed by match id.
- Stateless routes accept explicit `MatchState` payloads and return derived results.
- Bot-backed stateful routes add service-side seat metadata and can auto-advance a random bot.

Both route families delegate to the same core operations so there is only one engine behavior surface.

## Bot Flow

The first playable bot path is:

1. `POST /bot-matches`
2. `POST /matches/:id/start-hand/random`
3. `GET /matches/:id/players/:player/view`
4. `POST /matches/:id/actions`

For bot-backed matches, the service auto-runs bot turns after random hand start and after human actions until control returns to the human player or the hand ends.

## Intended Use

- Use stateful routes for the app while we build the first playable product.
- Use stateless routes for tooling, debugging, tests, and reproducible workflows.

## Current Project Role

This service is no longer just future-facing infrastructure. It is already part of the real playable web path:

- the Next.js app talks to it through the backend-for-frontend
- bot-backed stateful matches power the current human-vs-bot loop
- exploration and notation tooling in the lab also depend on it

So service changes should now be evaluated partly through product-flow quality, not only through backend cleanliness.

## Development

Run the service:

```bash
cargo run -p truco-server
```

Override the bind address:

```bash
TRUCO_SERVER_BIND=127.0.0.1:4010 cargo run -p truco-server
```

## Session Lifecycle Config

The service reads these env vars at startup to control hosted live-session
expiry and abuse limits:

- `TRUCO_SESSION_IDLE_TTL_SECONDS` — idle timeout for hosted matches. `0`
  disables idle expiry.
- `TRUCO_SESSION_MAX_ACTIVE_PER_SUBJECT` — max active matches allowed for one
  anonymous browser subject. `0` disables the cap.
- `TRUCO_SESSION_CREATE_RATE_LIMIT_WINDOW_SECONDS` — rolling rate-limit window
  for match creation per anonymous browser subject.
- `TRUCO_SESSION_CREATE_RATE_LIMIT_MAX` — max match creations allowed inside the
  rolling window. `0` disables the rate limit.

Defaults are environment-sensitive:

- outside production (`NODE_ENV!=production`): idle expiry disabled and quotas
  relaxed
- in production (`NODE_ENV=production`): 30 minute idle TTL, max 5 active
  matches per subject, and max 10 creations per 5 minutes

When a hosted match expires, stateful routes now return `MATCH_EXPIRED` with
HTTP `410 Gone` so the Next.js BFF and UI can surface expiry as a first-class
state.

## Solver Opponent (Policy Artifacts)

The `solver` hosted bot kind plays the solved CFR average strategy from
mmap-ed policy artifacts. Configuration:

- `SOLVER_POLICY_DIR` — directory containing `manifest.json`
  (`truco-policy-bot/v1`) plus one `.tpb` file per solved
  `(score, turnup class, dealer)` profile, produced by
  `solve export-bot-policy`. Unset or unloadable ⇒ the solver opponent is
  unavailable and creating a solver match returns `SOLVER_BOT_UNAVAILABLE`
  (HTTP 409). `GET /engine/solver-bot` reports `{ enabled, profiles }` so the
  frontend can gate the launcher option.

Matches against the solver must start at 10×10 (the earliest score whose
entire remaining game is solved — every continuation lands in the solved
mão-de-onze row or ends the match). The Next.js BFF enforces that starting
score when the solver kind is selected. Decisions outside the covered region,
or at info sets missing from the artifacts, fall back to the heuristic bot and
log a `solver bot fallback` line to stderr.

### Seeded Matches (Study Lab → Live)

`POST /bot-matches/seeded` hosts a live match starting from a specific
study-lab position instead of a random deal. It is atomic: the specified
hero (and villain) hand is dealt, the analyzed line is replayed verbatim onto
the engine, and the bot resumes from that exact node in one call, so the
hosted match's observed-action log is faithful from birth (including
resolved raises the exported state alone forgets).

Request body is `CreateBotMatchRequest`'s bot options (`bot_kind`,
`bot_profile`, `bot_model`, `api_key`, `seed`, `owner_subject`) flattened
with a seed spec:

- `score`, `dealer`, `vira_rank`, `human_player` — the position's game state.
- `hero_hand` (optional) — the human's three cards as abstract class indices
  (0..=12). Omit to sample the human's unspecified cards from the same
  line-conditioned posterior as the villain's; committed plays stay fixed.
- `villain_hand` (optional) — the bot's exact hand as class indices, when the
  lab pinned it. Omit to sample it instead.
- `history` — the replayed line so far, each entry
  `{ seat, kind, class?, to? }` (a play carries `class`, a raise carries
  `to`; accept/fold/eleven actions carry neither).

Villain sampling, reported back as `villain_sampling`:

- `pinned` — `villain_hand` was given; dealt exactly.
- `posterior` — no pin; sampled from `prior(hand) × Π σ*(observed villain
  action)`, i.e. the combinatorial deal prior weighted by the equilibrium
  likelihood of the villain's actions along the replayed line, computed from
  the mounted policy artifacts.
- `prior` — no artifacts mounted, or the line has zero equilibrium mass
  (fully off-equilibrium); sampling degrades to the deal prior alone and
  says so rather than pretending otherwise.

The `solver` bot kind additionally requires artifact coverage of the spot;
without it the call returns `SOLVER_BOT_UNAVAILABLE` (HTTP 409), same as a
normal solver create. Any other bot kind (e.g. `heuristic`) works without
artifacts. A malformed spec or a history that does not replay onto the
engine returns `INVALID_SEED` (HTTP 400). Session quotas and rate limits
from "Session Lifecycle Config" apply the same as a normal create.

## Dev-Only Hosted Routes

The service disables dev-only hosted routes when `NODE_ENV=production`. That
includes exact state/spec replacement, manual hand injection, bot action
overrides, and private views for a non-human opponent. Outside production these
routes default on for local tooling; set `TRUCO_ENABLE_DEV_ROUTES=false` to
turn them off in a dev build.
