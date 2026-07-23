pub mod bot_host;
mod deal;
pub mod notation;
mod routes;
mod session;
mod state;

pub use bot_host::HostedBotKind;
#[cfg(test)]
pub(crate) use session::MATCH_OWNER_SUBJECT_HEADER;
pub use state::AppState;
pub(crate) use state::{HostedMatch, SessionPolicy};
pub use truco_bot_core::BotProfile;

pub use routes::app;
pub use routes::{
    CreateBotMatchResponse, CreateMatchResponse, MatchMetadataResponse, ServiceError,
    StatelessActionRequest, StatelessStartHandRequest,
};
pub use truco_engine::MatchConfig;

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use axum::{
        body::{to_bytes, Body},
        http::{Method, Request, StatusCode},
    };
    use serde_json::{json, Value};
    use tower::ServiceExt;
    use truco_engine::{Action, Card, ExplorationMatchSpec, Hands, Rank, Score, Suit, Turnup};

    use super::{
        app, AppState, CreateBotMatchResponse, CreateMatchResponse, MatchConfig, SessionPolicy,
        StatelessActionRequest, StatelessStartHandRequest,
    };

    fn sample_hand_start_value() -> Value {
        json!({
            "turnup": { "rank": "A", "suit": "SPADES" },
            "hands": {
                "0": [
                    { "id": "p0c0", "rank": "7", "suit": "DIAMONDS" },
                    { "id": "p0c1", "rank": "6", "suit": "CLUBS" },
                    { "id": "p0c2", "rank": "4", "suit": "HEARTS" }
                ],
                "1": [
                    { "id": "p1c0", "rank": "3", "suit": "CLUBS" },
                    { "id": "p1c1", "rank": "5", "suit": "SPADES" },
                    { "id": "p1c2", "rank": "4", "suit": "DIAMONDS" }
                ]
            }
        })
    }

    async fn json_request(
        app: axum::Router,
        method: Method,
        uri: &str,
        payload: Value,
    ) -> (StatusCode, Value) {
        let response = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let json = serde_json::from_slice(&bytes).expect("response should be json");
        (status, json)
    }

    async fn empty_request(app: axum::Router, method: Method, uri: &str) -> (StatusCode, Value) {
        let response = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let json = serde_json::from_slice(&bytes).expect("response should be json");
        (status, json)
    }

    async fn json_request_with_owner_subject(
        app: axum::Router,
        method: Method,
        uri: &str,
        owner_subject: &str,
        payload: Value,
    ) -> (StatusCode, Value) {
        let response = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .header(super::MATCH_OWNER_SUBJECT_HEADER, owner_subject)
                    .body(Body::from(payload.to_string()))
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let json = serde_json::from_slice(&bytes).expect("response should be json");
        (status, json)
    }

    async fn empty_request_with_owner_subject(
        app: axum::Router,
        method: Method,
        uri: &str,
        owner_subject: &str,
    ) -> (StatusCode, Value) {
        let response = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(super::MATCH_OWNER_SUBJECT_HEADER, owner_subject)
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let json = serde_json::from_slice(&bytes).expect("response should be json");
        (status, json)
    }

    #[tokio::test]
    async fn stateful_routes_create_store_and_mutate_match() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/matches",
            serde_json::to_value(MatchConfig {
                starting_dealer: 1,
                score: Score { zero: 0, one: 0 },
            })
            .expect("config should serialize"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let created: CreateMatchResponse =
            serde_json::from_value(created).expect("create response should parse");

        let (status, started) = json_request(
            app.clone(),
            Method::POST,
            &format!("/matches/{}/start-hand", created.match_id),
            sample_hand_start_value(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(started["public_view"]["current_player"], json!(0));

        let (status, actions) = empty_request(
            app.clone(),
            Method::GET,
            &format!("/matches/{}/legal-actions", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(actions
            .as_array()
            .expect("actions list")
            .iter()
            .any(|action| { action["type"] == json!("play_face_up") }));

        let (status, applied) = json_request(
            app.clone(),
            Method::POST,
            &format!("/matches/{}/actions", created.match_id),
            json!({ "type": "play_face_up", "card_id": "p0c0" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(applied["result"]["acted_by"], json!(0));
        assert_eq!(applied["result"]["snapshot"]["current_player"], json!(1));

        let (status, player_view) = empty_request(
            app,
            Method::GET,
            &format!("/matches/{}/players/1/view", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(player_view["player"], json!(1));
        assert_eq!(
            player_view["hand"]["hand"]
                .as_array()
                .expect("hand list")
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn stateless_routes_round_trip_explicit_match_state() {
        let app = app(AppState::default());
        let hand = sample_hand_start_value();

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateMatchResponse =
            serde_json::from_value(created).expect("create response should parse");

        let request = StatelessStartHandRequest {
            state: created.state,
            hand: serde_json::from_value(hand).expect("hand should parse"),
        };
        let (status, started) = json_request(
            app.clone(),
            Method::POST,
            "/engine/start-hand",
            serde_json::to_value(request).expect("request should serialize"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, public_view) = json_request(
            app.clone(),
            Method::POST,
            "/engine/public-view",
            started["state"].clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(public_view["current_player"], json!(0));

        let (status, legal_actions) = json_request(
            app.clone(),
            Method::POST,
            "/engine/legal-actions",
            started["state"].clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(legal_actions
            .as_array()
            .expect("actions list")
            .iter()
            .any(|action| { action["type"] == json!("raise") }));

        let action_request = StatelessActionRequest {
            state: serde_json::from_value(started["state"].clone()).expect("state should parse"),
            action: truco_engine::Action::PlayFaceUp {
                card_id: "p0c0".into(),
            },
        };
        let (status, applied) = json_request(
            app.clone(),
            Method::POST,
            "/engine/actions",
            serde_json::to_value(action_request).expect("action request should serialize"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(applied["result"]["snapshot"]["current_player"], json!(1));

        let (status, player_view) = json_request(
            app,
            Method::POST,
            "/engine/players/1/view",
            applied["state"].clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(player_view["player"], json!(1));
        assert_eq!(
            player_view["hand"]["public_state"]["current_round"]["plays"][0]["card"]["rank"],
            json!("7")
        );
    }

    #[tokio::test]
    async fn stateless_notation_route_compiles_fragment() {
        let app = app(AppState::default());

        let (status, response) = json_request(
            app,
            Method::POST,
            "/engine/compile-notation",
            json!({
                "notation": "seed=13\nopp = [>=A, <=3]",
                "human_player": 0,
                "bot_player": 1
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["fragment"]["seed"], json!(13));
        assert_eq!(
            response["fragment"]["hidden_ranges"]["1"]["weighted_hands"][0]["hand"]["type"],
            json!("ordered")
        );
    }

    #[tokio::test]
    async fn stateless_notation_route_returns_structured_errors() {
        let app = app(AppState::default());

        let (status, response) = json_request(
            app,
            Method::POST,
            "/engine/compile-notation",
            json!({
                "notation": "villan = [>=A, <=3]",
                "human_player": 0,
                "bot_player": 1
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response["code"], json!("NOTATION_PARSE_ERROR"));
        assert_eq!(response["line"], json!(1));
        assert_eq!(response["column"], json!(1));
    }

    #[tokio::test]
    async fn can_create_stateful_match_directly_from_explicit_state() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateMatchResponse =
            serde_json::from_value(created).expect("create response should parse");

        let (status, started) = json_request(
            app.clone(),
            Method::POST,
            "/engine/start-hand",
            json!({
                "state": created.state,
                "hand": sample_hand_start_value()
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, restored) = json_request(
            app.clone(),
            Method::POST,
            "/matches/from-state",
            started["state"].clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(restored["public_view"]["current_player"], json!(0));

        let match_id = restored["match_id"]
            .as_str()
            .expect("match id should be present");
        let (status, player_view) = empty_request(
            app,
            Method::GET,
            &format!("/matches/{match_id}/players/0/view"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            player_view["hand"]["hand"]
                .as_array()
                .expect("hand list")
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn can_replace_existing_stateful_match_state_for_exploration() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateMatchResponse =
            serde_json::from_value(created).expect("create response should parse");

        let (status, started) = json_request(
            app.clone(),
            Method::POST,
            "/engine/start-hand",
            json!({
                "state": created.state,
                "hand": sample_hand_start_value()
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, applied) = json_request(
            app.clone(),
            Method::POST,
            "/engine/actions",
            json!({
                "state": started["state"].clone(),
                "action": { "type": "play_face_up", "card_id": "p0c0" }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, replaced) = json_request(
            app.clone(),
            Method::PUT,
            &format!("/matches/{}", created.match_id),
            applied["state"].clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(replaced["public_view"]["current_player"], json!(1));

        let (status, legal_actions) = empty_request(
            app,
            Method::GET,
            &format!("/matches/{}/legal-actions", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(legal_actions
            .as_array()
            .expect("actions list")
            .iter()
            .any(|action| { action["type"] == json!("play_face_up") }));
    }

    #[tokio::test]
    async fn can_resolve_ordered_hidden_range_spec_statefully() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateMatchResponse =
            serde_json::from_value(created).expect("create response should parse");

        let (status, started) = json_request(
            app.clone(),
            Method::POST,
            "/engine/start-hand",
            json!({
                "state": created.state,
                "hand": sample_hand_start_value()
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let spec = json!({
            "base_state": started["state"].clone(),
            "seed": 9,
            "hidden_ranges": {
                "1": {
                    "weighted_hands": [
                        {
                            "weight": 1,
                            "hand": {
                                "type": "ordered",
                                "slots": [
                                    { "allowed_ranks": ["4", "5", "6", "7"] },
                                    { "strength_at_least": "A" },
                                    { "strength_at_most": "3" }
                                ]
                            }
                        }
                    ]
                }
            }
        });

        let (status, restored) = json_request(
            app.clone(),
            Method::POST,
            "/matches/from-spec",
            spec.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(restored["public_view"]["current_player"], json!(0));

        let match_id = restored["match_id"]
            .as_str()
            .expect("match id should be present");
        let (status, player_view) = empty_request(
            app,
            Method::GET,
            &format!("/matches/{match_id}/players/1/view"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let hand = player_view["hand"]["hand"]
            .as_array()
            .expect("hand list should be present");
        assert_eq!(hand.len(), 3);
    }

    #[tokio::test]
    async fn stateless_spec_resolution_returns_exact_match_state() {
        let app = app(AppState::default());

        let base_spec = json!({
            "starting_dealer": 1,
            "score": { "0": 0, "1": 0 }
        });
        let (status, created) =
            json_request(app.clone(), Method::POST, "/matches", base_spec).await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateMatchResponse =
            serde_json::from_value(created).expect("create response should parse");
        let (status, started) = json_request(
            app.clone(),
            Method::POST,
            "/engine/start-hand",
            json!({
                "state": created.state,
                "hand": sample_hand_start_value()
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let spec: ExplorationMatchSpec = serde_json::from_value(json!({
            "base_state": started["state"].clone(),
            "seed": 3,
            "hidden_ranges": {
                "1": {
                    "weighted_hands": [
                        {
                            "weight": 1,
                            "hand": {
                                "type": "exact",
                                "cards": [
                                    { "rank": "A", "suit": "HEARTS" },
                                    { "rank": "2", "suit": "CLUBS" },
                                    { "rank": "3", "suit": "SPADES" }
                                ]
                            }
                        }
                    ]
                }
            }
        }))
        .expect("spec should parse");

        let (status, resolved) = json_request(
            app,
            Method::POST,
            "/engine/resolve-spec",
            serde_json::to_value(spec).expect("spec should serialize"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            resolved["current_hand"]["state"]["hands"]["1"]
                .as_array()
                .expect("hand list")
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn stateless_predeal_spec_can_materialize_a_fresh_hand() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app,
            Method::POST,
            "/engine/resolve-spec",
            json!({
                "base_state": {
                    "next_dealer": 1,
                    "score": { "0": 0, "1": 0 },
                    "winner": null,
                    "current_hand": null
                },
                "pending_hand": {
                    "turnup": { "rank": "A", "suit": "SPADES" }
                },
                "hidden_ranges": {
                    "0": {
                        "weighted_hands": [
                            {
                                "weight": 1,
                                "hand": {
                                    "type": "exact",
                                    "cards": [
                                        { "rank": "4", "suit": "SPADES" },
                                        { "rank": "6", "suit": "HEARTS" },
                                        { "rank": "7", "suit": "CLUBS" }
                                    ]
                                }
                            }
                        ]
                    }
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(created["current_hand"]["state"]["dealer"], json!(1));
        assert_eq!(created["current_hand"]["state"]["next_player"], json!(0));
        assert_eq!(
            created["current_hand"]["state"]["hands"]["0"][0],
            json!({ "id": "p0c0", "rank": "4", "suit": "SPADES" })
        );
    }

    #[tokio::test]
    async fn missing_stateful_match_returns_not_found() {
        let app = app(AppState::default());
        let (status, body) =
            empty_request(app, Method::GET, "/matches/match-999/public-view").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], json!("MATCH_NOT_FOUND"));
    }

    #[tokio::test]
    async fn metadata_route_reports_bot_configuration() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 },
                "human_player": 0,
                "bot_kind": "heuristic",
                "bot_profile": "tricky",
                "seed": 5
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateBotMatchResponse =
            serde_json::from_value(created).expect("bot create response should parse");

        let (status, metadata) = empty_request(
            app,
            Method::GET,
            &format!("/matches/{}/meta", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(metadata["human_player"], json!(0));
        assert_eq!(metadata["bot_player"], json!(1));
        assert_eq!(metadata["bot_kind"], json!("heuristic"));
        assert_eq!(metadata["bot_profile"], json!("tricky"));
        assert_eq!(metadata["last_bot_decision"], json!(null));
    }

    #[tokio::test]
    async fn owner_bound_matches_require_the_same_browser_subject_for_reads_and_writes() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 },
                "human_player": 0,
                "owner_subject": "browser-a",
                "seed": 13
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateBotMatchResponse =
            serde_json::from_value(created).expect("bot create response should parse");

        let (status, _) = empty_request_with_owner_subject(
            app.clone(),
            Method::GET,
            &format!("/matches/{}/public-view", created.match_id),
            "browser-a",
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = empty_request(
            app.clone(),
            Method::GET,
            &format!("/matches/{}/public-view", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], json!("MATCH_FORBIDDEN"));

        let (status, body) = empty_request_with_owner_subject(
            app.clone(),
            Method::GET,
            &format!("/matches/{}/public-view", created.match_id),
            "browser-b",
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], json!("MATCH_FORBIDDEN"));

        let (status, body) = empty_request_with_owner_subject(
            app.clone(),
            Method::POST,
            &format!("/matches/{}/start-hand/random", created.match_id),
            "browser-b",
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], json!("MATCH_FORBIDDEN"));

        let (status, started) = empty_request_with_owner_subject(
            app.clone(),
            Method::POST,
            &format!("/matches/{}/start-hand/random", created.match_id),
            "browser-a",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(started["public_view"]["hand_in_progress"], json!(true));

        let (status, player_view) = empty_request_with_owner_subject(
            app.clone(),
            Method::GET,
            &format!("/matches/{}/players/0/view", created.match_id),
            "browser-a",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let card_id = player_view["hand"]["hand"][0]["id"]
            .as_str()
            .expect("first card id should exist")
            .to_string();

        let (status, body) = json_request_with_owner_subject(
            app,
            Method::POST,
            &format!("/matches/{}/actions", created.match_id),
            "browser-b",
            json!({ "type": "play_face_up", "card_id": card_id }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], json!("MATCH_FORBIDDEN"));
    }

    #[tokio::test]
    async fn expired_owner_bound_match_returns_match_expired() {
        let state = AppState::with_session_policy(SessionPolicy::for_tests(
            Some(Duration::from_secs(60)),
            None,
            None,
            None,
        ));
        let app = app(state.clone());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 },
                "human_player": 0,
                "owner_subject": "browser-a",
                "seed": 23
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateBotMatchResponse =
            serde_json::from_value(created).expect("bot create response should parse");

        {
            let mut matches = state.matches.write().await;
            let hosted_match = matches
                .get_mut(&created.match_id)
                .expect("created match should exist");
            hosted_match.last_activity_at = Instant::now() - Duration::from_secs(61);
        }

        let (status, body) = empty_request_with_owner_subject(
            app.clone(),
            Method::GET,
            &format!("/matches/{}/public-view", created.match_id),
            "browser-a",
        )
        .await;
        assert_eq!(status, StatusCode::GONE);
        assert_eq!(body["code"], json!("MATCH_EXPIRED"));

        let (status, body) = empty_request_with_owner_subject(
            app,
            Method::GET,
            &format!("/matches/{}/public-view", created.match_id),
            "browser-a",
        )
        .await;
        assert_eq!(status, StatusCode::GONE);
        assert_eq!(body["code"], json!("MATCH_EXPIRED"));
    }

    #[tokio::test]
    async fn active_match_limit_blocks_new_matches_until_an_old_one_expires() {
        let state = AppState::with_session_policy(SessionPolicy::for_tests(
            Some(Duration::from_secs(60)),
            Some(1),
            None,
            None,
        ));
        let app = app(state.clone());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 },
                "human_player": 0,
                "owner_subject": "browser-a",
                "seed": 29
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateBotMatchResponse =
            serde_json::from_value(created).expect("bot create response should parse");

        let (status, body) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 0,
                "score": { "0": 0, "1": 0 },
                "human_player": 1,
                "owner_subject": "browser-a",
                "seed": 31
            }),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["code"], json!("SESSION_LIMIT_REACHED"));

        {
            let mut matches = state.matches.write().await;
            let hosted_match = matches
                .get_mut(&created.match_id)
                .expect("created match should exist");
            hosted_match.last_activity_at = Instant::now() - Duration::from_secs(61);
        }

        let (status, recreated) = json_request(
            app,
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 0,
                "score": { "0": 0, "1": 0 },
                "human_player": 1,
                "owner_subject": "browser-a",
                "seed": 37
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let recreated: CreateBotMatchResponse =
            serde_json::from_value(recreated).expect("bot create response should parse");
        assert_ne!(recreated.match_id, created.match_id);
    }

    #[tokio::test]
    async fn owner_subject_creation_rate_limit_returns_rate_limited() {
        let state = AppState::with_session_policy(SessionPolicy::for_tests(
            None,
            None,
            Some(Duration::from_secs(60)),
            Some(1),
        ));
        let app = app(state);

        let (status, _) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 },
                "human_player": 0,
                "owner_subject": "browser-a",
                "seed": 41
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = json_request(
            app,
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 0,
                "score": { "0": 0, "1": 0 },
                "human_player": 1,
                "owner_subject": "browser-a",
                "seed": 43
            }),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["code"], json!("RATE_LIMITED"));
    }

    #[tokio::test]
    async fn metadata_route_includes_last_bot_decision_after_bot_turn() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 },
                "human_player": 0,
                "bot_kind": "random",
                "seed": 5
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateBotMatchResponse =
            serde_json::from_value(created).expect("bot create response should parse");

        let (status, _started) = json_request(
            app.clone(),
            Method::POST,
            &format!("/matches/{}/start-hand", created.match_id),
            sample_hand_start_value(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _applied) = json_request(
            app.clone(),
            Method::POST,
            &format!("/matches/{}/actions", created.match_id),
            json!({ "type": "play_face_up", "card_id": "p0c0" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, metadata) = empty_request(
            app,
            Method::GET,
            &format!("/matches/{}/meta", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(metadata["bot_kind"], json!("random"));
        assert!(metadata["last_bot_decision"].is_object());
        assert!(metadata["last_bot_decision"]["plan"]["choices"]
            .as_array()
            .is_some_and(|choices| !choices.is_empty()));
    }

    #[tokio::test]
    async fn bot_override_route_sets_and_clears_pending_override_metadata() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 },
                "human_player": 0,
                "bot_kind": "random",
                "seed": 59
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateBotMatchResponse =
            serde_json::from_value(created).expect("bot create response should parse");

        let (status, response) = json_request(
            app.clone(),
            Method::PUT,
            &format!("/matches/{}/bot-override", created.match_id),
            json!({
                "action": { "type": "fold" }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["pending_override"], json!({ "type": "fold" }));

        let (status, metadata) = empty_request(
            app.clone(),
            Method::GET,
            &format!("/matches/{}/meta", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(metadata["pending_bot_override"], json!({ "type": "fold" }));

        let (status, response) = json_request(
            app.clone(),
            Method::PUT,
            &format!("/matches/{}/bot-override", created.match_id),
            json!({
                "action": null
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["pending_override"], json!(null));

        let (status, metadata) = empty_request(
            app,
            Method::GET,
            &format!("/matches/{}/meta", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(metadata["pending_bot_override"], json!(null));
    }

    #[tokio::test]
    async fn dev_only_hosted_routes_are_disabled_when_dev_routes_are_disabled() {
        let app = app(AppState::with_dev_routes_enabled(false));

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 },
                "human_player": 0,
                "bot_kind": "random",
                "seed": 59
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created_match_id = created["match_id"]
            .as_str()
            .expect("created match should include id");
        let created_state = created["state"].clone();

        let (status, response) = json_request(
            app.clone(),
            Method::PUT,
            &format!("/matches/{created_match_id}/bot-override"),
            json!({
                "action": { "type": "fold" }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(response["code"], json!("NOT_FOUND"));

        let (status, response) = json_request(
            app.clone(),
            Method::PUT,
            &format!("/matches/{created_match_id}"),
            created_state,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(response["code"], json!("NOT_FOUND"));

        let (status, response) = json_request(
            app,
            Method::POST,
            &format!("/matches/{created_match_id}/start-hand"),
            sample_hand_start_value(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(response["code"], json!("NOT_FOUND"));
    }

    #[tokio::test]
    async fn opponent_player_view_is_forbidden_when_dev_routes_are_disabled() {
        let app = app(AppState::with_dev_routes_enabled(false));

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 },
                "human_player": 0,
                "bot_kind": "random",
                "seed": 59
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created_match_id = created["match_id"]
            .as_str()
            .expect("created match should include id");

        let (status, _) = empty_request(
            app.clone(),
            Method::POST,
            &format!("/matches/{created_match_id}/start-hand/random"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = empty_request(
            app.clone(),
            Method::GET,
            &format!("/matches/{created_match_id}/players/0/view"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, response) = empty_request(
            app,
            Method::GET,
            &format!("/matches/{created_match_id}/players/1/view"),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(response["code"], json!("MATCH_FORBIDDEN"));
    }

    #[tokio::test]
    async fn bot_override_is_consumed_on_the_next_bot_turn() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 },
                "human_player": 0,
                "bot_kind": "random",
                "seed": 61
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateBotMatchResponse =
            serde_json::from_value(created).expect("bot create response should parse");

        let (status, _started) = json_request(
            app.clone(),
            Method::POST,
            &format!("/matches/{}/start-hand", created.match_id),
            sample_hand_start_value(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _override_set) = json_request(
            app.clone(),
            Method::PUT,
            &format!("/matches/{}/bot-override", created.match_id),
            json!({
                "action": {
                    "type": "play_face_up",
                    "card_id": "p1c1"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, applied) = json_request(
            app.clone(),
            Method::POST,
            &format!("/matches/{}/actions", created.match_id),
            json!({ "type": "play_face_up", "card_id": "p0c0" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            applied["state"]["current_hand"]["state"]["completed_rounds"][0]["plays"][1]["card"]
                ["id"],
            json!("p1c1")
        );
        assert_eq!(
            applied["state"]["current_hand"]["state"]["completed_rounds"][0]["plays"][1]["card"]
                ["rank"],
            json!("5")
        );
        assert_eq!(
            applied["state"]["current_hand"]["state"]["completed_rounds"][0]["plays"][1]["card"]
                ["suit"],
            json!("SPADES")
        );

        let (status, metadata) = empty_request(
            app,
            Method::GET,
            &format!("/matches/{}/meta", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(metadata["pending_bot_override"], json!(null));
        assert_eq!(
            metadata["last_bot_decision"]["action"],
            json!({
                "type": "play_face_up",
                "card_id": "p1c1"
            })
        );
    }

    #[tokio::test]
    async fn seeded_bot_match_replays_the_lab_line_and_hosts_the_bot() {
        let state = AppState::default();
        let app = app(state.clone());

        // 10x10, dealer 0, Jack vira. The human is seat 0 (pé); the bot sits
        // at seat 1 (mão) and its hand is pinned so the deal is deterministic.
        // Line: mão opens class 3, hero raises, mão accepts.
        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches/seeded",
            json!({
                "score": { "0": 10, "1": 10 },
                "dealer": 0,
                "vira_rank": "J",
                "human_player": 0,
                "hero_hand": [2, 5, 12],
                "villain_hand": [3, 1, 8],
                "history": [
                    { "seat": 1, "kind": "play_face_up", "class": 3 },
                    { "seat": 0, "kind": "raise", "to": 3 },
                    { "seat": 1, "kind": "accept_raise" }
                ],
                "bot_kind": "heuristic"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "seeded create failed: {created}");
        assert_eq!(created["villain_sampling"], json!("pinned"));
        assert_eq!(created["human_player"], json!(0));

        // The replay landed: hand value 3 (accepted raise) and the villain's
        // opening card is on the table. The human (pé) is to act.
        let hand = &created["state"]["current_hand"]["state"];
        assert_eq!(hand["hand_value"], json!(3));
        assert_eq!(hand["turnup"]["rank"], json!("J"));
        assert_eq!(hand["current_round"]["plays"][0]["player"], json!(1));
        assert_eq!(created["public_view"]["current_player"], json!(0));

        // The hosted match is live: the human can act on it.
        let match_id = created["match_id"].as_str().expect("match id");
        let (status, _) = json_request(
            app.clone(),
            Method::GET,
            &format!("/matches/{match_id}/legal-actions"),
            json!(null),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn seeded_bot_match_rejects_invalid_specs() {
        let state = AppState::default();
        let app = app(state.clone());

        // Two-card hero hand.
        let (status, body) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches/seeded",
            json!({
                "score": { "0": 10, "1": 10 },
                "dealer": 0,
                "vira_rank": "J",
                "human_player": 0,
                "hero_hand": [2, 5],
                "bot_kind": "heuristic"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], json!("INVALID_SEED"));

        // Out-of-turn history (pé cannot lead the first trick).
        let (status, body) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches/seeded",
            json!({
                "score": { "0": 10, "1": 10 },
                "dealer": 0,
                "vira_rank": "J",
                "human_player": 0,
                "hero_hand": [2, 5, 12],
                "history": [ { "seat": 0, "kind": "play_face_up", "class": 2 } ],
                "bot_kind": "heuristic"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], json!("INVALID_SEED"));

        // Solver opponent without artifacts mounted -> unavailable, not 500.
        if std::env::var(crate::bot_host::SOLVER_POLICY_DIR_ENV).is_err() {
            let (status, body) = json_request(
                app.clone(),
                Method::POST,
                "/bot-matches/seeded",
                json!({
                    "score": { "0": 10, "1": 10 },
                    "dealer": 0,
                    "vira_rank": "J",
                    "human_player": 0,
                    "hero_hand": [2, 5, 12],
                    "bot_kind": "solver"
                }),
            )
            .await;
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(body["code"], json!("SOLVER_BOT_UNAVAILABLE"));
        }
    }

    #[test]
    fn solver_bot_without_policy_artifacts_is_unavailable() {
        // The solver store is a process-wide OnceLock read from
        // SOLVER_POLICY_DIR; when the variable is absent, creating a
        // solver-bot match must fail with the dedicated, frontend-visible
        // error rather than a generic 500.
        if std::env::var(crate::bot_host::SOLVER_POLICY_DIR_ENV).is_ok() {
            return;
        }
        let game = truco_engine::Match::new(0, Score { zero: 10, one: 10 })
            .expect("match should initialize");
        let result = super::HostedMatch::human_vs_bot(
            game.export_state(),
            0,
            1,
            super::HostedBotKind::Solver,
            super::BotProfile::Balanced,
            None,
            Some(7),
            None,
            None,
        );
        assert!(matches!(
            result,
            Err(super::ServiceError::SolverBotUnavailable)
        ));
    }

    #[tokio::test]
    async fn next_legal_raise_override_forces_the_bots_next_available_raise() {
        let state = AppState::default();
        let app = app(state.clone());
        let hand: truco_engine::HandStart =
            serde_json::from_value(sample_hand_start_value()).expect("hand fixture should parse");

        let mut game = truco_engine::Match::new(1, Score { zero: 0, one: 0 })
            .expect("match should initialize");
        game.start_hand(hand.turnup, hand.hands)
            .expect("hand should start");
        game.apply_action_for_current_player(&truco_engine::Action::PlayFaceUp {
            card_id: "p0c0".into(),
        })
        .expect("hero should open the round");
        game.apply_action_for_current_player(&truco_engine::Action::Raise { to: 3 })
            .expect("villain raise should succeed");

        let hosted = super::HostedMatch::human_vs_bot(
            game.export_state(),
            0,
            1,
            super::HostedBotKind::Heuristic,
            super::BotProfile::Balanced,
            None,
            Some(109),
            None,
            None,
        )
        .expect("hosted bot match should initialize");

        {
            let mut matches = state.matches.write().await;
            matches.insert("match-next-legal-raise".to_string(), hosted);
        }

        let (status, response) = json_request(
            app.clone(),
            Method::PUT,
            "/matches/match-next-legal-raise/bot-override",
            json!({
                "action": { "type": "next_legal_raise" }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["pending_override"],
            json!({ "type": "next_legal_raise" })
        );

        let (status, applied) = json_request(
            app.clone(),
            Method::POST,
            "/matches/match-next-legal-raise/actions",
            json!({ "type": "raise", "to": 6 }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            applied["state"]["current_hand"]["state"]["pending_raise"],
            json!({
                "raised_by": 1,
                "to": 9,
                "previous_value": 6
            })
        );
        assert_eq!(
            applied["result"]["snapshot"]["hand"]["pending_raise"],
            json!({
                "raised_by": 1,
                "to": 9,
                "previous_value": 6
            })
        );

        let (status, metadata) =
            empty_request(app, Method::GET, "/matches/match-next-legal-raise/meta").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(metadata["pending_bot_override"], json!(null));
        assert_eq!(
            metadata["last_bot_decision"]["action"],
            json!({ "type": "raise", "to": 9 })
        );
        assert_eq!(
            metadata["last_bot_decision"]["plan"]["reasoning"],
            json!("dev override: next legal raise")
        );
    }

    #[tokio::test]
    async fn llm_provider_catalog_route_returns_known_providers() {
        let app = app(AppState::default());
        let (status, catalogs) = empty_request(app, Method::GET, "/engine/llm-providers").await;
        assert_eq!(status, StatusCode::OK);
        let catalogs = catalogs
            .as_array()
            .expect("provider catalogs should serialize as an array");
        assert_eq!(catalogs.len(), 3);
        assert!(catalogs
            .iter()
            .any(|catalog| catalog["provider"] == json!("openai")));
        assert!(catalogs
            .iter()
            .any(|catalog| catalog["provider"] == json!("anthropic")));
        assert!(catalogs
            .iter()
            .any(|catalog| catalog["provider"] == json!("openrouter")));
    }

    #[tokio::test]
    async fn bot_match_can_random_deal_and_auto_advance_to_human_turn() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 },
                "human_player": 1,
                "seed": 7
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateBotMatchResponse =
            serde_json::from_value(created).expect("bot create response should parse");
        assert_eq!(created.human_player, 1);
        assert_eq!(created.bot_player, 0);
        assert_eq!(created.bot_kind, super::HostedBotKind::Heuristic);
        assert_eq!(created.bot_profile, super::BotProfile::Balanced);

        let (status, started) = empty_request(
            app.clone(),
            Method::POST,
            &format!("/matches/{}/start-hand/random", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(started["public_view"]["current_player"], json!(1));
        assert_eq!(started["public_view"]["hand_in_progress"], json!(true));

        let (status, player_view) = empty_request(
            app.clone(),
            Method::GET,
            &format!("/matches/{}/players/1/view", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(player_view["player"], json!(1));
        assert_eq!(
            player_view["hand"]["hand"]
                .as_array()
                .expect("hand list should be present")
                .len(),
            3
        );

        let (status, legal_actions) = empty_request(
            app,
            Method::GET,
            &format!("/matches/{}/legal-actions", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!legal_actions
            .as_array()
            .expect("actions should be an array")
            .is_empty());
    }

    #[tokio::test]
    async fn bot_match_can_defer_bot_eleven_decision_after_random_deal() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 11 },
                "human_player": 0,
                "seed": 17
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateBotMatchResponse =
            serde_json::from_value(created).expect("bot create response should parse");

        let (status, response) = json_request(
            app.clone(),
            Method::PUT,
            &format!("/matches/{}/bot-override", created.match_id),
            json!({
                "action": { "type": "fold_eleven" }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["pending_override"],
            json!({ "type": "fold_eleven" })
        );

        let (status, started) = json_request(
            app.clone(),
            Method::POST,
            &format!("/matches/{}/start-hand/random", created.match_id),
            json!({ "defer_bot_eleven": true }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(started["public_view"]["hand_in_progress"], json!(true));
        assert_eq!(started["public_view"]["current_player"], json!(1));
        assert_eq!(
            started["public_view"]["hand"]["pending_decision"],
            json!({ "type": "mao_de_onze", "player": 1 })
        );

        let (status, metadata) = empty_request(
            app.clone(),
            Method::GET,
            &format!("/matches/{}/meta", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(metadata["last_bot_decision"], json!(null));
        assert_eq!(
            metadata["pending_bot_override"],
            json!({ "type": "fold_eleven" })
        );

        let (status, advanced) = empty_request(
            app.clone(),
            Method::POST,
            &format!("/matches/{}/bot-turns", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(advanced["public_view"]["hand_in_progress"], json!(false));
        assert_eq!(advanced["public_view"]["score"], json!({ "0": 1, "1": 11 }));
        assert_eq!(advanced["state"]["current_hand"]["hand_winner"], json!(0));
        assert_eq!(
            advanced["state"]["current_hand"]["state"]["pending_decision"],
            json!(null)
        );

        let (status, metadata) = empty_request(
            app,
            Method::GET,
            &format!("/matches/{}/meta", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            metadata["last_bot_decision"]["action"],
            json!({ "type": "fold_eleven" })
        );
        assert_eq!(metadata["pending_bot_override"], json!(null));
    }

    #[tokio::test]
    async fn bot_match_auto_responds_after_human_action() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app.clone(),
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 },
                "human_player": 0,
                "seed": 11
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateBotMatchResponse =
            serde_json::from_value(created).expect("bot create response should parse");

        let (status, started) = empty_request(
            app.clone(),
            Method::POST,
            &format!("/matches/{}/start-hand/random", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(started["public_view"]["current_player"], json!(0));

        let (status, player_view) = empty_request(
            app.clone(),
            Method::GET,
            &format!("/matches/{}/players/0/view", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let card_id = player_view["hand"]["hand"][0]["id"]
            .as_str()
            .expect("first card id should exist")
            .to_string();

        let (status, applied) = json_request(
            app.clone(),
            Method::POST,
            &format!("/matches/{}/actions", created.match_id),
            json!({ "type": "play_face_up", "card_id": card_id }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(applied["result"]["acted_by"], json!(0));
        assert_eq!(applied["result"]["snapshot"]["current_player"], json!(0));

        let (status, legal_actions) = empty_request(
            app,
            Method::GET,
            &format!("/matches/{}/legal-actions", created.match_id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!legal_actions
            .as_array()
            .expect("actions should be an array")
            .is_empty());
    }

    #[tokio::test]
    async fn bot_match_can_reload_after_human_reraises_a_villain_raise() {
        let state = AppState::default();
        let app = app(state.clone());
        let hand: truco_engine::HandStart =
            serde_json::from_value(sample_hand_start_value()).expect("hand fixture should parse");

        let mut game = truco_engine::Match::new(1, Score { zero: 0, one: 0 })
            .expect("match should initialize");
        game.start_hand(hand.turnup, hand.hands)
            .expect("hand should start");
        game.apply_action_for_current_player(&truco_engine::Action::PlayFaceUp {
            card_id: "p0c0".into(),
        })
        .expect("hero should open the round");
        game.apply_action_for_current_player(&truco_engine::Action::Raise { to: 3 })
            .expect("villain raise should succeed");

        let hosted = super::HostedMatch::human_vs_bot(
            game.export_state(),
            0,
            1,
            super::HostedBotKind::Heuristic,
            super::BotProfile::Balanced,
            None,
            Some(97),
            None,
            None,
        )
        .expect("hosted bot match should initialize");

        {
            let mut matches = state.matches.write().await;
            matches.insert("match-reraise".to_string(), hosted);
        }

        let (status, applied) = json_request(
            app.clone(),
            Method::POST,
            "/matches/match-reraise/actions",
            json!({ "type": "raise", "to": 6 }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(applied["result"]["acted_by"], json!(0));

        let (status, _public_view) = empty_request(
            app.clone(),
            Method::GET,
            "/matches/match-reraise/public-view",
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _player_view) = empty_request(
            app.clone(),
            Method::GET,
            "/matches/match-reraise/players/0/view",
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, legal_actions) =
            empty_request(app, Method::GET, "/matches/match-reraise/legal-actions").await;
        if status != StatusCode::OK {
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(matches!(
                legal_actions["code"].as_str(),
                Some("NO_ACTIVE_HAND" | "HAND_ALREADY_DECIDED" | "MATCH_ALREADY_DECIDED")
            ));
        }
    }

    #[tokio::test]
    async fn stateful_match_can_reload_after_reraise_is_accepted_before_round_two_lead_card() {
        let state = AppState::default();
        let app = app(state.clone());

        let mut game = truco_engine::Match::new(1, Score { zero: 3, one: 1 })
            .expect("match should initialize");
        game.start_hand(
            Turnup {
                rank: Rank::Jack,
                suit: Suit::Spades,
            },
            Hands {
                zero: smallvec::smallvec![
                    Card {
                        id: "p0c0".into(),
                        rank: Rank::Five,
                        suit: Suit::Diamonds,
                    },
                    Card {
                        id: "p0c1".into(),
                        rank: Rank::Seven,
                        suit: Suit::Clubs,
                    },
                    Card {
                        id: "p0c2".into(),
                        rank: Rank::Three,
                        suit: Suit::Hearts,
                    },
                ],
                one: smallvec::smallvec![
                    Card {
                        id: "p1c0".into(),
                        rank: Rank::Six,
                        suit: Suit::Spades,
                    },
                    Card {
                        id: "p1c1".into(),
                        rank: Rank::Two,
                        suit: Suit::Spades,
                    },
                    Card {
                        id: "p1c2".into(),
                        rank: Rank::Three,
                        suit: Suit::Clubs,
                    },
                ],
            },
        )
        .expect("hand should start");
        game.apply_action_for_current_player(&Action::PlayFaceUp {
            card_id: "p0c0".into(),
        })
        .expect("hero should open the first round");
        game.apply_action_for_current_player(&Action::PlayFaceUp {
            card_id: "p1c0".into(),
        })
        .expect("villain should win the first round");
        game.apply_action_for_current_player(&Action::Raise { to: 3 })
            .expect("villain should raise before leading round two");

        {
            let mut matches = state.matches.write().await;
            matches.insert(
                "match-reraise-round-two".to_string(),
                super::HostedMatch::human_only(game.export_state(), Some(41), None),
            );
        }

        let (status, applied) = json_request(
            app.clone(),
            Method::POST,
            "/matches/match-reraise-round-two/actions",
            json!({ "type": "raise", "to": 6 }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(applied["result"]["acted_by"], json!(0));

        let (status, accepted) = json_request(
            app.clone(),
            Method::POST,
            "/matches/match-reraise-round-two/actions",
            json!({ "type": "accept_raise" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(accepted["result"]["acted_by"], json!(1));
        assert_eq!(accepted["result"]["snapshot"]["current_player"], json!(1));
        assert_eq!(
            accepted["state"]["current_hand"]["state"]["current_round"]["leader"],
            json!(1)
        );

        let (status, _public_view) = empty_request(
            app.clone(),
            Method::GET,
            "/matches/match-reraise-round-two/public-view",
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _player_view) = empty_request(
            app.clone(),
            Method::GET,
            "/matches/match-reraise-round-two/players/0/view",
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, legal_actions) = empty_request(
            app,
            Method::GET,
            "/matches/match-reraise-round-two/legal-actions",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!legal_actions
            .as_array()
            .expect("actions should be an array")
            .is_empty());
    }

    #[tokio::test]
    async fn bot_match_can_request_random_bot_kind() {
        let app = app(AppState::default());

        let (status, created) = json_request(
            app,
            Method::POST,
            "/bot-matches",
            json!({
                "starting_dealer": 1,
                "score": { "0": 0, "1": 0 },
                "human_player": 0,
                "bot_kind": "random",
                "bot_profile": "aggressive",
                "seed": 19
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: CreateBotMatchResponse =
            serde_json::from_value(created).expect("bot create response should parse");
        assert_eq!(created.bot_kind, super::HostedBotKind::Random);
        assert_eq!(created.bot_profile, super::BotProfile::Aggressive);
    }
}
