use std::{collections::HashMap, time::Instant};

use axum::http::HeaderMap;

use crate::{AppState, HostedMatch, ServiceError, SessionPolicy};

pub(crate) const MATCH_OWNER_SUBJECT_HEADER: &str = "x-truco-anonymous-subject";

pub(crate) fn owner_subject_from_headers(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(MATCH_OWNER_SUBJECT_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
}

pub(crate) async fn load_authorized_hosted_match(
    state: &AppState,
    id: &str,
    owner_subject: Option<&str>,
) -> Result<HostedMatch, ServiceError> {
    let now = Instant::now();
    let mut guard = state.matches.write().await;
    purge_expired_matches_locked(&mut guard, &state.session_policy, now);

    let hosted = guard.get_mut(id).ok_or(ServiceError::MatchNotFound)?;
    authorize_hosted_match(hosted, owner_subject)?;

    if hosted.is_expired(now, &state.session_policy) {
        hosted.expire(now);
    }

    if hosted.is_marked_expired() {
        return Err(ServiceError::MatchExpired);
    }

    hosted.touch(now);
    Ok(hosted.clone())
}

fn authorize_hosted_match(
    hosted: &HostedMatch,
    owner_subject: Option<&str>,
) -> Result<(), ServiceError> {
    if let Some(expected_owner) = hosted.owner_subject.as_deref() {
        if owner_subject != Some(expected_owner) {
            return Err(ServiceError::MatchForbidden);
        }
    }

    Ok(())
}

pub(crate) async fn save_hosted_match(state: &AppState, id: &str, hosted_match: HostedMatch) {
    let mut hosted_match = hosted_match;
    hosted_match.touch(Instant::now());
    let mut guard = state.matches.write().await;
    guard.insert(id.to_string(), hosted_match);
}

pub(crate) async fn save_new_hosted_match(
    state: &AppState,
    id: &str,
    hosted_match: HostedMatch,
) -> Result<(), ServiceError> {
    let now = Instant::now();
    let owner_subject = hosted_match.owner_subject.clone();

    if let Some(owner_subject) = owner_subject.as_deref() {
        let mut recent_match_creations = state.recent_match_creations.write().await;
        purge_recent_match_creations_locked(
            &mut recent_match_creations,
            owner_subject,
            &state.session_policy,
            now,
        );

        if creation_rate_limit_reached(
            &recent_match_creations,
            owner_subject,
            &state.session_policy,
        ) {
            return Err(ServiceError::SessionCreateRateLimited);
        }

        let mut matches = state.matches.write().await;
        purge_expired_matches_locked(&mut matches, &state.session_policy, now);

        if active_match_limit_reached(&matches, owner_subject, &state.session_policy) {
            return Err(ServiceError::SessionLimitReached);
        }

        let mut hosted_match = hosted_match;
        hosted_match.touch(now);
        matches.insert(id.to_string(), hosted_match);

        recent_match_creations
            .entry(owner_subject.to_string())
            .or_default()
            .push(now);
        return Ok(());
    }

    let mut hosted_match = hosted_match;
    hosted_match.touch(now);
    let mut matches = state.matches.write().await;
    purge_expired_matches_locked(&mut matches, &state.session_policy, now);
    matches.insert(id.to_string(), hosted_match);
    Ok(())
}

pub(crate) fn purge_expired_matches_locked(
    matches: &mut HashMap<String, HostedMatch>,
    session_policy: &SessionPolicy,
    now: Instant,
) {
    let mut remove_ids = Vec::new();

    for (match_id, hosted_match) in matches.iter_mut() {
        if hosted_match.is_expired(now, session_policy) {
            hosted_match.expire(now);
        }

        if hosted_match.should_drop_expired_tombstone(now, session_policy) {
            remove_ids.push(match_id.clone());
        }
    }

    for match_id in remove_ids {
        matches.remove(&match_id);
    }
}

pub(crate) fn purge_recent_match_creations_locked(
    recent_match_creations: &mut HashMap<String, Vec<Instant>>,
    owner_subject: &str,
    session_policy: &SessionPolicy,
    now: Instant,
) {
    let Some(window) = session_policy.create_rate_limit_window else {
        return;
    };

    let Some(timestamps) = recent_match_creations.get_mut(owner_subject) else {
        return;
    };

    timestamps.retain(|timestamp| now.saturating_duration_since(*timestamp) < window);

    if timestamps.is_empty() {
        recent_match_creations.remove(owner_subject);
    }
}

pub(crate) fn creation_rate_limit_reached(
    recent_match_creations: &HashMap<String, Vec<Instant>>,
    owner_subject: &str,
    session_policy: &SessionPolicy,
) -> bool {
    let Some(max) = session_policy.create_rate_limit_max else {
        return false;
    };
    let Some(window) = session_policy.create_rate_limit_window else {
        return false;
    };

    if window.is_zero() {
        return false;
    }

    recent_match_creations
        .get(owner_subject)
        .map(|timestamps| timestamps.len() >= max)
        .unwrap_or(false)
}

pub(crate) fn active_match_limit_reached(
    matches: &HashMap<String, HostedMatch>,
    owner_subject: &str,
    session_policy: &SessionPolicy,
) -> bool {
    let Some(max) = session_policy.max_active_per_subject else {
        return false;
    };

    matches
        .values()
        .filter(|hosted_match| {
            hosted_match.owner_subject.as_deref() == Some(owner_subject)
                && !hosted_match.is_marked_expired()
        })
        .count()
        >= max
}
