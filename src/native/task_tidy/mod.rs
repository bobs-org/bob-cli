//! `bob task tidy`: repair open task blocks into the canonical child order
//! (`docs/task-requirements.md` §3.4, "Tidying task blocks").
//!
//! [`tidy_note`] is the pure per-note pass. It walks a note's tasks top to
//! bottom and asks `task_block_layout::plan_tidy` for each **open** task:
//! whole child subtrees move into their zones, and a req-owning task's loose
//! notes are filed into `NOTES` (the project `## Notes` section on a `^prj`
//! owner). A repair rewrites that task's block, and on a `^prj` owner may
//! also write the note's `## Notes` section; every task above it keeps its
//! line. The pass rescans after each repair and resumes just below the
//! repaired task line, which keeps nested subtasks (moved with their parent)
//! in reach. Done and cancelled tasks are never touched: history is never
//! rewritten.
//!
//! The command shell (scope, the dry-run report, and the one-commit apply)
//! lives in [`command`].

use std::collections::BTreeMap;

use super::{
    capture::line_spans,
    note_tasks::{self, NoteTask, NoteTaskSettings},
    task_block_layout::{
        block_last_line, find_task, plan_tidy, TidyPolicy, NOTES_SECTION_TITLE,
        NOTES_TITLE,
    },
};

mod command;

#[cfg(test)]
mod tests;

pub(crate) use command::{build_cli, run};

/// One repaired task block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskRepair {
    /// 0-based line of the task line after the repair. Tasks above a repair
    /// never move, so this is also its line in the tidied note.
    pub(crate) line_index: usize,
    /// The task's clean description.
    pub(crate) text: String,
    pub(crate) block_id: Option<String>,
    /// Whole child subtrees that changed place.
    pub(crate) moves: usize,
    /// Loose notes filed into `NOTES`.
    pub(crate) notes_filed: usize,
    /// `NOTES` was created to hold them.
    pub(crate) notes_created: bool,
    /// The task block before the repair, without line endings.
    pub(crate) before: Vec<String>,
    /// The task block after the repair, without line endings.
    pub(crate) after: Vec<String>,
}

/// One note's tidy outcome. `updated` is `None` when nothing changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteTidy {
    /// Open tasks the pass looked at.
    pub(crate) open_tasks: usize,
    pub(crate) updated: Option<String>,
    pub(crate) repairs: Vec<TaskRepair>,
}

impl NoteTidy {
    pub(crate) fn drawers_created(&self) -> usize {
        self.repairs
            .iter()
            .filter(|repair| repair.notes_created)
            .count()
    }
}

/// Plan every open task block in one note. Returns the note after all
/// repairs, or `None` when every open block is already canonical, so a
/// second pass over a tidied note reports nothing.
pub(crate) fn tidy_note(
    contents: &str,
    settings: &NoteTaskSettings,
) -> NoteTidy {
    let open_tasks = note_tasks::scan(contents, settings)
        .tasks()
        .iter()
        .filter(|task| task.status_type.is_open())
        .count();
    let policy = TidyPolicy { settings };
    let mut current = contents.to_string();
    let mut repairs = Vec::new();
    // Tasks above `cursor` are settled: either canonical or already
    // repaired, and no later repair can move them.
    let mut cursor = 0;
    'rescan: loop {
        let scan = note_tasks::scan(&current, settings);
        for task in scan.tasks() {
            if task.line_index < cursor {
                continue;
            }
            let Some(plan) = plan_tidy(&current, task, &policy) else {
                continue;
            };
            let after = find_task(&plan.updated, task.line_index, settings)
                .map(|repaired| block_excerpt(&plan.updated, &repaired))
                .unwrap_or_default();
            repairs.push(TaskRepair {
                line_index: task.line_index,
                text: task.description.clone(),
                block_id: task.block_id.clone(),
                moves: plan.moves,
                notes_filed: plan.notes_filed,
                notes_created: plan.notes_created,
                before: block_excerpt(&current, task),
                after,
            });
            current = plan.updated;
            cursor = task.line_index + 1;
            continue 'rescan;
        }
        break;
    }
    NoteTidy {
        open_tasks,
        updated: (current != contents).then_some(current),
        repairs,
    }
}

/// The task block's lines without endings, trailing blank lines dropped.
fn block_excerpt(contents: &str, task: &NoteTask) -> Vec<String> {
    let lines = line_spans(contents);
    let last = block_last_line(&lines, task);
    let mut excerpt: Vec<String> = lines[task.line_index..=last]
        .iter()
        .map(|line| line.text.trim_end_matches('\r').to_string())
        .collect();
    while excerpt.len() > 1
        && excerpt.last().is_some_and(|line| line.trim().is_empty())
    {
        excerpt.pop();
    }
    excerpt
}

/// The created task-child `NOTES` header line, trimmed.
fn notes_header() -> String {
    format!("- {NOTES_TITLE}")
}

/// The created `^prj` `## Notes` heading, trimmed.
fn notes_section_header() -> String {
    format!("## {NOTES_SECTION_TITLE}")
}

fn is_created_notes_header(line: &str) -> bool {
    line == notes_header() || line == notes_section_header()
}

/// Check that a tidy only moved, re-indented, and filed lines: every
/// nonblank line of `before` survives exactly once (indentation aside), and
/// the only new lines are the `drawers_created` `NOTES` headers (`- NOTES`
/// or, on a `^prj` owner, `## Notes`). Blank separator lines are ignored.
pub(crate) fn content_preserved(
    before: &str,
    after: &str,
    drawers_created: usize,
) -> Result<(), String> {
    let mut balance: BTreeMap<&str, isize> = BTreeMap::new();
    for line in before
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        *balance.entry(line).or_default() += 1;
    }
    for line in after.lines().map(str::trim).filter(|line| !line.is_empty()) {
        *balance.entry(line).or_default() -= 1;
    }
    let mut headers_added = 0isize;
    for (line, count) in balance {
        if is_created_notes_header(line) {
            headers_added += -count;
            continue;
        }
        if count > 0 {
            return Err(format!("it would drop the line {line:?}"));
        }
        if count < 0 {
            return Err(format!("it would add the line {line:?}"));
        }
    }
    if headers_added != drawers_created as isize {
        return Err(format!(
            "it would add {headers_added} NOTES headers, not {drawers_created}"
        ));
    }
    if drawers_created > 0
        && !after
            .lines()
            .any(|line| is_created_notes_header(line.trim()))
    {
        return Err("it reports a NOTES drawer it never wrote".to_string());
    }
    Ok(())
}

/// Whether `text` is a settled tidy result: no open block plans a repair.
pub(crate) fn settled(
    text: &str,
    settings: &NoteTaskSettings,
) -> Result<(), String> {
    match tidy_note(text, settings).repairs.first() {
        None => Ok(()),
        Some(repair) => Err(format!(
            "a second pass would still repair line {}",
            repair.line_index + 1
        )),
    }
}
