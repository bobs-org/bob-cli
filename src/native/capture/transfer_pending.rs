//! Read-only JSON context for a transfer whose destination is still being typed.

use super::*;
use crate::native::capture_language::transfer::{
    self as lexer, TransferNeed, TransferScan,
};
use crate::native::note_tasks::BlockIdLookup;
use serde_json::{json, Value};

pub(super) fn attach(error: &mut CaptureError, request: &CaptureRequest) {
    if !request.dry_run {
        return;
    }
    let Some(code) = error.code.as_deref() else {
        return;
    };
    if !matches!(
        code,
        "incomplete_transfer"
            | "incomplete_transfer_task"
            | "incomplete_transfer_block_id"
            | "incomplete_transfer_drawer"
            | "incomplete_transfer_section"
    ) {
        return;
    }

    let draft = capture_language::split_capture_draft(&request.raw_text);
    let pending = draft.items.iter().find_map(|item| {
        let parent = item.lines.first()?.raw;
        let TransferScan::Incomplete { sources, need, .. } =
            lexer::scan_transfer_line(
                &parent,
                &request.raw_text,
                item.lines.len() > 1,
            )
        else {
            return None;
        };
        (incomplete_code(need) == code).then_some((
            item.index + 1,
            sources,
            need,
        ))
    });
    let Some((item_number, sources, need)) = pending else {
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
    let forms = [
        ("@note", "move", "move into the note", None),
        (
            "@note+task",
            "merge",
            "merge into a task",
            Some(TransferNeed::BlockId),
        ),
        (
            "@note+task#drawer",
            "merge",
            "merge into a task's drawer",
            Some(TransferNeed::Drawer),
        ),
        (
            "@note#section",
            "merge",
            "merge into a note section",
            Some(TransferNeed::Section),
        ),
    ]
    .into_iter()
    .map(|(form, verb, text, active_need)| {
        json!({
            "form": form,
            "verb": verb,
            "text": text,
            "active": active_need.is_some_and(|active| {
                active == need
                    || (active == TransferNeed::BlockId
                        && need == TransferNeed::Task)
            }),
        })
    })
    .collect::<Vec<_>>();

    let mut extra = error.extra.take().unwrap_or_default();
    extra.insert(
        "transfer_pending".to_string(),
        json!({
            "item": item_number,
            "need": pending_need(need),
            "sources": source_rows,
            "forms": forms,
        }),
    );
    error.extra = Some(extra);
}

fn incomplete_code(need: TransferNeed) -> &'static str {
    match need {
        TransferNeed::Destination => "incomplete_transfer",
        TransferNeed::Task => "incomplete_transfer_task",
        TransferNeed::BlockId => "incomplete_transfer_block_id",
        TransferNeed::Drawer => "incomplete_transfer_drawer",
        TransferNeed::Section => "incomplete_transfer_section",
    }
}

fn pending_need(need: TransferNeed) -> &'static str {
    match need {
        TransferNeed::Destination => "transfer_destination",
        TransferNeed::Task | TransferNeed::BlockId => "block_id",
        TransferNeed::Drawer => "drawer",
        TransferNeed::Section => "section",
    }
}

fn source_json(
    source: &lexer::TransferSource,
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
        BlockIdLookup::Found(task) => json!({
            "marker": source.marker,
            "block_id": source.block_id,
            "path": relative,
            "line": task.line_index + 1,
            "title": task.description,
            "status_symbol": task.status_symbol.to_string(),
            "status_name": task.status_name,
        }),
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

fn source_error(
    source: &lexer::TransferSource,
    code: &'static str,
    message: String,
) -> Value {
    json!({
        "marker": source.marker,
        "block_id": source.block_id,
        "error": {"code": code, "message": message},
    })
}
