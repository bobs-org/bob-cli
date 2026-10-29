//! Previewed, verified, single-commit rewrite of legacy theme spellings.
//!
//! `bob types migrate` rewrites open-task `[theme::]` / `(theme::)` fields
//! to `[type::]` / `(type::)` and area/project `theme:` defaults to
//! `task_type:`, keeping value bytes and quote style.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};
use serde::Serialize;
use serde_json::json;

use crate::native::{
    capture_dependency_tasks, env as bob_env,
    markdown::split_line_ending,
    note_tasks, ob,
    pomodoro_homes::{NoteFacts, NoteKind, NoteTheme},
    projects::trim_yaml_scalar,
    task_fields::{canonical_theme_name, inline_fields},
    task_theme::RepairKind,
};

use super::{
    cache,
    rename::{
        bob_dir_arg, format_arg, help_arg, offline_arg, print_clap_error,
        print_failure, raw_lines_preserving, OutputFormat, RenameFailure,
    },
};

const COMMAND_NAME: &str = "bob types migrate";
const SCHEMA_VERSION: u32 = 1;
const TASK_FIELD_KEY: &str = "type";
const LEGACY_FIELD_KEY: &str = "theme";
const NOTE_DEFAULT_KEY: &str = "task_type";
const LEGACY_DEFAULT_KEY: &str = "theme";

type MigrateFailure = RenameFailure;

pub(crate) fn build_cli_migrate() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Rewrite legacy theme spellings to type in one commit")
        .long_about(
            "Rewrite open-task `[theme::]` / `(theme::)` fields to \
`[type::]` / `(type::)` and area/project `theme:` defaults to \
`task_type:` in a single verified Git commit.\n\n\
Each rewritten field keeps its bracket or paren form, spacing around `::`, \
and value bytes. Each rewritten default keeps its value bytes and quote \
style. When a note already has `task_type:`, a matching `theme:` line is \
removed; a different value is reported as `conflict` and left alone. Closed \
tasks, `done/`, repair-state lines, prose, fenced examples, `.bob/`, and \
`_conflicts/` are never touched. Repair lines are listed as `needs_repair`.\n\n\
The preview always rescans from source. The write takes bob_sync.lock, \
pre-syncs unless --offline, snapshots each target and compares again right \
before writing, writes atomically, verifies that every changed line differs \
only in the key span (or is a removed matching `theme:` default), makes one \
commit, then post-syncs. Undo is `git revert`. When nothing is legacy, the \
command refuses with `unchanged` and does not commit.\n\n\
Online writes require -b/--bob-dir and BOB_DIR (default ~/bob) to resolve to \
the same vault; a mismatch refuses before locks or sync. Set BOB_DIR to the \
selected vault or use --offline. Previews and offline writes do not require \
this match.",
        )
        .after_help(
            "Examples:\n  \
bob types migrate -d\n  \
bob types migrate -d -f json\n  \
bob types migrate -o\n\n\
Environment:\n  \
BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
BOB_VAULT_SYNC_LOCK_FILE  Maintenance lock; defaults to ${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock\n\n\
Exit codes:\n  \
0  migrated (or dry-run planned)\n  \
1  unchanged, lock busy, dirty paths, not a worktree, sync failed, verify failed, stale note, or I/O failure\n  \
2  usage error\n\n\
See docs/task-types.md for the migrate contract and revert runbook.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(offline_arg())
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Preview the rewrite without writing, committing, or syncing")
}

pub(crate) fn run_migrate(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_migrate();
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
                if prepared.files.is_empty() {
                    print_unchanged(&prepared.report("dry_run"), format)
                } else {
                    print_success(&prepared.report("dry_run"), format);
                    0
                }
            }
            Err(failure) => print_failure(COMMAND_NAME, format, failure),
        };
    }
    run_live(&matches, format)
}

#[derive(Debug, Clone, Serialize)]
struct FileView {
    path: String,
    fields: usize,
    defaults: usize,
}

#[derive(Debug, Clone, Serialize)]
struct RepairRow {
    path: String,
    line: usize,
    kind: RepairKind,
}

#[derive(Debug, Clone, Serialize)]
struct ConflictRow {
    path: String,
}

