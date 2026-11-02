//! Classifier-owned relation-successor vectors and regressions.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde_json::Value;

use super::super::relation_successors::classify::{
    classify_relation_successors, CountingReader, RelationCheck,
    RelationClassifyInput, RelationClassifyResult, SourceRecovery,
    TargetNoteRead, TargetNoteReader,
};
use super::super::relation_successors::{
    RelationEvent, RelationEventKind, RelationHeldReason, RelationSlotAnchor,
    RelationTarget,
};
use super::test_settings;
use crate::native::config::relations::CancelLabel;
use crate::native::task_status_hooks::parse_tasks;

fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/fixtures/relation_successor/vectors.json"
    ))
    .expect("typed relation-successor vectors parse")
}

const INTEGRATION_CASES: &[(&str, &str)] = &[
    ("SA14", "DEDUP into hand_off is owned by integrate"),
    ("SA15", "project recovery writes are owned by integrate"),
    (
        "SA18",
        "mixed group order with other offers is owned by integrate",
    ),
    ("SA20", "post-event edge replay is owned by integrate"),
    ("SA21", "later-item invalidation is owned by integrate"),
];

const RULE_OBJECT_KEYS: &[&str] = &["id", "name", "input", "cases", "expected"];
const INPUT_KEYS: &[&str] = &[
    "source",
    "target",
    "events",
    "config",
    "action",
    "selection",
    "staged_edges",
    "edge_added_after_event",
    "other_offers",
    "later_action",
];
const SOURCE_KEYS: &[&str] = &[
    "status",
    "tags",
    "baseline_live",
    "other_successor_targets",
    "successor_targets",
    "open_prerequisites",
    "scheduled",
    "project_blocked",
    "depends_on",
    "path",
    "id_field",
    "project_resident",
    "project_recovered",
];
const TARGET_KEYS: &[&str] = &[
    "baseline_live",
    "post_live",
    "status_before",
    "status_after",
    "id_field",
];
const EVENT_KEYS: &[&str] = &["target", "event", "anchor"];
const EXPECTED_KEYS: &[&str] = &[
    "groups",
    "held",
    "relation_check",
    "scan",
    "warning",
    "group_order",
    "reason",
    "waits_on",
    "waits_for",
    "scheduled",
    "project",
];
const GROUP_KEYS: &[&str] = &["group", "label", "event", "after", "rows"];
const ROW_KEYS: &[&str] = &["key", "lane", "suffix", "reason", "recovered"];
const HELD_KEYS: &[&str] = &[
    "key",
    "reason",
    "waits_on",
    "waits_for",
    "scheduled",
    "project",
    "display",
];

fn integration_reason(id: &str) -> Option<&'static str> {
    INTEGRATION_CASES
        .iter()
        .find(|(case, _)| *case == id)
        .map(|(_, reason)| *reason)
}

