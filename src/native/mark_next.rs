use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::{OsStr, OsString},
    fs, io, iter,
    ops::Range,
    path::{Component, Path, PathBuf},
    process,
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::{json, Value};

use super::{
    collect_done, env as bob_env, is_always_excluded_note_directory_name,
    pomodoro,
    style::{display_width, pad_right, Styler},
};

const COMMAND_NAME: &str = "bob mark-next-tasks";
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
    match sync_next_tasks(&request) {
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
        .about("Sync Next tasks, including transcluded dependency chains")
        .long_about(
            "Make today's Pomodoro ledger the source of truth for Next tasks.\n\n\
Tasks block-linked from child bullets of open Pomodoro entries are promoted \
from [ ] to [*]. Their dependency tasks are discovered recursively from sole \
transcluded block-link child bullets and promoted too. Existing [*] tasks not \
reachable from an open entry are reset to [ ]. Completed linked-task references \
are retired as struck, non-embedded links. References found under open \
Pomodoros keep the existing policy of moving their containing bullets beneath \
the current timed Pomodoro, or the last completed Pomodoro when \
there is no current one. In-progress [/] tasks and all other statuses are left \
unchanged.\n\n\
Only Markdown checkbox lines allowed by the Obsidian Tasks globalFilter are \
considered. The scan skips hidden directories, templates, generated notes, \
and done archives. Missing daily notes and daily notes without a Pomodoros \
section, as well as notes with multiple open timed Pomodoros, fail before any \
file is changed.",
        )
        .after_help(
            "Examples:\n  bob mark-next-tasks\n  bob mark-next-tasks --dry-run\n  bob mark-next-tasks --format json\n  bob mark-next-tasks --bob-dir /tmp/bob-vault\n\nEnvironment:\n  BOB_DAY_FILE  exact daily note used as the Pomodoro source\n  BOB_DIR       Bob vault root when --bob-dir is omitted\n  BOB_NOW       current date/time override for daily-note selection",
        )
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
}

