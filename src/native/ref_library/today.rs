//! `bob ref today`: pull a reference's reading task into today's Pomodoro.
use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};
use serde::Serialize;
use std::path::Path;

use super::cli::list_config_from_matches;
use super::output::{
    generated_at, print_find_error, print_show_error, ErrorCandidate, Format,
    REF_SCHEMA_VERSION,
};
use super::show::{resolve_show, ShowFailure};
use super::{build_index, RefRow, RefTaskView};
use crate::native::capture::{
    is_route_token, link_existing_task, list_today_link_pomodoros,
    today_error_code, TodayPomodoroRow,
};
use crate::native::capture_link_tasks::is_linkable_status;
use crate::native::capture_pomodoros::PomodoroRef;
use crate::native::capture_task_toggle::PomodoroLinkTarget;
use crate::native::env as bob_env;
use crate::native::highlights_ref::{bob_dir_arg, ref_dir_arg, COMMAND_NAME};
use crate::native::note_tasks::{self, BlockIdLookup};
use crate::native::pomodoro;
use crate::native::style::Styler;

const TODAY_COMMAND: &str = "ref today";

/// The `bob ref today` subcommand builder.
pub(crate) fn today_command() -> ClapCommand {
    ClapCommand::new("today")
        .about(
            "Pull a reference's reading task into today's Pomodoro (sets it Next)",
        )
        .arg(
            Arg::new("ref")
                .value_name("REF")
                .required(true)
                .help("Vault path, note stem or id, URL, arXiv ID, DOI, or exact title"),
        )
        .arg(bob_dir_arg())
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .short('d')
                .action(ArgAction::SetTrue)
                .help("Plan and report without writing"),
        )
        .arg(
            Arg::new("exact")
                .long("exact")
                .short('e')
                .action(ArgAction::SetTrue)
                .requires("pomodoro")
                .help("With -p, whole-slug match only; never a prefix match"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .value_parser(["human", "json"])
                .default_value("human")
                .help("Output format"),
        )
        .arg(
            Arg::new("pomodoro")
                .long("pomodoro")
                .short('p')
                .value_name("NAME")
                .conflicts_with("pomodoro-ref")
                .help("Named destination: whole-slug, then prefix, else create"),
        )
        .arg(
            Arg::new("pomodoro-ref")
                .long("pomodoro-ref")
                .short('P')
                .value_name("LINE:DIGEST")
                .help("Stale-safe Pomodoro ref from a listing's `ref` field"),
        )
        .arg(ref_dir_arg())
        .after_help(
            "Selectors:\n  \
(none)              Stay on the task's open Pomodoro (Q), else the implicit current/next entry\n  \
-p NAME             Capture's #name rule: whole-slug, then prefix, else create\n  \
-p NAME -e          Whole-slug among open entries only, else create\n  \
-P LINE:DIGEST      Exactly that entry; stale or ambiguous is stale_pomodoro\n\n\
The vault mutation matches `bob capture '^route:block-id[#name]'`. This command never starts a Pomodoro and never commits or syncs; vault-sync picks the change up, as with capture.\n\n\
Environment:\n  \
BOB_DAY_FILE              Daily note override; otherwise <bob-dir>/YYYY/YYYYMMDD.md\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
BOB_NOW                   Local datetime override for default daily-note selection\n\n\
Examples:\n  \
bob ref today <REF> -p reading\n  \
bob ref today <REF> -d -f json",
        )
}

#[derive(Debug, Clone, Serialize)]
struct TodayRefJson<'a> {
    path: &'a str,
    link: &'a str,
    title: &'a str,
    marker_id: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize)]
struct TodayTaskJson<'a> {
    path: &'a str,
    block_id: Option<&'a str>,
    link: &'a str,
    status_symbol: String,
    status_name: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct TodaySuccessEnvelope<'a> {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    mode: &'static str,
    changed: bool,
    r#ref: TodayRefJson<'a>,
    task: TodayTaskJson<'a>,
    pomodoros: &'a [TodayPomodoroRow],
    capture: &'a serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
