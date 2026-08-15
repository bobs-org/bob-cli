use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    io::{self, IsTerminal, Read, Write},
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
    capture_clip, capture_language,
    capture_language::{
        is_block_id, AuthoredSubBullet, CaptureKind, ClipRequest,
        ParsedCaptureItem, SubBulletTarget,
    },
    capture_schedule_log, collect_done, config, env as bob_env, markdown,
    note_tasks,
    note_tasks::{BlockIdLookup, RefLookup},
    pomodoro,
    style::Styler,
};

pub(crate) use super::capture_language::is_route_token;
#[cfg(test)]
use super::capture_language::ParsedCaptureText;

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
            "Capture one or more tasks or bullets into the Bob Obsidian vault.\n\n\
TEXT is split into ordered capture items by one or more blank or whitespace-only \
physical lines; leading, trailing, and repeated separators are ignored. Each \
item's first nonblank line is normalized and becomes that item's parent, \
formatted as a #task with a [created::] stamp unless a bullet or sub-bullet \
route is selected, and written to mac_inbox.md unless an @route token or \
--route target is provided. Existing target files prefer a Tasks section, \
then fall back to the last top-level task block. Missing target files are \
created when needed.\n\n\
Within each item, every later physical line must be either a column-zero \
Markdown bullet or a nested bullet prefixed by exactly two ASCII spaces. At \
either depth, '-', '*', or '+' must be followed by a space or tab; the source \
marker and separator are stripped and the item is rendered with the canonical \
'- <body>' marker. Column-zero authored items render one target-selected \
indentation unit beneath the parent; two-space source items render two units \
beneath the parent and attach to the nearest preceding nonempty column-zero \
authored item. A marker-only placeholder is skipped and never clears that \
owner. Unsupported indentation, prose continuation, or an orphaned nested \
item fails with a usage error naming the physical line, and so does an item \
left with no text once its own capture markers are removed. Every recognized \
terminal 's:<N>', 'p:<N>', '%...', and '@route' marker configures only its own \
capture item and is stripped from the rendered line it was typed on; a second \
line in the same item resolving the same marker is ambiguous and fails before \
anything is written. Only an item's first line keeps the established leading \
'@route text' form. Authored children render before any clipboard children \
and the priority schedule log.\n\n\
All items are planned against in-memory note snapshots before commit. Later \
items see earlier planned edits to the same target, and any parse, clipboard, \
validation, staging, or replace failure leaves notes, ledgers, and newly \
created clipboard files at their original state. Single-item JSON keeps its \
legacy shape; multi-item JSON keeps the first result at the top level and adds \
an ordered 'captures' array.\n\n\
Append a trailing lowercase 's:<N>' token, where N is a non-negative integer, \
to schedule the capture N days from today. The token is removed from the task \
text and rendered as [scheduled::YYYY-MM-DD] after [created::YYYY-MM-DD]. It \
may appear before or after a trailing @route token and is recognized only at \
the very end of the input.\n\n\
Append a trailing lowercase 'p:<N>' token, where N selects the Nth priority \
level configured in ~/.config/bob/config.yml (1-4 today: P1-P4), to write \
[priority::<value>] and roll a random [scheduled::YYYY-MM-DD] inside that \
level's day window. A task with no priority field is implicitly P0, so there \
is no p:0. An explicit s:<N> wins the scheduled date and p:<N> still writes \
the priority. Like s:<N> it is recognized only in the terminal token region \
and may appear on either side of a trailing @route token.\n\n\
Append a trailing clipboard marker: '%' captures one live value without a \
header; '%<positive integer>' captures exactly that many values without \
headers, starting with the live clipboard and then recent history newest first; \
and '%<nonnumeric header>' captures one live value under an explicit header. \
'%1' is equivalent to '%', while '%0' stays literal. Headers use letters, \
digits, '_' and '-'; '_' renders as a space. The marker composes with s:<N>, \
p:<N>, and every route kind in either terminal order. Each value is classified separately: \
small text stays inline; 2-10 flat text lines and 1-10 flat unordered Markdown \
list items become child bullets, with source list markers removed; copied file \
paths are saved under img/ or file/; and long or other Markdown-structured text \
is preserved in a timestamped file/clip-*.md snippet. Clipboard children use \
the target note's dominant tab-or-two-space indentation and fall back to a tab. \
History captures fail without writing \
unless the exact requested count succeeds. Use --clip[=HEADER] to force one live \
capture while keeping '%' tokens literal; --clip=<digits> requests a numeric \
header. Bare --clip also captures without a header. Use --no-clip to keep a \
genuine trailing '%...' token literal. Clipboard failures abort before the note \
or attachment files are changed.\n\n\
Use '@<route>:<block-id>' in the same leading or trailing position to create \
a next-status task and link it from today's Pomodoro ledger. The routed task \
renders as '- [*] #task <body> [created::YYYY-MM-DD] ^<block-id>' (with any \
scheduled property before the final block ID). The daily note comes from \
BOB_DAY_FILE or <bob-dir>/YYYY/YYYYMMDD.md. Capture prefers the single open \
timed entry in its Pomodoros section and otherwise uses the first open entry. \
Both notes are fully validated before either is replaced; duplicate block IDs, \
missing ledger structure, no open entry, and multiple open timed entries fail \
without a partial capture.\n\n\
Use '@<route>^<block-id>' in the same leading or trailing position to create \
an ordinary open task with the requested trailing Obsidian block ID, without \
creating or modifying a Pomodoro ledger link. It renders as '- [ ] #task \
<body> [created::YYYY-MM-DD] ^<block-id>' (with priority and scheduled \
properties before the final block ID). Duplicate block IDs in the destination \
note fail before the note is replaced; a missing destination note may still be \
created like any ordinary routed task. This form never reads or requires the \
daily note. The retired '@<route>::<block-id>' spelling is no longer accepted; \
use '@<route>^<block-id>' instead.\n\n\
Use '@<route>+<block-id>' in the same leading or trailing position to append \
an ordinary child bullet beneath an existing task without a [created::] stamp. It \
renders as '- <body>' and appends only the optional scheduled property. The note \
and task must already exist. Existing child indentation and line endings are \
preserved; run 'bob capture-tasks -r <route>' to list eligible task block IDs.\n\n\
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
            "Examples:\n  bob capture buy milk @groceries\n  bob capture buy milk s:1\n  bob capture buy milk s:2 @groceries\n  bob capture buy milk @groceries s:2\n  bob capture buy milk p:2\n  bob capture research rust p:4 @dev\n  bob capture buy milk %\n  bob capture research links %3\n  bob capture investigate %log @dev:blockid\n  bob capture --clip=screenshot -- save dashboard\n  bob capture '@dev^foobar' 'Some ordinary task.'\n  bob capture '@dev:foobar' 'Some foobar task.'\n  bob capture '@cash+goog-exit' 'Called Morgan Stanley today.'\n  bob capture jot idea @notes#Ideas\n  bob capture --route notes --section Ideas -- jot idea\n  bob capture @notes#Ideas jot idea\n  echo 'buy milk @groceries' | bob capture\n  bob capture -f json -- @work send status\n  printf 'Prepare launch\\n- Confirm owner\\n\\nSend status @work\\n' | bob capture\n  printf 'Prepare launch\\n- Confirm owner\\n- Attach checklist\\n' | bob capture\n\nEnvironment:\n  BOB_CLIPBOARD_CMD          whitespace-split command that prints the live clipboard; overrides platform tools\n  BOB_CLIPBOARD_HISTORY_CMD  whitespace-split history command; receives count and prints a newest-first JSON array of strings\n  BOB_CONFIG_FILE            exact bullet-property config file; defaults to $XDG_CONFIG_HOME/bob/config.yml or ~/.config/bob/config.yml\n  BOB_DAY_FILE               exact daily note used by Pomodoro-linked capture\n  BOB_DIR                    Bob vault root when --bob-dir is omitted\n  BOB_NOW                    current date/time override\n  BOB_PRIORITY_ROLL_SEED     fixed seed for p:<N> rolls; unset means random\n  XDG_CONFIG_HOME            base config directory for BOB_CONFIG_FILE's default; defaults to ~/.config\n\nClipboard source order:\n  Live: BOB_CLIPBOARD_CMD; macOS pbpaste; Linux wl-paste or xclip/xsel; tmux show-buffer\n  History: BOB_CLIPBOARD_HISTORY_CMD; otherwise read-only Clipy SQLite on macOS; no automatic provider elsewhere",
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
        .arg(task_arg())
        .arg(task_ref_arg())
        .arg(text_arg())
}

