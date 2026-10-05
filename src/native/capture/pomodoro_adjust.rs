//! Pomodoro adjust/shift planning and adjustment-range parsing.
use super::*;

pub(super) fn reject_duplicate_block_id(
    markdown: &str,
    block_id: &str,
    target: &Path,
) -> Result<(), CaptureError> {
    if collect_done::block_ids_in_markdown(markdown)
        .iter()
        .any(|existing| existing == block_id)
    {
        return Err(CaptureError::io(format!(
            "block ID ^{block_id} already exists in {}",
            target.display()
        )));
    }
    Ok(())
}

pub(super) fn reject_task_toggle_conflicts(
    parsed: &ParsedCaptureText,
    request: &CaptureRequest,
) -> Result<(), CaptureError> {
    if !request.forced_destination_flags.is_empty() {
        return Err(CaptureError::usage(format!(
            "task toggle capture cannot be combined with {}",
            request.forced_destination_flags.join(", ")
        )));
    }
    if request.forced_clip.is_some() {
        return Err(CaptureError::usage(
            "task toggle capture cannot be combined with --clip",
        ));
    }
    if parsed.clip.is_some() {
        return Err(CaptureError::usage(
            "task toggle capture cannot be combined with % clipboard markers",
        ));
    }
    if parsed.scheduled_offset.is_some() {
        return Err(CaptureError::usage(
            "task toggle capture cannot be combined with s:<N>",
        ));
    }
    if parsed.priority_level.is_some() {
        return Err(CaptureError::usage(
            "task toggle capture cannot be combined with p:<N>",
        ));
    }
    if !parsed.sub_bullets.is_empty() {
        return Err(CaptureError::usage(
            "task toggle capture cannot have authored child bullets",
        ));
    }
    Ok(())
}

pub(super) fn reject_pomodoro_adjust_conflicts(
    parsed: &ParsedCaptureText,
    request: &CaptureRequest,
) -> Result<(), CaptureError> {
    if !request.forced_destination_flags.is_empty() {
        return Err(CaptureError::usage(format!(
            "Pomodoro adjustment `+N`/`-N` cannot be combined with {}; capture the adjustment alone",
            request.forced_destination_flags.join(", ")
        )));
    }
    if request.forced_route.is_some()
        || request.forced_section.is_some()
        || request.forced_sub_bullet_target.is_some()
        || request.forced_task_section.is_some()
    {
        return Err(CaptureError::usage(
            "Pomodoro adjustment `+N`/`-N` cannot be combined with --route, --section, --task, --task-ref, or --task-section; capture the adjustment alone",
        ));
    }
    if request.forced_clip.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro adjustment `+N`/`-N` cannot be combined with --clip; capture the adjustment alone",
        ));
    }
    if parsed.route.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro adjustment items must contain only the signed count (for example `+5`); remove extra text, markers, or child lines",
        ));
    }
    if parsed.clip.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro adjustment `+N`/`-N` cannot be combined with % clipboard markers; capture the adjustment alone",
        ));
    }
    if parsed.scheduled_offset.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro adjustment `+N`/`-N` cannot be combined with s:<N>; capture the adjustment alone",
        ));
    }
    if parsed.priority_level.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro adjustment `+N`/`-N` cannot be combined with p:<N>; capture the adjustment alone",
        ));
    }
    if !parsed.sub_bullets.is_empty() {
        return Err(CaptureError::usage(
            "Pomodoro adjustment items must contain only the signed count (for example `+5`); remove extra text, markers, or child lines",
        ));
    }
    Ok(())
}

/// The running session both Pomodoro operators act on: today's single
/// open, timed, column-zero entry, plus its line span and parsed range.
pub(super) struct RunningSessionTarget {
    pub(super) day_file: PathBuf,
    pub(super) staged: String,
    pub(super) line: usize,
    pub(super) name: Option<String>,
    pub(super) line_text: String,
    pub(super) segment_start: usize,
    pub(super) range: AdjustRange,
}

