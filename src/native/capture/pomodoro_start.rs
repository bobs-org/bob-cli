//! Pomodoro start planning and placement.
use super::*;

pub(super) fn compute_pomodoro_start_range(
    now: chrono::NaiveDateTime,
    spec: &PomodoroStartSpec,
) -> Result<(String, String, u64, String), CaptureError> {
    use chrono::Timelike;
    let duration_minutes = spec.duration_units.checked_mul(5).ok_or_else(|| {
        CaptureError::usage("Pomodoro start suffix is too large; use a smaller duration or offset")
    })?;
    let offset_minutes = spec.offset_units.checked_mul(5).ok_or_else(|| {
        CaptureError::usage("Pomodoro start suffix is too large; use a smaller duration or offset")
    })?;
    let offset_i64: i64 = offset_minutes.try_into().map_err(|_| {
        CaptureError::usage("Pomodoro start suffix is too large; use a smaller duration or offset")
    })?;
    let duration_i64: i64 = duration_minutes.try_into().map_err(|_| {
        CaptureError::usage("Pomodoro start suffix is too large; use a smaller duration or offset")
    })?;
    let now_minutes = (now.time().hour() * 60 + now.time().minute()) as i64;
    let numerator = now_minutes.checked_sub(offset_i64).ok_or_else(|| {
        CaptureError::usage("Pomodoro start suffix is too large; use a smaller duration or offset")
    })?;
    let quot = if numerator >= 0 {
        numerator.checked_add(4).ok_or_else(|| {
            CaptureError::usage(
                "Pomodoro start suffix is too large; use a smaller duration or offset",
            )
        })? / 5
    } else {
        numerator / 5
    };
    let start_raw = quot.checked_mul(5).ok_or_else(|| {
        CaptureError::usage("Pomodoro start suffix is too large; use a smaller duration or offset")
    })?;
    let end_raw = start_raw.checked_add(duration_i64).ok_or_else(|| {
        CaptureError::usage("Pomodoro start suffix is too large; use a smaller duration or offset")
    })?;
    let start_mod = ((start_raw % 1440) + 1440) % 1440;
    let end_mod = ((end_raw % 1440) + 1440) % 1440;
    let start = format!("{:02}{:02}", start_mod / 60, start_mod % 60);
    let end = format!("{:02}{:02}", end_mod / 60, end_mod % 60);
    let time_range = format!("(**{start}-{end}** [t:: {duration_minutes}m])");
    Ok((start, end, duration_minutes, time_range))
}

/// Low-level link/task start used by Pomodoro task and link captures.
/// The fourth tuple element is the block-tracker `before` for the started
/// entry: `At` of the pre-state headline the start moved, or `Created`
/// when the start created its entry.
pub(super) fn plan_pomodoro_start(
    original_day: &str,
    block_link: &str,
    pomodoro_name: Option<&str>,
    spec: &PomodoroStartSpec,
    now: chrono::NaiveDateTime,
) -> Result<
    (String, Placement, PomodoroStartSummary, PomodoroBlockBefore),
    CaptureError,