impl Request {
    fn from_matches(matches: &ArgMatches) -> Self {
        let bob_dir = matches
            .get_one::<OsString>("bob-dir")
            .map(PathBuf::from)
            .map(|path| bob_env::expand_tilde(&path))
            .unwrap_or_else(bob_env::bob_dir);
        Self {
            bob_dir,
            dry_run: matches.get_flag("dry-run"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ChangeItem {
    path: String,
    line_number: usize,
    block_id: String,
    description: String,
    dependency: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct UnresolvedReference {
    target: String,
    block_id: String,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct StruckCompletedReference {
    target: String,
    block_id: String,
    pomodoro: String,
    removed_embed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct MovedCompletedReference {
    target: String,
    block_id: String,
    source_pomodoro: String,
    destination_pomodoro: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SyncResult {
    ok: bool,
    dry_run: bool,
    daily_file: String,
    open_pomodoros: usize,
    references: usize,
    dependency_references: usize,
    scanned_files: usize,
    marked_next: Vec<ChangeItem>,
    cleared: Vec<ChangeItem>,
    struck_completed_references: Vec<StruckCompletedReference>,
    #[serde(default)]
    embedded_completed_references: Vec<StruckCompletedReference>,
    moved_completed_references: Vec<MovedCompletedReference>,
    kept_next: usize,
    kept_in_progress: usize,
    unresolved_references: Vec<UnresolvedReference>,
}

#[derive(Debug, Clone)]
struct FileScan {
    path: PathBuf,
    relative_path: PathBuf,
    contents: String,
    tasks: Vec<TaskLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskLine {
    line_index: usize,
    status: char,
    status_byte_offset: usize,
    block_id: Option<String>,
    description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RawReference {
    target: String,
    block_id: String,
}

#[derive(Debug, Clone)]
struct PlannedChange {
    file_index: usize,
    status_byte_offset: usize,
    replacement: char,
}

#[derive(Debug, Clone)]
struct TasksSettings {
    global_filter: String,
    done_statuses: BTreeSet<char>,
}

#[derive(Debug, Clone)]
struct PomodoroEntry {
    line_index: usize,
    end_line: usize,
    open: bool,
    completed: bool,
    timed: bool,
    child_indentation: Option<String>,
    context: String,
}

#[derive(Debug, Clone)]
struct LinkOccurrence {
    reference: RawReference,
    edit_start: usize,
    edit_end: usize,
    canonical_token: String,
    embedded: bool,
    canonical: bool,
}

#[derive(Debug, Clone)]
struct LinkBullet {
    entry_index: usize,
    line_index: usize,
    end_line: usize,
    indentation: String,
    links: Vec<LinkOccurrence>,
}

#[derive(Debug, Clone)]
struct PomodoroModel {
    entries: Vec<PomodoroEntry>,
    bullets: Vec<LinkBullet>,
    open_pomodoros: usize,
    raw_references: BTreeSet<RawReference>,
    all_references: BTreeSet<RawReference>,
}

#[derive(Debug, Clone)]
struct ResolvedReference {
    path: PathBuf,
    statuses: Vec<char>,
}

#[derive(Debug, Clone)]
struct StructuralPlan {
    token_edits: BTreeMap<usize, Vec<TokenEdit>>,
    moves: Vec<BulletMove>,
    target_entry: Option<usize>,
    struck: Vec<StruckCompletedReference>,
    moved: Vec<MovedCompletedReference>,
}

#[derive(Debug, Clone)]
struct TokenEdit {
    start: usize,
    end: usize,
    replacement: String,
}

#[derive(Debug, Clone)]
struct BulletMove {
    start_line: usize,
    end_line: usize,
    source_indentation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transition {
    Promote,
    Clear,
    KeptNext,
    KeptInProgress,
    Unchanged,
}

fn sync_next_tasks(request: &Request) -> Result<SyncResult, SyncError> {
    let daily_path = pomodoro::day_file_for(&request.bob_dir);
    let daily_contents = fs::read_to_string(&daily_path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            SyncError::new(format!(
                "daily note does not exist: {}",
                daily_path.display()
            ))
        } else {
            SyncError::io("read daily note", &daily_path, error)
        }
    })?;
    let daily_lines = logical_lines(&daily_contents);
    let section =
        pomodoro::pomodoros_section_range(&daily_lines).ok_or_else(|| {
            SyncError::new(format!(
                "daily note has no Pomodoros section: {}",
                daily_path.display()
            ))
        })?;
    let pomodoro_model = scan_pomodoros(&daily_lines, section.clone());
    let timed_open = pomodoro_model
        .entries
        .iter()
        .filter(|entry| entry.open && entry.timed)
        .count();
    if timed_open > 1 {
        return Err(SyncError::new(
            "Bob daily note has multiple open timed Pomodoros",
        ));
    }

    let settings = read_tasks_settings(&request.bob_dir);
    let markdown_files = markdown_files(&request.bob_dir).map_err(|error| {
        SyncError::io("scan vault", &request.bob_dir, error)
    })?;
    let mut files = Vec::with_capacity(markdown_files.len());
    for path in markdown_files {
        let contents = if same_file_path(&path, &daily_path) {
            daily_contents.clone()
        } else {
            fs::read_to_string(&path)
                .map_err(|error| SyncError::io("read note", &path, error))?
        };
        let relative_path = path
            .strip_prefix(&request.bob_dir)
            .map(Path::to_path_buf)
            .map_err(|_| {
                SyncError::new(format!(
                    "note is not under the vault root: {}",
                    path.display()
                ))
            })?;
        let tasks = parse_tasks(&contents, &settings.global_filter);
        files.push(FileScan {
            path,
            relative_path,
            contents,
            tasks,
        });
    }

    let note_index = NoteIndex::from_paths(
        files.iter().map(|file| file.relative_path.clone()),
    );
    let task_blocks = task_blocks(&files);
    let daily_relative = daily_path.strip_prefix(&request.bob_dir).ok();
    let mut unresolved = Vec::new();
    let dependency_edges =
        dependency_edges(&files, &note_index, &task_blocks, &mut unresolved);
    let mut resolved_references = BTreeMap::new();
    for reference in &pomodoro_model.all_references {
        let resolved =
            note_index.resolve(daily_relative, reference.target.trim());
        let Some(path) = resolved else {
            unresolved.push(UnresolvedReference {
                target: reference.target.clone(),
                block_id: reference.block_id.clone(),
                reason: "note target did not resolve uniquely".to_string(),
            });
            continue;
        };
        let key = (path.clone(), reference.block_id.clone());
        let statuses = task_blocks.get(&key).cloned().unwrap_or_default();
        if statuses.is_empty() {
            unresolved.push(UnresolvedReference {
                target: reference.target.clone(),
                block_id: reference.block_id.clone(),
                reason: format!(
                    "{} has no matching task block",
                    display_path(&path)
                ),
            });
            continue;
        }
        if statuses.len() > 1 {
            let has_done = statuses
                .iter()
                .any(|status| is_done_status(*status, &settings.done_statuses));
            let has_not_done = statuses.iter().any(|status| {
                !is_done_status(*status, &settings.done_statuses)
            });
            let reason = if has_done && has_not_done {
                format!(
                    "{} has {} tasks with conflicting statuses; completed-link normalization was skipped",
                    display_path(&path),
                    statuses.len()
                )
            } else {
                format!(
                    "{} has {} tasks with this block id; all were matched",
                    display_path(&path),
                    statuses.len()
                )
            };
            unresolved.push(UnresolvedReference {
                target: reference.target.clone(),
                block_id: reference.block_id.clone(),
                reason,
            });
        }
        resolved_references
            .insert(reference.clone(), ResolvedReference { path, statuses });
    }

    let structural_plan = plan_structural_changes(
        &pomodoro_model,
        &resolved_references,
        &settings.done_statuses,
    );
    let structurally_updated_daily = apply_structural_plan(
        &daily_contents,
        &pomodoro_model,
        &structural_plan,
    );
    let updated_lines = logical_lines(&structurally_updated_daily);
    let updated_section = pomodoro::pomodoros_section_range(&updated_lines)
        .expect("the structural rewrite preserves the Pomodoros section");
    let final_references =
        scan_pomodoros(&updated_lines, updated_section).raw_references;
    let direct_desired = final_references
        .iter()
        .filter_map(|reference| {
            resolved_references.get(reference).map(|resolved| {
                (resolved.path.clone(), reference.block_id.clone())
            })
        })
        .collect::<BTreeSet<_>>();
    let desired = dependency_closure(&direct_desired, &dependency_edges);
    let dependency_desired = desired
        .difference(&direct_desired)
        .cloned()
        .collect::<BTreeSet<_>>();

    let mut marked_next = Vec::new();
    let mut cleared = Vec::new();
    let mut changes = Vec::new();
    let mut kept_next = 0;
    let mut kept_in_progress = 0;
    for (file_index, file) in files.iter().enumerate() {
        for task in &file.tasks {
            let referenced = task.block_id.as_ref().is_some_and(|block_id| {
                desired
                    .contains(&(file.relative_path.clone(), block_id.clone()))
            });
            match transition(task.status, referenced) {
                Transition::Promote => {
                    let dependency =
                        task.block_id.as_ref().is_some_and(|block_id| {
                            dependency_desired.contains(&(
                                file.relative_path.clone(),
                                block_id.clone(),
                            ))
                        });
                    let item = change_item(file, task, dependency);
                    marked_next.push(item.clone());
                    changes.push(PlannedChange {
                        file_index,
                        status_byte_offset: task.status_byte_offset,
                        replacement: '*',
                    });
                }
                Transition::Clear => {
                    let item = change_item(file, task, false);
                    cleared.push(item.clone());
                    changes.push(PlannedChange {
                        file_index,
                        status_byte_offset: task.status_byte_offset,
                        replacement: ' ',
                    });
                }
                Transition::KeptNext => kept_next += 1,
                Transition::KeptInProgress => kept_in_progress += 1,
                Transition::Unchanged => {}
            }
        }
    }

    let outputs = compose_outputs(
        &files,
        &changes,
        &daily_path,
        &daily_contents,
        &pomodoro_model,
        &structural_plan,
    );
    if !request.dry_run {
        apply_outputs(&outputs)?;
    }

    Ok(SyncResult {
        ok: true,
        dry_run: request.dry_run,
        daily_file: daily_relative
            .map(display_path)
            .unwrap_or_else(|| daily_path.to_string_lossy().into_owned()),
        open_pomodoros: pomodoro_model.open_pomodoros,
        references: pomodoro_model.raw_references.len(),
        dependency_references: dependency_desired.len(),
        scanned_files: files.len(),
        marked_next,
        cleared,
        struck_completed_references: structural_plan.struck,
        embedded_completed_references: Vec::new(),
        moved_completed_references: structural_plan.moved,
        kept_next,
        kept_in_progress,
        unresolved_references: unresolved,
    })
}

fn transition(status: char, referenced: bool) -> Transition {
    match (status, referenced) {
        (' ', true) => Transition::Promote,
        ('*', false) => Transition::Clear,
        ('*', true) => Transition::KeptNext,
        ('/', true) => Transition::KeptInProgress,
        _ => Transition::Unchanged,
    }
}

fn same_file_path(left: &Path, right: &Path) -> bool {
    left == right
        || (left.exists()
            && right.exists()
            && left.canonicalize().ok() == right.canonicalize().ok())
}

fn logical_lines(contents: &str) -> Vec<&str> {
    contents.split_inclusive('\n').map(logical_line).collect()
}

fn logical_line(segment: &str) -> &str {
    let without_lf = segment.strip_suffix('\n').unwrap_or(segment);
    without_lf.strip_suffix('\r').unwrap_or(without_lf)
}

fn scan_pomodoros(lines: &[&str], section: Range<usize>) -> PomodoroModel {
    let fenced_lines = fenced_lines(lines, section.clone());
    let mut entries = Vec::new();
    for line_index in section.clone() {
        let line = lines[line_index];
        if fenced_lines.contains(&line_index)
            || line.starts_with(' ')
            || line.starts_with('\t')
            || !line.starts_with('-')
        {
            continue;
        }
        let open_task = pomodoro::open_ledger_task(line);
        let completed_task = pomodoro::completed_ledger_task(line);
        if open_task.is_none() && completed_task.is_none() {
            continue;
        }
        let end_line = entry_block_end(lines, line_index, section.end);
        let child_indentation = (line_index + 1..end_line)
            .filter(|index| !fenced_lines.contains(index))
            .find_map(|index| bullet_indentation(lines[index]));
        entries.push(PomodoroEntry {
            line_index,
            end_line,
            open: open_task.is_some(),
            completed: completed_task.is_some(),
            timed: open_task
                .is_some_and(|task| pomodoro::task_time_range(task).is_some()),
            child_indentation,
            context: line.trim_end().to_string(),
        });
    }

    let mut bullets = Vec::new();
    let mut raw_references = BTreeSet::new();
    let mut all_references = BTreeSet::new();
    for (entry_index, entry) in entries.iter().enumerate() {
        for line_index in entry.line_index + 1..entry.end_line {
            if fenced_lines.contains(&line_index) {
                continue;
            }
            let Some(indentation) = bullet_indentation(lines[line_index])
            else {
                continue;
            };
            let links = block_link_occurrences(lines[line_index]);
            if links.is_empty() {
                continue;
            }
            all_references
                .extend(links.iter().map(|link| link.reference.clone()));
            if entry.open {
                raw_references
                    .extend(links.iter().map(|link| link.reference.clone()));
            }
            bullets.push(LinkBullet {
                entry_index,
                line_index,
                end_line: bullet_block_end(
                    lines,
                    line_index,
                    entry.end_line,
                    indentation.len(),
                ),
                indentation,
                links,
            });
        }
    }

    PomodoroModel {
        open_pomodoros: entries.iter().filter(|entry| entry.open).count(),
        entries,
        bullets,
        raw_references,
        all_references,
    }
}

fn is_sub_bullet(line: &str) -> bool {
    if line
        .chars()
        .next()
        .is_some_and(|character| matches!(character, '-' | '*' | '+'))
    {
        return line.chars().nth(1).is_some_and(char::is_whitespace);
    }
    let digits = line
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    digits > 0
        && line.as_bytes().get(digits).is_some_and(|byte| {
            matches!(byte, b'.' | b')')
                && line
                    .as_bytes()
                    .get(digits + 1)
                    .is_some_and(u8::is_ascii_whitespace)
        })
}

fn bullet_indentation(line: &str) -> Option<String> {
    let indentation_len = line
        .as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    (indentation_len > 0 && is_sub_bullet(&line[indentation_len..]))
        .then(|| line[..indentation_len].to_string())
}

fn block_link_occurrences(line: &str) -> Vec<LinkOccurrence> {
    let mut links = Vec::new();
    let mut rest = line;
    let mut base = 0;
    while let Some(open) = rest.find("[[") {
        let absolute_open = base + open;
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("]]") else {
            break;
        };
        let inside = &after_open[..close];
        let link_end = absolute_open + 2 + close + 2;
        let link_target = inside.split('|').next().unwrap_or("");
        if let Some(fragment) = link_target.find("#^") {
            let target = link_target[..fragment].trim();
            let block_id = link_target[fragment + 2..].trim();
            if !block_id.is_empty()
                && block_id.bytes().all(collect_done::is_block_id_byte)
            {
                let embedded = line[..absolute_open].ends_with('!');
                let token_start = absolute_open - usize::from(embedded);
                let struck = line[..token_start].ends_with("~~")
                    && line[link_end..].starts_with("~~");
                let edit_start = token_start - if struck { 2 } else { 0 };
                let edit_end = link_end + if struck { 2 } else { 0 };
                let wikilink = &line[absolute_open..link_end];
                let canonical_token = format!("~~{wikilink}~~");
                let canonical = !embedded
                    && struck
                    && &line[edit_start..edit_end] == canonical_token;
                links.push(LinkOccurrence {
                    reference: RawReference {
                        target: target.to_string(),
                        block_id: block_id.to_string(),
                    },
                    edit_start,
                    edit_end,
                    canonical_token,
                    embedded,
                    canonical,
                });
            }
        }
        base = absolute_open + 2 + close + 2;
        rest = &after_open[close + 2..];
    }
    links
}

fn read_tasks_settings(vault: &Path) -> TasksSettings {
    let mut settings = TasksSettings {
        global_filter: DEFAULT_GLOBAL_FILTER.to_string(),
        done_statuses: BTreeSet::from(['x', 'X']),
    };
    let Ok(contents) = fs::read_to_string(vault.join(TASKS_SETTINGS)) else {
        return settings;
    };
    let Ok(value) = serde_json::from_str::<Value>(&contents) else {
        return settings;
    };
    settings.global_filter = value
        .get("globalFilter")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_GLOBAL_FILTER)
        .to_string();
    for collection in ["coreStatuses", "customStatuses"] {
        let statuses = value
            .get("statusSettings")
            .and_then(|settings| settings.get(collection))
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for status in statuses {
            if status.get("type").and_then(Value::as_str) != Some("DONE") {
                continue;
            }
            let Some(symbol) = status.get("symbol").and_then(Value::as_str)
            else {
                continue;
            };
            let mut chars = symbol.chars();
            if let Some(symbol) = chars.next()
                && chars.next().is_none()
            {
                settings.done_statuses.insert(symbol);
            }
        }
    }
    settings
}

#[derive(Debug, Clone, Copy)]
struct MarkdownFence {
    character: u8,
    length: usize,
}

fn fenced_lines(lines: &[&str], section: Range<usize>) -> BTreeSet<usize> {
    let mut fenced = BTreeSet::new();
    let mut open = None;
    for index in section {
        let line = lines[index];
        if let Some(marker) = open {
            fenced.insert(index);
            if closes_fence(line, marker) {
                open = None;
            }
        } else if let Some(marker) = fence_marker(line) {
            fenced.insert(index);
            open = Some(marker);
        }
    }
    fenced
}

fn fence_marker(line: &str) -> Option<MarkdownFence> {
    let indentation = line.bytes().take_while(|byte| *byte == b' ').count();
    if indentation > 3 {
        return None;
    }
    let line = &line[indentation..];
    let character = *line.as_bytes().first()?;
    if !matches!(character, b'`' | b'~') {
        return None;
    }
    let length = line.bytes().take_while(|byte| *byte == character).count();
    (length >= 3).then_some(MarkdownFence { character, length })
}

fn closes_fence(line: &str, open: MarkdownFence) -> bool {
    let Some(marker) = fence_marker(line) else {
        return false;
    };
    let trimmed = line.trim_start();
    marker.character == open.character
        && marker.length >= open.length
        && trimmed[marker.length..].trim().is_empty()
}

fn entry_block_end(
    lines: &[&str],
    entry_line: usize,
    section_end: usize,
) -> usize {
    let mut index = entry_line + 1;
    while index < section_end {
        if leading_indentation_len(lines[index]) > 0
            || (lines[index].trim().is_empty()
                && next_nonblank_is_indented(lines, index + 1, section_end))
        {
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn bullet_block_end(
    lines: &[&str],
    bullet_line: usize,
    entry_end: usize,
    indentation: usize,
) -> usize {
    let mut index = bullet_line + 1;
    while index < entry_end {
        let line = lines[index];
        if leading_indentation_len(line) > indentation
            || (line.trim().is_empty()
                && next_nonblank_more_indented(
                    lines,
                    index + 1,
                    entry_end,
                    indentation,
                ))
        {
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn leading_indentation_len(line: &str) -> usize {
    line.as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

fn next_nonblank_is_indented(lines: &[&str], start: usize, end: usize) -> bool {
    lines[start..end]
        .iter()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| leading_indentation_len(line) > 0)
}

fn next_nonblank_more_indented(
    lines: &[&str],
    start: usize,
    end: usize,
    indentation: usize,
) -> bool {
    lines[start..end]
        .iter()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| leading_indentation_len(line) > indentation)
}

fn is_done_status(status: char, done_statuses: &BTreeSet<char>) -> bool {
    matches!(status, 'x' | 'X') || done_statuses.contains(&status)
}

fn plan_structural_changes(
    model: &PomodoroModel,
    resolved: &BTreeMap<RawReference, ResolvedReference>,
    done_statuses: &BTreeSet<char>,
) -> StructuralPlan {
    let current = model
        .entries
        .iter()
        .position(|entry| entry.open && entry.timed);
    let fallback = model.entries.iter().rposition(|entry| entry.completed);
    let target_entry = current.or(fallback);
    let mut token_edits: BTreeMap<usize, Vec<TokenEdit>> = BTreeMap::new();
    let mut move_candidates = Vec::new();
    let mut struck = Vec::new();
    let mut moved = Vec::new();

    for bullet in &model.bullets {
        let completed_links = bullet
            .links
            .iter()
            .filter(|link| {
                resolved.get(&link.reference).is_some_and(|reference| {
                    !reference.statuses.is_empty()
                        && reference.statuses.iter().all(|status| {
                            is_done_status(*status, done_statuses)
                        })
                })
            })
            .collect::<Vec<_>>();
        if completed_links.is_empty() {
            continue;
        }
        let source = &model.entries[bullet.entry_index];
        for link in &completed_links {
            if !link.canonical {
                token_edits.entry(bullet.line_index).or_default().push(
                    TokenEdit {
                        start: link.edit_start,
                        end: link.edit_end,
                        replacement: link.canonical_token.clone(),
                    },
                );
                struck.push(StruckCompletedReference {
                    target: link.reference.target.clone(),
                    block_id: link.reference.block_id.clone(),
                    pomodoro: source.context.clone(),
                    removed_embed: link.embedded,
                });
            }
        }
        if !source.open {
            continue;
        }
        let Some(target) = target_entry else {
            continue;
        };
        if target == bullet.entry_index {
            continue;
        }
        move_candidates.push(BulletMove {
            start_line: bullet.line_index,
            end_line: bullet.end_line,
            source_indentation: bullet.indentation.clone(),
        });
        for link in completed_links {
            moved.push(MovedCompletedReference {
                target: link.reference.target.clone(),
                block_id: link.reference.block_id.clone(),
                source_pomodoro: source.context.clone(),
                destination_pomodoro: model.entries[target].context.clone(),
            });
        }
    }

    move_candidates.sort_by_key(|item| (item.start_line, item.end_line));
    let mut moves: Vec<BulletMove> = Vec::new();
    for candidate in move_candidates {
        if moves
            .last()
            .is_some_and(|previous| candidate.start_line < previous.end_line)
        {
            continue;
        }
        moves.push(candidate);
    }

    StructuralPlan {
        token_edits,
        moves,
        target_entry,
        struck,
        moved,
    }
}

fn apply_structural_plan(
    contents: &str,
    model: &PomodoroModel,
    plan: &StructuralPlan,
) -> String {
    if plan.token_edits.is_empty() && plan.moves.is_empty() {
        return contents.to_string();
    }
    let mut lines = contents
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    for (line_index, edits) in &plan.token_edits {
        let mut edits = edits.iter().collect::<Vec<_>>();
        edits.sort_by_key(|edit| edit.start);
        for edit in edits.into_iter().rev() {
            lines[*line_index]
                .replace_range(edit.start..edit.end, &edit.replacement);
        }
    }

    let target_indentation = plan.target_entry.map(|target| {
        model.entries[target]
            .child_indentation
            .clone()
            .or_else(|| {
                model
                    .entries
                    .iter()
                    .find_map(|entry| entry.child_indentation.clone())
            })
            .unwrap_or_else(|| "  ".to_string())
    });
    let mut removed = BTreeSet::new();
    let mut moved_lines = Vec::new();
    for item in &plan.moves {
        let target_indentation = target_indentation
            .as_deref()
            .expect("moves always have a target Pomodoro");
        for (index, line) in lines
            .iter()
            .enumerate()
            .take(item.end_line)
            .skip(item.start_line)
        {
            removed.insert(index);
            moved_lines.push(reindent_segment(
                line,
                &item.source_indentation,
                target_indentation,
            ));
        }
    }
    let insertion_line = plan
        .target_entry
        .map(|target| model.entries[target].end_line);
    let ending = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut output =
        String::with_capacity(contents.len() + plan.struck.len() * 4);
    for index in 0..=lines.len() {
        if insertion_line == Some(index) && !moved_lines.is_empty() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push_str(ending);
            }
            for line in &moved_lines {
                output.push_str(line);
            }
            if index < lines.len() && !output.ends_with('\n') {
                output.push_str(ending);
            }
        }
        if index < lines.len() && !removed.contains(&index) {
            output.push_str(&lines[index]);
        }
    }
    output
}

fn reindent_segment(
    segment: &str,
    source_indentation: &str,
    target_indentation: &str,
) -> String {
    let line = logical_line(segment);
    if line.is_empty() || !line.starts_with(source_indentation) {
        return segment.to_string();
    }
    let ending = &segment[line.len()..];
    format!(
        "{target_indentation}{}{ending}",
        &line[source_indentation.len()..]
    )
}

fn markdown_files(vault: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_markdown_files(vault, &mut files)?;
    Ok(files)
}

fn collect_markdown_files(
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let mut entries =
        fs::read_dir(directory)?.collect::<Result<Vec<_>, io::Error>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let name = entry.file_name();
            if should_skip_directory(&name) {
                continue;
            }
            collect_markdown_files(&path, files)?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn should_skip_directory(name: &OsStr) -> bool {
    name == OsStr::new("done")
        || name.to_str().is_some_and(|name| name.starts_with('.'))
        || is_always_excluded_note_directory_name(name)
}

fn parse_tasks(contents: &str, global_filter: &str) -> Vec<TaskLine> {
    let mut tasks = Vec::new();
    let mut byte_start = 0;
    for (line_index, segment) in contents.split_inclusive('\n').enumerate() {
        let line = logical_line(segment);
        if let Some(mut task) = parse_task_line(line, global_filter) {
            task.line_index = line_index;
            task.status_byte_offset += byte_start;
            tasks.push(task);
        }
        byte_start += segment.len();
    }
    tasks
}

fn parse_task_line(line: &str, global_filter: &str) -> Option<TaskLine> {
    let bytes = line.as_bytes();
    let mut index = bytes
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    index = after_list_marker(line, index)?;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    let close_offset = line[index + 1..].find(']')?;
    let close = index + 1 + close_offset;
    let status_text = &line[index + 1..close];
    let mut status_chars = status_text.chars();
    let status = status_chars.next()?;
    if status_chars.next().is_some() {
        return None;
    }
    let after_checkbox = &line[close + 1..];
    if !after_checkbox.is_empty()
        && !after_checkbox.starts_with(char::is_whitespace)
    {
        return None;
    }
    let body = after_checkbox.trim_start();
    if !body.contains(global_filter) {
        return None;
    }
    let block_id = trailing_block_id(body);
    let description =
        task_description(body, global_filter, block_id.as_deref());
    Some(TaskLine {
        line_index: 0,
        status,
        status_byte_offset: index + 1,
        block_id,
        description,
    })
}

fn after_list_marker(line: &str, index: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    if matches!(bytes.get(index), Some(b'-' | b'*' | b'+')) {
        return bytes
            .get(index + 1)
            .is_some_and(u8::is_ascii_whitespace)
            .then_some(index + 1);
    }
    let digits = bytes[index..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 || !matches!(bytes.get(index + digits), Some(b'.' | b')')) {
        return None;
    }
    bytes
        .get(index + digits + 1)
        .is_some_and(u8::is_ascii_whitespace)
        .then_some(index + digits + 1)
}

fn trailing_block_id(body: &str) -> Option<String> {
    let token = body.split_whitespace().next_back()?;
    let block_id = token.strip_prefix('^')?;
    (!block_id.is_empty()
        && block_id.bytes().all(collect_done::is_block_id_byte))
    .then(|| block_id.to_string())
}

fn task_description(
    body: &str,
    global_filter: &str,
    block_id: Option<&str>,
) -> String {
    let without_block = block_id
        .and_then(|block_id| {
            body.trim_end()
                .strip_suffix(&format!("^{block_id}"))
                .map(str::trim_end)
        })
        .unwrap_or(body);
    without_block
        .replacen(global_filter, "", 1)
        .trim()
        .to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NoteIndex {
    relative_paths: BTreeSet<PathBuf>,
    basename_paths: BTreeMap<String, Option<PathBuf>>,
}

impl NoteIndex {
    fn from_paths<I>(paths: I) -> Self
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let mut relative_paths = BTreeSet::new();
        let mut basename_paths = BTreeMap::new();
        for path in paths {
            if let Some(stem) = path.file_stem().and_then(OsStr::to_str) {
                let stem = stem.to_lowercase();
                match basename_paths.entry(stem) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(Some(path.clone()));
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        if entry.get().as_ref() != Some(&path) {
                            entry.insert(None);
                        }
                    }
                }
            }
            relative_paths.insert(path);
        }
        Self {
            relative_paths,
            basename_paths,
        }
    }

    fn resolve(
        &self,
        current_path: Option<&Path>,
        target: &str,
    ) -> Option<PathBuf> {
        if target.is_empty() {
            return current_path.map(Path::to_path_buf);
        }
        let candidate = target_to_markdown_path(target)?;
        if self.relative_paths.contains(&candidate) {
            return Some(candidate);
        }
        if target.contains('/') || target.contains('\\') {
            return None;
        }
        let basename = Path::new(target)
            .file_stem()
            .and_then(OsStr::to_str)?
            .to_lowercase();
        self.basename_paths
            .get(&basename)
            .and_then(|path| path.clone())
    }
}

fn target_to_markdown_path(target: &str) -> Option<PathBuf> {
    let mut path = PathBuf::new();
    for component in Path::new(target).components() {
        match component {
            Component::Normal(part) => path.push(part),
            _ => return None,
        }
    }
    if !path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    {
        path.set_extension("md");
    }
    Some(path)
}

fn task_blocks(files: &[FileScan]) -> BTreeMap<(PathBuf, String), Vec<char>> {
    let mut blocks = BTreeMap::new();
    for file in files {
        for task in &file.tasks {
            if let Some(block_id) = &task.block_id {
                blocks
                    .entry((file.relative_path.clone(), block_id.clone()))
                    .or_insert_with(Vec::new)
                    .push(task.status);
            }
        }
    }
    blocks
}

fn dependency_edges(
    files: &[FileScan],
    note_index: &NoteIndex,
    task_blocks: &BTreeMap<(PathBuf, String), Vec<char>>,
    unresolved: &mut Vec<UnresolvedReference>,
) -> BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>> {
    let mut edges: BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>> =
        BTreeMap::new();
    for file in files {
        let lines = logical_lines(&file.contents);
        let fenced = fenced_lines(&lines, 0..lines.len());
        for task in &file.tasks {
            let Some(source_block_id) = &task.block_id else {
                continue;
            };
            let source = (file.relative_path.clone(), source_block_id.clone());
            let source_indent =
                leading_indentation_width(lines[task.line_index]);
            for line_index in task.line_index + 1..lines.len() {
                let line = lines[line_index];
                if line.trim().is_empty() {
                    continue;
                }
                let indentation = leading_indentation_width(line);
                if indentation <= source_indent {
                    break;
                }
                if fenced.contains(&line_index)
                    || nearest_parent_list_item(&lines, line_index)
                        != Some(task.line_index)
                {
                    continue;
                }
                let Some(reference) = sole_transcluded_block_reference(line)
                else {
                    continue;
                };
                let Some(target_path) = note_index.resolve(
                    Some(&file.relative_path),
                    reference.target.trim(),
                ) else {
                    unresolved.push(UnresolvedReference {
                        target: reference.target,
                        block_id: reference.block_id,
                        reason: format!(
                            "dependency from {}:{} did not resolve uniquely",
                            display_path(&file.relative_path),
                            task.line_index + 1
                        ),
                    });
                    continue;
                };
                let target = (target_path.clone(), reference.block_id.clone());
                if !task_blocks.contains_key(&target) {
                    unresolved.push(UnresolvedReference {
                        target: reference.target,
                        block_id: reference.block_id,
                        reason: format!(
                            "dependency from {}:{} resolved to {}, which has no matching task block",
                            display_path(&file.relative_path),
                            task.line_index + 1,
                            display_path(&target_path)
                        ),
                    });
                    continue;
                }
                edges.entry(source.clone()).or_default().insert(target);
            }
        }
    }
    edges
}

fn dependency_closure(
    direct: &BTreeSet<(PathBuf, String)>,
    edges: &BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>>,
) -> BTreeSet<(PathBuf, String)> {
    let mut closure = direct.clone();
    let mut queue = direct.iter().cloned().collect::<VecDeque<_>>();
    while let Some(source) = queue.pop_front() {
        let Some(targets) = edges.get(&source) else {
            continue;
        };
        for target in targets {
            if closure.insert(target.clone()) {
                queue.push_back(target.clone());
            }
        }
    }
    closure
}

fn leading_indentation_width(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .fold(0, |width, byte| {
            if byte == b'\t' {
                width + (4 - width % 4)
            } else {
                width + 1
            }
        })
}

fn nearest_parent_list_item(
    lines: &[&str],
    child_line: usize,
) -> Option<usize> {
    let child_indent = leading_indentation_width(lines.get(child_line)?);
    for line_index in (0..child_line).rev() {
        let line = lines[line_index];
        if line.trim().is_empty()
            || leading_indentation_width(line) >= child_indent
        {
            continue;
        }
        let byte_indent = line
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        if after_list_marker(line, byte_indent).is_some() {
            return Some(line_index);
        }
    }
    None
}

fn sole_transcluded_block_reference(line: &str) -> Option<RawReference> {
    let byte_indent = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let marker_end = after_list_marker(line, byte_indent)?;
    let body = line[marker_end..].trim();
    let inside = body.strip_prefix("![[")?.strip_suffix("]]")?;
    if inside.contains('|') {
        return None;
    }
    let fragment = inside.find("#^")?;
    let target = inside[..fragment].trim();
    let block_id = inside[fragment + 2..].trim();
    (!block_id.is_empty()
        && block_id.bytes().all(collect_done::is_block_id_byte))
    .then(|| RawReference {
        target: target.to_string(),
        block_id: block_id.to_string(),
    })
}

fn change_item(
    file: &FileScan,
    task: &TaskLine,
    dependency: bool,
) -> ChangeItem {
    ChangeItem {
        path: display_path(&file.relative_path),
        line_number: task.line_index + 1,
        block_id: task.block_id.clone().unwrap_or_default(),
        description: task.description.clone(),
        dependency,
    }
}

fn display_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn compose_outputs(
    files: &[FileScan],
    changes: &[PlannedChange],
    daily_path: &Path,
    daily_contents: &str,
    pomodoro_model: &PomodoroModel,
    structural_plan: &StructuralPlan,
) -> Vec<(PathBuf, String)> {
    let mut by_file: BTreeMap<usize, Vec<&PlannedChange>> = BTreeMap::new();
    for change in changes {
        by_file.entry(change.file_index).or_default().push(change);
    }
    let mut updated = BTreeMap::new();
    for (file_index, mut file_changes) in by_file {
        let file = &files[file_index];
        file_changes.sort_by_key(|change| change.status_byte_offset);
        let mut contents = file.contents.clone();
        for change in file_changes.into_iter().rev() {
            let offset = change.status_byte_offset;
            let status_len = contents[offset..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
            contents.replace_range(
                offset..offset + status_len,
                &change.replacement.to_string(),
            );
        }
        updated.insert(file_index, contents);
    }

    let daily_file_index = files
        .iter()
        .position(|file| same_file_path(&file.path, daily_path));
    let daily_base = daily_file_index
        .and_then(|index| updated.get(&index))
        .map(String::as_str)
        .unwrap_or(daily_contents);
    let updated_daily =
        apply_structural_plan(daily_base, pomodoro_model, structural_plan);
    if let Some(index) = daily_file_index {
        if updated_daily != files[index].contents {
            updated.insert(index, updated_daily.clone());
        } else {
            updated.remove(&index);
        }
    }

    let mut outputs = updated
        .into_iter()
        .filter_map(|(index, contents)| {
            (contents != files[index].contents)
                .then(|| (files[index].path.clone(), contents))
        })
        .collect::<Vec<_>>();
    if daily_file_index.is_none() && updated_daily != daily_contents {
        outputs.push((daily_path.to_path_buf(), updated_daily));
    }
    outputs
}

fn apply_outputs(outputs: &[(PathBuf, String)]) -> Result<(), SyncError> {
    for (path, contents) in outputs {
        atomic_write(path, contents)
            .map_err(|error| SyncError::io("write note", path, error))?;
    }
    Ok(())
}

fn atomic_write(path: &Path, contents: &str) -> io::Result<()> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no file name: {}", path.display()),
        )
    })?;
    let mut temp_name = OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".{}.tmp", process::id()));
    let temp_path = path.with_file_name(temp_name);
    let _ = fs::remove_file(&temp_path);
    fs::write(&temp_path, contents)?;
    fs::rename(&temp_path, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp_path);
    })
}

fn print_result(result: &SyncResult, format: OutputFormat) {
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string(result).expect("serialize mark-next result")
        ),
        OutputFormat::Human => print_human_result(result),
    }
    print_warnings(result);
}

fn print_human_result(result: &SyncResult) {
    let styler = Styler::detect();
    let change_count = result.marked_next.len()
        + result.cleared.len()
        + result.struck_completed_references.len()
        + result.moved_completed_references.len();
    let prefix = if result.dry_run {
        styler.success_prefix(true)
    } else {
        styler.green("\u{2713}")
    };
    if change_count == 0 {
        println!(
            "{prefix} mark-next-tasks  {} \u{2014} already in sync, no changes",
            styler.cyan(&result.daily_file)
        );
        return;
    }

    println!(
        "{prefix} mark-next-tasks  {}",
        styler.cyan(&result.daily_file)
    );
    println!(
        "  {} open pomodoros \u{b7} {} direct references \u{b7} {} dependency references \u{b7} {} files scanned",
        result.open_pomodoros,
        result.references,
        result.dependency_references,
        result.scanned_files
    );
    print_change_section(
        &styler,
        if result.dry_run {
            "would mark next"
        } else {
            "marked next"
        },
        "[ ] \u{2192} [*]",
        &result.marked_next,
        true,
    );
    print_change_section(
        &styler,
        if result.dry_run {
            "would clear"
        } else {
            "cleared"
        },
        "[*] \u{2192} [ ]",
        &result.cleared,
        false,
    );
    print_completed_reference_sections(result);
    if result.kept_next > 0 || result.kept_in_progress > 0 {
        println!();
        println!(
            "  kept {} already next \u{b7} {} in progress",
            result.kept_next, result.kept_in_progress
        );
    }
    println!(
        "Summary: {} marked next, {} cleared, {} struck, {} moved",
        result.marked_next.len(),
        result.cleared.len(),
        result.struck_completed_references.len(),
        result.moved_completed_references.len()
    );
}

fn print_completed_reference_sections(result: &SyncResult) {
    if !result.struck_completed_references.is_empty() {
        println!();
        println!(
            "  {} completed references",
            if result.dry_run {
                "would retire"
            } else {
                "retired"
            }
        );
        for item in &result.struck_completed_references {
            println!(
                "    ~~[[{}#^{}]]~~  {}{}",
                item.target,
                item.block_id,
                item.pomodoro,
                if item.removed_embed {
                    " (removed embed)"
                } else {
                    ""
                }
            );
        }
    }
    if !result.moved_completed_references.is_empty() {
        println!();
        println!(
            "  {} completed references",
            if result.dry_run {
                "would move"
            } else {
                "moved"
            }
        );
        for item in &result.moved_completed_references {
            println!(
                "    [[{}#^{}]]  {} -> {}",
                item.target,
                item.block_id,
                item.source_pomodoro,
                item.destination_pomodoro
            );
        }
    }
}

fn print_change_section(
    styler: &Styler,
    heading: &str,
    transition: &str,
    changes: &[ChangeItem],
    promotion: bool,
) {
    if changes.is_empty() {
        return;
    }
    println!();
    println!("  {heading}");
    let description_width = changes
        .iter()
        .map(|change| display_width(&change.description))
        .max()
        .unwrap_or(0);
    for change in changes {
        let transition = if promotion {
            styler.green(transition)
        } else {
            styler.yellow(transition)
        };
        let description = pad_right(&change.description, description_width);
        println!(
            "    {transition}  {description}  {} ^{}{}",
            styler.cyan(&change.path),
            change.block_id,
            if change.dependency {
                " (dependency)"
            } else {
                ""
            }
        );
    }
}

fn print_warnings(result: &SyncResult) {
    let styler = Styler::detect();
    for warning in &result.unresolved_references {
        eprintln!(
            "{}: [[{}#^{}]] \u{2014} {}",
            styler.warning_prefix(),
            warning.target,
            warning.block_id,
            warning.reason
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncError {
    message: String,
}

impl SyncError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn io(action: &str, path: &Path, error: io::Error) -> Self {
        Self::new(format!("failed to {action} {}: {error}", path.display()))
    }
}

fn print_error(error: SyncError, format: OutputFormat) -> i32 {
    match format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {}", error.message),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": error.message }))
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(target: &str, block_id: &str) -> RawReference {
        RawReference {
            target: target.to_string(),
            block_id: block_id.to_string(),
        }
    }

    fn resolved(
        values: &[(&str, &str, Vec<char>)],
    ) -> BTreeMap<RawReference, ResolvedReference> {
        values
            .iter()
            .map(|(target, block_id, statuses)| {
                (
                    reference(target, block_id),
                    ResolvedReference {
                        path: PathBuf::from(format!("{target}.md")),
                        statuses: statuses.clone(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn extracts_only_block_links_under_open_pomodoros() {
        let lines = [
            "- [ ] Open (0900-0930)",
            "  - [[dev#^one]] and [[Projects/Alpha.md#^two|alias]]",
            "  - ignore [[note]], [[note#Heading]], and [[note|alias #^fake]]",
            "- [x] Closed (0930-1000)",
            "  - [[dev#^closed]]",
        ];
        let model = scan_pomodoros(&lines, 0..lines.len());
        assert_eq!(model.open_pomodoros, 1);
        assert_eq!(
            model.raw_references,
            BTreeSet::from([
                RawReference {
                    target: "Projects/Alpha.md".to_string(),
                    block_id: "two".to_string(),
                },
                RawReference {
                    target: "dev".to_string(),
                    block_id: "one".to_string(),
                },
            ])
        );
    }

    #[test]
    fn resolves_exact_and_unique_case_insensitive_basenames() {
        let index = NoteIndex::from_paths([
            PathBuf::from("Areas/Home.md"),
            PathBuf::from("Projects/Alpha.md"),
        ]);
        assert_eq!(
            index.resolve(None, "Projects/Alpha"),
            Some(PathBuf::from("Projects/Alpha.md"))
        );
        assert_eq!(
            index.resolve(None, "alpha.md"),
            Some(PathBuf::from("Projects/Alpha.md"))
        );
        assert_eq!(
            index.resolve(None, "HOME"),
            Some(PathBuf::from("Areas/Home.md"))
        );
    }

    #[test]
    fn ambiguous_basename_does_not_resolve() {
        let index = NoteIndex::from_paths([
            PathBuf::from("Areas/Home.md"),
            PathBuf::from("Projects/home.md"),
        ]);
        assert_eq!(index.resolve(None, "home"), None);
    }

    #[test]
    fn parses_task_markers_and_preserves_status_offsets() {
        let contents = concat!(
            "  1. [ ] #task Todo ^todo\r\n",
            "* [*] #task Next ^next\r\n",
            "+ [/] #task Working ^work\r\n",
            "- [*] not a task ^ignored\r\n",
        );
        let tasks = parse_tasks(contents, "#task");
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].status, ' ');
        assert_eq!(tasks[0].block_id.as_deref(), Some("todo"));
        assert_eq!(tasks[0].description, "Todo");
        assert_eq!(&contents[tasks[1].status_byte_offset..][..1], "*");
        assert_eq!(tasks[2].status, '/');
    }

    #[test]
    fn replacement_changes_only_status_and_preserves_crlf() {
        let contents = "  - [ ] #task Keep everything ^id\r\n";
        let task = parse_tasks(contents, "#task").remove(0);
        let mut changed = contents.to_string();
        changed.replace_range(
            task.status_byte_offset..task.status_byte_offset + 1,
            "*",
        );
        assert_eq!(changed, "  - [*] #task Keep everything ^id\r\n");
    }

    #[test]
    fn transition_matrix_only_promotes_todo_and_clears_unreferenced_next() {
        assert_eq!(transition(' ', true), Transition::Promote);
        assert_eq!(transition(' ', false), Transition::Unchanged);
        assert_eq!(transition('*', false), Transition::Clear);
        assert_eq!(transition('*', true), Transition::KeptNext);
        assert_eq!(transition('/', true), Transition::KeptInProgress);
        assert_eq!(transition('/', false), Transition::Unchanged);
        for status in ['x', 'X', '-', '?'] {
            assert_eq!(transition(status, true), Transition::Unchanged);
            assert_eq!(transition(status, false), Transition::Unchanged);
        }
    }

    #[test]
    fn parses_embedded_alias_and_mixed_block_links() {
        let links = block_link_occurrences(
            "  - ![[dev#^done|Done alias]] and [[dev#^todo]]",
        );
        assert_eq!(links.len(), 2);
        assert!(links[0].embedded);
        assert_eq!(links[0].reference, reference("dev", "done"));
        assert!(!links[1].embedded);
        assert_eq!(links[1].reference, reference("dev", "todo"));
    }

    #[test]
    fn dependency_reference_requires_a_sole_transcluded_block_link() {
        assert_eq!(
            sole_transcluded_block_reference("  - ![[Projects/A#^dep]]"),
            Some(reference("Projects/A", "dep"))
        );
        for line in [
            "  - [[#^plain]]",
            "  - ![[#^dep|alias]]",
            "  - text ![[#^dep]]",
            "  - ![[#^dep]] trailing",
            "  - ![[#heading]]",
        ] {
            assert_eq!(sole_transcluded_block_reference(line), None, "{line}");
        }
    }

    #[test]
    fn moves_completed_mixed_bullet_subtree_to_current_and_strikes_only_done() {
        let contents = concat!(
            "- [ ] Current (0900-0930)\n",
            "  - Existing child\n",
            "- [ ] Future\n",
            "    - [[dev#^done|Done]] and [[dev#^todo]]\n",
            "      - Nested detail\n",
            "  ```\n",
            "  - [[dev#^fenced]]\n",
            "  ```\n",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        assert!(!model.raw_references.contains(&reference("dev", "fenced")));
        let plan = plan_structural_changes(
            &model,
            &resolved(&[
                ("dev", "done", vec!['x']),
                ("dev", "todo", vec![' ']),
            ]),
            &BTreeSet::from(['x', 'X']),
        );
        let updated = apply_structural_plan(contents, &model, &plan);
        assert_eq!(
            updated,
            concat!(
                "- [ ] Current (0900-0930)\n",
                "  - Existing child\n",
                "  - ~~[[dev#^done|Done]]~~ and [[dev#^todo]]\n",
                "    - Nested detail\n",
                "- [ ] Future\n",
                "  ```\n",
                "  - [[dev#^fenced]]\n",
                "  ```\n",
            )
        );
        assert_eq!(plan.struck.len(), 1);
        assert_eq!(plan.moved.len(), 1);
    }

    #[test]
    fn repairs_completed_pomodoro_links_in_place_and_is_idempotent() {
        let contents = concat!(
            "- [x] Historical (0800-0830)\r\n",
            "  - ![[dev#^done|Embedded]] and [[dev#^done|Plain]]\r\n",
            "  - ~~![[dev#^done|Stale]]~~ and ~~[[dev#^done|Canonical]]~~\r\n",
            "- [ ] Current (0900-0930)\r\n",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        assert!(model.raw_references.is_empty());
        assert_eq!(model.all_references.len(), 1);
        let plan = plan_structural_changes(
            &model,
            &resolved(&[("dev", "done", vec!['x'])]),
            &BTreeSet::from(['x', 'X']),
        );
        assert!(plan.moves.is_empty());
        assert_eq!(plan.struck.len(), 3);
        let updated = apply_structural_plan(contents, &model, &plan);
        assert_eq!(
            updated,
            concat!(
                "- [x] Historical (0800-0830)\r\n",
                "  - ~~[[dev#^done|Embedded]]~~ and ~~[[dev#^done|Plain]]~~\r\n",
                "  - ~~[[dev#^done|Stale]]~~ and ~~[[dev#^done|Canonical]]~~\r\n",
                "- [ ] Current (0900-0930)\r\n",
            )
        );
        let updated_lines = logical_lines(&updated);
        let updated_model =
            scan_pomodoros(&updated_lines, 0..updated_lines.len());
        let second = plan_structural_changes(
            &updated_model,
            &resolved(&[("dev", "done", vec!['x'])]),
            &BTreeSet::from(['x', 'X']),
        );
        assert!(second.token_edits.is_empty());
        assert_eq!(
            apply_structural_plan(&updated, &updated_model, &second),
            updated
        );
    }

    #[test]
    fn conflicting_duplicate_statuses_are_not_normalized() {
        let contents = "- [ ] Future\n  - [[dev#^duplicate]]\n";
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        let plan = plan_structural_changes(
            &model,
            &resolved(&[("dev", "duplicate", vec!['x', ' '])]),
            &BTreeSet::from(['x', 'X']),
        );
        assert!(plan.token_edits.is_empty());
        assert!(plan.moves.is_empty());
        assert_eq!(apply_structural_plan(contents, &model, &plan), contents);
    }

    #[test]
    fn completion_classification_accepts_conventional_and_custom_done_only() {
        let done = BTreeSet::from(['x', 'X', 'D']);
        for status in ['x', 'X', 'D'] {
            assert!(is_done_status(status, &done));
        }
        for status in [' ', '*', '/', '-', '?'] {
            assert!(!is_done_status(status, &done));
        }
    }
}
