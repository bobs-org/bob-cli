//! Successor-label authoring indication for `capture-complete`.

use std::path::{Path, PathBuf};

use super::{
    cancel::{self, ResolvedSource},
    model::{RelationSuccessor, RelationSuccessorTask},
};
use crate::native::{
    capture, capture_active_tasks, capture_language, config,
    config::relations::LabelEffect,
    note_tasks::TaskStatusType,
    pomodoro,
    relation_labels::relation_label_key,
    task_complete::relation_successors::copy::{self, ReleaseState},
    task_relations::label::normalize_relation_label,
};

pub(super) fn plan_relation_successor(
    bob_dir: &Path,
    raw_text: &str,
    separator: usize,
    editor_item: Option<&capture_language::EditorItemParse>,
    query: &str,
    policy: &config::RelationsConfig,
) -> Option<RelationSuccessor> {
    let typed = normalize_relation_label(query).ok()?;
    let key = relation_label_key(&typed);
    let direct = policy.is_successor_label(&key);
    let inverse = policy
        .label_for_inverse(&key)
        .filter(|(_, effect)| *effect == LabelEffect::Successor);
    if !direct && inverse.is_none() {
        return None;
    }
    let (_, direction, _) =
        capture_language::relation_label_token_at(raw_text, separator)?;
    let source = cancel::resolve_source(
        bob_dir,
        raw_text,
        separator,
        direction,
        editor_item,
    )?;
    let target =
        resolve_target(bob_dir, raw_text, separator, direction, editor_item)?;

    let (verdict, message, hint) = if direct {
        let state = release_state(bob_dir, &target);
        let (verdict, message) = match state {
            Some(ReleaseState::Parked) => (
                "already_parked",
                copy::release_caveat(
                    ReleaseState::Parked,
                    &target.text,
                    Some(&source.text),
                ),
            ),
            Some(ReleaseState::Closed) => (
                "already_closed",
                copy::release_caveat(ReleaseState::Closed, &target.text, None),
            ),
            None => (
                "offers",
                copy::capture_complete_verdict(&source.text, &target.text),
            ),
        };
        (Some(verdict), Some(message), None)
    } else {
        let (label, _) = inverse.expect("inverse successor label checked");
        (
            None,
            None,
            Some(copy::inverse_hint(&typed, &source.text, label)),
        )
    };

    Some(RelationSuccessor {
        source: endpoint(&source),
        target: endpoint(&target),
        verdict,
        message,
        hint,
    })
}

fn resolve_target(
    bob_dir: &Path,
    raw_text: &str,
    separator: usize,
    direction: capture_language::RelationDirection,
    editor_item: Option<&capture_language::EditorItemParse>,
) -> Option<ResolvedSource> {
    let item = editor_item?;
    match direction {
        capture_language::RelationDirection::To => {
            let (_, note, block_id) =
                cancel::token_target(raw_text, separator)?;
            cancel::read_task_source(bob_dir, &note, &block_id, false)
        }
        capture_language::RelationDirection::From => {
            let owner = item.dependency_target.as_ref()?;
            match owner.kind {
                capture_language::DependencyTargetKind::NewTask => {
                    let route = owner
                        .route
                        .as_deref()
                        .unwrap_or_else(|| capture::inbox_route());
                    Some(ResolvedSource {
                        path: capture::route_label(route),
                        block_id: None,
                        text: item.body.clone(),
                        status_symbol: ' ',
                        status_name: cancel::status_name_for(bob_dir, ' '),
                        status_type: TaskStatusType::Todo,
                        project_open_tasks: None,
                        raw_line: String::new(),
                        is_new: true,
                    })
                }
                capture_language::DependencyTargetKind::ExistingTask => {
                    cancel::read_task_source(
                        bob_dir,
                        owner.route.as_deref()?,
                        owner.block_id.as_deref()?,
                        false,
                    )
                }
            }
        }
    }
}

/// `Some(Parked)` requires a readable current ledger so an unavailable
/// day file cannot turn an uncertain target into a false caveat.
fn release_state(
    bob_dir: &Path,
    target: &ResolvedSource,
) -> Option<ReleaseState> {
    if target.status_type.is_terminal()
        || crate::native::done_archive::is_archive_path(Path::new(&target.path))
    {
        return Some(ReleaseState::Closed);
    }
    if target.status_symbol != '/' {
        return None;
    }
    let mut warnings = Vec::new();
    let ledger = capture_active_tasks::read_ledger(
        &pomodoro::day_file_for(bob_dir),
        &mut warnings,
    );
    if !warnings.is_empty() {
        return None;
    }
    let block_id = target.block_id.as_deref()?;
    let route = task_route(&target.path);
    if ledger.position(&(route, block_id.to_string())).is_none() {
        Some(ReleaseState::Parked)
    } else {
        None
    }
}

fn endpoint(task: &ResolvedSource) -> RelationSuccessorTask {
    RelationSuccessorTask {
        path: task.path.clone(),
        block_id: task.block_id.clone(),
        text: task.text.clone(),
        status_symbol: task.status_symbol,
        status_name: task.status_name.clone(),
    }
}

fn task_route(path: &str) -> String {
    let mut route = PathBuf::from(path);
    route.set_extension("");
    route.to_string_lossy().replace('\\', "/")
}
