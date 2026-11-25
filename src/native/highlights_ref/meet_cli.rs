//! Public `bob ref meet` command: list meetings and file meeting notes.
//!
//! This layer coordinates the read-only preparation service, the
//! first-filing/replacement route, clipboard/stdin/editor inputs, TTY
//! pickers, and human/JSON receipts. It implements no vault writer itself.

use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self, BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
    process::Stdio,
};

use chrono::NaiveDate;
use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;

use super::cli::{bob_dir_arg, ref_dir_arg, xlib_dir_arg};
use super::create::meet_route::{file_or_replace_meeting, MeetWriteResult};
use super::intake::IntakeOutcome;
use super::meet::{
    error_human, error_json, list_human, list_json, prepare_human,
    prepare_json, prepare_list, prepare_report, FilingReport, MatchReport,
    MeetingPrepareOptions, MeetingPrepareReport, MeetingRowReport,
    MeetingSelection, NotesReport, PrepareError, PreparedFiling,
};
use super::{Config, COMMAND_NAME};
use crate::native::bank::dates::resolve_day;
use crate::native::capture_clip::read_clipboard_raw;
use crate::native::config as bob_config;
use crate::native::env as bob_env;
use crate::native::meetings::{
    parse_event_token, read_notes, resolve_event, EventToken, NotesInput,
    NotesSource,
};
use crate::native::ref_library::output::{generated_at, REF_SCHEMA_VERSION};

const EDITOR_SCISSORS: &str =
    "------------------------ >8 ------------------------";

