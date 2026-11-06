//! Atomic execution of capture `<` merge-in clauses.
//!
//! The ordinary capture planner creates and fully stages the new target
//! first. This adapter then resolves the final target and sources from the
//! staged vault, and hands one explicit merge to the same engine used by `>`.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::*;
use crate::native::{
    capture_language::{transfer as lexer, MergeInClause, ParsedMergeInSource},
    relation_labels::relation_label_key,
    task_locator::TaskLocator,
    task_merge::MergeContainer,
    task_move::VaultSnapshot,
};

pub(super) struct MergeInExecution {
    pub(super) output: RelationMergeOutput,
    pub(super) repaired_files: Vec<transfer::TransferRepairedFileJson>,
    pub(super) task_block_refs: Vec<TaskBlockRef>,
    pub(super) target_line: String,
    pub(super) target_block_id: String,
    pub(super) target_block_link: String,
}

/// Resolve marker aliases and reject conflicts before the ordinary task,
/// dependency, or relation planners stage any writes for this item.
pub(super) fn preflight_marker_conflicts(
    request: &CaptureRequest,
    item: &ParsedCaptureItem,
    planner: &mut CaptureBatchPlanner,
    ctx: &DependencyContext,
) -> Result<(), CaptureError> {
    let Some(clause) = item.parsed.merge_in.as_ref() else {
        return Ok(());
    };
    let item_number = item.index + 1;

    // `~~|merge-label` would perform a second merge as part of the ordinary
    // relation path, so refuse it even if it names a different source.
    if let Some(relation) = item.parsed.relations.iter().find(|relation| {
        relation.direction == RelationDirection::From
            && relation.label.as_deref().is_some_and(|label| {
                let key = relation_label_key(label);
                ctx.merge_labels()
                    .iter()
                    .any(|configured| configured.key == key)
            })
    }) {
        return Err(merge_in_source_also_marked_error(
            item_number,
            relation.span.unwrap_or((clause.arrow.0, clause.arrow.1)),
        ));
    }

    let snapshot =
        relations_merge::load_staged_snapshot(planner, &request.bob_dir)?;
    let sources = resolve_sources(&snapshot, clause, item_number)?;

    let mut markers = Vec::new();
    markers.extend(item.parsed.dependencies.iter().map(|marker| {
        (marker.note.as_str(), marker.block_id.as_str(), marker.span)
    }));
    markers.extend(item.parsed.blocked_tasks.iter().map(|marker| {
        (marker.note.as_str(), marker.block_id.as_str(), marker.span)
    }));
    markers.extend(item.parsed.relations.iter().map(|marker| {
        (marker.note.as_str(), marker.block_id.as_str(), marker.span)
    }));

    for (note, block_id, span) in markers {
        // If a modifier's note cannot be resolved, its own planner will
        // report that established error. It cannot be the same canonical
        // source unless both names resolve to the same staged note.
        let Ok(path) = transfer::resolve_transfer_note(&snapshot, note) else {
            continue;
        };
        if let Some((source, _)) =
            sources.iter().find(|(source, source_path)| {
                source_path == &path && source.block_id == block_id
            })
        {
            return Err(merge_in_source_also_marked_error(
                item_number,
                span.unwrap_or((source.start, source.end)),
            ));
        }
    }
    Ok(())
}

