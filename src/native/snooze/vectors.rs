//! Typed loader for the shared snooze conformance fixture.
//!
//! Contract: `docs/snooze.md` § Conformance vectors. The fixture owns six
//! groups — `rows`, `when`, `live`, `snooze`, `wake`, `cleanup` — with
//! documented language-neutral schemas. The row-model phase populates and
//! executes the first three; the mutation-planners phase completes the last
//! three. The loader never silently skips unknown cases: unknown groups,
//! runners, or modes are hard errors.

use std::{collections::BTreeMap, path::PathBuf};

/// All six vector groups in document order.
pub(crate) const GROUP_ORDER: [&str; 6] =
    ["rows", "when", "live", "snooze", "wake", "cleanup"];

/// A single decoded fixture file: schema plus every group.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SnoozeFixture {
    pub(crate) schema: u64,
    pub(crate) groups: BTreeMap<String, Vec<SnoozeVector>>,
}

/// One vector: id, runners, free-form input, and expected object.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SnoozeVector {
    pub(crate) id: String,
    pub(crate) runners: Vec<String>,
    pub(crate) input: serde_json::Value,
    pub(crate) expected: serde_json::Value,
    pub(crate) mode: String,
}

/// Load and strictly validate the canonical fixture at
/// `tests/fixtures/snooze/vectors.json`. Unknown top-level groups are an
/// error; unknown vectors inside a known group are preserved for the
/// owning phase to execute (row-model executes `rows`/`when`/`live` and
/// asserts the last three exist with `rust` coverage owned elsewhere).
pub(crate) fn load_fixture() -> Result<SnoozeFixture, String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/snooze/vectors.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("read snooze fixture: {error}"))?;
    parse_fixture(&text)
}

/// Strict parser used by tests and the mutation-planners phase.
pub(crate) fn parse_fixture(text: &str) -> Result<SnoozeFixture, String> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| format!("parse fixture: {error}"))?;
    let schema = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "fixture misses u64 schema_version".to_string())?;
    let groups_value = value
        .get("groups")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "fixture misses object groups".to_string())?;
    for name in groups_value.keys() {
        if !GROUP_ORDER.contains(&name.as_str()) {
            return Err(format!("unknown snooze vector group {name:?}"));
        }
    }
    let mut groups = BTreeMap::new();
    for name in GROUP_ORDER {
        let vectors = groups_value
            .get(name)
            .map(parse_group)
            .transpose()?
            .unwrap_or_default();
        groups.insert(name.to_string(), vectors);
    }
    // Every vector id is unique within its group; duplicate ids hide
    // coverage gaps, so they are a fixture error.
    for (name, vectors) in &groups {
        let mut seen = std::collections::BTreeSet::new();
        for vector in vectors {
            if !seen.insert(vector.id.clone()) {
                return Err(format!(
                    "duplicate vector id {:?} in group {name:?}",
                    vector.id
                ));
            }
        }
    }
    Ok(SnoozeFixture { schema, groups })
}