#[derive(Debug, Clone, Serialize)]
struct SyncInfo {
    pre: String,
    post: String,
}

#[derive(Debug, Clone, Serialize)]
struct MigrateReport {
    schema_version: u32,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
    mode: String,
    fields: usize,
    defaults: usize,
    files: Vec<FileView>,
    needs_repair: Vec<RepairRow>,
    conflicts: Vec<ConflictRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<serde_json::Value>,
    sync: SyncInfo,
}

#[derive(Debug, Clone)]
struct KeyEdit {
    line: usize,
    start: usize,
    end: usize,
    after: &'static str,
}

#[derive(Debug, Clone)]
struct FilePlan {
    path: String,
    fields: usize,
    defaults: usize,
    key_edits: Vec<KeyEdit>,
    removed_lines: Vec<usize>,
    before_contents: String,
    after_contents: String,
}

struct PreparedMigrate {
    files: Vec<FilePlan>,
    needs_repair: Vec<RepairRow>,
    conflicts: Vec<ConflictRow>,
    sync_pre: &'static str,
}

impl PreparedMigrate {
    fn report(&self, mode: &str) -> MigrateReport {
        let fields: usize = self.files.iter().map(|file| file.fields).sum();
        let defaults: usize = self.files.iter().map(|file| file.defaults).sum();
        MigrateReport {
            schema_version: SCHEMA_VERSION,
            ok: true,
            code: None,
            message: None,
            hint: None,
            mode: mode.to_string(),
            fields,
            defaults,
            files: self
                .files
                .iter()
                .map(|file| FileView {
                    path: file.path.clone(),
                    fields: file.fields,
                    defaults: file.defaults,
                })
                .collect(),
            needs_repair: self.needs_repair.clone(),
            conflicts: self.conflicts.clone(),
            commit: None,
            sync: SyncInfo {
                pre: self.sync_pre.to_string(),
                post: "skipped".to_string(),
            },
        }
    }
}

fn vault_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

fn prepare_from_matches(
    matches: &ArgMatches,
    dry_run: bool,
) -> Result<PreparedMigrate, MigrateFailure> {
    let vault = vault_from_matches(matches);
    prepare(&vault, dry_run)
}

fn prepare(
    vault: &Path,
    dry_run: bool,
) -> Result<PreparedMigrate, MigrateFailure> {
    let settings = note_tasks::read_settings(vault);
    let (paths, _) = capture_dependency_tasks::vault_note_paths(vault);
    let mut files = Vec::new();
    let mut needs_repair = Vec::new();
    let mut conflicts = Vec::new();
    for relative in paths {
        if is_done_archive(&relative) {
            continue;
        }
        let path = capture_dependency_tasks::display_path(&relative);
        let absolute = vault.join(&relative);
        let contents = fs::read_to_string(&absolute).map_err(|error| {
            MigrateFailure::new(
                "io",
                format!("could not read {path}: {error}"),
                None::<String>,
            )
        })?;
        let planned = plan_file(&path, &contents, &settings);
        needs_repair.extend(planned.needs_repair);
        if planned.conflict {
            conflicts.push(ConflictRow { path: path.clone() });
        }
        if planned.fields == 0 && planned.defaults == 0 {
            continue;
        }
        files.push(FilePlan {
            path,
            fields: planned.fields,
            defaults: planned.defaults,
            key_edits: planned.key_edits,
            removed_lines: planned.removed_lines,
            before_contents: contents,
            after_contents: planned.after_contents,
        });
    }
    Ok(PreparedMigrate {
        files,
        needs_repair,
        conflicts,
        sync_pre: if dry_run { "skipped" } else { "ok" },
    })
}

struct PlannedFile {
    fields: usize,
    defaults: usize,
    key_edits: Vec<KeyEdit>,
    removed_lines: Vec<usize>,
    after_contents: String,
    needs_repair: Vec<RepairRow>,
    conflict: bool,
}