> {
    let (start, end, duration_minutes, time_range) =
        compute_pomodoro_start_range(now, spec)?;
    let lines = line_spans(original_day);
    let line_text = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let section =
        pomodoro::pomodoros_section_range(&line_text).ok_or_else(|| {
            CaptureError::io("Bob daily note has no Pomodoros section")
        })?;
    let scan = capture_pomodoros::scan(original_day);
    if !scan.has_section {
        return Err(CaptureError::io(
            "Bob daily note has no Pomodoros section",
        ));
    }
    if scan.entries.iter().any(|entry| {
        entry.state == capture_pomodoros::PomodoroState::Open
            && entry.time_range.is_some()
    }) {
        return Err(CaptureError::io(
            "Bob daily note has an active timed Pomodoro; finish the current Pomodoro first (close it with `=x`)",
        ));
    }
    if let Some(selector) = pomodoro_name {
        match capture_pomodoros::select_named(&scan, selector) {
            capture_pomodoros::NamedSelection::Found(entry) => {
                if !(entry.state == capture_pomodoros::PomodoroState::Open
                    && entry.placeholder
                    && entry.time_range.is_none())
                {
                    return Err(CaptureError::io(format!(
                        "selected Pomodoro `#{}` is not an untimed open placeholder; finish the current Pomodoro first or choose an untimed placeholder",
                        entry.slug
                    )));
                }
                let index = entry.line.checked_sub(1).ok_or_else(|| {
                    CaptureError::io("Pomodoro capture invariant failed: named entry has no line")
                })?;
                let (started_day, moved_index) = start_existing_pomodoro_entry(
                    original_day,
                    index,
                    &time_range,
                )?;
                let (updated_day, placement) = append_pomodoro_child_link(
                    &started_day,
                    moved_index,
                    &section,
                    block_link,
                )?;
                let resolved_name = started_day
                    .lines()
                    .nth(moved_index)
                    .and_then(|_| {
                        capture_pomodoros::scan(&started_day)
                            .entries
                            .iter()
                            .find(|candidate| candidate.line == moved_index + 1)
                            .and_then(|candidate| candidate.name.clone())
                    })
                    .or_else(|| {
                        scan.entries
                            .iter()
                            .find(|candidate| candidate.line == entry.line)
                            .and_then(|candidate| candidate.name.clone())
                    });
                return Ok((
                    updated_day,
                    placement,
                    PomodoroStartSummary {
                        start,
                        end,
                        duration_minutes,
                        offset_units: spec.offset_units,
                        pomodoro_name: resolved_name,
                        pomodoro_line: moved_index + 1,
                        created_pomodoro: false,
                        time_range,
                        tasks: None,
                        drop: Vec::new(),
                        dropped: Vec::new(),
                    },
                    PomodoroBlockBefore::At(index),
                ));
            }
            capture_pomodoros::NamedSelection::CompletedOnly(_)
            | capture_pomodoros::NamedSelection::Missing { .. } => {
                let canonical =
                    capture_pomodoros::canonicalize_pomodoro_name(selector)
                        .ok_or_else(|| {
                            CaptureError::usage(
                                capture_pomodoros::POMODORO_NAME_USAGE,
                            )
                        })?;
                let (updated_day, placement, created_line) =
                    create_started_pomodoro_entry(
                        original_day,
                        &lines,
                        &section,
                        Some(&canonical),
                        &time_range,
                        block_link,
                    )?;
                return Ok((
                    updated_day,
                    placement,
                    PomodoroStartSummary {
                        start,
                        end,
                        duration_minutes,
                        offset_units: spec.offset_units,
                        pomodoro_name: Some(canonical),
                        pomodoro_line: created_line + 1,
                        created_pomodoro: true,
                        time_range,
                        tasks: None,
                        drop: Vec::new(),
                        dropped: Vec::new(),
                    },
                    PomodoroBlockBefore::Created,
                ));
            }
        }
    }
    if let Some(entry) = capture_pomodoros::next_future_pomodoro(&scan) {
        let index = entry.line.checked_sub(1).ok_or_else(|| {
            CaptureError::io("Pomodoro capture invariant failed: placeholder entry has no line")
        })?;
        if line_text
            .get(index)
            .is_some_and(|text| pomodoro::completed_ledger_task(text).is_some())
        {
            return Err(CaptureError::io(
                "selected Pomodoro is not an untimed open placeholder",
            ));
        }
        let (started_day, moved_index) =
            start_existing_pomodoro_entry(original_day, index, &time_range)?;
        let (updated_day, placement) = append_pomodoro_child_link(
            &started_day,
            moved_index,
            &section,
            block_link,
        )?;
        let resolved_name = capture_pomodoros::scan(&started_day)
            .entries
            .iter()
            .find(|candidate| candidate.line == moved_index + 1)
            .and_then(|candidate| candidate.name.clone())
            .or_else(|| {
                scan.entries
                    .iter()
                    .find(|candidate| candidate.line == entry.line)
                    .and_then(|candidate| candidate.name.clone())
            });
        return Ok((
            updated_day,
            placement,
            PomodoroStartSummary {
                start,
                end,
                duration_minutes,
                offset_units: spec.offset_units,
                pomodoro_name: resolved_name,
                pomodoro_line: moved_index + 1,
                created_pomodoro: false,
                time_range,
                tasks: None,
                drop: Vec::new(),
                dropped: Vec::new(),
            },
            PomodoroBlockBefore::At(index),
        ));
    }
    let (updated_day, placement, created_line) = create_started_pomodoro_entry(
        original_day,
        &lines,
        &section,
        None,
        &time_range,
        block_link,
    )?;
    Ok((
        updated_day,
        placement,
        PomodoroStartSummary {
            start,
            end,
            duration_minutes,
            offset_units: spec.offset_units,
            pomodoro_name: None,
            pomodoro_line: created_line + 1,
            created_pomodoro: true,
            time_range,
            tasks: None,
            drop: Vec::new(),
            dropped: Vec::new(),
        },
        PomodoroBlockBefore::Created,
    ))
}

