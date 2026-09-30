//! Pomodoro close planning, summaries, and link/task close items.
use super::*;

pub(super) struct SnapshotCloseVault {
    pub(super) bob_dir: PathBuf,
    pub(super) resolver: vault_links::VaultLinkResolver,
    pub(super) staged: BTreeMap<PathBuf, Option<String>>,
}

impl SnapshotCloseVault {
    pub(super) fn from_planner(
        planner: &CaptureBatchPlanner,
        bob_dir: &Path,
    ) -> Self {
        Self {
            bob_dir: bob_dir.to_path_buf(),
            resolver: vault_links::VaultLinkResolver::new(
                bob_dir.to_path_buf(),
            ),
            staged: planner.staged_snapshot(),
        }
    }

    pub(super) fn read_filesystem(
        path: &Path,
    ) -> Result<Option<String>, String> {
        if !path.is_file() {
            return Ok(None);
        }
        std::fs::read_to_string(path)
            .map(Some)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))
    }

    /// The staged-new file a `[[target#^id]]` link names, if exactly one
    /// exists. Only files absent from the filesystem qualify, so notes the
    /// resolver already sees keep their existing resolution.
    pub(super) fn staged_new_target(&self, target: &str) -> Option<PathBuf> {
        let direct = self.bob_dir.join(format!("{target}.md"));
        if self
            .staged
            .get(&direct)
            .is_some_and(|contents| contents.is_some())
            && !direct.is_file()
        {
            return Some(direct);
        }
        if target.contains('/') || target.contains('\\') {
            return None;
        }
        let mut matches = self
            .staged
            .iter()
            .filter(|(path, contents)| {
                contents.is_some()
                    && !path.is_file()
                    && path.file_stem().is_some_and(|stem| stem == target)
            })
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            matches.pop()
        } else {
            None
        }
    }
}

impl capture_pomodoro_close::CloseVault for SnapshotCloseVault {
    fn bob_dir(&self) -> &Path {
        &self.bob_dir
    }

    fn resolve_target(
        &self,
        from_path: &Path,
        target: &str,
    ) -> vault_links::LinkResolution {
        if target.is_empty() {
            return vault_links::LinkResolution::Found(from_path.to_path_buf());
        }
        // A note created earlier in the same batch exists only in the
        // staged snapshot, where the filesystem resolver cannot see it.
        // Mirror the resolver's direct-join-then-basename order over those
        // staged-new files so same-draft tasks resolve.
        if let Some(path) = self.staged_new_target(target) {
            return vault_links::LinkResolution::Found(path);
        }
        match self.resolver.resolve(from_path, target) {
            vault_links::LinkResolution::Found(path) => {
                // Normalize to absolute so staged keys match the batch
                // planner's absolute paths.
                if path.is_absolute() {
                    vault_links::LinkResolution::Found(path)
                } else {
                    vault_links::LinkResolution::Found(self.bob_dir.join(path))
                }
            }
            other => other,
        }
    }

    fn read_latest(&self, path: &Path) -> Result<Option<String>, String> {
        if let Some(cached) = self.staged.get(path) {
            return Ok(cached.clone());
        }
        if path.is_relative()
            && let Some(cached) = self.staged.get(&self.bob_dir.join(path))
        {
            return Ok(cached.clone());
        }
        // Not yet loaded by the batch planner: the planner has staged
        // nothing for it, so the filesystem is current. A planner-keyed
        // path already carries the vault dir and must not be joined again.
        let filesystem_path =
            if path.is_absolute() || path.starts_with(&self.bob_dir) {
                path.to_path_buf()
            } else {
                self.bob_dir.join(path)
            };
        Self::read_filesystem(&filesystem_path)
    }
}

pub(super) fn format_close_hhmm(minutes: u64) -> String {
    format!("{:02}{:02}", minutes / 60, minutes % 60)
}

pub(super) fn close_day_relative(bob_dir: &Path, day_file: &Path) -> String {
    day_file
        .strip_prefix(bob_dir)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| day_file.display().to_string())
}

pub(super) fn parse_close_hhmm_range(text: &str) -> Option<(u64, u64)> {
    let (start_text, end_text) = text.split_once('-')?;
    if start_text.len() != 4
        || end_text.len() != 4
        || !start_text.bytes().all(|byte| byte.is_ascii_digit())
        || !end_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let start = start_text.parse::<u64>().ok()?;
    let end = end_text.parse::<u64>().ok()?;
    let start_minutes = (start / 100) * 60 + (start % 100);
    let end_minutes = (end / 100) * 60 + (end % 100);
    (start_minutes < 1440 && end_minutes < 1440)
        .then_some((start_minutes, end_minutes))
}

pub(super) fn signed_close_delta(end_minutes: u64, closed_at: u64) -> i64 {
    let mut delta = end_minutes as i64 - closed_at as i64;
    while delta <= -720 {
        delta += 1440;
    }
    while delta > 720 {
        delta -= 1440;
    }
    delta
}

pub(super) fn close_no_running_error(
    rel: &str,
    next_name: Option<&str>,
    next_line: Option<usize>,
    link_hint: Option<&str>,
) -> CaptureError {
    let mut message = format!(
        "no running Pomodoro to close: today's ledger (`{rel}`) has no open timed entry"
    );
    if let (Some(name), Some(line)) = (next_name, next_line) {
        let display = if name.is_empty() { "unnamed" } else { name };
        message.push_str(&format!("; next up is {display} at line {line}"));
    } else if let Some(line) = next_line {
        message.push_str(&format!("; next up at line {line}"));
    }
    if link_hint.is_none() && next_line.is_some() {
        message.push_str(" (start it with `=`)");
    }
    if let Some(hint) = link_hint {
        message.push_str(&format!(
            " (to start a session with this task instead, use `{hint}=`)"
        ));
    }
    CaptureError::io(message)
}

