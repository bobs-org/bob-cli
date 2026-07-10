use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs, io, iter,
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
        .about("Sync Next-status tasks from today's open pomodoros")
        .long_about(
            "Make today's Pomodoro ledger the source of truth for Next tasks.\n\n\
Tasks block-linked from child bullets of open Pomodoro entries are promoted \
from [ ] to [*]. Existing [*] tasks not linked from an open entry are reset \
to [ ]. In-progress [/] tasks and all other statuses are left unchanged.\n\n\
Only Markdown checkbox lines allowed by the Obsidian Tasks globalFilter are \
considered. The scan skips hidden directories, templates, generated notes, \
and done archives. Missing daily notes and daily notes without a Pomodoros \
section fail before any file is changed.",
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct UnresolvedReference {
    target: String,
    block_id: String,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SyncResult {
    ok: bool,
    dry_run: bool,
    daily_file: String,
    open_pomodoros: usize,
    references: usize,
    scanned_files: usize,
    marked_next: Vec<ChangeItem>,
    cleared: Vec<ChangeItem>,
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
    let (open_pomodoros, raw_references) =
        extract_references(&daily_lines[section]);

    let global_filter = read_global_filter(&request.bob_dir);
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
        let tasks = parse_tasks(&contents, &global_filter);
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
    let task_block_counts = task_block_counts(&files);
    let daily_relative = daily_path.strip_prefix(&request.bob_dir).ok();
    let mut desired = BTreeSet::new();
    let mut unresolved = Vec::new();
    for reference in &raw_references {
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
        let matches = task_block_counts.get(&key).copied().unwrap_or(0);
        if matches == 0 {
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
        desired.insert(key);
        if matches > 1 {
            unresolved.push(UnresolvedReference {
                target: reference.target.clone(),
                block_id: reference.block_id.clone(),
                reason: format!(
                    "{} has {matches} tasks with this block id; all were matched",
                    display_path(&path)
                ),
            });
        }
    }

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
                    let item = change_item(file, task);
                    marked_next.push(item.clone());
                    changes.push(PlannedChange {
                        file_index,
                        status_byte_offset: task.status_byte_offset,
                        replacement: '*',
                    });
                }
                Transition::Clear => {
                    let item = change_item(file, task);
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

    if !request.dry_run {
        apply_changes(&files, &changes)?;
    }

    Ok(SyncResult {
        ok: true,
        dry_run: request.dry_run,
        daily_file: daily_relative
            .map(display_path)
            .unwrap_or_else(|| daily_path.to_string_lossy().into_owned()),
        open_pomodoros,
        references: raw_references.len(),
        scanned_files: files.len(),
        marked_next,
        cleared,
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

fn extract_references(lines: &[&str]) -> (usize, BTreeSet<RawReference>) {
    let mut open_pomodoros = 0;
    let mut current_open = false;
    let mut references = BTreeSet::new();

    for line in lines {
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if !indented && line.starts_with('-') {
            current_open = pomodoro::open_ledger_task(line).is_some();
            if current_open {
                open_pomodoros += 1;
            }
            continue;
        }
        if indented && current_open && is_sub_bullet(line.trim_start()) {
            references.extend(block_links(line));
        }
    }

    (open_pomodoros, references)
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

fn block_links(line: &str) -> Vec<RawReference> {
    let mut links = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find("[[") {
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("]]") else {
            break;
        };
        let inside = &after_open[..close];
        let link_target = inside.split('|').next().unwrap_or("");
        if let Some(fragment) = link_target.find("#^") {
            let target = link_target[..fragment].trim();
            let block_id = link_target[fragment + 2..].trim();
            if !block_id.is_empty()
                && block_id.bytes().all(collect_done::is_block_id_byte)
            {
                links.push(RawReference {
                    target: target.to_string(),
                    block_id: block_id.to_string(),
                });
            }
        }
        rest = &after_open[close + 2..];
    }
    links
}

fn read_global_filter(vault: &Path) -> String {
    let Ok(contents) = fs::read_to_string(vault.join(TASKS_SETTINGS)) else {
        return DEFAULT_GLOBAL_FILTER.to_string();
    };
    let Ok(value) = serde_json::from_str::<Value>(&contents) else {
        return DEFAULT_GLOBAL_FILTER.to_string();
    };
    value
        .get("globalFilter")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_GLOBAL_FILTER)
        .to_string()
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

fn task_block_counts(files: &[FileScan]) -> BTreeMap<(PathBuf, String), usize> {
    let mut counts = BTreeMap::new();
    for file in files {
        for task in &file.tasks {
            if let Some(block_id) = &task.block_id {
                *counts
                    .entry((file.relative_path.clone(), block_id.clone()))
                    .or_insert(0) += 1;
            }
        }
    }
    counts
}

fn change_item(file: &FileScan, task: &TaskLine) -> ChangeItem {
    ChangeItem {
        path: display_path(&file.relative_path),
        line_number: task.line_index + 1,
        block_id: task.block_id.clone().unwrap_or_default(),
        description: task.description.clone(),
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

fn apply_changes(
    files: &[FileScan],
    changes: &[PlannedChange],
) -> Result<(), SyncError> {
    let mut by_file: BTreeMap<usize, Vec<&PlannedChange>> = BTreeMap::new();
    for change in changes {
        by_file.entry(change.file_index).or_default().push(change);
    }
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
        atomic_write(&file.path, &contents)
            .map_err(|error| SyncError::io("write note", &file.path, error))?;
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
    let change_count = result.marked_next.len() + result.cleared.len();
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
        "  {} open pomodoros \u{b7} {} references \u{b7} {} files scanned",
        result.open_pomodoros, result.references, result.scanned_files
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
    if result.kept_next > 0 || result.kept_in_progress > 0 {
        println!();
        println!(
            "  kept {} already next \u{b7} {} in progress",
            result.kept_next, result.kept_in_progress
        );
    }
    println!(
        "Summary: {} marked next, {} cleared",
        result.marked_next.len(),
        result.cleared.len()
    );
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
            "    {transition}  {description}  {} ^{}",
            styler.cyan(&change.path),
            change.block_id
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

    #[test]
    fn extracts_only_block_links_under_open_pomodoros() {
        let lines = [
            "- [ ] Open (0900-0930)",
            "  - [[dev#^one]] and [[Projects/Alpha.md#^two|alias]]",
            "  - ignore [[note]], [[note#Heading]], and [[note|alias #^fake]]",
            "- [x] Closed (0930-1000)",
            "  - [[dev#^closed]]",
        ];
        let (open, references) = extract_references(&lines);
        assert_eq!(open, 1);
        assert_eq!(
            references,
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
}
