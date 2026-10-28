//! Previewed, verified, single-commit per-entry theme split.
//!
//! `bob themes split OLD` rewrites selected ` — NAME` tails of canonical
//! daily notes from a TSV map (or prints that map with `-t`). Unlike
//! rename, it writes no journal event.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    rc::Rc,
};

use chrono::NaiveDate;
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command as ClapCommand};
use serde::Serialize;
use serde_json::json;

use crate::native::{
    capture_pomodoros::{self, PomodoroRef, PomodoroRefLookup, PomodoroState},
    env as bob_env,
    markdown::split_line_ending,
    ob::{self},
    plan_budget::{normalize_component, split_components},
};

use super::{
    cache,
    rename::{
        bob_dir_arg, format_arg, help_arg, offline_arg, print_clap_error,
        print_failure, raw_lines_preserving, replace_name_tail, rewrite_name,
        verify_raw_tail_splice, OutputFormat, RenameFailure,
    },
    scan, Catalog,
};

const COMMAND_NAME: &str = "bob themes split";
const SCHEMA_VERSION: u32 = 1;

type SplitFailure = RenameFailure;

pub(crate) fn build_cli_split() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Split one Pomodoro theme across selected entries in one commit")
        .long_about(
            "Rewrite selected Pomodoro theme components in ` — NAME` tails of \
canonical daily notes (past, today, and future, open and closed) from a \
tab-separated map, in a single verified Git commit.\n\n\
`-t/--template` prints a TSV map of every entry whose name contains OLD; \
it never takes the lock or writes. `_templates/daily.md` is out of scope. \
Fill the THEME column (an untouched row keeps OLD), then apply with \
`-m/--map`. `-m -` reads the map from stdin.\n\n\
Only the OLD component in mapped tails changes: separators keep their `+` \
spelling, a resulting duplicate collapses (`JOB + BUILD` with THEME BUILD \
becomes `BUILD`), and every other byte is preserved. Unmapped OLD entries \
are left alone. Split writes no journal event: OLD keeps its hidden state \
and pin, and every target keeps its own.\n\n\
The preview and the template always rescan from source. A write takes \
bob_sync.lock, pre-syncs unless --offline, snapshots each target and \
compares again right before writing, writes atomically, verifies by \
rescanning, makes one commit, then post-syncs. Undo is `git revert`.\n\n\
Online writes require -b/--bob-dir and BOB_DIR (default ~/bob) to resolve \
to the same vault; a mismatch refuses before locks or sync. Set BOB_DIR \
to the selected vault or use --offline. Previews, templates, and offline \
writes do not require this match.",
        )
        .after_help(
            "Examples:\n  \
bob themes split JOB -t > job.tsv\n  \
bob themes split JOB -m job.tsv -d\n  \
bob themes split JOB -m job.tsv -d -f json\n  \
bob themes split JOB -m - -o\n\n\
Map format:\n  \
# comment lines and blank lines are skipped\n  \
DATE<TAB>REF<TAB>THEME[<TAB>NOTE]\n  \
DATE is YYYY-MM-DD; REF is LINE:DIGEST; THEME is one + component; NOTE is ignored.\n\n\
Environment:\n  \
BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
BOB_NOW                   Local datetime override that pins today\n  \
HOSTNAME                  Host label vault-sync uses (split does not write a journal)\n  \
BOB_VAULT_SYNC_LOCK_FILE  Maintenance lock; defaults to ${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock\n\n\
Exit codes:\n  \
0  split (or dry-run planned, or template printed)\n  \
1  unknown theme, invalid name, unchanged, stale entry, lock busy, dirty paths, not a worktree, sync failed, verify failed, stale note, or I/O failure\n  \
2  usage error or invalid map\n\n\
See docs/pomodoro-themes.md for the split contract, map format, refusal table, and revert runbook.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(map_arg())
        .arg(offline_arg())
        .arg(template_arg())
        .arg(old_arg())
        .group(
            ArgGroup::new("source")
                .args(["map", "template"])
                .required(true),
        )
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Preview the split without writing, committing, or syncing")
}

fn map_arg() -> Arg {
    Arg::new("map")
        .long("map")
        .short('m')
        .value_name("FILE")
        .help("Tab-separated map of DATE, REF, THEME; - reads stdin")
}

fn template_arg() -> Arg {
    Arg::new("template")
        .long("template")
        .short('t')
        .action(ArgAction::SetTrue)
        .help("Print a TSV map of OLD entries without writing")
}

fn old_arg() -> Arg {
    Arg::new("old")
        .value_name("OLD")
        .required(true)
        .help("Theme name to split, as it appears in a Pomodoro tail")
}

pub(crate) fn run_split(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_split();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(COMMAND_NAME, error),
    };
    let format = OutputFormat::from_matches(&matches);
    if matches.get_flag("template") {
        return run_template(&matches, format);
    }
    let dry_run = matches.get_flag("dry-run");
    if dry_run {
        return match prepare_from_matches(&matches, true) {
            Ok(prepared) => {
                print_split_success(&prepared.report("preview"), format);
                0
            }
            Err(failure) => print_failure(COMMAND_NAME, format, failure),
        };
    }
    run_live(&matches, format)
}