/// Pre-image running-entry diagnostics for link/body closes. The link step
/// inserts into _R_'s sub-bullet range, so a close planned afterwards would
/// report shifted line numbers; check before linking instead.
pub(super) fn pre_link_running_error(
    day_contents: &str,
    rel: &str,
    link_hint: &str,
) -> Option<CaptureError> {
    match capture_pomodoro_close::find_running_pomodoro(day_contents) {
        Ok(_) => None,
        Err(find_error) => Some(map_close_plan_error(
            capture_pomodoro_close::PomodoroClosePlanError::FindRunning(
                find_error,
            ),
            rel,
            Some(link_hint),
        )),
    }
}

/// Next-placeholder context from pre-link contents, so link forms keep the
/// "; next up is NAME at line N" tail of the none-running diagnostic.
pub(super) fn close_next_from_contents(
    day_contents: &str,
) -> (Option<String>, Option<usize>) {
    match capture_pomodoro_close::find_running_pomodoro(day_contents) {
        Err(capture_pomodoro_close::FindRunningError::NoneRunning {
            next_name,
            next_line,
        }) => (next_name, next_line),
        _ => (None, None),
    }
}

pub(super) fn map_close_plan_error(
    error: capture_pomodoro_close::PomodoroClosePlanError,
    rel: &str,
    link_hint: Option<&str>,
) -> CaptureError {
    match error {
        capture_pomodoro_close::PomodoroClosePlanError::FindRunning(
            capture_pomodoro_close::FindRunningError::NoSection,
        ) => CaptureError::io("Bob daily note has no Pomodoros section"),
        capture_pomodoro_close::PomodoroClosePlanError::FindRunning(
            capture_pomodoro_close::FindRunningError::NoneRunning {
                next_name,
                next_line,
            },
        ) => close_no_running_error(
            rel,
            next_name.as_deref(),
            next_line,
            link_hint,
        ),
        capture_pomodoro_close::PomodoroClosePlanError::FindRunning(
            capture_pomodoro_close::FindRunningError::Multiple(entries),
        ) => {
            let list = entries
                .iter()
                .map(|entry| {
                    let name = entry
                        .name
                        .as_deref()
                        .filter(|name| !name.is_empty())
                        .unwrap_or("unnamed");
                    format!("{name} at line {}", entry.line)
                })
                .collect::<Vec<_>>()
                .join(", ");
            CaptureError::io(format!(
                "multiple open timed Pomodoros ({list}); finish all but one before closing"
            ))
        }
        capture_pomodoro_close::PomodoroClosePlanError::VaultRead(message) => {
            CaptureError::io(message)
        }
        capture_pomodoro_close::PomodoroClosePlanError::Selection(error) => {
            CaptureError::io(error.to_string())
        }
    }
}

