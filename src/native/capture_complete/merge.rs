//! Merge-label cue for `capture-complete` relation labels
//! (`docs/task-relations.md` §21 ML8).

use std::path::Path;

use super::{
    cancel::{resolve_source, resolve_target_text},
    model::{RelationMerge, RelationMergeSource},
};
use crate::native::{
    capture_language, config, relation_labels::relation_label_key,
    task_complete::is_recurring_task_line,
    task_relations::label as relation_label,
};

pub(super) struct MergeCuePlan {
    pub(super) response: RelationMerge,
    pub(super) row_disable: Option<String>,
}

/// Resolve only the token source. Completion never scans the vault; the
/// relation target read is used only for Bob-authored refusal hints.
pub(super) fn plan_relation_merge(
    bob_dir: &Path,
    raw_text: &str,
    separator: usize,
    editor_item: Option<&capture_language::EditorItemParse>,
    query: &str,
    policy: &config::RelationsConfig,
) -> Option<MergeCuePlan> {
    if policy.merge_labels.is_empty() {
        return None;
    }
    let (_, direction, _) =
        capture_language::relation_label_token_at(raw_text, separator)?;
    let source =
        resolve_source(bob_dir, raw_text, separator, direction, editor_item)?;
    let target_text = resolve_target_text(
        bob_dir,
        raw_text,
        separator,
        direction,
        editor_item,
    );
    let (verdict, message) = if source.is_new {
        (
            "new_task",
            format!(
                "“{}” is new — it would be born merged · to fold “{}”, use ~~",
                source.text,
                target_text.as_deref().unwrap_or("the other task")
            ),
        )
    } else if is_recurring_task_line(&source.raw_line) {
        (
            "recurring",
            format!("“{}” repeats — merge it by hand first", source.text),
        )
    } else if source.status_type.is_terminal() {
        ("already_closed", "Already closed · link only".to_string())
    } else {
        (
            "merged",
            format!(
                "⤵ Merges “{}” into “{}” · notes move and the source is cancelled",
                source.text,
                target_text.as_deref().unwrap_or("the other task")
            ),
        )
    };
    let hint = inverse_hint(query, target_text.as_deref(), policy);
    let row_disable =
        matches!(verdict, "recurring" | "new_task").then_some(message.clone());
    Some(MergeCuePlan {
        response: RelationMerge {
            source: RelationMergeSource {
                path: source.path,
                block_id: source.block_id,
                text: source.text,
            },
            verdict,
            message,
            hint,
        },
        row_disable,
    })
}

fn inverse_hint(
    query: &str,
    target_text: Option<&str>,
    policy: &config::RelationsConfig,
) -> Option<String> {
    let typed = relation_label::normalize_relation_label(query).ok()?;
    let key = relation_label_key(&typed);
    let (merge, effect) = policy.label_for_inverse(&key)?;
    (effect == config::relations::LabelEffect::Merge).then(|| {
        format!(
            "“{typed}” never merges · to fold “{}”, use ~~ with “{merge}”",
            target_text.unwrap_or("the other task")
        )
    })
}
