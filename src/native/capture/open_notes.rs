//! Batch-level `open_notes` for `bob capture --format json`.
//!
//! Built from the planned batch before commit so dry-run and real-run
//! JSON match. Clients pass these entries through; they never re-derive
//! paths, lines, or digests.

use super::*;

/// One note Control-Return should open, in first-touch order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct OpenNoteJson {
    pub(super) path: String,
    pub(super) relative_path: String,
    pub(super) role: &'static str,
    pub(super) label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) r#ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) block_id: Option<String>,
}

/// Notes this capture should open: each item's `target` in source order,
/// then the day file when the batch changed it. Deduped by path; the
/// first touch decides the landing. An entry that would land on a routed
/// task opens the destination at the moved line instead.
pub(super) fn build_open_notes(
    bob_dir: &Path,
    batch: &PlannedCaptureBatch,
) -> Vec<OpenNoteJson> {
    let day_abs = absolute_day_file(bob_dir);
    let day_relative = relative_path(bob_dir, &day_abs);
    let settings = note_tasks::read_settings(bob_dir);
    let first_pomodoro =
        first_pomodoro_line(&batch.pomodoro_blocks, &day_relative);
    let mut notes = Vec::new();
    let mut seen = Vec::new();

    for item in batch.items.iter().map(|item| &item.result) {
        if let Some(movement) = routed_landing(&batch.routed_landings, item) {
            if seen.iter().any(|seen| seen == &movement.dest) {
                continue;
            }
            seen.push(movement.dest.clone());
            notes.push(open_note_for_routed_task(
                bob_dir,
                movement,
                &batch.text_files,
            ));
            continue;
        }
        let Some(path) = resolve_path(bob_dir, &item.target) else {
            continue;
        };
        if seen.iter().any(|seen| seen == &path) {
            continue;
        }
        seen.push(path.clone());
        notes.push(open_note_for_item(
            bob_dir,
            item,
            &path,
            &day_abs,
            &batch.text_files,
            &batch.task_blocks,
            first_pomodoro,
            &settings,
        ));
    }

    let day_changed = batch.text_files.iter().any(|file| {
        resolve_staged_path(bob_dir, &file.target) == day_abs
            && file.updated_target != file.original_target
    });
    if day_changed && !seen.iter().any(|seen| seen == &day_abs) {
        let text = file_text(bob_dir, &batch.text_files, &day_abs);
        notes.push(landed_note(
            OpenNoteJson {
                path: day_abs.display().to_string(),
                relative_path: day_relative,
                role: "day",
                label: "Today".to_string(),
                line: None,
                r#ref: None,
                block_id: None,
            },
            text.as_deref(),
            first_pomodoro,
        ));
    }
    notes
}

/// An item whose target task moved to another note in this batch: the
/// entry opens the destination at the moved line. Only tasks with block
/// IDs reroute; id-less landings keep today's source-note behavior.
fn routed_landing<'a>(
    landings: &'a [RoutedTaskMove],
    item: &CaptureItemResult,
) -> Option<&'a RoutedTaskMove> {
    let block_id = item.block_id.as_deref().filter(|id| !id.is_empty())?;
    landings.iter().find(|movement| {
        movement.source_relative == item.relative_target
            && movement.old_block_id.as_deref() == Some(block_id)
    })
}

fn open_note_for_routed_task(
    bob_dir: &Path,
    movement: &RoutedTaskMove,
    text_files: &[StagedTextFile],
) -> OpenNoteJson {
    let label = movement
        .dest_relative
        .strip_suffix(".md")
        .unwrap_or(&movement.dest_relative)
        .rsplit('/')
        .next()
        .unwrap_or(&movement.dest_relative)
        .to_string();
    let text = file_text(bob_dir, text_files, &movement.dest);
    landed_note(
        OpenNoteJson {
            path: movement.dest.display().to_string(),
            relative_path: movement.dest_relative.clone(),
            role: "note",
            label,
            line: None,
            r#ref: None,
            block_id: None,
        },
        text.as_deref(),
        Some(movement.new_line),
    )
}

