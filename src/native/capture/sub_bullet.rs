//! Sub-bullet/note placement and managed-log parsing.
use super::*;

pub(super) fn plan_pomodoro_note_capture(
    planner: &mut CaptureBatchPlanner,
    day_file: &Path,
    capture_block: &str,
) -> Result<CaptureWritePlan, CaptureError> {
    if !day_file.is_file() {
        return Err(CaptureError::io(format!(
            "Bob daily note does not exist: {}",
            day_file.display()
        )));
    }

    let contents = planner.read_existing(day_file)?;
    let (updated, placement, pomodoro_line, pomodoro_text) =
        insert_pomodoro_child_block(
            &contents,
            capture_block,
            PomodoroSelection::CurrentOrLastCompleted,
        )?;
    planner.stage(day_file, updated)?;

    Ok(CaptureWritePlan {
        placement,
        pomodoro: None,
        sub_bullet: None,
        pomodoro_note: Some(PomodoroNoteDetails {
            day_file: day_file.display().to_string(),
            pomodoro_line: pomodoro_line + 1,
            pomodoro_text,
        }),
        toggle: None,
        pomodoro_link: None,
        // Pomodoro notes rely on block auto-detection.
        pomodoro_refs: Vec::new(),
    })
}

pub(super) fn plan_sub_bullet_capture(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target: &Path,
    route: &str,
    sub_bullet_target: &SubBulletTarget,
    section_selector: Option<&TaskSectionSelector>,
    capture_block: &str,
) -> Result<CaptureWritePlan, CaptureError> {
    let contents = planner.read_existing(target)?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let parent = match sub_bullet_target {
        SubBulletTarget::BlockId(block_id) => {
            match scan.by_block_id(block_id) {
                BlockIdLookup::Found(task) => task,
                BlockIdLookup::NotATask {
                    line_index,
                    excerpt,
                } => {
                    return Err(CaptureError::io(format!(
                    "^{block_id} in {route}.md is not a task (line {}: {excerpt})",
                    line_index + 1
                )));
                }
                BlockIdLookup::Duplicate(count) => {
                    return Err(CaptureError::io(format!(
                    "block ID ^{block_id} appears {count} times in {route}.md; make it unique before capturing"
                )));
                }
                BlockIdLookup::Missing => {
                    let choices = format!("run 'bob capture-tasks -r {route}' to list task block IDs");
                    let message = match scan.suggest_block_id(block_id) {
                    Some(suggestion) => format!(
                        "no task with block ID ^{block_id} in {route}.md; did you mean ^{suggestion}? ({choices})"
                    ),
                    None => format!("no task with block ID ^{block_id} in {route}.md ({choices})"),
                };
                    return Err(CaptureError::io(message));
                }
            }
        }
        SubBulletTarget::Ref { line, digest } => {
            match scan.by_ref(*line, digest) {
                RefLookup::Found(task) => task,
                RefLookup::Stale => {
                    return Err(CaptureError::io(format!(
                    "the selected task is no longer in {route}.md; rerun the task picker"
                )));
                }
                RefLookup::Ambiguous => {
                    return Err(CaptureError::io(format!(
                    "the selected task matches more than one line in {route}.md; rerun the task picker"
                )));
                }
            }
        }
    };

    let (insertion_offset, indentation, parent_section) =
        if let Some(selector) = section_selector {
            let sections =
                capture_task_sections::task_sections(&contents, parent);
            let section =
                resolve_parent_section(route, parent, selector, &sections)?;
            let insertion =
                capture_task_sections::section_insertion(&contents, section);
            (
                insertion.offset,
                insertion.indentation,
                Some(section.title.clone()),
            )
        } else {
            let lines = line_spans(&contents);
            let indentation = first_child_indentation(
                &lines,
                parent.line_index,
                parent.block_end,
                &parent.indentation,
            )
            .or_else(|| {
                dominant_indent_unit(&lines)
                    .map(|unit| format!("{}{}", parent.indentation, unit))
            })
            .unwrap_or_else(|| format!("{}\t", parent.indentation));
            let insertion_offset = first_direct_managed_log_start(
                &lines,
                parent.line_index,
                parent.block_end,
            )
            .unwrap_or(parent.block_end);
            (insertion_offset, indentation, None)
        };
    let indented_block = capture_block
        .split('\n')
        .map(|line| format!("{indentation}{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let addition = insertion_text_preserving_line_endings(
        &contents,
        insertion_offset,
        &indented_block,
    );
    let placement = if insertion_offset >= contents.len() {
        Placement::Appended
    } else {
        Placement::Inserted
    };
    let details = SubBulletCaptureDetails {
        block_id: parent.block_id.clone(),
        parent_line: parent.line_index + 1,
        parent_text: parent.description.clone(),
        parent_section,
        parent_status_symbol: parent.status_symbol,
        parent_status_name: parent.status_name.clone(),
    };

    let updated_target = insert_at(&contents, insertion_offset, &addition);
    planner.stage(target, updated_target)?;

    Ok(CaptureWritePlan {
        placement,
        pomodoro: None,
        sub_bullet: Some(details),
        pomodoro_note: None,
        toggle: None,
        pomodoro_link: None,
        pomodoro_refs: Vec::new(),
    })
}

pub(super) const MAX_LISTED_SECTION_TITLES: usize = 8;

pub(super) fn resolve_parent_section<'a>(
    route: &str,
    parent: &note_tasks::NoteTask,
    selector: &TaskSectionSelector,
    sections: &'a [capture_task_sections::TaskSection],
) -> Result<&'a capture_task_sections::TaskSection, CaptureError> {
    if sections.is_empty() {
        return Err(CaptureError::io(format!(
            "{} has no task sections ({})",
            parent_task_label(route, parent),
            capture_task_sections_hint(route, parent),
        )));
    }
    if let Some(section) = capture_task_sections::match_section(
        sections,
        &selector.text,
        selector.exact,
    ) {
        return Ok(section);
    }
    let listed = format_section_titles(sections);
    let suggestion = capture_task_sections::suggest_section(
        sections,
        &selector.text,
        selector.exact,
    )
    .map(|section| format!("; did you mean {}?", section.title))
    .unwrap_or_default();
    Err(CaptureError::io(format!(
        "no task section matching '{}' under {}{suggestion} (have: {listed}; {})",
        selector.text,
        parent_under_label(route, parent),
        capture_task_sections_hint(route, parent),
    )))
}

