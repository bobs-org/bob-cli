//! Classifier-owned relation-successor vectors and regressions, plus the
//! integrate-phase planner merge, dedup, and capture-compatibility checks.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde_json::Value;

use super::super::relation_successors::classify::{
    classify_relation_successors, CountingReader, RelationCheck,
    RelationClassifyInput, RelationClassifyResult, SourceRecovery,
    TargetNoteRead, TargetNoteReader,
};
use super::super::relation_successors::copy;
use super::super::relation_successors::{
    edge_added_after_release_warning, relation_offer_survives_final_state,
    RelationEvent, RelationEventKind, RelationHeldReason, RelationSlotAnchor,
    RelationSuccessorInput, RelationTarget,
};
use super::super::successors::{
    plan_successors, CompletedTask, OfferGroup, SuccessorInput,
    SuccessorSelection,
};
use super::test_settings;
use crate::native::config::relations::CancelLabel;
use crate::native::env::TestEnvGuard;
use crate::native::task_status_hooks::parse_tasks;
use crate::native::vault_links::NoteIndex;

fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/fixtures/relation_successor/vectors.json"
    ))
    .expect("typed relation-successor vectors parse")
}

#[derive(Clone, Copy)]
enum IntegrationOwner {
    Planner,
    EventWarning,
    FinalState,
}