fn plan_file(
    path: &str,
    contents: &str,
    settings: &note_tasks::NoteTaskSettings,
) -> PlannedFile {
    let mut key_edits = Vec::new();
    let mut removed_lines = Vec::new();
    let mut needs_repair = Vec::new();
    let mut fields = 0usize;
    let scan = note_tasks::scan(contents, settings);
    let owned = raw_lines_preserving(contents);
    for task in scan.tasks() {
        if !task.status_type.is_open() {
            continue;
        }
        let Some(raw) = owned.get(task.line_index) else {
            continue;
        };
        let (body, _) = split_line_ending(raw);
        let mut found = inline_fields(body, TASK_FIELD_KEY);
        found.extend(inline_fields(body, LEGACY_FIELD_KEY));
        found.sort_by_key(|field| field.start);
        let line = task.line_index + 1;
        match found.as_slice() {
            [] => {}
            [field] => {
                if canonical_theme_name(&field.value).is_none() {
                    needs_repair.push(RepairRow {
                        path: path.to_string(),
                        line,
                        kind: RepairKind::Invalid,
                    });
                    continue;
                }
                if field.key != LEGACY_FIELD_KEY {
                    continue;
                }
                let Some(edit) = field_key_edit(field) else {
                    continue;
                };
                key_edits.push(KeyEdit {
                    line,
                    start: edit.0,
                    end: edit.1,
                    after: TASK_FIELD_KEY,
                });
                fields += 1;
            }
            _ => needs_repair.push(RepairRow {
                path: path.to_string(),
                line,
                kind: RepairKind::Duplicate,
            }),
        }
    }

    let default_plan = plan_default(contents);
    let mut defaults = 0usize;
    let mut conflict = false;
    match default_plan {
        DefaultPlan::Rewrite(edit) => {
            key_edits.push(edit);
            defaults = 1;
        }
        DefaultPlan::Remove(line) => {
            removed_lines.push(line);
            defaults = 1;
        }
        DefaultPlan::Conflict => conflict = true,
        DefaultPlan::None => {}
    }

    key_edits.sort_by_key(|edit| (edit.line, edit.start));
    removed_lines.sort_unstable();
    let after_contents = apply_edits(contents, &key_edits, &removed_lines);
    PlannedFile {
        fields,
        defaults,
        key_edits,
        removed_lines,
        after_contents,
        needs_repair,
        conflict,
    }
}

enum DefaultPlan {
    None,
    Rewrite(KeyEdit),
    Remove(usize),
    Conflict,
}

fn plan_default(contents: &str) -> DefaultPlan {
    let facts = NoteFacts::parse(contents);
    if !matches!(facts.kind, NoteKind::Area | NoteKind::Project) {
        return DefaultPlan::None;
    }
    let mut task_type: Option<(usize, Option<String>)> = None;
    let mut theme = None;
    for (index, raw) in raw_lines_preserving(contents).iter().enumerate() {
        if index == 0 {
            continue;
        }
        let (body, _) = split_line_ending(raw);
        if body.trim() == "---" {
            break;
        }
        if let Some((_, end)) = yaml_key_span(body, NOTE_DEFAULT_KEY) {
            task_type = Some((index + 1, scalar_after_key(body, end)));
        }
        if let Some((start, end)) = yaml_key_span(body, LEGACY_DEFAULT_KEY) {
            theme = Some((index + 1, start, end, scalar_after_key(body, end)));
        }
    }
    match (task_type, theme) {
        (_, None) => DefaultPlan::None,
        (None, Some((line, start, end, scalar))) => {
            if scalar.as_deref().is_some_and(is_block_indicator) {
                return DefaultPlan::None;
            }
            DefaultPlan::Rewrite(KeyEdit {
                line,
                start,
                end,
                after: NOTE_DEFAULT_KEY,
            })
        }
        (Some((_, canonical_raw)), Some((line, _, _, legacy_raw))) => {
            let canonical =
                canonical_raw.as_deref().and_then(parse_default_theme);
            let legacy = legacy_raw.as_deref().and_then(parse_default_theme);
            if canonical.is_some() && canonical == legacy {
                DefaultPlan::Remove(line)
            } else {
                DefaultPlan::Conflict
            }
        }
    }
}

