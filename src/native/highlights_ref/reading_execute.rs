//! Guarded v2 reading-task execution (phase `v2-execution`).
//!
//! Side-effectful counterpart to [`plan_reading_task`]: it consumes
//! [`ReadingTaskPlan`] actions — insert, exact line edit, or adoption —
//! called from the scan entrypoints' v2 execution path.
//!
//! Mandated per-PDF write order: destination
//! reading-task and routed insertion actions first, then the explicitly
//! authorized PDF marker write, then the ref note, using the actual final ID
//! and refreshed metadata from [`ReadingTaskExecution`]. A detected changed
//! reading line fails here, before any of that PDF's destination, marker, or
//! ref-note writes begin.
//!
//! This is deliberately not a multi-file transaction and keeps no birth
//! journal: when a later marker/ref write fails after the parent succeeded,
//! a rerun adopts the existing task (same ID, same residence) and finishes
//! without duplication.

use super::*;
use crate::native::collect_done::trailing_block_id_in_line as ref_trailing_block_id;
use crate::native::ref_tasks::{
    edit_reading_task_checkbox, find_managed_embed,
    insert_ref_task_with_preferred_id, locate_original_line,
    render_ref_task_line, slug_ref_stem, strip_blockquote_prefix, task_mark,
    LocatedRefTask, READING_TASK_CHANGED,
};

/// What one executed reading-task action did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadingTaskExecutionAction {
    /// A fresh line was inserted (birth or reopen).
    Inserted,
    /// The located line's checkbox (and maybe its close stamp) was edited.
    LineEdited,
}

/// The execution result one PDF's ref-note rendering and reports consume
/// instead of preview IDs: the actual destination, final block ID, final
/// task line, and what happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReadingTaskExecution {
    pub destination: PathBuf,
    pub block_id: String,
    pub task_line: String,
    pub action: ReadingTaskExecutionAction,
}

/// Inputs to [`execute_reading_insert`]: everything the plan's
/// `Insert` action leaves to the caller's PDF context.
pub(super) struct ReadingInsertInputs<'a> {
    pub bob_dir: &'a Path,
    /// Canonical route (`sase`, `mac_inbox`, …); the destination is
    /// `<bob_dir>/<route>.md`.
    pub destination_route: &'a str,
    /// Ref-note link target (`ref/<type>/<stem>`), never the PDF.
    pub ref_target: &'a str,
    /// Ref-note stem, seeding ID allocation.
    pub ref_stem: &'a str,
    pub title_raw: &'a str,
    /// Invocation date (`YYYY-MM-DD`) for `[created::]` and close stamps.
    pub created: &'a str,
    pub mark: char,
    /// Already-formatted warning text, indented as a child bullet here.
    pub warning_child: Option<String>,
    /// A reopen's old (possibly user-renamed) address, reused when free.
    pub prefer_block_id: Option<&'a str>,
}

/// Execute a birth or reopen insertion: render the reading line with a
/// preview ID, allocate the final ID against fresh destination bytes (plus
/// its archive and any preferred reopen ID), and write through capture's
/// guarded insertion path.
pub(super) fn execute_reading_insert(
    inputs: ReadingInsertInputs<'_>,
) -> Result<ReadingTaskExecution> {
    let destination = inputs
        .bob_dir
        .join(format!("{}.md", inputs.destination_route));
    let preview = render_ref_task_line(
        inputs.mark,
        inputs.ref_target,
        inputs.title_raw,
        inputs.created,
        &format!("ref-{}", slug_ref_stem(inputs.ref_stem)),
    );
    let children = inputs
        .warning_child
        .map(|warning| format!("  - {warning}"))
        .into_iter()
        .collect::<Vec<_>>();
    let inserted = insert_ref_task_with_preferred_id(
        inputs.bob_dir,
        &destination,
        &preview,
        &children,
        inputs.prefer_block_id,
    )
    .map_err(CommandError::new)?;
    Ok(ReadingTaskExecution {
        destination,
        block_id: inserted.block_id,
        task_line: inserted.task_line,
        action: ReadingTaskExecutionAction::Inserted,
    })
}

/// Execute a planned checkbox edit on the located task line.
///
/// Archived tasks are refused: scan never mutates `done/`. A same-mark
/// terminal task that only needs its stamp is still refused here — the
/// planner routes archive reopens through insertion instead.
pub(super) fn execute_reading_line_edit(
    bob_dir: &Path,
    task: &LocatedRefTask,
    target_mark: char,
) -> Result<ReadingTaskExecution> {
    if task.archived {
        return Err(CommandError::new(format!(
            "refusing to edit archived reading task {} (scan never mutates done/)",
            task.path
        )));
    }
    let destination = bob_dir.join(&task.path);
    let edited = edit_reading_task_checkbox(
        &destination,
        task.line_index,
        &task.line,
        target_mark,
    )
    .map_err(CommandError::new)?;
    let block_id = ref_trailing_block_id(&edited.task_line)
        .or_else(|| task.block_id.clone())
        .unwrap_or_default();
    Ok(ReadingTaskExecution {
        destination,
        block_id,
        task_line: edited.task_line,
        action: ReadingTaskExecutionAction::LineEdited,
    })
}