const INTEGRATION_CASES: &[(&str, &str, IntegrationOwner)] = &[
    (
        "SA14",
        "DEDUP into hand_off is owned by integrate",
        IntegrationOwner::Planner,
    ),
    (
        "SA15",
        "project recovery writes are owned by integrate",
        IntegrationOwner::Planner,
    ),
    (
        "SA18",
        "mixed group order with other offers is owned by integrate",
        IntegrationOwner::Planner,
    ),
    (
        "SA20",
        "late-edge warning is owned by the event-time helper",
        IntegrationOwner::EventWarning,
    ),
    (
        "SA21",
        "final-state revalidation is owned by the pure helper",
        IntegrationOwner::FinalState,
    ),
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

fn integration_case(id: &str) -> Option<(&'static str, IntegrationOwner)> {
    INTEGRATION_CASES
        .iter()
        .find(|(case, _, _)| *case == id)
        .map(|(_, reason, owner)| (*reason, *owner))
}

#[test]
fn classifier_rule_vectors_call_production_code() {
    let data = vectors();
    let mut seen = BTreeSet::new();
    for vector in data["rule"].as_array().expect("rule vectors") {
        let id = vector["id"].as_str().expect("rule id");
        seen.insert(id.to_string());
        assert_known_keys(id, vector, RULE_OBJECT_KEYS);
        if let Some((_reason, owner)) = integration_case(id) {
            run_integration_vector(vector, owner);
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

#[derive(Clone)]
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
    } else if overlay.get("status").and_then(Value::as_str) == Some("Blocked")
        && overlay.get("depends_on").is_none()
    {
        // A fixture-level Blocked source with no more specific reason is
        // materialized as a real open prerequisite, so waits_on assertions
        // exercise resolved task data rather than placeholder rows.
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
    let dependency_id =
        target.split_once("#^").map(|(_, id)| id).unwrap_or(target);
    insert_token(
        notes,
        path,
        source_id,
        &format!("[dependsOn:: {dependency_id}]"),
    );
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

// ---------------------------------------------------------------------------
// Integrate phase: merge into plan_successors after recovery.
// ---------------------------------------------------------------------------

fn planner_index(notes: &BTreeMap<PathBuf, String>) -> NoteIndex {
    let mut paths: Vec<PathBuf> = notes.keys().cloned().collect();
    paths.sort();
    NoteIndex::from_paths(paths)
}

fn completed_from_events(
    events: &[RelationEvent],
    notes: &BTreeMap<PathBuf, String>,
) -> Vec<CompletedTask> {
    let settings = test_settings();
    events
        .iter()
        .filter(|event| matches!(event.kind, RelationEventKind::Done))
        .map(|event| {
            let contents = notes
                .get(&event.target.note_path)
                .cloned()
                .unwrap_or_default();
            let task_id = parse_tasks(&contents, &settings)
                .into_iter()
                .find(|task| {
                    task.block_id.as_deref()
                        == Some(event.target.block_id.as_str())
                })
                .and_then(|task| task.task_id)
                .unwrap_or_else(|| event.target.block_id.clone());
            CompletedTask {
                relative_path: event.target.note_path.clone(),
                block_id: event.target.block_id.clone(),
                task_id,
                text: event.target.text.clone(),
                is_root: true,
            }
        })
        .collect()
}

fn parked_from_events(events: &[RelationEvent]) -> Vec<RelationEvent> {
    events
        .iter()
        .filter(|event| matches!(event.kind, RelationEventKind::Parked))
        .cloned()
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn run_planner(
    world: &World,
    completed: Vec<CompletedTask>,
    parked: Vec<RelationEvent>,
    selection: SuccessorSelection,
    closing: bool,
    link_unblocked: bool,
    snapshot_complete: bool,
    relation_snapshot_complete: bool,
    previous_daily: Option<PathBuf>,
) -> super::super::successors::SuccessorPlan {
    run_planner_with_options(
        world,
        completed,
        parked,
        selection,
        closing,
        link_unblocked,
        snapshot_complete,
        relation_snapshot_complete,
        previous_daily,
        PlannerOptions::default(),
    )
}

#[derive(Default)]
struct PlannerOptions<'a> {
    keep_moving: bool,
    bob_dir: Option<&'a Path>,
    omit_snapshot_paths: BTreeSet<PathBuf>,
    closing_context: Option<super::super::successors::ClosingContext>,
}

#[allow(clippy::too_many_arguments)]
fn run_planner_with_options<'a>(
    world: &World,
    completed: Vec<CompletedTask>,
    parked: Vec<RelationEvent>,
    selection: SuccessorSelection,
    closing: bool,
    link_unblocked: bool,
    snapshot_complete: bool,
    relation_snapshot_complete: bool,
    previous_daily: Option<PathBuf>,
    options: PlannerOptions<'a>,
) -> super::super::successors::SuccessorPlan {
    let settings = test_settings();
    let snapshot: Vec<(PathBuf, String)> = world
        .notes
        .iter()
        .filter(|(path, _)| !options.omit_snapshot_paths.contains(*path))
        .map(|(path, contents)| (path.clone(), contents.clone()))
        .collect();
    let index = planner_index(&world.notes);
    let closing_ctx = if closing {
        Some(options.closing_context.clone().unwrap_or_else(|| {
            super::super::successors::ClosingContext {
                continuation_name: String::new(),
                closed_line: 0,
                continuation: None,
            }
        }))
    } else {
        None
    };
    // Leak the index and settings for the planner's borrowed lifetime in
    // tests; the process exits after the suite so this is contained.
    let index_static: &'static NoteIndex = Box::leak(Box::new(index));
    let settings_static: &'static crate::native::task_status_hooks::TasksSettings =
        Box::leak(Box::new(settings));
    let note_settings_static: &'static crate::native::note_tasks::NoteTaskSettings =
        Box::leak(Box::new(test_settings()));
    let _ = &settings_static;
    let relation = RelationSuccessorInput {
        labels: world.labels.clone(),
        parked,
        relation_snapshot_complete,
        dependency_snapshot_complete: false,
        previous_daily,
        unavailable_targets: BTreeSet::new(),
    };
    plan_successors(SuccessorInput {
        bob_dir: options.bob_dir,
        completed,
        snapshot: snapshot.clone(),
        note_texts: snapshot,
        pre_day: Some(world.day_before.clone()),
        post_day: Some(world.day_after.clone()),
        closing: closing_ctx,
        day_relative: world.day_path.clone(),
        today: world.today,
        tasks_settings: settings_static,
        note_settings: note_settings_static,
        link_unblocked,
        keep_moving: options.keep_moving,
        snapshot_complete,
        selection,
        index: index_static,
        relation: Some(relation),
    })
}

fn planner_world_from_notes(
    notes: BTreeMap<PathBuf, String>,
    day_before: &str,
    day_after: &str,
) -> World {
    World {
        notes,
        day_before: day_before.to_string(),
        day_after: day_after.to_string(),
        day_path: PathBuf::from("2026/20261102.md"),
        events: Vec::new(),
        labels: labels_from(&[Value::String("starts after".to_string())]),
        link_unblocked: true,
        snapshot_complete: true,
        dependency_snapshot_complete: true,
        previous_daily: None,
        today: NaiveDate::from_ymd_opt(2026, 11, 2).expect("today"),
        recovered: BTreeMap::new(),
        expect_scan: None,
    }
}

fn completed_task(path: &str, id: &str, text: &str) -> CompletedTask {
    CompletedTask {
        relative_path: PathBuf::from(path),
        block_id: id.to_string(),
        task_id: id.to_string(),
        text: text.to_string(),
        is_root: true,
    }
}

fn recovery_world() -> (World, CompletedTask) {
    let notes = BTreeMap::from([
        (
            PathBuf::from("source.md"),
            concat!(
                "- [ ] #task Source [dependsOn:: closed] [id:: source] ^source\n",
                "  - 🪢 **LINKS:** starts after [[area/target#^event]]\n",
            )
            .to_string(),
        ),
        (
            PathBuf::from("prereq.md"),
            "- [x] #task Closed prerequisite [id:: closed] ^closed\n"
                .to_string(),
        ),
        (
            PathBuf::from("target.md"),
            "- [x] #task Event target [id:: event] ^event\n".to_string(),
        ),
    ]);
    let before = concat!(
        "## Pomodoros\n",
        "- [ ] () — EVENT\n",
        "  - [[target#^event]]\n",
        "- [ ] () — OTHER\n",
    );
    let after = concat!(
        "## Pomodoros\n",
        "- [ ] () — EVENT\n",
        "  - ~~[[target#^event]]~~\n",
        "- [ ] () — OTHER\n",
    );
    (
        planner_world_from_notes(notes, before, after),
        completed_task("target.md", "event", "Event target"),
    )
}

fn run_integration_vector(vector: &Value, owner: IntegrationOwner) {
    let id = vector["id"].as_str().expect("integration vector id");
    match owner {
        IntegrationOwner::Planner => {
            let (world, completed, parked) = planner_world(id);
            let selection = match vector
                .pointer("/input/selection")
                .and_then(Value::as_str)
            {
                Some("all") => SuccessorSelection::All,
                Some("none") | None => SuccessorSelection::None,
                Some(other) => panic!("{id}: unknown selection {other}"),
            };
            let plan = run_planner(
                &world, completed, parked, selection, false, true, true, true,
                None,
            );
            assert_planner_result(id, &plan, &vector["expected"]);
        }
        IntegrationOwner::EventWarning => {
            let (mut world, _, _) = planner_world(id);
            let expected = &vector["expected"];
            let source_path = PathBuf::from("agents.md");
            if let Some(source) = world.notes.get_mut(&source_path) {
                *source = source
                    .lines()
                    .filter(|line| !line.contains("🪢 **LINKS:**"))
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n";
            }
            let no_edge = world.classify();
            assert_result(id, &no_edge, expected, &world);
            let source = world
                .notes
                .get(&source_path)
                .and_then(|text| {
                    parse_tasks(text, &test_settings()).into_iter().next()
                })
                .expect("source task");
            let target = world.events.first().expect("released event");
            let label = world.labels.first().expect("configured label");
            let warning = edge_added_after_release_warning(
                &label.name,
                &source.description,
                &target.target.text,
            );
            assert_eq!(
                Some(warning.code),
                expected.get("warning").and_then(Value::as_str),
                "{id} warning code"
            );
        }
        IntegrationOwner::FinalState => {
            let cases = vector["cases"].as_array().expect("SA21 cases");
            for case in cases {
                let action =
                    case["later_action"].as_str().expect("later action");
                let (world, _, _) = planner_world(id);
                let source_status = world
                    .notes
                    .get(&PathBuf::from("agents.md"))
                    .and_then(|text| {
                        parse_tasks(text, &test_settings()).into_iter().next()
                    })
                    .map(|task| task.status)
                    .unwrap_or(' ');
                let target_status = world
                    .events
                    .first()
                    .map(|event| {
                        if event.kind == RelationEventKind::Parked {
                            '/'
                        } else {
                            'x'
                        }
                    })
                    .unwrap_or('/');
                let survives = relation_offer_survives_final_state(
                    source_status,
                    action == "link source",
                    false,
                    false,
                    RelationEventKind::Parked,
                    target_status,
                    action == "relink target",
                );
                assert_eq!(
                    survives,
                    !case["expected"]["groups"]
                        .as_array()
                        .expect("expected groups")
                        .is_empty(),
                    "{id} {action} group survival"
                );
                assert!(case["expected"]["held"]
                    .as_array()
                    .expect("expected held")
                    .is_empty());
            }
        }
    }
}

fn assert_planner_result(
    id: &str,
    plan: &super::super::successors::SuccessorPlan,
    expected: &Value,
) {
    let mut rows = plan
        .unblocked
        .iter()
        .chain(plan.relation_successors.iter())
        .chain(plan.keep_moving.iter())
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| {
        row.offer
            .as_ref()
            .map(|offer| offer.n)
            .unwrap_or(usize::MAX)
    });
    if let Some(group_order) =
        expected.get("group_order").and_then(Value::as_array)
    {
        let actual = rows
            .iter()
            .filter_map(|row| row.offer.as_ref())
            .map(|offer| offer.group.as_str())
            .collect::<Vec<_>>();
        let expected = group_order
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "{id} offer group order");
    }
    if let Some(groups) = expected.get("groups").and_then(Value::as_array) {
        let expected_offer_count = groups
            .iter()
            .map(|group| group["rows"].as_array().expect("group rows").len())
            .sum::<usize>();
        let actual_offer_count =
            rows.iter().filter(|row| row.offer.is_some()).count();
        assert_eq!(
            actual_offer_count, expected_offer_count,
            "{id} offered rows"
        );
        for group in groups {
            let group_name = group
                .get("group")
                .and_then(Value::as_str)
                .expect("expected group");
            let actual = rows
                .iter()
                .filter(|row| {
                    row.offer
                        .as_ref()
                        .is_some_and(|offer| offer.group.as_str() == group_name)
                })
                .copied()
                .collect::<Vec<_>>();
            let expected_rows = group["rows"].as_array().expect("group rows");
            assert_eq!(
                actual.len(),
                expected_rows.len(),
                "{id} {group_name} rows"
            );
            if let Some(label) = group.get("label").and_then(Value::as_str) {
                assert!(
                    actual.iter().any(|row| {
                        row.relation
                            .as_ref()
                            .is_some_and(|relation| relation.label == label)
                    }),
                    "{id} relation label"
                );
            }
            if let Some(event) = group.get("event").and_then(Value::as_str) {
                assert!(
                    actual.iter().any(|row| {
                        row.relation.as_ref().is_some_and(|relation| {
                            match relation.event {
                                RelationEventKind::Parked => event == "parked",
                                RelationEventKind::Done => event == "done",
                            }
                        })
                    }),
                    "{id} relation event"
                );
            }
            if let Some(after) = group.get("after").and_then(Value::as_str) {
                assert!(
                    actual.iter().any(|row| {
                        row.relation.as_ref().is_some_and(|relation| {
                            relation.after.first().is_some_and(|target| {
                                format!(
                                    "{}#^{}",
                                    path_key(&target.note_path),
                                    target.block_id
                                ) == after
                            })
                        })
                    }),
                    "{id} relation target"
                );
            }
            for (row, expected_row) in actual.iter().zip(expected_rows) {
                if let Some(key) =
                    expected_row.get("key").and_then(Value::as_str)
                {
                    assert_eq!(
                        format!(
                            "{}#^{}",
                            path_key(&row.note_path),
                            row.block_id
                        ),
                        key,
                        "{id} row key"
                    );
                }
                if let Some(lane) =
                    expected_row.get("lane").and_then(Value::as_str)
                {
                    let actual_lane = if row.previous_status_symbol == '*'
                        || row.status_symbol == '*'
                    {
                        "Next"
                    } else if row.previous_status_symbol == '?'
                        || row.status_symbol == ' '
                    {
                        "Ready"
                    } else {
                        "Other"
                    };
                    assert_eq!(actual_lane, lane, "{id} row lane");
                }
                if let Some(reason) =
                    expected_row.get("reason").and_then(Value::as_str)
                {
                    let relation =
                        row.relation.as_ref().expect("relation reason");
                    let target =
                        relation.after.first().expect("relation target");
                    let actual = copy::secondary_reason(copy::GroupCopy {
                        label: &relation.label,
                        target: &target.text,
                    });
                    assert_eq!(actual, reason, "{id} secondary reason");
                }
                if expected_row.get("recovered").and_then(Value::as_bool)
                    == Some(true)
                {
                    assert!(
                        plan.changed_files.values().any(|text| text
                            .contains("- [ ] #task Prepare evaluation prompt")
                            && !text.contains("#prj_is_blocked")),
                        "{id} project recovery write: {:?}",
                        plan.changed_files
                    );
                }
            }
        }
    }
    if let Some(held) = expected.get("held").and_then(Value::as_array) {
        assert_eq!(plan.relation_held.len(), held.len(), "{id} held rows");
    }
}

fn planner_world(id: &str) -> (World, Vec<CompletedTask>, Vec<RelationEvent>) {
    let data = vectors();
    let vector = data["rule"]
        .as_array()
        .expect("rule")
        .iter()
        .find(|vector| vector["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("{id} vector exists"));
    let input = vector.get("input").unwrap_or(&Value::Null);
    // SA21 has cases; use the first case for the generic helper.
    let (input_overlay, case) =
        if let Some(cases) = vector.get("cases").and_then(Value::as_array) {
            (input.clone(), cases[0].clone())
        } else {
            (input.clone(), Value::Null)
        };
    let mut world = World::materialize(&data["base"], &input_overlay, &case);
    if let Some(other_offers) =
        input_overlay.get("other_offers").and_then(Value::as_array)
    {
        for offer in other_offers {
            if offer.get("group").and_then(Value::as_str) != Some("hand_off") {
                continue;
            }
            let key =
                offer.get("key").and_then(Value::as_str).expect("offer key");
            let (path, block_id) = parse_ref(key);
            let task_id = block_id.clone();
            let target = world
                .events
                .iter()
                .find(|event| event.kind == RelationEventKind::Done)
                .map(|event| {
                    format!(
                        "{}#^{}",
                        path_key(&event.target.note_path),
                        event.target.block_id
                    )
                })
                .expect("handoff event target");
            let note = world.notes.entry(path.clone()).or_default();
            if !note.contains(&format!("^{block_id}")) {
                note.push_str(&format!(
                    "- [?] #task Vector hand-off [dependsOn:: {}] [id:: {task_id}] ^{block_id}\n",
                    target.split_once("#^").map(|(_, id)| id).unwrap_or_default()
                ));
            }
        }
    }
    // Apply case-level config overlays that World::materialize only reads
    // from input/case source/target/config events; SA24-style gates live
    // in expected, so handle the common parked/done split here.
    let mut completed = completed_from_events(&world.events, &world.notes);
    let mut parked = parked_from_events(&world.events);
    // SA14-P and SA17 style: no explicit events in the vector means the
    // base parked event still applies; keep it.
    if world.events.is_empty() {
        completed = Vec::new();
        parked = Vec::new();
    }
    // Materialize Done events in the staged notes as the close engine
    // would. This lets the planner derive real dependency recovery.
    for event in world
        .events
        .iter()
        .filter(|event| event.kind == RelationEventKind::Done)
    {
        rewrite_status(
            &mut world.notes,
            &event.target.note_path,
            &event.target.block_id,
            'x',
        );
    }
    // A vector project recovery becomes a real ^prj dependent of a closed
    // gate task, with the relation source resident in that project note.
    // The relation event remains the fixture's parked target.
    if input
        .pointer("/source/project_recovered")
        .and_then(Value::as_bool)
        == Some(true)
    {
        let path = PathBuf::from("agents.md");
        let body = world.notes.get(&path).cloned().unwrap_or_default();
        let projected = format!(
            "---\ntype: [[project]]\n---\n- [x] #task Project gate [id:: project-close] ^project-close\n- [?] #task Project [dependsOn:: project-close] [id:: project] ^prj\n{body}"
        );
        world.notes.insert(path.clone(), projected);
        insert_token(&mut world.notes, &path, "evaluate", "#prj_is_blocked");
        rewrite_status(&mut world.notes, &path, "evaluate", '?');
        completed.push(CompletedTask {
            relative_path: path,
            block_id: "project-close".to_string(),
            task_id: "project-close".to_string(),
            text: "Project gate".to_string(),
            is_root: true,
        });
    }
    // Ensure completed task ids are present for legacy hand-off detection.
    for task in &mut completed {
        if task.task_id.is_empty() {
            task.task_id = task.block_id.clone();
        }
    }
    let _ = vector;
    (world, completed, parked)
}

#[test]
fn integrate_sa1_park_offers_at_closing_anchor() {
    let data = vectors();
    let world = World::materialize(&data["base"], &Value::Null, &Value::Null);
    let completed = Vec::new();
    let parked = parked_from_events(&world.events);
    let plan = run_planner(
        &world,
        completed,
        parked,
        SuccessorSelection::All,
        true,
        true,
        true,
        true,
        None,
    );
    assert_eq!(plan.relation_check, Some(RelationCheck::Checked));
    assert_eq!(plan.relation_successors.len(), 1);
    let row = &plan.relation_successors[0];
    assert_eq!(row.trigger, "relation");
    assert_eq!(
        row.offer.as_ref().expect("offer").group,
        OfferGroup::Relation
    );
    assert!(plan.unblocked.is_empty());
    assert!(plan.relation_held.is_empty());
    assert_eq!(plan.relation_groups.len(), 1);
    assert_eq!(plan.relation_groups[0].label, "starts after");
}

#[test]
fn integrate_sa14_direct_dependent_keeps_hand_off_with_relation_reason() {
    let (mut world, completed, parked) = planner_world("SA14");
    // Legacy hand_off detection matches short [dependsOn::] ids; the
    // fixture adds a managed Depends-On line, so add the short token too
    // for the real direct-dependent path.
    insert_token(
        &mut world.notes,
        Path::new("agents.md"),
        "evaluate",
        "[dependsOn:: implement]",
    );
    // Legacy recovery sees the post-gesture snapshot: the Done target must
    // read closed there, otherwise the dependent stays blocked.
    rewrite_status(&mut world.notes, Path::new("agents.md"), "implement", 'x');
    assert!(!completed.is_empty(), "SA14 needs a Done completion");
    let plan = run_planner(
        &world,
        completed,
        parked,
        SuccessorSelection::All,
        false,
        true,
        true,
        true,
        None,
    );
    assert_eq!(plan.relation_check, Some(RelationCheck::Checked));
    assert!(
        plan.relation_successors.is_empty(),
        "hand_off wins: {:?}",
        plan.relation_successors
    );
    assert_eq!(plan.unblocked.len(), 1);
    let row = &plan.unblocked[0];
    assert_eq!(
        row.offer.as_ref().expect("offer").group,
        OfferGroup::HandOff
    );
    let info = row.relation.as_ref().expect("second reason");
    assert_eq!(info.label, "starts after");
    assert_eq!(info.key, "starts after");
    assert!(matches!(info.event, RelationEventKind::Done));
    assert_eq!(plan.relation_groups.len(), 0);
}

#[test]
fn integrate_sa15_relation_wins_but_project_recovery_remains() {
    // Real recovered-project state: proj.md ^prj recovers via done-p,
    // while its resident also has a relation edge to a parked target.
    let settings = test_settings();
    let today = NaiveDate::from_ymd_opt(2026, 11, 2).expect("today");
    let day_path = PathBuf::from("2026/20261102.md");
    let mut notes = BTreeMap::new();
    notes.insert(
        PathBuf::from("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
        )
        .to_string(),
    );
    notes.insert(
        PathBuf::from("proj.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\n---\n",
            "- [?] #task Apply [fresh:: 2026-11-01] [dependsOn:: done-p] [id:: apply] ^prj\n",
            "- [ ] #task Pitch [fresh:: 2026-11-01] [id:: pitch] ^pitch\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [ ] #task Other [fresh:: 2026-11-01] #prj_is_blocked [id:: other] ^other\n",
            "- [x] #task Done P [id:: done-p] ^done-p\n",
        )
        .to_string(),
    );
    notes.insert(
        PathBuf::from("other.md"),
        "- [ ] #task Keep place [id:: keep] ^keep\n".to_string(),
    );
    let day_before = concat!(
        "## Pomodoros\n",
        "- [ ] (**) — BUILD\n",
        "  - [[agents#^implement]]\n",
        "- [ ] () — WRITE\n",
        "  - [[other#^keep]]\n",
    )
    .to_string();
    let day_after = concat!(
        "## Pomodoros\n",
        "- [x] (**) — BUILD\n",
        "  - ~~[[agents#^implement]]~~\n",
        "- [ ] () — BUILD\n",
        "- [ ] () — WRITE\n",
        "  - [[other#^keep]]\n",
    )
    .to_string();
    let labels = vec![CancelLabel {
        key: "starts after".to_string(),
        name: "starts after".to_string(),
    }];
    let parked = vec![RelationEvent {
        target: RelationTarget {
            note_path: PathBuf::from("agents.md"),
            block_id: "implement".to_string(),
            text: "Send implementation prompt".to_string(),
        },
        kind: RelationEventKind::Parked,
        slot: Some(RelationSlotAnchor {
            note_path: day_path.clone(),
            line: 5,
        }),
    }];
    let completed = vec![CompletedTask {
        relative_path: PathBuf::from("proj.md"),
        block_id: "done-p".to_string(),
        task_id: "done-p".to_string(),
        text: "Done P".to_string(),
        is_root: true,
    }];
    let snapshot: Vec<(PathBuf, String)> = notes
        .iter()
        .map(|(path, contents)| (path.clone(), contents.clone()))
        .collect();
    let index = planner_index(&notes);
    let index_static: &'static NoteIndex = Box::leak(Box::new(index));
    let settings_static: &'static crate::native::task_status_hooks::TasksSettings =
        Box::leak(Box::new(settings));
    let note_settings_static: &'static crate::native::note_tasks::NoteTaskSettings =
        Box::leak(Box::new(test_settings()));
    let relation = RelationSuccessorInput {
        labels,
        parked,
        relation_snapshot_complete: true,
        dependency_snapshot_complete: false,
        previous_daily: None,
        unavailable_targets: BTreeSet::new(),
    };
    let plan = plan_successors(SuccessorInput {
        bob_dir: None,
        completed,
        snapshot: snapshot.clone(),
        note_texts: snapshot,
        pre_day: Some(day_before),
        post_day: Some(day_after),
        closing: Some(super::super::successors::ClosingContext {
            continuation_name: "BUILD".to_string(),
            closed_line: 1,
            continuation: Some(super::super::successors::ClosingContinuation {
                line: 4,
                created: true,
            }),
        }),
        day_relative: day_path,
        today,
        tasks_settings: settings_static,
        note_settings: note_settings_static,
        link_unblocked: true,
        keep_moving: false,
        snapshot_complete: true,
        selection: SuccessorSelection::All,
        index: index_static,
        relation: Some(relation),
    });
    assert_eq!(plan.relation_check, Some(RelationCheck::Checked));
    // The pitch resident moves to relation but keeps project recovery
    // (changed_files still shows the project recovery via Apply/Other).
    let pitch_relation = plan
        .relation_successors
        .iter()
        .find(|row| row.block_id == "pitch")
        .expect("pitch in relation_successors");
    assert_eq!(
        pitch_relation.offer.as_ref().expect("offer").group,
        OfferGroup::Relation
    );
    let relation_offer = pitch_relation.offer.as_ref().expect("offer");
    assert_eq!(relation_offer.destination.entry_name, "WRITE");
    assert_eq!(
        relation_offer.destination.entry_role,
        super::super::successors::EntryRole::Slot
    );
    assert!(
        plan.new_day_text
            .as_deref()
            .is_some_and(|text| text.contains(
                "- [ ] () — WRITE\n  - [[agents#^evaluate]]\n  - [[proj#^pitch]]\n  - [[other#^keep]]\n"
            )),
        "relation trigger controls the placement: {:?}",
        plan.new_day_text
    );
    assert!(pitch_relation.relation.is_some());
    let updated = plan
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed preserves recovery");
    assert!(updated.contains("[unblocked:: 2026-11-02]"), "{updated}");
    assert!(
        plan.unblocked.iter().any(|row| row.block_id == "other"),
        "project recovery still offers Other: {:?}",
        plan.unblocked
    );
}

#[test]
fn relation_wins_over_project_finished_and_keeps_hide_recovery() {
    let notes = BTreeMap::from([
        (
            PathBuf::from("project.md"),
            concat!(
                "---\ntype: [[project]]\nstatus: wip\n---\n",
                "- [x] #task Finish work [id:: done] ^done\n",
                "- [ ] #task #prj Ship it! #hide [id:: prj] ^prj\n",
                "  - 🪢 **LINKS:** starts after [[target#^event]]\n",
            )
            .to_string(),
        ),
        (
            PathBuf::from("target.md"),
            "- [/] #task Parked target [id:: event] ^event\n".to_string(),
        ),
        (
            PathBuf::from("other.md"),
            "- [ ] #task Keep place [id:: keep] ^keep\n".to_string(),
        ),
    ]);
    let world = planner_world_from_notes(
        notes,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**) — FIX\n",
            "  - [[target#^event]]\n",
            "- [ ] () — WRITE\n",
            "  - [[other#^keep]]\n",
        ),
        concat!(
            "## Pomodoros\n",
            "- [x] (**) — FIX\n",
            "  - ~~[[target#^event]]~~\n",
            "- [ ] () — WRITE\n",
            "  - [[other#^keep]]\n",
        ),
    );
    let completed = vec![completed_task("project.md", "done", "Finish work")];
    let parked = vec![RelationEvent {
        target: RelationTarget {
            note_path: PathBuf::from("target.md"),
            block_id: "event".to_string(),
            text: "Parked target".to_string(),
        },
        kind: RelationEventKind::Parked,
        slot: Some(RelationSlotAnchor {
            note_path: world.day_path.clone(),
            line: 4,
        }),
    }];
    let plan = run_planner(
        &world,
        completed,
        parked,
        SuccessorSelection::All,
        false,
        true,
        true,
        true,
        None,
    );
    let row = plan
        .relation_successors
        .iter()
        .find(|row| row.block_id == "prj")
        .expect("the project-finished pull-in is relation-winning");
    let offer = row.offer.as_ref().expect("relation offer");
    assert_eq!(offer.group, OfferGroup::Relation);
    assert_eq!(row.trigger, "relation");
    assert!(row.hide_removed);
    assert_eq!(offer.destination.entry_name, "WRITE");
    assert_eq!(
        offer.destination.entry_role,
        super::super::successors::EntryRole::Slot
    );
    let updated = plan
        .changed_files
        .get(Path::new("project.md"))
        .expect("project recovery write");
    assert!(
        updated.contains(
            "- [*] #task #prj Ship it! [fresh:: 2026-11-02] [id:: prj] ^prj"
        ),
        "{updated}"
    );
    assert!(!updated.contains("#hide"), "{updated}");
    assert!(
        plan.new_day_text
            .as_deref()
            .is_some_and(|text| text.contains(
                "- [ ] () — WRITE\n  - [[project#^prj]]\n  - [[other#^keep]]\n"
            )),
        "the relation slot overrides the project-finished destination: {:?}",
        plan.new_day_text
    );
}

#[test]
fn relation_wins_over_keep_moving_and_reanchors_the_offer() {
    let notes = BTreeMap::from([
        (
            PathBuf::from("dev.md"),
            "---\ntype: [[area]]\n---\n".to_string(),
        ),
        (
            PathBuf::from("parent.md"),
            "---\ntype: [[project]]\nparent: [[dev]]\nstatus: wip\n---\n- [ ] #task Parent ^prj\n".to_string(),
        ),
        (
            PathBuf::from("project.md"),
            concat!(
                "---\ntype: [[project]]\nparent: [[parent]]\nstatus: wip\n---\n",
                "- [x] #task Completed spec [id:: done] ^done\n",
                "- [ ] #task #prj Project anchor ^prj\n",
                "- [ ] #task Follow-up [id:: follow] ^follow\n",
                "  - 🪢 **LINKS:** starts after [[target#^event]]\n",
            )
            .to_string(),
        ),
        (
            PathBuf::from("target.md"),
            "- [/] #task Parked target [id:: event] ^event\n".to_string(),
        ),
        (
            PathBuf::from("other.md"),
            "- [ ] #task Keep place [id:: keep] ^keep\n".to_string(),
        ),
    ]);
    let world = planner_world_from_notes(
        notes,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — FIX\n",
            "  - [[project#^done]]\n",
            "  - [[target#^event]]\n",
            "- [ ] () — WRITE\n",
            "  - [[other#^keep]]\n",
        ),
        concat!(
            "## Pomodoros\n",
            "- [ ] () — FIX\n",
            "  - ~~[[project#^done]]~~\n",
            "  - ~~[[target#^event]]~~\n",
            "- [ ] () — WRITE\n",
            "  - [[other#^keep]]\n",
        ),
    );
    let completed =
        vec![completed_task("project.md", "done", "Completed spec")];
    let parked = vec![RelationEvent {
        target: RelationTarget {
            note_path: PathBuf::from("target.md"),
            block_id: "event".to_string(),
            text: "Parked target".to_string(),
        },
        kind: RelationEventKind::Parked,
        slot: Some(RelationSlotAnchor {
            note_path: world.day_path.clone(),
            line: 5,
        }),
    }];
    let plan = run_planner_with_options(
        &world,
        completed,
        parked,
        SuccessorSelection::All,
        false,
        true,
        true,
        true,
        None,
        PlannerOptions {
            keep_moving: true,
            ..PlannerOptions::default()
        },
    );
    let row = plan
        .relation_successors
        .iter()
        .find(|row| row.block_id == "follow")
        .expect("keep-moving resident is relation-winning");
    let offer = row.offer.as_ref().expect("offer");
    assert_eq!(offer.group, OfferGroup::Relation);
    assert_eq!(row.trigger, "relation");
    assert_eq!(offer.destination.entry_name, "WRITE");
    assert_eq!(
        offer.destination.entry_role,
        super::super::successors::EntryRole::Slot
    );
    assert!(plan.keep_moving.iter().all(|row| row.block_id != "follow"));
    assert!(plan.keep_moving_folds.is_empty());
    assert!(
        plan.new_day_text
            .as_deref()
            .is_some_and(|text| text.contains("- [ ] () — WRITE\n  - [[project#^follow]]\n  - [[other#^keep]]\n")),
        "the relation trigger reanchors the former keep-moving row: {:?}",
        plan.new_day_text
    );
}

