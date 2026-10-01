//! Read-only top-level `bob ready`: each area/project note's Ready
//! lane against the per-note cap, as a bar overview, a single-note
//! worklist, or JSON.
//!
//! The contract lives in `docs/plan.md` ("## Ready cap per note").
//! Exit codes: 0 for a report; 1 for a vault I/O failure; 2 for an
//! invalid global cap, a non-Dataview task format, or an unresolvable
//! note; 3 with `--check` when any note is crowded.

use std::{
    ffi::OsString,
    iter,
    path::{Path, PathBuf},
};

use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};
use serde_json::json;

use super::{
    super::{env as bob_env, style::Styler},
    render::{render_overview, render_worklist, AlsoHere},
    scan::{
        scan_note_ready_overview, scan_note_ready_with_preview, ScanReport,
    },
    NoteReady, NoteState,
};

const COMMAND_NAME: &str = "bob ready";

/// Bump only for a breaking change to the JSON objects below; new
/// optional fields keep the current version.
const SCHEMA_VERSION: u32 = 1;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let request = match ReadyRequest::from_matches(&matches) {
        Ok(request) => request,
        Err(message) => {
            return print_ready_error(
                &ReadyError::Usage(message),
                output_format,
                "invalid_cap",
            );
        }
    };

    match show_ready(&request) {
        Ok(success) => {
            print_success(&success, &request, output_format);
            if request.check && is_crowded(&success) {
                3
            } else {
                0
            }
        }
        Err(error) => print_scan_error(error, output_format),
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
        .about("Show ready tasks per area/project note against the per-note cap")
        .long_about(
            "Show each area/project note's Ready lane against the \
            per-note cap. The Ready lane is visible, pullable [ ] tasks \
            whatever their freshness (dash READY also hides NEW and \
            ROTTEN). A note is crowded above its cap, full at it, and \
            has room below it.\n\n\
            The command is read-only. Crowded notes never fail the \
            report; only --check maps them to an exit code. See \
            docs/plan.md for the full definition.",
        )
        .after_help(
            "Examples:\n  bob ready\n  bob ready sase_remote\n  bob ready -a\n  bob ready -n 7\n  bob ready -c -f json\n\nEnvironment:\n  BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  BOB_DIR                   Bob vault root when --bob-dir is omitted\n  BOB_NOW                   Local datetime override for review date selection\n  NO_COLOR                  Disable colored output\n\nExit codes:\n  0  report (even when notes are crowded)\n  1  vault I/O failure\n  2  invalid plan.max_ready_per_note, non-Dataview task format, or unresolvable note\n  3  --check and at least one note is crowded\n\nOverrides:\n  ready_cap:              Per-note frontmatter cap (1-999) or off; invalid values lint once per note and fall back to the default",
        )
        .disable_help_flag(true)
        .arg(note_arg())
        .arg(all_arg())
        .arg(bob_dir_arg())
        .arg(cap_arg())
        .arg(check_arg())
        .arg(format_arg())
        .arg(help_arg())
}

fn note_arg() -> Arg {
    Arg::new("note")
        .value_name("NOTE")
        .help("List one note's lane tasks (vault path, or note name)")
}