/// Select today's running session through the staged daily file: day-file
/// existence, `## Pomodoros` section, exactly one open timed entry, line
/// span/segment start, and parsed range. `verb` is `adjust` or `shift` and
/// only changes the error copy.
pub(super) fn select_running_session(
    request: &CaptureRequest,
    planner: &mut CaptureBatchPlanner,
    verb: &str,
) -> Result<RunningSessionTarget, CaptureError> {
    let invariant = if verb == "shift" {
        "Pomodoro shift invariant failed: target line is out of range"
    } else {
        "Pomodoro adjustment invariant failed: target line is out of range"
    };
    let day_file = pomodoro::day_file_for(&request.bob_dir);
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
    let timed_open = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == capture_pomodoros::PomodoroState::Open
                && entry.time_range.is_some()
        })
        .collect::<Vec<_>>();
    if timed_open.is_empty() {
        let mut message =
            format!("Bob daily note has no open timed Pomodoro to {verb}");
        if let Some(next) = capture_pomodoros::next_future_pomodoro(&scan) {
            match next.name.as_deref().filter(|name| !name.is_empty()) {
                Some(name) => message.push_str(&format!(
                    "; next up is {name} at line {} (start it with `=`)",
                    next.line,
                )),
                None => message.push_str(&format!(
                    "; next up at line {} (start it with `=`)",
                    next.line,
                )),
            }
        }
        return Err(CaptureError::io(message));
    }
    if timed_open.len() > 1 {
        return Err(CaptureError::io(format!(
            "Bob daily note has multiple open timed Pomodoros; finish all but one before {verb}ing"
        )));
    }
    let target = timed_open[0];
    let line_index = target
        .line
        .checked_sub(1)
        .ok_or_else(|| CaptureError::io(invariant))?;
    let lines = line_spans(&staged);
    let line = lines
        .get(line_index)
        .ok_or_else(|| CaptureError::io(invariant))?;
    let line_text = line.text.to_string();
    let segment_start = if line_index == 0 {
        0
    } else {
        lines[line_index - 1].end
    };
    let range = parse_adjustment_range(&line_text).ok_or_else(|| {
        CaptureError::io(format!(
            "selected Pomodoro has an unparseable time range and cannot be {verb}ed"
        ))
    })?;
    Ok(RunningSessionTarget {
        day_file,
        staged,
        line: target.line,
        name: target.name.clone(),
        line_text,
        segment_start,
        range,
    })
}

pub(super) fn reject_pomodoro_shift_conflicts(
    parsed: &ParsedCaptureText,
    request: &CaptureRequest,
) -> Result<(), CaptureError> {
    if !request.forced_destination_flags.is_empty() {
        return Err(CaptureError::usage(format!(
            "Pomodoro shift `++N`/`--N` cannot be combined with {}; capture the shift alone",
            request.forced_destination_flags.join(", ")
        )));
    }
    if request.forced_route.is_some()
        || request.forced_section.is_some()
        || request.forced_sub_bullet_target.is_some()
        || request.forced_task_section.is_some()
    {
        return Err(CaptureError::usage(
            "Pomodoro shift `++N`/`--N` cannot be combined with --route, --section, --task, --task-ref, or --task-section; capture the shift alone",
        ));
    }
    if request.forced_clip.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro shift `++N`/`--N` cannot be combined with --clip; capture the shift alone",
        ));
    }
    if parsed.route.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro shift items must contain only the operator (for example `++3` or `--`); remove extra text, markers, or child lines",
        ));
    }
    if parsed.clip.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro shift `++N`/`--N` cannot be combined with % clipboard markers; capture the shift alone",
        ));
    }
    if parsed.scheduled_offset.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro shift `++N`/`--N` cannot be combined with s:<N>; capture the shift alone",
        ));
    }
    if parsed.priority_level.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro shift `++N`/`--N` cannot be combined with p:<N>; capture the shift alone",
        ));
    }
    if !parsed.sub_bullets.is_empty() {
        return Err(CaptureError::usage(
            "Pomodoro shift items must contain only the operator (for example `++3` or `--`); remove extra text, markers, or child lines",
        ));
    }
    Ok(())
}

