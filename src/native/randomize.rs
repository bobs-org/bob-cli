//! `bob randomize`: bulk re-roll of due prioritized tasks.
//!
//! Re-schedules every due P1–P4 Obsidian task to an independent random date
//! inside that task's priority window, then publishes the whole change as a
//! single scoped commit between two vault-sync cycles under the shared
//! maintenance lock. Status and grouping are composed in the same write so
//! `task-status-hooks` has nothing left to do in the touched notes.

use std::{
    ffi::OsString,
    iter,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use chrono::NaiveDate;
use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde_json::json;

use super::{
    config, env as bob_env, ob, pomodoro,
    randomize_plan::{self, NoteSnapshot, Plan, PlanContext, Reroll},
    style::{display_width, pad_right, Styler},
    task_status_hooks::{
        daily_anchor_date, markdown_files, read_tasks_settings,
        validate_blocked_status,
    },
    task_status_hooks_write::{
        apply_plan, capture_optional, capture_required, planned_write,
        ApplyOutcome, ApplySession, CaptureError, InputKind, InputSnapshot,
        ReasonCode, WritePlan,
    },
    vault_sync::{self, CycleReport},
};

const COMMAND_NAME: &str = "bob randomize";
const DEFAULT_RETRY_TIMEOUT_SECS: u64 = 60;
const RANDOMIZE_TOOL: &str = "randomize";
const SPARK_BLOCKS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let format = OutputFormat::from_matches(&matches);
    run_request(&matches, format)
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Re-roll due prioritized tasks within their priority windows")
        .long_about(
            "Re-schedule every due prioritized task to an independent random date inside that task's own priority window from the Bob config.\n\n\
A task qualifies when it is an open Ready or Blocked task with exactly one \
`priority` field naming a configured level and exactly one strict \
`YYYY-MM-DD` `scheduled` date on or before the cutoff. Each re-roll replaces \
the scheduled date, flips a future-dated Ready task to Blocked, and writes a \
🎲 Schedule Log entry; eligible project notes are regrouped in the same write.\n\n\
P0 tasks, Next and In Progress tasks, tasks linked from today's open \
Pomodoros, and tasks with `due` or `repeat` dates are always left alone, as \
are tasks owned by `bob projects sync` (`^prj`).\n\n\
A live run holds the shared vault-maintenance lock while it syncs, plans, \
writes, commits exactly the rewritten notes as one `bob randomize` commit, \
and syncs again. Undo a run with `git -C ~/bob revert <sha> && bob vault-sync`; \
re-running with the printed seed replays the same dates.",
        )
        .after_help(format!(
            "Examples:\n  {COMMAND_NAME} --dry-run\n                                 Preview what would move and where\n  {COMMAND_NAME} --seed 0x7f3a91c2\n                                 Apply the dates a dry run showed\n  {COMMAND_NAME} --level P2 --level P3\n                                 Leave P1 tasks for hand triage\n  {COMMAND_NAME} --until +7\n                                 Clear a week for P0 work\n  {COMMAND_NAME} --dry-run --format json\n                                 Machine-readable preview\n\nEnvironment:\n  BOB_CONFIG_FILE  priority-window config; defaults to ~/.config/bob/config.yml\n  BOB_DAY_FILE  exact current daily ledger; its dated filename anchors the effective day\n  BOB_DIR       Bob vault root\n  BOB_NOW       current date/time override for the effective day\n  BOB_PRIORITY_ROLL_SEED  default base seed when --seed is omitted\n  BOB_VAULT_SYNC_LOCK_FILE  shared vault-maintenance lock file\n  BOB_VAULT_SYNC_STATE_FILE  vault-sync status record\n  NO_COLOR      disable colored output\n  XDG_CONFIG_HOME  config root when BOB_CONFIG_FILE is unset\n  XDG_STATE_HOME  recovery-record root"
        ))
        .disable_help_flag(true)
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .short('d')
                .action(ArgAction::SetTrue)
                .help("Preview the full plan without locking, syncing, or writing"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .value_parser(["human", "json"])
                .default_value("human")
                .help("Output format: human or json"),
        )
        .arg(
            Arg::new("help")
                .long("help")
                .short('h')
                .action(ArgAction::Help)
                .help("Show help"),
        )
        .arg(
            Arg::new("level")
                .long("level")
                .short('l')
                .value_name("LABEL")
                .action(ArgAction::Append)
                .value_parser(OsStringValueParser::new())
                .help("Only re-roll these configured labels; repeatable"),
        )
        .arg(
            Arg::new("offline")
                .long("offline")
                .short('o')
                .action(ArgAction::SetTrue)
                .help("Skip both vault-sync cycles; commit locally without pushing"),
        )
        .arg(
            Arg::new("retry-timeout")
                .long("retry-timeout")
                .short('r')
                .value_name("SECONDS")
                .value_parser(clap::value_parser!(u64))
                .default_value("60")
                .help("Budget for waiting on the maintenance lock and for re-planning after concurrent-edit races; 0 fails fast"),
        )
        .arg(
            Arg::new("seed")
                .long("seed")
                .short('s')
                .value_name("SEED")
                .value_parser(OsStringValueParser::new())
                .help("Base seed, decimal or 0x-hex; a dry run prints the seed that reproduces its dates"),
        )
        .arg(
            Arg::new("until")
                .long("until")
                .short('u')
                .value_name("DATE|+N")
                .value_parser(OsStringValueParser::new())
                .help("Treat tasks scheduled through DATE (or N days from today) as due and roll their windows from that date"),
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human")
        {
            "json" => Self::Json,
            _ => Self::Human,
        }
    }
}

/// Usage errors exit 2 with a plain stderr line. They precede any run
/// state, so there is no JSON document for them even in `--format json`.
fn usage_error(message: &str) -> i32 {
    eprintln!("{COMMAND_NAME}: {message}");
    2
}