pub(super) fn build_close_summary_json(
    spec: &PomodoroCloseSpec,
    day_relative: &str,
    plan: &capture_pomodoro_close::PomodoroClosePlan,
    now: chrono::NaiveDateTime,
) -> (PomodoroCloseSummaryJson, Vec<String>) {
    use chrono::Timelike;
    let running = &plan.summary.running;
    let ledger = &plan.summary.ledger;
    let closed_at_minutes =
        u64::from(now.time().hour()) * 60 + u64::from(now.time().minute());
    let closed_at = format_close_hhmm(closed_at_minutes);
    let mut extra_warnings = Vec::new();
    let (planned, closed, remaining_minutes, decremented_minutes) =
        match &ledger.timing {
            Some(timing) => (
                PomodoroCloseTimingJson {
                    start: format_close_hhmm(timing.planned_start),
                    end: format_close_hhmm(timing.planned_end),
                    duration_minutes: timing.planned_duration,
                    time_range: format!(
                        "{}-{}",
                        format_close_hhmm(timing.planned_start),
                        format_close_hhmm(timing.planned_end)
                    ),
                },
                PomodoroCloseTimingJson {
                    start: format_close_hhmm(timing.closed_start),
                    end: format_close_hhmm(timing.closed_end),
                    duration_minutes: timing.closed_duration,
                    time_range: format!(
                        "{}-{}",
                        format_close_hhmm(timing.closed_start),
                        format_close_hhmm(timing.closed_end)
                    ),
                },
                timing.remaining_minutes,
                timing.decremented_minutes,
            ),
            None => {
                extra_warnings.push(
                    "running Pomodoro time range was left as written: it could not be parsed"
                        .to_string(),
                );
                let (planned_start, planned_end) =
                    parse_close_hhmm_range(&running.time_range)
                        .unwrap_or((0, 0));
                let duration = normalize_minutes(
                    planned_end as i64 - planned_start as i64,
                );
                let remaining =
                    signed_close_delta(planned_end, closed_at_minutes);
                let timing = PomodoroCloseTimingJson {
                    start: format_close_hhmm(planned_start),
                    end: format_close_hhmm(planned_end),
                    duration_minutes: duration,
                    time_range: format!(
                        "{}-{}",
                        format_close_hhmm(planned_start),
                        format_close_hhmm(planned_end)
                    ),
                };
                (timing.clone(), timing, remaining, 0)
            }
        };
    let entry_line = ledger
        .contents
        .lines()
        .nth(running.line.saturating_sub(1))
        .unwrap_or("")
        .to_string();
    let tasks = plan
        .summary
        .tasks
        .iter()
        .map(|task| PomodoroCloseTaskJson {
            role: task.role.as_str(),
            block_link: task.block_link.clone(),
            ledger_line: task.ledger_line,
            index: task.index,
            resolved: task.resolved,
            relative_target: task.relative_target.clone(),
            block_id: task.block_id.clone(),
            text: task.text.clone(),
            previous_status_symbol: task.previous_status_symbol,
            previous_status_name: task.previous_status_name.clone(),
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            status_changed: task.status_changed,
            carried: task.carried,
            work_log: task.work_log.clone(),
            work_log_created: task.work_log_created,
            typed_work_log: task.typed_work_log.clone(),
            warning: task.warning.clone(),
        })
        .collect();
    let mut carried_links: Vec<_> = ledger
        .classified_links
        .iter()
        .filter(|link| link.carried)
        .collect();
    // Carry order: worked-on lines in source order, then deferred lines in
    // source order (matching the ledger's carried-lines order).
    carried_links.sort_by_key(|link| {
        let group = match link.role {
            capture_pomodoro_close::LedgerLinkRole::Worked
            | capture_pomodoro_close::LedgerLinkRole::Mentioned => 0,
            capture_pomodoro_close::LedgerLinkRole::Deferred => 1,
            capture_pomodoro_close::LedgerLinkRole::Struck
            | capture_pomodoro_close::LedgerLinkRole::Embedded
            | capture_pomodoro_close::LedgerLinkRole::Dropped => 2,
        };
        (group, link.line)
    });
    let carried = carried_links
        .iter()
        .map(|link| {
            let kind = match link.role {
                capture_pomodoro_close::LedgerLinkRole::Worked => "worked",
                capture_pomodoro_close::LedgerLinkRole::Mentioned => {
                    "mentioned"
                }
                capture_pomodoro_close::LedgerLinkRole::Deferred => "deferred",
                capture_pomodoro_close::LedgerLinkRole::Dropped => "dropped",
                capture_pomodoro_close::LedgerLinkRole::Struck => "struck",
                capture_pomodoro_close::LedgerLinkRole::Embedded => "embedded",
            };
            PomodoroCloseCarriedJson {
                kind,
                text: link.raw_target.clone(),
            }
        })
        .collect();
    let next_pomodoro =
        ledger
            .next_pomodoro
            .as_ref()
            .map(|next| PomodoroCloseNextJson {
                line: next.line,
                name: next.name.clone(),
                time_range: next.time_range.clone(),
                created: next.created,
            });
    let task_links = plan
        .summary
        .task_links
        .iter()
        .map(|link| PomodoroCloseTaskLinkJson {
            index: link.index,
            ledger_line: link.line,
            block_link: link.block_link.clone(),
            block_id: link.block_id.clone(),
            marker: link.marker.as_str(),
            outcome: link.outcome.as_str(),
            source: link.source.as_str(),
        })
        .collect();
    let summary = PomodoroCloseSummaryJson {
        raw: spec.raw.clone(),
        in_progress: spec.in_progress.clone(),
        complete: spec.complete.clone(),
        drop: spec.drop.clone(),
        log: spec
            .log
            .iter()
            .map(|entry| PomodoroCloseLogEntryJson {
                index: entry.index,
                text: entry.text.clone(),
            })
            .collect(),
        task_links,
        pomodoro_line: running.line,
        pomodoro_name: running.name.clone(),
        day_relative: day_relative.to_string(),
        entry_line,
        planned,
        closed,
        closed_at,
        remaining_minutes,
        decremented_minutes,
        tasks,
        carried,
        notes: ledger.notes.clone(),
        next_pomodoro,
    };
    (summary, extra_warnings)
}

pub(super) fn reject_pomodoro_close_conflicts(
    parsed: &ParsedCaptureText,
    request: &CaptureRequest,
) -> Result<(), CaptureError> {
    if !request.forced_destination_flags.is_empty() {
        return Err(CaptureError::usage(format!(
            "Pomodoro close `=x` cannot be combined with {}; capture the close alone",
            request.forced_destination_flags.join(", ")
        )));
    }
    if request.forced_route.is_some()
        || request.forced_section.is_some()
        || request.forced_sub_bullet_target.is_some()
        || request.forced_task_section.is_some()
    {
        return Err(CaptureError::usage(
            "Pomodoro close `=x` cannot be combined with --route, --section, --task, --task-ref, --task-section, or --clip; capture the close alone",
        ));
    }
    if request.forced_clip.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro close `=x` cannot be combined with --clip; capture the close alone",
        ));
    }
    if parsed.clip.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro close `=x` cannot be combined with % clipboard markers; capture the close alone",
        ));
    }
    if parsed.scheduled_offset.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro close `=x` cannot be combined with s:<N>; capture the close alone",
        ));
    }
    if parsed.priority_level.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro close `=x` cannot be combined with p:<N>; capture the close alone",
        ));
    }
    if !parsed.sub_bullets.is_empty() {
        return Err(CaptureError::usage(
            "`=x` takes no child lines; write Work Log entries on its line (for example `=x 1 wrote the tests`)",
        ));
    }
    Ok(())
}

pub(super) fn stage_close_plan(
    planner: &mut CaptureBatchPlanner,
    plan: &capture_pomodoro_close::PomodoroClosePlan,
) -> Result<(), CaptureError> {
    for (path, contents) in &plan.changed_files {
        planner.stage(path, contents.clone())?;
    }
    Ok(())
}

pub(super) fn final_task_line_for(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    route: &str,
    block_id: &str,
) -> Option<String> {
    let target = bob_dir.join(relative_target(Some(route)));
    let contents = planner.current_contents(&target).ok()??;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    match scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => line_text_at(&contents, task.line_index)
            .ok()
            .map(str::to_string),
        _ => None,
    }
}