pub(super) fn plan_pomodoro_adjust_item(
    request: &CaptureRequest,
    parsed: ParsedCaptureText,
    spec: PomodoroAdjustSpec,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
) -> Result<PlannedCaptureItem, CaptureError> {
    reject_pomodoro_adjust_conflicts(&parsed, request)?;
    let session = select_running_session(request, planner, "adjust")?;
    let day_file = session.day_file;
    let staged = session.staged;
    let target_line = session.line;
    let target_name = session.name;
    let line_text = session.line_text;
    let segment_start = session.segment_start;
    let range = session.range;
    let old_duration = adjustment_duration_minutes(&line_text, &range)
        .unwrap_or_else(|| {
            normalize_minutes(
                range.end_minutes as i64 - range.start_minutes as i64,
            )
        });
    let delta_requested = spec.units.checked_mul(5).ok_or_else(|| {
        CaptureError::usage(
            "Pomodoro adjustment is too large; use a smaller unit count",
        )
    })?;
    let new_duration = if spec.plus {
        old_duration.checked_add(delta_requested).ok_or_else(|| {
            CaptureError::usage(
                "Pomodoro adjustment is too large; use a smaller unit count",
            )
        })?
    } else {
        old_duration.saturating_sub(delta_requested)
    };
    let start_total = range
        .start_minutes
        .checked_add(new_duration)
        .ok_or_else(|| {
            CaptureError::usage(
                "Pomodoro adjustment is too large; use a smaller unit count",
            )
        })?;
    let new_end_minutes = (start_total % 1440) as u16;
    let requested_minutes: i64 = if spec.plus {
        delta_requested.try_into().map_err(|_| {
            CaptureError::usage(
                "Pomodoro adjustment is too large; use a smaller unit count",
            )
        })?
    } else {
        -(i64::try_from(delta_requested).map_err(|_| {
            CaptureError::usage(
                "Pomodoro adjustment is too large; use a smaller unit count",
            )
        })?)
    };
    let actual_minutes = new_duration as i64 - old_duration as i64;
    let new_range_text = format_adjusted_range(
        range.start_minutes,
        new_end_minutes,
        new_duration,
        &range.metadata,
    );
    let global_start = segment_start + range.start_ch;
    let global_end = segment_start + range.end_ch;
    if !staged.is_char_boundary(global_start)
        || !staged.is_char_boundary(global_end)
    {
        return Err(CaptureError::io(
            "Pomodoro adjustment invariant failed: target range is not on a character boundary",
        ));
    }
    let mut updated =
        String::with_capacity(staged.len() + new_range_text.len());
    updated.push_str(&staged[..global_start]);
    updated.push_str(&new_range_text);
    updated.push_str(&staged[global_end..]);
    planner.stage(&day_file, updated)?;
    let before_start = format!(
        "{:02}{:02}",
        range.start_minutes / 60,
        range.start_minutes % 60
    );
    let before_end =
        format!("{:02}{:02}", range.end_minutes / 60, range.end_minutes % 60);
    let after_start = format!(
        "{:02}{:02}",
        range.start_minutes / 60,
        range.start_minutes % 60
    );
    let after_end =
        format!("{:02}{:02}", new_end_minutes / 60, new_end_minutes % 60);
    let new_line = format!(
        "{}{}{}",
        &line_text[..range.start_ch],
        new_range_text,
        &line_text[range.end_ch..]
    );
    let relative_target = day_file
        .strip_prefix(&request.bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| day_file.clone());
    let summary = PomodoroAdjustSummary {
        direction: if spec.plus { "plus" } else { "minus" },
        requested_units: spec.units,
        requested_minutes,
        delta_minutes: actual_minutes,
        before_start,
        before_end,
        before_duration_minutes: old_duration,
        after_start,
        after_end,
        after_duration_minutes: new_duration,
        pomodoro_line: target_line,
        pomodoro_name: target_name.clone(),
        time_range: new_range_text.clone(),
        clamped: actual_minutes != requested_minutes,
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
            text: spec.raw.clone(),
            task_line: new_line,
            kind: capture_kind_label(&parsed.kind),
            created: date_string(today),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Toggled,
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
            pomodoro_name: target_name.clone(),
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
            pomodoro_adjust: Some(summary),
            pomodoro_shift: None,
            pomodoro_close: None,
            dependency_update: None,
            task_complete: None,
            toggle_task_description: None,
        },
        clip_plan: None,
        pomodoro_refs: vec![PomodoroBlockRef::at(
            PomodoroBlockRole::Adjusted,
            target_line.saturating_sub(1),
            target_line.saturating_sub(1),
        )],
        task_block_refs: Vec::new(),
    })
}