struct TodayErrorEnvelope<'a> {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    error: TodayErrorBody<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidates: Option<&'a [ErrorCandidate]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    r#ref: Option<TodayRefJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task: Option<TodayTaskJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoros: Option<&'a [TodayPomodoroRow]>,
}

#[derive(Debug, Clone, Serialize)]
struct TodayErrorBody<'a> {
    code: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'a str>,
}

struct ResolvedReadingTask {
    path: String,
    block_id: String,
    link: String,
    status_symbol: char,
    status_name: String,
    route: String,
}

pub(crate) fn run_today(matches: &ArgMatches) -> i32 {
    let format = Format::from_name(
        matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human"),
    );
    let dry_run = matches.get_flag("dry-run");
    let query = matches
        .get_one::<String>("ref")
        .map(String::as_str)
        .unwrap_or_default();
    let config = list_config_from_matches(matches);
    let index = match build_index(&config) {
        Ok(index) => index,
        Err(error) => {
            print_find_error(
                format,
                TODAY_COMMAND,
                "missing_ref_dir",
                &error.to_string(),
                Some("pass -b/--bob-dir or -r/--ref-dir, or set BOB_DIR"),
            );
            return 1;
        }
    };
    let row = match resolve_show(&index, query) {
        Ok(resolved) => &index.rows[resolved.row],
        Err(ShowFailure::Ambiguous { query, candidates }) => {
            print_show_error(
                format,
                TODAY_COMMAND,
                "ambiguous_reference",
                &format!("ambiguous reference: {query}"),
                &candidates,
            );
            return 1;
        }
        Err(ShowFailure::NotFound { query, candidates }) => {
            print_show_error(
                format,
                TODAY_COMMAND,
                "unknown_reference",
                &format!("no reference note matches {query}"),
                &candidates,
            );
            return 1;
        }
    };

    let ledger_target = match parse_ledger_target(matches) {
        Ok(target) => target,
        Err((code, message, hint)) => {
            let pomodoros = list_pomodoros(&config.bob_dir, None);
            return fail(
                format,
                dry_run,
                code,
                &message,
                hint,
                Some(row),
                None,
                &pomodoros,
            );
        }
    };

    match classify_reading_task(row, &config.bob_dir) {
        Ok(task) => run_link(
            format,
            dry_run,
            &config.bob_dir,
            row,
            &task,
            ledger_target.as_ref(),
        ),
        Err(refusal) => {
            let pomodoros =
                list_pomodoros(&config.bob_dir, refusal.task.as_ref());
            fail(
                format,
                dry_run,
                refusal.code,
                &refusal.message,
                refusal.hint,
                Some(row),
                refusal.task.as_ref(),
                &pomodoros,
            )
        }
    }
}

struct LedgerTargetOwned {
    kind: LedgerKind,
    name: Option<String>,
    pomodoro_ref: Option<PomodoroRef>,
}

enum LedgerKind {
    Implicit,
    Named,
    ExactName,
    Entry,
}

impl LedgerTargetOwned {
    fn as_ref(&self) -> PomodoroLinkTarget<'_> {
        match self.kind {
            LedgerKind::Implicit => PomodoroLinkTarget::Implicit,
            LedgerKind::Named => {
                PomodoroLinkTarget::Named(self.name.as_deref().unwrap_or(""))
            }
            LedgerKind::ExactName => PomodoroLinkTarget::ExactName(
                self.name.as_deref().unwrap_or(""),
            ),
            LedgerKind::Entry => PomodoroLinkTarget::Entry(
                self.pomodoro_ref.as_ref().expect("entry ref"),
            ),
        }
    }
}

