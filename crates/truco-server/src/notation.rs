use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use thiserror::Error;
use truco_engine::Player;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotationCompileRequest {
    pub notation: String,
    pub human_player: Player,
    pub bot_player: Player,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotationCompileResponse {
    pub fragment: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[error("{message}")]
pub struct NotationParseError {
    pub message: String,
    pub line: usize,
    pub column: usize,
    pub statement: String,
    pub source_line: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct NotationContext {
    pub human_player: Player,
    pub bot_player: Player,
}

#[derive(Debug)]
struct StatementMeta {
    text: String,
    line: usize,
    column: usize,
}

#[derive(Debug)]
struct NotationIssue {
    message: String,
    token: Option<String>,
}

impl NotationParseError {
    pub fn code(&self) -> &'static str {
        "NOTATION_PARSE_ERROR"
    }
}

impl NotationIssue {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            token: None,
        }
    }

    fn with_token(message: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            token: Some(token.into()),
        }
    }
}

pub fn compile_notation_fragment(
    input: &str,
    context: NotationContext,
) -> Result<Value, NotationParseError> {
    let lines: Vec<&str> = input.split('\n').collect();
    let statements = split_notation_statements(input);

    if statements.is_empty() {
        return Err(NotationParseError {
            message: "Add at least one notation statement to compile.".to_string(),
            line: 1,
            column: 1,
            statement: String::new(),
            source_line: lines.first().copied().unwrap_or_default().to_string(),
            token: None,
        });
    }

    let mut fragment = Map::new();
    for statement in statements {
        let compiled = compile_statement(&statement.text, context)
            .map_err(|error| build_notation_error(&error, &statement, lines[statement.line - 1]))?;
        merge_fragment(&mut fragment, compiled, &statement.text)
            .map_err(|error| build_notation_error(&error, &statement, lines[statement.line - 1]))?;
    }

    Ok(Value::Object(fragment))
}

fn normalize_player_ref(token: &str, context: NotationContext) -> Result<Player, NotationIssue> {
    let normalized = token.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "hero" | "h" | "p0" => Ok(0),
        "villain" | "v" | "p1" => Ok(1),
        "me" => Ok(context.human_player),
        "opp" | "opponent" | "bot" => Ok(context.bot_player),
        _ => Err(NotationIssue::with_token(
            format!("Unknown player reference: {token}"),
            token.to_string(),
        )),
    }
}

fn parse_rank(token: &str) -> Result<&'static str, NotationIssue> {
    match token.trim().to_ascii_uppercase().as_str() {
        "A" => Ok("A"),
        "2" => Ok("2"),
        "3" => Ok("3"),
        "4" => Ok("4"),
        "5" => Ok("5"),
        "6" => Ok("6"),
        "7" => Ok("7"),
        "J" => Ok("J"),
        "Q" => Ok("Q"),
        "K" => Ok("K"),
        _ => Err(NotationIssue::with_token(
            format!("Unknown rank: {token}"),
            token.to_string(),
        )),
    }
}

fn parse_suit(token: &str) -> Result<&'static str, NotationIssue> {
    match token.trim().to_ascii_uppercase().as_str() {
        "S" | "SPADES" => Ok("SPADES"),
        "H" | "HEARTS" => Ok("HEARTS"),
        "D" | "DIAMONDS" => Ok("DIAMONDS"),
        "C" | "CLUBS" => Ok("CLUBS"),
        _ => Err(NotationIssue::with_token(
            format!("Unknown suit: {token}"),
            token.to_string(),
        )),
    }
}

fn parse_exact_card(token: &str) -> Result<Value, NotationIssue> {
    let compact = token.trim().replace(char::is_whitespace, "");
    let bytes = compact.as_bytes();
    if bytes.len() != 2 {
        return Err(NotationIssue::with_token(
            format!("Expected exact card like As or 2c, got: {token}"),
            token.to_string(),
        ));
    }

    let rank = parse_rank(&compact[0..1])?;
    let suit = match compact[1..2].to_ascii_uppercase().as_str() {
        "S" => "SPADES",
        "H" => "HEARTS",
        "D" => "DIAMONDS",
        "C" => "CLUBS",
        _ => {
            return Err(NotationIssue::with_token(
                format!("Expected exact card like As or 2c, got: {token}"),
                token.to_string(),
            ))
        }
    };

    Ok(json!({
        "rank": rank,
        "suit": suit,
    }))
}

