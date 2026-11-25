//! Public `bob snooze` commands over the shared row model and planners.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs, iter,
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(test)]
use std::{cell::RefCell, collections::VecDeque};

use chrono::{NaiveDate, NaiveDateTime, TimeZone, Timelike};
use chrono_tz::Tz;
use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde_json::{json, Value};

use crate::native::{
    capture::{line_spans, nearest_shallower_list_item_parent},
    capture_pomodoros::{self, PomodoroState},
    config::{self, PlanConfig},
    env as bob_env, event_task, markdown, note_tasks,
    plan_budget::{self, off_clock::DiskCloseVault},
    pomodoro,
    pomodoro_homes::{DiskHomeSource, HomeResolver},
    pomodoro_themes,
    style::Styler,
    task_dependencies::block_link_spans,
    task_locator::TaskLocator,
    task_status_hooks::TaskStatusType,
    task_theme::{parse_task_theme, TaskThemeState},
    vault_links::{LinkResolution, NoteIndex},
};

use super::{
    parse::{self, SnoozeRow, SnoozeRowKind},
    plan::{
        self, AddInput, AddRefusal, DropInput, DropRefusal, FilePostimage,
        TaskFact, WakeInput, WakePlan,
    },
    shell::{self, InputSnapshot, InstallError, LockOutcome},
    state::{
        self, ClassifiedRows, ClassifyInput, ClassifyRow, ResolutionKind,
        RowState, TaskIdentity, TaskState,
    },
    when::{parse_snooze_when, ResolvedTime, SnoozeClock},
};

const SCHEMA_VERSION: u32 = 1;
const COMMAND: &str = "bob snooze";
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(test)]
thread_local! {
    static TEST_CLOCK_SEQUENCE: RefCell<VecDeque<NaiveDateTime>> =
        RefCell::new(VecDeque::new());
}

#[cfg(test)]
fn set_test_clock_sequence(values: impl IntoIterator<Item = NaiveDateTime>) {
    TEST_CLOCK_SEQUENCE
        .with(|clock| *clock.borrow_mut() = values.into_iter().collect());
}

#[cfg(test)]
fn next_test_clock() -> Option<NaiveDateTime> {
    TEST_CLOCK_SEQUENCE.with(|clock| clock.borrow_mut().pop_front())
}

#[cfg(not(test))]
fn next_test_clock() -> Option<NaiveDateTime> {
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Human,
    Json,
}

#[derive(Debug, Clone)]
struct Request {
    bob_dir: PathBuf,
    dry_run: bool,
    format: Format,
    quiet: bool,
}

#[derive(Debug, Clone)]
struct ClockNow {
    clock: SnoozeClock,
    rfc3339: String,
}

#[derive(Debug, Clone)]
struct Day {
    absolute: PathBuf,
    relative: PathBuf,
    date: NaiveDate,
    contents: String,
}

#[derive(Debug, Clone)]
struct ResolvedTask {
    fact: TaskFact,
    title: String,
    off_clock: bool,
}

#[derive(Debug, Clone)]
struct RowDetail {
    parsed: SnoozeRow,
    identity: Option<TaskIdentity>,
    title: Option<String>,
}

#[derive(Debug, Clone)]
struct Evaluation {
    rows: Vec<RowDetail>,
    classified: ClassifiedRows,
    facts: Vec<TaskFact>,
    titles: BTreeMap<TaskIdentity, String>,
    off_clock: BTreeMap<TaskIdentity, bool>,
    identities: plan::IdentityMap,
}

#[derive(Debug)]
struct CliError {
    code: i32,
    kind: &'static str,
    message: String,
}

impl CliError {
    fn runtime(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            kind: "runtime",
            message: message.into(),
        }
    }

    fn refusal(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            code: 2,
            kind,
            message: message.into(),
        }
    }
}

pub(crate) fn build_cli_list() -> ClapCommand {
    base_cli("bob snooze list", "List today's Snoozed queue")
}

pub(crate) fn build_cli_add() -> ClapCommand {
    base_cli("bob snooze add", "Commit a task to today's Snoozed queue")
        .arg(task_arg(true))
        .arg(
            Arg::new("when")
                .value_name("WHEN")
                .required(true)
                .help("Future same-day time, such as 15:30 or +45m"),
        )
}

pub(crate) fn build_cli_drop() -> ClapCommand {
    base_cli(
        "bob snooze drop",
        "Remove a task from today's Snoozed queue",
    )
    .arg(task_arg(true))
}

pub(crate) fn build_cli_wake() -> ClapCommand {
    base_cli("bob snooze wake", "Wake due tasks, or selected tasks now")
        .arg(task_arg(false).num_args(0..))
}

fn base_cli(name: &'static str, about: &'static str) -> ClapCommand {
    ClapCommand::new(name)
        .about(about)
        .long_about(format!(
            "{about}. The read-only list is also the bare `bob snooze` default. Mutations are same-day only; `--dry-run` reports a plan without locking or writing.\n\nSee docs/snooze.md for the row grammar, time rules, and JSON contract."
        ))
        .after_help(
            "Examples:\n  bob snooze list -f json\n  bob snooze add jobs:task 15:30\n  bob snooze drop jobs:task\n  bob snooze wake\n  bob snooze wake jobs:task\n\nEnvironment:\n  BOB_CONFIG_FILE  Exact Bob config file\n  BOB_DAY_FILE     Daily note override\n  BOB_DIR          Bob vault root when --bob-dir is omitted\n  BOB_NOW          Local datetime override for today and the clock\n  NO_COLOR         Disable colored human output",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(quiet_arg())
}

fn task_arg(required: bool) -> Arg {
    Arg::new("task")
        .value_name("TASK")
        .required(required)
        .help("Task locator: note:block-id or [[note#^block-id]]")
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Plan and report without locking or writing")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn quiet_arg() -> Arg {
    Arg::new("quiet")
        .long("quiet")
        .short('q')
        .action(ArgAction::SetTrue)
        .help("Suppress successful human output")
}

pub(crate) fn run_list(args: Vec<OsString>) -> i32 {
    let matches = match matches(build_cli_list(), "bob snooze list", args) {
        Ok(matches) => matches,
        Err(code) => return code,
    };
    let request = Request::from_matches(&matches);
    match list(&request) {
        Ok(result) => {
            print_list(&result, request.format, request.quiet);
            0
        }
        Err(error) => print_error(&error, request.format),
    }
}

pub(crate) fn run_add(args: Vec<OsString>) -> i32 {
    let matches = match matches(build_cli_add(), "bob snooze add", args) {
        Ok(matches) => matches,
        Err(code) => return code,
    };
    let request = Request::from_matches(&matches);
    let task = matches
        .get_one::<String>("task")
        .expect("required by clap")
        .clone();
    let when = matches
        .get_one::<String>("when")
        .expect("required by clap")
        .clone();
    match add(&request, &task, &when) {
        Ok(result) => {
            print_action(&result, request.format, request.quiet);
            0
        }
        Err(error) => print_error(&error, request.format),
    }
}

pub(crate) fn run_drop(args: Vec<OsString>) -> i32 {
    let matches = match matches(build_cli_drop(), "bob snooze drop", args) {
        Ok(matches) => matches,
        Err(code) => return code,
    };
    let request = Request::from_matches(&matches);
    let task = matches
        .get_one::<String>("task")
        .expect("required by clap")
        .clone();
    match drop_task(&request, &task) {
        Ok(result) => {
            print_action(&result, request.format, request.quiet);
            0
        }
        Err(error) => print_error(&error, request.format),
    }
}

pub(crate) fn run_wake(args: Vec<OsString>) -> i32 {
    let matches = match matches(build_cli_wake(), "bob snooze wake", args) {
        Ok(matches) => matches,
        Err(code) => return code,
    };
    let request = Request::from_matches(&matches);
    let tasks = matches
        .get_many::<String>("task")
        .map(|values| values.cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    match wake(&request, &tasks) {
        Ok(result) => {
            print_wake(&result, request.format, request.quiet);
            0
        }
        Err(error) => print_error(&error, request.format),
    }
}

fn matches(
    mut command: ClapCommand,
    name: &'static str,
    args: Vec<OsString>,
) -> Result<ArgMatches, i32> {
    match command
        .try_get_matches_from_mut(iter::once(OsString::from(name)).chain(args))
    {
        Ok(matches) => Ok(matches),
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            Err(code)
        }
    }
}

impl Request {
    fn from_matches(matches: &ArgMatches) -> Self {
        let bob_dir = matches
            .get_one::<OsString>("bob-dir")
            .map(PathBuf::from)
            .map(|path| bob_env::expand_tilde(&path))
            .unwrap_or_else(bob_env::bob_dir);
        Self {
            bob_dir: absolute(&bob_dir),
            dry_run: matches.get_flag("dry-run"),
            format: if matches
                .get_one::<String>("format")
                .is_some_and(|value| value == "json")
            {
                Format::Json
            } else {
                Format::Human
            },
            quiet: matches.get_flag("quiet"),
        }
    }
}

fn list(request: &Request) -> Result<Value, CliError> {
    let snapshot = read_snapshot(&request.bob_dir)?;
    let now = clock_now()?;
    let day = day(&request.bob_dir, &snapshot, &now)?;
    let plan_config = load_plan_config()?;
    let evaluation =
        evaluate(&request.bob_dir, &snapshot, &day, &now, &plan_config)?;
    let count = ledger_count(
        &request.bob_dir,
        &day.relative,
        &day.contents,
        &plan_config,
    );
    let cap = plan_config.max_links as usize;
    let rows = evaluation
        .rows
        .iter()
        .zip(&evaluation.classified.rows)
        .map(|(detail, classified)| row_json(detail, classified))
        .collect::<Vec<_>>();
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "date": now.clock.date.to_string(),
        "now": now.rfc3339,
        "rows": rows,
        "next_wake_at": evaluation.classified.next_wake_at,
        "plan": {"count": count, "cap": cap, "over": count > cap},
    }))
}

