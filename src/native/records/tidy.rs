//! `bob records tidy`: remove empty `## Notes` sections from record notes.

use std::{
    cell::RefCell,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde_json::{json, Value};
use similar::TextDiff;

use super::notes_section::strip_empty_notes_sections;
use crate::native::{
    albums::registry::{AlbumRegistry, DiscoveryHealth},
    host_catalog::{
        self, apply_and_commit, CommitInfo, FileEdit, PlannedTxn,
        ReadDependencies, TxnResult,
    },
    notebooks::snapshot::VaultSnapshot,
    ob::{self, ChildEnv},
    projects::{frontmatter_type_names, parse_frontmatter},
    style::Styler,
    vault_rewrite::TxnFailure,
};

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub(crate) struct ChangedFile {
    pub(crate) path: String,
    pub(crate) before: String,
    pub(crate) after: String,
}

impl ChangedFile {
    fn diff(&self) -> String {
        TextDiff::from_lines(&self.before, &self.after)
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{}", self.path), &format!("b/{}", self.path))
            .to_string()
    }
}

#[derive(Debug, Clone)]
struct TidyPlan {
    scanned: usize,
    changes: Vec<ChangedFile>,
    edits: Vec<FileEdit>,
    dependencies: ReadDependencies,
    io_errors: Vec<String>,
}

#[derive(Debug, Clone)]
struct TidyContext {
    scanned: usize,
    changes: Vec<ChangedFile>,
    io_errors: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct TidyReport {
    dry_run: bool,
    scanned: usize,
    changes: Vec<ChangedFile>,
    commit: Option<CommitInfo>,
    sync_pre: Option<&'static str>,
    sync_post: Option<&'static str>,
    warnings: Vec<String>,
    io_errors: Vec<String>,
    failure: Option<TxnFailure>,
}

impl TidyReport {
    fn failed(dry_run: bool, failure: TxnFailure) -> Self {
        Self {
            dry_run,
            scanned: 0,
            changes: Vec::new(),
            commit: None,
            sync_pre: None,
            sync_post: None,
            warnings: Vec::new(),
            io_errors: Vec::new(),
            failure: Some(failure),
        }
    }

    fn ok(&self) -> bool {
        self.failure.is_none() && self.io_errors.is_empty()
    }

    pub(crate) fn exit_code(&self) -> i32 {
        i32::from(!self.ok())
    }

    pub(crate) fn to_json(&self) -> Value {
        let mut payload = json!({
            "schema_version": SCHEMA_VERSION,
            "ok": self.ok(),
            "command": "tidy",
            "dry_run": self.dry_run,
            "counts": {
                "scanned": self.scanned,
                "changed": self.changes.len(),
            },
            "changes": self.changes.iter().map(|change| {
                let mut item = json!({"path": change.path});
                if self.dry_run {
                    item["diff"] = Value::String(change.diff());
                }
                item
            }).collect::<Vec<_>>(),
            "commit": match &self.commit {
                Some(commit) => json!({"sha": commit.sha, "subject": commit.subject}),
                None => Value::Null,
            },
            "sync": match (self.sync_pre, self.sync_post) {
                (Some(pre), Some(post)) => json!({"pre": pre, "post": post}),
                _ => Value::Null,
            },
            "warnings": self.warnings,
            "io_errors": self.io_errors,
        });
        if let Some(failure) = &self.failure {
            payload["error"] = json!({
                "code": failure.code,
                "message": failure.message,
                "hint": failure.hint,
            });
        }
        payload
    }

    pub(crate) fn print_human(&self) {
        let styler = Styler::detect();
        if let Some(failure) = &self.failure {
            eprintln!("{} {}", styler.red("✗"), failure.message);
            if let Some(hint) = &failure.hint {
                eprintln!("  {}", styler.dim(hint));
            }
        }
        for error in &self.io_errors {
            eprintln!("{} {error}", styler.red("✗"));
        }
        if self.dry_run {
            for change in &self.changes {
                print!("{}", color_diff(&change.diff(), &styler));
            }
            if self.changes.is_empty()
                && self.failure.is_none()
                && self.io_errors.is_empty()
            {
                println!(
                    "{} records are tidy: {} scanned, 0 tidied",
                    styler.green("✓"),
                    self.scanned
                );
            } else if !self.changes.is_empty() {
                println!(
                    "{} dry run: {} {} would change; nothing was written",
                    styler.yellow("!"),
                    self.changes.len(),
                    if self.changes.len() == 1 {
                        "note"
                    } else {
                        "notes"
                    }
                );
            }
            return;
        }
        if self.failure.is_some() {
            return;
        }
        match &self.commit {
            Some(commit) => {
                println!(
                    "{} tidied {} {} ({}): {} scanned",
                    styler.green("✓"),
                    self.changes.len(),
                    if self.changes.len() == 1 {
                        "note"
                    } else {
                        "notes"
                    },
                    &commit.sha[..commit.sha.len().min(9)],
                    self.scanned
                );
                for change in &self.changes {
                    println!("  {}", change.path);
                }
            }
            None => {
                if self.io_errors.is_empty() {
                    println!(
                        "{} records are tidy: {} scanned, 0 tidied",
                        styler.green("✓"),
                        self.scanned
                    );
                }
            }
        }
        for warning in &self.warnings {
            println!("{} {warning}", styler.yellow("!"));
        }
    }
}

fn color_diff(diff: &str, styler: &Styler) -> String {
    let mut out = String::new();
    for line in diff.lines() {
        let painted = if line.starts_with("+++") || line.starts_with("---") {
            styler.bold(line)
        } else if line.starts_with('+') {
            styler.green(line)
        } else if line.starts_with('-') {
            styler.red(line)
        } else if line.starts_with("@@") {
            styler.cyan(line)
        } else {
            line.to_owned()
        };
        out.push_str(&painted);
        out.push('\n');
    }
    out
}

pub(crate) fn build_cli_tidy() -> ClapCommand {
    ClapCommand::new("bob records tidy")
        .about("Remove empty `## Notes` sections from Record Notes")
        .long_about(
            "Remove every empty level-2 `## Notes` section from record notes: a Notes section is empty when every line after its heading until the next level-1 or level-2 heading (or EOF) is blank. Any other content keeps the section. Only record bodies change. A real run follows the shared maintenance lifecycle: it takes the shared vault lock, pre-syncs, guards each rewritten note by its full preimage, verifies that no rewritten note keeps an empty Notes section, makes one scoped `records: tidy (N notes)` commit when any note changed, and post-syncs only when it committed. TYPE accepts a noun or album stem, as `bob records check` does.",
        )
        .after_help(
            "Examples:\n  bob records tidy\n  bob records tidy -d\n  bob records tidy eat -d\n  bob records tidy -f json\n\nSee docs/records.md for the Notes rule and lifecycle.",
        )
        .disable_help_flag(true)
        .arg(record_type_arg())
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
}

fn record_type_arg() -> Arg {
    Arg::new("type")
        .value_name("TYPE")
        .required(false)
        .value_hint(clap::ValueHint::Other)
        .help("Record noun or album stem")
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .short('b')
        .long("bob-dir")
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .short('d')
        .long("dry-run")
        .action(ArgAction::SetTrue)
        .help("Print planned unified diffs; write nothing")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .short('f')
        .long("format")
        .value_name("FORMAT")
        .default_value("human")
        .value_parser(["human", "json"])
        .help("Output format: human or json")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .short('h')
        .long("help")
        .action(ArgAction::Help)
        .help("Show help")
}

pub(crate) fn run_tidy(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_tidy();
    let name = command.get_name().to_owned();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(name)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => {
            let code = error.exit_code();
            if let Err(print_error) = error.print()
                && print_error.kind() != io::ErrorKind::BrokenPipe
            {
                eprintln!(
                    "bob records tidy: failed to print command-line error: {print_error}"
                );
            }
            return code;
        }
    };
    execute_tidy(&matches)
}

fn format_from(matches: &ArgMatches) -> bool {
    matches
        .get_one::<String>("format")
        .is_some_and(|format| format == "json")
}

fn vault_from(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .unwrap_or_else(crate::native::env::bob_dir)
}

fn execute_tidy(matches: &ArgMatches) -> i32 {
    let json_mode = format_from(matches);
    let vault = vault_from(matches);
    let selector = matches.get_one::<String>("type").map(String::as_str);
    let dry_run = matches.get_flag("dry-run");

    if dry_run {
        let report = dry_run_report(&vault, selector);
        if report
            .failure
            .as_ref()
            .is_some_and(|failure| failure.code == "unknown_type")
        {
            print_report(&report, json_mode);
            return 2;
        }
        print_report(&report, json_mode);
        return report.exit_code();
    }

    // Usage errors precede the lock lifecycle so an unknown TYPE exits 2
    // even when vault-sync would fail.
    if let Some(wanted) = selector {
        let snapshot = VaultSnapshot::load(&vault);
        let registry = AlbumRegistry::from_snapshot(&vault, &snapshot);
        if !matches!(registry.health, DiscoveryHealth::Partial(_))
            && let Err(failure) = check_selector(&registry, Some(wanted))
        {
            let report = TidyReport::failed(false, failure);
            print_report(&report, json_mode);
            return 2;
        }
    }

    // A real run takes the shared lock and pre/post-syncs around one
    // guarded transaction, mirroring `bob albums sync` standalone runs.
    let child_env = ob::child_env();
    let captured: RefCell<Option<TidyContext>> = RefCell::new(None);
    let selector_owned = selector.map(str::to_owned);
    let vault_clone = vault.clone();
    let result = host_catalog::run_locked(
        &vault,
        &child_env,
        json_mode,
        "record",
        || {
            let snapshot = VaultSnapshot::load(&vault_clone);
            let registry =
                AlbumRegistry::from_snapshot(&vault_clone, &snapshot);
            let plan = plan_tidy(
                &vault_clone,
                &registry,
                &snapshot,
                selector_owned.as_deref(),
            )?;
            *captured.borrow_mut() = Some(TidyContext {
                scanned: plan.scanned,
                changes: plan.changes.clone(),
                io_errors: plan.io_errors.clone(),
            });
            Ok(planned_txn(&vault_clone, plan))
        },
    );
    let report = match result {
        Ok(result) => report_of(false, result),
        Err(failure) => {
            let mut report = TidyReport::failed(false, failure);
            if let Some(context) = captured.into_inner() {
                report.scanned = context.scanned;
                report.changes = context.changes;
                report.io_errors = context.io_errors;
            }
            // A usage error for an unknown TYPE is reported as exit 2 by
            // the planner; surface it without a transaction failure shape.
            report
        }
    };
    // Unknown TYPE is a usage error (exit 2), not a runtime failure.
    // The planner reports it through the failure channel; remap the code
    // here so the exit status matches `records check`.
    if report
        .failure
        .as_ref()
        .is_some_and(|failure| failure.code == "unknown_type")
    {
        print_report(&report, json_mode);
        return 2;
    }
    print_report(&report, json_mode);
    report.exit_code()
}

fn print_report(report: &TidyReport, json_mode: bool) {
    if json_mode {
        println!("{}", report.to_json());
    } else {
        report.print_human();
    }
}

/// Read-only dry run: no lock, no Git, no vault-sync.
fn dry_run_report(vault: &Path, selector: Option<&str>) -> TidyReport {
    let snapshot = VaultSnapshot::load(vault);
    let registry = AlbumRegistry::from_snapshot(vault, &snapshot);
    let plan = match plan_tidy(vault, &registry, &snapshot, selector) {
        Ok(plan) => plan,
        Err(failure) => {
            return TidyReport::failed(true, failure);
        }
    };
    TidyReport {
        dry_run: true,
        scanned: plan.scanned,
        changes: plan.changes,
        commit: None,
        sync_pre: None,
        sync_post: None,
        warnings: Vec::new(),
        io_errors: plan.io_errors,
        failure: None,
    }
}

fn report_of(dry_run: bool, result: TxnResult<TidyContext>) -> TidyReport {
    TidyReport {
        dry_run,
        scanned: result.context.scanned,
        changes: result.context.changes,
        commit: result.commit,
        sync_pre: Some(result.sync_pre),
        sync_post: Some(result.sync_post),
        warnings: Vec::new(),
        io_errors: result.context.io_errors,
        failure: None,
    }
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn unknown_type_failure(wanted: &str) -> TxnFailure {
    TxnFailure::new(
        "unknown_type",
        format!("unknown record type `{wanted}`"),
        Some(
            "use a record noun or album stem from `bob records list`"
                .to_string(),
        ),
    )
}

fn check_selector(
    registry: &AlbumRegistry,
    selector: Option<&str>,
) -> Result<(), TxnFailure> {
    let Some(wanted) = selector else {
        return Ok(());
    };
    match registry.resolve_route(wanted) {
        crate::native::albums::registry::RouteResolution::AlbumValid {
            record_type,
            ..
        } => {
            let path = match &record_type.source {
                crate::native::albums::registry::RecordTypeSource::Album {
                    path,
                } => path,
            };
            if registry
                .albums
                .iter()
                .any(|entry| entry.declaration.path == *path)
            {
                Ok(())
            } else {
                Err(unknown_type_failure(wanted))
            }
        }
        crate::native::albums::registry::RouteResolution::AlbumInvalid {
            paths,
            ..
        } => {
            if paths.len() == 1
                && registry
                    .albums
                    .iter()
                    .any(|entry| entry.declaration.path == paths[0])
            {
                Ok(())
            } else {
                Err(unknown_type_failure(wanted))
            }
        }
        crate::native::albums::registry::RouteResolution::Unclaimed {
            ..
        } => Err(unknown_type_failure(wanted)),
        crate::native::albums::registry::RouteResolution::DiscoveryIncomplete {
            errors,
        } => {
            let first =
                errors.first().map(String::as_str).unwrap_or("unknown I/O error");
            Err(TxnFailure::new(
                "io",
                format!("the vault scan was incomplete: {first}"),
                Some(
                    "fix unreadable vault paths, then retry; nothing was written"
                        .to_string(),
                ),
            ))
        }
    }
}

fn plan_tidy(
    vault: &Path,
    registry: &AlbumRegistry,
    snapshot: &VaultSnapshot,
    selector: Option<&str>,
) -> Result<TidyPlan, TxnFailure> {
    if let DiscoveryHealth::Partial(errors) = &registry.health {
        let first = errors
            .first()
            .map(String::as_str)
            .unwrap_or("unknown I/O error");
        let more = errors.len().saturating_sub(1);
        return Err(TxnFailure::new(
            "io",
            if more == 0 {
                format!("the vault scan was incomplete: {first}")
            } else {
                format!("the vault scan was incomplete: {first} (+{more} more)")
            },
            Some(
                "fix unreadable vault paths, then retry; nothing was written"
                    .to_string(),
            ),
        ));
    }

    let selected: Vec<&crate::native::albums::registry::AlbumEntry> =
        match selector {
            Some(wanted) => {
                match registry.resolve_route(wanted) {
                    crate::native::albums::registry::RouteResolution::AlbumValid {
                        record_type,
                        ..
                    } => {
                        let path = match &record_type.source {
                            crate::native::albums::registry::RecordTypeSource::Album {
                                path,
                            } => path.clone(),
                        };
                        let Some(entry) = registry
                            .albums
                            .iter()
                            .find(|entry| entry.declaration.path == path)
                        else {
                            return Err(TxnFailure::new(
                                "unknown_type",
                                format!("unknown record type `{wanted}`"),
                                Some(
                                    "use a record noun or album stem from `bob records list`"
                                        .to_string(),
                                ),
                            ));
                        };
                        vec![entry]
                    }
                    crate::native::albums::registry::RouteResolution::AlbumInvalid {
                        paths,
                        ..
                    } => {
                        if paths.len() == 1 {
                            let Some(entry) = registry
                                .albums
                                .iter()
                                .find(|entry| {
                                    entry.declaration.path == paths[0]
                                })
                            else {
                                return Err(TxnFailure::new(
                                    "unknown_type",
                                    format!(
                                        "unknown record type `{wanted}`"
                                    ),
                                    Some(
                                        "use a record noun or album stem from `bob records list`"
                                            .to_string(),
                                    ),
                                ));
                            };
                            vec![entry]
                        } else {
                            return Err(TxnFailure::new(
                                "unknown_type",
                                format!(
                                    "unknown record type `{wanted}`"
                                ),
                                Some(
                                    "use a record noun or album stem from `bob records list`"
                                        .to_string(),
                                ),
                            ));
                        }
                    }
                    crate::native::albums::registry::RouteResolution::Unclaimed {
                        ..
                    } => {
                        return Err(TxnFailure::new(
                            "unknown_type",
                            format!("unknown record type `{wanted}`"),
                            Some(
                                "use a record noun or album stem from `bob records list`"
                                    .to_string(),
                            ),
                        ));
                    }
                    crate::native::albums::registry::RouteResolution::DiscoveryIncomplete {
                        errors,
                    } => {
                        let first = errors.first().map(String::as_str).unwrap_or(
                            "unknown I/O error",
                        );
                        return Err(TxnFailure::new(
                            "io",
                            format!(
                                "the vault scan was incomplete: {first}"
                            ),
                            Some(
                                "fix unreadable vault paths, then retry; nothing was written"
                                    .to_string(),
                            ),
                        ));
                    }
                }
            }
            None => registry.albums.iter().collect(),
        };

    let mut scanned = 0usize;
    let mut changes = Vec::new();
    let mut edits = Vec::new();
    let mut dependencies = ReadDependencies::default();
    let mut io_errors = Vec::new();

    for album in selected {
        let folder = album.declaration.stem.clone();
        let mut accepted = vec!["record".to_string()];
        if let Some(record_type) = &album.declaration.record_type
            && !accepted.contains(&record_type.id)
        {
            accepted.push(record_type.id.clone());
        }
        if let Some(noun) = &album.declaration.noun_claim
            && !accepted.contains(noun)
        {
            accepted.push(noun.clone());
        }

        // Direct Markdown children of the album folder.
        let mut candidates: Vec<PathBuf> = snapshot
            .discovered
            .iter()
            .filter(|relative| {
                let Some(parent) = relative.parent() else {
                    return false;
                };
                if parent.to_string_lossy().replace('\\', "/") != folder {
                    return false;
                }
                relative
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
                    && relative
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .is_some_and(|stem| !stem.is_empty())
            })
            .cloned()
            .collect();
        candidates.sort();
        // Never touch the album declaration itself (it lives at the vault
        // root, not in the folder, but guard anyway).
        for relative in candidates {
            let contents = match snapshot.contents(&relative) {
                Some(contents) => contents.to_owned(),
                None => match fs::read_to_string(vault.join(&relative)) {
                    Ok(contents) => contents,
                    Err(error) => {
                        io_errors.push(format!(
                            "{}: failed to read file: {error}",
                            slash(&relative)
                        ));
                        continue;
                    }
                },
            };
            let Some(frontmatter) = parse_frontmatter(&contents) else {
                continue;
            };
            let names = frontmatter_type_names(&frontmatter);
            if !names.iter().any(|name| accepted.contains(name)) {
                continue;
            }
            scanned += 1;
            let Some(after) = strip_empty_notes_sections(&contents) else {
                continue;
            };
            // Defensive: a planned write must itself be clean.
            if strip_empty_notes_sections(&after).is_some() {
                continue;
            }
            changes.push(ChangedFile {
                path: slash(&relative),
                before: contents.clone(),
                after: after.clone(),
            });
            edits.push(FileEdit {
                path: relative.clone(),
                before: contents.clone(),
                after,
            });
            dependencies.insert(relative, Some(contents));
        }
    }

    changes.sort_by(|left, right| left.path.cmp(&right.path));
    edits.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(TidyPlan {
        scanned,
        changes,
        edits,
        dependencies,
        io_errors,
    })
}

fn planned_txn(vault: &Path, plan: TidyPlan) -> PlannedTxn<TidyContext> {
    let hosts = plan.edits.iter().map(|edit| edit.path.clone()).collect();
    let files = host_catalog::staged_rewrites(vault, &plan.edits);
    let count = files.len();
    let verify_vault = vault.to_path_buf();
    let verify = Box::new(move || verify_clean(&verify_vault, hosts));
    PlannedTxn {
        files,
        appends: Vec::new(),
        deps: plan.dependencies,
        subject: format!("records: tidy ({count} notes)"),
        verify,
        context: TidyContext {
            scanned: plan.scanned,
            changes: plan.changes,
            io_errors: plan.io_errors,
        },
        kind: "record",
    }
}

fn verify_clean(vault: &Path, hosts: Vec<PathBuf>) -> Result<(), String> {
    for relative in hosts {
        let contents =
            fs::read_to_string(vault.join(&relative)).map_err(|error| {
                format!("could not re-read {}: {error}", slash(&relative))
            })?;
        if strip_empty_notes_sections(&contents).is_some() {
            return Err(format!(
                "{} still holds an empty ## Notes section after the write",
                slash(&relative)
            ));
        }
    }
    Ok(())
}

/// Lock-free core for nightly. The caller holds the shared vault lock and
/// owns the surrounding vault-sync cycles.
pub(crate) fn sync_with_existing_lock(
    vault: &Path,
    child_env: &ChildEnv,
) -> TidyReport {
    let snapshot = VaultSnapshot::load(vault);
    let registry = AlbumRegistry::from_snapshot(vault, &snapshot);
    let plan = match plan_tidy(vault, &registry, &snapshot, None) {
        Ok(plan) => plan,
        Err(failure) => return TidyReport::failed(false, failure),
    };
    match apply_and_commit(vault, child_env, planned_txn(vault, plan)) {
        Ok((commit, context)) => report_of(
            false,
            TxnResult {
                commit,
                sync_pre: "skipped",
                sync_post: "skipped",
                post_sync_warning: None,
                context,
            },
        ),
        Err(failure) => TidyReport::failed(false, failure),
    }
}

/// Standalone real execution: shared lock, pre-sync, guarded transaction,
/// one scoped commit, then post-sync only when records changed.
pub(crate) fn run_standalone(vault: &Path, json_mode: bool) -> TidyReport {
    let child_env = ob::child_env();
    let captured: RefCell<Option<TidyContext>> = RefCell::new(None);
    let result = host_catalog::run_locked(
        vault,
        &child_env,
        json_mode,
        "record",
        || {
            let snapshot = VaultSnapshot::load(vault);
            let registry = AlbumRegistry::from_snapshot(vault, &snapshot);
            let plan = plan_tidy(vault, &registry, &snapshot, None)?;
            *captured.borrow_mut() = Some(TidyContext {
                scanned: plan.scanned,
                changes: plan.changes.clone(),
                io_errors: plan.io_errors.clone(),
            });
            Ok(planned_txn(vault, plan))
        },
    );
    match result {
        Ok(result) => report_of(false, result),
        Err(failure) => {
            let mut report = TidyReport::failed(false, failure);
            if let Some(context) = captured.into_inner() {
                report.scanned = context.scanned;
                report.changes = context.changes;
                report.io_errors = context.io_errors;
            }
            report
        }
    }
}

pub(crate) fn nightly_step(vault: &Path, child_env: &ChildEnv) -> i32 {
    let report = sync_with_existing_lock(vault, child_env);
    report.print_human();
    report.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn album_note(noun: &str) -> String {
        format!(
            "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Test.\nrecord:\n  noun: {noun}\n  fields: {{}}\n---\n\n# Albums\n"
        )
    }

    fn fixture(
        noun: &str,
        records: &[(&str, &str)],
    ) -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let vault = temp.path().join("vault");
        fs::create_dir_all(vault.join("eat")).expect("eat dir");
        fs::write(
            vault.join("body.md"),
            "---\ntype: [[area]]\n---\n\n# Body\n",
        )
        .expect("body");
        fs::write(vault.join("eat.md"), album_note(noun)).expect("album");
        for (stem, contents) in records {
            fs::write(vault.join("eat").join(format!("{stem}.md")), contents)
                .expect("record");
        }
        // Initialize a Git worktree so standalone runs can commit.
        let run = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(&vault)
                .args(args)
                .output()
                .expect("git runs")
                .status
                .success());
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);
        (temp, vault)
    }

    #[test]
    fn dry_run_plans_and_leaves_head_unchanged() {
        let (_temp, vault) = fixture(
            "restaurant",
            &[
                (
                    "one",
                    "---\ntype: \"[[record]]\"\ntitle: \"One\"\n---\n\n# One\n\n## Notes\n",
                ),
                (
                    "two",
                    "---\ntype: \"[[record]]\"\ntitle: \"Two\"\n---\n\n# Two\n\n## Notes\n\n- note\n",
                ),
                (
                    "skip",
                    "---\ntype: \"[[area]]\"\n---\n\n# Skip\n\n## Notes\n",
                ),
            ],
        );
        let head_before = git_head(&vault);
        let report = dry_run_report(&vault, None);
        assert!(report.failure.is_none(), "{:?}", report.failure);
        assert_eq!(report.scanned, 2);
        assert_eq!(report.changes.len(), 1);
        assert_eq!(report.changes[0].path, "eat/one.md");
        assert_eq!(git_head(&vault), head_before);
        assert!(vault.join("eat/one.md").exists());
        let contents =
            fs::read_to_string(vault.join("eat/one.md")).expect("read");
        assert!(contents.contains("## Notes"));
    }

    #[test]
    fn unknown_type_is_usage_error() {
        let (_temp, vault) = fixture("restaurant", &[]);
        let snapshot = VaultSnapshot::load(&vault);
        let registry = AlbumRegistry::from_snapshot(&vault, &snapshot);
        let failure = plan_tidy(&vault, &registry, &snapshot, Some("nope"))
            .expect_err("unknown type refuses");
        assert_eq!(failure.code, "unknown_type");
    }

    fn git_head(vault: &Path) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git rev-parse runs");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }
}