fn parse_atomic_card_constraint(token: &str) -> Result<Value, NotationIssue> {
    let normalized = token.trim();
    if normalized.is_empty() {
        return Err(NotationIssue::new("Empty card constraint"));
    }

    if normalized.eq_ignore_ascii_case("manilha") {
        return Ok(json!({ "is_manilha": true }));
    }

    if normalized.eq_ignore_ascii_case("!manilha") {
        return Ok(json!({ "is_manilha": false }));
    }

    if let Some(rank) = normalized.strip_prefix(">=") {
        return Ok(json!({ "strength_at_least": parse_rank(rank.trim())? }));
    }

    if let Some(rank) = normalized.strip_prefix("<=") {
        return Ok(json!({ "strength_at_most": parse_rank(rank.trim())? }));
    }

    if normalized.ends_with('*') && normalized.len() == 2 {
        return Ok(json!({ "allowed_ranks": [parse_rank(&normalized[..1])?] }));
    }

    if let Some(ranks) = normalized.strip_prefix("rank in ") {
        let ranks = ranks.trim();
        if !(ranks.starts_with('{') && ranks.ends_with('}')) {
            return Err(NotationIssue::with_token(
                format!("Expected rank set like {{A,2,3}}, got: {ranks}"),
                ranks.to_string(),
            ));
        }

        let allowed_ranks: Vec<&'static str> = split_top_level(&ranks[1..ranks.len() - 1], ',')
            .into_iter()
            .map(|rank| parse_rank(&rank))
            .collect::<Result<_, _>>()?;

        return Ok(json!({ "allowed_ranks": allowed_ranks }));
    }

    if let Some(suits) = normalized.strip_prefix("suit in ") {
        let suits = suits.trim();
        if !(suits.starts_with('{') && suits.ends_with('}')) {
            return Err(NotationIssue::with_token(
                format!("Expected suit set like {{SPADES, CLUBS}}, got: {suits}"),
                suits.to_string(),
            ));
        }

        let allowed_suits: Vec<&'static str> = split_top_level(&suits[1..suits.len() - 1], ',')
            .into_iter()
            .map(|suit| parse_suit(&suit))
            .collect::<Result<_, _>>()?;

        return Ok(json!({ "allowed_suits": allowed_suits }));
    }

    Ok(json!({ "exact": parse_exact_card(normalized)? }))
}

fn parse_card_constraint(token: &str) -> Result<Value, NotationIssue> {
    let parts = split_top_level(token, '&');
    if parts.len() <= 1 {
        return parse_atomic_card_constraint(token);
    }

    let mut merged = Map::new();
    for part in parts {
        let Value::Object(fragment) = parse_atomic_card_constraint(&part)? else {
            return Err(NotationIssue::with_token(
                format!("Unsupported card predicate: {part}"),
                part,
            ));
        };

        for (key, value) in fragment {
            if merged.contains_key(&key) {
                return Err(NotationIssue::with_token(
                    format!("Duplicate predicate field in conjunction: {key}"),
                    part.clone(),
                ));
            }
            merged.insert(key, value);
        }
    }

    Ok(Value::Object(merged))
}

fn split_top_level(input: &str, delimiter: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut bracket_depth = 0;

    for character in input.chars() {
        if matches!(character, '[' | '{' | '(') {
            bracket_depth += 1;
        } else if matches!(character, ']' | '}' | ')') {
            bracket_depth -= 1;
        }

        if character == delimiter && bracket_depth == 0 {
            if !current.trim().is_empty() {
                parts.push(current.trim().to_string());
            }
            current.clear();
            continue;
        }

        current.push(character);
    }

    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    parts
}

fn split_weighted_branch(branch: &str) -> Result<(String, u64), NotationIssue> {
    let parts = split_top_level(branch, '@');
    match parts.as_slice() {
        [expression] => Ok((expression.to_string(), 1)),
        [expression, weight] => {
            let weight = weight.parse::<u64>().map_err(|_| {
                NotationIssue::with_token("Weight must be an integer", weight.clone())
            })?;
            Ok((expression.to_string(), weight))
        }
        _ => Err(NotationIssue::with_token(
            format!("Unsupported weighted branch: {branch}"),
            branch.to_string(),
        )),
    }
}