/// Revalidate an adopted or otherwise selected task immediately before its
/// address or status is used to write the v2 ref note — even when no
/// checkbox edit is needed. A moved line refreshes its index; a moved block
/// ID or mark flows into the healed embed and parent. Changed, deleted, or
/// ambiguous originals fail with the reading-task-changed error so no stale
/// healed embed or parent is written.
pub(super) fn revalidate_located_task(
    bob_dir: &Path,
    task: &LocatedRefTask,
) -> Result<LocatedRefTask> {
    let destination = bob_dir.join(&task.path);
    let contents = match fs::read_to_string(&destination) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CommandError::new(READING_TASK_CHANGED));
        }
        Err(error) => {
            return Err(CommandError::new(format!(
                "read {}: {error}",
                destination.display()
            )));
        }
    };
    let line_index =
        locate_original_line(&contents, task.line_index, &task.line)
            .map_err(CommandError::new)?;
    let current: String =
        contents.lines().nth(line_index).unwrap_or("").to_string();
    let Some(mark) = task_mark(strip_blockquote_prefix(&current)) else {
        return Err(CommandError::new(READING_TASK_CHANGED));
    };
    Ok(LocatedRefTask {
        path: task.path.clone(),
        line_index,
        line: current.clone(),
        mark,
        block_id: ref_trailing_block_id(&current)
            .or_else(|| task.block_id.clone()),
        archived: task.archived,
        residence: task.residence.clone(),
        closed_on: task.closed_on.clone(),
        in_capture_target: task.in_capture_target,
    })
}

/// Strip every managed-anatomy embed from `body`, preserving endings.
fn without_managed_embeds(body: &str) -> String {
    let mut current = body.to_string();
    while find_managed_embed(&current).is_some() {
        let found = find_managed_embed(&current).expect("just checked");
        let (mut lines, ending, trailing) = split_body_lines(&current);
        if found.line_index >= lines.len() {
            break;
        }
        lines.remove(found.line_index);
        current = join_body_lines(&lines, ending, trailing);
    }
    current
}

/// True when the two bodies differ only inside the managed embed slot:
/// deletion, insertion, repointing, or duplication of the canonical
/// `![[<target>#^<id>]]` line. Authored material around the slot — including
/// unrelated embeds elsewhere — must match byte-for-byte.
pub(super) fn v2_body_change_confined_to_managed_embed(
    base_body: &str,
    current_body: &str,
) -> bool {
    without_managed_embeds(base_body) == without_managed_embeds(current_body)
}

/// True when the two full note contents differ in frontmatter only through
/// the `parent` key (added, removed, or repointed). The residence parent is
/// rendered from the located task and excluded from the sync snapshot, so a
/// parent-only move is not a conflicting edit.
pub(super) fn frontmatter_change_is_residence_only(
    base_contents: &str,
    current_contents: &str,
) -> bool {
    let (Some((base_frontmatter, _)), Some((current_frontmatter, _))) = (
        split_frontmatter(base_contents),
        split_frontmatter(current_contents),
    ) else {
        return false;
    };
    let mut base_rest = BTreeMap::new();
    for raw in &base_frontmatter {
        let entry = parse_frontmatter_entry(raw);
        if entry.key.as_deref() != Some(FIELD_PARENT) {
            base_rest.insert(entry.key.clone(), entry.raw.clone());
        }
    }
    let mut current_rest = BTreeMap::new();
    for raw in &current_frontmatter {
        let entry = parse_frontmatter_entry(raw);
        if entry.key.as_deref() != Some(FIELD_PARENT) {
            current_rest.insert(entry.key.clone(), entry.raw.clone());
        }
    }
    base_rest == current_rest
}

/// The v2 ref-note dirty allowance: tracked modifications confined to the
/// managed embed (deletion, insertion, repointing) plus permitted
/// frontmatter edits. A residence-only frontmatter change is allowed even
/// though `parent` is excluded from the sync contribution. Unrelated body
/// edits still refuse. The v1 guard is untouched — this predicate only runs
/// on the v2 branch, so v1 notes keep their exact checkbox-only allowance.
pub(super) fn v2_dirty_note_allowed(
    base_contents: &str,
    current_contents: &str,
    frontmatter_contributed: bool,
) -> bool {
    if base_contents == current_contents {
        return true;
    }
    let (Some((_, base_body)), Some((_, current_body))) = (
        split_frontmatter(base_contents),
        split_frontmatter(current_contents),
    ) else {
        return v2_body_change_confined_to_managed_embed(
            base_contents,
            current_contents,
        );
    };
    if !v2_body_change_confined_to_managed_embed(&base_body, &current_body) {
        return false;
    }
    let (Some((base_frontmatter, _)), Some((current_frontmatter, _))) = (
        split_frontmatter(base_contents),
        split_frontmatter(current_contents),
    ) else {
        return false;
    };
    if base_frontmatter == current_frontmatter {
        return true;
    }
    frontmatter_contributed
        || frontmatter_change_is_residence_only(base_contents, current_contents)
}