pub(super) fn parent_task_label(
    route: &str,
    parent: &note_tasks::NoteTask,
) -> String {
    match parent.block_id.as_deref() {
        Some(block_id) => format!("task ^{block_id} in {route}.md"),
        None => format!("the selected task in {route}.md"),
    }
}

pub(super) fn parent_under_label(
    route: &str,
    parent: &note_tasks::NoteTask,
) -> String {
    match parent.block_id.as_deref() {
        Some(block_id) => format!("^{block_id} in {route}.md"),
        None => format!("the selected task in {route}.md"),
    }
}

pub(super) fn capture_task_sections_hint(
    route: &str,
    parent: &note_tasks::NoteTask,
) -> String {
    match parent.block_id.as_deref() {
        Some(block_id) => {
            format!("run 'bob capture-task-sections -r {route} -i {block_id}' to list them")
        }
        None => {
            format!("run 'bob capture-task-sections -r {route}' to list them")
        }
    }
}

pub(super) fn format_section_titles(
    sections: &[capture_task_sections::TaskSection],
) -> String {
    let mut listed = sections
        .iter()
        .take(MAX_LISTED_SECTION_TITLES)
        .map(|section| section.title.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if sections.len() > MAX_LISTED_SECTION_TITLES {
        listed.push_str(", ...");
    }
    listed
}

pub(crate) fn first_child_indentation(
    lines: &[LineSpan<'_>],
    parent_line_index: usize,
    block_end: usize,
    parent_indentation: &str,
) -> Option<String> {
    lines[parent_line_index + 1..]
        .iter()
        .take_while(|line| line.end <= block_end)
        .filter(|line| !line.text.trim().is_empty())
        .map(|line| leading_whitespace(line.text))
        .find(|indentation| indentation.len() > parent_indentation.len())
        .map(str::to_string)
}

// Plugin-compatible Schedule Log / Work Log markers: optional matching emoji,
// the canonical or legacy bold label, an optional colon, and no trailing
// text. Direct-child ancestry skips blanks and uses the nearest shallower
// list item, so a log nested under another child does not move insertion.
// Parity note: the navigation-hotkeys-only `❌ **CANCEL LOG**` is
// deliberately NOT a managed-log anchor here. It is placed first under the
// task, so newly captured notes land below it via the Schedule/Work Log
// anchors, and emoji-led bullets are never task sections anyway.
pub(super) const SCHEDULE_LOG_EMOJI: &str = "🗓️";
pub(super) const WORK_LOG_EMOJI: &str = "🛠️";
pub(super) const MANAGED_TASK_LOG_LABELS: &[(&str, ManagedTaskLogKind)] = &[
    ("SCHEDULE LOG", ManagedTaskLogKind::Schedule),
    ("Schedule log", ManagedTaskLogKind::Schedule),
    ("WORK LOG", ManagedTaskLogKind::Work),
    ("Work log", ManagedTaskLogKind::Work),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedTaskLogKind {
    Schedule,
    Work,
}

pub(crate) fn first_direct_managed_log_start(
    lines: &[LineSpan<'_>],
    parent_line_index: usize,
    block_end: usize,
) -> Option<usize> {
    let mut line_start = lines[parent_line_index].end;
    for (offset, line) in lines[parent_line_index + 1..].iter().enumerate() {
        if line.end > block_end {
            break;
        }
        let line_index = parent_line_index + 1 + offset;
        if !line.text.trim().is_empty()
            && parse_managed_task_log_marker(line.text).is_some()
            && nearest_shallower_list_item_parent(lines, line_index)
                == Some(parent_line_index)
        {
            return Some(line_start);
        }
        line_start = line.end;
    }
    None
}

pub(crate) fn nearest_shallower_list_item_parent(
    lines: &[LineSpan<'_>],
    child_index: usize,
) -> Option<usize> {
    if child_index == 0 {
        return None;
    }
    let child_indent = leading_spaces_or_tabs_len(lines[child_index].text);
    for index in (0..child_index).rev() {
        let text = lines[index].text;
        if text.trim().is_empty()
            || leading_spaces_or_tabs_len(text) >= child_indent
        {
            continue;
        }
        if list_item_body(text).is_some() {
            return Some(index);
        }
    }
    None
}

pub(crate) fn parse_managed_task_log_marker(
    line: &str,
) -> Option<ManagedTaskLogKind> {
    let rest = list_item_body(line)?;
    if let Some(after_emoji) = strip_log_emoji(rest, SCHEDULE_LOG_EMOJI) {
        return parse_managed_task_log_label(after_emoji)
            .filter(|kind| *kind == ManagedTaskLogKind::Schedule);
    }
    if let Some(after_emoji) = strip_log_emoji(rest, WORK_LOG_EMOJI) {
        return parse_managed_task_log_label(after_emoji)
            .filter(|kind| *kind == ManagedTaskLogKind::Work);
    }
    parse_managed_task_log_label(rest)
}

pub(super) fn parse_managed_task_log_label(
    rest: &str,
) -> Option<ManagedTaskLogKind> {
    let rest = rest.strip_prefix("**")?;
    for (label, kind) in MANAGED_TASK_LOG_LABELS {
        let Some(after_label) = rest.strip_prefix(label) else {
            continue;
        };
        let after_label = after_label.strip_prefix(':').unwrap_or(after_label);
        let Some(after_close) = after_label.strip_prefix("**") else {
            continue;
        };
        if after_close.bytes().all(|byte| matches!(byte, b' ' | b'\t')) {
            return Some(*kind);
        }
    }
    None
}

pub(super) fn strip_log_emoji<'a>(
    rest: &'a str,
    emoji: &str,
) -> Option<&'a str> {
    let after_emoji = rest.strip_prefix(emoji)?;
    let whitespace = leading_spaces_or_tabs_len(after_emoji);
    (whitespace > 0).then_some(&after_emoji[whitespace..])
}

pub(crate) fn list_item_body(line: &str) -> Option<&str> {
    let indent_len = leading_spaces_or_tabs_len(line);
    let after_indent = &line[indent_len..];
    let marker_len = list_marker_len(after_indent)?;
    let after_marker = &after_indent[marker_len..];
    let whitespace = leading_spaces_or_tabs_len(after_marker);
    (whitespace > 0).then_some(&after_marker[whitespace..])
}

pub(crate) fn list_marker_len(after_indent: &str) -> Option<usize> {
    let bytes = after_indent.as_bytes();
    match bytes.first() {
        Some(b'-' | b'*' | b'+') => Some(1),
        Some(b'0'..=b'9') => {
            let digits = bytes
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            match bytes.get(digits) {
                Some(b'.' | b')') => Some(digits + 1),
                _ => None,
            }
        }
        _ => None,
    }
}

pub(crate) fn leading_spaces_or_tabs_len(text: &str) -> usize {
    text.as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

pub(crate) fn dominant_indent_unit(
    lines: &[LineSpan<'_>],
) -> Option<&'static str> {
    let (tabs, spaces) = lines.iter().fold((0usize, 0usize), |counts, line| {
        match line.text.as_bytes().first() {
            Some(b'\t') => (counts.0 + 1, counts.1),
            Some(b' ') => (counts.0, counts.1 + 1),
            _ => counts,
        }
    });
    if tabs + spaces == 0 {
        None
    } else if tabs > spaces {
        Some("\t")
    } else {
        Some("  ")
    }
}

pub(super) fn child_indent_unit(
    planner: &mut CaptureBatchPlanner,
    target: &Path,
) -> Result<String, CaptureError> {
    let Some(contents) = planner.current_contents(target)? else {
        return Ok("\t".to_string());
    };
    Ok(dominant_indent_unit(&line_spans(&contents))
        .unwrap_or("\t")
        .to_string())
}

pub(crate) fn leading_whitespace(line: &str) -> &str {
    let end = line
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(line.len());
    &line[..end]
}