/// Build the `bob ref meet` leaf in the Highlights pipeline group.
pub(crate) fn command() -> ClapCommand {
    ClapCommand::new("meet")
        .about(
            "List meetings and file meeting notes as a meetings reference",
        )
        .arg(
            Arg::new("event")
                .value_name("EVENT")
                .num_args(0..)
                .help("Bare/path note#^id, aliased wikilink, note:id, leading ~, Google Calendar URL with eid, or title-query words; joined as the existing parser expects"),
        )
        .arg(bob_dir_arg())
        .arg(
            Arg::new("clipboard")
                .long("clipboard")
                .short('c')
                .action(ArgAction::SetTrue)
                .conflicts_with("input")
                .help("Read notes through Bob's clipboard helper"),
        )
        .arg(
            Arg::new("day")
                .long("day")
                .short('D')
                .value_name("DAY")
                .value_parser(OsStringValueParser::new())
                .help("Local day using the gcal note parser"),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .short('d')
                .action(ArgAction::SetTrue)
                .help("Resolve and preview; no vault/store writes, locks, PDF rendering, network, or model calls"),
        )
        .arg(
            Arg::new("edit")
                .long("edit")
                .short('e')
                .action(ArgAction::SetTrue)
                .conflicts_with("list")
                .help("Review normalized notes in VISUAL, EDITOR, or vi; empty starting buffer if no source"),
        )
        .arg(
            Arg::new("expect-sha256")
                .long("expect-sha256")
                .value_name("HEX")
                .value_parser(OsStringValueParser::new())
                .hide(true)
                .conflicts_with("list")
                .help("Client guard: refuse when the exact input bytes differ"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .value_parser(["human", "json"])
                .default_value("human")
                .help("Human default; one JSON envelope on stdout"),
        )
        .arg(
            Arg::new("input")
                .long("input")
                .short('i')
                .value_name("FILE")
                .value_parser(OsStringValueParser::new())
                .conflicts_with("list")
                .help("UTF-8 text file; `-` reads stdin"),
        )
        .arg(
            Arg::new("lib-dir")
                .long("lib-dir")
                .short('L')
                .value_name("PATH")
                .value_parser(OsStringValueParser::new())
                .help("Highlights PDF library; defaults to BOB_HIGHLIGHTS_LIB_DIR or lib"),
        )
        .arg(
            Arg::new("list")
                .long("list")
                .short('l')
                .action(ArgAction::SetTrue)
                .conflicts_with("clipboard")
                .conflicts_with("input")
                .conflicts_with("edit")
                .conflicts_with("overwrite")
                .conflicts_with("expect-sha256")
                .conflicts_with("status")
                .conflicts_with("title")
                .conflicts_with("transcript")
                .conflicts_with("producer")
                .conflicts_with("parent")
                .help("Recent meetings and filed state"),
        )
        .arg(
            Arg::new("overwrite")
                .long("overwrite")
                .short('O')
                .action(ArgAction::SetTrue)
                .conflicts_with("list")
                .help("Replace the existing occurrence's notes"),
        )
        .arg(
            Arg::new("parent")
                .long("parent")
                .short('P')
                .value_name("NOTE")
                .value_parser(OsStringValueParser::new())
                .conflicts_with("list")
                .help("Filing destination; may authorize inbox/missing-root re-root"),
        )
        .arg(
            Arg::new("producer")
                .long("producer")
                .short('p')
                .value_name("NAME")
                .value_parser(OsStringValueParser::new())
                .conflicts_with("list")
                .help("Override configured producer"),
        )
        .arg(ref_dir_arg())
        .arg(
            Arg::new("status")
                .long("status")
                .short('s')
                .value_name("STATUS")
                .value_parser(OsStringValueParser::new())
                .conflicts_with("list")
                .help("New task status; explicit override on overwrite"),
        )
        .arg(
            Arg::new("title")
                .long("title")
                .short('T')
                .value_name("TITLE")
                .value_parser(OsStringValueParser::new())
                .conflicts_with("list")
                .help("Override derived reference title"),
        )
        .arg(
            Arg::new("transcript")
                .long("transcript")
                .short('t')
                .action(ArgAction::SetTrue)
                .conflicts_with("list")
                .help("PDF/source appendix only; never the recap"),
        )
        .arg(
            Arg::new("window")
                .long("window")
                .short('w')
                .value_name("DAYS")
                .value_parser(OsStringValueParser::new())
                .help("Lookback, default configured 7 days"),
        )
        .arg(xlib_dir_arg())
        .after_help(
            "Modes:\n  With -c, -i, or -e it files; with none it lists. -l explicitly lists and conflicts with filing-only options. Clipboard and input conflict.\n\nMeeting and destination selection:\n  Accept bare/path note#^id, aliased wikilinks, note:id, leading ~, a Google Calendar URL carrying eid, or title-query words. Join query arguments as the existing parser expects. Explicit identities win over query and automatic matching. Missing block IDs need an actionable refusal.\n\nNotes, config, and editing:\n  Read at most 1 MiB with the existing notes reader; reject empty, binary, unreadable, invalid UTF-8, and oversized notes. For -d, do not launch an editor: preview supplied notes; an empty editor-only dry run returns no_notes with a hint to supply notes or drop -d.\n\nOutput and failures:\n  Exit 0 for completed list/preview/file/queue. Exit 1 for operational/input failure. Exit 2 for no_meeting/ambiguous_meeting needing selection, needs_parent, or already_filed. Required codes: notes_unreadable, no_notes, no_meeting, ambiguous_meeting with evidenced candidates, needs_parent with root and parent candidates, already_filed with {note,task}, and notes_changed.\n\nExamples:\n  bob ref meet\n  bob ref meet -l -f json\n  bob ref meet -c 20261124#^cedar-review\n  bob ref meet -i notes.md 20261124#^cedar-review\n  bob ref meet -e 20261124#^cedar-review -P job\n  bob ref meet -c -d 20261124#^cedar-review\n  bob ref meet -i - 20261124#^cedar-review < notes.md\n  bob ref meet --overwrite -i revised.md 20261124#^cedar-review\n\nEnvironment:\n  BOB_DIR            Bob vault root; defaults to ~/bob\n  BOB_CONFIG_FILE    meetings config; defaults to ~/.config/bob/config.yml\n  BOB_NOW            Local datetime override for day/window selection\n  BOB_GCAL_ASSUME_TTY  Set to 1 to force interactive prompts in tests\n  VISUAL, EDITOR       Editor used by -e (vi if both are unset)\n\nSee docs/meeting-notes.md for the full contract.",
        )
}

/// Run `bob ref meet`.
pub(crate) fn run(matches: &ArgMatches) -> i32 {
    let json = matches
        .get_one::<String>("format")
        .is_some_and(|format| format == "json");
    let config = Config::from_matches(matches);
    let list_flag = matches.get_flag("list");
    let has_source = matches.get_flag("clipboard")
        || matches.contains_id("input")
        || matches.get_flag("edit");
    let has_filing_modifier = matches.get_flag("overwrite")
        || matches.contains_id("status")
        || matches.contains_id("title")
        || matches.get_flag("transcript")
        || matches.contains_id("producer")
        || matches.contains_id("parent")
        || matches.contains_id("expect-sha256");
    if list_flag || (!has_source && !has_filing_modifier) {
        return run_list(&config, matches, json);
    }
    run_file(&config, matches, json)
}

fn run_list(config: &Config, matches: &ArgMatches, json: bool) -> i32 {
    let day = match parse_day(matches) {
        Ok(day) => day,
        Err(message) => {
            return report_simple_error(json, "invalid_day", &message, None, 1);
        }
    };
    let window = match parse_window(matches) {
        Ok(window) => window,
        Err(message) => {
            return report_simple_error(
                json,
                "invalid_window",
                &message,
                None,
                1,
            );
        }
    };
    let query = event_query(matches);
    // Explicit query filters without touching clipboard or stdin.
    let list = match prepare_list(config, window, day) {
        Ok(list) => list,
        Err(error) => return report_prepare_error(&error, json),
    };
    let window_days = window.unwrap_or_else(|| {
        bob_config::load_meetings_config(&bob_config::config_path())
            .map(|config| config.window_days)
            .unwrap_or(7)
    });
    let filtered =
        filter_list_by_query(config, list, query.as_deref(), window_days, day);
    if json {
        println!("{}", list_json(&filtered));
        return 0;
    }
    println!("{}", list_human(&filtered));
    0
}

fn run_file(config: &Config, matches: &ArgMatches, json: bool) -> i32 {
    let dry_run = matches.get_flag("dry-run");
    let edit = matches.get_flag("edit");
    let day = match parse_day(matches) {
        Ok(day) => day,
        Err(message) => {
            return report_simple_error(json, "invalid_day", &message, None, 1);
        }
    };
    let window = match parse_window(matches) {
        Ok(window) => window,
        Err(message) => {
            return report_simple_error(
                json,
                "invalid_window",
                &message,
                None,
                1,
            );
        }
    };
    let parent = matches
        .get_one::<OsString>("parent")
        .map(|value| value.to_string_lossy().into_owned());
    let producer = matches
        .get_one::<OsString>("producer")
        .map(|value| value.to_string_lossy().into_owned());
    let status = matches
        .get_one::<OsString>("status")
        .map(|value| value.to_string_lossy().into_owned());
    let title = matches
        .get_one::<OsString>("title")
        .map(|value| value.to_string_lossy().into_owned());
    let expected_sha = matches
        .get_one::<OsString>("expect-sha256")
        .map(|value| value.to_string_lossy().into_owned());
    let include_transcript = matches.get_flag("transcript");
    let overwrite = matches.get_flag("overwrite");

    // Read the chosen notes source exactly once.
    let stdin_consumed = matches
        .get_one::<OsString>("input")
        .is_some_and(|path| path == OsStr::new("-"));
    let source = match read_notes_source(matches) {
        Ok(source) => source,
        Err((code, message, hint)) => {
            return report_simple_error(
                json,
                &code,
                &message,
                hint.as_deref(),
                1,
            );
        }
    };

    // Dry run never launches an editor.
    if dry_run && edit && source.is_none() {
        return report_simple_error(
            json,
            "no_notes",
            "meeting notes are empty",
            Some(
                "supply notes with -c or -i, or drop -d to edit interactively",
            ),
            1,
        );
    }
    if expected_sha.is_some() && source.is_none() {
        return report_simple_error(
            json,
            "notes_changed",
            "meeting notes changed after they were selected",
            Some("supply notes with -c or -i to check --expect-sha256"),
            1,
        );
    }

    let mut input = match source {
        Some(input) => {
            if let Some(expected) = expected_sha.as_deref()
                && !expected.eq_ignore_ascii_case(&input.sha256)
            {
                return report_simple_error(
                    json,
                    "notes_changed",
                    "meeting notes changed after they were selected",
                    Some("read the notes again and retry"),
                    1,
                );
            }
            Some(input)
        }
        None => None,
    };

    // Editor review: trusted identity/instructions stay outside the
    // editable notes; the accepted body parses back into notes.
    let mut edited = false;
    if edit && !dry_run {
        let seed = input
            .as_ref()
            .map(|input| input.text.as_str())
            .unwrap_or("");
        let original_sha = input.as_ref().map(|input| input.sha256.clone());
        match edit_draft(seed) {
            Ok(mut candidate) => {
                edited = input.as_ref().map(|input| input.text.as_str())
                    != Some(candidate.text.as_str());
                if let Some(sha) = original_sha {
                    candidate.sha256 = sha;
                }
                input = Some(candidate);
            }
            Err((code, message, hint)) => {
                return report_simple_error(
                    json,
                    &code,
                    &message,
                    hint.as_deref(),
                    1,
                );
            }
        }
    }
    let Some(input) = input else {
        return report_simple_error(
            json,
            "no_notes",
            "meeting notes are empty",
            Some("supply notes with -c, -i, or -e"),
            1,
        );
    };

    // Meeting selection, with TTY pickers that preserve the draft.
    let base_query = event_query(matches);
    let mut selection = match base_query {
        Some(query) => MeetingSelection::Token(query),
        None => MeetingSelection::Auto,
    };
    let mut parent_override = parent.clone();
    let interactive_picker = !json && !stdin_consumed && is_tty();
    // Retry loop for recoverable ambiguous/needs-parent choices.
    let prepared = loop {
        let options = MeetingPrepareOptions {
            selection: selection.clone(),
            day,
            window_days: window,
            parent: parent_override.clone(),
            producer: producer.clone(),
            status: status.clone(),
            title: title.clone(),
            include_transcript,
            overwrite,
            expected_sha256: None,
            edited,
        };
        match super::meet::prepare_filing(config, &input, &options) {
            Ok(prepared) => break prepared,
            Err(error) => {
                if error.code == "ambiguous_meeting"
                    && !error.candidates.is_empty()
                {
                    if !interactive_picker {
                        return report_prepare_error(&error, json);
                    }
                    match pick_meeting(&error) {
                        Ok(picked) => {
                            selection = picked;
                            continue;
                        }
                        Err(code) => return code,
                    }
                }
                if error.code == "needs_parent"
                    && !error.parent_candidates.is_empty()
                {
                    if !interactive_picker {
                        return report_prepare_error(&error, json);
                    }
                    match pick_parent(&error) {
                        Ok(chosen) => {
                            parent_override = Some(chosen);
                            // Retain picked meeting evidence when a picker
                            // already chose it.
                            if selection == MeetingSelection::Auto
                                && let Some(token) = event_query(matches)
                            {
                                selection = MeetingSelection::Token(token);
                            }
                            continue;
                        }
                        Err(code) => return code,
                    }
                }
                return report_prepare_error(&error, json);
            }
        }
    };

    if dry_run {
        return report_preview(&prepared, json);
    }

    // Real filing: render and ingest in one locked commit, or replace in
    // the overwrite transaction.
    let mut scratch = match super::ScratchDir::create("meet") {
        Ok(scratch) => scratch,
        Err(error) => {
            return report_simple_error(
                json,
                "render_failed",
                &error.to_string(),
                None,
                1,
            );
        }
    };
    let written = match file_or_replace_meeting(config, &prepared, &mut scratch)
    {
        Ok(written) => written,
        Err(error) => {
            let code = error.code.unwrap_or("render_failed");
            let message = error.to_string();
            return report_simple_error(
                json,
                code,
                &message,
                None,
                error.exit_code.unwrap_or(1),
            );
        }
    };
    report_written(config, &prepared, &written, json)
}

fn parse_day(matches: &ArgMatches) -> Result<Option<NaiveDate>, String> {
    let Some(raw) = matches.get_one::<OsString>("day") else {
        return Ok(None);
    };
    let text = raw.to_string_lossy();
    let today = bob_env::current_datetime().date();
    resolve_day(&text, today).map(Some)
}

fn parse_window(matches: &ArgMatches) -> Result<Option<u32>, String> {
    let Some(raw) = matches.get_one::<OsString>("window") else {
        return Ok(None);
    };
    let text = raw.to_string_lossy();
    let trimmed = text.trim();
    match trimmed.parse::<u32>() {
        Ok(days) if (1..=365).contains(&days) => Ok(Some(days)),
        _ => Err(format!(
            "invalid window {text:?}; use an integer from 1 to 365 days"
        )),
    }
}

fn event_query(matches: &ArgMatches) -> Option<String> {
    let values: Vec<String> = matches
        .get_many::<String>("event")
        .map(|iter| iter.cloned().collect())
        .unwrap_or_default();
    if values.is_empty() {
        return None;
    }
    Some(values.join(" "))
}

fn filter_list_by_query(
    config: &Config,
    list: super::meet::MeetingListReport,
    query: Option<&str>,
    window_days: u32,
    day: Option<NaiveDate>,
) -> super::meet::MeetingListReport {
    let Some(query) = query else {
        return list;
    };
    let token = parse_event_token(query);
    match token {
        EventToken::Explicit { .. } | EventToken::Url { .. } => {
            match resolve_event(&config.bob_dir, &token, window_days, day) {
                Ok(meeting) => {
                    let path =
                        meeting.path.to_string_lossy().replace('\\', "/");
                    let meetings = list
                        .meetings
                        .into_iter()
                        .filter(|row| {
                            row.path == path && row.block_id == meeting.block_id
                        })
                        .collect();
                    super::meet::MeetingListReport { meetings }
                }
                Err(_) => super::meet::MeetingListReport {
                    meetings: Vec::new(),
                },
            }
        }
        EventToken::Query { words } => {
            if words.is_empty() {
                return list;
            }
            let folded: Vec<String> =
                words.iter().map(|word| word.to_ascii_lowercase()).collect();
            let meetings = list
                .meetings
                .into_iter()
                .filter(|row| {
                    let title = row.title.to_ascii_lowercase();
                    folded.iter().all(|word| title.contains(word))
                })
                .collect();
            super::meet::MeetingListReport { meetings }
        }
    }
}

fn read_notes_source(
    matches: &ArgMatches,
) -> Result<Option<NotesInput>, (String, String, Option<String>)> {
    let clipboard = matches.get_flag("clipboard");
    let input = matches.get_one::<OsString>("input");
    if clipboard {
        match read_clipboard_raw() {
            Ok(bytes) => read_notes(NotesSource::Bytes(&bytes))
                .map(Some)
                .map_err(|error| {
                    (
                        error.code().to_string(),
                        error.to_string(),
                        Some(clipboard_hint(&error).to_string()),
                    )
                }),
            Err(message) => Err((
                "notes_unreadable".to_string(),
                format!("could not read clipboard: {message}"),
                Some("check BOB_CLIPBOARD_CMD and retry".to_string()),
            )),
        }
    } else if let Some(path) = input {
        if path == OsStr::new("-") {
            read_notes(NotesSource::Stdin).map(Some).map_err(|error| {
                (
                    error.code().to_string(),
                    error.to_string(),
                    Some("supply UTF-8 meeting notes on stdin".to_string()),
                )
            })
        } else {
            let path_buf = PathBuf::from(path);
            read_notes(NotesSource::Path(&path_buf)).map(Some).map_err(
                |error| {
                    (
                        error.code().to_string(),
                        error.to_string(),
                        Some(format!(
                            "check {} is readable UTF-8 text",
                            path_buf.display()
                        )),
                    )
                },
            )
        }
    } else {
        Ok(None)
    }
}

fn clipboard_hint(error: &crate::native::meetings::NotesError) -> &'static str {
    use crate::native::meetings::NotesError;
    match error {
        NotesError::Empty => "copy meeting notes to the clipboard and retry",
        _ => "check the clipboard contents and retry",
    }
}