/// Run one explicit source merge after the target's ordinary capture writes.
#[allow(clippy::too_many_arguments)]
pub(super) fn execute(
    request: &CaptureRequest,
    item_number: usize,
    clause: &MergeInClause,
    planner: &mut CaptureBatchPlanner,
    ctx: &DependencyContext,
    warnings: &mut Vec<CaptureWarning>,
    target_path: &Path,
    target_block_id: &str,
) -> Result<MergeInExecution, CaptureError> {
    let snapshot =
        relations_merge::load_staged_snapshot(planner, &request.bob_dir)?;
    let sources = resolve_sources(&snapshot, clause, item_number)?;

    let settings = note_tasks::read_settings(&request.bob_dir);
    let duplicate_index = first_resolved_duplicate(&sources);
    transfer::refuse_resolved_duplicates(&sources).map_err(|error| {
        error.with_item_span(
            item_number,
            duplicate_index
                .and_then(|index| clause.sources.get(index))
                .map(|source| (source.start, source.end))
                .unwrap_or_else(|| clause_span(clause)),
        )
    })?;
    transfer::refuse_overlapping_sources(&snapshot, &sources, &settings)
        .map_err(|error| {
            let span = overlapping_error_span(&error, clause);
            error.with_item_span(item_number, span)
        })?;

    let target_note = target_path
        .strip_prefix(&request.bob_dir)
        .map_err(|error| CaptureError::io(error.to_string()))?
        .with_extension("")
        .to_string_lossy()
        .replace('\\', "/");
    let target = MergeContainer::Task(TaskLocator {
        note: target_note.clone(),
        block_id: target_block_id.to_string(),
    });
    let (output, repairs) = transfer::plan_explicit_merge(
        request, planner, ctx, &snapshot, &sources, target, warnings,
    )
    .map_err(|error| error.with_item_span(item_number, clause_span(clause)))?;

    let final_text = planner.peek_text(target_path).ok_or_else(|| {
        CaptureError::io("merge-in target disappeared from staged text")
    })?;
    let final_scan = note_tasks::scan(&final_text, &settings);
    let final_task = match final_scan.by_block_id(target_block_id) {
        BlockIdLookup::Found(task) => task,
        _ => {
            return Err(CaptureError::io(
                "merge-in target is missing from final staged text",
            ));
        }
    };
    let target_line = final_text
        .lines()
        .nth(final_task.line_index)
        .unwrap_or_default()
        .to_string();
    let target_block_link = format!("[[{target_note}#^{target_block_id}]]");
    let task_block_refs = transfer::merge_task_block_refs(
        planner,
        &request.bob_dir,
        &settings,
        &output,
    );
    let repaired_files = repairs
        .into_iter()
        .map(|file| transfer::TransferRepairedFileJson {
            path: super::dependencies::display_relative(&file.path),
            link_count: file.link_count,
            dependency_token_count: file.dependency_token_count,
        })
        .collect();

    Ok(MergeInExecution {
        output,
        repaired_files,
        task_block_refs,
        target_line,
        target_block_id: target_block_id.to_string(),
        target_block_link,
    })
}

fn resolve_sources(
    snapshot: &VaultSnapshot,
    clause: &MergeInClause,
    item_number: usize,
) -> Result<Vec<(lexer::TransferSource, PathBuf)>, CaptureError> {
    clause
        .sources
        .iter()
        .map(|source| {
            let transfer_source = source_as_transfer(source);
            let path = transfer::resolve_transfer_note(snapshot, &source.route)
                .map_err(|error| {
                    CaptureError::usage_code(error.to_string(), error.code())
                        .with_item_span(item_number, (source.start, source.end))
                })?;
            Ok((transfer_source, path))
        })
        .collect()
}

fn source_as_transfer(source: &ParsedMergeInSource) -> lexer::TransferSource {
    lexer::TransferSource {
        marker: source.marker.clone(),
        route: source.route.clone(),
        block_id: source.block_id.clone(),
        start: source.start,
        end: source.end,
    }
}

fn first_resolved_duplicate(
    sources: &[(lexer::TransferSource, PathBuf)],
) -> Option<usize> {
    let mut seen = HashSet::new();
    sources.iter().position(|(source, path)| {
        !seen.insert((path.clone(), source.block_id.clone()))
    })
}

fn clause_span(clause: &MergeInClause) -> (usize, usize) {
    (
        clause.arrow.0,
        clause
            .sources
            .last()
            .map(|source| source.end)
            .unwrap_or(clause.arrow.1),
    )
}

fn overlapping_error_span(
    error: &CaptureError,
    clause: &MergeInClause,
) -> (usize, usize) {
    clause
        .sources
        .iter()
        .find(|source| {
            error.message.contains(&source.block_id)
                || error.message.contains(&source.marker)
        })
        .map(|source| (source.start, source.end))
        .unwrap_or_else(|| clause_span(clause))
}

fn merge_in_source_also_marked_error(
    item_number: usize,
    span: (usize, usize),
) -> CaptureError {
    CaptureError::usage_code(
        "that source merges into the new task; it can't also be its prerequisite/dependent/relation",
        capture_language::merge_in::MERGE_IN_SOURCE_ALSO_MARKED_CODE,
    )
    .with_item_span(item_number, span)
}