#[test]
fn integrate_sa18_mixed_done_and_parked_group_order() {
    let data = vectors();
    let base = &data["base"];
    let mut world = World::materialize(base, &Value::Null, &Value::Null);
    // Add a second Done target (schema) with a direct dependent so the
    // gesture has both a hand_off (Done) and a relation (parked) group.
    world.notes.insert(
        PathBuf::from("schema.md"),
        concat!(
            "- [x] #task Schema change [id:: schema] ^schema\n",
            "- [ ] #task Schema follow-up [dependsOn:: schema] [id:: schema-change] ^schema-change\n",
        )
        .to_string(),
    );
    ensure_target_task(&mut world.notes, "agents.md#^deploy");
    let deploy_event = RelationEvent {
        target: RelationTarget {
            note_path: PathBuf::from("agents.md"),
            block_id: "deploy".to_string(),
            text: "Deploy release".to_string(),
        },
        kind: RelationEventKind::Done,
        slot: None,
    };
    let mut completed = completed_from_events(&world.events, &world.notes);
    completed.push(CompletedTask {
        relative_path: PathBuf::from("schema.md"),
        block_id: "schema".to_string(),
        task_id: "schema".to_string(),
        text: "Schema change".to_string(),
        is_root: true,
    });
    let parked = parked_from_events(&world.events);
    let _ = deploy_event;
    let plan = run_planner(
        &world,
        completed,
        parked,
        SuccessorSelection::All,
        true,
        true,
        true,
        true,
        None,
    );
    assert_eq!(plan.relation_check, Some(RelationCheck::Checked));
    assert!(!plan.unblocked.is_empty(), "hand_off present");
    assert!(!plan.relation_successors.is_empty(), "relation present");
    // Global precedence: every hand_off numbers before any relation.
    let hand_off_n = plan
        .unblocked
        .iter()
        .filter_map(|row| row.offer.as_ref().map(|offer| offer.n))
        .min()
        .expect("hand_off n");
    let relation_n = plan
        .relation_successors
        .iter()
        .filter_map(|row| row.offer.as_ref().map(|offer| offer.n))
        .min()
        .expect("relation n");
    assert!(
        hand_off_n < relation_n,
        "hand_off {hand_off_n} before relation {relation_n}"
    );
}

