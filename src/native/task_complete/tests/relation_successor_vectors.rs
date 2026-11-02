//! Shared config/key/copy vectors for relation successors.

use std::path::Path;

use serde_json::{Map, Value};

use super::super::relation_successors::copy::{
    self, EventKind, FoldCopy, GroupCopy, HeldNoticeCopy, HeldReason,
    PickedNoticeCopy, ReleaseState,
};
use crate::native::{
    config::relations::{parse_relations_config, LabelEffect},
    relation_labels::relation_label_key,
};

fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/fixtures/relation_successor/vectors.json"
    ))
    .expect("typed relation-successor vectors parse")
}

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("expected string at {key}: {value}"))
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|value| value.as_str().expect("string array entry").to_string())
        .collect()
}

fn number(value: &Value, key: &str) -> usize {
    value[key].as_u64().unwrap_or(0) as usize
}

fn event(value: &Value) -> EventKind {
    match value.as_str() {
        Some("parked") => EventKind::Parked,
        Some("done") => EventKind::Done,
        other => panic!("unknown relation event: {other:?}"),
    }
}

fn put(output: &mut Map<String, Value>, key: &str, value: impl Into<Value>) {
    output.insert(key.to_string(), value.into());
}

#[test]
fn config_vectors_exercise_the_three_effect_policy() {
    let data = vectors();
    for vector in data["config"].as_array().expect("config vectors") {
        let id = string(vector, "id");
        let parsed = parse_relations_config(
            string(vector, "yaml"),
            Path::new("/config.yml"),
        )
        .unwrap_or_else(|error| panic!("{id}: config should parse: {error}"));
        let expected = &vector["expected"];
        let actual_cancel = parsed
            .cancel_labels
            .iter()
            .map(|label| label.key.clone())
            .collect::<Vec<_>>();
        let actual_complete = parsed
            .complete_labels
            .iter()
            .map(|label| label.key.clone())
            .collect::<Vec<_>>();
        let actual_successor = parsed
            .successor_labels
            .iter()
            .map(|label| label.key.clone())
            .collect::<Vec<_>>();
        assert_eq!(actual_cancel, strings(expected.get("cancel")), "{id}");
        assert_eq!(actual_complete, strings(expected.get("complete")), "{id}");
        assert_eq!(
            actual_successor,
            strings(expected.get("successor")),
            "{id}"
        );
        assert_eq!(
            parsed.warnings.len(),
            number(expected, "warnings"),
            "{id} warnings: {:?}",
            parsed.warnings
        );
    }
}

#[test]
fn overlap_warnings_preserve_the_legacy_bytes_and_report_successor_conflicts() {
    let legacy = parse_relations_config(
        "relations:\n  cancel_labels: [starts after]\n  complete_labels: [starts after]\n",
        Path::new("/config.yml"),
    )
    .expect("legacy overlapping config parses");
    assert_eq!(
        legacy.warnings,
        ["relations: “starts after” is in both cancel_labels and complete_labels in /config.yml; it closes nothing"]
    );

    let legacy_order = parse_relations_config(
        "relations:\n  cancel_labels: [z label, a label]\n  complete_labels: [a label, z label]\n",
        Path::new("/config.yml"),
    )
    .expect("multiple legacy overlaps parse");
    assert_eq!(
        legacy_order.warnings,
        [
            "relations: “z label” is in both cancel_labels and complete_labels in /config.yml; it closes nothing",
            "relations: “a label” is in both cancel_labels and complete_labels in /config.yml; it closes nothing"
        ]
    );

    let successor_conflict = parse_relations_config(
        "relations:\n  cancel_labels: [starts after]\n  complete_labels: [starts after]\n  successor_labels: [starts after]\n",
        Path::new("/config.yml"),
    )
    .expect("three-way overlap parses");
    assert_eq!(
        successor_conflict.warnings,
        ["relations: “starts after” is in more than one of cancel_labels, complete_labels, successor_labels in /config.yml; it has no effect"]
    );
}

