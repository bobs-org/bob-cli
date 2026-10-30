//! LINKED FROM courtesy writes for capture.
//!
//! Capture `~` / `~~` stages the target's generated
//! `🪢 **LINKED FROM:**` entry (and the source mint the edge needs) in
//! the same batch. Reconcile stays the authority: a skipped mirror
//! (archived or previous-daily T, a malformed line, or the kill switch)
//! never fails the capture. Reporting is additive
//! (`linked_from_line_updates`, `minted_relation_block_ids`,
//! `mirror_skipped`).

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::native::{
    collect_done::trailing_block_id_in_line,
    task_dependencies::blocks::BlocksLinePlan,
    task_dependencies::format as dependency_format,
    task_relations::{
        parse as relation_parse,
        plan::{linked_from_desired_after_delta, plan_linked_from_line},
        LinkedFromLine, RelationEntry, RelationEntryKind, MENTION_LABEL,
    },
    task_status_hooks,
    vault_links::NoteIndex,
};

/// Why a courtesy LINKED FROM write was skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MirrorSkipReason {
    ArchivedTarget,
    PreviousDailyTarget,
    MalformedLine,
    Disabled,
}

impl MirrorSkipReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::ArchivedTarget => "archived_target",
            Self::PreviousDailyTarget => "previous_daily_target",
            Self::MalformedLine => "malformed_line",
            Self::Disabled => "disabled",
        }
    }
}

/// Load the shared close policy (`relations.cancel_labels` and
/// `relations.complete_labels`), failing toward fewer closes and
/// warning through the capture warning string on invalid values. A
/// missing key, `null`, or `[]` is a valid empty policy; overlap and
/// anything else invalid yields the valid entries (possibly none) plus
/// one warning. The cancel-only substring filter is not used, so a
/// both-lists overlap still surfaces.
pub(super) fn load_close_policy() -> (
    Vec<config::relations::CancelLabel>,
    Vec<config::relations::CompleteLabel>,
    Option<String>,
) {
    match config::load_relations_config(&config::config_path()) {
        Ok(loaded) => {
            let warning = loaded
                .warnings
                .iter()
                .find(|warning| {
                    warning.contains("relations.cancel_labels")
                        || warning.contains("relations.complete_labels")
                        || warning.contains(
                            "is in both cancel_labels and complete_labels",
                        )
                })
                .map(|warning| format!("invalid relations config: {warning}"));
            (loaded.cancel_labels, loaded.complete_labels, warning)
        }
        Err(error) => (
            Vec::new(),
            Vec::new(),
            Some(format!(
                "invalid relations config: {error}; cancelling nothing"
            )),
        ),
    }
}

/// Load `relations.linked_from_lines`, falling back to `true` and
/// warning through the capture warning string on invalid values.
pub(super) fn load_linked_from_lines_enabled() -> (bool, Option<String>) {
    match config::load_relations_config(&config::config_path()) {
        Ok(loaded) => (
            loaded.linked_from_lines,
            loaded
                .warnings
                .iter()
                .find(|warning| warning.starts_with("relations.linked_from_lines"))
                .map(|warning| format!("invalid relations config: {warning}")),
        ),
        Err(error) => (
            true,
            Some(format!(
                "invalid relations config: {error}; using relations.linked_from_lines=true"
            )),
        ),
    }
}

/// Load `relations.linked_from_mentions`, falling back to `true` and
/// warning through the capture warning string on invalid values.
pub(super) fn load_linked_from_mentions_enabled() -> (bool, Option<String>) {
    match config::load_relations_config(&config::config_path()) {
        Ok(loaded) => (
            loaded.linked_from_mentions,
            loaded
                .warnings
                .iter()
                .find(|warning| {
                    warning.starts_with("relations.linked_from_mentions")
                })
                .map(|warning| format!("invalid relations config: {warning}")),
        ),
        Err(error) => (
            true,
            Some(format!(
                "invalid relations config: {error}; using relations.linked_from_mentions=true"
            )),
        ),
    }
}

