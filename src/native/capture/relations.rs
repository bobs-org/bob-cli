//! Capture `~` / `~~` execution: resolve, refuse, stage LINKS, and
//! courtesy-write LINKED FROM in the same batch.

use std::collections::BTreeSet;
use std::path::PathBuf;

use super::*;
use crate::native::{
    collect_done::trailing_block_id_in_line,
    task_dependencies::{self, format as dependency_format},
    task_relations::{
        parse as relation_parse,
        plan::{plan_links_edit, LinksEdit, LinksLinePlan},
        LinksLine,
    },
    task_status_hooks,
};

/// Reports from one item's `~` / `~~` writes.
#[derive(Debug, Clone, Default)]
pub(super) struct PlannedRelationReports {
    pub updates: Vec<RelationUpdateJson>,
    pub linked_from: Vec<BlocksLineUpdateJson>,
    pub minted: Vec<MintedRelationBlockIdJson>,
    pub skipped: Vec<MirrorSkippedJson>,
    pub source_refs: Vec<TaskBlockRef>,
    pub owner_block_id: Option<String>,
}

/// Locate the item's relation owner after `&` / `*` have staged, then
/// plan every `~` / `~~` modifier. Refuses before anything is staged
/// when a target is illegal or the same target is typed twice with
/// different labels.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_relation_writes(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    owner: &RelationOwner,
    relations: &[ParsedRelation],
    warnings: &mut Vec<String>,
    existing_owner: bool,
) -> Result<PlannedRelationReports, CaptureError> {
    if relations.is_empty() {
        return Ok(PlannedRelationReports {
            owner_block_id: owner.block_id.clone(),
            ..PlannedRelationReports::default()
        });
    }
    let _ = ctx.linked_from_lines_enabled();
    let mut resolved = Vec::with_capacity(relations.len());
    for typed in relations {
        resolved.push(resolve_relation_target(
            ctx, planner, bob_dir, owner, typed,
        )?);
    }
    refuse_duplicate_labels(&resolved)?;

    let mut reports = PlannedRelationReports {
        owner_block_id: owner.block_id.clone(),
        ..PlannedRelationReports::default()
    };
    let mut owner_state = owner.clone();
    let mut seen = BTreeSet::new();
    for target in resolved {
        let key = (
            matches!(target.direction, RelationDirection::From),
            display_relative(&target.relative),
            target.block_id.clone(),
        );
        let duplicate = !seen.insert(key);
        apply_one_relation(
            ctx,
            planner,
            bob_dir,
            &mut owner_state,
            &target,
            duplicate,
            existing_owner,
            warnings,
            &mut reports,
        )?;
    }
    reports.owner_block_id = owner_state.block_id;
    Ok(reports)
}

/// The item's source (for `~`) or target (for `~~`): a vault-relative
/// note plus an optional block id, with a line so a missing id can be
/// minted onto the already-staged task.
#[derive(Debug, Clone)]
pub(super) struct RelationOwner {
    pub relative: PathBuf,
    pub block_id: Option<String>,
    pub line_index: usize,
}

impl RelationOwner {
    pub(super) fn from_dependent(
        ctx: &DependencyContext,
        planner: &mut CaptureBatchPlanner,
        bob_dir: &Path,
        owner: &DependentRef,
    ) -> Result<Self, CaptureError> {
        let located = locate_task_by_block_id(
            ctx,
            planner,
            bob_dir,
            &owner.relative,
            &owner.block_id,
        )?;
        Ok(Self {
            relative: owner.relative.clone(),
            block_id: Some(owner.block_id.clone()),
            line_index: located.line,
        })
    }
}

