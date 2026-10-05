//! Pomodoro ledger scan and empty-entry cleanup plans.
use super::*;

pub(crate) fn logical_lines(contents: &str) -> Vec<&str> {
    contents.split_inclusive('\n').map(logical_line).collect()
}

pub(super) fn logical_line(segment: &str) -> &str {
    let without_lf = segment.strip_suffix('\n').unwrap_or(segment);
    without_lf.strip_suffix('\r').unwrap_or(without_lf)
}

pub(crate) fn scan_pomodoros(
    lines: &[&str],
    section: Range<usize>,
) -> PomodoroModel {
    let fenced_lines = fenced_lines(lines, section.clone());
    let mut entries = Vec::new();
    for line_index in section.clone() {
        let line = lines[line_index];
        if fenced_lines.contains(&line_index)
            || line.starts_with(' ')
            || line.starts_with('\t')
            || !line.starts_with('-')
        {
            continue;
        }
        let open_task = native_pomodoro::open_ledger_task(line);
        let completed_task = native_pomodoro::completed_ledger_task(line);
        if open_task.is_none() && completed_task.is_none() {
            continue;
        }
        let end_line = entry_block_end(lines, line_index, section.end);
        let child_indentation = direct_child_indentation(
            lines,
            line_index,
            end_line,
            &fenced_lines,
        );
        entries.push(PomodoroEntry {
            line_index,
            end_line,
            open: open_task.is_some(),
            completed: completed_task.is_some(),
            timed: open_task.is_some_and(|task| {
                native_pomodoro::task_time_range(task).is_some()
            }),
            has_child: child_indentation.is_some(),
            child_indentation,
            context: line.trim_end().to_string(),
        });
    }

    let mut bullets = Vec::new();
    let mut raw_references = BTreeSet::new();
    let mut recent_references = BTreeSet::new();
    let mut all_references = BTreeSet::new();
    for (entry_index, entry) in entries.iter().enumerate() {
        for line_index in entry.line_index + 1..entry.end_line {
            if fenced_lines.contains(&line_index) {
                continue;
            }
            let Some(indentation) =
                pomodoro_bullet_indentation(lines[line_index])
            else {
                continue;
            };
            let links = block_link_occurrences(lines[line_index]);
            if links.is_empty() {
                continue;
            }
            all_references
                .extend(links.iter().map(|link| link.reference.clone()));
            recent_references.extend(
                links
                    .iter()
                    .filter(|link| !link.struck)
                    .map(|link| link.reference.clone()),
            );
            if entry.open {
                raw_references.extend(
                    links
                        .iter()
                        .filter(|link| !link.struck)
                        .map(|link| link.reference.clone()),
                );
            }
            bullets.push(LinkBullet {
                entry_index,
                line_index,
                end_line: bullet_block_end(
                    lines,
                    line_index,
                    entry.end_line,
                    indentation.len(),
                ),
                indentation,
                links,
            });
        }
    }

    PomodoroModel {
        open_pomodoros: entries.iter().filter(|entry| entry.open).count(),
        entries,
        bullets,
        raw_references,
        recent_references,
        all_references,
    }
}

pub(super) fn direct_child_indentation(
    lines: &[&str],
    entry_line: usize,
    entry_end: usize,
    fenced_lines: &BTreeSet<usize>,
) -> Option<String> {
    (entry_line + 1..entry_end).find_map(|line_index| {
        direct_child_indentation_at(lines, entry_line, line_index, fenced_lines)
    })
}

pub(super) fn direct_child_indentation_at(
    lines: &[&str],
    entry_line: usize,
    child_line: usize,
    fenced_lines: &BTreeSet<usize>,
) -> Option<String> {
    if fenced_lines.contains(&child_line) {
        return None;
    }
    let line = lines[child_line];
    let indentation_len = leading_indentation_len(line);
    if indentation_len == 0
        || after_list_marker(line, indentation_len).is_none()
        || nearest_parent_list_item(lines, child_line) != Some(entry_line)
    {
        return None;
    }
    Some(line[..indentation_len].to_string())
}

pub(super) fn pomodoro_bullet_indentation(line: &str) -> Option<String> {
    let indentation_len = leading_indentation_len(line);
    (indentation_len > 0 && after_list_marker(line, indentation_len).is_some())
        .then(|| line[..indentation_len].to_string())
}

