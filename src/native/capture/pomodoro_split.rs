//! Pomodoro split planning (`=/`).
use super::*;
use crate::native::pomodoro_themes::{
    merged::{
        SelectEntry, SelectFilter, SelectInput, SelectOutcome, SplitError,
        SplitPlanInput,
    },
    routing,
};

pub(super) fn reject_pomodoro_split_conflicts(
    parsed: &ParsedCaptureText,
    request: &CaptureRequest,
) -> Result<(), CaptureError> {
    if !request.forced_destination_flags.is_empty() {
        return Err(CaptureError::usage(format!(
            "Pomodoro split `=/` cannot be combined with {}; capture the split alone",
            request.forced_destination_flags.join(", ")
        )));
    }
    if request.forced_route.is_some()
        || request.forced_section.is_some()
        || request.forced_sub_bullet_target.is_some()
        || request.forced_drawer.is_some()
    {
        return Err(CaptureError::usage(
            "Pomodoro split `=/` cannot be combined with --route, --section, --task, --task-ref, or --drawer; capture the split alone",
        ));
    }
    if request.forced_clip.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro split `=/` cannot be combined with --clip; capture the split alone",
        ));
    }
    if parsed.route.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro split `=/` must be the whole capture item; remove extra text, markers, or child lines",
        ));
    }
    if parsed.clip.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro split `=/` cannot be combined with %c; capture the split alone",
        ));
    }
    if parsed.scheduled_offset.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro split `=/` cannot be combined with s:<N>; capture the split alone",
        ));
    }
    if parsed.priority_level.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro split `=/` cannot be combined with $<N>; capture the split alone",
        ));
    }
    if !parsed.sub_bullets.is_empty() {
        return Err(CaptureError::usage(
            "Pomodoro split `=/` must be the whole capture item; remove extra text, markers, or child lines",
        ));
    }
    if !parsed.dependencies.is_empty()
        || !parsed.blocked_tasks.is_empty()
        || !parsed.relations.is_empty()
    {
        return Err(CaptureError::usage(
            "Pomodoro split `=/` cannot be combined with task relationships; capture the split alone",
        ));
    }
    Ok(())
}

fn split_guard_error(error: &SplitError, target_name: &str) -> CaptureError {
    let code = error.code();
    let message = match error {
        SplitError::Closed => "Closed Pomodoros are history".to_string(),
        SplitError::Meet => {
            "MEET entries never split; calendar sync owns the MEET name"
                .to_string()
        }
        SplitError::NotMerged => {
            let components = routing::type_components(
                Some(target_name),
                &crate::native::config::themes::load_theme_sources(),
            )
            .into_iter()
            .filter(|item| item.is_type)
            .count();
            if components == 0 {
                format!("{target_name} holds any type — nothing to split")
            } else {
                format!("{target_name} has one type — nothing to split")
            }
        }
        SplitError::TypeNotNeeded => {
            format!(
                "{target_name} splits implicitly; use `=/` to split all merged-in types"
            )
        }
        SplitError::BaseType { name } => {
            format!("{name} is the base of {target_name} and stays")
        }
        SplitError::TypeUnknown { selector } => {
            format!("{selector} is not part of {target_name}")
        }
        SplitError::TypeRepeated { name } => {
            format!("{name} is named twice; list each type once")
        }
    };
    CaptureError::usage_code(message, code)
}

fn eligible_merged_examples(
    scan: &crate::native::capture_pomodoros::PomodoroScan,
    sources: &crate::native::config::themes::ThemeSources,
) -> Vec<String> {
    scan.entries
        .iter()
        .filter(|entry| {
            entry.state == crate::native::capture_pomodoros::PomodoroState::Open
                && routing::type_components(entry.name.as_deref(), sources)
                    .iter()
                    .filter(|item| item.is_type)
                    .count()
                    >= 2
        })
        .filter_map(|entry| entry.name.clone())
        .collect()
}