fn parse_ledger_target(
    matches: &ArgMatches,
) -> Result<LedgerTargetOwned, (&'static str, String, Option<&'static str>)> {
    let exact = matches.get_flag("exact");
    if let Some(raw) = matches.get_one::<String>("pomodoro-ref") {
        let parsed = PomodoroRef::parse(raw).ok_or((
            "stale_pomodoro",
            format!("invalid Pomodoro ref {raw:?}; expected LINE:DIGEST"),
            Some("copy `ref` from `bob ref today <REF> -d -f json`"),
        ))?;
        return Ok(LedgerTargetOwned {
            kind: LedgerKind::Entry,
            name: None,
            pomodoro_ref: Some(parsed),
        });
    }
    if let Some(name) = matches.get_one::<String>("pomodoro") {
        return Ok(LedgerTargetOwned {
            kind: if exact {
                LedgerKind::ExactName
            } else {
                LedgerKind::Named
            },
            name: Some(name.clone()),
            pomodoro_ref: None,
        });
    }
    Ok(LedgerTargetOwned {
        kind: LedgerKind::Implicit,
        name: None,
        pomodoro_ref: None,
    })
}

struct TaskRefusal {
    code: &'static str,
    message: String,
    hint: Option<&'static str>,
    task: Option<ResolvedReadingTask>,
}

fn classify_reading_task(
    row: &RefRow,
    bob_dir: &Path,
) -> Result<ResolvedReadingTask, TaskRefusal> {
    if row
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "multiple_open_ref_tasks")
    {
        return Err(TaskRefusal {
            code: "ambiguous_reading_task",
            message: format!(
                "reference {} has several open reading tasks",
                row.path
            ),
            hint: Some("keep one open v2 reading task and retry"),
            task: None,
        });
    }
    let Some(task) = row.task.as_ref() else {
        return Err(TaskRefusal {
            code: "no_reading_task",
            message: format!("reference {} has no reading task", row.path),
            hint: Some("legacy trackerless refs cannot be planned for today"),
            task: None,
        });
    };
    if task.path == row.path {
        return Err(TaskRefusal {
            code: "v1_reading_task",
            message: format!(
                "reading task for {} is an in-note v1 ^ref tracker",
                row.path
            ),
            hint: Some("bob ref migrate-tasks"),
            task: Some(resolved_from_view(task, "v1")),
        });
    }
    let status_name = reading_status_name(task);
    if task.archived || !is_linkable_status(task.mark) {
        let state = if task.archived {
            "archived".to_string()
        } else {
            match task.mark {
                'x' | 'X' => "already read".to_string(),
                '-' => "abandoned".to_string(),
                _ => status_name.clone(),
            }
        };
        return Err(TaskRefusal {
            code: "reading_task_closed",
            message: format!("reading task is {state}"),
            hint: None,
            task: Some(resolved_from_view(task, &status_name)),
        });
    }
    let route = task
        .path
        .strip_suffix(".md")
        .or_else(|| task.path.strip_suffix(".MD"))
        .unwrap_or(&task.path);
    if task.path.contains('/')
        || task.path.contains('\\')
        || !is_route_token(route)
    {
        return Err(TaskRefusal {
            code: "reading_task_unroutable",
            message: format!(
                "reading task residence {} is not a root capture route",
                task.path
            ),
            hint: Some(
                "move the reading task onto a root area or project note",
            ),
            task: Some(resolved_from_view(task, &status_name)),
        });
    }
    let Some(block_id) = task.block_id.as_deref() else {
        return Err(TaskRefusal {
            code: "no_reading_task",
            message: format!("reading task for {} has no block ID", row.path),
            hint: None,
            task: Some(resolved_from_view(task, &status_name)),
        });
    };
    let (status_symbol, status_name) =
        lookup_task_status(bob_dir, &task.path, block_id)
            .unwrap_or((task.mark, status_name));
    Ok(ResolvedReadingTask {
        path: task.path.clone(),
        block_id: block_id.to_string(),
        link: task.link.clone(),
        status_symbol,
        status_name,
        route: route.to_string(),
    })
}

fn resolved_from_view(
    task: &RefTaskView,
    status_name: &str,
) -> ResolvedReadingTask {
    ResolvedReadingTask {
        path: task.path.clone(),
        block_id: task.block_id.clone().unwrap_or_default(),
        link: task.link.clone(),
        status_symbol: task.mark,
        status_name: status_name.to_string(),
        route: String::new(),
    }
}

