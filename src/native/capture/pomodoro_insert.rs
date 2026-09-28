//! Pomodoro block/child insertion helpers.
use super::*;

/// Which ledger entry a daily-note child insertion attaches to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PomodoroSelection<'a> {
    /// Current Pomodoro, else the first future one. Used by `@<route>:<id>`
    /// ledger links, which must attach to a Pomodoro that is still open.
    CurrentOrFuture,
    /// Current Pomodoro, else the last completed one, else the first future
    /// one. Used by the bare `#` Pomodoro-note marker.
    CurrentOrLastCompleted,
    /// Explicit named selector from `@<route>:<id>#<pomodoro>`. Resolves
    /// an open match first, otherwise creates a named future entry using the
    /// current/last-completed/first insertion anchor.
    NamedOrCreate(&'a str),
}

impl PomodoroSelection<'_> {
    pub(super) fn no_entry_message(self) -> &'static str {
        match self {
            Self::CurrentOrFuture | Self::NamedOrCreate(_) => {
                "Bob daily note has no eligible open Pomodoro"
            }
            Self::CurrentOrLastCompleted => {
                "Bob daily note has no eligible Pomodoro"
            }
        }
    }
}

pub(super) fn insert_pomodoro_block_link(
    contents: &str,
    block_link: &str,
    pomodoro_name: Option<&str>,
) -> Result<(String, Placement), CaptureError> {
    let selection = match pomodoro_name {
        Some(name) => PomodoroSelection::NamedOrCreate(name),
        None => PomodoroSelection::CurrentOrFuture,
    };
    let (updated, placement, _, _) = insert_pomodoro_child_block(
        contents,
        &format!("- {block_link}"),
        selection,
    )?;
    Ok((updated, placement))
}

