use std::sync::{Arc, OnceLock};

use rand::rngs::StdRng;
use serde::{Deserialize, Serialize};
use truco_bot_core::{
    choose_decision_for_match, turn_for_match, BotDecision, BotPlan, BotProfile, HeuristicBot,
    SimpleTrainerBot, UniformRandomBot,
};
use truco_bots::{LlmProviderBot, LlmProviderKind};
use truco_engine::{Action, Card, Match, MatchState, Player};
use truco_policy_bot::{ObservedAction, PolicyStore, SolverPolicyBot};

use crate::deal::seeded_rng;
use crate::{HostedMatch, ServiceError};

/// Directory holding the solver bot's policy artifacts (`manifest.json` plus
/// one `.tpb` per solved profile). Absent or unloadable => the solver opponent
/// is unavailable and match creation with it fails cleanly.
pub const SOLVER_POLICY_DIR_ENV: &str = "SOLVER_POLICY_DIR";

/// The policy store is process-wide: mmap-ed profiles are immutable and shared
/// by every hosted match. Loaded once, on first use.
pub(crate) fn solver_policy_store() -> Option<Arc<PolicyStore>> {
    static STORE: OnceLock<Option<Arc<PolicyStore>>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let dir = std::env::var(SOLVER_POLICY_DIR_ENV).ok()?;
            match PolicyStore::load(std::path::Path::new(&dir)) {
                Ok(store) => {
                    eprintln!(
                        "solver bot: loaded {} policy profiles from {dir}",
                        store.profile_count()
                    );
                    Some(Arc::new(store))
                }
                Err(error) => {
                    eprintln!("solver bot: failed to load policy dir {dir}: {error}");
                    None
                }
            }
        })
        .clone()
}

/// Resolve the concrete card behind a play action before it leaves the hand,
/// so the observed-action log stays reconstructable for the solver bridge.
fn resolve_played_card(state: &MatchState, player: Player, action: &Action) -> Option<Card> {
    let card_id = match action {
        Action::PlayFaceUp { card_id } | Action::PlayFaceDown { card_id } => card_id,
        _ => return None,
    };
    state
        .current_hand
        .as_ref()?
        .state
        .hands
        .player(player)
        .iter()
        .find(|card| card.id == *card_id)
        .cloned()
}

