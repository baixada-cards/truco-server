use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use rand::rngs::StdRng;
use tokio::sync::RwLock;
use truco_engine::MatchState;

use crate::bot_host::{new_hosted_bot, BotSession, HostedBotKind, HostedMatchMode};
use crate::deal::seeded_rng;
use crate::{MatchMetadataResponse, ServiceError};
use truco_bot_core::BotProfile;
use truco_engine::{EngineError, Player};

#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) matches: Arc<RwLock<HashMap<String, HostedMatch>>>,
    pub(crate) recent_match_creations: Arc<RwLock<HashMap<String, Vec<Instant>>>>,
    pub(crate) session_policy: SessionPolicy,
    pub(crate) dev_routes_enabled: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new(SessionPolicy::from_env())
    }
}

impl AppState {
    pub(crate) fn new(session_policy: SessionPolicy) -> Self {
        Self {
            matches: Arc::new(RwLock::new(HashMap::new())),
            recent_match_creations: Arc::new(RwLock::new(HashMap::new())),
            session_policy,
            dev_routes_enabled: dev_routes_enabled_from_env(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_session_policy(session_policy: SessionPolicy) -> Self {
        Self::new(session_policy)
    }

    #[cfg(test)]
    pub(crate) fn with_dev_routes_enabled(dev_routes_enabled: bool) -> Self {
        Self {
            matches: Arc::new(RwLock::new(HashMap::new())),
            recent_match_creations: Arc::new(RwLock::new(HashMap::new())),
            session_policy: SessionPolicy::from_env(),
            dev_routes_enabled,
        }
    }

    pub(crate) fn allocate_match_id(&self) -> String {
        use rand::{distributions::Alphanumeric, Rng};
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(24)
            .map(char::from)
            .collect()
    }
}

fn dev_routes_enabled_from_env() -> bool {
    let production_defaults = std::env::var("NODE_ENV")
        .map(|value| value.eq_ignore_ascii_case("production"))
        .unwrap_or(false);
    if production_defaults {
        return false;
    }

    std::env::var("TRUCO_ENABLE_DEV_ROUTES")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionPolicy {
    pub(crate) idle_ttl: Option<Duration>,
    pub(crate) max_active_per_subject: Option<usize>,
    pub(crate) create_rate_limit_window: Option<Duration>,
    pub(crate) create_rate_limit_max: Option<usize>,
    pub(crate) expired_tombstone_ttl: Duration,
}

impl SessionPolicy {
    const DEV_IDLE_TTL_SECONDS: u64 = 0;
    const PROD_IDLE_TTL_SECONDS: u64 = 30 * 60;
    const DEV_MAX_ACTIVE_PER_SUBJECT: usize = 0;
    const PROD_MAX_ACTIVE_PER_SUBJECT: usize = 5;
    const DEV_CREATE_RATE_LIMIT_WINDOW_SECONDS: u64 = 60;
    const PROD_CREATE_RATE_LIMIT_WINDOW_SECONDS: u64 = 5 * 60;
    const DEV_CREATE_RATE_LIMIT_MAX: usize = 0;
    const PROD_CREATE_RATE_LIMIT_MAX: usize = 10;
    const EXPIRED_TOMBSTONE_TTL_SECONDS: u64 = 5 * 60;

    pub(crate) fn from_env() -> Self {
        let production_defaults = std::env::var("NODE_ENV")
            .map(|value| value.eq_ignore_ascii_case("production"))
            .unwrap_or(false);

        let default_idle_ttl_seconds = if production_defaults {
            Self::PROD_IDLE_TTL_SECONDS
        } else {
            Self::DEV_IDLE_TTL_SECONDS
        };
        let default_max_active_per_subject = if production_defaults {
            Self::PROD_MAX_ACTIVE_PER_SUBJECT
        } else {
            Self::DEV_MAX_ACTIVE_PER_SUBJECT
        };
        let default_rate_limit_window_seconds = if production_defaults {
            Self::PROD_CREATE_RATE_LIMIT_WINDOW_SECONDS
        } else {
            Self::DEV_CREATE_RATE_LIMIT_WINDOW_SECONDS
        };
        let default_rate_limit_max = if production_defaults {
            Self::PROD_CREATE_RATE_LIMIT_MAX
        } else {
            Self::DEV_CREATE_RATE_LIMIT_MAX
        };

        Self {
            idle_ttl: duration_from_seconds_env(
                "TRUCO_SESSION_IDLE_TTL_SECONDS",
                default_idle_ttl_seconds,
            ),
            max_active_per_subject: usize_from_env(
                "TRUCO_SESSION_MAX_ACTIVE_PER_SUBJECT",
                default_max_active_per_subject,
            ),
            create_rate_limit_window: duration_from_seconds_env(
                "TRUCO_SESSION_CREATE_RATE_LIMIT_WINDOW_SECONDS",
                default_rate_limit_window_seconds,
            ),
            create_rate_limit_max: usize_from_env(
                "TRUCO_SESSION_CREATE_RATE_LIMIT_MAX",
                default_rate_limit_max,
            ),
            expired_tombstone_ttl: Duration::from_secs(Self::EXPIRED_TOMBSTONE_TTL_SECONDS),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests(
        idle_ttl: Option<Duration>,
        max_active_per_subject: Option<usize>,
        create_rate_limit_window: Option<Duration>,
        create_rate_limit_max: Option<usize>,
    ) -> Self {
        Self {
            idle_ttl,
            max_active_per_subject,
            create_rate_limit_window,
            create_rate_limit_max,
            expired_tombstone_ttl: Duration::from_secs(Self::EXPIRED_TOMBSTONE_TTL_SECONDS),
        }
    }
}

fn duration_from_seconds_env(name: &str, default: u64) -> Option<Duration> {
    nonzero_u64_from_env(name, default).map(Duration::from_secs)
}

fn usize_from_env(name: &str, default: usize) -> Option<usize> {
    let raw = match std::env::var(name) {
        Ok(value) => value,
        Err(_) => return Some(default).filter(|value| *value > 0),
    };

    let parsed = raw
        .parse::<usize>()
        .unwrap_or_else(|_| panic!("invalid {name}: expected a non-negative integer, got {raw:?}"));
    Some(parsed).filter(|value| *value > 0)
}

fn nonzero_u64_from_env(name: &str, default: u64) -> Option<u64> {
    let raw = match std::env::var(name) {
        Ok(value) => value,
        Err(_) => return Some(default).filter(|value| *value > 0),
    };

    let parsed = raw
        .parse::<u64>()
        .unwrap_or_else(|_| panic!("invalid {name}: expected a non-negative integer, got {raw:?}"));
    Some(parsed).filter(|value| *value > 0)
}

#[derive(Debug, Clone)]
pub(crate) struct HostedMatch {
    pub(crate) state: MatchState,
    pub(crate) deal_rng: StdRng,
    pub(crate) mode: HostedMatchMode,
    pub(crate) owner_subject: Option<String>,
    pub(crate) last_activity_at: Instant,
    pub(crate) lifecycle: HostedMatchLifecycle,
    /// The current hand's applied actions in order. The engine's exported
    /// state drops where resolved raises sat in the sequence, so this log is
    /// the solver bot's only faithful source for info-set reconstruction.
    /// Cleared on start-hand and whenever the state is replaced wholesale.
    pub(crate) hand_action_log: Vec<truco_policy_bot::ObservedAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostedMatchLifecycle {
    Active,
    Expired { expired_at: Instant },
}

impl HostedMatch {
    pub(crate) fn human_only(
        state: MatchState,
        seed: Option<u64>,
        owner_subject: Option<String>,
    ) -> Self {
        Self {
            state,
            deal_rng: seeded_rng(seed, 0xD3A1_C001),
            mode: HostedMatchMode::HumanOnly,
            owner_subject,
            last_activity_at: Instant::now(),
            lifecycle: HostedMatchLifecycle::Active,
            hand_action_log: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn human_vs_bot(
        state: MatchState,
        human_player: Player,
        bot_player: Player,
        bot_kind: HostedBotKind,
        bot_profile: BotProfile,
        bot_model: Option<String>,
        seed: Option<u64>,
        owner_subject: Option<String>,
        // Bring-your-own-key: used only to construct the bot below. It is never
        // stored on the session, echoed in metadata, or logged.
        bot_api_key: Option<String>,
    ) -> Result<Self, ServiceError> {
        if !matches!(human_player, 0 | 1)
            || !matches!(bot_player, 0 | 1)
            || human_player == bot_player
        {
            return Err(ServiceError::Engine(EngineError::InvalidInitialState));
        }

        let bot = new_hosted_bot(bot_kind, bot_profile, bot_model.clone(), seed, bot_api_key)?;

        Ok(Self {
            state,
            deal_rng: seeded_rng(seed, 0xD3A1_C001),
            mode: HostedMatchMode::HumanVsBot(Box::new(BotSession {
                bot_player,
                bot,
                profile: bot_profile,
                model: bot_model,
                last_decision: None,
                next_action_override: None,
            })),
            owner_subject,
            last_activity_at: Instant::now(),
            lifecycle: HostedMatchLifecycle::Active,
            hand_action_log: Vec::new(),
        })
    }

    pub(crate) fn touch(&mut self, now: Instant) {
        self.last_activity_at = now;
        self.lifecycle = HostedMatchLifecycle::Active;
    }

    pub(crate) fn expire(&mut self, now: Instant) {
        self.lifecycle = HostedMatchLifecycle::Expired { expired_at: now };
    }

    pub(crate) fn is_expired(&self, now: Instant, session_policy: &SessionPolicy) -> bool {
        if self.is_marked_expired() {
            return true;
        }

        let Some(idle_ttl) = session_policy.idle_ttl else {
            return false;
        };

        now.saturating_duration_since(self.last_activity_at) >= idle_ttl
    }

    pub(crate) fn is_marked_expired(&self) -> bool {
        matches!(self.lifecycle, HostedMatchLifecycle::Expired { .. })
    }

    pub(crate) fn should_drop_expired_tombstone(
        &self,
        now: Instant,
        session_policy: &SessionPolicy,
    ) -> bool {
        match self.lifecycle {
            HostedMatchLifecycle::Active => false,
            HostedMatchLifecycle::Expired { expired_at } => {
                now.saturating_duration_since(expired_at) >= session_policy.expired_tombstone_ttl
            }
        }
    }

    pub(crate) fn metadata(&self) -> MatchMetadataResponse {
        match &self.mode {
            HostedMatchMode::HumanOnly => MatchMetadataResponse {
                human_player: 0,
                bot_player: None,
                bot_kind: None,
                bot_profile: None,
                bot_model: None,
                last_bot_decision: None,
                pending_bot_override: None,
            },
            HostedMatchMode::HumanVsBot(session) => MatchMetadataResponse {
                human_player: crate::deal::other_player(session.bot_player),
                bot_player: Some(session.bot_player),
                bot_kind: Some(session.bot.kind()),
                bot_profile: Some(session.profile),
                bot_model: session.model.clone(),
                last_bot_decision: session.last_decision.clone(),
                pending_bot_override: session.next_action_override.clone(),
            },
        }
    }
}
