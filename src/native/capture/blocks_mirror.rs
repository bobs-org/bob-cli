//! BLOCKS-mirror courtesy writes for capture.
//!
//! `docs/task-dependencies.md` §14.7: capture `&` stages each added
//! prerequisite's `🧱 **BLOCKS:**` entry (and the dependent's minted
//! `^id`) in the same batch, and capture dependency drops (§13.5.1)
//! stage the dependent's removal from each dropped prerequisite's
//! line. Reconcile stays the authority: every skip here (closed or
//! previous-daily owners, archive targets, malformed lines) is left
//! for the next reconcile, and the capture itself never fails for the
//! mirror. Reporting is additive (`blocks_line_updates` plus the
//! singular `minted_dependent_block_id`), so Bob Mac Capture needs no
//! Swift changes.

use std::collections::{BTreeSet, HashSet};

use super::*;
use crate::native::{
    capture_block_ids::mint_block_id,
    collect_done::block_ids_in_markdown,
    task_dependencies::{
        self, blocks as blocks_mod, format as dependency_format,
    },
    task_status_hooks,
    vault_links::NoteIndex,
};

/// Whether one BLOCKS edit adds or removes the dependent entry.
pub(super) enum BlocksDelta {
    Add,
    Remove,
}

/// Stage one owner's BLOCKS edit for a capture delta, using the shared
/// Rust planner (`plan_blocks_line`, the same planner reconcile owns).
///
/// Returns the additive report row plus the owner's `dependency_target`
/// task-block ref when an edit stages, or `None` when the mirror
/// needs no write (kill switch off, archive or previous-daily owner,
/// closed owner, or a byte-identical line). Malformed or unreadable
/// owners warn and leave the capture itself standing.
#[allow(clippy::too_many_arguments)]
pub(super) fn stage_blocks_delta(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    index: &NoteIndex,
    owner_relative: &Path,
    owner_block: &str,
    dependent: &(String, String),
    delta: BlocksDelta,
    warnings: &mut Vec<String>,
) -> Result<Option<(BlocksLineUpdateJson, TaskBlockRef)>, CaptureError> {
    if !ctx.blocks_lines_enabled() {
        return Ok(None);
    }
    let owner_display = display_relative(owner_relative);
    if task_dependencies::is_archive_path(owner_relative) {
        return Ok(None);
    }
    let owner_absolute = absolute_note(bob_dir, owner_relative);
    if ctx.previous_daily() == Some(owner_absolute.as_path()) {
        return Ok(None);
    }
    let contents = match current_note_text(planner, &owner_absolute) {
        Ok(contents) => contents,
        Err(error) => {
            warnings.push(format!(
                "🧱 BLOCKS not updated for ^{owner} in {owner_display}: {error}",
                owner = dependent.1,
                error = error.message,
            ));
            return Ok(None);
        }
    };
    let scan = note_tasks::scan(&contents, ctx.settings());
    let owner = match scan.by_block_id(owner_block) {
        note_tasks::BlockIdLookup::Found(task) => task.clone(),
        _ => {
            warnings.push(format!(
                "🧱 BLOCKS not updated for ^{owner} in {owner_display}: prerequisite task ^{target} is missing or ambiguous there",
                owner = dependent.1,
                target = owner_block,
            ));
            return Ok(None);
        }
    };
    if !owner.status_type.is_open() {
        return Ok(None);
    }
    let owned: Vec<String> = contents.lines().map(str::to_string).collect();
    let lines: Vec<&str> = owned.iter().map(String::as_str).collect();
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let children =
        blocks_mod::blocks_children_of(&lines, &fenced, owner.line_index);
    if children
        .iter()
        .any(|child| matches!(child.parsed, blocks_mod::BlocksLine::Malformed))
    {
        warnings.push(format!(
            "🧱 BLOCKS not updated for ^{owner} in {owner_display}: malformed BLOCKS line left alone",
            owner = dependent.1,
        ));
        return Ok(None);
    }
    let desired = desired_after_delta(
        index,
        owner_relative,
        &children,
        &lines,
        dependent,
        delta,
    );
    match blocks_mod::plan_blocks_line(
        &lines,
        &fenced,
        owner.line_index,
        &desired,
    ) {
        blocks_mod::BlocksLinePlan::Noop => Ok(None),
        blocks_mod::BlocksLinePlan::Malformed => {
            warnings.push(format!(
                "🧱 BLOCKS not updated for ^{owner} in {owner_display}: malformed BLOCKS line left alone",
                owner = dependent.1,
            ));
            Ok(None)
        }
        blocks_mod::BlocksLinePlan::Edits { edits, report } => {
            let line = edits
                .first()
                .map(edit_line_index)
                .unwrap_or(owner.line_index)
                + 1;
            let staged = apply_blocks_edits(&contents, edits);
            planner.stage(&owner_absolute, staged)?;
            let update = BlocksLineUpdateJson {
                path: owner_display.clone(),
                line,
                task: owner.description.clone(),
                kind: report.kind.as_str().to_string(),
                added: report.added,
                removed: report.removed,
            };
            let task_ref = owner_task_ref(
                ctx,
                planner,
                owner_relative,
                &owner_absolute,
                owner_block,
            )?;
            Ok(Some((update, task_ref)))
        }
    }
}