#[test]
fn integrate_sa20_edge_added_after_event_does_not_replay() {
    let data = vectors();
    // Plan the parked event with its then-current texts (no edge).
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
    let completed = Vec::new();
    let parked = parked_from_events(&world.events);
    // Rebuild events against the stripped texts so the target text still
    // resolves; the classifier sees no edge.
    let plan = run_planner(
        &world,
        completed,
        parked,
        SuccessorSelection::All,
        true,
        true,
        true,
        true,
        None,
    );
    assert!(plan.relation_successors.is_empty());
    assert!(plan.relation_held.is_empty());
    // Edge-free lookup returns no relation fields.
    assert_eq!(plan.relation_check, None);
    // Adding the edge afterward does not retroactively offer; authoring
    // warns with the shared already-released code.
    let warning = edge_added_after_release_warning(
        "starts after",
        "Prepare evaluation prompt",
        "Send implementation prompt",
    );
    assert_eq!(warning.code, "relation_successor_already_released");
    assert!(warning.message.contains("already parked"));
}

#[test]
fn integrate_sa21_final_state_filter_removes_stale_rows() {
    // Parked target re-linked: the parked event no longer holds.
    assert!(!relation_offer_survives_final_state(
        ' ',
        false,
        false,
        false,
        crate::native::task_complete::relation_successors::copy::EventKind::Parked,
        '/',
        true,
    ));
    // Done events survive target liveness; only the source matters.
    assert!(relation_offer_survives_final_state(
        ' ',
        false,
        false,
        false,
        crate::native::task_complete::relation_successors::copy::EventKind::Done,
        'x',
        true,
    ));
    // Every specified source invalidation removes the row.
    for (status, live, blocked, hidden) in [
        ('x', false, false, false),
        ('-', false, false, false),
        ('~', false, false, false),
        ('/', false, false, false),
        (' ', true, false, false),
        (' ', false, true, false),
        (' ', false, false, true),
        ('?', false, false, false),
    ] {
        assert!(
            !relation_offer_survives_final_state(
                status,
                live,
                blocked,
                hidden,
                crate::native::task_complete::relation_successors::copy::EventKind::Parked,
                '/',
                false,
            ),
            "invalidated source {status} live={live} blocked={blocked} hidden={hidden}"
        );
    }
    // A live planner replay also drops the group when the parked target
    // regains a live link in the final day.
    let data = vectors();
    let world = World::materialize(&data["base"], &Value::Null, &Value::Null);
    let completed = Vec::new();
    let parked = parked_from_events(&world.events);
    let mut relinked = world;
    relinked.day_after = relinked.day_before.clone();
    let plan = run_planner(
        &relinked,
        completed,
        parked,
        SuccessorSelection::All,
        true,
        true,
        true,
        true,
        None,
    );
    assert!(
        plan.relation_successors.is_empty() && plan.relation_held.is_empty(),
        "relinked parked target drops groups: {:?} {:?}",
        plan.relation_successors,
        plan.relation_held
    );
}

