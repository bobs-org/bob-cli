//! Reader vectors and selector tests for journal metrics.
//!
//! The data-driven cases live in
//! `tests/fixtures/stats/journal_reader_vectors.json` (kept stable and
//! additive); the R1-R24 writer vectors in `review_answers.json` prove the
//! recorded/countable writer cases feed the reader. Inline tests cover
//! custom tier filters and the review-closed wiring.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use chrono::NaiveDate;
use serde_json::Value;

use super::*;
use crate::native::{
    config::parse_stats_config, stats::registry::build_registry,
};

fn date(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
}

fn builtin(id: &str) -> MetricDef {
    let config =
        parse_stats_config("stats:\n", Path::new("/config.yml")).unwrap();
    build_registry(&config).get(id).unwrap().clone()
}

fn custom_metric(yaml: &str, id: &str) -> MetricDef {
    let config = parse_stats_config(
        &format!("stats:\n  metrics:\n    {id}:\n{}", indent(yaml)),
        Path::new("/config.yml"),
    )
    .unwrap();
    build_registry(&config).get(id).unwrap().clone()
}

fn indent(yaml: &str) -> String {
    yaml.lines().map(|line| format!("      {line}\n")).collect()
}

fn event(
    host: &str,
    day: NaiveDate,
    ts: &str,
    tier: &str,
    outcome: &str,
    key: &str,
) -> super::super::index::IndexedJournalEvent {
    super::super::index::IndexedJournalEvent {
        day,
        ts: ts.to_string(),
        host: host.to_string(),
        tier: tier.to_string(),
        outcome: outcome.to_string(),
        key: key.to_string(),
        path: format!(".bob/stats/review/2026-10.{host}.jsonl"),
        line: 1,
    }
}

/// A journal index holding exactly `events`, with the reader's since (the
/// earliest event day) recorded the way `index::read_journal` does.
fn journal_index(
    events: Vec<super::super::index::IndexedJournalEvent>,
) -> StatsIndex {
    let mut index = StatsIndex::default();
    index.health.journal.since = events.iter().map(|event| event.day).min();
    index.journal_events = events;
    index
}

/// A `review_closed` evaluation met exactly on `met` days.
fn closed_on(
    dates: &[NaiveDate],
    met: &[NaiveDate],
) -> BTreeMap<String, MetricEvaluation> {
    let values = dates
        .iter()
        .map(|day| DailyValue {
            date: *day,
            value: Some(i64::from(met.contains(day))),
            num: None,
            den: None,
            quality: DataQuality::Ok,
            reason: None,
            provisional: false,
            parts: BTreeMap::new(),
            checklist_state: None,
        })
        .collect();
    BTreeMap::from([(
        "review_closed".to_string(),
        MetricEvaluation {
            since: dates.first().copied(),
            values,
            warnings: Vec::new(),
        },
    )])
}

fn quality_named(name: &str) -> DataQuality {
    match name {
        "ok" => DataQuality::Ok,
        "partial" => DataQuality::Partial,
        "missing" => DataQuality::Missing,
        "not_tracked" => DataQuality::NotTracked,
        other => panic!("unknown quality {other:?}"),
    }
}

fn counts_map(value: &Value) -> BTreeMap<String, i64> {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, count)| (key.clone(), count.as_i64().unwrap()))
        .collect()
}

