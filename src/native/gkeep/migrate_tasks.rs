//! `bob gkeep migrate-tasks`: compact generated Keep metadata on open tasks.
//!
//! This is deliberately separate from `migrate-markers`: it changes only
//! open tasks and only when the complete generated legacy shape is proven.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    imports::{self, EntryState, ImportEntry, TransactionFile},
    ledger::{self, parse_markers},
    ui, GkeepError, MigrateTasksArgs,
};
use crate::native::{
    dataview::tasks_fingerprint, env as bob_env, markdown, note_tasks, ob,
    style::Styler,
};

// This is a local recovery journal, separate from the durable, Git-tracked
// import history in `.bob/gkeep/imports/`. The vault intentionally ignores
// this directory, and older Bob binaries do not read it.
const RECEIPT_DIR: &str = ".bob/gkeep/migrate-tasks";
const RECEIPT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MigrationReceipt {
    schema_version: u32,
    transaction_id: String,
    destination: String,
    before_sha256: String,
    after_sha256: String,
    state: EntryState,
}

#[derive(Debug, Clone)]
struct PhysicalLine<'a> {
    index: usize,
    start: usize,
    content_end: usize,
    end: usize,
    content: &'a str,
}

#[derive(Debug, Clone)]
struct Edit {
    start: usize,
    end: usize,
    replacement: String,
}

#[derive(Debug, Clone)]
struct MarkerProof {
    id: String,
    fp: String,
    url: Option<String>,
}

#[derive(Debug, Clone)]
struct TaskReport {
    task_index: usize,
    line: usize,
    title: String,
    status: &'static str,
    before: String,
    after: String,
    markers: Vec<MarkerProof>,
    source_url: Option<String>,
    url_less: bool,
    skip: Option<String>,
}

#[derive(Debug)]
struct FilePlan {
    rel: String,
    path: PathBuf,
    before_bytes: Vec<u8>,
    permissions: fs::Permissions,
    after: String,
    edits: Vec<Edit>,
    tasks: Vec<TaskReport>,
    closed_tasks: usize,
    already_current: usize,
}

#[derive(Debug, Default)]
struct Counts {
    planned_tasks: usize,
    applied_tasks: usize,
    planned_files: usize,
    applied_files: usize,
    planned_markers: usize,
    applied_markers: usize,
    planned_url_less: usize,
    applied_url_less: usize,
    closed_tasks: usize,
    already_current: usize,
    skipped: usize,
}

fn pull_lock_path() -> PathBuf {
    bob_env::bob_cli_state_dir().join("gkeep").join("pull.lock")
}

fn acquire_pull_lock() -> Result<File, GkeepError> {
    let path = pull_lock_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            GkeepError::runtime(
                "lock",
                format!(
                    "create pull lock directory {}: {error}",
                    parent.display()
                ),
            )
        })?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| {
            GkeepError::runtime(
                "lock",
                format!("open pull lock {}: {error}", path.display()),
            )
        })?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            Err(GkeepError::runtime(
                "lock",
                "another bob gkeep pull is already running".into(),
            ))
        }
        Err(error) => Err(GkeepError::runtime(
            "lock",
            format!("lock {}: {error}", path.display()),
        )),
    }
}

fn receipt_dir(bob_dir: &Path) -> PathBuf {
    bob_dir.join(RECEIPT_DIR)
}

fn validate_migration_receipt(
    receipt: &MigrationReceipt,
    path: &Path,
) -> Result<(), String> {
    if receipt.schema_version != RECEIPT_SCHEMA_VERSION
        || !receipt.transaction_id.starts_with("mt-")
        || receipt.transaction_id.trim().is_empty()
        || !receipt.transaction_id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(format!(
            "invalid task migration receipt {}; repair or remove it before running `bob gkeep migrate-tasks`",
            path.display()
        ));
    }
    let destination = Path::new(&receipt.destination);
    if receipt.destination.trim().is_empty()
        || destination.is_absolute()
        || destination.components().any(|component| {
            matches!(
                component,
                Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "task migration receipt {} has an unsafe destination; repair or remove it before running `bob gkeep migrate-tasks`",
            path.display()
        ));
    }
    for (name, digest) in [
        ("before_sha256", receipt.before_sha256.as_str()),
        ("after_sha256", receipt.after_sha256.as_str()),
    ] {
        if digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!(
                "task migration receipt {} has an invalid {name}; repair or remove it before running `bob gkeep migrate-tasks`",
                path.display()
            ));
        }
    }
    let expected_name = format!("{}.json", receipt.transaction_id);
    if path.file_name().and_then(|name| name.to_str())
        != Some(expected_name.as_str())
    {
        return Err(format!(
            "task migration receipt filename does not match its transaction id: {}; repair or remove it before running `bob gkeep migrate-tasks`",
            path.display()
        ));
    }
    Ok(())
}

fn read_migration_receipts(
    bob_dir: &Path,
) -> Result<Vec<(PathBuf, MigrationReceipt)>, String> {
    let dir = receipt_dir(bob_dir);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(format!(
                "read task migration receipt directory {}: {error}",
                dir.display()
            ));
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "read task migration receipt directory {}: {error}",
                dir.display()
            )
        })?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && name.contains(".tmp.") {
            continue;
        }
        if path.extension().is_none_or(|extension| extension != "json") {
            return Err(format!(
                "unexpected file in task migration receipt directory: {}; remove it or move it before running `bob gkeep migrate-tasks`",
                path.display()
            ));
        }
        if fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(format!(
                "task migration receipt is a symlink: {}; remove it before running `bob gkeep migrate-tasks`",
                path.display()
            ));
        }
        paths.push(path);
    }
    paths.sort();
    let mut receipts = Vec::with_capacity(paths.len());
    for path in paths {
        let bytes = fs::read(&path).map_err(|error| {
            format!("read task migration receipt {}: {error}", path.display())
        })?;
        let text = String::from_utf8(bytes).map_err(|error| {
            format!(
                "task migration receipt {} is not UTF-8: {error}",
                path.display()
            )
        })?;
        let receipt: MigrationReceipt =
            serde_json::from_str(&text).map_err(|error| {
                format!(
                    "task migration receipt {} is malformed: {error}",
                    path.display()
                )
            })?;
        validate_migration_receipt(&receipt, &path)?;
        receipts.push((path, receipt));
    }
    Ok(receipts)
}

fn persist_migration_receipt(
    bob_dir: &Path,
    receipt: &MigrationReceipt,
) -> Result<PathBuf, String> {
    let dir = receipt_dir(bob_dir);
    fs::create_dir_all(&dir).map_err(|error| {
        format!(
            "create task migration receipt directory {}: {error}",
            dir.display()
        )
    })?;
    let path = dir.join(format!("{}.json", receipt.transaction_id));
    validate_migration_receipt(receipt, &path)?;
    persist_receipt_at(&path, receipt)?;
    Ok(path)
}

fn persist_receipt_at(
    path: &Path,
    receipt: &MigrationReceipt,
) -> Result<(), String> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = parent.join(format!(
        ".{name}.tmp.{}.{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = serde_json::to_vec_pretty(receipt).map_err(|error| {
        format!("serialize task migration receipt: {error}")
    })?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| {
            format!("create temporary receipt {}: {error}", temp.display())
        })?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "write task migration receipt {}: {error}",
            path.display()
        ));
    }
    fs::rename(&temp, path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("install task migration receipt {}: {error}", path.display())
    })?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            format!(
                "sync task migration receipt directory {}: {error}",
                parent.display()
            )
        })
}

fn remove_migration_receipt(path: &Path) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::remove_file(path).map_err(|error| {
        format!(
            "remove stale task migration receipt {}: {error}",
            path.display()
        )
    })?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            format!(
                "sync task migration receipt directory {}: {error}",
                parent.display()
            )
        })
}

fn resolve_migration_receipts(
    bob_dir: &Path,
    receipts: &[(PathBuf, MigrationReceipt)],
) -> Result<Vec<PathBuf>, String> {
    let mut changed_paths = Vec::new();
    for (receipt_path, receipt) in receipts {
        if receipt.state != EntryState::Prepared {
            continue;
        }
        let note_path = bob_dir.join(&receipt.destination);
        let bytes = fs::read(&note_path).map_err(|error| {
            format!(
                "read pending migration destination {}: {error}",
                note_path.display()
            )
        })?;
        let sha = imports::sha256_hex(&bytes);
        if sha == receipt.before_sha256 {
            remove_migration_receipt(receipt_path)?;
            changed_paths.push(
                receipt_path
                    .strip_prefix(bob_dir)
                    .unwrap_or(receipt_path)
                    .to_path_buf(),
            );
        } else if sha == receipt.after_sha256 {
            let mut completed = receipt.clone();
            completed.state = EntryState::Verified;
            persist_receipt_at(receipt_path, &completed)?;
            changed_paths.push(
                receipt_path
                    .strip_prefix(bob_dir)
                    .unwrap_or(receipt_path)
                    .to_path_buf(),
            );
            changed_paths.push(PathBuf::from(&receipt.destination));
        } else {
            return Err(format!(
                "unresolved task migration {}: destination {} matches neither its before-image nor its recorded post-image; evidence retained at {}; resolve manually before running `bob gkeep migrate-tasks`",
                receipt.transaction_id,
                receipt.destination,
                receipt_path.display()
            ));
        }
    }
    Ok(changed_paths)
}

