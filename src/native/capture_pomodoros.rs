//! Named-Pomodoro ledger scanner and read-only `bob capture-pomodoros` CLI.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fmt, fs, io, iter,
    path::{Path, PathBuf},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::{Serialize, Serializer};
use serde_json::json;

use super::{
    capture::{
        leading_spaces_or_tabs_len, line_spans, list_item_body,
        nearest_shallower_list_item_parent, LineSpan,
    },
    capture_language, capture_task_sections, env as bob_env, markdown,
    note_tasks, pomodoro,
    style::{display_width, pad_right, Styler},
};

const COMMAND_NAME: &str = "bob capture-pomodoros";
pub(crate) const POMODORO_NAME_USAGE: &str =
    "Pomodoro name must contain only A-Z, 0-9 or \
`& ' ( ) + , . / -` and must start with a letter or digit";

/// Bump only for a breaking change to the JSON object below; new optional
/// fields keep version 1.
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
    let request = CapturePomodorosRequest::from_matches(&matches);

    match list_capture_pomodoros(&request) {
        Ok(result) => {
            print_success(&result, output_format);
            0
        }
        Err(error) => print_error(error, output_format),
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
        .about("List today's Pomodoro ledger entries")
        .long_about(
            "List Pomodoro entries from today's Bob daily note.\n\n\
The command is read-only and reports Pomodoros in document order with stable \
refs, names, selector slugs, time ranges, current-session status, and child \
link counts for picker callers. Open entries are listed by default; use \
--all to include completed entries. A missing daily note or missing \
Pomodoros section returns a successful empty list with a warning.",
        )
        .after_help(
            "Examples:\n  bob capture-pomodoros\n  bob capture-pomodoros --all\n  bob capture-pomodoros -f json\n  bob capture-pomodoros -b ~/bob -a -f json\n\nEnvironment:\n  BOB_DAY_FILE              Daily note override; otherwise <bob-dir>/YYYY/YYYYMMDD.md\n  BOB_DIR                   Bob vault root when --bob-dir is omitted\n  BOB_NOW                   Local datetime override for default daily-note selection",
        )
        .disable_help_flag(true)
        .arg(all_arg())
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
}

fn all_arg() -> Arg {
    Arg::new("all")
        .long("all")
        .short('a')
        .action(ArgAction::SetTrue)
        .help("Include completed Pomodoro entries")
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturePomodorosRequest {
    bob_dir: PathBuf,
    include_all: bool,
}

impl CapturePomodorosRequest {
    fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: bob_dir_from_matches(matches),
            include_all: matches.get_flag("all"),
        }
    }
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CapturePomodorosResult {
    ok: bool,
    schema_version: u32,
    day_file: String,
    relative_day_file: String,
    count: usize,
    pomodoros: Vec<PomodoroEntry>,
    warnings: Vec<String>,
}

fn list_capture_pomodoros(
    request: &CapturePomodorosRequest,
) -> Result<CapturePomodorosResult, CapturePomodorosError> {
    let day_file = pomodoro::day_file_for(&request.bob_dir);
    let relative_day_file = relative_day_file(&day_file, &request.bob_dir);
    let mut warnings = Vec::new();
    let contents = match fs::read_to_string(&day_file) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            warnings.push(bounded_warning(format!(
                "Bob daily note does not exist: {}",
                day_file.display()
            )));
            return Ok(CapturePomodorosResult {
                ok: true,
                schema_version: SCHEMA_VERSION,
                day_file: day_file.display().to_string(),
                relative_day_file,
                count: 0,
                pomodoros: Vec::new(),
                warnings,
            });
        }
        Err(error) => {
            return Err(CapturePomodorosError::io(format!(
                "read daily note {}: {error}",
                day_file.display()
            )));
        }
    };

    let scan = scan(&contents);
    if !scan.has_section {
        warnings.push(bounded_warning(format!(
            "Bob daily note has no Pomodoros section: {}",
            day_file.display()
        )));
    }
    warnings.extend(scan.warnings.iter().cloned());
    let pomodoros = scan
        .entries
        .into_iter()
        .filter(|entry| {
            request.include_all || entry.state == PomodoroState::Open
        })
        .collect::<Vec<_>>();

    Ok(CapturePomodorosResult {
        ok: true,
        schema_version: SCHEMA_VERSION,
        day_file: day_file.display().to_string(),
        relative_day_file,
        count: pomodoros.len(),
        pomodoros,
        warnings,
    })
}