/// Stage T's LINKED FROM edit for one added source, using the shared
/// cli-core planner. Returns the additive row when an edit stages, a
/// skip when the mirror cannot be written, or `None` when the line is
/// already the desired form.
#[allow(clippy::too_many_arguments)]
pub(super) fn stage_linked_from_add(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    index: &NoteIndex,
    mirror_owner_relative: &Path,
    mirror_owner_block: &str,
    source: &(PathBuf, String),
    label: Option<&str>,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<Option<BlocksLineUpdateJson>, CaptureError> {
    let owner_display = display_relative(mirror_owner_relative);
    let owner_absolute = absolute_note(bob_dir, mirror_owner_relative);
    let contents = match current_note_text(planner, &owner_absolute) {
        Ok(contents) => contents,
        Err(error) => {
            warnings.push(format!(
                "🪢 LINKED FROM not updated for ^{owner} in {owner_display}: {error}",
                owner = source.1,
                error = error.message,
            ).into());
            return Ok(None);
        }
    };
    let scan = note_tasks::scan(&contents, ctx.settings());
    let owner = match scan.by_block_id(mirror_owner_block) {
        note_tasks::BlockIdLookup::Found(task) => task.clone(),
        _ => {
            warnings.push(format!(
                "🪢 LINKED FROM not updated for ^{owner} in {owner_display}: task ^{target} is missing or ambiguous there",
                owner = source.1,
                target = mirror_owner_block,
            ).into());
            return Ok(None);
        }
    };
    let owned: Vec<String> = contents.lines().map(str::to_string).collect();
    let lines: Vec<&str> = owned.iter().map(String::as_str).collect();
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let children = relation_parse::linked_from_children_of(
        &lines,
        &fenced,
        owner.line_index,
    );
    if children
        .iter()
        .any(|child| matches!(child.parsed, LinkedFromLine::Malformed))
    {
        warnings.push(format!(
            "🪢 LINKED FROM not updated for ^{owner} in {owner_display}: malformed LINKED FROM line left alone",
            owner = source.1,
        ).into());
        return Ok(None);
    }
    let existing: Vec<RelationEntry> = children
        .iter()
        .filter_map(|child| match &child.parsed {
            LinkedFromLine::Accepted { entries, .. } => {
                Some(entries.as_slice())
            }
            _ => None,
        })
        .flatten()
        .cloned()
        .collect();
    let source_identity = relation_identity(&source.0, &source.1);
    let source_link = dependency_format::canonical_link(
        &source.0,
        &source.1,
        mirror_owner_relative,
        index,
    );
    let mut add = BTreeMap::new();
    add.insert(source_identity, (source_link, label.map(str::to_string)));
    let desired_entries = linked_from_desired_after_delta(
        &existing,
        |entry| resolve_relation_entry(index, mirror_owner_relative, entry),
        &add,
        &BTreeSet::new(),
    );
    let desired: BTreeSet<_> = desired_entries.into_iter().collect();
    let (mentions_added, mentions_removed) =
        mention_changes(&existing, &desired);
    match plan_linked_from_line(&lines, &fenced, owner.line_index, &desired) {
        BlocksLinePlan::Noop => Ok(None),
        BlocksLinePlan::Malformed => {
            warnings.push(format!(
                "🪢 LINKED FROM not updated for ^{owner} in {owner_display}: malformed LINKED FROM line left alone",
                owner = source.1,
            ).into());
            Ok(None)
        }
        BlocksLinePlan::Edits { edits, report } => {
            let line = edits
                .first()
                .map(edit_line_index)
                .unwrap_or(owner.line_index)
                + 1;
            let staged = apply_blocks_edits(&contents, edits);
            planner.stage(&owner_absolute, staged)?;
            Ok(Some(BlocksLineUpdateJson {
                path: owner_display,
                line,
                task: owner.description.clone(),
                kind: report.kind.as_str().to_string(),
                added: report.added,
                removed: report.removed,
                mentions_added,
                mentions_removed,
            }))
        }
    }
}

/// Stage T's LINKED FROM edit for one removed source identity.
#[allow(clippy::too_many_arguments)]
pub(super) fn stage_linked_from_remove(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    index: &NoteIndex,
    mirror_owner_relative: &Path,
    mirror_owner_block: &str,
    source: &(PathBuf, String),
    warnings: &mut Vec<CaptureWarning>,
) -> Result<Option<BlocksLineUpdateJson>, CaptureError> {
    let owner_display = display_relative(mirror_owner_relative);
    let owner_absolute = absolute_note(bob_dir, mirror_owner_relative);
    let contents = match current_note_text(planner, &owner_absolute) {
        Ok(contents) => contents,
        Err(error) => {
            warnings.push(format!(
                "🪢 LINKED FROM not updated for ^{owner} in {owner_display}: {error}",
                owner = source.1,
                error = error.message,
            ).into());
            return Ok(None);
        }
    };
    let scan = note_tasks::scan(&contents, ctx.settings());
    let owner = match scan.by_block_id(mirror_owner_block) {
        note_tasks::BlockIdLookup::Found(task) => task.clone(),
        _ => {
            warnings.push(format!(
                "🪢 LINKED FROM not updated for ^{owner} in {owner_display}: task ^{target} is missing or ambiguous there",
                owner = source.1,
                target = mirror_owner_block,
            ).into());
            return Ok(None);
        }
    };
    let owned: Vec<String> = contents.lines().map(str::to_string).collect();
    let lines: Vec<&str> = owned.iter().map(String::as_str).collect();
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let children = relation_parse::linked_from_children_of(
        &lines,
        &fenced,
        owner.line_index,
    );
    if children
        .iter()
        .any(|child| matches!(child.parsed, LinkedFromLine::Malformed))
    {
        warnings.push(format!(
            "🪢 LINKED FROM not updated for ^{owner} in {owner_display}: malformed LINKED FROM line left alone",
            owner = source.1,
        ).into());
        return Ok(None);
    }
    let existing: Vec<RelationEntry> = children
        .iter()
        .filter_map(|child| match &child.parsed {
            LinkedFromLine::Accepted { entries, .. } => {
                Some(entries.as_slice())
            }
            _ => None,
        })
        .flatten()
        .cloned()
        .collect();
    let source_identity = relation_identity(&source.0, &source.1);
    let keep_relation = existing.iter().any(|entry| {
        resolve_relation_entry(index, mirror_owner_relative, entry).is_some_and(
            |(identity, _)| {
                identity == source_identity
                    && entry.kind == RelationEntryKind::Relation
            },
        )
    });
    if keep_relation {
        return Ok(None);
    }
    let mut remove = BTreeSet::new();
    remove.insert(source_identity);
    let desired_entries = linked_from_desired_after_delta(
        &existing,
        |entry| resolve_relation_entry(index, mirror_owner_relative, entry),
        &BTreeMap::new(),
        &remove,
    );
    let desired: BTreeSet<_> = desired_entries.into_iter().collect();
    let (mentions_added, mentions_removed) =
        mention_changes(&existing, &desired);
    match plan_linked_from_line(&lines, &fenced, owner.line_index, &desired) {
        BlocksLinePlan::Noop => Ok(None),
        BlocksLinePlan::Malformed => {
            warnings.push(format!(
                "🪢 LINKED FROM not updated for ^{owner} in {owner_display}: malformed LINKED FROM line left alone",
                owner = source.1,
            ).into());
            Ok(None)
        }
        BlocksLinePlan::Edits { edits, report } => {
            let line = edits
                .first()
                .map(edit_line_index)
                .unwrap_or(owner.line_index)
                + 1;
            let staged = apply_blocks_edits(&contents, edits);
            planner.stage(&owner_absolute, staged)?;
            Ok(Some(BlocksLineUpdateJson {
                path: owner_display,
                line,
                task: owner.description.clone(),
                kind: report.kind.as_str().to_string(),
                added: report.added,
                removed: report.removed,
                mentions_added,
                mentions_removed,
            }))
        }
    }
}

fn mention_changes(
    existing: &[RelationEntry],
    desired: &BTreeSet<(String, Option<String>)>,
) -> (Vec<String>, Vec<String>) {
    let before: BTreeSet<_> = existing
        .iter()
        .filter(|entry| entry.kind == RelationEntryKind::Mention)
        .map(RelationEntry::canonical_text)
        .collect();
    let after: BTreeSet<_> = desired
        .iter()
        .filter(|(_, label)| label.as_deref() == Some(MENTION_LABEL))
        .map(|(link, _)| link.clone())
        .collect();
    (
        after.difference(&before).cloned().collect(),
        before.difference(&after).cloned().collect(),
    )
}

/// Whether T's LINKED FROM is malformed, so capture can skip the
/// mirror without failing.
pub(super) fn linked_from_is_malformed(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    owner_relative: &Path,
    owner_block: &str,
) -> Result<bool, CaptureError> {
    let owner_absolute = absolute_note(bob_dir, owner_relative);
    let contents = current_note_text(planner, &owner_absolute)?;
    let scan = note_tasks::scan(&contents, ctx.settings());
    let owner = match scan.by_block_id(owner_block) {
        note_tasks::BlockIdLookup::Found(task) => task,
        _ => return Ok(false),
    };
    let lines: Vec<&str> = contents.lines().collect();
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let children = relation_parse::linked_from_children_of(
        &lines,
        &fenced,
        owner.line_index,
    );
    Ok(children
        .iter()
        .any(|child| matches!(child.parsed, LinkedFromLine::Malformed)))
}

/// Mint a `^block-id` onto an already-staged task line and return the
/// minted id. Never adds `[id::]`.
pub(super) fn mint_and_append_block_id(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    relative: &Path,
    line_index: usize,
    task_line: &str,
) -> Result<String, CaptureError> {
    let absolute = absolute_note(bob_dir, relative);
    let minted =
        mint_staged_block_id(ctx, planner, &absolute, relative, task_line)?;
    let contents = current_note_text(planner, &absolute)?;
    let ending = dependency_format::note_line_ending(&contents);
    let final_newline = dependency_format::has_final_newline(&contents);
    let mut lines: Vec<String> = contents.lines().map(str::to_string).collect();
    if let Some(line) = lines.get_mut(line_index)
        && trailing_block_id_in_line(line).is_none()
    {
        let trimmed = line.trim_end();
        *line = format!("{trimmed} ^{minted}");
    }
    let joined = lines.join(ending);
    let staged = if final_newline && !lines.is_empty() {
        format!("{joined}{ending}")
    } else {
        joined
    };
    planner.stage(&absolute, staged)?;
    Ok(minted)
}

pub(super) fn relation_identity(relative: &Path, block_id: &str) -> String {
    format!("{}#^{}", display_relative(relative), block_id)
}

fn resolve_relation_entry(
    index: &NoteIndex,
    owner_relative: &Path,
    entry: &RelationEntry,
) -> Option<(String, String)> {
    let relative = if entry.target.is_empty() {
        Some(owner_relative.to_path_buf())
    } else {
        index.resolve(Some(owner_relative), entry.target.trim())
    };
    match relative {
        Some(path) => {
            let identity = relation_identity(&path, &entry.block_id);
            let canonical = dependency_format::canonical_link(
                &path,
                &entry.block_id,
                owner_relative,
                index,
            );
            Some((identity, canonical))
        }
        None => {
            let stored = entry.canonical_text();
            Some((stored.clone(), stored))
        }
    }
}