fn reading_status_name(task: &RefTaskView) -> String {
    match task.mark {
        ' ' => "Ready",
        '*' => "Next",
        '/' => "In Progress",
        '?' => "Blocked",
        'x' | 'X' => "already read",
        '-' => "abandoned",
        _ => "unknown",
    }
    .to_string()
}

fn lookup_task_status(
    bob_dir: &Path,
    relative: &str,
    block_id: &str,
) -> Option<(char, String)> {
    let contents = std::fs::read_to_string(bob_dir.join(relative)).ok()?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    match scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => {
            Some((task.status_symbol, task.status_name.clone()))
        }
        _ => None,
    }
}

fn list_pomodoros(
    bob_dir: &Path,
    task: Option<&ResolvedReadingTask>,
) -> Vec<TodayPomodoroRow> {
    let day_file = pomodoro::day_file_for(bob_dir);
    let today = bob_env::current_datetime().date();
    let block_link = task
        .filter(|task| !task.route.is_empty() && !task.block_id.is_empty())
        .map(|task| format!("[[{}#^{}]]", task.route, task.block_id))
        .unwrap_or_default();
    let daily_path = day_file
        .strip_prefix(bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| day_file.clone());
    std::fs::read_to_string(&day_file)
        .ok()
        .map(|contents| {
            list_today_link_pomodoros(
                bob_dir,
                &daily_path,
                &contents,
                &block_link,
                today,
            )
        })
        .unwrap_or_default()
}