fn split_target_missing_error(
    scan: &crate::native::capture_pomodoros::PomodoroScan,
    sources: &crate::native::config::themes::ThemeSources,
    day_rel: &str,
) -> CaptureError {
    let eligible = eligible_merged_examples(scan, sources);
    if eligible.is_empty() {
        return CaptureError::usage_code(
            format!(
                "No open merged Pomodoro to split in today's ledger (`{day_rel}`)"
            ),
            "split_target_missing",
        );
    }
    let mut examples = Vec::new();
    for name in eligible.iter().take(3) {
        let components = routing::type_components(Some(name), sources);
        let types: Vec<String> = components
            .into_iter()
            .filter(|item| item.is_type)
            .map(|item| item.name)
            .collect();
        if types.len() >= 2 {
            let base = types[0].clone();
            if types.len() == 2 {
                examples.push(format!("`=/` or `=/{base}`"));
            } else {
                let first = types[1].clone();
                examples.push(format!("`=/` or `=/{base}#{first}`"));
            }
        }
    }
    CaptureError::usage_code(
        format!(
            "No matching merged Pomodoro; today's merged entries: {}. Try {}",
            eligible.join(", "),
            examples.join(", ")
        ),
        "split_target_missing",
    )
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

fn resolve_task_type_key(
    task_theme_context: &mut CaptureTaskThemeContext,
    planner: &mut CaptureBatchPlanner,
    residence_route: &str,
    task_line: &str,
    warnings: &mut Vec<CaptureWarning>,
) -> Option<String> {
    task_theme_context
        .placement_for_task(planner, residence_route, task_line, warnings)
        .map(|placed| placed.suggestion.theme.key.clone())
}

fn find_task_line(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &std::path::Path,
    day_file: &std::path::Path,
    path_part: &str,
    block_id: &str,
) -> Option<(String, String)> {
    let target_path = if path_part.is_empty() {
        day_file.to_path_buf()
    } else {
        bob_dir.join(format!("{path_part}.md"))
    };
    let residence_route = if path_part.is_empty() {
        day_file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("today")
            .to_string()
    } else {
        path_part.to_string()
    };
    let contents = if let Some(staged) = planner.peek_text(&target_path) {
        Some(staged)
    } else if target_path.exists()
        && let Ok(text) = std::fs::read_to_string(&target_path)
    {
        Some(text)
    } else {
        None
    }?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    match scan.by_block_id(block_id) {
        note_tasks::BlockIdLookup::Found(task) => {
            let line = contents.lines().nth(task.line_index)?.to_string();
            Some((line, residence_route))
        }
        _ => None,
    }
}

pub(super) fn plan_pomodoro_split_item(
    request: &CaptureRequest,
    parsed: ParsedCaptureText,
    spec: PomodoroSplitSpec,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    task_theme_context: &mut CaptureTaskThemeContext,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<PlannedCaptureItem, CaptureError> {
    reject_pomodoro_split_conflicts(&parsed, request)?;
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
    let entries: Vec<SelectEntry<'_>> = scan
        .entries
        .iter()
        .map(|entry| SelectEntry {
            name: entry.name.as_deref(),
            open: entry.state
                == crate::native::capture_pomodoros::PomodoroState::Open,
            running: entry.is_current,
            timed: entry.time_range.is_some(),
        })
        .collect();
    let target_index = if let Some(base) = spec.base.as_deref() {
        let input = SelectInput {
            entries: &entries,
            selector: base,
            filter: SelectFilter::MergedBase,
            sources: &sources,
        };
        match crate::native::pomodoro_themes::merged::select_by_type(&input) {
            SelectOutcome::Entry(index) => index,
            SelectOutcome::Missing => {
                return Err(split_target_missing_error(
                    &scan, &sources, &day_rel,
                ));
            }
        }
    } else {
        let timed_open: Vec<usize> = scan
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.state
                    == crate::native::capture_pomodoros::PomodoroState::Open
                    && entry.time_range.is_some()
            })
            .map(|(index, _)| index)
            .collect();
        if timed_open.len() > 1 {
            return Err(CaptureError::io(
                "cannot split the current Pomodoro: today's ledger has multiple open timed Pomodoros; finish all but one first",
            ));
        }
        if timed_open.len() == 1 {
            timed_open[0]
        } else if let Some(next) =
            crate::native::capture_pomodoros::next_future_pomodoro(&scan)
        {
            scan.entries
                .iter()
                .position(|entry| {
                    entry.line == next.line
                        && entry.name == next.name
                        && entry.state == next.state
                })
                .ok_or_else(|| {
                    split_target_missing_error(&scan, &sources, &day_rel)
                })?
        } else {
            return Err(split_target_missing_error(&scan, &sources, &day_rel));
        }
    };
    let target = scan
        .entries
        .get(target_index)
        .ok_or_else(|| split_target_missing_error(&scan, &sources, &day_rel))?;
    let target_name = target.name.clone().unwrap_or_default();
    let target_open =
        target.state == crate::native::capture_pomodoros::PomodoroState::Open;
    let entry_line = target.line;
    if entry_line == 0 {
        return Err(CaptureError::io(
            "Pomodoro split invariant failed: target entry has no line",
        ));
    }
    let spans = line_spans(&staged);
    let line_texts: Vec<&str> = spans.iter().map(|span| span.text).collect();
    let entry_index = entry_line.checked_sub(1).ok_or_else(|| {
        CaptureError::io(
            "Pomodoro split invariant failed: target entry is out of range",
        )
    })?;
    if line_texts.get(entry_index).is_none() {
        return Err(CaptureError::io(
            "Pomodoro split invariant failed: target entry is out of range",
        ));
    }
    let top_indices = top_level_child_indices(&spans, entry_index);
    let fenced = markdown::fenced_lines(&line_texts, 0..line_texts.len());
    let range = if top_indices.is_empty() {
        entry_index + 1..entry_index + 1
    } else {
        let start = *top_indices.first().unwrap_or(&(entry_index + 1));
        let end = crate::native::capture_task_toggle::child_block_end_line(
            &spans,
            entry_index,
        ) + 1;
        start..end.min(line_texts.len())
    };
    let numbered =
        crate::native::capture_pomodoro_close::number_task_links_in_range(
            &line_texts,
            &fenced,
            range,
        );
    let top_set: std::collections::BTreeSet<usize> =
        top_indices.iter().copied().collect();
    let mut ordered_links: Vec<(usize, String, String)> = Vec::new();
    for link in numbered {
        let zero = link.line.checked_sub(1).unwrap_or(usize::MAX);
        if !top_set.contains(&zero) {
            continue;
        }
        ordered_links.push((zero, link.path_part, link.block_id));
    }
    ordered_links.sort_by_key(|(line, _, _)| *line);
    let mut link_keys: Vec<Option<String>> = Vec::new();
    for (_, path_part, block_id) in ordered_links.iter() {
        let resolved = find_task_line(
            planner,
            &request.bob_dir,
            &day_file,
            path_part,
            block_id,
        );
        if let Some((task_line, residence)) = resolved {
            let key = resolve_task_type_key(
                task_theme_context,
                planner,
                &residence,
                &task_line,
                warnings,
            );
            link_keys.push(key);
        } else {
            link_keys.push(None);
        }
    }
    let link_key_refs: Vec<Option<&str>> =
        link_keys.iter().map(|key| key.as_deref()).collect();
    let selector_strings: Vec<String> = spec
        .selectors
        .iter()
        .map(|selector| selector.text.clone())
        .collect();
    let selector_refs: Vec<&str> = selector_strings
        .iter()
        .map(std::string::String::as_str)
        .collect();
    let split_selectors: Option<&[&str]> = if selector_refs.is_empty() {
        None
    } else {
        Some(&selector_refs)
    };
    let plan_input = SplitPlanInput {
        name: &target_name,
        open: target_open,
        link_keys: &link_key_refs,
        split_selectors,
        sources: &sources,
    };
    let plan =
        match crate::native::pomodoro_themes::merged::split_plan(&plan_input) {
            Ok(plan) => plan,
            Err(error) => return Err(split_guard_error(&error, &target_name)),
        };
    let before_name = target_name.clone();
    let after_name = plan.remaining_name.clone();
    let Some(renamed) =
        crate::native::pomodoro_themes::relocation_apply::rename_entry(
            &staged,
            entry_line,
            &after_name,
        )
    else {
        return Err(CaptureError::io(
            "Pomodoro split invariant failed: cannot rename source entry",
        ));
    };
    let mut current = renamed;
    let mut spans_now = line_spans(&current);
    let mut source_end =
        crate::native::capture_task_toggle::child_block_end_line(
            &spans_now,
            entry_index,
        );
    let ending = if current[..spans_now[source_end].end.min(current.len())]
        .ends_with("\r\n")
    {
        "\r\n"
    } else {
        "\n"
    };
    let mut created_lines: Vec<usize> = Vec::new();
    for created in plan.created.iter() {
        let placeholder =
            crate::native::capture_pomodoros::format_named_placeholder_line(
                &created.name,
            );
        let insert_offset = spans_now
            .get(source_end)
            .map(|span| span.end)
            .unwrap_or_else(|| current.len());
        current = format!(
            "{}{}{}{}",
            &current[..insert_offset],
            placeholder,
            ending,
            &current[insert_offset..]
        );
        spans_now = line_spans(&current);
        source_end += 1;
        created_lines.push(source_end + 1);
        let _ = spans_now;
    }
    for created in plan.created.iter() {
        for _ in created.link_indices.iter() {
            spans_now = line_spans(&current);
            let line_texts_now: Vec<&str> =
                spans_now.iter().map(|span| span.text).collect();
            let src_end_now =
                crate::native::capture_task_toggle::child_block_end_line(
                    &spans_now,
                    entry_index,
                );
            let tops = top_level_child_indices(&spans_now, entry_index);
            if tops.is_empty() {
                break;
            }
            let fenced_now = markdown::fenced_lines(
                &line_texts_now,
                0..line_texts_now.len(),
            );
            let start = *tops.first().unwrap();
            let end = (src_end_now + 1).min(line_texts_now.len());
            let numbered_now = crate::native::capture_pomodoro_close::number_task_links_in_range(
                &line_texts_now,
                &fenced_now,
                start..end,
            );
            let top_now: std::collections::BTreeSet<usize> =
                tops.iter().copied().collect();
            let mut moved = false;
            for link in numbered_now {
                let zero = link.line.checked_sub(1).unwrap_or(usize::MAX);
                if !top_now.contains(&zero) {
                    continue;
                }
                let resolved = find_task_line(
                    planner,
                    &request.bob_dir,
                    &day_file,
                    &link.path_part,
                    &link.block_id,
                );
                let key =
                    resolved.as_ref().and_then(|(task_line, residence)| {
                        resolve_task_type_key(
                            task_theme_context,
                            planner,
                            residence,
                            task_line,
                            warnings,
                        )
                    });
                let wanted = crate::native::plan_budget::normalize_component(
                    &created.name,
                );
                if key.as_deref().is_some_and(|key| {
                    crate::native::plan_budget::normalize_component(key)
                        == wanted
                }) {
                    let dest_entry_index = {
                        let scan_now = capture_pomodoros::scan(&current);
                        scan_now
                            .entries
                            .iter()
                            .find(|entry| {
                                entry.line > entry_line
                                    && entry.name.as_deref()
                                        == Some(created.name.as_str())
                            })
                            .and_then(|entry| entry.line.checked_sub(1))
                            .unwrap_or_else(|| entry_index + 1)
                    };
                    let sub_end = subtree_end_for(&spans_now, zero);
                    match crate::native::capture_task_toggle::move_subtree_to_entry(
                        &current,
                        zero,
                        sub_end,
                        dest_entry_index,
                    ) {
                        Ok((updated, _)) => {
                            current = updated;
                            moved = true;
                            break;
                        }
                        Err(_) => {
                            return Err(CaptureError::io(
                                "Pomodoro split invariant failed: cannot move Task Link subtree",
                            ));
                        }
                    }
                }
            }
            if !moved {
                break;
            }
        }
    }
    let final_scan = capture_pomodoros::scan(&current);
    let stayed = {
        let spans_final = line_spans(&current);
        let texts_final: Vec<&str> =
            spans_final.iter().map(|span| span.text).collect();
        let src_idx = entry_index;
        if texts_final.get(src_idx).is_none() {
            0
        } else {
            let tops = top_level_child_indices(&spans_final, src_idx);
            if tops.is_empty() {
                0
            } else {
                let fenced_final =
                    markdown::fenced_lines(&texts_final, 0..texts_final.len());
                let start = *tops.first().unwrap();
                let src_end_final =
                    crate::native::capture_task_toggle::child_block_end_line(
                        &spans_final,
                        src_idx,
                    );
                let end = (src_end_final + 1).min(texts_final.len());
                let links_final = crate::native::capture_pomodoro_close::number_task_links_in_range(
                    &texts_final,
                    &fenced_final,
                    start..end,
                );
                let top_final: std::collections::BTreeSet<usize> =
                    tops.iter().copied().collect();
                links_final
                    .into_iter()
                    .filter(|link| {
                        link.line
                            .checked_sub(1)
                            .is_some_and(|zero| top_final.contains(&zero))
                    })
                    .count()
            }
        }
    };
    let final_entries = final_scan.entries.clone();
    let mut final_created_lines: Vec<usize> = Vec::new();
    {
        let mut used: std::collections::BTreeSet<usize> = Default::default();
        for created in plan.created.iter() {
            let mut found: Option<usize> = None;
            for entry in final_entries.iter() {
                if entry.line <= entry_line {
                    continue;
                }
                if used.contains(&entry.line) {
                    continue;
                }
                if entry.name.as_deref() == Some(created.name.as_str()) {
                    found = Some(entry.line);
                    break;
                }
            }
            if let Some(line) = found {
                used.insert(line);
                final_created_lines.push(line);
            } else {
                final_created_lines.push(entry_line + 1);
            }
        }
    }
    planner.stage(&day_file, current.clone())?;
    let relative_target = day_file
        .strip_prefix(&request.bob_dir)
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|_| day_file.clone());
    let base_component = routing::type_components(Some(&before_name), &sources)
        .into_iter()
        .find(|item| item.is_type);
    let (base_key, base_name) = base_component
        .map(|item| (item.key, item.name))
        .unwrap_or_else(|| ("".to_string(), before_name.clone()));
    let base_ref = catalog.theme_ref(&base_key, &base_name);
    let mut created_json = Vec::new();
    for (created_index, created) in plan.created.iter().enumerate() {
        let key =
            crate::native::plan_budget::normalize_component(&created.name);
        let type_ref = catalog.theme_ref(&key, &created.name);
        let line = final_created_lines
            .get(created_index)
            .copied()
            .unwrap_or(entry_line + 1 + created_index);
        created_json.push(PomodoroSplitCreated {
            type_ref,
            line,
            name: created.name.clone(),
            moved_links: created.link_indices.len(),
        });
    }
    let summary = PomodoroSplitSummary {
        path: relative_target.to_string_lossy().into_owned(),
        entry_line,
        before: before_name.clone(),
        after: after_name.clone(),
        base: base_ref,
        created: created_json,
        stayed_links: stayed,
    };
    let source_line_text =
        current.lines().nth(entry_index).unwrap_or("").to_string();
    let mut refs = Vec::new();
    refs.push(PomodoroBlockRef::at(
        PomodoroBlockRole::Split,
        entry_index,
        entry_index,
    ));
    for line in final_created_lines.iter() {
        let after = line.checked_sub(1).unwrap_or(*line);
        refs.push(PomodoroBlockRef::created(PomodoroBlockRole::Split, after));
    }
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
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
            pomodoro_split: Some(summary),
            pomodoro_merge: None,
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
            linked_from_line_updates: Vec::new(),
            minted_relation_block_ids: Vec::new(),
            mirror_skipped: Vec::new(),
            relation_label_hides: Vec::new(),
        },
        clip_plan: None,
        pomodoro_refs: refs,
        task_block_refs: Vec::new(),
    })
}