/// Desired BLOCKS entries after a gesture delta: every existing entry
/// kept (unresolvable ones verbatim, never pruned on uncertainty),
/// plus the dependent for adds or minus it for removes, rendered as
/// canonical links from the owner's note.
fn desired_after_delta(
    index: &NoteIndex,
    owner_relative: &Path,
    children: &[blocks_mod::BlocksChild],
    lines: &[&str],
    dependent: &(String, String),
    delta: BlocksDelta,
) -> BTreeSet<String> {
    let mut desired = BTreeSet::new();
    for child in children {
        let blocks_mod::BlocksLine::Accepted { links, .. } = &child.parsed
        else {
            continue;
        };
        let child_text = lines.get(child.line_index).copied().unwrap_or("");
        let spans = task_dependencies::block_link_spans(child_text);
        for (position, link) in links.iter().enumerate() {
            let resolved =
                index.resolve(Some(owner_relative), link.target.trim());
            let identity = resolved.as_ref().map(|relative| {
                (display_relative(relative), link.block_id.clone())
            });
            let is_dependent = identity.as_ref() == Some(dependent);
            if is_dependent && matches!(delta, BlocksDelta::Remove) {
                continue;
            }
            match (resolved, identity) {
                (Some(relative), Some(_)) => {
                    desired.insert(dependency_format::canonical_link(
                        &relative,
                        &link.block_id,
                        owner_relative,
                        index,
                    ));
                }
                _ => {
                    desired.insert(spans.get(position).map_or_else(
                        || {
                            if link.target.is_empty() {
                                format!("[[#^{}]]", link.block_id)
                            } else {
                                format!(
                                    "[[{}#^{}]]",
                                    link.target, link.block_id
                                )
                            }
                        },
                        |span| {
                            child_text[span.full_start..span.full_end]
                                .to_string()
                        },
                    ));
                }
            }
        }
    }
    if matches!(delta, BlocksDelta::Add) {
        let dependent_path = PathBuf::from(&dependent.0);
        desired.insert(dependency_format::canonical_link(
            &dependent_path,
            &dependent.1,
            owner_relative,
            index,
        ));
    }
    desired
}

/// Re-resolve the owner's task line after staging, so the task-block
/// ref points at the post-write text even when the insert shifted it.
fn owner_task_ref(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    owner_relative: &Path,
    owner_absolute: &Path,
    owner_block: &str,
) -> Result<TaskBlockRef, CaptureError> {
    let contents = current_note_text(planner, owner_absolute)?;
    let scan = note_tasks::scan(&contents, ctx.settings());
    let route_label = display_relative(owner_relative);
    let task = lookup_staged_task(&scan, &route_label, owner_block)?;
    let mut without_extension = owner_relative.to_path_buf();
    without_extension.set_extension("");
    Ok(TaskBlockRef {
        target: owner_absolute.to_path_buf(),
        relative_target: route_label,
        route: display_relative(&without_extension),
        line: task.line_index,
        block_id: Some(owner_block.to_string()),
        role: TaskBlockRole::DependencyTarget,
    })
}