#[test]
fn reader_vectors_match_the_shared_fixture() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/stats/journal_reader_vectors.json");
    let raw = std::fs::read_to_string(&fixture).unwrap();
    let fixture: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(fixture["schema"], 1);
    let reviewed = builtin("reviewed");
    let keep_rate = builtin("keep_rate");

    for case in fixture["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let today = NaiveDate::parse_from_str(
            case["today"].as_str().unwrap(),
            "%Y-%m-%d",
        )
        .unwrap();
        let dates: Vec<NaiveDate> = case["dates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|day| {
                NaiveDate::parse_from_str(day.as_str().unwrap(), "%Y-%m-%d")
                    .unwrap()
            })
            .collect();
        let closed: Vec<NaiveDate> = case["review_closed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|day| {
                NaiveDate::parse_from_str(day.as_str().unwrap(), "%Y-%m-%d")
                    .unwrap()
            })
            .collect();
        let events = case["events"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(position, line)| {
                let host = line["host"].as_str().unwrap();
                let mut staged = event(
                    host,
                    NaiveDate::parse_from_str(
                        line["day"].as_str().unwrap(),
                        "%Y-%m-%d",
                    )
                    .unwrap(),
                    line["ts"].as_str().unwrap(),
                    line["tier"].as_str().unwrap(),
                    line["outcome"].as_str().unwrap(),
                    line["key"].as_str().unwrap(),
                );
                staged.path = line["path"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or(staged.path);
                staged.line = line["line"].as_u64().map_or(
                    u32::try_from(position + 1).unwrap_or(u32::MAX),
                    |number| u32::try_from(number).unwrap_or(u32::MAX),
                );
                staged
            })
            .collect();
        let index = journal_index(events);
        let evaluations = closed_on(&dates, &closed);
        let expect = &case["expect"];

        for (metric, select) in
            [(&reviewed, "reviewed"), (&keep_rate, "keep_rate")]
        {
            let result = evaluate_journal(
                metric,
                &index,
                &dates,
                today,
                None,
                &evaluations,
            )
            .unwrap();
            let expected_since = expect["since"].as_str().unwrap();
            assert_eq!(
                result.since.map(|day| day.to_string()).as_deref(),
                Some(expected_since),
                "{id}/{select}: since",
            );
            for day in &dates {
                let text = day.to_string();
                let expected = &expect[select][text.as_str()];
                let value = result
                    .values
                    .iter()
                    .find(|value| value.date == *day)
                    .unwrap();
                assert_eq!(
                    value.value,
                    expected["value"].as_i64(),
                    "{id}/{select}/{text}: value",
                );
                assert_eq!(
                    value.quality,
                    quality_named(expected["quality"].as_str().unwrap()),
                    "{id}/{select}/{text}: quality",
                );
                assert_eq!(
                    value.provisional,
                    *day == today,
                    "{id}/{select}/{text}: provisional",
                );
                if select == "keep_rate" {
                    assert_eq!(
                        value.num,
                        expected.get("num").and_then(Value::as_i64),
                        "{id}/{select}/{text}: num",
                    );
                    assert_eq!(
                        value.den,
                        expected.get("den").and_then(Value::as_i64),
                        "{id}/{select}/{text}: den",
                    );
                }
                assert_eq!(
                    value.parts.get("outcome").cloned().unwrap_or_default(),
                    counts_map(&expected["outcome"]),
                    "{id}/{select}/{text}: outcome parts",
                );
                assert_eq!(
                    value.parts.get("tier").cloned().unwrap_or_default(),
                    counts_map(&expected["tier"]),
                    "{id}/{select}/{text}: tier parts",
                );
            }
            let mut gaps: Vec<String> = result
                .warnings
                .iter()
                .filter(|warning| warning.code == "journal_gap")
                .map(|warning| warning.date.unwrap().to_string())
                .collect();
            gaps.sort();
            let mut expected_gaps: Vec<String> = expect["gaps"][select]
                .as_array()
                .unwrap()
                .iter()
                .map(|day| day.as_str().unwrap().to_string())
                .collect();
            expected_gaps.sort();
            assert_eq!(gaps, expected_gaps, "{id}/{select}: gap days");
            assert!(
                result.warnings.iter().all(|warning| {
                    warning.code == "journal_gap"
                        && warning.metric.as_deref() == Some(metric.id.as_str())
                        && warning.date.is_some()
                        && !warning.message.is_empty()
                }),
                "{id}/{select}: gap warnings carry metric, date, and message",
            );
        }
    }
}