fn open_note_for_item(
    bob_dir: &Path,
    item: &CaptureItemResult,
    path: &Path,
    day_abs: &Path,
    text_files: &[StagedTextFile],
    task_blocks: &[TaskBlockJson],
    first_pomodoro: Option<usize>,
    settings: &note_tasks::NoteTaskSettings,
) -> OpenNoteJson {
    let path_is_day = path == day_abs;
    let relative = if !item.relative_target.is_empty() {
        item.relative_target.clone()
    } else {
        relative_path(bob_dir, path)
    };
    let (role, label) = if path_is_day {
        ("day", "Today".to_string())
    } else {
        ("note", note_label(item, &relative))
    };
    let text = file_text(bob_dir, text_files, path);
    let line = text.as_deref().and_then(|text| {
        note_landing_line(
            item,
            text,
            task_blocks,
            first_pomodoro,
            settings,
            path_is_day,
        )
    });
    landed_note(
        OpenNoteJson {
            path: path.display().to_string(),
            relative_path: relative,
            role,
            label,
            line: None,
            r#ref: None,
            block_id: None,
        },
        text.as_deref(),
        line,
    )
}

fn note_landing_line(
    item: &CaptureItemResult,
    text: &str,
    task_blocks: &[TaskBlockJson],
    first_pomodoro: Option<usize>,
    settings: &note_tasks::NoteTaskSettings,
    path_is_day: bool,
) -> Option<usize> {
    match item.kind {
        "ref" => None,
        "pomodoro_start" | "pomodoro_adjust" | "pomodoro_shift"
        | "pomodoro_close" | "pomodoro_reset" => first_pomodoro,
        "task_complete" => task_block_line(
            task_blocks,
            &item.relative_target,
            TaskBlockRole::Completed,
            item.block_id.as_deref(),
        )
        .or_else(|| line_for_block_id(text, item.block_id.as_deref(), settings))
        .or_else(|| last_exact_line(text, &item.task_line)),
        "task_dependency" => task_block_line(
            task_blocks,
            &item.relative_target,
            TaskBlockRole::Dependency,
            item.block_id.as_deref(),
        )
        .or_else(|| line_for_block_id(text, item.block_id.as_deref(), settings))
        .or_else(|| last_exact_line(text, &item.task_line)),
        "sub_bullet" => first_added_child_line(item, task_blocks, text)
            .or_else(|| first_indented_match(text, &item.task_line)),
        "task_toggle" | "link_guard" | "pomodoro_link" | "pomodoro_task" => {
            line_for_block_id(text, item.block_id.as_deref(), settings)
                .or_else(|| last_exact_line(text, &item.task_line))
        }
        "task" | "bullet" | "project_note" => {
            last_exact_line(text, &item.task_line).or_else(|| {
                line_for_block_id(text, item.block_id.as_deref(), settings)
            })
        }
        "pomodoro_note" if path_is_day => {
            first_indented_match(text, &item.task_line).or(first_pomodoro)
        }
        _ if path_is_day => first_pomodoro.or_else(|| {
            last_exact_line(text, &item.task_line)
                .or_else(|| first_indented_match(text, &item.task_line))
        }),
        _ => last_exact_line(text, &item.task_line).or_else(|| {
            line_for_block_id(text, item.block_id.as_deref(), settings)
        }),
    }
}

fn landed_note(
    mut note: OpenNoteJson,
    text: Option<&str>,
    line: Option<usize>,
) -> OpenNoteJson {
    let Some(line) = line.filter(|line| *line > 0) else {
        return note;
    };
    let Some(physical) = text.and_then(|text| text.lines().nth(line - 1))
    else {
        return note;
    };
    note.line = Some(line);
    note.r#ref = Some(
        note_tasks::TaskRef {
            line,
            digest: note_tasks::line_digest(physical),
        }
        .to_string(),
    );
    note.block_id =
        crate::native::collect_done::trailing_block_id_in_line(physical);
    note
}

fn first_pomodoro_line(
    blocks: &[PomodoroBlockJson],
    day_relative: &str,
) -> Option<usize> {
    blocks.iter().find_map(|block| {
        let removed =
            block.removed || block.roles.contains(&PomodoroBlockRole::Removed);
        (block.relative_target == day_relative && !removed)
            .then_some(block.line)
    })
}