fn parse_until(
    raw: Option<&OsString>,
    today: NaiveDate,
) -> Result<NaiveDate, String> {
    let Some(raw) = raw else {
        return Ok(today);
    };
    let text = raw.to_string_lossy().trim().to_string();
    if let Some(plus) = text.strip_prefix('+') {
        let days: u64 = plus.parse().map_err(|_| {
            format!("invalid --until {text:?}: expected YYYY-MM-DD or +N")
        })?;
        let days: i64 = days.try_into().map_err(|_| {
            format!("invalid --until {text:?}: date out of range")
        })?;
        // `try_days` instead of `days`: the latter panics on huge spans
        // such as `i64::MAX`, while the former reports `None`.
        let delta = chrono::Duration::try_days(days).ok_or_else(|| {
            format!("invalid --until {text:?}: date out of range")
        })?;
        let date = today.checked_add_signed(delta).ok_or_else(|| {
            format!("invalid --until {text:?}: date out of range")
        })?;
        if date < today {
            return Err(format!(
                "invalid --until {}: expected today or later",
                date.format("%Y-%m-%d")
            ));
        }
        return Ok(date);
    }
    let date = NaiveDate::parse_from_str(&text, "%Y-%m-%d").map_err(|_| {
        format!("invalid --until {text:?}: expected YYYY-MM-DD or +N")
    })?;
    if date < today {
        return Err(format!(
            "invalid --until {}: expected today or later",
            date.format("%Y-%m-%d")
        ));
    }
    Ok(date)
}

fn parse_seed_value(text: &str) -> Option<u64> {
    let trimmed = text.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        if hex.is_empty() {
            return None;
        }
        u64::from_str_radix(hex, 16).ok()
    } else {
        trimmed.parse::<u64>().ok()
    }
}

fn resolve_seed(raw: Option<&OsString>) -> Result<u64, String> {
    if let Some(raw) = raw {
        let text = raw.to_string_lossy().into_owned();
        return parse_seed_value(&text).ok_or_else(|| {
            format!(
                "invalid --seed {text:?}: expected a decimal or 0x-hex integer"
            )
        });
    }
    if let Some(from_env) = std::env::var("BOB_PRIORITY_ROLL_SEED")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| parse_seed_value(&value))
    {
        return Ok(from_env);
    }
    Ok(config::roll_seed() & 0xffff_ffff)
}

fn seed_hex(seed: u64) -> String {
    format!("0x{seed:08x}")
}

fn run_request(matches: &ArgMatches, format: OutputFormat) -> i32 {
    let dry_run = matches.get_flag("dry-run");
    let offline = matches.get_flag("offline");
    let retry_secs = matches
        .get_one::<u64>("retry-timeout")
        .copied()
        .unwrap_or(DEFAULT_RETRY_TIMEOUT_SECS);
    let retry_timeout = Duration::from_secs(retry_secs);

    let vault = bob_env::bob_dir();
    let daily_path = pomodoro::day_file_for(&vault);
    let today =
        daily_anchor_date(&daily_path, bob_env::current_datetime().date());

    let until = match parse_until(matches.get_one::<OsString>("until"), today) {
        Ok(until) => until,
        Err(message) => return usage_error(&message),
    };
    let seed = match resolve_seed(matches.get_one::<OsString>("seed")) {
        Ok(seed) => seed,
        Err(message) => return usage_error(&message),
    };
    let property = match config::load_priority_property(&config::config_path())
    {
        Ok(property) => property,
        Err(error) => {
            let message = match error {
                config::ConfigError::Read(message)
                | config::ConfigError::Invalid(message) => message,
            };
            // A config failure precedes any plan: report the empty
            // shape so the JSON contract still holds.
            let mut report = Report::new(
                format,
                dry_run,
                offline,
                retry_secs,
                RunParams::new(today, until, seed, None),
                &vault,
                &daily_path,
            );
            report.fail(
                "config",
                format!("load priority config: {message}"),
                Some(
                    "set BOB_CONFIG_FILE or run 'chezmoi apply ~/.config/bob/config.yml'",
                ),
            );
            print_report(&report);
            return 1;
        }
    };

    let level_args: Vec<OsString> = matches
        .get_many::<OsString>("level")
        .map(|values| values.cloned().collect())
        .unwrap_or_default();
    let mut selected = Vec::new();
    for level in &level_args {
        let text = level.to_string_lossy().into_owned();
        match property.level_by_label(&text) {
            Some(matched) => {
                let label = matched.label().to_string();
                if !selected.contains(&label) {
                    selected.push(label);
                }
            }
            None => {
                return usage_error(&format!(
                    "unknown --level {text:?}; valid labels: {}",
                    property.labels()
                ));
            }
        }
    }
    // Canonical labels in config order; None when --level was not given.
    let selected = if level_args.is_empty() {
        None
    } else {
        Some(
            property
                .levels()
                .iter()
                .map(|level| level.label().to_string())
                .filter(|label| selected.contains(label))
                .collect::<Vec<_>>(),
        )
    };

    let mut report = Report::new(
        format,
        dry_run,
        offline,
        retry_secs,
        RunParams::new(today, until, seed, selected),
        &vault,
        &daily_path,
    );
    let code = execute(&mut report, retry_timeout);
    print_report(&report);
    code
}

/// The effective-day, cutoff, seed, and level inputs one run plans from.
struct RunParams {
    today: NaiveDate,
    until: NaiveDate,
    seed: u64,
    selected: Option<Vec<String>>,
}

impl RunParams {
    fn new(
        today: NaiveDate,
        until: NaiveDate,
        seed: u64,
        selected: Option<Vec<String>>,
    ) -> Self {
        Self {
            today,
            until,
            seed,
            selected,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitMode {
    Sync,
    Offline,
    NotAWorktree,
    DryRun,
}

impl GitMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Sync => "sync",
            Self::Offline => "offline",
            Self::NotAWorktree => "not_a_worktree",
            Self::DryRun => "dry_run",
        }
    }
}

#[derive(Debug, Clone, Default)]
struct SyncSection {
    ok: bool,
    files_committed: usize,
    pushed: bool,
    conflicts: Vec<String>,
    error: Option<String>,
}