fn parse_ordered_slots(expression: &str) -> Result<Vec<Value>, NotationIssue> {
    let inner = expression[1..expression.len() - 1].trim();
    let slots: Vec<Value> = split_top_level(inner, ',')
        .into_iter()
        .map(|slot| parse_card_constraint(&slot))
        .collect::<Result<_, _>>()?;

    if slots.is_empty() {
        return Err(NotationIssue::new(
            "Ordered hand ranges need at least one slot",
        ));
    }

    Ok(slots)
}

fn parse_exact_hand_cards(expression: &str) -> Result<Vec<Value>, NotationIssue> {
    let inner = expression[1..expression.len() - 1].trim();
    let cards: Vec<Value> = split_top_level(inner, ',')
        .into_iter()
        .map(|card| parse_exact_card(&card))
        .collect::<Result<_, _>>()?;

    if cards.is_empty() {
        return Err(NotationIssue::new(
            "Exact hand ranges need at least one card",
        ));
    }

    Ok(cards)
}

fn parse_hand_expression(expression: &str) -> Result<Value, NotationIssue> {
    let weighted_hands: Vec<Value> = split_top_level(expression, '|')
        .into_iter()
        .map(|branch| {
            let (expression, weight) = split_weighted_branch(&branch)?;
            let trimmed = expression.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                return Ok(json!({
                    "weight": weight,
                    "hand": {
                        "type": "ordered",
                        "slots": parse_ordered_slots(trimmed)?,
                    }
                }));
            }

            if trimmed.starts_with('{') && trimmed.ends_with('}') {
                return Ok(json!({
                    "weight": weight,
                    "hand": {
                        "type": "exact",
                        "cards": parse_exact_hand_cards(trimmed)?,
                    }
                }));
            }

            Err(NotationIssue::with_token(
                format!("Unsupported hand expression: {expression}"),
                expression,
            ))
        })
        .collect::<Result<_, _>>()?;

    Ok(json!({
        "weighted_hands": weighted_hands
    }))
}

fn parse_weighted_card_constraints(expression: &str) -> Result<Value, NotationIssue> {
    let weighted_cards: Vec<Value> = split_top_level(expression, '|')
        .into_iter()
        .map(|branch| {
            let (expression, weight) = split_weighted_branch(&branch)?;
            Ok(json!({
                "weight": weight,
                "card": parse_card_constraint(&expression)?,
            }))
        })
        .collect::<Result<_, _>>()?;

    Ok(Value::Array(weighted_cards))
}

enum HiddenTarget {
    Current { player: Player },
    Completed { round_index: usize, player: Player },
}

fn parse_hidden_target(
    target: &str,
    context: NotationContext,
) -> Result<HiddenTarget, NotationIssue> {
    if !target.contains(',') && !target.contains('=') {
        return Ok(HiddenTarget::Current {
            player: normalize_player_ref(target, context)?,
        });
    }

    let parts: Vec<&str> = target.split(',').map(str::trim).collect();
    if parts.len() == 2 {
        let mut round_index = None;
        let mut player = None;

        for part in parts {
            if let Some(value) = part.strip_prefix("round=") {
                round_index = value.trim().parse::<usize>().ok();
                continue;
            }

            if let Some(value) = part.strip_prefix("player=") {
                player = Some(normalize_player_ref(value.trim(), context)?);
                continue;
            }
        }

        if let (Some(round_index), Some(player)) = (round_index, player) {
            return Ok(HiddenTarget::Completed {
                round_index,
                player,
            });
        }
    }

    Err(NotationIssue::with_token(
        format!("Unsupported hidden(...) target: {target}"),
        target.to_string(),
    ))
}

fn split_deal_args(inner: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut bracket_depth = 0;

    for character in inner.chars() {
        if matches!(character, '[' | '{' | '(') {
            bracket_depth += 1;
        } else if matches!(character, ']' | '}' | ')') {
            bracket_depth -= 1;
        }

        if character == ',' && bracket_depth == 0 {
            if !current.trim().is_empty() {
                args.push(current.trim().to_string());
            }
            current.clear();
            continue;
        }

        current.push(character);
    }

    if !current.trim().is_empty() {
        args.push(current.trim().to_string());
    }

    args
}

