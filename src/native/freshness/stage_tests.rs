//! Fixture-driven checklist stage vectors SC1–SC10 and CS1–CS16
//! (`tests/fixtures/freshness_stages/vectors.json`, the shared
//! contract in `docs/freshness.md` §10). The Rust suite is the source
//! of truth: it runs every case, including the lint assertions that
//! stay Rust-only. bob-ledger-tools runs the cases whose `runners`
//! include `js` against its byte-for-byte copy.

use std::path::Path;

use chrono::NaiveDate;

use super::super::scan::{
    stage_conflict_message, stage_unused_message, stages_invalid_message,
};
use super::{
    checklist_from_tags, conflict_winner, queue, unused_stages, ChecklistKind,
    FreshnessRow, StageMember,
};
use crate::native::config::freshness::parse_freshness_config;

fn stage_fixture() -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/freshness_stages/vectors.json");
    let text = std::fs::read_to_string(&path).expect("read stages fixture");
    serde_json::from_str(&text).expect("parse stages fixture")
}

fn runners(vector: &serde_json::Value) -> Vec<&str> {
    vector["runners"]
        .as_array()
        .expect("runners array")
        .iter()
        .map(|runner| runner.as_str().expect("runner string"))
        .collect()
}

fn fixture_date(value: &serde_json::Value) -> NaiveDate {
    let text = value.as_str().expect("date string");
    NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("valid date")
}

/// Config vectors SC1–SC10: the real parser normalizes stage lists
/// per `docs/freshness.md` §4 without ever returning `ConfigError`.
#[test]
fn stage_config_vectors_match_rust() {
    let fixture = stage_fixture();
    let vectors = fixture["config"].as_array().expect("config array");
    assert!(!vectors.is_empty(), "fixture must carry config vectors");
    for vector in vectors {
        let id = vector["id"].as_str().unwrap_or("?");
        if !runners(vector).contains(&"rust") {
            continue;
        }
        let yaml = vector["yaml"].as_str().expect("yaml");
        let config = parse_freshness_config(yaml, Path::new("/config.yml"))
            .unwrap_or_else(|error| panic!("{id}: parse: {error:?}"));
        let pre: Vec<&str> = vector["pre"]
            .as_array()
            .expect("pre array")
            .iter()
            .map(|stage| stage.as_str().expect("stage string"))
            .collect();
        let post: Vec<&str> = vector["post"]
            .as_array()
            .expect("post array")
            .iter()
            .map(|stage| stage.as_str().expect("stage string"))
            .collect();
        assert_eq!(config.pre_stages, pre, "{id}: pre_stages");
        assert_eq!(config.post_stages, post, "{id}: post_stages");
        if let Some(days) = vector.get("interval").and_then(|days| {
            days.as_u64().and_then(|days| u16::try_from(days).ok())
        }) {
            assert_eq!(config.interval, days, "{id}: interval");
        }
        let mut diagnostics = Vec::new();
        if config.pre_stages_invalid.is_some() {
            diagnostics.push("pre_stages");
        }
        if config.post_stages_invalid.is_some() {
            diagnostics.push("post_stages");
        }
        let expected: Vec<&str> = vector["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .map(|diagnostic| {
                diagnostic["key"].as_str().expect("diagnostic key")
            })
            .collect();
        assert_eq!(diagnostics, expected, "{id}: diagnostics");
    }
}

/// One neutral fixture row becomes a real evaluator input: `is_todo`
/// follows the status symbol, future-scheduled rows leave the lane,
/// and `x` rows are closed. No fixture row is blocked, hidden,
/// templated, daily-note, or Today-linked.
fn fixture_row(
    definition: &serde_json::Value,
    today: NaiveDate,
) -> (FreshnessRow, bool) {
    let path = definition["path"].as_str().expect("row path").to_string();
    let line = definition["line"].as_u64().expect("row line") as u32;
    let status = definition["status"]
        .as_str()
        .expect("row status")
        .chars()
        .next()
        .expect("status symbol");
    let tags: Vec<String> = definition["tags"]
        .as_array()
        .expect("row tags")
        .iter()
        .map(|tag| tag.as_str().expect("tag string").to_string())
        .collect();
    let scheduled = definition
        .get("scheduled")
        .and_then(|date| date.as_str())
        .map(|text| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("valid date")
        });
    let recurring = definition
        .get("recurring")
        .and_then(|recurring| recurring.as_bool())
        .unwrap_or(false);
    let open = !matches!(status, 'x' | 'X' | '-');
    let row = FreshnessRow {
        path,
        line,
        status,
        is_todo: status == ' ',
        recurring,
        lane_visible: scheduled.is_none_or(|date| date <= today),
        is_daily_note: false,
        is_today: false,
        scheduled,
        due: None,
        start: None,
        created: None,
        raw_line: format!("- [{status}] {}", tags.join(" ")),
        note_refresh_raw: None,
        tracker: None,
        checklist: checklist_from_tags(&tags),
        tags,
    };
    (row, open)
}

