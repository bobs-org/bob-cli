//! Shared-vector coverage for complete labels
//! (`tests/fixtures/relation_complete/vectors.json`).

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde::Deserialize;

use super::super::tree::{
    add_or_replace_completion_field, line_text_at, RootPolicy,
};
use super::super::{complete_task_tree, is_recurring_task_line};
use super::MemoryVault;
use crate::native::{
    capture_task_toggle::set_task_line_status,
    config::relations::{parse_relations_config, CloseEffect},
    relation_labels::relation_label_key,
    task_cancel::verdicts::{
        relation_policy_verdict, ItemActionConflict, RelationAction,
        RelationDirection, RelationPolicyInput, RelationTrigger,
    },
    task_complete::verdicts::relation_complete_policy_verdict,
    task_status_hooks::{TaskStatusDefinition, TaskStatusType, TasksSettings},
};

#[derive(Debug, Deserialize)]
struct VectorsFile {
    done: Vec<DoneVector>,
    rule: Vec<RuleVector>,
    key: Vec<KeyVector>,
    config: Vec<ConfigVector>,
}

#[derive(Debug, Deserialize)]
struct RuleVector {
    name: String,
    configured_cancel: Vec<String>,
    configured_complete: Vec<String>,
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
    expected: RuleExpected,
}

#[derive(Debug, Deserialize)]
struct RuleExpected {
    verdict: String,
    effect: String,
    refusal: Option<String>,
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
    cancel_keys: Vec<String>,
    complete_keys: Vec<String>,
    warning_count: usize,
    linked_from_lines: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct DoneVector {
    name: String,
    line: String,
    date: String,
    #[serde(default)]
    eol: Option<String>,
    expected: DoneExpected,
}

#[derive(Debug, Deserialize)]
struct DoneExpected {
    ok: bool,
    #[serde(default)]
    unchanged: bool,
    #[serde(default)]
    reason: Option<String>,
    line: String,
}

fn vectors() -> VectorsFile {
    serde_json::from_str(include_str!(
        "../../../../tests/fixtures/relation_complete/vectors.json"
    ))
    .expect("typed relation-complete vectors parse")
}

fn trailing_block_id(line: &str) -> Option<String> {
    let trimmed = line.trim_end_matches([' ', '\t', '\r']);
    let start = trimmed.rfind('^')?;
    (start > 0 && trimmed.as_bytes()[start - 1].is_ascii_whitespace())
        .then(|| trimmed[start + 1..].to_string())
        .filter(|id| !id.is_empty())
}

fn root_rewrite(line: &str, date: &str) -> String {
    let Some(updated) = set_task_line_status(line, 'x') else {
        return line.to_string();
    };
    add_or_replace_completion_field(&updated, date)
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

fn policy_yaml(cancel: &[String], complete: &[String]) -> String {
    serde_yaml::to_string(&serde_json::json!({
        "relations": {
            "cancel_labels": cancel,
            "complete_labels": complete,
        }
    }))
    .expect("serialize policy fixture")
}

fn relation_action(raw: &str) -> RelationAction {
    match raw {
        "added" => RelationAction::Added,
        "relabeled" => RelationAction::Relabeled,
        "unchanged" => RelationAction::Unchanged,
        other => panic!("unknown relation action {other}"),
    }
}

fn relation_direction(raw: &str) -> RelationDirection {
    match raw {
        "links_to" => RelationDirection::LinksTo,
        "linked_from" => RelationDirection::LinkedFrom,
        other => panic!("unknown relation direction {other}"),
    }
}

fn relation_trigger(raw: &str) -> RelationTrigger {
    match raw {
        "explicit" => RelationTrigger::Explicit,
        "relabel_away" => RelationTrigger::RelabelAway,
        "unlink" => RelationTrigger::Unlink,
        "reconcile" => RelationTrigger::Reconcile,
        "rename" => RelationTrigger::Rename,
        "rename_reverse" => RelationTrigger::Rename,
        "hand_edit" => RelationTrigger::HandEdit,
        "agent_edit" => RelationTrigger::AgentEdit,
        "mirror" => RelationTrigger::Mirror,
        "sync" => RelationTrigger::Sync,
        "config_change" => RelationTrigger::ConfigChange,
        other => panic!("unknown relation trigger {other}"),
    }
}

#[test]
fn relation_complete_vector_sections_exist() {
    let vectors = vectors();
    assert!(!vectors.done.is_empty(), "done");
    assert_eq!(vectors.rule.len(), 39, "rule");
    assert_eq!(vectors.key.len(), 10, "key");
    assert_eq!(vectors.config.len(), 13, "config");
}

#[test]
fn complete_label_config_vectors_drive_the_production_parser() {
    let vectors = vectors();
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
            vector.expected.cancel_keys,
            "{}: cancel keys",
            vector.name
        );
        assert_eq!(
            config
                .complete_labels
                .iter()
                .map(|label| label.key.clone())
                .collect::<Vec<_>>(),
            vector.expected.complete_keys,
            "{}: complete keys",
            vector.name
        );
        assert_eq!(
            config.warnings.len(),
            vector.expected.warning_count,
            "{}: warning count",
            vector.name
        );
        if let Some(expected) = vector.expected.linked_from_lines {
            assert_eq!(
                config.linked_from_lines, expected,
                "{}: linked_from_lines",
                vector.name
            );
        }
    }
}