fn clip_arg() -> Arg {
    Arg::new("clip")
        .long("clip")
        .short('c')
        .value_name("HEADER")
        .num_args(0..=1)
        .require_equals(true)
        .conflicts_with("no-clip")
        .help("Capture the clipboard, optionally with HEADER")
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
        .help("Keep trailing %... clipboard markers literal")
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
        .conflicts_with_all(["task", "task-ref"])
        .help("Force a bullet into the exact section TITLE; requires --route")
}

fn task_arg() -> Arg {
    Arg::new("task")
        .long("task")
        .short('t')
        .value_name("BLOCK-ID")
        .conflicts_with_all(["section", "task-ref"])
        .help("Append beneath task BLOCK-ID; requires --route")
}

fn task_ref_arg() -> Arg {
    Arg::new("task-ref")
        .long("task-ref")
        .value_name("REF")
        .conflicts_with_all(["section", "task"])
        .hide(true)
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
    forced_clip: Option<ClipRequest>,
    forced_route: Option<String>,
    forced_section: Option<String>,
    forced_sub_bullet_target: Option<SubBulletTarget>,
    no_clip: bool,
    raw_text: String,
}

impl CaptureRequest {
    fn from_matches(matches: &ArgMatches) -> Result<Self, CaptureError> {
        let forced_clip =
            matches.contains_id("clip").then(|| ClipRequest::Current {
                header: matches.get_one::<String>("clip").cloned(),
            });
        if let Some(ClipRequest::Current {
            header: Some(header),
        }) = forced_clip.as_ref()
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
        let forced_sub_bullet_target =
            forced_sub_bullet_target_from_matches(matches)?;
        if forced_sub_bullet_target.is_some() && forced_route.is_none() {
            let option = if matches.contains_id("task") {
                "--task"
            } else {
                "--task-ref"
            };
            return Err(CaptureError::usage(format!(
                "{option} requires --route"
            )));
        }

        Ok(Self {
            bob_dir: bob_dir_from_matches(matches),
            dry_run: matches.get_flag("dry-run"),
            forced_clip,
            forced_route,
            forced_section,
            forced_sub_bullet_target,
            no_clip: matches.get_flag("no-clip"),
            raw_text: raw_text_from_matches(matches)?,
        })
    }
}

fn forced_sub_bullet_target_from_matches(
    matches: &ArgMatches,
) -> Result<Option<SubBulletTarget>, CaptureError> {
    if let Some(block_id) = matches.get_one::<String>("task") {
        if !is_block_id(block_id) {
            return Err(CaptureError::usage(
                "sub-bullet capture block ID must be non-empty and contain only A-Z, a-z, 0-9 or '-'",
            ));
        }
        return Ok(Some(SubBulletTarget::BlockId(block_id.clone())));
    }
    matches
        .get_one::<String>("task-ref")
        .map(|task_ref| parse_task_ref(task_ref).map(Some))
        .transpose()
        .map(Option::flatten)
}

fn parse_task_ref(value: &str) -> Result<SubBulletTarget, CaptureError> {
    let valid_digest = |digest: &str| {
        digest.len() == 8
            && digest.bytes().all(|byte| {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            })
    };
    let parsed = value.split_once(':').and_then(|(line, digest)| {
        let line = line.parse::<usize>().ok().filter(|line| *line > 0)?;
        valid_digest(digest).then(|| SubBulletTarget::Ref {
            line,
            digest: digest.to_string(),
        })
    });
    parsed.ok_or_else(|| {
        CaptureError::usage("--task-ref must use <line>:<digest>")
    })
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

    let mut text = String::new();
    io::stdin()
        .lock()
        .read_to_string(&mut text)
        .map_err(|error| CaptureError::io(format!("read stdin: {error}")))?;
    Ok(text)
}

fn capture(request: CaptureRequest) -> Result<CaptureResult, CaptureError> {
    let batch = plan_capture_batch(&request)?;
    if !request.dry_run {
        commit_capture_batch(&batch)?;
    }
    Ok(CaptureResult::from_items(
        batch.items.into_iter().map(|item| item.result).collect(),
    ))
}

struct PlannedCaptureBatch {
    items: Vec<PlannedCaptureItem>,
    text_files: Vec<StagedTextFile>,
}

struct PlannedCaptureItem {
    result: CaptureItemResult,
    clip_plan: Option<capture_clip::ClipPlan>,
}

fn plan_capture_batch(
    request: &CaptureRequest,
) -> Result<PlannedCaptureBatch, CaptureError> {
    let parse_clip_markers = request.forced_clip.is_none() && !request.no_clip;
    let parsed_items = parse_capture_items_with_clip_control(
        &request.raw_text,
        request.forced_route.as_deref(),
        request.forced_section.as_deref(),
        parse_clip_markers,
    )?;
    let now = bob_env::current_datetime();
    let today = now.date();
    let roll_seed = config::roll_seed();
    let mut planner = CaptureBatchPlanner::default();
    let mut clip_reservations = capture_clip::ClipReservations::default();
    let mut items = Vec::new();

    for parsed_item in parsed_items {
        let item_number = parsed_item.index + 1;
        let line_start = parsed_item.line_start;
        let planned = plan_capture_item(
            request,
            parsed_item,
            now,
            today,
            roll_seed,
            &mut planner,
            &mut clip_reservations,
        )
        .map_err(|mut error| {
            error.message = format!(
                "capture item {item_number} starting on line {line_start}: {}",
                error.message
            );
            error
        })?;
        items.push(planned);
    }

    Ok(PlannedCaptureBatch {
        items,
        text_files: planner.into_staged_files(),
    })
}