/// Select a Pomodoro in the daily note's `## Pomodoros` section according to
/// `selection` and insert `block` -- indented to match the entry's existing
/// children -- at the end of that entry's child block.
///
/// `CurrentOrFuture` prefers the single open timed entry, else the first
/// open entry. `CurrentOrLastCompleted` prefers the single open timed
/// entry, else the last completed entry, else the first open entry.
/// `NamedOrCreate` resolves an explicit slug against open named entries and
/// skips the multiple-open-timed guard when it finds one. If no open name
/// matches, it creates a new named placeholder after the current entry, else
/// after the last completed entry, else before the first Pomodoro.
/// Returns the updated contents, the placement, the selected entry's
/// 0-based line index, and its ledger task text (trimmed of the leading
/// checkbox but not of its `(start-end)` time range or bracket fields).
pub(super) fn insert_pomodoro_child_block(
    contents: &str,
    block: &str,
    selection: PomodoroSelection<'_>,
) -> Result<(String, Placement, usize, String), CaptureError> {
    let lines = line_spans(contents);
    let line_text = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let section =
        pomodoro::pomodoros_section_range(&line_text).ok_or_else(|| {
            CaptureError::io("Bob daily note has no Pomodoros section")
        })?;

    let mut open = Vec::new();
    let mut timed = Vec::new();
    let mut completed = Vec::new();
    let fenced = super::markdown::fenced_lines(&line_text, section.clone());
    for index in section.clone() {
        if fenced.contains(&index) {
            continue;
        }
        let line = lines[index].text;
        if is_indented_line(line) {
            continue;
        }
        if let Some(task) = pomodoro::open_ledger_task(line) {
            open.push((index, task));
            if pomodoro::task_time_range(task).is_some() {
                timed.push((index, task));
            }
        } else if let Some(task) = pomodoro::completed_ledger_task(line) {
            completed.push((index, task));
        }
    }

    let (selected, selected_text) = match selection {
        PomodoroSelection::NamedOrCreate(selector) => {
            match select_named_pomodoro(contents, &lines, selector)? {
                NamedPomodoroResolution::Found(selected) => selected,
                NamedPomodoroResolution::Create => {
                    return insert_named_pomodoro_child_block(
                        contents, block, selector, &lines, &section, &timed,
                        &completed, &open,
                    );
                }
            }
        }
        PomodoroSelection::CurrentOrFuture
        | PomodoroSelection::CurrentOrLastCompleted => {
            if timed.len() > 1 {
                return Err(CaptureError::io(
                    "Bob daily note has multiple open timed Pomodoros",
                ));
            }
            match selection {
                PomodoroSelection::CurrentOrFuture => {
                    timed.first().or(open.first())
                }
                PomodoroSelection::CurrentOrLastCompleted => {
                    timed.first().or(completed.last()).or(open.first())
                }
                PomodoroSelection::NamedOrCreate(_) => {
                    unreachable!("named handled")
                }
            }
            .copied()
            .ok_or_else(|| CaptureError::io(selection.no_entry_message()))?
        }
    };
    let pomodoro_text = selected_text.to_string();
    let insertion_index = task_block_end(&lines, selected);
    let indentation =
        child_bullet_indentation(&lines, selected + 1, insertion_index)
            .or_else(|| {
                nearby_child_bullet_indentation(
                    &lines,
                    section.start,
                    section.end,
                )
            })
            .unwrap_or_else(|| "  ".to_string());
    let indented_block = block
        .split('\n')
        .map(|line| format!("{indentation}{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let addition = insertion_text_preserving_line_endings(
        contents,
        insertion_index,
        &indented_block,
    );
    let placement = if insertion_index >= contents.len() {
        Placement::Appended
    } else {
        Placement::Inserted
    };
    Ok((
        insert_at(contents, insertion_index, &addition),
        placement,
        selected,
        pomodoro_text,
    ))
}

pub(super) enum NamedPomodoroResolution<'a> {
    Found((usize, &'a str)),
    Create,
}

pub(super) fn select_named_pomodoro<'a>(
    contents: &str,
    lines: &'a [LineSpan<'a>],
    selector: &str,
) -> Result<NamedPomodoroResolution<'a>, CaptureError> {
    let scan = capture_pomodoros::scan(contents);
    match capture_pomodoros::select_named(&scan, selector) {
        capture_pomodoros::NamedSelection::Found(entry) => {
            let index = entry.line.checked_sub(1).ok_or_else(|| {
                CaptureError::io("Pomodoro capture invariant failed: named entry has no line")
            })?;
            let text = pomodoro::open_ledger_task(lines[index].text).ok_or_else(|| {
                CaptureError::io(
                    "Pomodoro capture invariant failed: named entry is not an open ledger task",
                )
            })?;
            Ok(NamedPomodoroResolution::Found((index, text)))
        }
        capture_pomodoros::NamedSelection::CompletedOnly(_)
        | capture_pomodoros::NamedSelection::Missing { .. } => {
            Ok(NamedPomodoroResolution::Create)
        }
    }
}