fn add(
    request: &Request,
    locator_text: &str,
    when_text: &str,
) -> Result<Value, CliError> {
    let initial_snapshot = read_snapshot(&request.bob_dir)?;
    let initial_now = clock_now()?;
    let initial_day = day(&request.bob_dir, &initial_snapshot, &initial_now)?;
    ensure_today(&initial_day, &initial_now)?;
    let plan_config = load_plan_config()?;
    let initial = add_plan(
        &request.bob_dir,
        &initial_snapshot,
        &initial_day,
        &initial_now,
        &plan_config,
        locator_text,
        when_text,
    )?;
    if request.dry_run {
        return Ok(add_report(
            &initial,
            &initial_day,
            &initial_now,
            &plan_config,
            true,
        ));
    }

    let _lock = match shell::acquire(LOCK_TIMEOUT) {
        Ok(lock) => lock,
        Err(LockOutcome::Deferred) => {
            return Ok(deferred_action(&initial_day, &initial_now));
        }
        Err(LockOutcome::Failed(message)) => {
            return Err(CliError::runtime(message));
        }
    };
    for attempt in 0..2 {
        let snapshot = read_snapshot(&request.bob_dir)?;
        let plan_config = load_plan_config()?;
        let now = clock_now()?;
        let day = day(&request.bob_dir, &snapshot, &now)?;
        if now.clock.date != initial_now.clock.date
            || day.absolute != initial_day.absolute
            || day.relative != initial_day.relative
        {
            return Ok(deferred_action(&day, &now));
        }
        ensure_today(&day, &now)?;
        let plan = add_plan(
            &request.bob_dir,
            &snapshot,
            &day,
            &now,
            &plan_config,
            locator_text,
            when_text,
        )?;
        match shell::install(&request.bob_dir, &plan.files, &snapshot) {
            Ok(()) => {
                return Ok(add_report(&plan, &day, &now, &plan_config, false));
            }
            Err(InstallError::Changed) if attempt == 0 => continue,
            Err(InstallError::Changed) => {
                return Ok(deferred_action(&day, &now));
            }
            Err(InstallError::Failed(message)) => {
                return Err(CliError::runtime(message));
            }
        }
    }
    unreachable!("bounded replan loop always returns")
}

fn drop_task(request: &Request, locator_text: &str) -> Result<Value, CliError> {
    let initial_snapshot = read_snapshot(&request.bob_dir)?;
    let initial_now = clock_now()?;
    let initial_day = day(&request.bob_dir, &initial_snapshot, &initial_now)?;
    ensure_today(&initial_day, &initial_now)?;
    let _initial = drop_plan(&initial_snapshot, &initial_day, locator_text)?;
    if request.dry_run {
        return Ok(drop_report(&initial_day, &initial_now, true));
    }

    let _lock = match shell::acquire(LOCK_TIMEOUT) {
        Ok(lock) => lock,
        Err(LockOutcome::Deferred) => {
            return Ok(deferred_action(&initial_day, &initial_now));
        }
        Err(LockOutcome::Failed(message)) => {
            return Err(CliError::runtime(message));
        }
    };
    for attempt in 0..2 {
        let snapshot = read_snapshot(&request.bob_dir)?;
        let now = clock_now()?;
        let day = day(&request.bob_dir, &snapshot, &now)?;
        if now.clock.date != initial_now.clock.date
            || day.absolute != initial_day.absolute
            || day.relative != initial_day.relative
        {
            return Ok(deferred_action(&day, &now));
        }
        ensure_today(&day, &now)?;
        let plan = drop_plan(&snapshot, &day, locator_text)?;
        let files = [FilePostimage {
            path: day.relative.clone(),
            contents: plan.day,
        }];
        match shell::install(&request.bob_dir, &files, &snapshot) {
            Ok(()) => return Ok(drop_report(&day, &now, false)),
            Err(InstallError::Changed) if attempt == 0 => continue,
            Err(InstallError::Changed) => {
                return Ok(deferred_action(&day, &now));
            }
            Err(InstallError::Failed(message)) => {
                return Err(CliError::runtime(message));
            }
        }
    }
    unreachable!("bounded replan loop always returns")
}