/// Resolve the item's owner after `&` / `*` so relation writes can
/// re-find it by block id (or by the just-inserted task line).
#[allow(clippy::too_many_arguments)]
pub(super) fn locate_relation_owner(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    parsed: &ParsedCaptureText,
    relative_target: &Path,
    task_block_id: Option<&str>,
    minted_dependent: Option<&MintedDependentBlockIdJson>,
    minted_blocker: Option<&MintedDependentBlockIdJson>,
    task_line: &str,
) -> Result<RelationOwner, CaptureError> {
    if let Some(target) = parsed.dependency_target.as_ref()
        && target.kind == ParsedDependencyTargetKind::ExistingTask
        && let (Some(route), Some(block_id)) =
            (target.route.as_deref(), target.block_id.as_deref())
    {
        return RelationOwner::from_dependent(
            ctx,
            planner,
            bob_dir,
            &DependentRef::from_route(route, block_id),
        );
    }
    let block_id = task_block_id
        .map(str::to_string)
        .or_else(|| minted_dependent.map(|minted| minted.block_id.clone()))
        .or_else(|| minted_blocker.map(|minted| minted.block_id.clone()));
    if let Some(block_id) = block_id {
        return RelationOwner::from_dependent(
            ctx,
            planner,
            bob_dir,
            &DependentRef {
                relative: relative_target.to_path_buf(),
                block_id,
            },
        );
    }
    let absolute = absolute_note(bob_dir, relative_target);
    let contents = current_note_text(planner, &absolute)?;
    let line_index = last_exact_line_index(&contents, task_line).ok_or_else(
        || {
            CaptureError::io(
                "relation capture invariant failed: new owner task line is missing from staged text",
            )
        },
    )?;
    Ok(RelationOwner {
        relative: relative_target.to_path_buf(),
        block_id: None,
        line_index,
    })
}

struct ResolvedRelation {
    typed: ParsedRelation,
    direction: RelationDirection,
    relative: PathBuf,
    block_id: String,
    description: String,
    line_index: usize,
    archived: bool,
}

fn resolve_relation_target(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    owner: &RelationOwner,
    typed: &ParsedRelation,
) -> Result<ResolvedRelation, CaptureError> {
    let token = typed.raw.as_str();
    if typed.direction == RelationDirection::From {
        refuse_from_target_path(token, &typed_note_as_relative(&typed.note))?;
    }
    let relative = resolve_prerequisite_note(
        ctx,
        planner,
        bob_dir,
        &typed.note,
    )
    .map_err(|error| {
        CaptureError::usage(format!("cannot relate {token}: {}", error.message))
    })?;
    if typed.direction == RelationDirection::From {
        refuse_from_target_path(token, &relative)?;
    }
    let absolute = absolute_note(bob_dir, &relative);
    let contents = current_note_text(planner, &absolute).map_err(|error| {
        CaptureError::usage(format!("cannot relate {token}: {}", error.message))
    })?;
    let scan = note_tasks::scan(&contents, ctx.settings());
    let label = display_relative(&relative);
    let task = match lookup_staged_task(&scan, &label, &typed.block_id) {
        Ok(task) => task,
        Err(error) => {
            if block_id_in_fenced_code(&contents, &typed.block_id) {
                return Err(CaptureError::usage(format!(
                    "cannot relate {token}: ^{id} lives inside a fenced code block and is not an editable task",
                    id = typed.block_id,
                )));
            }
            if matches!(
                scan.by_block_id(&typed.block_id),
                note_tasks::BlockIdLookup::NotATask { .. }
            ) {
                return Err(CaptureError::usage(format!(
                    "cannot relate {token}: ^{id} in {label} is not a task",
                    id = typed.block_id,
                )));
            }
            if matches!(
                scan.by_block_id(&typed.block_id),
                note_tasks::BlockIdLookup::Missing
            ) {
                return Err(CaptureError::usage(format!(
                    "cannot relate {token}: no task with block ID ^{id} in {label}",
                    id = typed.block_id,
                )));
            }
            return Err(CaptureError::usage(format!(
                "cannot relate {token}: {}",
                error.message
            )));
        }
    };
    if let Some(owner_id) = owner.block_id.as_deref()
        && display_relative(&owner.relative) == label
        && owner_id == typed.block_id
    {
        return Err(CaptureError::usage(format!(
            "cannot relate {token}: that is the owner task itself; a task cannot relate to itself"
        )));
    }
    Ok(ResolvedRelation {
        typed: typed.clone(),
        direction: typed.direction,
        relative: relative.clone(),
        block_id: typed.block_id.clone(),
        description: task.description.clone(),
        line_index: task.line_index,
        archived: task_dependencies::is_archive_path(&relative),
    })
}

