use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use notation::{
    compile_notation_fragment, NotationCompileRequest, NotationCompileResponse, NotationContext,
    NotationParseError,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use truco_bot_core::{BotDecision, BotError};
use truco_bots::{LlmProviderBot, LlmProviderCatalog, LlmProviderError};
use truco_engine::{
    resolve_match_spec, Action, ActionResult, ApiMatch, EngineError, ExplorationError,
    ExplorationMatchSpec, HandStart, MatchConfig, MatchSnapshot, MatchState, PendingDecisionKind,
    Player, PlayerMatchView, Rank, Score,
};

use crate::bot_host::{
    auto_advance_bot_turns, log_observed_action, solver_policy_store, BotActionOverride,
    HostedBotKind, HostedMatchMode,
};
use crate::deal::{other_player, random_hand_start, random_hand_start_with_vira, seeded_rng};
use crate::notation;
use crate::session::{
    load_authorized_hosted_match, owner_subject_from_headers, save_hosted_match,
    save_new_hosted_match,
};
use crate::state::{AppState, HostedMatch};
use truco_bot_core::BotProfile;
use truco_policy_bot::seed::{
    build_seeded_hand, store_covers_spec, SeedError, SeedSpec, VillainSampling,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateMatchResponse {
    pub match_id: String,
    pub state: MatchState,
    pub public_view: MatchSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateBotMatchRequest {
    pub starting_dealer: Player,
    pub score: Score,
    #[serde(default)]
    pub human_player: Player,
    #[serde(default)]
    pub bot_kind: HostedBotKind,
    #[serde(default)]
    pub bot_profile: BotProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_model: Option<String>,
    // Bring-your-own-key for LLM bots. Never serialized back out (no
    // `Serialize` echo) and never stored on the session; used only to build the
    // bot. NOTE: transits from the BFF in plaintext.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_subject: Option<String>,
}

/// A study-lab position to host as a live match: the seed spec (score, dealer,
/// vira, hero hand, optional pinned villain hand, replayed line) plus the same
/// bot options as a normal create.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreateSeededBotMatchRequest {
    #[serde(flatten)]
    pub spec: SeedSpec,
    #[serde(default)]
    pub bot_kind: HostedBotKind,
    #[serde(default)]
    pub bot_profile: BotProfile,
    #[serde(default)]
    pub bot_model: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub owner_subject: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSeededBotMatchResponse {
    pub match_id: String,
    pub human_player: Player,
    pub bot_player: Player,
    pub bot_kind: HostedBotKind,
    pub state: MatchState,
    pub public_view: MatchSnapshot,
    /// How the villain's hand was chosen: pinned exactly, sampled from the
    /// equilibrium posterior, or prior-only (no artifacts / off-equilibrium).
    pub villain_sampling: VillainSampling,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateBotMatchResponse {
    pub match_id: String,
    pub human_player: Player,
    pub bot_player: Player,
    pub bot_kind: HostedBotKind,
    pub bot_profile: BotProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_model: Option<String>,
    pub state: MatchState,
    pub public_view: MatchSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchMetadataResponse {
    pub human_player: Player,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_player: Option<Player>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_kind: Option<HostedBotKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_profile: Option<BotProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_bot_decision: Option<BotDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_bot_override: Option<BotActionOverride>,
}

#[derive(Debug, Clone, Deserialize)]
struct BotOverrideBody {
    action: Option<BotActionOverride>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct StartRandomHandBody {
    #[serde(default)]
    defer_bot_eleven: bool,
    /// Dev-only: force the next hand's turnup to this rank (suit stays
    /// random). Rejected when dev routes are disabled.
    #[serde(default)]
    vira_rank: Option<Rank>,
}

#[derive(Debug, Clone, Serialize)]
struct BotOverrideResponse {
    ok: bool,
    pending_override: Option<BotActionOverride>,
}

#[derive(Debug, Clone, Copy, Default)]
struct StartHandOptions {
    defer_bot_eleven: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateMutationResponse {
    pub state: MatchState,
    pub public_view: MatchSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatelessActionRequest {
    pub state: MatchState,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatelessActionResponse {
    pub state: MatchState,
    pub result: ActionResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatelessStartHandRequest {
    pub state: MatchState,
    pub hand: HandStart,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("{0}")]
    Engine(#[from] EngineError),
    #[error("{0}")]
    Exploration(#[from] ExplorationError),
    #[error("{0}")]
    Notation(#[from] NotationParseError),
    #[error("{0}")]
    Bot(#[from] BotError),
    #[error("{0}")]
    BotProvider(#[from] LlmProviderError),
    #[error("match not found")]
    MatchNotFound,
    #[error("match expired after inactivity")]
    MatchExpired,
    #[error("match is owned by another browser")]
    MatchForbidden,
    #[error("too many active matches for this browser")]
    SessionLimitReached,
    #[error("too many matches created recently for this browser")]
    SessionCreateRateLimited,
    #[error("not found")]
    DevRouteDisabled,
    #[error("solver policy artifacts are not configured on this service")]
    SolverBotUnavailable,
    #[error("{0}")]
    Seed(#[from] SeedError),
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        match self {
            Self::Engine(error) => (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    code: error.code().to_string(),
                    message: error.to_string(),
                }),
            )
                .into_response(),
            Self::Exploration(error) => (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    code: error.code().to_string(),
                    message: error.to_string(),
                }),
            )
                .into_response(),
            Self::Notation(error) => (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "code": error.code(),
                    "message": error.to_string(),
                    "line": error.line,
                    "column": error.column,
                    "statement": error.statement,
                    "source_line": error.source_line,
                    "token": error.token,
                })),
            )
                .into_response(),
            Self::Bot(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: "BOT_ERROR".to_string(),
                    message: error.to_string(),
                }),
            )
                .into_response(),
            Self::BotProvider(error) => {
                let (status, code) = match error {
                    LlmProviderError::MissingConfig(_) => {
                        (StatusCode::BAD_REQUEST, "BOT_CONFIG_ERROR")
                    }
                    // The provider is out of credits/over its spend cap (402).
                    // Distinct so the UI can offer bring-your-own-key.
                    LlmProviderError::Exhausted(_) => {
                        (StatusCode::PAYMENT_REQUIRED, "LLM_PROVIDER_EXHAUSTED")
                    }
                    LlmProviderError::Transport(_) => {
                        (StatusCode::BAD_GATEWAY, "BOT_PROVIDER_ERROR")
                    }
                    LlmProviderError::InvalidResponse(_) => {
                        (StatusCode::BAD_GATEWAY, "BOT_PROVIDER_RESPONSE_ERROR")
                    }
                    LlmProviderError::Bot(_) => (StatusCode::INTERNAL_SERVER_ERROR, "BOT_ERROR"),
                };

                (
                    status,
                    Json(ErrorResponse {
                        code: code.to_string(),
                        message: error.to_string(),
                    }),
                )
                    .into_response()
            }
            Self::SolverBotUnavailable => (
                StatusCode::CONFLICT,
                Json(ErrorResponse {
                    code: "SOLVER_BOT_UNAVAILABLE".to_string(),
                    message: "solver policy artifacts are not configured on this service"
                        .to_string(),
                }),
            )
                .into_response(),
            Self::Seed(error) => (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    code: "INVALID_SEED".to_string(),
                    message: error.to_string(),
                }),
            )
                .into_response(),
            Self::MatchNotFound => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    code: "MATCH_NOT_FOUND".to_string(),
                    message: "match not found".to_string(),
                }),
            )
                .into_response(),
            Self::MatchExpired => (
                StatusCode::GONE,
                Json(ErrorResponse {
                    code: "MATCH_EXPIRED".to_string(),
                    message: "match expired after inactivity".to_string(),
                }),
            )
                .into_response(),
            Self::MatchForbidden => (
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    code: "MATCH_FORBIDDEN".to_string(),
                    message: "match is owned by another browser".to_string(),
                }),
            )
                .into_response(),
            Self::SessionLimitReached => (
                StatusCode::TOO_MANY_REQUESTS,
                Json(ErrorResponse {
                    code: "SESSION_LIMIT_REACHED".to_string(),
                    message: "too many active matches for this browser".to_string(),
                }),
            )
                .into_response(),
            Self::SessionCreateRateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                Json(ErrorResponse {
                    code: "RATE_LIMITED".to_string(),
                    message: "too many matches created recently for this browser".to_string(),
                }),
            )
                .into_response(),
            Self::DevRouteDisabled => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    code: "NOT_FOUND".to_string(),
                    message: "not found".to_string(),
                }),
            )
                .into_response(),
        }
    }
}