fn split_top_level_assignment(input: &str) -> Option<(String, String)> {
    let mut bracket_depth = 0;

    for (index, character) in input.char_indices() {
        if matches!(character, '[' | '{' | '(') {
            bracket_depth += 1;
            continue;
        }

        if matches!(character, ']' | '}' | ')') {
            bracket_depth -= 1;
            continue;
        }

        if character == '=' && bracket_depth == 0 {
            let left = input[..index].trim().to_string();
            let right = input[index + 1..].trim().to_string();
            return Some((left, right));
        }
    }

    None
}

fn compile_statement(statement: &str, context: NotationContext) -> Result<Value, NotationIssue> {
    let trimmed = statement.trim();

    if let Some(value) = trimmed.strip_prefix("seed=") {
        let seed = value
            .trim()
            .parse::<u64>()
            .map_err(|_| NotationIssue::with_token("Seed must be an integer", value.trim()))?;
        return Ok(json!({ "seed": seed }));
    }

    if trimmed.starts_with("hidden(") {
        let (left, right) = split_top_level_assignment(trimmed).ok_or_else(|| {
            NotationIssue::with_token(
                format!("Unsupported notation statement: {statement}"),
                statement.to_string(),
            )
        })?;
        let target = left
            .strip_prefix("hidden(")
            .and_then(|value| value.strip_suffix(')'))
            .ok_or_else(|| {
                NotationIssue::with_token(
                    format!("Unsupported notation statement: {statement}"),
                    statement.to_string(),
                )
            })?;
        let weighted_cards = parse_weighted_card_constraints(&right)?;

        return Ok(match parse_hidden_target(target, context)? {
            HiddenTarget::Current { player } => json!({
                "hidden_plays": {
                    player.to_string(): {
                        "weighted_cards": weighted_cards
                    }
                }
            }),
            HiddenTarget::Completed {
                round_index,
                player,
            } => json!({
                "completed_hidden_plays": [
                    {
                        "round_index": round_index,
                        "player": player,
                        "weighted_cards": weighted_cards,
                    }
                ]
            }),
        });
    }

    if trimmed.starts_with("deal(") && trimmed.ends_with(')') {
        let inner = &trimmed[5..trimmed.len() - 1];
        let args = split_deal_args(inner);
        let mut fragment = Map::new();
        let mut hidden_ranges = Map::new();

        for arg in args {
            if let Some(turnup) = arg.strip_prefix("turnup=") {
                fragment.insert(
                    "pending_hand".to_string(),
                    json!({ "turnup": parse_exact_card(turnup.trim())? }),
                );
                continue;
            }

            let pair: Vec<&str> = arg.splitn(2, '=').collect();
            if pair.len() == 2 {
                let player = normalize_player_ref(pair[0].trim(), context)?;
                hidden_ranges.insert(player.to_string(), parse_hand_expression(pair[1].trim())?);
                continue;
            }

            return Err(NotationIssue::with_token(
                format!("Unsupported deal(...) argument: {arg}"),
                arg,
            ));
        }

        if !hidden_ranges.is_empty() {
            fragment.insert("hidden_ranges".to_string(), Value::Object(hidden_ranges));
        }

        return Ok(Value::Object(fragment));
    }

    if let Some((left, right)) = split_top_level_assignment(trimmed) {
        let player = normalize_player_ref(left.trim(), context)?;
        return Ok(json!({
            "hidden_ranges": {
                player.to_string(): parse_hand_expression(right.trim())?,
            }
        }));
    }

    Err(NotationIssue::with_token(
        format!("Unsupported notation statement: {statement}"),
        statement.to_string(),
    ))
}