pub(super) fn plan_pomodoro_shift_item(
    request: &CaptureRequest,
    parsed: ParsedCaptureText,
    spec: PomodoroShiftSpec,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
) -> Result<PlannedCaptureItem, CaptureError> {
    reject_pomodoro_shift_conflicts(&parsed, request)?;
    let session = select_running_session(request, planner, "shift")?;
    let day_file = session.day_file;
    let staged = session.staged;
    let target_line = session.line;
    let target_name = session.name;
    let segment_start = session.segment_start;
    let line_text = session.line_text;
    let range = session.range;
    let duration = adjustment_duration_minutes(&line_text, &range)
        .unwrap_or_else(|| {
            normalize_minutes(
                range.end_minutes as i64 - range.start_minutes as i64,
            )
        });
    let units_delta = spec.units.checked_mul(5).ok_or_else(|| {
        CaptureError::usage(
            "Pomodoro shift is too large; use a smaller unit count",
        )
    })?;
    let delta_minutes: i64 = if spec.later {
        units_delta.try_into().map_err(|_| {
            CaptureError::usage(
                "Pomodoro shift is too large; use a smaller unit count",
            )
        })?
    } else {
        -(i64::try_from(units_delta).map_err(|_| {
            CaptureError::usage(
                "Pomodoro shift is too large; use a smaller unit count",
            )
        })?)
    };
    let new_start_minutes =
        (((range.start_minutes as i64 + delta_minutes) % 1440 + 1440) % 1440)
            as u64;
    let new_end_minutes = (((range.end_minutes as i64 + delta_minutes) % 1440
        + 1440)
        % 1440) as u64;
    let new_range_text = format_adjusted_range(
        new_start_minutes,
        new_end_minutes as u16,
        duration,
        &range.metadata,
    );
    let global_start = segment_start + range.start_ch;
    let global_end = segment_start + range.end_ch;
    if !staged.is_char_boundary(global_start)
        || !staged.is_char_boundary(global_end)
    {
        return Err(CaptureError::io(
            "Pomodoro shift invariant failed: target range is not on a character boundary",
        ));
    }
    let mut updated =
        String::with_capacity(staged.len() + new_range_text.len());
    updated.push_str(&staged[..global_start]);
    updated.push_str(&new_range_text);
    updated.push_str(&staged[global_end..]);
    planner.stage(&day_file, updated)?;
    let before_start = format!(
        "{:02}{:02}",
        range.start_minutes / 60,
        range.start_minutes % 60
    );
    let before_end =
        format!("{:02}{:02}", range.end_minutes / 60, range.end_minutes % 60);
    let after_start =
        format!("{:02}{:02}", new_start_minutes / 60, new_start_minutes % 60);
    let after_end =
        format!("{:02}{:02}", new_end_minutes / 60, new_end_minutes % 60);
    let new_line = format!(
        "{}{}{}",
        &line_text[..range.start_ch],
        new_range_text,
        &line_text[range.end_ch..]
    );
    let relative_target = day_file
        .strip_prefix(&request.bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| day_file.clone());
    let summary = PomodoroShiftSummary {
        direction: if spec.later { "later" } else { "earlier" },
        requested_units: spec.units,
        delta_minutes,
        before_start,
        before_end,
        after_start,
        after_end,
        duration_minutes: duration,
        pomodoro_line: target_line,
        pomodoro_name: target_name.clone(),
        time_range: new_range_text.clone(),
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
            text: spec.raw.clone(),
            task_line: new_line,
            kind: capture_kind_label(&parsed.kind),
            created: date_string(today),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Toggled,
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
            pomodoro_name: target_name.clone(),
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
            pomodoro_shift: Some(summary),
            pomodoro_close: None,
            dependency_update: None,
            task_complete: None,
            toggle_task_description: None,
        },
        clip_plan: None,
        pomodoro_refs: vec![PomodoroBlockRef::at(
            PomodoroBlockRole::Shifted,
            target_line.saturating_sub(1),
            target_line.saturating_sub(1),
        )],
        task_block_refs: Vec::new(),
    })
}