fn notes_error(
    error: crate::native::meetings::NotesError,
) -> (String, String, Option<String>) {
    use crate::native::meetings::NotesError;
    let hint = match error {
        NotesError::Empty => {
            Some("write notes in the editor or supply -c/-i".to_string())
        }
        _ => Some("check VISUAL/EDITOR and retry".to_string()),
    };
    (error.code().to_string(), error.to_string(), hint)
}

fn split_before_scissors(bytes: &[u8]) -> &[u8] {
    let marker = EDITOR_SCISSORS.as_bytes();
    bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .map_or(bytes, |pos| &bytes[..pos])
}

fn edit_draft(
    seed: &str,
) -> Result<NotesInput, (String, String, Option<String>)> {
    let editor = std::env::var_os("VISUAL")
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var_os("EDITOR").filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| OsString::from("vi"));
    let mut file = tempfile::NamedTempFile::new().map_err(|error| {
        (
            "editor_failed".to_string(),
            format!("create editor file: {error}"),
            Some("check VISUAL/EDITOR and retry".to_string()),
        )
    })?;
    let mut initial = String::new();
    if !seed.is_empty() {
        initial.push_str(seed);
        if !seed.ends_with('\n') {
            initial.push('\n');
        }
    }
    initial.push_str(EDITOR_SCISSORS);
    initial.push_str("\nDo not modify or remove the line above; everything below it is ignored.\n");
    initial.push_str("Write meeting notes above the scissors. The first line names the meeting; add TL;DR, next steps, and sections.\n");
    file.write_all(initial.as_bytes()).map_err(|error| {
        (
            "editor_failed".to_string(),
            format!("write editor file: {error}"),
            Some("check VISUAL/EDITOR and retry".to_string()),
        )
    })?;
    file.flush().map_err(|error| {
        (
            "editor_failed".to_string(),
            format!("flush editor file: {error}"),
            Some("check VISUAL/EDITOR and retry".to_string()),
        )
    })?;
    let mut words: Vec<String> = editor
        .to_string_lossy()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if words.is_empty() {
        return Err((
            "editor_failed".to_string(),
            "$EDITOR is empty".to_string(),
            Some("check VISUAL/EDITOR and retry".to_string()),
        ));
    }
    let program = words.remove(0);
    let status = std::process::Command::new(program)
        .args(words)
        .arg(file.path())
        .stdin(Stdio::inherit())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| {
            (
                "editor_failed".to_string(),
                format!("launch editor: {error}"),
                Some("check VISUAL/EDITOR and retry".to_string()),
            )
        })?;
    if !status.success() {
        return Err((
            "editor_failed".to_string(),
            format!("editor exited with {status}"),
            Some("check VISUAL/EDITOR and retry".to_string()),
        ));
    }
    let contents = fs::read(file.path()).map_err(|error| {
        (
            "editor_failed".to_string(),
            format!("read editor file: {error}"),
            Some("check VISUAL/EDITOR and retry".to_string()),
        )
    })?;
    let accepted = split_before_scissors(&contents);
    read_notes(NotesSource::Bytes(accepted)).map_err(notes_error)
}