fn parse_default_theme(raw: &str) -> Option<NoteTheme> {
    if is_block_indicator(raw) {
        return None;
    }
    let trimmed = trim_yaml_scalar(raw);
    if trimmed.eq_ignore_ascii_case("off") {
        return Some(NoteTheme::Off);
    }
    match canonical_theme_name(trimmed) {
        Some(name) => Some(NoteTheme::Name(name)),
        None => Some(NoteTheme::Invalid(trimmed.to_string())),
    }
}

fn yaml_key_span(body: &str, key: &str) -> Option<(usize, usize)> {
    let start = body.len() - body.trim_start().len();
    let trimmed = &body[start..];
    let rest = trimmed.strip_prefix(key)?;
    rest.starts_with(':').then_some((start, start + key.len()))
}

fn scalar_after_key(body: &str, key_end: usize) -> Option<String> {
    let rest = body.get(key_end..)?.strip_prefix(':')?;
    Some(rest.trim().to_string())
}

fn is_block_indicator(scalar: &str) -> bool {
    scalar.is_empty()
        || scalar.starts_with('|')
        || scalar.starts_with('>')
        || scalar.starts_with('[')
        || scalar.starts_with('{')
}

fn field_key_edit(
    field: &crate::native::task_fields::InlineField,
) -> Option<(usize, usize)> {
    let start = field.start + 1;
    let end = start + field.key.len();
    (end <= field.end && field.key == LEGACY_FIELD_KEY).then_some((start, end))
}

fn apply_edits(
    contents: &str,
    key_edits: &[KeyEdit],
    removed_lines: &[usize],
) -> String {
    if key_edits.is_empty() && removed_lines.is_empty() {
        return contents.to_string();
    }
    let mut owned = raw_lines_preserving(contents);
    let mut by_line: Vec<(usize, Vec<&KeyEdit>)> = Vec::new();
    for edit in key_edits {
        match by_line.iter_mut().find(|(line, _)| *line == edit.line) {
            Some((_, group)) => group.push(edit),
            None => by_line.push((edit.line, vec![edit])),
        }
    }
    for (line, mut group) in by_line {
        let index = line.saturating_sub(1);
        let Some(raw) = owned.get(index).cloned() else {
            continue;
        };
        let (body, ending) = split_line_ending(&raw);
        group.sort_by_key(|edit| std::cmp::Reverse(edit.start));
        let mut rewritten = body.to_string();
        for edit in group {
            if edit.end > rewritten.len() || edit.start > edit.end {
                continue;
            }
            rewritten.replace_range(edit.start..edit.end, edit.after);
        }
        owned[index] = format!("{rewritten}{ending}");
    }
    for line in removed_lines.iter().copied().rev() {
        let index = line.saturating_sub(1);
        if index < owned.len() {
            owned.remove(index);
        }
    }
    owned.concat()
}

fn is_done_archive(relative: &Path) -> bool {
    relative
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == "done")
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
        "task type migrations",
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
            if prepared.files.is_empty() {
                return Err(crate::native::vault_rewrite::TxnFailure::new(
                    "unchanged",
                    unchanged_message(&prepared),
                    Some(unchanged_hint()),
                ));
            }
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
            let fields: usize =
                inner.files.iter().map(|file| file.fields).sum();
            let defaults: usize =
                inner.files.iter().map(|file| file.defaults).sum();
            let subject = commit_subject(fields, defaults);
            let verify_inner = Rc::clone(&inner);
            let verify_vault = vault.clone();
            let invalidate_vault = vault.clone();
            Ok(crate::native::vault_rewrite::ReadyPlan {
                files,
                subject,
                post_write: Box::new(|| Ok(Vec::new())),
                verify: Box::new(move || {
                    verify_migrated(&verify_vault, &verify_inner)
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
            report.sync.post = outcome.sync_post.to_string();
            print_success(&report, format);
            0
        }
        Err(failure) => {
            if failure.code == "unchanged" {
                match prepare_from_matches(matches, false) {
                    Ok(prepared) => {
                        print_unchanged(&prepared.report("write"), format)
                    }
                    Err(_) => print_failure(
                        COMMAND_NAME,
                        format,
                        MigrateFailure::new(
                            failure.code,
                            failure.message,
                            failure.hint,
                        ),
                    ),
                }
            } else {
                print_failure(
                    COMMAND_NAME,
                    format,
                    MigrateFailure::new(
                        failure.code,
                        failure.message,
                        failure.hint,
                    ),
                )
            }
        }
    }
}

fn commit_subject(fields: usize, defaults: usize) -> String {
    format!("bob types migrate: {fields} fields, {defaults} defaults")
}

fn unchanged_message(prepared: &PreparedMigrate) -> String {
    match (prepared.needs_repair.len(), prepared.conflicts.len()) {
        (0, 0) => {
            "no legacy theme fields or defaults to migrate".to_string()
        }
        _ => {
            "no legacy theme fields or defaults to migrate; repair and conflict rows were left alone".to_string()
        }
    }
}

fn unchanged_hint() -> &'static str {
    "open-task fields already use [type::] / (type::) and note defaults already use task_type:, or remaining rows need repair"
}