pub(crate) fn run(args: &MigrateTasksArgs) -> i32 {
    let bob_dir = args.bob_dir();
    let styler = Styler::detect();

    let _lock_guards = if args.dry_run {
        None
    } else {
        let pull = match acquire_pull_lock() {
            Ok(guard) => guard,
            Err(error) => return report_early_error(args, &error),
        };
        let waiting =
            || eprintln!("waiting for another vault maintenance run…");
        let vault = match ob::acquire_lock_waiting(
            std::time::Duration::from_secs(60),
            waiting,
        ) {
            Ok(guard) => guard,
            Err(error) => {
                return report_early_error(
                    args,
                    &GkeepError::runtime(
                        "lock",
                        format!("acquire vault maintenance lock: {error}"),
                    ),
                );
            }
        };
        Some((pull, vault))
    };

    let mut recovery_paths = Vec::<PathBuf>::new();
    let initial_store = match imports::read_all(&bob_dir) {
        Ok(files) => files,
        Err(error) => {
            return report_early_error(
                args,
                &GkeepError::runtime(
                    "vault",
                    format!("read the gkeep import store: {error}"),
                ),
            );
        }
    };
    let initial_receipts = match read_migration_receipts(&bob_dir) {
        Ok(receipts) => receipts,
        Err(message) => {
            return report_early_error(
                args,
                &GkeepError::runtime("vault", message),
            );
        }
    };
    if !args.dry_run {
        let settings = note_tasks::read_settings(&bob_dir);
        let preliminary = match scan_candidates(&bob_dir, &settings) {
            Ok(plans) => plans,
            Err(error) => return report_early_error(args, &error),
        };
        let pending_migration_destinations: BTreeSet<String> = initial_receipts
            .iter()
            .filter_map(|(_, receipt)| {
                (receipt.state == EntryState::Prepared)
                    .then(|| receipt.destination.clone())
            })
            .collect();
        let mut recovery_destinations: BTreeSet<String> = preliminary
            .iter()
            .filter(|plan| !plan.edits.is_empty())
            .map(|plan| plan.rel.clone())
            .collect();
        recovery_destinations.extend(pending_migration_destinations);
        let verified: BTreeSet<_> = imports::verified_pairs(&initial_store)
            .into_iter()
            .collect();
        let needs_import_receipts = preliminary.iter().any(|plan| {
            plan.tasks.iter().any(|task| {
                task.skip.is_none()
                    && task.markers.iter().any(|marker| {
                        !verified
                            .contains(&(marker.id.clone(), marker.fp.clone()))
                    })
            })
        });
        let pending_import_recovery = imports::unfinished(&initial_store)
            .iter()
            .any(|(_, tx)| recovery_destinations.contains(&tx.destination));

        let child_env = ob::child_env();
        let worktree = match ob::detect_git_worktree(&bob_dir, &child_env) {
            Ok(value) => value,
            Err(error) => {
                return report_early_error(
                    args,
                    &GkeepError::runtime(
                        "commit",
                        format!("detect the vault Git worktree: {error}"),
                    ),
                );
            }
        };
        if worktree {
            let mut paths: Vec<PathBuf> =
                recovery_destinations.iter().map(PathBuf::from).collect();
            if needs_import_receipts || pending_import_recovery {
                paths.push(PathBuf::from(imports::IMPORTS_DIR));
            }
            if let Err(error) =
                imports::preflight_trackable(&bob_dir, &child_env, &paths)
            {
                return report_early_error(args, &error.into_gkeep());
            }
        }

        match resolve_migration_receipts(&bob_dir, &initial_receipts) {
            Ok(paths) => recovery_paths.extend(paths),
            Err(message) => {
                return report_early_error(
                    args,
                    &GkeepError::runtime("vault", message),
                );
            }
        }
        for destination in recovery_destinations {
            let before = match imports::read_all(&bob_dir) {
                Ok(files) => files,
                Err(error) => {
                    return report_early_error(
                        args,
                        &GkeepError::runtime(
                            "vault",
                            format!("read the gkeep import store: {error}"),
                        ),
                    );
                }
            };
            if let Err(error) = super::pull::resolve_unfinished(
                &bob_dir,
                Path::new(&destination),
                &settings,
            ) {
                return report_early_error(args, &error);
            }
            let after = match imports::read_all(&bob_dir) {
                Ok(files) => files,
                Err(error) => {
                    return report_early_error(
                        args,
                        &GkeepError::runtime(
                            "vault",
                            format!("read the gkeep import store: {error}"),
                        ),
                    );
                }
            };
            let after_by_path: BTreeMap<_, _> =
                after.iter().map(|(path, file)| (path, file)).collect();
            for (path, file) in &before {
                let changed = match after_by_path.get(path) {
                    Some(updated) => *updated != file,
                    None => true,
                };
                if changed {
                    recovery_paths.push(
                        path.strip_prefix(&bob_dir)
                            .unwrap_or(path)
                            .to_path_buf(),
                    );
                    if after_by_path
                        .get(path)
                        .is_some_and(|updated| imports::is_completed(updated))
                    {
                        recovery_paths.push(PathBuf::from(&file.destination));
                    }
                }
            }
        }
    }

    let settings = note_tasks::read_settings(&bob_dir);
    let mut plans = match scan_candidates(&bob_dir, &settings) {
        Ok(plans) => plans,
        Err(error) => return report_early_error(args, &error),
    };
    plans.sort_by(|left, right| left.rel.cmp(&right.rel));

    let store = match imports::read_all(&bob_dir) {
        Ok(files) => files,
        Err(error) => {
            return report_early_error(
                args,
                &GkeepError::runtime(
                    "vault",
                    format!("read the gkeep import store: {error}"),
                ),
            );
        }
    };
    let mut verified: BTreeSet<(String, String)> =
        imports::verified_pairs(&store).into_iter().collect();

    if !args.dry_run {
        let needs_import_receipts = plans.iter().any(|plan| {
            plan.tasks.iter().any(|task| {
                task.skip.is_none()
                    && task.markers.iter().any(|marker| {
                        !verified
                            .contains(&(marker.id.clone(), marker.fp.clone()))
                    })
            })
        });
        let child_env = ob::child_env();
        let worktree = match ob::detect_git_worktree(&bob_dir, &child_env) {
            Ok(value) => value,
            Err(error) => {
                return report_early_error(
                    args,
                    &GkeepError::runtime(
                        "commit",
                        format!("detect the vault Git worktree: {error}"),
                    ),
                );
            }
        };
        if worktree {
            let mut paths: Vec<PathBuf> = plans
                .iter()
                .filter(|plan| !plan.edits.is_empty())
                .map(|plan| PathBuf::from(&plan.rel))
                .collect();
            if needs_import_receipts {
                paths.push(PathBuf::from(imports::IMPORTS_DIR));
            }
            if let Err(error) =
                imports::preflight_trackable(&bob_dir, &child_env, &paths)
            {
                return report_early_error(args, &error.into_gkeep());
            }
        }
    }

    let mut counts = summarize(&plans);
    if args.dry_run {
        return report(args, &plans, &counts, None, &[], true, &styler);
    }

    let mut changed_notes = Vec::<PathBuf>::new();
    let mut receipt_paths = recovery_paths;
    let mut failures = Vec::<(String, String)>::new();
    for plan in plans.iter_mut().filter(|plan| !plan.edits.is_empty()) {
        if let Err(message) = apply_file_plan(
            plan,
            &bob_dir,
            &settings,
            &mut verified,
            &mut receipt_paths,
        ) {
            let installed = fs::read(&plan.path)
                .is_ok_and(|bytes| bytes == plan.after.as_bytes());
            if installed {
                changed_notes.push(PathBuf::from(&plan.rel));
                if let Ok(receipts) = read_migration_receipts(&bob_dir) {
                    receipt_paths.extend(receipts.iter().filter_map(
                        |(path, receipt)| {
                            (receipt.destination == plan.rel).then(|| {
                                path.strip_prefix(&bob_dir)
                                    .unwrap_or(path)
                                    .to_path_buf()
                            })
                        },
                    ));
                }
                if let Ok(files) = imports::read_all(&bob_dir) {
                    receipt_paths.extend(files.iter().filter_map(
                        |(path, tx)| {
                            (tx.destination == plan.rel
                                && tx.transaction_id.starts_with("mt-"))
                            .then(|| {
                                path.strip_prefix(&bob_dir)
                                    .unwrap_or(path)
                                    .to_path_buf()
                            })
                        },
                    ));
                }
            }
            for task in &mut plan.tasks {
                if task.skip.is_none() && task.before != task.after {
                    task.status =
                        if installed { "installed" } else { "failed" };
                }
            }
            failures.push((plan.rel.clone(), message));
            continue;
        }
        changed_notes.push(PathBuf::from(&plan.rel));
        for task in &mut plan.tasks {
            if task.skip.is_none() && task.before != task.after {
                task.status = "applied";
            }
        }
    }
    changed_notes.sort();
    changed_notes.dedup();
    receipt_paths.sort();
    receipt_paths.dedup();

    counts.applied_files = changed_notes.len();
    counts.applied_tasks = plans
        .iter()
        .flat_map(|plan| &plan.tasks)
        .filter(|task| matches!(task.status, "applied" | "installed"))
        .count();
    counts.applied_markers = plans
        .iter()
        .flat_map(|plan| &plan.tasks)
        .filter(|task| matches!(task.status, "applied" | "installed"))
        .map(|task| task.markers.len())
        .sum();
    counts.applied_url_less = plans
        .iter()
        .flat_map(|plan| &plan.tasks)
        .filter(|task| {
            matches!(task.status, "applied" | "installed") && task.url_less
        })
        .count();

    // A prior failed commit leaves a verified migration receipt and exact
    // post-image. Include the note on clean reruns, scoped to that path.
    // The separate operation journal is local recovery state and remains
    // ignored; only durable import receipts under `imports/` are committed.
    let mut commit_paths = changed_notes.clone();
    commit_paths.extend(
        receipt_paths
            .into_iter()
            .filter(|path| !path.starts_with(RECEIPT_DIR)),
    );
    let current_store = match imports::read_all(&bob_dir) {
        Ok(files) => files,
        Err(error) => {
            failures.push((
                "receipt store".into(),
                format!("read the gkeep import store: {error}"),
            ));
            Vec::new()
        }
    };
    let current_receipts = match read_migration_receipts(&bob_dir) {
        Ok(receipts) => receipts,
        Err(message) => {
            failures.push(("receipt store".into(), message));
            Vec::new()
        }
    };
    commit_paths.extend(pending_migration_commit_paths(
        &bob_dir,
        &current_store,
        &current_receipts,
    ));
    if failures.iter().any(|(path, _)| path == "receipt store") {
        commit_paths.clear();
    }
    commit_paths.sort();
    commit_paths.dedup();

    let mut commit_sha = None;
    if !args.no_commit && !commit_paths.is_empty() {
        let child_env = ob::child_env();
        match ob::detect_git_worktree(&bob_dir, &child_env) {
            Ok(true) => match ob::commit_paths(
                &bob_dir,
                &child_env,
                &format!(
                    "bob gkeep migrate-tasks: {} open tasks",
                    counts.applied_tasks
                ),
                &commit_paths,
            ) {
                Ok(sha) => commit_sha = sha,
                Err(error) => failures.push((
                    "commit".into(),
                    format!("commit the vault: {error}"),
                )),
            },
            Ok(false) => {}
            Err(error) => failures.push((
                "commit".into(),
                format!("detect the vault Git worktree: {error}"),
            )),
        }
    }

    report(args, &plans, &counts, commit_sha, &failures, false, &styler)
}