/// Convert a parsed `=x[<N>][!<M>][~<K>]` spec into the planner's
/// selection. Plain `=x` yields `None`, so the close runs exactly as before
/// while still reporting the numbered lineup. A spec with lists or typed
/// Work Log entries yields a selection.
fn selection_from_spec(
    spec: &PomodoroCloseSpec,
) -> Option<capture_pomodoro_close::CloseSelection> {
    if !spec.has_selection() && !spec.has_log() {
        return None;
    }
    Some(
        capture_pomodoro_close::CloseSelection::new(
            spec.in_progress
                .clone()
                .map(|numbers| numbers.into_iter().collect()),
            spec.complete.iter().copied().collect(),
            spec.drop.iter().copied().collect(),
            spec.raw.clone(),
        )
        .with_log(spec.log.clone()),
    )
}

pub(super) fn plan_pomodoro_close_item(
    request: &CaptureRequest,
    parsed: ParsedCaptureText,
    spec: PomodoroCloseSpec,
    now: chrono::NaiveDateTime,
    today: chrono::NaiveDate,
    planner: &mut CaptureBatchPlanner,
    warnings: &mut Vec<String>,
) -> Result<PlannedCaptureItem, CaptureError> {
    reject_pomodoro_close_conflicts(&parsed, request)?;
    let selection = selection_from_spec(&spec);
    let day_file = pomodoro::day_file_for(&request.bob_dir);
    let rel = close_day_relative(&request.bob_dir, &day_file);
    if !planner.currently_exists(&day_file)? {
        return Err(CaptureError::io(format!(
            "no running Pomodoro to close: today's daily note `{rel}` does not exist"
        )));
    }
    let day_contents = planner.read_existing(&day_file)?;
    let vault = SnapshotCloseVault::from_planner(planner, &request.bob_dir);
    let plan = capture_pomodoro_close::plan_pomodoro_close(
        &day_file,
        &day_contents,
        now,
        &vault,
        selection.as_ref(),
    )
    .map_err(|error| map_close_plan_error(error, &rel, None))?;
    let (summary, extra_warnings) =
        build_close_summary_json(&spec, &rel, &plan, now);
    warnings.extend(plan.warnings.clone());
    warnings.extend(extra_warnings.clone());
    // Surface close warnings at the top level as well.
    let mut all_warnings = plan.warnings.clone();
    all_warnings.extend(extra_warnings);
    for warning in &all_warnings {
        if !warnings.contains(warning) {
            warnings.push(warning.clone());
        }
    }
    stage_close_plan(planner, &plan)?;
    let post_day = planner.read_existing(&day_file)?;
    let mut pomodoro_refs = vec![PomodoroBlockRef::at(
        PomodoroBlockRole::Closed,
        plan.summary.running.line.saturating_sub(1),
        locate_close_headline(&post_day, &summary),
    )];
    // The next entry is named by the close card, so the preview shows it
    // even when the close left it byte-identical.
    if let Some(next) = summary.next_pomodoro.as_ref() {
        let after = locate_next_headline(&post_day, next);
        pomodoro_refs.push(if next.created {
            PomodoroBlockRef::created(PomodoroBlockRole::Next, after)
        } else {
            PomodoroBlockRef::resolved(PomodoroBlockRole::Next, after)
        });
    }
    let relative_target = day_file
        .strip_prefix(&request.bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| day_file.clone());
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
            task_line: summary.entry_line.clone(),
            kind: capture_kind_label(&parsed.kind),
            created: date_string(today),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Closed,
            sub_bullets: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: None,
            day_file: Some(day_file.display().to_string()),
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
            pomodoro_name: summary.pomodoro_name.clone(),
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
            pomodoro_close: Some(summary),
            toggle_task_description: None,
        },
        clip_plan: None,
        pomodoro_refs,
    })
}

/// Post-state headline index for the closed entry. `summary.pomodoro_line`
/// is pre-image; when the close inserts Work Log lines above
/// `## Pomodoros` (a linked task living in the day file), every section
/// line drifts — and `summary.entry_line` reads the same stale line, so it
/// cannot locate the rewrite. Prefer the candidate when its text still
/// matches, else find the newly completed entry with this name and time
/// range, nearest to the candidate.
fn locate_close_headline(
    post_day: &str,
    summary: &PomodoroCloseSummaryJson,
) -> usize {
    let lines: Vec<&str> = post_day.lines().collect();
    let candidate = summary.pomodoro_line.saturating_sub(1);
    if !summary.entry_line.is_empty()
        && lines
            .get(candidate)
            .is_some_and(|line| *line == summary.entry_line)
    {
        return candidate;
    }
    let scan = capture_pomodoros::scan(post_day);
    let closed_range =
        format!("{}-{}", summary.closed.start, summary.closed.end);
    let mut named = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == capture_pomodoros::PomodoroState::Completed
                && entry.name == summary.pomodoro_name
        })
        .collect::<Vec<_>>();
    named.sort_by_key(|entry| entry.line.saturating_sub(1).abs_diff(candidate));
    if let Some(entry) = named
        .iter()
        .find(|entry| entry.time_range.as_deref() == Some(&closed_range))
        .or_else(|| named.first())
    {
        return entry.line.saturating_sub(1);
    }
    candidate
}

/// Post-state headline index for the next entry, matched by name and time
/// range against the post-state scan (nearest to the ledger-reported line
/// wins), with the ledger line as fallback.
fn locate_next_headline(post_day: &str, next: &PomodoroCloseNextJson) -> usize {
    let fallback = next.line.saturating_sub(1);
    let scan = capture_pomodoros::scan(post_day);
    scan.entries
        .iter()
        .filter(|entry| {
            entry.name == next.name && entry.time_range == next.time_range
        })
        .min_by_key(|entry| entry.line.abs_diff(next.line))
        .map(|entry| entry.line.saturating_sub(1))
        .unwrap_or(fallback)
}