fn wake(request: &Request, task_texts: &[String]) -> Result<Value, CliError> {
    let explicit = !task_texts.is_empty();
    let initial_snapshot = read_snapshot(&request.bob_dir)?;
    let initial_now = clock_now()?;
    let initial_day = day(&request.bob_dir, &initial_snapshot, &initial_now)?;
    ensure_today(&initial_day, &initial_now)?;
    let plan_config = load_plan_config()?;
    let initial_explicit =
        explicit_identities(&initial_snapshot, &initial_day, task_texts)?;
    let initial = wake_plan(
        &request.bob_dir,
        &initial_snapshot,
        &initial_day,
        &initial_now,
        &plan_config,
        initial_explicit.as_deref(),
    )?;
    if let Some(identities) = initial_explicit.as_deref() {
        validate_explicit_wakes(&initial, identities)?;
    }

    if !explicit && !snooze_wake_host_matches()? {
        let count = ledger_count(
            &request.bob_dir,
            &initial_day.relative,
            &initial_day.contents,
            &plan_config,
        );
        return Ok(wake_report(
            "skipped",
            &initial_day,
            &initial_now,
            &initial,
            &initial,
            &plan_config,
            count,
            &BTreeMap::new(),
        ));
    }

    let initial_changed = changed_files(&initial.plan.files, &initial_snapshot);
    if initial_changed.is_empty() {
        let count = ledger_count(
            &request.bob_dir,
            &initial_day.relative,
            &initial_day.contents,
            &plan_config,
        );
        let result = wake_report(
            "nothing_due",
            &initial_day,
            &initial_now,
            &initial,
            &initial,
            &plan_config,
            count,
            &initial.off_clock,
        );
        if explicit && !initial.plan.woken.is_empty() {
            return Ok(result);
        }
        return Ok(result);
    }
    if request.dry_run {
        let count = ledger_count(
            &request.bob_dir,
            &initial_day.relative,
            &initial_day.contents,
            &plan_config,
        );
        return Ok(wake_report(
            if initial.plan.woken.is_empty() {
                "nothing_due"
            } else {
                "woke"
            },
            &initial_day,
            &initial_now,
            &initial,
            &initial,
            &plan_config,
            count,
            &initial.off_clock,
        ));
    }

    let _lock = match shell::acquire(LOCK_TIMEOUT) {
        Ok(lock) => lock,
        Err(LockOutcome::Deferred) => {
            return Ok(wake_report(
                "deferred",
                &initial_day,
                &initial_now,
                &initial,
                &initial,
                &plan_config,
                ledger_count(
                    &request.bob_dir,
                    &initial_day.relative,
                    &initial_day.contents,
                    &plan_config,
                ),
                &initial.off_clock,
            ));
        }
        Err(LockOutcome::Failed(message)) => {
            return Err(CliError::runtime(message));
        }
    };
    for attempt in 0..2 {
        let snapshot = read_snapshot(&request.bob_dir)?;
        let plan_config = load_plan_config()?;
        let now = clock_now()?;
        let day = day(&request.bob_dir, &snapshot, &now)?;
        if now.clock.date != initial_now.clock.date
            || day.absolute != initial_day.absolute
            || day.relative != initial_day.relative
        {
            return Ok(wake_report(
                "deferred",
                &day,
                &now,
                &initial,
                &initial,
                &plan_config,
                ledger_count(
                    &request.bob_dir,
                    &day.relative,
                    &day.contents,
                    &plan_config,
                ),
                &initial.off_clock,
            ));
        }
        if !explicit && !snooze_wake_host_matches()? {
            let count = ledger_count(
                &request.bob_dir,
                &day.relative,
                &day.contents,
                &plan_config,
            );
            return Ok(wake_report(
                "skipped",
                &day,
                &now,
                &initial,
                &initial,
                &plan_config,
                count,
                &BTreeMap::new(),
            ));
        }
        let explicit_identities =
            explicit_identities(&snapshot, &day, task_texts)?;
        let fresh = wake_plan(
            &request.bob_dir,
            &snapshot,
            &day,
            &now,
            &plan_config,
            explicit_identities.as_deref(),
        )?;
        if let Some(identities) = explicit_identities.as_deref() {
            validate_explicit_wakes(&fresh, identities)?;
        }
        if changed_files(&fresh.plan.files, &snapshot).is_empty() {
            let count = ledger_count(
                &request.bob_dir,
                &day.relative,
                &day.contents,
                &plan_config,
            );
            return Ok(wake_report(
                "nothing_due",
                &day,
                &now,
                &fresh,
                &fresh,
                &plan_config,
                count,
                &fresh.off_clock,
            ));
        }
        match shell::install(&request.bob_dir, &fresh.plan.files, &snapshot) {
            Ok(()) => {
                let count = ledger_count(
                    &request.bob_dir,
                    &day.relative,
                    &day.contents,
                    &plan_config,
                );
                return Ok(wake_report(
                    if fresh.plan.woken.is_empty() {
                        "nothing_due"
                    } else {
                        "woke"
                    },
                    &day,
                    &now,
                    &fresh,
                    &fresh,
                    &plan_config,
                    count,
                    &fresh.off_clock,
                ));
            }
            Err(InstallError::Changed) if attempt == 0 => continue,
            Err(InstallError::Changed) => {
                return Ok(wake_report(
                    "deferred",
                    &day,
                    &now,
                    &fresh,
                    &fresh,
                    &plan_config,
                    ledger_count(
                        &request.bob_dir,
                        &day.relative,
                        &day.contents,
                        &plan_config,
                    ),
                    &fresh.off_clock,
                ));
            }
            Err(InstallError::Failed(message)) => {
                return Err(CliError::runtime(message));
            }
        }
    }
    unreachable!("bounded replan loop always returns")
}

struct AddPlanResult {
    files: Vec<FilePostimage>,
    fact: ResolvedTask,
    when: ResolvedTime,
    current_count: usize,
}

fn add_plan(
    root: &Path,
    snapshot: &InputSnapshot,
    day: &Day,
    now: &ClockNow,
    config: &PlanConfig,
    locator_text: &str,
    when_text: &str,
) -> Result<AddPlanResult, CliError> {
    let locator = TaskLocator::parse(locator_text).ok_or_else(|| {
        CliError::refusal(
            "task_not_found",
            "TASK must be note:block-id or [[note#^block-id]]",
        )
    })?;
    let when = parse_snooze_when(when_text, now.clock)
        .map_err(|error| CliError::refusal("invalid_time", error.message()))?;
    let settings = note_tasks::read_settings(root);
    let note_index = snapshot.note_index();
    let (mut identities, _) =
        ledger_identities(&day.contents, &day.relative, &note_index);
    let target_path =
        match note_index.resolve_detailed(Some(&day.relative), &locator.note) {
            LinkResolution::Found(path) => path,
            LinkResolution::Ambiguous => {
                return Err(CliError::refusal(
                    "ambiguous_note",
                    format!("`{}` matches more than one note", locator.note),
                ));
            }
            LinkResolution::Missing => {
                return Err(CliError::refusal(
                    "task_not_found",
                    format!("no note matches `{}`", locator.note),
                ));
            }
        };
    let contents = snapshot.notes.get(&target_path).ok_or_else(|| {
        CliError::runtime(format!(
            "cannot read task note {}",
            target_path.display()
        ))
    })?;
    let scan = note_tasks::scan(contents, &settings);
    let note_task = match scan.by_block_id(&locator.block_id) {
        note_tasks::BlockIdLookup::Found(task) => task.clone(),
        note_tasks::BlockIdLookup::NotATask { line_index, .. } => {
            return Err(CliError::refusal(
                "not_task",
                format!(
                    "^{} in {} is not a #task line (line {})",
                    locator.block_id,
                    target_path.display(),
                    line_index + 1
                ),
            ));
        }
        note_tasks::BlockIdLookup::Duplicate(_) => {
            return Err(CliError::refusal(
                "ambiguous_task",
                format!(
                    "^{} appears more than once in {}",
                    locator.block_id,
                    target_path.display()
                ),
            ));
        }
        note_tasks::BlockIdLookup::Missing => {
            return Err(CliError::refusal(
                "task_not_found",
                format!(
                    "no task ^{} in {}",
                    locator.block_id,
                    target_path.display()
                ),
            ));
        }
    };
    let identity = TaskIdentity {
        route: target_path.clone(),
        block_id: locator.block_id.clone(),
    };
    identities.insert(
        (locator.note.clone(), locator.block_id.clone()),
        identity.clone(),
    );
    identities.insert(
        (route_target(&target_path), locator.block_id.clone()),
        identity.clone(),
    );
    if target_path == day.relative {
        identities.insert(
            (String::new(), locator.block_id.clone()),
            identity.clone(),
        );
    }
    let resolved = task_fact(
        root,
        contents,
        &target_path,
        &day.relative,
        &day.contents,
        &identity,
        &note_task,
        &settings,
        config,
    );
    if resolved.fact.state.is_event {
        return Err(add_refusal(AddRefusal::EventTask));
    }
    if resolved.fact.blocked || resolved.fact.state.status == '?' {
        return Err(add_refusal(AddRefusal::Blocked));
    }
    let plan = plan::plan_add(&AddInput {
        day_path: &day.relative,
        day: &day.contents,
        task: &resolved.fact,
        when: &when,
        clock: now.clock,
        identities: &identities,
    })
    .map_err(add_refusal)?;
    Ok(AddPlanResult {
        files: plan.files,
        fact: resolved,
        when,
        current_count: ledger_count(root, &day.relative, &day.contents, config),
    })
}