fn require_dev_routes_enabled(state: &AppState) -> Result<(), ServiceError> {
    if state.dev_routes_enabled {
        Ok(())
    } else {
        Err(ServiceError::DevRouteDisabled)
    }
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/matches", post(create_match))
        .route("/bot-matches", post(create_bot_match))
        .route("/bot-matches/seeded", post(create_seeded_bot_match))
        .route("/matches/from-state", post(create_match_from_state))
        .route("/matches/from-spec", post(create_match_from_spec))
        .route(
            "/matches/:id",
            get(get_match_state).put(replace_match_state),
        )
        .route("/matches/:id/meta", get(get_match_metadata))
        .route("/matches/:id/from-spec", put(replace_match_from_spec))
        .route("/matches/:id/start-hand", post(start_hand_stateful))
        .route(
            "/matches/:id/start-hand/random",
            post(start_hand_random_stateful),
        )
        .route("/matches/:id/bot-turns", post(advance_bot_turns_stateful))
        .route("/matches/:id/public-view", get(get_public_view))
        .route("/matches/:id/players/:player/view", get(get_player_view))
        .route("/matches/:id/legal-actions", get(get_legal_actions))
        .route("/matches/:id/actions", post(apply_action_stateful))
        .route("/matches/:id/bot-override", put(set_bot_override))
        .route("/engine/compile-notation", post(compile_notation_stateless))
        .route("/engine/llm-providers", get(list_llm_providers))
        .route("/engine/solver-bot", get(solver_bot_status))
        .route("/engine/resolve-spec", post(resolve_spec_stateless))
        .route("/engine/public-view", post(public_view_stateless))
        .route("/engine/players/:player/view", post(player_view_stateless))
        .route("/engine/legal-actions", post(legal_actions_stateless))
        .route("/engine/start-hand", post(start_hand_stateless))
        .route("/engine/actions", post(apply_action_stateless))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn create_match(
    State(state): State<AppState>,
    Json(config): Json<MatchConfig>,
) -> Result<Json<CreateMatchResponse>, ServiceError> {
    let response = create_match_response(&state, config).await?;
    Ok(Json(response))
}

async fn create_bot_match(
    State(state): State<AppState>,
    Json(config): Json<CreateBotMatchRequest>,
) -> Result<Json<CreateBotMatchResponse>, ServiceError> {
    let response = create_bot_match_response(&state, config).await?;
    Ok(Json(response))
}

/// Create a match seeded from a study-lab position: deal the specified hero
/// hand (and pinned or posterior-sampled villain hand), replay the analyzed
/// line, and let the bot continue from that exact node. Atomic — no separate
/// start-hand call — so the observed-action log is faithful from birth.
async fn create_seeded_bot_match(
    State(state): State<AppState>,
    Json(config): Json<CreateSeededBotMatchRequest>,
) -> Result<Json<CreateSeededBotMatchResponse>, ServiceError> {
    let store = solver_policy_store();
    if config.bot_kind == HostedBotKind::Solver {
        let store = store.as_ref().ok_or(ServiceError::SolverBotUnavailable)?;
        if !store_covers_spec(store, &config.spec) {
            return Err(ServiceError::SolverBotUnavailable);
        }
    }

    let mut rng = seeded_rng(config.seed, 0x5EED_0001);
    let seeded = build_seeded_hand(&config.spec, store.as_ref(), &mut rng)?;

    let human_player = config.spec.human_player;
    let bot_player = other_player(human_player);
    let mut hosted = HostedMatch::human_vs_bot(
        seeded.state,
        human_player,
        bot_player,
        config.bot_kind,
        config.bot_profile,
        config.bot_model,
        config.seed,
        config.owner_subject,
        config.api_key,
    )?;
    hosted.hand_action_log = seeded.log;
    // If the analyzed node has the bot to act, it moves before the human sees
    // the table — same contract as a normal start-hand.
    auto_advance_bot_turns(&mut hosted).await?;

    let match_id = state.allocate_match_id();
    let match_state = hosted.state.clone();
    let public_view = public_view_from_state(match_state.clone())?;
    save_new_hosted_match(&state, &match_id, hosted).await?;

    Ok(Json(CreateSeededBotMatchResponse {
        match_id,
        human_player,
        bot_player,
        bot_kind: config.bot_kind,
        state: match_state,
        public_view,
        villain_sampling: seeded.sampling,
    }))
}

async fn create_match_from_state(
    State(state): State<AppState>,
    Json(match_state): Json<MatchState>,
) -> Result<Json<CreateMatchResponse>, ServiceError> {
    require_dev_routes_enabled(&state)?;
    let response = create_match_from_state_response(&state, match_state).await?;
    Ok(Json(response))
}

async fn create_match_from_spec(
    State(state): State<AppState>,
    Json(spec): Json<ExplorationMatchSpec>,
) -> Result<Json<CreateMatchResponse>, ServiceError> {
    require_dev_routes_enabled(&state)?;
    let match_state = resolve_match_spec(&spec)?;
    let response = create_match_from_state_response(&state, match_state).await?;
    Ok(Json(response))
}

async fn get_match_state(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<MatchState>, ServiceError> {
    let stored =
        load_authorized_hosted_match(&state, &id, owner_subject_from_headers(&headers)).await?;
    Ok(Json(stored.state))
}

async fn get_match_metadata(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<MatchMetadataResponse>, ServiceError> {
    let stored =
        load_authorized_hosted_match(&state, &id, owner_subject_from_headers(&headers)).await?;
    Ok(Json(stored.metadata()))
}

async fn list_llm_providers() -> Json<Vec<LlmProviderCatalog>> {
    Json(LlmProviderBot::fetch_catalogs().await)
}

/// Whether the solver opponent can be offered: policy artifacts loaded and how
/// many profiles they cover. The frontend gates the launcher option on this.
async fn solver_bot_status() -> Json<serde_json::Value> {
    let store = crate::bot_host::solver_policy_store();
    Json(json!({
        "enabled": store.is_some(),
        "profiles": store.map(|s| s.profile_count()).unwrap_or(0),
    }))
}

async fn replace_match_from_spec(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(spec): Json<ExplorationMatchSpec>,
) -> Result<Json<StateMutationResponse>, ServiceError> {
    require_dev_routes_enabled(&state)?;
    let mut hosted =
        load_authorized_hosted_match(&state, &id, owner_subject_from_headers(&headers)).await?;
    hosted.state = resolve_match_spec(&spec)?;
    // A wholesale state replacement invalidates the observed-action log; the
    // solver bot falls back to the heuristic for the remainder of this hand.
    hosted.hand_action_log.clear();
    auto_advance_bot_turns(&mut hosted).await?;
    let response = replace_match_state_response(&hosted)?;
    save_hosted_match(&state, &id, hosted).await;
    Ok(Json(response))
}

async fn compile_notation_stateless(
    Json(request): Json<NotationCompileRequest>,
) -> Result<Json<NotationCompileResponse>, ServiceError> {
    let fragment = compile_notation_fragment(
        &request.notation,
        NotationContext {
            human_player: request.human_player,
            bot_player: request.bot_player,
        },
    )?;
    Ok(Json(NotationCompileResponse { fragment }))
}

async fn replace_match_state(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(match_state): Json<MatchState>,
) -> Result<Json<StateMutationResponse>, ServiceError> {
    require_dev_routes_enabled(&state)?;
    let mut hosted =
        load_authorized_hosted_match(&state, &id, owner_subject_from_headers(&headers)).await?;
    hosted.state = match_state;
    hosted.hand_action_log.clear();
    auto_advance_bot_turns(&mut hosted).await?;
    let response = replace_match_state_response(&hosted)?;
    save_hosted_match(&state, &id, hosted).await;
    Ok(Json(response))
}

async fn start_hand_stateful(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(hand): Json<HandStart>,
) -> Result<Json<StateMutationResponse>, ServiceError> {
    require_dev_routes_enabled(&state)?;
    let mut hosted =
        load_authorized_hosted_match(&state, &id, owner_subject_from_headers(&headers)).await?;
    let response = start_hand_response(&mut hosted, hand, StartHandOptions::default()).await?;
    save_hosted_match(&state, &id, hosted).await;
    Ok(Json(response))
}

async fn start_hand_random_stateful(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<StartRandomHandBody>>,
) -> Result<Json<StateMutationResponse>, ServiceError> {
    let body = body.map(|Json(body)| body).unwrap_or_default();
    if body.vira_rank.is_some() {
        // Forcing the vira is a dev affordance (it biases the deal); prod
        // clients cannot use it.
        require_dev_routes_enabled(&state)?;
    }
    let mut hosted =
        load_authorized_hosted_match(&state, &id, owner_subject_from_headers(&headers)).await?;
    let hand = match body.vira_rank {
        Some(rank) => random_hand_start_with_vira(&mut hosted.deal_rng, rank),
        None => random_hand_start(&mut hosted.deal_rng),
    };
    let response = start_hand_response(
        &mut hosted,
        hand,
        StartHandOptions {
            defer_bot_eleven: body.defer_bot_eleven,
        },
    )
    .await?;
    save_hosted_match(&state, &id, hosted).await;
    Ok(Json(response))
}

async fn advance_bot_turns_stateful(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<StateMutationResponse>, ServiceError> {
    let mut hosted =
        load_authorized_hosted_match(&state, &id, owner_subject_from_headers(&headers)).await?;
    auto_advance_bot_turns(&mut hosted).await?;
    let public_view = public_view_from_state(hosted.state.clone())?;
    let response = StateMutationResponse {
        state: hosted.state.clone(),
        public_view,
    };
    save_hosted_match(&state, &id, hosted).await;
    Ok(Json(response))
}

async fn get_public_view(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<MatchSnapshot>, ServiceError> {
    let stored =
        load_authorized_hosted_match(&state, &id, owner_subject_from_headers(&headers)).await?;
    Ok(Json(public_view_from_state(stored.state)?))
}

async fn get_player_view(
    Path((id, player)): Path<(String, Player)>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<PlayerMatchView>, ServiceError> {
    let stored =
        load_authorized_hosted_match(&state, &id, owner_subject_from_headers(&headers)).await?;
    if !state.dev_routes_enabled && player != stored.metadata().human_player {
        return Err(ServiceError::MatchForbidden);
    }
    Ok(Json(player_view_from_state(stored.state, player)?))
}

async fn get_legal_actions(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Action>>, ServiceError> {
    let stored =
        load_authorized_hosted_match(&state, &id, owner_subject_from_headers(&headers)).await?;
    Ok(Json(legal_actions_from_state(stored.state)?))
}

async fn apply_action_stateful(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(action): Json<Action>,
) -> Result<Json<StatelessActionResponse>, ServiceError> {
    let mut hosted =
        load_authorized_hosted_match(&state, &id, owner_subject_from_headers(&headers)).await?;
    let response = apply_action_response(&mut hosted, action).await?;
    save_hosted_match(&state, &id, hosted).await;
    Ok(Json(response))
}

async fn set_bot_override(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BotOverrideBody>,
) -> Result<Json<BotOverrideResponse>, ServiceError> {
    require_dev_routes_enabled(&state)?;
    let mut hosted =
        load_authorized_hosted_match(&state, &id, owner_subject_from_headers(&headers)).await?;
    let pending_override = match &mut hosted.mode {
        HostedMatchMode::HumanVsBot(session) => {
            session.next_action_override = body.action;
            session.next_action_override.clone()
        }
        HostedMatchMode::HumanOnly => None,
    };
    save_hosted_match(&state, &id, hosted).await;
    Ok(Json(BotOverrideResponse {
        ok: true,
        pending_override,
    }))
}

async fn public_view_stateless(
    Json(match_state): Json<MatchState>,
) -> Result<Json<MatchSnapshot>, ServiceError> {
    Ok(Json(public_view_from_state(match_state)?))
}

async fn resolve_spec_stateless(
    Json(spec): Json<ExplorationMatchSpec>,
) -> Result<Json<MatchState>, ServiceError> {
    let state = resolve_match_spec(&spec)?;
    Ok(Json(state))
}

async fn player_view_stateless(
    Path(player): Path<Player>,
    Json(match_state): Json<MatchState>,
) -> Result<Json<PlayerMatchView>, ServiceError> {
    Ok(Json(player_view_from_state(match_state, player)?))
}

async fn legal_actions_stateless(
    Json(match_state): Json<MatchState>,
) -> Result<Json<Vec<Action>>, ServiceError> {
    Ok(Json(legal_actions_from_state(match_state)?))
}

async fn start_hand_stateless(
    Json(request): Json<StatelessStartHandRequest>,
) -> Result<Json<StateMutationResponse>, ServiceError> {
    Ok(Json(start_hand_response_stateless(
        request.state,
        request.hand,
    )?))
}

async fn apply_action_stateless(
    Json(request): Json<StatelessActionRequest>,
) -> Result<Json<StatelessActionResponse>, ServiceError> {
    Ok(Json(apply_action_response_stateless(
        request.state,
        request.action,
    )?))
}

async fn create_match_response(
    state: &AppState,
    config: MatchConfig,
) -> Result<CreateMatchResponse, ServiceError> {
    let api_match = ApiMatch::new(config)?;
    let match_id = state.allocate_match_id();
    let public_view = api_match.public_view();
    let match_state = api_match.export_state();
    save_new_hosted_match(
        state,
        &match_id,
        HostedMatch::human_only(match_state.clone(), None, None),
    )
    .await?;

    Ok(CreateMatchResponse {
        match_id,
        state: match_state,
        public_view,
    })
}

async fn create_bot_match_response(
    state: &AppState,
    config: CreateBotMatchRequest,
) -> Result<CreateBotMatchResponse, ServiceError> {
    let match_id = state.allocate_match_id();
    let api_match = ApiMatch::new(MatchConfig {
        starting_dealer: config.starting_dealer,
        score: config.score,
    })?;
    let public_view = api_match.public_view();
    let match_state = api_match.export_state();
    let bot_player = other_player(config.human_player);
    let hosted = HostedMatch::human_vs_bot(
        match_state.clone(),
        config.human_player,
        bot_player,
        config.bot_kind,
        config.bot_profile,
        config.bot_model.clone(),
        config.seed,
        config.owner_subject.clone(),
        config.api_key.clone(),
    )?;
    save_new_hosted_match(state, &match_id, hosted).await?;

    Ok(CreateBotMatchResponse {
        match_id,
        human_player: config.human_player,
        bot_player,
        bot_kind: config.bot_kind,
        bot_profile: config.bot_profile,
        bot_model: config.bot_model,
        state: match_state,
        public_view,
    })
}

async fn create_match_from_state_response(
    state: &AppState,
    match_state: MatchState,
) -> Result<CreateMatchResponse, ServiceError> {
    let api_match = ApiMatch::from_state(match_state)?;
    let match_id = state.allocate_match_id();
    let public_view = api_match.public_view();
    let match_state = api_match.export_state();
    save_new_hosted_match(
        state,
        &match_id,
        HostedMatch::human_only(match_state.clone(), None, None),
    )
    .await?;

    Ok(CreateMatchResponse {
        match_id,
        state: match_state,
        public_view,
    })
}

fn public_view_from_state(match_state: MatchState) -> Result<MatchSnapshot, ServiceError> {
    let api_match = ApiMatch::from_state(match_state)?;
    Ok(api_match.public_view())
}

fn player_view_from_state(
    match_state: MatchState,
    player: Player,
) -> Result<PlayerMatchView, ServiceError> {
    let api_match = ApiMatch::from_state(match_state)?;
    Ok(api_match.player_view(player)?)
}

fn legal_actions_from_state(match_state: MatchState) -> Result<Vec<Action>, ServiceError> {
    let api_match = ApiMatch::from_state(match_state)?;
    Ok(api_match.legal_actions_for_current_player()?)
}

async fn start_hand_response(
    hosted: &mut HostedMatch,
    hand: HandStart,
    options: StartHandOptions,
) -> Result<StateMutationResponse, ServiceError> {
    let mut api_match = ApiMatch::from_state(hosted.state.clone())?;
    api_match.start_hand(hand)?;
    hosted.state = api_match.export_state();
    hosted.hand_action_log.clear();
    if !options.defer_bot_eleven || !has_pending_bot_eleven_decision(hosted) {
        auto_advance_bot_turns(hosted).await?;
    }
    let public_view = public_view_from_state(hosted.state.clone())?;
    Ok(StateMutationResponse {
        state: hosted.state.clone(),
        public_view,
    })
}

fn has_pending_bot_eleven_decision(hosted: &HostedMatch) -> bool {
    let bot_player = match &hosted.mode {
        HostedMatchMode::HumanOnly => return false,
        HostedMatchMode::HumanVsBot(session) => session.bot_player,
    };

    hosted
        .state
        .current_hand
        .as_ref()
        .and_then(|hand| hand.state.pending_decision.as_ref())
        .is_some_and(|decision| {
            decision.kind == PendingDecisionKind::MaoDeOnze && decision.player == bot_player
        })
}

async fn apply_action_response(
    hosted: &mut HostedMatch,
    action: Action,
) -> Result<StatelessActionResponse, ServiceError> {
    let acting_player = hosted
        .state
        .current_hand
        .as_ref()
        .and_then(|hand| hand.state.next_player);
    let mut api_match = ApiMatch::from_state(hosted.state.clone())?;
    let mut result = api_match.apply_action_for_current_player(&action)?;
    if let Some(player) = acting_player {
        // hosted.state is still the pre-action state here, so the played
        // card can be resolved for the solver bridge's observed-action log.
        log_observed_action(&mut hosted.hand_action_log, &hosted.state, player, &action);
    }
    hosted.state = api_match.export_state();
    auto_advance_bot_turns(hosted).await?;
    result.snapshot = public_view_from_state(hosted.state.clone())?;
    Ok(StatelessActionResponse {
        state: hosted.state.clone(),
        result,
    })
}

fn start_hand_response_stateless(
    match_state: MatchState,
    hand: HandStart,
) -> Result<StateMutationResponse, ServiceError> {
    let mut api_match = ApiMatch::from_state(match_state)?;
    let public_view = api_match.start_hand(hand)?;
    let state = api_match.export_state();
    Ok(StateMutationResponse { state, public_view })
}

fn apply_action_response_stateless(
    match_state: MatchState,
    action: Action,
) -> Result<StatelessActionResponse, ServiceError> {
    let mut api_match = ApiMatch::from_state(match_state)?;
    let result = api_match.apply_action_for_current_player(&action)?;
    let state = api_match.export_state();
    Ok(StatelessActionResponse { state, result })
}

fn replace_match_state_response(
    hosted: &HostedMatch,
) -> Result<StateMutationResponse, ServiceError> {
    let api_match = ApiMatch::from_state(hosted.state.clone())?;
    let public_view = api_match.public_view();
    let state = api_match.export_state();
    Ok(StateMutationResponse { state, public_view })
}

#[cfg(test)]
mod error_response_tests {
    use super::{ErrorResponse, ServiceError};
    use axum::{body::to_bytes, http::StatusCode, response::IntoResponse};
    use truco_bots::LlmProviderError;

    async fn response_code_and_status(error: ServiceError) -> (StatusCode, String) {
        let response = error.into_response();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let parsed: ErrorResponse = serde_json::from_slice(&bytes).expect("body should be json");
        (status, parsed.code)
    }

    #[tokio::test]
    async fn exhausted_provider_signals_bring_your_own_key() {
        let (status, code) = response_code_and_status(ServiceError::BotProvider(
            LlmProviderError::Exhausted("insufficient credits".to_string()),
        ))
        .await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(code, "LLM_PROVIDER_EXHAUSTED");
    }

    #[tokio::test]
    async fn generic_transport_failure_stays_a_bad_gateway() {
        let (status, code) = response_code_and_status(ServiceError::BotProvider(
            LlmProviderError::Transport("network reset".to_string()),
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(code, "BOT_PROVIDER_ERROR");
    }
}