fn summarize(plans: &[FilePlan]) -> Counts {
    let mut counts = Counts::default();
    for plan in plans {
        counts.closed_tasks += plan.closed_tasks;
        counts.already_current += plan.already_current;
        for task in &plan.tasks {
            if task.skip.is_some() {
                counts.skipped += 1;
            } else if task.before != task.after {
                counts.planned_tasks += 1;
                counts.planned_markers += task.markers.len();
                counts.planned_url_less += usize::from(task.url_less);
            }
        }
        if !plan.edits.is_empty() {
            counts.planned_files += 1;
        }
    }
    counts
}

fn report_early_error(args: &MigrateTasksArgs, error: &GkeepError) -> i32 {
    if args.format.is_json() {
        println!(
            "{}",
            json!({
                "schema_version": 1,
                "ok": false,
                "dry_run": args.dry_run,
                "files": [],
                "summary": {
                    "planned": {"files": 0, "tasks": 0, "markers_removed": 0, "url_less": 0},
                    "applied": {"files": 0, "tasks": 0, "markers_removed": 0, "url_less": 0},
                    "closed_tasks_excluded": 0,
                    "already_current": 0,
                    "skipped": 0,
                    "failed": 1
                },
                "commit": null,
                "failures": [{"path": null, "message": error.message()}]
            })
        );
        error.exit_code()
    } else {
        ui::report_error("migrate-tasks", error, args.error_format())
    }
}

fn scan_candidates(
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
) -> Result<Vec<FilePlan>, GkeepError> {
    let mut paths = Vec::new();
    visit_dir(bob_dir, bob_dir, &mut paths)?;
    paths.sort();
    let mut plans = Vec::new();
    for path in paths {
        if path.extension().is_none_or(|extension| extension != "md")
            || path.starts_with(imports::imports_dir(bob_dir))
        {
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| {
            GkeepError::runtime(
                "vault",
                format!("read candidate note {}: {error}", path.display()),
            )
        })?;
        let text = String::from_utf8(bytes.clone()).map_err(|_| {
            GkeepError::runtime(
                "vault",
                format!("candidate Markdown is not UTF-8: {}", path.display()),
            )
        })?;
        if let Some(plan) = scan_file(bob_dir, &path, bytes, &text, settings)? {
            plans.push(plan);
        }
    }
    Ok(plans)
}

fn visit_dir(
    dir: &Path,
    bob_dir: &Path,
    out: &mut Vec<PathBuf>,
) -> Result<(), GkeepError> {
    let entries = fs::read_dir(dir).map_err(|error| {
        GkeepError::runtime(
            "vault",
            format!("scan the vault {}: {error}", dir.display()),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            GkeepError::runtime("vault", format!("scan the vault: {error}"))
        })?;
        let kind = entry.file_type().map_err(|error| {
            GkeepError::runtime(
                "vault",
                format!("inspect vault entry: {error}"),
            )
        })?;
        if kind.is_dir() {
            if crate::native::is_always_excluded_note_directory_name(
                &entry.file_name(),
            ) || entry.path() == imports::imports_dir(bob_dir)
            {
                continue;
            }
            visit_dir(&entry.path(), bob_dir, out)?;
        } else if kind.is_file() {
            out.push(entry.path());
        }
        // Symlinks are neither files nor directories here, so never followed.
    }
    Ok(())
}