#[test]
fn complete_label_key_vectors_use_the_production_policy_api() {
    let vectors = vectors();
    for vector in vectors.key {
        let yaml = policy_yaml(&[], &vector.configured);
        let config = parse_relations_config(&yaml, Path::new("/config.yml"))
            .unwrap_or_else(|error| panic!("{}: {error:?}", vector.name));
        let key = relation_label_key(&vector.input);
        assert_eq!(key, vector.expected.key, "{}: key", vector.name);
        assert_eq!(
            config.is_complete_label(&key),
            vector.expected.matches,
            "{}: configured match",
            vector.name
        );
        assert_eq!(
            config.close_effect(&key),
            vector.expected.matches.then_some(CloseEffect::Complete),
            "{}: close effect",
            vector.name
        );
    }
}

#[test]
fn complete_label_rule_vectors_exercise_production_policy_verdicts() {
    let settings = tasks_settings();
    for vector in vectors().rule {
        let yaml =
            policy_yaml(&vector.configured_cancel, &vector.configured_complete);
        let config = parse_relations_config(&yaml, Path::new("/config.yml"))
            .unwrap_or_else(|error| panic!("{}: {error:?}", vector.name));
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
        let conflicts = vector
            .item_modifiers
            .iter()
            .filter_map(|modifier| ItemActionConflict::from_modifier(modifier))
            .collect();
        let close_effect_conflict = vector
            .item_modifiers
            .iter()
            .any(|modifier| modifier == "cancel_label");
        let (verdict, effect, refusal) = match config
            .close_effect(&relation_label_key(&vector.label))
        {
            Some(CloseEffect::Cancel) => {
                let configured = config
                    .cancel_labels
                    .iter()
                    .map(|label| label.name.clone())
                    .collect::<Vec<_>>();
                let verdict = relation_policy_verdict(RelationPolicyInput {
                    configured_labels: &configured,
                    label: &vector.label,
                    previous_label: vector.previous_label.as_deref(),
                    action: relation_action(&vector.action),
                    direction: relation_direction(&vector.direction),
                    trigger: relation_trigger(&vector.trigger),
                    source_line: &source_line,
                    tasks_settings: &settings,
                    source_is_new: vector.source_is_new,
                    conflicts,
                });
                let effect = if verdict.cancels() { "cancel" } else { "none" };
                (verdict.as_str(), effect, None)
            }
            effect => {
                let configured = config
                    .complete_labels
                    .iter()
                    .map(|label| label.name.clone())
                    .collect::<Vec<_>>();
                let verdict = relation_complete_policy_verdict(
                    RelationPolicyInput {
                        configured_labels: &configured,
                        label: &vector.label,
                        previous_label: vector.previous_label.as_deref(),
                        action: relation_action(&vector.action),
                        direction: relation_direction(&vector.direction),
                        trigger: relation_trigger(&vector.trigger),
                        source_line: &source_line,
                        tasks_settings: &settings,
                        source_is_new: vector.source_is_new,
                        conflicts,
                    },
                    close_effect_conflict,
                );
                assert_ne!(effect, Some(CloseEffect::Cancel));
                (verdict.as_str(), verdict.effect(), verdict.refusal())
            }
        };
        assert_eq!(
            verdict, vector.expected.verdict,
            "{}: verdict",
            vector.name
        );
        assert_eq!(effect, vector.expected.effect, "{}: effect", vector.name);
        assert_eq!(
            refusal,
            vector.expected.refusal.as_deref(),
            "{}: refusal",
            vector.name
        );
    }
}

