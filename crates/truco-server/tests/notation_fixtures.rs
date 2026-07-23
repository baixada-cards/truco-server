use std::{fs, path::PathBuf};

use serde::Deserialize;
use serde_json::Value;
use truco_server::notation::{compile_notation_fragment, NotationContext};

#[derive(Debug, Deserialize)]
struct NotationFixtureContext {
    human_player: u8,
    bot_player: u8,
}

#[derive(Debug, Deserialize)]
struct ExpectedNotationError {
    code: String,
    message_contains: String,
    line: usize,
    column: usize,
}

#[derive(Debug, Deserialize)]
struct NotationFixture {
    id: String,
    ruleset: String,
    context: NotationFixtureContext,
    notation: String,
    #[serde(default)]
    expect_fragment: Option<Value>,
    #[serde(default)]
    expect_error: Option<ExpectedNotationError>,
}

fn fixture_paths() -> Vec<PathBuf> {
    let fixture_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.cache/truco-spec/notation/fixtures");
    let mut paths = Vec::new();
    for aspect_dir in fs::read_dir(&fixture_root).expect("fixture root should read") {
        let aspect_dir = aspect_dir.expect("aspect entry should read");
        if !aspect_dir
            .file_type()
            .expect("aspect type should read")
            .is_dir()
        {
            continue;
        }
        for entry in fs::read_dir(aspect_dir.path()).expect("aspect dir should read") {
            let entry = entry.expect("fixture entry should read");
            if entry.path().extension().and_then(|value| value.to_str()) == Some("json") {
                paths.push(entry.path());
            }
        }
    }
    paths.sort();
    paths
}

fn assert_json_equal(actual: &Value, expected: &Value, id: &str) {
    assert_eq!(actual, expected, "{id}: compiled fragment mismatch");
}

#[test]
fn notation_fixture_corpus_passes_in_process() {
    for path in fixture_paths() {
        let raw = fs::read_to_string(&path).expect("fixture should read");
        let fixture: NotationFixture = serde_json::from_str(&raw).expect("fixture should parse");
        assert_eq!(fixture.ruleset, "truco-2p-v1", "{}", fixture.id);

        let context = NotationContext {
            human_player: fixture.context.human_player,
            bot_player: fixture.context.bot_player,
        };

        match (
            fixture.expect_fragment.as_ref(),
            fixture.expect_error.as_ref(),
        ) {
            (Some(expected), None) => {
                let compiled = compile_notation_fragment(&fixture.notation, context)
                    .unwrap_or_else(|error| panic!("{}: unexpected error: {error}", fixture.id));
                assert_json_equal(&compiled, expected, &fixture.id);
            }
            (None, Some(expected)) => {
                let error = compile_notation_fragment(&fixture.notation, context)
                    .expect_err("fixture should have produced an error");
                assert_eq!(error.code(), expected.code, "{}", fixture.id);
                assert!(
                    error.message.contains(&expected.message_contains),
                    "{}: expected message containing {:?}, got {:?}",
                    fixture.id,
                    expected.message_contains,
                    error.message
                );
                assert_eq!(error.line, expected.line, "{}", fixture.id);
                assert_eq!(error.column, expected.column, "{}", fixture.id);
            }
            _ => panic!("{}: fixture must declare exactly one outcome", fixture.id),
        }
    }
}