fn plan_capture_item(
    request: &CaptureRequest,
    parsed_item: ParsedCaptureItem,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    roll_seed: u64,
    planner: &mut CaptureBatchPlanner,
    clip_reservations: &mut capture_clip::ClipReservations,
) -> Result<PlannedCaptureItem, CaptureError> {
    let mut parsed = parsed_item.parsed;
    if let Some(target) = request.forced_sub_bullet_target.as_ref() {
        parsed.kind = CaptureKind::SubBullet {
            target: target.clone(),
        };
    }
    if let Some(clip) = request.forced_clip.as_ref() {
        parsed.clip = Some(clip.clone());
    }
    let created = date_string(today);
    let priority = match parsed.priority_level {
        Some(number) => Some(resolve_priority(
            number,
            parsed.scheduled_offset,
            item_roll_seed(roll_seed, parsed_item.index),
        )?),
        None => None,
    };
    let priority_field = priority
        .as_ref()
        .map(|resolved| (resolved.name.as_str(), resolved.value.as_str()));
    let schedule_log_reason = priority.as_ref().and_then(|resolved| {
        resolved.rolled_offset.map(|rolled_days| {
            capture_schedule_log::priority_roll_reason(
                capture_schedule_log::IMPLICIT_LEVEL_LABEL,
                &resolved.label,
                rolled_days,
                resolved.min_days,
                resolved.max_days,
            )
        })
    });
    let scheduled_offset = parsed.scheduled_offset.or_else(|| {
        priority
            .as_ref()
            .and_then(|resolved| resolved.rolled_offset)
    });
    let scheduled = scheduled_offset
        .map(|offset| scheduled_date_string(today, offset))
        .transpose()?;
    let capture_line = match &parsed.kind {
        CaptureKind::Task => format_task_line(
            &parsed.body,
            &created,
            priority_field,
            scheduled.as_deref(),
        ),
        CaptureKind::TaskWithBlockId { block_id } => {
            format_task_with_block_id_line(
                &parsed.body,
                &created,
                priority_field,
                scheduled.as_deref(),
                block_id,
            )
        }
        CaptureKind::Bullet { .. } => format_bullet_line(
            &parsed.body,
            &created,
            priority_field,
            scheduled.as_deref(),
        ),
        CaptureKind::SubBullet { .. } => format_sub_bullet_line(
            &parsed.body,
            priority_field,
            scheduled.as_deref(),
        ),
        CaptureKind::Pomodoro { block_id } => format_pomodoro_task_line(
            &parsed.body,
            &created,
            priority_field,
            scheduled.as_deref(),
            block_id,
        ),
    };
    let kind_label = capture_kind_label(&parsed.kind);
    let task_block_id = match &parsed.kind {
        CaptureKind::TaskWithBlockId { block_id } => Some(block_id.clone()),
        _ => None,
    };
    let relative_target = relative_target(parsed.route.as_deref());
    let target = request.bob_dir.join(&relative_target);
    let child_indent = (parsed.clip.is_some()
        || schedule_log_reason.is_some()
        || !parsed.sub_bullets.is_empty())
    .then(|| child_indent_unit(planner, &target))
    .transpose()?;
    let sub_bullet_lines: Vec<String> = if parsed.sub_bullets.is_empty() {
        Vec::new()
    } else {
        let indent = child_indent.as_deref().unwrap_or("\t");
        render_authored_sub_bullets(&parsed.sub_bullets, indent)
    };
    let clip_plan = match parsed.clip.as_ref() {
        Some(ClipRequest::Current { header }) => {
            let clipboard =
                capture_clip::read_clipboard().map_err(CaptureError::io)?;
            Some(
                capture_clip::plan_with_reservations(
                    &request.bob_dir,
                    header.as_deref(),
                    &clipboard,
                    now,
                    child_indent.as_deref().unwrap_or("\t"),
                    clip_reservations,
                )
                .map_err(CaptureError::io)?,
            )
        }
        Some(ClipRequest::History { count }) if count.get() == 1 => {
            let clipboard =
                capture_clip::read_clipboard().map_err(CaptureError::io)?;
            Some(
                capture_clip::plan_with_reservations(
                    &request.bob_dir,
                    None,
                    &clipboard,
                    now,
                    child_indent.as_deref().unwrap_or("\t"),
                    clip_reservations,
                )
                .map_err(CaptureError::io)?,
            )
        }
        Some(ClipRequest::History { count }) => {
            let clipboards = capture_clip::read_clipboard_history(count.get())
                .map_err(CaptureError::io)?;
            Some(
                capture_clip::plan_history_with_reservations(
                    &request.bob_dir,
                    &clipboards,
                    now,
                    child_indent.as_deref().unwrap_or("\t"),
                    clip_reservations,
                )
                .map_err(CaptureError::io)?,
            )
        }
        None => None,
    };
    let clip_output = clip_plan.as_ref().map(|plan| plan.output.clone());
    let schedule_log = schedule_log_reason.and_then(|reason| {
        scheduled.as_deref().map(|scheduled| {
            capture_schedule_log::plan(
                child_indent.as_deref().unwrap_or("\t"),
                scheduled,
                reason,
            )
        })
    });
    let capture_block = assemble_capture_block(
        &capture_line,
        (!sub_bullet_lines.is_empty()).then_some(sub_bullet_lines.as_slice()),
        clip_plan.as_ref().map(|plan| plan.output.lines.as_slice()),
        schedule_log.as_ref().map(|log| log.lines.as_slice()),
    );
    let note_plan = match &parsed.kind {
        CaptureKind::SubBullet {
            target: sub_bullet_target,
        } => {
            let route = parsed.route.as_deref().ok_or_else(|| {
                CaptureError::io(
                    "sub-bullet capture invariant failed: route is missing",
                )
            })?;
            plan_sub_bullet_capture(
                planner,
                &request.bob_dir,
                &target,
                route,
                sub_bullet_target,
                &capture_block,
            )?
        }
        CaptureKind::Pomodoro { block_id } => {
            let route = parsed.route.as_deref().ok_or_else(|| {
                CaptureError::io(
                    "Pomodoro capture invariant failed: route is missing",
                )
            })?;
            plan_capture_with_pomodoro_link(
                planner,
                &request.bob_dir,
                &target,
                route,
                block_id,
                &capture_block,
            )?
        }
        _ => plan_capture_to_target(
            planner,
            &target,
            &capture_block,
            &parsed.kind,
        )?,
    };
    let special = note_plan.pomodoro.as_ref();
    let sub_bullet = note_plan.sub_bullet.as_ref();

    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
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
            priority: priority.as_ref().map(|resolved| resolved.value.clone()),
            priority_label: priority
                .as_ref()
                .map(|resolved| resolved.label.clone()),
            placement: note_plan.placement,
            sub_bullets: sub_bullet_lines,
            clip: clip_output,
            schedule_log,
            block_id: task_block_id
                .or_else(|| {
                    special.as_ref().map(|edit| edit.details.block_id.clone())
                })
                .or_else(|| sub_bullet.and_then(|edit| edit.block_id.clone())),
            day_file: special
                .as_ref()
                .map(|edit| edit.details.day_file.clone()),
            block_link: special
                .as_ref()
                .map(|edit| edit.details.block_link.clone()),
            pomodoro_link_placement: special
                .as_ref()
                .map(|edit| edit.details.pomodoro_link_placement),
            parent_line: sub_bullet.map(|edit| edit.parent_line),
            parent_text: sub_bullet.map(|edit| edit.parent_text.clone()),
            parent_status_symbol: sub_bullet
                .map(|edit| edit.parent_status_symbol),
            parent_status_name: sub_bullet
                .map(|edit| edit.parent_status_name.clone()),
        },
        clip_plan,
    })
}

fn item_roll_seed(base: u64, item_index: usize) -> u64 {
    base.wrapping_add((item_index as u64).wrapping_mul(0x9E3779B97F4A7C15))
}

/// Join the capture line with its authored children (if any), its clip
/// children (if any), and its schedule log lines (if any), in note order:
/// the captured line first, then the authored bullets the user typed
/// beneath it, then any clipboard children, and the schedule log last since
/// it documents the whole block above it.
fn assemble_capture_block(
    capture_line: &str,
    sub_bullet_lines: Option<&[String]>,
    clip_lines: Option<&[String]>,
    schedule_log_lines: Option<&[String]>,
) -> String {
    let mut lines = vec![capture_line];
    if let Some(sub_bullet_lines) = sub_bullet_lines {
        lines.extend(sub_bullet_lines.iter().map(String::as_str));
    }
    if let Some(clip_lines) = clip_lines {
        lines.extend(clip_lines.iter().map(String::as_str));
    }
    if let Some(schedule_log_lines) = schedule_log_lines {
        lines.extend(schedule_log_lines.iter().map(String::as_str));
    }
    lines.join("\n")
}

fn render_authored_sub_bullets(
    sub_bullets: &[AuthoredSubBullet],
    indent_unit: &str,
) -> Vec<String> {
    sub_bullets
        .iter()
        .map(|item| {
            let indentation = indent_unit.repeat(item.depth.indent_units());
            format!("{indentation}- {}", item.body)
        })
        .collect()
}