pub(super) fn close_link_hint_for_solo(
    spelling: &str,
    route: &str,
    block_id: &str,
) -> String {
    match spelling {
        "^" => format!("^{route}:{block_id}"),
        _ => format!("@{route}:{block_id}"),
    }
}

pub(super) fn append_link_to_running(
    day_contents: &str,
    running_line: usize,
    block_link: &str,
) -> Result<(String, Placement), CaptureError> {
    let lines = line_spans(day_contents);
    let line_texts = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let entry_index = running_line.saturating_sub(1);
    if entry_index >= line_texts.len() {
        return Err(CaptureError::io(
            "pomodoro close capture invariant failed: running entry is out of range",
        ));
    }
    let range =
        capture_pomodoro_close::sub_bullet_range(&line_texts, entry_index);
    // Child indentation: first direct child's indent, else two spaces.
    let mut indent = "  ".to_string();
    for index in range.clone() {
        let text = line_texts.get(index).copied().unwrap_or("");
        let len = leading_spaces_or_tabs_len(text);
        if len > 0 && len < text.len() {
            indent = text[..len].to_string();
            break;
        }
    }
    let block = format!("- {block_link}");
    let indented = format!("{indent}{block}");
    // Insert after the contiguous sub-bullet range, preserving line endings.
    let spans = line_spans(day_contents);
    let insertion_index = if range.end < spans.len() {
        // Start offset of line range.end (i.e. end offset of range.end - 1).
        if range.end == 0 {
            0
        } else {
            spans[range.end - 1].end
        }
    } else if spans.is_empty() {
        0
    } else {
        spans[spans.len() - 1].end
    };
    let addition = insertion_text_preserving_line_endings(
        day_contents,
        insertion_index,
        &indented,
    );
    let placement = if insertion_index >= day_contents.len() {
        Placement::Appended
    } else {
        Placement::Inserted
    };
    Ok((
        insert_at(day_contents, insertion_index, &addition),
        placement,
    ))
}

/// Outcome of linking an existing task into the running Pomodoro: the
/// updated day contents, the link action, the move source, the destination,
/// and the ledger placement.
pub(super) type RunningLinkOutcome = (
    String,
    &'static str,
    Option<PomodoroLinkEndpoint>,
    PomodoroLinkEndpoint,
    Option<Placement>,
);