pub(super) fn replace_placeholder_range(
    contents: &str,
    line_index: usize,
    time_range: &str,
) -> Result<(String, Placement), CaptureError> {
    let lines = line_spans(contents);
    let line = lines.get(line_index).ok_or_else(|| {
        CaptureError::io("Pomodoro capture invariant failed: placeholder line is out of range")
    })?;
    let line_start = if line_index == 0 {
        0
    } else {
        lines[line_index - 1].end
    };
    let text = line.text;
    let open_offset = text.find('(').ok_or_else(|| {
        CaptureError::io("selected Pomodoro is not an untimed open placeholder")
    })?;
    let after_open = open_offset + 1;
    let mut close_offset = None;
    for (offset, character) in text[after_open..].char_indices() {
        if character == ')' {
            close_offset = Some(after_open + offset);
            break;
        }
        if !matches!(character, ' ' | '\t') {
            return Err(CaptureError::io(
                "selected Pomodoro is not an untimed open placeholder",
            ));
        }
    }
    let close_offset = close_offset.ok_or_else(|| {
        CaptureError::io("selected Pomodoro is not an untimed open placeholder")
    })?;
    let global_open = line_start + open_offset;
    let global_close = line_start + close_offset + 1;
    if !contents[global_open..global_close]
        .chars()
        .all(|character| matches!(character, '(' | ')' | ' ' | '\t'))
    {
        return Err(CaptureError::io(
            "selected Pomodoro is not an untimed open placeholder",
        ));
    }
    let mut updated = String::with_capacity(contents.len() + time_range.len());
    updated.push_str(&contents[..global_open]);
    updated.push_str(time_range);
    updated.push_str(&contents[global_close..]);
    Ok((updated, Placement::Inserted))
}

pub(super) fn append_pomodoro_child_link(
    contents: &str,
    selected: usize,
    section: &std::ops::Range<usize>,
    block_link: &str,
) -> Result<(String, Placement), CaptureError> {
    let lines = line_spans(contents);
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
    let block = format!("- {block_link}");
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
    Ok((insert_at(contents, insertion_index, &addition), placement))
}