fn capture_kind_label(kind: &CaptureKind) -> &'static str {
    match kind {
        CaptureKind::Task | CaptureKind::TaskWithBlockId { .. } => "task",
        CaptureKind::Bullet { .. } => "bullet",
        CaptureKind::Pomodoro { .. } => "pomodoro_task",
        CaptureKind::SubBullet { .. } => "sub_bullet",
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

/// A resolved `p:<N>` level: its field name/value/label plus the level's
/// roll window and, when the date was actually rolled, the chosen offset.
struct ResolvedPriority {
    name: String,
    value: String,
    label: String,
    min_days: u64,
    max_days: u64,
    rolled_offset: Option<u64>,
}

/// Resolve a `p:<N>` level. The rolled offset is only computed when no
/// explicit `s:<N>` offset is present, since an explicit offset always wins
/// the scheduled date.
fn resolve_priority(
    number: u64,
    explicit_scheduled_offset: Option<u64>,
    roll_seed: u64,
) -> Result<ResolvedPriority, CaptureError> {
    let property = config::load_priority_property(&config::config_path())
        .map_err(|error| match error {
            config::ConfigError::Read(message) => CaptureError::io(message),
            config::ConfigError::Invalid(message) => {
                CaptureError::usage(message)
            }
        })?;
    let level = property.level(number).ok_or_else(|| {
        CaptureError::usage(format!(
            "p:{number} is not a configured priority level; use p:1 through p:{} ({})",
            property.level_count(),
            property.labels()
        ))
    })?;
    let rolled_offset = explicit_scheduled_offset
        .is_none()
        .then(|| level.roll_offset(roll_seed));
    Ok(ResolvedPriority {
        name: property.name().to_string(),
        value: level.value().to_string(),
        label: level.label().to_string(),
        min_days: level.min_days(),
        max_days: level.max_days(),
        rolled_offset,
    })
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
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
) -> String {
    let mut line = format!("- [ ] #task {body} [created::{created}]");
    append_priority_property(&mut line, priority);
    append_scheduled_property(&mut line, scheduled);
    line
}

fn format_task_with_block_id_line(
    body: &str,
    created: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
    block_id: &str,
) -> String {
    let mut line = format_task_line(body, created, priority, scheduled);
    append_block_id(&mut line, block_id);
    line
}

fn format_bullet_line(
    body: &str,
    created: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
) -> String {
    let mut line = format!("- {body} [created::{created}]");
    append_priority_property(&mut line, priority);
    append_scheduled_property(&mut line, scheduled);
    line
}

fn format_sub_bullet_line(
    body: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
) -> String {
    let mut line = format!("- {body}");
    append_priority_property(&mut line, priority);
    append_scheduled_property(&mut line, scheduled);
    line
}

fn format_pomodoro_task_line(
    body: &str,
    created: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
    block_id: &str,
) -> String {
    let mut line = format!("- [*] #task {body} [created::{created}]");
    append_priority_property(&mut line, priority);
    append_scheduled_property(&mut line, scheduled);
    append_block_id(&mut line, block_id);
    line
}

fn append_priority_property(line: &mut String, priority: Option<(&str, &str)>) {
    if let Some((property, value)) = priority {
        line.push_str(&format!(" [{property}::{value}]"));
    }
}

fn append_scheduled_property(line: &mut String, scheduled: Option<&str>) {
    if let Some(scheduled) = scheduled {
        line.push_str(&format!(" [scheduled::{scheduled}]"));
    }
}

fn append_block_id(line: &mut String, block_id: &str) {
    line.push_str(&format!(" ^{block_id}"));
}

#[derive(Default)]
struct CaptureBatchPlanner {
    files: Vec<BatchTextFile>,
    by_path: HashMap<PathBuf, usize>,
}

struct BatchTextFile {
    path: PathBuf,
    existed: bool,
    present: bool,
    original: String,
    current: String,
}

struct StagedTextFile {
    target: PathBuf,
    target_existed: bool,
    original_target: String,
    updated_target: String,
}

impl CaptureBatchPlanner {
    fn currently_exists(&mut self, path: &Path) -> Result<bool, CaptureError> {
        let index = self.ensure_loaded(path)?;
        Ok(self.files[index].present)
    }

    fn read_existing(&mut self, path: &Path) -> Result<String, CaptureError> {
        let index = self.ensure_loaded(path)?;
        if !self.files[index].present {
            return Err(CaptureError::io(format!(
                "note does not exist: {}",
                path.display()
            )));
        }
        Ok(self.files[index].current.clone())
    }

    fn current_contents(
        &mut self,
        path: &Path,
    ) -> Result<Option<String>, CaptureError> {
        let index = self.ensure_loaded(path)?;
        Ok(self.files[index]
            .present
            .then(|| self.files[index].current.clone()))
    }

    fn stage(
        &mut self,
        path: &Path,
        updated: String,
    ) -> Result<(), CaptureError> {
        let index = self.ensure_loaded(path)?;
        self.files[index].present = true;
        self.files[index].current = updated;
        Ok(())
    }

    fn ensure_loaded(&mut self, path: &Path) -> Result<usize, CaptureError> {
        if let Some(index) = self.by_path.get(path) {
            return Ok(*index);
        }

        let present = path.exists();
        let original = if present {
            read_target(path)?
        } else {
            String::new()
        };
        let index = self.files.len();
        self.by_path.insert(path.to_path_buf(), index);
        self.files.push(BatchTextFile {
            path: path.to_path_buf(),
            existed: present,
            present,
            current: original.clone(),
            original,
        });
        Ok(index)
    }

    fn into_staged_files(self) -> Vec<StagedTextFile> {
        self.files
            .into_iter()
            .filter(|file| {
                file.present && (!file.existed || file.current != file.original)
            })
            .map(|file| StagedTextFile {
                target: file.path,
                target_existed: file.existed,
                original_target: file.original,
                updated_target: file.current,
            })
            .collect()
    }
}

fn plan_capture_to_target(
    planner: &mut CaptureBatchPlanner,
    target: &Path,
    capture_block: &str,
    kind: &CaptureKind,
) -> Result<CaptureWritePlan, CaptureError> {
    if !planner.currently_exists(target)? {
        validate_target_parent(target)?;
        let updated_target = format!("{capture_block}\n");
        planner.stage(target, updated_target)?;
        return Ok(CaptureWritePlan {
            placement: Placement::Created,
            pomodoro: None,
            sub_bullet: None,
        });
    }

    let contents = planner.read_existing(target)?;
    if let CaptureKind::TaskWithBlockId { block_id } = kind {
        reject_duplicate_block_id(&contents, block_id, target)?;
    }
    let (updated, placement) = match kind {
        CaptureKind::Task
        | CaptureKind::TaskWithBlockId { .. }
        | CaptureKind::Pomodoro { .. } => {
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
        CaptureKind::SubBullet { .. } => {
            return Err(CaptureError::io(
                "sub-bullet capture invariant failed: wrong write planner",
            ));
        }
    };
    planner.stage(target, updated)?;
    Ok(CaptureWritePlan {
        placement,
        pomodoro: None,
        sub_bullet: None,
    })
}

#[derive(Debug)]
struct CaptureWritePlan {
    placement: Placement,
    pomodoro: Option<PlannedPomodoroEdit>,
    sub_bullet: Option<SubBulletCaptureDetails>,
}

#[derive(Debug)]
struct SubBulletCaptureDetails {
    block_id: Option<String>,
    parent_line: usize,
    parent_text: String,
    parent_status_symbol: char,
    parent_status_name: String,
}

#[derive(Debug)]
struct PlannedPomodoroEdit {
    details: PomodoroCaptureDetails,
}

#[derive(Debug)]
struct PomodoroCaptureDetails {
    block_id: String,
    day_file: String,
    block_link: String,
    pomodoro_link_placement: Placement,
}

fn plan_capture_with_pomodoro_link(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target: &Path,
    route: &str,
    block_id: &str,
    capture_block: &str,
) -> Result<CaptureWritePlan, CaptureError> {
    let target_existed = planner.currently_exists(target)?;
    let original_target = planner.current_contents(target)?.unwrap_or_default();
    reject_duplicate_block_id(&original_target, block_id, target)?;

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

    let original_day = planner.read_existing(&day_file)?;
    let block_link = format!("[[{route}#^{block_id}]]");
    if original_day.contains(&block_link) {
        return Err(CaptureError::io(format!(
            "Pomodoro ledger already contains {block_link}"
        )));
    }
    let (updated_day, pomodoro_link_placement) =
        insert_pomodoro_block_link(&original_day, &block_link)?;
    let day_file_label = day_file.display().to_string();
    planner.stage(target, updated_target)?;
    planner.stage(&day_file, updated_day)?;

    Ok(CaptureWritePlan {
        placement,
        pomodoro: Some(PlannedPomodoroEdit {
            details: PomodoroCaptureDetails {
                block_id: block_id.to_string(),
                day_file: day_file_label,
                block_link,
                pomodoro_link_placement,
            },
        }),
        sub_bullet: None,
    })
}

fn reject_duplicate_block_id(
    markdown: &str,
    block_id: &str,
    target: &Path,
) -> Result<(), CaptureError> {
    if collect_done::block_ids_in_markdown(markdown)
        .iter()
        .any(|existing| existing == block_id)
    {
        return Err(CaptureError::io(format!(
            "block ID ^{block_id} already exists in {}",
            target.display()
        )));
    }
    Ok(())
}

fn plan_sub_bullet_capture(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target: &Path,
    route: &str,
    sub_bullet_target: &SubBulletTarget,
    capture_block: &str,
) -> Result<CaptureWritePlan, CaptureError> {
    let contents = planner.read_existing(target)?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let parent = match sub_bullet_target {
        SubBulletTarget::BlockId(block_id) => {
            match scan.by_block_id(block_id) {
                BlockIdLookup::Found(task) => task,
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
                    let message = match scan.suggest_block_id(block_id) {
                    Some(suggestion) => format!(
                        "no task with block ID ^{block_id} in {route}.md; did you mean ^{suggestion}? ({choices})"
                    ),
                    None => format!(
                        "no task with block ID ^{block_id} in {route}.md ({choices})"
                    ),
                };
                    return Err(CaptureError::io(message));
                }
            }
        }
        SubBulletTarget::Ref { line, digest } => {
            match scan.by_ref(*line, digest) {
                RefLookup::Found(task) => task,
                RefLookup::Stale => {
                    return Err(CaptureError::io(format!(
                        "the selected task is no longer in {route}.md; rerun the task picker"
                    )));
                }
                RefLookup::Ambiguous => {
                    return Err(CaptureError::io(format!(
                        "the selected task matches more than one line in {route}.md; rerun the task picker"
                    )));
                }
            }
        }
    };

    let lines = line_spans(&contents);
    let indentation = first_child_indentation(
        &lines,
        parent.line_index,
        parent.block_end,
        &parent.indentation,
    )
    .or_else(|| {
        dominant_indent_unit(&lines)
            .map(|unit| format!("{}{}", parent.indentation, unit))
    })
    .unwrap_or_else(|| format!("{}\t", parent.indentation));
    let indented_block = capture_block
        .split('\n')
        .map(|line| format!("{indentation}{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let addition = insertion_text_preserving_line_endings(
        &contents,
        parent.block_end,
        &indented_block,
    );
    let placement = if parent.block_end >= contents.len() {
        Placement::Appended
    } else {
        Placement::Inserted
    };
    let details = SubBulletCaptureDetails {
        block_id: parent.block_id.clone(),
        parent_line: parent.line_index + 1,
        parent_text: parent.description.clone(),
        parent_status_symbol: parent.status_symbol,
        parent_status_name: parent.status_name.clone(),
    };

    let updated_target = insert_at(&contents, parent.block_end, &addition);
    planner.stage(target, updated_target)?;

    Ok(CaptureWritePlan {
        placement,
        pomodoro: None,
        sub_bullet: Some(details),
    })
}

fn first_child_indentation(
    lines: &[LineSpan<'_>],
    parent_line_index: usize,
    block_end: usize,
    parent_indentation: &str,
) -> Option<String> {
    lines[parent_line_index + 1..]
        .iter()
        .take_while(|line| line.end <= block_end)
        .filter(|line| !line.text.trim().is_empty())
        .map(|line| leading_whitespace(line.text))
        .find(|indentation| indentation.len() > parent_indentation.len())
        .map(str::to_string)
}

fn dominant_indent_unit(lines: &[LineSpan<'_>]) -> Option<&'static str> {
    let (tabs, spaces) = lines.iter().fold((0usize, 0usize), |counts, line| {
        match line.text.as_bytes().first() {
            Some(b'\t') => (counts.0 + 1, counts.1),
            Some(b' ') => (counts.0, counts.1 + 1),
            _ => counts,
        }
    });
    if tabs + spaces == 0 {
        None
    } else if tabs > spaces {
        Some("\t")
    } else {
        Some("  ")
    }
}

fn child_indent_unit(
    planner: &mut CaptureBatchPlanner,
    target: &Path,
) -> Result<String, CaptureError> {
    let Some(contents) = planner.current_contents(target)? else {
        return Ok("\t".to_string());
    };
    Ok(dominant_indent_unit(&line_spans(&contents))
        .unwrap_or("\t")
        .to_string())
}

fn leading_whitespace(line: &str) -> &str {
    let end = line
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(line.len());
    &line[..end]
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

fn commit_capture_batch(
    batch: &PlannedCaptureBatch,
) -> Result<(), CaptureError> {
    let created_clip_files = save_clip_plans(&batch.items)?;
    if let Err(mut error) = write_staged_files(&batch.text_files) {
        if !created_clip_files.is_empty() {
            let cleanup = capture_clip::cleanup_created(&created_clip_files);
            capture_clip::append_cleanup_message(&mut error.message, &cleanup);
        }
        return Err(error);
    }
    Ok(())
}

fn save_clip_plans(
    items: &[PlannedCaptureItem],
) -> Result<Vec<PathBuf>, CaptureError> {
    let mut created = Vec::new();
    for item in items {
        let Some(plan) = &item.clip_plan else {
            continue;
        };
        match plan.save() {
            Ok(paths) => created.extend(paths),
            Err(mut message) => {
                if !created.is_empty() {
                    let cleanup = capture_clip::cleanup_created(&created);
                    capture_clip::append_cleanup_message(
                        &mut message,
                        &cleanup,
                    );
                }
                return Err(CaptureError::io(message));
            }
        }
    }
    Ok(created)
}

struct PendingTextFile<'a> {
    staged: &'a StagedTextFile,
    temporary: PathBuf,
    backup: Option<PathBuf>,
}

struct AppliedTextFile {
    target: PathBuf,
    backup: Option<PathBuf>,
    target_existed: bool,
}

fn write_staged_files(files: &[StagedTextFile]) -> Result<(), CaptureError> {
    let mut pending = Vec::new();
    for (index, staged) in files.iter().enumerate() {
        let role = format!("batch-{index}");
        let temporary = match write_temporary_file(
            &staged.target,
            &staged.updated_target,
            &role,
        ) {
            Ok(path) => path,
            Err(error) => {
                cleanup_pending_text_files(&pending);
                return Err(error);
            }
        };
        let backup = if staged.target_existed {
            match write_temporary_file(
                &staged.target,
                &staged.original_target,
                "backup",
            ) {
                Ok(path) => Some(path),
                Err(error) => {
                    remove_temporary_file(&temporary);
                    cleanup_pending_text_files(&pending);
                    return Err(error);
                }
            }
        } else {
            None
        };
        pending.push(PendingTextFile {
            staged,
            temporary,
            backup,
        });
    }

    let mut applied = Vec::new();
    while !pending.is_empty() {
        let pending_file = pending.remove(0);
        if let Err(error) =
            fs::rename(&pending_file.temporary, &pending_file.staged.target)
        {
            remove_temporary_file(&pending_file.temporary);
            if let Some(backup) = &pending_file.backup {
                remove_temporary_file(backup);
            }
            cleanup_pending_text_files(&pending);
            let mut message = format!(
                "replace target {}: {error}",
                pending_file.staged.target.display()
            );
            append_rollback_message(
                &mut message,
                rollback_applied_files(&applied),
            );
            return Err(CaptureError::io(message));
        }
        applied.push(AppliedTextFile {
            target: pending_file.staged.target.clone(),
            backup: pending_file.backup,
            target_existed: pending_file.staged.target_existed,
        });
    }

    for file in &applied {
        if let Some(backup) = &file.backup {
            remove_temporary_file(backup);
        }
    }
    Ok(())
}

fn cleanup_pending_text_files(files: &[PendingTextFile<'_>]) {
    for file in files {
        remove_temporary_file(&file.temporary);
        if let Some(backup) = &file.backup {
            remove_temporary_file(backup);
        }
    }
}

fn rollback_applied_files(files: &[AppliedTextFile]) -> Vec<String> {
    let mut failures = Vec::new();
    for file in files.iter().rev() {
        let rollback = match &file.backup {
            Some(backup) => fs::rename(backup, &file.target),
            None if file.target_existed => Ok(()),
            None => fs::remove_file(&file.target),
        };
        if let Err(error) = rollback {
            let suffix = file
                .backup
                .as_ref()
                .map(|backup| {
                    format!("; original remains at {}", backup.display())
                })
                .unwrap_or_default();
            failures.push(format!(
                "rollback of {} failed: {error}{suffix}",
                file.target.display()
            ));
        }
    }
    failures
}

fn append_rollback_message(message: &mut String, failures: Vec<String>) {
    if failures.is_empty() {
        message.push_str("; rolled back earlier note writes");
    } else {
        message.push_str("; ");
        message.push_str(&failures.join("; "));
    }
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

/// Run the shared capture grammar and re-wrap its message as this command's
/// usage error, which owns the exit code `bob capture` reports.
#[cfg(test)]
fn parse_capture_text_with_clip_control(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    parse_clip_markers: bool,
) -> Result<ParsedCaptureText, CaptureError> {
    capture_language::parse_capture_text_with_clip_control(
        raw_text,
        forced_route,
        forced_section,
        parse_clip_markers,
    )
    .map_err(CaptureError::usage)
}

fn parse_capture_items_with_clip_control(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    parse_clip_markers: bool,
) -> Result<Vec<ParsedCaptureItem>, CaptureError> {
    capture_language::parse_capture_items_with_clip_control(
        raw_text,
        forced_route,
        forced_section,
        parse_clip_markers,
    )
    .map_err(CaptureError::usage)
}

#[cfg(test)]
fn extract_trailing_schedule(tokens: &mut Vec<&str>) -> Option<u64> {
    capture_language::extract_terminal_markers(tokens, false)
        .0
        .scheduled_offset
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

        if let Some((level, title)) = markdown::atx_heading(line.text) {
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
    #[serde(flatten)]
    item: CaptureItemResult,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    captures: Vec<CaptureItemResult>,
}

impl CaptureResult {
    fn from_items(items: Vec<CaptureItemResult>) -> Self {
        let item = items
            .first()
            .cloned()
            .expect("capture batch always contains at least one item");
        let captures = if items.len() > 1 { items } else { Vec::new() };
        Self { item, captures }
    }
}

impl std::ops::Deref for CaptureResult {
    type Target = CaptureItemResult;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureItemResult {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority_label: Option<String>,
    placement: Placement,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sub_bullets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clip: Option<capture_clip::ClipOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schedule_log: Option<capture_schedule_log::ScheduleLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    day_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_link_placement: Option<Placement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_status_symbol: Option<char>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_status_name: Option<String>,
}

fn print_success(result: &CaptureResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureResult) {
    if result.captures.is_empty() {
        print_human_item_success(result, None);
        return;
    }

    let total = result.captures.len();
    for (index, item) in result.captures.iter().enumerate() {
        if index > 0 {
            println!();
        }
        print_human_item_success(item, Some((index + 1, total)));
    }
}

fn print_human_item_success(
    result: &CaptureItemResult,
    ordinal: Option<(usize, usize)>,
) {
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
    let ordinal = ordinal
        .map(|(index, total)| format!("{index}/{total}  "))
        .unwrap_or_default();
    println!("{prefix} {verb}  {ordinal}{target_label}");
    if let (Some(symbol), Some(parent_text)) =
        (result.parent_status_symbol, result.parent_text.as_deref())
    {
        let marker = style_task_status_marker(&styler, symbol);
        let block_id = result
            .block_id
            .as_deref()
            .map(|id| format!("  {}", styler.cyan(&format!("^{id}"))))
            .unwrap_or_default();
        println!("  under {marker} {parent_text}{block_id}");
    }
    println!("  {}", styler.dim(&result.task_line));
    for line in &result.sub_bullets {
        println!("  {}", styler.dim(line));
    }
    if let Some(clip) = &result.clip {
        for line in &clip.lines {
            println!("  {}", styler.dim(line));
        }
        for (saved, reused) in clip.file_confirmations() {
            print_clip_file_confirmation(
                &styler,
                result.dry_run,
                &saved,
                reused,
            );
        }
    }
    if let Some(schedule_log) = &result.schedule_log {
        for line in &schedule_log.lines {
            println!("  {}", styler.dim(line));
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

fn style_task_status_marker(styler: &Styler, symbol: char) -> String {
    let marker = format!("[{symbol}]");
    match symbol {
        '/' => styler.blue(&marker),
        '*' => styler.yellow(&marker),
        '?' => styler.red(&marker),
        _ => styler.dim(&marker),
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
    use std::num::NonZeroUsize;

    use super::*;
    // Grammar helpers now live in `capture_language`; these tests keep
    // exercising them through `bob capture` so the move stays behavior
    // preserving.
    use crate::native::capture_language::{
        extract_terminal_markers, normalize_task_text, parse_priority_token,
        parse_schedule_token,
    };

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
    fn parses_priority_tokens() {
        assert_eq!(parse_priority_token("p:1"), Some(1));
        assert_eq!(parse_priority_token("p:4"), Some(4));
        assert_eq!(parse_priority_token("p:12"), Some(12));

        for token in [
            "p:",
            "p:abc",
            "p:-1",
            "p:1.5",
            "P:1",
            "px:1",
            "p:18446744073709551616",
        ] {
            assert_eq!(parse_priority_token(token), None, "{token}");
        }
    }

    #[test]
    fn extracts_priority_markers_from_terminal_region() {
        let mut tokens = vec!["buy", "milk", "p:2"];
        assert_eq!(
            extract_terminal_markers(&mut tokens, false)
                .0
                .priority_level,
            Some(2)
        );
        assert_eq!(tokens, vec!["buy", "milk"]);

        let mut tokens = vec!["buy", "milk", "p:2", "@groceries"];
        assert_eq!(
            extract_terminal_markers(&mut tokens, false)
                .0
                .priority_level,
            Some(2)
        );
        assert_eq!(tokens, vec!["buy", "milk", "@groceries"]);

        let mut tokens = vec!["buy", "milk", "@groceries", "p:3"];
        assert_eq!(
            extract_terminal_markers(&mut tokens, false)
                .0
                .priority_level,
            Some(3)
        );
        assert_eq!(tokens, vec!["buy", "milk", "@groceries"]);

        let mut tokens = vec!["set", "p:2", "priority"];
        assert_eq!(
            extract_terminal_markers(&mut tokens, false)
                .0
                .priority_level,
            None
        );
        assert_eq!(tokens, vec!["set", "p:2", "priority"]);

        let mut tokens = vec!["buy", "p:1", "p:2"];
        assert_eq!(
            extract_terminal_markers(&mut tokens, false)
                .0
                .priority_level,
            Some(2)
        );
        assert_eq!(tokens, vec!["buy", "p:1"]);
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
            (
                "body %",
                "body",
                None,
                None,
                ClipRequest::Current { header: None },
                None,
            ),
            (
                "body %20",
                "body",
                None,
                None,
                ClipRequest::History {
                    count: NonZeroUsize::new(20).expect("nonzero"),
                },
                None,
            ),
            (
                "body %log @notes",
                "body",
                Some("notes"),
                None,
                ClipRequest::Current {
                    header: Some("log".to_string()),
                },
                None,
            ),
            (
                "body s:1 % @groceries",
                "body",
                Some("groceries"),
                Some(1),
                ClipRequest::Current { header: None },
                None,
            ),
            (
                "body % s:1 @groceries",
                "body",
                Some("groceries"),
                Some(1),
                ClipRequest::Current { header: None },
                None,
            ),
            (
                "body @groceries s:1 %log",
                "body",
                Some("groceries"),
                Some(1),
                ClipRequest::Current {
                    header: Some("log".to_string()),
                },
                None,
            ),
            (
                "body %log @groceries s:1",
                "body",
                Some("groceries"),
                Some(1),
                ClipRequest::Current {
                    header: Some("log".to_string()),
                },
                None,
            ),
            (
                "body s:1 @groceries %log",
                "body",
                Some("groceries"),
                Some(1),
                ClipRequest::Current {
                    header: Some("log".to_string()),
                },
                None,
            ),
            (
                "@groceries body %foo_bar",
                "body",
                Some("groceries"),
                None,
                ClipRequest::Current {
                    header: Some("foo_bar".to_string()),
                },
                None,
            ),
            (
                "body %log @dev:blockid",
                "body",
                Some("dev"),
                None,
                ClipRequest::Current {
                    header: Some("log".to_string()),
                },
                None,
            ),
            (
                "body %log @notes#Ideas",
                "body",
                Some("notes"),
                None,
                ClipRequest::Current {
                    header: Some("log".to_string()),
                },
                None,
            ),
            (
                "body %3 s:2 @groceries",
                "body",
                Some("groceries"),
                Some(2),
                ClipRequest::History {
                    count: NonZeroUsize::new(3).expect("nonzero"),
                },
                None,
            ),
            (
                "body @groceries %3 s:2",
                "body",
                Some("groceries"),
                Some(2),
                ClipRequest::History {
                    count: NonZeroUsize::new(3).expect("nonzero"),
                },
                None,
            ),
            (
                "body %2 @notes#Ideas",
                "body",
                Some("notes"),
                None,
                ClipRequest::History {
                    count: NonZeroUsize::new(2).expect("nonzero"),
                },
                None,
            ),
            (
                "body @dev:blockid %2",
                "body",
                Some("dev"),
                None,
                ClipRequest::History {
                    count: NonZeroUsize::new(2).expect("nonzero"),
                },
                None,
            ),
            (
                "body p:2 s:1 % @groceries",
                "body",
                Some("groceries"),
                Some(1),
                ClipRequest::Current { header: None },
                Some(2),
            ),
            (
                "body @groceries %log p:3 s:4",
                "body",
                Some("groceries"),
                Some(4),
                ClipRequest::Current {
                    header: Some("log".to_string()),
                },
                Some(3),
            ),
        ];

        for (raw, body, route, scheduled, clip, priority) in cases {
            let parsed = parse_capture_text(raw, None)
                .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
            assert_eq!(parsed.body, body, "{raw}");
            assert_eq!(parsed.route.as_deref(), route, "{raw}");
            assert_eq!(parsed.scheduled_offset, scheduled, "{raw}");
            assert_eq!(parsed.clip, Some(clip), "{raw}");
            assert_eq!(parsed.priority_level, priority, "{raw}");
        }
    }

    #[test]
    fn clip_markers_are_terminal_forgiving_and_can_be_disabled() {
        for raw in ["save % now", "body %bad!", "body 50%", "body 100%"] {
            let parsed = parse_capture_text(raw, None).expect("literal text");
            assert_eq!(parsed.body, raw, "{raw}");
            assert_eq!(parsed.clip, None, "{raw}");
        }

        let parsed = super::parse_capture_text_with_clip_control(
            "body %log",
            None,
            None,
            false,
        )
        .expect("disabled clip marker");
        assert_eq!(parsed.body, "body %log");
        assert_eq!(parsed.clip, None);

        let parsed = super::parse_capture_text("body %log", Some("work"), None)
            .expect("forced route still extracts marker");
        assert_eq!(parsed.body, "body");
        assert_eq!(parsed.route.as_deref(), Some("work"));
        assert_eq!(
            parsed.clip,
            Some(ClipRequest::Current {
                header: Some("log".to_string())
            })
        );

        let parsed = super::parse_capture_text(
            "body %section_clip",
            Some("notes"),
            Some("Ideas"),
        )
        .expect("forced section still extracts marker");
        assert_eq!(parsed.body, "body");
        assert_eq!(
            parsed.clip,
            Some(ClipRequest::Current {
                header: Some("section_clip".to_string())
            })
        );
        assert!(matches!(
            parsed.kind,
            CaptureKind::Bullet { exact: true, .. }
        ));

        let parsed = parse_capture_text("body %first %second", None)
            .expect("one marker extracted");
        assert_eq!(parsed.body, "body %first");
        assert_eq!(
            parsed.clip,
            Some(ClipRequest::Current {
                header: Some("second".to_string())
            })
        );

        let parsed = parse_capture_text("body %2 %3", None)
            .expect("one numeric marker extracted");
        assert_eq!(parsed.body, "body %2");
        assert_eq!(
            parsed.clip,
            Some(ClipRequest::History {
                count: NonZeroUsize::new(3).expect("nonzero")
            })
        );

        for raw in ["body %0", "body %184467440737095516160"] {
            let parsed =
                parse_capture_text(raw, None).expect("literal numeric");
            assert_eq!(parsed.body, raw, "{raw}");
            assert_eq!(parsed.clip, None, "{raw}");
        }
        for (raw, count) in [("body %1", 1), ("body %01", 1), ("body %3", 3)] {
            let parsed = parse_capture_text(raw, None).expect("history marker");
            assert_eq!(parsed.body, "body", "{raw}");
            assert_eq!(
                parsed.clip,
                Some(ClipRequest::History {
                    count: NonZeroUsize::new(count).expect("nonzero")
                }),
                "{raw}"
            );
        }

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
    fn parses_task_block_id_routes_in_terminal_positions_with_schedules() {
        let cases = [
            ("@Dev^Foo-Bar Do thing", "Do thing", None),
            ("Do thing @Dev^Foo-Bar", "Do thing", None),
            ("Do thing s:2 @Dev^Foo-Bar", "Do thing", Some(2)),
            ("Do thing @Dev^Foo-Bar s:2", "Do thing", Some(2)),
            ("@Dev^Foo-Bar Do thing s:2", "Do thing", Some(2)),
        ];

        for (raw, body, scheduled_offset) in cases {
            let parsed = parse_capture_text(raw, None)
                .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
            assert_eq!(parsed.body, body, "{raw}");
            assert_eq!(parsed.route.as_deref(), Some("dev"), "{raw}");
            assert_eq!(parsed.scheduled_offset, scheduled_offset, "{raw}");
            assert_eq!(
                parsed.kind,
                CaptureKind::TaskWithBlockId {
                    block_id: "Foo-Bar".to_string(),
                },
                "{raw}"
            );
        }
    }

    #[test]
    fn parses_sub_bullet_routes_with_precedence_and_terminal_markers() {
        let cases = [
            ("@Cash+Goog-Exit Called today", "Called today", None, None),
            ("Called today @Cash+Goog-Exit", "Called today", None, None),
            (
                "Called today s:1 @Cash+Goog-Exit",
                "Called today",
                Some(1),
                None,
            ),
            (
                "Called today @Cash+Goog-Exit s:1",
                "Called today",
                Some(1),
                None,
            ),
            (
                "Called today %log @Cash+Goog-Exit",
                "Called today",
                None,
                Some("log"),
            ),
            (
                "Called today @Cash+Goog-Exit %log",
                "Called today",
                None,
                Some("log"),
            ),
        ];

        for (raw, body, scheduled_offset, clip_header) in cases {
            let parsed = parse_capture_text(raw, None)
                .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
            assert_eq!(parsed.body, body, "{raw}");
            assert_eq!(parsed.route.as_deref(), Some("cash"), "{raw}");
            assert_eq!(parsed.scheduled_offset, scheduled_offset, "{raw}");
            assert_eq!(
                parsed.kind,
                CaptureKind::SubBullet {
                    target: SubBulletTarget::BlockId("Goog-Exit".to_string())
                },
                "{raw}"
            );
            assert_eq!(
                parsed.clip,
                clip_header.map(|header| ClipRequest::Current {
                    header: Some(header.to_string())
                }),
                "{raw}"
            );
        }

        for raw in ["body @foo+bad:id", "body @foo+bad#section"] {
            let error = parse_capture_text(raw, None)
                .expect_err("plus must take precedence");
            assert!(error.message.contains("sub-bullet"), "{raw}: {error:?}");
        }
        for raw in ["body @foo^bad:id", "body @foo^bad#section"] {
            let error = parse_capture_text(raw, None)
                .expect_err("caret must take precedence");
            assert!(
                error.message.contains("task block-ID"),
                "{raw}: {error:?}"
            );
        }
        let error = parse_capture_text("body @foo::id", None)
            .expect_err("retired double colon is not ID-only or Pomodoro");
        assert!(
            error
                .message
                .contains("'@<route>::<block-id>' is no longer accepted"),
            "{error:?}"
        );
        let parsed = parse_capture_text("body @foo^id", None)
            .expect("caret is ordinary task-with-ID");
        assert!(matches!(parsed.kind, CaptureKind::TaskWithBlockId { .. }));
        let parsed = parse_capture_text("body @foo:id", None)
            .expect("colon remains Pomodoro");
        assert!(matches!(parsed.kind, CaptureKind::Pomodoro { .. }));
        let parsed = parse_capture_text("body @foo#section", None)
            .expect("hash remains bullet");
        assert!(matches!(parsed.kind, CaptureKind::Bullet { .. }));
    }

    #[test]
    fn malformed_sub_bullet_markers_are_usage_errors() {
        for (raw, expected) in [
            ("body @cash+", "requires a block ID"),
            ("body @+id", "must use @<route>+<block-id>"),
            ("body @bad.route+id", "route must contain"),
            ("body @cash+bad.id", "block ID must be"),
            ("body @cash+bad:id", "block ID must be"),
            ("@cash+id", "task text is required"),
        ] {
            let error = parse_capture_text(raw, None)
                .expect_err(&format!("{raw} should fail"));
            assert_eq!(error.kind, CaptureErrorKind::Usage, "{raw}");
            assert!(error.message.contains(expected), "{raw}: {error:?}");
        }

        let parsed = parse_capture_text("Discuss @cash+id later", None)
            .expect("mid-text marker remains literal");
        assert_eq!(parsed.body, "Discuss @cash+id later");
        assert_eq!(parsed.kind, CaptureKind::Task);
    }

    #[test]
    fn malformed_task_block_id_markers_are_usage_errors() {
        for (raw, expected) in [
            ("body @cash^", "block ID must be"),
            ("body @^id", "route must contain"),
            ("body @bad.route^id", "route must contain"),
            ("body @cash^bad.id", "block ID must be"),
            ("@cash^id", "task text is required"),
        ] {
            let error = parse_capture_text(raw, None)
                .expect_err(&format!("{raw} should fail"));
            assert_eq!(error.kind, CaptureErrorKind::Usage, "{raw}");
            assert!(error.message.contains(expected), "{raw}: {error:?}");
        }

        let parsed = parse_capture_text("Discuss @cash^id later", None)
            .expect("mid-text caret marker remains literal");
        assert_eq!(parsed.body, "Discuss @cash^id later");
        assert_eq!(parsed.kind, CaptureKind::Task);
    }

    #[test]
    fn retired_double_colon_markers_are_usage_errors() {
        for raw in [
            "body @cash::id",
            "body @cash::",
            "body @::id",
            "@cash::id body",
        ] {
            let error = parse_capture_text(raw, None)
                .expect_err(&format!("{raw} should fail"));
            assert_eq!(error.kind, CaptureErrorKind::Usage, "{raw}");
            assert!(
                error
                    .message
                    .contains("'@<route>::<block-id>' is no longer accepted"),
                "{raw}: {error:?}"
            );
        }

        let parsed = parse_capture_text("Discuss @cash::id later", None)
            .expect("mid-text retired marker remains literal");
        assert_eq!(parsed.body, "Discuss @cash::id later");
        assert_eq!(parsed.kind, CaptureKind::Task);
    }

    #[test]
    fn parses_picker_task_refs_strictly() {
        assert_eq!(
            parse_task_ref("24:1f3a9c2b").expect("valid ref"),
            SubBulletTarget::Ref {
                line: 24,
                digest: "1f3a9c2b".to_string()
            }
        );
        for value in ["", "0:1f3a9c2b", "24:ABCDEF12", "24:abc", "x:1f3a9c2b"] {
            let error = parse_task_ref(value).expect_err("invalid ref");
            assert_eq!(error.message, "--task-ref must use <line>:<digest>");
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
    fn assembles_capture_block_with_clip_children_then_schedule_log() {
        let capture_line = "- [ ] #task someday idea [created::2026-08-07] [priority::lowest] [scheduled::2026-11-02]";
        let clip_lines = vec![
            "\t- clip child one".to_string(),
            "\t- clip child two".to_string(),
        ];
        let schedule_log_lines = vec![
            "\t- 🗓️ **SCHEDULE LOG**".to_string(),
            "\t\t- *2026-11-02* — 🎲 P0 → P4 · in **91** (91–365) days"
                .to_string(),
        ];

        let block = assemble_capture_block(
            capture_line,
            None,
            Some(&clip_lines),
            Some(&schedule_log_lines),
        );

        assert_eq!(
            block,
            [
                capture_line,
                "\t- clip child one",
                "\t- clip child two",
                "\t- 🗓️ **SCHEDULE LOG**",
                "\t\t- *2026-11-02* — 🎲 P0 → P4 · in **91** (91–365) days",
            ]
            .join("\n")
        );
    }

    #[test]
    fn assembles_capture_block_with_sub_bullets_before_clip_and_schedule_log() {
        let capture_line = "- [ ] #task plan trip [created::2026-08-07]";
        let sub_bullet_lines = vec![
            "\t- book flights".to_string(),
            "\t- reserve hotel".to_string(),
        ];
        let clip_lines = vec!["\t- clip child".to_string()];
        let schedule_log_lines = vec!["\t- 🗓️ **SCHEDULE LOG**".to_string()];

        let block = assemble_capture_block(
            capture_line,
            Some(&sub_bullet_lines),
            Some(&clip_lines),
            Some(&schedule_log_lines),
        );

        assert_eq!(
            block,
            [
                capture_line,
                "\t- book flights",
                "\t- reserve hotel",
                "\t- clip child",
                "\t- 🗓️ **SCHEDULE LOG**",
            ]
            .join("\n")
        );
    }

    #[test]
    fn formats_task_line() {
        assert_eq!(
            format_task_line("buy milk", "2026-06-15", None, None),
            "- [ ] #task buy milk [created::2026-06-15]"
        );
        assert_eq!(
            format_task_line(
                "buy milk",
                "2026-06-15",
                None,
                Some("2026-06-16")
            ),
            "- [ ] #task buy milk [created::2026-06-15] [scheduled::2026-06-16]"
        );
        assert_eq!(
            format_task_line(
                "buy milk",
                "2026-06-15",
                Some(("priority", "high")),
                None,
            ),
            "- [ ] #task buy milk [created::2026-06-15] [priority::high]"
        );
        assert_eq!(
            format_task_line(
                "buy milk",
                "2026-06-15",
                Some(("priority", "high")),
                Some("2026-06-16"),
            ),
            "- [ ] #task buy milk [created::2026-06-15] [priority::high] [scheduled::2026-06-16]"
        );
    }

    #[test]
    fn formats_task_with_block_id_as_ordinary_task_with_final_block_id() {
        assert_eq!(
            format_task_with_block_id_line(
                "Some foobar task.",
                "2026-07-10",
                None,
                None,
                "foobar",
            ),
            "- [ ] #task Some foobar task. [created::2026-07-10] ^foobar"
        );
        assert_eq!(
            format_task_with_block_id_line(
                "Some foobar task.",
                "2026-07-10",
                Some(("priority", "lowest")),
                Some("2026-07-12"),
                "foobar",
            ),
            "- [ ] #task Some foobar task. [created::2026-07-10] [priority::lowest] [scheduled::2026-07-12] ^foobar"
        );
    }

    #[test]
    fn formats_pomodoro_task_with_block_id_as_final_token() {
        assert_eq!(
            format_pomodoro_task_line(
                "Some foobar task.",
                "2026-07-10",
                None,
                None,
                "foobar",
            ),
            "- [*] #task Some foobar task. [created::2026-07-10] ^foobar"
        );
        assert_eq!(
            format_pomodoro_task_line(
                "Some foobar task.",
                "2026-07-10",
                None,
                Some("2026-07-12"),
                "foobar",
            ),
            "- [*] #task Some foobar task. [created::2026-07-10] [scheduled::2026-07-12] ^foobar"
        );
        assert_eq!(
            format_pomodoro_task_line(
                "Some foobar task.",
                "2026-07-10",
                Some(("priority", "lowest")),
                Some("2026-07-12"),
                "foobar",
            ),
            "- [*] #task Some foobar task. [created::2026-07-10] [priority::lowest] [scheduled::2026-07-12] ^foobar"
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
        let result = CaptureResult::from_items(vec![CaptureItemResult {
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
            priority: None,
            priority_label: None,
            placement: Placement::Inserted,
            sub_bullets: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: None,
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            parent_status_symbol: None,
            parent_status_name: None,
        }]);

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
        assert!(value.get("sub_bullets").is_none(), "{value}");
        assert!(value.get("captures").is_none(), "{value}");
        for special_field in [
            "priority",
            "priority_label",
            "schedule_log",
            "block_id",
            "day_file",
            "block_link",
            "pomodoro_link_placement",
            "parent_line",
            "parent_text",
            "parent_status_symbol",
            "parent_status_name",
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
            format_bullet_line("some idea", "2026-06-15", None, None),
            "- some idea [created::2026-06-15]"
        );
        assert_eq!(
            format_bullet_line(
                "some idea",
                "2026-06-15",
                None,
                Some("2026-06-16")
            ),
            "- some idea [created::2026-06-15] [scheduled::2026-06-16]"
        );
        assert_eq!(
            format_bullet_line(
                "some idea",
                "2026-06-15",
                Some(("priority", "medium")),
                Some("2026-06-16"),
            ),
            "- some idea [created::2026-06-15] [priority::medium] [scheduled::2026-06-16]"
        );
    }

    #[test]
    fn formats_sub_bullet_line() {
        assert_eq!(
            format_sub_bullet_line("some idea", None, None),
            "- some idea"
        );
        assert_eq!(
            format_sub_bullet_line("some idea", None, Some("2026-06-16")),
            "- some idea [scheduled::2026-06-16]"
        );
        assert_eq!(
            format_sub_bullet_line(
                "some idea",
                Some(("priority", "low")),
                Some("2026-06-16"),
            ),
            "- some idea [priority::low] [scheduled::2026-06-16]"
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