pub(super) fn insert_named_pomodoro_child_block(
    contents: &str,
    block: &str,
    selector: &str,
    lines: &[LineSpan<'_>],
    section: &std::ops::Range<usize>,
    timed: &[(usize, &str)],
    completed: &[(usize, &str)],
    open: &[(usize, &str)],
) -> Result<(String, Placement, usize, String), CaptureError> {
    if timed.len() > 1 {
        return Err(CaptureError::io(
            "Bob daily note has multiple open timed Pomodoros",
        ));
    }

    let name = capture_pomodoros::canonicalize_pomodoro_name(selector)
        .ok_or_else(|| {
            CaptureError::usage(capture_pomodoros::POMODORO_NAME_USAGE)
        })?;
    let anchor = timed
        .first()
        .or_else(|| completed.last())
        .map(|(index, _)| *index);
    let insertion_index = new_pomodoro_insertion_index(
        lines,
        section,
        anchor,
        open.first().map(|(index, _)| *index),
    );
    let indentation = anchor
        .and_then(|index| {
            child_bullet_indentation(
                lines,
                index + 1,
                task_block_end(lines, index),
            )
        })
        .or_else(|| {
            nearby_child_bullet_indentation(lines, section.start, section.end)
        })
        .unwrap_or_else(|| "  ".to_string());
    let ledger_line = capture_pomodoros::format_named_placeholder_line(&name);
    let indented_block = block
        .split('\n')
        .map(|line| format!("{indentation}{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let entry_block = format!("{ledger_line}\n{indented_block}");
    let addition = insertion_text_preserving_line_endings(
        contents,
        insertion_index,
        &entry_block,
    );
    let placement = if insertion_index >= contents.len() {
        Placement::Appended
    } else {
        Placement::Inserted
    };
    let updated = insert_at(contents, insertion_index, &addition);
    let created_line = line_index_at_offset(lines, insertion_index);
    verify_created_named_pomodoro(&updated, created_line, &name, block)?;
    Ok((updated, placement, created_line, format!("() — {name}")))
}

pub(super) fn verify_created_named_pomodoro(
    contents: &str,
    created_line: usize,
    name: &str,
    block: &str,
) -> Result<(), CaptureError> {
    let slug = capture_language::selector_slug(name);
    let scan = capture_pomodoros::scan(contents);
    let entry = scan
        .entries
        .iter()
        .find(|entry| entry.line == created_line + 1)
        .ok_or_else(|| {
            CaptureError::io("Pomodoro capture invariant failed: created Pomodoro disappeared")
        })?;
    if entry.state != capture_pomodoros::PomodoroState::Open
        || entry.name.as_deref() != Some(name)
        || entry.slug != slug
        || !entry.placeholder
        || !entry.selectable
    {
        return Err(CaptureError::io(format!(
            "Pomodoro capture invariant failed: created Pomodoro `{name}` is not selectable"
        )));
    }

    let lines = line_spans(contents);
    let block_start = lines[created_line].end;
    let block_end = task_block_end(&lines, created_line);
    if !contents[block_start..block_end].contains(block) {
        return Err(CaptureError::io(format!(
            "Pomodoro capture invariant failed: created Pomodoro `{name}` does not own the new child block"
        )));
    }
    Ok(())
}

pub(super) fn line_start(lines: &[LineSpan<'_>], index: usize) -> usize {
    if index == 0 {
        0
    } else {
        lines[index - 1].end
    }
}

/// Byte offset for a newly created Pomodoro entry: after the anchor's complete
/// block, otherwise before the first open Pomodoro so blank lines after the
/// heading stay put, otherwise the top of the section.
pub(super) fn new_pomodoro_insertion_index(
    lines: &[LineSpan<'_>],
    section: &std::ops::Range<usize>,
    anchor: Option<usize>,
    first_open: Option<usize>,
) -> usize {
    if let Some(anchor) = anchor {
        return task_block_end(lines, anchor);
    }
    if let Some(first_open) = first_open {
        return line_start(lines, first_open);
    }
    line_start(lines, section.start)
}

pub(super) fn line_index_at_offset(
    lines: &[LineSpan<'_>],
    offset: usize,
) -> usize {
    lines.iter().take_while(|line| line.end <= offset).count()
}

pub(super) fn child_bullet_indentation(
    lines: &[LineSpan<'_>],
    start_line: usize,
    insertion_index: usize,
) -> Option<String> {
    lines[start_line..]
        .iter()
        .take_while(|line| line.end <= insertion_index)
        .find_map(|line| unordered_child_indentation(line.text))
}

pub(super) fn nearby_child_bullet_indentation(
    lines: &[LineSpan<'_>],
    start_line: usize,
    end_line: usize,
) -> Option<String> {
    lines[start_line..end_line]
        .iter()
        .find_map(|line| unordered_child_indentation(line.text))
}

pub(super) fn unordered_child_indentation(line: &str) -> Option<String> {
    let indentation_len = line
        .as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    if indentation_len == 0 {
        return None;
    }
    let rest = &line[indentation_len..];
    matches!(rest.as_bytes(), [b'-' | b'*' | b'+', b' ', ..])
        .then(|| line[..indentation_len].to_string())
}

pub(super) fn insertion_text_preserving_line_endings(
    contents: &str,
    index: usize,
    line: &str,
) -> String {
    let ending = document_line_ending(contents);
    let line = line.replace('\n', ending);
    let needs_leading_ending = index > 0 && !contents[..index].ends_with('\n');
    if needs_leading_ending {
        format!("{ending}{line}{ending}")
    } else {
        format!("{line}{ending}")
    }
}