#[test]
fn integrate_gates_do_no_relation_work() {
    let data = vectors();
    let world = World::materialize(&data["base"], &Value::Null, &Value::Null);
    let completed = Vec::new();
    let parked = parked_from_events(&world.events);
    // Empty labels.
    let mut empty_labels = world.clone();
    empty_labels.labels.clear();
    let plan = run_planner(
        &empty_labels,
        completed.clone(),
        parked.clone(),
        SuccessorSelection::All,
        true,
        true,
        true,
        true,
        None,
    );
    assert_eq!(plan.relation_check, None);
    assert!(plan.relation_successors.is_empty());
    assert!(plan.relation_groups.is_empty());
    // link_unblocked false.
    let plan = run_planner(
        &world,
        completed.clone(),
        parked.clone(),
        SuccessorSelection::All,
        true,
        false,
        true,
        true,
        None,
    );
    assert_eq!(plan.relation_check, None);
    assert!(plan.relation_successors.is_empty());
    // Missing event IDs.
    let mut missing = parked.clone();
    for event in &mut missing {
        event.target.block_id.clear();
    }
    let plan = run_planner(
        &world,
        completed.clone(),
        missing,
        SuccessorSelection::All,
        true,
        true,
        true,
        true,
        None,
    );
    assert_eq!(plan.relation_check, None);
    assert!(plan.relation_successors.is_empty());
    // Empty C with no parked events.
    let plan = run_planner(
        &world,
        Vec::new(),
        Vec::new(),
        SuccessorSelection::All,
        true,
        true,
        true,
        true,
        None,
    );
    assert_eq!(plan.relation_check, None);
    assert!(plan.relation_successors.is_empty());
}