#[test]
fn key_vectors_cover_effects_inverse_hints_and_legacy_close_views() {
    let data = vectors();
    for vector in data["key"].as_array().expect("key vectors") {
        let id = string(vector, "id");
        let labels = strings(vector.get("successor_labels"));
        let yaml = format!(
            "relations:\n  successor_labels:\n{}",
            labels
                .iter()
                .map(|label| {
                    format!("    - '{}'\n", label.replace('\'', "''"))
                })
                .collect::<String>()
        );
        let parsed = parse_relations_config(&yaml, Path::new("/config.yml"))
            .unwrap_or_else(|error| {
                panic!("{id}: config should parse: {error}")
            });
        let input = string(vector, "label");
        let key = relation_label_key(input);
        let expected_effect = vector["expected"]["effect"].as_str();
        let actual_effect =
            parsed.label_effect(&key).map(|effect| match effect {
                LabelEffect::Cancel => "cancel",
                LabelEffect::Complete => "complete",
                LabelEffect::Successor => "successor",
            });
        assert_eq!(actual_effect, expected_effect, "{id} effect");
        assert_eq!(parsed.close_effect(&key), None, "{id} close effect");

        let expected_inverse = &vector["expected"]["inverse"];
        match expected_inverse {
            Value::String(phrase) => {
                assert_eq!(
                    crate::native::task_relations::label::inverse_phrase(input)
                        .as_deref(),
                    Some(phrase.as_str()),
                    "{id} inverse spelling"
                );
                let inverse_key = relation_label_key(phrase);
                assert_eq!(
                    parsed.label_for_inverse(&inverse_key),
                    labels.first().map(|label| {
                        (label.as_str(), LabelEffect::Successor)
                    }),
                    "{id} inverse policy lookup"
                );
                assert_eq!(
                    parsed.close_label_for_inverse(&inverse_key),
                    None,
                    "{id} legacy close lookup must omit successor labels"
                );
            }
            Value::Object(inverse) => {
                let label = inverse["label"].as_str().expect("inverse label");
                let effect = match inverse["effect"].as_str() {
                    Some("successor") => LabelEffect::Successor,
                    Some(other) => panic!("unexpected inverse effect: {other}"),
                    None => panic!("inverse effect missing"),
                };
                assert_eq!(
                    parsed.label_for_inverse(&key),
                    Some((label, effect)),
                    "{id} inverse lookup"
                );
                assert_eq!(parsed.close_label_for_inverse(&key), None, "{id}");
            }
            Value::Null => {
                assert_eq!(
                    parsed.label_for_inverse(&key),
                    None,
                    "{id} inverse"
                );
            }
            other => panic!("unexpected {id} inverse expectation: {other}"),
        }
    }
}