#[test]
fn recorded_writer_vectors_feed_the_reader() {
    // Every R-vector with recorded=true becomes one journal line; the
    // fixture's reviewed flag must match the decision-tier reader exactly.
    // The journal writer lives in the Obsidian plugins, so this file stays
    // stable: only the reader behavior is pinned here.
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/stats/review_answers.json");
    let raw = std::fs::read_to_string(&fixture).unwrap();
    let fixture: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(fixture["schema"], 1);
    let vectors = fixture["vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 24);

    let mut events = Vec::new();
    let mut expected_reviewed = 0;
    for (position, vector) in vectors.iter().enumerate() {
        if !vector["recorded"].as_bool().unwrap() {
            continue;
        }
        if vector["reviewed"].as_bool().unwrap() {
            expected_reviewed += 1;
        }
        events.push(event(
            "apollo",
            date(10),
            &format!("2026-10-10T05:00:{:02}-04:00", position + 1),
            vector["tier"].as_str().unwrap(),
            vector["outcome"].as_str().unwrap(),
            vector["id"].as_str().unwrap(),
        ));
    }
    assert_eq!(expected_reviewed, 17);

    let expected_keys: BTreeSet<String> = vectors
        .iter()
        .filter(|vector| {
            vector["recorded"].as_bool().unwrap()
                && vector["reviewed"].as_bool().unwrap()
        })
        .map(|vector| vector["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(expected_keys.len(), 17);

    let index = journal_index(events);
    let evaluations = closed_on(&[date(10)], &[]);
    let reviewed = evaluate_journal(
        &builtin("reviewed"),
        &index,
        &[date(10)],
        date(11),
        None,
        &evaluations,
    )
    .unwrap();
    assert_eq!(reviewed.values[0].value, Some(17));
    let MetricSpec::Journal { select, tiers } = &builtin("reviewed").spec
    else {
        panic!("reviewed is a journal metric");
    };
    let counted = reviewed_keys_on(&index, date(10), *select, tiers);
    assert_eq!(counted, expected_keys);
    // Both the shipped decay spellings and the normalized ones are
    // recognized outcomes, never `other`.
    let outcome = &reviewed.values[0].parts["outcome"];
    assert!(outcome.get("other").is_none());
    for spelling in [
        "decay:notNow",
        "decay:level:0",
        "decay:lessOften",
        "decay:reword",
        "decay:drop",
        "decay:keep",
    ] {
        assert_eq!(outcome.get(spelling), Some(&1), "{spelling}");
    }

    // Only the pending/next rows feed keep rate; neither last answer is keep.
    let keep_rate = evaluate_journal(
        &builtin("keep_rate"),
        &index,
        &[date(10)],
        date(11),
        None,
        &evaluations,
    )
    .unwrap();
    assert_eq!(keep_rate.values[0].value, Some(0));
    assert_eq!(keep_rate.values[0].num, Some(0));
    assert_eq!(keep_rate.values[0].den, Some(7));
}

#[test]
fn custom_tier_lists_filter_reviewed_and_intersect_keep_rate() {
    let index = journal_index(vec![
        event(
            "apollo",
            date(10),
            "2026-10-10T05:00:00-04:00",
            "next",
            "keep",
            "a",
        ),
        event(
            "apollo",
            date(10),
            "2026-10-10T06:00:00-04:00",
            "pending",
            "keep",
            "b",
        ),
        event(
            "apollo",
            date(10),
            "2026-10-10T07:00:00-04:00",
            "new",
            "done",
            "c",
        ),
    ]);
    let evaluations = closed_on(&[date(10)], &[]);

    let reviewed = custom_metric(
        "label: Next only\nkind: journal\nselect: reviewed\ntiers: [next]",
        "next_only",
    );
    let result = evaluate_journal(
        &reviewed,
        &index,
        &[date(10)],
        date(11),
        None,
        &evaluations,
    )
    .unwrap();
    assert_eq!(result.values[0].value, Some(1));
    assert_eq!(
        result.values[0].parts["tier"],
        BTreeMap::from([("next".to_string(), 1)])
    );

    // Keep rate stays within pending/next even with `tiers: all`, and
    // intersects a narrower list down to just next.
    let keep_all = custom_metric(
        "label: Keep all\nkind: journal\nselect: keep_rate\ntiers: all",
        "keep_all",
    );
    let result = evaluate_journal(
        &keep_all,
        &index,
        &[date(10)],
        date(11),
        None,
        &evaluations,
    )
    .unwrap();
    assert_eq!(result.values[0].den, Some(2));

    let keep_next = custom_metric(
        "label: Keep next\nkind: journal\nselect: keep_rate\ntiers: [next]",
        "keep_next",
    );
    let result = evaluate_journal(
        &keep_next,
        &index,
        &[date(10)],
        date(11),
        None,
        &evaluations,
    )
    .unwrap();
    assert_eq!(result.values[0].den, Some(1));
    assert_eq!(result.values[0].num, Some(1));
}

#[test]
fn gap_uses_the_registry_review_closed_evaluation() {
    use crate::native::stats::index::{
        DateDiagnostic, DatedValue, IndexedTask, TaskKindPart,
    };
    use crate::native::stats::metrics::evaluate_metric;

    // A Morning review chore completed on the 10th, evaluated through the
    // real checklist evaluator: no second built-in checker hides here.
    let dated = |raw: Option<&str>| {
        let parsed = raw.and_then(|value| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
        });
        DatedValue {
            raw: raw.map(str::to_string),
            date: parsed,
            diagnostic: if parsed.is_some() {
                DateDiagnostic::Ok
            } else {
                DateDiagnostic::Missing
            },
        }
    };
    let checklist_index = StatsIndex {
        tasks: vec![IndexedTask {
            source_path: "2026/20261010.md".to_string(),
            archive_path: None,
            status_type: "DONE".to_string(),
            created: dated(Some("2026-10-09")),
            done: dated(Some("2026-10-10")),
            cancelled: dated(None),
            scheduled: dated(None),
            recurring: false,
            tags: vec!["gtd".to_string(), "post".to_string()],
            block_id: None,
            description: "Morning review".to_string(),
            task_kind: TaskKindPart::Task,
        }],
        ..StatsIndex::default()
    };
    let review_closed = builtin("review_closed");
    let closed = evaluate_metric(
        &review_closed,
        &checklist_index,
        &[date(9), date(10)],
        date(11),
        None,
    )
    .unwrap();
    assert_eq!(closed.values[0].value, Some(0));
    assert_eq!(closed.values[1].value, Some(1));
    let evaluations = BTreeMap::from([("review_closed".to_string(), closed)]);

    // No journal events at all: every day is not_tracked and gap
    // detection does not fire (there is no journal start).
    let empty = journal_index(Vec::new());
    let reviewed = evaluate_journal(
        &builtin("reviewed"),
        &empty,
        &[date(9), date(10)],
        date(11),
        None,
        &evaluations,
    )
    .unwrap();
    assert_eq!(reviewed.since, None);
    assert_eq!(reviewed.values[0].quality, DataQuality::NotTracked);
    assert_eq!(
        reviewed.values[0].reason.as_deref(),
        Some("no review answers have been recorded")
    );
    assert_eq!(reviewed.values[1].quality, DataQuality::NotTracked);
    assert!(reviewed
        .warnings
        .iter()
        .all(|warning| warning.code != "journal_gap"));

    // Tracking has started (an unrelated day has an event) so a closed
    // review with no events is a plugin gap.
    let index = journal_index(vec![event(
        "apollo",
        date(8),
        "2026-10-08T09:00:00-04:00",
        "pre",
        "keep",
        "warmup",
    )]);
    let reviewed = evaluate_journal(
        &builtin("reviewed"),
        &index,
        &[date(9), date(10)],
        date(11),
        None,
        &evaluations,
    )
    .unwrap();
    assert_eq!(reviewed.since, Some(date(8)));
    assert_eq!(reviewed.values[0].value, Some(0));
    assert_eq!(reviewed.values[1].value, None);
    assert_eq!(reviewed.values[1].quality, DataQuality::Missing);
    assert_eq!(reviewed.warnings.len(), 1);
    assert_eq!(reviewed.warnings[0].code, "journal_gap");
    assert_eq!(reviewed.warnings[0].date, Some(date(10)));

    // A redefined review_closed is honored as-is: the gap follows the
    // registry result, not the Morning review.
    let evening = custom_metric(
        "label: Evening\nkind: checklist\ntags: [gtd, post]\nmatch: Evening review\nmeasure: bool\nunit: bool",
        "review_closed",
    );
    assert_eq!(evening.id, "review_closed");
    let mut evening_index = checklist_index;
    evening_index.tasks[0].description = "Evening review".to_string();
    evening_index.tasks[0].done = dated(Some("2026-10-09"));
    let closed = evaluate_metric(
        &evening,
        &evening_index,
        &[date(9), date(10)],
        date(11),
        None,
    )
    .unwrap();
    assert_eq!(closed.values[0].value, Some(1));
    let evaluations = BTreeMap::from([("review_closed".to_string(), closed)]);
    let reviewed = evaluate_journal(
        &builtin("reviewed"),
        &index,
        &[date(9), date(10)],
        date(11),
        None,
        &evaluations,
    )
    .unwrap();
    assert_eq!(reviewed.warnings.len(), 1);
    assert_eq!(reviewed.warnings[0].date, Some(date(9)));
}

#[test]
fn missing_review_closed_reports_a_limitation_not_a_gap() {
    let index = journal_index(vec![event(
        "apollo",
        date(10),
        "2026-10-10T05:00:00-04:00",
        "next",
        "keep",
        "a",
    )]);
    let reviewed = evaluate_journal(
        &builtin("reviewed"),
        &index,
        &[date(10)],
        date(11),
        None,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(reviewed.values[0].value, Some(1));
    assert_eq!(reviewed.warnings.len(), 1);
    assert_eq!(reviewed.warnings[0].code, "journal_limitation");
    assert!(reviewed.warnings[0].message.contains("review_closed"));
}

#[test]
fn non_journal_metrics_evaluate_elsewhere() {
    let index = StatsIndex::default();
    assert!(evaluate_journal(
        &builtin("worked"),
        &index,
        &[date(10)],
        date(11),
        None,
        &BTreeMap::new(),
    )
    .is_none());
    assert!(super::super::formula::evaluate_formula(
        &builtin("reviewed"),
        &BTreeMap::new(),
        &[date(10)],
        date(11),
        None,
        None,
    )
    .is_none());
}

#[test]
fn unknown_outcome_including_checkmark_suffix_is_other() {
    let index = journal_index(vec![event(
        "apollo",
        date(10),
        "2026-10-10T09:00:00-04:00",
        "next",
        "done ✓",
        "row",
    )]);
    let evaluations = closed_on(&[date(10)], &[]);
    let reviewed = evaluate_journal(
        &builtin("reviewed"),
        &index,
        &[date(10)],
        date(11),
        None,
        &evaluations,
    )
    .unwrap();
    assert_eq!(reviewed.values[0].parts["outcome"]["other"], 1);
    assert!(!reviewed.values[0].parts["outcome"].contains_key("done"));
}

#[test]
fn last_answer_follows_instant_not_timestamp_string() {
    // 09:00Z is 09:00 UTC; 06:00-04:00 is 10:00 UTC, so the second event
    // is later even though its timestamp string sorts first.
    let mut first = event(
        "apollo",
        date(9),
        "2026-10-09T09:00:00+00:00",
        "next",
        "keep",
        "row",
    );
    first.path = ".bob/stats/review/2026-10.apollo.jsonl".to_string();
    first.line = 1;
    let mut second = event(
        "mac",
        date(9),
        "2026-10-09T06:00:00-04:00",
        "next",
        "done",
        "row",
    );
    second.path = ".bob/stats/review/2026-10.mac.jsonl".to_string();
    second.line = 1;
    let index = journal_index(vec![first, second]);
    let evaluations = closed_on(&[date(9)], &[]);
    let reviewed = evaluate_journal(
        &builtin("reviewed"),
        &index,
        &[date(9)],
        date(11),
        None,
        &evaluations,
    )
    .unwrap();
    assert_eq!(reviewed.values[0].parts["outcome"]["done"], 1);
    assert!(!reviewed.values[0].parts["outcome"].contains_key("keep"));
}

#[test]
fn malformed_only_closed_review_day_is_a_gap() {
    let index = journal_index(vec![event(
        "apollo",
        date(8),
        "2026-10-08T09:00:00-04:00",
        "next",
        "keep",
        "start",
    )]);
    let evaluations = closed_on(&[date(8), date(9)], &[date(9)]);
    let reviewed = evaluate_journal(
        &builtin("reviewed"),
        &index,
        &[date(8), date(9)],
        date(11),
        None,
        &evaluations,
    )
    .unwrap();
    assert_eq!(reviewed.values[1].quality, DataQuality::Missing);
    assert_eq!(
        reviewed.values[1].reason.as_deref(),
        Some("review closed but no answers were recorded")
    );
    assert_eq!(reviewed.warnings[0].code, "journal_gap");
    assert_eq!(reviewed.warnings[0].date, Some(date(9)));
}