#[test]
fn classifier_rule_vectors_call_production_code() {
    let data = vectors();
    let mut seen = BTreeSet::new();
    for vector in data["rule"].as_array().expect("rule vectors") {
        let id = vector["id"].as_str().expect("rule id");
        seen.insert(id.to_string());
        assert_known_keys(id, vector, RULE_OBJECT_KEYS);
        if let Some(_reason) = integration_reason(id) {
            continue;
        }
        if let Some(cases) = vector.get("cases").and_then(Value::as_array) {
            for (index, case) in cases.iter().enumerate() {
                let expected = case
                    .get("expected")
                    .or_else(|| vector.get("expected"))
                    .unwrap_or(&Value::Null);
                run_rule_case(
                    &format!("{id}[{index}]"),
                    &data["base"],
                    vector.get("input").unwrap_or(&Value::Null),
                    case,
                    expected,
                );
            }
        } else {
            run_rule_case(
                id,
                &data["base"],
                vector.get("input").unwrap_or(&Value::Null),
                &Value::Null,
                vector.get("expected").unwrap_or(&Value::Null),
            );
        }
    }
    let expected_ids = data["rule"]
        .as_array()
        .expect("rule")
        .iter()
        .map(|vector| vector["id"].as_str().expect("id").to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(seen, expected_ids, "every rule vector must be visited");
}

#[test]
fn rule_vector_fields_cannot_be_silently_ignored() {
    let data = vectors();
    for vector in data["rule"].as_array().expect("rule vectors") {
        let id = vector["id"].as_str().expect("id");
        walk_keys(id, vector);
    }
}

fn walk_keys(id: &str, value: &Value) {
    assert_known_keys(id, value, RULE_OBJECT_KEYS);
    if let Some(input) = value.get("input") {
        assert_known_keys(id, input, INPUT_KEYS);
        if let Some(source) = input.get("source") {
            assert_known_keys(id, source, SOURCE_KEYS);
        }
        if let Some(target) = input.get("target") {
            assert_known_keys(id, target, TARGET_KEYS);
        }
        if let Some(events) = input.get("events").and_then(Value::as_array) {
            for event in events {
                assert_known_keys(id, event, EVENT_KEYS);
            }
        }
    }
    if let Some(expected) = value.get("expected") {
        assert_known_keys(id, expected, EXPECTED_KEYS);
        if let Some(groups) = expected.get("groups").and_then(Value::as_array) {
            for group in groups {
                assert_known_keys(id, group, GROUP_KEYS);
                if let Some(rows) = group.get("rows").and_then(Value::as_array)
                {
                    for row in rows {
                        assert_known_keys(id, row, ROW_KEYS);
                    }
                }
            }
        }
        if let Some(held) = expected.get("held").and_then(Value::as_array) {
            for row in held {
                assert_known_keys(id, row, HELD_KEYS);
            }
        }
    }
    if let Some(cases) = value.get("cases").and_then(Value::as_array) {
        for case in cases {
            assert_known_keys(
                id,
                case,
                &["source", "expected", "config", "later_action"],
            );
            if let Some(source) = case.get("source") {
                assert_known_keys(id, source, SOURCE_KEYS);
            }
            if let Some(expected) = case.get("expected") {
                assert_known_keys(id, expected, EXPECTED_KEYS);
            }
        }
    }
}

fn assert_known_keys(id: &str, value: &Value, allowed: &[&str]) {
    let Some(object) = value.as_object() else {
        return;
    };
    for key in object.keys() {
        assert!(
            allowed.contains(&key.as_str()),
            "{id}: unknown field {key:?} is not covered by the classifier adapter"
        );
    }
}

fn run_rule_case(
    id: &str,
    base: &Value,
    input: &Value,
    case: &Value,
    expected: &Value,
) {
    let world = World::materialize(base, input, case);
    let result = world.classify();
    assert_result(id, &result, expected, &world);
}

struct World {
    notes: BTreeMap<PathBuf, String>,
    day_before: String,
    day_after: String,
    day_path: PathBuf,
    events: Vec<RelationEvent>,
    labels: Vec<CancelLabel>,
    link_unblocked: bool,
    snapshot_complete: bool,
    dependency_snapshot_complete: bool,
    previous_daily: Option<PathBuf>,
    today: NaiveDate,
    recovered: BTreeMap<String, SourceRecovery>,
    expect_scan: Option<bool>,
}

impl World {
    fn materialize(base: &Value, input: &Value, case: &Value) -> Self {
        let mut notes = BTreeMap::new();
        for note in base["notes"].as_array().expect("base notes") {
            notes.insert(
                PathBuf::from(note["path"].as_str().expect("path")),
                note["body"].as_str().expect("body").to_string(),
            );
        }
        let mut day_before = base["day_before"].as_str().unwrap().to_string();
        let mut day_after = base["day_after"].as_str().unwrap().to_string();
        let today = NaiveDate::parse_from_str(
            base["config"]["today"].as_str().unwrap_or("2026-11-02"),
            "%Y-%m-%d",
        )
        .expect("today");
        let day_path = PathBuf::from(format!(
            "{}/{}{}.md",
            today.format("%Y"),
            today.format("%Y"),
            today.format("%m%d")
        ));
        let mut labels = labels_from(
            base["config"]["successor_labels"]
                .as_array()
                .expect("labels"),
        );
        let mut link_unblocked =
            base["config"]["link_unblocked"].as_bool().unwrap_or(true);
        let mut previous_daily = None;
        let mut recovered = BTreeMap::new();
        let mut expect_scan = None;

        let source_overlay =
            merge_objects(input.get("source"), case.get("source"));
        let target_overlay =
            merge_objects(input.get("target"), case.get("target"));
        let config_overlay =
            merge_objects(input.get("config"), case.get("config"));

        apply_source(
            &mut notes,
            &mut day_before,
            &mut previous_daily,
            &mut recovered,
            &source_overlay,
        );
        apply_target(
            &mut notes,
            &mut day_before,
            &mut day_after,
            &target_overlay,
        );
        apply_config(&mut labels, &mut link_unblocked, &config_overlay);
        if let Some(scan) = case
            .pointer("/expected/scan")
            .or_else(|| input.pointer("/expected/scan"))
            .and_then(Value::as_bool)
        {
            expect_scan = Some(scan);
        }

        let events_value = case
            .get("events")
            .or_else(|| input.get("events"))
            .unwrap_or(&base["events"]);
        let events = events_from(events_value, &notes);

        if let Some(edges) = input.get("staged_edges").and_then(Value::as_array)
        {
            for edge in edges {
                ensure_edge(&mut notes, edge);
            }
        }

        Self {
            notes,
            day_before,
            day_after,
            day_path,
            events,
            labels,
            link_unblocked,
            snapshot_complete: true,
            dependency_snapshot_complete: true,
            previous_daily,
            today,
            recovered,
            expect_scan,
        }
    }

    fn classify(&self) -> RelationClassifyResult {
        let known = self.notes.keys().cloned().collect::<Vec<_>>();
        let archives = known
            .iter()
            .filter(|path| {
                path.components().any(|part| part.as_os_str() == "done")
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut reader = self.notes.clone();
        let input = RelationClassifyInput {
            labels: &self.labels,
            events: &self.events,
            source_texts: &self.notes,
            known_paths: &known,
            archive_paths: &archives,
            day_before: Some(&self.day_before),
            day_after: Some(&self.day_after),
            day_path: &self.day_path,
            previous_daily: self.previous_daily.as_deref(),
            today: self.today,
            link_unblocked: self.link_unblocked,
            snapshot_complete: self.snapshot_complete,
            dependency_snapshot_complete: self.dependency_snapshot_complete,
            recovered: &self.recovered,
            tasks_settings: &test_settings(),
        };
        classify_relation_successors(&input, &mut reader)
    }
}

fn labels_from(values: &[Value]) -> Vec<CancelLabel> {
    values
        .iter()
        .filter_map(Value::as_str)
        .map(|name| CancelLabel {
            key: crate::native::relation_labels::relation_label_key(name),
            name: name.to_string(),
        })
        .collect()
}

fn merge_objects(left: Option<&Value>, right: Option<&Value>) -> Value {
    let mut merged = serde_json::Map::new();
    if let Some(Value::Object(object)) = left {
        merged.extend(object.clone());
    }
    if let Some(Value::Object(object)) = right {
        merged.extend(object.clone());
    }
    Value::Object(merged)
}

fn apply_source(
    notes: &mut BTreeMap<PathBuf, String>,
    day_before: &mut String,
    previous_daily: &mut Option<PathBuf>,
    recovered: &mut BTreeMap<String, SourceRecovery>,
    overlay: &Value,
) {
    if overlay.as_object().is_none_or(|object| object.is_empty()) {
        return;
    }
    let source_id = "evaluate";
    let mut path = find_task_path(notes, source_id)
        .unwrap_or_else(|| PathBuf::from("agents.md"));
    if let Some(new_path) = overlay.get("path").and_then(Value::as_str) {
        let destination = PathBuf::from(new_path);
        move_task(notes, &path, source_id, &destination);
        path = destination;
        if path.starts_with("2026/") {
            *previous_daily = Some(path.clone());
        }
    }
    if let Some(status) = overlay.get("status").and_then(Value::as_str) {
        rewrite_status(notes, &path, source_id, status_symbol(status));
    }
    if let Some(tags) = overlay.get("tags").and_then(Value::as_array) {
        for tag in tags.iter().filter_map(Value::as_str) {
            insert_token(notes, &path, source_id, tag);
        }
    }
    if overlay.get("id_field").is_some_and(Value::is_null) {
        strip_id_field(notes, &path, source_id);
    }
    if let Some(scheduled) = overlay.get("scheduled").and_then(Value::as_str) {
        insert_token(
            notes,
            &path,
            source_id,
            &format!("[scheduled:: {scheduled}]"),
        );
    }
    if overlay
        .get("open_prerequisites")
        .and_then(Value::as_u64)
        .is_some_and(|count| count > 0)
    {
        add_open_prerequisite(notes, &path, source_id);
    }
    if let Some(depends) = overlay.get("depends_on").and_then(Value::as_array) {
        for target in depends.iter().filter_map(Value::as_str) {
            add_depends_on(notes, &path, source_id, target);
        }
    }
    let mut extra_targets = Vec::new();
    if let Some(targets) =
        overlay.get("successor_targets").and_then(Value::as_array)
    {
        extra_targets = targets
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        set_successor_targets(notes, &path, source_id, &extra_targets);
    }
    if let Some(targets) = overlay
        .get("other_successor_targets")
        .and_then(Value::as_array)
    {
        for target in targets.iter().filter_map(Value::as_str) {
            extra_targets.push(target.to_string());
            add_successor_target(notes, &path, source_id, target);
        }
    }
    for target in &extra_targets {
        ensure_target_task(notes, target);
    }
    if overlay.get("baseline_live").and_then(Value::as_bool) == Some(true)
        && !day_before.contains("#^evaluate")
    {
        *day_before =
            day_before.trim_end().to_string() + "\n  - [[agents#^evaluate]]\n";
    }
    if overlay.get("project_blocked").and_then(Value::as_bool) == Some(true) {
        insert_token(notes, &path, source_id, "#prj_is_blocked");
        recovered.insert(
            format!("{}#^{}", path_key(&path), source_id),
            SourceRecovery {
                project_path: Some(PathBuf::from("agents-project.md")),
                recovered_project: overlay
                    .get("project_recovered")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                ..SourceRecovery::default()
            },
        );
        notes
            .entry(PathBuf::from("agents-project.md"))
            .or_insert_with(|| {
                "- [ ] #task Project [id:: prj] ^prj\n".to_string()
            });
    }
    if overlay.get("project_recovered").and_then(Value::as_bool) == Some(true)
        && !recovered.contains_key(&format!(
            "{}#^{}",
            path_key(&path),
            source_id
        ))
    {
        recovered.insert(
            format!("{}#^{}", path_key(&path), source_id),
            SourceRecovery {
                recovered_project: true,
                project_path: overlay
                    .get("project_resident")
                    .and_then(Value::as_str)
                    .map(PathBuf::from),
                ..SourceRecovery::default()
            },
        );
    }
}

fn apply_target(
    notes: &mut BTreeMap<PathBuf, String>,
    day_before: &mut String,
    day_after: &mut String,
    overlay: &Value,
) {
    if overlay.as_object().is_none_or(|object| object.is_empty()) {
        return;
    }
    let path = PathBuf::from("agents.md");
    if let Some(status) = overlay
        .get("status_after")
        .or_else(|| overlay.get("status_before"))
        .and_then(Value::as_str)
    {
        rewrite_status(notes, &path, "implement", status_symbol(status));
    }
    if overlay.get("id_field").is_some_and(Value::is_null) {
        strip_id_field(notes, &path, "implement");
    }
    if overlay.get("post_live").and_then(Value::as_bool) == Some(true) {
        *day_after = day_before.clone();
    }
    if overlay.get("baseline_live").and_then(Value::as_bool) == Some(false) {
        *day_before = day_before.replace("  - [[agents#^implement]]\n", "");
    }
}

fn apply_config(
    labels: &mut Vec<CancelLabel>,
    link_unblocked: &mut bool,
    overlay: &Value,
) {
    if let Some(list) =
        overlay.get("successor_labels").and_then(Value::as_array)
    {
        *labels = labels_from(list);
    }
    if let Some(enabled) =
        overlay.get("plan.link_unblocked").and_then(Value::as_bool)
    {
        *link_unblocked = enabled;
    }
}

fn events_from(
    value: &Value,
    notes: &BTreeMap<PathBuf, String>,
) -> Vec<RelationEvent> {
    let Some(events) = value.as_array() else {
        return Vec::new();
    };
    events
        .iter()
        .map(|event| {
            let (path, block_id) =
                parse_ref(event["target"].as_str().expect("event target"));
            let text = task_text(notes, &path, &block_id)
                .unwrap_or_else(|| block_id.clone());
            let kind = match event["event"].as_str() {
                Some("done") => RelationEventKind::Done,
                _ => RelationEventKind::Parked,
            };
            let slot = match event.get("anchor").and_then(Value::as_str) {
                Some("slot") => Some(RelationSlotAnchor {
                    note_path: path.clone(),
                    line: 0,
                }),
                _ => None,
            };
            RelationEvent {
                target: RelationTarget {
                    note_path: path,
                    block_id,
                    text,
                },
                kind,
                slot,
            }
        })
        .collect()
}

fn ensure_edge(notes: &mut BTreeMap<PathBuf, String>, edge: &Value) {
    let (path, id) = parse_ref(edge["source"].as_str().expect("source"));
    let target = edge["target"].as_str().expect("target");
    add_successor_target(notes, &path, &id, target);
}

fn assert_result(
    id: &str,
    result: &RelationClassifyResult,
    expected: &Value,
    world: &World,
) {
    if expected.is_null() {
        return;
    }
    if let Some(check) = expected.get("relation_check") {
        match check {
            Value::Null => {
                assert_eq!(result.check, None, "{id} relation_check")
            }
            Value::String(value) if value == "checked" => {
                assert_eq!(
                    result.check,
                    Some(RelationCheck::Checked),
                    "{id} relation_check"
                );
            }
            Value::String(value) if value == "unavailable" => {
                assert_eq!(
                    result.check,
                    Some(RelationCheck::Unavailable),
                    "{id} relation_check"
                );
            }
            other => panic!("{id}: unexpected relation_check {other}"),
        }
    }
    if expected.get("scan").and_then(Value::as_bool) == Some(false)
        || world.expect_scan == Some(false)
    {
        assert_eq!(result.target_reads, 0, "{id} must not read targets");
        assert_eq!(result.check, None, "{id} omits relation_check when gated");
    }
    if let Some(reason) = expected.get("reason").and_then(Value::as_str) {
        assert!(
            result.groups.is_empty(),
            "{id} held-only case has no groups"
        );
        assert_eq!(result.held.len(), 1, "{id} held row: {:?}", result.held);
        assert_held_reason(id, &result.held[0].reason, expected, reason);
        return;
    }
    if let Some(groups) = expected.get("groups").and_then(Value::as_array) {
        assert_eq!(
            result.groups.len(),
            groups.len(),
            "{id} groups {:?}",
            result
                .groups
                .iter()
                .map(|group| group.title.clone())
                .collect::<Vec<_>>()
        );
        for (actual, expected_group) in result.groups.iter().zip(groups) {
            if let Some(group) =
                expected_group.get("group").and_then(Value::as_str)
            {
                assert_eq!(group, "relation", "{id} group");
            }
            if let Some(label) =
                expected_group.get("label").and_then(Value::as_str)
            {
                assert_eq!(actual.label, label, "{id} label");
            }
            if let Some(event) =
                expected_group.get("event").and_then(Value::as_str)
            {
                let actual_event = match actual.event {
                    RelationEventKind::Parked => "parked",
                    RelationEventKind::Done => "done",
                };
                assert_eq!(actual_event, event, "{id} event");
            }
            if let Some(after) =
                expected_group.get("after").and_then(Value::as_str)
            {
                let actual_after = format!(
                    "{}#^{}",
                    path_key(&actual.target.note_path),
                    actual.target.block_id
                );
                assert_eq!(actual_after, after, "{id} after");
            }
            if let Some(rows) =
                expected_group.get("rows").and_then(Value::as_array)
            {
                assert_eq!(actual.rows.len(), rows.len(), "{id} rows");
                for (row, expected_row) in actual.rows.iter().zip(rows) {
                    if let Some(key) =
                        expected_row.get("key").and_then(Value::as_str)
                    {
                        let actual_key = format!(
                            "{}#^{}",
                            path_key(&row.source.note_path),
                            row.source.block_id
                        );
                        assert_eq!(actual_key, key, "{id} row key");
                    }
                    if let Some(lane) =
                        expected_row.get("lane").and_then(Value::as_str)
                    {
                        assert_eq!(row.lane, lane, "{id} lane");
                    }
                    if let Some(suffix) =
                        expected_row.get("suffix").and_then(Value::as_str)
                    {
                        assert_eq!(
                            row.suffix.as_deref(),
                            Some(suffix),
                            "{id} suffix"
                        );
                    }
                }
            }
        }
    }
    if let Some(held) = expected.get("held").and_then(Value::as_array) {
        assert_eq!(
            result.held.len(),
            held.len(),
            "{id} held {:?}",
            result.held
        );
        for (actual, expected_row) in result.held.iter().zip(held) {
            if let Some(key) = expected_row.get("key").and_then(Value::as_str) {
                let actual_key = format!(
                    "{}#^{}",
                    path_key(&actual.source.note_path),
                    actual.source.block_id
                );
                assert_eq!(actual_key, key, "{id} held key");
            }
            if let Some(reason) =
                expected_row.get("reason").and_then(Value::as_str)
            {
                assert_held_reason(id, &actual.reason, expected_row, reason);
            }
        }
    }
}

fn assert_held_reason(
    id: &str,
    reason: &RelationHeldReason,
    expected: &Value,
    name: &str,
) {
    match (name, reason) {
        ("already_planned", RelationHeldReason::AlreadyPlanned) => {}
        ("already_started", RelationHeldReason::AlreadyStarted) => {}
        ("hidden", RelationHeldReason::Hidden) => {}
        (
            "blocked",
            RelationHeldReason::Blocked {
                waits_on,
                scheduled,
                project,
            },
        ) => {
            if let Some(count) =
                expected.get("waits_on").and_then(Value::as_u64)
            {
                assert_eq!(waits_on.len() as u64, count, "{id} waits_on");
            }
            if let Some(value) =
                expected.get("scheduled").and_then(Value::as_str)
            {
                assert_eq!(scheduled.as_deref(), Some(value), "{id} scheduled");
            }
            if let Some(value) = expected.get("project").and_then(Value::as_str)
            {
                assert_eq!(
                    project.as_ref().map(|path| path_key(path)),
                    Some(value.to_string()),
                    "{id} project"
                );
            }
        }
        ("waits_for", RelationHeldReason::WaitsFor(targets)) => {
            let expected_targets = expected
                .get("waits_for")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>();
            let actual = targets
                .iter()
                .map(|target| {
                    format!(
                        "{}#^{}",
                        path_key(&target.note_path),
                        target.block_id
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(actual, expected_targets, "{id} waits_for");
        }
        other => panic!("{id}: held reason mismatch {other:?}"),
    }
}

fn status_symbol(status: &str) -> char {
    match status {
        "Ready" => ' ',
        "Next" => '*',
        "Pending" => '/',
        "Blocked" => '?',
        "Done" => 'x',
        "Cancelled" => '-',
        other => panic!("unknown status {other}"),
    }
}

fn parse_ref(value: &str) -> (PathBuf, String) {
    let (path, id) = value.split_once("#^").expect("path#^id");
    (PathBuf::from(path), id.to_string())
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn find_task_path(
    notes: &BTreeMap<PathBuf, String>,
    block_id: &str,
) -> Option<PathBuf> {
    let settings = test_settings();
    notes.iter().find_map(|(path, contents)| {
        parse_tasks(contents, &settings)
            .iter()
            .any(|task| task.block_id.as_deref() == Some(block_id))
            .then(|| path.clone())
    })
}

fn task_text(
    notes: &BTreeMap<PathBuf, String>,
    path: &Path,
    block_id: &str,
) -> Option<String> {
    parse_tasks(notes.get(path)?, &test_settings())
        .into_iter()
        .find(|task| task.block_id.as_deref() == Some(block_id))
        .map(|task| task.description)
}

fn rewrite_status(
    notes: &mut BTreeMap<PathBuf, String>,
    path: &Path,
    block_id: &str,
    status: char,
) {
    let contents = notes.get(path).cloned().unwrap_or_default();
    let Some(line_index) = task_line_index(&contents, block_id) else {
        return;
    };
    let mut lines = contents.lines().map(str::to_string).collect::<Vec<_>>();
    if let Some(line) = lines.get_mut(line_index)
        && let Some(start) = line.find('[')
    {
        let bytes = line.as_mut_str();
        if let Some(ch) = bytes[start + 1..].chars().next() {
            *line = format!(
                "{}{}{}",
                &line[..start + 1],
                status,
                &line[start + 1 + ch.len_utf8()..]
            );
        }
    }
    *notes.get_mut(path).expect("note") = join_lines(&lines, &contents);
}

fn insert_token(
    notes: &mut BTreeMap<PathBuf, String>,
    path: &Path,
    block_id: &str,
    token: &str,
) {
    let contents = notes.get(path).cloned().unwrap_or_default();
    let Some(line_index) = task_line_index(&contents, block_id) else {
        return;
    };
    let mut lines = contents.lines().map(str::to_string).collect::<Vec<_>>();
    if let Some(line) = lines.get_mut(line_index) {
        if line.contains(token) {
            return;
        }
        let caret = format!("^{block_id}");
        if let Some(at) = line.find(&caret) {
            *line = format!("{}{} {}", &line[..at], token, &line[at..]);
        } else {
            line.push(' ');
            line.push_str(token);
        }
    }
    *notes.get_mut(path).expect("note") = join_lines(&lines, &contents);
}

fn strip_id_field(
    notes: &mut BTreeMap<PathBuf, String>,
    path: &Path,
    block_id: &str,
) {
    let contents = notes.get(path).cloned().unwrap_or_default();
    let Some(line_index) = task_line_index(&contents, block_id) else {
        return;
    };
    let mut lines = contents.lines().map(str::to_string).collect::<Vec<_>>();
    if let Some(line) = lines.get_mut(line_index)
        && let Some(start) = line.find("[id::")
        && let Some(end) = line[start..].find(']')
    {
        let after = start + end + 1;
        let mut next = line[..start].to_string();
        next.push_str(line[after..].trim_start());
        *line = next;
    }
    *notes.get_mut(path).expect("note") = join_lines(&lines, &contents);
}

fn add_open_prerequisite(
    notes: &mut BTreeMap<PathBuf, String>,
    path: &Path,
    source_id: &str,
) {
    let contents = notes.entry(path.to_path_buf()).or_default();
    if !contents.contains("^block") {
        contents.push_str("- [ ] #task Blocker [id:: block] ^block\n");
    }
    insert_token(notes, path, source_id, "[dependsOn:: block]");
    add_depends_on(notes, path, source_id, "agents.md#^block");
}

fn add_depends_on(
    notes: &mut BTreeMap<PathBuf, String>,
    path: &Path,
    source_id: &str,
    target: &str,
) {
    let contents = notes.get(path).cloned().unwrap_or_default();
    let Some(line_index) = task_line_index(&contents, source_id) else {
        return;
    };
    let link = if let Some((note, id)) = target.split_once("#^") {
        format!("[[{}#^{id}]]", note.trim_end_matches(".md"))
    } else {
        format!("[[#^{target}]]")
    };
    let mut lines = contents.lines().map(str::to_string).collect::<Vec<_>>();
    lines.insert(line_index + 1, format!("  - ⛓️ **DEPENDS ON:** {link}"));
    *notes.get_mut(path).expect("note") = join_lines(&lines, &contents);
}

fn add_successor_target(
    notes: &mut BTreeMap<PathBuf, String>,
    path: &Path,
    source_id: &str,
    target: &str,
) {
    let (target_path, target_id) = parse_ref(target);
    let wiki = format!(
        "[[{}#^{}]]",
        target_path
            .file_stem()
            .and_then(|part| part.to_str())
            .unwrap_or("agents"),
        target_id
    );
    let contents = notes.get(path).cloned().unwrap_or_default();
    let Some(line_index) = links_line_index(&contents, source_id) else {
        let Some(task_line) = task_line_index(&contents, source_id) else {
            return;
        };
        let mut lines =
            contents.lines().map(str::to_string).collect::<Vec<_>>();
        lines.insert(
            task_line + 1,
            format!("  - 🪢 **LINKS:** starts after {wiki}"),
        );
        *notes.get_mut(path).expect("note") = join_lines(&lines, &contents);
        return;
    };
    let mut lines = contents.lines().map(str::to_string).collect::<Vec<_>>();
    if !lines[line_index].contains(&wiki) {
        lines[line_index] =
            format!("{} • starts after {wiki}", lines[line_index]);
    }
    *notes.get_mut(path).expect("note") = join_lines(&lines, &contents);
}

fn set_successor_targets(
    notes: &mut BTreeMap<PathBuf, String>,
    path: &Path,
    source_id: &str,
    targets: &[String],
) {
    let contents = notes.get(path).cloned().unwrap_or_default();
    let Some(line_index) = links_line_index(&contents, source_id) else {
        return;
    };
    let entries = targets
        .iter()
        .map(|target| {
            let (target_path, target_id) = parse_ref(target);
            format!(
                "starts after [[{}#^{}]]",
                target_path
                    .file_stem()
                    .and_then(|part| part.to_str())
                    .unwrap_or("agents"),
                target_id
            )
        })
        .collect::<Vec<_>>()
        .join(" • ");
    let mut lines = contents.lines().map(str::to_string).collect::<Vec<_>>();
    lines[line_index] = format!("  - 🪢 **LINKS:** {entries}");
    *notes.get_mut(path).expect("note") = join_lines(&lines, &contents);
}

fn ensure_target_task(notes: &mut BTreeMap<PathBuf, String>, target: &str) {
    let (path, id) = parse_ref(target);
    if path.as_os_str() == "missing.md" {
        return;
    }
    let contents = notes.entry(path.clone()).or_default();
    let already = parse_tasks(contents, &test_settings())
        .iter()
        .any(|task| task.block_id.as_deref() == Some(id.as_str()));
    if already {
        return;
    }
    let text = match id.as_str() {
        "deploy" => "Deploy release",
        "implement" => "Send implementation prompt",
        other => other,
    };
    contents.push_str(&format!("- [ ] #task {text} [id:: {id}] ^{id}\n"));
}

fn move_task(
    notes: &mut BTreeMap<PathBuf, String>,
    from: &Path,
    block_id: &str,
    to: &Path,
) {
    let contents = notes.get(from).cloned().unwrap_or_default();
    let Some(start) = task_line_index(&contents, block_id) else {
        return;
    };
    let lines = contents.lines().collect::<Vec<_>>();
    let mut end = start + 1;
    while end < lines.len() {
        if lines[end].starts_with(' ') || lines[end].starts_with('\t') {
            end += 1;
        } else {
            break;
        }
    }
    let subtree = lines[start..end].join("\n") + "\n";
    let mut remaining = lines[..start].to_vec();
    remaining.extend_from_slice(&lines[end..]);
    notes.insert(from.to_path_buf(), remaining.join("\n") + "\n");
    notes
        .entry(to.to_path_buf())
        .and_modify(|body| body.push_str(&subtree))
        .or_insert(subtree);
}

fn task_line_index(contents: &str, block_id: &str) -> Option<usize> {
    parse_tasks(contents, &test_settings())
        .into_iter()
        .find(|task| task.block_id.as_deref() == Some(block_id))
        .map(|task| task.line_index)
}

fn links_line_index(contents: &str, block_id: &str) -> Option<usize> {
    let start = task_line_index(contents, block_id)?;
    contents
        .lines()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| {
            line.contains("LINKS")
                .then_some(index)
                .filter(|_| line.starts_with(' ') || line.starts_with('\t'))
        })
}

fn join_lines(lines: &[String], original: &str) -> String {
    let mut joined = lines.join("\n");
    if original.ends_with('\n') && !joined.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

#[test]
fn gates_do_not_read_targets() {
    let data = vectors();
    let mut world =
        World::materialize(&data["base"], &Value::Null, &Value::Null);
    world.labels.clear();
    let mut reader = CountingReader::new(world.notes.clone());
    let known = world.notes.keys().cloned().collect::<Vec<_>>();
    let input = RelationClassifyInput {
        labels: &world.labels,
        events: &world.events,
        source_texts: &world.notes,
        known_paths: &known,
        archive_paths: &[],
        day_before: Some(&world.day_before),
        day_after: Some(&world.day_after),
        day_path: &world.day_path,
        previous_daily: None,
        today: world.today,
        link_unblocked: true,
        snapshot_complete: true,
        dependency_snapshot_complete: true,
        recovered: &world.recovered,
        tasks_settings: &test_settings(),
    };
    let result = classify_relation_successors(&input, &mut reader);
    assert_eq!(result.target_reads, 0, "empty labels");
    assert_eq!(reader.reads, 0, "empty labels");
    assert_eq!(result.check, None);

    world.labels = labels_from(&[Value::String("starts after".into())]);
    world.link_unblocked = false;
    let mut reader = CountingReader::new(world.notes.clone());
    let input = RelationClassifyInput {
        labels: &world.labels,
        events: &world.events,
        source_texts: &world.notes,
        known_paths: &known,
        archive_paths: &[],
        day_before: Some(&world.day_before),
        day_after: Some(&world.day_after),
        day_path: &world.day_path,
        previous_daily: None,
        today: world.today,
        link_unblocked: false,
        snapshot_complete: true,
        dependency_snapshot_complete: true,
        recovered: &world.recovered,
        tasks_settings: &test_settings(),
    };
    let result = classify_relation_successors(&input, &mut reader);
    assert_eq!(result.target_reads, 0, "link_unblocked false");
    assert_eq!(result.check, None);
}

#[test]
fn incomplete_snapshot_is_unavailable_without_reads() {
    let data = vectors();
    let world = World::materialize(&data["base"], &Value::Null, &Value::Null);
    let mut reader = CountingReader::new(world.notes.clone());
    let known = world.notes.keys().cloned().collect::<Vec<_>>();
    let input = RelationClassifyInput {
        labels: &world.labels,
        events: &world.events,
        source_texts: &world.notes,
        known_paths: &known,
        archive_paths: &[],
        day_before: Some(&world.day_before),
        day_after: Some(&world.day_after),
        day_path: &world.day_path,
        previous_daily: None,
        today: world.today,
        link_unblocked: true,
        snapshot_complete: false,
        dependency_snapshot_complete: true,
        recovered: &world.recovered,
        tasks_settings: &test_settings(),
    };
    let result = classify_relation_successors(&input, &mut reader);
    assert_eq!(result.check, Some(RelationCheck::Unavailable));
    assert!(result.groups.is_empty());
    assert_eq!(result.target_reads, 0);
    assert_eq!(reader.reads, 0);
}

#[test]
fn staged_edge_removal_drops_the_offer() {
    let data = vectors();
    let mut world =
        World::materialize(&data["base"], &Value::Null, &Value::Null);
    let path = PathBuf::from("agents.md");
    let contents = world.notes.get_mut(&path).expect("agents");
    *contents = contents
        .lines()
        .filter(|line| !line.contains("LINKS"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let result = world.classify();
    assert!(result.groups.is_empty(), "{result:?}");
    assert!(result.held.is_empty());
}

#[test]
fn missing_source_id_still_offers_with_provisional_identity() {
    let data = vectors();
    let mut world =
        World::materialize(&data["base"], &Value::Null, &Value::Null);
    let path = PathBuf::from("agents.md");
    let contents = world.notes.get_mut(&path).expect("agents");
    *contents = contents.replace(" ^evaluate", "");
    let result = world.classify();
    assert_eq!(result.groups.len(), 1, "{result:?}");
    assert!(!result.groups[0].rows[0].source.block_id.is_empty());
}

#[test]
fn next_orders_before_ready_across_notes() {
    let data = vectors();
    let mut world =
        World::materialize(&data["base"], &Value::Null, &Value::Null);
    world.notes.insert(
        PathBuf::from("later.md"),
        concat!(
            "- [*] #task Next follow-up [id:: next] ^next\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
        )
        .to_string(),
    );
    let result = world.classify();
    let keys = result.groups[0]
        .rows
        .iter()
        .map(|row| row.lane)
        .collect::<Vec<_>>();
    assert_eq!(keys, ["Next", "Ready"], "{result:?}");
}

#[test]
fn r6_treats_done_cancelled_and_parked_targets_as_released() {
    let data = vectors();
    for (status, released) in
        [('x', true), ('-', true), ('/', true), (' ', false)]
    {
        let mut world =
            World::materialize(&data["base"], &Value::Null, &Value::Null);
        add_successor_target(
            &mut world.notes,
            Path::new("agents.md"),
            "evaluate",
            "agents.md#^deploy",
        );
        ensure_target_task(&mut world.notes, "agents.md#^deploy");
        rewrite_status(
            &mut world.notes,
            Path::new("agents.md"),
            "deploy",
            status,
        );
        let result = world.classify();
        if released {
            assert_eq!(result.groups.len(), 1, "status {status:?}: {result:?}");
            assert!(result.held.is_empty(), "status {status:?}");
        } else {
            assert!(result.groups.is_empty(), "status {status:?}: {result:?}");
            assert_eq!(result.held.len(), 1, "status {status:?}");
        }
    }
}

#[test]
fn unread_resolved_target_does_not_count_as_released() {
    let data = vectors();
    let mut world =
        World::materialize(&data["base"], &Value::Null, &Value::Null);
    add_successor_target(
        &mut world.notes,
        Path::new("agents.md"),
        "evaluate",
        "other.md#^other",
    );
    world.notes.insert(
        PathBuf::from("other.md"),
        "- [ ] #task Other [id:: other] ^other\n".to_string(),
    );
    struct Unavailable;
    impl TargetNoteReader for Unavailable {
        fn read_target(&mut self, path: &Path) -> TargetNoteRead {
            if path == Path::new("other.md") {
                TargetNoteRead::Unavailable
            } else {
                TargetNoteRead::Missing
            }
        }
    }
    let mut source_texts = world.notes.clone();
    source_texts.remove(Path::new("other.md"));
    let known = world.notes.keys().cloned().collect::<Vec<_>>();
    let mut reader = Unavailable;
    let input = RelationClassifyInput {
        labels: &world.labels,
        events: &world.events,
        source_texts: &source_texts,
        known_paths: &known,
        archive_paths: &[],
        day_before: Some(&world.day_before),
        day_after: Some(&world.day_after),
        day_path: &world.day_path,
        previous_daily: None,
        today: world.today,
        link_unblocked: true,
        snapshot_complete: true,
        dependency_snapshot_complete: true,
        recovered: &world.recovered,
        tasks_settings: &test_settings(),
    };
    let result = classify_relation_successors(&input, &mut reader);
    assert!(result.groups.is_empty(), "{result:?}");
    assert_eq!(result.held.len(), 1, "{result:?}");
    assert!(matches!(
        result.held[0].reason,
        RelationHeldReason::WaitsFor(_)
    ));
}

#[test]
fn previous_daily_gap_is_not_today_minus_one() {
    let data = vectors();
    let mut world =
        World::materialize(&data["base"], &Value::Null, &Value::Null);
    let previous = PathBuf::from("2026/20261030.md");
    move_task(
        &mut world.notes,
        Path::new("agents.md"),
        "evaluate",
        &previous,
    );
    world.previous_daily = Some(previous);
    let result = world.classify();
    assert!(result.groups.is_empty(), "{result:?}");
    assert!(result.held.is_empty(), "{result:?}");
}

#[test]
fn r4_uses_caller_supplied_recovered_state() {
    let data = vectors();
    let mut world =
        World::materialize(&data["base"], &Value::Null, &Value::Null);
    insert_token(
        &mut world.notes,
        Path::new("agents.md"),
        "evaluate",
        "#prj_is_blocked",
    );
    world.recovered.insert(
        "agents.md#^evaluate".to_string(),
        SourceRecovery {
            recovered_project: true,
            project_path: Some(PathBuf::from("agents-project.md")),
            ..SourceRecovery::default()
        },
    );
    let result = world.classify();
    assert_eq!(result.groups.len(), 1, "{result:?}");
    assert!(result.held.is_empty());
}

#[test]
fn km31_unavailable_dependency_data_holds_blocked() {
    let data = vectors();
    let mut world =
        World::materialize(&data["base"], &Value::Null, &Value::Null);
    insert_token(
        &mut world.notes,
        Path::new("agents.md"),
        "evaluate",
        "[dependsOn:: block]",
    );
    world.dependency_snapshot_complete = false;
    let result = world.classify();
    assert!(result.groups.is_empty(), "{result:?}");
    assert!(matches!(
        result.held[0].reason,
        RelationHeldReason::Blocked { .. }
    ));
}
