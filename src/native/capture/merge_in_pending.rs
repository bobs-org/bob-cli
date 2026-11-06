//! Read-only JSON context for a merge-in whose sources are still being typed.

use super::*;
use crate::native::capture_language::merge_in::{self as lexer, MergeInScan};
use crate::native::note_tasks::BlockIdLookup;
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
            let mergeability =
                mergeability_json(contents, task, settings, &source.block_id);
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

/// Mergeability for a successfully located source row. Open status,
/// recurrence, project identity, and open subtasks decide; every
/// successfully located row is not reported as mergeable. Reads only,
/// never mints IDs or writes.
fn mergeability_json(
    contents: &str,
    task: &note_tasks::NoteTask,
    settings: &note_tasks::NoteTaskSettings,
    block_id: &str,
) -> Value {
    if !task.status_type.is_open() {
        return json!({
            "mergeable": false,
            "mergeable_code": "merge_source_closed",
            "mergeable_message": format!("^{block_id} is closed"),
        });
    }
    let lines = line_spans(contents);
    if let Some(line) = lines.get(task.line_index)
        && crate::native::task_complete::is_recurring_task_line(line.text)
    {
        return json!({
            "mergeable": false,
            "mergeable_code": "merge_source_recurring",
            "mergeable_message": format!("^{block_id} is recurring"),
        });
    }
    if task.block_id.as_deref() == Some("prj") {
        return json!({
            "mergeable": false,
            "mergeable_code": "merge_source_project",
            "mergeable_message": "the ^prj task cannot be merged".to_string(),
        });
    }
    let scan = note_tasks::scan(contents, settings);
    let open_nested = scan
        .tasks()
        .iter()
        .filter(|candidate| candidate.line_index > task.line_index)
        .filter(|candidate| candidate.status_type.is_open())
        .filter(|candidate| {
            lines
                .get(candidate.line_index)
                .is_some_and(|line| line.end <= task.block_end)
        })
        .count();
    if open_nested > 0 {
        return json!({
            "mergeable": false,
            "mergeable_code": "merge_source_open_subtasks",
            "mergeable_message": format!(
                "“{}” has {open_nested} open subtasks; move or close ^{block_id} first",
                task.description,
            ),
        });
    }
    json!({ "mergeable": true })
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
