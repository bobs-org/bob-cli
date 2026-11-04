//! Pomodoro merge planning (`=*`).
use super::*;
use crate::native::pomodoro_themes::{
    merged::{
        merge_names, select_by_type, SelectEntry, SelectFilter, SelectInput,
        SelectOutcome,
    },
    routing,
};

pub(super) fn reject_pomodoro_merge_conflicts(
    parsed: &ParsedCaptureText,
    request: &CaptureRequest,
) -> Result<(), CaptureError> {
    if !request.forced_destination_flags.is_empty() {
        return Err(CaptureError::usage(format!(
            "Pomodoro merge `=*` cannot be combined with {}; capture the merge alone",
            request.forced_destination_flags.join(", ")
        )));
    }
    if request.forced_route.is_some()
        || request.forced_section.is_some()
        || request.forced_sub_bullet_target.is_some()
        || request.forced_drawer.is_some()
    {
        return Err(CaptureError::usage(
            "Pomodoro merge `=*` cannot be combined with --route, --section, --task, --task-ref, or --drawer; capture the merge alone",
        ));
    }
    if request.forced_clip.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro merge `=*` cannot be combined with --clip; capture the merge alone",
        ));
    }
    if parsed.route.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro merge `=*` must be the whole capture item; remove extra text, markers, or child lines",
        ));
    }
    if parsed.clip.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro merge `=*` cannot be combined with %c; capture the merge alone",
        ));
    }
    if parsed.scheduled_offset.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro merge `=*` cannot be combined with s:<N>; capture the merge alone",
        ));
    }
    if parsed.priority_level.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro merge `=*` cannot be combined with $<N>; capture the merge alone",
        ));
    }
    if !parsed.sub_bullets.is_empty() {
        return Err(CaptureError::usage(
            "Pomodoro merge `=*` must be the whole capture item; remove extra text, markers, or child lines",
        ));
    }
    if !parsed.dependencies.is_empty()
        || !parsed.blocked_tasks.is_empty()
        || !parsed.relations.is_empty()
    {
        return Err(CaptureError::usage(
            "Pomodoro merge `=*` cannot be combined with task relationships; capture the merge alone",
        ));
    }
    Ok(())
}

fn is_indented_list_line(line: &str) -> bool {
    let indent = leading_spaces_or_tabs_len(line);
    if indent == 0 {
        return false;
    }
    let after = &line[indent..];
    let Some(marker_len) = list_marker_len(after) else {
        return false;
    };
    let rest = &after[marker_len..];
    rest.is_empty() || rest.starts_with([' ', '\t'])
}

fn top_level_child_indices(
    lines: &[LineSpan<'_>],
    entry_index: usize,
) -> Vec<usize> {
    let end = crate::native::capture_task_toggle::child_block_end_line(
        lines,
        entry_index,
    );
    let parent_indent = leading_spaces_or_tabs_len(lines[entry_index].text);
    let mut first_indent: Option<usize> = None;
    for index in (entry_index + 1)..=end {
        let text = lines[index].text;
        if text.trim().is_empty() {
            continue;
        }
        if !is_indented_list_line(text) {
            continue;
        }
        let indent = leading_spaces_or_tabs_len(text);
        if indent <= parent_indent {
            continue;
        }
        if first_indent.is_none() {
            first_indent = Some(indent);
        }
    }
    let Some(top_indent) = first_indent else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for index in (entry_index + 1)..=end {
        let text = lines[index].text;
        if text.trim().is_empty() {
            continue;
        }
        if !is_indented_list_line(text) {
            continue;
        }
        if leading_spaces_or_tabs_len(text) == top_indent {
            out.push(index);
        }
    }
    out
}

fn subtree_end_for(lines: &[LineSpan<'_>], child_index: usize) -> usize {
    crate::native::capture_task_toggle::child_block_end_line(lines, child_index)
}

fn select_entries<'a>(
    scan: &'a crate::native::capture_pomodoros::PomodoroScan,
) -> Vec<SelectEntry<'a>> {
    scan.entries
        .iter()
        .map(|entry| SelectEntry {
            name: entry.name.as_deref(),
            open: entry.state
                == crate::native::capture_pomodoros::PomodoroState::Open,
            running: entry.is_current,
            timed: entry.time_range.is_some(),
        })
        .collect()
}