/// Append an applied action to the hand's observed-action log.
pub(crate) fn log_observed_action(
    log: &mut Vec<ObservedAction>,
    state_before: &MatchState,
    player: Player,
    action: &Action,
) {
    let card = resolve_played_card(state_before, player, action);
    log.push(ObservedAction {
        player,
        action: action.clone(),
        card,
    });
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BotActionOverride {
    Action(Action),
    Directive(BotOverrideDirective),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BotOverrideDirective {
    #[serde(rename = "next_legal_raise")]
    NextLegalRaise,
}

impl BotActionOverride {
    fn resolve(&self, legal_actions: &[Action]) -> Option<Action> {
        match self {
            Self::Action(action) => legal_actions.contains(action).then(|| action.clone()),
            Self::Directive(BotOverrideDirective::NextLegalRaise) => {
                legal_actions.iter().find_map(|action| match action {
                    Action::Raise { .. } => Some(action.clone()),
                    _ => None,
                })
            }
        }
    }

    fn should_persist_when_unavailable(&self) -> bool {
        matches!(self, Self::Directive(BotOverrideDirective::NextLegalRaise))
    }

    fn reasoning(&self) -> &'static str {
        match self {
            Self::Action(_) => "dev override",
            Self::Directive(BotOverrideDirective::NextLegalRaise) => {
                "dev override: next legal raise"
            }
        }
    }
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum HostedMatchMode {
    HumanOnly,
    HumanVsBot(Box<BotSession>),
}

#[derive(Debug, Clone)]
pub(crate) struct BotSession {
    pub(crate) bot_player: Player,
    pub(crate) bot: HostedBot,
    pub(crate) profile: BotProfile,
    pub(crate) model: Option<String>,
    pub(crate) last_decision: Option<BotDecision>,
    pub(crate) next_action_override: Option<BotActionOverride>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HostedBotKind {
    Random,
    Simple,
    #[default]
    Heuristic,
    Solver,
    #[serde(rename = "openai")]
    OpenAi,
    Anthropic,
    #[serde(rename = "openrouter")]
    OpenRouter,
}

#[derive(Debug, Clone)]
pub(crate) enum HostedBot {
    Random(UniformRandomBot<StdRng>),
    Simple(SimpleTrainerBot<StdRng>),
    Heuristic(HeuristicBot<StdRng>),
    Solver(Box<SolverPolicyBot>),
    OpenAi(LlmProviderBot),
    Anthropic(LlmProviderBot),
    OpenRouter(LlmProviderBot),
}

impl HostedBot {
    pub(crate) fn kind(&self) -> HostedBotKind {
        match self {
            Self::Random(_) => HostedBotKind::Random,
            Self::Simple(_) => HostedBotKind::Simple,
            Self::Heuristic(_) => HostedBotKind::Heuristic,
            Self::Solver(_) => HostedBotKind::Solver,
            Self::OpenAi(_) => HostedBotKind::OpenAi,
            Self::Anthropic(_) => HostedBotKind::Anthropic,
            Self::OpenRouter(_) => HostedBotKind::OpenRouter,
        }
    }

    pub(crate) async fn choose_decision(
        &mut self,
        game: &Match,
        player: Player,
        hand_log: &[ObservedAction],
    ) -> Result<BotDecision, ServiceError> {
        match self {
            Self::Random(bot) => Ok(choose_decision_for_match(bot, game, player)?),
            Self::Simple(bot) => Ok(choose_decision_for_match(bot, game, player)?),
            Self::Heuristic(bot) => Ok(choose_decision_for_match(bot, game, player)?),
            Self::Solver(bot) => {
                let turn = turn_for_match(game, player)?;
                Ok(bot.choose_decision(&turn, hand_log)?)
            }
            Self::OpenAi(bot) => Ok(bot.choose_decision(game, player).await?),
            Self::Anthropic(bot) => Ok(bot.choose_decision(game, player).await?),
            Self::OpenRouter(bot) => Ok(bot.choose_decision(game, player).await?),
        }
    }
}

/// Build an LLM-backed bot, using a caller-supplied key (bring your own key)
/// when present and falling back to the environment's shared key otherwise.
fn new_llm_bot(
    provider: LlmProviderKind,
    bot_model: Option<String>,
    seed: Option<u64>,
    bot_api_key: Option<String>,
) -> Result<LlmProviderBot, ServiceError> {
    let bot = match bot_api_key.filter(|key| !key.trim().is_empty()) {
        Some(key) => LlmProviderBot::with_api_key(provider, bot_model, seed, key)?,
        None => LlmProviderBot::from_env(provider, bot_model, seed)?,
    };
    Ok(bot)
}

pub(crate) fn new_hosted_bot(
    bot_kind: HostedBotKind,
    bot_profile: BotProfile,
    bot_model: Option<String>,
    seed: Option<u64>,
    bot_api_key: Option<String>,
) -> Result<HostedBot, ServiceError> {
    match bot_kind {
        HostedBotKind::Random => Ok(HostedBot::Random(UniformRandomBot::new(seeded_rng(
            seed, 0xB07_0001,
        )))),
        HostedBotKind::Simple => Ok(HostedBot::Simple(SimpleTrainerBot::new(seeded_rng(
            seed, 0xB07_0002,
        )))),
        HostedBotKind::Heuristic => Ok(HostedBot::Heuristic(HeuristicBot::new(
            seeded_rng(seed, 0xB07_0003),
            bot_profile,
        ))),
        HostedBotKind::Solver => {
            let store = solver_policy_store().ok_or(ServiceError::SolverBotUnavailable)?;
            Ok(HostedBot::Solver(Box::new(SolverPolicyBot::new(
                store, seed,
            ))))
        }
        HostedBotKind::OpenAi => Ok(HostedBot::OpenAi(new_llm_bot(
            LlmProviderKind::OpenAi,
            bot_model,
            seed,
            bot_api_key,
        )?)),
        HostedBotKind::Anthropic => Ok(HostedBot::Anthropic(new_llm_bot(
            LlmProviderKind::Anthropic,
            bot_model,
            seed,
            bot_api_key,
        )?)),
        HostedBotKind::OpenRouter => Ok(HostedBot::OpenRouter(new_llm_bot(
            LlmProviderKind::OpenRouter,
            bot_model,
            seed,
            bot_api_key,
        )?)),
    }
}

pub(crate) async fn auto_advance_bot_turns(hosted: &mut HostedMatch) -> Result<(), ServiceError> {
    loop {
        let bot_player = match &hosted.mode {
            HostedMatchMode::HumanOnly => return Ok(()),
            HostedMatchMode::HumanVsBot(session) => session.bot_player,
        };

        let game = Match::from_state(hosted.state.clone())?;
        if game.winner().is_some() || !game.public_view().hand_in_progress {
            return Ok(());
        }

        let Some(current_player) = game.current_player() else {
            return Ok(());
        };
        if current_player != bot_player {
            return Ok(());
        }

        // Snapshot for the solver bridge: the hand's exact action sequence so
        // far (`hosted.mode` is borrowed mutably below).
        let hand_log = hosted.hand_action_log.clone();

        let decision = match &mut hosted.mode {
            HostedMatchMode::HumanOnly => return Ok(()),
            HostedMatchMode::HumanVsBot(session) => {
                if let Some(override_action) = session.next_action_override.clone() {
                    let forced_action = game
                        .legal_actions_for_current_player()
                        .ok()
                        .and_then(|actions| override_action.resolve(&actions));
                    if let Some(action) = forced_action {
                        session.next_action_override = None;
                        let decision = BotDecision {
                            action,
                            plan: BotPlan {
                                choices: vec![],
                                reasoning: Some(override_action.reasoning().to_string()),
                            },
                        };
                        session.last_decision = Some(decision.clone());
                        decision
                    } else {
                        if !override_action.should_persist_when_unavailable() {
                            session.next_action_override = None;
                        }
                        let decision = session
                            .bot
                            .choose_decision(&game, session.bot_player, &hand_log)
                            .await?;
                        session.last_decision = Some(decision.clone());
                        decision
                    }
                } else {
                    let decision = session
                        .bot
                        .choose_decision(&game, session.bot_player, &hand_log)
                        .await?;
                    session.last_decision = Some(decision.clone());
                    decision
                }
            }
        };
        let mut game = game;
        log_observed_action(
            &mut hosted.hand_action_log,
            &hosted.state,
            bot_player,
            &decision.action,
        );
        game.apply_action_for_current_player(&decision.action)?;
        hosted.state = game.export_state();
    }
}
