//! Vault scan for `bob freshness`: rich task rows, Today
//! membership, per-note `task_refresh`, file contents for the seed's
//! concurrent-change guard, and warning collection.
//!
//! The contract lives in `docs/freshness.md`. Row evaluation itself is
//! [`super::state`]; this module only gathers its inputs.

use std::{
    collections::{BTreeSet, HashMap},
    fs, io,
    path::Path,
};

use chrono::NaiveDate;

use super::super::{
    config::{self, ConfigError, FreshnessConfig},
    dataview::{self, DataviewError, RichTask, TaskFormat},
    env as bob_env,
    plan_budget::today::today_tasks,
    pomodoro,
    projects::{frontmatter_value, parse_frontmatter},
    task_status_hooks::canonical_daily_date,
};
use super::{
    placement::read_freshness,
    state::{evaluate, FreshnessRow},
};

/// Today membership plus warnings for one daily note.
type TodayScan = (
    BTreeSet<(String, String)>,
    BTreeSet<(String, u32)>,
    Vec<Warning>,
);

/// A scanned task row plus the scope inputs the evaluator needs.
#[derive(Debug, Clone)]
pub(crate) struct RowCtx {
    pub(crate) task: RichTask,
    pub(crate) note_refresh_raw: Option<String>,
    pub(crate) is_daily_note: bool,
    pub(crate) is_today: bool,
    /// Own-note open-task count for `^prj` rows; 0 otherwise.
    pub(crate) project_open_count: u32,
    /// Valid frontmatter `scheduled` on the tracker's own note.
    pub(crate) project_scheduled: Option<NaiveDate>,
    /// The own note carries a malformed `scheduled`.
    pub(crate) project_schedule_invalid: bool,
}

impl RowCtx {
    /// Build the evaluator input. `lane_visible` is the NEXT/PENDING
    /// lane predicate: rows from [`dataview::READY_QUERY`] already
    /// carry it from the engine, so callers pass `true` there. For
    /// exact `^prj`/`^ref` trackers callers pass the
    /// freshness-specific visibility (hide allowed); every other
    /// exclusion still applies.
    pub(crate) fn freshness_row(&self, lane_visible: bool) -> FreshnessRow {
        FreshnessRow {
            path: self.task.path.clone(),
            line: self.task.line,
            status: self.task.status_symbol.chars().next().unwrap_or(' '),
            is_todo: self.task.status_type == "TODO",
            is_open: super::state::is_open_status_type(&self.task.status_type),
            recurring: self.task.is_recurring,
            lane_visible,
            is_daily_note: self.is_daily_note,
            is_today: self.is_today,
            scheduled: self.task.scheduled,
            created: self.task.created,
            raw_line: self.task.original_markdown.clone(),
            note_refresh_raw: self.note_refresh_raw.clone(),
            tracker: super::state::TrackerKind::from_block_id(
                self.task.block_id.as_deref(),
            ),
            project_open_count: self.project_open_count,
            project_scheduled: self.project_scheduled,
            project_schedule_invalid: self.project_schedule_invalid,
        }
    }
}

/// One lint or Today warning for human and JSON output.
#[derive(Debug, Clone)]
pub(crate) struct Warning {
    pub(crate) code: String,
    pub(crate) path: String,
    pub(crate) line: Option<u32>,
    pub(crate) message: String,
}

/// Everything `list` and `seed` need from the vault.
pub(crate) struct Snapshot {
    pub(crate) today: NaiveDate,
    pub(crate) weekday: String,
    pub(crate) config: FreshnessConfig,
    /// Rows matching [`dataview::READY_QUERY`]; the engine already
    /// applied the lane predicate.
    pub(crate) ready: Vec<RowCtx>,
    /// Rows matching [`dataview::PENDING_QUERY`]: the `[/]` lane,
    /// lane-visible by construction.
    pub(crate) pending: Vec<RowCtx>,
    /// Rows matching [`dataview::NEXT_QUERY`]: the `[*]` lane,
    /// lane-visible by construction.
    pub(crate) next: Vec<RowCtx>,
    /// Hidden exact `^prj`/`^ref` candidates from the all-task scan:
    /// freshness-specific visibility (hide allowed) with every other
    /// exclusion still applied, excluding rows already in a lane
    /// query. Ordinary hidden tasks never land here.
    pub(crate) trackers: Vec<RowCtx>,
    /// Rows matching [`dataview::OPEN_QUERY`]: the seed universe.
    pub(crate) open: Vec<RowCtx>,
    /// Every task of any status: `refreshed_today`, `upkeep_today`,
    /// and warnings.
    pub(crate) all: Vec<RowCtx>,
    /// `today_link_unresolved` passthrough from the Today engine.
    pub(crate) today_warnings: Vec<Warning>,
    /// Scan-time file contents for the seed's concurrent-change guard.
    pub(crate) file_contents: HashMap<String, String>,
}