pub(crate) fn relative_day_file(day_file: &Path, bob_dir: &Path) -> String {
    day_file
        .strip_prefix(bob_dir)
        .unwrap_or(day_file)
        .display()
        .to_string()
}

fn bounded_warning(message: String) -> String {
    const LIMIT: usize = 300;
    if message.chars().count() <= LIMIT {
        return message;
    }
    let mut truncated = message.chars().take(LIMIT - 3).collect::<String>();
    truncated.push_str("...");
    truncated
}

pub(crate) fn canonicalize_pomodoro_name(raw: &str) -> Option<String> {
    let name = capture_language::normalize_task_text(raw).to_ascii_uppercase();
    capture_task_sections::is_pomodoro_name(&name).then_some(name)
}

pub(crate) fn format_named_placeholder_line(name: &str) -> String {
    format!("- [ ] () — {name}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PomodoroScan {
    pub(crate) has_section: bool,
    pub(crate) entries: Vec<PomodoroEntry>,
    pub(crate) warnings: Vec<String>,
}

impl PomodoroScan {
    /// Resolve a Pomodoro ref whose line component is one-based.
    pub(crate) fn by_ref(
        &self,
        pomodoro_ref: &PomodoroRef,
    ) -> PomodoroRefLookup<'_> {
        if pomodoro_ref.line > 0
            && let Some(entry) = self
                .entries
                .iter()
                .find(|entry| entry.line == pomodoro_ref.line)
            && entry.pomodoro_ref.digest == pomodoro_ref.digest
        {
            return PomodoroRefLookup::Found(entry);
        }

        let mut matches = self
            .entries
            .iter()
            .filter(|entry| entry.pomodoro_ref.digest == pomodoro_ref.digest);
        match (matches.next(), matches.next()) {
            (Some(entry), None) => PomodoroRefLookup::Found(entry),
            (None, _) => PomodoroRefLookup::Stale,
            (Some(_), Some(_)) => PomodoroRefLookup::Ambiguous,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PomodoroRefLookup<'a> {
    Found(&'a PomodoroEntry),
    Stale,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PomodoroEntry {
    #[serde(rename = "ref")]
    pub(crate) pomodoro_ref: PomodoroRef,
    pub(crate) line: usize,
    pub(crate) state: PomodoroState,
    pub(crate) status_symbol: char,
    pub(crate) name: Option<String>,
    pub(crate) slug: String,
    pub(crate) selectable: bool,
    pub(crate) time_range: Option<String>,
    pub(crate) placeholder: bool,
    pub(crate) is_current: bool,
    pub(crate) child_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PomodoroRef {
    pub(crate) line: usize,
    pub(crate) digest: String,
}

impl PomodoroRef {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let (line, digest) = value.split_once(':')?;
        let line = line.parse::<usize>().ok().filter(|line| *line > 0)?;
        let valid_digest = digest.len() == 8
            && digest.bytes().all(|byte| {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            });
        valid_digest.then(|| Self {
            line,
            digest: digest.to_string(),
        })
    }
}

impl fmt::Display for PomodoroRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.line, self.digest)
    }
}

impl Serialize for PomodoroRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PomodoroState {
    Open,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NamedSelection<'a> {
    Found(&'a PomodoroEntry),
    CompletedOnly(&'a PomodoroEntry),
    Missing {
        suggestion: Option<&'a PomodoroEntry>,
    },
}

pub(crate) fn scan(contents: &str) -> PomodoroScan {
    let lines = line_spans(contents);
    let line_text = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let Some(section) = pomodoro::pomodoros_section_range(&line_text) else {
        return PomodoroScan {
            has_section: false,
            entries: Vec::new(),
            warnings: Vec::new(),
        };
    };
    let fenced = markdown::fenced_lines(&line_text, section.clone());
    let mut entries = Vec::new();

    for index in section.clone() {
        if fenced.contains(&index) {
            continue;
        }
        let line = lines[index].text;
        if leading_spaces_or_tabs_len(line) > 0 {
            continue;
        }
        let Some((state, status_symbol, body)) = parse_ledger_task(line) else {
            continue;
        };
        let parts = parse_entry_body(body);
        let slug = parts
            .name
            .as_deref()
            .map(capture_language::selector_slug)
            .unwrap_or_default();
        let selectable = parts.name.is_some()
            && capture_language::is_pomodoro_selector_component(&slug);
        entries.push(PomodoroEntry {
            pomodoro_ref: PomodoroRef {
                line: index + 1,
                digest: note_tasks::line_digest(line),
            },
            line: index + 1,
            state,
            status_symbol,
            name: parts.name,
            slug,
            selectable,
            time_range: parts.time_range,
            placeholder: parts.placeholder,
            is_current: false,
            child_count: direct_child_count(
                &lines,
                index,
                section.end,
                &fenced,
            ),
        });
    }

    let timed_open = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry.state == PomodoroState::Open && entry.time_range.is_some()
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let mut warnings = Vec::new();
    if timed_open.len() == 1 {
        entries[timed_open[0]].is_current = true;
    } else if timed_open.len() > 1 {
        warnings.push(
            "Bob daily note has multiple open timed Pomodoros".to_string(),
        );
    }

    PomodoroScan {
        has_section: true,
        entries,
        warnings,
    }
}

pub(crate) fn select_named<'a>(
    scan: &'a PomodoroScan,
    selector: &str,
) -> NamedSelection<'a> {
    let open = scan
        .entries
        .iter()
        .filter(|entry| entry.state == PomodoroState::Open && entry.selectable)
        .collect::<Vec<_>>();
    if let Some(entry) = select_by_slug(&open, selector) {
        return NamedSelection::Found(entry);
    }

    let completed = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == PomodoroState::Completed && entry.selectable
        })
        .collect::<Vec<_>>();
    if let Some(entry) = select_by_slug(&completed, selector) {
        return NamedSelection::CompletedOnly(entry);
    }

    let requested = capture_language::selector_slug(selector);
    let suggestion = if requested.is_empty() {
        None
    } else {
        let mut close = open.into_iter().filter(|entry| {
            note_tasks::bounded_levenshtein(&requested, &entry.slug, 2)
                .is_some()
        });
        match (close.next(), close.next()) {
            (Some(entry), None) => Some(entry),
            _ => None,
        }
    };
    NamedSelection::Missing { suggestion }
}