fn verify_migrated(
    vault: &Path,
    prepared: &PreparedMigrate,
) -> Result<(), String> {
    let settings = note_tasks::read_settings(vault);
    let mut verified = BTreeSet::new();
    for planned in &prepared.files {
        let absolute = vault.join(&planned.path);
        let contents = fs::read_to_string(&absolute)
            .map_err(|error| format!("reread {}: {error}", planned.path))?;
        let replanned =
            plan_file(&planned.path, &planned.before_contents, &settings);
        if replanned.after_contents != planned.after_contents {
            return Err(format!(
                "{} changed outside a type key span",
                planned.path
            ));
        }
        if contents != planned.after_contents {
            return Err(format!("{} changed during the migrate", planned.path));
        }
        verify_key_span_only(planned)?;
        let leftover = plan_file(&planned.path, &contents, &settings);
        if leftover.fields > 0 || leftover.defaults > 0 {
            return Err(format!(
                "{} still contains a legacy theme spelling",
                planned.path
            ));
        }
        verified.insert(planned.path.as_str());
    }
    if prepared
        .files
        .iter()
        .any(|planned| !verified.contains(planned.path.as_str()))
    {
        return Err(
            "a planned migrate file is missing after the migrate".into()
        );
    }
    Ok(())
}

fn verify_key_span_only(planned: &FilePlan) -> Result<(), String> {
    let before = raw_lines_preserving(&planned.before_contents);
    let after = raw_lines_preserving(&planned.after_contents);
    let removed: BTreeSet<usize> =
        planned.removed_lines.iter().copied().collect();
    let expected_len = before.len().saturating_sub(removed.len());
    if after.len() != expected_len {
        return Err(format!("{} line count changed", planned.path));
    }
    for (index, before_raw) in before.iter().enumerate() {
        let line = index + 1;
        if removed.contains(&line) {
            let (body, _) = split_line_ending(before_raw);
            if yaml_key_span(body, LEGACY_DEFAULT_KEY).is_none() {
                return Err(format!(
                    "{} removed a line that is not a legacy theme default",
                    planned.path
                ));
            }
            continue;
        }
        let after_index = index
            - removed
                .iter()
                .filter(|removed_line| **removed_line < line)
                .count();
        let Some(after_raw) = after.get(after_index) else {
            return Err(format!(
                "{} is missing a rewritten line",
                planned.path
            ));
        };
        let mut edits: Vec<&KeyEdit> = planned
            .key_edits
            .iter()
            .filter(|edit| edit.line == line)
            .collect();
        if edits.is_empty() {
            if before_raw != after_raw {
                return Err(format!(
                    "{} changed outside a type key span",
                    planned.path
                ));
            }
            continue;
        }
        let (body, ending) = split_line_ending(before_raw);
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.start));
        let mut rewritten = body.to_string();
        for edit in edits {
            if body.get(edit.start..edit.end) != Some(LEGACY_FIELD_KEY) {
                return Err(format!(
                    "{} rewritten key span is not a legacy theme key",
                    planned.path
                ));
            }
            rewritten.replace_range(edit.start..edit.end, edit.after);
        }
        if format!("{rewritten}{ending}") != *after_raw {
            return Err(format!(
                "{} changed outside a type key span",
                planned.path
            ));
        }
    }
    Ok(())
}