fn task_block_line(
    blocks: &[TaskBlockJson],
    relative: &str,
    role: TaskBlockRole,
    block_id: Option<&str>,
) -> Option<usize> {
    if let Some(id) = block_id.filter(|id| !id.is_empty()) {
        return blocks.iter().find_map(|block| {
            (block.relative_target == relative
                && block.roles.contains(&role)
                && block.block_id.as_deref() == Some(id))
            .then_some(block.line)
        });
    }
    blocks.iter().find_map(|block| {
        (block.relative_target == relative && block.roles.contains(&role))
            .then_some(block.line)
    })
}

fn first_added_child_line(
    item: &CaptureItemResult,
    task_blocks: &[TaskBlockJson],
    _text: &str,
) -> Option<usize> {
    let block = if let Some(id) =
        item.block_id.as_deref().filter(|id| !id.is_empty())
    {
        task_blocks.iter().find(|block| {
            block.relative_target == item.relative_target
                && block.block_id.as_deref() == Some(id)
        })
    } else {
        task_blocks.iter().find(|block| {
            block.relative_target == item.relative_target
                && block.roles.contains(&TaskBlockRole::SubBullet)
        })
    }?;
    let offset = block
        .lines
        .iter()
        .filter(|line| line.change != BlockLineChange::Removed)
        .position(|line| {
            line.change == BlockLineChange::Added && line.depth > 0
        })?;
    Some(block.line + offset)
}

fn line_for_block_id(
    text: &str,
    block_id: Option<&str>,
    settings: &note_tasks::NoteTaskSettings,
) -> Option<usize> {
    let block_id = block_id.filter(|id| !id.is_empty())?;
    match note_tasks::scan(text, settings).by_block_id(block_id) {
        note_tasks::BlockIdLookup::Found(task) => Some(task.line_index + 1),
        _ => None,
    }
}

/// 0-based last full-line match of `needle`. Shared with
/// [`locate_new_task_ref`].
pub(super) fn last_exact_line_index(text: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    text.lines()
        .enumerate()
        .filter(|(_, line)| *line == needle)
        .last()
        .map(|(index, _)| index)
}

fn last_exact_line(text: &str, needle: &str) -> Option<usize> {
    last_exact_line_index(text, needle).map(|index| index + 1)
}

fn first_exact_line(text: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    text.lines()
        .position(|line| line == needle)
        .map(|index| index + 1)
}

fn first_indented_match(text: &str, task_line: &str) -> Option<usize> {
    first_exact_line(text, task_line).or_else(|| {
        if task_line.is_empty() {
            return None;
        }
        let trimmed = task_line.trim_start();
        text.lines()
            .position(|line| line.trim_start() == trimmed)
            .map(|index| index + 1)
    })
}

fn note_label(item: &CaptureItemResult, relative_path: &str) -> String {
    if !item.route_label.is_empty() {
        item.route_label.clone()
    } else {
        relative_path
            .strip_suffix(".md")
            .unwrap_or(relative_path)
            .to_string()
    }
}

fn absolute_day_file(bob_dir: &Path) -> PathBuf {
    let day = pomodoro::day_file_for(bob_dir);
    if day.is_absolute() {
        day
    } else {
        bob_dir.join(day)
    }
}

fn resolve_path(bob_dir: &Path, path: &str) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    Some(resolve_staged_path(bob_dir, Path::new(path)))
}

fn resolve_staged_path(bob_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        bob_dir.join(path)
    }
}

fn relative_path(bob_dir: &Path, absolute: &Path) -> String {
    absolute
        .strip_prefix(bob_dir)
        .map(display_relative)
        .unwrap_or_else(|_| absolute.display().to_string())
}

fn file_text(
    bob_dir: &Path,
    text_files: &[StagedTextFile],
    path: &Path,
) -> Option<String> {
    text_files
        .iter()
        .find(|file| resolve_staged_path(bob_dir, &file.target) == path)
        .map(|file| file.updated_target.clone())
        .or_else(|| fs::read_to_string(path).ok())
}