#[test]
fn complete_task_tree_root_rewrite_matches_every_non_refused_done_case() {
    let vectors = vectors();
    for vector in vectors.done {
        if !vector.expected.ok {
            assert_eq!(
                vector.expected.reason.as_deref(),
                Some("recurring"),
                "{}",
                vector.name
            );
            assert!(is_recurring_task_line(&vector.line), "{}", vector.name);
            assert_eq!(vector.expected.line, vector.line, "{}", vector.name);
            let Some(block_id) = trailing_block_id(&vector.line) else {
                continue;
            };
            let mut vault = MemoryVault::new();
            vault.insert("note.md", &format!("{}\n", vector.line));
            let path = vault.absolute("note.md");
            let outcome = complete_task_tree(
                &vault,
                &path,
                &block_id,
                &vector.date,
                RootPolicy::Explicit,
            )
            .unwrap_or_else(|error| panic!("{}: {error}", vector.name));
            assert!(
                outcome.root.is_none(),
                "{}: recurring root stayed open",
                vector.name
            );
            assert!(
                !outcome.changed_files.contains_key(&path),
                "{}: recurring rewrite wrote nothing",
                vector.name
            );
            continue;
        }

        if vector.expected.unchanged {
            assert_eq!(vector.expected.line, vector.line, "{}", vector.name);
            let Some(block_id) = trailing_block_id(&vector.line) else {
                continue;
            };
            let mut vault = MemoryVault::new();
            vault.insert("note.md", &format!("{}\n", vector.line));
            let path = vault.absolute("note.md");
            let outcome = complete_task_tree(
                &vault,
                &path,
                &block_id,
                &vector.date,
                RootPolicy::Explicit,
            )
            .unwrap_or_else(|error| panic!("{}: {error}", vector.name));
            assert!(
                outcome.root.is_none(),
                "{}: already-closed root stayed closed",
                vector.name
            );
            assert!(
                !outcome.changed_files.contains_key(&path),
                "{}: already-closed rewrite wrote nothing",
                vector.name
            );
            continue;
        }

        let helper = root_rewrite(&vector.line, &vector.date);
        assert_eq!(
            helper, vector.expected.line,
            "{}: helper rewrite",
            vector.name
        );

        let Some(block_id) = trailing_block_id(&vector.line) else {
            continue;
        };
        let crlf = vector.eol.as_deref() == Some("crlf");
        let body = if crlf {
            format!("{}\r\n", vector.line)
        } else {
            format!("{}\n", vector.line)
        };
        let mut vault = MemoryVault::new();
        vault.insert("note.md", &body);
        let path = vault.absolute("note.md");
        let outcome = complete_task_tree(
            &vault,
            &path,
            &block_id,
            &vector.date,
            RootPolicy::Explicit,
        )
        .unwrap_or_else(|error| panic!("{}: {error}", vector.name));
        let rewritten = outcome
            .changed_files
            .get(&path)
            .unwrap_or_else(|| panic!("{}: expected a write", vector.name));
        let actual = line_text_at(rewritten, 0).unwrap_or_default();
        assert_eq!(actual, vector.expected.line, "{}", vector.name);
        if crlf {
            assert!(
                rewritten.contains("\r\n"),
                "{}: CRLF terminator preserved",
                vector.name
            );
        }
        assert!(outcome.root.is_some(), "{}: root closed", vector.name);
    }
}