fn scan_file(
    bob_dir: &Path,
    path: &Path,
    before_bytes: Vec<u8>,
    text: &str,
    settings: &note_tasks::NoteTaskSettings,
) -> Result<Option<FilePlan>, GkeepError> {
    let lines = physical_lines(text);
    let logical: Vec<&str> = lines.iter().map(|line| line.content).collect();
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&logical);
    let scan_start = frontmatter_end.map_or(0, |index| index + 1);
    let fenced = markdown::fenced_lines(&logical, scan_start..logical.len());
    let blocked =
        blocked_comment_and_quote_lines(&logical, &fenced, frontmatter_end);
    let scan = note_tasks::scan(text, settings);
    let mut raw_tasks = Vec::<(usize, String)>::new();
    for line in &lines {
        if !blocked.contains(&line.index)
            && let Some(indent) = raw_checkbox_indent(line.content)
        {
            raw_tasks.push((line.index, indent.to_string()));
        }
    }
    let open_tasks: BTreeMap<usize, &note_tasks::NoteTask> = scan
        .open_tasks()
        .map(|task| (task.line_index, task))
        .collect();
    let all_tasks: BTreeMap<usize, &note_tasks::NoteTask> = scan
        .tasks()
        .iter()
        .map(|task| (task.line_index, task))
        .collect();
    let mut metadata_by_task = BTreeMap::<usize, Vec<usize>>::new();
    let mut owner_uncertain = BTreeMap::<usize, String>::new();

    for line in &lines {
        if blocked.contains(&line.index) || line.content.trim().is_empty() {
            continue;
        }
        if !is_metadata_candidate(line.content) {
            continue;
        }
        let candidate_indent = list_item_indent(line.content).or_else(|| {
            is_standalone_marker(line.content).then(|| {
                let len = line
                    .content
                    .find(|character: char| !character.is_whitespace())
                    .unwrap_or(line.content.len());
                &line.content[..len]
            })
        });
        let Some(candidate_indent) = candidate_indent else {
            continue;
        };
        let owner = raw_tasks
            .iter()
            .filter(|(index, indent)| {
                *index < line.index
                    && indent.len() < candidate_indent.len()
                    && candidate_indent.starts_with(indent)
            })
            .max_by_key(|(index, indent)| (indent.len(), *index));
        let Some((owner_line, owner_indent)) = owner else {
            owner_uncertain.insert(
                line.index,
                "candidate metadata has no provable owning task".into(),
            );
            continue;
        };
        let has_boundary =
            lines[*owner_line + 1..line.index].iter().any(|between| {
                !between.content.trim().is_empty()
                    && (markdown::atx_heading(between.content).is_some()
                        || between
                            .content
                            .find(|character: char| !character.is_whitespace())
                            .unwrap_or(between.content.len())
                            <= owner_indent.len())
            });
        if has_boundary {
            owner_uncertain.insert(
                line.index,
                "a Markdown block boundary separates the candidate from its task".into(),
            );
            metadata_by_task
                .entry(*owner_line)
                .or_default()
                .push(line.index);
            continue;
        }
        if !is_direct_child_indent(owner_indent, candidate_indent) {
            owner_uncertain.insert(
                line.index,
                "metadata indentation does not establish a direct task child"
                    .into(),
            );
            metadata_by_task
                .entry(*owner_line)
                .or_default()
                .push(line.index);
            continue;
        }
        metadata_by_task
            .entry(*owner_line)
            .or_default()
            .push(line.index);
    }

    let mut plans = Vec::new();
    let mut all_edits = Vec::new();
    let mut already_current = 0;
    let closed_tasks = scan
        .tasks()
        .iter()
        .filter(|task| {
            !task.status_type.is_open()
                && metadata_by_task.contains_key(&task.line_index)
        })
        .count();
    for (&task_line, task) in &open_tasks {
        let task_index = scan
            .tasks()
            .iter()
            .position(|candidate| candidate.line_index == task_line)
            .unwrap_or(0);
        let current_line = &lines[task_line];
        let bulb_urls = bulb_urls(current_line.content);
        let raw_bulb_count = current_line.content.matches("[💡](").count();
        if bulb_urls.len() == 1 {
            already_current += 1;
        }
        let mut child_indices = metadata_by_task
            .get(&task_line)
            .cloned()
            .unwrap_or_default();
        child_indices.sort_unstable();
        child_indices.dedup();
        let mut source_indices = Vec::new();
        let mut standalone_indices = Vec::new();
        let mut malformed_indices = Vec::new();
        for index in child_indices {
            if owner_uncertain.contains_key(&index) {
                malformed_indices.push(index);
            } else if is_source_item(lines[index].content) {
                source_indices.push(index);
            } else if is_standalone_marker(lines[index].content) {
                standalone_indices.push(index);
            } else if lines[index].content.contains("%%gkeep:") {
                malformed_indices.push(index);
            }
        }
        let inline_marker = current_line.content.contains("%%gkeep:");
        if source_indices.is_empty()
            && standalone_indices.is_empty()
            && malformed_indices.is_empty()
            && !inline_marker
        {
            continue;
        }

        let before_block = task_block_text(text, task, &lines);
        let mut task_edits = Vec::<Edit>::new();
        let mut markers = Vec::<MarkerProof>::new();
        let mut source_parsed = Vec::<(usize, ParsedLegacySource)>::new();
        let mut source_signatures = BTreeSet::new();
        let mut skip = None;

        if inline_marker {
            skip = Some(
                "unsupported inline bookkeeping marker on task line".into(),
            );
        }
        if raw_bulb_count != bulb_urls.len() {
            skip.get_or_insert_with(|| {
                "task contains a malformed or conflicting Keep bulb".into()
            });
        }

        for index in &malformed_indices {
            skip.get_or_insert_with(|| {
                owner_uncertain.get(index).cloned().unwrap_or_else(|| {
                    "unsupported or malformed Keep bookkeeping marker".into()
                })
            });
        }
        for index in &source_indices {
            let line = &lines[*index];
            let marker_tokens = parse_markers(line.content);
            let terminal_marker = terminal_marker(line.content);
            if marker_tokens.len() > 1
                || (!marker_tokens.is_empty() && terminal_marker.is_none())
            {
                skip.get_or_insert_with(|| "malformed, multiple, or non-terminal marker on Source child".into());
                continue;
            }
            if line_has_descendants(*index, &lines) {
                skip.get_or_insert_with(|| {
                    "Source child has additional descendants".into()
                });
                continue;
            }
            match parse_legacy_source(line.content, terminal_marker.as_ref()) {
                Some(parsed) => {
                    let signature = terminal_marker
                        .as_ref()
                        .map(|(id, fp)| {
                            let token = ledger::format_marker(id, fp);
                            line.content
                                .strip_suffix(&format!(" {token}"))
                                .unwrap_or(line.content)
                                .to_string()
                        })
                        .unwrap_or_else(|| line.content.to_string());
                    source_signatures.insert(signature);
                    if let Some((id, fp)) = terminal_marker {
                        markers.push(MarkerProof {
                            id,
                            fp,
                            url: parsed.url.clone().map(|url| {
                                decode_legacy_url(&url).unwrap_or(url)
                            }),
                        });
                    }
                    source_parsed.push((*index, parsed));
                }
                None if !marker_tokens.is_empty()
                    || line.content.contains("%%gkeep:")
                    || is_metadata_candidate(line.content) =>
                {
                    skip.get_or_insert_with(|| "Source child does not match the full generated Keep grammar".into());
                }
                None => {}
            }
        }
        if source_parsed.len() > 1 && source_signatures.len() > 1 {
            skip.get_or_insert_with(|| {
                "duplicate Source children are not identical generated metadata"
                    .into()
            });
        }
        for index in &standalone_indices {
            let line = &lines[*index];
            match standalone_marker(line.content) {
                Some((id, fp)) => markers.push(MarkerProof {
                    id,
                    fp,
                    url: bulb_urls
                        .first()
                        .cloned()
                        .map(|url| decode_legacy_url(&url).unwrap_or(url)),
                }),
                None => {
                    skip.get_or_insert_with(|| {
                        "standalone line is not one supported marker-only token"
                            .into()
                    });
                }
            };
        }
        let distinct_markers: BTreeSet<_> = markers
            .iter()
            .map(|marker| (marker.id.clone(), marker.fp.clone()))
            .collect();
        if distinct_markers.len() > 1 {
            skip.get_or_insert_with(|| {
                "conflicting Keep ids or fingerprints".into()
            });
        }

        let mut urls: Vec<String> = source_parsed
            .iter()
            .filter_map(|(_, source)| source.url.clone())
            .map(|url| tooltip_safe_url(&url))
            .collect();
        urls.extend(bulb_urls.iter().cloned());
        urls.sort();
        urls.dedup();
        if urls.len() > 1 {
            skip.get_or_insert_with(|| {
                "multiple different Keep destinations or a conflicting bulb"
                    .into()
            });
        }

        let mut labels: Vec<String> = source_parsed
            .iter()
            .filter_map(|(_, source)| source.labels.clone())
            .collect();
        labels.sort();
        labels.dedup();
        if source_parsed
            .iter()
            .any(|(_, source)| source.label_ambiguous)
        {
            skip.get_or_insert_with(|| "label text contains a metadata separator and cannot be split safely".into());
        }
        if labels.len() > 1 {
            skip.get_or_insert_with(|| {
                "duplicate Source children carry different labels".into()
            });
        }
        let revision = source_parsed.iter().any(|(_, source)| source.revision);
        let source_url = urls.first().cloned();
        if source_parsed.iter().any(|(_, source)| source.url.is_none())
            && !bulb_urls.is_empty()
        {
            skip.get_or_insert_with(|| {
                "URL-less Source conflicts with an existing linked bulb".into()
            });
        }
        if !standalone_indices.is_empty()
            && source_parsed.is_empty()
            && bulb_urls.is_empty()
        {
            skip.get_or_insert_with(|| {
                "standalone marker has no recognized Source child or Keep bulb"
                    .into()
            });
        }
        if source_parsed.iter().any(|(_, source)| source.url.is_some())
            && source_url.is_none()
        {
            skip.get_or_insert_with(|| {
                "source destination is malformed".into()
            });
        }
        if let Some(url) = source_url.as_deref() {
            if !supported_keep_url(url) {
                skip.get_or_insert_with(|| {
                    "source destination is not a supported HTTP(S) Keep URL"
                        .into()
                });
            }
        }
        if bulb_urls.len() > 1 {
            skip.get_or_insert_with(|| {
                "task has multiple generated Keep bulbs".into()
            });
        }
        if let (Some(existing), Some(source)) = (
            bulb_urls.first(),
            source_parsed
                .iter()
                .find_map(|(_, source)| source.url.as_ref()),
        ) && existing != &tooltip_safe_url(source)
        {
            skip.get_or_insert_with(|| {
                "existing bulb destination conflicts with Source child".into()
            });
        }

        if skip.is_none() {
            if let Some(url) = source_url.as_deref() {
                if bulb_urls.is_empty() {
                    let destination = tooltip_safe_url(url);
                    match insert_metadata(current_line.content, Some(&destination), revision) {
                        Some(after_line) => {
                            if tasks_fingerprint(current_line.content) != tasks_fingerprint(&after_line) {
                                skip = Some("bulb insertion changes parsed task semantics".into());
                            } else {
                                task_edits.push(Edit {
                                    start: current_line.start,
                                    end: current_line.content_end,
                                    replacement: after_line,
                                });
                            }
                        }
                        None => skip = Some("the task metadata suffix has no safe insertion point".into()),
                    }
                }
            } else if source_parsed
                .iter()
                .any(|(_, source)| source.url.is_some())
            {
                skip = Some("source URL disappeared during analysis".into());
            }
            if skip.is_none() && revision {
                if !bulb_urls.is_empty() {
                    if let Some(after_line) =
                        add_revision_before_bulb(current_line.content)
                    {
                        if tasks_fingerprint(current_line.content)
                            != tasks_fingerprint(&after_line)
                        {
                            skip = Some("revision insertion changes parsed task semantics".into());
                        } else if after_line != current_line.content {
                            task_edits.push(Edit {
                                start: current_line.start,
                                end: current_line.content_end,
                                replacement: after_line,
                            });
                        }
                    } else {
                        skip = Some("the existing Keep bulb has no safe revision insertion point".into());
                    }
                } else if source_url.is_none() && !source_parsed.is_empty() {
                    match insert_metadata(current_line.content, None, true) {
                        Some(after_line) => {
                            if tasks_fingerprint(current_line.content) != tasks_fingerprint(&after_line) {
                                skip = Some("revision insertion changes parsed task semantics".into());
                            } else {
                                task_edits.push(Edit { start: current_line.start, end: current_line.content_end, replacement: after_line });
                            }
                        }
                        None => skip = Some("the task metadata suffix has no safe revision insertion point".into()),
                    }
                }
            }
        }

        let mut replacement_indices = Vec::new();
        if skip.is_none() {
            let mut emitted_labels = false;
            for (index, source) in &source_parsed {
                let line = &lines[*index];
                let new_text = if let Some(labels) = source.labels.as_deref() {
                    if emitted_labels {
                        String::new()
                    } else {
                        emitted_labels = true;
                        source.label_prefix.clone().unwrap_or_default() + labels
                    }
                } else {
                    String::new()
                };
                let replacement = if new_text.is_empty() {
                    String::new()
                } else {
                    format!("{new_text}{}", &text[line.content_end..line.end])
                };
                task_edits.push(Edit {
                    start: line.start,
                    end: line.end,
                    replacement,
                });
                replacement_indices.push(*index);
            }
            for index in &standalone_indices {
                let line = &lines[*index];
                if standalone_marker(line.content).is_some() {
                    task_edits.push(Edit {
                        start: line.start,
                        end: line.end,
                        replacement: String::new(),
                    });
                }
            }
        }

        if skip.is_some() {
            task_edits.clear();
        }
        let after_task = apply_task_local_edits(
            &before_block,
            current_line.start,
            &task_edits,
        )
        .unwrap_or_else(|| before_block.clone());
        let display = task.description.clone();
        plans.push(TaskReport {
            task_index,
            line: task.line_index + 1,
            title: if display.is_empty() {
                current_line.content.to_string()
            } else {
                display
            },
            status: if skip.is_some() {
                "skipped"
            } else if task_edits.is_empty() {
                "current"
            } else {
                "planned"
            },
            before: before_block,
            after: after_task,
            markers,
            source_url,
            url_less: skip.is_none()
                && source_parsed.iter().any(|(_, source)| source.url.is_none()),
            skip,
        });
        if let Some(report) = plans.last()
            && report.skip.is_none()
        {
            all_edits.extend(task_edits);
        }
        let _ = replacement_indices;
    }

    {
        // Candidate metadata with unproven ownership is never attached by guesswork.
        for (line_index, reason) in owner_uncertain {
            if metadata_by_task
                .values()
                .any(|indices| indices.contains(&line_index))
            {
                let line_indent = lines[line_index]
                    .content
                    .find(|character: char| !character.is_whitespace())
                    .unwrap_or(lines[line_index].content.len());
                let closed_owner = raw_tasks
                    .iter()
                    .filter(|(index, indent)| {
                        *index < line_index && indent.len() < line_indent
                    })
                    .max_by_key(|(index, indent)| (indent.len(), *index))
                    .and_then(|(owner, _)| all_tasks.get(owner));
                if closed_owner.is_some_and(|task| !task.status_type.is_open())
                {
                    continue;
                }
            }
            let line = &lines[line_index];
            plans.push(TaskReport {
                task_index: 0,
                line: line_index + 1,
                title: line.content.to_string(),
                status: "skipped",
                before: line.content.to_string(),
                after: line.content.to_string(),
                markers: Vec::new(),
                source_url: None,
                url_less: false,
                skip: Some(reason),
            });
        }
    }

    if all_edits.is_empty()
        && plans.is_empty()
        && already_current == 0
        && closed_tasks == 0
    {
        return Ok(None);
    }
    let after =
        apply_edits(text, &all_edits).unwrap_or_else(|| text.to_string());
    if after == text
        && plans.is_empty()
        && already_current == 0
        && closed_tasks == 0
    {
        return Ok(None);
    }
    let permissions = fs::metadata(path)
        .map_err(|error| {
            GkeepError::runtime(
                "vault",
                format!("stat candidate note {}: {error}", path.display()),
            )
        })?
        .permissions();
    let rel = path
        .strip_prefix(bob_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    Ok(Some(FilePlan {
        rel,
        path: path.to_path_buf(),
        before_bytes,
        permissions,
        after,
        edits: all_edits,
        tasks: plans,
        closed_tasks,
        already_current,
    }))
}