/// Scan failures: I/O-like errors exit 1, usage errors (invalid
/// config, unsupported task format) exit 2.
pub(crate) enum ScanError {
    Io(String),
    Usage(String),
}

impl ScanError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Io(message) | Self::Usage(message) => message,
        }
    }

    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            Self::Io(_) => 1,
            Self::Usage(_) => 2,
        }
    }
}

/// Scan the vault for a freshness review or seed.
pub(crate) fn scan(bob_dir: &Path) -> Result<Snapshot, ScanError> {
    let config = config::load_freshness_config(&config::config_path())
        .map_err(|error| match error {
            ConfigError::Read(message) => ScanError::Io(message),
            ConfigError::Invalid(message) => ScanError::Usage(message),
        })?;

    let format = dataview::read_task_format(bob_dir)
        .map_err(|error| ScanError::Io(dataview_message(&error)))?;
    if format != TaskFormat::Dataview {
        return Err(ScanError::Usage(
            "bob freshness needs the vault's Tasks task format to be \
            Dataview (docs/freshness.md rule 1); refusing to run"
                .to_string(),
        ));
    }

    let now = bob_env::current_datetime();
    let today = now.date();
    let weekday = today.format("%a").to_string();

    let ready_tasks =
        dataview::query_rich_tasks(bob_dir, dataview::READY_QUERY, now)
            .map_err(|error| ScanError::Io(dataview_message(&error)))?;
    let pending_tasks =
        dataview::query_rich_tasks(bob_dir, dataview::PENDING_QUERY, now)
            .map_err(|error| ScanError::Io(dataview_message(&error)))?;
    let next_tasks =
        dataview::query_rich_tasks(bob_dir, dataview::NEXT_QUERY, now)
            .map_err(|error| ScanError::Io(dataview_message(&error)))?;
    let open_tasks =
        dataview::query_rich_tasks(bob_dir, dataview::OPEN_QUERY, now)
            .map_err(|error| ScanError::Io(dataview_message(&error)))?;
    let all_tasks = dataview::scan_all_rich_tasks(bob_dir, now)
        .map_err(|error| ScanError::Io(dataview_message(&error)))?;

    let day_file = pomodoro::day_file_for(bob_dir);
    let daily_relative = relative_path(&day_file, bob_dir);
    let (today_blocks, today_lines, today_warnings) =
        read_today(bob_dir, &day_file, &daily_relative)?;

    let mut notes: HashMap<String, Option<String>> = HashMap::new();
    let mut file_contents: HashMap<String, String> = HashMap::new();
    let mut context = |task: &RichTask| -> Result<RowCtx, ScanError> {
        let note_refresh_raw = match notes.get(&task.path) {
            Some(cached) => cached.clone(),
            None => {
                let value = read_note_refresh(bob_dir, &task.path)?;
                notes.insert(task.path.clone(), value.clone());
                value
            }
        };
        if !file_contents.contains_key(&task.path) {
            let contents = fs::read_to_string(bob_dir.join(&task.path))
                .map_err(|error| {
                    ScanError::Io(format!(
                        "read {}: {error}",
                        bob_dir.join(&task.path).display()
                    ))
                })?;
            file_contents.insert(task.path.clone(), contents);
        }
        Ok(RowCtx {
            task: task.clone(),
            note_refresh_raw,
            is_daily_note: canonical_daily_date(Path::new(&task.path))
                .is_some(),
            is_today: is_today_task(task, &today_blocks, &today_lines),
            project_open_count: 0,
            project_scheduled: None,
            project_schedule_invalid: false,
        })
    };

    let mut ready = Vec::with_capacity(ready_tasks.len());
    for task in &ready_tasks {
        ready.push(context(task)?);
    }
    let mut pending = Vec::with_capacity(pending_tasks.len());
    for task in &pending_tasks {
        pending.push(context(task)?);
    }
    let mut next = Vec::with_capacity(next_tasks.len());
    for task in &next_tasks {
        next.push(context(task)?);
    }
    let mut open = Vec::with_capacity(open_tasks.len());
    for task in &open_tasks {
        open.push(context(task)?);
    }
    let mut all = Vec::with_capacity(all_tasks.len());
    for task in &all_tasks {
        // Files behind done tasks are only needed for warnings, which
        // read the scanned lines, not the files; still record them so
        // the map covers the vault uniformly.
        all.push(context(task)?);
    }

    // Per-path open-task totals, built once from the unfiltered
    // task inventory (`all`): every open status by Tasks status type,
    // excluding every exact `^prj` row. Includes hidden, recurring,
    // future-scheduled, Today-linked, fresh, NEW, RETURNED, and
    // ROTTEN tasks; an open `^ref` counts. Never rolls up children,
    // embeds, backlinks, or parents. One pass, then O(1) lookups.
    let mut open_counts: HashMap<String, u32> = HashMap::new();
    for row in &all {
        if !super::state::is_open_status_type(&row.task.status_type) {
            continue;
        }
        if super::state::is_open_project_row(true, row.task.block_id.as_deref())
        {
            *open_counts.entry(row.task.path.clone()).or_default() += 1;
        }
    }
    // Own-note frontmatter schedule context, cached once per file.
    let mut schedule_cache: HashMap<String, (Option<NaiveDate>, bool)> =
        HashMap::new();
    let mut schedule_for = |path: &str| -> (Option<NaiveDate>, bool) {
        if let Some(cached) = schedule_cache.get(path) {
            return *cached;
        }
        let contents = file_contents.get(path).cloned().unwrap_or_default();
        let parsed = parse_project_scheduled(&contents);
        schedule_cache.insert(path.to_string(), parsed);
        parsed
    };
    let mut fill_project_context = |row: &mut RowCtx| {
        let is_prj = row.task.block_id.as_deref() == Some("prj");
        if !is_prj {
            row.project_open_count = 0;
            row.project_scheduled = None;
            row.project_schedule_invalid = false;
            return;
        }
        row.project_open_count =
            open_counts.get(&row.task.path).copied().unwrap_or(0);
        let (scheduled, invalid) = schedule_for(&row.task.path);
        row.project_scheduled = scheduled;
        row.project_schedule_invalid = invalid;
    };
    for row in &mut ready {
        fill_project_context(row);
    }
    for row in &mut pending {
        fill_project_context(row);
    }
    for row in &mut next {
        fill_project_context(row);
    }
    for row in &mut all {
        fill_project_context(row);
    }

    // Hidden tracker candidates: exact `^prj`/`^ref` rows from the
    // all-task scan that pass every scope exclusion except `#hide`,
    // and are not already in a lane query. Ordinary hidden tasks stay
    // out. Deduplicate by (path, line) when combining.
    let mut lane_keys: std::collections::BTreeSet<(String, u32)> =
        std::collections::BTreeSet::new();
    for row in ready.iter().chain(pending.iter()).chain(next.iter()) {
        lane_keys.insert((row.task.path.clone(), row.task.line));
    }
    let mut trackers = Vec::new();
    for row in &all {
        let block_id = row.task.block_id.as_deref();
        if block_id != Some("prj") && block_id != Some("ref") {
            continue;
        }
        let key = (row.task.path.clone(), row.task.line);
        if lane_keys.contains(&key) {
            continue;
        }
        if tracker_candidate_visible(row, today) {
            trackers.push(row.clone());
        }
    }

    Ok(Snapshot {
        today,
        weekday,
        config,
        ready,
        pending,
        next,
        trackers,
        open,
        all,
        today_warnings,
        file_contents,
    })
}