fn split_notation_statements(input: &str) -> Vec<StatementMeta> {
    let mut statements = Vec::new();

    for (line_index, line_text) in input.split('\n').enumerate() {
        let line_text = strip_top_level_comment(line_text);
        let mut current = String::new();
        let mut statement_column = 1;
        let mut bracket_depth = 0;

        for (index, character) in line_text.char_indices() {
            if current.is_empty() && !character.is_whitespace() {
                statement_column = index + 1;
            }

            if matches!(character, '[' | '{' | '(') {
                bracket_depth += 1;
            } else if matches!(character, ']' | '}' | ')') {
                bracket_depth -= 1;
            }

            if character == ';' && bracket_depth == 0 {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    statements.push(StatementMeta {
                        text: trimmed.to_string(),
                        line: line_index + 1,
                        column: statement_column,
                    });
                }
                current.clear();
                statement_column = index + 2;
                continue;
            }

            current.push(character);
        }

        let trimmed = current.trim();
        if !trimmed.is_empty() {
            let first_non_whitespace = line_text
                .char_indices()
                .find(|(_, character)| !character.is_whitespace())
                .map(|(index, _)| index + 1)
                .unwrap_or(statement_column);
            statements.push(StatementMeta {
                text: trimmed.to_string(),
                line: line_index + 1,
                column: first_non_whitespace,
            });
        }
    }

    statements
}

fn strip_top_level_comment(input: &str) -> &str {
    let mut bracket_depth = 0;

    for (index, character) in input.char_indices() {
        if matches!(character, '[' | '{' | '(') {
            bracket_depth += 1;
            continue;
        }

        if matches!(character, ']' | '}' | ')') {
            bracket_depth -= 1;
            continue;
        }

        if character == '#' && bracket_depth == 0 {
            return &input[..index];
        }
    }

    input
}

fn build_notation_error(
    issue: &NotationIssue,
    statement: &StatementMeta,
    raw_line: &str,
) -> NotationParseError {
    let token = issue.token.clone();
    let token_index = token
        .as_ref()
        .and_then(|token| statement.text.find(token))
        .unwrap_or(0);

    NotationParseError {
        message: issue.message.clone(),
        line: statement.line,
        column: statement.column + token_index,
        statement: statement.text.clone(),
        source_line: raw_line.to_string(),
        token,
    }
}

fn merge_fragment(
    target: &mut Map<String, Value>,
    fragment: Value,
    statement_text: &str,
) -> Result<(), NotationIssue> {
    let Value::Object(fragment) = fragment else {
        return Ok(());
    };

    for (key, value) in fragment {
        match (target.get_mut(&key), value) {
            (Some(Value::Array(existing)), Value::Array(mut incoming)) => {
                if key == "completed_hidden_plays" {
                    reject_conflicting_completed_hidden_targets(
                        existing,
                        &incoming,
                        statement_text,
                    )?;
                }
                existing.append(&mut incoming);
            }
            (Some(Value::Object(existing)), Value::Object(incoming)) => {
                for (child_key, child_value) in incoming {
                    if existing.contains_key(&child_key) {
                        return Err(NotationIssue::with_token(
                            format!("Conflicting notation assignment for {key}.{child_key}"),
                            statement_text.to_string(),
                        ));
                    }
                    existing.insert(child_key, child_value);
                }
            }
            (Some(_), _) => {
                return Err(NotationIssue::with_token(
                    format!("Conflicting notation assignment for {key}"),
                    statement_text.to_string(),
                ));
            }
            (None, replacement) => {
                target.insert(key, replacement);
            }
        }
    }

    Ok(())
}

fn reject_conflicting_completed_hidden_targets(
    existing: &[Value],
    incoming: &[Value],
    statement_text: &str,
) -> Result<(), NotationIssue> {
    let mut known_targets = std::collections::BTreeSet::new();

    for entry in existing {
        if let Some(target) = completed_hidden_play_target(entry) {
            known_targets.insert(target);
        }
    }

    for entry in incoming {
        if let Some((round_index, player)) = completed_hidden_play_target(entry) {
            if known_targets.contains(&(round_index, player)) {
                return Err(NotationIssue::with_token(
                    format!(
                        "Conflicting notation assignment for completed_hidden_plays[round={},player={}]",
                        round_index, player
                    ),
                    statement_text.to_string(),
                ));
            }
            known_targets.insert((round_index, player));
        }
    }

    Ok(())
}

fn completed_hidden_play_target(entry: &Value) -> Option<(usize, u8)> {
    let object = entry.as_object()?;
    let round_index = object.get("round_index")?.as_u64()? as usize;
    let player = object.get("player")?.as_u64()? as u8;
    Some((round_index, player))
}