#[derive(Debug, Clone)]
struct ParsedLegacySource {
    url: Option<String>,
    labels: Option<String>,
    label_prefix: Option<String>,
    revision: bool,
    label_ambiguous: bool,
}

fn parse_legacy_source(
    line: &str,
    marker: Option<&(String, String)>,
) -> Option<ParsedLegacySource> {
    let (prefix, item) = list_item_prefix(line)?;
    let _ = prefix;
    let mut body = item.strip_prefix("Source: ")?;
    if let Some((id, fp)) = marker {
        let token = ledger::format_marker(id, fp);
        body = body.strip_suffix(&format!(" {token}"))?;
    }
    let (source, timestamp_tail) = body.split_once(" · ")?;
    let timestamp = timestamp_tail.get(..16)?;
    let bytes = timestamp.as_bytes();
    if bytes.len() != 16
        || !(bytes[0..4].iter().all(u8::is_ascii_digit)
            && bytes[4] == b'-'
            && bytes[5..7].iter().all(u8::is_ascii_digit)
            && bytes[7] == b'-'
            && bytes[8..10].iter().all(u8::is_ascii_digit)
            && bytes[10] == b' '
            && bytes[11..13].iter().all(u8::is_ascii_digit)
            && bytes[13] == b':'
            && bytes[14..16].iter().all(u8::is_ascii_digit))
    {
        return None;
    }
    let tail = &timestamp_tail[16..];
    let (labels, revision, label_ambiguous) = if tail.is_empty() {
        (None, false, false)
    } else if tail == " · revised" {
        (None, true, false)
    } else if let Some(raw_labels) = tail.strip_prefix(" · 🏷 ") {
        if let Some(labels) = raw_labels.strip_suffix(" · revised") {
            let ambiguous = labels.contains(" · ");
            (Some(labels), true, ambiguous)
        } else {
            let ambiguous = raw_labels.contains(" · ");
            (Some(raw_labels), false, ambiguous)
        }
    } else {
        return None;
    };
    if labels
        .as_deref()
        .is_some_and(|labels| labels.trim().is_empty())
    {
        return None;
    }
    let label_prefix = labels.as_ref().map(|_| {
        let (prefix, _) = list_item_prefix(line).expect("matched above");
        format!("{prefix}🏷 ")
    });
    let url = if source == "Google Keep" {
        None
    } else {
        let raw = source.strip_prefix("[Google Keep](")?.strip_suffix(')')?;
        if raw.is_empty()
            || raw.contains(['(', ')', '<', '>'])
            || raw.chars().any(char::is_whitespace)
        {
            return None;
        }
        Some(raw.to_string())
    };
    Some(ParsedLegacySource {
        url,
        labels: labels.map(str::to_string),
        label_prefix,
        revision,
        label_ambiguous,
    })
}

fn list_item_prefix(line: &str) -> Option<(&str, &str)> {
    let indent_len = line
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(line.len());
    let rest = &line[indent_len..];
    let bullet_len = rest
        .chars()
        .next()
        .filter(|character| matches!(character, '-' | '*' | '+'))?
        .len_utf8();
    let after = rest.get(bullet_len..)?.strip_prefix(' ')?;
    Some((&line[..indent_len + bullet_len + 1], after))
}

fn is_source_item(line: &str) -> bool {
    list_item_prefix(line).is_some_and(|(_, item)| item.starts_with("Source:"))
}

fn is_metadata_candidate(line: &str) -> bool {
    is_standalone_marker(line)
        || (is_source_item(line)
            && (line.contains("%%gkeep:")
                || terminal_marker(line).is_some()
                || parse_legacy_source(line, None).is_some()
                || line.contains("[Google Keep](")
                || line.contains("Source: Google Keep")))
        || line.contains("%%gkeep:") && list_item_prefix(line).is_some()
}

fn list_item_indent(line: &str) -> Option<&str> {
    let indent_len = line
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(line.len());
    let rest = &line[indent_len..];
    matches!(rest.chars().next(), Some('-' | '*' | '+'))
        .then_some(&line[..indent_len])
}

fn is_direct_child_indent(parent: &str, child: &str) -> bool {
    let Some(extra) = child.strip_prefix(parent) else {
        return false;
    };
    extra == "  " || extra == "\t"
}

fn raw_checkbox_indent(line: &str) -> Option<&str> {
    let indent_len = line
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(line.len());
    let rest = &line[indent_len..];
    let rest = rest.strip_prefix("- [")?;
    let status = rest.chars().next()?;
    let after = rest.get(status.len_utf8()..)?.strip_prefix("] ")?;
    (!after.is_empty()).then_some(&line[..indent_len])
}

fn standalone_marker(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if !line.starts_with([' ', '\t']) || parse_markers(trimmed).len() != 1 {
        return None;
    }
    let (id, fp) = parse_markers(trimmed).into_iter().next()?;
    (trimmed == ledger::format_marker(&id, &fp)).then_some((id, fp))
}

fn is_standalone_marker(line: &str) -> bool {
    standalone_marker(line).is_some() || line.trim().starts_with("%%gkeep:")
}

fn terminal_marker(line: &str) -> Option<(String, String)> {
    let markers = parse_markers(line);
    if markers.len() != 1 {
        return None;
    }
    let (id, fp) = markers.into_iter().next()?;
    let token = ledger::format_marker(&id, &fp);
    line.ends_with(&format!(" {token}")).then_some((id, fp))
}

fn supported_keep_url(value: &str) -> bool {
    url::Url::parse(value).ok().is_some_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some_and(|host| {
                host.eq_ignore_ascii_case("keep.google.com")
            })
    })
}

fn tooltip_safe_url(value: &str) -> String {
    value.replace('\\', "%5C").replace('"', "%22")
}

fn decode_legacy_url(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut raw = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let digits = value.get(index + 1..index + 3)?;
            raw.push(u8::from_str_radix(digits, 16).ok()?);
            index += 3;
        } else {
            raw.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(raw).ok()
}

fn bulb_urls(line: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find("[💡](") {
        let after = &rest[start + "[💡](".len()..];
        if let Some((url, suffix)) =
            after.split_once(" \"Open in Google Keep\")")
            && !url.is_empty()
            && !url.contains(['(', ')', '<', '>', '"', '\\'])
            && supported_keep_url(url)
        {
            urls.push(url.to_string());
            rest = suffix;
        } else {
            rest = after;
        }
    }
    urls
}

fn insert_metadata(
    line: &str,
    url: Option<&str>,
    revision: bool,
) -> Option<String> {
    let checkbox = task_checkbox_body_start(line)?;
    let body = &line[checkbox..];
    let suffix = safe_metadata_suffix_start(body).unwrap_or(body.len());
    let point = checkbox + suffix;
    let left = &line[..point];
    let right = &line[point..];
    let has_revision = left.trim_end().ends_with("· revised");
    let mut insertion = String::new();
    if revision && !has_revision {
        if !left.chars().last().is_some_and(char::is_whitespace) {
            insertion.push(' ');
        }
        insertion.push_str("· revised");
    }
    if let Some(url) = url {
        if !insertion.is_empty()
            || !left.chars().last().is_some_and(char::is_whitespace)
        {
            insertion.push(' ');
        }
        insertion.push_str(&format!("[💡]({url} \"Open in Google Keep\")"));
    }
    if !insertion.is_empty()
        && !right.chars().next().is_some_and(char::is_whitespace)
    {
        insertion.push(' ');
    }
    Some(format!("{left}{insertion}{right}"))
}