/// Count `refreshed_today` over every task: tasks of any status,
/// outside `_templates` / `_conflicts`, whose `fresh` equals today.
/// Ready-only rows would miss Next, Pending, and done tasks (S15).
pub(crate) fn refreshed_today(rows: &[RowCtx], today: NaiveDate) -> u32 {
    let mut count = 0;
    for row in rows {
        if super::state::is_excluded_count_path(&row.task.path) {
            continue;
        }
        if read_freshness(&row.task.original_markdown, today).fresh
            == Some(today)
        {
            count += 1;
        }
    }
    count
}

/// Count `upkeep_today` over every task: `refreshed_today` tasks
/// whose status symbol is neither `/` nor `*`. Lane stamps are daily
/// review progress, not upkeep.
pub(crate) fn upkeep_today(rows: &[RowCtx], today: NaiveDate) -> u32 {
    let mut count = 0;
    for row in rows {
        if super::state::is_excluded_count_path(&row.task.path) {
            continue;
        }
        if read_freshness(&row.task.original_markdown, today).fresh
            != Some(today)
        {
            continue;
        }
        let symbol = row.task.status_symbol.chars().next().unwrap_or(' ');
        if symbol == '/' || symbol == '*' {
            continue;
        }
        count += 1;
    }
    count
}

