use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::{OsStr, OsString},
    fs, io, iter,
    ops::Range,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use chrono::{Datelike, NaiveDate};
use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::{json, Value};

const POMODORO_MARKER: &str = "🍅";

use super::{
    capture_pomodoros, collect_done, config as bob_config, env as bob_env,
    is_always_excluded_note_directory_name, plan_budget,
    pomodoro as native_pomodoro, projects,
    style::{display_width, pad_right, Styler},
    task_status_groups::{
        self, GroupedSection, GroupingWarning, TaskClassification,
    },
    task_status_hooks_write::{
        acquire_maintenance_lock, apply_plan, capture_optional,
        capture_required, new_run_id, planned_write, snapshot_for_path,
        ApplyError, ApplyOutcome, ApplySession, CaptureError, InputKind,
        InputSnapshot, WritePlan,
    },
    vault_links::{target_to_markdown_path, NoteIndex},
};

const COMMAND_NAME: &str = "bob task-status-hooks";
const DEFAULT_GLOBAL_FILTER: &str = "#task";
const TASKS_SETTINGS: &str =
    ".obsidian/plugins/obsidian-tasks-plugin/data.json";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let format = OutputFormat::from_matches(&matches);
    let request = Request::from_matches(&matches);
    let outcome = if request.dry_run || request.retry_timeout.is_zero() {
        sync_task_statuses(&request)
    } else {
        run_with_retries(&request, &RetryEnv::production(format))
    };
    match outcome {
        Ok(result) => {
            print_result(&result, format);
            0
        }
        Err(error) => print_error(error, format),
    }
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

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Sync active and derived-Blocked task statuses")
        .long_about(
            "Make the current Pomodoro ledger the source of truth for active task statuses and use the latest existing earlier daily note as a read-only recent-activity source.\n\n\
Tasks block-linked from child bullets of open Pomodoro entries have a minimum \
desired status of Next [*]; tasks already In Progress [/] keep that stronger \
status. Dependency tasks are discovered recursively from sole transcluded \
block-link child bullets and inherit the strongest effective parent status, \
promoting Ready [ ] tasks to Next or In Progress and Next tasks to In Progress. \
Status propagation never lowers a task, and removing a link never changes a \
lane: unlinked Next [*] and In Progress [/] tasks stay as they are. Only a \
daily-note Next task still clears: a [*] that lives in a canonical daily note \
or the selected current ledger and is not reachable from an open entry is reset \
to [ ], unless it is directly referenced by recent activity, in which case it is \
kept. Historical links supply recovery-only rank for Blocked tasks without \
promoting Ready tasks; a recovered directly referenced Next task stays Next \
while it remains recent, and the historical note is never modified. Tasks whose Dataview \
[dependsOn:: ...] metadata names an open \
vault-wide [id:: ...] task, or whose task-level [scheduled:: YYYY-MM-DD] date \
is later than the effective daily anchor, are marked Blocked [?], overriding \
Ready, Next, and In Progress. A schedule on the anchor date or earlier is not \
future. When every derived blocking reason clears, Blocked tasks recover to \
Next when directly recent, In Progress only through a stronger eligible \
transclusion path, or Ready when unreachable. No pre-Blocked status is stored. \
Project scheduled frontmatter is not task-level metadata and is ignored by \
this rule. \
Completed linked-task references \
are retired as struck, non-embedded links. References found under open \
Pomodoros keep the existing policy of moving their containing bullets beneath \
the current timed Pomodoro, or the last completed Pomodoro when \
there is no current one. A link to an unambiguously cancelled Tasks task \
removes its complete Markdown list-item subtree from an open Pomodoro, \
including for custom single-character statuses whose Tasks type is CANCELLED; \
the cancelled task status itself is left unchanged. \
After duplicate, canceled, completed-reference, and marker repairs are \
composed, childless open or completed Pomodoro entries in the current daily \
ledger are removed with their full entry blocks. \
Done, cancelled, non-task, and unknown task statuses are never transitioned.\n\n\
When the same resolved task is linked beneath multiple open Pomodoros, the \
first open Pomodoro in file order keeps ownership and every conflicting \
physical line beneath later open Pomodoros is removed in full. Aliases, \
embeds, same-note links, and alternate note spellings compare by resolved \
vault-relative path plus block ID. Repeats within one owning Pomodoro are \
preserved, as are unresolved links and links beneath completed or cancelled \
Pomodoros. If a block ID matches multiple task lines, canceled-reference \
list-item removal requires every match to have a recognized CANCELLED status.\n\n\
After final checkbox and daily structural changes are composed, eligible \
area/project Tasks sections are grouped into generated child headings: \
Next & In Progress, Blocked, and Done & Canceled. Ready [ ] tasks and \
introductory prose stay in the unheaded intake where ordinary capture adds \
new tasks. Decorated containers carry a linked status-count badge row, authored \
topic headings keep their local context, generated groups use hidden ownership \
comments, and unsupported containers are warned and left \
unchanged. Grouping excludes daily notes, the selected previous daily, \
ordinary notes, archives, generated notes, and templates.\n\n\
Only Markdown checkbox lines allowed by the Obsidian Tasks globalFilter are \
considered. The scan skips hidden directories, templates, generated notes, \
and done archives. Blocked writes require exactly one compatible Tasks status \
named Blocked with symbol [?], type ON_HOLD, and next status Ready. Missing \
current daily notes and current daily notes without a Pomodoros section, as \
well as current notes with multiple non-empty open timed Pomodoros, fail before any file \
is changed. No earlier daily note is valid; an earlier note without a \
Pomodoros section contributes no historical references. Live writes use \
guarded snapshots, recovery copies, and a bounded quiet-period check for \
structural regrouping. Dry-run computes the same grouping preview without \
locking, waiting, staging, writing notes, or creating recovery records.",
        )
        .after_help(format!(
            "Examples:\n  {COMMAND_NAME}\n  {COMMAND_NAME} --dry-run\n  {COMMAND_NAME} --format json\n  {COMMAND_NAME} --bob-dir /tmp/bob-vault\n  {COMMAND_NAME} --retry-timeout 0\n  {COMMAND_NAME} --retry-timeout 300\n\nEnvironment:\n  BOB_DAY_FILE  exact current daily ledger; its dated filename anchors earlier-note lookup and future schedules\n  BOB_DIR       Bob vault root when --bob-dir is omitted\n  BOB_NOW       current date/time fallback for current and earlier-note selection"
        ))
        .disable_help_flag(true)
        .arg(
            Arg::new("bob-dir")
                .long("bob-dir")
                .short('b')
                .value_name("DIR")
                .value_parser(OsStringValueParser::new())
                .help("Bob vault root; defaults to BOB_DIR or ~/bob"),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .short('d')
                .action(ArgAction::SetTrue)
                .help("Compute and report the sync without writing notes"),
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
            Arg::new("retry-timeout")
                .long("retry-timeout")
                .short('r')
                .value_name("SECONDS")
                .value_parser(clap::value_parser!(u64))
                .default_value("120")
                .help(
                    "Retry recoverable failures (lock contention, a changed vault, an unstable read) for up to this many seconds with jittered backoff; 0 attempts once and fails fast",
                ),
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

#[derive(Debug, Clone)]
struct Request {
    bob_dir: PathBuf,
    dry_run: bool,
    retry_timeout: Duration,
}

impl Request {
    fn from_matches(matches: &ArgMatches) -> Self {
        let bob_dir = matches
            .get_one::<OsString>("bob-dir")
            .map(PathBuf::from)
            .map(|path| bob_env::expand_tilde(&path))
            .unwrap_or_else(bob_env::bob_dir);
        let retry_timeout = matches
            .get_one::<u64>("retry-timeout")
            .copied()
            .unwrap_or(120);
        Self {
            bob_dir,
            dry_run: matches.get_flag("dry-run"),
            retry_timeout: Duration::from_secs(retry_timeout),
        }
    }
}

mod compose;
mod model;
mod output;
mod parse;
mod pomodoro;
mod references;
mod retry;
mod settings;
mod structure;
mod sync;
#[cfg(test)]
mod tests;

use compose::*;
use model::*;
use output::*;
use parse::*;
use pomodoro::*;
use references::*;
use retry::*;
use settings::*;
use structure::*;
use sync::*;

pub(crate) use compose::{grouping_eligible_note, task_group_classification};
pub(crate) use model::{TaskStatusDefinition, TaskStatusType, TasksSettings};
pub(crate) use output::SyncError;
pub(crate) use parse::{markdown_files, task_metadata, TaskMetadata};
pub(crate) use settings::{read_tasks_settings, validate_blocked_status};
pub(crate) use sync::daily_anchor_date;