fn add_revision_before_bulb(line: &str) -> Option<String> {
    let start = line.find("[💡](")?;
    let prefix = &line[..start];
    if prefix.trim_end().ends_with("· revised") {
        return Some(line.to_string());
    }
    Some(format!("{prefix}· revised {}", &line[start..]))
}

fn task_checkbox_body_start(line: &str) -> Option<usize> {
    let indent = line
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(line.len());
    let after = line.get(indent..)?.strip_prefix("- [")?;
    let symbol = after.chars().next()?;
    let after_symbol = indent + 3 + symbol.len_utf8();
    line.get(after_symbol..)?.strip_prefix("] ")?;
    Some(after_symbol + 2)
}

fn safe_metadata_suffix_start(body: &str) -> Option<usize> {
    let mut boundary = body.trim_end().len();
    let mut earliest = body.len();
    if let Some(index) = body.rfind(" ^")
        && body[index + 2..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !body[index + 2..].is_empty()
    {
        boundary = index + 1;
        earliest = boundary;
    }
    static FIELDS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| {
            regex::Regex::new(r"\[[A-Za-z][A-Za-z0-9_-]*::[^\]]*\]")
                .expect("metadata field regex")
        });
    static TRAILING_TAG: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| {
            regex::Regex::new(r##"(?:^|\s)#[^ !@#$%^&*(),.?":{}|<>]+$"##)
                .expect("trailing task tag regex")
        });
    static EMOJI_FIELD: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| {
            regex::Regex::new(
                r"(?:^|\s)(?:(?:⏫|🔼|🔽|🔺|⏬)\u{FE0F}?|(?:(?:🛫|➕|⏳|⌛|📅|📆|🗓|✅|❌)\u{FE0F}?\s+\d{4}-\d{2}-\d{2})|(?:🔁\u{FE0F}?\s+[a-zA-Z0-9, !]+)|(?:🏁\u{FE0F}?\s+[a-zA-Z]+)|(?:🆔\u{FE0F}?\s+[a-zA-Z0-9_-]+)|(?:⛔\u{FE0F}?\s+[a-zA-Z0-9_-]+(?:\s*,\s*[a-zA-Z0-9_-]+)*))$",
            )
            .expect("trailing Tasks metadata regex")
        });
    for _ in 0..24 {
        let end = body[..boundary].trim_end().len();
        if end == 0 {
            break;
        }
        let current = &body[..end];
        if let Some(tag) = TRAILING_TAG.find(current)
            && !inside_inline_code(current, tag.start())
        {
            earliest = earliest.min(tag.start());
            boundary = tag.start();
            continue;
        }
        if let Some(field) = FIELDS
            .find_iter(current)
            .filter(|field| !inside_inline_code(current, field.start()))
            .last()
            .filter(|field| current[field.end()..].trim().is_empty())
        {
            earliest = earliest.min(field.start());
            boundary = field.start();
            continue;
        }
        if let Some(field) = EMOJI_FIELD.find(current)
            && !inside_inline_code(current, field.start())
        {
            let token_start = field.start()
                + current[field.start()..field.end()]
                    .find(|character: char| !character.is_whitespace())
                    .unwrap_or(0);
            earliest = earliest.min(token_start);
            boundary = field.start();
            continue;
        }
        break;
    }
    (earliest < body.len()).then_some(earliest)
}

fn inside_inline_code(line: &str, point: usize) -> bool {
    let before = &line[..point.min(line.len())];
    before.matches('`').count() % 2 == 1
}

fn physical_lines(text: &str) -> Vec<PhysicalLine<'_>> {
    let mut offset = 0usize;
    text.split_inclusive('\n')
        .enumerate()
        .map(|(index, segment)| {
            let start = offset;
            offset += segment.len();
            let (without_lf, ending) = markdown::split_line_ending(segment);
            let content_end = start + without_lf.len();
            let _ = ending;
            PhysicalLine {
                index,
                start,
                content_end,
                end: offset,
                content: without_lf,
            }
        })
        .collect()
}

fn blocked_comment_and_quote_lines(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    frontmatter_end: Option<usize>,
) -> BTreeSet<usize> {
    let mut blocked = BTreeSet::new();
    let mut html_comment = false;
    let mut obsidian_comment = false;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let excluded = frontmatter_end.is_some_and(|end| index <= end)
            || fenced.contains(&index)
            || trimmed.starts_with('>')
            || html_comment
            || obsidian_comment
            || trimmed.starts_with("<!--")
            || markdown::atx_heading(line).is_some()
            || (trimmed.starts_with("%%") && !trimmed.starts_with("%%gkeep:"));
        if excluded {
            blocked.insert(index);
        }
        html_comment =
            update_multiline_comment(html_comment, line, "<!--", "-->");
        // A generated marker is a balanced single-line %% token and is not
        // treated as a comment region. Other Obsidian comments are excluded.
        if !line.contains("%%gkeep:") {
            obsidian_comment =
                update_multiline_comment(obsidian_comment, line, "%%", "%%");
        }
    }
    blocked
}

fn update_multiline_comment(
    open: bool,
    line: &str,
    start: &str,
    end: &str,
) -> bool {
    let mut state = open;
    let mut rest = line;
    loop {
        if state {
            if let Some(end_index) = rest.find(end) {
                rest = &rest[end_index + end.len()..];
                state = false;
            } else {
                return true;
            }
        } else if let Some(start_index) = rest.find(start) {
            let after_start = &rest[start_index + start.len()..];
            if let Some(end_index) = after_start.find(end) {
                rest = &after_start[end_index + end.len()..];
            } else {
                return true;
            }
        } else {
            return false;
        }
    }
}

fn line_has_descendants(index: usize, lines: &[PhysicalLine<'_>]) -> bool {
    let Some(indent) = list_item_indent(lines[index].content) else {
        return true;
    };
    for line in &lines[index + 1..] {
        if line.content.trim().is_empty() {
            continue;
        }
        let next_indent = line
            .content
            .find(|character: char| !character.is_whitespace())
            .unwrap_or(line.content.len());
        if next_indent <= indent.len() {
            return false;
        }
        return true;
    }
    false
}

fn task_block_text<'a>(
    text: &'a str,
    task: &note_tasks::NoteTask,
    lines: &[PhysicalLine<'a>],
) -> String {
    let start = lines
        .get(task.line_index)
        .map(|line| line.start)
        .unwrap_or(0);
    let end = task.block_end.min(text.len()).max(start);
    text[start..end].to_string()
}

fn apply_edits(text: &str, edits: &[Edit]) -> Option<String> {
    let mut ordered = edits.to_vec();
    ordered.sort_by(|left, right| right.start.cmp(&left.start));
    let mut previous_start = text.len();
    let mut output = text.to_string();
    for edit in ordered {
        if edit.start > edit.end
            || edit.end > text.len()
            || edit.end > previous_start
        {
            return None;
        }
        output.replace_range(edit.start..edit.end, &edit.replacement);
        previous_start = edit.start;
    }
    Some(output)
}

fn apply_task_local_edits(
    block: &str,
    block_start: usize,
    edits: &[Edit],
) -> Option<String> {
    let local = edits
        .iter()
        .filter(|edit| {
            edit.start >= block_start && edit.end <= block_start + block.len()
        })
        .map(|edit| Edit {
            start: edit.start - block_start,
            end: edit.end - block_start,
            replacement: edit.replacement.clone(),
        })
        .collect::<Vec<_>>();
    apply_edits(block, &local)
}

fn apply_file_plan(
    plan: &FilePlan,
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
    verified: &mut BTreeSet<(String, String)>,
    receipt_paths: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let current = fs::read(&plan.path)
        .map_err(|error| format!("re-read {}: {error}", plan.path.display()))?;
    if current != plan.before_bytes {
        return Err(
            "the note changed during migration; no write was installed".into(),
        );
    }
    if plan.after.as_bytes() == plan.before_bytes {
        return Ok(());
    }
    verify_semantics(&plan.before_bytes, &plan.after, settings)?;
    let after_sha = imports::sha256_hex(plan.after.as_bytes());
    let mut entries = Vec::new();
    let after_lines = physical_lines(&plan.after);
    let after_scan = note_tasks::scan(&plan.after, settings);
    for task_report in &plan.tasks {
        if task_report.skip.is_some() || task_report.before == task_report.after
        {
            continue;
        }
        let pairs: BTreeSet<_> = task_report
            .markers
            .iter()
            .map(|marker| (marker.id.clone(), marker.fp.clone()))
            .collect();
        for (id, fp) in pairs {
            if verified.contains(&(id.clone(), fp.clone())) {
                continue;
            }
            let Some(task) = after_scan.tasks().get(task_report.task_index)
            else {
                return Err(
                    "migrated task disappeared after write planning".into()
                );
            };
            let line_start = after_lines
                .get(task.line_index)
                .map(|line| line.start)
                .ok_or_else(|| "missing final task line".to_string())?;
            let block_end =
                task.block_end.min(plan.after.len()).max(line_start);
            let intended = plan.after[line_start..block_end].to_string();
            let block_digest = imports::block_digest(&intended);
            let url = task_report
                .markers
                .iter()
                .find(|marker| marker.id == id && marker.fp == fp)
                .and_then(|marker| marker.url.clone());
            entries.push(ImportEntry {
                id,
                fp,
                url,
                path: plan.rel.clone(),
                block_digest,
                intended: Some(intended),
                state: EntryState::Prepared,
                dest_digest: None,
            });
        }
    }
    let new_pairs: Vec<(String, String)> = entries
        .iter()
        .map(|entry| (entry.id.clone(), entry.fp.clone()))
        .collect();
    let transaction_id = format!("mt-{}", imports::new_transaction_id());
    let mut migration_receipt = MigrationReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        transaction_id: transaction_id.clone(),
        destination: plan.rel.clone(),
        before_sha256: imports::sha256_hex(&plan.before_bytes),
        after_sha256: after_sha.clone(),
        state: EntryState::Prepared,
    };
    let migration_receipt_path =
        persist_migration_receipt(bob_dir, &migration_receipt)?;
    let import_receipt_path = if entries.is_empty() {
        None
    } else {
        let tx = TransactionFile {
            schema_version: imports::SCHEMA_VERSION,
            transaction_id,
            destination: plan.rel.clone(),
            before_sha256: migration_receipt.before_sha256.clone(),
            after_sha256: Some(after_sha.clone()),
            baseline_counts: BTreeMap::new(),
            entries,
        };
        Some(
            imports::persist_prepared(bob_dir, &tx)
                .map_err(|error| format!("persist import evidence: {error}"))?,
        )
    };

    install_atomic(plan, &plan.after)?;
    let installed = fs::read(&plan.path)
        .map_err(|error| format!("verify {}: {error}", plan.path.display()))?;
    if installed != plan.after.as_bytes() {
        return Err(
            "installed note does not match the planned transformation".into()
        );
    }
    verify_semantics(&plan.before_bytes, &plan.after, settings)?;
    if let Some(tx_path) = import_receipt_path {
        let mut store = imports::read_all(bob_dir)
            .map_err(|error| format!("re-read import store: {error}"))?;
        let (_, tx) = store
            .iter_mut()
            .find(|(path, _)| *path == tx_path)
            .ok_or_else(|| {
                "prepared import evidence disappeared".to_string()
            })?;
        let mut completed = tx.clone();
        for entry in &mut completed.entries {
            entry.state = EntryState::Verified;
            entry.intended = None;
            entry.dest_digest = Some(after_sha.clone());
        }
        imports::persist_verified(&tx_path, &completed)
            .map_err(|error| format!("finalize import evidence: {error}"))?;
        receipt_paths.push(
            tx_path
                .strip_prefix(bob_dir)
                .unwrap_or(&tx_path)
                .to_path_buf(),
        );
    }
    migration_receipt.state = EntryState::Verified;
    persist_receipt_at(&migration_receipt_path, &migration_receipt)?;
    receipt_paths.push(
        migration_receipt_path
            .strip_prefix(bob_dir)
            .unwrap_or(&migration_receipt_path)
            .to_path_buf(),
    );
    verified.extend(new_pairs);
    Ok(())
}