#[allow(clippy::too_many_arguments)]
pub(super) fn link_existing_task_into_running(
    _planner: &mut CaptureBatchPlanner,
    _bob_dir: &Path,
    _target: &Path,
    _route: &str,
    _block_id: &str,
    _today: chrono::NaiveDate,
    _warnings: &mut Vec<String>,
    _day_file: &Path,
    day_contents: &str,
    block_link: &str,
) -> Result<RunningLinkOutcome, CaptureError> {
    let scan = capture_pomodoros::scan(day_contents);
    let running = scan
        .entries
        .iter()
        .find(|entry| {
            entry.state == capture_pomodoros::PomodoroState::Open
                && entry.time_range.is_some()
        })
        .ok_or_else(|| {
            CaptureError::io(
                "pomodoro close capture invariant failed: running entry disappeared",
            )
        })?;
    let running_line = running.line;
    let running_index = running_line.saturating_sub(1);
    let lines = line_spans(day_contents);
    let line_texts = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let section =
        pomodoro::pomodoros_section_range(&line_texts).ok_or_else(|| {
            CaptureError::io("Bob daily note has no Pomodoros section")
        })?;
    let movable = capture_task_toggle::find_movable_task_links(
        &lines, &scan, section, block_link,
    );
    if movable.len() > 1 {
        return Err(CaptureError::io(format!(
            "found more than one movable open Pomodoro Task Link for {block_link}; make the dedicated Task Link unique before capturing"
        )));
    }
    if let Some(source) = movable.first() {
        if source.owner.line == running_line {
            let dest = PomodoroLinkEndpoint {
                line: running.line,
                name: running.name.clone(),
                time_range: running.time_range.clone(),
                role: Some("current"),
            };
            return Ok((
                day_contents.to_string(),
                "already_current",
                None,
                dest,
                None,
            ));
        }
        let source_endpoint = PomodoroLinkEndpoint {
            line: source.owner.line,
            name: source.owner.name.clone(),
            time_range: source.owner.time_range.clone(),
            role: None,
        };
        let (moved_day, placement) =
            capture_task_toggle::move_subtree_to_entry(
                day_contents,
                source.line_index,
                source.subtree_end,
                running_index,
            )
            .map_err(|_| {
                CaptureError::io(
                    "pomodoro close capture invariant failed: queued link could not be moved",
                )
            })?;
        let dest = PomodoroLinkEndpoint {
            line: running.line,
            name: running.name.clone(),
            time_range: running.time_range.clone(),
            role: Some("current"),
        };
        let placement = Some(match placement {
            capture_task_toggle::LinkPlacement::Inserted => Placement::Inserted,
            capture_task_toggle::LinkPlacement::Appended => Placement::Appended,
        });
        return Ok((
            moved_day,
            "moved",
            Some(source_endpoint),
            dest,
            placement,
        ));
    }
    let (updated_day, placement) =
        append_link_to_running(day_contents, running_line, block_link)?;
    let dest = PomodoroLinkEndpoint {
        line: running.line,
        name: running.name.clone(),
        time_range: running.time_range.clone(),
        role: Some("current"),
    };
    Ok((updated_day, "linked", None, dest, Some(placement)))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_pomodoro_close_link_item(
    request: &CaptureRequest,
    parsed: ParsedCaptureText,
    now: chrono::NaiveDateTime,
    today: chrono::NaiveDate,
    planner: &mut CaptureBatchPlanner,
    warnings: &mut Vec<String>,
    block_id: &str,
    close_spec: PomodoroCloseSpec,
) -> Result<PlannedCaptureItem, CaptureError> {
    reject_pomodoro_link_conflicts(&parsed, request)?;
    let selection = selection_from_spec(&close_spec);
    let route = parsed.route.clone().ok_or_else(|| {
        CaptureError::io(
            "pomodoro link capture invariant failed: route is missing",
        )
    })?;
    let spelling_hint = match &parsed.kind {
        CaptureKind::PomodoroLink { spelling, .. } => match spelling {
            capture_language::PomodoroLinkSpelling::Caret => "^",
            capture_language::PomodoroLinkSpelling::At => "@",
        },
        _ => "@",
    };
    let link_hint = close_link_hint_for_solo(spelling_hint, &route, block_id);
    let created = date_string(today);
    let rel_target = relative_target(Some(&route));
    let target = request.bob_dir.join(&rel_target);
    // Resolve and gate exactly like the solo link.
    let contents = planner.read_existing(&target).map_err(|_| {
        CaptureError::io(format!(
            "no task with block ID ^{block_id} in {route}.md (run 'bob capture-tasks -r {route}' to list task block IDs)"
        ))
    })?;
    let settings = note_tasks::read_settings(&request.bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let task = match scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => task.clone(),
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
            let choices = format!(
                "run 'bob capture-tasks -r {route}' to list task block IDs"
            );
            let hint = format!(
                "to create a new Pomodoro-linked task, add text: `@{route}:{block_id} <text>`"
            );
            let message = match scan.suggest_block_id(block_id) {
                Some(suggestion) => format!(
                    "no task with block ID ^{block_id} in {route}.md; did you mean ^{suggestion}? ({choices}; {hint})"
                ),
                None => format!(
                    "no task with block ID ^{block_id} in {route}.md ({choices}; {hint})"
                ),
            };
            return Err(CaptureError::io(message));
        }
    };
    let previous_status_symbol = task.status_symbol;
    let previous_status_name = task.status_name.clone();
    let task_description = task.description.clone();
    let previous_task_line =
        line_text_at(&contents, task.line_index)?.to_string();
    match previous_status_symbol {
        ' ' | '?' | '*' | '/' => {}
        _ => {
            return Err(CaptureError::io(format!(
                "task ^{block_id} is {previous_status_name}; only Ready, Blocked, Next, and In Progress tasks can be linked to a Pomodoro"
            )));
        }
    }
    let task_plan =
        capture_task_toggle::plan_task_link(&contents, task.line_index, today)
            .ok_or_else(|| {
                CaptureError::io(
                    "pomodoro link capture invariant failed: task line could not be updated",
                )
            })?;
    let staged_task_contents = task_plan.content.clone();
    let updated_scan = note_tasks::scan(&staged_task_contents, &settings);
    let updated_task = match updated_scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => task.clone(),
        _ => {
            return Err(CaptureError::io(
                "pomodoro link capture invariant failed: linked task disappeared",
            ));
        }
    };
    if previous_status_symbol != updated_task.status_symbol
        && task_line_declares_dependencies(&previous_task_line)
    {
        warnings.push(format!(
            "^{block_id} still declares dependencies; bob task-status-hooks may return it to Blocked"
        ));
    }
    if staged_task_contents != contents {
        planner.stage(&target, staged_task_contents.clone())?;
    }
    let day_file = pomodoro::day_file_for(&request.bob_dir);
    let rel = close_day_relative(&request.bob_dir, &day_file);
    if paths_refer_to_same_file(&target, &day_file) {
        return Err(CaptureError::io(
            "routed note and Bob daily note must be different files",
        ));
    }
    if !planner.currently_exists(&day_file)? {
        return Err(CaptureError::io(format!(
            "no running Pomodoro to close: today's daily note `{rel}` does not exist"
        )));
    }
    let mut day_contents = planner.read_existing(&day_file)?;
    // Surface section, none-running, and multiple-entry diagnostics from
    // pre-image lines before the link step shifts them.
    if let Some(error) = pre_link_running_error(&day_contents, &rel, &link_hint)
    {
        return Err(error);
    }
    let block_link = format!("[[{route}#^{block_id}]]");
    let day_file_label = day_file.display().to_string();
    // Link into _R_ (destination always the running entry).
    let (linked_day, action, source, mut destination, link_placement) =
        link_existing_task_into_running(
            planner,
            &request.bob_dir,
            &target,
            &route,
            block_id,
            today,
            warnings,
            &day_file,
            &day_contents,
            &block_link,
        )
        .map_err(|error| {
            // Attach the start-hint to the none-running case, keeping the
            // next-placeholder tail from the pre-link contents.
            let message = error.message.clone();
            if message.contains("has no open timed entry")
                || message.contains("running entry disappeared")
            {
                let (next_name, next_line) =
                    close_next_from_contents(&day_contents);
                close_no_running_error(
                    &rel,
                    next_name.as_deref(),
                    next_line,
                    Some(&link_hint),
                )
            } else {
                error
            }
        })?;
    // The running entry must exist; surface the full none-running diagnostic.
    if linked_day.is_empty() {
        let (next_name, next_line) = close_next_from_contents(&day_contents);
        return Err(close_no_running_error(
            &rel,
            next_name.as_deref(),
            next_line,
            Some(&link_hint),
        ));
    }
    planner.stage(&day_file, linked_day.clone())?;
    day_contents = linked_day;
    // Close _R_ on the staged ledger.
    let vault = SnapshotCloseVault::from_planner(planner, &request.bob_dir);
    let plan = capture_pomodoro_close::plan_pomodoro_close(
        &day_file,
        &day_contents,
        now,
        &vault,
        selection.as_ref(),
    )
    .map_err(|error| map_close_plan_error(error, &rel, Some(&link_hint)))?;
    let (summary, extra_warnings) =
        build_close_summary_json(&close_spec, &rel, &plan, now);
    for warning in plan.warnings.iter().chain(extra_warnings.iter()) {
        if !warnings.contains(warning) {
            warnings.push(warning.clone());
        }
    }
    stage_close_plan(planner, &plan)?;
    // Block refs: the closed entry was the pre-link running entry
    // (`destination` still carries its pre-link line); a moved source
    // resolves through the item's line map; the next entry is named by
    // the close card, so it shows even when byte-identical.
    let post_day = planner.read_existing(&day_file)?;
    let mut pomodoro_refs = vec![PomodoroBlockRef::at(
        PomodoroBlockRole::Closed,
        destination.line.saturating_sub(1),
        locate_close_headline(&post_day, &summary),
    )];
    if let Some(moved) = source.as_ref() {
        pomodoro_refs.push(PomodoroBlockRef::unlinked_before(
            moved.line.saturating_sub(1),
        ));
    }
    if let Some(next) = summary.next_pomodoro.as_ref() {
        let after = locate_next_headline(&post_day, next);
        pomodoro_refs.push(if next.created {
            PomodoroBlockRef::created(PomodoroBlockRole::Next, after)
        } else {
            PomodoroBlockRef::resolved(PomodoroBlockRole::Next, after)
        });
    }
    // Final post-image for the linked task (close starts it to [/]).
    let final_task_line =
        final_task_line_for(planner, &request.bob_dir, &route, block_id)
            .unwrap_or_else(|| {
                staged_task_contents
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string()
            });
    let (final_status_symbol, final_status_name) = {
        let target_path = request.bob_dir.join(rel_target.clone());
        let contents = planner
            .current_contents(&target_path)
            .ok()
            .flatten()
            .unwrap_or_default();
        let scan = note_tasks::scan(&contents, &settings);
        match scan.by_block_id(block_id) {
            BlockIdLookup::Found(task) => {
                (task.status_symbol, task.status_name.clone())
            }
            _ => (updated_task.status_symbol, updated_task.status_name.clone()),
        }
    };
    destination.time_range =
        Some(format!("{}-{}", summary.closed.start, summary.closed.end));
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            ok: true,
            dry_run: request.dry_run,
            routed: true,
            route: Some(route.clone()),
            route_label: route_label(&route),
            relative_target: rel_target.to_string_lossy().into_owned(),
            target: target.display().to_string(),
            text: String::new(),
            task_line: final_task_line,
            kind: capture_kind_label(&parsed.kind),
            created,
            scheduled: None,
            priority: None,
            priority_label: None,
            // Link forms stay strictly additive: keep the non-close
            // counterpart's placement. Only whole-item `=x` is "closed".
            placement: Placement::Linked,
            sub_bullets: Vec::new(),
            clip: None,
            schedule_log: task_plan.schedule_log.clone(),
            block_id: Some(block_id.to_string()),
            day_file: Some(day_file_label),
            block_link: Some(block_link),
            pomodoro_link_placement: link_placement,
            parent_line: None,
            parent_text: None,
            parent_section: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: Some(previous_task_line),
            status_symbol: Some(final_status_symbol),
            status_name: Some(final_status_name),
            previous_status_symbol: Some(previous_status_symbol),
            previous_status_name: Some(previous_status_name),
            pomodoro_name: destination.name.clone(),
            creates_pomodoro: Some(false),
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: task_plan.removed_scheduled.clone(),
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: Some(previous_status_symbol != final_status_symbol),
            pomodoro_link_action: Some(action),
            pomodoro_link_source: source,
            pomodoro_link_destination: Some(destination),
            project_note: None,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: Some(summary),
            toggle_task_description: Some(task_description),
        },
        clip_plan: None,
        pomodoro_refs,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_pomodoro_close_task_item(
    request: &CaptureRequest,
    parsed: ParsedCaptureText,
    now: chrono::NaiveDateTime,
    today: chrono::NaiveDate,
    planner: &mut CaptureBatchPlanner,
    warnings: &mut Vec<String>,
    route: &str,
    block_id: &str,
    close_spec: &PomodoroCloseSpec,
    capture_block: &str,
) -> Result<PlannedCaptureItem, CaptureError> {
    let selection = selection_from_spec(close_spec);
    // Body-bearing `<text> @route:block-id=x`: today's `:` new-task capture
    // forced into _R_, then close. This helper stages the new task and the
    // link into _R_, then returns a write plan; the caller closes afterwards.
    // To keep the planner atomic, we stage here and let the caller run the
    // close over the staged snapshot.
    let target = request.bob_dir.join(relative_target(Some(route)));
    let day_file = pomodoro::day_file_for(&request.bob_dir);
    let rel = close_day_relative(&request.bob_dir, &day_file);
    // Name the item's own spelling in the start-hint, not the solo form
    // that would fail with a missing-ID error.
    let body_text = parsed.body.trim();
    let link_hint = if body_text.is_empty() {
        format!("@{route}:{block_id}")
    } else {
        format!("{body_text} @{route}:{block_id}")
    };
    if paths_refer_to_same_file(&target, &day_file) {
        return Err(CaptureError::io(
            "routed note and Bob daily note must be different files",
        ));
    }
    if !planner.currently_exists(&day_file)? {
        return Err(CaptureError::io(format!(
            "no running Pomodoro to close: today's daily note `{rel}` does not exist"
        )));
    }
    let target_existed = planner.currently_exists(&target)?;
    let original_target =
        planner.current_contents(&target)?.unwrap_or_default();
    reject_duplicate_block_id(&original_target, block_id, &target)?;
    let (updated_target, placement) = if target_existed {
        insert_task_line(&original_target, capture_block)
    } else {
        validate_target_parent(&target)?;
        (format!("{capture_block}\n"), Placement::Created)
    };
    planner.stage(&target, updated_target)?;
    let day_contents = planner.read_existing(&day_file)?;
    // Surface section, none-running, and multiple-entry diagnostics from
    // pre-image lines before the link step shifts them.
    if let Some(error) = pre_link_running_error(&day_contents, &rel, &link_hint)
    {
        return Err(error);
    }
    let block_link = format!("[[{route}#^{block_id}]]");
    let (linked_day, _action, link_source, running_dest, _placement) =
        link_existing_task_into_running(
            planner,
            &request.bob_dir,
            &target,
            route,
            block_id,
            today,
            warnings,
            &day_file,
            &day_contents,
            &block_link,
        )
        .map_err(|error| {
            let message = error.message.clone();
            if message.contains("has no open timed entry")
                || message.contains("running entry disappeared")
            {
                let (next_name, next_line) =
                    close_next_from_contents(&day_contents);
                close_no_running_error(
                    &rel,
                    next_name.as_deref(),
                    next_line,
                    Some(&link_hint),
                )
            } else {
                error
            }
        })?;
    planner.stage(&day_file, linked_day.clone())?;
    let vault = SnapshotCloseVault::from_planner(planner, &request.bob_dir);
    let plan = capture_pomodoro_close::plan_pomodoro_close(
        &day_file,
        &linked_day,
        now,
        &vault,
        selection.as_ref(),
    )
    .map_err(|error| map_close_plan_error(error, &rel, Some(&link_hint)))?;
    let (summary, extra_warnings) =
        build_close_summary_json(close_spec, &rel, &plan, now);
    for warning in plan.warnings.iter().chain(extra_warnings.iter()) {
        if !warnings.contains(warning) {
            warnings.push(warning.clone());
        }
    }
    stage_close_plan(planner, &plan)?;
    // Block refs: the closed entry was the pre-link running entry; a
    // moved source resolves through the item's line map; the next entry
    // is named by the close card, so it shows even when byte-identical.
    let post_day = planner.read_existing(&day_file)?;
    let mut pomodoro_refs = vec![PomodoroBlockRef::at(
        PomodoroBlockRole::Closed,
        running_dest.line.saturating_sub(1),
        locate_close_headline(&post_day, &summary),
    )];
    if let Some(moved) = link_source.as_ref() {
        pomodoro_refs.push(PomodoroBlockRef::unlinked_before(
            moved.line.saturating_sub(1),
        ));
    }
    if let Some(next) = summary.next_pomodoro.as_ref() {
        let after = locate_next_headline(&post_day, next);
        pomodoro_refs.push(if next.created {
            PomodoroBlockRef::created(PomodoroBlockRole::Next, after)
        } else {
            PomodoroBlockRef::resolved(PomodoroBlockRole::Next, after)
        });
    }
    let final_task_line =
        final_task_line_for(planner, &request.bob_dir, route, block_id)
            .unwrap_or_else(|| {
                capture_block.lines().next().unwrap_or("").to_string()
            });
    let (final_status_symbol, final_status_name) = {
        let target_path = request.bob_dir.join(relative_target(Some(route)));
        let contents = planner
            .current_contents(&target_path)
            .ok()
            .flatten()
            .unwrap_or_default();
        let settings = note_tasks::read_settings(&request.bob_dir);
        let scan = note_tasks::scan(&contents, &settings);
        match scan.by_block_id(block_id) {
            BlockIdLookup::Found(task) => {
                (Some(task.status_symbol), Some(task.status_name.clone()))
            }
            _ => (None, None),
        }
    };
    let day_file_label = day_file.display().to_string();
    let block_link = format!("[[{route}#^{block_id}]]");
    let destination = summary
        .next_pomodoro
        .as_ref()
        .map(|_| PomodoroLinkEndpoint {
            line: summary.pomodoro_line,
            name: summary.pomodoro_name.clone(),
            time_range: Some(format!(
                "{}-{}",
                summary.closed.start, summary.closed.end
            )),
            role: Some("current"),
        })
        .unwrap_or(PomodoroLinkEndpoint {
            line: summary.pomodoro_line,
            name: summary.pomodoro_name.clone(),
            time_range: Some(format!(
                "{}-{}",
                summary.closed.start, summary.closed.end
            )),
            role: Some("current"),
        });
    let relative_target_path = relative_target(Some(route));
    let target_path = request.bob_dir.join(&relative_target_path);
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            ok: true,
            dry_run: request.dry_run,
            routed: true,
            route: Some(route.to_string()),
            route_label: route_label(route),
            relative_target: relative_target_path
                .to_string_lossy()
                .into_owned(),
            target: target_path.display().to_string(),
            text: parsed.body.clone(),
            task_line: final_task_line,
            kind: capture_kind_label(&parsed.kind),
            created: date_string(today),
            scheduled: None,
            priority: None,
            priority_label: None,
            // Body-bearing closes keep the new-task placement, like the
            // non-close counterpart. Only whole-item `=x` is "closed".
            placement,
            sub_bullets: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: Some(block_id.to_string()),
            day_file: Some(day_file_label),
            block_link: Some(block_link),
            pomodoro_link_placement: Some(placement),
            parent_line: None,
            parent_text: None,
            parent_section: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: None,
            status_symbol: final_status_symbol,
            status_name: final_status_name,
            previous_status_symbol: Some(' '),
            previous_status_name: Some("Ready".to_string()),
            pomodoro_name: summary.pomodoro_name.clone(),
            creates_pomodoro: None,
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: Some(true),
            pomodoro_link_action: Some("linked"),
            pomodoro_link_source: None,
            pomodoro_link_destination: Some(destination),
            project_note: None,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: Some(summary),
            toggle_task_description: None,
        },
        clip_plan: None,
        pomodoro_refs,
    })
}