fn drop_plan(
    snapshot: &InputSnapshot,
    day: &Day,
    locator_text: &str,
) -> Result<plan::DropPlan, CliError> {
    let locator = TaskLocator::parse(locator_text).ok_or_else(|| {
        CliError::refusal(
            "task_not_found",
            "TASK must be note:block-id or [[note#^block-id]]",
        )
    })?;
    let note_index = snapshot.note_index();
    let target_path =
        match note_index.resolve_detailed(Some(&day.relative), &locator.note) {
            LinkResolution::Found(path) => path,
            LinkResolution::Ambiguous => {
                return Err(CliError::refusal(
                    "ambiguous_note",
                    format!("`{}` matches more than one note", locator.note),
                ))
            }
            LinkResolution::Missing => {
                return Err(CliError::refusal(
                    "task_not_found",
                    format!("no note matches `{}`", locator.note),
                ))
            }
        };
    let identity = TaskIdentity {
        route: target_path,
        block_id: locator.block_id.clone(),
    };
    let (mut identities, _) =
        ledger_identities(&day.contents, &day.relative, &note_index);
    identities
        .insert((locator.note, locator.block_id.clone()), identity.clone());
    identities.insert(
        (route_target(&identity.route), locator.block_id.clone()),
        identity.clone(),
    );
    if identity.route == day.relative {
        identities.insert((String::new(), locator.block_id), identity.clone());
    }
    plan::plan_drop(&DropInput {
        day: &day.contents,
        identity: &identity,
        identities: &identities,
    })
    .map_err(drop_refusal)
}

fn wake_plan(
    root: &Path,
    snapshot: &InputSnapshot,
    day: &Day,
    now: &ClockNow,
    config: &PlanConfig,
    explicit: Option<&[TaskIdentity]>,
) -> Result<WakeEvaluation, CliError> {
    let evaluation = evaluate(root, snapshot, day, now, config)?;
    let plan = plan::plan_wake(&WakeInput {
        day_path: &day.relative,
        day: &day.contents,
        clock: now.clock,
        facts: &evaluation.facts,
        identities: &evaluation.identities,
        exempt: config.exempt(),
        explicit: explicit.map(<[TaskIdentity]>::to_vec),
        link_count: Some(ledger_count(
            root,
            &day.relative,
            &day.contents,
            config,
        )),
        max_links: Some(config.max_links as usize),
    });
    let off_clock = evaluation.off_clock.clone();
    Ok(WakeEvaluation {
        plan,
        rows: evaluation.rows,
        classified: evaluation.classified,
        titles: evaluation.titles,
        off_clock,
    })
}