fn is_tty() -> bool {
    if bob_env::var("BOB_GCAL_ASSUME_TTY")
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
    {
        return true;
    }
    io::stdin().is_terminal()
}

fn read_line() -> Option<String> {
    let stdin = io::stdin();
    let mut locked = stdin.lock();
    let mut line = String::new();
    match locked.read_line(&mut line) {
        Ok(_) => Some(line),
        Err(_) => None,
    }
}

fn pick_meeting(error: &PrepareError) -> Result<MeetingSelection, i32> {
    eprintln!("several meetings match; choose one:");
    for (index, candidate) in error.candidates.iter().enumerate() {
        eprintln!("  {}  {}", index + 1, candidate.detail);
        for evidence in &candidate.evidence {
            eprintln!("      {evidence}");
        }
    }
    eprint!("Which? [1] › ");
    let _ = io::stderr().flush();
    let line = read_line().unwrap_or_default();
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return picked_token(error, 0);
    }
    if trimmed.eq_ignore_ascii_case("q") || trimmed.eq_ignore_ascii_case("quit")
    {
        eprintln!("{COMMAND_NAME}: error: selection cancelled");
        return Err(2);
    }
    match trimmed.parse::<usize>() {
        Ok(number) if number >= 1 && number <= error.candidates.len() => {
            picked_token(error, number - 1)
        }
        _ => {
            eprintln!(
                "{COMMAND_NAME}: error: invalid choice {trimmed:?}; enter 1-{}",
                error.candidates.len()
            );
            Err(2)
        }
    }
}