/// Freshness-specific visibility for an exact `^prj`/`^ref`
/// candidate from the all-task scan: every scope exclusion still
/// applies except the conventional `#hide` tag. Ordinary hidden tasks
/// never reach here (callers filter by exact tracker identity first).
fn tracker_candidate_visible(row: &RowCtx, today: NaiveDate) -> bool {
    use super::state::lane_for_row;
    let status = row.task.status_symbol.chars().next().unwrap_or(' ');
    let is_todo = row.task.status_type == "TODO";
    // Unsupported/closed status, dependency blocking, and recurring
    // tasks stay out.
    if lane_for_row(status, is_todo).is_none() {
        return false;
    }
    if row.task.is_blocked || row.task.is_recurring {
        return false;
    }
    if row.is_daily_note || row.is_today {
        return false;
    }
    // Template/conflict paths stay out (mirror the lane queries).
    if row.task.path.contains("_templates")
        || row.task.path.contains("_conflicts")
    {
        return false;
    }
    // Future inline scheduling suppresses review; the project
    // frontmatter gate is applied by the evaluator.
    if row.task.scheduled.is_some_and(|date| date > today) {
        return false;
    }
    true
}

/// Parse an own-note frontmatter `scheduled` for `^prj` review gating,
/// reusing the project schedule shape (exact `YYYY-MM-DD`). Returns
/// `(date, invalid)`: `invalid` is true when the note carries a
/// `scheduled` key that is missing, duplicated, or malformed, which
/// suppresses the tracker until fixed.
fn parse_project_scheduled(contents: &str) -> (Option<NaiveDate>, bool) {
    // Local frontmatter scan (mirrors the project schedule shape):
    // lines between the opening `---` and the closing `---`.
    let mut lines = contents.lines();
    let first = lines.next().unwrap_or("");
    if first.trim_end_matches('\r') != "---" {
        return (None, false);
    }
    let mut fm_lines = Vec::new();
    for line in lines {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line == "---" {
            break;
        }
        fm_lines.push(line);
    }
    // If we never saw a closing fence, there is no frontmatter.
    // (A body `---` later still ends the scan above; an absent fence
    // leaves every line collected, but `scheduled` keys in the body
    // are indented task fields, not `scheduled:` at column 0 —
    // still, require the fence by re-scanning strictly.)
    let has_fence = contents
        .lines()
        .skip(1)
        .any(|line| line.strip_suffix('\r').unwrap_or(line) == "---");
    if !has_fence {
        return (None, false);
    }
    let fields: Vec<&str> = fm_lines
        .iter()
        .filter_map(|line| {
            let rest = line.strip_prefix("scheduled")?;
            rest.strip_prefix(':')
        })
        .collect();
    if fields.is_empty() {
        return (None, false);
    }
    if fields.len() > 1 {
        return (None, true);
    }
    let raw = trim_yaml_scalar(fields[0]);
    if !is_exact_project_date_shape(raw) {
        return (None, true);
    }
    match NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        Ok(date) => (Some(date), false),
        Err(_) => (None, true),
    }
}

fn trim_yaml_scalar(raw: &str) -> &str {
    let trimmed = raw.trim().trim_matches(['"', '\'']).trim();
    // Strip a trailing YAML comment.
    match trimmed.find(" #") {
        Some(index) => trimmed[..index].trim_end(),
        None => trimmed,
    }
}