fn refuse_duplicate_labels(
    resolved: &[ResolvedRelation],
) -> Result<(), CaptureError> {
    let mut seen: Vec<(&ResolvedRelation, Option<&str>)> = Vec::new();
    for target in resolved {
        let label = target.typed.label.as_deref();
        if let Some(previous) = seen.iter().find(|(existing, _)| {
            existing.direction == target.direction
                && existing.relative == target.relative
                && existing.block_id == target.block_id
        }) {
            if previous.1 != label {
                return Err(CaptureError::usage(format!(
                    "cannot relate {}: the same target is linked twice with different labels",
                    target.typed.raw
                )));
            }
        } else {
            seen.push((target, label));
        }
    }
    Ok(())
}

fn refuse_from_target_path(
    token: &str,
    relative: &Path,
) -> Result<(), CaptureError> {
    let display = display_relative(relative);
    if task_status_hooks::canonical_daily_date(relative).is_some() {
        return Err(CaptureError::usage(format!(
            "cannot relate {token}: {display} is a daily note; capture cannot write another task's LINKS there"
        )));
    }
    if task_dependencies::is_archive_path(relative) {
        return Err(CaptureError::usage(format!(
            "cannot relate {token}: {display} is under done/; archived tasks cannot gain a LINKS entry from capture"
        )));
    }
    for component in relative.components() {
        let os_name = component.as_os_str();
        let Some(name) = os_name.to_str() else {
            continue;
        };
        if crate::native::is_always_excluded_note_directory_name(os_name)
            && matches!(name, "_templates" | "_generated" | "_conflicts")
        {
            return Err(CaptureError::usage(format!(
                "cannot relate {token}: {display} is under {name}/, which is not an editable LINKS target"
            )));
        }
        if name.starts_with('.') {
            return Err(CaptureError::usage(format!(
                "cannot relate {token}: {display} is in a dot-directory; capture cannot write LINKS there"
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_one_relation(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    owner: &mut RelationOwner,
    target: &ResolvedRelation,
    duplicate: bool,
    existing_owner: bool,
    warnings: &mut Vec<String>,
    reports: &mut PlannedRelationReports,
) -> Result<(), CaptureError> {
    let index = super::dependencies::staged_note_index(ctx, planner, bob_dir);
    match target.direction {
        RelationDirection::To => apply_links_to(
            ctx,
            planner,
            bob_dir,
            &index,
            owner,
            target,
            duplicate,
            existing_owner,
            warnings,
            reports,
        ),
        RelationDirection::From => apply_linked_from(
            ctx, planner, bob_dir, &index, owner, target, duplicate, warnings,
            reports,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_links_to(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    index: &crate::native::vault_links::NoteIndex,
    owner: &mut RelationOwner,
    target: &ResolvedRelation,
    duplicate: bool,
    existing_owner: bool,
    warnings: &mut Vec<String>,
    reports: &mut PlannedRelationReports,
) -> Result<(), CaptureError> {
    let (action, previous_label, source_text, source_line) = stage_links_add(
        ctx,
        planner,
        bob_dir,
        index,
        &owner.relative,
        owner.block_id.as_deref(),
        owner.line_index,
        &target.relative,
        &target.block_id,
        target.typed.label.clone(),
        &target.typed.raw,
    )?;
    let action = if duplicate && action == RelationAction::Added {
        RelationAction::Unchanged
    } else {
        action
    };
    if action != RelationAction::Unchanged && existing_owner {
        if let Some(block_id) = owner.block_id.as_deref() {
            let located = locate_task_by_block_id(
                ctx,
                planner,
                bob_dir,
                &owner.relative,
                block_id,
            )?;
            reports.source_refs.push(TaskBlockRef {
                role: TaskBlockRole::RelationSource,
                ..located
            });
        }
    }
    reports.updates.push(relation_update_json(
        RelationDirection::To,
        &owner.relative,
        source_line,
        owner.block_id.clone(),
        &source_text,
        target,
        action,
        previous_label,
    ));
    if action == RelationAction::Unchanged {
        return Ok(());
    }
    maybe_stage_mirror(
        ctx,
        planner,
        bob_dir,
        index,
        owner,
        &target.relative,
        &target.block_id,
        target.typed.label.as_deref(),
        target.archived,
        warnings,
        reports,
        true,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_linked_from(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    index: &crate::native::vault_links::NoteIndex,
    owner: &mut RelationOwner,
    target: &ResolvedRelation,
    duplicate: bool,
    warnings: &mut Vec<String>,
    reports: &mut PlannedRelationReports,
) -> Result<(), CaptureError> {
    ensure_owner_block_id(
        ctx,
        planner,
        bob_dir,
        owner,
        MintedRelationRole::Target,
        reports,
    )?;
    let owner_id = owner.block_id.clone().ok_or_else(|| {
        CaptureError::io(
            "relation capture invariant failed: ~~ owner has no block ID after mint",
        )
    })?;
    let (action, previous_label, source_text, source_line) = stage_links_add(
        ctx,
        planner,
        bob_dir,
        index,
        &target.relative,
        Some(&target.block_id),
        target.line_index,
        &owner.relative,
        &owner_id,
        target.typed.label.clone(),
        &target.typed.raw,
    )?;
    let action = if duplicate && action == RelationAction::Added {
        RelationAction::Unchanged
    } else {
        action
    };
    if action != RelationAction::Unchanged {
        let located = locate_task_by_block_id(
            ctx,
            planner,
            bob_dir,
            &target.relative,
            &target.block_id,
        )?;
        reports.source_refs.push(TaskBlockRef {
            role: TaskBlockRole::RelationSource,
            ..located
        });
    }
    let mut source_endpoint = target_as_endpoint(target);
    source_endpoint.line = source_line;
    source_endpoint.text = source_text;
    let owner_text = owner_task_text(ctx, planner, bob_dir, owner)?;
    reports.updates.push(RelationUpdateJson {
        direction: RelationDirection::From,
        source: source_endpoint,
        target: RelationEndpointJson {
            path: display_relative(&owner.relative),
            line: owner.line_index + 1,
            block_id: owner_id.clone(),
            text: owner_text,
            archived: None,
        },
        label: target.typed.label.clone(),
        previous_label,
        action,
    });
    if action == RelationAction::Unchanged {
        return Ok(());
    }
    let owner_relative = owner.relative.clone();
    maybe_stage_mirror(
        ctx,
        planner,
        bob_dir,
        index,
        owner,
        &owner_relative,
        &owner_id,
        target.typed.label.as_deref(),
        false,
        warnings,
        reports,
        false,
        Some((target.relative.clone(), target.block_id.clone())),
    )
}

/// `~` mints the source when the mirror will be written. `~~` always
/// mints the item task (the relation target).
fn ensure_owner_block_id(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    owner: &mut RelationOwner,
    role: MintedRelationRole,
    reports: &mut PlannedRelationReports,
) -> Result<(), CaptureError> {
    if owner.block_id.is_some() {
        return Ok(());
    }
    let absolute = absolute_note(bob_dir, &owner.relative);
    let contents = current_note_text(planner, &absolute)?;
    let task_line = contents
        .lines()
        .nth(owner.line_index)
        .unwrap_or_default()
        .to_string();
    if let Some(existing) = trailing_block_id_in_line(&task_line) {
        owner.block_id = Some(existing);
        return Ok(());
    }
    let minted = mint_and_append_block_id(
        ctx,
        planner,
        bob_dir,
        &owner.relative,
        owner.line_index,
        &task_line,
    )?;
    reports.minted.push(MintedRelationBlockIdJson {
        path: display_relative(&owner.relative),
        line: owner.line_index + 1,
        block_id: minted.clone(),
        role,
    });
    owner.block_id = Some(minted);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn maybe_stage_mirror(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    index: &crate::native::vault_links::NoteIndex,
    source_owner: &mut RelationOwner,
    mirror_relative: &Path,
    mirror_block: &str,
    label: Option<&str>,
    archived_target: bool,
    warnings: &mut Vec<String>,
    reports: &mut PlannedRelationReports,
    mint_source: bool,
    source_override: Option<(PathBuf, String)>,
) -> Result<(), CaptureError> {
    let skip_path = display_relative(mirror_relative);
    if !ctx.linked_from_lines_enabled() {
        reports.skipped.push(MirrorSkippedJson {
            path: skip_path,
            block_id: mirror_block.to_string(),
            reason: MirrorSkipReason::Disabled.as_str(),
        });
        return Ok(());
    }
    if archived_target {
        reports.skipped.push(MirrorSkippedJson {
            path: skip_path,
            block_id: mirror_block.to_string(),
            reason: MirrorSkipReason::ArchivedTarget.as_str(),
        });
        return Ok(());
    }
    let mirror_absolute = absolute_note(bob_dir, mirror_relative);
    if ctx.previous_daily() == Some(mirror_absolute.as_path()) {
        reports.skipped.push(MirrorSkippedJson {
            path: skip_path,
            block_id: mirror_block.to_string(),
            reason: MirrorSkipReason::PreviousDailyTarget.as_str(),
        });
        return Ok(());
    }
    if linked_from_is_malformed(
        ctx,
        planner,
        bob_dir,
        mirror_relative,
        mirror_block,
    )? {
        warnings.push(format!(
            "🪢 LINKED FROM not updated for ^{mirror_block} in {skip_path}: malformed LINKED FROM line left alone"
        ));
        reports.skipped.push(MirrorSkippedJson {
            path: skip_path,
            block_id: mirror_block.to_string(),
            reason: MirrorSkipReason::MalformedLine.as_str(),
        });
        return Ok(());
    }
    if mint_source {
        ensure_owner_block_id(
            ctx,
            planner,
            bob_dir,
            source_owner,
            MintedRelationRole::Source,
            reports,
        )?;
    }
    let source = source_override.unwrap_or_else(|| {
        (
            source_owner.relative.clone(),
            source_owner.block_id.clone().unwrap_or_default(),
        )
    });
    if source.1.is_empty() {
        return Ok(());
    }
    if let Some(update) = stage_linked_from_add(
        ctx,
        planner,
        bob_dir,
        index,
        mirror_relative,
        mirror_block,
        &source,
        label,
        warnings,
    )? {
        reports.linked_from.push(update);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn stage_links_add(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    index: &crate::native::vault_links::NoteIndex,
    source_relative: &Path,
    source_block: Option<&str>,
    source_line_hint: usize,
    target_relative: &Path,
    target_block: &str,
    label: Option<String>,
    token: &str,
) -> Result<(RelationAction, Option<String>, String, usize), CaptureError> {
    let source_absolute = absolute_note(bob_dir, source_relative);
    let contents = current_note_text(planner, &source_absolute)?;
    let scan = note_tasks::scan(&contents, ctx.settings());
    let source_display = display_relative(source_relative);
    let (line_index, source_text) = if let Some(block_id) = source_block {
        let task = lookup_staged_task(&scan, &source_display, block_id)?;
        (task.line_index, task.description.clone())
    } else {
        let text = contents
            .lines()
            .nth(source_line_hint)
            .unwrap_or_default()
            .to_string();
        (source_line_hint, task_description_of(&text))
    };
    let owned: Vec<String> = contents.lines().map(str::to_string).collect();
    let lines: Vec<&str> = owned.iter().map(String::as_str).collect();
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let children =
        relation_parse::links_children_of(&lines, &fenced, line_index);
    let previous = existing_links_entry(
        &children,
        index,
        source_relative,
        target_relative,
        target_block,
    );
    let add_link = previous
        .as_ref()
        .map(|(link, _)| link.clone())
        .unwrap_or_else(|| {
            dependency_format::canonical_link(
                target_relative,
                target_block,
                source_relative,
                index,
            )
        });
    let previous_label = previous.and_then(|(_, label)| label);
    let edit = LinksEdit {
        add: vec![(add_link, label)],
        ..LinksEdit::default()
    };
    match plan_links_edit(&lines, &fenced, line_index, &edit) {
        LinksLinePlan::Noop => Ok((
            RelationAction::Unchanged,
            previous_label,
            source_text,
            line_index + 1,
        )),
        LinksLinePlan::Malformed => Err(CaptureError::usage(format!(
            "cannot relate {token}: its LINKS line is malformed"
        ))),
        LinksLinePlan::InvalidLabel(error) => {
            Err(CaptureError::usage(format!(
                "cannot relate {token}: relation label is {}",
                error.as_str()
            )))
        }
        LinksLinePlan::Edits { edits, report } => {
            let action = if !report.relabeled.is_empty() {
                RelationAction::Relabeled
            } else if !report.added.is_empty() {
                RelationAction::Added
            } else {
                RelationAction::Unchanged
            };
            let staged = apply_blocks_edits(&contents, edits);
            planner.stage(&source_absolute, staged)?;
            Ok((action, previous_label, source_text, line_index + 1))
        }
    }
}

fn existing_links_entry(
    children: &[relation_parse::LinksChild],
    index: &crate::native::vault_links::NoteIndex,
    source_relative: &Path,
    target_relative: &Path,
    target_block: &str,
) -> Option<(String, Option<String>)> {
    for child in children {
        let LinksLine::Accepted { entries, .. } = &child.parsed else {
            continue;
        };
        for entry in entries {
            let resolved = if entry.target.is_empty() {
                Some(source_relative.to_path_buf())
            } else {
                index.resolve(Some(source_relative), entry.target.trim())
            };
            if resolved.as_deref() == Some(target_relative)
                && entry.block_id == target_block
            {
                return Some((entry.canonical_text(), entry.label.clone()));
            }
        }
    }
    None
}

fn relation_update_json(
    direction: RelationDirection,
    source_relative: &Path,
    source_line: usize,
    source_block: Option<String>,
    source_text: &str,
    target: &ResolvedRelation,
    action: RelationAction,
    previous_label: Option<String>,
) -> RelationUpdateJson {
    RelationUpdateJson {
        direction,
        source: RelationEndpointJson {
            path: display_relative(source_relative),
            line: source_line,
            block_id: source_block.unwrap_or_default(),
            text: source_text.to_string(),
            archived: None,
        },
        target: target_as_endpoint(target),
        label: target.typed.label.clone(),
        previous_label,
        action,
    }
}

fn target_as_endpoint(target: &ResolvedRelation) -> RelationEndpointJson {
    RelationEndpointJson {
        path: display_relative(&target.relative),
        line: target.line_index + 1,
        block_id: target.block_id.clone(),
        text: target.description.clone(),
        archived: target.archived.then_some(true),
    }
}

fn owner_task_text(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    owner: &RelationOwner,
) -> Result<String, CaptureError> {
    let absolute = absolute_note(bob_dir, &owner.relative);
    let contents = current_note_text(planner, &absolute)?;
    if let Some(block_id) = owner.block_id.as_deref() {
        let scan = note_tasks::scan(&contents, ctx.settings());
        let label = display_relative(&owner.relative);
        if let Ok(task) = lookup_staged_task(&scan, &label, block_id) {
            return Ok(task.description.clone());
        }
    }
    Ok(task_description_of(
        contents.lines().nth(owner.line_index).unwrap_or_default(),
    ))
}

fn block_id_in_fenced_code(contents: &str, block_id: &str) -> bool {
    let lines: Vec<&str> = contents.lines().collect();
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    fenced.iter().any(|line_index| {
        lines.get(*line_index).is_some_and(|line| {
            trailing_block_id_in_line(line).as_deref() == Some(block_id)
        })
    })
}

fn typed_note_as_relative(typed: &str) -> PathBuf {
    if typed
        .rsplit('/')
        .next()
        .is_some_and(|file| file.contains('.'))
    {
        PathBuf::from(typed)
    } else {
        PathBuf::from(format!("{typed}.md"))
    }
}