fn all_arg() -> Arg {
    Arg::new("all")
        .long("all")
        .short('a')
        .action(ArgAction::SetTrue)
        .help("Also list room notes in full, empty, exempt, and done projects")
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(clap::builder::OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn cap_arg() -> Arg {
    Arg::new("cap")
        .long("cap")
        .short('n')
        .value_name("N")
        .help("Preview a different default cap (1-999) for this run only")
}

fn check_arg() -> Arg {
    Arg::new("check")
        .long("check")
        .short('c')
        .action(ArgAction::SetTrue)
        .help("Exit 3 when any note is crowded (for scripts and tmux)")
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
        .help("Print help")
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
struct ReadyRequest {
    bob_dir: PathBuf,
    note: Option<String>,
    show_all: bool,
    preview_cap: Option<u32>,
    check: bool,
}

impl ReadyRequest {
    fn from_matches(matches: &ArgMatches) -> Result<Self, String> {
        let preview_cap = match matches.get_one::<String>("cap") {
            Some(raw) => match raw.parse::<i64>() {
                Ok(number) if (1..=999).contains(&number) => {
                    Some(number as u32)
                }
                _ => {
                    return Err(format!(
                        "--cap must be an integer from 1 to 999; got {raw}"
                    ));
                }
            },
            None => None,
        };
        Ok(Self {
            bob_dir: matches
                .get_one::<std::ffi::OsString>("bob-dir")
                .map(PathBuf::from)
                .map(|path| bob_env::expand_tilde(&path))
                .unwrap_or_else(bob_env::bob_dir),
            note: matches.get_one::<String>("note").cloned(),
            show_all: matches.get_flag("all"),
            preview_cap,
            check: matches.get_flag("check"),
        })
    }
}

enum ReadyView {
    Overview,
    Worklist { entry: NoteReady, also: AlsoHere },
}

struct ReadySuccess {
    scan: ScanReport,
    view: ReadyView,
}

#[derive(Debug, Clone)]
enum ReadyError {
    Usage(String),
    Io(String),
}

fn is_crowded(success: &ReadySuccess) -> bool {
    match &success.view {
        ReadyView::Overview => success.scan.report.totals.crowded > 0,
        ReadyView::Worklist { entry, .. } => entry.state == NoteState::Crowded,
    }
}

fn show_ready(request: &ReadyRequest) -> Result<ReadySuccess, ReadyError> {
    // The overview skips the worklist lane passes; the worklist
    // needs them for its `also here` line.
    let scan_result = if request.note.is_some() {
        scan_note_ready_with_preview(&request.bob_dir, request.preview_cap)
    } else {
        scan_note_ready_overview(&request.bob_dir, request.preview_cap)
    };
    let scan = scan_result.map_err(|error| {
        let message = error.message().to_string();
        match error.exit_code() {
            1 => ReadyError::Io(message),
            _ => ReadyError::Usage(message),
        }
    })?;
    let view = match &request.note {
        None => ReadyView::Overview,
        Some(input) => {
            let entry = resolve_note(&scan, &request.bob_dir, input)?;
            let also = AlsoHere {
                next: scan
                    .next_counts
                    .get(entry.path.as_str())
                    .copied()
                    .unwrap_or(0),
                pending: scan
                    .pending_counts
                    .get(entry.path.as_str())
                    .copied()
                    .unwrap_or(0),
                blocked: scan
                    .blocked_counts
                    .get(entry.path.as_str())
                    .copied()
                    .unwrap_or(0),
                recurring: entry.recurring,
            };
            ReadyView::Worklist { entry, also }
        }
    };
    Ok(ReadySuccess { scan, view })
}

/// Resolve NOTE in order: vault path (with or without `.md`), exact
/// stem, case-insensitive stem. Ambiguous stems exit 2 listing every
/// candidate; unknown or non-area/project notes exit 2 with up to
/// three closest-name suggestions.
fn resolve_note(
    scan: &ScanReport,
    bob_dir: &Path,
    input: &str,
) -> Result<NoteReady, ReadyError> {
    let trimmed = input.trim();
    let path_forms = [trimmed.to_string(), format!("{trimmed}.md")];
    let mut by_path = Vec::new();
    for note in &scan.report.notes {
        let normalized = trimmed.trim_start_matches("./");
        if note.path == normalized || path_forms.contains(&note.path) {
            by_path.push(note.clone());
        }
    }
    if by_path.len() == 1 {
        return Ok(by_path.into_iter().next().expect("one path match"));
    }
    if by_path.len() > 1 {
        return Err(ambiguous(trimmed, &by_path));
    }

    let stem = trimmed.strip_suffix(".md").unwrap_or(trimmed);
    let exact: Vec<NoteReady> = scan
        .report
        .notes
        .iter()
        .filter(|note| note.name == stem)
        .cloned()
        .collect();
    if exact.len() == 1 {
        return Ok(exact.into_iter().next().expect("one exact match"));
    }
    if exact.len() > 1 {
        return Err(ambiguous(trimmed, &exact));
    }

    let lowered = stem.to_lowercase();
    let folded: Vec<NoteReady> = scan
        .report
        .notes
        .iter()
        .filter(|note| note.name.to_lowercase() == lowered)
        .cloned()
        .collect();
    if folded.len() == 1 {
        return Ok(folded.into_iter().next().expect("one folded match"));
    }
    if folded.len() > 1 {
        return Err(ambiguous(trimmed, &folded));
    }

    // A vault file that is not an area/project note resolves to a
    // targeted error instead of the unknown-note suggestions.
    for form in &path_forms {
        if bob_dir.join(form).is_file() {
            return Err(ReadyError::Usage(format!(
                "note {trimmed} is not an area or project note{}",
                suggestions(&scan.report.notes, stem),
            )));
        }
    }
    Err(ReadyError::Usage(format!(
        "unknown note {trimmed}{}",
        suggestions(&scan.report.notes, stem),
    )))
}

fn ambiguous(input: &str, candidates: &[NoteReady]) -> ReadyError {
    let mut paths: Vec<&str> =
        candidates.iter().map(|note| note.path.as_str()).collect();
    paths.sort();
    ReadyError::Usage(format!(
        "ambiguous note {input} matches:\n{}",
        paths
            .iter()
            .map(|path| format!("  {path}"))
            .collect::<Vec<_>>()
            .join("\n"),
    ))
}

/// Up to three closest note names: substring matches first, then
/// edit distance within a length-scaled threshold.
fn suggestions(notes: &[NoteReady], stem: &str) -> String {
    let lowered = stem.to_lowercase();
    let mut substring: Vec<&str> = Vec::new();
    let mut ranked: Vec<(&str, usize)> = Vec::new();
    for note in notes {
        let name = note.name.to_lowercase();
        if name.contains(&lowered) || lowered.contains(&name) {
            substring.push(note.name.as_str());
        } else {
            ranked.push((note.name.as_str(), edit_distance(&name, &lowered)));
        }
    }
    substring.sort();
    ranked.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));
    let threshold = lowered.len() / 2 + 2;
    let mut picked: Vec<&str> = substring;
    for (name, distance) in ranked {
        if picked.len() >= 3 || distance > threshold {
            break;
        }
        picked.push(name);
    }
    picked.truncate(3);
    if picked.is_empty() {
        String::new()
    } else {
        format!(". Did you mean: {}?", picked.join(", "))
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, &left) in a.iter().enumerate() {
        let mut next = vec![i + 1];
        for (j, &right) in b.iter().enumerate() {
            next.push(
                (row[j] + usize::from(left != right))
                    .min(next[j] + 1)
                    .min(row[j + 1] + 1),
            );
        }
        row = next;
    }
    row[b.len()]
}

fn note_json(entry: &NoteReady) -> serde_json::Value {
    json!({
        "path": entry.path,
        "name": entry.name,
        "kind": entry.kind,
        "status": entry.status,
        "parent": entry.parent,
        "count": entry.count,
        "cap": entry.cap,
        "cap_source": entry.cap_source.as_str(),
        "state": entry.state.as_str(),
        "over_by": entry.over_by,
        "make_up": entry.make_up.map(|make_up| json!({
            "ready": make_up.ready,
            "new": make_up.new,
            "rotten": make_up.rotten,
        })),
        "recurring": entry.recurring,
    })
}

fn totals_json(scan: &ScanReport) -> serde_json::Value {
    let totals = &scan.report.totals;
    json!({
        "notes": totals.notes,
        "areas": totals.areas,
        "projects": totals.projects,
        "crowded": totals.crowded,
        "full": totals.full,
        "room": totals.room,
        "empty": totals.empty,
        "exempt": totals.exempt,
        "counted": totals.counted,
        "excess": totals.excess,
        "recurring": totals.recurring,
    })
}

fn warnings_json(scan: &ScanReport) -> serde_json::Value {
    scan.report
        .lints
        .iter()
        .map(|lint| {
            json!({
                "code": lint.code,
                "path": lint.path,
                "message": lint.message,
            })
        })
        .collect()
}

fn envelope_base(scan: &ScanReport) -> serde_json::Value {
    json!({
        "ok": true,
        "schema_version": SCHEMA_VERSION,
        "date": scan.today.format("%Y-%m-%d").to_string(),
        "definition": "ready_lane",
        "cap": {
            "default": scan.default_cap,
            "source": scan.default_source.as_str(),
        },
        "totals": totals_json(scan),
        "warnings": warnings_json(scan),
    })
}

fn overview_json(success: &ReadySuccess) -> serde_json::Value {
    let mut value = envelope_base(&success.scan);
    value["notes"] = success.scan.report.notes.iter().map(note_json).collect();
    value
}

fn worklist_json(
    success: &ReadySuccess,
    entry: &NoteReady,
    also: &AlsoHere,
) -> serde_json::Value {
    let mut tasks: Vec<&super::ReadyDetail> = success
        .scan
        .ready_details
        .iter()
        .filter(|task| task.path == entry.path)
        .collect();
    tasks.sort_by_key(|task| task.line);
    let mut value = envelope_base(&success.scan);
    value["note"] = note_json(entry);
    value["tasks"] = tasks
        .iter()
        .map(|task| {
            json!({
                "path": task.path,
                "line": task.line,
                "text": task.text,
                "block_id": task.block_id,
                "bucket": task.bucket,
                "fresh_on": task.fresh_on.map(|date| date.format("%Y-%m-%d").to_string()),
            })
        })
        .collect();
    value["also"] = json!({
        "next": also.next,
        "pending": also.pending,
        "blocked": also.blocked,
        "recurring": also.recurring,
    });
    value
}

fn print_success(
    success: &ReadySuccess,
    request: &ReadyRequest,
    output_format: OutputFormat,
) {
    let styler = Styler::detect();
    match (&success.view, output_format) {
        (ReadyView::Overview, OutputFormat::Human) => {
            print!(
                "{}",
                render_overview(&success.scan, request.show_all, &styler)
            );
        }
        (ReadyView::Worklist { entry, also }, OutputFormat::Human) => {
            print!("{}", render_worklist(&success.scan, entry, also, &styler));
        }
        (ReadyView::Overview, OutputFormat::Json) => {
            println!("{}", overview_json(success));
        }
        (ReadyView::Worklist { entry, also }, OutputFormat::Json) => {
            println!("{}", worklist_json(success, entry, also));
        }
    }
}

fn print_ready_error(
    error: &ReadyError,
    output_format: OutputFormat,
    code: &str,
) -> i32 {
    let message = match error {
        ReadyError::Usage(message) | ReadyError::Io(message) => message.clone(),
    };
    match output_format {
        OutputFormat::Json => {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "schema_version": SCHEMA_VERSION,
                    "error": { "code": code, "message": message },
                })
            );
        }
        OutputFormat::Human => {
            eprintln!("{COMMAND_NAME}: {message}");
        }
    }
    match error {
        ReadyError::Usage(_) => 2,
        ReadyError::Io(_) => 1,
    }
}