pub(super) fn block_link_occurrences(line: &str) -> Vec<LinkOccurrence> {
    let mut links = Vec::new();
    let struck_spans = task_dependencies::strikethrough_spans(line);
    for span in task_dependencies::raw_wikilink_spans(line) {
        let absolute_open = span.open;
        let inside = &line[span.open + 2..span.end - 2];
        let link_end = span.end;
        if let Some((target, block_id)) =
            task_dependencies::parse_block_link_inside(inside)
        {
            let embedded = line[..absolute_open].ends_with('!');
            let token_start = absolute_open - usize::from(embedded);
            let struck_span = struck_spans.iter().find(|span| {
                token_start >= span.start + 2 && link_end <= span.end - 2
            });
            let struck = struck_span.is_some();
            let exact_struck_span = struck_span.filter(|span| {
                token_start == span.start + 2 && link_end == span.end - 2
            });
            let display_start =
                exact_struck_span.map_or(token_start, |span| span.start);
            let edit_end = exact_struck_span.map_or(link_end, |span| span.end);
            let (edit_start, marker_count) =
                pomodoro_marker_prefix(line, display_start);
            let wikilink = &line[absolute_open..link_end];
            let before = if !struck && line[..token_start].ends_with("~~") {
                " "
            } else {
                ""
            };
            let after = if !struck && line[link_end..].starts_with("~~") {
                " "
            } else {
                ""
            };
            let preserved = &line[display_start..edit_end];
            let (retired_marked_token, retired_unmarked_token) = if struck {
                if exact_struck_span.is_some() {
                    let token = format!("~~{wikilink}~~");
                    (format!("{POMODORO_MARKER} {token}"), token)
                } else {
                    (
                        format!("{POMODORO_MARKER} {wikilink}"),
                        wikilink.to_string(),
                    )
                }
            } else {
                (
                    format!("{before}{POMODORO_MARKER} ~~{wikilink}~~{after}"),
                    format!("{before}~~{wikilink}~~{after}"),
                )
            };
            links.push(LinkOccurrence {
                reference: RawReference { target, block_id },
                edit_start,
                edit_end,
                current_token: line[edit_start..edit_end].to_string(),
                preserved_marked_token: format!(
                    "{POMODORO_MARKER} {preserved}"
                ),
                preserved_unmarked_token: preserved.to_string(),
                retired_marked_token,
                retired_unmarked_token,
                embedded,
                struck,
                marker_count,
            });
        }
    }
    links
}

pub(super) fn pomodoro_marker_prefix(
    line: &str,
    token_start: usize,
) -> (usize, usize) {
    let mut cursor = token_start;
    let mut count = 0;
    loop {
        let whitespace_end = cursor;
        let mut marker_end = cursor;
        while marker_end > 0
            && matches!(line.as_bytes()[marker_end - 1], b' ' | b'\t')
        {
            marker_end -= 1;
        }
        if marker_end == whitespace_end
            || !line[..marker_end].ends_with(POMODORO_MARKER)
        {
            break;
        }
        cursor = marker_end - POMODORO_MARKER.len();
        count += 1;
    }
    if count == 0 {
        return (token_start, 0);
    }
    (cursor, count)
}

pub(super) fn desired_link_token(
    link: &LinkOccurrence,
    retire: bool,
    marked: bool,
) -> &str {
    match (retire, marked) {
        (true, true) => &link.retired_marked_token,
        (true, false) => &link.retired_unmarked_token,
        (false, true) => &link.preserved_marked_token,
        (false, false) => &link.preserved_unmarked_token,
    }
}

pub(super) fn completed_pomodoro_marker_expected(
    link: &LinkOccurrence,
) -> bool {
    if link.embedded {
        return false;
    }
    if link.struck {
        return link.marker_count > 0;
    }
    true
}

pub(super) fn marker_expected_for_occurrence(
    entry: &PomodoroEntry,
    link: &LinkOccurrence,
) -> bool {
    entry.completed && completed_pomodoro_marker_expected(link)
}

pub(crate) fn plan_empty_pomodoro_removals(
    contents: &str,
    original_model: &PomodoroModel,
) -> EmptyPomodoroPlan {
    let lines = logical_lines(contents);
    let Some(section) = native_pomodoro::pomodoros_section_range(&lines) else {
        return EmptyPomodoroPlan::default();
    };
    let model = scan_pomodoros(&lines, section);
    let mut plan = EmptyPomodoroPlan::default();

    for (entry_index, entry) in model.entries.iter().enumerate() {
        if entry.has_child {
            continue;
        }
        plan.deleted_lines.extend(entry.line_index..entry.end_line);
        let original = original_model.entries.get(entry_index).unwrap_or(entry);
        plan.removed.push(RemovedEmptyPomodoro {
            line_number: original.line_index + 1,
            line: original.context.clone(),
        });
    }

    plan
}

pub(crate) fn apply_empty_pomodoro_plan(
    contents: &str,
    plan: &EmptyPomodoroPlan,
) -> String {
    if plan.deleted_lines.is_empty() {
        return contents.to_string();
    }
    contents
        .split_inclusive('\n')
        .enumerate()
        .filter_map(|(index, segment)| {
            (!plan.deleted_lines.contains(&index)).then_some(segment)
        })
        .collect()
}
