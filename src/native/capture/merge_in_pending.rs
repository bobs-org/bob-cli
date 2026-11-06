//! Read-only JSON context for a merge-in whose sources are still being typed.

use super::*;
use crate::native::capture_language::merge_in::{self as lexer, MergeInScan};
use crate::native::note_tasks::BlockIdLookup;
use crate::native::task_merge;
use crate::native::vault_links::NoteIndex;
use serde_json::{json, Value};

pub(super) fn attach(error: &mut CaptureError, request: &CaptureRequest) {
    if !request.dry_run {
        return;
    }
    let Some(code) = error.code.as_deref() else {
        return;
    };
    if code != lexer::INCOMPLETE_MERGE_IN_CODE {
        return;
    }

    let draft = capture_language::split_capture_draft(&request.raw_text);
    let pending = draft.items.iter().find_map(|item| {
        let parent = item.lines.first()?.raw;
        let MergeInScan::Incomplete { sources, .. } = lexer::scan_merge_in_line(
            &parent,
            &request.raw_text,
            item.lines.len() > 1,
        ) else {
            return None;
        };
        Some((item.index + 1, sources))
    });
    let Some((item_number, sources)) = pending else {
        return;
    };

    // The context is best-effort when the vault itself cannot be scanned. A
    // source-level missing/ambiguous note or task is represented below; an
    // I/O failure must not replace the original capture failure.
    let snapshot = match batch_move::destination_snapshot(
        &request.bob_dir,
        &CaptureBatchPlanner::default(),
    ) {
        Ok(snapshot) => snapshot,
        Err(_) => return,
    };
    let settings = note_tasks::read_settings(&request.bob_dir);
    let source_rows = sources
        .iter()
        .map(|source| source_json(source, &snapshot, &settings))
        .collect::<Vec<_>>();

    let mut extra = error.extra.take().unwrap_or_default();
    extra.insert(
        "merge_in_pending".to_string(),
        json!({
            "item": item_number,
            "need": "merge_in_source",
            "sources": source_rows,
        }),
    );
    error.extra = Some(extra);
}

fn source_json(
    source: &lexer::MergeInSource,
    snapshot: &crate::native::task_move::VaultSnapshot,
    settings: &note_tasks::NoteTaskSettings,
) -> Value {
    let path = match transfer::resolve_transfer_note(snapshot, &source.route) {
        Ok(path) => path,
        Err(error) => {
            return json!({
                "marker": source.marker,
                "block_id": source.block_id,
                "error": {"code": error.code(), "message": error.to_string()},
            });
        }
    };
    let relative = dependencies::display_relative(&path);
    let Some(contents) = snapshot.notes.get(&path) else {
        return source_error(
            source,
            "missing_note",
            format!("note {relative} could not be read"),
        );
    };
    let scan = note_tasks::scan(contents, settings);
    match scan.by_block_id(&source.block_id) {
        BlockIdLookup::Found(task) => {
            let title = task.description.clone();
            let mergeability = mergeability_json(
                snapshot,
                &path,
                contents,
                task,
                settings,
                &source.block_id,
            );
            let mut row = json!({
                "marker": source.marker,
                "block_id": source.block_id,
                "path": relative,
                "line": task.line_index + 1,
                "title": title,
                "status_symbol": task.status_symbol.to_string(),
                "status_name": task.status_name,
            });
            if let Some(mergeable) = mergeability.as_object() {
                for (key, value) in mergeable {
                    row[key] = value.clone();
                }
            }
            row
        }
        lookup => {
            let (code, message) = match lookup {
                BlockIdLookup::Missing => ("missing_task", None),
                BlockIdLookup::NotATask { .. } => ("not_a_task", None),
                BlockIdLookup::Duplicate(_) => ("ambiguous_task", None),
                BlockIdLookup::Found(_) => unreachable!(),
            };
            let message = message.unwrap_or_else(|| {
                scan.lookup_capture_task(&relative, &source.block_id)
                    .expect_err("non-found lookup has an error")
            });
            source_error(source, code, message)
        }
    }
}

/// Mergeability for a successfully located source row, decided by the merge
/// engine's own source refusals in execution order: recurring or `^prj`,
/// open subtasks, then closed (with the already-merged teaching copy). Reads
/// only; never mints IDs or writes.
fn mergeability_json(
    snapshot: &crate::native::task_move::VaultSnapshot,
    path: &Path,
    contents: &str,
    task: &note_tasks::NoteTask,
    settings: &note_tasks::NoteTaskSettings,
    block_id: &str,
) -> Value {
    let lines = line_spans(contents);
    let source_line = lines
        .get(task.line_index)
        .map(|line| line.text)
        .unwrap_or_default();
    let scan = note_tasks::scan(contents, settings);
    let refusal =
        task_merge::source_identity_refusal(source_line, task, block_id)
            .or_else(|| {
                task_merge::open_subtasks_refusal(
                    task,
                    task_merge::open_subtask_count(scan.tasks(), &lines, task),
                )
            })
            .or_else(|| {
                (!task.status_type.is_open()).then(|| {
                    let index =
                        NoteIndex::from_paths(snapshot.notes.keys().cloned());
                    task_merge::closed_source_refusal(
                        snapshot, path, block_id, settings, &index,
                    )
                })
            });
    match refusal {
        Some(refusal) => json!({
            "mergeable": false,
            "mergeable_code": refusal.code,
            "mergeable_message": refusal.message,
        }),
        None => json!({ "mergeable": true }),
    }
}

fn source_error(
    source: &lexer::MergeInSource,
    code: &'static str,
    message: String,
) -> Value {
    json!({
        "marker": source.marker,
        "block_id": source.block_id,
        "error": {"code": code, "message": message},
    })
}