pub(super) fn pomodoro_placement_scan(
    lines: &[LineSpan<'_>],
    section: &std::ops::Range<usize>,
) -> (Vec<usize>, Vec<usize>) {
    let line_text = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let fenced = super::markdown::fenced_lines(&line_text, section.clone());
    let mut completed = Vec::new();
    let mut opens = Vec::new();
    for index in section.clone() {
        if fenced.contains(&index) {
            continue;
        }
        if is_indented_line(lines[index].text) {
            continue;
        }
        if pomodoro::completed_ledger_task(lines[index].text).is_some() {
            completed.push(index);
        } else if pomodoro::open_ledger_task(lines[index].text).is_some() {
            opens.push(index);
        }
    }
    (completed, opens)
}

pub(super) fn move_started_pomodoro_to_current_slot(
    contents: &str,
    entry_index: usize,
) -> Result<(String, usize), CaptureError> {
    let lines = line_spans(contents);
    let line_texts = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let section =
        pomodoro::pomodoros_section_range(&line_texts).ok_or_else(|| {
            CaptureError::io("Bob daily note has no Pomodoros section")
        })?;
    if entry_index >= lines.len() {
        return Err(CaptureError::io(
            "Pomodoro capture invariant failed: started entry is out of range",
        ));
    }
    let (completed, opens) = pomodoro_placement_scan(&lines, &section);
    if completed.iter().any(|index| *index > entry_index) {
        // A completed entry sits after the started one; fall through to move.
    } else {
        let anchor = completed.last().copied();
        let between = opens.iter().any(|index| {
            *index != entry_index
                && anchor.map_or(true, |anchor| *index > anchor)
                && *index < entry_index
        });
        if !between {
            return Ok((contents.to_string(), entry_index));
        }
    }
    let anchor = completed.last().copied();
    let first_other_open =
        opens.iter().find(|index| **index != entry_index).copied();
    let slot = new_pomodoro_insertion_index(
        &lines,
        &section,
        anchor,
        first_other_open,
    );
    let block_start = line_start(&lines, entry_index);
    let block_end = task_block_end(&lines, entry_index);
    if slot == block_start || slot == block_end {
        return Ok((contents.to_string(), entry_index));
    }
    let block_len = block_end - block_start;
    let block_text = contents[block_start..block_end].to_string();
    let without =
        format!("{}{}", &contents[..block_start], &contents[block_end..]);
    let slot_in_without = if slot <= block_start {
        slot
    } else {
        slot - block_len
    };
    let without_lines = line_spans(&without);
    let new_index = line_index_at_offset(&without_lines, slot_in_without);
    let ending = document_line_ending(contents);
    let has_final_newline = contents.ends_with('\n');
    let updated = if has_final_newline {
        format!(
            "{}{}{}",
            &without[..slot_in_without],
            block_text,
            &without[slot_in_without..]
        )
    } else {
        let block_at_eof = block_end == contents.len();
        let slot_at_eof = slot_in_without == without.len();
        if block_at_eof && !slot_at_eof {
            let mut moved = format!(
                "{}{}{}{}",
                &without[..slot_in_without],
                block_text,
                ending,
                &without[slot_in_without..]
            );
            if let Some(stripped) = moved.strip_suffix(ending) {
                moved = stripped.to_string();
            }
            moved
        } else if !block_at_eof && slot_at_eof {
            let mut moved = format!("{without}{ending}{block_text}");
            if let Some(stripped) = moved.strip_suffix(ending) {
                moved = stripped.to_string();
            }
            moved
        } else {
            format!(
                "{}{}{}",
                &without[..slot_in_without],
                block_text,
                &without[slot_in_without..]
            )
        }
    };
    Ok((updated, new_index))
}

pub(super) fn start_existing_pomodoro_entry(
    contents: &str,
    index: usize,
    time_range: &str,
) -> Result<(String, usize), CaptureError> {
    let (started, _) = replace_placeholder_range(contents, index, time_range)?;
    move_started_pomodoro_to_current_slot(&started, index)
}

pub(super) fn create_started_pomodoro_entry(
    contents: &str,
    lines: &[LineSpan<'_>],
    section: &std::ops::Range<usize>,
    name: Option<&str>,
    time_range: &str,
    block_link: &str,
) -> Result<(String, Placement, usize), CaptureError> {
    let (completed, opens) = pomodoro_placement_scan(lines, section);
    let insertion_index = new_pomodoro_insertion_index(
        lines,
        section,
        completed.last().copied(),
        opens.first().copied(),
    );
    let indentation = completed
        .last()
        .and_then(|index| {
            child_bullet_indentation(
                lines,
                index + 1,
                task_block_end(lines, *index),
            )
        })
        .or_else(|| {
            nearby_child_bullet_indentation(lines, section.start, section.end)
        })
        .unwrap_or_else(|| "  ".to_string());
    let ledger_line = match name {
        Some(resolved) => format!("- [ ] {time_range} — {resolved}"),
        None => format!("- [ ] {time_range}"),
    };
    let indented_block = format!("{indentation}- {block_link}");
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
    Ok((updated, placement, created_line))
}

pub(super) fn reject_pomodoro_start_conflicts(
    _parsed: &ParsedCaptureText,
    request: &CaptureRequest,
) -> Result<(), CaptureError> {
    if !request.forced_destination_flags.is_empty()
        || request.forced_route.is_some()
        || request.forced_section.is_some()
        || request.forced_sub_bullet_target.is_some()
        || request.forced_task_section.is_some()
        || request.forced_clip.is_some()
    {
        return Err(CaptureError::usage(
            POMODORO_START_FORCED_ERROR.to_string(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_pomodoro_start_item(
    request: &CaptureRequest,
    parsed: ParsedCaptureText,
    spec: PomodoroStartSpec,
    pomodoro_name: Option<String>,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    warnings: &mut Vec<String>,
) -> Result<PlannedCaptureItem, CaptureError> {
    reject_pomodoro_start_conflicts(&parsed, request)?;
    let (start, end, duration_minutes, time_range) =
        compute_pomodoro_start_range(now, &spec)?;
    let day_file = pomodoro::day_file_for(&request.bob_dir);
    let rel = close_day_relative(&request.bob_dir, &day_file);
    if let Some(selector) = pomodoro_name {
        return plan_named_pomodoro_start_item(
            request,
            parsed,
            &spec,
            &selector,
            &start,
            &end,
            duration_minutes,
            &time_range,
            now,
            today,
            planner,
            warnings,
            &day_file,
            &rel,
        );
    }
    if !planner.currently_exists(&day_file)? {
        return Err(CaptureError::io(format!(
            "no future Pomodoro to start: today's daily note `{rel}` does not exist"
        )));
    }
    let staged = planner.read_existing(&day_file)?;
    let scan = capture_pomodoros::scan(&staged);
    if !scan.has_section {
        return Err(CaptureError::io(
            "Bob daily note has no Pomodoros section",
        ));
    }
    let timed_open = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == capture_pomodoros::PomodoroState::Open
                && entry.time_range.is_some()
        })
        .collect::<Vec<_>>();
    if timed_open.len() == 1 {
        let running = timed_open[0];
        let range = running.time_range.clone().unwrap_or_default();
        let subject =
            match running.name.as_deref().filter(|name| !name.is_empty()) {
                Some(name) => format!("{name} {range}"),
                None => format!("the current session {range}"),
            };
        // Echo the full typed token (including any `~<K>` drop list) so
        // the taught switch idiom stays copy-pasteable.
        let token = parsed.body.clone();
        return Err(CaptureError::io(format!(
            "cannot start the next Pomodoro: {subject} is still running at line {}; close it with `=x` first, or capture `=x`, a blank line, then `{token}` to switch sessions",
            running.line,
        )));
    }
    if timed_open.len() > 1 {
        return Err(CaptureError::io(
            "cannot start the next Pomodoro: today's ledger has multiple open timed Pomodoros; finish all but one first",
        ));
    }
    let Some(entry) = capture_pomodoros::next_future_pomodoro(&scan) else {
        return Err(CaptureError::io(format!(
            "no future Pomodoro to start: today's ledger (`{rel}`) has no open `- [ ] ()` placeholder (start a new named session with `=#<name>`, or a task's session with `^route:block-id=`)"
        )));
    };
    let index = entry.line.checked_sub(1).ok_or_else(|| {
        CaptureError::io(
            "Pomodoro capture invariant failed: started entry is out of range",
        )
    })?;
    let entry_name = entry.name.clone();
    // Drops apply after the guards above and before the start rewrite, on
    // the staged pre-image; removals sit after the entry line, so `index`
    // still points at the entry afterwards.
    let token = parsed.body.clone();
    let owner = match entry_name.as_deref().filter(|name| !name.is_empty()) {
        Some(name) => capture_pomodoro_start::StartOwner::Named {
            name: name.to_string(),
        },
        None => capture_pomodoro_start::StartOwner::Next,
    };
    let drop_plan = capture_pomodoro_start::plan_start_drop(
        &staged, index, &spec.drop, &owner, &token,
    )
    .map_err(|error| CaptureError::io(error.to_string()))?;
    let (updated, moved_index) =
        start_existing_pomodoro_entry(&drop_plan.contents, index, &time_range)?;
    let task_line = line_spans(&updated)
        .get(moved_index)
        .map(|line| line.text.to_string())
        .ok_or_else(|| {
            CaptureError::io(
                "Pomodoro capture invariant failed: started entry is out of range",
            )
        })?;
    planner.stage(&day_file, updated)?;
    warnings.extend(drop_plan.warnings);
    let relative_target = day_file
        .strip_prefix(&request.bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| day_file.clone());
    // Queued-task lineup: zip the post-image day file's direct-child Task
    // Links with the kept pre-image numbers and resolve both kept and
    // dropped rows read-only through the batch planner's staged vault view,
    // so a task captured earlier in the same draft resolves.
    let staged_day = planner.read_existing(&day_file)?;
    let vault = SnapshotCloseVault::from_planner(planner, &request.bob_dir);
    let (tasks, dropped) = plan_start_rows_json(
        &vault,
        &day_file,
        &staged_day,
        moved_index,
        &drop_plan.kept,
        &drop_plan.dropped,
    );
    let mut drop_list = spec.drop.clone();
    drop_list.sort_unstable();
    let summary = PomodoroStartSummary {
        start,
        end,
        duration_minutes,
        offset_units: spec.offset_units,
        pomodoro_name: entry_name.clone(),
        pomodoro_line: moved_index + 1,
        created_pomodoro: false,
        time_range,
        tasks: Some(tasks),
        drop: drop_list,
        dropped,
    };
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            ok: true,
            dry_run: request.dry_run,
            routed: false,
            route: None,
            route_label: String::new(),
            relative_target: relative_target.to_string_lossy().into_owned(),
            target: day_file.display().to_string(),
            text: parsed.body.clone(),
            task_line,
            kind: capture_kind_label(&parsed.kind),
            created: date_string(today),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Started,
            sub_bullets: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: None,
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            parent_section: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: None,
            status_symbol: None,
            status_name: None,
            previous_status_symbol: None,
            previous_status_name: None,
            pomodoro_name: entry_name,
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
            pomodoro_start: Some(summary),
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            dependency_update: None,
            toggle_task_description: None,
        },
        clip_plan: None,
        pomodoro_refs: vec![PomodoroBlockRef::at(
            PomodoroBlockRole::Started,
            index,
            moved_index,
        )],
        task_block_refs: Vec::new(),
    })
}

/// One lineup row as JSON, with the close's explicit-null convention:
/// unresolved rows carry `None` fields plus a `warning`.
fn start_task_json(
    task: &capture_pomodoro_start::StartTaskRow,
    index: u32,
    nested_lines: u32,
) -> PomodoroStartTaskJson {
    PomodoroStartTaskJson {
        index,
        block_link: task.block_link.clone(),
        embedded: task.embedded,
        ledger_line: task.ledger_line,
        resolved: task.resolved,
        relative_target: task.relative_target.clone(),
        block_id: task.block_id.clone(),
        text: task.text.clone(),
        status_symbol: task.status_symbol,
        status_name: task.status_name.clone(),
        warning: task.warning.clone(),
        nested_lines,
    }
}

/// Zip the post-image lineup at `moved_index` with the kept pre-image
/// numbers (in ledger order, so a drop leaves gaps) and resolve the
/// dropped rows, all through the same staged vault view.
fn plan_start_rows_json(
    vault: &SnapshotCloseVault,
    day_file: &Path,
    staged_day: &str,
    moved_index: usize,
    kept: &[u32],
    dropped: &[capture_pomodoro_start::StartDroppedLink],
) -> (Vec<PomodoroStartTaskJson>, Vec<PomodoroStartTaskJson>) {
    let links =
        capture_pomodoro_start::list_queued_links(staged_day, moved_index);
    let rows =
        capture_pomodoro_start::resolve_queued_links(vault, day_file, &links);
    let tasks = rows
        .iter()
        .zip(kept.iter().copied())
        .map(|(row, index)| start_task_json(row, index, 0))
        .collect();
    // Resolution maps each link to exactly one row, so the zip below
    // holds every dropped row.
    let dropped_links: Vec<capture_pomodoro_start::StartLink> =
        dropped.iter().map(|row| row.link.clone()).collect();
    let dropped_rows = capture_pomodoro_start::resolve_queued_links(
        vault,
        day_file,
        &dropped_links,
    );
    let dropped = dropped
        .iter()
        .zip(dropped_rows.iter())
        .map(|(row, task)| {
            start_task_json(task, row.link.index, row.nested_lines)
        })
        .collect();
    (tasks, dropped)
}

fn named_relocation_error(
    error: capture_task_toggle::LinkRelocationError,
) -> CaptureError {
    match error {
        capture_task_toggle::LinkRelocationError::NoPomodorosSection => {
            CaptureError::io("Bob daily note has no Pomodoros section")
        }
        capture_task_toggle::LinkRelocationError::NoEligibleOpenEntry => {
            CaptureError::io("Bob daily note has no eligible open Pomodoro")
        }
        capture_task_toggle::LinkRelocationError::MultipleOpenTimedEntries => {
            CaptureError::io(
                "Bob daily note has multiple open timed Pomodoros",
            )
        }
        capture_task_toggle::LinkRelocationError::InvalidPomodoroName => {
            CaptureError::usage(capture_pomodoros::POMODORO_NAME_USAGE)
        }
        capture_task_toggle::LinkRelocationError::NoMovableLink
        | capture_task_toggle::LinkRelocationError::MultipleMovableLinks => {
            CaptureError::io(
                "Pomodoro capture invariant failed: named start needs no movable link",
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_named_pomodoro_start_item(
    request: &CaptureRequest,
    parsed: ParsedCaptureText,
    spec: &PomodoroStartSpec,
    selector: &str,
    start: &str,
    end: &str,
    duration_minutes: u64,
    time_range: &str,
    _now: chrono::NaiveDateTime,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    warnings: &mut Vec<String>,
    day_file: &Path,
    rel: &str,
) -> Result<PlannedCaptureItem, CaptureError> {
    let fallback_name = capture_pomodoros::canonicalize_pomodoro_name(selector)
        .unwrap_or_else(|| format!("`#{selector}`"));
    if !planner.currently_exists(day_file)? {
        return Err(CaptureError::io(format!(
            "cannot start {fallback_name}: today's daily note `{rel}` does not exist"
        )));
    }
    let staged = planner.read_existing(day_file)?;
    let scan = capture_pomodoros::scan(&staged);
    if !scan.has_section {
        return Err(CaptureError::io(
            "Bob daily note has no Pomodoros section",
        ));
    }
    let selection = capture_pomodoros::select_named(&scan, selector);
    let display_name = match &selection {
        capture_pomodoros::NamedSelection::Found(entry) => {
            entry.name.clone().unwrap_or_else(|| fallback_name.clone())
        }
        capture_pomodoros::NamedSelection::CompletedOnly(entry) => entry
            .name
            .as_deref()
            .and_then(capture_pomodoros::canonicalize_pomodoro_name)
            .or_else(|| capture_pomodoros::canonicalize_pomodoro_name(selector))
            .unwrap_or_else(|| format!("`#{selector}`")),
        capture_pomodoros::NamedSelection::Missing { .. } => {
            fallback_name.clone()
        }
    };
    let timed_open = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == capture_pomodoros::PomodoroState::Open
                && entry.time_range.is_some()
        })
        .collect::<Vec<_>>();
    if timed_open.len() > 1 {
        return Err(CaptureError::io(format!(
            "cannot start {display_name}: today's ledger has multiple open timed Pomodoros; finish all but one first"
        )));
    }
    if let Some(running) = timed_open.first() {
        let range = running.time_range.clone().unwrap_or_default();
        let is_found_running = matches!(
            &selection,
            capture_pomodoros::NamedSelection::Found(entry)
                if entry.line == running.line
        );
        if is_found_running {
            return Err(CaptureError::io(format!(
                "{display_name} is already running ({range} at line {}); use `+N`/`-N` to resize it, `++N`/`--N` to shift it, or `=x` to close it",
                running.line,
            )));
        }
        let subject =
            match running.name.as_deref().filter(|name| !name.is_empty()) {
                Some(name) => format!("{name} {range}"),
                None => format!("the current session {range}"),
            };
        return Err(CaptureError::io(format!(
            "cannot start {display_name}: {subject} is still running at line {}; close it with `=x` first, or capture `=x {}` to switch sessions",
            running.line,
            parsed.body,
        )));
    }
    let start_before = match &selection {
        capture_pomodoros::NamedSelection::Found(entry) => {
            PomodoroBlockBefore::At(entry.line.saturating_sub(1))
        }
        capture_pomodoros::NamedSelection::CompletedOnly(_)
        | capture_pomodoros::NamedSelection::Missing { .. } => {
            PomodoroBlockBefore::Created
        }
    };
    // A created or "again" named session starts empty, so any drop list
    // fails before insertion with the created-session diagnostic.
    if !spec.drop.is_empty() {
        match &selection {
            capture_pomodoros::NamedSelection::Found(_) => {}
            _ => {
                let mut bad = spec.drop.clone();
                bad.sort_unstable();
                bad.dedup();
                return Err(CaptureError::io(
                    capture_pomodoro_start::StartDropError {
                        token: parsed.body.clone(),
                        bad_numbers: bad,
                        total: 0,
                        owner: capture_pomodoro_start::StartOwner::Created {
                            name: display_name.clone(),
                        },
                    }
                    .to_string(),
                ));
            }
        }
    }
    let (updated, moved_index, created, suggestion_warning, drop_plan) =
        match selection {
            capture_pomodoros::NamedSelection::Found(entry) => {
                if !(entry.state == capture_pomodoros::PomodoroState::Open
                    && entry.placeholder
                    && entry.time_range.is_none())
                {
                    return Err(CaptureError::io(format!(
                        "selected Pomodoro `#{}` is not an untimed open placeholder; finish the current Pomodoro first or choose an untimed placeholder",
                        entry.slug
                    )));
                }
                let index = entry.line.checked_sub(1).ok_or_else(|| {
                    CaptureError::io(
                        "Pomodoro capture invariant failed: started entry is out of range",
                    )
                })?;
                // Drops apply after the guards above and before the start
                // rewrite, on the staged pre-image.
                let token = parsed.body.clone();
                let owner = capture_pomodoro_start::StartOwner::Named {
                    name: display_name.clone(),
                };
                let drop_plan = capture_pomodoro_start::plan_start_drop(
                    &staged, index, &spec.drop, &owner, &token,
                )
                .map_err(|error| CaptureError::io(error.to_string()))?;
                let (updated, moved_index) = start_existing_pomodoro_entry(
                    &drop_plan.contents,
                    index,
                    time_range,
                )?;
                (updated, moved_index, false, None, Some(drop_plan))
            }
            capture_pomodoros::NamedSelection::CompletedOnly(entry) => {
                let name = entry
                    .name
                    .as_deref()
                    .and_then(capture_pomodoros::canonicalize_pomodoro_name)
                    .or_else(|| {
                        capture_pomodoros::canonicalize_pomodoro_name(selector)
                    })
                    .ok_or_else(|| {
                        CaptureError::usage(format!(
                            "cannot create Pomodoro `#{selector}`: {}",
                            capture_pomodoros::POMODORO_NAME_USAGE
                        ))
                    })?;
                let (with_placeholder, created_line, _) =
                    capture_task_toggle::insert_named_placeholder(
                        &staged, &name,
                    )
                    .map_err(named_relocation_error)?;
                let (updated, moved_index) = start_existing_pomodoro_entry(
                    &with_placeholder,
                    created_line,
                    time_range,
                )?;
                (updated, moved_index, true, None, None)
            }
            capture_pomodoros::NamedSelection::Missing { suggestion } => {
                let name =
                    capture_pomodoros::canonicalize_pomodoro_name(selector)
                        .ok_or_else(|| {
                            CaptureError::usage(format!(
                                "cannot create Pomodoro `#{selector}`: {}",
                                capture_pomodoros::POMODORO_NAME_USAGE
                            ))
                        })?;
                let warning = suggestion.map(|entry| {
                    let suggestion_name = entry
                        .name
                        .clone()
                        .unwrap_or_else(|| entry.slug.clone());
                    let slug = entry.slug.clone();
                    let raw = spec.raw.clone();
                    format!(
                        "no open Pomodoro matches `#{selector}`; created {name} (did you mean {suggestion_name}? use `={raw}#{slug}`)"
                    )
                });
                let (with_placeholder, created_line, _) =
                    capture_task_toggle::insert_named_placeholder(
                        &staged, &name,
                    )
                    .map_err(named_relocation_error)?;
                let (updated, moved_index) = start_existing_pomodoro_entry(
                    &with_placeholder,
                    created_line,
                    time_range,
                )?;
                (updated, moved_index, true, warning, None)
            }
        };
    planner.stage(day_file, updated)?;
    if let Some(warning) = suggestion_warning {
        warnings.push(warning);
    }
    let relative_target = day_file
        .strip_prefix(&request.bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| day_file.to_path_buf());
    let staged_day = planner.read_existing(day_file)?;
    let vault = SnapshotCloseVault::from_planner(planner, &request.bob_dir);
    // Zip the post-image lineup with the kept pre-image numbers; created
    // sessions keep the whole (empty) lineup.
    let (kept, dropped_links) = match drop_plan.as_ref() {
        Some(plan) => (plan.kept.clone(), plan.dropped.clone()),
        None => (Vec::new(), Vec::new()),
    };
    let (tasks, dropped) = if drop_plan.is_some() {
        let (tasks, dropped) = plan_start_rows_json(
            &vault,
            day_file,
            &staged_day,
            moved_index,
            &kept,
            &dropped_links,
        );
        if let Some(plan) = drop_plan.as_ref() {
            warnings.extend(plan.warnings.clone());
        }
        (tasks, dropped)
    } else {
        let links =
            capture_pomodoro_start::list_queued_links(&staged_day, moved_index);
        let rows = capture_pomodoro_start::resolve_queued_links(
            &vault, day_file, &links,
        );
        let tasks = rows
            .iter()
            .map(|row| start_task_json(row, row.index, 0))
            .collect();
        (tasks, Vec::new())
    };
    let task_line = line_spans(&staged_day)
        .get(moved_index)
        .map(|line| line.text.to_string())
        .ok_or_else(|| {
            CaptureError::io(
                "Pomodoro capture invariant failed: started entry is out of range",
            )
        })?;
    let mut drop_list = spec.drop.clone();
    drop_list.sort_unstable();
    let summary = PomodoroStartSummary {
        start: start.to_string(),
        end: end.to_string(),
        duration_minutes,
        offset_units: spec.offset_units,
        pomodoro_name: Some(display_name.clone()),
        pomodoro_line: moved_index + 1,
        created_pomodoro: created,
        time_range: time_range.to_string(),
        tasks: Some(tasks),
        drop: drop_list,
        dropped,
    };
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            ok: true,
            dry_run: request.dry_run,
            routed: false,
            route: None,
            route_label: String::new(),
            relative_target: relative_target.to_string_lossy().into_owned(),
            target: day_file.display().to_string(),
            text: parsed.body.clone(),
            task_line,
            kind: capture_kind_label(&parsed.kind),
            created: date_string(today),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Started,
            sub_bullets: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: None,
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            parent_section: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: None,
            status_symbol: None,
            status_name: None,
            previous_status_symbol: None,
            previous_status_name: None,
            pomodoro_name: Some(display_name),
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
            pomodoro_start: Some(summary),
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            dependency_update: None,
            toggle_task_description: None,
        },
        clip_plan: None,
        pomodoro_refs: vec![PomodoroBlockRef {
            role: PomodoroBlockRole::Started,
            before: start_before,
            after: Some(moved_index),
        }],
        task_block_refs: Vec::new(),
    })
}