#[test]
fn integrate_snapshots_keep_legacy_recovery_safe() {
    let data = vectors();
    let world = World::materialize(&data["base"], &Value::Null, &Value::Null);
    // Ordinary dependency recovery still runs when the relation snapshot
    // is unavailable; only relation offers are suppressed.
    let mut notes = world.notes.clone();
    notes.insert(
        PathBuf::from("dep.md"),
        "- [?] #task Follows [dependsOn:: implement] [id:: follows] ^follows\n"
            .to_string(),
    );
    let mut world_unavailable =
        World::materialize(&data["base"], &Value::Null, &Value::Null);
    world_unavailable.notes = notes;
    let completed = vec![CompletedTask {
        relative_path: PathBuf::from("agents.md"),
        block_id: "implement".to_string(),
        task_id: "implement".to_string(),
        text: "Send implementation prompt".to_string(),
        is_root: true,
    }];
    let parked = parked_from_events(&world_unavailable.events);
    let plan = run_planner(
        &world_unavailable,
        completed,
        parked,
        SuccessorSelection::All,
        false,
        true,
        true,
        false,
        None,
    );
    assert_eq!(plan.relation_check, Some(RelationCheck::Unavailable));
    assert!(plan.relation_successors.is_empty());
    assert!(
        !plan.unblocked.is_empty() || !plan.still_blocked.is_empty(),
        "legacy recovery still runs: {:?} {:?}",
        plan.unblocked,
        plan.still_blocked
    );
    // Unavailable dependency data must not leak unsafe relation offers:
    // a source with an open Depends-On stays held, never offered.
    let mut blocked_world =
        World::materialize(&data["base"], &Value::Null, &Value::Null);
    insert_token(
        &mut blocked_world.notes,
        Path::new("agents.md"),
        "evaluate",
        "[dependsOn:: block]",
    );
    let completed = Vec::new();
    let parked = parked_from_events(&blocked_world.events);
    let settings = test_settings();
    let snapshot: Vec<(PathBuf, String)> = blocked_world
        .notes
        .iter()
        .map(|(path, contents)| (path.clone(), contents.clone()))
        .collect();
    let index = planner_index(&blocked_world.notes);
    let index_static: &'static NoteIndex = Box::leak(Box::new(index));
    let settings_static: &'static crate::native::task_status_hooks::TasksSettings =
        Box::leak(Box::new(settings));
    let note_settings_static: &'static crate::native::note_tasks::NoteTaskSettings =
        Box::leak(Box::new(test_settings()));
    let relation = RelationSuccessorInput {
        labels: blocked_world.labels.clone(),
        parked,
        relation_snapshot_complete: true,
        dependency_snapshot_complete: false,
        previous_daily: None,
        unavailable_targets: BTreeSet::new(),
    };
    let plan = plan_successors(SuccessorInput {
        bob_dir: None,
        completed,
        snapshot: snapshot.clone(),
        note_texts: snapshot,
        pre_day: Some(blocked_world.day_before.clone()),
        post_day: Some(blocked_world.day_after.clone()),
        closing: None,
        day_relative: blocked_world.day_path.clone(),
        today: blocked_world.today,
        tasks_settings: settings_static,
        note_settings: note_settings_static,
        link_unblocked: true,
        keep_moving: false,
        snapshot_complete: false,
        selection: SuccessorSelection::All,
        index: index_static,
        relation: Some(relation),
    });
    assert!(plan.relation_successors.is_empty());
    assert_eq!(plan.relation_held.len(), 1);
}

#[test]
fn integrate_selection_keys_numbering_and_day_bytes() {
    let data = vectors();
    let world = World::materialize(&data["base"], &Value::Null, &Value::Null);
    let completed = Vec::new();
    let parked = parked_from_events(&world.events);
    let picked = run_planner(
        &world,
        completed.clone(),
        parked.clone(),
        SuccessorSelection::All,
        true,
        true,
        true,
        true,
        None,
    );
    let unpicked = run_planner(
        &world,
        completed,
        parked,
        SuccessorSelection::None,
        true,
        true,
        true,
        true,
        None,
    );
    assert_eq!(picked.relation_successors.len(), 1);
    assert_eq!(unpicked.relation_successors.len(), 1);
    let picked_row = &picked.relation_successors[0];
    let unpicked_row = &unpicked.relation_successors[0];
    assert_eq!(
        picked_row.offer.as_ref().expect("offer").key,
        unpicked_row.offer.as_ref().expect("offer").key
    );
    assert_eq!(
        picked_row.offer.as_ref().expect("offer").n,
        unpicked_row.offer.as_ref().expect("offer").n
    );
    assert!(picked_row.link.is_some());
    assert!(unpicked_row.link.is_none());
    assert_eq!(unpicked_row.not_linked, Some("not_selected"));
    // Picked writes: Ready -> Next, mint when missing, fresh stamp, Task Link.
    assert_eq!(picked_row.previous_status_symbol, ' ');
    assert_eq!(picked_row.status_symbol, '*');
    assert!(picked.new_day_text.is_some());
    assert!(unpicked.new_day_text.is_none());
    // Unpicked relation-only rows write absolutely nothing.
    assert!(unpicked.changed_files.is_empty());
    // Missing source IDs keep provisional keys stable through selection.
    let mut missing = world;
    let path = PathBuf::from("agents.md");
    let contents = missing.notes.get_mut(&path).expect("agents");
    *contents = contents.replace(" ^evaluate", "");
    let parked = parked_from_events(&missing.events);
    let picked = run_planner(
        &missing,
        Vec::new(),
        parked.clone(),
        SuccessorSelection::All,
        true,
        true,
        true,
        true,
        None,
    );
    let unpicked = run_planner(
        &missing,
        Vec::new(),
        parked,
        SuccessorSelection::None,
        true,
        true,
        true,
        true,
        None,
    );
    assert_eq!(picked.relation_successors.len(), 1);
    assert!(!picked.relation_successors[0].block_id.is_empty());
    assert_eq!(
        picked.relation_successors[0]
            .offer
            .as_ref()
            .expect("offer")
            .key,
        unpicked.relation_successors[0]
            .offer
            .as_ref()
            .expect("offer")
            .key
    );
}

#[test]
fn integrate_repeated_park_then_done_is_stateless() {
    let data = vectors();
    let world = World::materialize(&data["base"], &Value::Null, &Value::Null);
    // Unpicked park writes nothing, so a later Done still offers.
    let parked = parked_from_events(&world.events);
    let unpicked_park = run_planner(
        &world,
        Vec::new(),
        parked,
        SuccessorSelection::None,
        true,
        true,
        true,
        true,
        None,
    );
    assert_eq!(unpicked_park.relation_successors.len(), 1);
    assert!(unpicked_park.changed_files.is_empty());
    let done_completed = completed_from_events(&world.events, &world.notes)
        .into_iter()
        .map(|mut task| {
            task.task_id = task.block_id.clone();
            task
        })
        .collect::<Vec<_>>();
    // If the world had no Done events (base is parked), synthesize the
    // later Done from the same target.
    let done_events = if done_completed.is_empty() {
        vec![CompletedTask {
            relative_path: PathBuf::from("agents.md"),
            block_id: "implement".to_string(),
            task_id: "implement".to_string(),
            text: "Send implementation prompt".to_string(),
            is_root: true,
        }]
    } else {
        done_completed
    };
    let done_plan = run_planner(
        &world,
        done_events,
        Vec::new(),
        SuccessorSelection::None,
        false,
        true,
        true,
        true,
        None,
    );
    assert_eq!(done_plan.relation_successors.len(), 1);
}

fn relation_slot_world() -> World {
    let notes = BTreeMap::from([
        (
            PathBuf::from("target.md"),
            "- [/] #task Parked target [id:: event] ^event\n".to_string(),
        ),
        (
            PathBuf::from("sources.md"),
            concat!(
                "- [ ] #task Follow-up [id:: follow] ^follow\n",
                "  - 🪢 **LINKS:** starts after [[target#^event]]\n",
            )
            .to_string(),
        ),
        (
            PathBuf::from("other.md"),
            "- [ ] #task First [id:: first] ^first\n- [ ] #task Second [id:: second] ^second\n".to_string(),
        ),
    ]);
    planner_world_from_notes(
        notes,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — CLOSED\n",
            "  - [[target#^event]]\n",
            "  - [[other#^first]]\n",
            "- [ ] () — OTHER\n",
            "  - [[other#^second]]\n",
        ),
        concat!(
            "## Pomodoros\n",
            "- [x] () — CLOSED\n",
            "  - ~~[[target#^event]]~~\n",
            "- [ ] () — OTHER\n",
            "  - [[other#^first]]\n",
            "- [ ] () — CONT\n",
        ),
    )
}