fn print_success(report: &MigrateReport, format: OutputFormat) {
    match format {
        OutputFormat::Json => print_json(report),
        OutputFormat::Human => print_human(report),
    }
}

fn print_unchanged(report: &MigrateReport, format: OutputFormat) -> i32 {
    let mut refused = report.clone();
    refused.ok = false;
    refused.code = Some("unchanged");
    refused.message =
        Some("no legacy theme fields or defaults to migrate".to_string());
    refused.hint = Some(unchanged_hint().to_string());
    match format {
        OutputFormat::Json => print_json(&refused),
        OutputFormat::Human => {
            eprintln!(
                "{COMMAND_NAME}: {}",
                refused.message.as_deref().unwrap()
            );
            eprintln!("hint: {}", unchanged_hint());
            print_repair_and_conflict_human(&refused);
        }
    }
    1
}

fn print_json(report: &MigrateReport) {
    match serde_json::to_string(report) {
        Ok(text) => println!("{text}"),
        Err(error) => {
            eprintln!("{COMMAND_NAME}: failed to serialize JSON: {error}");
        }
    }
}

fn print_human(report: &MigrateReport) {
    let styler = crate::native::style::Styler::detect();
    let mode_suffix = if report.mode == "dry_run" {
        " (dry run)"
    } else {
        ""
    };
    println!(
        "{} migrate {} field{}, {} default{}{}",
        styler.success_prefix(report.mode == "dry_run"),
        report.fields,
        if report.fields == 1 { "" } else { "s" },
        report.defaults,
        if report.defaults == 1 { "" } else { "s" },
        mode_suffix,
    );
    if report.files.is_empty() {
        println!("  no files");
    }
    for file in &report.files {
        let mut bits = Vec::new();
        if file.fields > 0 {
            bits.push(format!(
                "{} field{}",
                file.fields,
                if file.fields == 1 { "" } else { "s" },
            ));
        }
        if file.defaults > 0 {
            bits.push(format!(
                "{} default{}",
                file.defaults,
                if file.defaults == 1 { "" } else { "s" },
            ));
        }
        println!("  {}: {}", file.path, bits.join(", "));
    }
    print_repair_and_conflict_human(report);
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
}