#[derive(Debug, Clone)]
pub(crate) struct AdjustRange {
    pub(crate) start_ch: usize,
    pub(crate) end_ch: usize,
    pub(crate) start_minutes: u64,
    pub(crate) end_minutes: u64,
    pub(crate) metadata: String,
}

pub(crate) fn parse_adjustment_range(line: &str) -> Option<AdjustRange> {
    let bytes = line.as_bytes();
    let mut search = 0;
    while let Some(relative_open) = line[search..].find('(') {
        let open = search + relative_open;
        let Some(relative_close) = line[open..].find(')') else {
            return None;
        };
        let close = open + relative_close;
        if let Some(range) = parse_adjustment_inner(line, open, close) {
            return Some(range);
        }
        search = close + 1;
        if search >= bytes.len() {
            break;
        }
    }
    None
}

pub(super) fn parse_adjustment_inner(
    line: &str,
    open: usize,
    close: usize,
) -> Option<AdjustRange> {
    let inner = &line[open + 1..close];
    let (inner, bold) = match inner.strip_prefix("**") {
        Some(rest) => (rest, true),
        None => (inner, false),
    };
    let (start_minutes, start_len) = parse_adjustment_time(inner)?;
    let mut rest = &inner[start_len..];
    rest = rest.trim_start_matches([' ', '\t']);
    rest = rest.strip_prefix('-')?;
    rest = rest.trim_start_matches([' ', '\t']);
    let (end_minutes, end_len) = parse_adjustment_time(rest)?;
    rest = &rest[end_len..];
    if bold {
        rest = rest.strip_prefix("**")?;
    } else if rest.starts_with("**") {
        return None;
    }
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let start_ch = open;
    let end_ch = close + 1;
    Some(AdjustRange {
        start_ch,
        end_ch,
        start_minutes,
        end_minutes,
        metadata: rest.to_string(),
    })
}

pub(super) fn parse_adjustment_time(text: &str) -> Option<(u64, usize)> {
    let bytes = text.as_bytes();
    if bytes.len() >= 5
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b':'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
    {
        let hour = ((bytes[0] - b'0') as u64) * 10 + (bytes[1] - b'0') as u64;
        let minute = ((bytes[3] - b'0') as u64) * 10 + (bytes[4] - b'0') as u64;
        if hour > 23 || minute > 59 {
            return None;
        }
        return Some((hour * 60 + minute, 5));
    }
    if bytes.len() >= 4
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
    {
        let hour = ((bytes[0] - b'0') as u64) * 10 + (bytes[1] - b'0') as u64;
        let minute = ((bytes[2] - b'0') as u64) * 10 + (bytes[3] - b'0') as u64;
        if hour > 23 || minute > 59 {
            return None;
        }
        return Some((hour * 60 + minute, 4));
    }
    None
}

pub(crate) fn adjustment_duration_minutes(
    line: &str,
    range: &AdjustRange,
) -> Option<u64> {
    duration_from_range_text(&line[range.start_ch..range.end_ch])
}

pub(crate) fn adjustment_duration_for_range(range: &AdjustRange) -> u64 {
    duration_from_range_text(&range.metadata).unwrap_or_else(|| {
        normalize_minutes(range.end_minutes as i64 - range.start_minutes as i64)
    })
}