fn copy_outputs(vector: &Value) -> Map<String, Value> {
    let input = &vector["input"];
    let mut output = Map::new();
    let label = input.get("label").and_then(Value::as_str).unwrap_or("");
    let target = input.get("target").and_then(Value::as_str).unwrap_or("");
    let source = input.get("source").and_then(Value::as_str).unwrap_or("");
    let group = GroupCopy { label, target };

    match string(vector, "id") {
        "SU-P1" => put(&mut output, "title", copy::group_title(group)),
        "SU-P2" => {
            put(
                &mut output,
                "why",
                copy::event_why(target, event(&input["event"])),
            );
            put(&mut output, "tag", copy::contrast_tag());
        }
        "SU-P3" => put(
            &mut output,
            "why",
            copy::event_why(target, event(&input["event"])),
        ),
        "SU-P4" => {
            let waits = strings(input.get("waits_for"));
            let fold = copy::fold_line(FoldCopy {
                source: Some(source),
                waits_for: &waits,
                already_started: number(input, "already_started"),
                blocked: number(input, "blocked"),
                hidden: number(input, "hidden"),
            })
            .expect("nonempty fold vector");
            put(&mut output, "fold", fold);
        }
        "SU-P5" | "SU-P29" => {
            let targets = strings(input.get("targets"));
            put(
                &mut output,
                "suffix",
                copy::trigger_suffix(&targets).expect("multiple targets"),
            );
        }
        "SU-P6" => put(&mut output, "reason", copy::secondary_reason(group)),
        "SU-P7" | "SU-P30" => put(
            &mut output,
            "title",
            copy::picker_title(group, Some(number(input, "count")), None),
        ),
        "SU-P8" => put(&mut output, "capsule", copy::capsule(label)),
        "SU-P9" | "SU-P10" => put(
            &mut output,
            "notice",
            copy::picked_notice(PickedNoticeCopy {
                count: input.get("count").and_then(Value::as_u64).unwrap_or(1)
                    as usize,
                destination: string(input, "destination"),
                source: input.get("source").and_then(Value::as_str),
                label,
                target,
            }),
        ),
        "SU-P11" => put(
            &mut output,
            "title",
            copy::picker_title(
                group,
                None,
                input.get("existing_title").and_then(Value::as_str),
            ),
        ),
        "SU-P12" | "SU-P13" | "SU-P14" | "SU-P15" => {
            let reason = match input["reason"].as_str() {
                Some("waits_for") => HeldReason::WaitsFor(vec![input
                    ["other_target"]
                    .as_str()
                    .expect("other target")
                    .to_string()]),
                Some("already_started") => HeldReason::AlreadyStarted,
                Some("blocked") => HeldReason::Blocked {
                    waits_on: number(input, "waits_on"),
                },
                Some("scheduled") => HeldReason::Scheduled(
                    input["date"].as_str().expect("date").to_string(),
                ),
                other => panic!("unexpected held reason: {other:?}"),
            };
            put(
                &mut output,
                "notice",
                copy::held_notice(HeldNoticeCopy {
                    source,
                    label,
                    target,
                    reason: &reason,
                })
                .expect("visible held notice"),
            );
        }
        "SU-P16" => {
            let notices = input["reasons"]
                .as_array()
                .expect("held reasons")
                .iter()
                .map(|reason| {
                    let reason = match reason.as_str() {
                        Some("project") => HeldReason::Project,
                        Some("hidden") => HeldReason::Hidden,
                        other => panic!("unexpected held reason: {other:?}"),
                    };
                    copy::held_notice(HeldNoticeCopy {
                        source,
                        label,
                        target,
                        reason: &reason,
                    })
                    .expect("visible held notice")
                })
                .collect::<Vec<_>>();
            put(&mut output, "notices", notices);
        }
        "SU-P17" => {
            put(
                &mut output,
                "header",
                copy::cli_header(group, event(&input["event"])),
            );
            put(
                &mut output,
                "fold",
                copy::cli_held_fold(
                    source,
                    target,
                    number(input, "already_started"),
                ),
            );
        }
        "SU-P18" => {
            put(
                &mut output,
                "header",
                copy::cli_header(group, event(&input["event"])),
            );
            put(&mut output, "linked_suffix", copy::secondary_reason(group));
        }
        "SU-P19" => {
            put(
                &mut output,
                "notification",
                copy::mac_notification(number(input, "count"), label, target),
            );
            put(&mut output, "badge_help", copy::mac_badge_help());
        }
        "SU-P20" | "SU-P21" => {
            let labels = input
                .get("labels")
                .map(|labels| strings(Some(labels)))
                .unwrap_or_else(|| vec![label.to_string()]);
            put(
                &mut output,
                "notice",
                copy::uncertainty_notice(&labels).expect("labels present"),
            );
        }
        "SU-P22" => put(&mut output, "target", copy::task_text(target)),
        "SU-P23" => {
            put(&mut output, "suggestion", copy::suggestion());
            put(&mut output, "relation_row", copy::relation_row());
            put(
                &mut output,
                "list_header",
                copy::list_header(number(input, "count")),
            );
            put(
                &mut output,
                "list_row",
                copy::list_row(number(input, "count")),
            );
        }
        "SU-P24" => put(
            &mut output,
            "message",
            copy::release_caveat(ReleaseState::Parked, target, Some(source)),
        ),
        "SU-P25" => put(
            &mut output,
            "message",
            copy::release_caveat(ReleaseState::Closed, target, None),
        ),
        "SU-P26" => put(
            &mut output,
            "hint",
            copy::inverse_hint(
                string(input, "typed"),
                source,
                string(input, "label"),
            ),
        ),
        "SU-P27" => {
            let warning = copy::already_released_warning(label, source, target);
            put(&mut output, "code", warning.code);
            put(&mut output, "message", warning.message);
        }
        "SU-P28" => put(&mut output, "message", copy::rename_warning(label)),
        "SU-P31" => put(
            &mut output,
            "message",
            copy::capture_complete_verdict(source, target),
        ),
        other => panic!("unhandled relation copy vector {other}"),
    }
    output
}

#[test]
fn copy_vectors_exercise_all_shared_presentations() {
    let data = vectors();
    for vector in data["copy"].as_array().expect("copy vectors") {
        let id = string(vector, "id");
        let actual = copy_outputs(vector);
        let expected = vector["expected"]
            .as_object()
            .expect("copy expected fields")
            .clone();
        assert_eq!(actual, expected, "{id}");
    }

    let unicode = copy::task_text(
        "é界🙂012345678901234567890123456789012345678901234567",
    );
    assert_eq!(unicode.chars().count(), 48);
    assert!(unicode.ends_with('…'));
    assert_eq!(copy::task_text(&"界".repeat(48)), "界".repeat(48));

    let waits = vec![
        "Target one".to_string(),
        "Target two".to_string(),
        "Target three".to_string(),
    ];
    assert_eq!(
        copy::fold_line(FoldCopy {
            source: Some("Source"),
            waits_for: &waits,
            already_started: 0,
            blocked: 0,
            hidden: 0,
        })
        .as_deref(),
        Some("not offered · “Source” waits for “Target one” +2 more waiting")
    );
    let already_planned = HeldReason::AlreadyPlanned;
    assert_eq!(
        copy::held_notice(HeldNoticeCopy {
            source: "Source",
            label: "starts after",
            target: "Target",
            reason: &already_planned,
        }),
        None
    );
    assert_eq!(
        copy::fold_line(FoldCopy {
            source: None,
            waits_for: &[],
            already_started: 0,
            blocked: 0,
            hidden: 0,
        }),
        None
    );
}
