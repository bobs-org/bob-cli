use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;
use serde::Deserialize;

use super::verdicts::{
    relation_policy_verdict, ItemActionConflict, RelationAction,
    RelationDirection, RelationPolicyInput, RelationTrigger,
};
use super::*;
use crate::native::{
    config::relations::parse_relations_config,
    env::TestEnvGuard,
    relation_labels::relation_label_key,
    task_status_hooks::{TaskStatusDefinition, TaskStatusType},
    vault_links::NoteIndex,
};

#[derive(Debug, Deserialize)]
struct Vectors {
    cancel: Vec<CancelVector>,
    prune: Vec<PruneVector>,
    rule: Vec<RuleVector>,
    key: Vec<KeyVector>,
    config: Vec<ConfigVector>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelVector {
    name: String,
    content: String,
    line: usize,
    date: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    fallback_reason: String,
    expected: CancelExpected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelExpected {
    valid: bool,
    recurring: bool,
    error: Option<String>,
    content: String,
    cancelled_count: usize,
    skipped_closed_count: usize,
    logged_count: usize,
    created_log_count: usize,
    fallback_logged_count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PruneVector {
    name: String,
    content: String,
    daily_path: String,
    note_paths: Vec<String>,
    targets: Vec<TaskIdentity>,
    expected: PruneExpected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PruneExpected {
    changed: bool,
    content: String,
    removed_link_count: usize,
    removed_bullet_count: usize,
    removed_entry_count: usize,
    unresolved_count: usize,
}

#[derive(Debug, Deserialize)]
struct RuleVector {
    name: String,
    configured: Vec<String>,
    label: String,
    previous_label: Option<String>,
    action: String,
    direction: String,
    source_status: String,
    source_is_new: bool,
    source_is_project: bool,
    source_is_recurring: bool,
    item_modifiers: Vec<String>,
    trigger: String,
    #[allow(dead_code)]
    gesture: Option<String>,
    #[allow(dead_code)]
    label_hidden: Option<bool>,
    expected: RuleExpected,
}

#[derive(Debug, Deserialize)]
struct RuleExpected {
    verdict: String,
    cancels: bool,
}

#[derive(Debug, Deserialize)]
struct KeyVector {
    name: String,
    configured: Vec<String>,
    input: String,
    expected: KeyExpected,
}

#[derive(Debug, Deserialize)]
struct KeyExpected {
    key: String,
    matches: bool,
}

#[derive(Debug, Deserialize)]
struct ConfigVector {
    name: String,
    yaml: String,
    expected: ConfigExpected,
}

#[derive(Debug, Deserialize)]
struct ConfigExpected {
    keys: Vec<String>,
    warning_count: usize,
    linked_from_lines: Option<bool>,
}

fn vectors() -> Vectors {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/task_cancel/vectors.json"
    ))
    .expect("typed task-cancel vectors parse")
}

fn tasks_settings() -> TasksSettings {
    let status_types = BTreeMap::from([
        (' ', TaskStatusType::Todo),
        ('*', TaskStatusType::OnHold),
        ('/', TaskStatusType::InProgress),
        ('?', TaskStatusType::Todo),
        ('x', TaskStatusType::Done),
        ('X', TaskStatusType::Done),
        ('-', TaskStatusType::Cancelled),
        ('>', TaskStatusType::Todo),
    ]);
    TasksSettings {
        global_filter: "#task".to_string(),
        done_statuses: BTreeSet::from(['x', 'X']),
        status_types,
        status_definitions: Vec::<TaskStatusDefinition>::new(),
        status_settings_error: None,
    }
}

fn date(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("fixture date")
}

fn block_id(line: &str) -> Option<String> {
    let trimmed = line.trim_end();
    let start = trimmed.rfind('^')?;
    (start > 0 && trimmed.as_bytes()[start - 1].is_ascii_whitespace())
        .then(|| trimmed[start + 1..].to_string())
}

#[test]
fn cancel_vectors_drive_the_real_source_writer_and_plan() {
    let settings = tasks_settings();
    let index = NoteIndex::from_paths([
        PathBuf::from("source.md"),
        PathBuf::from("2026/20261022.md"),
    ]);
    let vectors = vectors();
    assert_eq!(vectors.cancel.len(), 15);
    for vector in vectors.cancel {
        let source_line = vector
            .content
            .split_inclusive('\n')
            .nth(vector.line)
            .map(super::logical_line)
            .unwrap_or_default();
        let reasons = if !vector.fallback_reason.is_empty() {
            vec![vector.fallback_reason.clone()]
        } else if !vector.reason.is_empty() {
            vec![vector.reason.clone()]
        } else {
            Vec::new()
        };
        let result = plan_task_cancel(TaskCancelInput {
            source_path: PathBuf::from("source.md"),
            source_block_id: block_id(source_line),
            source_line_index: vector.line,
            source_text: vector.content.clone(),
            source_is_new: false,
            ordered_reasons: reasons,
            today: date(&vector.date),
            note_index: &index,
            tasks_settings: &settings,
            note_settings: &settings,
            daily_path: PathBuf::from("2026/20261022.md"),
            daily_state: DailyReadState::Absent,
            dependency_snapshots: vec![(
                PathBuf::from("source.md"),
                vector.content.clone(),
            )],
        });
        if !vector.expected.valid {
            assert!(vector.expected.recurring, "{}", vector.name);
            assert_eq!(
                vector.expected.error.as_deref(),
                Some("Recurring tasks are cancelled with Obsidian Tasks so the next occurrence is handled; no tasks were updated"),
                "{}",
                vector.name
            );
            assert!(matches!(
                result,
                Err(TaskCancelError::Refused(CancelSourceVerdict::Recurring))
            ));
            continue;
        }
        let plan = result.unwrap_or_else(|error| {
            panic!("{} unexpectedly refused: {error:?}", vector.name)
        });
        assert_eq!(
            plan.source_postimage, vector.expected.content,
            "{}",
            vector.name
        );
        assert_eq!(plan.removed_pomodoro_links, 0, "{}", vector.name);
        assert_eq!(
            plan.cancel_log == CancelLogOutcome::Prepended,
            vector.expected.logged_count == 1,
            "{}",
            vector.name
        );
        assert_eq!(
            usize::from(plan.verdict.is_cancelled()),
            vector.expected.cancelled_count,
            "{}",
            vector.name
        );
        assert_eq!(
            usize::from(matches!(
                plan.verdict,
                CancelSourceVerdict::AlreadyClosed { .. }
            )),
            vector.expected.skipped_closed_count,
            "{}",
            vector.name
        );
        assert_eq!(
            usize::from(plan.cancel_log == CancelLogOutcome::Prepended),
            vector.expected.logged_count,
            "{}",
            vector.name
        );
        assert_eq!(vector.expected.created_log_count, 0, "{}", vector.name);
        assert_eq!(
            usize::from(plan.cancel_log == CancelLogOutcome::Prepended),
            vector.expected.fallback_logged_count,
            "{}",
            vector.name
        );
    }
}

#[test]
fn prune_vectors_use_task_identity_and_immediate_pomodoro_semantics() {
    let vectors = vectors();
    assert_eq!(vectors.prune.len(), 9);
    for vector in vectors.prune {
        let daily_path = PathBuf::from(&vector.daily_path);
        let index =
            NoteIndex::from_paths(vector.note_paths.iter().map(PathBuf::from));
        let targets = vector.targets.into_iter().collect::<BTreeSet<_>>();
        let result = prune::prune_daily(
            &daily_path,
            Some(&vector.content),
            &targets,
            &index,
        );
        assert_eq!(result.changed, vector.expected.changed, "{}", vector.name);
        assert_eq!(
            result.content.as_deref(),
            Some(vector.expected.content.as_str()),
            "{}",
            vector.name
        );
        assert_eq!(
            result.removed_link_count, vector.expected.removed_link_count,
            "{}",
            vector.name
        );
        assert_eq!(
            result.removed_bullet_count, vector.expected.removed_bullet_count,
            "{}",
            vector.name
        );
        assert_eq!(
            result.removed_entry_count, vector.expected.removed_entry_count,
            "{}",
            vector.name
        );
        assert_eq!(
            result.unresolved_count, vector.expected.unresolved_count,
            "{}",
            vector.name
        );
    }
}

#[test]
fn relation_rule_vectors_exercise_the_production_policy_verdict() {
    let settings = tasks_settings();
    let vectors = vectors();
    assert_eq!(vectors.rule.len(), 34);
    for vector in vectors.rule {
        let mut source_line =
            format!("- [{}] #task Source", vector.source_status);
        if vector.source_is_project {
            source_line.push_str(" #prj ^prj");
        } else {
            source_line.push_str(" ^source");
        }
        if vector.source_is_recurring {
            source_line.push_str(" [repeat:: weekly]");
        }
        let action = match vector.action.as_str() {
            "added" => RelationAction::Added,
            "relabeled" => RelationAction::Relabeled,
            "unchanged" => RelationAction::Unchanged,
            other => panic!("unknown action {other}"),
        };
        let direction = match vector.direction.as_str() {
            "links_to" => RelationDirection::LinksTo,
            "linked_from" => RelationDirection::LinkedFrom,
            other => panic!("unknown direction {other}"),
        };
        let trigger = match vector.trigger.as_str() {
            "explicit" => RelationTrigger::Explicit,
            "relabel_away" => RelationTrigger::RelabelAway,
            "unlink" => RelationTrigger::Unlink,
            "reconcile" => RelationTrigger::Reconcile,
            "rename" => RelationTrigger::Rename,
            "hand_edit" => RelationTrigger::HandEdit,
            "agent_edit" => RelationTrigger::AgentEdit,
            "mirror" => RelationTrigger::Mirror,
            "sync" => RelationTrigger::Sync,
            "config_change" => RelationTrigger::ConfigChange,
            other => panic!("unknown trigger {other}"),
        };
        let conflicts = vector
            .item_modifiers
            .iter()
            .filter_map(|modifier| ItemActionConflict::from_modifier(modifier))
            .collect();
        let verdict = relation_policy_verdict(RelationPolicyInput {
            configured_labels: &vector.configured,
            label: &vector.label,
            previous_label: vector.previous_label.as_deref(),
            action,
            direction,
            trigger,
            source_line: &source_line,
            tasks_settings: &settings,
            source_is_new: vector.source_is_new,
            conflicts,
        });
        assert_eq!(
            verdict.as_str(),
            vector.expected.verdict,
            "{}",
            vector.name
        );
        assert_eq!(
            verdict.cancels(),
            vector.expected.cancels,
            "{}",
            vector.name
        );
    }
}

#[test]
fn key_vectors_share_the_relation_label_key_function() {
    let vectors = vectors();
    assert_eq!(vectors.key.len(), 10);
    for vector in vectors.key {
        let key = relation_label_key(&vector.input);
        let matches = vector
            .configured
            .iter()
            .any(|label| relation_label_key(label) == key);
        assert_eq!(key, vector.expected.key, "{}", vector.name);
        assert_eq!(matches, vector.expected.matches, "{}", vector.name);
    }
}

#[test]
fn config_vectors_keep_cancel_policy_conservative_and_independent() {
    let vectors = vectors();
    assert_eq!(vectors.config.len(), 10);
    for vector in vectors.config {
        let config =
            parse_relations_config(&vector.yaml, Path::new("/config.yml"))
                .unwrap_or_else(|error| panic!("{}: {error:?}", vector.name));
        assert_eq!(
            config
                .cancel_labels
                .iter()
                .map(|label| label.key.clone())
                .collect::<Vec<_>>(),
            vector.expected.keys,
            "{}",
            vector.name
        );
        assert_eq!(
            config.warnings.len(),
            vector.expected.warning_count,
            "{}",
            vector.name
        );
        if let Some(expected) = vector.expected.linked_from_lines {
            assert_eq!(config.linked_from_lines, expected, "{}", vector.name);
        }
    }
}

#[test]
fn yaml_mapping_entry_is_rejected_without_dropping_valid_peers() {
    let config = parse_relations_config(
        "relations:\n  cancel_labels:\n    - duplicate of\n    - duplicate of: true\n",
        Path::new("/config.yml"),
    )
    .expect("a bad entry is not a config error");
    assert_eq!(
        config
            .cancel_labels
            .iter()
            .map(|label| label.key.as_str())
            .collect::<Vec<_>>(),
        ["duplicate of"]
    );
    assert_eq!(config.warnings.len(), 1);
}

#[test]
fn new_york_midnight_date_is_supplied_to_the_pure_engine() {
    let temp = tempfile::tempdir().expect("scratch config");
    let config_path = temp.path().join("config.yml");
    std::fs::write(&config_path, "timezone: America/New_York\n")
        .expect("write scratch config");
    let settings = tasks_settings();
    let index = NoteIndex::from_paths([PathBuf::from("cash.md")]);
    for (clock, expected_date) in [
        ("2026-10-28 23:59:59", "2026-10-28"),
        ("2026-10-29 00:00:00", "2026-10-29"),
    ] {
        let _guard = TestEnvGuard::set(&[
            ("BOB_CONFIG_FILE", Some(config_path.as_os_str())),
            ("BOB_NOW", Some(OsStr::new(clock))),
            ("DATE", None),
        ]);
        let today = crate::native::env::current_datetime().date();
        assert_eq!(today.to_string(), expected_date);
        let source = "- [ ] #task Sell stocks ^sell-stocks";
        let plan = plan_task_cancel(TaskCancelInput {
            source_path: PathBuf::from("cash.md"),
            source_block_id: Some("sell-stocks".to_string()),
            source_line_index: 0,
            source_text: source.to_string(),
            source_is_new: false,
            ordered_reasons: Vec::new(),
            today,
            note_index: &index,
            tasks_settings: &settings,
            note_settings: &settings,
            daily_path: PathBuf::from("2026/20261028.md"),
            daily_state: DailyReadState::Absent,
            dependency_snapshots: vec![(
                PathBuf::from("cash.md"),
                source.to_string(),
            )],
        })
        .expect("cancel plans with explicit Bob day");
        assert!(plan
            .source_postimage
            .contains(&format!("[cancelled:: {expected_date}]")));
    }
}

#[test]
fn source_cancel_preserves_authored_theme_field_bytes() {
    let source = "- [ ] #task Sell stocks (theme::  WRITE) ^sell-stocks\n";
    let edit = line::edit_source(source, 0, date("2026-10-22"), &[]);
    assert_eq!(
        edit.content,
        "- [-] #task Sell stocks (theme::  WRITE) [cancelled:: 2026-10-22] ^sell-stocks\n"
    );
    assert!(edit.content.contains("(theme::  WRITE)"));
    assert_eq!(edit.content.matches("(theme::  WRITE)").count(), 1);
}

#[test]
fn source_cancel_preserves_canonical_type_field_bytes() {
    let source = "- [ ] #task Sell stocks [type:: WRITE] ^sell-stocks\n";
    let edit = line::edit_source(source, 0, date("2026-10-22"), &[]);
    assert_eq!(
        edit.content,
        "- [-] #task Sell stocks [type:: WRITE] [cancelled:: 2026-10-22] ^sell-stocks\n"
    );
    assert!(edit.content.contains("[type:: WRITE]"));
}

#[test]
fn source_cancel_preserves_authored_root_field_bytes() {
    let source = "- [ ] #task Sell stocks (root::  sase) ^sell-stocks\n";
    let edit = line::edit_source(source, 0, date("2026-10-22"), &[]);
    assert_eq!(
        edit.content,
        "- [-] #task Sell stocks (root::  sase) [cancelled:: 2026-10-22] ^sell-stocks\n"
    );
    assert!(edit.content.contains("(root::  sase)"));
    assert_eq!(edit.content.matches("(root::  sase)").count(), 1);
}

#[test]
fn source_cancel_log_preserves_crlf_and_final_newline_state() {
    let source = "- [ ] #task T ^t\r\n  - ❌ **CANCEL LOG**\r\n    - *2026-10-01* — older";
    let edit = line::edit_source(
        source,
        0,
        date("2026-10-22"),
        &["🪢 duplicate of [[target#^t]]".into()],
    );
    assert_eq!(
        edit.content,
        "- [-] #task T [cancelled:: 2026-10-22] ^t\r\n  - ❌ **CANCEL LOG**\r\n    - *2026-10-22* — 🪢 duplicate of [[target#^t]]\r\n    - *2026-10-01* — older"
    );
    assert_eq!(edit.log, CancelLogOutcome::Prepended);
}

#[test]
fn cancelled_dependency_recovers_with_existing_rules_without_offers() {
    let settings = tasks_settings();
    let source_path = PathBuf::from("source.md");
    let dependent_path = PathBuf::from("dependent.md");
    let daily_path = PathBuf::from("2026/20261022.md");
    let source = "- [ ] #task Root [id:: root-id] ^root\n";
    let dependent = concat!(
        "- [?] #task Waiting [fresh:: 2026-10-01] ",
        "[dependsOn:: root-id] [id:: waiter] ^waiter\n",
    );
    let index = NoteIndex::from_paths([
        source_path.clone(),
        dependent_path.clone(),
        daily_path.clone(),
    ]);
    let plan = plan_task_cancel(TaskCancelInput {
        source_path: source_path.clone(),
        source_block_id: Some("root".into()),
        source_line_index: 0,
        source_text: source.into(),
        source_is_new: false,
        ordered_reasons: Vec::new(),
        today: date("2026-10-22"),
        note_index: &index,
        tasks_settings: &settings,
        note_settings: &settings,
        daily_path: daily_path.clone(),
        daily_state: DailyReadState::Absent,
        dependency_snapshots: vec![
            (source_path.clone(), source.into()),
            (dependent_path.clone(), dependent.into()),
        ],
    })
    .expect("eligible task cancels");

    assert!(plan.source_postimage.contains("- [-] #task Root"));
    assert_eq!(plan.daily_postimage, None);
    assert_eq!(plan.recovered.len(), 1);
    assert_eq!(plan.recovered[0].block_id.as_deref(), Some("waiter"));
    let dependent_postimage = plan
        .postimages
        .get(&dependent_path)
        .expect("dependent recovery is staged");
    assert!(dependent_postimage.contains("- [ ] #task Waiting"));
    assert!(dependent_postimage.contains("[unblocked:: 2026-10-22]"));
    assert!(!dependent_postimage.contains("#prj_is_blocked"));
    assert!(!dependent_postimage.contains("successor"));

    let source_postimage = plan.source_postimage.clone();
    let recovery = recovery::recover_cancelled_source(
        &TaskIdentity {
            path: source_path.clone(),
            block_id: "root".into(),
        },
        0,
        &source_postimage,
        &[
            (source_path, source_postimage.clone()),
            (dependent_path, dependent.into()),
        ],
        &daily_path,
        None,
        date("2026-10-22"),
        &settings,
        &settings,
        &index,
    );
    assert_eq!(recovery.successor.unblocked.len(), 1);
    assert!(recovery.successor.unblocked[0].offer.is_none());
    assert!(recovery.successor.unblocked[0].link.is_none());
    assert!(recovery.successor.new_day_text.is_none());
}

#[test]
fn other_open_prerequisite_and_future_schedule_keep_dependents_blocked() {
    let settings = tasks_settings();
    let source_path = PathBuf::from("source.md");
    let dependent_path = PathBuf::from("dependent.md");
    let daily_path = PathBuf::from("2026/20261022.md");
    let source = "- [ ] #task Root [id:: root-id] ^root\n";
    let other = "- [ ] #task Other open prerequisite [id:: other-id] ^other\n";
    let dependent = concat!(
        "- [?] #task Waiting on both [dependsOn:: root-id, other-id] ",
        "[id:: both] ^both\n",
        "- [?] #task Waiting for date [dependsOn:: root-id] ",
        "[scheduled:: 2026-10-23] [id:: future] ^future\n",
    );
    let index = NoteIndex::from_paths([
        source_path.clone(),
        dependent_path.clone(),
        daily_path.clone(),
    ]);
    let plan = plan_task_cancel(TaskCancelInput {
        source_path: source_path.clone(),
        source_block_id: Some("root".into()),
        source_line_index: 0,
        source_text: source.into(),
        source_is_new: false,
        ordered_reasons: Vec::new(),
        today: date("2026-10-22"),
        note_index: &index,
        tasks_settings: &settings,
        note_settings: &settings,
        daily_path,
        daily_state: DailyReadState::Absent,
        dependency_snapshots: vec![
            (source_path, source.into()),
            (PathBuf::from("other.md"), other.into()),
            (dependent_path.clone(), dependent.into()),
        ],
    })
    .expect("eligible task cancels");
    assert!(plan.recovered.is_empty());
    assert!(!plan.postimages.contains_key(&dependent_path));
}

#[test]
fn another_project_dependency_keeps_a_resident_blocked() {
    let settings = tasks_settings();
    let source_path = PathBuf::from("source.md");
    let project_path = PathBuf::from("project.md");
    let daily_path = PathBuf::from("2026/20261022.md");
    let source = "- [ ] #task Root [id:: root-id] ^root\n";
    let other = "- [ ] #task Other [id:: other-id] ^other\n";
    let project = format!(
        "---\ntype: [[project]]\nstatus: wip\n---\n{}{}",
        "- [?] #task Project [dependsOn:: other-id] [id:: project-id] ^prj\n",
        "- [?] #task Resident [fresh:: 2026-10-01] #prj_is_blocked \
         [dependsOn:: root-id] [id:: resident] ^resident\n",
    );
    let index = NoteIndex::from_paths([
        source_path.clone(),
        project_path.clone(),
        PathBuf::from("other.md"),
        daily_path.clone(),
    ]);
    let plan = plan_task_cancel(TaskCancelInput {
        source_path: source_path.clone(),
        source_block_id: Some("root".into()),
        source_line_index: 0,
        source_text: source.into(),
        source_is_new: false,
        ordered_reasons: Vec::new(),
        today: date("2026-10-22"),
        note_index: &index,
        tasks_settings: &settings,
        note_settings: &settings,
        daily_path,
        daily_state: DailyReadState::Absent,
        dependency_snapshots: vec![
            (source_path, source.into()),
            (PathBuf::from("other.md"), other.into()),
            (project_path.clone(), project.clone()),
        ],
    })
    .expect("eligible task cancels");
    assert!(!plan
        .recovered
        .iter()
        .any(|task| task.block_id.as_deref() == Some("resident")));
    let project_after = plan.postimages.get(&project_path).unwrap_or(&project);
    assert!(project_after.contains("- [?] #task Resident"));
    assert!(project_after.contains("#prj_is_blocked"));
}

#[test]
fn inherited_project_blocking_keeps_a_dependent_blocked() {
    let settings = tasks_settings();
    let source_path = PathBuf::from("source.md");
    let parent_path = PathBuf::from("grand.md");
    let child_path = PathBuf::from("child.md");
    let daily_path = PathBuf::from("2026/20261022.md");
    let source = "- [ ] #task Root [id:: root-id] ^root\n";
    let parent = concat!(
        "---\ntype: [[project]]\nstatus: wip\n---\n",
        "- [?] #task Parent [dependsOn:: still-open] ",
        "#prj_is_blocked ^prj\n",
    );
    let child = concat!(
        "---\ntype: [[project]]\nstatus: wip\nparent: \"[[grand]]\"\n---\n",
        "- [?] #task Child [dependsOn:: root-id] #prj_is_blocked ^prj\n",
        "- [?] #task Resident [dependsOn:: root-id] #prj_is_blocked ^resident\n",
    );
    let index = NoteIndex::from_paths([
        source_path.clone(),
        parent_path.clone(),
        child_path.clone(),
        daily_path.clone(),
    ]);
    let plan = plan_task_cancel(TaskCancelInput {
        source_path: source_path.clone(),
        source_block_id: Some("root".into()),
        source_line_index: 0,
        source_text: source.into(),
        source_is_new: false,
        ordered_reasons: Vec::new(),
        today: date("2026-10-22"),
        note_index: &index,
        tasks_settings: &settings,
        note_settings: &settings,
        daily_path,
        daily_state: DailyReadState::Absent,
        dependency_snapshots: vec![
            (source_path, source.into()),
            (parent_path, parent.into()),
            (child_path.clone(), child.into()),
        ],
    })
    .expect("eligible task cancels");
    assert!(plan.recovered.is_empty());
    assert!(!plan.postimages.contains_key(&child_path));
}

#[test]
fn cancelling_a_project_does_not_reconcile_its_descendant_tree() {
    let settings = tasks_settings();
    let source_path = PathBuf::from("project.md");
    let child_path = PathBuf::from("child.md");
    let daily_path = PathBuf::from("2026/20261022.md");
    let source = concat!(
        "---\ntype: [[project]]\nstatus: wip\n---\n",
        "- [ ] #task Cancel project ^prj\n",
        "- [?] #task Resident [fresh:: 2026-10-01] ",
        "#prj_is_blocked ^resident\n",
    );
    let child = concat!(
        "---\ntype: [[project]]\nstatus: wip\nparent: \"[[project]]\"\n---\n",
        "- [?] #task Child #prj_is_blocked ^prj\n",
        "- [?] #task Child resident #prj_is_blocked ^child-resident\n",
    );
    let index = NoteIndex::from_paths([
        source_path.clone(),
        child_path.clone(),
        daily_path.clone(),
    ]);
    let plan = plan_task_cancel(TaskCancelInput {
        source_path: source_path.clone(),
        source_block_id: Some("prj".into()),
        source_line_index: 4,
        source_text: source.into(),
        source_is_new: false,
        ordered_reasons: Vec::new(),
        today: date("2026-10-22"),
        note_index: &index,
        tasks_settings: &settings,
        note_settings: &settings,
        daily_path,
        daily_state: DailyReadState::Absent,
        dependency_snapshots: vec![
            (source_path, source.into()),
            (child_path.clone(), child.into()),
        ],
    })
    .expect("project task cancels");
    assert!(!plan.postimages.contains_key(&child_path));
    assert!(!plan.recovered.iter().any(|task| {
        task.path == child_path
            || task.block_id.as_deref() == Some("child-resident")
    }));
}

#[test]
fn cancelled_project_without_id_recovers_only_eligible_residents() {
    let settings = tasks_settings();
    let source_path = PathBuf::from("project.md");
    let other_path = PathBuf::from("other.md");
    let daily_path = PathBuf::from("2026/20261022.md");
    let source = format!(
        "---\ntype: [[project]]\nstatus: wip\n---\n{}{}{}{}",
        "- [ ] #task Cancel this project ^prj\n",
        "- [?] #task Eligible [fresh:: 2026-10-01] #prj_is_blocked ^eligible\n",
        "- [?] #task Own dependency #prj_is_blocked [dependsOn:: keep-open] ^own\n",
        "- [?] #task Future #prj_is_blocked [scheduled:: 2026-10-23] ^future\n",
    );
    let other = "- [ ] #task Keep open [id:: keep-open] ^keep-open\n";
    let index = NoteIndex::from_paths([
        source_path.clone(),
        other_path.clone(),
        daily_path.clone(),
    ]);
    let plan = plan_task_cancel(TaskCancelInput {
        source_path: source_path.clone(),
        source_block_id: Some("prj".into()),
        source_line_index: 4,
        source_text: source.clone(),
        source_is_new: false,
        ordered_reasons: Vec::new(),
        today: date("2026-10-22"),
        note_index: &index,
        tasks_settings: &settings,
        note_settings: &settings,
        daily_path,
        daily_state: DailyReadState::Absent,
        dependency_snapshots: vec![
            (source_path.clone(), source),
            (other_path, other.into()),
        ],
    })
    .expect("project task cancels without an id");
    assert_eq!(plan.recovered.len(), 1);
    assert_eq!(plan.recovered[0].block_id.as_deref(), Some("eligible"));
    let project_after = plan
        .postimages
        .get(&source_path)
        .expect("project residents were recovered");
    assert!(project_after.contains("- [-] #task Cancel this project"));
    assert!(project_after.contains("- [ ] #task Eligible"));
    assert!(project_after.contains("[unblocked:: 2026-10-22]"));
    assert!(!project_after
        .contains("Eligible [fresh:: 2026-10-01] #prj_is_blocked"));
    assert!(project_after.contains("- [?] #task Own dependency"));
    assert!(project_after.contains("- [?] #task Future"));
    assert!(!project_after.contains("Own dependency #prj_is_blocked"));
    assert!(!project_after.contains("Future #prj_is_blocked"));
    assert!(!project_after
        .lines()
        .any(|line| line.contains("^prj") && line.contains("[id::")));
}

#[test]
fn failed_daily_read_propagates_without_a_partial_plan() {
    let settings = tasks_settings();
    let index = NoteIndex::from_paths([PathBuf::from("source.md")]);
    let result = plan_task_cancel(TaskCancelInput {
        source_path: PathBuf::from("source.md"),
        source_block_id: Some("task".into()),
        source_line_index: 0,
        source_text: "- [ ] #task T ^task".into(),
        source_is_new: false,
        ordered_reasons: Vec::new(),
        today: date("2026-10-22"),
        note_index: &index,
        tasks_settings: &settings,
        note_settings: &settings,
        daily_path: PathBuf::from("2026/20261022.md"),
        daily_state: DailyReadState::Failed("permission denied".into()),
        dependency_snapshots: Vec::new(),
    });
    assert_eq!(
        result,
        Err(TaskCancelError::DailyRead("permission denied".into()))
    );
}

#[test]
fn source_and_daily_overlap_produce_one_coherent_postimage() {
    let settings = tasks_settings();
    let daily_path = PathBuf::from("2026/20261022.md");
    let source = concat!(
        "- [ ] #task Self-linked ^self\n",
        "## Pomodoros\n",
        "- [ ] Focus (0900-0930)\n",
        "  - [[2026/20261022#^self]]\n",
    );
    let index = NoteIndex::from_paths([daily_path.clone()]);
    let plan = plan_task_cancel(TaskCancelInput {
        source_path: daily_path.clone(),
        source_block_id: Some("self".into()),
        source_line_index: 0,
        source_text: source.into(),
        source_is_new: false,
        ordered_reasons: Vec::new(),
        today: date("2026-10-22"),
        note_index: &index,
        tasks_settings: &settings,
        note_settings: &settings,
        daily_path: daily_path.clone(),
        daily_state: DailyReadState::Present(source.into()),
        dependency_snapshots: vec![(daily_path.clone(), source.into())],
    })
    .expect("overlapping source and daily snapshots plan");
    assert_eq!(plan.source_postimage, plan.daily_postimage.clone().unwrap());
    assert_eq!(plan.postimages.len(), 1);
    assert!(plan.postimages.contains_key(&daily_path));
    assert!(plan.source_postimage.contains("- [-] #task Self-linked"));
    assert!(!plan.source_postimage.contains("[[2026/20261022#^self]]"));
    assert!(plan.source_postimage.contains("- [ ] Focus (0900-0930)"));
}

#[test]
fn a_second_cancel_attempt_is_closed_and_does_not_duplicate_the_log() {
    let settings = tasks_settings();
    let path = PathBuf::from("source.md");
    let index = NoteIndex::from_paths([path.clone()]);
    let source = "- [ ] #task T ^task\n  - ❌ **CANCEL LOG**\n";
    let first = plan_task_cancel(TaskCancelInput {
        source_path: path.clone(),
        source_block_id: Some("task".into()),
        source_line_index: 0,
        source_text: source.into(),
        source_is_new: false,
        ordered_reasons: vec!["🪢 duplicate of [[target#^target]]".into()],
        today: date("2026-10-22"),
        note_index: &index,
        tasks_settings: &settings,
        note_settings: &settings,
        daily_path: PathBuf::from("2026/20261022.md"),
        daily_state: DailyReadState::Absent,
        dependency_snapshots: vec![(path.clone(), source.into())],
    })
    .expect("first cancel plans");
    assert_eq!(first.cancel_log, CancelLogOutcome::Prepended);
    assert_eq!(first.source_postimage.matches("duplicate of").count(), 1);

    let second = plan_task_cancel(TaskCancelInput {
        source_path: path.clone(),
        source_block_id: Some("task".into()),
        source_line_index: 0,
        source_text: first.source_postimage.clone(),
        source_is_new: false,
        ordered_reasons: vec!["🪢 duplicate of [[target#^target]]".into()],
        today: date("2026-10-22"),
        note_index: &index,
        tasks_settings: &settings,
        note_settings: &settings,
        daily_path: PathBuf::from("2026/20261022.md"),
        daily_state: DailyReadState::Absent,
        dependency_snapshots: vec![(path, first.source_postimage.clone())],
    })
    .expect("closed task is a no-op");
    assert_eq!(second.source_postimage, first.source_postimage);
    assert!(second.postimages.is_empty());
    assert_eq!(second.cancel_log, CancelLogOutcome::None);
}