fn install_atomic(plan: &FilePlan, contents: &str) -> Result<(), String> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let parent = plan.path.parent().unwrap_or_else(|| Path::new("."));
    let name = plan.path.file_name().unwrap_or_default().to_string_lossy();
    let temp = parent.join(format!(
        ".{name}.migrate-tasks.{}.{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut created = false;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| {
                format!("create temporary note {}: {error}", temp.display())
            })?;
        created = true;
        file.write_all(contents.as_bytes()).map_err(|error| {
            format!("write temporary note {}: {error}", temp.display())
        })?;
        file.set_permissions(plan.permissions.clone())
            .map_err(|error| {
                format!("preserve permissions on {}: {error}", temp.display())
            })?;
        file.sync_all().map_err(|error| {
            format!("sync temporary note {}: {error}", temp.display())
        })?;
        let current = fs::read(&plan.path).map_err(|error| {
            format!("re-read {}: {error}", plan.path.display())
        })?;
        if current != plan.before_bytes {
            return Err("the note changed during migration; prepared evidence is retained and no overwrite occurred".into());
        }
        fs::rename(&temp, &plan.path).map_err(|error| {
            format!("install {}: {error}", plan.path.display())
        })?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                format!("sync directory {}: {error}", parent.display())
            })?;
        Ok(())
    })();
    if created {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn verify_semantics(
    before: &[u8],
    after: &str,
    settings: &note_tasks::NoteTaskSettings,
) -> Result<(), String> {
    let before =
        std::str::from_utf8(before).map_err(|error| error.to_string())?;
    let before_scan = note_tasks::scan(before, settings);
    let after_scan = note_tasks::scan(after, settings);
    if before_scan.tasks().len() != after_scan.tasks().len() {
        return Err("task count changed during migration".into());
    }
    for (old, new) in before_scan.tasks().iter().zip(after_scan.tasks()) {
        if tasks_fingerprint(
            before.lines().nth(old.line_index).unwrap_or_default(),
        ) != tasks_fingerprint(
            after.lines().nth(new.line_index).unwrap_or_default(),
        ) {
            return Err(format!(
                "task semantics changed at line {}",
                old.line_index + 1
            ));
        }
    }
    Ok(())
}

fn pending_migration_commit_paths(
    bob_dir: &Path,
    files: &[(PathBuf, TransactionFile)],
    receipts: &[(PathBuf, MigrationReceipt)],
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for (_receipt_path, receipt) in receipts {
        if receipt.state != EntryState::Verified {
            continue;
        }
        let note = bob_dir.join(&receipt.destination);
        let Ok(bytes) = fs::read(&note) else { continue };
        if imports::sha256_hex(&bytes) == receipt.after_sha256 {
            paths.push(PathBuf::from(&receipt.destination));
        }
    }
    for (tx_path, tx) in files {
        if !tx.transaction_id.starts_with("mt-") || !imports::is_completed(tx) {
            continue;
        }
        let note = bob_dir.join(&tx.destination);
        let Ok(bytes) = fs::read(&note) else { continue };
        let sha = imports::sha256_hex(&bytes);
        if tx.after_sha256.as_deref() == Some(&sha)
            && tx
                .entries
                .iter()
                .all(|entry| entry.dest_digest.as_deref() == Some(&sha))
        {
            paths.push(PathBuf::from(&tx.destination));
            paths.push(
                tx_path
                    .strip_prefix(bob_dir)
                    .unwrap_or(tx_path)
                    .to_path_buf(),
            );
        }
    }
    paths
}

fn report(
    args: &MigrateTasksArgs,
    plans: &[FilePlan],
    counts: &Counts,
    commit: Option<String>,
    failures: &[(String, String)],
    dry_run: bool,
    styler: &Styler,
) -> i32 {
    let mut files = Vec::<Value>::new();
    let mut unresolved = 0usize;
    for plan in plans {
        let tasks: Vec<Value> = plan.tasks.iter().map(|task| {
            if task.skip.is_some() { unresolved += 1; }
            json!({
                "line": task.line,
                "title": task.title,
                "status": task.status,
                "before_markdown": task.before,
                "after_markdown": task.after,
                "markers": task.markers.iter().map(|marker| json!({"id": marker.id, "fp": marker.fp})).collect::<Vec<_>>(),
                "source_url": task.source_url,
                "url_less": task.url_less,
                "skip_reason": task.skip,
            })
        }).collect();
        if !tasks.is_empty() || !plan.edits.is_empty() {
            files.push(json!({"path": plan.rel, "tasks": tasks, "closed_tasks_excluded": plan.closed_tasks}));
        }
    }
    let failed = failures.len() > 0;
    let ok = !failed && unresolved == 0;
    if args.format.is_json() {
        if args.quiet && ok {
            return 0;
        }
        println!(
            "{}",
            json!({
                "schema_version": 1,
                "ok": ok,
                "dry_run": dry_run,
                "files": files,
                "summary": {
                    "planned": {"files": counts.planned_files, "tasks": counts.planned_tasks, "markers_removed": counts.planned_markers, "url_less": counts.planned_url_less},
                    "applied": {"files": counts.applied_files, "tasks": counts.applied_tasks, "markers_removed": counts.applied_markers, "url_less": counts.applied_url_less},
                    "closed_tasks_excluded": counts.closed_tasks,
                    "already_current": counts.already_current,
                    "skipped": unresolved,
                    "failed": failures.len()
                },
                "commit": commit,
                "failures": failures.iter().map(|(path, message)| json!({"path": path, "message": message})).collect::<Vec<_>>(),
            })
        );
        return if ok { 0 } else { 1 };
    }
    if args.quiet {
        if ok {
            return 0;
        }
        for (path, message) in failures {
            eprintln!("bob gkeep migrate-tasks: {path}: {message}");
        }
        for task in plans
            .iter()
            .flat_map(|plan| &plan.tasks)
            .filter(|task| task.skip.is_some())
        {
            eprintln!(
                "bob gkeep migrate-tasks: line {}: {}",
                task.line,
                task.skip.as_deref().unwrap_or("unresolved candidate")
            );
        }
        return 1;
    }
    println!("gkeep migrate-tasks{} · {} planned · {} applied · {} marker(s) · {} closed excluded · {} retained", if dry_run { " --dry-run" } else { "" }, counts.planned_tasks, counts.applied_tasks, if dry_run { counts.planned_markers } else { counts.applied_markers }, counts.closed_tasks, unresolved);
    for plan in plans {
        for task in &plan.tasks {
            if task.skip.is_some() {
                println!(
                    "  · {}:{} retained · {}",
                    plan.rel,
                    task.line,
                    task.skip.as_deref().unwrap_or("ambiguous")
                );
            } else if task.before != task.after {
                println!("  · {}:{} {}", plan.rel, task.line, task.title);
                if dry_run {
                    println!(
                        "    before: {}",
                        task.before.replace('\n', "\\n")
                    );
                    println!("    after:  {}", task.after.replace('\n', "\\n"));
                }
            }
        }
    }
    for (path, message) in failures {
        eprintln!("bob gkeep migrate-tasks: {path}: {message}");
    }
    if let Some(sha) = commit {
        println!(
            "{} vault commit {}",
            styler.success_prefix(false),
            sha.chars().take(7).collect::<String>()
        );
    }
    if dry_run {
        println!(
            "{} dry run: no writes, metadata, locks, or commits",
            styler.success_prefix(true)
        );
    }
    if ok {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_legacy_source_and_preserves_label_bytes() {
        let line = "  - Source: [Google Keep](https://keep.google.com/u/0/#NOTE/a%2520b) · 2026-09-27 21:14 · 🏷 errands, weekend · revised";
        let parsed = parse_legacy_source(line, None).expect("generated source");
        assert_eq!(
            parsed.url.as_deref(),
            Some("https://keep.google.com/u/0/#NOTE/a%2520b")
        );
        assert_eq!(parsed.labels.as_deref(), Some("errands, weekend"));
        assert!(parsed.revision);
        assert!(!parsed.label_ambiguous);
    }

    #[test]
    fn rejects_ambiguous_label_delimiters_and_unsupported_source_shape() {
        let line = "  - Source: [Google Keep](https://keep.google.com/n) · 2026-09-27 21:14 · 🏷 errands · weekend · revised";
        let parsed = parse_legacy_source(line, None)
            .expect("shape parses conservatively");
        assert!(parsed.label_ambiguous);
        assert!(parse_legacy_source(
            "  - Source: [Google Keep](https://keep.google.com/n) · today",
            None
        )
        .is_none());
    }

    #[test]
    fn inserts_bulb_before_inline_fields_and_block_id() {
        let line = "- [ ] #task Call dentist [created::2026-09-27] [due::2026-10-15] ^dentist";
        let after = insert_metadata(
            line,
            Some("https://keep.google.com/u/0/#NOTE/n"),
            false,
        )
        .unwrap();
        assert_eq!(after, "- [ ] #task Call dentist [💡](https://keep.google.com/u/0/#NOTE/n \"Open in Google Keep\") [created::2026-09-27] [due::2026-10-15] ^dentist");
        assert_eq!(tasks_fingerprint(line), tasks_fingerprint(&after));
    }

    #[test]
    fn edits_preserve_mixed_line_endings() {
        let source = "one\r\ntwo\nthree";
        let lines = physical_lines(source);
        assert_eq!(lines[0].content, "one");
        assert_eq!(&source[lines[0].content_end..lines[0].end], "\r\n");
        assert_eq!(&source[lines[1].content_end..lines[1].end], "\n");
        let edited = apply_edits(
            source,
            &[Edit {
                start: 5,
                end: 9,
                replacement: String::new(),
            }],
        )
        .unwrap();
        assert_eq!(edited, "one\r\nthree");
    }

    #[test]
    fn plans_compact_open_source_and_keeps_closed_source_unchanged() {
        let vault = tempfile::tempdir().expect("temp vault");
        let path = vault.path().join("tasks.md");
        let marker = ledger::format_marker("note-1", "0123456789ab");
        let input = format!(
            "- [ ] #task Call dentist [created::2026-09-27] [due::2026-10-15] ^dentist\n  - Ask about the crown\n  - Source: [Google Keep](https://keep.google.com/u/0/#NOTE/note-1) · 2026-09-27 21:14 · 🏷 errands {marker}\n- [x] #task Done task\n  - Source: [Google Keep](https://keep.google.com/u/0/#NOTE/closed) · 2026-09-20 08:00\n"
        );
        fs::write(&path, &input).expect("write fixture");
        let settings = note_tasks::read_settings(vault.path());
        let plan = scan_file(
            vault.path(),
            &path,
            input.as_bytes().to_vec(),
            &input,
            &settings,
        )
        .expect("scan")
        .expect("candidate file");
        assert!(plan.after.contains(
            "- [ ] #task Call dentist [💡](https://keep.google.com/u/0/#NOTE/note-1 \"Open in Google Keep\") [created::2026-09-27] [due::2026-10-15] ^dentist"
        ));
        assert!(plan.after.contains("  - 🏷 errands\n"));
        assert!(plan
            .after
            .contains("- [x] #task Done task\n  - Source: [Google Keep]"));
        assert!(!plan.after.contains(marker.as_str()));
        assert_eq!(plan.closed_tasks, 1);
        assert_eq!(plan.tasks.len(), 1);
        assert_eq!(plan.tasks[0].markers.len(), 1);
    }

    #[test]
    fn nested_closed_checkbox_blocks_parent_source_ownership() {
        let vault = tempfile::tempdir().expect("temp vault");
        let path = vault.path().join("nested.md");
        let input = concat!(
            "- [ ] #task Parent\n",
            "  - [x] #task Closed child\n",
            "    - Source: [Google Keep](https://keep.google.com/u/0/#NOTE/closed-child) · 2026-09-20 08:00\n",
            "  - Source: [Google Keep](https://keep.google.com/u/0/#NOTE/parent) · 2026-09-20 08:00\n",
            "- [x] #task Closed parent\n",
            "  - [ ] #task Open nested ^open-nested\n",
            "    - Source: [Google Keep](https://keep.google.com/u/0/#NOTE/open-nested) · 2026-09-20 08:00\n",
        );
        fs::write(&path, input).expect("write fixture");
        let settings = note_tasks::read_settings(vault.path());
        let plan = scan_file(
            vault.path(),
            &path,
            input.as_bytes().to_vec(),
            input,
            &settings,
        )
        .expect("scan")
        .expect("candidate file");
        assert!(plan.after.contains("    - Source: [Google Keep](https://keep.google.com/u/0/#NOTE/closed-child)"));
        assert!(plan.after.contains("- [x] #task Closed parent\n  - [ ] #task Open nested [💡](https://keep.google.com/u/0/#NOTE/open-nested \"Open in Google Keep\") ^open-nested"));
        assert!(plan.after.contains("- [ ] #task Parent [💡](https://keep.google.com/u/0/#NOTE/parent \"Open in Google Keep\")"));
        assert_eq!(plan.tasks.len(), 2);
        assert_eq!(plan.closed_tasks, 1);
    }

    #[test]
    fn marker_free_migration_keeps_the_legacy_import_store_empty() {
        let vault = tempfile::tempdir().expect("temp vault");
        let path = vault.path().join("tasks.md");
        let input = concat!(
            "- [ ] #task Call dentist [created::2026-09-27]\n",
            "  - Source: [Google Keep](https://keep.google.com/u/0/#NOTE/note-1) · 2026-09-27 21:14\n",
        );
        fs::write(&path, input).expect("write fixture");
        let settings = note_tasks::read_settings(vault.path());
        let plan = scan_file(
            vault.path(),
            &path,
            input.as_bytes().to_vec(),
            input,
            &settings,
        )
        .expect("scan")
        .expect("candidate file");
        let mut verified = BTreeSet::new();
        let mut receipt_paths = Vec::new();

        apply_file_plan(
            &plan,
            vault.path(),
            &settings,
            &mut verified,
            &mut receipt_paths,
        )
        .expect("apply marker-free migration");

        let store = imports::read_all(vault.path()).expect("import store");
        assert!(store.is_empty());
        assert!(fs::read_to_string(&path)
            .expect("read result")
            .contains("[💡](https://keep.google.com/u/0/#NOTE/note-1"));
        let receipts = read_migration_receipts(vault.path()).expect("journal");
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].1.state, EntryState::Verified);
        assert!(receipt_paths
            .iter()
            .any(|path| path.starts_with(RECEIPT_DIR)));
        assert!(pending_migration_commit_paths(
            vault.path(),
            &store,
            &receipts
        )
        .contains(&PathBuf::from("tasks.md")));
        assert!(!pending_migration_commit_paths(
            vault.path(),
            &store,
            &receipts
        )
        .iter()
        .any(|path| path.starts_with(RECEIPT_DIR)));
    }

    #[test]
    fn migration_journal_recovers_only_an_exact_file_post_image() {
        let vault = tempfile::tempdir().expect("temp vault");
        let before = "before\n";
        let after = "after\n";
        let note = vault.path().join("note.md");
        fs::write(&note, after).expect("write post-image");
        let receipt = MigrationReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            transaction_id: "mt-recovery-test".to_string(),
            destination: "note.md".to_string(),
            before_sha256: imports::sha256_hex(before.as_bytes()),
            after_sha256: imports::sha256_hex(after.as_bytes()),
            state: EntryState::Prepared,
        };
        let receipt_path = persist_migration_receipt(vault.path(), &receipt)
            .expect("prepared receipt");
        let receipts =
            read_migration_receipts(vault.path()).expect("read journal");

        let changed = resolve_migration_receipts(vault.path(), &receipts)
            .expect("recover exact post-image");

        let recovered = read_migration_receipts(vault.path())
            .expect("read finalized journal");
        assert_eq!(recovered[0].1.state, EntryState::Verified);
        assert!(changed.contains(&PathBuf::from("note.md")));
        assert!(receipt_path.exists());

        fs::write(&note, "later edit\n").expect("edit note");
        let mut unproven = recovered[0].1.clone();
        unproven.state = EntryState::Prepared;
        persist_receipt_at(&receipt_path, &unproven).expect("reopen receipt");
        let error = resolve_migration_receipts(
            vault.path(),
            &[(receipt_path.clone(), unproven)],
        )
        .expect_err("changed image must not be accepted");
        assert!(error.contains("matches neither its before-image"));
    }
}