pub(super) fn duration_from_range_text(range_text: &str) -> Option<u64> {
    if let Some(value) = duration_field_value(range_text)
        && let Some(minutes) = parse_adjustment_duration(&value)
    {
        return Some(minutes);
    }
    if let Some(value) = legacy_stopwatch_value(range_text)
        && let Some(minutes) = parse_adjustment_duration(&value)
    {
        return Some(minutes);
    }
    None
}

pub(super) fn duration_field_value(range_text: &str) -> Option<String> {
    let lower = range_text.to_ascii_lowercase();
    let relative = lower.find("[t::")?;
    let open = relative;
    let after = open + "[t::".len();
    let relative_close = range_text[after..].find(']')?;
    let close = after + relative_close;
    Some(range_text[after..close].to_string())
}

pub(super) fn legacy_stopwatch_value(range_text: &str) -> Option<String> {
    let stopwatch = '\u{23F1}';
    let mut search = 0;
    while let Some(relative) = range_text[search..].find(stopwatch) {
        let mut index = search + relative + stopwatch.len_utf8();
        if range_text[index..].starts_with('\u{FE0F}') {
            index += '\u{FE0F}'.len_utf8();
        }
        while range_text[index..].starts_with([' ', '\t']) {
            index += 1;
        }
        let rest = &range_text[index..];
        if let Some((value, _)) = parse_stopwatch_duration_prefix(rest) {
            return Some(value);
        }
        search = index.max(search + relative + 1);
        if search >= range_text.len() {
            break;
        }
    }
    None
}

pub(super) fn parse_stopwatch_duration_prefix(
    text: &str,
) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut hours: Option<u64> = None;
    let mut minutes: Option<u64> = None;
    let start = 0;
    let mut digits_start: Option<usize> = None;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        if digits_start.is_none() {
            digits_start = Some(index);
        }
        index += 1;
    }
    if let Some(begin) = digits_start {
        let number = text[begin..index].parse::<u64>().ok()?;
        let saved = index;
        while index < bytes.len() && matches!(bytes[index], b' ' | b'\t') {
            index += 1;
        }
        if index < bytes.len() && (bytes[index] == b'h' || bytes[index] == b'H')
        {
            hours = Some(number);
            index += 1;
            while index < bytes.len() && matches!(bytes[index], b' ' | b'\t') {
                index += 1;
            }
            let mut second_start: Option<usize> = None;
            let mut second_end = index;
            while second_end < bytes.len() && bytes[second_end].is_ascii_digit()
            {
                if second_start.is_none() {
                    second_start = Some(second_end);
                }
                second_end += 1;
            }
            if let Some(begin) = second_start {
                let mut probe = second_end;
                while probe < bytes.len()
                    && matches!(bytes[probe], b' ' | b'\t')
                {
                    probe += 1;
                }
                if probe < bytes.len()
                    && (bytes[probe] == b'm' || bytes[probe] == b'M')
                {
                    let second = text[begin..second_end].parse::<u64>().ok()?;
                    minutes = Some(second);
                    index = probe + 1;
                }
            }
        } else if index < bytes.len()
            && (bytes[index] == b'm' || bytes[index] == b'M')
        {
            minutes = Some(number);
            index += 1;
        } else {
            index = saved;
        }
    }
    match (hours, minutes) {
        (Some(h), Some(m)) => h
            .checked_mul(60)?
            .checked_add(m)
            .map(|_total| (text[start..index].to_string(), index)),
        (Some(h), None) => h
            .checked_mul(60)
            .map(|_total| (text[start..index].to_string(), index)),
        (None, Some(_m)) => Some((text[start..index].to_string(), index)),
        (None, None) => None,
    }
}