impl SyncSection {
    fn from_report(report: &CycleReport, for_pre_sync: bool) -> Self {
        Self {
            ok: report.ok,
            files_committed: if for_pre_sync {
                report.files_committed
            } else {
                0
            },
            pushed: if for_pre_sync { false } else { report.pushed },
            conflicts: report.conflicts.clone(),
            error: report.error.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct CommitSection {
    sha: String,
    subject: String,
    paths: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct FailureSection {
    stage: &'static str,
    message: String,
    hint: Option<String>,
}

/// The full run state. Rendering (human or JSON) reads only this struct,
/// so stdout stays a pure function of the run even on failure.
struct Report {
    format: OutputFormat,
    dry_run: bool,
    today: NaiveDate,
    until: NaiveDate,
    seed: u64,
    selected: Option<Vec<String>>,
    vault: PathBuf,
    vault_display: String,
    daily_path: PathBuf,
    plan: Option<Plan>,
    warnings: Vec<String>,
    git_mode: GitMode,
    pre_sync: Option<SyncSection>,
    commit: Option<CommitSection>,
    post_sync: Option<SyncSection>,
    recovery_directory: Option<String>,
    failure: Option<FailureSection>,
    retry_secs: u64,
    offline: bool,
}

impl Report {
    fn new(
        format: OutputFormat,
        dry_run: bool,
        offline: bool,
        retry_secs: u64,
        params: RunParams,
        vault: &Path,
        daily_path: &Path,
    ) -> Self {
        Self {
            format,
            dry_run,
            today: params.today,
            until: params.until,
            seed: params.seed,
            selected: params.selected,
            vault: vault.to_path_buf(),
            vault_display: display_path(vault),
            daily_path: daily_path.to_path_buf(),
            plan: None,
            warnings: Vec::new(),
            git_mode: if dry_run {
                GitMode::DryRun
            } else if offline {
                GitMode::Offline
            } else {
                GitMode::Sync
            },
            pre_sync: None,
            commit: None,
            post_sync: None,
            recovery_directory: None,
            failure: None,
            retry_secs,
            offline,
        }
    }

    fn fail(
        &mut self,
        stage: &'static str,
        message: impl Into<String>,
        hint: Option<&str>,
    ) {
        self.failure = Some(FailureSection {
            stage,
            message: message.into(),
            hint: hint.map(str::to_string),
        });
    }

    fn failed(&self) -> bool {
        self.failure.is_some()
    }

    fn warn(&mut self, warning: String) {
        if self.format == OutputFormat::Json {
            // Progress and warnings go to stderr in JSON mode; the
            // human renderer prints them at the end instead.
            eprintln!("warning: {warning}");
        }
        self.warnings.push(warning);
    }

    /// One stderr progress line in JSON mode; a no-op for human output,
    /// which renders the full report to stdout at the end.
    fn progress(&self, line: &str) {
        if self.format == OutputFormat::Json {
            eprintln!("{line}");
        }
    }
}

/// Vault root for display: `~/bob` when under `$HOME`.
fn display_path(path: &Path) -> String {
    let home = bob_env::home_dir();
    if let Ok(relative) = path.strip_prefix(&home) {
        if relative.as_os_str().is_empty() {
            return "~".to_string();
        }
        return format!("~/{}", relative.display());
    }
    path.display().to_string()
}

/// Lock → sync → plan → apply → commit → sync. Returns the process
/// exit code; the rendered report is read from `report` afterwards.
fn execute(report: &mut Report, retry_timeout: Duration) -> i32 {
    // Config loads before the lock is taken.
    let property = match config::load_priority_property(&config::config_path())
    {
        Ok(property) => property,
        Err(error) => {
            let message = match error {
                config::ConfigError::Read(message)
                | config::ConfigError::Invalid(message) => message,
            };
            report.fail(
                    "config",
                    format!("load priority config: {message}"),
                    Some(
                        "set BOB_CONFIG_FILE or run 'chezmoi apply ~/.config/bob/config.yml'",
                    ),
                );
            return 1;
        }
    };

    let _lock = if report.dry_run {
        None
    } else {
        let styler = Styler::detect();
        let waiting = format!(
            "  {}",
            styler.dim("waiting for another vault maintenance run…")
        );
        let json_mode = report.format == OutputFormat::Json;
        let on_first_wait = move || {
            if json_mode {
                eprintln!("waiting for another vault maintenance run…");
            } else {
                eprintln!("{waiting}");
            }
        };
        match ob::acquire_lock_waiting(retry_timeout, on_first_wait) {
            Ok(lock) => Some(lock),
            Err(error) => {
                report.fail(
                    "lock",
                    format!("acquire vault maintenance lock: {error}"),
                    Some(&format!(
                        "another maintenance run held the lock for {} s; try again",
                        report.retry_secs
                    )),
                );
                return 1;
            }
        }
    };

    let child_env = ob::child_env();
    // A non-worktree vault skips every git step: no syncs, no commit.
    // Detect it before the pre-sync so a vault without git never fails
    // a cycle it should never have run. Dry runs never touch git, so
    // they skip the probe (and its warning) entirely.
    let worktree = if report.dry_run {
        false
    } else {
        let inside = match ob::detect_git_worktree(&report.vault, &child_env) {
            Ok(inside) => inside,
            Err(error) => {
                report.warn(format!(
                    "git is unavailable ({error}); skipping commit and push"
                ));
                false
            }
        };
        if !inside {
            report.git_mode = GitMode::NotAWorktree;
            report.warn(format!(
                "vault is not a git worktree ({}); skipping commit and push",
                report.vault.display()
            ));
        }
        inside
    };
    if !report.dry_run && !report.offline && worktree {
        let cycle =
            vault_sync::run_cycle_with_existing_lock_report(&child_env, true);
        let section = SyncSection::from_report(&cycle, true);
        report.progress(&format!("synced vault: {}", presync_detail(&section)));
        report.pre_sync = Some(section);
        if !cycle.ok {
            report.fail(
                "pre_sync",
                format!(
                    "pre-sync failed: {}",
                    cycle.error.clone().unwrap_or_else(|| {
                        "vault sync reported failure".to_string()
                    })
                ),
                Some("re-run with --offline to commit locally without syncing"),
            );
            return 1;
        }
    } else if report.offline {
        report.progress("offline: skipped vault sync");
    }

    let applied = match plan_and_apply(report, &property, &retry_timeout) {
        Ok(applied) => applied,
        Err(code) => return code,
    };

    // Dry runs stop after planning: no lock was taken and nothing may
    // be written, committed, or synced.
    if report.dry_run {
        return if report.failed() { 1 } else { 0 };
    }

    if report
        .plan
        .as_ref()
        .is_some_and(|plan| plan.rerolls.is_empty())
    {
        return 0;
    }

    // Git steps: scoped commit, then the post-sync cycle.
    if !worktree {
        return 0;
    }

    let commit = match commit_applied(report, &applied, &child_env) {
        Ok(commit) => commit,
        Err(code) => return code,
    };
    report.commit = commit;

    if report.offline {
        return if report.failed() { 1 } else { 0 };
    }
    post_sync(report, &child_env)
}

fn presync_detail(section: &SyncSection) -> String {
    if section.files_committed == 0 {
        "already in sync".to_string()
    } else if section.files_committed == 1 {
        "committed 1 pending note separately".to_string()
    } else {
        format!(
            "committed {} pending notes separately",
            section.files_committed
        )
    }
}

/// One full scan from disk plus the pure plan. Non-UTF-8 notes are
/// warned on and skipped; anything else unreadable fails the plan.
struct Scan {
    snapshots: Vec<NoteSnapshot>,
    inputs: Vec<InputSnapshot>,
    daily_contents: Option<String>,
}

fn scan_vault(report: &mut Report) -> Result<Scan, String> {
    let files = markdown_files(&report.vault).map_err(|error| {
        format!("scan vault {}: {error}", report.vault.display())
    })?;
    let mut snapshots = Vec::with_capacity(files.len());
    let mut inputs = Vec::with_capacity(files.len());
    for path in &files {
        let snapshot = match capture_required(path, InputKind::Note) {
            Ok(snapshot) => snapshot,
            Err(CaptureError::NotFound(_)) => continue,
            Err(error) => {
                return Err(format!(
                    "read note {}: {}",
                    relative_display(&report.vault, path),
                    error.message()
                ));
            }
        };
        let contents = match snapshot.utf8_contents() {
            Ok(Some(contents)) => contents,
            Ok(None) => {
                return Err(format!(
                    "read note {}: file does not exist",
                    relative_display(&report.vault, path)
                ));
            }
            Err(_) => {
                report.warn(format!(
                    "{}: note is not valid UTF-8; skipped",
                    relative_display(&report.vault, path)
                ));
                continue;
            }
        };
        snapshots.push(NoteSnapshot {
            path: path.clone(),
            relative_path: path.strip_prefix(&report.vault).map_or_else(
                |_| PathBuf::from(path.file_name().unwrap_or_default()),
                Path::to_path_buf,
            ),
            contents,
        });
        inputs.push(snapshot);
    }
    let from_snapshot = snapshots
        .iter()
        .find(|snapshot| paths_match(&snapshot.path, &report.daily_path))
        .map(|snapshot| snapshot.contents.clone());
    let daily_contents = match from_snapshot {
        Some(contents) => Some(contents),
        None => match capture_optional(&report.daily_path, InputKind::Daily) {
            Ok(snapshot) => match snapshot.utf8_contents() {
                Ok(contents) => contents,
                Err(_) => {
                    report.warn(format!(
                            "{}: daily note is not valid UTF-8; Pomodoro exclusions skipped",
                            report.daily_path.display()
                        ));
                    None
                }
            },
            Err(error) => {
                return Err(format!(
                    "read daily note {}: {}",
                    report.daily_path.display(),
                    error.message()
                ));
            }
        },
    };
    Ok(Scan {
        snapshots,
        inputs,
        daily_contents,
    })
}

fn paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Plan, validate the Blocked registry when needed, and apply with
/// retries. Returns the applied absolute paths; an empty plan (nothing
/// due) succeeds with an empty vec.
/// A planning failure with its own actionable hint. Date-range
/// overflows carry no hint; only the Blocked registry check points at
/// the Tasks settings.
struct PlanFailure {
    message: String,
    hint: Option<String>,
}

fn plan_and_apply(
    report: &mut Report,
    property: &config::PriorityProperty,
    retry_timeout: &Duration,
) -> Result<Vec<PathBuf>, i32> {
    if report.dry_run {
        let scan = scan_vault(report).map_err(|message| {
            report.fail("plan", message, None);
            1
        })?;
        let plan = plan_once(report, &scan, property).map_err(|failure| {
            report.fail("plan", failure.message, failure.hint.as_deref());
            1
        })?;
        report.plan = Some(plan);
        return Ok(Vec::new());
    }

    let started = Instant::now();
    loop {
        let scan = scan_vault(report).map_err(|message| {
            report.fail("plan", message, None);
            1
        })?;
        let plan = plan_once(report, &scan, property).map_err(|failure| {
            report.fail("plan", failure.message, failure.hint.as_deref());
            1
        })?;
        if plan.rerolls.is_empty() {
            report.plan = Some(plan);
            return Ok(Vec::new());
        }
        let write_plan = match build_write_plan(report, &scan, &plan) {
            Ok(write_plan) => write_plan,
            Err(message) => {
                report.fail("apply", message, None);
                report.plan = Some(plan);
                return Err(1);
            }
        };
        let mut session = ApplySession::production(Box::new(|| Ok(Vec::new())));
        session.tool = RANDOMIZE_TOOL;
        match apply_plan(&write_plan, &session) {
            Ok(ApplyOutcome::NoOp) => {
                report.plan = Some(plan);
                return Ok(Vec::new());
            }
            Ok(ApplyOutcome::Applied {
                applied_files,
                recovery_directory,
            }) => {
                report.plan = Some(plan);
                report.recovery_directory =
                    Some(recovery_directory.display().to_string());
                report.progress(&format!(
                    "applied {} note(s)",
                    applied_files.len()
                ));
                return Ok(applied_files);
            }
            Err(error) => {
                let retryable = matches!(
                    error.reason,
                    ReasonCode::VaultChanged
                        | ReasonCode::QuietPeriod
                        | ReasonCode::UnstableRead
                ) && error.applied_files.is_empty();
                if retryable && started.elapsed() < *retry_timeout {
                    continue;
                }
                report.plan = Some(plan);
                if error.applied_files.is_empty() {
                    if let Some(recovery) = error.recovery_directory {
                        report.recovery_directory =
                            Some(recovery.display().to_string());
                    }
                    report.fail(
                        "apply",
                        error.message,
                        Some("re-run the command to converge"),
                    );
                    return Err(1);
                }
                // Partial apply: the written notes are each
                // self-consistent. Commit them, post-sync, and report
                // the rest.
                if let Some(recovery) = error.recovery_directory.clone() {
                    report.recovery_directory =
                        Some(recovery.display().to_string());
                }
                let remaining = error
                    .deferred_files
                    .iter()
                    .map(|path| relative_display(&report.vault, path))
                    .collect::<Vec<_>>()
                    .join(", ");
                let child_env = ob::child_env();
                let worktree =
                    ob::detect_git_worktree(&report.vault, &child_env)
                        .unwrap_or(false);
                if worktree {
                    if let Ok(commit) = commit_paths_for(
                        report,
                        &error.applied_files,
                        &child_env,
                    ) {
                        report.commit = commit;
                    }
                    if !report.offline {
                        let _ = post_sync(report, &child_env);
                    }
                }
                report.fail(
                    "apply",
                    format!(
                        "{}; remaining notes were not written: {remaining}",
                        error.message
                    ),
                    Some(
                        "re-run the command to converge on the remaining notes",
                    ),
                );
                return Err(1);
            }
        }
    }
}

/// Scan-free plan over one snapshot set, plus the Blocked registry
/// check. Fails before any write when new Blocked statuses appear or
/// when a cutoff plus a configured roll leaves the representable date
/// range.
fn plan_once(
    report: &Report,
    scan: &Scan,
    property: &config::PriorityProperty,
) -> Result<Plan, PlanFailure> {
    let settings = read_tasks_settings(&report.vault);
    let context = PlanContext {
        today: report.today,
        until: report.until,
        seed: report.seed,
        priority: property,
        selected_labels: report.selected.as_deref(),
        tasks_settings: &settings,
        daily_path: report.daily_path.as_path(),
        daily_contents: scan.daily_contents.as_deref(),
    };
    let plan = randomize_plan::plan_notes(&scan.snapshots, &context).map_err(
        |message| PlanFailure {
            message,
            hint: None,
        },
    )?;
    if plan.needs_blocked_status {
        validate_blocked_status(&settings).map_err(|error| PlanFailure {
            message: error.message().to_string(),
            hint: Some(
                "configure one custom Tasks status named Blocked with symbol '?', type ON_HOLD, next status ' ', and availableAsCommand true"
                    .to_string(),
            ),
        })?;
    }
    Ok(plan)
}

fn build_write_plan(
    report: &Report,
    scan: &Scan,
    plan: &Plan,
) -> Result<WritePlan, String> {
    let vault_canonical = report
        .vault
        .canonicalize()
        .unwrap_or_else(|_| report.vault.clone());
    let mut outputs = Vec::with_capacity(plan.notes.len());
    for note in &plan.notes {
        let Some(snapshot) =
            scan.inputs.iter().find(|input| input.path == note.path)
        else {
            return Err(format!(
                "read note {}: file changed during planning; re-run the command",
                relative_display(&report.vault, &note.path)
            ));
        };
        planned_write(
            note.path.clone(),
            snapshot,
            note.updated.clone().into_bytes(),
            true,
        )
        .map_err(|error| error.message())
        .map(|write| outputs.push(write))?;
    }
    Ok(WritePlan {
        vault_canonical,
        inputs: scan
            .inputs
            .iter()
            .filter(|input| {
                plan.notes.iter().any(|note| note.path == input.path)
            })
            .cloned()
            .collect(),
        scan_paths: Vec::new(),
        outputs,
    })
}

fn commit_paths_for(
    report: &Report,
    applied: &[PathBuf],
    child_env: &ob::ChildEnv,
) -> Result<Option<CommitSection>, String> {
    if applied.is_empty() {
        return Ok(None);
    }
    let mut relative: Vec<PathBuf> = applied
        .iter()
        .map(|path| {
            path.strip_prefix(&report.vault)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| path.clone())
        })
        .collect();
    relative.sort();
    let message = commit_message(report);
    let sha = ob::commit_paths(&report.vault, child_env, &message, &relative)?;
    Ok(sha.map(|sha| {
        let subject = message.lines().next().unwrap_or("").to_string();
        let paths = relative
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        CommitSection {
            sha,
            subject,
            paths,
        }
    }))
}

fn commit_applied(
    report: &mut Report,
    applied: &[PathBuf],
    child_env: &ob::ChildEnv,
) -> Result<Option<CommitSection>, i32> {
    match commit_paths_for(report, applied, child_env) {
        Ok(commit) => {
            if let Some(commit) = &commit {
                report.progress(&format!(
                    "committed {} {}",
                    short_sha(&commit.sha),
                    commit.subject
                ));
            }
            Ok(commit)
        }
        Err(message) => {
            report.fail(
                "commit",
                format!("commit notes: {message}"),
                Some(
                    "resolve the git failure and re-run; notes are already written",
                ),
            );
            Err(1)
        }
    }
}

/// The post-sync cycle. On failure the local commit stands: report it
/// and exit 1, never rolling back good local edits. A conflict counts as
/// failure even when the sync cycle itself reports success: vault-sync's
/// policy keeps the remote copy in place, so the reroll is lost locally.
fn post_sync(report: &mut Report, child_env: &ob::ChildEnv) -> i32 {
    let cycle =
        vault_sync::run_cycle_with_existing_lock_report(child_env, true);
    let section = SyncSection::from_report(&cycle, false);
    report.post_sync = Some(section.clone());
    if !cycle.conflicts.is_empty() {
        let mut conflicts = cycle.conflicts.clone();
        conflicts.sort();
        let names = conflicts.join(", ");
        report.fail(
            "post_sync",
            format!(
                "post-sync conflict in {names}: the remote copy wins in place and randomize's version is kept under _conflicts/; re-running re-rolls whatever is still due"
            ),
            Some("resolve the _conflicts/ copies, then re-run to converge"),
        );
        return 1;
    }
    if cycle.ok {
        report.progress("pushed to origin/master");
        return if report.failed() { 1 } else { 0 };
    }
    let sha = report
        .commit
        .as_ref()
        .map(|commit| short_sha(&commit.sha))
        .unwrap_or_else(|| "unknown".to_string());
    report.fail(
        "post_sync",
        format!(
            "post-sync failed ({}); committed `{sha}` locally; background vault-sync will publish it",
            cycle.error.clone().unwrap_or_else(|| {
                "vault sync reported failure".to_string()
            })
        ),
        Some("run bob vault-sync later, or re-run with --offline"),
    );
    1
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// Vault-relative `a/b.md` display form shared with the planner.
fn relative_display(vault: &Path, path: &Path) -> String {
    path.strip_prefix(vault)
        .map_or_else(|_| path.display().to_string(), display_relative)
}

fn display_relative(relative: &Path) -> String {
    relative
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => {
                Some(part.to_string_lossy().into_owned())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn iso(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

fn long_day(date: NaiveDate) -> String {
    date.format("%a %b %-d").to_string()
}

fn plural(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

/// The scoped commit message: one subject line plus per-level, seed,
/// and per-note accounting.
fn commit_message(report: &Report) -> String {
    let plan = report.plan.as_ref();
    let rerolled = plan.map_or(0, |plan| plan.rerolls.len());
    let notes = plan.map_or(0, |plan| plan.notes.len());
    let subject = format!(
        "bob randomize {}: {} in {}",
        iso(report.today),
        plural(rerolled, "task", "tasks"),
        plural(notes, "note", "notes")
    );
    let mut levels = level_rows(plan);
    levels.sort_by(|left, right| left.label.cmp(&right.label));
    let level_line = levels
        .iter()
        .map(|row| format!("{} {}", row.label, row.count))
        .collect::<Vec<_>>()
        .join(" · ");
    let mut note_rows: Vec<(String, usize)> = plan
        .map(|plan| {
            plan.notes
                .iter()
                .map(|note| {
                    (display_relative(&note.relative_path), note.rerolls.len())
                })
                .collect()
        })
        .unwrap_or_default();
    note_rows.sort_by(|left, right| {
        right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0))
    });
    let width = note_rows
        .iter()
        .map(|row| row.1.to_string().len())
        .max()
        .unwrap_or(1);
    let note_lines = note_rows
        .iter()
        .map(|row| format!("{:>width$} {}", row.1, row.0))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{subject}\n\n{level_line}\nuntil {} · seed {}\n\n{note_lines}",
        iso(report.until),
        seed_hex(report.seed)
    )
}

#[derive(Debug, Clone)]
struct LevelRow {
    label: String,
    value: String,
    count: usize,
    min_days: u64,
    max_days: u64,
    first: NaiveDate,
    last: NaiveDate,
}

fn level_rows(plan: Option<&Plan>) -> Vec<LevelRow> {
    let mut rows: Vec<LevelRow> = Vec::new();
    for reroll in plan.map_or(&[][..], |plan| plan.rerolls.as_slice()) {
        let to = NaiveDate::parse_from_str(&reroll.to, "%Y-%m-%d")
            .unwrap_or(report_today_fallback());
        if let Some(row) = rows.iter_mut().find(|row| row.label == reroll.level)
        {
            row.count += 1;
            row.first = row.first.min(to);
            row.last = row.last.max(to);
        } else {
            rows.push(LevelRow {
                label: reroll.level.clone(),
                value: reroll.value.clone(),
                count: 1,
                min_days: reroll.min_days,
                max_days: reroll.max_days,
                first: to,
                last: to,
            });
        }
    }
    rows
}

fn report_today_fallback() -> NaiveDate {
    bob_env::current_datetime().date()
}

fn heat(styler: &Styler, label: &str) -> String {
    match label {
        "P1" => styler.red(label),
        "P2" => styler.yellow(label),
        "P3" => styler.blue(label),
        "P4" => styler.dim(label),
        _ => label.to_string(),
    }
}

/// Cut a description with `…` so the full line fits in 100 columns.
fn truncate_description(description: &str, used_width: usize) -> String {
    const MAX_WIDTH: usize = 100;
    let available = MAX_WIDTH.saturating_sub(used_width);
    if display_width(description) <= available {
        return description.to_string();
    }
    if available <= 1 {
        return "…".to_string();
    }
    let kept = description
        .chars()
        .take(available.saturating_sub(1))
        .collect::<String>();
    format!("{kept}…")
}

fn sparkline(counts: &[usize]) -> (String, usize, usize) {
    let peak = counts.iter().copied().max().unwrap_or(0);
    let peak_index =
        counts.iter().position(|count| *count == peak).unwrap_or(0);
    // `max(1)` keeps the ratio finite without a zero check: an empty
    // window renders flat, and every other window has a nonzero peak.
    let cells = counts
        .iter()
        .map(|count| {
            let ratio = *count as f64 / peak.max(1) as f64;
            let index = (ratio * 7.0).round() as usize;
            SPARK_BLOCKS[index.min(7)]
        })
        .collect::<String>();
    (cells, peak, peak_index)
}

fn skip_reason_text(
    reason: randomize_plan::SkipReason,
    detail: Option<&str>,
) -> Option<String> {
    use randomize_plan::SkipReason as Reason;
    match reason {
        Reason::DuplicateField => Some("duplicate field".to_string()),
        Reason::InvalidScheduled => Some("invalid scheduled date".to_string()),
        Reason::UnknownPriority => Some(match detail {
            Some(value) => format!("unknown priority {value:?}"),
            None => "unknown priority".to_string(),
        }),
        Reason::HardDate => Some("has due or repeat date".to_string()),
        Reason::OtherStatus => Some("unsupported status".to_string()),
        Reason::Next | Reason::InProgress | Reason::Pomodoro => None,
        Reason::NotSelected => None,
    }
}

fn print_report(report: &Report) {
    match report.format {
        OutputFormat::Json => {
            println!("{}", json_document(report));
        }
        OutputFormat::Human => print_human(report),
    }
}

fn print_human(report: &Report) {
    let styler = Styler::detect();
    let rerolled = report.plan.as_ref().map_or(0, |plan| plan.rerolls.len());
    let note_count = report.plan.as_ref().map_or(0, |plan| plan.notes.len());

    let mut header = format!(
        "🎲 {COMMAND_NAME} · {} · {}",
        long_day(report.today),
        report.vault_display
    );
    if report.dry_run {
        header.push_str(" · dry run");
    }
    println!("{header}");

    if let Some(plan) = &report.plan
        && report.dry_run
        && !plan.rerolls.is_empty()
    {
        println!();
        print_dry_run_tasks(&styler, plan);
    }

    if !report.dry_run {
        println!();
        match report.git_mode {
            GitMode::Offline => {
                println!(
                    " {}",
                    styler.dim("○ Offline mode — skipped vault sync")
                );
            }
            GitMode::NotAWorktree => {}
            _ => {
                if let Some(pre_sync) = &report.pre_sync {
                    println!(
                        " {} {}  {}",
                        styler.green("✓"),
                        pad_right("Synced vault", 20),
                        presync_detail(pre_sync)
                    );
                }
            }
        }
        if report
            .plan
            .as_ref()
            .is_some_and(|plan| !plan.rerolls.is_empty())
        {
            println!(
                " {} Re-rolled {} in {}",
                styler.green("✓"),
                plural(rerolled, "task", "tasks"),
                plural(note_count, "note", "notes")
            );
        }
    } else if report
        .plan
        .as_ref()
        .is_some_and(|plan| !plan.rerolls.is_empty())
    {
        println!();
        println!(
            " {} Would re-roll {} in {}",
            styler.green("✓"),
            plural(rerolled, "task", "tasks"),
            plural(note_count, "note", "notes")
        );
    }

    if report
        .plan
        .as_ref()
        .is_some_and(|plan| plan.rerolls.is_empty())
    {
        println!(
            " {} Nothing to re-roll — no prioritized tasks are due by {}.",
            styler.green("✓"),
            iso(report.until)
        );
    }

    if let Some(plan) = &report.plan {
        if !plan.rerolls.is_empty() {
            println!();
            print_level_table(&styler, plan);
            println!();
            print_load_and_notes(plan);
        } else {
            println!();
        }
        print_left_alone(&styler, plan);
        print_still_due(&styler, plan);
        print_needs_a_look(&styler, plan);
    }

    if !report.dry_run {
        if let Some(commit) = &report.commit {
            println!();
            println!(
                " {} Committed {}  {}",
                styler.green("✓"),
                short_sha(&commit.sha),
                commit.subject
            );
        }
        if !report.failed()
            && report.git_mode == GitMode::Sync
            && report.post_sync.as_ref().is_some_and(|sync| sync.ok)
        {
            println!(" {} Pushed to origin/master", styler.green("✓"));
        }
        if report.commit.is_some() {
            println!();
            let short = short_sha(
                &report
                    .commit
                    .as_ref()
                    .map(|commit| commit.sha.clone())
                    .unwrap_or_default(),
            );
            println!(
                "   seed {} {} undo: git -C {} revert {short} && bob vault-sync",
                seed_hex(report.seed),
                styler.separator(),
                report.vault_display
            );
        }
    } else if report
        .plan
        .as_ref()
        .is_some_and(|plan| !plan.rerolls.is_empty())
    {
        println!();
        print_replay_line(report);
    }

    for warning in &report.warnings {
        eprintln!("{}: {warning}", styler.warning_prefix());
    }
    if let Some(failure) = &report.failure {
        eprintln!("{} {}: {}", styler.red("✗"), failure.stage, failure.message);
        if let Some(hint) = &failure.hint {
            eprintln!("  hint: {hint}");
        }
    }
}

fn print_dry_run_tasks(styler: &Styler, plan: &Plan) {
    let mut by_note: Vec<(&str, Vec<&Reroll>)> = Vec::new();
    for reroll in &plan.rerolls {
        match by_note
            .iter_mut()
            .find(|(path, _)| *path == reroll.path.as_str())
        {
            Some((_, tasks)) => tasks.push(reroll),
            None => by_note.push((reroll.path.as_str(), vec![reroll])),
        }
    }
    by_note.sort_by(|left, right| {
        right
            .1
            .len()
            .cmp(&left.1.len())
            .then_with(|| left.0.cmp(right.0))
    });
    for (path, tasks) in by_note {
        println!("   {path} ({})", tasks.len());
        let mut ordered = tasks;
        ordered.sort_by_key(|reroll| reroll.line);
        for reroll in ordered {
            let prefix = format!(
                "   {}  {} → {}  ",
                reroll.level, reroll.from, reroll.to
            );
            let description = truncate_description(
                &reroll.description,
                display_width(&prefix),
            );
            println!(
                "   {}  {} → {}  {description}",
                heat(styler, &reroll.level),
                styler.cyan(&reroll.from),
                styler.cyan(&reroll.to)
            );
        }
    }
}

fn print_level_table(styler: &Styler, plan: &Plan) {
    let mut rows = level_rows(Some(plan));
    rows.sort_by(|left, right| left.label.cmp(&right.label));
    for row in rows {
        println!(
            "   {}  {} {}   {} → {}     {}–{} days",
            heat(styler, &row.label),
            pad_right(&row.value, 8),
            row.count,
            styler.cyan(&long_day(row.first)),
            styler.cyan(&long_day(row.last)),
            row.min_days,
            row.max_days
        );
    }
}

fn print_load_and_notes(plan: &Plan) {
    let counts = plan.load.iter().map(|day| day.count).collect::<Vec<_>>();
    let (cells, peak, peak_index) = sparkline(&counts);
    let peak_date = plan
        .load
        .get(peak_index)
        .map(|day| long_day(day.date))
        .unwrap_or_default();
    println!(
        "   {}  {cells}   peak {peak} · {peak_date}",
        pad_right("Next 5 weeks", 14)
    );
    let mut note_rows: Vec<(String, usize)> = plan
        .notes
        .iter()
        .map(|note| (display_relative(&note.relative_path), note.rerolls.len()))
        .collect();
    note_rows.sort_by(|left, right| {
        right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0))
    });
    let top: Vec<String> = note_rows
        .iter()
        .take(3)
        .map(|row| format!("{} {}", row.0, row.1))
        .collect();
    let mut notes_line = top.join(" · ");
    if note_rows.len() > 3 {
        notes_line.push_str(&format!(" · +{} more", note_rows.len() - 3));
    }
    println!("   {}  {notes_line}", pad_right("Notes", 14));
}

fn print_left_alone(styler: &Styler, plan: &Plan) {
    let mut items = Vec::new();
    let count = |reason| {
        plan.skipped
            .iter()
            .filter(|skip| skip.reason == reason)
            .count()
    };
    use randomize_plan::SkipReason as Reason;
    let next = count(Reason::Next);
    let in_progress = count(Reason::InProgress);
    let pomodoro = count(Reason::Pomodoro);
    if next > 0 {
        items.push(plural(next, "next", "next"));
    }
    if in_progress > 0 {
        items.push(plural(in_progress, "in progress", "in progress"));
    }
    if pomodoro > 0 {
        items.push(format!("{pomodoro} in today's Pomodoros",));
    }
    if plan.not_selected > 0 {
        items.push(plural(plan.not_selected, "not selected", "not selected"));
    }
    if items.is_empty() {
        return;
    }
    println!("   {}  {}", pad_right("Left alone", 14), items.join(" · "));
    let _ = styler;
}

fn print_still_due(styler: &Styler, plan: &Plan) {
    if plan.still_due_p0 == 0 {
        return;
    }
    println!(
        "   {}  {}",
        pad_right("Still due", 14),
        plural(plan.still_due_p0, "P0 task", "P0 tasks")
    );
    let _ = styler;
}

fn print_needs_a_look(styler: &Styler, plan: &Plan) {
    let mut flagged: Vec<(&randomize_plan::Skip, String)> = plan
        .skipped
        .iter()
        .filter_map(|skip| {
            skip_reason_text(skip.reason, skip.detail.as_deref())
                .map(|text| (skip, text))
        })
        .collect();
    if flagged.is_empty() {
        return;
    }
    flagged.sort_by(|left, right| {
        left.0
            .path
            .cmp(&right.0.path)
            .then_with(|| left.0.line.cmp(&right.0.line))
    });
    println!();
    println!(
        " {} {}",
        styler.yellow("⚠"),
        plural(flagged.len(), "task needs a look", "tasks need a look")
    );
    for (skip, text) in flagged {
        println!("     {}:{}   {text}", skip.path, skip.line);
    }
}

fn print_replay_line(report: &Report) {
    let mut command = format!("bob randomize --seed {}", seed_hex(report.seed));
    if let Some(levels) = &report.selected {
        for level in levels {
            command.push_str(&format!(" --level {level}"));
        }
    }
    if report.until != report.today {
        command.push_str(&format!(" --until {}", iso(report.until)));
    }
    println!("   Nothing was written. Apply these dates with: {command}");
}

fn schedule_log_text(
    insertion: super::capture_schedule_log::LogInsertion,
) -> &'static str {
    match insertion {
        super::capture_schedule_log::LogInsertion::Prepended => "prepended",
        super::capture_schedule_log::LogInsertion::Created => "created",
    }
}

/// The stable JSON contract: one document on stdout, even on failure.
fn json_document(report: &Report) -> serde_json::Value {
    let plan = report.plan.as_ref();
    let mut rerolls = plan.map(|plan| plan.rerolls.clone()).unwrap_or_default();
    rerolls.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.line.cmp(&right.line))
    });
    let tasks = rerolls
        .iter()
        .map(|reroll| {
            json!({
                "path": reroll.path,
                "line": reroll.line,
                "ref": reroll.task_ref,
                "block_id": reroll.block_id,
                "description": reroll.description,
                "level": reroll.level,
                "value": reroll.value,
                "min_days": reroll.min_days,
                "max_days": reroll.max_days,
                "offset_days": reroll.offset_days,
                "from": reroll.from,
                "to": reroll.to,
                "status_from": reroll.status_from.to_string(),
                "status_to": reroll.status_to.to_string(),
                "schedule_log": schedule_log_text(reroll.schedule_log),
            })
        })
        .collect::<Vec<_>>();

    let mut skipped = plan.map(|plan| plan.skipped.clone()).unwrap_or_default();
    skipped.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.line.cmp(&right.line))
    });
    let skipped = skipped
        .iter()
        .map(|skip| {
            json!({
                "path": skip.path,
                "line": skip.line,
                "reason": skip.reason.as_str(),
                "detail": skip.detail,
            })
        })
        .collect::<Vec<_>>();

    let mut note_rows: Vec<(String, usize, bool)> = plan
        .map(|plan| {
            plan.notes
                .iter()
                .map(|note| {
                    (
                        display_relative(&note.relative_path),
                        note.rerolls.len(),
                        note.regrouped,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    note_rows.sort_by(|left, right| {
        right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0))
    });
    let notes = note_rows
        .iter()
        .map(|row| {
            json!({
                "path": row.0,
                "tasks": row.1,
                "regrouped": row.2,
            })
        })
        .collect::<Vec<_>>();

    let mut levels = level_rows(plan);
    levels.sort_by(|left, right| left.label.cmp(&right.label));
    let by_level = levels
        .iter()
        .map(|row| {
            json!({
                "label": row.label,
                "value": row.value,
                "count": row.count,
                "min_days": row.min_days,
                "max_days": row.max_days,
                "first": iso(row.first),
                "last": iso(row.last),
            })
        })
        .collect::<Vec<_>>();

    let load = plan
        .map(|plan| {
            plan.load
                .iter()
                .map(|day| json!({ "date": iso(day.date), "count": day.count }))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let pre_sync = report.pre_sync.as_ref().map(|sync| {
        json!({
            "ok": sync.ok,
            "files_committed": sync.files_committed,
            "error": sync.error,
        })
    });
    let commit = report.commit.as_ref().map(|commit| {
        json!({
            "sha": commit.sha,
            "subject": commit.subject,
            "paths": commit.paths,
        })
    });
    let post_sync = report.post_sync.as_ref().map(|sync| {
        json!({
            "ok": sync.ok,
            "pushed": sync.pushed,
            "conflicts": sync.conflicts,
            "error": sync.error,
        })
    });
    let error = report.failure.as_ref().map(|failure| {
        json!({
            "stage": failure.stage,
            "message": failure.message,
        })
    });

    json!({
        "schema_version": 1,
        "ok": !report.failed(),
        "dry_run": report.dry_run,
        "today": iso(report.today),
        "until": iso(report.until),
        "seed": seed_hex(report.seed),
        "levels": report.selected,
        "summary": {
            "rerolled": plan.map_or(0, |plan| plan.rerolls.len()),
            "notes": plan.map_or(0, |plan| plan.notes.len()),
            "unchanged": plan.map_or(0, |plan| plan.unchanged),
            "still_due_p0": plan.map_or(0, |plan| plan.still_due_p0),
            "by_level": by_level,
        },
        "tasks": tasks,
        "skipped": skipped,
        "notes": notes,
        "load": load,
        "warnings": report.warnings,
        "git": {
            "mode": report.git_mode.as_str(),
            "pre_sync": pre_sync,
            "commit": commit,
            "post_sync": post_sync,
        },
        "recovery_directory": report.recovery_directory,
        "error": error,
    })
}