fn run_template(matches: &ArgMatches, format: OutputFormat) -> i32 {
    match collect_template(matches) {
        Ok(report) => {
            print_template_success(&report, format);
            0
        }
        Err(failure) => print_failure(COMMAND_NAME, format, failure),
    }
}

fn vault_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

fn old_from_matches(matches: &ArgMatches) -> String {
    matches
        .get_one::<String>("old")
        .map(String::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn validate_old(old_raw: &str) -> Result<(String, String), SplitFailure> {
    if old_raw.is_empty() {
        return Err(SplitFailure::new(
            "usage",
            "OLD is required",
            Some("usage: bob themes split OLD (-m/--map FILE | -t/--template)"),
        ));
    }
    if old_raw.contains('+') || split_components(old_raw).len() != 1 {
        return Err(SplitFailure::new(
            "invalid_name",
            format!("invalid theme name: {old_raw}"),
            Some("split one + component at a time"),
        ));
    }
    let old_key = normalize_component(old_raw);
    if old_key.is_empty() {
        return Err(SplitFailure::new(
            "invalid_name",
            format!("invalid theme name: {old_raw}"),
            Some("choose a non-blank theme name"),
        ));
    }
    Ok((old_key, old_raw.to_string()))
}

fn collect_template(
    matches: &ArgMatches,
) -> Result<TemplateReport, SplitFailure> {
    let vault = vault_from_matches(matches);
    let old_raw = old_from_matches(matches);
    let (old_key, _) = validate_old(&old_raw)?;
    let entries = collect_old_entries(&vault, &old_key)?;
    if entries.is_empty() {
        return Err(unknown_theme(&old_raw));
    }
    let old_name = display_name(&vault, &old_key, &old_raw);
    Ok(TemplateReport {
        schema_version: SCHEMA_VERSION,
        ok: true,
        mode: "template".to_string(),
        theme: ThemeSide {
            key: old_key,
            name: old_name,
        },
        entries: entries.iter().map(TemplateEntryView::from).collect(),
    })
}

fn unknown_theme(old_raw: &str) -> SplitFailure {
    SplitFailure::new(
        "unknown_theme",
        format!("unknown theme: {old_raw}"),
        Some("list known themes with bob themes list -a"),
    )
}

fn display_name(vault: &Path, old_key: &str, old_raw: &str) -> String {
    let today = bob_env::current_datetime().date();
    Catalog::load(vault, today, true)
        .ok()
        .and_then(|catalog| {
            catalog.get(old_key).map(|record| record.name.clone())
        })
        .unwrap_or_else(|| old_raw.trim().to_string())
}

#[derive(Debug, Clone, Serialize)]
struct ThemeSide {
    key: String,
    name: String,
}

#[derive(Debug, Clone, Serialize)]
struct TemplateReport {
    schema_version: u32,
    ok: bool,
    mode: String,
    theme: ThemeSide,
    entries: Vec<TemplateEntryView>,
}

#[derive(Debug, Clone, Serialize)]
struct TemplateEntryView {
    date: String,
    path: String,
    #[serde(rename = "ref")]
    pomodoro_ref: String,
    line: usize,
    state: PomodoroState,
    time_range: Option<String>,
    name: String,
    links: Vec<String>,
    notes: Vec<String>,
}

impl From<&OldEntry> for TemplateEntryView {
    fn from(entry: &OldEntry) -> Self {
        Self {
            date: entry.date.to_string(),
            path: entry.path.clone(),
            pomodoro_ref: entry.pomodoro_ref.to_string(),
            line: entry.line,
            state: entry.state,
            time_range: entry.time_range.clone(),
            name: entry.name.clone(),
            links: entry.links.clone(),
            notes: entry.notes.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct OldEntry {
    date: NaiveDate,
    path: String,
    pomodoro_ref: PomodoroRef,
    line: usize,
    state: PomodoroState,
    time_range: Option<String>,
    name: String,
    links: Vec<String>,
    notes: Vec<String>,
}

fn collect_old_entries(
    vault: &Path,
    old_key: &str,
) -> Result<Vec<OldEntry>, SplitFailure> {
    let mut entries = Vec::new();
    for (date, relative, absolute) in scan::list_daily_notes(vault) {
        let contents = fs::read_to_string(&absolute).map_err(|error| {
            SplitFailure::new(
                "io",
                format!("could not read {relative}: {error}"),
                None::<String>,
            )
        })?;
        let scan = capture_pomodoros::scan(&contents);
        for entry in &scan.entries {
            let Some(name) = entry.name.as_deref() else {
                continue;
            };
            if !name_has_key(name, old_key) {
                continue;
            }
            let (links, notes) = child_links_and_notes(&contents, entry.line);
            entries.push(OldEntry {
                date,
                path: relative.clone(),
                pomodoro_ref: entry.pomodoro_ref.clone(),
                line: entry.line,
                state: entry.state,
                time_range: entry.time_range.clone(),
                name: name.to_string(),
                links,
                notes,
            });
        }
    }
    entries.sort_by(|left, right| {
        left.date.cmp(&right.date).then(left.line.cmp(&right.line))
    });
    Ok(entries)
}

fn name_has_key(name: &str, key: &str) -> bool {
    split_components(name)
        .iter()
        .any(|component| normalize_component(component) == key)
}

fn child_links_and_notes(
    contents: &str,
    entry_line: usize,
) -> (Vec<String>, Vec<String>) {
    let mut links = Vec::new();
    let mut notes = Vec::new();
    for line in contents.lines().skip(entry_line) {
        if !line.trim().is_empty() && leading_indent(line) == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(body) = list_item_body(trimmed) else {
            continue;
        };
        let found = wiki_block_links(body);
        if found.is_empty() {
            let truncated: String = body.chars().take(60).collect();
            if !truncated.is_empty() {
                notes.push(truncated);
            }
        } else {
            links.extend(found);
        }
    }
    (links, notes)
}

fn leading_indent(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

fn list_item_body(trimmed: &str) -> Option<&str> {
    let rest = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))?;
    Some(rest)
}

fn wiki_block_links(line: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find("[[") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("]]") else {
            break;
        };
        let mut inner = &after[..close];
        inner = inner.strip_suffix('#').unwrap_or(inner);
        inner = inner.split('|').next().unwrap_or(inner);
        if let Some((target, block_id)) = inner.split_once("#^") {
            let target = target.strip_suffix(".md").unwrap_or(target).trim();
            let block_id = block_id.trim();
            if !block_id.is_empty() {
                links.push(format!("{target}#^{block_id}"));
            }
        }
        rest = &after[close + 2..];
    }
    links
}

fn tsv_cell(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

fn print_template_success(report: &TemplateReport, format: OutputFormat) {
    match format {
        OutputFormat::Json => match serde_json::to_string(report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("{COMMAND_NAME}: failed to serialize JSON: {error}");
            }
        },
        OutputFormat::Human => print_template_tsv(report),
    }
}

fn print_template_tsv(report: &TemplateReport) {
    println!("# DATE\tREF\tTHEME\tNOTE");
    println!(
        "# Fill THEME with the destination name. Leave THEME as {} to keep the entry. NOTE is ignored on apply.",
        report.theme.name
    );
    for entry in &report.entries {
        let note = note_column_from_view(entry);
        println!(
            "{}\t{}\t{}\t{note}",
            entry.date, entry.pomodoro_ref, report.theme.name
        );
    }
}

fn note_column_from_view(entry: &TemplateEntryView) -> String {
    let checkbox = match entry.state {
        PomodoroState::Completed => "[x]",
        PomodoroState::Open => "[ ]",
    };
    let time = entry.time_range.as_deref().unwrap_or("()");
    let mut parts =
        vec![checkbox.to_string(), time.to_string(), entry.name.clone()];
    parts.extend(entry.links.iter().cloned());
    parts.extend(entry.notes.iter().cloned());
    tsv_cell(&parts.join(", "))
}

#[derive(Debug, Clone, Serialize)]
struct SplitReport {
    schema_version: u32,
    ok: bool,
    mode: String,
    theme: ThemeSide,
    targets: Vec<TargetSummary>,
    kept: usize,
    unmapped: usize,
    remaining: usize,
    files: Vec<FileView>,
    commit: Option<serde_json::Value>,
    sync: SyncInfo,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TargetSummary {
    key: String,
    name: String,
    entries: usize,
    new: bool,
    exempt: bool,
}

#[derive(Debug, Clone, Serialize)]
struct FileView {
    path: String,
    changes: Vec<ChangeView>,
}

#[derive(Debug, Clone, Serialize)]
struct ChangeView {
    line: usize,
    #[serde(rename = "ref")]
    pomodoro_ref: String,
    before: String,
    after: String,
    #[serde(skip)]
    to_name: String,
    #[serde(skip)]
    time_range: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SyncInfo {
    pre: String,
    post: String,
}

#[derive(Debug, Clone)]
struct PlannedChange {
    line: usize,
    pomodoro_ref: PomodoroRef,
    before: String,
    after: String,
    to_name: String,
    time_range: Option<String>,
}

#[derive(Debug, Clone)]
struct FilePlan {
    path: String,
    changes: Vec<PlannedChange>,
    before_contents: String,
    after_contents: String,
}

struct PreparedSplit {
    old_key: String,
    old_name: String,
    files: Vec<FilePlan>,
    targets: Vec<TargetSummary>,
    kept: usize,
    unmapped: usize,
    remaining: usize,
    warnings: Vec<String>,
    sync_pre: &'static str,
}

impl PreparedSplit {
    fn report(&self, mode: &str) -> SplitReport {
        SplitReport {
            schema_version: SCHEMA_VERSION,
            ok: true,
            mode: mode.to_string(),
            theme: ThemeSide {
                key: self.old_key.clone(),
                name: self.old_name.clone(),
            },
            targets: self.targets.clone(),
            kept: self.kept,
            unmapped: self.unmapped,
            remaining: self.remaining,
            files: self
                .files
                .iter()
                .map(|file| FileView {
                    path: file.path.clone(),
                    changes: file
                        .changes
                        .iter()
                        .map(|change| ChangeView {
                            line: change.line,
                            pomodoro_ref: change.pomodoro_ref.to_string(),
                            before: change.before.clone(),
                            after: change.after.clone(),
                            to_name: change.to_name.clone(),
                            time_range: change.time_range.clone(),
                        })
                        .collect(),
                })
                .collect(),
            commit: None,
            sync: SyncInfo {
                pre: self.sync_pre.to_string(),
                post: "skipped".to_string(),
            },
            warnings: self.warnings.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct MapRow {
    line_number: usize,
    date: NaiveDate,
    pomodoro_ref: PomodoroRef,
    theme: String,
}

fn parse_map_rows(text: &str) -> Result<Vec<MapRow>, SplitFailure> {
    let mut rows = Vec::new();
    let mut seen: BTreeSet<(NaiveDate, String)> = BTreeSet::new();
    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let columns: Vec<&str> = line.split('\t').collect();
        if columns.len() < 3 {
            return Err(SplitFailure::new(
                "invalid_map",
                format!("map line {line_number}: expected DATE, REF, THEME"),
                Some("regenerate with -t"),
            ));
        }
        let date = NaiveDate::parse_from_str(columns[0], "%Y-%m-%d").map_err(
            |_| {
                SplitFailure::new(
                    "invalid_map",
                    format!(
                        "map line {line_number}: invalid date '{}'",
                        columns[0]
                    ),
                    Some("use YYYY-MM-DD"),
                )
            },
        )?;
        let Some(pomodoro_ref) = PomodoroRef::parse(columns[1]) else {
            return Err(SplitFailure::new(
                "invalid_map",
                format!("map line {line_number}: invalid ref '{}'", columns[1]),
                Some("REF is LINE:DIGEST from -t"),
            ));
        };
        let theme = columns[2].trim();
        if theme.is_empty() {
            return Err(SplitFailure::new(
                "invalid_name",
                format!("map line {line_number}: invalid theme name: (blank)"),
                Some("choose a single + component without +"),
            ));
        }
        if theme.contains('+') || split_components(theme).len() != 1 {
            return Err(SplitFailure::new(
                "invalid_name",
                format!(
                    "map line {line_number}: invalid theme name: {theme} (a theme never contains +)"
                ),
                Some("choose a single + component without +"),
            ));
        }
        let theme_key = normalize_component(theme);
        if theme_key.is_empty() {
            return Err(SplitFailure::new(
                "invalid_name",
                format!("map line {line_number}: invalid theme name: {theme}"),
                Some("choose a non-blank theme name"),
            ));
        }
        let identity = (date, pomodoro_ref.to_string());
        if !seen.insert(identity) {
            return Err(SplitFailure::new(
                "invalid_map",
                format!(
                    "map line {line_number}: duplicate entry {} {}",
                    date, pomodoro_ref
                ),
                Some("each DATE+REF may appear once"),
            ));
        }
        rows.push(MapRow {
            line_number,
            date,
            pomodoro_ref,
            theme: theme.to_string(),
        });
    }
    Ok(rows)
}

fn read_map(path: &OsString) -> Result<String, SplitFailure> {
    if path == "-" {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf).map_err(|error| {
            SplitFailure::new(
                "io",
                format!("could not read map from stdin: {error}"),
                None::<String>,
            )
        })?;
        return Ok(buf);
    }
    fs::read_to_string(Path::new(path)).map_err(|error| {
        SplitFailure::new(
            "io",
            format!("could not read {}: {error}", path.to_string_lossy()),
            None::<String>,
        )
    })
}

fn prepare_from_matches(
    matches: &ArgMatches,
    dry_run: bool,
) -> Result<PreparedSplit, SplitFailure> {
    let vault = vault_from_matches(matches);
    let old_raw = old_from_matches(matches);
    let (old_key, _) = validate_old(&old_raw)?;
    let map_path = matches
        .get_one::<String>("map")
        .map(OsString::from)
        .or_else(|| matches.get_one::<OsString>("map").cloned())
        .unwrap_or_else(|| OsString::from("-"));
    let text = read_map(&map_path)?;
    let offline = matches.get_flag("offline");
    plan_split(&vault, &old_raw, &old_key, &text, dry_run, offline)
}

fn plan_split(
    vault: &Path,
    old_raw: &str,
    old_key: &str,
    map_text: &str,
    dry_run: bool,
    offline: bool,
) -> Result<PreparedSplit, SplitFailure> {
    let old_entries = collect_old_entries(vault, old_key)?;
    if old_entries.is_empty() {
        return Err(unknown_theme(old_raw));
    }
    let old_name = display_name(vault, old_key, old_raw);
    let rows = parse_map_rows(map_text)?;
    let today = bob_env::current_datetime().date();
    let catalog = Catalog::load(vault, today, true).map_err(|error| {
        SplitFailure::new(
            "io",
            format!("read theme catalog in {}: {error}", vault.display()),
            None::<String>,
        )
    })?;

    let mut by_path: BTreeMap<String, String> = BTreeMap::new();
    let mut mapped: BTreeSet<(String, usize)> = BTreeSet::new();
    let mut kept = 0usize;
    let mut target_counts: BTreeMap<String, (String, usize)> = BTreeMap::new();
    let mut file_changes: BTreeMap<String, Vec<PlannedChange>> =
        BTreeMap::new();

    for row in &rows {
        let relative = scan::canonical_relative(row.date);
        let absolute = vault.join(&relative);
        if !absolute.is_file() {
            return Err(SplitFailure::new(
                "invalid_map",
                format!(
                    "map line {}: no daily note for {}",
                    row.line_number, row.date
                ),
                Some("regenerate with -t"),
            ));
        }
        let contents = match by_path.get(&relative) {
            Some(existing) => existing.clone(),
            None => {
                let read = fs::read_to_string(&absolute).map_err(|error| {
                    SplitFailure::new(
                        "io",
                        format!("could not read {relative}: {error}"),
                        None::<String>,
                    )
                })?;
                by_path.insert(relative.clone(), read.clone());
                read
            }
        };
        let scan = capture_pomodoros::scan(&contents);
        let lookup = scan.by_ref(&row.pomodoro_ref);
        let entry = match lookup {
            PomodoroRefLookup::Found(entry) => entry,
            PomodoroRefLookup::Stale | PomodoroRefLookup::Ambiguous => {
                return Err(stale_entry(row.date, &row.pomodoro_ref));
            }
        };
        let Some(name) = entry.name.as_deref() else {
            return Err(stale_entry(row.date, &row.pomodoro_ref));
        };
        if !name_has_key(name, old_key) {
            return Err(stale_entry(row.date, &row.pomodoro_ref));
        }
        let identity = (relative.clone(), entry.line);
        if !mapped.insert(identity) {
            return Err(SplitFailure::new(
                "invalid_map",
                format!(
                    "map line {}: duplicate entry {} {}",
                    row.line_number, row.date, row.pomodoro_ref
                ),
                Some("each DATE+REF may appear once"),
            ));
        }
        let theme_key = normalize_component(&row.theme);
        if theme_key == old_key {
            kept += 1;
            continue;
        }
        let Some(new_name) = rewrite_name(name, old_key, &row.theme) else {
            kept += 1;
            continue;
        };
        let slot = target_counts
            .entry(theme_key)
            .or_insert((row.theme.clone(), 0));
        slot.0 = row.theme.clone();
        slot.1 = slot.1.saturating_add(1);
        file_changes
            .entry(relative)
            .or_default()
            .push(PlannedChange {
                line: entry.line,
                pomodoro_ref: entry.pomodoro_ref.clone(),
                before: name.to_string(),
                after: new_name,
                to_name: row.theme.clone(),
                time_range: entry.time_range.clone(),
            });
    }

    if file_changes.is_empty() {
        return Err(SplitFailure::new(
            "unchanged",
            format!("every mapped entry keeps {old_name}"),
            Some("edit THEME in the map, then re-run"),
        ));
    }

    let unmapped = old_entries
        .iter()
        .filter(|entry| !mapped.contains(&(entry.path.clone(), entry.line)))
        .count();
    let remaining = kept + unmapped;

    let mut files = Vec::new();
    for (relative, mut changes) in file_changes {
        changes.sort_by_key(|change| change.line);
        let before_contents =
            by_path.get(&relative).cloned().ok_or_else(|| {
                SplitFailure::new(
                    "io",
                    format!("missing planned contents for {relative}"),
                    None::<String>,
                )
            })?;
        let after_contents = apply_changes(&before_contents, &changes)?;
        files.push(FilePlan {
            path: relative,
            changes,
            before_contents,
            after_contents,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let mut targets: Vec<TargetSummary> = target_counts
        .into_iter()
        .map(|(key, (name, entries))| TargetSummary {
            new: catalog.get(&key).is_none(),
            exempt: catalog.is_exempt(&key),
            key,
            name,
            entries,
        })
        .collect();
    targets.sort_by(|left, right| left.name.cmp(&right.name));

    if !dry_run {
        let touched: BTreeSet<PathBuf> =
            files.iter().map(|file| PathBuf::from(&file.path)).collect();
        let dirty =
            crate::native::vault_rewrite::dirty_touched_paths(vault, &touched)
                .map_err(|failure| {
                    SplitFailure::new(
                        failure.code,
                        failure.message,
                        failure.hint,
                    )
                })?;
        if offline && !dirty.is_empty() {
            return Err(SplitFailure::new(
                "dirty_paths",
                format!("split paths are dirty: {}", dirty.join(", ")),
                Some("commit or restore those paths before an offline split"),
            ));
        }
    }

    Ok(PreparedSplit {
        old_key: old_key.to_string(),
        old_name,
        files,
        targets,
        kept,
        unmapped,
        remaining,
        warnings: Vec::new(),
        sync_pre: "skipped",
    })
}

fn stale_entry(date: NaiveDate, pomodoro_ref: &PomodoroRef) -> SplitFailure {
    SplitFailure::new(
        "stale_entry",
        format!("stale entry {date} {pomodoro_ref}"),
        Some("regenerate with -t"),
    )
}

fn apply_changes(
    contents: &str,
    changes: &[PlannedChange],
) -> Result<String, SplitFailure> {
    let mut owned = raw_lines_preserving(contents);
    // An empty file has no lines; any planned change there is stale.
    // A file that ends with a newline has exactly lines().count()
    // raw lines, so 1-based entry lines index directly.
    for change in changes {
        let index = change.line.saturating_sub(1);
        let Some(raw) = owned.get(index).cloned() else {
            return Err(SplitFailure::new(
                "stale_entry",
                format!("stale entry line {}", change.line),
                Some("regenerate with -t"),
            ));
        };
        let (body, ending) = split_line_ending(&raw);
        let Some(rewritten) =
            replace_name_tail(body, &change.before, &change.after)
        else {
            return Err(SplitFailure::new(
                "stale_entry",
                format!("stale entry line {}", change.line),
                Some("regenerate with -t"),
            ));
        };
        owned[index] = format!("{rewritten}{ending}");
    }
    Ok(owned.concat())
}

fn run_live(matches: &ArgMatches, format: OutputFormat) -> i32 {
    let child_env = ob::child_env();
    let vault = vault_from_matches(matches);
    let offline = matches.get_flag("offline");
    let json_mode = format == OutputFormat::Json;

    let outcome = crate::native::vault_rewrite::run(
        &vault,
        &child_env,
        COMMAND_NAME,
        "theme splits",
        offline,
        json_mode,
        || {
            let prepared =
                prepare_from_matches(matches, false).map_err(|failure| {
                    crate::native::vault_rewrite::TxnFailure::new(
                        failure.code,
                        failure.message,
                        failure.hint,
                    )
                })?;
            let inner = Rc::new(prepared);
            let files = inner
                .files
                .iter()
                .map(|file| crate::native::vault_rewrite::FileRewrite {
                    path: file.path.clone(),
                    before_contents: file.before_contents.clone(),
                    after_contents: file.after_contents.clone(),
                })
                .collect::<Vec<_>>();
            let subject = commit_subject(&inner.old_name, &inner.targets);
            let verify_inner = Rc::clone(&inner);
            let verify_vault = vault.clone();
            let invalidate_vault = vault.clone();
            Ok(crate::native::vault_rewrite::ReadyPlan {
                files,
                subject,
                post_write: Box::new(|| Ok(Vec::new())),
                verify: Box::new(move || {
                    verify_split(&verify_vault, &verify_inner)
                }),
                on_committed: Box::new(move || {
                    cache::invalidate_vault(&invalidate_vault);
                }),
                context: Rc::clone(&inner),
            })
        },
    );
    match outcome {
        Ok(outcome) => {
            let mut report = outcome.context.report("write");
            report.sync.pre = outcome.sync_pre.to_string();
            report.commit =
                Some(json!({ "sha": outcome.sha, "subject": outcome.subject }));
            let mut warnings = outcome.context.warnings.clone();
            if let Some(dirty) = outcome.dirty_warning {
                warnings.push(dirty);
            }
            report.warnings = warnings;
            report.sync.post = outcome.sync_post.to_string();
            if let Some(post) = outcome.post_sync_warning {
                report.warnings.push(post);
            }
            print_split_success(&report, format);
            0
        }
        Err(failure) => print_failure(
            COMMAND_NAME,
            format,
            SplitFailure::new(failure.code, failure.message, failure.hint),
        ),
    }
}

fn commit_subject(old_name: &str, targets: &[TargetSummary]) -> String {
    let names = targets
        .iter()
        .map(|target| target.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!("bob themes split: {old_name} → {names}")
}

fn verify_split(vault: &Path, prepared: &PreparedSplit) -> Result<(), String> {
    for file in &prepared.files {
        let absolute = vault.join(&file.path);
        let contents = fs::read_to_string(&absolute)
            .map_err(|error| format!("reread {}: {error}", file.path))?;
        if contents != file.after_contents {
            return Err(format!("{} changed during the split", file.path));
        }
        let planned: BTreeSet<usize> =
            file.changes.iter().map(|change| change.line).collect();
        verify_raw_tail_splice(
            &file.path,
            &file.before_contents,
            &file.after_contents,
            &contents,
            &planned,
            "split",
        )?;
        let before_lines: Vec<&str> = file.before_contents.lines().collect();
        let after_lines: Vec<&str> = contents.lines().collect();
        if before_lines.len() != after_lines.len() {
            return Err(format!("{} line count changed", file.path));
        }
        let mut actual_changed = BTreeSet::new();
        for (index, (before, after)) in
            before_lines.iter().zip(after_lines.iter()).enumerate()
        {
            if before == after {
                continue;
            }
            actual_changed.insert(index + 1);
            if capture_pomodoros::without_name_tail(before)
                != capture_pomodoros::without_name_tail(after)
            {
                return Err(format!(
                    "{} changed outside a Pomodoro name tail",
                    file.path
                ));
            }
        }
        if actual_changed != planned {
            return Err(format!(
                "{} changed a different set of lines",
                file.path
            ));
        }
        let after_scan = capture_pomodoros::scan(&contents);
        let before_scan = capture_pomodoros::scan(&file.before_contents);
        if after_scan.entries.len() != before_scan.entries.len() {
            return Err(format!("{} entry count changed", file.path));
        }
        for change in &file.changes {
            let Some(entry) = after_scan
                .entries
                .iter()
                .find(|entry| entry.line == change.line)
            else {
                return Err(format!(
                    "{}:{} missing after split",
                    file.path, change.line
                ));
            };
            if entry.name.as_deref() != Some(change.after.as_str()) {
                return Err(format!(
                    "{}:{} does not carry {}",
                    file.path, change.line, change.after
                ));
            }
        }
    }
    Ok(())
}

fn print_split_success(report: &SplitReport, format: OutputFormat) {
    match format {
        OutputFormat::Json => match serde_json::to_string(report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("{COMMAND_NAME}: failed to serialize JSON: {error}");
            }
        },
        OutputFormat::Human => print_human(report),
    }
}

fn print_human(report: &SplitReport) {
    let styler = crate::native::style::Styler::detect();
    let dry = report.mode == "preview";
    let mode_suffix = if dry { " (dry run)" } else { "" };
    println!(
        "{} split {}{}",
        styler.success_prefix(dry),
        report.theme.name,
        mode_suffix,
    );
    for target in &report.targets {
        let mut flags = Vec::new();
        if target.new {
            flags.push("new");
        }
        if target.exempt {
            flags.push("exempt");
        }
        if flags.is_empty() {
            println!("  {}: {}", target.name, target.entries);
        } else {
            println!(
                "  {}: {}  {}",
                target.name,
                target.entries,
                flags.join(" ")
            );
        }
    }
    println!("  kept {} · unmapped {}", report.kept, report.unmapped);
    if report.targets.iter().any(|target| target.exempt) {
        println!("  exempt: these sessions count as review time in Stats");
    }
    for file in &report.files {
        for change in &file.changes {
            let time = change.time_range.as_deref().unwrap_or("()");
            println!(
                "  {}:{}  {} → {}  {time}",
                file.path, change.line, report.theme.name, change.to_name,
            );
        }
    }
    if report.mode == "write" && report.remaining == 0 {
        println!(
            "  {} no longer appears in any daily note",
            report.theme.name
        );
    }
    if let Some(commit) = &report.commit {
        let sha = commit
            .get("sha")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("(unknown)");
        let subject = commit
            .get("subject")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(COMMAND_NAME);
        println!("  committed {sha} {subject}");
    }
    println!("  sync: pre {}, post {}", report.sync.pre, report.sync.post);
    for warning in &report.warnings {
        eprintln!("{}: {warning}", styler.warning_prefix());
    }
}

#[cfg(test)]
mod split_tests {
    use super::*;

    #[test]
    fn parse_map_rows_skips_comments_blanks_and_note_column() {
        let text = "# header\n\n2026-10-13\t36:abcd1234\tCHORE\tignored note\n";
        let rows = parse_map_rows(text).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].date,
            NaiveDate::from_ymd_opt(2026, 10, 13).unwrap()
        );
        assert_eq!(rows[0].theme, "CHORE");
        assert_eq!(rows[0].pomodoro_ref.to_string(), "36:abcd1234");
    }

    #[test]
    fn parse_map_rows_tolerates_crlf() {
        let text = "2026-10-13\t36:abcd1234\tWRITE\r\n2026-10-14\t12:deadbeef\tREAD\r\n";
        let rows = parse_map_rows(text).expect("parse");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].theme, "WRITE");
        assert_eq!(rows[1].theme, "READ");
    }

    #[test]
    fn parse_map_rows_refuses_short_row() {
        let error = parse_map_rows("2026-10-13\t36:abcd1234\n").unwrap_err();
        assert_eq!(error.code, "invalid_map");
        assert!(error.message.contains("line 1"), "{}", error.message);
    }

    #[test]
    fn parse_map_rows_refuses_blank_or_plus_theme() {
        let blank = parse_map_rows("2026-10-13\t36:abcd1234\t\n").unwrap_err();
        assert_eq!(blank.code, "invalid_name");
        let plus =
            parse_map_rows("2026-10-13\t36:abcd1234\tA+B\n").unwrap_err();
        assert_eq!(plus.code, "invalid_name");
    }

    fn test_change(line: usize, before: &str, after: &str) -> PlannedChange {
        PlannedChange {
            line,
            pomodoro_ref: PomodoroRef::parse("2:abcd1234").expect("test ref"),
            before: before.to_string(),
            after: after.to_string(),
            to_name: after.to_string(),
            time_range: None,
        }
    }

    #[test]
    fn apply_changes_preserves_crlf_terminators() {
        let before =
            "## Pomodoros\r\n- [ ] () — JOB\r\n\t- keep this child\r\n";
        let changes = vec![test_change(2, "JOB", "WRITE")];
        let after = apply_changes(before, &changes).expect("apply");
        assert_eq!(
            after,
            "## Pomodoros\r\n- [ ] () — WRITE\r\n\t- keep this child\r\n"
        );
    }

    #[test]
    fn apply_changes_preserves_mixed_endings_blank_and_trailing_space() {
        let before = "## Pomodoros\n\n- [ ] () — JOB \r\n- [ ] () — KEEP\ntrailing  \nlast without newline";
        let changes = vec![test_change(3, "JOB", "WRITE")];
        let after = apply_changes(before, &changes).expect("apply");
        assert_eq!(
            after,
            "## Pomodoros\n\n- [ ] () — WRITE \r\n- [ ] () — KEEP\ntrailing  \nlast without newline"
        );
        // No final newline stays absent; blank line and trailing
        // spaces survive untouched.
        assert!(!after.ends_with('\n'));
        assert!(after.contains("\n\n"));
        assert!(after.contains("trailing  \n"));
    }

    #[test]
    fn apply_changes_preserves_no_final_newline_on_changed_line() {
        let before = "## Pomodoros\n- [ ] () — JOB";
        let changes = vec![test_change(2, "JOB", "WRITE")];
        let after = apply_changes(before, &changes).expect("apply");
        assert_eq!(after, "## Pomodoros\n- [ ] () — WRITE");
    }

    #[test]
    fn verify_split_rejects_collateral_terminator_rewrite() {
        let temp = tempfile::tempdir().expect("scratch vault");
        let vault = temp.path();
        let relative = "2026/20261020.md".to_string();
        let absolute = vault.join(&relative);
        std::fs::create_dir_all(absolute.parent().unwrap()).unwrap();
        let before =
            "## Pomodoros\r\n- [ ] () — JOB\r\n\t- keep this child\r\n";
        // Malformed planned image normalizes every CRLF to LF.
        let malformed = "## Pomodoros\n- [ ] () — WRITE\n\t- keep this child\n";
        std::fs::write(&absolute, malformed).unwrap();
        let prepared = PreparedSplit {
            old_key: "job".to_string(),
            old_name: "JOB".to_string(),
            files: vec![FilePlan {
                path: relative.clone(),
                changes: vec![test_change(2, "JOB", "WRITE")],
                before_contents: before.to_string(),
                after_contents: malformed.to_string(),
            }],
            targets: Vec::new(),
            kept: 0,
            unmapped: 0,
            remaining: 0,
            warnings: Vec::new(),
            sync_pre: "skipped",
        };
        let error = verify_split(vault, &prepared).expect_err("must reject");
        assert!(
            error.contains("outside a Pomodoro name tail")
                || error.contains("different set"),
            "{error}"
        );
    }
}