/// Mint a `^block-id` inside the atomic staged batch
/// (`docs/task-dependencies.md` §14.4): the SB pipeline over the task
/// description, against every block id in the note plus this batch's
/// reservations. Never adds `[id::]`. Does not consult
/// `dependencies.blocks_lines`; `*` always mints even when the kill
/// switch is off.
pub(super) fn mint_staged_block_id(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    absolute: &Path,
    dep_relative: &Path,
    task_line: &str,
) -> Result<String, CaptureError> {
    let base = match planner.current_contents(absolute)? {
        Some(staged) => staged,
        None => std::fs::read_to_string(absolute).unwrap_or_default(),
    };
    let mut used: BTreeSet<String> =
        block_ids_in_markdown(&base).into_iter().collect();
    used.extend(ctx.block_reservations_for(dep_relative));
    let used_refs: HashSet<&str> = used.iter().map(String::as_str).collect();
    let description = note_tasks::clean_description(
        &task_description_of(task_line),
        &ctx.settings().global_filter,
        None,
    );
    let minted = mint_block_id(&description, &used_refs);
    ctx.reserve_block_id(dep_relative.to_path_buf(), minted.clone());
    Ok(minted)
}

/// `&` alias for [`mint_staged_block_id`]: mint the dependent's id for
/// the BLOCKS mirror when the kill switch is on.
pub(super) fn mint_staged_dependent_id(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    absolute: &Path,
    dep_relative: &Path,
    task_line: &str,
) -> Result<String, CaptureError> {
    mint_staged_block_id(ctx, planner, absolute, dep_relative, task_line)
}

fn edit_line_index(edit: &blocks_mod::BlocksEdit) -> usize {
    match edit {
        blocks_mod::BlocksEdit::Insert { line_index, .. }
        | blocks_mod::BlocksEdit::Replace { line_index, .. }
        | blocks_mod::BlocksEdit::Remove { line_index } => *line_index,
    }
}

/// Apply shared-planner edits to note text, preserving the note's
/// line ending and final-newline state.
fn apply_blocks_edits(
    contents: &str,
    edits: Vec<blocks_mod::BlocksEdit>,
) -> String {
    let ending = dependency_format::note_line_ending(contents);
    let final_newline = dependency_format::has_final_newline(contents);
    let mut lines: Vec<String> = contents.lines().map(str::to_string).collect();
    let mut ordered = edits;
    ordered.sort_by(|left, right| {
        edit_line_index(right)
            .cmp(&edit_line_index(left))
            .then_with(|| edit_rank(left).cmp(&edit_rank(right)))
    });
    for edit in ordered {
        match edit {
            blocks_mod::BlocksEdit::Replace { line_index, text } => {
                if line_index < lines.len() {
                    lines[line_index] = text;
                }
            }
            blocks_mod::BlocksEdit::Remove { line_index } => {
                if line_index < lines.len() {
                    lines.remove(line_index);
                }
            }
            blocks_mod::BlocksEdit::Insert { line_index, text } => {
                let at = line_index.min(lines.len());
                lines.insert(at, text);
            }
        }
    }
    let joined = lines.join(ending);
    if final_newline && !lines.is_empty() {
        format!("{joined}{ending}")
    } else {
        joined
    }
}

fn edit_rank(edit: &blocks_mod::BlocksEdit) -> u8 {
    match edit {
        blocks_mod::BlocksEdit::Replace { .. } => 0,
        blocks_mod::BlocksEdit::Remove { .. } => 1,
        blocks_mod::BlocksEdit::Insert { .. } => 2,
    }
}

/// Load `dependencies.blocks_lines`, falling back to `true` and
/// warning through the capture warning string on invalid values
/// (`docs/task-dependencies.md` §14.10).
pub(super) fn load_blocks_lines_enabled() -> (bool, Option<String>) {
    match config::load_dependencies_config(&config::config_path()) {
        Ok(loaded) => (loaded.blocks_lines, None),
        Err(error) => (
            true,
            Some(format!(
                "invalid dependencies config: {error}; using dependencies.blocks_lines=true"
            )),
        ),
    }
}
