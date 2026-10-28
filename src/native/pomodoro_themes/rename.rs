//! Previewed, verified, single-commit global rename for Pomodoro themes.
//!
//! `bob themes rename OLD NEW` rewrites the theme component in ` — NAME`
//! tails of canonical daily notes (past, today, and future, open and
//! closed) plus `_templates/daily.md`, modeled on `bob ref rename`.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use chrono::{DateTime, FixedOffset};
use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use crate::native::{
    capture_pomodoros, config, env as bob_env,
    markdown::split_line_ending,
    ob::{self},
    plan_budget::{normalize_component, split_components},
    pomodoro_homes::Hue,
};

use super::{
    cache,
    journal::{self, EventPayload},
    scan, Catalog,
};

const COMMAND_NAME: &str = "bob themes rename";
pub(super) const SCHEMA_VERSION: u32 = 1;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    pub(super) fn from_matches(matches: &ArgMatches) -> Self {
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

pub(crate) fn build_cli_rename() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Rename a Pomodoro theme everywhere in one commit")
        .long_about(
            "Rename one Pomodoro theme across every canonical daily note \
(past, today, and future, open and closed) plus _templates/daily.md, in a \
single verified Git commit.\n\n\
Only the theme component in ` — NAME` tails changes: separators keep their \
`+` spelling, a resulting duplicate collapses (`FIX + FIXES` → `FIX`), and \
every other byte is preserved. Prose, task text, and fenced examples are \
never touched.\n\n\
The preview always rescans from source. The write takes bob_sync.lock, \
pre-syncs unless --offline, snapshots each target and compares again right \
before writing, writes atomically, appends one theme.rename journal event, \
verifies by rescanning, makes one commit, then post-syncs. Undo is \
`git revert`.\n\n\
Online writes require -b/--bob-dir and BOB_DIR (default ~/bob) to resolve to the same vault; a mismatch refuses before locks or sync. Set BOB_DIR to the selected vault or use --offline. Previews and offline writes do not require this match.",
        )
        .after_help(
            "Examples:\n  \
bob themes rename FIXES FIX -d\n  \
bob themes rename FIXES FIX -d -f json\n  \
bob themes rename OLD NEW --merge\n  \
bob themes rename FIXES FIX -o\n\n\
Environment:\n  \
BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
BOB_NOW                   Local datetime override that pins the event timestamp\n  \
HOSTNAME                  Host label for the journal file; same slug vault-sync uses\n  \
BOB_VAULT_SYNC_LOCK_FILE  Maintenance lock; defaults to ${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock\n\n\
Exit codes:\n  \
0  renamed (or dry-run planned)\n  \
1  unknown theme, invalid name, unchanged, theme exists, config reference, lock busy, dirty paths, not a worktree, sync failed, verify failed, stale note, or I/O failure\n  \
2  usage error\n\n\
See docs/pomodoro-themes.md for the rename contract, refusal table, and revert runbook.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(merge_arg())
        .arg(offline_arg())
        .arg(via_arg())
        .arg(old_arg())
        .arg(new_arg())
}

pub(super) fn bob_dir_arg() -> Arg {
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
        .help("Preview the rename without writing, committing, or syncing")
}

pub(super) fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

pub(super) fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn merge_arg() -> Arg {
    Arg::new("merge")
        .long("merge")
        .short('m')
        .action(ArgAction::SetTrue)
        .help("Merge OLD into NEW when NEW is already used")
}

pub(super) fn offline_arg() -> Arg {
    Arg::new("offline")
        .long("offline")
        .short('o')
        .action(ArgAction::SetTrue)
        .help("Commit locally; skip vault sync before and after")
}

fn via_arg() -> Arg {
    Arg::new("via")
        .long("via")
        .value_name("CLIENT")
        .hide(true)
        .help("Internal client id written on the journal event")
}

fn old_arg() -> Arg {
    Arg::new("old")
        .value_name("OLD")
        .required(true)
        .help("Current theme name as it appears in a Pomodoro tail")
}

fn new_arg() -> Arg {
    Arg::new("new")
        .value_name("NEW")
        .required(true)
        .help("New single theme name (must not contain +)")
}

pub(crate) fn run_rename(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_rename();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(COMMAND_NAME, error),
    };
    let format = OutputFormat::from_matches(&matches);
    let dry_run = matches.get_flag("dry-run");
    if dry_run {
        return match prepare_from_matches(&matches, true) {
            Ok(prepared) => {
                print_success(&prepared.report(), format);
                0
            }
            Err(failure) => print_failure(COMMAND_NAME, format, failure),
        };
    }
    run_live(&matches, format)
}

#[derive(Debug, Clone)]
pub(super) struct RenameFailure {
    pub(super) code: &'static str,
    pub(super) message: String,
    pub(super) hint: Option<String>,
}

impl RenameFailure {
    pub(super) fn new(
        code: &'static str,
        message: impl Into<String>,
        hint: Option<impl Into<String>>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            hint: hint.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct TailChange {
    line: usize,
    before: String,
    after: String,
}

#[derive(Debug, Clone, Serialize)]
struct FilePlan {
    path: String,
    entries: usize,
    tails: Vec<TailChange>,
    #[serde(skip_serializing)]
    before_contents: String,
    #[serde(skip_serializing)]
    after_contents: String,
}

#[derive(Debug, Clone, Serialize)]
struct RenameReport {
    schema_version: u32,
    ok: bool,
    mode: String,
    changed: bool,
    from: RenameSide,
    to: RenameSide,
    merge: bool,
    files: Vec<FilePlanView>,
    entries: usize,
    state: StateChange,
    color: ColorChange,
    commit: Option<serde_json::Value>,
    sync: SyncInfo,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct RenameSide {
    key: String,
    name: String,
}

#[derive(Debug, Clone, Serialize)]
struct FilePlanView {
    path: String,
    entries: usize,
    tails: Vec<TailChange>,
}

#[derive(Debug, Clone, Serialize)]
struct StateChange {
    hidden_before: bool,
    hidden_after: bool,
    moved: bool,
    note: String,
}

#[derive(Debug, Clone, Serialize)]
struct ColorChange {
    before: Option<String>,
    before_source: String,
    after: Option<String>,
    after_source: String,
    note: String,
}

#[derive(Debug, Clone, Serialize)]
struct SyncInfo {
    pre: String,
    post: String,
}

struct PreparedRename {
    vault: PathBuf,
    old_key: String,
    old_name: String,
    new_key: String,
    new_name: String,
    merge: bool,
    via: String,
    files: Vec<FilePlan>,
    total_before_entries: usize,
    warnings: Vec<String>,
    sync_pre: &'static str,
    hidden_before: bool,
}

impl PreparedRename {
    fn report(&self) -> RenameReport {
        let (color_before, source_before) =
            hue_for_key(&self.vault, &self.old_key);
        let (color_after, source_after) =
            hue_for_key_after_rename(&self.vault, &self.old_key, &self.new_key);
        // The fold rule: source hidden moves to target unless target already
        // has explicit state. Compute precisely from prefs.
        let (hidden_after, moved) =
            moved_hidden(&self.vault, &self.old_key, &self.new_key);
        let state_note = if moved {
            format!("hidden {} moves to {}", self.old_name, self.new_name)
        } else if self.hidden_before {
            format!(
                "{} stays hidden; {} already has explicit state",
                self.old_name, self.new_name
            )
        } else {
            format!("{} is not hidden", self.old_name)
        };
        let color_note = match (color_before, color_after) {
            (Some(before), Some(after)) if before == after => {
                format!("color follows {} as {}", self.new_name, after.name())
            }
            (Some(before), Some(after)) => {
                format!(
                    "color {} → {} with {}",
                    before.name(),
                    after.name(),
                    self.new_name
                )
            }
            (None, Some(after)) => {
                format!("color follows {} as {}", self.new_name, after.name())
            }
            (Some(before), None) => {
                format!(
                    "{} muted after rename (was {})",
                    self.new_name,
                    before.name()
                )
            }
            (None, None) => {
                format!("{} stays muted", self.new_name)
            }
        };
        let _ = hidden_after;
        RenameReport {
            schema_version: SCHEMA_VERSION,
            ok: true,
            mode: "dry_run".to_string(),
            changed: !self.files.is_empty(),
            from: RenameSide {
                key: self.old_key.clone(),
                name: self.old_name.clone(),
            },
            to: RenameSide {
                key: self.new_key.clone(),
                name: self.new_name.clone(),
            },
            merge: self.merge,
            entries: self.files.iter().map(|file| file.entries).sum(),
            files: self
                .files
                .iter()
                .map(|file| FilePlanView {
                    path: file.path.clone(),
                    entries: file.entries,
                    tails: file.tails.clone(),
                })
                .collect(),
            state: StateChange {
                hidden_before: self.hidden_before,
                hidden_after,
                moved,
                note: state_note,
            },
            color: ColorChange {
                before: color_before.map(|hue| hue.name().to_string()),
                before_source: source_before.to_string(),
                after: color_after.map(|hue| hue.name().to_string()),
                after_source: source_after.to_string(),
                note: color_note,
            },
            commit: None,
            sync: SyncInfo {
                pre: self.sync_pre.to_string(),
                post: "skipped".to_string(),
            },
            warnings: self.warnings.clone(),
        }
    }
}

fn hue_for_key(vault: &Path, key: &str) -> (Option<Hue>, &'static str) {
    let today = bob_env::current_datetime().date();
    let Ok(catalog) = Catalog::load(vault, today, true) else {
        return (None, "hash");
    };
    let name = catalog
        .get(key)
        .map(|record| record.name.clone())
        .unwrap_or_else(|| key.to_string());
    let theme_ref = catalog.theme_ref(key, &name);
    let source = match theme_ref.hue_source {
        super::HueSource::Pin => "pin",
        super::HueSource::Exempt => "exempt",
        super::HueSource::Area => "area",
        super::HueSource::Hash => "hash",
    };
    (theme_ref.hue, source)
}

fn hue_for_key_after_rename(
    vault: &Path,
    old_key: &str,
    new_key: &str,
) -> (Option<Hue>, &'static str) {
    // Preview the hue the new name will wear: the old pin moves with the
    // rename unless the target already has a pin, then exempt/area/hash.
    let today = bob_env::current_datetime().date();
    let Ok(catalog) = Catalog::load(vault, today, true) else {
        return (None, "hash");
    };
    let old_pin = catalog.pin(old_key);
    let target_has_state = target_has_explicit_state(vault, new_key);
    let effective_pin = if target_has_state {
        catalog.pin(new_key)
    } else {
        old_pin.or_else(|| catalog.pin(new_key))
    };
    let exempt: BTreeSet<String> =
        config::load_plan_config(&config::config_path())
            .map(|plan| {
                plan.exempt()
                    .iter()
                    .map(|value| normalize_component(value))
                    .collect()
            })
            .unwrap_or_default();
    let areas = scan::scan_root_areas(vault);
    // Reuse the catalog's hue rule with the effective pin.
    let (hue, source) =
        super::resolve_hue(new_key, effective_pin, &exempt, &areas);
    let source = match source {
        super::HueSource::Pin => "pin",
        super::HueSource::Exempt => "exempt",
        super::HueSource::Area => "area",
        super::HueSource::Hash => "hash",
    };
    (hue, source)
}

fn target_has_explicit_state(vault: &Path, key: &str) -> bool {
    let Ok((events, _)) = journal::read_journal(vault) else {
        return false;
    };
    let prefs = journal::fold_events(&events);
    prefs.by_key.contains_key(key)
}

fn moved_hidden(vault: &Path, from: &str, to: &str) -> (bool, bool) {
    let Ok((events, _)) = journal::read_journal(vault) else {
        return (false, false);
    };
    let prefs = journal::fold_events(&events);
    let hidden_before = prefs.hidden(from);
    let target_has_state = prefs.by_key.contains_key(to);
    if hidden_before && !target_has_state {
        (true, true)
    } else {
        (prefs.hidden(to), false)
    }
}

fn prepare_from_matches(
    matches: &ArgMatches,
    dry_run: bool,
) -> Result<PreparedRenameLive, RenameFailure> {
    let vault = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let old_raw = matches
        .get_one::<String>("old")
        .map(String::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let new_raw = matches
        .get_one::<String>("new")
        .map(String::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let merge = matches.get_flag("merge");
    let offline = matches.get_flag("offline");
    let via = matches
        .get_one::<String>("via")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("bob-cli")
        .to_string();
    plan_rename(&vault, &old_raw, &new_raw, merge, &via, offline, dry_run)
}

// Internal prepared form that carries the live sync state.
struct PreparedRenameLive {
    inner: PreparedRename,
    mode: &'static str,
}

impl PreparedRenameLive {
    fn report(&self) -> RenameReport {
        let mut report = self.inner.report();
        report.mode = self.mode.to_string();
        report.sync.pre = self.inner.sync_pre.to_string();
        report
    }
}

fn plan_rename(
    vault: &Path,
    old_raw: &str,
    new_raw: &str,
    merge: bool,
    via: &str,
    offline: bool,
    dry_run: bool,
) -> Result<PreparedRenameLive, RenameFailure> {
    if old_raw.is_empty() || new_raw.is_empty() {
        return Err(RenameFailure::new(
            "usage",
            "OLD and NEW are required",
            Some("usage: bob themes rename OLD NEW [-d/--dry-run] [-f/--format human|json] [-m/--merge] [-o/--offline]"),
        ));
    }
    // NEW must be a valid single theme: no `+`, non-empty key.
    if new_raw.contains('+') || split_components(new_raw).len() != 1 {
        return Err(RenameFailure::new(
            "invalid_name",
            format!("invalid theme name: {new_raw} (a theme never contains +)"),
            Some("choose a single + component without +"),
        ));
    }
    if split_components(old_raw).len() != 1
        || normalize_component(old_raw).is_empty()
    {
        return Err(RenameFailure::new(
            "invalid_name",
            format!("invalid theme name: {old_raw}"),
            Some("rename one + component at a time"),
        ));
    }
    let new_key = normalize_component(new_raw);
    if new_key.is_empty() {
        return Err(RenameFailure::new(
            "invalid_name",
            format!("invalid theme name: {new_raw}"),
            Some("choose a non-blank theme name"),
        ));
    }
    let old_key = normalize_component(old_raw);
    if old_key.is_empty() {
        return Err(RenameFailure::new(
            "invalid_name",
            format!("invalid theme name: {old_raw}"),
            Some("choose a non-blank theme name"),
        ));
    }
    if old_key == new_key {
        return Err(RenameFailure::new(
            "unchanged",
            format!("{old_raw} is already named {new_raw}"),
            Some("choose a different NEW name"),
        ));
    }

    let scope = scope_files(vault);
    let mut files = Vec::new();
    let mut total_before = 0usize;
    let mut new_occurrences = 0usize;
    for (relative, absolute) in &scope {
        let contents = fs::read_to_string(absolute).map_err(|error| {
            RenameFailure::new(
                "io",
                format!("could not read {}: {error}", relative),
                None::<String>,
            )
        })?;
        let scan = capture_pomodoros::scan(&contents);
        total_before += scan.entries.len();
        // Count occurrences of NEW in this file for theme_exists.
        for entry in &scan.entries {
            let Some(name) = entry.name.as_deref() else {
                continue;
            };
            let keys = split_components(name)
                .iter()
                .map(|component| normalize_component(component))
                .collect::<Vec<_>>();
            if keys.iter().any(|key| key == &new_key) {
                new_occurrences += 1;
            }
        }
        if let Some((after, tails)) =
            rewrite_file_contents(&contents, &old_key, new_raw.trim())
        {
            let changed_entries = tails.len();
            files.push(FilePlan {
                path: relative.clone(),
                entries: changed_entries,
                tails,
                before_contents: contents,
                after_contents: after,
            });
        }
    }
    // The preview always rescans from source, so totals above are live.
    if files.is_empty() {
        return Err(RenameFailure::new(
            "unknown_theme",
            format!("unknown theme: {old_raw}"),
            Some("list known themes with bob themes list -a"),
        ));
    }
    if new_occurrences > 0 && !merge {
        return Err(RenameFailure::new(
            "theme_exists",
            format!(
                "{new_raw} is already used; pass --merge to combine {old_raw} into it"
            ),
            Some("re-run with --merge to collapse duplicates"),
        ));
    }
    if let Some(reference) =
        config_reference(vault, &old_key, &new_key, old_raw, new_raw)
    {
        return Err(reference);
    }

    let warnings = Vec::new();
    let touched_relative = files
        .iter()
        .map(|file| PathBuf::from(&file.path))
        .collect::<BTreeSet<_>>();

    // Dirty-path and worktree checks only matter for a live write; the
    // dry run reports the plan without touching Git.
    if !dry_run {
        let dirty = crate::native::vault_rewrite::dirty_touched_paths(
            vault,
            &touched_relative,
        )
        .map_err(|failure| {
            RenameFailure::new(failure.code, failure.message, failure.hint)
        })?;
        if offline && !dirty.is_empty() {
            return Err(RenameFailure::new(
                "dirty_paths",
                format!("rename paths are dirty: {}", dirty.join(", ")),
                Some("commit or restore those paths before an offline rename"),
            ));
        }
        // Online dirty paths are only warnings; the pre-sync commits them.
        // The caller computes them after the pre-sync, so stash nothing here.
        let _ = dirty;
    }

    let today = bob_env::current_datetime().date();
    let catalog = Catalog::load(vault, today, false).map_err(|error| {
        RenameFailure::new(
            "io",
            format!("read theme catalog in {}: {error}", vault.display()),
            None::<String>,
        )
    })?;
    let old_display = catalog
        .get(&old_key)
        .map(|record| record.name.clone())
        .unwrap_or_else(|| old_raw.trim().to_string());
    let hidden_before = catalog.is_hidden(&old_key);

    Ok(PreparedRenameLive {
        inner: PreparedRename {
            vault: vault.to_path_buf(),
            old_key,
            old_name: old_display,
            new_key,
            new_name: new_raw.trim().to_string(),
            merge,
            via: via.to_string(),
            files,
            total_before_entries: total_before,
            warnings,
            sync_pre: "skipped",
            hidden_before,
        },
        mode: if dry_run { "dry_run" } else { "write" },
    })
}

fn scope_files(vault: &Path) -> Vec<(String, PathBuf)> {
    let mut scoped = Vec::new();
    for (date, relative, absolute) in scan::list_daily_notes(vault) {
        let _ = date;
        scoped.push((relative, absolute));
    }
    let template_relative = "_templates/daily.md".to_string();
    let template_absolute = vault.join(&template_relative);
    if template_absolute.is_file() {
        scoped.push((template_relative, template_absolute));
    }
    scoped.sort();
    scoped
}

/// Physical lines with their terminators, so concatenating them
/// reproduces `contents` byte for byte. Line `n` (1-based) is index
/// `n - 1`, matching `capture_pomodoros::scan` entry lines.
pub(super) fn raw_lines_preserving(contents: &str) -> Vec<String> {
    contents.split_inclusive('\n').map(str::to_string).collect()
}

/// Raw byte check shared by rename and split: lines outside
/// `planned_lines` must keep exact bytes and planned lines must keep
/// their original terminators. This catches a planned after-image
/// that normalizes CRLF or touches unrelated bytes even when the
/// reread equals it. `action` names the write in race errors.
pub(super) fn verify_raw_tail_splice(
    path: &str,
    before_contents: &str,
    planned_contents: &str,
    reread_contents: &str,
    planned_lines: &BTreeSet<usize>,
    action: &str,
) -> Result<(), String> {
    let before_raw = raw_lines_preserving(before_contents);
    let planned_raw = raw_lines_preserving(planned_contents);
    let reread_raw = raw_lines_preserving(reread_contents);
    if before_raw.len() != planned_raw.len()
        || reread_raw.len() != planned_raw.len()
    {
        return Err(format!("{path} line count changed"));
    }
    let outside = || format!("{path} changed outside a Pomodoro name tail");
    let raced = || format!("{path} changed during the {action}");
    for (index, ((before, planned_after), reread)) in before_raw
        .iter()
        .zip(planned_raw.iter())
        .zip(reread_raw.iter())
        .enumerate()
    {
        if !planned_lines.contains(&(index + 1)) {
            if before != planned_after {
                return Err(outside());
            }
            if reread != planned_after {
                return Err(raced());
            }
            continue;
        }
        let (_, before_ending) = split_line_ending(before);
        let (_, planned_ending) = split_line_ending(planned_after);
        let (_, reread_ending) = split_line_ending(reread);
        if before_ending != planned_ending {
            return Err(outside());
        }
        if reread_ending != planned_ending {
            return Err(raced());
        }
    }
    Ok(())
}

/// Rewrite the theme component in every Pomodoro ` — NAME` tail.
/// Returns the new contents plus one tail change per rewritten entry.
///
/// Every byte outside the precise renamed spans is preserved: each raw
/// line keeps its original terminator (`\n`, `\r\n`, or none) and all
/// untouched lines keep their exact bytes. Only the NAME tail body is
/// spliced; the file is concatenated from raw lines, never rebuilt
/// from `lines()` with `join("\n")`.
fn rewrite_file_contents(
    contents: &str,
    old_key: &str,
    new_display_trimmed: &str,
) -> Option<(String, Vec<TailChange>)> {
    let scan = capture_pomodoros::scan(contents);
    if scan.entries.is_empty() {
        return None;
    }
    // Map line number (1-based) to the entry name for lines we may rewrite.
    let mut by_line: BTreeMap<usize, &str> = BTreeMap::new();
    for entry in &scan.entries {
        let Some(name) = entry.name.as_deref() else {
            continue;
        };
        if split_components(name)
            .iter()
            .any(|component| normalize_component(component) == old_key)
        {
            by_line.insert(entry.line, name);
        }
    }
    if by_line.is_empty() {
        return None;
    }
    // Raw lines keep their terminators so splicing one tail never
    // normalizes CRLF, mixed endings, trailing spaces, or a missing
    // final newline elsewhere in the file.
    let mut owned = raw_lines_preserving(contents);
    let mut tails = Vec::new();
    for (line_number, old_name) in by_line {
        let index = line_number.saturating_sub(1);
        let Some(raw) = owned.get(index).cloned() else {
            continue;
        };
        let (body, ending) = split_line_ending(&raw);
        let Some(new_name) =
            rewrite_name(old_name, old_key, new_display_trimmed)
        else {
            continue;
        };
        let Some(rewritten) = replace_name_tail(body, old_name, &new_name)
        else {
            continue;
        };
        owned[index] = format!("{rewritten}{ending}");
        tails.push(TailChange {
            line: line_number,
            before: old_name.to_string(),
            after: new_name,
        });
    }
    if tails.is_empty() {
        return None;
    }
    Some((owned.concat(), tails))
}

/// Replace matching `+` components with the new display spelling,
/// collapsing resulting duplicates. `None` when nothing matches.
pub(super) fn rewrite_name(
    old_name: &str,
    old_key: &str,
    new_display_trimmed: &str,
) -> Option<String> {
    let mut parts: Vec<String> =
        old_name.split('+').map(str::to_string).collect();
    let mut matched = false;
    for part in &mut parts {
        let trimmed = part.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        if normalize_component(&trimmed) == old_key {
            matched = true;
            // Preserve the part's surrounding whitespace.
            let start = part.find(trimmed.as_str()).unwrap_or(0);
            let end = start + trimmed.len();
            part.replace_range(start..end, new_display_trimmed);
        }
    }
    if !matched {
        return None;
    }
    // Collapse duplicate keys, keeping the first occurrence.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut kept: Vec<String> = Vec::new();
    for part in parts {
        let trimmed = part.trim().to_string();
        if trimmed.is_empty() {
            kept.push(part);
            continue;
        }
        let key = normalize_component(&trimmed);
        if seen.insert(key) {
            kept.push(part);
        }
    }
    let joined = kept.join("+");
    let final_name = joined.trim().to_string();
    if final_name == old_name {
        return None;
    }
    Some(final_name)
}

/// Replace the trimmed NAME tail in one ledger line, preserving every
/// other byte (the ` — ` separator spelling and trailing spaces).
pub(super) fn replace_name_tail(
    line: &str,
    old_name_trimmed: &str,
    new_name: &str,
) -> Option<String> {
    let dash = line.find('—')?;
    let after_dash = &line[dash..];
    let offset = after_dash.find(old_name_trimmed)?;
    let start = dash + offset;
    let end = start + old_name_trimmed.len();
    let mut rewritten = String::with_capacity(line.len() + new_name.len());
    rewritten.push_str(&line[..start]);
    rewritten.push_str(new_name);
    rewritten.push_str(&line[end..]);
    Some(rewritten)
}

fn config_reference(
    vault: &Path,
    old_key: &str,
    new_key: &str,
    old_raw: &str,
    new_raw: &str,
) -> Option<RenameFailure> {
    let config_path = config::config_path();
    // plan.exempt and plan.inventory_labels.
    if let Ok(plan) = config::load_plan_config(&config_path) {
        let exempt = plan
            .exempt()
            .iter()
            .map(|value| normalize_component(value))
            .collect::<BTreeSet<_>>();
        let inventory = plan
            .inventory_labels()
            .iter()
            .map(|value| normalize_component(value))
            .collect::<BTreeSet<_>>();
        let old_in_config =
            exempt.contains(old_key) || inventory.contains(old_key);
        let new_in_config =
            exempt.contains(new_key) || inventory.contains(new_key);
        if old_in_config && !new_in_config {
            return Some(RenameFailure::new(
                "config_reference",
                format!(
                    "{old_raw} is a plan config reference; add {new_raw} to plan.exempt or plan.inventory_labels first"
                ),
                Some("edit the chezmoi config to add NEW first"),
            ));
        }
    }
    // Stats metric theme filters.
    if let Ok(stats) = config::load_stats_config(&config_path) {
        let registry = crate::native::stats::build_registry(&stats);
        let mut old_hit = false;
        let mut new_hit = false;
        for def in &registry.defs {
            if let crate::native::stats::registry::MetricSpec::Pomodoro {
                themes,
                exclude_themes,
                ..
            } = &def.spec
            {
                for theme in themes.iter().chain(exclude_themes.iter()) {
                    let key = normalize_component(theme);
                    if key == old_key {
                        old_hit = true;
                    }
                    if key == new_key {
                        new_hit = true;
                    }
                }
            }
        }
        if old_hit && !new_hit {
            return Some(RenameFailure::new(
                "config_reference",
                format!(
                    "{old_raw} is a Stats metric filter; add {new_raw} to that metric first"
                ),
                Some("edit the chezmoi config to add NEW first"),
            ));
        }
    }
    let _ = vault;
    None
}

fn run_live(matches: &ArgMatches, format: OutputFormat) -> i32 {
    let child_env = ob::child_env();
    let vault = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let offline = matches.get_flag("offline");
    let json_mode = format == OutputFormat::Json;

    let outcome = crate::native::vault_rewrite::run(
        &vault,
        &child_env,
        COMMAND_NAME,
        "theme renames",
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
            let inner = Rc::new(prepared.inner);
            let files = inner
                .files
                .iter()
                .map(|file| crate::native::vault_rewrite::FileRewrite {
                    path: file.path.clone(),
                    before_contents: file.before_contents.clone(),
                    after_contents: file.after_contents.clone(),
                })
                .collect::<Vec<_>>();
            let subject = format!(
                "bob themes rename: {} → {}",
                inner.old_name, inner.new_name
            );
            let post_inner = Rc::clone(&inner);
            let post_vault = vault.clone();
            let verify_inner = Rc::clone(&inner);
            let verify_vault = vault.clone();
            let invalidate_vault = vault.clone();
            Ok(crate::native::vault_rewrite::ReadyPlan {
                files,
                subject,
                post_write: Box::new(move || {
                    append_rename_event(
                        &post_vault,
                        &post_inner.old_key,
                        &post_inner.new_key,
                        &post_inner.old_name,
                        &post_inner.new_name,
                        &post_inner.via,
                    )
                    .map(|relative| vec![PathBuf::from(relative)])
                    .map_err(|message| {
                        format!("could not append theme.rename: {message}")
                    })
                }),
                verify: Box::new(move || {
                    verify_renamed(&verify_vault, &verify_inner)
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
            let mut report = outcome.context.report();
            report.mode = "write".to_string();
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
            print_success(&report, format);
            0
        }
        Err(failure) => print_failure(
            COMMAND_NAME,
            format,
            RenameFailure::new(failure.code, failure.message, failure.hint),
        ),
    }
}

fn append_rename_event(
    vault: &Path,
    from_key: &str,
    to_key: &str,
    from_name: &str,
    to_name: &str,
    via: &str,
) -> Result<String, String> {
    let host = journal::host_label();
    let ts = journal::event_timestamp();
    let payload = EventPayload::Rename {
        from: from_key.to_string(),
        to: to_key.to_string(),
        from_name: from_name.to_string(),
        to_name: to_name.to_string(),
    };
    let path = journal::host_journal_path(vault, &host);
    journal::append_event_at(vault, &host, &payload, via, ts)
        .map_err(|error| error.to_string())?;
    Ok(path
        .strip_prefix(vault)
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/"))
}

fn verify_renamed(
    vault: &Path,
    prepared: &PreparedRename,
) -> Result<(), String> {
    // OLD must be absent everywhere in scope.
    let scope = scope_files(vault);
    let mut after_total = 0usize;
    for (relative, absolute) in &scope {
        let contents = fs::read_to_string(absolute)
            .map_err(|error| format!("reread {relative}: {error}"))?;
        let scan = capture_pomodoros::scan(&contents);
        after_total += scan.entries.len();
        for entry in &scan.entries {
            let Some(name) = entry.name.as_deref() else {
                continue;
            };
            if split_components(name).iter().any(|component| {
                normalize_component(component) == prepared.old_key
            }) {
                return Err(format!(
                    "{} still contains {}",
                    relative, prepared.old_name
                ));
            }
        }
        // Touched files must match the planned after-image exactly.
        if let Some(planned) =
            prepared.files.iter().find(|file| &file.path == relative)
        {
            if contents != planned.after_contents {
                return Err(format!("{} changed during the rename", relative));
            }
            let planned_lines: BTreeSet<usize> =
                planned.tails.iter().map(|tail| tail.line).collect();
            verify_raw_tail_splice(
                relative,
                &planned.before_contents,
                &planned.after_contents,
                &contents,
                &planned_lines,
                "rename",
            )?;
            // Only name tails may differ: every changed line shares the
            // same ledger prefix before the tail.
            let before_lines: Vec<&str> =
                planned.before_contents.lines().collect();
            let after_lines: Vec<&str> = contents.lines().collect();
            if before_lines.len() != after_lines.len() {
                return Err(format!("{} line count changed", relative));
            }
            for (before, after) in before_lines.iter().zip(after_lines.iter()) {
                if before == after {
                    continue;
                }
                if capture_pomodoros::without_name_tail(before)
                    != capture_pomodoros::without_name_tail(after)
                {
                    return Err(format!(
                        "{} changed outside a Pomodoro name tail",
                        relative
                    ));
                }
            }
        }
    }
    if after_total != prepared.total_before_entries {
        return Err(format!(
            "entry count changed (was {}, now {})",
            prepared.total_before_entries, after_total
        ));
    }
    Ok(())
}

fn print_success(report: &RenameReport, format: OutputFormat) {
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

fn print_human(report: &RenameReport) {
    let styler = crate::native::style::Styler::detect();
    let mode_suffix = if report.mode == "dry_run" {
        " (dry run)"
    } else {
        ""
    };
    println!(
        "{} rename {} → {}{}",
        styler.success_prefix(report.mode == "dry_run"),
        report.from.name,
        report.to.name,
        mode_suffix,
    );
    if report.files.is_empty() {
        println!("  no files");
    }
    for file in &report.files {
        println!(
            "  {}: {} entr{}",
            file.path,
            file.entries,
            if file.entries == 1 { "y" } else { "ies" },
        );
        for tail in &file.tails {
            println!(
                "    line {}: {} → {}",
                tail.line, tail.before, tail.after
            );
        }
    }
    println!("  state: {}", report.state.note);
    println!("  color: {}", report.color.note);
    if let Some(commit) = &report.commit {
        let sha = commit
            .get("sha")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("(unknown)");
        let subject = commit
            .get("subject")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("bob themes rename");
        println!("  committed {sha} {subject}");
    }
    println!("  sync: pre {}, post {}", report.sync.pre, report.sync.post);
    for warning in &report.warnings {
        eprintln!("{}: {warning}", styler.warning_prefix());
    }
}

pub(super) fn print_clap_error(command_name: &str, error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        // A closed pipe renders nothing: stay quiet instead of
        // reporting the failed render, but keep clap's own exit
        // code so a usage error never reports success.
        if print_error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!(
                "{command_name}: failed to print command-line error: {print_error}"
            );
        }
    }
    exit_code
}

pub(super) fn print_failure(
    command_name: &str,
    format: OutputFormat,
    failure: RenameFailure,
) -> i32 {
    let exit = match failure.code {
        "usage" | "invalid_map" => 2,
        _ => 1,
    };
    match format {
        OutputFormat::Json => {
            let mut value = json!({
                "schema_version": SCHEMA_VERSION,
                "ok": false,
                "code": failure.code,
                "message": failure.message,
            });
            if let Some(hint) = failure.hint {
                value["hint"] = json!(hint);
            }
            println!("{value}");
        }
        OutputFormat::Human => {
            eprintln!("{command_name}: {}", failure.message);
            if let Some(hint) = failure.hint {
                eprintln!("hint: {hint}");
            }
        }
    }
    exit
}

fn writer_order(
    event: &journal::JournalEvent,
) -> (DateTime<FixedOffset>, &str, &str, u32) {
    (
        event.ts,
        event.host.as_str(),
        event.path.as_str(),
        event.line,
    )
}

/// A later rename *to* this key, or a later color/unhide naming it,
/// means the name was deliberately re-adopted. A later hide does not.
fn re_adopted_later(
    events: &[journal::JournalEvent],
    rename: &journal::JournalEvent,
    key: &str,
) -> bool {
    let order = writer_order(rename);
    events.iter().any(|event| {
        if writer_order(event) <= order {
            return false;
        }
        match &event.payload {
            EventPayload::Rename { to, .. } => to == key,
            EventPayload::Color { key: named_key, .. }
            | EventPayload::Unhide { key: named_key, .. } => named_key == key,
            EventPayload::Hide { .. } => false,
        }
    })
}

/// `renamed_reappeared` for `bob themes list`: every rename source key
/// that still has occurrences after its rename event.
pub(crate) fn renamed_reappeared(
    vault: &Path,
    catalog: &Catalog,
) -> Vec<serde_json::Value> {
    let Ok((events, _)) = journal::read_journal(vault) else {
        return Vec::new();
    };
    // Latest rename per source key.
    let mut latest: BTreeMap<String, &journal::JournalEvent> = BTreeMap::new();
    for event in &events {
        if let EventPayload::Rename { from, .. } = &event.payload {
            let replace = match latest.get(from) {
                Some(current) => writer_order(event) > writer_order(current),
                None => true,
            };
            if replace {
                latest.insert(from.clone(), event);
            }
        }
    }
    let mut reappeared = Vec::new();
    for (from, event) in latest {
        if re_adopted_later(&events, event, &from) {
            continue;
        }
        let Some(record) = catalog.get(&from) else {
            continue;
        };
        let (to, to_name) = match &event.payload {
            EventPayload::Rename { to, to_name, .. } => (to, to_name),
            _ => continue,
        };
        reappeared.push(json!({
            "key": record.key,
            "name": record.name,
            "renamed_to": to,
            "renamed_to_name": to_name,
        }));
    }
    reappeared.sort_by(|left, right| {
        left.get("name")
            .and_then(serde_json::Value::as_str)
            .cmp(&right.get("name").and_then(serde_json::Value::as_str))
    });
    reappeared
}

#[cfg(test)]
mod rename_tests {
    use super::*;

    #[test]
    fn rewrite_name_replaces_component_and_preserves_separators() {
        assert_eq!(
            rewrite_name("FIXES", "fixes", "FIX").as_deref(),
            Some("FIX")
        );
        assert_eq!(
            rewrite_name("A + FIXES + B", "fixes", "FIX").as_deref(),
            Some("A + FIX + B")
        );
        assert_eq!(
            rewrite_name("FIX + FIXES", "fixes", "FIX").as_deref(),
            Some("FIX")
        );
        assert_eq!(
            rewrite_name("A+FIXES+B", "fixes", "FIX").as_deref(),
            Some("A+FIX+B")
        );
        assert_eq!(rewrite_name("BOB", "fixes", "FIX"), None);
    }

    #[test]
    fn replace_name_tail_preserves_prefix_bytes() {
        let line = "- [ ] () — FIX + FIXES";
        let rewritten =
            replace_name_tail(line, "FIX + FIXES", "FIX").expect("rewrite");
        assert_eq!(rewritten, "- [ ] () — FIX");
    }

    fn test_event(
        ts: &str,
        host: &str,
        path: &str,
        line: u32,
        payload: EventPayload,
    ) -> journal::JournalEvent {
        journal::JournalEvent {
            ts: DateTime::parse_from_rfc3339(ts).expect("ts"),
            host: host.to_string(),
            path: path.to_string(),
            line,
            payload,
        }
    }

    fn rename_event(
        ts: &str,
        host: &str,
        path: &str,
        line: u32,
        from: &str,
        to: &str,
    ) -> journal::JournalEvent {
        test_event(
            ts,
            host,
            path,
            line,
            EventPayload::Rename {
                from: from.to_string(),
                to: to.to_string(),
                from_name: from.to_string(),
                to_name: to.to_string(),
            },
        )
    }

    #[test]
    fn writer_order_uses_path_before_line() {
        let ts = "2026-10-27T12:00:00Z";
        // Same ts/host, different paths: later path wins even with
        // a smaller line number.
        let earlier = test_event(
            ts,
            "apollo",
            "a.jsonl",
            5,
            EventPayload::Hide {
                key: "fixes".to_string(),
                name: "FIXES".to_string(),
            },
        );
        let later = test_event(
            ts,
            "apollo",
            "b.jsonl",
            3,
            EventPayload::Hide {
                key: "fixes".to_string(),
                name: "FIXES".to_string(),
            },
        );
        assert!(writer_order(&later) > writer_order(&earlier));

        // Equal line numbers across paths: path still orders.
        let earlier_equal = test_event(
            ts,
            "apollo",
            "a.jsonl",
            3,
            EventPayload::Hide {
                key: "fixes".to_string(),
                name: "FIXES".to_string(),
            },
        );
        let later_equal = test_event(
            ts,
            "apollo",
            "b.jsonl",
            3,
            EventPayload::Hide {
                key: "fixes".to_string(),
                name: "FIXES".to_string(),
            },
        );
        assert!(writer_order(&later_equal) > writer_order(&earlier_equal));
    }

    #[test]
    fn re_adopted_later_respects_journal_path_tiebreak() {
        let ts = "2026-10-27T12:00:00Z";
        let rename = rename_event(ts, "apollo", "a.jsonl", 5, "fixes", "fix");
        // Same ts/host, later path but inverted (smaller) line number:
        // path orders before line, so this still re-adopts.
        let color_in_later_path = test_event(
            ts,
            "apollo",
            "b.jsonl",
            3,
            EventPayload::Color {
                key: "fixes".to_string(),
                name: "FIXES".to_string(),
                hue: Some(Hue::Red),
            },
        );
        assert!(re_adopted_later(
            &[rename.clone(), color_in_later_path.clone()],
            &rename,
            "fixes"
        ));

        // Equal line numbers across paths also re-adopt.
        let rename_equal =
            rename_event(ts, "apollo", "a.jsonl", 3, "fixes", "fix");
        let color_equal = test_event(
            ts,
            "apollo",
            "b.jsonl",
            3,
            EventPayload::Color {
                key: "fixes".to_string(),
                name: "FIXES".to_string(),
                hue: None,
            },
        );
        assert!(re_adopted_later(
            &[rename_equal.clone(), color_equal],
            &rename_equal,
            "fixes"
        ));
    }

    #[test]
    fn re_adopted_later_covers_color_unpin_unhide_and_rename_to() {
        let ts = "2026-10-27T12:00:00Z";
        let rename =
            rename_event(ts, "apollo", "apollo.jsonl", 1, "fixes", "fix");
        let events = [
            test_event(
                ts,
                "apollo",
                "apollo.jsonl",
                2,
                EventPayload::Color {
                    key: "fixes".to_string(),
                    name: "FIXES".to_string(),
                    hue: None,
                },
            ),
            test_event(
                ts,
                "apollo",
                "apollo.jsonl",
                2,
                EventPayload::Unhide {
                    key: "fixes".to_string(),
                    name: "FIXES".to_string(),
                },
            ),
            test_event(
                ts,
                "apollo",
                "apollo.jsonl",
                2,
                EventPayload::Rename {
                    from: "fix".to_string(),
                    to: "fixes".to_string(),
                    from_name: "FIX".to_string(),
                    to_name: "FIXES".to_string(),
                },
            ),
        ];
        for event in &events {
            assert!(
                re_adopted_later(
                    &[rename.clone(), event.clone()],
                    &rename,
                    "fixes"
                ),
                "{event:?}"
            );
        }
        let hide = test_event(
            ts,
            "apollo",
            "apollo.jsonl",
            2,
            EventPayload::Hide {
                key: "fixes".to_string(),
                name: "FIXES".to_string(),
            },
        );
        assert!(!re_adopted_later(&[rename.clone(), hide], &rename, "fixes"));
    }

    #[test]
    fn later_rename_away_restores_the_warning() {
        let ts = "2026-10-27T12:00:00Z";
        let first =
            rename_event(ts, "apollo", "apollo.jsonl", 1, "fixes", "fix");
        let readopt = test_event(
            ts,
            "apollo",
            "apollo.jsonl",
            2,
            EventPayload::Color {
                key: "fixes".to_string(),
                name: "FIXES".to_string(),
                hue: Some(Hue::Red),
            },
        );
        let second =
            rename_event(ts, "apollo", "apollo.jsonl", 3, "fixes", "other");
        let events = vec![first.clone(), readopt, second.clone()];
        // The first rename was re-adopted, but the latest rename has
        // no later re-adoption, so the warning returns.
        assert!(re_adopted_later(&events, &first, "fixes"));
        assert!(!re_adopted_later(&events, &second, "fixes"));
    }

    #[test]
    fn rewrite_preserves_crlf_terminators() {
        let before =
            "## Pomodoros\r\n- [ ] () — JOB\r\n\t- keep this child\r\n";
        let (after, tails) =
            rewrite_file_contents(before, "job", "WRITE").expect("rewrite");
        assert_eq!(
            after,
            "## Pomodoros\r\n- [ ] () — WRITE\r\n\t- keep this child\r\n"
        );
        assert_eq!(tails.len(), 1);
        assert_eq!(tails[0].line, 2);
    }

    #[test]
    fn rewrite_preserves_mixed_endings_blank_and_trailing_space() {
        let before = "## Pomodoros\n\n- [ ] () — JOB \r\n- [ ] () — KEEP\ntrailing  \nlast without newline";
        let (after, tails) =
            rewrite_file_contents(before, "job", "WRITE").expect("rewrite");
        assert_eq!(
            after,
            "## Pomodoros\n\n- [ ] () — WRITE \r\n- [ ] () — KEEP\ntrailing  \nlast without newline"
        );
        assert_eq!(tails.len(), 1);
        // No final newline stays absent; blank line and trailing
        // spaces survive untouched.
        assert!(!after.ends_with('\n'));
        assert!(after.contains("\n\n"));
        assert!(after.contains("trailing  \n"));
    }

    #[test]
    fn rewrite_preserves_no_final_newline_on_changed_line() {
        let before = "## Pomodoros\n- [ ] () — JOB";
        let (after, _) =
            rewrite_file_contents(before, "job", "WRITE").expect("rewrite");
        assert_eq!(after, "## Pomodoros\n- [ ] () — WRITE");
    }

    #[test]
    fn rewrite_preserves_untouched_heading_and_child_rows() {
        let before = "## Pomodoros\r\n- [ ] () — JOB\r\n\t- keep this child\r\n- [ ] () — JOB + KEEP\r\n- [ ] () — KEEP\r\n";
        let (after, tails) =
            rewrite_file_contents(before, "job", "WRITE").expect("rewrite");
        assert_eq!(
            after,
            "## Pomodoros\r\n- [ ] () — WRITE\r\n\t- keep this child\r\n- [ ] () — WRITE + KEEP\r\n- [ ] () — KEEP\r\n"
        );
        // Two selected entries rewritten; heading, child, and the
        // KEEP-only row keep their exact bytes.
        assert_eq!(tails.len(), 2);
        assert!(after.contains("## Pomodoros\r\n"));
        assert!(after.contains("\t- keep this child\r\n"));
        assert!(after.contains("- [ ] () — KEEP\r\n"));
    }

    #[test]
    fn raw_lines_round_trip_every_terminator() {
        for contents in ["", "\n", "a\r\nb\n\nc", "a\n", "x\r\n\r\n"] {
            assert_eq!(raw_lines_preserving(contents).concat(), contents);
        }
        assert_eq!(raw_lines_preserving("a\r\nb"), ["a\r\n", "b"]);
    }

    #[test]
    fn raw_tail_splice_names_the_racing_action() {
        let before = "## Pomodoros\r\n- [ ] () — JOB\r\n";
        let planned = "## Pomodoros\r\n- [ ] () — WRITE\r\n";
        let lines = BTreeSet::from([2]);
        assert_eq!(
            verify_raw_tail_splice(
                "n.md", before, planned, planned, &lines, "split"
            ),
            Ok(())
        );
        // A concurrent edit to an untouched line is a race, not a splice bug.
        let raced = "## Pomodoros!\r\n- [ ] () — WRITE\r\n";
        assert_eq!(
            verify_raw_tail_splice(
                "n.md", before, planned, raced, &lines, "split"
            ),
            Err("n.md changed during the split".to_string())
        );
        // Dropping the CR on the spliced line is collateral damage.
        let lf = "## Pomodoros\r\n- [ ] () — WRITE\n";
        assert_eq!(
            verify_raw_tail_splice("n.md", before, lf, lf, &lines, "rename"),
            Err("n.md changed outside a Pomodoro name tail".to_string())
        );
    }

    #[test]
    fn verify_rejects_collateral_terminator_rewrite() {
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
        let prepared = PreparedRename {
            vault: vault.to_path_buf(),
            old_key: "job".to_string(),
            old_name: "JOB".to_string(),
            new_key: "write".to_string(),
            new_name: "WRITE".to_string(),
            merge: false,
            via: "test".to_string(),
            files: vec![FilePlan {
                path: relative.clone(),
                entries: 1,
                tails: vec![TailChange {
                    line: 2,
                    before: "JOB".to_string(),
                    after: "WRITE".to_string(),
                }],
                before_contents: before.to_string(),
                after_contents: malformed.to_string(),
            }],
            total_before_entries: 1,
            warnings: Vec::new(),
            sync_pre: "skipped",
            hidden_before: false,
        };
        let error = verify_renamed(vault, &prepared).expect_err("must reject");
        assert!(
            error.contains("outside a Pomodoro name tail")
                || error.contains("line count"),
            "{error}"
        );
    }
}
