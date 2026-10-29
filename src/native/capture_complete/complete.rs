//! Complete-label cue for `capture-complete` relation labels
//! (`docs/task-relations.md` §18 CP5–CP7).

use std::path::Path;

use super::{
    cancel::{resolve_source, resolve_target_text},
    model::{RelationComplete, RelationCompleteSource},
};
use crate::native::{
    capture_language, config,
    config::relations::CloseEffect,
    note_tasks,
    relation_labels::relation_label_key,
    task_complete::verdicts::{complete_verdict, CompleteSourceVerdict},
    task_relations::label as relation_label,
};

/// The complete consequence for the `relation_label` token at `separator`,
/// plus the row-disable reason when completing through this gesture would
/// be refused (`recurring` / `new_task`). Source resolution is limited to
/// the item owner or the `~~` token's task; it never scans the vault.
pub(super) struct CompletePlan {
    pub(super) response: RelationComplete,
    pub(super) row_disable: Option<String>,
}

pub(super) fn plan_relation_complete(
    bob_dir: &Path,
    raw_text: &str,
    separator: usize,
    editor_item: Option<&capture_language::EditorItemParse>,
    query: &str,
    policy: &config::RelationsConfig,
) -> Option<CompletePlan> {
    if policy.complete_labels.is_empty() {
        return None;
    }
    let (_, direction, _) =
        capture_language::relation_label_token_at(raw_text, separator)?;
    let source =
        resolve_source(bob_dir, raw_text, separator, direction, editor_item)?;
    let settings = note_tasks::read_settings(bob_dir);
    let verdict = complete_verdict(&source.raw_line, &settings, source.is_new);
    let target_text = resolve_target_text(
        bob_dir,
        raw_text,
        separator,
        direction,
        editor_item,
    );
    let (verdict_name, message) = match verdict {
        CompleteSourceVerdict::Completes {
            previous_status,
            project,
        } => {
            let mut message = format!(
                "✓ Completes “{}” · {} → Done",
                source.text,
                status_display(&source, previous_status)
            );
            if project {
                let open = source.project_open_tasks.unwrap_or(0);
                message.push_str(&format!(
                    " · its {open} open task{} stay open",
                    if open == 1 { "" } else { "s" }
                ));
            }
            ("completes", message)
        }
        CompleteSourceVerdict::AlreadyClosed { .. } => {
            let message = match source.status_symbol {
                'x' => "Already Done · link only".to_string(),
                '-' => "Stays Cancelled · link only".to_string(),
                _ => format!("Stays {} · link only", source.status_name),
            };
            ("already_closed", message)
        }
        CompleteSourceVerdict::Recurring => (
            "recurring",
            format!(
                "“{}” repeats — complete it in Obsidian so Tasks writes the next one",
                source.text
            ),
        ),
        CompleteSourceVerdict::NewTask => (
            "new_task",
            format!(
                "“{}” is new — it would be born done · to finish “{}”, use ~~",
                source.text,
                target_text.as_deref().unwrap_or("the other task")
            ),
        ),
    };
    let hint = inverse_hint(query, target_text.as_deref(), policy);
    let row_disable = matches!(
        verdict,
        CompleteSourceVerdict::Recurring | CompleteSourceVerdict::NewTask
    )
    .then_some(message.clone());
    Some(CompletePlan {
        response: RelationComplete {
            source: RelationCompleteSource {
                path: source.path,
                block_id: source.block_id,
                text: source.text,
                status_symbol: source.status_symbol,
                status_name: source.status_name,
                project_open_tasks: source.project_open_tasks,
            },
            verdict: verdict_name,
            message,
            hint,
        },
        row_disable,
    })
}

fn status_display(
    source: &super::cancel::ResolvedSource,
    previous: char,
) -> String {
    if source.status_symbol == previous {
        source.status_name.clone()
    } else {
        previous.to_string()
    }
}

/// `“{typed}” never completes · to finish “{T}”, use ~~ with “{complete}”`
/// when the typed label is the inverse of a configured complete label.
fn inverse_hint(
    query: &str,
    target_text: Option<&str>,
    policy: &config::RelationsConfig,
) -> Option<String> {
    let typed = relation_label::normalize_relation_label(query).ok()?;
    let key = relation_label_key(&typed);
    let (complete, effect) = policy.close_label_for_inverse(&key)?;
    (effect == CloseEffect::Complete).then(|| {
        format!(
            "“{typed}” never completes · to finish “{}”, use ~~ with “{complete}”",
            target_text.unwrap_or("the other task")
        )
    })
}
