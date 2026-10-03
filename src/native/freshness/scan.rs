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
}

impl RowCtx {
    /// Build the evaluator input. `lane_visible` is the NEXT/PENDING
    /// lane predicate: rows from [`dataview::READY_QUERY`] already
    /// carry it from the engine, so callers pass `true` there.
    pub(crate) fn freshness_row(&self, lane_visible: bool) -> FreshnessRow {
        FreshnessRow {
            path: self.task.path.clone(),
            line: self.task.line,
            status: self.task.status_symbol.chars().next().unwrap_or(' '),
            is_todo: self.task.status_type == "TODO",
            recurring: self.task.is_recurring,
            lane_visible,
            is_daily_note: self.is_daily_note,
            is_today: self.is_today,
            scheduled: self.task.scheduled,
            created: self.task.created,
            raw_line: self.task.original_markdown.clone(),
            note_refresh_raw: self.note_refresh_raw.clone(),
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

    Ok(Snapshot {
        today,
        weekday,
        config,
        ready,
        pending,
        next,
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