fn is_exact_project_date_shape(value: &str) -> bool {
    value.len() == 10
        && value.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

/// Bucket and confirmation date for one Ready-lane row, sharing
/// the single `RowCtx::freshness_row(true)` construction so
/// `note_ready` never duplicates it.
pub(crate) fn row_bucket(
    row: &RowCtx,
    today: NaiveDate,
    config: &FreshnessConfig,
) -> (Option<&'static str>, Option<NaiveDate>) {
    use super::state::bucket_for_state;
    let evaluated = evaluate(&row.freshness_row(true), today, config);
    (bucket_for_state(evaluated.state), evaluated.fresh)
}

/// Collect one warning per linted row over every task, in scan order.
pub(crate) fn collect_warnings(
    rows: &[RowCtx],
    today: NaiveDate,
    config: &FreshnessConfig,
) -> Vec<Warning> {
    let mut warnings = Vec::new();
    for row in rows {
        // The lane predicate is irrelevant to lints: evaluation
        // returns the line and note lints whatever the scope.
        let evaluated = evaluate(&row.freshness_row(true), today, config);
        for lint in &evaluated.lints {
            warnings.push(Warning {
                code: lint.clone(),
                path: row.task.path.clone(),
                line: Some(row.task.line),
                message: lint_message(lint),
            });
        }
    }
    warnings
}

/// Human text for each lint code in `docs/freshness.md`.
pub(crate) fn lint_message(code: &str) -> String {
    match code {
        "fresh_malformed" => {
            "fresh date is not a valid YYYY-MM-DD date; ignored".to_string()
        }
        "fresh_future" => {
            "fresh date is in the future; treated as never confirmed"
                .to_string()
        }
        "fresh_duplicate" => {
            "more than one fresh field; the latest valid one wins"
                .to_string()
        }
        "fresh_misplaced" => {
            "fresh, refresh, or keeps sits inside the Tasks suffix; the next stamp repairs it"
                .to_string()
        }
        "refresh_invalid" => {
            "refresh is not 1-365 days; falling through to the next level"
                .to_string()
        }
        "keeps_invalid" => {
            "keeps is not a decimal integer 1-999; ignored".to_string()
        }
        "keeps_duplicate" => {
            "more than one keeps field; the first valid one wins".to_string()
        }
        "task_refresh_invalid" => {
            "note task_refresh is not 1-365 days; falling through to config"
                .to_string()
        }
        "project_scheduled_invalid" => {
            "project scheduled is not a valid YYYY-MM-DD date; review suppressed until fixed"
                .to_string()
        }
        "freshness_stale_daily_budget_deprecated" => {
            "freshness.stale_daily_budget is deprecated; use freshness.rotten_daily_budget"
                .to_string()
        }
        "today_link_unresolved" => {
            "a Today Task Link resolves to no countable task".to_string()
        }
        other => format!("{other}: see docs/freshness.md"),
    }
}

/// A task is Today's when its block ID matches a ledger link, or —
/// for tasks without one — when its line does.
fn is_today_task(
    task: &RichTask,
    today_blocks: &BTreeSet<(String, String)>,
    today_lines: &BTreeSet<(String, u32)>,
) -> bool {
    if let Some(block_id) = &task.block_id
        && today_blocks.contains(&(task.path.clone(), block_id.clone()))
    {
        return true;
    }
    task.block_id.is_none()
        && today_lines.contains(&(task.path.clone(), task.line))
}

fn read_today(
    bob_dir: &Path,
    day_file: &Path,
    daily_relative: &str,
) -> Result<TodayScan, ScanError> {
    let contents = match fs::read_to_string(day_file) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // An absent daily note means an empty Today.
            return Ok((BTreeSet::new(), BTreeSet::new(), Vec::new()));
        }
        Err(error) => {
            return Err(ScanError::Io(format!(
                "read daily note {}: {error}",
                day_file.display()
            )));
        }
    };
    let result = today_tasks(bob_dir, daily_relative, &contents);
    let mut blocks = BTreeSet::new();
    let mut lines = BTreeSet::new();
    for task in &result.tasks {
        if !task.block_id.is_empty() {
            blocks.insert((task.path.clone(), task.block_id.clone()));
        }
        lines.insert((
            task.path.clone(),
            u32::try_from(task.line).unwrap_or(u32::MAX),
        ));
    }
    let warnings = result
        .warnings
        .iter()
        .map(|warning| Warning {
            code: warning.code.clone(),
            path: daily_relative.to_string(),
            line: warning.line.map(|line| line as u32),
            message: warning.message.clone(),
        })
        .collect();
    Ok((blocks, lines, warnings))
}

fn read_note_refresh(
    bob_dir: &Path,
    relative: &str,
) -> Result<Option<String>, ScanError> {
    let path = bob_dir.join(relative);
    let contents = fs::read_to_string(&path).map_err(|error| {
        ScanError::Io(format!("read {}: {error}", path.display()))
    })?;
    let Some(frontmatter) = parse_frontmatter(&contents) else {
        return Ok(None);
    };
    Ok(frontmatter_value(&frontmatter, "task_refresh")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

fn relative_path(path: &Path, bob_dir: &Path) -> String {
    path.strip_prefix(bob_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_string()
}

fn dataview_message(error: &DataviewError) -> String {
    match error {
        DataviewError::TasksQuery { message }
        | DataviewError::NativeQuery { message }
        | DataviewError::DataviewQuery { message }
        | DataviewError::DataviewMissing { message } => message.clone(),
        DataviewError::NativeVaultRead { path, error } => {
            format!("read {}: {error}", path.display())
        }
        DataviewError::TasksSettingsRead { path, error } => {
            format!("read Tasks settings {}: {error}", path.display())
        }
        DataviewError::TasksSettingsParse { path, error } => {
            format!("parse Tasks settings {}: {error}", path.display())
        }
        other => format!("{other:?}"),
    }
}