fn print_scan_error(error: ReadyError, output_format: OutputFormat) -> i32 {
    let code = match &error {
        ReadyError::Usage(message)
            if message.starts_with("unknown note")
                || message.starts_with("ambiguous note") =>
        {
            if message.starts_with("unknown") {
                "unknown_note"
            } else {
                "ambiguous_note"
            }
        }
        ReadyError::Usage(message)
            if message.contains("not an area or project note") =>
        {
            "note_not_capped"
        }
        ReadyError::Usage(message)
            if message.contains("plan.max_ready_per_note") =>
        {
            "invalid_config"
        }
        ReadyError::Usage(_) => "usage",
        ReadyError::Io(_) => "io",
    };
    // Surface the config path the way `bob plan` does.
    if code == "invalid_config" {
        let message = match &error {
            ReadyError::Usage(message) | ReadyError::Io(message) => {
                message.clone()
            }
        };
        return print_ready_error(
            &ReadyError::Usage(format!("invalid plan config: {message}")),
            output_format,
            code,
        );
    }
    print_ready_error(&error, output_format, code)
}

#[cfg(test)]
mod tests {
    use super::super::CapSource;
    use super::*;

    #[test]
    fn closest_names_suggest_up_to_three() {
        let notes = vec![
            NoteReady {
                path: "sase_remote.md".to_string(),
                name: "sase_remote".to_string(),
                kind: "project".to_string(),
                status: "wip".to_string(),
                parent: None,
                count: 0,
                cap: Some(5),
                cap_source: CapSource::Default,
                state: NoteState::Empty,
                over_by: 0,
                make_up: None,
                recurring: 0,
            },
            NoteReady {
                path: "sase.md".to_string(),
                name: "sase".to_string(),
                kind: "project".to_string(),
                status: "wip".to_string(),
                parent: None,
                count: 0,
                cap: Some(5),
                cap_source: CapSource::Default,
                state: NoteState::Empty,
                over_by: 0,
                make_up: None,
                recurring: 0,
            },
        ];
        let text = suggestions(&notes, "sase_remtoe");
        assert!(text.contains("sase_remote"), "{text}");
        assert_eq!(edit_distance("kitten", "sitting"), 3);
    }
}