fn run_link(
    format: Format,
    dry_run: bool,
    bob_dir: &Path,
    row: &RefRow,
    task: &ResolvedReadingTask,
    ledger_target: PomodoroLinkTarget<'_>,
) -> i32 {
    let day_file = pomodoro::day_file_for(bob_dir);
    let today = bob_env::current_datetime().date();
    let block_link = format!("[[{}#^{}]]", task.route, task.block_id);
    let daily_path = day_file
        .strip_prefix(bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| day_file.clone());
    let pre_pomodoros = if day_file.is_file() {
        std::fs::read_to_string(&day_file)
            .ok()
            .map(|contents| {
                list_today_link_pomodoros(
                    bob_dir,
                    &daily_path,
                    &contents,
                    &block_link,
                    today,
                )
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let styler = Styler::detect();
    match link_existing_task(
        bob_dir,
        &task.route,
        &task.block_id,
        ledger_target,
        dry_run,
        &styler,
    ) {
        Ok(outcome) => {
            let pomodoros = if dry_run {
                pre_pomodoros
            } else if let Ok(contents) = std::fs::read_to_string(&day_file) {
                list_today_link_pomodoros(
                    bob_dir,
                    &daily_path,
                    &contents,
                    &block_link,
                    today,
                )
            } else {
                pre_pomodoros
            };
            print_success(
                format, dry_run, row, task, &pomodoros, &outcome, &styler,
            );
            0
        }
        Err(error) => {
            let code = today_error_code(&error);
            let hint = today_error_hint(code);
            fail(
                format,
                dry_run,
                code,
                &error.message,
                hint,
                Some(row),
                Some(task),
                &pre_pomodoros,
            )
        }
    }
}

fn today_error_hint(code: &str) -> Option<&'static str> {
    match code {
        "multiple_running" => {
            Some("pick a Pomodoro with -p NAME or -P LINE:DIGEST")
        }
        "no_open_pomodoro" => Some("pass -p NAME to plan a named Pomodoro"),
        "no_daily_note" => {
            Some("create today's daily note, or set BOB_DAY_FILE")
        }
        "stale_pomodoro" => {
            Some("list again with -d -f json and copy a fresh ref")
        }
        "note_changed" => Some("retry; the note changed after planning"),
        _ => None,
    }
}

fn print_success(
    format: Format,
    dry_run: bool,
    row: &RefRow,
    task: &ResolvedReadingTask,
    pomodoros: &[TodayPomodoroRow],
    outcome: &crate::native::capture::ExistingTaskLinkOutcome,
    styler: &Styler,
) {
    let mode = if dry_run { "dry_run" } else { "write" };
    match format {
        Format::Json => {
            let envelope = TodaySuccessEnvelope {
                ok: true,
                schema_version: REF_SCHEMA_VERSION,
                command: TODAY_COMMAND,
                generated_at: generated_at(),
                mode,
                changed: outcome.changed,
                r#ref: today_ref_json(row),
                task: today_task_json(task),
                pomodoros,
                capture: &outcome.capture,
            };
            println!(
                "{}",
                serde_json::to_string(&envelope)
                    .expect("today envelope serializes")
            );
        }
        Format::Human | Format::Markdown => {
            let verb = if outcome.kind == "link_guard" {
                if dry_run {
                    "would not link"
                } else {
                    "not linked"
                }
            } else if dry_run {
                "would link"
            } else {
                "linked"
            };
            let mark = if dry_run {
                styler.dim("·")
            } else {
                styler.green("✓")
            };
            println!("{mark} {verb} 📖 {}", row.title);
            print!("{}", outcome.human_details);
            if dry_run {
                print_open_pomodoros(pomodoros, styler);
            }
        }
    }
}

fn print_open_pomodoros(pomodoros: &[TodayPomodoroRow], styler: &Styler) {
    if pomodoros.is_empty() {
        return;
    }
    println!("{}", styler.cyan("Open Pomodoros"));
    for row in pomodoros {
        let glyph = if row.running {
            "●"
        } else if row.linked {
            "✓"
        } else if row.default {
            "◆"
        } else {
            "○"
        };
        let name = row
            .name
            .as_deref()
            .filter(|name| !name.is_empty())
            .map(|name| name.to_string())
            .or_else(|| {
                row.homes.as_ref().and_then(|homes| {
                    homes.chip.as_deref().map(|_| homes.label.clone())
                })
            })
            .unwrap_or_else(|| "(untitled)".to_string());
        let when = row.time_range.as_deref().unwrap_or("planned");
        println!("  {glyph} {name}  {when}  -P {}", row.pomodoro_ref);
    }
}

fn fail(
    format: Format,
    dry_run: bool,
    code: &str,
    message: &str,
    hint: Option<&str>,
    row: Option<&RefRow>,
    task: Option<&ResolvedReadingTask>,
    pomodoros: &[TodayPomodoroRow],
) -> i32 {
    let _ = dry_run;
    match format {
        Format::Json => {
            let envelope = TodayErrorEnvelope {
                ok: false,
                schema_version: REF_SCHEMA_VERSION,
                command: TODAY_COMMAND,
                generated_at: generated_at(),
                error: TodayErrorBody {
                    code,
                    message,
                    hint,
                },
                candidates: None,
                r#ref: row.map(today_ref_json),
                task: task.map(today_task_json),
                pomodoros: (!pomodoros.is_empty()).then_some(pomodoros),
            };
            println!(
                "{}",
                serde_json::to_string(&envelope)
                    .expect("today error envelope serializes")
            );
        }
        Format::Human | Format::Markdown => {
            eprintln!("{COMMAND_NAME}: error: {message}");
            if let Some(hint) = hint {
                eprintln!("hint: {hint}");
            }
        }
    }
    if code == "invalid_pomodoro_name" {
        2
    } else {
        1
    }
}

fn today_ref_json(row: &RefRow) -> TodayRefJson<'_> {
    TodayRefJson {
        path: &row.path,
        link: &row.link,
        title: &row.title,
        marker_id: row.marker_id.as_deref(),
    }
}

fn today_task_json(task: &ResolvedReadingTask) -> TodayTaskJson<'_> {
    TodayTaskJson {
        path: &task.path,
        block_id: (!task.block_id.is_empty()).then_some(task.block_id.as_str()),
        link: &task.link,
        status_symbol: task.status_symbol.to_string(),
        status_name: &task.status_name,
    }
}