fn explicit_identities(
    snapshot: &InputSnapshot,
    day: &Day,
    task_texts: &[String],
) -> Result<Option<Vec<TaskIdentity>>, CliError> {
    if task_texts.is_empty() {
        return Ok(None);
    }
    let note_index = snapshot.note_index();
    task_texts
        .iter()
        .map(|text| {
            let locator = TaskLocator::parse(text).ok_or_else(|| {
                CliError::refusal(
                    "task_not_found",
                    format!(
                        "`{text}` is not note:block-id or [[note#^block-id]]"
                    ),
                )
            })?;
            let route = match note_index
                .resolve_detailed(Some(&day.relative), &locator.note)
            {
                LinkResolution::Found(path) => path,
                LinkResolution::Ambiguous => {
                    return Err(CliError::refusal(
                        "ambiguous_note",
                        format!(
                            "`{}` matches more than one note",
                            locator.note
                        ),
                    ));
                }
                LinkResolution::Missing => {
                    return Err(CliError::refusal(
                        "task_not_found",
                        format!("no note matches `{}`", locator.note),
                    ));
                }
            };
            Ok(TaskIdentity {
                route,
                block_id: locator.block_id,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn snooze_wake_host_matches() -> Result<bool, CliError> {
    let (host, writer) =
        crate::native::tick::snooze_writer().map_err(CliError::runtime)?;
    Ok(!writer.as_ref().is_some_and(|writer| writer != &host))
}

#[derive(Debug, Clone)]
struct WakeEvaluation {
    plan: WakePlan,
    rows: Vec<RowDetail>,
    classified: ClassifiedRows,
    titles: BTreeMap<TaskIdentity, String>,
    off_clock: BTreeMap<TaskIdentity, bool>,
}

fn validate_explicit_wakes(
    evaluation: &WakeEvaluation,
    identities: &[TaskIdentity],
) -> Result<(), CliError> {
    for identity in identities {
        let matching = evaluation
            .rows
            .iter()
            .zip(&evaluation.classified.rows)
            .filter(|(detail, _)| detail.identity.as_ref() == Some(identity));
        let Some((_, row)) = matching.clone().find(|(_, row)| {
            matches!(row.state, RowState::Asleep | RowState::Due)
        }) else {
            let Some((_, row)) = matching.into_iter().next() else {
                return Err(CliError::refusal(
                    "not_snoozed",
                    format!("{} has no live Snoozed row", identity.key()),
                ));
            };
            match row.state {
                RowState::Blocked => {
                    return Err(CliError::refusal(
                        "blocked",
                        format!("{} is Blocked; snooze waits on the clock, not prerequisites", identity.key()),
                    ));
                }
                RowState::Ignored(reason) => {
                    return Err(CliError::refusal(
                        reason.as_str(),
                        format!(
                            "{} cannot wake: {}",
                            identity.key(),
                            reason.as_str()
                        ),
                    ));
                }
                RowState::History => {
                    return Err(CliError::refusal(
                        "not_snoozed",
                        format!("{} is already woken", identity.key()),
                    ));
                }
                RowState::Asleep | RowState::Due => unreachable!(),
            }
        };
        let _ = row;
    }
    Ok(())
}

fn evaluate(
    root: &Path,
    snapshot: &InputSnapshot,
    day: &Day,
    now: &ClockNow,
    config: &PlanConfig,
) -> Result<Evaluation, CliError> {
    let lines: Vec<&str> = day.contents.lines().collect();
    let section = parse::parse_section(&lines);
    let note_index = snapshot.note_index();
    let settings = note_tasks::read_settings(root);
    let (mut identities, live_links) =
        ledger_identities(&day.contents, &day.relative, &note_index);
    let mut details = Vec::new();
    let mut facts_by_identity = BTreeMap::<TaskIdentity, ResolvedTask>::new();

    if let Some(section) = &section {
        for row in &section.rows {
            let mut identity = None;
            let mut title = None;
            if let (Some(target), Some(block_id)) =
                (row.target.as_deref(), row.block_id.as_deref())
            {
                match note_index.resolve_detailed(Some(&day.relative), target) {
                    LinkResolution::Found(path) => {
                        let resolved_identity = TaskIdentity {
                            route: path.clone(),
                            block_id: block_id.to_string(),
                        };
                        identities.insert(
                            (target.to_string(), block_id.to_string()),
                            resolved_identity.clone(),
                        );
                        identity = Some(resolved_identity.clone());
                        if let Some(contents) = snapshot.notes.get(&path) {
                            let scan = note_tasks::scan(contents, &settings);
                            match scan.by_block_id(block_id) {
                                note_tasks::BlockIdLookup::Found(task)
                                    if task.status_type
                                        != TaskStatusType::NonTask =>
                                {
                                    let resolved = task_fact(
                                        root,
                                        contents,
                                        &path,
                                        &day.relative,
                                        &day.contents,
                                        &resolved_identity,
                                        task,
                                        &settings,
                                        config,
                                    );
                                    title = Some(resolved.title.clone());
                                    facts_by_identity
                                        .entry(resolved_identity)
                                        .or_insert(resolved);
                                }
                                _ => {}
                            }
                        }
                    }
                    LinkResolution::Ambiguous | LinkResolution::Missing => {}
                }
            }
            details.push(RowDetail {
                parsed: row.clone(),
                identity,
                title,
            });
        }
    }

    let mut classify_rows = Vec::with_capacity(details.len());
    for detail in &details {
        let row = &detail.parsed;
        let is_history = row.kind == SnoozeRowKind::History;
        let malformed = row.kind == SnoozeRowKind::Malformed;
        let (resolution, task) = if malformed {
            (ResolutionKind::Missing, None)
        } else if let (Some(identity), Some(block_id)) =
            (&detail.identity, row.block_id.as_deref())
        {
            if let Some(contents) = snapshot.notes.get(&identity.route) {
                match note_tasks::scan(contents, &settings)
                    .by_block_id(block_id)
                {
                    note_tasks::BlockIdLookup::Found(task)
                        if task.status_type != TaskStatusType::NonTask =>
                    {
                        let is_event = event_task::event_task_at(
                            &identity.route,
                            contents,
                            task.line_index,
                        )
                        .is_some();
                        let status = normalized_status(
                            task.status_symbol,
                            task.status_type,
                            &settings.done_statuses,
                        );
                        (
                            ResolutionKind::Found,
                            Some(TaskState {
                                status,
                                is_task: true,
                                is_event,
                            }),
                        )
                    }
                    note_tasks::BlockIdLookup::Found(task) => (
                        ResolutionKind::Found,
                        Some(TaskState {
                            status: task.status_symbol,
                            is_task: false,
                            is_event: false,
                        }),
                    ),
                    note_tasks::BlockIdLookup::NotATask { .. } => (
                        ResolutionKind::Found,
                        Some(TaskState {
                            status: ' ',
                            is_task: false,
                            is_event: false,
                        }),
                    ),
                    note_tasks::BlockIdLookup::Duplicate(_) => {
                        (ResolutionKind::Ambiguous, None)
                    }
                    note_tasks::BlockIdLookup::Missing => {
                        (ResolutionKind::Missing, None)
                    }
                }
            } else {
                (ResolutionKind::Missing, None)
            }
        } else if row.target.is_some() && row.block_id.is_some() {
            let target = row.target.as_deref().unwrap_or("");
            match note_index.resolve_detailed(Some(&day.relative), target) {
                LinkResolution::Ambiguous => (ResolutionKind::Ambiguous, None),
                _ => (ResolutionKind::Missing, None),
            }
        } else {
            (ResolutionKind::Missing, None)
        };
        let has_live_link = detail
            .identity
            .as_ref()
            .is_some_and(|identity| live_links.contains(identity));
        classify_rows.push(ClassifyRow {
            line: row.line,
            minutes: row.minutes,
            is_history,
            malformed,
            resolution,
            identity: detail.identity.clone(),
            task,
            has_live_link,
        });
    }
    let classified = state::classify_rows(&ClassifyInput {
        today: now.clock.date,
        note_date: day.date,
        now_minutes: now.clock.now_minutes,
        zone: now.clock.zone,
        rows: classify_rows,
    });

    let mut off_clock = BTreeMap::new();
    let mut titles = BTreeMap::new();
    let mut facts = Vec::new();
    for (identity, resolved) in facts_by_identity {
        off_clock.insert(identity.clone(), resolved.off_clock);
        titles.insert(identity.clone(), resolved.title.clone());
        facts.push(resolved.fact);
    }
    Ok(Evaluation {
        rows: details,
        classified,
        facts,
        titles,
        off_clock,
        identities,
    })
}

fn ledger_identities(
    day: &str,
    day_path: &Path,
    note_index: &NoteIndex,
) -> (plan::IdentityMap, BTreeSet<TaskIdentity>) {
    let mut identities = BTreeMap::new();
    let mut live = BTreeSet::new();
    let lines = line_spans(day);
    let line_text = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let Some(section) = pomodoro::pomodoros_section_range(&line_text) else {
        return (identities, live);
    };
    let fenced = markdown::fenced_lines(&line_text, section.clone());
    for entry in capture_pomodoros::scan(day).entries {
        if entry.state != PomodoroState::Open {
            continue;
        }
        let entry_index = entry.line.saturating_sub(1);
        let range = capture_pomodoros::pomodoro_block_range(
            &line_text,
            entry_index,
            section.end,
        );
        for index in range.start.saturating_add(1)..range.end {
            if fenced.contains(&index)
                || nearest_shallower_list_item_parent(&lines, index)
                    != Some(entry_index)
            {
                continue;
            }
            let text = line_text[index];
            for span in block_link_spans(text) {
                if span.struck {
                    continue;
                }
                let LinkResolution::Found(path) =
                    note_index.resolve_detailed(Some(day_path), &span.target)
                else {
                    continue;
                };
                let identity = TaskIdentity {
                    route: path,
                    block_id: span.block_id.clone(),
                };
                identities
                    .insert((span.target, span.block_id), identity.clone());
                live.insert(identity);
            }
        }
    }
    (identities, live)
}

fn task_fact(
    root: &Path,
    contents: &str,
    path: &Path,
    day_path: &Path,
    day_contents: &str,
    identity: &TaskIdentity,
    task: &note_tasks::NoteTask,
    settings: &note_tasks::NoteTaskSettings,
    config: &PlanConfig,
) -> ResolvedTask {
    let lines = line_spans(contents);
    let line = lines
        .get(task.line_index)
        .map(|line| line.text)
        .unwrap_or("");
    let is_event =
        event_task::event_task_at(path, contents, task.line_index).is_some();
    let status = normalized_status(
        task.status_symbol,
        task.status_type,
        &settings.done_statuses,
    );
    let mut type_resolver = HomeResolver::new(DiskHomeSource::new(root));
    let type_key = pomodoro_themes::suggest::effective_type_key(
        &mut type_resolver,
        config.reading_theme(),
        config.default_type(),
        path,
        line,
    );
    let type_name = match parse_task_theme(line) {
        TaskThemeState::Set { value, .. } => Some(value),
        _ => type_key.as_ref().map(|key| key.to_ascii_uppercase()),
    };
    let blocked = status == '?';
    let target = if path == day_path {
        String::new()
    } else {
        route_target(path)
    };
    let link_text = if target.is_empty() {
        format!("[[#^{}]]", identity.block_id)
    } else {
        format!("[[{}#^{}]]", target, identity.block_id)
    };
    let fact = TaskFact {
        identity: identity.clone(),
        path: path.to_path_buf(),
        contents: contents.to_string(),
        line_index: task.line_index,
        state: TaskState {
            status,
            is_task: task.status_type != TaskStatusType::NonTask,
            is_event,
        },
        blocked,
        type_key,
        type_name,
        link_text,
    };
    let disk_vault = DiskCloseVault::new(root.to_path_buf());
    let mut context =
        crate::native::capture_pomodoro_start::LinkClockContext::new(
            Some(&disk_vault),
            day_path,
            day_contents,
        );
    let off_clock = context
        .resolve(&target, &identity.block_id, false)
        .fact
        .off_clock
        == Some(true);
    ResolvedTask {
        fact,
        title: task.description.clone(),
        off_clock,
    }
}

fn normalized_status(
    status: char,
    kind: TaskStatusType,
    done: &BTreeSet<char>,
) -> char {
    match kind {
        TaskStatusType::Done => 'x',
        TaskStatusType::Cancelled => '-',
        // Bob's Tasks config uses ON_HOLD for both Next (`*`) and
        // Blocked (`?`); only the actual blocked symbol is blocked.
        TaskStatusType::OnHold => status,
        TaskStatusType::NonTask | TaskStatusType::Empty => status,
        TaskStatusType::Todo | TaskStatusType::InProgress => {
            if done.contains(&status) {
                'x'
            } else {
                status
            }
        }
    }
}

fn route_target(path: &Path) -> String {
    path.with_extension("").to_string_lossy().replace('\\', "/")
}

fn add_refusal(refusal: AddRefusal) -> CliError {
    let message = match refusal {
        AddRefusal::EventTask => "Event Tasks wait on their own clock",
        AddRefusal::Blocked => "Blocked by prerequisites — snooze waits on the clock, not prerequisites",
        AddRefusal::Done => "completed tasks cannot be snoozed",
        AddRefusal::Cancelled => "cancelled tasks cannot be snoozed",
        AddRefusal::NotTask => "TASK must resolve to one #task",
        AddRefusal::NoPomodorosSection => "today's note has no ## Pomodoros section",
    };
    CliError::refusal(refusal.as_str(), message)
}

fn drop_refusal(refusal: DropRefusal) -> CliError {
    match refusal {
        DropRefusal::NotSnoozed => {
            CliError::refusal("not_snoozed", "TASK has no active Snoozed row")
        }
    }
}

fn add_report(
    plan: &AddPlanResult,
    day: &Day,
    now: &ClockNow,
    config: &PlanConfig,
    dry_run: bool,
) -> Value {
    let cap = config.max_links as usize;
    let count = plan.current_count;
    json!({
        "schema_version": SCHEMA_VERSION,
        "date": now.clock.date.to_string(),
        "now": now.rfc3339,
        "status": if dry_run { "planned" } else { "added" },
        "task": {"route": plan.fact.fact.identity.route, "block_id": plan.fact.fact.identity.block_id, "title": plan.fact.title},
        "target": {"time": format!("{}:{}", &plan.when.hhmm[..2], &plan.when.hhmm[2..]), "at": plan.when.rfc3339},
        "lane": if plan.fact.fact.state.status == ' ' { "next" } else { "unchanged" },
        "daily_file": day.relative,
        "plan": {"count": count, "cap": cap, "over": count > cap},
        "dry_run": dry_run,
    })
}

fn drop_report(day: &Day, now: &ClockNow, dry_run: bool) -> Value {
    json!({"schema_version":SCHEMA_VERSION,"date":now.clock.date.to_string(),"now":now.rfc3339,"status":if dry_run {"planned"} else {"dropped"},"daily_file":day.relative,"dry_run":dry_run})
}

fn deferred_action(day: &Day, now: &ClockNow) -> Value {
    json!({"schema_version":SCHEMA_VERSION,"date":now.clock.date.to_string(),"now":now.rfc3339,"status":"deferred","daily_file":day.relative,"notified":"none","wake_id":Value::Null})
}

fn wake_report(
    status: &str,
    day: &Day,
    now: &ClockNow,
    plan: &WakeEvaluation,
    _original: &WakeEvaluation,
    config: &PlanConfig,
    current_count: usize,
    off_clock: &BTreeMap<TaskIdentity, bool>,
) -> Value {
    let mut count = current_count;
    let mut woken = Vec::new();
    for entry in &plan.plan.woken {
        let is_off_clock =
            off_clock.get(&entry.identity).copied().unwrap_or(false);
        if !is_off_clock {
            count += 1;
        }
        woken.push(json!({
            "route": entry.identity.route,
            "block_id": entry.identity.block_id,
            "title": plan.titles.get(&entry.identity).cloned().unwrap_or_else(|| entry.identity.block_id.clone()),
            "target": entry.target,
            "woke_at": entry.woke_at,
            "destination": {"line": entry.destination.line, "name": entry.destination.name, "created": entry.destination.created},
            "over_cap": count > config.max_links as usize,
        }));
    }
    let dropped = reasoned_json(&plan.plan.dropped, plan);
    let ignored = reasoned_json(&plan.plan.ignored, plan);
    let pending = plan.plan.pending.iter().map(|row| json!({
        "route": row.identity.route,
        "block_id": row.identity.block_id,
        "target": plan.rows.iter().find(|detail| detail.identity.as_ref() == Some(&row.identity)).and_then(|detail| detail.parsed.hhmm.clone()),
        "state": row.state,
    })).collect::<Vec<_>>();
    json!({
        "schema_version": SCHEMA_VERSION,
        "date": now.clock.date.to_string(),
        "now": now.rfc3339,
        "status": status,
        "woken": woken,
        "dropped": dropped,
        "ignored": ignored,
        "pending": pending,
        "next_wake_at": if plan.plan.woken.is_empty() { plan.classified.next_wake_at.clone() } else { next_wake_after(plan) },
        "plan": {"count": count, "cap": config.max_links, "over": count > config.max_links as usize},
        "notified": "none",
        "wake_id": Value::Null,
        "daily_file": day.relative,
    })
}

fn next_wake_after(plan: &WakeEvaluation) -> Option<String> {
    // The postimage's pending set is the authoritative survivor list. The
    // planner already recalculated the forecast after its own write.
    plan.plan.next_wake_at.clone()
}

fn reasoned_json(
    rows: &[plan::ReasonedRow],
    evaluation: &WakeEvaluation,
) -> Vec<Value> {
    rows.iter().map(|reasoned| {
        let detail = evaluation.rows.iter().find(|detail| detail.identity == reasoned.identity);
        json!({
            "route": reasoned.identity.as_ref().map(|identity| identity.route.to_string_lossy().into_owned()).or_else(|| detail.and_then(|detail| detail.parsed.target.clone())),
            "block_id": reasoned.identity.as_ref().map(|identity| identity.block_id.clone()).or_else(|| detail.and_then(|detail| detail.parsed.block_id.clone())),
            "target": detail.and_then(|detail| detail.parsed.hhmm.clone()),
            "reason": reasoned.reason,
        })
    }).collect()
}

fn row_json(detail: &RowDetail, classified: &state::ClassifiedRow) -> Value {
    let (state_name, reason) = match classified.state {
        RowState::Ignored(reason) => ("ignored", Some(reason.as_str())),
        RowState::Asleep => ("asleep", None),
        RowState::Due => ("due", None),
        RowState::Blocked => ("blocked", None),
        RowState::History => ("woken", None),
    };
    json!({
        "line": detail.parsed.line,
        "route": detail.identity.as_ref().map(|identity| identity.route.to_string_lossy().into_owned()).or_else(|| detail.parsed.target.clone()),
        "block_id": detail.parsed.block_id,
        "title": detail.title,
        "target": detail.parsed.hhmm,
        "state": state_name,
        "reason": reason,
        "woke_at": detail.parsed.woke_hhmm,
    })
}

fn ledger_count(
    root: &Path,
    day_path: &Path,
    contents: &str,
    config: &PlanConfig,
) -> usize {
    let vault = DiskCloseVault::new(root.to_path_buf());
    let key = day_path.to_string_lossy();
    plan_budget::off_clock::compute_for_daily_in_vault(
        &vault,
        day_path,
        contents,
        config,
        Some(&key),
    )
    .links
    .count
}

fn changed_files(
    files: &[FilePostimage],
    snapshot: &InputSnapshot,
) -> Vec<FilePostimage> {
    files
        .iter()
        .filter(|file| snapshot.notes.get(&file.path) != Some(&file.contents))
        .cloned()
        .collect()
}

fn read_snapshot(root: &Path) -> Result<InputSnapshot, CliError> {
    InputSnapshot::read(root).map_err(|error| {
        CliError::runtime(format!("read vault snapshot: {error}"))
    })
}

fn load_plan_config() -> Result<PlanConfig, CliError> {
    config::load_plan_config(&config::config_path())
        .map_err(|error| CliError::runtime(error.to_string()))
}

fn day(
    root: &Path,
    snapshot: &InputSnapshot,
    now: &ClockNow,
) -> Result<Day, CliError> {
    let configured = pomodoro::day_file_for(root);
    let absolute = if configured.is_absolute() {
        configured
    } else {
        root.join(configured)
    };
    let relative = absolute
        .strip_prefix(root)
        .map_err(|_| {
            CliError::refusal(
                "day_file_outside_vault",
                "BOB_DAY_FILE must be inside the Bob vault",
            )
        })?
        .to_path_buf();
    let date = absolute
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(pomodoro::parse_day_file_date)
        .unwrap_or(now.clock.date);
    let contents = snapshot.notes.get(&relative).cloned().unwrap_or_default();
    Ok(Day {
        absolute,
        relative,
        date,
        contents,
    })
}

fn ensure_today(day: &Day, now: &ClockNow) -> Result<(), CliError> {
    if day.date != now.clock.date {
        return Err(CliError::refusal(
            "not_today",
            "snooze rows are for today's daily note only",
        ));
    }
    Ok(())
}

fn clock_now() -> Result<ClockNow, CliError> {
    let wall = next_test_clock().unwrap_or_else(bob_env::current_datetime);
    let zone = bob_env::configured_timezone()
        .or_else(host_timezone)
        .ok_or_else(|| {
            CliError::runtime(
                "cannot identify the host timezone; set timezone in Bob's config",
            )
        })?;
    let now_minutes = (wall.hour() * 60 + wall.minute()) as u16;
    let local = zone.from_local_datetime(&wall);
    let rfc3339 = match local {
        chrono::LocalResult::Single(value) => value.to_rfc3339(),
        chrono::LocalResult::Ambiguous(first, second) => {
            if first.timestamp() <= second.timestamp() {
                first.to_rfc3339()
            } else {
                second.to_rfc3339()
            }
        }
        chrono::LocalResult::None => wall.and_utc().to_rfc3339(),
    };
    Ok(ClockNow {
        clock: SnoozeClock {
            date: wall.date(),
            now_minutes,
            zone,
        },
        rfc3339,
    })
}

fn host_timezone() -> Option<Tz> {
    if let Some(zone) = bob_env::var("TZ")
        .ok()
        .and_then(|name| name.parse::<Tz>().ok())
    {
        return Some(zone);
    }
    if let Ok(name) = fs::read_to_string("/etc/timezone")
        && let Ok(zone) = name.trim().parse::<Tz>()
    {
        return Some(zone);
    }
    let localtime = fs::read_link("/etc/localtime").ok()?;
    let path = localtime.to_string_lossy();
    let (_, name) = path.split_once("/zoneinfo/")?;
    name.parse::<Tz>().ok()
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}

fn print_list(value: &Value, format: Format, quiet: bool) {
    if format == Format::Json {
        println!("{value}");
    } else if !quiet {
        println!("{}", human_list(value, &Styler::detect()));
    }
}

fn human_list(value: &Value, styler: &Styler) -> String {
    let rows = value["rows"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let count =
        |state: &str| rows.iter().filter(|row| row["state"] == state).count();
    let asleep = count("asleep");
    let due = count("due");
    let blocked = count("blocked");
    let woken = count("woken");
    let mut header = Vec::new();
    if asleep > 0 {
        header.push(format!("{asleep} asleep"));
    }
    if due > 0 {
        header.push(format!("{} due", due));
    }
    if blocked > 0 {
        header.push(format!("{blocked} blocked"));
    }
    if woken > 0 {
        header.push(format!("{woken} woke"));
    }
    let next = value["next_wake_at"]
        .as_str()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.format("%H:%M").to_string());
    if let Some(next) = next {
        header.push(format!("next {next}"));
    }
    let mut out = format!(
        "{}\n",
        if header.is_empty() {
            "No snoozed tasks".to_string()
        } else {
            header.join(" · ")
        }
    );
    for row in rows {
        let target = row["target"].as_str().unwrap_or("----");
        let time = if target.len() == 4 {
            format!("{}:{}", &target[..2], &target[2..])
        } else {
            "----".into()
        };
        let title = row["title"]
            .as_str()
            .or_else(|| row["route"].as_str())
            .unwrap_or("unresolved task");
        let state = row["state"].as_str().unwrap_or("ignored");
        let line = match state {
            "asleep" => format!("{time}  {title}"),
            "due" => format!("{time}  {title} · due"),
            "blocked" => format!("{time}  {title} · waits on prerequisites"),
            "woken" => format!(
                "{title} · woke {}",
                row["woke_at"].as_str().unwrap_or("--:--")
            ),
            _ => format!(
                "{time}  {title} · ignored ({})",
                row["reason"].as_str().unwrap_or(state)
            ),
        };
        let painted = match state {
            "asleep" => styler.paint("38;5;141", &line),
            "due" => styler.paint("38;5;214;1", &line),
            "blocked" | "woken" => styler.dim(&line),
            _ => styler.dim(&line),
        };
        out.push_str("  ");
        out.push_str(&painted);
        out.push('\n');
    }
    out
}

fn print_action(value: &Value, format: Format, quiet: bool) {
    if format == Format::Json {
        println!("{value}");
    } else if !quiet {
        match value["status"].as_str() {
            Some("deferred") => {
                println!("snooze deferred; retry on the next run")
            }
            Some("planned") => {
                println!("{}", Styler::detect().yellow("[dry-run] planned"))
            }
            Some("added") => println!(
                "snoozed {} until {}",
                value["task"]["title"].as_str().unwrap_or("task"),
                value["target"]["time"].as_str().unwrap_or("")
            ),
            Some("dropped") => {
                println!("removed task from today's Snoozed queue")
            }
            _ => println!("{value}"),
        }
    }
}

fn print_wake(value: &Value, format: Format, quiet: bool) {
    if format == Format::Json {
        println!("{value}");
        return;
    }
    if quiet {
        return;
    }
    let status = value["status"].as_str().unwrap_or("nothing_due");
    match status {
        "woke" => {
            let count = value["woken"].as_array().map_or(0, Vec::len);
            println!(
                "woke {count} snoozed task{}",
                if count == 1 { "" } else { "s" }
            );
            for row in
                value["woken"].as_array().map(Vec::as_slice).unwrap_or(&[])
            {
                println!(
                    "  {} · woke {} → {}",
                    row["title"].as_str().unwrap_or("task"),
                    row["woke_at"].as_str().unwrap_or(""),
                    row["destination"]["name"].as_str().unwrap_or("(unnamed)")
                );
            }
        }
        "deferred" => println!("snooze deferred; retry on the next run"),
        "skipped" => println!("snooze wake skipped on this host"),
        _ => println!("nothing due"),
    }
}

fn print_error(error: &CliError, format: Format) -> i32 {
    if format == Format::Json {
        println!(
            "{}",
            json!({"ok":false,"schema_version":SCHEMA_VERSION,"error":{"kind":error.kind,"message":error.message}})
        );
    } else {
        eprintln!("{COMMAND}: {}", error.message);
    }
    error.code
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsStr, fs, path::PathBuf};

    const DAY_BEFORE: &str = "# 2026-10-08\n\n## Pomodoros\n\n## Tasks\n";
    const TASK_BEFORE: &str = "- [ ] #task Review the launch plan ^job\n";

    fn add_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("vault");
        let day = root.join("2026/20261008.md");
        let task = root.join("jobs.md");
        let config = temp.path().join("bob.yml");
        fs::create_dir_all(day.parent().unwrap()).expect("daily directory");
        fs::create_dir_all(
            root.join(".obsidian/plugins/obsidian-tasks-plugin"),
        )
        .expect("Tasks settings directory");
        fs::write(&day, DAY_BEFORE).expect("write day");
        fs::write(&task, TASK_BEFORE).expect("write task");
        fs::write(&config, "timezone: America/New_York\n").expect("config");
        fs::write(
            root.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            r##"{"globalFilter":"#task","statusSettings":{"coreStatuses":[{"symbol":" ","name":"Todo","type":"TODO"},{"symbol":"x","name":"Done","type":"DONE"}],"customStatuses":[{"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},{"symbol":"*","name":"Next","type":"ON_HOLD"},{"symbol":"?","name":"Blocked","type":"ON_HOLD"},{"symbol":"-","name":"Canceled","type":"CANCELLED"}]}}"##,
        )
        .expect("Tasks settings");
        let dependency = root.join("type.md");
        fs::write(&dependency, "type resolver input\n").expect("dependency");
        (temp, root, day, task)
    }

    fn add_env(
        root: &Path,
        temp: &tempfile::TempDir,
        day: &Path,
    ) -> crate::native::env::TestEnvGuard {
        let config = temp.path().join("bob.yml");
        let lock = temp.path().join("vault.lock");
        crate::native::env::TestEnvGuard::set(&[
            ("BOB_DIR", Some(root.as_os_str())),
            ("BOB_DAY_FILE", Some(day.as_os_str())),
            ("BOB_CONFIG_FILE", Some(config.as_os_str())),
            ("BOB_NOW", Some(OsStr::new("2026-10-08T12:00:00-04:00"))),
            ("BOB_VAULT_SYNC_LOCK_FILE", Some(lock.as_os_str())),
        ])
    }

    fn add_request(root: &Path) -> Request {
        Request {
            bob_dir: root.to_path_buf(),
            dry_run: false,
            format: Format::Json,
            quiet: true,
        }
    }

    fn date(value: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
            .expect("test time")
    }

    #[test]
    fn add_refreshes_read_inputs_after_one_preimage_drift() {
        let (temp, root, day, task) = add_fixture();
        let _env = add_env(&root, &temp, &day);
        let dependency = root.join("type.md");
        shell::before_next_validation({
            let dependency = dependency.clone();
            move || fs::write(dependency, "first drift\n").expect("drift")
        });

        let result = add(&add_request(&root), "jobs:job", "15:30")
            .expect("one drift is refreshed and committed");
        assert_eq!(result["status"], "added");
        assert!(fs::read_to_string(day).unwrap().contains("- **1530**"));
        assert!(fs::read_to_string(task).unwrap().contains("[*]"));
    }

    #[test]
    fn add_defers_after_a_second_preimage_drift_without_partial_write() {
        let (temp, root, day, task) = add_fixture();
        let _env = add_env(&root, &temp, &day);
        let dependency = root.join("type.md");
        shell::before_next_validation({
            let dependency = dependency.clone();
            move || {
                fs::write(&dependency, "first drift\n").expect("first drift");
                shell::before_next_validation(move || {
                    fs::write(dependency, "second drift\n")
                        .expect("second drift");
                });
            }
        });

        let result = add(&add_request(&root), "jobs:job", "15:30")
            .expect("second drift defers");
        assert_eq!(result["status"], "deferred");
        assert_eq!(fs::read_to_string(day).unwrap(), DAY_BEFORE);
        assert_eq!(fs::read_to_string(task).unwrap(), TASK_BEFORE);
    }

    #[test]
    fn add_defers_when_the_day_rolls_over_before_write() {
        let (temp, root, day, task) = add_fixture();
        let _env = add_env(&root, &temp, &day);
        set_test_clock_sequence([
            date("2026-10-08T12:00:00"),
            date("2026-10-09T00:00:01"),
        ]);

        let result = add(&add_request(&root), "jobs:job", "15:30")
            .expect("rollover defers");
        assert_eq!(result["status"], "deferred");
        assert_eq!(fs::read_to_string(day).unwrap(), DAY_BEFORE);
        assert_eq!(fs::read_to_string(task).unwrap(), TASK_BEFORE);
    }

    #[test]
    fn failed_second_file_install_rolls_back_the_daily_note() {
        let (temp, root, day, task) = add_fixture();
        let _env = add_env(&root, &temp, &day);
        crate::native::capture::test_fail_rename_at(Some(&task));
        let result = add(&add_request(&root), "jobs:job", "15:30");
        crate::native::capture::test_fail_rename_at(None);

        assert!(
            matches!(result, Err(CliError { code: 1, .. })),
            "{result:?}"
        );
        assert_eq!(fs::read_to_string(day).unwrap(), DAY_BEFORE);
        assert_eq!(fs::read_to_string(task).unwrap(), TASK_BEFORE);
    }
}