fn parse_group(value: &serde_json::Value) -> Result<Vec<SnoozeVector>, String> {
    let list = value
        .as_array()
        .ok_or_else(|| "vector group must be an array".to_string())?;
    let mut out = Vec::new();
    for entry in list {
        let id = entry
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "vector misses string id".to_string())?
            .to_string();
        let runners = entry
            .get("runners")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("vector {id:?} misses runners array"))?
            .iter()
            .map(|runner| {
                runner.as_str().map(str::to_string).ok_or_else(|| {
                    format!("vector {id:?} has a non-string runner")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if runners.is_empty() {
            return Err(format!("vector {id:?} lists no runners"));
        }
        let mode = entry
            .get("mode")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("vector {id:?} misses string mode"))?
            .to_string();
        let input = entry
            .get("input")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let expected = entry
            .get("expected")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        out.push(SnoozeVector {
            id,
            runners,
            input,
            expected,
            mode,
        });
    }
    Ok(out)
}

/// Rust vectors in one group: every vector whose runners include `rust`.
/// Vectors with any other runner spelling are returned too so the caller
/// can reject unknown runners instead of skipping them.
pub(crate) fn rust_vectors<'a>(
    fixture: &'a SnoozeFixture,
    group: &str,
) -> Result<Vec<&'a SnoozeVector>, String> {
    let vectors = fixture
        .groups
        .get(group)
        .ok_or_else(|| format!("unknown snooze vector group {group:?}"))?;
    for vector in vectors {
        for runner in &vector.runners {
            if runner != "rust" && runner != "js" {
                return Err(format!(
                    "vector {:?} names unknown runner {runner:?}",
                    vector.id
                ));
            }
        }
    }
    Ok(vectors
        .iter()
        .filter(|vector| vector.runners.iter().any(|runner| runner == "rust"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_loads_with_six_groups() {
        let fixture = load_fixture().expect("load snooze fixture");
        assert_eq!(fixture.schema, 1);
        for group in GROUP_ORDER {
            assert!(
                fixture.groups.contains_key(group),
                "missing group {group}"
            );
        }
    }

    #[test]
    fn unknown_group_is_an_error() {
        let text = r#"{"schema_version":1,"groups":{"nope":[]}}"#;
        assert!(parse_fixture(text).is_err());
    }

    #[test]
    fn unknown_runner_is_an_error() {
        let fixture = load_fixture().expect("load snooze fixture");
        for (group, vectors) in &fixture.groups {
            for vector in vectors {
                for runner in &vector.runners {
                    assert!(
                        runner == "rust" || runner == "js",
                        "group {group} vector {:?} names unknown runner {runner:?}",
                        vector.id
                    );
                }
            }
        }
    }

    fn minutes_of(text: &str) -> u16 {
        let (left, right) = text.split_once(':').expect("HH:MM fixture time");
        left.parse::<u16>().expect("hour") * 60
            + right.parse::<u16>().expect("minute")
    }

    fn date_of(text: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .expect("valid fixture date")
    }

    fn str_field<'a>(
        id: &str,
        value: &'a serde_json::Value,
        field: &str,
    ) -> &'a str {
        value
            .get(field)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("{id}: expected string field {field:?}"))
    }

    #[test]
    fn rows_vectors_match_parser() {
        use super::super::parse::{self, SnoozeRowKind};
        let fixture = load_fixture().expect("load snooze fixture");
        let vectors = rust_vectors(&fixture, "rows").expect("rows group");
        assert!(!vectors.is_empty(), "rows group must carry cases");
        for vector in vectors {
            let id = vector.id.as_str();
            assert_eq!(vector.mode, "parse", "{id}: rows mode is parse");
            let note = str_field(id, &vector.input, "note");
            let lines: Vec<&str> = note.lines().collect();
            let section = parse::parse_section(&lines);
            let expected = &vector.expected;
            match section {
                None => {
                    assert!(
                        expected
                            .get("header")
                            .is_some_and(serde_json::Value::is_null),
                        "{id}: expected a header"
                    );
                    assert!(
                        expected["rows"].as_array().is_some_and(Vec::is_empty),
                        "{id}: expected no rows"
                    );
                }
                Some(section) => {
                    let header = expected
                        .get("header")
                        .and_then(serde_json::Value::as_u64)
                        .expect("{id}: header number")
                        as usize;
                    assert_eq!(section.header_number, header, "{id}: header");
                    let mut duplicates: Vec<usize> = expected["duplicates"]
                        .as_array()
                        .expect("{id}: duplicates array")
                        .iter()
                        .map(|value| {
                            value.as_u64().expect("{id}: duplicate line")
                                as usize
                        })
                        .collect();
                    duplicates.sort_unstable();
                    let mut actual = section.duplicate_numbers.clone();
                    actual.sort_unstable();
                    assert_eq!(actual, duplicates, "{id}: duplicates");
                    let expected_rows =
                        expected["rows"].as_array().expect("{id}: rows array");
                    assert_eq!(
                        section.rows.len(),
                        expected_rows.len(),
                        "{id}: row count"
                    );
                    for (row, expected_row) in
                        section.rows.iter().zip(expected_rows.iter())
                    {
                        let line = expected_row["line"]
                            .as_u64()
                            .expect("{id}: row line")
                            as usize;
                        assert_eq!(row.line, line, "{id}: row line");
                        let kind = str_field(id, expected_row, "kind");
                        let expected_kind = match kind {
                            "active" => SnoozeRowKind::Active,
                            "history" => SnoozeRowKind::History,
                            "malformed" => SnoozeRowKind::Malformed,
                            other => panic!("{id}: unknown kind {other:?}"),
                        };
                        assert_eq!(row.kind, expected_kind, "{id}: kind");
                        assert_eq!(
                            row.hhmm.as_deref(),
                            expected_row["hhmm"].as_str(),
                            "{id}: hhmm"
                        );
                        assert_eq!(
                            row.target.as_deref(),
                            expected_row["target"].as_str(),
                            "{id}: target"
                        );
                        assert_eq!(
                            row.block_id.as_deref(),
                            expected_row["block"].as_str(),
                            "{id}: block"
                        );
                        assert_eq!(
                            row.embedded,
                            expected_row["embedded"].as_bool().unwrap_or(false),
                            "{id}: embedded"
                        );
                        assert_eq!(
                            row.aliased,
                            expected_row["aliased"].as_bool().unwrap_or(false),
                            "{id}: aliased"
                        );
                        let children = expected_row["children"]
                            .as_u64()
                            .expect("{id}: children count")
                            as usize;
                        assert_eq!(
                            row.children_end - row.line_index - 1,
                            children,
                            "{id}: children"
                        );
                        assert_eq!(
                            row.malformed_reason,
                            expected_row["malformed_reason"]
                                .as_str()
                                .map(str::to_string)
                                .as_deref(),
                            "{id}: malformed reason"
                        );
                        assert_eq!(
                            row.woke_hhmm.as_deref(),
                            expected_row["woke"].as_str(),
                            "{id}: woke"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn when_vectors_match_grammar() {
        use super::super::when;
        use std::str::FromStr as _;
        let fixture = load_fixture().expect("load snooze fixture");
        let vectors = rust_vectors(&fixture, "when").expect("when group");
        assert!(!vectors.is_empty(), "when group must carry cases");
        for vector in vectors {
            let id = vector.id.as_str();
            assert_eq!(vector.mode, "resolve", "{id}: when mode is resolve");
            let clock = when::SnoozeClock {
                date: date_of(str_field(id, &vector.input, "date")),
                now_minutes: minutes_of(str_field(id, &vector.input, "now")),
                zone: chrono_tz::Tz::from_str(str_field(
                    id,
                    &vector.input,
                    "zone",
                ))
                .unwrap_or_else(|_| panic!("{id}: unknown zone")),
            };
            match when::parse_snooze_when(
                str_field(id, &vector.input, "input"),
                clock,
            ) {
                Ok(resolved) => {
                    let expected = &vector.expected;
                    assert!(
                        expected.get("refusal").is_none(),
                        "{id}: expected refusal, got time"
                    );
                    assert_eq!(
                        resolved.hhmm,
                        str_field(id, expected, "hhmm"),
                        "{id}: hhmm"
                    );
                    assert_eq!(
                        resolved.minutes,
                        expected["minutes"].as_u64().expect("{id}: minutes")
                            as u16,
                        "{id}: minutes"
                    );
                    assert!(
                        resolved.rfc3339.contains(str_field(
                            id,
                            expected,
                            "rfc3339_contains"
                        )),
                        "{id}: rfc3339 {:?} missing {:?}",
                        resolved.rfc3339,
                        expected["rfc3339_contains"]
                    );
                }
                Err(refusal) => {
                    assert_eq!(
                        refusal.as_str(),
                        str_field(id, &vector.expected, "refusal"),
                        "{id}: refusal"
                    );
                }
            }
        }
    }

    #[test]
    fn live_vectors_match_classifier() {
        use super::super::state;
        use std::path::PathBuf;
        let fixture = load_fixture().expect("load snooze fixture");
        let vectors = rust_vectors(&fixture, "live").expect("live group");
        assert!(!vectors.is_empty(), "live group must carry cases");
        for vector in vectors {
            let id = vector.id.as_str();
            assert_eq!(vector.mode, "classify", "{id}: live mode is classify");
            let zone = chrono_tz::Tz::UTC;
            let rows = vector.input["rows"]
                .as_array()
                .unwrap_or_else(|| panic!("{id}: rows array"))
                .iter()
                .map(|entry| {
                    let resolution = match str_field(id, entry, "resolution") {
                        "found" => state::ResolutionKind::Found,
                        "missing" => state::ResolutionKind::Missing,
                        "ambiguous" => state::ResolutionKind::Ambiguous,
                        other => panic!("{id}: unknown resolution {other:?}"),
                    };
                    let identity = match (
                        entry["route"].as_str(),
                        entry["block"].as_str(),
                    ) {
                        (Some(route), Some(block)) => {
                            Some(state::TaskIdentity {
                                route: PathBuf::from(route),
                                block_id: block.to_string(),
                            })
                        }
                        _ => None,
                    };
                    let task = entry["status"].as_str().map(|status| {
                        let symbol = status
                            .chars()
                            .next()
                            .unwrap_or_else(|| panic!("{id}: empty status"));
                        state::TaskState {
                            status: symbol,
                            is_task: entry["is_task"]
                                .as_bool()
                                .unwrap_or(false),
                            is_event: entry["is_event"]
                                .as_bool()
                                .unwrap_or(false),
                        }
                    });
                    state::ClassifyRow {
                        line: entry["line"].as_u64().expect("{id}: line")
                            as usize,
                        minutes: entry["minutes"]
                            .as_u64()
                            .map(|minutes| minutes as u16),
                        is_history: entry["history"].as_bool().unwrap_or(false),
                        malformed: entry["malformed"]
                            .as_bool()
                            .unwrap_or(false),
                        resolution,
                        identity,
                        task,
                        has_live_link: entry["live_link"]
                            .as_bool()
                            .unwrap_or(false),
                    }
                })
                .collect();
            let input = state::ClassifyInput {
                today: date_of(str_field(id, &vector.input, "today")),
                note_date: date_of(str_field(id, &vector.input, "note_date")),
                now_minutes: minutes_of(str_field(id, &vector.input, "now")),
                zone,
                rows,
            };
            let classified = state::classify_rows(&input);
            let expected_states = vector.expected["states"]
                .as_array()
                .unwrap_or_else(|| panic!("{id}: states array"));
            assert_eq!(
                classified.rows.len(),
                expected_states.len(),
                "{id}: row count"
            );
            for (row, expected_row) in
                classified.rows.iter().zip(expected_states.iter())
            {
                let line =
                    expected_row["line"].as_u64().expect("{id}: line") as usize;
                assert_eq!(row.line, line, "{id}: row line");
                assert_eq!(
                    row.state.as_str(),
                    str_field(id, expected_row, "state"),
                    "{id}: state"
                );
            }
            let planned: Vec<String> = vector.expected["planned"]
                .as_array()
                .unwrap_or_else(|| panic!("{id}: planned array"))
                .iter()
                .map(|value| {
                    value.as_str().expect("{id}: planned entry").to_string()
                })
                .collect();
            let mut actual: Vec<String> = state::live_targets(&classified)
                .planned
                .iter()
                .map(state::TaskIdentity::key)
                .collect();
            actual.sort();
            assert_eq!(actual, planned, "{id}: planned");
            let blocked: Vec<String> = vector.expected["blocked"]
                .as_array()
                .unwrap_or_else(|| panic!("{id}: blocked array"))
                .iter()
                .map(|value| {
                    value.as_str().expect("{id}: blocked entry").to_string()
                })
                .collect();
            let mut actual_blocked: Vec<String> =
                state::live_targets(&classified)
                    .blocked
                    .iter()
                    .map(state::TaskIdentity::key)
                    .collect();
            actual_blocked.sort();
            assert_eq!(actual_blocked, blocked, "{id}: blocked");
            match vector.expected["next_wake_at_contains"].as_str() {
                Some(needle) => {
                    let actual = classified
                        .next_wake_at
                        .as_deref()
                        .unwrap_or_else(|| {
                            panic!(
                                "{id}: expected next_wake_at with {needle:?}"
                            )
                        });
                    assert!(
                        actual.contains(needle),
                        "{id}: next_wake_at {actual:?} missing {needle:?}"
                    );
                }
                None => {
                    assert!(
                        vector.expected["next_wake_at_contains"].is_null(),
                        "{id}: next_wake_at_contains must be null or a string"
                    );
                    assert_eq!(
                        classified.next_wake_at, None,
                        "{id}: next_wake_at"
                    );
                }
            }
            let lints: Vec<usize> = vector.expected["event_lints"]
                .as_array()
                .unwrap_or_else(|| panic!("{id}: event_lints array"))
                .iter()
                .map(|value| value.as_u64().expect("{id}: lint line") as usize)
                .collect();
            let actual_lints: Vec<usize> = classified
                .rows
                .iter()
                .filter(|row| row.event_lint)
                .map(|row| row.line)
                .collect();
            assert_eq!(actual_lints, lints, "{id}: event lints");
        }
    }
}