fn picked_token(
    error: &PrepareError,
    index: usize,
) -> Result<MeetingSelection, i32> {
    let Some(candidate) = error.candidates.get(index) else {
        eprintln!("{COMMAND_NAME}: error: invalid choice");
        return Err(2);
    };
    let token = candidate
        .meeting
        .block_id
        .as_deref()
        .map(|id| {
            format!(
                "{}#^{id}",
                candidate.meeting.path.to_string_lossy().replace('\\', "/")
            )
        })
        .unwrap_or_else(|| candidate.detail.clone());
    Ok(MeetingSelection::Picked {
        token,
        evidence: candidate.evidence.clone(),
    })
}

fn pick_parent(error: &PrepareError) -> Result<String, i32> {
    if let Some(root) = error.root.as_deref() {
        eprintln!("root: {root}");
    }
    eprintln!("choose a filing parent:");
    for (index, candidate) in error.parent_candidates.iter().enumerate() {
        eprintln!(
            "  {}  {} · {} · {}",
            index + 1,
            candidate.route,
            candidate.kind,
            candidate.label
        );
    }
    eprint!("File under which note? › ");
    let _ = io::stderr().flush();
    let line = read_line().unwrap_or_default();
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("q")
        || trimmed.eq_ignore_ascii_case("quit")
    {
        eprintln!("{COMMAND_NAME}: error: selection cancelled");
        return Err(2);
    }
    if let Ok(number) = trimmed.parse::<usize>()
        && number >= 1
        && number <= error.parent_candidates.len()
    {
        return Ok(error.parent_candidates[number - 1].route.clone());
    }
    // Accept a typed route as well as a number.
    Ok(trimmed.to_string())
}