/// Render the case's stage lists as the real config document, so the
/// queue runs through the same parser as production (CS8's reserved
/// `gtd` list arrives invalid, exactly as configured).
fn fixture_yaml(
    case: &serde_json::Value,
    defaults: &serde_json::Value,
) -> String {
    let pre = case.get("pre_stages").unwrap_or(&defaults["pre_stages"]);
    let post = case.get("post_stages").unwrap_or(&defaults["post_stages"]);
    format!("freshness:\n  pre_stages: {pre}\n  post_stages: {post}\n")
}

fn queue_key(path: &str, line: u32) -> String {
    format!("{path}:{line}")
}

/// Queue vectors CS1–CS16: stage order, rank/total, tiers, lanes,
/// and the Rust-only lint assertions.
#[test]
fn stage_queue_vectors_match_rust() {
    let fixture = stage_fixture();
    let defaults = &fixture["defaults"];
    let today = fixture_date(&defaults["today"]);
    let vectors = fixture["queue"].as_array().expect("queue array");
    assert!(!vectors.is_empty(), "fixture must carry queue vectors");
    for vector in vectors {
        let id = vector["id"].as_str().unwrap_or("?");
        if !runners(vector).contains(&"rust") {
            continue;
        }
        let yaml = fixture_yaml(vector, defaults);
        let config = parse_freshness_config(&yaml, Path::new("/config.yml"))
            .unwrap_or_else(|error| panic!("{id}: parse: {error:?}"));
        let built: Vec<(FreshnessRow, bool)> = vector["rows"]
            .as_array()
            .expect("rows array")
            .iter()
            .map(|definition| fixture_row(definition, today))
            .collect();
        let rows: Vec<FreshnessRow> =
            built.iter().map(|(row, _)| row.clone()).collect();
        let queued = queue(&rows, today, &config);

        let order: Vec<String> = vector["order"]
            .as_array()
            .expect("order array")
            .iter()
            .map(|key| key.as_str().expect("order key").to_string())
            .collect();
        let actual: Vec<String> = queued
            .iter()
            .map(|entry| queue_key(&entry.path, entry.line))
            .collect();
        assert_eq!(actual, order, "{id}: queue order");

        let stages = vector["stages"].as_object().expect("stages map");
        assert_eq!(
            stages.len(),
            queued.len(),
            "{id}: stages must cover every queued row"
        );
        for entry in &queued {
            let key = queue_key(&entry.path, entry.line);
            let expected = stages.get(&key).unwrap_or_else(|| {
                panic!("{id}: no stage expectation for {key}")
            });
            match expected {
                serde_json::Value::Null => {
                    assert_eq!(entry.stage, None, "{id}: {key} stage");
                    assert_eq!(entry.stage_rank, None, "{id}: {key} rank");
                    assert_eq!(entry.stage_total, None, "{id}: {key} total");
                }
                serde_json::Value::Object(_) => {
                    let name = expected["stage"].as_str().expect("stage name");
                    assert_eq!(
                        entry.stage.as_deref(),
                        Some(name),
                        "{id}: {key} stage"
                    );
                    assert_eq!(
                        entry.stage_rank,
                        Some(expected["rank"].as_u64().expect("rank") as u32),
                        "{id}: {key} rank"
                    );
                    assert_eq!(
                        entry.stage_total,
                        Some(expected["total"].as_u64().expect("total") as u32),
                        "{id}: {key} total"
                    );
                }
                other => panic!("{id}: {key} bad stage expectation {other}"),
            }
        }

        if let Some(tiers) = vector.get("tiers") {
            for (key, tier) in tiers.as_object().expect("tiers map") {
                let entry = queued
                    .iter()
                    .find(|entry| queue_key(&entry.path, entry.line) == *key)
                    .unwrap_or_else(|| panic!("{id}: no queued row {key}"));
                assert_eq!(
                    entry.tier.as_str(),
                    tier.as_str().expect("tier string"),
                    "{id}: {key} tier"
                );
            }
        }
        if let Some(lanes) = vector.get("lanes") {
            for (key, lane) in lanes.as_object().expect("lanes map") {
                let entry = queued
                    .iter()
                    .find(|entry| queue_key(&entry.path, entry.line) == *key)
                    .unwrap_or_else(|| panic!("{id}: no queued row {key}"));
                assert_eq!(
                    entry.lane.map(|lane| lane.as_str()),
                    lane.as_str(),
                    "{id}: {key} lane"
                );
            }
        }

        // Rust-only lint assertions in canonical emission order:
        // per-row conflicts in scan order, then invalid lists
        // (`pre_stages`, `post_stages`), then unused stages. A case
        // opts out by naming other `lint_runners`.
        let linted_in_rust = vector
            .get("lint_runners")
            .map(|runners| {
                runners
                    .as_array()
                    .expect("lint runners array")
                    .iter()
                    .any(|runner| runner.as_str() == Some("rust"))
            })
            .unwrap_or(true);
        if !linted_in_rust {
            continue;
        }
        let mut actual_lints = Vec::new();
        for (row, open) in &built {
            if !open {
                continue;
            }
            if let Some(kind) = row.checklist
                && let Some(winner) = conflict_winner(&row.tags, kind, &config)
            {
                actual_lints.push((
                    "checklist_stage_conflict".to_string(),
                    row.path.clone(),
                    Some(row.line),
                    stage_conflict_message(&winner),
                ));
            }
        }
        for (key, problem) in [
            ("pre_stages", config.pre_stages_invalid.as_ref()),
            ("post_stages", config.post_stages_invalid.as_ref()),
        ] {
            if let Some(problem) = problem {
                actual_lints.push((
                    "freshness_stages_invalid".to_string(),
                    String::new(),
                    None,
                    stages_invalid_message(key, problem),
                ));
            }
        }
        let members: Vec<StageMember> = built
            .iter()
            .map(|(row, open)| StageMember {
                kind: row.checklist,
                open: *open,
                tags: row.tags.clone(),
            })
            .collect();
        for (kind, stage) in unused_stages(&members, &config) {
            let key = match kind {
                ChecklistKind::Pre => "pre_stages",
                ChecklistKind::Post => "post_stages",
            };
            actual_lints.push((
                "checklist_stage_unused".to_string(),
                key.to_string(),
                None,
                stage_unused_message(kind, &stage),
            ));
        }
        let expected_lints: Vec<(String, String, Option<u32>, String)> = vector
            ["lints"]
            .as_array()
            .expect("lints array")
            .iter()
            .map(|lint| {
                let code = lint["code"].as_str().expect("lint code");
                match code {
                    "checklist_stage_conflict" => (
                        code.to_string(),
                        lint["path"].as_str().expect("lint path").to_string(),
                        Some(lint["line"].as_u64().expect("lint line") as u32),
                        stage_conflict_message(
                            lint["winner"].as_str().expect("winner"),
                        ),
                    ),
                    "freshness_stages_invalid" => {
                        let key = lint["key"].as_str().expect("lint key");
                        let problem = match key {
                            "pre_stages" => config
                                .pre_stages_invalid
                                .as_ref()
                                .expect("pre problem"),
                            "post_stages" => config
                                .post_stages_invalid
                                .as_ref()
                                .expect("post problem"),
                            other => panic!("{id}: bad lint key {other}"),
                        };
                        (
                            code.to_string(),
                            String::new(),
                            None,
                            stages_invalid_message(key, problem),
                        )
                    }
                    "checklist_stage_unused" => {
                        let key = lint["key"].as_str().expect("lint key");
                        let stage = lint["stage"].as_str().expect("lint stage");
                        let kind = match key {
                            "pre_stages" => ChecklistKind::Pre,
                            "post_stages" => ChecklistKind::Post,
                            other => panic!("{id}: bad lint key {other}"),
                        };
                        (
                            code.to_string(),
                            key.to_string(),
                            None,
                            stage_unused_message(kind, stage),
                        )
                    }
                    other => panic!("{id}: bad lint code {other}"),
                }
            })
            .collect();
        // Invalid and unused warnings land on the config path in
        // production; the fixture pins their identity by key, so the
        // comparison folds the path into the code string.
        let fold = |(code, scope, line, message): &(
            String,
            String,
            Option<u32>,
            String,
        )| {
            (format!("{code} {scope}"), *line, message.clone())
        };
        assert_eq!(
            actual_lints.iter().map(fold).collect::<Vec<_>>(),
            expected_lints.iter().map(fold).collect::<Vec<_>>(),
            "{id}: lints"
        );
    }
}
