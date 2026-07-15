use std::{
    ffi::OsString,
    fs,
    io::{self, BufRead, IsTerminal, Write},
    iter,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::{Datelike, Days, NaiveDate};
use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{
    capture_clip, collect_done, env as bob_env, pomodoro, style::Styler,
};

const COMMAND_NAME: &str = "bob capture";
pub(crate) const INBOX_FILE: &str = "mac_inbox.md";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let request = match CaptureRequest::from_matches(&matches) {
        Ok(request) => request,
        Err(error) => return print_capture_error(error, output_format),
    };

    match capture(request) {
        Ok(result) => {
            print_success(&result, output_format);
            0
        }
        Err(error) => print_capture_error(error, output_format),
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
        .about("Capture a task or bullet into the Bob vault")
        .long_about(
            "Capture one task into the Bob Obsidian vault.\n\n\
Text is normalized to one line, formatted as a #task with a [created::] stamp, \
and written to mac_inbox.md unless an @route token or --route target is \
provided. Existing target files prefer a Tasks section, then fall back to the \
last top-level task block. Missing target files are created when needed.\n\n\
Append a trailing lowercase 's:<N>' token, where N is a non-negative integer, \
to schedule the capture N days from today. The token is removed from the task \
text and rendered as [scheduled::YYYY-MM-DD] after [created::YYYY-MM-DD]. It \
may appear before or after a trailing @route token and is recognized only at \
the very end of the input.\n\n\
Append a trailing '%' or '%<header>' token to capture the system clipboard as \
an indented child bullet. Headers use letters, digits, '_' and '-'; '_' renders \
as a space, and a bare '%' renders as '**CLIP:**'. The marker composes with \
s:<N> and every route kind in either terminal order. Small text stays inline, \
2-10 flat lines become nested bullets, copied file paths are saved under img/ \
or file/, and long or Markdown-structured text is preserved in a timestamped \
file/clip-*.md snippet. Use --clip[=HEADER] to force capture while keeping '%' \
tokens literal, or --no-clip to keep a genuine trailing '%...' token literal. \
The accepted '%20' header quirk can be escaped with --no-clip. Clipboard \
failures abort before the note or attachment files are changed.\n\n\
Use '@<route>:<block-id>' in the same leading or trailing position to create \
a next-status task and link it from today's Pomodoro ledger. The routed task \
renders as '- [*] #task <body> [created::YYYY-MM-DD] ^<block-id>' (with any \
scheduled property before the final block ID). The daily note comes from \
BOB_DAY_FILE or <bob-dir>/YYYY/YYYYMMDD.md. Capture prefers the single open \
timed entry in its Pomodoros section and otherwise uses the first open entry. \
Both notes are fully validated before either is replaced; duplicate block IDs, \
missing ledger structure, no open entry, and multiple open timed entries fail \
without a partial capture.\n\n\
Append '#<section-prefix>' or a bare '#' to an @route token (such as \
'@notes#Ideas' or '@notes#') to capture an ordinary bullet instead. It renders \
as '- <body> [created::YYYY-MM-DD]' and is placed in a non-Tasks section whose \
heading title starts with the prefix (compared case insensitively), or any \
non-Tasks section for a bare '#'. A matching non-H1 section is preferred; a \
matching H1 heading is used only when no non-H1 heading matches. The marker may \
lead ('@notes#Ideas jot idea') or trail ('jot idea @notes#Ideas') the body. \
Standalone terminal '#...' markers are no longer accepted and fail with a \
usage error.\n\n\
Use --route with --section to force bullet mode while keeping @tokens literal. \
The section title is matched exactly, case insensitively, against non-Tasks \
headings; if no heading matches, the bullet falls back to the pre-heading \
section.",
        )
        .after_help(
            "Examples:\n  bob capture buy milk @groceries\n  bob capture buy milk s:1\n  bob capture buy milk s:2 @groceries\n  bob capture buy milk @groceries s:2\n  bob capture buy milk %\n  bob capture investigate %log @dev:blockid\n  bob capture --clip=screenshot -- save dashboard\n  bob capture '@dev:foobar' 'Some foobar task.'\n  bob capture jot idea @notes#Ideas\n  bob capture --route notes --section Ideas -- jot idea\n  bob capture @notes#Ideas jot idea\n  echo 'buy milk @groceries' | bob capture\n  bob capture -f json -- @work send status\n\nEnvironment:\n  BOB_CLIPBOARD_CMD  whitespace-split command that prints clipboard text; overrides platform tools\n  BOB_DAY_FILE       exact daily note used by Pomodoro-linked capture\n  BOB_DIR            Bob vault root when --bob-dir is omitted\n  BOB_NOW            current date/time override\n\nClipboard source order:\n  BOB_CLIPBOARD_CMD; macOS pbpaste; Linux wl-paste or xclip/xsel; tmux show-buffer",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(clip_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(no_clip_arg())
        .arg(route_arg())
        .arg(section_arg())
        .arg(text_arg())
}

fn clip_arg() -> Arg {
    Arg::new("clip")
        .long("clip")
        .short('c')
        .value_name("HEADER")
        .num_args(0..=1)
        .require_equals(true)
        .default_missing_value(capture_clip::DEFAULT_HEADER)
        .conflicts_with("no-clip")
        .help("Capture the clipboard, optionally labeling it HEADER")
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
        .help("Plan and report without writing notes or clipboard files")
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

fn no_clip_arg() -> Arg {
    Arg::new("no-clip")
        .long("no-clip")
        .short('n')
        .action(ArgAction::SetTrue)
        .conflicts_with("clip")
        .help("Keep trailing % clipboard markers literal")
}

fn route_arg() -> Arg {
    Arg::new("route")
        .long("route")
        .short('r')
        .value_name("NAME")
        .help("Force the route to NAME.md and keep @tokens in text literal")
}

fn section_arg() -> Arg {
    Arg::new("section")
        .long("section")
        .short('s')
        .value_name("TITLE")
        .help("Force a bullet into the exact section TITLE; requires --route")
}

fn text_arg() -> Arg {
    Arg::new("text")
        .value_name("TEXT")
        .num_args(0..)
        .trailing_var_arg(true)
        .allow_hyphen_values(true)
        .value_parser(OsStringValueParser::new())
        .help("Task text; multiple args are joined with spaces")
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
struct CaptureRequest {
    bob_dir: PathBuf,
    dry_run: bool,
    forced_clip_header: Option<String>,
    forced_route: Option<String>,
    forced_section: Option<String>,
    no_clip: bool,
    raw_text: String,
}

impl CaptureRequest {
    fn from_matches(matches: &ArgMatches) -> Result<Self, CaptureError> {
        let forced_clip_header = matches.get_one::<String>("clip").cloned();
        if let Some(header) = forced_clip_header.as_deref()
            && !capture_clip::is_valid_header(header)
        {
            return Err(CaptureError::usage(
                "--clip HEADER must contain only A-Z, a-z, 0-9, '_' or '-'",
            ));
        }
        let forced_route = matches.get_one::<String>("route").cloned();
        let forced_section = forced_section_from_matches(matches)?;
        if forced_section.is_some() && forced_route.is_none() {
            return Err(CaptureError::usage("--section requires --route"));
        }

        Ok(Self {
            bob_dir: bob_dir_from_matches(matches),
            dry_run: matches.get_flag("dry-run"),
            forced_clip_header,
            forced_route,
            forced_section,
            no_clip: matches.get_flag("no-clip"),
            raw_text: raw_text_from_matches(matches)?,
        })
    }
}

fn forced_section_from_matches(
    matches: &ArgMatches,
) -> Result<Option<String>, CaptureError> {
    let Some(section) = matches.get_one::<String>("section") else {
        return Ok(None);
    };
    if section.trim().is_empty() {
        return Err(CaptureError::usage("--section must not be empty"));
    }
    Ok(Some(section.clone()))
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

fn raw_text_from_matches(matches: &ArgMatches) -> Result<String, CaptureError> {
    if let Some(values) = matches.get_many::<OsString>("text") {
        return Ok(values
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" "));
    }

    if io::stdin().is_terminal() {
        return Ok(String::new());
    }

    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| CaptureError::io(format!("read stdin: {error}")))?;
    Ok(line)
}

fn capture(request: CaptureRequest) -> Result<CaptureResult, CaptureError> {
    let parse_clip_markers =
        request.forced_clip_header.is_none() && !request.no_clip;
    let mut parsed = parse_capture_text_with_clip_control(
        &request.raw_text,
        request.forced_route.as_deref(),
        request.forced_section.as_deref(),
        parse_clip_markers,
    )?;
    if let Some(header) = request.forced_clip_header.as_deref() {
        parsed.clip_header = Some(header.to_string());
    }
    let now = bob_env::current_datetime();
    let today = now.date();
    let created = date_string(today);
    let scheduled = parsed
        .scheduled_offset
        .map(|offset| scheduled_date_string(today, offset))
        .transpose()?;
    let capture_line = match &parsed.kind {
        CaptureKind::Task => {
            format_task_line(&parsed.body, &created, scheduled.as_deref())
        }
        CaptureKind::Bullet { .. } => {
            format_bullet_line(&parsed.body, &created, scheduled.as_deref())
        }
        CaptureKind::Pomodoro { block_id } => format_pomodoro_task_line(
            &parsed.body,
            &created,
            scheduled.as_deref(),
            block_id,
        ),
    };
    let kind_label = capture_kind_label(&parsed.kind);
    let clip_plan = parsed
        .clip_header
        .as_deref()
        .map(|header| {
            let clipboard =
                capture_clip::read_clipboard().map_err(CaptureError::io)?;
            capture_clip::plan(&request.bob_dir, header, &clipboard, now)
                .map_err(CaptureError::io)
        })
        .transpose()?;
    let capture_block = match &clip_plan {
        Some(plan) => {
            format!("{capture_line}\n{}", plan.output.lines.join("\n"))
        }
        None => capture_line.clone(),
    };
    let relative_target = relative_target(parsed.route.as_deref());
    let target = request.bob_dir.join(&relative_target);
    let note_plan = match &parsed.kind {
        CaptureKind::Pomodoro { block_id } => {
            let route = parsed.route.as_deref().ok_or_else(|| {
                CaptureError::io(
                    "Pomodoro capture invariant failed: route is missing",
                )
            })?;
            plan_capture_with_pomodoro_link(
                &request.bob_dir,
                &target,
                route,
                block_id,
                &capture_block,
            )?
        }
        _ => plan_capture_to_target(&target, &capture_block, &parsed.kind)?,
    };
    if !request.dry_run {
        let created_clip_files = match &clip_plan {
            Some(plan) => plan.save().map_err(CaptureError::io)?,
            None => Vec::new(),
        };
        if let Err(mut error) = write_capture_plan(&note_plan) {
            if !created_clip_files.is_empty() {
                let cleanup =
                    capture_clip::cleanup_created(&created_clip_files);
                capture_clip::append_cleanup_message(
                    &mut error.message,
                    &cleanup,
                );
            }
            return Err(error);
        }
    }
    let special = note_plan.pomodoro.as_ref();

    Ok(CaptureResult {
        ok: true,
        dry_run: request.dry_run,
        routed: parsed.route.is_some(),
        route_label: parsed
            .route
            .as_deref()
            .map(route_label)
            .unwrap_or_default(),
        route: parsed.route,
        relative_target: relative_target.to_string_lossy().into_owned(),
        target: target.display().to_string(),
        text: parsed.body,
        task_line: capture_line,
        kind: kind_label,
        created,
        scheduled,
        placement: note_plan.placement,
        clip: clip_plan.map(|plan| plan.output),
        block_id: special.as_ref().map(|edit| edit.details.block_id.clone()),
        day_file: special.as_ref().map(|edit| edit.details.day_file.clone()),
        block_link: special
            .as_ref()
            .map(|edit| edit.details.block_link.clone()),
        pomodoro_link_placement: special
            .as_ref()
            .map(|edit| edit.details.pomodoro_link_placement),
    })
}

fn capture_kind_label(kind: &CaptureKind) -> &'static str {
    match kind {
        CaptureKind::Task => "task",
        CaptureKind::Bullet { .. } => "bullet",
        CaptureKind::Pomodoro { .. } => "pomodoro_task",
    }
}

fn date_string(date: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

fn scheduled_date_string(
    today: NaiveDate,
    offset_days: u64,
) -> Result<String, CaptureError> {
    let scheduled =
        today
            .checked_add_days(Days::new(offset_days))
            .ok_or_else(|| {
                CaptureError::usage("scheduled offset is out of range")
            })?;
    Ok(date_string(scheduled))
}

fn relative_target(route: Option<&str>) -> PathBuf {
    route
        .map(|route| PathBuf::from(route_label(route)))
        .unwrap_or_else(|| PathBuf::from(INBOX_FILE))
}

pub(crate) fn inbox_route() -> &'static str {
    INBOX_FILE.strip_suffix(".md").unwrap_or(INBOX_FILE)
}

pub(crate) fn route_label(route: &str) -> String {
    format!("{route}.md")
}

fn format_task_line(
    body: &str,
    created: &str,
    scheduled: Option<&str>,
) -> String {
    let mut line = format!("- [ ] #task {body} [created::{created}]");
    append_scheduled_property(&mut line, scheduled);
    line
}

fn format_bullet_line(
    body: &str,
    created: &str,
    scheduled: Option<&str>,
) -> String {
    let mut line = format!("- {body} [created::{created}]");
    append_scheduled_property(&mut line, scheduled);
    line
}

fn format_pomodoro_task_line(
    body: &str,
    created: &str,
    scheduled: Option<&str>,
    block_id: &str,
) -> String {
    let mut line = format!("- [*] #task {body} [created::{created}]");
    append_scheduled_property(&mut line, scheduled);
    line.push_str(&format!(" ^{block_id}"));
    line
}

fn append_scheduled_property(line: &mut String, scheduled: Option<&str>) {
    if let Some(scheduled) = scheduled {
        line.push_str(&format!(" [scheduled::{scheduled}]"));
    }
}

fn plan_capture_to_target(
    target: &Path,
    capture_block: &str,
    kind: &CaptureKind,
) -> Result<CaptureWritePlan, CaptureError> {
    if !target.exists() {
        validate_target_parent(target)?;
        return Ok(CaptureWritePlan {
            target: target.to_path_buf(),
            target_existed: false,
            original_target: String::new(),
            updated_target: format!("{capture_block}\n"),
            placement: Placement::Created,
            pomodoro: None,
        });
    }

    let contents = read_target(target)?;
    let (updated, placement) = match kind {
        CaptureKind::Task | CaptureKind::Pomodoro { .. } => {
            insert_task_line(&contents, capture_block)
        }
        CaptureKind::Bullet {
            section_prefix,
            exact,
        } => insert_bullet_line(
            &contents,
            capture_block,
            section_prefix.as_deref(),
            *exact,
        ),
    };
    Ok(CaptureWritePlan {
        target: target.to_path_buf(),
        target_existed: true,
        original_target: contents,
        updated_target: updated,
        placement,
        pomodoro: None,
    })
}

#[derive(Debug)]
struct CaptureWritePlan {
    target: PathBuf,
    target_existed: bool,
    original_target: String,
    updated_target: String,
    placement: Placement,
    pomodoro: Option<PlannedPomodoroEdit>,
}

#[derive(Debug)]
struct PlannedPomodoroEdit {
    details: PomodoroCaptureDetails,
    day_file: PathBuf,
    updated_day: String,
}

#[derive(Debug)]
struct PomodoroCaptureDetails {
    block_id: String,
    day_file: String,
    block_link: String,
    pomodoro_link_placement: Placement,
}

fn plan_capture_with_pomodoro_link(
    bob_dir: &Path,
    target: &Path,
    route: &str,
    block_id: &str,
    capture_block: &str,
) -> Result<CaptureWritePlan, CaptureError> {
    let target_existed = target.exists();
    let original_target = if target_existed {
        read_target(target)?
    } else {
        String::new()
    };
    if collect_done::block_ids_in_markdown(&original_target)
        .iter()
        .any(|existing| existing == block_id)
    {
        return Err(CaptureError::io(format!(
            "block ID ^{block_id} already exists in {}",
            target.display()
        )));
    }

    let (updated_target, placement) = if target_existed {
        insert_task_line(&original_target, capture_block)
    } else {
        validate_target_parent(target)?;
        (format!("{capture_block}\n"), Placement::Created)
    };

    let day_file = pomodoro::day_file_for(bob_dir);
    if !day_file.is_file() {
        return Err(CaptureError::io(format!(
            "Bob daily note does not exist: {}",
            day_file.display()
        )));
    }
    if paths_refer_to_same_file(target, &day_file) {
        return Err(CaptureError::io(
            "routed note and Bob daily note must be different files",
        ));
    }

    let original_day = read_target(&day_file)?;
    let block_link = format!("[[{route}#^{block_id}]]");
    if original_day.contains(&block_link) {
        return Err(CaptureError::io(format!(
            "Pomodoro ledger already contains {block_link}"
        )));
    }
    let (updated_day, pomodoro_link_placement) =
        insert_pomodoro_block_link(&original_day, &block_link)?;

    Ok(CaptureWritePlan {
        target: target.to_path_buf(),
        target_existed,
        original_target,
        updated_target,
        placement,
        pomodoro: Some(PlannedPomodoroEdit {
            details: PomodoroCaptureDetails {
                block_id: block_id.to_string(),
                day_file: day_file.display().to_string(),
                block_link,
                pomodoro_link_placement,
            },
            day_file,
            updated_day,
        }),
    })
}

fn validate_target_parent(target: &Path) -> Result<(), CaptureError> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    if parent.is_dir() {
        Ok(())
    } else {
        Err(CaptureError::io(format!(
            "create target {}: Bob vault root does not exist: {}",
            target.display(),
            parent.display(),
        )))
    }
}

fn write_capture_plan(plan: &CaptureWritePlan) -> Result<(), CaptureError> {
    if let Some(pomodoro) = &plan.pomodoro {
        return coordinated_replace(
            &plan.target,
            plan.target_existed.then_some(plan.original_target.as_str()),
            &plan.updated_target,
            &pomodoro.day_file,
            &pomodoro.updated_day,
        );
    }

    replace_single_file(&plan.target, plan.target_existed, &plan.updated_target)
}

fn replace_single_file(
    target: &Path,
    target_existed: bool,
    updated: &str,
) -> Result<(), CaptureError> {
    let temporary = write_temporary_file(target, updated, "target")?;
    if let Err(error) = fs::rename(&temporary, target) {
        remove_temporary_file(&temporary);
        return Err(fs_error("replace target", target, error));
    }
    if !target_existed && !target.is_file() {
        return Err(CaptureError::io(format!(
            "create target {} failed",
            target.display()
        )));
    }
    Ok(())
}

fn paths_refer_to_same_file(first: &Path, second: &Path) -> bool {
    if first == second {
        return true;
    }
    match (fs::canonicalize(first), fs::canonicalize(second)) {
        (Ok(first), Ok(second)) => first == second,
        _ => false,
    }
}

fn insert_pomodoro_block_link(
    contents: &str,
    block_link: &str,
) -> Result<(String, Placement), CaptureError> {
    let lines = line_spans(contents);
    let line_text = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let section =
        pomodoro::pomodoros_section_range(&line_text).ok_or_else(|| {
            CaptureError::io("Bob daily note has no Pomodoros section")
        })?;

    let mut open = Vec::new();
    let mut timed = Vec::new();
    let fenced = super::markdown::fenced_lines(&line_text, section.clone());
    for index in section.clone() {
        if fenced.contains(&index) {
            continue;
        }
        let line = lines[index].text;
        if is_indented_line(line) {
            continue;
        }
        let Some(task) = pomodoro::open_ledger_task(line) else {
            continue;
        };
        open.push(index);
        if pomodoro::task_time_range(task).is_some() {
            timed.push(index);
        }
    }

    if timed.len() > 1 {
        return Err(CaptureError::io(
            "Bob daily note has multiple open timed Pomodoros",
        ));
    }
    let selected = timed
        .first()
        .copied()
        .or_else(|| open.first().copied())
        .ok_or_else(|| {
            CaptureError::io("Bob daily note has no eligible open Pomodoro")
        })?;
    let insertion_index = task_block_end(&lines, selected);
    let indentation =
        child_bullet_indentation(&lines, selected + 1, insertion_index)
            .or_else(|| {
                nearby_child_bullet_indentation(
                    &lines,
                    section.start,
                    section.end,
                )
            })
            .unwrap_or_else(|| "  ".to_string());
    let line = format!("{indentation}- {block_link}");
    let addition = insertion_text_preserving_line_endings(
        contents,
        insertion_index,
        &line,
    );
    let placement = if insertion_index >= contents.len() {
        Placement::Appended
    } else {
        Placement::Inserted
    };
    Ok((insert_at(contents, insertion_index, &addition), placement))
}

fn child_bullet_indentation(
    lines: &[LineSpan<'_>],
    start_line: usize,
    insertion_index: usize,
) -> Option<String> {
    lines[start_line..]
        .iter()
        .take_while(|line| line.end <= insertion_index)
        .find_map(|line| unordered_child_indentation(line.text))
}

fn nearby_child_bullet_indentation(
    lines: &[LineSpan<'_>],
    start_line: usize,
    end_line: usize,
) -> Option<String> {
    lines[start_line..end_line]
        .iter()
        .find_map(|line| unordered_child_indentation(line.text))
}

fn unordered_child_indentation(line: &str) -> Option<String> {
    let indentation_len = line
        .as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    if indentation_len == 0 {
        return None;
    }
    let rest = &line[indentation_len..];
    matches!(rest.as_bytes(), [b'-' | b'*' | b'+', b' ', ..])
        .then(|| line[..indentation_len].to_string())
}

fn insertion_text_preserving_line_endings(
    contents: &str,
    index: usize,
    line: &str,
) -> String {
    let ending = document_line_ending(contents);
    let line = line.replace('\n', ending);
    let needs_leading_ending = index > 0 && !contents[..index].ends_with('\n');
    if needs_leading_ending {
        format!("{ending}{line}{ending}")
    } else {
        format!("{line}{ending}")
    }
}

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn coordinated_replace(
    target: &Path,
    original_target: Option<&str>,
    updated_target: &str,
    day_file: &Path,
    updated_day: &str,
) -> Result<(), CaptureError> {
    let target_temp = write_temporary_file(target, updated_target, "target")?;
    let day_temp = match write_temporary_file(day_file, updated_day, "day") {
        Ok(path) => path,
        Err(error) => {
            remove_temporary_file(&target_temp);
            return Err(error);
        }
    };
    let target_backup = match original_target {
        Some(contents) => {
            match write_temporary_file(target, contents, "backup") {
                Ok(path) => Some(path),
                Err(error) => {
                    remove_temporary_file(&target_temp);
                    remove_temporary_file(&day_temp);
                    return Err(error);
                }
            }
        }
        None => None,
    };

    if let Err(error) = fs::rename(&target_temp, target) {
        remove_temporary_file(&target_temp);
        remove_temporary_file(&day_temp);
        if let Some(backup) = &target_backup {
            remove_temporary_file(backup);
        }
        return Err(fs_error("replace target", target, error));
    }

    if let Err(error) = fs::rename(&day_temp, day_file) {
        let rollback = match &target_backup {
            Some(backup) => fs::rename(backup, target),
            None => fs::remove_file(target),
        };
        remove_temporary_file(&day_temp);
        let mut message =
            format!("replace daily note {}: {error}", day_file.display());
        if let Err(rollback_error) = rollback {
            message.push_str(&format!(
                "; rollback of {} also failed: {rollback_error}{}",
                target.display(),
                target_backup
                    .as_ref()
                    .map(|backup| format!(
                        "; original remains at {}",
                        backup.display()
                    ))
                    .unwrap_or_default()
            ));
        }
        return Err(CaptureError::io(message));
    }

    if let Some(backup) = &target_backup {
        remove_temporary_file(backup);
    }
    Ok(())
}

fn write_temporary_file(
    destination: &Path,
    contents: &str,
    role: &str,
) -> Result<PathBuf, CaptureError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("note");

    for _ in 0..100 {
        let sequence = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{file_name}.bob-capture-{}-{sequence}-{role}.tmp",
            std::process::id()
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                continue;
            }
            Err(error) => {
                return Err(fs_error(
                    "create temporary file for",
                    destination,
                    error,
                ));
            }
        };
        if let Ok(metadata) = fs::metadata(destination)
            && let Err(error) = file.set_permissions(metadata.permissions())
        {
            remove_temporary_file(&path);
            return Err(fs_error(
                "set temporary file permissions for",
                destination,
                error,
            ));
        }
        if let Err(error) = file.write_all(contents.as_bytes()) {
            remove_temporary_file(&path);
            return Err(fs_error(
                "write temporary file for",
                destination,
                error,
            ));
        }
        if let Err(error) = file.sync_all() {
            remove_temporary_file(&path);
            return Err(fs_error(
                "sync temporary file for",
                destination,
                error,
            ));
        }
        return Ok(path);
    }

    Err(CaptureError::io(format!(
        "could not allocate temporary file for {}",
        destination.display()
    )))
}

fn remove_temporary_file(path: &Path) {
    let _ = fs::remove_file(path);
}

fn read_target(target: &Path) -> Result<String, CaptureError> {
    fs::read_to_string(target)
        .map_err(|error| fs_error("read target", target, error))
}

fn fs_error(action: &str, path: &Path, error: io::Error) -> CaptureError {
    CaptureError::io(format!("{action} {}: {error}", path.display()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CaptureKind {
    Task,
    Bullet {
        section_prefix: Option<String>,
        exact: bool,
    },
    Pomodoro {
        block_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedCaptureText {
    body: String,
    clip_header: Option<String>,
    route: Option<String>,
    kind: CaptureKind,
    scheduled_offset: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TerminalMarkers {
    clip_header: Option<String>,
    scheduled_offset: Option<u64>,
}

struct RouteToken {
    route: String,
    kind: CaptureKind,
}

impl RouteToken {
    fn into_parsed(
        self,
        body: String,
        markers: TerminalMarkers,
    ) -> ParsedCaptureText {
        ParsedCaptureText {
            body,
            clip_header: markers.clip_header,
            route: Some(self.route),
            kind: self.kind,
            scheduled_offset: markers.scheduled_offset,
        }
    }
}

#[cfg(test)]
fn parse_capture_text(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
) -> Result<ParsedCaptureText, CaptureError> {
    parse_capture_text_with_clip_control(
        raw_text,
        forced_route,
        forced_section,
        true,
    )
}

fn parse_capture_text_with_clip_control(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    parse_clip_markers: bool,
) -> Result<ParsedCaptureText, CaptureError> {
    let normalized = normalize_task_text(raw_text);
    if normalized.is_empty() {
        return Err(missing_text_error());
    }

    let mut tokens: Vec<&str> = normalized.split(' ').collect();
    let markers = extract_terminal_markers(&mut tokens, parse_clip_markers);
    if tokens.is_empty() {
        return Err(missing_text_error());
    }
    let body = tokens.join(" ");

    if let Some(section) = forced_section {
        let Some(route) = forced_route else {
            return Err(CaptureError::usage("--section requires --route"));
        };
        if section.trim().is_empty() {
            return Err(CaptureError::usage("--section must not be empty"));
        }
        let route = normalize_forced_route(route)?;
        reject_legacy_bullet_markers(&tokens, false)?;
        return Ok(ParsedCaptureText {
            body,
            clip_header: markers.clip_header,
            route: Some(route),
            kind: CaptureKind::Bullet {
                section_prefix: Some(section.to_string()),
                exact: true,
            },
            scheduled_offset: markers.scheduled_offset,
        });
    }

    if let Some(route) = forced_route {
        let route = normalize_forced_route(route)?;
        reject_legacy_bullet_markers(&tokens, false)?;
        return Ok(ParsedCaptureText {
            body,
            clip_header: markers.clip_header,
            route: Some(route),
            kind: CaptureKind::Task,
            scheduled_offset: markers.scheduled_offset,
        });
    }

    reject_legacy_bullet_markers(&tokens, true)?;

    // Leading route wins: when the first token is a route token followed by
    // body text, route by it and do not inspect later route-looking tokens.
    if let Some(token) = parse_terminal_route_token(tokens[0])? {
        let body = tokens[1..].join(" ");
        if body.is_empty() {
            if !matches!(token.kind, CaptureKind::Task) {
                return Err(missing_text_error());
            }
            // A bare `@foo` with no body stays literal task text.
        } else {
            return Ok(token.into_parsed(body, markers));
        }
    }

    validate_special_terminal_markers(&tokens)?;

    // Otherwise a trailing route token routes the body that precedes it.
    if let Some((&last, rest)) = tokens.split_last()
        && !rest.is_empty()
        && let Some(token) = parse_terminal_route_token(last)?
    {
        return Ok(token.into_parsed(rest.join(" "), markers));
    }

    Ok(ParsedCaptureText {
        body,
        clip_header: markers.clip_header,
        route: None,
        kind: CaptureKind::Task,
        scheduled_offset: markers.scheduled_offset,
    })
}

fn missing_text_error() -> CaptureError {
    CaptureError::usage(
        "task text is required; pass TEXT or pipe one line on stdin",
    )
}

fn legacy_marker_error() -> CaptureError {
    CaptureError::usage(
        "bullet section markers must be appended to an @route token; use \
@foo#bar instead of #bar @foo",
    )
}

fn normalize_task_text(raw_text: &str) -> String {
    raw_text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse one whitespace-free token as an `@route` token, returning `None` when
/// it does not begin with `@` or its route part is not a valid route name. A
/// `#` suffix selects bullet mode and is split off before route validation.
fn parse_route_token(token: &str) -> Option<RouteToken> {
    let rest = token.strip_prefix('@')?;
    let (route_part, bullet) = match rest.split_once('#') {
        Some((route, prefix)) => {
            let marker = (!prefix.is_empty()).then(|| prefix.to_string());
            (route, Some(marker))
        }
        None => (rest, None),
    };
    is_route_token(route_part).then(|| RouteToken {
        route: route_part.to_ascii_lowercase(),
        kind: match bullet {
            Some(section_prefix) => CaptureKind::Bullet {
                section_prefix,
                exact: false,
            },
            None => CaptureKind::Task,
        },
    })
}

fn parse_terminal_route_token(
    token: &str,
) -> Result<Option<RouteToken>, CaptureError> {
    if is_pomodoro_marker_candidate(token) {
        return parse_pomodoro_route_token(token).map(Some);
    }
    Ok(parse_route_token(token))
}

fn parse_pomodoro_route_token(token: &str) -> Result<RouteToken, CaptureError> {
    let marker = token
        .strip_prefix("@!")
        .or_else(|| token.strip_prefix('@'))
        .ok_or_else(|| {
            CaptureError::usage(
                "Pomodoro capture markers must use @<route>:<block-id>",
            )
        })?;
    let Some((route, block_id)) = marker.split_once(':') else {
        return Err(CaptureError::usage(
            "Pomodoro capture markers must use @<route>:<block-id>",
        ));
    };
    if !is_route_token(route) {
        return Err(CaptureError::usage(
            "Pomodoro capture route must contain only A-Z, a-z, 0-9, '_' or '-'",
        ));
    }
    if !is_block_id(block_id) {
        return Err(CaptureError::usage(
            "Pomodoro capture block ID must be non-empty and contain only A-Z, a-z, 0-9, '_' or '-'",
        ));
    }

    Ok(RouteToken {
        route: route.to_ascii_lowercase(),
        kind: CaptureKind::Pomodoro {
            block_id: block_id.to_string(),
        },
    })
}

/// Return whether a terminal token belongs to the Pomodoro-marker grammar.
/// A colon that follows `#` remains part of an ordinary bullet section prefix.
fn is_pomodoro_marker_candidate(token: &str) -> bool {
    let Some(marker) =
        token.strip_prefix("@!").or_else(|| token.strip_prefix('@'))
    else {
        return false;
    };
    if token.starts_with("@!") {
        return true;
    }

    let colon = marker.find(':');
    let hash = marker.find('#');
    colon.is_some_and(|colon| {
        hash.is_none_or(|hash| colon < hash)
            && marker[..colon]
                .bytes()
                .any(|byte| byte.is_ascii_alphabetic())
    })
}

fn validate_special_terminal_markers(
    tokens: &[&str],
) -> Result<(), CaptureError> {
    for token in tokens.first().into_iter().chain(tokens.last()) {
        if is_pomodoro_marker_candidate(token) {
            parse_pomodoro_route_token(token)?;
        }
    }
    Ok(())
}

fn is_block_id(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(collect_done::is_block_id_byte)
}

/// Parse one whitespace-free token as a schedule offset (`s:<N>`), returning
/// the non-negative day count. Invalid or overflowing tokens stay literal.
fn parse_schedule_token(token: &str) -> Option<u64> {
    let digits = token.strip_prefix("s:")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()
}

fn parse_clip_token(token: &str) -> Option<String> {
    let header = token.strip_prefix('%')?;
    if header.is_empty() {
        return Some(capture_clip::DEFAULT_HEADER.to_string());
    }
    capture_clip::is_valid_header(header).then(|| header.to_string())
}

/// Remove schedule and clipboard markers from the terminal marker region.
/// Each marker kind is extracted at most once, in either order, on either
/// side of a trailing route token. A duplicate or non-marker stops parsing.
fn extract_terminal_markers(
    tokens: &mut Vec<&str>,
    parse_clip_markers: bool,
) -> TerminalMarkers {
    let marker_like = |token: &str| {
        parse_schedule_token(token).is_some()
            || (parse_clip_markers && parse_clip_token(token).is_some())
    };
    let route_index =
        if tokens.last().is_some_and(|token| is_route_marker(token)) {
            Some(tokens.len() - 1)
        } else {
            let mut index = tokens.len();
            while index > 0 && marker_like(tokens[index - 1]) {
                index -= 1;
            }
            if index > 0 && is_route_marker(tokens[index - 1]) {
                Some(index - 1)
            } else {
                None
            }
        };
    let mut cursor = match route_index {
        Some(index) if index == tokens.len() - 1 => index,
        _ => tokens.len(),
    };
    let route_before_trailing_markers =
        route_index.is_some_and(|index| index < tokens.len() - 1);
    let mut markers = TerminalMarkers::default();
    let mut reached_route = false;

    while cursor > 0 {
        let index = cursor - 1;
        if route_index == Some(index) {
            reached_route = true;
            break;
        }
        let token = tokens[index];
        if !extract_terminal_marker(token, parse_clip_markers, &mut markers) {
            break;
        }
        tokens.remove(index);
        cursor -= 1;
    }

    if reached_route && route_before_trailing_markers {
        cursor = route_index.expect("reached route");
        while cursor > 0 {
            let index = cursor - 1;
            let token = tokens[index];
            if !extract_terminal_marker(token, parse_clip_markers, &mut markers)
            {
                break;
            }
            tokens.remove(index);
            cursor -= 1;
        }
    }

    markers
}

fn extract_terminal_marker(
    token: &str,
    parse_clip_markers: bool,
    markers: &mut TerminalMarkers,
) -> bool {
    if let Some(offset) = parse_schedule_token(token) {
        if markers.scheduled_offset.is_some() {
            return false;
        }
        markers.scheduled_offset = Some(offset);
        return true;
    }
    if parse_clip_markers
        && let Some(header) = parse_clip_token(token)
        && markers.clip_header.is_none()
    {
        markers.clip_header = Some(header);
        return true;
    }
    false
}

#[cfg(test)]
fn extract_trailing_schedule(tokens: &mut Vec<&str>) -> Option<u64> {
    extract_terminal_markers(tokens, false).scheduled_offset
}

fn is_route_marker(token: &str) -> bool {
    parse_route_token(token).is_some()
        || (is_pomodoro_marker_candidate(token)
            && parse_pomodoro_route_token(token).is_ok())
}

/// Reject the retired standalone bullet marker forms so they fail clearly
/// instead of silently capturing literal `#...` text. The marker is honored
/// only when appended to an `@route` token (`@foo#bar`).
///
/// Two terminal positions are rejected: a final token that itself starts with
/// `#`, and (when `allow_route`) a final plain `@route` token preceded by a
/// `#...` token. A `#tag` anywhere else stays literal task text.
fn reject_legacy_bullet_markers(
    tokens: &[&str],
    allow_route: bool,
) -> Result<(), CaptureError> {
    let Some(&last) = tokens.last() else {
        return Ok(());
    };

    if last.starts_with('#') {
        return Err(legacy_marker_error());
    }

    if allow_route
        && tokens.len() >= 2
        && tokens[tokens.len() - 2].starts_with('#')
        && parse_route_token(last)
            .is_some_and(|token| matches!(token.kind, CaptureKind::Task))
    {
        return Err(legacy_marker_error());
    }

    Ok(())
}

fn normalize_forced_route(route: &str) -> Result<String, CaptureError> {
    if is_route_token(route) {
        return Ok(route.to_ascii_lowercase());
    }

    Err(CaptureError::usage(
        "--route must contain only A-Z, a-z, 0-9, '_' or '-'",
    ))
}

pub(crate) fn is_route_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
}

fn insert_task_line(contents: &str, task_line: &str) -> (String, Placement) {
    let lines = line_spans(contents);
    if let Some(section) = tasks_section(&lines) {
        let index = last_task_block_insert_index_in_range(
            &lines,
            section.start_line,
            section.end_line,
        )
        .unwrap_or(section.heading_end);
        let addition = if index == section.heading_end {
            empty_section_insertion_text(contents, index, task_line)
        } else {
            insertion_text(contents, index, task_line)
        };
        return (insert_at(contents, index, &addition), Placement::Inserted);
    }

    let Some(index) =
        last_task_block_insert_index_in_range(&lines, 0, lines.len())
    else {
        let addition = insertion_text(contents, contents.len(), task_line);
        return (
            insert_at(contents, contents.len(), &addition),
            Placement::Appended,
        );
    };

    let addition = insertion_text(contents, index, task_line);
    (insert_at(contents, index, &addition), Placement::Inserted)
}

fn insert_at(contents: &str, index: usize, addition: &str) -> String {
    let mut updated = String::with_capacity(contents.len() + addition.len());
    updated.push_str(&contents[..index]);
    updated.push_str(addition);
    updated.push_str(&contents[index..]);
    updated
}

fn insertion_text(contents: &str, index: usize, line: &str) -> String {
    let ending = document_line_ending(contents);
    let line = line.replace('\n', ending);
    let needs_leading_newline = index > 0 && !contents[..index].ends_with('\n');
    if needs_leading_newline {
        format!("{ending}{line}{ending}")
    } else {
        format!("{line}{ending}")
    }
}

fn empty_section_insertion_text(
    contents: &str,
    index: usize,
    line: &str,
) -> String {
    let ending = document_line_ending(contents);
    let line = line.replace('\n', ending);
    if index > 0 && contents[..index].ends_with('\n') {
        format!("{ending}{line}{ending}")
    } else {
        format!("{ending}{ending}{line}{ending}")
    }
}

fn document_line_ending(contents: &str) -> &'static str {
    if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn last_task_block_insert_index_in_range(
    lines: &[LineSpan<'_>],
    start_line: usize,
    end_line: usize,
) -> Option<usize> {
    let mut last_index = None;
    for (index, line) in lines[start_line..end_line].iter().enumerate() {
        if is_top_level_task_line(line.text) {
            last_index = Some(task_block_end(lines, start_line + index));
        }
    }
    last_index
}

fn insert_bullet_line(
    contents: &str,
    bullet_line: &str,
    section_prefix: Option<&str>,
    exact: bool,
) -> (String, Placement) {
    let lines = line_spans(contents);
    let headings = markdown_headings(&lines);
    let section =
        target_bullet_section(&lines, &headings, section_prefix, exact);

    if let Some(index) = last_bullet_block_insert_index_in_range(
        &lines,
        section.start_line,
        section.end_line,
    ) {
        let addition = insertion_text(contents, index, bullet_line);
        return (insert_at(contents, index, &addition), Placement::Inserted);
    }

    match section.heading_end {
        Some(heading_end) => {
            let addition = empty_section_insertion_text(
                contents,
                heading_end,
                bullet_line,
            );
            (
                insert_at(contents, heading_end, &addition),
                Placement::Inserted,
            )
        }
        None => {
            let index = section.insertion_start;
            let addition = insertion_text(contents, index, bullet_line);
            let placement = if index >= contents.len() {
                Placement::Appended
            } else {
                Placement::Inserted
            };
            (insert_at(contents, index, &addition), placement)
        }
    }
}

/// A Markdown section the bullet capture can target.
///
/// `heading_end` is the byte offset just past the heading line, or `None` for
/// the zeroth (pre-heading) section. `start_line`/`end_line` bound the section
/// body for bullet scanning, and `insertion_start` is where an empty zeroth
/// section receives its first bullet.
#[derive(Debug, Clone, Copy)]
struct MarkdownSection {
    heading_end: Option<usize>,
    start_line: usize,
    end_line: usize,
    insertion_start: usize,
}

fn target_bullet_section(
    lines: &[LineSpan<'_>],
    headings: &[MarkdownHeading<'_>],
    section_prefix: Option<&str>,
    exact: bool,
) -> MarkdownSection {
    let matches = |heading: &MarkdownHeading<'_>| {
        heading.title != "Tasks"
            && heading_matches_bullet_selector(
                heading.title,
                section_prefix,
                exact,
            )
    };
    // Prefer the first matching non-H1 heading, falling back to the first
    // matching H1 heading only when no non-H1 heading matches.
    let target = headings
        .iter()
        .position(|heading| heading.level != 1 && matches(heading))
        .or_else(|| {
            headings
                .iter()
                .position(|heading| heading.level == 1 && matches(heading))
        });

    match target {
        Some(pos) => {
            let heading_index = headings[pos].line_index;
            let heading_end = lines[heading_index].end;
            let end_line = headings
                .get(pos + 1)
                .map(|heading| heading.line_index)
                .unwrap_or(lines.len());
            MarkdownSection {
                heading_end: Some(heading_end),
                start_line: heading_index + 1,
                end_line,
                insertion_start: heading_end,
            }
        }
        None => {
            let (start_line, insertion_start) = match frontmatter_span(lines) {
                Some((line_after, byte_end)) => (line_after, byte_end),
                None => (0, 0),
            };
            let end_line = headings
                .first()
                .map(|heading| heading.line_index)
                .unwrap_or(lines.len());
            MarkdownSection {
                heading_end: None,
                start_line,
                end_line,
                insertion_start,
            }
        }
    }
}

/// Whether `title` matches a bullet capture's section selector. A bare marker
/// (no selector) matches every heading; otherwise exact selectors compare the
/// whole title case insensitively, and prefix selectors compare against the
/// start of `title` case insensitively.
fn heading_matches_bullet_selector(
    title: &str,
    section_prefix: Option<&str>,
    exact: bool,
) -> bool {
    match section_prefix {
        None => true,
        Some(selector) => {
            let title = title.to_lowercase();
            let selector = selector.to_lowercase();
            if exact {
                title == selector
            } else {
                title.starts_with(&selector)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SectionHeading {
    pub(crate) title: String,
    pub(crate) level: usize,
}

pub(crate) fn non_tasks_section_headings(
    contents: &str,
) -> Vec<SectionHeading> {
    let lines = line_spans(contents);
    markdown_headings(&lines)
        .into_iter()
        .filter(|heading| heading.title != "Tasks")
        .map(|heading| SectionHeading {
            title: heading.title.to_string(),
            level: heading.level,
        })
        .collect()
}

fn last_bullet_block_insert_index_in_range(
    lines: &[LineSpan<'_>],
    start_line: usize,
    end_line: usize,
) -> Option<usize> {
    let mut last_index = None;
    for (offset, line) in lines[start_line..end_line].iter().enumerate() {
        if is_top_level_bullet_line(line.text) {
            last_index = Some(task_block_end(lines, start_line + offset));
        }
    }
    last_index
}

fn is_top_level_bullet_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("- ") else {
        return false;
    };
    !is_checkbox_marker(rest)
}

fn is_checkbox_marker(after_dash: &str) -> bool {
    let mut chars = after_dash.chars();
    chars.next() == Some('[')
        && chars.next().is_some()
        && chars.next() == Some(']')
}

/// An ATX heading discovered while scanning a note.
///
/// `line_index` is the heading's line, `level` is its ATX level (number of
/// leading `#`), and `title` is the stripped heading text.
#[derive(Debug, Clone, Copy)]
struct MarkdownHeading<'a> {
    line_index: usize,
    level: usize,
    title: &'a str,
}

/// Collect every ATX heading, skipping YAML frontmatter and fenced code blocks.
fn markdown_headings<'a>(lines: &[LineSpan<'a>]) -> Vec<MarkdownHeading<'a>> {
    let mut headings = Vec::new();
    let mut in_frontmatter = false;
    let mut fence = None;

    for (index, line) in lines.iter().enumerate() {
        if index == 0 && line.text.trim() == "---" {
            in_frontmatter = true;
            continue;
        }

        if in_frontmatter {
            if line.text.trim() == "---" {
                in_frontmatter = false;
            }
            continue;
        }

        if let Some(open_fence) = fence {
            if closes_fence(line.text, open_fence) {
                fence = None;
            }
            continue;
        }

        if let Some(open_fence) = fence_marker(line.text) {
            fence = Some(open_fence);
            continue;
        }

        if let Some((level, title)) = atx_heading(line.text) {
            headings.push(MarkdownHeading {
                line_index: index,
                level,
                title,
            });
        }
    }

    headings
}

/// Byte span of YAML frontmatter as `(line_after, end_byte)` when the document
/// opens with a closed `---` block.
fn frontmatter_span(lines: &[LineSpan<'_>]) -> Option<(usize, usize)> {
    if lines.first().map(|line| line.text.trim()) != Some("---") {
        return None;
    }

    lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.text.trim() == "---")
        .map(|(index, line)| (index + 1, line.end))
}

#[derive(Debug, Clone, Copy)]
struct TasksSection {
    heading_end: usize,
    start_line: usize,
    end_line: usize,
}

fn tasks_section(lines: &[LineSpan<'_>]) -> Option<TasksSection> {
    let headings = markdown_headings(lines);
    let pos = headings
        .iter()
        .position(|heading| heading.title == "Tasks")?;
    let heading_index = headings[pos].line_index;
    let end_line = headings
        .get(pos + 1)
        .map(|heading| heading.line_index)
        .unwrap_or(lines.len());
    Some(TasksSection {
        heading_end: lines[heading_index].end,
        start_line: heading_index + 1,
        end_line,
    })
}

#[derive(Debug, Clone, Copy)]
struct FenceMarker {
    character: u8,
    length: usize,
}

fn fence_marker(line: &str) -> Option<FenceMarker> {
    let (marker, _) = fence_sequence(line)?;
    Some(marker)
}

fn closes_fence(line: &str, open_fence: FenceMarker) -> bool {
    let Some((marker, remainder)) = fence_sequence(line) else {
        return false;
    };

    marker.character == open_fence.character
        && marker.length >= open_fence.length
        && remainder.trim().is_empty()
}

fn fence_sequence(line: &str) -> Option<(FenceMarker, &str)> {
    let line = markdown_indented_line(line)?;
    let bytes = line.as_bytes();
    let character = *bytes.first()?;
    if !matches!(character, b'`' | b'~') {
        return None;
    }

    let length = bytes.iter().take_while(|byte| **byte == character).count();
    if length < 3 {
        return None;
    }

    Some((FenceMarker { character, length }, &line[length..]))
}

/// Parse an ATX heading into its `(level, title)`, where `level` is the number
/// of leading `#` characters.
fn atx_heading(line: &str) -> Option<(usize, &str)> {
    let line = markdown_indented_line(line)?;
    let hashes = line
        .as_bytes()
        .iter()
        .take_while(|byte| **byte == b'#')
        .count();
    if !(1..=6).contains(&hashes) {
        return None;
    }

    if line
        .as_bytes()
        .get(hashes)
        .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        return None;
    }

    Some((hashes, strip_closing_atx_hashes(line[hashes..].trim())))
}

fn strip_closing_atx_hashes(title: &str) -> &str {
    let trimmed = title.trim_end();
    let without_hashes = trimmed.trim_end_matches('#');
    if without_hashes.len() == trimmed.len() {
        return trimmed;
    }

    if without_hashes
        .chars()
        .next_back()
        .map_or(true, char::is_whitespace)
    {
        without_hashes.trim_end()
    } else {
        trimmed
    }
}

fn markdown_indented_line(line: &str) -> Option<&str> {
    let spaces = line
        .as_bytes()
        .iter()
        .take_while(|byte| **byte == b' ')
        .count();
    if spaces > 3 {
        return None;
    }
    Some(&line[spaces..])
}

fn task_block_end(lines: &[LineSpan<'_>], task_index: usize) -> usize {
    let mut index = task_index + 1;
    while index < lines.len() {
        let line = lines[index].text;
        if is_indented_line(line)
            || (is_blank_line(line)
                && next_nonblank_is_indented(lines, index + 1))
        {
            index += 1;
            continue;
        }
        break;
    }
    lines[index - 1].end
}

fn next_nonblank_is_indented(
    lines: &[LineSpan<'_>],
    start_index: usize,
) -> bool {
    lines[start_index..]
        .iter()
        .find(|line| !is_blank_line(line.text))
        .is_some_and(|line| is_indented_line(line.text))
}

#[derive(Debug, Clone, Copy)]
struct LineSpan<'a> {
    end: usize,
    text: &'a str,
}

fn line_spans(contents: &str) -> Vec<LineSpan<'_>> {
    let mut spans = Vec::new();
    let mut start = 0;
    for segment in contents.split_inclusive('\n') {
        let end = start + segment.len();
        spans.push(LineSpan {
            end,
            text: logical_line(segment),
        });
        start = end;
    }
    spans
}

fn logical_line(segment: &str) -> &str {
    let without_lf = segment.strip_suffix('\n').unwrap_or(segment);
    without_lf.strip_suffix('\r').unwrap_or(without_lf)
}

fn is_top_level_task_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("- [") else {
        return false;
    };
    let mut chars = rest.chars();
    if chars.next().is_none() || chars.next() != Some(']') {
        return false;
    }
    let after_checkbox = chars.as_str();
    after_checkbox
        .chars()
        .next()
        .is_some_and(|character| character.is_whitespace())
        && after_checkbox.contains("#task")
}

fn is_indented_line(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

fn is_blank_line(line: &str) -> bool {
    line.trim().is_empty()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Placement {
    Created,
    Inserted,
    Appended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureResult {
    ok: bool,
    dry_run: bool,
    routed: bool,
    route: Option<String>,
    route_label: String,
    relative_target: String,
    target: String,
    text: String,
    task_line: String,
    kind: &'static str,
    created: String,
    scheduled: Option<String>,
    placement: Placement,
    #[serde(skip_serializing_if = "Option::is_none")]
    clip: Option<capture_clip::ClipOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    day_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_link_placement: Option<Placement>,
}

fn print_success(result: &CaptureResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureResult) {
    let styler = Styler::detect();
    let target_label = if result.route_label.is_empty() {
        result.relative_target.as_str()
    } else {
        result.route_label.as_str()
    };
    let target_label = styler.cyan(target_label);
    let verb = if result.dry_run {
        "would capture"
    } else {
        "captured"
    };
    let prefix = if result.dry_run {
        styler.success_prefix(true)
    } else {
        styler.green("\u{2713}")
    };
    println!("{prefix} {verb}  {target_label}");
    println!("  {}", styler.dim(&result.task_line));
    if let Some(clip) = &result.clip {
        for line in &clip.lines {
            println!("  {}", styler.dim(line));
        }
        for attachment in &clip.attachments {
            print_clip_file_confirmation(
                &styler,
                result.dry_run,
                &attachment.saved,
                attachment.reused,
            );
        }
        if let Some(snippet) = &clip.snippet {
            print_clip_file_confirmation(
                &styler,
                result.dry_run,
                snippet,
                false,
            );
        }
    }
    if let (Some(day_file), Some(block_link)) =
        (&result.day_file, &result.block_link)
    {
        let link_verb = if result.dry_run {
            "would link"
        } else {
            "linked"
        };
        println!("{prefix} {link_verb}   {}", styler.cyan(day_file));
        println!("  {}", styler.dim(&format!("- {block_link}")));
    }
}

fn print_clip_file_confirmation(
    styler: &Styler,
    dry_run: bool,
    saved: &str,
    reused: bool,
) {
    let prefix = if dry_run {
        styler.success_prefix(true)
    } else {
        styler.green("\u{2713}")
    };
    let verb = if dry_run {
        "would save"
    } else if reused {
        "reused"
    } else {
        "saved"
    };
    let note = if reused && dry_run { " (reused)" } else { "" };
    println!("{prefix} {verb:<10}{}{note}", styler.cyan(saved));
}

fn success_json(result: &CaptureResult) -> String {
    serde_json::to_string(result).expect("serialize capture result")
}

fn print_capture_error(
    error: CaptureError,
    output_format: OutputFormat,
) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {}", error.message),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": error.message }))
        }
    }
    error.kind.exit_code()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureError {
    kind: CaptureErrorKind,
    message: String,
}

impl CaptureError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureErrorKind::Usage,
            message: message.into(),
        }
    }

    fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureErrorKind::Io,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureErrorKind {
    Usage,
    Io,
}

impl CaptureErrorKind {
    fn exit_code(self) -> i32 {
        match self {
            Self::Usage => 2,
            Self::Io => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TASK: &str = "- [ ] #task new thing [created::2026-06-15]";
    const BULLET: &str = "- new idea [created::2026-06-15]";

    fn parse_capture_text(
        raw_text: &str,
        forced_route: Option<&str>,
    ) -> Result<ParsedCaptureText, CaptureError> {
        super::parse_capture_text(raw_text, forced_route, None)
    }

    fn insert_bullet_line(
        contents: &str,
        bullet_line: &str,
        section_prefix: Option<&str>,
    ) -> (String, Placement) {
        super::insert_bullet_line(contents, bullet_line, section_prefix, false)
    }

    #[test]
    fn normalizes_whitespace() {
        assert_eq!(
            normalize_task_text(" \n buy\t  milk \r\n @groceries  "),
            "buy milk @groceries"
        );
    }

    #[test]
    fn parses_schedule_tokens() {
        assert_eq!(parse_schedule_token("s:0"), Some(0));
        assert_eq!(parse_schedule_token("s:1"), Some(1));
        assert_eq!(parse_schedule_token("s:42"), Some(42));

        for token in [
            "s:",
            "s:abc",
            "s:-1",
            "s:1.5",
            "S:1",
            "sx:1",
            "s:18446744073709551616",
        ] {
            assert_eq!(parse_schedule_token(token), None, "{token}");
        }
    }

    #[test]
    fn extracts_trailing_schedule_from_terminal_region() {
        let mut tokens = vec!["buy", "milk", "s:1"];
        assert_eq!(extract_trailing_schedule(&mut tokens), Some(1));
        assert_eq!(tokens, vec!["buy", "milk"]);

        let mut tokens = vec!["buy", "milk", "s:2", "@groceries"];
        assert_eq!(extract_trailing_schedule(&mut tokens), Some(2));
        assert_eq!(tokens, vec!["buy", "milk", "@groceries"]);

        let mut tokens = vec!["buy", "milk", "@groceries", "s:3"];
        assert_eq!(extract_trailing_schedule(&mut tokens), Some(3));
        assert_eq!(tokens, vec!["buy", "milk", "@groceries"]);

        let mut tokens = vec!["take", "s:1", "pill"];
        assert_eq!(extract_trailing_schedule(&mut tokens), None);
        assert_eq!(tokens, vec!["take", "s:1", "pill"]);

        let mut tokens = vec!["buy", "s:1", "s:2"];
        assert_eq!(extract_trailing_schedule(&mut tokens), Some(2));
        assert_eq!(tokens, vec!["buy", "s:1"]);

        let mut tokens = vec!["buy", "s:abc"];
        assert_eq!(extract_trailing_schedule(&mut tokens), None);
        assert_eq!(tokens, vec!["buy", "s:abc"]);
    }

    #[test]
    fn extracts_clip_and_schedule_markers_from_terminal_region() {
        let cases = [
            ("body %", "body", None, None, "clip"),
            ("body %20", "body", None, None, "20"),
            ("body %log @notes", "body", Some("notes"), None, "log"),
            (
                "body s:1 % @groceries",
                "body",
                Some("groceries"),
                Some(1),
                "clip",
            ),
            (
                "body % s:1 @groceries",
                "body",
                Some("groceries"),
                Some(1),
                "clip",
            ),
            (
                "body @groceries s:1 %log",
                "body",
                Some("groceries"),
                Some(1),
                "log",
            ),
            (
                "body %log @groceries s:1",
                "body",
                Some("groceries"),
                Some(1),
                "log",
            ),
            (
                "body s:1 @groceries %log",
                "body",
                Some("groceries"),
                Some(1),
                "log",
            ),
            (
                "@groceries body %foo_bar",
                "body",
                Some("groceries"),
                None,
                "foo_bar",
            ),
            ("body %log @dev:blockid", "body", Some("dev"), None, "log"),
            ("body %log @notes#Ideas", "body", Some("notes"), None, "log"),
        ];

        for (raw, body, route, scheduled, header) in cases {
            let parsed = parse_capture_text(raw, None)
                .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
            assert_eq!(parsed.body, body, "{raw}");
            assert_eq!(parsed.route.as_deref(), route, "{raw}");
            assert_eq!(parsed.scheduled_offset, scheduled, "{raw}");
            assert_eq!(parsed.clip_header.as_deref(), Some(header), "{raw}");
        }
    }

    #[test]
    fn clip_markers_are_terminal_forgiving_and_can_be_disabled() {
        for raw in ["save % now", "body %bad!", "body 50%", "body 100%"] {
            let parsed = parse_capture_text(raw, None).expect("literal text");
            assert_eq!(parsed.body, raw, "{raw}");
            assert_eq!(parsed.clip_header, None, "{raw}");
        }

        let parsed = super::parse_capture_text_with_clip_control(
            "body %log",
            None,
            None,
            false,
        )
        .expect("disabled clip marker");
        assert_eq!(parsed.body, "body %log");
        assert_eq!(parsed.clip_header, None);

        let parsed = super::parse_capture_text("body %log", Some("work"), None)
            .expect("forced route still extracts marker");
        assert_eq!(parsed.body, "body");
        assert_eq!(parsed.route.as_deref(), Some("work"));
        assert_eq!(parsed.clip_header.as_deref(), Some("log"));

        let parsed = super::parse_capture_text(
            "body %section_clip",
            Some("notes"),
            Some("Ideas"),
        )
        .expect("forced section still extracts marker");
        assert_eq!(parsed.body, "body");
        assert_eq!(parsed.clip_header.as_deref(), Some("section_clip"));
        assert!(matches!(
            parsed.kind,
            CaptureKind::Bullet { exact: true, .. }
        ));

        let parsed = parse_capture_text("body %first %second", None)
            .expect("one marker extracted");
        assert_eq!(parsed.body, "body %first");
        assert_eq!(parsed.clip_header.as_deref(), Some("second"));

        let error = parse_capture_text("%", None)
            .expect_err("marker-only capture has no parent text");
        assert_eq!(error.kind, CaptureErrorKind::Usage);
    }

    #[test]
    fn parses_auto_routes_like_hammerspoon() {
        let cases = [
            (
                "@Groceries Buy Milk",
                "Buy Milk",
                Some("groceries"),
                "prefix route wins and lower-cases",
            ),
            (
                "Buy Milk @Groceries",
                "Buy Milk",
                Some("groceries"),
                "suffix route lower-cases",
            ),
            ("a @b @C", "a @b", Some("c"), "last suffix token wins"),
            (
                "@Work buy milk @home",
                "buy milk @home",
                Some("work"),
                "prefix wins before suffix",
            ),
            (
                "Email @home soon",
                "Email @home soon",
                None,
                "middle @token stays literal",
            ),
            ("@route", "@route", None, "bare route stays literal"),
            (
                "@bad! body @Good",
                "@bad! body",
                Some("good"),
                "invalid prefix can still use suffix",
            ),
        ];

        for (raw, body, route, label) in cases {
            let parsed =
                parse_capture_text(raw, None).unwrap_or_else(|error| {
                    panic!("{label}: unexpected error: {error:?}")
                });
            assert_eq!(parsed.body, body, "{label}");
            assert_eq!(parsed.route.as_deref(), route, "{label}");
        }
    }

    #[test]
    fn time_tokens_stay_literal_and_leading_route_wins() {
        for raw in ["call dentist @5:30pm", "standup @10:00"] {
            let parsed = parse_capture_text(raw, None).expect("time literal");
            assert_eq!(parsed.body, raw);
            assert_eq!(parsed.route, None);
            assert_eq!(parsed.kind, CaptureKind::Task);
        }
        let parsed =
            parse_capture_text("task @dev:foo", None).expect("valid marker");
        assert_eq!(parsed.route.as_deref(), Some("dev"));
        let parsed = parse_capture_text("@groceries ping @x:", None)
            .expect("leading route wins");
        assert_eq!(parsed.route.as_deref(), Some("groceries"));
        assert_eq!(parsed.body, "ping @x:");
    }

    #[test]
    fn parses_scheduled_offsets_with_routes() {
        let cases = [
            ("Buy Milk s:1", "Buy Milk", None, Some(1)),
            (
                "Buy Milk s:2 @Groceries",
                "Buy Milk",
                Some("groceries"),
                Some(2),
            ),
            (
                "Buy Milk @Groceries s:2",
                "Buy Milk",
                Some("groceries"),
                Some(2),
            ),
            (
                "@Groceries Buy Milk s:3",
                "Buy Milk",
                Some("groceries"),
                Some(3),
            ),
            ("take s:1 pill", "take s:1 pill", None, None),
            ("Buy Milk s:1 s:2", "Buy Milk s:1", None, Some(2)),
            ("Buy Milk s:abc", "Buy Milk s:abc", None, None),
            ("Buy Milk S:1", "Buy Milk S:1", None, None),
        ];

        for (raw, body, route, offset) in cases {
            let parsed = parse_capture_text(raw, None)
                .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
            assert_eq!(parsed.body, body, "{raw}");
            assert_eq!(parsed.route.as_deref(), route, "{raw}");
            assert_eq!(parsed.scheduled_offset, offset, "{raw}");
        }

        let error = parse_capture_text("s:1", None).expect_err("schedule only");
        assert_eq!(error.kind, CaptureErrorKind::Usage);
    }

    #[test]
    fn parses_pomodoro_routes_in_terminal_positions_with_schedules() {
        let cases = [
            ("@Dev:Foo-Bar Do thing", "Do thing", None),
            ("Do thing @Dev:Foo-Bar", "Do thing", None),
            ("Do thing s:2 @Dev:Foo-Bar", "Do thing", Some(2)),
            ("Do thing @Dev:Foo-Bar s:2", "Do thing", Some(2)),
            ("@Dev:Foo-Bar Do thing s:2", "Do thing", Some(2)),
            ("@!Dev:Foo-Bar Do thing", "Do thing", None),
            ("Do thing @!Dev:Foo-Bar", "Do thing", None),
            ("Do thing @!Dev:Foo-Bar s:2", "Do thing", Some(2)),
        ];

        for (raw, body, scheduled_offset) in cases {
            let parsed = parse_capture_text(raw, None)
                .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
            assert_eq!(parsed.body, body, "{raw}");
            assert_eq!(parsed.route.as_deref(), Some("dev"), "{raw}");
            assert_eq!(parsed.scheduled_offset, scheduled_offset, "{raw}");
            assert_eq!(
                parsed.kind,
                CaptureKind::Pomodoro {
                    block_id: "Foo-Bar".to_string(),
                },
                "{raw}"
            );
        }
    }

    #[test]
    fn malformed_terminal_pomodoro_routes_are_usage_errors() {
        for raw in [
            "Do thing @dev:",
            "Do thing @dev:bad.id",
            "Do thing @bad/route:id",
            "Do thing @dev:id:extra",
            "Do thing @!",
            "Do thing @!dev",
            "Do thing @!:id",
            "Do thing @!dev:",
            "Do thing @!dev:bad.id",
            "Do thing @!bad/route:id",
            "Do thing @!dev:id:extra",
            "@!dev Do thing",
        ] {
            let error = parse_capture_text(raw, None)
                .expect_err(&format!("{raw} should fail"));
            assert_eq!(error.kind, CaptureErrorKind::Usage, "{raw}");
        }
    }

    #[test]
    fn pomodoro_route_requires_a_body_and_stays_literal_in_middle_or_forced() {
        let error = parse_capture_text("@dev:id", None)
            .expect_err("marker-only capture should fail");
        assert_eq!(error.kind, CaptureErrorKind::Usage);

        let parsed = parse_capture_text("Discuss @dev:id later", None)
            .expect("middle marker stays literal");
        assert_eq!(parsed.body, "Discuss @dev:id later");
        assert_eq!(parsed.kind, CaptureKind::Task);

        let parsed = parse_capture_text("Do thing @dev:id", Some("Work"))
            .expect("forced route keeps marker literal");
        assert_eq!(parsed.body, "Do thing @dev:id");
        assert_eq!(parsed.route.as_deref(), Some("work"));
        assert_eq!(parsed.kind, CaptureKind::Task);

        let parsed = parse_capture_text("Jot @notes#time:box", None)
            .expect("a colon in a bullet prefix is not a Pomodoro marker");
        assert_eq!(parsed.body, "Jot");
        assert_eq!(parsed.route.as_deref(), Some("notes"));
        assert_eq!(
            parsed.kind,
            CaptureKind::Bullet {
                section_prefix: Some("time:box".to_string()),
                exact: false,
            }
        );
    }

    #[test]
    fn forced_route_bypasses_auto_route_parsing() {
        let parsed =
            parse_capture_text("Buy milk @Groceries", Some("Work-Queue"))
                .expect("parse forced route");
        assert_eq!(parsed.body, "Buy milk @Groceries");
        assert_eq!(parsed.route.as_deref(), Some("work-queue"));
        assert_eq!(parsed.scheduled_offset, None);

        let parsed =
            parse_capture_text("Buy milk s:2 @Groceries", Some("Work-Queue"))
                .expect("parse forced route with schedule");
        assert_eq!(parsed.body, "Buy milk @Groceries");
        assert_eq!(parsed.route.as_deref(), Some("work-queue"));
        assert_eq!(parsed.scheduled_offset, Some(2));

        let error = parse_capture_text("Buy milk", Some("../bad"))
            .expect_err("invalid forced route must fail");
        assert_eq!(error.kind, CaptureErrorKind::Usage);
    }

    #[test]
    fn formats_task_line() {
        assert_eq!(
            format_task_line("buy milk", "2026-06-15", None),
            "- [ ] #task buy milk [created::2026-06-15]"
        );
        assert_eq!(
            format_task_line("buy milk", "2026-06-15", Some("2026-06-16")),
            "- [ ] #task buy milk [created::2026-06-15] [scheduled::2026-06-16]"
        );
    }

    #[test]
    fn formats_pomodoro_task_with_block_id_as_final_token() {
        assert_eq!(
            format_pomodoro_task_line(
                "Some foobar task.",
                "2026-07-10",
                None,
                "foobar",
            ),
            "- [*] #task Some foobar task. [created::2026-07-10] ^foobar"
        );
        assert_eq!(
            format_pomodoro_task_line(
                "Some foobar task.",
                "2026-07-10",
                Some("2026-07-12"),
                "foobar",
            ),
            "- [*] #task Some foobar task. [created::2026-07-10] [scheduled::2026-07-12] ^foobar"
        );
    }

    #[test]
    fn pomodoro_link_prefers_the_single_timed_open_entry() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] Untimed first\n",
            "- [x] Completed (0800-0830)\n",
            "- [ ] (**0900-0930** [t:: 30m]) Timed\n",
            "  - existing child\n",
            "## Later\n",
            "- [ ] Outside (1000-1030)\n",
        );
        let (updated, placement) =
            insert_pomodoro_block_link(contents, "[[dev#^foobar]]")
                .expect("select timed Pomodoro");
        assert_eq!(placement, Placement::Inserted);
        assert_eq!(
            updated,
            concat!(
                "## Pomodoros\n",
                "- [ ] Untimed first\n",
                "- [x] Completed (0800-0830)\n",
                "- [ ] (**0900-0930** [t:: 30m]) Timed\n",
                "  - existing child\n",
                "  - [[dev#^foobar]]\n",
                "## Later\n",
                "- [ ] Outside (1000-1030)\n",
            )
        );
    }

    #[test]
    fn pomodoro_link_falls_back_to_first_open_and_ignores_nested_tasks() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [x] Completed\n",
            "  - [ ] Nested (0800-0830)\n",
            "- [ ] First open\n",
            "- [ ] Second open\n",
        );
        let (updated, placement) =
            insert_pomodoro_block_link(contents, "[[dev#^fallback]]")
                .expect("select first open Pomodoro");
        assert_eq!(placement, Placement::Inserted);
        assert_eq!(
            updated,
            concat!(
                "## Pomodoros\n",
                "- [x] Completed\n",
                "  - [ ] Nested (0800-0830)\n",
                "- [ ] First open\n",
                "  - [[dev#^fallback]]\n",
                "- [ ] Second open\n",
            )
        );
    }

    #[test]
    fn pomodoro_link_rejects_missing_section_target_and_timed_ambiguity() {
        for (contents, expected) in [
            ("## Notes\n- [ ] (0800-0830) Outside\n", "no Pomodoros"),
            ("## Pomodoros\n- [x] Complete\n", "no eligible"),
            (
                "## Pomodoros\n- [ ] (0800-0830) One\n- [ ] (**0900-0930**) Two\n",
                "multiple open timed",
            ),
        ] {
            let error = insert_pomodoro_block_link(contents, "[[dev#^id]]")
                .expect_err("invalid ledger should fail");
            assert!(error.message.contains(expected), "{error:?}");
        }
    }

    #[test]
    fn pomodoro_link_preserves_crlf_and_reuses_nearby_child_indentation() {
        let contents = concat!(
            "## Pomodoros\r\n",
            "- [x] Old\r\n",
            "\t- old child\r\n",
            "- [ ] Next\r\n",
        );
        let (updated, placement) =
            insert_pomodoro_block_link(contents, "[[dev#^id]]")
                .expect("insert CRLF link");
        assert_eq!(placement, Placement::Appended);
        assert_eq!(
            updated,
            concat!(
                "## Pomodoros\r\n",
                "- [x] Old\r\n",
                "\t- old child\r\n",
                "- [ ] Next\r\n",
                "\t- [[dev#^id]]\r\n",
            )
        );
    }

    #[test]
    fn pomodoro_section_scan_ignores_fenced_lookalikes() {
        let contents = concat!(
            "```md\n",
            "## Pomodoros\n",
            "- [ ] (0800-0830) Example\n",
            "```\n",
            "## Pomodoros\n",
            "- [ ] Real\n",
        );
        let (updated, _) =
            insert_pomodoro_block_link(contents, "[[dev#^real]]")
                .expect("find real section");
        assert!(updated.ends_with("- [ ] Real\n  - [[dev#^real]]\n"));
        assert!(!updated.contains("Example\n  - [[dev#^real]]"));
    }

    #[test]
    fn formats_scheduled_date_from_offset() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 15).expect("valid date");
        assert_eq!(
            scheduled_date_string(today, 0).expect("same day"),
            "2026-06-15"
        );
        assert_eq!(
            scheduled_date_string(today, 1).expect("tomorrow"),
            "2026-06-16"
        );

        let error = scheduled_date_string(today, 9_999_999_999)
            .expect_err("calendar overflow must fail");
        assert_eq!(error.kind, CaptureErrorKind::Usage);
    }

    #[test]
    fn appends_to_empty_and_no_task_files() {
        assert_eq!(
            insert_task_line("", TASK),
            (format!("{TASK}\n"), Placement::Appended)
        );
        assert_eq!(
            insert_task_line("# Header", TASK),
            (format!("# Header\n{TASK}\n"), Placement::Appended)
        );
        assert_eq!(
            insert_task_line("# Header\n", TASK),
            (format!("# Header\n{TASK}\n"), Placement::Appended)
        );
    }

    #[test]
    fn inserts_after_single_top_level_task() {
        let contents = "- [ ] #task old\nPlain paragraph\n";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!("- [ ] #task old\n{TASK}\nPlain paragraph\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn skips_indented_and_blank_then_indented_continuation_lines() {
        let contents = "- [ ] #task old\n  child\n\n\tdeep\n\nNext\n";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!("- [ ] #task old\n  child\n\n\tdeep\n{TASK}\n\nNext\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn inserts_after_last_of_many_task_blocks() {
        let contents = "- [ ] #task first\n- [x] #task second\n  note\nTail\n";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!(
                    "- [ ] #task first\n- [x] #task second\n  note\n{TASK}\nTail\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn adds_leading_newline_when_inserting_after_non_newline_eof() {
        let contents = "- [*] #task old";
        assert_eq!(
            insert_task_line(contents, TASK),
            (format!("- [*] #task old\n{TASK}\n"), Placement::Inserted,)
        );
    }

    #[test]
    fn inserts_multiline_capture_as_one_task_block() {
        let block = format!("{TASK}\n  - **CLIP:** hello");
        let contents = "- [ ] #task old\n  - old child\nTail\n";
        assert_eq!(
            insert_task_line(contents, &block),
            (
                format!("- [ ] #task old\n  - old child\n{block}\nTail\n"),
                Placement::Inserted,
            )
        );

        let crlf = "- [ ] #task old\r\nTail\r\n";
        assert_eq!(
            insert_task_line(crlf, &block).0,
            format!(
                "- [ ] #task old\r\n{}\r\nTail\r\n",
                block.replace('\n', "\r\n")
            )
        );
    }

    #[test]
    fn inserts_after_final_continuation_running_to_eof() {
        let contents = "- [/] #task old\n  note";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!("- [/] #task old\n  note\n{TASK}\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn ignores_indented_task_lines_as_insertion_anchors() {
        let contents = "  - [ ] #task nested";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!("  - [ ] #task nested\n{TASK}\n"),
                Placement::Appended,
            )
        );
    }

    #[test]
    fn tasks_section_wins_over_root_task_when_empty() {
        let contents = "# Project\n- [ ] #task root\n## Tasks\nNotes\n";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!(
                    "# Project\n- [ ] #task root\n## Tasks\n\n{TASK}\nNotes\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn tasks_section_inserts_after_last_task_block_in_section() {
        let contents = concat!(
            "# Project\n",
            "- [ ] #task root\n",
            "## Tasks\n",
            "Intro\n",
            "- [ ] #task old\n",
            "  detail\n",
            "\n",
            "\tmore\n",
            "After\n",
        );
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!(
                    "{}{TASK}\nAfter\n",
                    concat!(
                        "# Project\n",
                        "- [ ] #task root\n",
                        "## Tasks\n",
                        "Intro\n",
                        "- [ ] #task old\n",
                        "  detail\n",
                        "\n",
                        "\tmore\n",
                    )
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn later_task_outside_tasks_section_does_not_win() {
        let contents =
            "## Tasks\n- [ ] #task in section\n## Other\n- [ ] #task outside\n";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!(
                    "## Tasks\n- [ ] #task in section\n{TASK}\n## Other\n- [ ] #task outside\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn ignores_tasks_headings_in_frontmatter_and_fenced_code() {
        let contents = concat!(
            "---\n",
            "# Tasks\n",
            "---\n",
            "```md\n",
            "## Tasks\n",
            "```\n",
            "- [ ] #task old\n",
            "Tail\n",
        );
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!(
                    "---\n\
                     # Tasks\n\
                     ---\n\
                     ```md\n\
                     ## Tasks\n\
                     ```\n\
                     - [ ] #task old\n\
                     {TASK}\n\
                     Tail\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn nested_heading_stops_empty_tasks_section_insertion() {
        let contents = "## Tasks\n### Later\n- [ ] #task later\n";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!("## Tasks\n\n{TASK}\n### Later\n- [ ] #task later\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn tasks_heading_at_eof_inserts_after_blank_line() {
        assert_eq!(
            insert_task_line("## Tasks", TASK),
            (format!("## Tasks\n\n{TASK}\n"), Placement::Inserted,)
        );
        assert_eq!(
            insert_task_line("## Tasks ##\n", TASK),
            (format!("## Tasks ##\n\n{TASK}\n"), Placement::Inserted,)
        );
    }

    #[test]
    fn json_success_shape_is_stable() {
        let result = CaptureResult {
            ok: true,
            dry_run: false,
            routed: true,
            route: Some("groceries".to_string()),
            route_label: "groceries.md".to_string(),
            relative_target: "groceries.md".to_string(),
            target: "/tmp/bob/groceries.md".to_string(),
            text: "buy milk".to_string(),
            task_line: "- [ ] #task buy milk [created::2026-06-15]".to_string(),
            kind: "task",
            created: "2026-06-15".to_string(),
            scheduled: None,
            placement: Placement::Inserted,
            clip: None,
            block_id: None,
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
        };

        let value: serde_json::Value =
            serde_json::from_str(&success_json(&result)).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["dry_run"], false);
        assert_eq!(value["routed"], true);
        assert_eq!(value["route"], "groceries");
        assert_eq!(value["route_label"], "groceries.md");
        assert_eq!(value["relative_target"], "groceries.md");
        assert_eq!(value["target"], "/tmp/bob/groceries.md");
        assert_eq!(value["text"], "buy milk");
        assert_eq!(
            value["task_line"],
            "- [ ] #task buy milk [created::2026-06-15]"
        );
        assert_eq!(value["kind"], "task");
        assert_eq!(value["created"], "2026-06-15");
        assert!(value["scheduled"].is_null(), "{value}");
        assert_eq!(value["placement"], "inserted");
        assert!(value.get("clip").is_none(), "{value}");
        for special_field in [
            "block_id",
            "day_file",
            "block_link",
            "pomodoro_link_placement",
        ] {
            assert!(value.get(special_field).is_none(), "{value}");
        }
    }

    #[test]
    fn parses_suffixed_route_token_as_bullet() {
        let cases = [
            (
                "Some note @foo#bar",
                "Some note",
                "foo",
                Some("bar"),
                "trailing route with section prefix",
            ),
            (
                "@foo#bar Some note",
                "Some note",
                "foo",
                Some("bar"),
                "leading route with section prefix",
            ),
            (
                "Some note @foo#",
                "Some note",
                "foo",
                None,
                "trailing bare bullet marker",
            ),
            (
                "@foo# Some note",
                "Some note",
                "foo",
                None,
                "leading bare bullet marker",
            ),
            (
                "Some note @Foo-Bar#R",
                "Some note",
                "foo-bar",
                Some("R"),
                "route lower-cases and prefix is preserved",
            ),
        ];

        for (raw, body, route, prefix, label) in cases {
            let parsed = parse_capture_text(raw, None)
                .unwrap_or_else(|error| panic!("{label}: {error:?}"));
            assert_eq!(parsed.body, body, "{label}");
            assert_eq!(parsed.route.as_deref(), Some(route), "{label}");
            assert_eq!(
                parsed.kind,
                CaptureKind::Bullet {
                    section_prefix: prefix.map(str::to_string),
                    exact: false,
                },
                "{label}"
            );
        }
    }

    #[test]
    fn forced_section_forces_exact_bullet_with_forced_route() {
        let parsed = super::parse_capture_text(
            "Some note @other s:1",
            Some("Foo"),
            Some("Ideas"),
        )
        .expect("parse forced section");
        assert_eq!(parsed.body, "Some note @other");
        assert_eq!(parsed.route.as_deref(), Some("foo"));
        assert_eq!(parsed.scheduled_offset, Some(1));
        assert_eq!(
            parsed.kind,
            CaptureKind::Bullet {
                section_prefix: Some("Ideas".to_string()),
                exact: true,
            }
        );
    }

    #[test]
    fn forced_section_requires_route_and_non_empty_title() {
        let error = super::parse_capture_text("Some note", None, Some("Ideas"))
            .expect_err("section without route must fail");
        assert_eq!(error.kind, CaptureErrorKind::Usage);
        assert!(
            error.message.contains("requires --route"),
            "unexpected error: {error:?}"
        );

        let error =
            super::parse_capture_text("Some note", Some("foo"), Some(""))
                .expect_err("empty section must fail");
        assert_eq!(error.kind, CaptureErrorKind::Usage);
        assert!(
            error.message.contains("must not be empty"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn suffixed_route_token_without_body_is_usage_error() {
        for raw in ["@foo#bar", "@foo#"] {
            let error = parse_capture_text(raw, None)
                .expect_err(&format!("{raw} should require body"));
            assert_eq!(error.kind, CaptureErrorKind::Usage, "{raw}");
        }
    }

    #[test]
    fn legacy_standalone_bullet_markers_are_rejected() {
        for raw in [
            "Some note #bar @foo",
            "Some note @foo #bar",
            "Some note #bar",
            "Some note #",
        ] {
            let error = parse_capture_text(raw, None)
                .expect_err(&format!("{raw} should be a usage error"));
            assert_eq!(error.kind, CaptureErrorKind::Usage, "{raw}");
        }
    }

    #[test]
    fn forced_route_rejects_terminal_marker_but_keeps_middle_hashtag() {
        let error = parse_capture_text("Some note #bar", Some("Work"))
            .expect_err("forced terminal marker must fail");
        assert_eq!(error.kind, CaptureErrorKind::Usage);

        let parsed = parse_capture_text("Some #tag note", Some("Work"))
            .expect("middle hashtag stays literal");
        assert_eq!(parsed.body, "Some #tag note");
        assert_eq!(parsed.route.as_deref(), Some("work"));
        assert_eq!(parsed.kind, CaptureKind::Task);
    }

    #[test]
    fn marker_only_bullet_input_is_usage_error() {
        let error = parse_capture_text("#", None).expect_err("marker only");
        assert_eq!(error.kind, CaptureErrorKind::Usage);

        let error = parse_capture_text("#", Some("Work"))
            .expect_err("forced marker only");
        assert_eq!(error.kind, CaptureErrorKind::Usage);
    }

    #[test]
    fn formats_bullet_line() {
        assert_eq!(
            format_bullet_line("some idea", "2026-06-15", None),
            "- some idea [created::2026-06-15]"
        );
        assert_eq!(
            format_bullet_line("some idea", "2026-06-15", Some("2026-06-16")),
            "- some idea [created::2026-06-15] [scheduled::2026-06-16]"
        );
    }

    #[test]
    fn bullet_inserts_after_matched_section_header() {
        let contents = "# Notes\n## Ideas\nNotes\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ideas")),
            (
                format!("# Notes\n## Ideas\n\n{BULLET}\nNotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_inserts_after_last_ordinary_bullet_block() {
        let contents = "## Ideas\n- first\n  detail\n\n\tmore\nAfter\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ideas")),
            (
                format!(
                    "## Ideas\n- first\n  detail\n\n\tmore\n{BULLET}\nAfter\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_treats_checkbox_only_section_as_empty() {
        let contents = "## Ideas\n- [ ] #task t\n- [x] done\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ideas")),
            (
                format!("## Ideas\n\n{BULLET}\n- [ ] #task t\n- [x] done\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_skips_tasks_section_matching_prefix() {
        let contents = "## Tasks\n- [ ] #task t\n## Ta-da\nNotes\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ta")),
            (
                format!(
                    "## Tasks\n- [ ] #task t\n## Ta-da\n\n{BULLET}\nNotes\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bare_bullet_marker_selects_first_non_tasks_section() {
        let contents = "## Tasks\n- [ ] #task t\n## Ideas\nNotes\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, None),
            (
                format!(
                    "## Tasks\n- [ ] #task t\n## Ideas\n\n{BULLET}\nNotes\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn unmatched_prefix_falls_back_to_zeroth_section() {
        let contents = "Intro line\n## Tasks\n- [ ] #task t\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ideas")),
            (
                format!("{BULLET}\nIntro line\n## Tasks\n- [ ] #task t\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn zeroth_section_insertion_after_frontmatter() {
        let contents = "---\ntype: area\n---\nIntro\n## Tasks\n- [ ] #task t\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ideas")),
            (
                format!(
                    "---\ntype: area\n---\n{BULLET}\nIntro\n## Tasks\n- [ ] #task t\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_prefers_non_h1_match_over_earlier_h1_match() {
        let contents = "# Roadmap\nintro\n\n## Research\nnotes\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("R")),
            (
                format!("# Roadmap\nintro\n\n## Research\n\n{BULLET}\nnotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_uses_h1_match_when_no_non_h1_match_exists() {
        let contents = "# Research\nnotes\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("R")),
            (
                format!("# Research\n\n{BULLET}\nnotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bare_bullet_marker_prefers_non_h1_section() {
        let contents = "# Title\nintro\n\n## Notes\nbody\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, None),
            (
                format!("# Title\nintro\n\n## Notes\n\n{BULLET}\nbody\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_section_prefix_matches_case_insensitively() {
        let contents = "## Research\nnotes\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("r")),
            (
                format!("## Research\n\n{BULLET}\nnotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_ignores_headings_in_frontmatter_and_fences() {
        let contents = concat!(
            "---\n",
            "## Ideas\n",
            "---\n",
            "```md\n",
            "## Ideas\n",
            "```\n",
            "## Ideas\n",
            "Notes\n",
        );
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ideas")),
            (
                format!(
                    "---\n## Ideas\n---\n```md\n## Ideas\n```\n## Ideas\n\n{BULLET}\nNotes\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn exact_bullet_section_wins_over_prefix_sibling() {
        let contents = "## Ideas\nnotes\n## Idea\nnotes\n";
        assert_eq!(
            super::insert_bullet_line(contents, BULLET, Some("Idea"), true),
            (
                format!("## Ideas\nnotes\n## Idea\n\n{BULLET}\nnotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn exact_bullet_section_keeps_non_h1_preference() {
        let contents = "# Idea\nintro\n## Idea\nnotes\n";
        assert_eq!(
            super::insert_bullet_line(contents, BULLET, Some("Idea"), true),
            (
                format!("# Idea\nintro\n## Idea\n\n{BULLET}\nnotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn exact_bullet_section_matches_case_insensitively() {
        let contents = "## Research\nnotes\n";
        assert_eq!(
            super::insert_bullet_line(contents, BULLET, Some("research"), true),
            (
                format!("## Research\n\n{BULLET}\nnotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn exact_bullet_section_no_match_falls_back_to_zeroth_section() {
        let contents = "Intro\n## Ideas\nnotes\n";
        assert_eq!(
            super::insert_bullet_line(contents, BULLET, Some("Idea"), true),
            (
                format!("{BULLET}\nIntro\n## Ideas\nnotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bare_bullet_marker_ignores_exact_flag() {
        let contents = "# Title\nintro\n\n## Notes\nbody\n";
        assert_eq!(
            super::insert_bullet_line(contents, BULLET, None, true),
            insert_bullet_line(contents, BULLET, None)
        );
    }

    #[test]
    fn non_tasks_section_headings_match_bullet_heading_scan() {
        let contents = concat!(
            "---\n",
            "## Frontmatter\n",
            "---\n",
            "# Title\n",
            "```md\n",
            "## Fenced\n",
            "```\n",
            "## Tasks\n",
            "### Ideas ###\n",
            "###### Log\n",
        );
        assert_eq!(
            non_tasks_section_headings(contents),
            vec![
                SectionHeading {
                    title: "Title".to_string(),
                    level: 1,
                },
                SectionHeading {
                    title: "Ideas".to_string(),
                    level: 3,
                },
                SectionHeading {
                    title: "Log".to_string(),
                    level: 6,
                },
            ]
        );
    }
}