fn report_simple_error(
    json: bool,
    code: &str,
    message: &str,
    hint: Option<&str>,
    exit_code: i32,
) -> i32 {
    if json {
        let envelope = serde_json::json!({
            "ok": false,
            "schema_version": REF_SCHEMA_VERSION,
            "command": "ref meet",
            "generated_at": generated_at(),
            "error": {
                "code": code,
                "message": message,
                "hint": hint,
            },
        });
        println!(
            "{}",
            serde_json::to_string(&envelope).expect("meet error serializes")
        );
        return exit_code;
    }
    eprintln!("{COMMAND_NAME}: error: {message}");
    if let Some(hint) = hint {
        eprintln!("hint: {hint}");
    }
    exit_code
}

fn report_prepare_error(error: &PrepareError, json: bool) -> i32 {
    if json {
        println!("{}", error_json(error));
        return error.exit_code;
    }
    eprintln!("{}", error_human(error));
    error.exit_code
}

fn report_preview(prepared: &PreparedFiling, json: bool) -> i32 {
    if json {
        println!("{}", prepare_json(prepared));
        return 0;
    }
    println!("{}", prepare_human(prepared));
    0
}

#[derive(Serialize)]
struct QueuedDetail {
    path: String,
    reason: String,
    detail: String,
}

#[derive(Serialize)]
struct FileSuccessPayload {
    outcome: &'static str,
    meeting: MeetingRowReport,
    #[serde(rename = "match")]
    match_report: MatchReport,
    filing: FilingReport,
    notes: NotesReport,
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sync_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    push_pending: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queued: Option<QueuedDetail>,
    pages: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_pointer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replaced: Option<bool>,
}

#[derive(Serialize)]
struct FileSuccessEnvelope {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    #[serde(flatten)]
    payload: FileSuccessPayload,
}