fn parked_slot_event(world: &World, line: Option<usize>) -> RelationEvent {
    RelationEvent {
        target: RelationTarget {
            note_path: PathBuf::from("target.md"),
            block_id: "event".to_string(),
            text: "Parked target".to_string(),
        },
        kind: RelationEventKind::Parked,
        slot: line.map(|line| RelationSlotAnchor {
            note_path: world.day_path.clone(),
            line,
        }),
    }
}

fn relation_close_context() -> super::super::successors::ClosingContext {
    super::super::successors::ClosingContext {
        continuation_name: "CONT".to_string(),
        closed_line: 1,
        continuation: Some(super::super::successors::ClosingContinuation {
            line: 6,
            created: true,
        }),
    }
}

#[test]
fn explicit_relation_slot_precedes_close_and_unpicked_is_write_free() {
    let world = relation_slot_world();
    // Post-gesture line 4 is the surviving child in the open OTHER entry.
    let event = parked_slot_event(&world, Some(4));
    let unpicked = run_planner_with_options(
        &world,
        Vec::new(),
        vec![event.clone()],
        SuccessorSelection::None,
        true,
        true,
        true,
        true,
        None,
        PlannerOptions {
            closing_context: Some(relation_close_context()),
            ..PlannerOptions::default()
        },
    );
    assert_eq!(unpicked.relation_successors.len(), 1);
    assert_eq!(
        unpicked.relation_successors[0].not_linked,
        Some("not_selected")
    );
    let offer = unpicked.relation_successors[0]
        .offer
        .as_ref()
        .expect("unpicked relation offer");
    assert_eq!(offer.destination.entry_name, "OTHER");
    assert_eq!(
        offer.destination.entry_role,
        super::super::successors::EntryRole::Slot
    );
    assert!(!offer.destination.fallback);
    assert!(unpicked.changed_files.is_empty());
    assert!(unpicked.new_day_text.is_none());

    // The explicit slot points to OTHER; the close continuation is CONT.
    // The slot wins, and its 0-based post-gesture line is used as the
    // insertion anchor in the exact returned day bytes.
    let picked = run_planner_with_options(
        &world,
        Vec::new(),
        vec![event],
        SuccessorSelection::All,
        true,
        true,
        true,
        true,
        None,
        PlannerOptions {
            closing_context: Some(relation_close_context()),
            ..PlannerOptions::default()
        },
    );
    assert_eq!(picked.relation_successors.len(), 1);
    assert_eq!(
        picked.new_day_text.as_deref(),
        Some(concat!(
            "## Pomodoros\n",
            "- [x] () — CLOSED\n",
            "  - ~~[[target#^event]]~~\n",
            "- [ ] () — OTHER\n",
            "  - [[sources#^follow]]\n",
            "  - [[other#^first]]\n",
            "- [ ] () — CONT\n",
        )),
        "the relation successor is inserted at its explicit slot"
    );
    let row = &picked.relation_successors[0];
    assert_eq!(row.link.as_ref().expect("link").entry_name, "OTHER");
    assert_eq!(row.link.as_ref().expect("link").entry_line, 4);
}

#[test]
fn invalid_deleted_slot_falls_back_to_implicit_even_during_close() {
    let world = relation_slot_world();
    for invalid_line in [2, 100] {
        // Line 2 is inside the closed entry; line 100 no longer exists.
        let event = parked_slot_event(&world, Some(invalid_line));
        let plan = run_planner_with_options(
            &world,
            Vec::new(),
            vec![event],
            SuccessorSelection::None,
            true,
            true,
            true,
            true,
            None,
            PlannerOptions {
                closing_context: Some(relation_close_context()),
                ..PlannerOptions::default()
            },
        );
        let row = plan.relation_successors.first().expect("held offer");
        let offer = row.offer.as_ref().expect("offer");
        assert_eq!(
            offer.destination.entry_role,
            super::super::successors::EntryRole::Implicit
        );
        assert!(offer.destination.fallback);
        assert!(plan.changed_files.is_empty());
        assert!(plan.new_day_text.is_none());
    }
}

#[test]
fn closing_continuation_last_is_used_when_there_is_no_slot() {
    let world = relation_slot_world();
    let event = parked_slot_event(&world, None);
    let plan = run_planner_with_options(
        &world,
        Vec::new(),
        vec![event],
        SuccessorSelection::All,
        true,
        true,
        true,
        true,
        None,
        PlannerOptions {
            closing_context: Some(relation_close_context()),
            ..PlannerOptions::default()
        },
    );
    let row = plan.relation_successors.first().expect("relation row");
    let offer = row.offer.as_ref().expect("offer");
    assert_eq!(offer.destination.entry_name, "CONT");
    assert_eq!(
        offer.destination.entry_role,
        super::super::successors::EntryRole::Created
    );
    assert!(plan
        .new_day_text
        .as_deref()
        .expect("inserted day")
        .contains("- [ ] () — CONT\n\t- [[sources#^follow]]\n"));
}

#[test]
fn relation_slot_routes_to_the_task_type_as_final_destination() {
    let dir = tempfile::tempdir().expect("temporary routing home");
    let root = dir.path();
    let config = root.join("config.yml");
    std::fs::write(
        &config,
        concat!(
            "plan:\n  default_type: CHORE\n",
            "types:\n  FIX: blue\n  WRITE: magenta\n  CHORE: yellow\n",
        ),
    )
    .expect("write routing config");
    let _env =
        TestEnvGuard::set(&[("BOB_CONFIG_FILE", Some(config.as_os_str()))]);

    let notes = BTreeMap::from([
        (
            PathBuf::from("target.md"),
            "- [/] #task Parked target [id:: event] ^event\n".to_string(),
        ),
        (
            PathBuf::from("sources.md"),
            concat!(
                "- [ ] #task Typed follow-up [type:: WRITE] [id:: follow] ^follow\n",
                "  - 🪢 **LINKS:** starts after [[target#^event]]\n",
            )
            .to_string(),
        ),
        (
            PathBuf::from("other.md"),
            "- [ ] #task First [id:: first] ^first\n- [ ] #task Second [id:: second] ^second\n".to_string(),
        ),
    ]);
    for (path, contents) in &notes {
        std::fs::write(root.join(path), contents).expect("write indexed note");
    }
    let world = planner_world_from_notes(
        notes,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — FIX\n",
            "  - [[target#^event]]\n",
            "  - [[other#^first]]\n",
            "- [ ] () — WRITE\n",
            "  - [[other#^second]]\n",
        ),
        concat!(
            "## Pomodoros\n",
            "- [ ] () — FIX\n",
            "  - [[other#^first]]\n",
            "- [ ] () — WRITE\n",
            "  - [[other#^second]]\n",
        ),
    );
    let event = RelationEvent {
        target: RelationTarget {
            note_path: PathBuf::from("target.md"),
            block_id: "event".to_string(),
            text: "Parked target".to_string(),
        },
        kind: RelationEventKind::Parked,
        // Slot line 2 is the surviving child under FIX in post text.
        slot: Some(RelationSlotAnchor {
            note_path: world.day_path.clone(),
            line: 2,
        }),
    };
    let plan = run_planner_with_options(
        &world,
        Vec::new(),
        vec![event],
        SuccessorSelection::All,
        false,
        true,
        true,
        true,
        None,
        PlannerOptions {
            bob_dir: Some(root),
            ..PlannerOptions::default()
        },
    );
    let row = plan.relation_successors.first().expect("relation row");
    let offer = row.offer.as_ref().expect("offer");
    assert_eq!(offer.group, OfferGroup::Relation);
    assert_eq!(offer.destination.entry_name, "WRITE");
    assert_eq!(
        offer.destination.entry_role,
        super::super::successors::EntryRole::Typed
    );
    assert!(!offer.destination.fallback);
    assert!(
        plan.new_day_text
            .as_deref()
            .is_some_and(|text| text.contains("- [ ] () — WRITE\n  - [[other#^second]]\n\t- [[sources#^follow]]\n")),
        "the typed destination is the final route: {:?}",
        plan.new_day_text
    );
    assert_eq!(row.link.as_ref().expect("link").entry_name, "WRITE");
}