pub(super) fn parse_adjustment_duration(value: &str) -> Option<u64> {
    let text = value.trim().to_ascii_lowercase();
    if text.is_empty() {
        return None;
    }
    if let Some(number) = text.strip_suffix('m').and_then(|core| {
        let core = core.trim_end();
        (!core.is_empty() && core.bytes().all(|byte| byte.is_ascii_digit()))
            .then_some(core)
    }) {
        // Distinguish `30m` from `1h 30m`: the latter contains `h`.
        if !text.contains('h') {
            return number.parse::<u64>().ok();
        }
    }
    let (hours_part, minutes_part) = match text.split_once('h') {
        Some((hours, rest)) => (Some(hours), Some(rest)),
        None => (None, Some(text.as_str())),
    };
    let hours = match hours_part {
        Some(part) => {
            let part = part.trim();
            if part.is_empty() {
                0
            } else if part.bytes().all(|byte| byte.is_ascii_digit()) {
                part.parse::<u64>().ok()?
            } else {
                return None;
            }
        }
        None => 0,
    };
    let minutes = match minutes_part {
        Some(part) => {
            let part = part.trim();
            let core = part.strip_suffix('m')?;
            let core = core.trim_end();
            if core.is_empty() {
                return None;
            }
            if !core.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            core.parse::<u64>().ok()?
        }
        None => return None,
    };
    if hours_part.is_none() && minutes_part.is_none() {
        return None;
    }
    if hours == 0 && minutes == 0 && !text.contains(['h', 'm']) {
        return None;
    }
    hours.checked_mul(60)?.checked_add(minutes)
}

pub(super) fn remove_adjustment_duration_metadata(metadata: &str) -> String {
    let without_fields = remove_bracket_duration_fields(metadata);
    let without_legacy = remove_legacy_stopwatch_fields(&without_fields);
    without_legacy
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn remove_bracket_duration_fields(metadata: &str) -> String {
    let mut output = String::with_capacity(metadata.len());
    let mut index = 0;
    let lower = metadata.to_ascii_lowercase();
    while index < metadata.len() {
        if let Some(relative) = lower[index..].find("[t::") {
            let open = index + relative;
            output.push_str(&metadata[index..open]);
            let after = open + "[t::".len();
            if let Some(relative_close) = metadata[after..].find(']') {
                index = after + relative_close + 1;
                continue;
            }
            output.push_str(&metadata[open..]);
            break;
        }
        output.push_str(&metadata[index..]);
        break;
    }
    output
}

pub(super) fn remove_legacy_stopwatch_fields(metadata: &str) -> String {
    let stopwatch = '\u{23F1}';
    let mut output = String::new();
    let mut index = 0;
    while index < metadata.len() {
        if let Some(relative) = metadata[index..].find(stopwatch) {
            let open = index + relative;
            output.push_str(&metadata[index..open]);
            let mut cursor = open + stopwatch.len_utf8();
            if metadata[cursor..].starts_with('\u{FE0F}') {
                cursor += '\u{FE0F}'.len_utf8();
            }
            let probe_start = cursor;
            while cursor < metadata.len()
                && metadata[cursor..].starts_with([' ', '\t'])
            {
                cursor += 1;
            }
            if let Some((_, length)) =
                parse_stopwatch_duration_prefix(&metadata[cursor..])
            {
                index = cursor + length;
                continue;
            }
            output.push_str(&metadata[open..probe_start.max(open + 1)]);
            index = probe_start.max(open + 1);
            continue;
        }
        output.push_str(&metadata[index..]);
        break;
    }
    output
}

pub(crate) fn format_adjusted_range(
    start_minutes: u64,
    end_minutes: u16,
    duration_minutes: u64,
    metadata: &str,
) -> String {
    let start = format!("{:02}{:02}", start_minutes / 60, start_minutes % 60);
    let end = format!("{:02}{:02}", end_minutes / 60, end_minutes % 60);
    let rest = remove_adjustment_duration_metadata(metadata);
    if rest.is_empty() {
        format!("(**{start}-{end}** [t:: {duration_minutes}m])")
    } else {
        format!("(**{start}-{end}** [t:: {duration_minutes}m] {rest})")
    }
}

pub(crate) fn normalize_minutes(value: i64) -> u64 {
    (((value % 1440) + 1440) % 1440) as u64
}