fn vault_relative(path: &Path, vault: &Path) -> String {
    path.strip_prefix(vault)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

fn task_route(path: &Path, vault: &Path) -> String {
    let relative = vault_relative(path, vault);
    relative
        .strip_suffix(".md")
        .unwrap_or(&relative)
        .to_string()
}

fn overlay_actual_paths(
    report: &mut MeetingPrepareReport,
    vault: &Path,
    note: Option<&Path>,
    pdf: Option<&Path>,
    task_destination: Option<&Path>,
    task_block_id: Option<&str>,
    task_status: Option<&str>,
) {
    if let Some(note) = note {
        report.filing.note = vault_relative(note, vault);
    }
    if let Some(pdf) = pdf {
        report.filing.pdf = vault_relative(pdf, vault);
    }
    if let Some(destination) = task_destination {
        report.filing.task.destination = task_route(destination, vault);
    }
    if let Some(block_id) = task_block_id {
        report.filing.task.block_id = block_id.to_string();
    }
    if let Some(status) = task_status {
        report.filing.task.status = status.to_string();
    }
}

fn print_success_envelope(payload: FileSuccessPayload) {
    let envelope = FileSuccessEnvelope {
        ok: true,
        schema_version: REF_SCHEMA_VERSION,
        command: "ref meet",
        generated_at: generated_at(),
        payload,
    };
    println!(
        "{}",
        serde_json::to_string(&envelope).expect("meet success serializes")
    );
}

fn report_written(
    config: &Config,
    prepared: &PreparedFiling,
    written: &MeetWriteResult,
    json: bool,
) -> i32 {
    match written {
        MeetWriteResult::Filed(result) => match &result.outcome {
            IntakeOutcome::Ingested {
                commit_sha,
                commit_warning,
                sync_status,
                note,
                pdf,
                task_destination,
                task_block_id,
                ..
            } => {
                let mut warnings = result.warnings.clone();
                if let Some((from, to)) = result.reroot_applied.as_ref() {
                    warnings.push(format!(
                        "re-rooted the event from {} to {to}",
                        from.as_deref().unwrap_or("<missing>")
                    ));
                }
                if let Some(warning) = commit_warning.as_deref() {
                    warnings.push(warning.to_string());
                }
                let mut report = prepare_report(prepared);
                overlay_actual_paths(
                    &mut report,
                    &config.bob_dir,
                    Some(note),
                    Some(pdf),
                    task_destination.as_deref(),
                    task_block_id.as_deref(),
                    None,
                );
                report_filed(
                    report,
                    warnings,
                    commit_sha.clone(),
                    commit_warning.clone(),
                    Some(sync_status.clone()),
                    None,
                    result.pages,
                    result.source_pointer.clone(),
                    false,
                    json,
                )
            }
            IntakeOutcome::Queued {
                path,
                reason,
                detail,
            } => report_queued(
                prepared,
                &result.warnings,
                path,
                reason.as_str(),
                detail,
                result.pages,
                result.source_pointer.as_deref(),
                result.skipped_reroot,
                json,
            ),
            IntakeOutcome::Incomplete { error } => {
                report_simple_error(json, "intake_failed", error, None, 1)
            }
            IntakeOutcome::Kept { why } | IntakeOutcome::WouldKeep { why } => {
                report_simple_error(json, "intake_failed", why, None, 1)
            }
            IntakeOutcome::WouldIngest { .. } => report_simple_error(
                json,
                "intake_failed",
                "unexpected preview outcome",
                None,
                1,
            ),
        },
        MeetWriteResult::Replaced(result) => {
            let overwrite = &result.overwrite;
            let mut warnings = result.warnings.clone();
            if overwrite.push_pending {
                warnings.push(
                    "push pending; background vault-sync will publish it"
                        .to_string(),
                );
            }
            let mut report = prepare_report(prepared);
            report.filing.note = overwrite.note.clone();
            report.filing.pdf = overwrite.pdf.clone();
            report.filing.task.destination = overwrite
                .task_path
                .strip_suffix(".md")
                .unwrap_or(&overwrite.task_path)
                .to_string();
            if let Some(block_id) = overwrite.task_block_id.as_deref() {
                report.filing.task.block_id = block_id.to_string();
            }
            report.filing.task.status = overwrite.task_status.clone();
            report_filed(
                report,
                warnings,
                Some(overwrite.commit_sha.clone()),
                None,
                None,
                Some(overwrite.push_pending),
                result.pages,
                result.source_pointer.clone(),
                true,
                json,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn report_filed(
    mut report: MeetingPrepareReport,
    warnings: Vec<String>,
    commit: Option<String>,
    commit_warning: Option<String>,
    sync_status: Option<String>,
    push_pending: Option<bool>,
    pages: usize,
    source_pointer: Option<String>,
    replaced: bool,
    json: bool,
) -> i32 {
    report.outcome = "filed";
    if json {
        print_success_envelope(FileSuccessPayload {
            outcome: "filed",
            meeting: report.meeting,
            match_report: report.match_report,
            filing: report.filing,
            notes: report.notes,
            warnings,
            commit,
            commit_warning,
            sync_status: sync_status.filter(|status| !status.is_empty()),
            push_pending,
            queued: None,
            pages,
            source_pointer,
            replaced: replaced.then_some(true),
        });
        return 0;
    }
    if replaced {
        println!("Replaced meeting notes as {}", report.filing.note);
    } else {
        println!("Filed meeting notes as {}", report.filing.note);
    }
    println!(
        "task      {}#^{} ({})",
        report.filing.task.destination,
        report.filing.task.block_id,
        report.filing.task.status
    );
    println!(
        "relation  {} {}",
        report.filing.relation.label, report.filing.relation.target
    );
    println!("pdf       {} ({} pages)", report.filing.pdf, pages);
    if let Some(commit) = commit.as_deref() {
        println!("commit    {commit}");
    }
    if let Some(warning) = commit_warning.as_deref() {
        println!("commit    warning: {warning}");
    }
    if let Some(sync_status) = sync_status.as_deref()
        && !sync_status.is_empty()
    {
        println!("sync      {sync_status}");
    }
    if push_pending == Some(true) {
        println!("push      pending");
    }
    if let Some(pointer) = source_pointer.as_deref() {
        println!("source    {pointer}");
    }
    for warning in &warnings {
        println!("warning   {warning}");
    }
    0
}

#[allow(clippy::too_many_arguments)]
fn report_queued(
    prepared: &PreparedFiling,
    warnings: &[String],
    path: &Path,
    reason: &str,
    detail: &str,
    pages: usize,
    source_pointer: Option<&str>,
    skipped_reroot: bool,
    json: bool,
) -> i32 {
    let mut report = prepare_report(prepared);
    report.outcome = "queued";
    report.filing.reroot = None;
    let mut warnings = warnings.to_vec();
    if skipped_reroot {
        warnings.push(
            "skipped the event re-root because intake queued; `bob ref scan` will file the notes without re-rooting"
                .to_string(),
        );
    }
    if json {
        print_success_envelope(FileSuccessPayload {
            outcome: "queued",
            meeting: report.meeting,
            match_report: report.match_report,
            filing: report.filing,
            notes: report.notes,
            warnings,
            commit: None,
            commit_warning: None,
            sync_status: None,
            push_pending: None,
            queued: Some(QueuedDetail {
                path: path.display().to_string(),
                reason: reason.to_string(),
                detail: detail.to_string(),
            }),
            pages,
            source_pointer: source_pointer.map(str::to_string),
            replaced: None,
        });
        eprintln!("queued; run `bob ref scan` to file the notes");
        return 0;
    }
    println!("Queued meeting notes as {}", report.filing.note);
    println!("queued    {} ({reason}: {detail})", path.display());
    println!("pdf       {} ({} pages)", report.filing.pdf, pages);
    println!("run `bob ref scan` to file the queued notes; nothing was filed");
    if skipped_reroot {
        println!("warning   skipped the event re-root because intake queued");
    }
    for warning in &warnings {
        println!("warning   {warning}");
    }
    0
}

#[cfg(test)]
mod clap_tests {
    use super::*;

    fn parse(args: &[&str]) -> clap::ArgMatches {
        command()
            .try_get_matches_from(args)
            .unwrap_or_else(|error| panic!("parse {args:?}: {error}"))
    }

    fn os(matches: &clap::ArgMatches, id: &str) -> String {
        matches
            .get_one::<OsString>(id)
            .unwrap_or_else(|| panic!("missing {id}"))
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn public_value_flags_parse_as_os_string() {
        const SHA: &str =
            "abababababababababababababababababababababababababababababababab";
        let cases: &[(&[&str], &str, &str)] = &[
            (&["meet", "-c", "--parent", "job"], "parent", "job"),
            (&["meet", "-c", "-P", "job"], "parent", "job"),
            (
                &["meet", "-c", "--producer", "VoiceQuill"],
                "producer",
                "VoiceQuill",
            ),
            (
                &["meet", "-c", "-p", "VoiceQuill"],
                "producer",
                "VoiceQuill",
            ),
            (&["meet", "-c", "--status", "next"], "status", "next"),
            (&["meet", "-c", "-s", "next"], "status", "next"),
            (&["meet", "-c", "--title", "Custom"], "title", "Custom"),
            (&["meet", "-c", "-T", "Custom"], "title", "Custom"),
            (
                &["meet", "-c", "--lib-dir", "/tmp/lib"],
                "lib-dir",
                "/tmp/lib",
            ),
            (&["meet", "-c", "-L", "/tmp/lib"], "lib-dir", "/tmp/lib"),
            (&["meet", "-c", "--day", "today"], "day", "today"),
            (&["meet", "-c", "-D", "today"], "day", "today"),
            (&["meet", "-c", "--window", "3"], "window", "3"),
            (&["meet", "-c", "-w", "3"], "window", "3"),
            (&["meet", "-i", "notes.md"], "input", "notes.md"),
            (
                &["meet", "-c", "--expect-sha256", SHA],
                "expect-sha256",
                SHA,
            ),
        ];
        for (args, id, expected) in cases {
            let matches = parse(args);
            assert_eq!(os(&matches, id), *expected, "{args:?} {id}");
        }
    }

    #[test]
    fn split_before_scissors_keeps_exact_prefix_bytes() {
        let mut body = b"hello\r\n".to_vec();
        body.extend_from_slice(EDITOR_SCISSORS.as_bytes());
        body.extend_from_slice(b"\ninstructions\n");
        assert_eq!(split_before_scissors(&body), b"hello\r\n");
        assert_eq!(split_before_scissors(b"no marker"), b"no marker");
    }
}