fn first_type_selector(
    name: &str,
    sources: &crate::native::config::themes::ThemeSources,
) -> String {
    routing::type_components(Some(name), sources)
        .into_iter()
        .find(|item| item.is_type)
        .map(|item| item.key)
        .unwrap_or_else(|| {
            crate::native::plan_budget::normalize_component(name)
        })
}

/// The merged-in component of `src_name` that `dest_selector` names, in
/// the name's own spelling (whole key first, then key prefix).
fn dest_already_part_of_src(
    src_name: &str,
    dest_selector: &str,
    sources: &crate::native::config::themes::ThemeSources,
) -> Option<String> {
    let types: Vec<_> = routing::type_components(Some(src_name), sources)
        .into_iter()
        .filter(|item| item.is_type)
        .collect();
    if types.len() < 2 {
        return None;
    }
    let parsed = routing::parse_type_selector(dest_selector);
    let merged_in = &types[1..];
    merged_in
        .iter()
        .find(|item| item.key == parsed.key)
        .or_else(|| {
            merged_in
                .iter()
                .find(|item| item.key.starts_with(&parsed.key))
        })
        .map(|item| item.name.clone())
}

fn top_level_trimmed(
    lines: &[LineSpan<'_>],
    entry_index: usize,
) -> Vec<String> {
    top_level_child_indices(lines, entry_index)
        .into_iter()
        .map(|index| lines[index].text.trim().to_string())
        .collect()
}

fn shift_index_after_remove(
    index: usize,
    start: usize,
    end: usize,
) -> Option<usize> {
    if index >= start && index <= end {
        None
    } else if index > end {
        Some(index - (end - start + 1))
    } else {
        Some(index)
    }
}

fn shift_index_after_insert_after(
    index: usize,
    after: usize,
    count: usize,
) -> usize {
    if index > after {
        index + count
    } else {
        index
    }
}

fn shift_index_after_move_subtree(
    index: usize,
    child: usize,
    sub_end: usize,
    dest_child_end: usize,
) -> Option<usize> {
    let subtree_len = sub_end - child + 1;
    let removed_before = if child <= dest_child_end {
        subtree_len
    } else {
        0
    };
    let dest_after = dest_child_end.saturating_sub(removed_before);
    let after_remove = shift_index_after_remove(index, child, sub_end)?;
    Some(shift_index_after_insert_after(
        after_remove,
        dest_after,
        subtree_len,
    ))
}

fn retarget_tracked_lines(
    src_zero: &mut usize,
    dest_zeros: &mut [usize],
    from_dest: usize,
    mut shift: impl FnMut(usize) -> Option<usize>,
) -> Result<(), CaptureError> {
    *src_zero = shift(*src_zero).ok_or_else(|| {
        CaptureError::io(
            "Pomodoro merge invariant failed: source header was removed",
        )
    })?;
    for dest in dest_zeros.iter_mut().skip(from_dest) {
        if let Some(next) = shift(*dest) {
            *dest = next;
        }
    }
    Ok(())
}

fn task_link_zero_indices(
    lines: &[LineSpan<'_>],
    entry_index: usize,
) -> std::collections::BTreeSet<usize> {
    let tops = top_level_child_indices(lines, entry_index);
    if tops.is_empty() {
        return Default::default();
    }
    let texts: Vec<&str> = lines.iter().map(|span| span.text).collect();
    let fenced = markdown::fenced_lines(&texts, 0..texts.len());
    let start = *tops.first().unwrap();
    let end = crate::native::capture_task_toggle::child_block_end_line(
        lines,
        entry_index,
    ) + 1;
    let numbered =
        crate::native::capture_pomodoro_close::number_task_links_in_range(
            &texts,
            &fenced,
            start..end.min(texts.len()),
        );
    let top_set: std::collections::BTreeSet<usize> = tops.into_iter().collect();
    numbered
        .into_iter()
        .filter_map(|link| {
            let zero = link.line.checked_sub(1)?;
            top_set.contains(&zero).then_some(zero)
        })
        .collect()
}

fn merge_target_missing_error(
    scan: &crate::native::capture_pomodoros::PomodoroScan,
    day_rel: &str,
    selector: Option<&str>,
) -> CaptureError {
    let planned: Vec<String> = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == crate::native::capture_pomodoros::PomodoroState::Open
                && entry.time_range.is_none()
                && !entry.is_current
        })
        .filter_map(|entry| entry.name.clone())
        .collect();
    if let Some(selector) = selector {
        if planned.is_empty() {
            return CaptureError::usage_code(
                format!(
                    "No open Pomodoro matching `{selector}` in today's ledger (`{day_rel}`)"
                ),
                "merge_target_missing",
            );
        }
        return CaptureError::usage_code(
            format!(
                "No open Pomodoro matching `{selector}`; today's planned entries: {}",
                planned.join(", ")
            ),
            "merge_target_missing",
        );
    }
    if planned.is_empty() {
        return CaptureError::usage_code(
            format!(
                "No open Pomodoro to merge in today's ledger (`{day_rel}`)"
            ),
            "merge_target_missing",
        );
    }
    CaptureError::usage_code(
        format!(
            "No matching Pomodoro to merge; today's planned entries: {}",
            planned.join(", ")
        ),
        "merge_target_missing",
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedMerge {
    src_index: usize,
    source_role: PomodoroMergeSourceRole,
    dests: Vec<(usize, PomodoroMergeAbsorbedRole)>,
}

fn merge_selector_for_name(
    name: &str,
    sources: &crate::native::config::themes::ThemeSources,
) -> String {
    let selector = first_type_selector(name, sources);
    if crate::native::capture_language::is_pomodoro_selector_component(
        &selector,
    ) {
        selector
    } else {
        format!("\"{name}\"")
    }
}

fn merge_destination_spelling(
    name: &str,
    sources: &crate::native::config::themes::ThemeSources,
) -> String {
    let selector = first_type_selector(name, sources);
    if crate::native::capture_language::is_pomodoro_selector_component(
        &selector,
    ) {
        format!("#{selector}")
    } else {
        format!("#\"{name}\"")
    }
}

fn merge_next_missing_error(
    scan: &crate::native::capture_pomodoros::PomodoroScan,
    src_index: usize,
    src_name: &str,
    sources: &crate::native::config::themes::ThemeSources,
    passed_meet: bool,
) -> CaptureError {
    let above = scan.entries.iter().enumerate().find_map(|(index, entry)| {
        (index < src_index
            && entry.state
                == crate::native::capture_pomodoros::PomodoroState::Open
            && entry.time_range.is_none()
            && !entry.is_current
            && entry.name.is_some()
            && !routing::is_meet_name(entry.name.as_deref()))
        .then_some(entry.name.as_deref())
        .flatten()
    });
    let mut message = format!("Nothing planned below {src_name} to merge");
    if passed_meet {
        message.push_str(" (MEET never merges)");
    }
    if let Some(dest_name) = above {
        let src_selector = merge_selector_for_name(src_name, sources);
        let dest_selector = merge_destination_spelling(dest_name, sources);
        message.push_str(&format!(
            "; name one to absorb (for example `=*{src_selector}{dest_selector}`)"
        ));
    }
    CaptureError::usage_code(message, "merge_next_missing")
}

fn implied_source_meet_error(
    scan: &crate::native::capture_pomodoros::PomodoroScan,
    src_index: usize,
    sources: &crate::native::config::themes::ThemeSources,
) -> CaptureError {
    let example = scan.entries.iter().enumerate().find_map(|(index, entry)| {
        (index != src_index
            && entry.state
                == crate::native::capture_pomodoros::PomodoroState::Open
            && entry.time_range.is_none()
            && !entry.is_current
            && entry.name.is_some()
            && !routing::is_meet_name(entry.name.as_deref()))
        .then_some(entry.name.as_deref())
        .flatten()
        .map(|name| {
            format!(
                " (for example `=*{}`)",
                merge_selector_for_name(name, sources)
            )
        })
    });
    CaptureError::usage_code(
        format!(
            "The current Pomodoro is MEET, which never merges; name the one to keep{}",
            example.unwrap_or_default()
        ),
        "merge_meet",
    )
}

fn resolve_merge(
    spec: &PomodoroMergeSpec,
    scan: &crate::native::capture_pomodoros::PomodoroScan,
    entries: &[SelectEntry<'_>],
    sources: &crate::native::config::themes::ThemeSources,
    day_rel: &str,
) -> Result<ResolvedMerge, CaptureError> {
    let (src_index, source_role) = if let Some(src) = spec.src.as_deref() {
        let input = SelectInput {
            entries,
            selector: src,
            filter: SelectFilter::Open,
            sources,
        };
        match select_by_type(&input) {
            SelectOutcome::Entry(index) => {
                (index, PomodoroMergeSourceRole::Named)
            }
            SelectOutcome::Missing => {
                return Err(merge_target_missing_error(
                    scan,
                    day_rel,
                    Some(src),
                ));
            }
        }
    } else {
        match capture_pomodoros::current_pomodoro(scan) {
            capture_pomodoros::CurrentPomodoro::Running(index) => {
                (index, PomodoroMergeSourceRole::Current)
            }
            capture_pomodoros::CurrentPomodoro::NextUp(index) => {
                (index, PomodoroMergeSourceRole::NextUp)
            }
            capture_pomodoros::CurrentPomodoro::Ambiguous => {
                return Err(CaptureError::io(
                    "cannot merge into the current Pomodoro: today's ledger has multiple open timed Pomodoros; finish all but one first",
                ));
            }
            capture_pomodoros::CurrentPomodoro::Missing => {
                return Err(merge_target_missing_error(scan, day_rel, None));
            }
        }
    };
    let src_entry = scan.entries.get(src_index).ok_or_else(|| {
        merge_target_missing_error(scan, day_rel, spec.src.as_deref())
    })?;
    if routing::is_meet_name(src_entry.name.as_deref()) {
        if spec.src.is_none() {
            return Err(implied_source_meet_error(scan, src_index, sources));
        }
        return Err(CaptureError::usage_code(
            "MEET entries never merge; calendar sync owns the MEET name"
                .to_string(),
            "merge_meet",
        ));
    }
    let src_name = src_entry.name.as_deref().unwrap_or_default();
    let mut dests = Vec::new();
    if spec.dests.is_empty() {
        match crate::native::pomodoro_themes::merged::next_planned_below(
            entries,
            src_index,
            &[],
        ) {
            crate::native::pomodoro_themes::merged::NextPlanned::Entry(
                index,
            ) => dests.push((index, PomodoroMergeAbsorbedRole::Next)),
            crate::native::pomodoro_themes::merged::NextPlanned::Missing {
                passed_meet,
            } => {
                return Err(merge_next_missing_error(
                    scan,
                    src_index,
                    src_name,
                    sources,
                    passed_meet,
                ));
            }
        }
        return Ok(ResolvedMerge {
            src_index,
            source_role,
            dests,
        });
    }

    for dest in &spec.dests {
        let untimed = SelectInput {
            entries,
            selector: &dest.text,
            filter: SelectFilter::UntimedPlanned {
                except: Some(src_index),
            },
            sources,
        };
        let mut dest_index = match select_by_type(&untimed) {
            SelectOutcome::Entry(index) => Some(index),
            SelectOutcome::Missing => None,
        };
        if dest_index.is_none() {
            let open = SelectInput {
                entries,
                selector: &dest.text,
                filter: SelectFilter::Open,
                sources,
            };
            dest_index = match select_by_type(&open) {
                SelectOutcome::Entry(index) => Some(index),
                SelectOutcome::Missing => None,
            };
        }
        let Some(dest_index) = dest_index else {
            return Err(merge_target_missing_error(
                scan,
                day_rel,
                Some(&dest.text),
            ));
        };
        if dest_index == src_index {
            if let Some(part) =
                dest_already_part_of_src(src_name, &dest.text, sources)
            {
                return Err(CaptureError::usage_code(
                    format!(
                        "{part} is already part of {src_name}; nothing to merge"
                    ),
                    "merge_already_part",
                ));
            }
            return Err(CaptureError::usage_code(
                format!("{src_name} cannot merge into itself"),
                "merge_same_pomodoro",
            ));
        }
        let dest_entry = scan.entries.get(dest_index).ok_or_else(|| {
            merge_target_missing_error(scan, day_rel, Some(&dest.text))
        })?;
        if dest_entry.time_range.is_some() || dest_entry.is_current {
            let timed_name = dest_entry.name.clone().unwrap_or_default();
            let timed_sel = first_type_selector(&timed_name, sources);
            let src_sel = first_type_selector(src_name, sources);
            return Err(CaptureError::usage_code(
                format!(
                    "{timed_name} is already running; merge the other way: `=*{timed_sel}#{src_sel}`"
                ),
                "merge_timed_pomodoro",
            ));
        }
        if routing::is_meet_name(dest_entry.name.as_deref()) {
            return Err(CaptureError::usage_code(
                "MEET entries never merge; calendar sync owns the MEET name"
                    .to_string(),
                "merge_meet",
            ));
        }
        if dests.iter().any(|(index, _)| *index == dest_index) {
            return Err(CaptureError::usage_code(
                format!(
                    "{} is named twice; list each destination once",
                    dest_entry
                        .name
                        .clone()
                        .unwrap_or_else(|| dest.text.clone())
                ),
                "merge_dest_repeated",
            ));
        }
        dests.push((dest_index, PomodoroMergeAbsorbedRole::Named));
    }
    Ok(ResolvedMerge {
        src_index,
        source_role,
        dests,
    })
}

fn explicit_merge_spelling(
    resolved: &ResolvedMerge,
    scan: &crate::native::capture_pomodoros::PomodoroScan,
    entries: &[SelectEntry<'_>],
    sources: &crate::native::config::themes::ThemeSources,
    day_rel: &str,
) -> Option<String> {
    let implied = resolved.source_role != PomodoroMergeSourceRole::Named
        || resolved
            .dests
            .iter()
            .any(|(_, role)| *role == PomodoroMergeAbsorbedRole::Next);
    if !implied {
        return None;
    }
    let source = scan.entries.get(resolved.src_index)?.name.as_deref()?;
    let mut token = format!("=*{}", merge_selector_for_name(source, sources));
    for (index, _) in &resolved.dests {
        let dest = scan.entries.get(*index)?.name.as_deref()?;
        token.push_str(&merge_destination_spelling(dest, sources));
    }
    let parsed =
        crate::native::capture_language::editor_item_at(&token, token.len())?
            .pomodoro_merge?;
    let round_trip =
        resolve_merge(&parsed, scan, entries, sources, day_rel).ok()?;
    (round_trip.src_index == resolved.src_index
        && round_trip.dests
            == resolved
                .dests
                .iter()
                .map(|(index, _)| (*index, PomodoroMergeAbsorbedRole::Named))
                .collect::<Vec<_>>())
    .then_some(token)
}

pub(super) fn plan_pomodoro_merge_item(
    request: &CaptureRequest,
    parsed: ParsedCaptureText,
    spec: PomodoroMergeSpec,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    _task_theme_context: &mut CaptureTaskThemeContext,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<PlannedCaptureItem, CaptureError> {
    reject_pomodoro_merge_conflicts(&parsed, request)?;
    let day_file = pomodoro::day_file_for(&request.bob_dir);
    let day_rel =
        capture_pomodoros::relative_day_file(&day_file, &request.bob_dir);
    if !planner.currently_exists(&day_file)? {
        return Err(CaptureError::io(format!(
            "Bob daily note does not exist: {}",
            day_file.display()
        )));
    }
    let staged = planner.read_existing(&day_file)?;
    let scan = capture_pomodoros::scan(&staged);
    if !scan.has_section {
        return Err(CaptureError::io(
            "Bob daily note has no Pomodoros section",
        ));
    }
    let catalog = crate::native::pomodoro_themes::Catalog::load(
        &request.bob_dir,
        today,
        false,
    )
    .map_err(|error| {
        CaptureError::io(format!("read theme catalog: {error}"))
    })?;
    let sources = catalog.theme_sources();
    let entries = select_entries(&scan);
    let resolved = resolve_merge(&spec, &scan, &entries, &sources, &day_rel)?;
    let src_index = resolved.src_index;
    let source_role = resolved.source_role;
    let src_entry = scan.entries.get(src_index).ok_or_else(|| {
        merge_target_missing_error(&scan, &day_rel, spec.src.as_deref())
    })?;
    let src_name = src_entry.name.clone().unwrap_or_default();
    let src_line = src_entry.line;
    if src_line == 0 {
        return Err(CaptureError::io(
            "Pomodoro merge invariant failed: source entry has no line",
        ));
    }
    let dest_indices: Vec<usize> =
        resolved.dests.iter().map(|(index, _)| *index).collect();
    let dest_roles: Vec<PomodoroMergeAbsorbedRole> =
        resolved.dests.iter().map(|(_, role)| *role).collect();
    let mut dest_names: Vec<String> = Vec::new();
    let mut dest_original_lines: Vec<usize> = Vec::new();
    for dest_index in &dest_indices {
        let dest_entry = scan
            .entries
            .get(*dest_index)
            .ok_or_else(|| merge_target_missing_error(&scan, &day_rel, None))?;
        dest_names.push(dest_entry.name.clone().unwrap_or_default());
        dest_original_lines.push(dest_entry.line);
    }
    let dest_name_refs: Vec<&str> =
        dest_names.iter().map(String::as_str).collect();
    let after_name = merge_names(&src_name, &dest_name_refs);
    let Some(renamed) =
        crate::native::pomodoro_themes::relocation_apply::rename_entry(
            &staged,
            src_line,
            &after_name,
        )
    else {
        return Err(CaptureError::io(
            "Pomodoro merge invariant failed: cannot rename source entry",
        ));
    };
    let mut current = renamed;
    let mut absorbed = Vec::new();
    let mut links_moved = 0usize;
    let mut src_zero = src_line.checked_sub(1).ok_or_else(|| {
        CaptureError::io(
            "Pomodoro merge invariant failed: source entry has no line",
        )
    })?;
    let mut dest_zeros: Vec<usize> = dest_original_lines
        .iter()
        .map(|line| line.saturating_sub(1))
        .collect();
    for dest_i in 0..dest_zeros.len() {
        let dest_name = dest_names[dest_i].clone();
        let original_line = dest_original_lines[dest_i];
        let role = dest_roles[dest_i];
        let mut moved_lines = 0usize;
        let mut dropped_duplicates = 0usize;
        loop {
            let spans_now = line_spans(&current);
            let dest_zero = dest_zeros[dest_i];
            if dest_zero >= spans_now.len() || src_zero >= spans_now.len() {
                return Err(CaptureError::io(
                    "Pomodoro merge invariant failed: dest entry is out of range",
                ));
            }
            let children = top_level_child_indices(&spans_now, dest_zero);
            if children.is_empty() {
                current = crate::native::capture_task_toggle::remove_line_range(
                    &current, &spans_now, dest_zero, dest_zero,
                );
                retarget_tracked_lines(
                    &mut src_zero,
                    &mut dest_zeros,
                    dest_i,
                    |index| {
                        shift_index_after_remove(index, dest_zero, dest_zero)
                    },
                )?;
                break;
            }
            let child = children[0];
            let src_trims = top_level_trimmed(&spans_now, src_zero);
            let child_trim = spans_now[child].text.trim().to_string();
            let is_link =
                task_link_zero_indices(&spans_now, dest_zero).contains(&child);
            if is_link && src_trims.contains(&child_trim) {
                let sub_end = subtree_end_for(&spans_now, child);
                current = crate::native::capture_task_toggle::remove_line_range(
                    &current, &spans_now, child, sub_end,
                );
                retarget_tracked_lines(
                    &mut src_zero,
                    &mut dest_zeros,
                    dest_i,
                    |index| shift_index_after_remove(index, child, sub_end),
                )?;
                dropped_duplicates += 1;
                continue;
            }
            let sub_end = subtree_end_for(&spans_now, child);
            let subtree_len = sub_end.saturating_sub(child) + 1;
            let dest_child_end =
                crate::native::capture_task_toggle::child_block_end_line(
                    &spans_now, src_zero,
                );
            match crate::native::capture_task_toggle::move_subtree_to_entry(
                &current, child, sub_end, src_zero,
            ) {
                Ok((updated, _)) => {
                    current = updated;
                    retarget_tracked_lines(
                        &mut src_zero,
                        &mut dest_zeros,
                        dest_i,
                        |index| {
                            shift_index_after_move_subtree(
                                index,
                                child,
                                sub_end,
                                dest_child_end,
                            )
                        },
                    )?;
                    moved_lines += subtree_len;
                    if is_link {
                        links_moved += 1;
                    }
                }
                Err(_) => {
                    return Err(CaptureError::io(
                        "Pomodoro merge invariant failed: cannot move dest subtree",
                    ));
                }
            }
        }
        absorbed.push(PomodoroMergeAbsorbed {
            name: dest_name,
            role,
            from_line: original_line,
            moved_lines,
            dropped_duplicates,
        });
    }
    let entry_line = src_zero + 1;
    planner.stage(&day_file, current.clone())?;
    restamp_session_clock(
        planner,
        &request.bob_dir,
        &day_file,
        Path::new(&day_rel),
        entry_line,
        now,
        warnings,
    )?;
    current = planner.read_existing(&day_file)?;
    let relative_target = day_file
        .strip_prefix(&request.bob_dir)
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|_| day_file.clone());
    let explicit =
        explicit_merge_spelling(&resolved, &scan, &entries, &sources, &day_rel);
    let summary = PomodoroMergeSummary {
        path: relative_target.to_string_lossy().into_owned(),
        entry_line,
        before: src_name.clone(),
        after: after_name.clone(),
        source_role,
        explicit,
        absorbed,
        moved_links: links_moved,
    };
    let source_line_text = current
        .lines()
        .nth(entry_line.saturating_sub(1))
        .unwrap_or("")
        .to_string();
    let mut refs = Vec::new();
    refs.push(PomodoroBlockRef::at(
        PomodoroBlockRole::Merge,
        src_line.saturating_sub(1),
        entry_line.saturating_sub(1),
    ));
    for line in dest_original_lines {
        refs.push(PomodoroBlockRef::removed(line.saturating_sub(1)));
    }
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            note_pull: None,
            task_theme: None,
            type_relocation: None,
            ok: true,
            dry_run: request.dry_run,
            routed: false,
            route: None,
            route_label: String::new(),
            relative_target: relative_target.to_string_lossy().into_owned(),
            target: day_file.display().to_string(),
            text: parsed.body.clone(),
            task_line: source_line_text,
            kind: capture_kind_label(&parsed.kind),
            fanout: None,
            created: date_string(today),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Toggled,
            sub_bullets: Vec::new(),
            notes: Vec::new(),
            drawers: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: None,
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            drawer: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: None,
            status_symbol: None,
            status_name: None,
            previous_status_symbol: None,
            previous_status_name: None,
            pomodoro_name: Some(after_name),
            creates_pomodoro: None,
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: None,
            pomodoro_link_action: None,
            pomodoro_link_source: None,
            pomodoro_link_destination: None,
            project_note: None,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_split: None,
            pomodoro_merge: Some(summary),
            pomodoro_close: None,
            pomodoro_reset: None,
            dependency_update: None,
            consumed_links: Vec::new(),
            task_complete: None,
            toggle_task_description: None,
            r#ref: None,
            link_guard: None,
            dropped_dependencies: Vec::new(),
            blocks_line_updates: Vec::new(),
            minted_dependent_block_id: None,
            dependency_updates: Vec::new(),
            minted_blocker_block_id: None,
            relation_updates: Vec::new(),
            relation_cancels: Vec::new(),
            relation_completes: Vec::new(),
            relation_merge_output: Default::default(),
            linked_from_line_updates: Vec::new(),
            minted_relation_block_ids: Vec::new(),
            mirror_skipped: Vec::new(),
            list: None,
            relation_label_hides: Vec::new(),
            relation_successors: Vec::new(),
            relation_held: Vec::new(),
            relation_check: None,
            relation_groups: Vec::new(),
        },
        clip_plan: None,
        pomodoro_refs: refs,
        task_block_refs: Vec::new(),
        project_move_landings: Vec::new(),
    })
}