/// Canonical name for a future Pomodoro `bob capture` would create for
/// `selector`, if that selector is valid and today's ledger can uniquely
/// place the new entry. `None` when an open exact/prefix name would win,
/// the name is empty or invalid, the Pomodoros section is missing, or
/// multiple open timed Pomodoros make the insertion anchor ambiguous.
pub(crate) fn named_creation_name(
    scan: &PomodoroScan,
    selector: &str,
) -> Option<String> {
    if selector.is_empty() {
        return None;
    }
    let name = canonicalize_pomodoro_name(selector)?;
    if !scan.has_section {
        return None;
    }
    let timed_open = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == PomodoroState::Open && entry.time_range.is_some()
        })
        .count();
    if timed_open > 1 {
        return None;
    }
    match select_named(scan, selector) {
        NamedSelection::Found(_) => None,
        NamedSelection::CompletedOnly(_) | NamedSelection::Missing { .. } => {
            Some(name)
        }
    }
}

fn select_by_slug<'a>(
    entries: &[&'a PomodoroEntry],
    selector: &str,
) -> Option<&'a PomodoroEntry> {
    let needle = capture_language::selector_slug(selector);
    if needle.is_empty() {
        return None;
    }
    entries
        .iter()
        .copied()
        .find(|entry| entry.slug == needle)
        .or_else(|| {
            entries
                .iter()
                .copied()
                .find(|entry| entry.slug.starts_with(&needle))
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedEntryBody {
    name: Option<String>,
    time_range: Option<String>,
    placeholder: bool,
}

fn parse_entry_body(body: &str) -> ParsedEntryBody {
    let range = leading_range(body);
    let name = range
        .range_len
        .and_then(|range_len| parse_name_tail(&body[range_len..]));
    ParsedEntryBody {
        name,
        time_range: range.time_range,
        placeholder: range.placeholder,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LeadingRange {
    range_len: Option<usize>,
    time_range: Option<String>,
    placeholder: bool,
}

fn leading_range(body: &str) -> LeadingRange {
    if let Some(range_len) = placeholder_range_len(body) {
        return LeadingRange {
            range_len: Some(range_len),
            time_range: None,
            placeholder: true,
        };
    }

    if let Some((raw_range, start, end)) = pomodoro::task_time_range(body)
        && body.starts_with(raw_range)
    {
        return LeadingRange {
            range_len: Some(raw_range.len()),
            time_range: Some(format!("{start}-{end}")),
            placeholder: false,
        };
    }

    LeadingRange {
        range_len: None,
        time_range: None,
        placeholder: false,
    }
}

fn placeholder_range_len(body: &str) -> Option<usize> {
    let rest = body.strip_prefix('(')?;
    let whitespace_len = rest
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    rest[whitespace_len..]
        .strip_prefix(')')
        .map(|_| '('.len_utf8() + whitespace_len + ')'.len_utf8())
}

fn parse_name_tail(remaining_body: &str) -> Option<String> {
    let trimmed = remaining_body.trim_start_matches([' ', '\t']);
    let rest = trimmed.strip_prefix('—')?;
    let name = rest.trim_start_matches([' ', '\t']).trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Strip a parsed em-dash name tail from a physical Pomodoro line.
pub(crate) fn without_name_tail(line: &str) -> &str {
    let Some((_, _, body)) = parse_ledger_task(line) else {
        return line;
    };
    let Some(range_len) = leading_range(body).range_len else {
        return line;
    };
    let remaining = &body[range_len..];
    if parse_name_tail(remaining).is_none() {
        return line;
    }
    line.strip_suffix(remaining).unwrap_or(line)
}

fn parse_ledger_task(line: &str) -> Option<(PomodoroState, char, &str)> {
    let status_symbol = checkbox_status_symbol(line)?;
    if let Some(body) = pomodoro::completed_ledger_task(line) {
        return Some((PomodoroState::Completed, status_symbol, body));
    }
    pomodoro::open_ledger_task(line)
        .map(|body| (PomodoroState::Open, status_symbol, body))
}

fn checkbox_status_symbol(line: &str) -> Option<char> {
    let rest = line.strip_prefix("- [")?;
    let mut chars = rest.chars();
    let status_symbol = chars.next()?;
    let after_status = &rest[status_symbol.len_utf8()..];
    after_status.strip_prefix(']')?;
    Some(status_symbol)
}

fn direct_child_count(
    lines: &[LineSpan<'_>],
    parent_index: usize,
    section_end: usize,
    fenced: &BTreeSet<usize>,
) -> usize {
    let mut count = 0;
    for index in parent_index + 1..section_end {
        let line = lines[index].text;
        if !line.trim().is_empty() && leading_spaces_or_tabs_len(line) == 0 {
            break;
        }
        if fenced.contains(&index) {
            continue;
        }
        if list_item_body(line).is_some()
            && nearest_shallower_list_item_parent(lines, index)
                == Some(parent_index)
        {
            count += 1;
        }
    }
    count
}

fn print_success(result: &CapturePomodorosResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CapturePomodorosResult) {
    let styler = Styler::detect();
    print!("{}", human_success(result, &styler));
}

fn human_success(result: &CapturePomodorosResult, styler: &Styler) -> String {
    let mut output = format!(
        "Capture Pomodoros {} {}\n\n",
        styler.separator(),
        styler.cyan(&result.relative_day_file)
    );

    for warning in &result.warnings {
        output.push_str(&format!(
            "  {} {} {}\n",
            styler.warning_prefix(),
            styler.separator(),
            warning
        ));
    }
    if !result.warnings.is_empty() {
        output.push('\n');
    }

    if result.pomodoros.is_empty() {
        output.push_str("  No Pomodoros found.\n");
    } else {
        let name_width = result
            .pomodoros
            .iter()
            .map(|entry| {
                display_width(entry.name.as_deref().unwrap_or("unnamed"))
            })
            .max()
            .unwrap_or(0);
        let slug_width = result
            .pomodoros
            .iter()
            .map(|entry| display_width(slug_label(entry)))
            .max()
            .unwrap_or(0);
        let time_width = result
            .pomodoros
            .iter()
            .map(|entry| display_width(time_label(entry)))
            .max()
            .unwrap_or(0);

        for entry in &result.pomodoros {
            let raw_name = entry.name.as_deref().unwrap_or("unnamed");
            let name = if entry.name.is_some() && entry.selectable {
                styler.cyan(&pad_right(raw_name, name_width))
            } else {
                styler.dim(&pad_right(raw_name, name_width))
            };
            let slug = styler.dim(&pad_right(slug_label(entry), slug_width));
            let time = pad_right(time_label(entry), time_width);
            let badges = entry_badges(entry).join(" ");
            output.push_str(&format!("  {name}  {slug}  {time}  {badges}\n"));
        }
    }

    output.push('\n');
    output.push_str(&format!(
        "{} {}\n",
        result.count,
        plural(result.count, "Pomodoro", "Pomodoros")
    ));
    output
}

fn slug_label(entry: &PomodoroEntry) -> &str {
    if entry.slug.is_empty() {
        "-"
    } else {
        &entry.slug
    }
}

fn time_label(entry: &PomodoroEntry) -> &str {
    entry.time_range.as_deref().unwrap_or("planned")
}

fn entry_badges(entry: &PomodoroEntry) -> Vec<String> {
    let mut badges = Vec::new();
    if entry.is_current {
        badges.push("current".to_string());
    }
    if entry.state == PomodoroState::Completed {
        badges.push("completed".to_string());
    }
    if entry.child_count == 0 {
        badges.push("empty".to_string());
    } else {
        badges.push(format!(
            "{} {}",
            entry.child_count,
            plural(entry.child_count, "link", "links")
        ));
    }
    badges
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

fn success_json(result: &CapturePomodorosResult) -> String {
    serde_json::to_string(result).expect("serialize capture pomodoros result")
}

fn print_error(
    error: CapturePomodorosError,
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
struct CapturePomodorosError {
    kind: CapturePomodorosErrorKind,
    message: String,
}

impl CapturePomodorosError {
    fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CapturePomodorosErrorKind::Io,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapturePomodorosErrorKind {
    Io,
}

impl CapturePomodorosErrorKind {
    fn exit_code(self) -> i32 {
        match self {
            Self::Io => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn entries(contents: &str) -> Vec<PomodoroEntry> {
        scan(contents).entries
    }

    fn entry_named<'a>(
        entries: &'a [PomodoroEntry],
        name: &str,
    ) -> &'a PomodoroEntry {
        entries
            .iter()
            .find(|entry| entry.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("missing {name:?}: {entries:#?}"))
    }

    #[test]
    fn scans_timed_placeholder_and_range_less_entries() {
        let entries = entries(concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [ ] (**09:20 - 09:50** [t:: 30m]) — CURRENT\n",
            "- [ ] () — PLANNED\n",
            "- [ ] plain checklist item — NOT A NAME\n",
        ));
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].time_range.as_deref(), Some("0920-0950"));
        assert!(!entries[0].placeholder);
        assert_eq!(entries[0].name.as_deref(), Some("CURRENT"));
        assert_eq!(entries[1].time_range, None);
        assert!(entries[1].placeholder);
        assert_eq!(entries[1].name.as_deref(), Some("PLANNED"));
        assert_eq!(entries[2].time_range, None);
        assert!(!entries[2].placeholder);
        assert_eq!(entries[2].name, None);
    }

    #[test]
    fn parses_names_after_range_tail_only() {
        let entries = entries(concat!(
            "## Pomodoros\n",
            "- [ ] (0920-0950)  —  DEEP  WORK  \n",
            "- [ ] () — NAME — WITH DASH\n",
            "- [ ] (0955-1025) stray — NOPE\n",
            "- [ ] (1030-1100) —   \n",
        ));
        assert_eq!(entries[0].name.as_deref(), Some("DEEP  WORK"));
        assert_eq!(entries[0].slug, "deep-work");
        assert_eq!(entries[1].name.as_deref(), Some("NAME — WITH DASH"));
        assert_eq!(entries[2].name, None);
        assert_eq!(entries[3].name, None);
    }

    #[test]
    fn includes_completed_entries_and_status_symbols() {
        let entries = entries(concat!(
            "## Pomodoros\n",
            "- [x] (0900-0930) — DONE\n",
            "- [/] (0935-1005) — ACTIVE\n",
            "- [-] (1010-1040) — CANCELED\n",
        ));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].state, PomodoroState::Completed);
        assert_eq!(entries[0].status_symbol, 'x');
        assert_eq!(entries[1].state, PomodoroState::Open);
        assert_eq!(entries[1].status_symbol, '/');
    }

    #[test]
    fn ignores_nested_and_fenced_lookalikes() {
        let entries = entries(concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — REAL\n",
            "\t- [ ] (0935-1005) — NESTED\n",
            "```md\n",
            "- [ ] (1010-1040) — FENCED\n",
            "```\n",
            "- [ ] () — AFTER\n",
        ));
        assert_eq!(
            entries
                .iter()
                .filter_map(|entry| entry.name.as_deref())
                .collect::<Vec<_>>(),
            ["REAL", "AFTER"]
        );
        assert_eq!(entry_named(&entries, "REAL").child_count, 1);
    }

    #[test]
    fn current_requires_exactly_one_open_timed_entry() {
        let none = entries("## Pomodoros\n- [ ] ()\n");
        assert!(!none[0].is_current);

        let one = entries(concat!(
            "## Pomodoros\n",
            "- [x] (0800-0830) — DONE\n",
            "- [ ] (0900-0930) — ONE\n",
            "- [ ] () — LATER\n",
        ));
        assert!(entry_named(&one, "ONE").is_current);
        assert!(!entry_named(&one, "LATER").is_current);

        let two = scan(concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — ONE\n",
            "- [ ] (0935-1005) — TWO\n",
        ));
        assert!(two.entries.iter().all(|entry| !entry.is_current));
        assert_eq!(
            two.warnings,
            ["Bob daily note has multiple open timed Pomodoros"]
        );
    }

    #[test]
    fn classifies_slugs_and_selectability() {
        let entries = entries(concat!(
            "## Pomodoros\n",
            "- [ ] () — Q&A (DRAFT), V2.0/OK-GO\n",
            "- [ ] () — SNAKE_CASE\n",
            "- [ ] () — NAME — WITH DASH\n",
            "- [ ] ()\n",
        ));
        assert_eq!(entries[0].slug, "q&a-(draft),-v2.0/ok-go");
        assert!(entries[0].selectable);
        assert_eq!(entries[1].slug, "snake_case");
        assert!(!entries[1].selectable);
        assert_eq!(entries[2].slug, "name-—-with-dash");
        assert!(!entries[2].selectable);
        assert_eq!(entries[3].slug, "");
        assert!(!entries[3].selectable);
    }

    #[test]
    fn plus_names_are_selectable_and_prefix_matched() {
        let existing = scan(concat!(
            "## Pomodoros\n",
            "- [ ] () — C++\n",
            "- [ ] () — BOB+SASE\n",
        ));
        assert_eq!(existing.entries[0].slug, "c++");
        assert!(existing.entries[0].selectable);
        assert_eq!(existing.entries[1].slug, "bob+sase");
        assert!(existing.entries[1].selectable);
        assert!(matches!(
            select_named(&existing, "c++"),
            NamedSelection::Found(entry) if entry.name.as_deref() == Some("C++")
        ));
        assert!(matches!(
            select_named(&existing, "c+"),
            NamedSelection::Found(entry) if entry.name.as_deref() == Some("C++")
        ));
        assert!(matches!(
            select_named(&existing, "bob+sase"),
            NamedSelection::Found(entry)
                if entry.name.as_deref() == Some("BOB+SASE")
        ));
        assert_eq!(named_creation_name(&existing, "c++"), None);

        let novel = scan("## Pomodoros\n- [ ] () — MEMORY\n");
        assert_eq!(
            named_creation_name(&novel, "c++").as_deref(),
            Some("C++")
        );
        assert_eq!(
            named_creation_name(&novel, "bob+sase").as_deref(),
            Some("BOB+SASE")
        );
    }

    #[test]
    fn selection_uses_whole_slug_before_earlier_prefix() {
        let scan = scan(concat!(
            "## Pomodoros\n",
            "- [ ] () — MEMORY WORK\n",
            "- [ ] () — MEMORY\n",
        ));
        assert!(matches!(
            select_named(&scan, "memory"),
            NamedSelection::Found(entry) if entry.name.as_deref() == Some("MEMORY")
        ));
        assert!(matches!(
            select_named(&scan, "mem"),
            NamedSelection::Found(entry) if entry.name.as_deref() == Some("MEMORY WORK")
        ));
    }

    #[test]
    fn selection_reports_completed_only_and_unique_suggestion() {
        let scan = scan(concat!(
            "## Pomodoros\n",
            "- [x] () — BUGS\n",
            "- [ ] () — MEMORY\n",
            "- [ ] () — FOCUS\n",
        ));
        assert!(matches!(
            select_named(&scan, "bugs"),
            NamedSelection::CompletedOnly(entry) if entry.name.as_deref() == Some("BUGS")
        ));
        assert!(matches!(
            select_named(&scan, "memry"),
            NamedSelection::Missing { suggestion: Some(entry) }
                if entry.name.as_deref() == Some("MEMORY")
        ));
        assert!(matches!(
            select_named(&scan, "zzzz"),
            NamedSelection::Missing { suggestion: None }
        ));
        assert_eq!(named_creation_name(&scan, "bugs").as_deref(), Some("BUGS"));
        assert_eq!(named_creation_name(&scan, "zzzz").as_deref(), Some("ZZZZ"));
        assert_eq!(named_creation_name(&scan, "mem"), None);
        assert_eq!(named_creation_name(&scan, ""), None);
        assert_eq!(named_creation_name(&scan, "bad_id"), None);
    }

    #[test]
    fn refs_resolve_exact_shifted_stale_and_ambiguous() {
        let original = scan(concat!(
            "## Pomodoros\n",
            "- [ ] () — SAME\n",
            "- [ ] () — UNIQUE\n",
        ));
        let same_ref = original.entries[0].pomodoro_ref.clone();
        let unique_ref = original.entries[1].pomodoro_ref.clone();
        assert!(matches!(
            original.by_ref(&same_ref),
            PomodoroRefLookup::Found(_)
        ));

        let shifted = scan(concat!(
            "## Pomodoros\n",
            "Intro\n",
            "- [ ] () — SAME\n",
            "- [ ] () — UNIQUE\n",
        ));
        assert!(
            matches!(shifted.by_ref(&unique_ref), PomodoroRefLookup::Found(entry) if entry.line == 4)
        );
        assert_eq!(
            shifted.by_ref(&PomodoroRef {
                line: 2,
                digest: "deadbeef".to_string(),
            }),
            PomodoroRefLookup::Stale
        );

        let ambiguous = scan(concat!(
            "## Pomodoros\n",
            "- [ ] () — SAME\n",
            "- [ ] () — SAME\n",
        ));
        assert_eq!(
            ambiguous.by_ref(&PomodoroRef {
                line: 99,
                digest: same_ref.digest,
            }),
            PomodoroRefLookup::Ambiguous
        );
    }

    #[test]
    fn json_success_shape_is_stable() {
        let result = CapturePomodorosResult {
            ok: true,
            schema_version: 1,
            day_file: "/tmp/bob/2026/20260828.md".to_string(),
            relative_day_file: "2026/20260828.md".to_string(),
            count: 1,
            pomodoros: vec![PomodoroEntry {
                pomodoro_ref: PomodoroRef {
                    line: 31,
                    digest: "1a2b3c4d".to_string(),
                },
                line: 31,
                state: PomodoroState::Open,
                status_symbol: ' ',
                name: Some("MEMORY".to_string()),
                slug: "memory".to_string(),
                selectable: true,
                time_range: Some("1205-1230".to_string()),
                placeholder: false,
                is_current: true,
                child_count: 5,
            }],
            warnings: Vec::new(),
        };

        let value: serde_json::Value =
            serde_json::from_str(&success_json(&result)).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["day_file"], "/tmp/bob/2026/20260828.md");
        assert_eq!(value["relative_day_file"], "2026/20260828.md");
        assert_eq!(value["count"], 1);
        assert_eq!(value["pomodoros"][0]["ref"], "31:1a2b3c4d");
        assert_eq!(value["pomodoros"][0]["line"], 31);
        assert_eq!(value["pomodoros"][0]["state"], "open");
        assert_eq!(value["pomodoros"][0]["status_symbol"], " ");
        assert_eq!(value["pomodoros"][0]["name"], "MEMORY");
        assert_eq!(value["pomodoros"][0]["slug"], "memory");
        assert_eq!(value["pomodoros"][0]["selectable"], true);
        assert_eq!(value["pomodoros"][0]["time_range"], "1205-1230");
        assert_eq!(value["pomodoros"][0]["placeholder"], false);
        assert_eq!(value["pomodoros"][0]["is_current"], true);
        assert_eq!(value["pomodoros"][0]["child_count"], 5);
        assert_eq!(value["warnings"].as_array().expect("warnings").len(), 0);
    }

    #[test]
    fn missing_note_and_missing_section_are_warning_successes() {
        let temp = TempDir::new("bob-cli-capture-pomodoros-missing");
        let missing = list_capture_pomodoros(&CapturePomodorosRequest {
            bob_dir: temp.path().to_path_buf(),
            include_all: false,
        })
        .expect("missing note success");
        assert_eq!(missing.count, 0);
        assert!(missing.warnings[0].contains("does not exist"));

        write_file(&temp.path().join("2026/20260828.md"), "# Day\n");
        let sectionless = with_env(
            "BOB_DAY_FILE",
            temp.path().join("2026/20260828.md"),
            || {
                list_capture_pomodoros(&CapturePomodorosRequest {
                    bob_dir: temp.path().to_path_buf(),
                    include_all: false,
                })
            },
        )
        .expect("missing section success");
        assert_eq!(sectionless.count, 0);
        assert!(sectionless.warnings[0].contains("no Pomodoros section"));
    }

    #[test]
    fn human_output_is_plain_and_lists_badges() {
        let result = CapturePomodorosResult {
            ok: true,
            schema_version: 1,
            day_file: "/tmp/bob/2026/20260828.md".to_string(),
            relative_day_file: "2026/20260828.md".to_string(),
            count: 2,
            pomodoros: vec![
                PomodoroEntry {
                    pomodoro_ref: PomodoroRef {
                        line: 2,
                        digest: "11111111".to_string(),
                    },
                    line: 2,
                    state: PomodoroState::Open,
                    status_symbol: ' ',
                    name: Some("MEMORY".to_string()),
                    slug: "memory".to_string(),
                    selectable: true,
                    time_range: Some("0900-0930".to_string()),
                    placeholder: false,
                    is_current: true,
                    child_count: 1,
                },
                PomodoroEntry {
                    pomodoro_ref: PomodoroRef {
                        line: 3,
                        digest: "22222222".to_string(),
                    },
                    line: 3,
                    state: PomodoroState::Completed,
                    status_symbol: 'x',
                    name: None,
                    slug: String::new(),
                    selectable: false,
                    time_range: None,
                    placeholder: true,
                    is_current: false,
                    child_count: 0,
                },
            ],
            warnings: vec!["watch out".to_string()],
        };
        let human = human_success(&result, &Styler::plain());
        assert!(human.contains("warning - watch out"));
        assert!(human.contains("MEMORY"));
        assert!(human.contains("memory"));
        assert!(human.contains("0900-0930"));
        assert!(human.contains("current 1 link"));
        assert!(human.contains("unnamed"));
        assert!(human.contains("planned"));
        assert!(human.contains("completed empty"));
        assert!(!human.contains('\u{1b}'));
    }

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("file parent"))
            .expect("create file parent");
        fs::write(path, contents).expect("write file");
    }

    fn with_env<T>(
        key: &str,
        value: impl Into<OsString>,
        f: impl FnOnce() -> T,
    ) -> T {
        let old = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value.into());
        }
        let result = f();
        unsafe {
            match old {
                Some(old) => std::env::set_var(key, old),
                None => std::env::remove_var(key),
            }
        }
        result
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "{prefix}-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                eprintln!("failed to remove {}: {error}", self.path.display());
            }
        }
    }
}