#[test]
fn effective_dependencies_use_closed_prerequisites_and_hold_incomplete_data() {
    let (world, event) = recovery_world();
    let none = run_planner(
        &world,
        vec![event.clone()],
        Vec::new(),
        SuccessorSelection::None,
        false,
        true,
        true,
        true,
        None,
    );
    assert_eq!(
        none.relation_successors.len(),
        1,
        "closed prerequisite is ready"
    );
    assert_eq!(none.relation_successors[0].not_linked, Some("not_selected"));
    assert!(none.changed_files.is_empty());
    assert!(none.new_day_text.is_none());

    let picked = run_planner(
        &world,
        vec![event.clone()],
        Vec::new(),
        SuccessorSelection::All,
        false,
        true,
        true,
        true,
        None,
    );
    assert_eq!(picked.relation_successors.len(), 1);
    assert!(picked.relation_successors[0].link.is_some());
    assert!(picked.changed_files[Path::new("source.md")]
        .contains("[fresh:: 2026-11-02]"));

    for selection in [SuccessorSelection::None, SuccessorSelection::All] {
        let held = run_planner(
            &world,
            vec![event.clone()],
            Vec::new(),
            selection,
            false,
            true,
            false,
            true,
            None,
        );
        assert!(held.relation_successors.is_empty());
        assert_eq!(held.relation_held.len(), 1);
        assert!(matches!(
            held.relation_held[0].reason,
            RelationHeldReason::Blocked { .. }
        ));
        assert!(held.changed_files.is_empty());
        assert!(held.new_day_text.is_none());
    }
}

#[test]
fn integration_holds_indexed_unread_targets_and_caches_their_note_read() {
    let notes = BTreeMap::from([
        (
            PathBuf::from("area/target.md"),
            "- [/] #task Event target [id:: event] ^event\n".to_string(),
        ),
        (
            PathBuf::from("sources.md"),
            concat!(
                "- [ ] #task Follow-up [id:: follow] ^follow\n",
                "  - 🪢 **LINKS:** starts after [[target#^event]]\n",
                "  - 🪢 **LINKS:** starts after [[unheld#^one]]\n",
                "  - 🪢 **LINKS:** starts after [[unheld#^two]]\n",
            )
            .to_string(),
        ),
        (
            PathBuf::from("unheld.md"),
            "- [ ] #task One [id:: one] ^one\n- [ ] #task Two [id:: two] ^two\n"
                .to_string(),
        ),
    ]);
    let world = planner_world_from_notes(
        notes,
        "## Pomodoros\n- [ ] () — EVENT\n  - [[area/target#^event]]\n",
        "## Pomodoros\n- [ ] () — EVENT\n",
    );
    let event = RelationEvent {
        target: RelationTarget {
            note_path: PathBuf::from("area/target.md"),
            block_id: "event".to_string(),
            text: "Event target".to_string(),
        },
        kind: RelationEventKind::Parked,
        slot: None,
    };
    let mut omit = BTreeSet::new();
    omit.insert(PathBuf::from("unheld.md"));
    let plan = run_planner_with_options(
        &world,
        Vec::new(),
        vec![event],
        SuccessorSelection::All,
        false,
        true,
        true,
        true,
        None,
        PlannerOptions {
            omit_snapshot_paths: omit,
            ..PlannerOptions::default()
        },
    );
    assert_eq!(
        plan.relation_target_reads, 1,
        "one read per target note; groups={:?} held={:?}",
        plan.relation_groups, plan.relation_held
    );
    assert!(plan.relation_successors.is_empty());
    assert_eq!(plan.relation_held.len(), 1);
    let RelationHeldReason::WaitsFor(waiting) = &plan.relation_held[0].reason
    else {
        panic!(
            "indexed but unavailable prerequisites must hold: {:?}",
            plan.relation_held
        );
    };
    assert_eq!(waiting.len(), 2);
    assert!(waiting
        .iter()
        .all(|target| target.note_path == Path::new("unheld.md")));
    assert!(plan.changed_files.is_empty());
    assert!(plan.new_day_text.is_none());
}

#[test]
fn integration_ambiguous_indexed_basename_is_unresolved() {
    let notes = BTreeMap::from([
        (
            PathBuf::from("area/target.md"),
            "- [/] #task Event target [id:: event] ^event\n".to_string(),
        ),
        (
            PathBuf::from("folder/target.md"),
            "- [ ] #task Different target [id:: other] ^other\n".to_string(),
        ),
        (
            PathBuf::from("sources.md"),
            concat!(
                "- [ ] #task Follow-up [id:: follow] ^follow\n",
                "  - 🪢 **LINKS:** starts after [[target#^event]]\n",
            )
            .to_string(),
        ),
    ]);
    let world = planner_world_from_notes(
        notes,
        "## Pomodoros\n- [ ] () — EVENT\n  - [[area/target#^event]]\n",
        "## Pomodoros\n- [ ] () — EVENT\n",
    );
    let event = RelationEvent {
        target: RelationTarget {
            note_path: PathBuf::from("area/target.md"),
            block_id: "event".to_string(),
            text: "Event target".to_string(),
        },
        kind: RelationEventKind::Parked,
        slot: None,
    };
    let plan = run_planner(
        &world,
        Vec::new(),
        vec![event],
        SuccessorSelection::All,
        false,
        true,
        true,
        true,
        None,
    );
    assert!(plan.relation_successors.is_empty());
    assert!(plan.relation_held.is_empty());
    assert_eq!(plan.relation_target_reads, 0);
    assert!(plan.changed_files.is_empty());
}

#[test]
fn classifier_orders_same_lane_by_source_line() {
    let data = vectors();
    let mut world =
        World::materialize(&data["base"], &Value::Null, &Value::Null);
    world.notes.insert(
        PathBuf::from("sources.md"),
        concat!(
            "- [ ] #task First source ^z-first\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [ ] #task Second source ^a-second\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
        )
        .to_string(),
    );
    let result = world.classify();
    let ids = result.groups[0]
        .rows
        .iter()
        .filter(|row| row.source.note_path == Path::new("sources.md"))
        .map(|row| row.source.block_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["z-first", "a-second"]);
}

#[test]
fn planner_numbers_next_before_ready_across_notes() {
    let data = vectors();
    let mut world =
        World::materialize(&data["base"], &Value::Null, &Value::Null);
    world.notes.insert(
        PathBuf::from("later.md"),
        concat!(
            "- [*] #task Next source ^next\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
        )
        .to_string(),
    );
    let parked = parked_from_events(&world.events);
    let plan = run_planner(
        &world,
        Vec::new(),
        parked,
        SuccessorSelection::None,
        true,
        true,
        true,
        true,
        None,
    );
    let next = plan
        .relation_successors
        .iter()
        .find(|row| row.block_id == "next")
        .unwrap();
    let ready = plan
        .relation_successors
        .iter()
        .find(|row| row.block_id == "evaluate")
        .unwrap();
    assert!(
        next.offer.as_ref().unwrap().n < ready.offer.as_ref().unwrap().n,
        "Next n={} Ready n={}",
        next.offer.as_ref().unwrap().n,
        ready.offer.as_ref().unwrap().n
    );
}

#[test]
fn recovered_blocked_direct_dependent_keeps_relation_reason() {
    let (mut world, completed, parked) = planner_world("SA14");
    insert_token(
        &mut world.notes,
        Path::new("agents.md"),
        "evaluate",
        "[dependsOn:: implement]",
    );
    rewrite_status(&mut world.notes, Path::new("agents.md"), "implement", 'x');
    rewrite_status(&mut world.notes, Path::new("agents.md"), "evaluate", '?');
    let plan = run_planner(
        &world,
        completed,
        parked,
        SuccessorSelection::All,
        false,
        true,
        true,
        true,
        None,
    );
    let row = plan
        .unblocked
        .iter()
        .find(|row| row.block_id == "evaluate")
        .unwrap();
    assert!(
        row.relation.is_some(),
        "recovered direct dependent lost relation metadata: {row:?}"
    );
}

#[test]
fn missing_id_selects_the_exact_source_row() {
    let data = vectors();
    let mut world =
        World::materialize(&data["base"], &Value::Null, &Value::Null);
    world.notes.insert(
        PathBuf::from("sources.md"),
        concat!(
            "- [ ] #task Same description\n",
            "- [ ] #task Same description\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
        )
        .to_string(),
    );
    let parked = parked_from_events(&world.events);
    let plan = run_planner(
        &world,
        Vec::new(),
        parked,
        SuccessorSelection::All,
        true,
        true,
        true,
        true,
        None,
    );
    let updated = plan.changed_files.get(Path::new("sources.md")).unwrap();
    assert!(
        updated.starts_with(
            "- [ ] #task Same description\n- [*] #task Same description"
        ),
        "{updated}"
    );
}