fn print_repair_and_conflict_human(report: &MigrateReport) {
    if !report.needs_repair.is_empty() {
        println!("  needs repair: {}", report.needs_repair.len());
        for row in &report.needs_repair {
            let kind = match row.kind {
                RepairKind::Duplicate => "duplicate",
                RepairKind::Invalid => "invalid",
            };
            println!("    {}:{} {kind}", row.path, row.line);
        }
    }
    if !report.conflicts.is_empty() {
        println!("  conflicts: {}", report.conflicts.len());
        for row in &report.conflicts {
            println!("    {}", row.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::note_tasks::NoteTaskSettings;
    use std::path::Path;

    fn settings() -> NoteTaskSettings {
        note_tasks::read_settings(Path::new("/nonexistent"))
    }

    fn plan(contents: &str) -> PlannedFile {
        plan_file("note.md", contents, &settings())
    }

    #[test]
    fn open_legacy_fields_become_type_and_keep_value_bytes() {
        let contents = "\
---\ntype: [[area]]\n---\n\
- [ ] #task Open [theme:: BUILD] ^open\n\
- [ ] #task Paren (theme::  WRITE) ^paren\n\
- [x] #task Done [theme:: BUILD] ^done\n\
Prose [theme:: BUILD] stays.\n\
```md\n\
- [ ] #task Fenced [theme:: BUILD]\n\
```\n";
        let planned = plan(contents);
        assert_eq!(planned.fields, 2);
        assert_eq!(planned.defaults, 0);
        assert!(planned.needs_repair.is_empty());
        assert!(planned.after_contents.contains("[type:: BUILD]"));
        assert!(planned.after_contents.contains("(type::  WRITE)"));
        assert!(planned
            .after_contents
            .contains("- [x] #task Done [theme:: BUILD]"));
        assert!(planned
            .after_contents
            .contains("Prose [theme:: BUILD] stays."));
        assert!(planned
            .after_contents
            .contains("- [ ] #task Fenced [theme:: BUILD]"));
    }

    #[test]
    fn canonical_fields_are_left_alone() {
        let contents =
            "---\ntype: [[area]]\n---\n- [ ] #task Open [type:: BUILD] ^open\n";
        let planned = plan(contents);
        assert_eq!(planned.fields, 0);
        assert_eq!(planned.after_contents, contents);
    }

    #[test]
    fn repair_lines_are_listed_and_not_rewritten() {
        let duplicate = "\
---\ntype: [[area]]\n---\n\
- [ ] #task Mixed [type:: BUILD] (theme:: BUILD) ^mixed\n";
        let planned = plan(duplicate);
        assert_eq!(planned.fields, 0);
        assert_eq!(planned.needs_repair.len(), 1);
        assert_eq!(planned.needs_repair[0].kind, RepairKind::Duplicate);
        assert_eq!(planned.after_contents, duplicate);

        let invalid = "\
---\ntype: [[area]]\n---\n\
- [ ] #task Bad [theme:: BUILD + FIX] ^bad\n";
        let planned = plan(invalid);
        assert_eq!(planned.fields, 0);
        assert_eq!(planned.needs_repair.len(), 1);
        assert_eq!(planned.needs_repair[0].kind, RepairKind::Invalid);
        assert_eq!(planned.after_contents, invalid);
    }

    #[test]
    fn note_default_rewrites_key_and_keeps_quotes() {
        let unquoted = "---\ntype: [[area]]\ntheme: BUILD\n---\n";
        let planned = plan(unquoted);
        assert_eq!(planned.defaults, 1);
        assert_eq!(
            planned.after_contents,
            "---\ntype: [[area]]\ntask_type: BUILD\n---\n"
        );
        let quoted = "---\ntype: [[project]]\ntheme: \"BUILD\"\n---\n";
        let planned = plan(quoted);
        assert_eq!(
            planned.after_contents,
            "---\ntype: [[project]]\ntask_type: \"BUILD\"\n---\n"
        );
        let off = "---\ntype: [[area]]\ntheme: off\n---\n";
        let planned = plan(off);
        assert_eq!(
            planned.after_contents,
            "---\ntype: [[area]]\ntask_type: off\n---\n"
        );
    }

    #[test]
    fn matching_task_type_removes_legacy_default() {
        let contents =
            "---\ntype: [[area]]\ntask_type: BUILD\ntheme: BUILD\n---\n";
        let planned = plan(contents);
        assert_eq!(planned.defaults, 1);
        assert!(!planned.conflict);
        assert_eq!(
            planned.after_contents,
            "---\ntype: [[area]]\ntask_type: BUILD\n---\n"
        );
    }

    #[test]
    fn conflicting_defaults_are_left_alone() {
        let contents =
            "---\ntype: [[area]]\ntask_type: BUILD\ntheme: FIX\n---\n";
        let planned = plan(contents);
        assert!(planned.conflict);
        assert_eq!(planned.defaults, 0);
        assert_eq!(planned.after_contents, contents);
        let blank = "---\ntype: [[area]]\ntask_type:\ntheme: BUILD\n---\n";
        let planned = plan(blank);
        assert!(planned.conflict);
        assert_eq!(planned.after_contents, blank);
    }

    #[test]
    fn other_note_kinds_are_not_defaults() {
        let contents = "---\ntype: [[ref]]\ntheme: BUILD\n---\n";
        let planned = plan(contents);
        assert_eq!(planned.defaults, 0);
        assert_eq!(planned.after_contents, contents);
    }

    #[test]
    fn key_span_verify_accepts_field_rewrite_and_default_removal() {
        let before = "\
---\ntype: [[area]]\ntask_type: BUILD\ntheme: BUILD\n---\n\
- [ ] #task Open [theme:: BUILD] ^open\n";
        let planned = plan(before);
        let file = FilePlan {
            path: "note.md".into(),
            fields: planned.fields,
            defaults: planned.defaults,
            key_edits: planned.key_edits,
            removed_lines: planned.removed_lines,
            before_contents: before.to_string(),
            after_contents: planned.after_contents,
        };
        verify_key_span_only(&file).expect("key span");
    }
}
