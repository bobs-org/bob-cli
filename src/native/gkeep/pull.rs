//! `bob gkeep pull`: guarded drain transaction.
//!
//! A Keep note is archived only when its current content fingerprint
//! matches a task block in the vault that was atomically written,
//! fsynced, re-read and parse-verified, and committed when the vault is
//! a Git worktree. A duplicate is always preferred over data loss.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};

use fs2::FileExt;
use serde_json::json;

use super::{
    adapter::{AdapterClient, Credentials},
    config::GkeepConfig,
    imports::{self, EntryState, ImportEntry, TransactionFile},
    ledger::{Journal, JournalEvent, JournalRecord, Ledger},
    model::{note_ref, ArchiveStatus, KeepContent, KeepNote},
    plan::{NoteState, Plan, PlanAction, PlanOptions, PlannedNote},
    render::{display_title, render_note, render_note_with_fallback},
    ui,
};
use super::{GkeepError, PullArgs};
use crate::native::highlights_ref::ingest::{
    ingest_url, IngestOutcome, IngestRequest,
};
use crate::native::url_routing::{
    library_verdicts, LibraryVerdict, UrlIntent, UrlRoutingPolicy,
};
use crate::native::{
    capture, env as bob_env, note_tasks, ob,
    style::{self, Styler},
};

/// One URL-only note's clip outcome from the pre-pass, plus the
/// canonical parent route selected for it before any clipping.
#[derive(Debug, Clone)]
struct ClipReport {
    id: String,
    intent: UrlIntent,
    parent: String,
    outcome: ClipOutcome,
}

/// The pre-pass result for one URL-only note.
#[derive(Debug, Clone)]
enum ClipOutcome {
    Created {
        pdf: String,
    },
    AlreadyInLibrary {
        note: String,
    },
    AlreadyQueued {
        pdf: String,
    },
    FailedRetryable {
        kind: String,
        message: String,
    },
    FailedPermanent {
        kind: String,
        message: String,
        fallback: String,
    },
    WouldClip {
        verdict: LibraryVerdict,
    },
}

impl ClipOutcome {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Created { .. } => "created",
            Self::AlreadyInLibrary { .. } => "already_in_library",
            Self::AlreadyQueued { .. } => "already_queued",
            Self::FailedRetryable { .. } => "failed_retryable",
            Self::FailedPermanent { .. } => "failed_permanent",
            Self::WouldClip { .. } => "would_clip",
        }
    }
}

/// The clip pre-pass: per-note reports plus the successful clips due
/// for archive `(id, Keep content, attachment count)`.
#[derive(Debug, Clone, Default)]
struct ClipSet {
    reports: Vec<ClipReport>,
    archives: Vec<(String, KeepContent, usize)>,
}

impl ClipSet {
    fn for_id(&self, id: &str) -> Option<&ClipReport> {
        self.reports.iter().find(|report| report.id == id)
    }

    /// Successful clips (an intake PDF or library note exists).
    fn clipped(&self) -> usize {
        self.reports
            .iter()
            .filter(|report| {
                matches!(report.outcome, ClipOutcome::Created { .. })
            })
            .count()
    }

    /// Retryable failures: the note stays in Keep and the run fails.
    fn failed_retryable(&self) -> usize {
        self.reports
            .iter()
            .filter(|report| {
                matches!(report.outcome, ClipOutcome::FailedRetryable { .. })
            })
            .count()
    }

    fn count(&self, outcome: &str) -> usize {
        self.reports
            .iter()
            .filter(|report| report.outcome.as_str() == outcome)
            .count()
    }
}

/// Load the URL routing policy for `pull`. `-R` disables routing;
/// a config error warns and disables it too (a bare URL then simply
/// stays a task). A `gkeep: false` toggle disables it silently.
fn load_gkeep_routing(no_ref: bool) -> Option<UrlRoutingPolicy> {
    if no_ref {
        return None;
    }
    match UrlRoutingPolicy::load() {
        Ok(policy) => {
            if policy.gkeep {
                Some(policy)
            } else {
                None
            }
        }
        Err(error) => {
            eprintln!("bob gkeep pull: warning: URL routing is off: {error}");
            None
        }
    }
}

/// Undocumented test hook (debug builds only): a shell snippet run just
/// before each compare-and-swap re-read so tests can simulate Obsidian
/// racing the rename.
#[cfg(debug_assertions)]
fn maybe_run_before_rename_hook() {
    if let Ok(script) = bob_env::var("BOB_GKEEP_TEST_BEFORE_RENAME")
        && !script.trim().is_empty()
    {
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg(&script)
            .status();
    }
}

#[cfg(not(debug_assertions))]
fn maybe_run_before_rename_hook() {}

fn pull_lock_path() -> PathBuf {
    bob_env::bob_cli_state_dir().join("gkeep").join("pull.lock")
}

fn journal_path() -> PathBuf {
    bob_env::bob_cli_state_dir()
        .join("gkeep")
        .join("journal.jsonl")
}

fn current_ts() -> String {
    ui::now_utc().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub(crate) fn run(args: &PullArgs) -> i32 {
    let is_json = args.format.is_json();
    let styler = Styler::detect();
    let format_name = args.error_format();

    // Guard: per-host pull lock, skipped for dry runs. Contention
    // reports "already running"; other I/O errors report the real error.
    let _pull_guard = if args.dry_run {
        None
    } else {
        match acquire_pull_lock() {
            Ok(guard) => Some(guard),
            Err(error) => {
                return ui::report_error("pull", &error, format_name);
            }
        }
    };

    // Resolve config, token, adapter, then snapshot Keep.
    let config = match GkeepConfig::resolve(None) {
        Ok(config) => config,
        Err(error) => return ui::report_error("pull", &error, format_name),
    };
    let (token, _) = match config.read_token() {
        Ok(pair) => pair,
        Err(error) => return ui::report_error("pull", &error, format_name),
    };
    let client = match AdapterClient::resolve(&config) {
        Ok(client) => client,
        Err(error) => return ui::report_error("pull", &error, format_name),
    };
    let creds = Credentials::from_config(&config, &token);
    let spinner_label = if is_json || args.quiet {
        None
    } else {
        Some("Syncing Google Keep")
    };
    let notes = match client.snapshot(&creds, false, spinner_label) {
        Ok(notes) => notes,
        Err(error) => return ui::report_error("pull", &error, format_name),
    };

    // Strict --id resolution before planning.
    if !args.id.is_empty()
        && let Err(error) = super::plan::resolve_ids(&notes, &args.id)
    {
        return ui::report_error("pull", &error, format_name);
    }

    let bob_dir = args.bob_dir();
    let target_rel = PathBuf::from(config.target());
    let target_path = config.target_path(&bob_dir);

    // Scan the ledger, vault import store, and journal before the vault
    // lock: the clip pre-pass below runs under the pull lock but outside
    // the vault lock, so long clips never block vault maintenance.
    // Malformed store files fail before any task write or archive call.
    let ledger = match Ledger::scan(&bob_dir) {
        Ok(ledger) => ledger,
        Err(error) => {
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    format!("scan the vault ledger: {error}"),
                ),
                format_name,
            );
        }
    };
    let store_files = match imports::read_all(&bob_dir) {
        Ok(files) => files,
        Err(error) => {
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    format!("read the gkeep import store: {error}"),
                ),
                format_name,
            );
        }
    };
    let journal_file = journal_path();
    let journal = match Journal::read(&journal_file) {
        Ok(journal) => journal,
        Err(error) => {
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    format!("read the gkeep journal: {error}"),
                ),
                format_name,
            );
        }
    };
    if journal.skipped > 0 {
        ui::warn(&format!(
            "skipped {} corrupt journal lines",
            journal.skipped
        ));
    }
    for group in &ledger.duplicates() {
        eprintln!(
            "warning duplicate gkeep marker {}:{} at {}",
            group.id,
            group.fp,
            group.locations.join(", ")
        );
    }
    if !imports::unfinished(&store_files).is_empty() && !args.dry_run {
        ui::warn(
            "unfinished gkeep import transaction(s) present; recovery runs under the vault lock",
        );
    }

    let routing = load_gkeep_routing(args.no_ref);
    let plan_opts = PlanOptions {
        include_pinned: args.include_pinned,
        include_shared: args.include_shared,
        ids: args.id.clone(),
        limit: args.limit.map(|value| value as usize),
        routing,
    };
    let plan = super::plan::classify_with_imports(
        &notes,
        &ledger,
        &journal,
        &store_files,
        &plan_opts,
    );

    // Select and remember Keep parents before clipping: note `@route`,
    // explicit `-P`, an interactive answer, then `gkeep_inbox`. An
    // invalid CLI parent fails before any clip or mutation.
    let parents = match select_keep_parents(
        &plan,
        &bob_dir,
        args.parent.as_deref(),
        args.quiet,
        is_json,
    ) {
        Ok(parents) => parents,
        Err(error) => return ui::report_error("pull", &error, format_name),
    };

    // Clip URL-only notes before the target check and the vault lock,
    // under the pull lock. A dry run only computes offline verdicts.
    let clips = run_clip_pre_pass(
        &plan,
        &bob_dir,
        &parents,
        args.dry_run,
        args.quiet,
        is_json,
    );

    // Dry runs stop at the plan: no locks, no clips, no writes, no
    // archive calls, no receipt writes, no marker cleanup. The indent
    // comes from the target when it reads, else the default. An
    // unresolved transaction is explained, never presented as safely
    // archived.
    if args.dry_run {
        if !imports::unfinished(&store_files).is_empty() {
            ui::warn(
                "unresolved gkeep import transaction(s) present; a real pull recovers them under the vault lock (dry-run changes nothing)",
            );
        }
        let indent = fs::read(&target_path)
            .ok()
            .map(|bytes| {
                let contents = String::from_utf8_lossy(&bytes).into_owned();
                capture::dominant_indent_unit(&capture::line_spans(&contents))
                    .unwrap_or("\t")
                    .to_string()
            })
            .unwrap_or_else(|| "\t".to_string());
        let writes = build_writes(&plan, &clips, &indent);
        return print_dry_run(args, &config, &plan, &writes, &clips, &styler);
    }

    // Task writes (including permanent clip-failure fallbacks) need
    // the target note. A pull with reading-queue notes (URL-only
    // `CreateRef`, clip reports, or `ref_created` archive-only notes)
    // never needs it otherwise — retryable failures stay in Keep and
    // crash-recovery archives need no task write. Runs with none of
    // those keep today's existence check.
    let needs_target = plan.notes.iter().any(|planned| {
        matches!(
            planned.action,
            PlanAction::Write | PlanAction::WriteRevision
        )
    }) || clips.reports.iter().any(|report| {
        matches!(report.outcome, ClipOutcome::FailedPermanent { .. })
    });
    let has_create_ref = plan
        .notes
        .iter()
        .any(|planned| matches!(planned.action, PlanAction::CreateRef));
    let has_ref_archive = {
        let ref_ids: std::collections::HashSet<&str> = journal
            .records
            .iter()
            .filter(|record| record.event == JournalEvent::RefCreated)
            .map(|record| record.id.as_str())
            .collect();
        plan.notes.iter().any(|planned| {
            matches!(planned.action, PlanAction::ArchiveOnly)
                && ref_ids.contains(planned.note.id.as_str())
        })
    };
    let has_ref_content =
        has_create_ref || !clips.reports.is_empty() || has_ref_archive;
    let all_clipped = !needs_target && !clips.archives.is_empty();
    let require_target = if needs_target || !has_ref_content {
        !all_clipped
    } else {
        false
    };
    if require_target && !target_path.is_file() {
        return ui::report_error(
            "pull",
            &GkeepError::setup(
                "target",
                format!(
                    "target note {} does not exist",
                    target_rel.to_string_lossy()
                ),
            )
            .with_hint("create it or set gkeep.target"),
            format_name,
        );
    }

    // Nothing to write (only pending/skipped, or clips alone): skip
    // write, verify, and commit, take no vault lock, and go straight
    // to the guarded archive — unless an unfinished transaction or a
    // verified-but-uncommitted receipt needs vault-lock recovery first.
    // A verified receipt is not proof of a Git commit.
    let child_env_early = ob::child_env();
    let git_worktree = if args.no_commit {
        false
    } else {
        match ob::detect_git_worktree(&bob_dir, &child_env_early) {
            Ok(inside) => inside,
            Err(error) => {
                if has_git_ancestor(&bob_dir) {
                    return ui::report_error(
                        "pull",
                        &GkeepError::runtime(
                            "commit",
                            format!("detect the vault Git worktree: {error}"),
                        ),
                        format_name,
                    );
                }
                false
            }
        }
    };
    let receipt_backed_archive = plan.notes.iter().any(|planned| {
        matches!(planned.action, PlanAction::ArchiveOnly)
            && imports::has_verified(
                &store_files,
                &planned.note.id,
                &planned.note.content.fingerprint(),
            )
    });
    if !needs_target
        && imports::unfinished(&store_files).is_empty()
        && !(git_worktree && receipt_backed_archive)
    {
        return finish_with_archive(
            args,
            &config,
            &client,
            &creds,
            &target_rel,
            &plan,
            &[],
            None,
            &clips,
            &styler,
        );
    }

    // Take `bob_sync.lock` before re-reading import evidence and the
    // target. (Obsidian ignores the lock, so CAS still guards the
    // write.) Newly synced vault records cannot bypass planning: history
    // is re-read under this lock and unfinished transactions resolve
    // before any fresh import is classified.
    let _vault_guard = {
        let waiting = format!(
            "  {}",
            styler.dim("waiting for another vault maintenance run…")
        );
        let json_mode = is_json;
        let on_first_wait = move || {
            if json_mode {
                eprintln!("waiting for another vault maintenance run…");
            } else {
                eprintln!("{waiting}");
            }
        };
        match ob::acquire_lock_waiting(
            std::time::Duration::from_secs(60),
            on_first_wait,
        ) {
            Ok(guard) => guard,
            Err(error) => {
                return ui::report_error(
                    "pull",
                    &GkeepError::runtime(
                        "lock",
                        format!("acquire vault maintenance lock: {error}"),
                    ),
                    format_name,
                );
            }
        }
    };

    // Re-read import evidence under the vault lock and resolve
    // unfinished transactions before classifying fresh imports. Slow
    // clipping stays outside the lock; completed clips are reused.
    let settings_for_recovery = note_tasks::read_settings(&bob_dir);
    let fresh_store =
        match resolve_unfinished(&bob_dir, &target_rel, &settings_for_recovery)
        {
            Ok(files) => files,
            Err(error) => return ui::report_error("pull", &error, format_name),
        };
    let fresh_ledger = match Ledger::scan(&bob_dir) {
        Ok(ledger) => ledger,
        Err(error) => {
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    format!("scan the vault ledger: {error}"),
                ),
                format_name,
            );
        }
    };
    let fresh_journal = match Journal::read(&journal_file) {
        Ok(journal) => journal,
        Err(error) => {
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    format!("read the gkeep journal: {error}"),
                ),
                format_name,
            );
        }
    };
    // Refresh the plan with newly synced history; clips are reused, never
    // redone. `plan` stays for reporting; `fresh_plan` drives writes.
    let fresh_plan = super::plan::classify_with_imports(
        &notes,
        &fresh_ledger,
        &fresh_journal,
        &fresh_store,
        &plan_opts,
    );
    let fresh_needs_target = fresh_plan.notes.iter().any(|planned| {
        matches!(
            planned.action,
            PlanAction::Write | PlanAction::WriteRevision
        )
    }) || clips.reports.iter().any(|report| {
        matches!(report.outcome, ClipOutcome::FailedPermanent { .. })
    });
    if !fresh_needs_target {
        // Archive-only recovery: a verified working-tree record alone must
        // not bypass the commit requirement. Complete the scoped commit
        // of outstanding receipt batches and their recorded destinations
        // before any archive.
        if !args.no_commit {
            let child_env = ob::child_env();
            let worktree = match ob::detect_git_worktree(&bob_dir, &child_env) {
                Ok(inside) => inside,
                Err(error) => {
                    if has_git_ancestor(&bob_dir) {
                        return ui::report_error(
                            "pull",
                            &GkeepError::runtime(
                                "commit",
                                format!(
                                    "detect the vault Git worktree: {error}"
                                ),
                            ),
                            format_name,
                        );
                    }
                    false
                }
            };
            if worktree {
                let message =
                    "bob gkeep pull: recover verified imports".to_string();
                match commit_outstanding_imports(
                    &bob_dir,
                    &child_env,
                    &message,
                    &fresh_store,
                    &fresh_plan,
                    &[],
                ) {
                    Ok(sha) => {
                        return finish_with_archive(
                            args,
                            &config,
                            &client,
                            &creds,
                            &target_rel,
                            &fresh_plan,
                            &[],
                            sha,
                            &clips,
                            &styler,
                        );
                    }
                    Err(error) => {
                        return ui::report_error("pull", &error, format_name);
                    }
                }
            }
        }
        return finish_with_archive(
            args,
            &config,
            &client,
            &creds,
            &target_rel,
            &fresh_plan,
            &[],
            None,
            &clips,
            &styler,
        );
    }

    let target_bytes = match fs::read(&target_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    format!("read {}: {error}", target_path.display()),
                ),
                format_name,
            );
        }
    };
    let target_contents = String::from_utf8_lossy(&target_bytes).into_owned();

    // Render new and revised notes with the target indent unit.
    // Permanent clip failures render as tasks with a ⚠️ child.
    // Fresh planning drives writes; the pre-lock `plan` stays for
    // reporting context where the two agree.
    let indent =
        capture::dominant_indent_unit(&capture::line_spans(&target_contents))
            .unwrap_or("\t")
            .to_string();
    let mut writes: Vec<WriteItem> = build_writes(&fresh_plan, &clips, &indent);

    // Marker-free transaction: persist a prepared record before
    // installing the target bytes. The record carries the intended
    // blocks, before/after SHAs, and baseline multiplicity counts.
    let settings = note_tasks::read_settings(&bob_dir);
    let base_counts = baseline_counts(&target_contents, &settings, &writes);
    let (first_contents, _) = {
        let joined = writes
            .iter()
            .map(|item| item.markdown.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        capture::insert_task_line(&target_contents, &joined)
    };
    let mut prepared = build_prepared_transaction(
        &writes,
        &target_rel,
        &target_bytes,
        &first_contents,
        &base_counts,
    );
    // CAS retry must update this record to describe the fresh input and
    // freshly indented output before rename; an aborted CAS never leaves
    // a verified record (we have not verified yet).
    if fault_injected("BOB_GKEEP_TEST_FAIL_AFTER_PREPARE") {
        return ui::report_error(
            "pull",
            &GkeepError::runtime(
                "vault",
                "injected failure after prepare".to_string(),
            ),
            format_name,
        );
    }
    let tx_rel_preview = PathBuf::from(imports::IMPORTS_DIR)
        .join(format!("{}.json", prepared.transaction_id));
    if !args.no_commit {
        let child_env = ob::child_env();
        // Preflight only when the vault is a worktree; non-Git vaults
        // still require durable verified records before archival.
        if ob::detect_git_worktree(&bob_dir, &child_env).unwrap_or(false)
            && let Err(error) = imports::preflight_trackable(
                &bob_dir,
                &child_env,
                &[target_rel.clone(), tx_rel_preview.clone()],
            )
        {
            return ui::report_error("pull", &error.into_gkeep(), format_name);
        }
    }
    let tx_path = match imports::persist_prepared(&bob_dir, &prepared) {
        Ok(path) => path,
        Err(error) => {
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    format!("persist the gkeep import store: {error}"),
                ),
                format_name,
            );
        }
    };
    let tx_rel: PathBuf = tx_path
        .strip_prefix(&bob_dir)
        .map(|relative| relative.to_path_buf())
        .unwrap_or_else(|_| tx_rel_preview.clone());
    // Build the insertion once; CAS re-reads immediately before the
    // rename, after the temp file is written and synced.
    let joined = writes
        .iter()
        .map(|item| item.markdown.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let (first_contents, _) =
        capture::insert_task_line(&target_contents, &joined);
    let perms = match fs::metadata(&target_path) {
        Ok(meta) => meta.permissions(),
        Err(error) => {
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    format!("stat {}: {error}", target_path.display()),
                ),
                format_name,
            );
        }
    };
    let mut temp = match write_temp(&target_path, &first_contents, &perms) {
        Ok(temp) => temp,
        Err(error) => {
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    format!("write {}: {error}", target_path.display()),
                ),
                format_name,
            );
        }
    };
    maybe_run_before_rename_hook();
    let current_bytes = match fs::read(&target_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = fs::remove_file(&temp);
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    format!("re-read {}: {error}", target_path.display()),
                ),
                format_name,
            );
        }
    };
    if current_bytes != target_bytes {
        // The vault changed under us: delete the temp, re-render with the
        // fresh indent, and update the prepared record to describe the
        // fresh input and freshly indented output before rename. An
        // aborted CAS never leaves a verified record.
        let _ = fs::remove_file(&temp);
        let fresh = String::from_utf8_lossy(&current_bytes).into_owned();
        let fresh_indent =
            capture::dominant_indent_unit(&capture::line_spans(&fresh))
                .unwrap_or("\t")
                .to_string();
        let mut fresh_writes = Vec::with_capacity(writes.len());
        for item in &writes {
            let revision = matches!(item.planned.state, NoteState::Revised);
            let block = if matches!(item.planned.action, PlanAction::CreateRef)
            {
                // A permanent clip-failure fallback: re-render with the
                // fresh indent, keeping the ⚠️ child.
                let fallback = clips
                    .for_id(&item.planned.note.id)
                    .and_then(|report| match &report.outcome {
                        ClipOutcome::FailedPermanent { fallback, .. } => {
                            Some(fallback.as_str())
                        }
                        _ => None,
                    })
                    .unwrap_or("");
                render_note_with_fallback(
                    &item.planned.note,
                    &fresh_indent,
                    revision,
                    fallback,
                )
            } else {
                render_note(&item.planned.note, &fresh_indent, revision)
            };
            fresh_writes.push(WriteItem {
                planned: item.planned.clone(),
                markdown: block.markdown,
                fp: item.fp.clone(),
            });
        }
        let fresh_joined = fresh_writes
            .iter()
            .map(|item| item.markdown.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let (replanned, _) = capture::insert_task_line(&fresh, &fresh_joined);
        // Update the prepared record to the fresh input/output before the
        // second rename attempt, keeping the original transaction id so
        // recovery sees one batch, not two. An aborted CAS never leaves a
        // verified record.
        {
            let fresh_base = baseline_counts(&fresh, &settings, &fresh_writes);
            let fresh_after = imports::sha256_hex(replanned.as_bytes());
            prepared.before_sha256 = imports::sha256_hex(&current_bytes);
            prepared.after_sha256 = Some(fresh_after);
            prepared.baseline_counts = fresh_base;
            prepared.entries = fresh_writes
                .iter()
                .map(|item| {
                    let url = item
                        .planned
                        .note
                        .url
                        .clone()
                        .filter(|url| !url.trim().is_empty());
                    ImportEntry {
                        id: item.planned.note.id.clone(),
                        fp: item.fp.clone(),
                        url,
                        path: target_rel.to_string_lossy().replace('\\', "/"),
                        block_digest: imports::block_digest(&item.markdown),
                        intended: Some(item.markdown.clone()),
                        state: EntryState::Prepared,
                        dest_digest: None,
                    }
                })
                .collect();
            prepared.destination =
                target_rel.to_string_lossy().replace('\\', "/");
            if let Err(error) = imports::persist_verified(&tx_path, &prepared) {
                let _ = fs::remove_file(&temp);
                return ui::report_error(
                    "pull",
                    &GkeepError::runtime(
                        "vault",
                        format!("update the gkeep import store: {error}"),
                    ),
                    format_name,
                );
            }
        }
        temp = match write_temp(&target_path, &replanned, &perms) {
            Ok(temp) => temp,
            Err(error) => {
                return ui::report_error(
                    "pull",
                    &GkeepError::runtime(
                        "vault",
                        format!("write {}: {error}", target_path.display()),
                    ),
                    format_name,
                );
            }
        };
        maybe_run_before_rename_hook();
        let second = match fs::read(&target_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return ui::report_error(
                    "pull",
                    &GkeepError::runtime(
                        "vault",
                        format!("re-read {}: {error}", target_path.display()),
                    ),
                    format_name,
                );
            }
        };
        if second != current_bytes {
            let _ = fs::remove_file(&temp);
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "conflict",
                    "the target note changed during pull; re-run `bob gkeep pull`"
                        .to_string(),
                ),
                format_name,
            );
        }
        // Use the fresh rendering from here on (indent may differ).
        writes = fresh_writes;
    }
    // Rename the synced temp immediately after the successful re-read.
    if let Err(error) = finish_rename(&temp, &target_path) {
        let _ = fs::remove_file(&temp);
        return ui::report_error(
            "pull",
            &GkeepError::runtime(
                "vault",
                format!("write {}: {error}", target_path.display()),
            ),
            format_name,
        );
    }
    if fault_injected("BOB_GKEEP_TEST_FAIL_AFTER_TARGET") {
        return ui::report_error(
            "pull",
            &GkeepError::runtime(
                "vault",
                "injected failure after target install".to_string(),
            ),
            format_name,
        );
    }
    // Verify: complete parsed blocks at task boundaries, open top-level
    // tasks, baseline-plus-inserted multiplicity. Never URL, substring,
    // or fingerprint presence alone.
    let verified_contents = match fs::read_to_string(&target_path) {
        Ok(text) => text,
        Err(error) => {
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    format!("re-read {}: {error}", target_path.display()),
                ),
                format_name,
            );
        }
    };
    let verify_outcome = verify_writes(
        &verified_contents,
        &writes,
        &settings,
        &prepared.baseline_counts,
    );
    let verified: Vec<WriteItem> = writes
        .iter()
        .enumerate()
        .filter(|(index, _)| verify_outcome.ok[*index])
        .map(|(_, item)| item.clone())
        .collect();
    let verify_failed = writes.len() - verified.len();
    let verified_ids: std::collections::BTreeSet<String> = verified
        .iter()
        .map(|item| item.planned.note.id.clone())
        .collect();

    // Persist verified receipts only for successful imports. Distinct
    // Keep ids that render identical Markdown share one multiplicity
    // check; failed entries retain prepared evidence and never inherit a
    // sibling's state. Completed entries never change.
    {
        let dest_sha = imports::sha256_hex(verified_contents.as_bytes());
        let mut finalized = prepared.clone();
        finalized.after_sha256 = Some(dest_sha.clone());
        for entry in &mut finalized.entries {
            let ok = verified_ids.contains(&entry.id)
                && verified.iter().any(|item| {
                    item.planned.note.id == entry.id && item.fp == entry.fp
                });
            if ok {
                entry.state = EntryState::Verified;
                entry.intended = None;
                entry.dest_digest = Some(dest_sha.clone());
            }
        }
        if fault_injected("BOB_GKEEP_TEST_FAIL_AFTER_VERIFY") {
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    "injected failure after verify".to_string(),
                ),
                format_name,
            );
        }
        if let Err(error) = imports::persist_verified(&tx_path, &finalized) {
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    format!("persist the gkeep import store: {error}"),
                ),
                format_name,
            );
        }
        prepared = finalized;
        if fault_injected("BOB_GKEEP_TEST_FAIL_AFTER_RECEIPT") {
            return ui::report_error(
                "pull",
                &GkeepError::runtime(
                    "vault",
                    "injected failure after receipt".to_string(),
                ),
                format_name,
            );
        }
    }

    // Commit when the vault is a Git worktree, unless --no-commit.
    // Non-Git and --no-commit paths still require durable verified
    // records before archival (persisted above). When `git` cannot be
    // started, only vaults that really are repos fail: without a `.git`
    // ancestor the vault simply is not a worktree.
    let mut commit_sha: Option<String> = None;
    if !args.no_commit {
        let child_env = ob::child_env();
        let worktree = match ob::detect_git_worktree(&bob_dir, &child_env) {
            Ok(inside) => inside,
            Err(error) => {
                if has_git_ancestor(&bob_dir) {
                    return ui::report_error(
                        "pull",
                        &GkeepError::runtime(
                            "commit",
                            format!("detect the vault Git worktree: {error}"),
                        ),
                        format_name,
                    );
                }
                false
            }
        };
        if worktree {
            // Finish the scoped commit of the affected target and this
            // operation's metadata before archiving. Other dirty or staged
            // files remain untouched. A failed commit leaves Keep
            // untouched for the affected imports.
            if fault_injected("BOB_GKEEP_TEST_FAIL_AFTER_COMMIT") {
                return ui::report_error(
                    "pull",
                    &GkeepError::runtime(
                        "commit",
                        "injected commit failure".to_string(),
                    ),
                    format_name,
                );
            }
            let count = verified.len();
            let message =
                format!("bob gkeep pull: {count} notes from Google Keep");
            let extra = vec![target_rel.clone(), tx_rel.clone()];
            let store_now = imports::read_all(&bob_dir).unwrap_or_default();
            match commit_outstanding_imports(
                &bob_dir,
                &child_env,
                &message,
                &store_now,
                &fresh_plan,
                &extra,
            ) {
                Ok(sha) => {
                    commit_sha = sha;
                    // Re-read the committed target evidence so an
                    // intervening editor save cannot be accepted as the
                    // verified write. If the committed content cannot be
                    // established, stop and preserve evidence.
                    if commit_sha.is_some() {
                        let committed = std::process::Command::new("git")
                            .arg("-C")
                            .arg(&bob_dir)
                            .arg("show")
                            .arg(format!(
                                "HEAD:{}",
                                target_rel.to_string_lossy().replace('\\', "/")
                            ))
                            .output();
                        if let Ok(output) = committed
                            && output.status.success()
                        {
                            let text = String::from_utf8_lossy(&output.stdout)
                                .into_owned();
                            let committed_outcome = verify_writes(
                                &text,
                                &verified,
                                &settings,
                                &prepared.baseline_counts,
                            );
                            if committed_outcome.ok.iter().any(|ok| !ok) {
                                return ui::report_error(
                                    "pull",
                                    &GkeepError::runtime(
                                        "vault",
                                        "the target note changed during commit and the verified content cannot be established; evidence retained, Keep untouched".to_string(),
                                    ),
                                    format_name,
                                );
                            }
                        }
                    }
                }
                Err(error) => {
                    return ui::report_error("pull", &error, format_name);
                }
            }
        }
    }
    drop(_vault_guard);

    if verify_failed > 0 {
        // Verified notes still archive; failures are reported with exit 1.
        // Failed entries remain recoverable and cannot become
        // archive-only on the next run (they stay prepared).
        let code = finish_with_archive(
            args,
            &config,
            &client,
            &creds,
            &target_rel,
            &fresh_plan,
            &verified,
            commit_sha.clone(),
            &clips,
            &styler,
        );
        if !args.quiet {
            for (index, item) in writes.iter().enumerate() {
                if !verify_outcome.ok[index] {
                    eprintln!(
                        "bob gkeep pull: verification failed for {}",
                        item.planned.note.id
                    );
                }
            }
        }
        return code.max(1);
    }

    finish_with_archive(
        args,
        &config,
        &client,
        &creds,
        &target_rel,
        &fresh_plan,
        &verified,
        commit_sha,
        &clips,
        &styler,
    )
}

#[derive(Debug, Clone)]
struct WriteItem {
    planned: PlannedNote,
    markdown: String,
    fp: String,
}

/// Build task writes: new/revised notes plus permanent clip-failure
/// fallbacks, with the ⚠️ child after the note's ordinary children.
fn build_writes(plan: &Plan, clips: &ClipSet, indent: &str) -> Vec<WriteItem> {
    let mut writes: Vec<WriteItem> = Vec::new();
    for planned in &plan.notes {
        let revision = matches!(planned.state, NoteState::Revised);
        if matches!(
            planned.action,
            PlanAction::Write | PlanAction::WriteRevision
        ) {
            let block = render_note(&planned.note, indent, revision);
            writes.push(WriteItem {
                planned: planned.clone(),
                markdown: block.markdown,
                fp: planned.note.content.fingerprint(),
            });
        } else if matches!(planned.action, PlanAction::CreateRef)
            && let Some(report) = clips.for_id(&planned.note.id)
            && let ClipOutcome::FailedPermanent { fallback, .. } =
                &report.outcome
        {
            let block = render_note_with_fallback(
                &planned.note,
                indent,
                revision,
                fallback,
            );
            writes.push(WriteItem {
                planned: planned.clone(),
                markdown: block.markdown,
                fp: planned.note.content.fingerprint(),
            });
        }
    }
    writes
}

/// Choose the parent for each new URL: a valid note `@route`, explicit
/// `-P`, an interactive answer, then `gkeep_inbox`. All explicit values
/// use the strict resolver and canonical route. An invalid note route
/// warns with resolver hints and falls through; an invalid CLI parent is
/// an error before clipping. Selection applies only to references.
fn select_keep_parents(
    plan: &Plan,
    bob_dir: &Path,
    cli_parent: Option<&str>,
    quiet: bool,
    is_json: bool,
) -> Result<HashMap<String, String>, GkeepError> {
    let mut parents = HashMap::new();
    let targets: Vec<&PlannedNote> = plan
        .notes
        .iter()
        .filter(|planned| {
            matches!(planned.action, PlanAction::CreateRef)
                && planned.ref_intent.is_some()
        })
        .collect();
    if targets.is_empty() {
        return Ok(parents);
    }
    // An invalid CLI parent fails before any clipping or mutation.
    let cli_canonical = if let Some(raw) = cli_parent {
        match crate::native::parent_notes::resolve_parent(bob_dir, raw) {
            Ok(resolved) => Some(resolved.route),
            Err(error) => {
                return Err(GkeepError::setup(
                    "invalid_parent",
                    error.message(),
                ));
            }
        }
    } else {
        None
    };
    let prompt_eligible = std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
        && !quiet
        && !is_json;
    let mut sticky_default = "gkeep_inbox".to_string();
    let mut eof_sticky = false;
    for planned in targets {
        // A valid note `@route` wins over `-P` and the prompt.
        if let Some(route_token) = planned.ref_route.as_deref() {
            match crate::native::parent_notes::resolve_parent(
                bob_dir,
                route_token,
            ) {
                Ok(resolved) => {
                    parents.insert(planned.note.id.clone(), resolved.route);
                    continue;
                }
                Err(error) => {
                    ui::warn(&format!(
                        "ignoring @{} on {}: {}",
                        route_token,
                        planned.ref_.as_str(),
                        error.message().replace('\n', " · "),
                    ));
                }
            }
        }
        if let Some(cli) = cli_canonical.clone() {
            parents.insert(planned.note.id.clone(), cli);
            continue;
        }
        if prompt_eligible && !eof_sticky {
            let intent = planned.ref_intent.clone().expect("filtered intent");
            match prompt_keep_parent(bob_dir, &intent.display, &sticky_default)
            {
                PromptAnswer::Parent(canonical) => {
                    sticky_default = canonical.clone();
                    parents.insert(planned.note.id.clone(), canonical);
                    continue;
                }
                PromptAnswer::Eof => {
                    eof_sticky = true;
                    parents.insert(
                        planned.note.id.clone(),
                        sticky_default.clone(),
                    );
                    continue;
                }
            }
        }
        parents.insert(
            planned.note.id.clone(),
            cli_canonical
                .clone()
                .unwrap_or_else(|| sticky_default.clone()),
        );
    }
    // A non-TTY, quiet, or JSON run without higher-precedence parents
    // files everything under the inbox default.
    Ok(parents)
}

enum PromptAnswer {
    Parent(String),
    Eof,
}

/// Ask per new URL that lacks a higher-precedence parent:
/// `File example.com/essay under [gkeep_inbox]: `. Enter accepts the
/// default; each accepted answer becomes the next prompt's default.
/// Invalid answers print hints and re-prompt. EOF uses the default
/// without looping.
fn prompt_keep_parent(
    bob_dir: &Path,
    display: &str,
    default: &str,
) -> PromptAnswer {
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    prompt_keep_parent_from(bob_dir, &mut handle, display, default)
}

fn prompt_keep_parent_from(
    bob_dir: &Path,
    reader: &mut dyn std::io::BufRead,
    display: &str,
    default: &str,
) -> PromptAnswer {
    loop {
        eprint!("File {display} under [{default}]: ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return PromptAnswer::Eof,
            Ok(_) => {}
            Err(_) => return PromptAnswer::Eof,
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return PromptAnswer::Parent(default.to_string());
        }
        match crate::native::parent_notes::resolve_parent(bob_dir, trimmed) {
            Ok(resolved) => return PromptAnswer::Parent(resolved.route),
            Err(error) => {
                eprintln!(
                    "bob gkeep pull: {}",
                    error.message().replace('\n', "\n  hint: ")
                );
            }
        }
    }
}

/// Clip every `CreateRef` note sequentially, before the vault lock. A
/// dry run only computes offline verdicts and clips nothing. Terminal
/// outcomes (`Created`, `AlreadyInLibrary`, `AlreadyQueued`) append
/// one `ref_created` journal batch per clip immediately, so a crash
/// between clip and archive re-pulls as `ArchiveOnly`.
fn run_clip_pre_pass(
    plan: &Plan,
    bob_dir: &Path,
    parents: &HashMap<String, String>,
    dry_run: bool,
    quiet: bool,
    is_json: bool,
) -> ClipSet {
    let targets: Vec<&PlannedNote> = plan
        .notes
        .iter()
        .filter(|planned| {
            matches!(planned.action, PlanAction::CreateRef)
                && planned.ref_intent.is_some()
        })
        .collect();
    if targets.is_empty() {
        return ClipSet::default();
    }
    if dry_run {
        let intents: Vec<&UrlIntent> = targets
            .iter()
            .filter_map(|planned| planned.ref_intent.as_ref())
            .collect();
        let verdicts = library_verdicts(bob_dir, &intents);
        let reports = targets
            .iter()
            .zip(verdicts)
            .map(|(planned, verdict)| ClipReport {
                id: planned.note.id.clone(),
                intent: planned.ref_intent.clone().expect("filtered intent"),
                parent: parents
                    .get(&planned.note.id)
                    .cloned()
                    .unwrap_or_else(|| "gkeep_inbox".to_string()),
                outcome: ClipOutcome::WouldClip { verdict },
            })
            .collect();
        return ClipSet {
            reports,
            archives: Vec::new(),
        };
    }
    let silent = quiet || is_json;
    let total = targets.len();
    let mut clips = ClipSet::default();
    for (index, planned) in targets.iter().enumerate() {
        let intent = planned.ref_intent.clone().expect("filtered intent");
        let parent = parents
            .get(&planned.note.id)
            .cloned()
            .unwrap_or_else(|| "gkeep_inbox".to_string());
        let _spinner = if silent {
            None
        } else {
            Some(ui::Spinner::start(&format!(
                "Clipping {} ({}/{})",
                intent.display,
                index + 1,
                total,
            )))
        };
        let report_progress = |message: &str| {
            eprintln!("  {message}");
        };
        let progress: Option<&dyn Fn(&str)> =
            if silent { None } else { Some(&report_progress) };
        let request = IngestRequest {
            bob_dir,
            url: &intent.cleaned,
            parent: parent.as_str(),
            progress,
        };
        // Clone for the error path: `request` borrows `parent`.
        let parent_for_error = parent.clone();
        let outcome = match ingest_url(&request) {
            Ok(ingest) => match ingest {
                IngestOutcome::Created { pdf, .. } => {
                    ClipOutcome::Created { pdf }
                }
                IngestOutcome::AlreadyInLibrary { note } => {
                    ClipOutcome::AlreadyInLibrary { note }
                }
                IngestOutcome::AlreadyQueued { pdf } => {
                    ClipOutcome::AlreadyQueued { pdf }
                }
            },
            Err(error) => {
                if error.retryable() {
                    ClipOutcome::FailedRetryable {
                        kind: error.kind.as_str().to_string(),
                        message: first_line(&error.message),
                    }
                } else {
                    let fallback =
                        error.fallback_note(&intent.cleaned, &parent_for_error);
                    ClipOutcome::FailedPermanent {
                        kind: error.kind.as_str().to_string(),
                        message: first_line(&error.message),
                        fallback,
                    }
                }
            }
        };
        // Terminal outcomes join the archive set and journal now, one
        // batch per clip, recording the selected parent for replay.
        match &outcome {
            ClipOutcome::Created { pdf } => {
                append_ref_created(
                    &planned.note,
                    pdf,
                    Some(intent.cleaned.clone()),
                    Some(parent.clone()),
                );
                clips.archives.push((
                    planned.note.id.clone(),
                    planned.note.content.clone(),
                    planned.note.attachments.len(),
                ));
            }
            ClipOutcome::AlreadyInLibrary { note } => {
                append_ref_created(
                    &planned.note,
                    note,
                    Some(intent.cleaned.clone()),
                    Some(parent.clone()),
                );
                clips.archives.push((
                    planned.note.id.clone(),
                    planned.note.content.clone(),
                    planned.note.attachments.len(),
                ));
            }
            ClipOutcome::AlreadyQueued { pdf } => {
                append_ref_created(
                    &planned.note,
                    pdf,
                    Some(intent.cleaned.clone()),
                    Some(parent.clone()),
                );
                clips.archives.push((
                    planned.note.id.clone(),
                    planned.note.content.clone(),
                    planned.note.attachments.len(),
                ));
            }
            ClipOutcome::FailedRetryable { .. }
            | ClipOutcome::FailedPermanent { .. }
            | ClipOutcome::WouldClip { .. } => {}
        }
        clips.reports.push(ClipReport {
            id: planned.note.id.clone(),
            intent,
            parent,
            outcome,
        });
    }
    clips
}

/// Append one `ref_created` journal batch: `path` is the intake PDF or
/// the existing ref note, `url` the clipped URL, `parent` the canonical
/// route the pull selected for this URL.
fn append_ref_created(
    note: &KeepNote,
    path: &str,
    url: Option<String>,
    parent: Option<String>,
) {
    let record = JournalRecord {
        ts: current_ts(),
        event: JournalEvent::RefCreated,
        id: note.id.clone(),
        ref_: note_ref(&note.id),
        fp: note.content.fingerprint(),
        path: path.to_string(),
        commit: None,
        status: None,
        url,
        parent,
    };
    if let Err(error) = Journal::append(&journal_path(), &[record]) {
        ui::warn(&format!("append the gkeep journal: {error}"));
    }
}

/// First line of a message, for compact clip reports.
fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or_default().to_string()
}

fn acquire_pull_lock() -> Result<File, GkeepError> {
    let path = pull_lock_path();
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return Err(GkeepError::runtime(
            "lock",
            format!("create pull lock dir {}: {error}", parent.display()),
        ));
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
                "another bob gkeep pull is already running".to_string(),
            ))
        }
        Err(error) => Err(GkeepError::runtime(
            "lock",
            format!("lock {}: {error}", path.display()),
        )),
    }
}

/// Whether `dir` or any ancestor contains a `.git` entry.
///
/// Used when `git` cannot be started: a vault with no `.git` ancestor is
/// simply not a worktree, while one with an ancestor keeps the commit
/// failure.
fn has_git_ancestor(dir: &Path) -> bool {
    let mut current = Some(dir);
    while let Some(path) = current {
        let dot_git = path.join(".git");
        if dot_git.is_dir() || dot_git.is_file() {
            return true;
        }
        current = path.parent();
    }
    false
}

/// Write `contents` to a same-dir temp file, set permissions before
/// syncing, sync, and return the temp path. The caller re-reads the
/// target for CAS and renames with [`finish_rename`] immediately after.
fn write_temp(
    target: &Path,
    contents: &str,
    perms: &std::fs::Permissions,
) -> io::Result<PathBuf> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let file_name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "gkeep_inbox.md".to_string());
    let mut attempt = 0;
    let temp = loop {
        let candidate = parent
            .join(format!(".{file_name}.tmp.{}-{attempt}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                drop(file);
                break candidate;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                attempt += 1;
                if attempt > 100 {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    };
    let outcome = (|| -> io::Result<()> {
        fs::write(&temp, contents)?;
        fs::set_permissions(&temp, perms.clone())?;
        let file = File::open(&temp)?;
        file.sync_all()?;
        Ok(())
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&temp);
    }
    outcome.map(|()| temp)
}

/// Rename a synced temp file onto `target` and fsync the parent dir.
fn finish_rename(temp: &Path, target: &Path) -> io::Result<()> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    fs::rename(temp, target)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

/// Build a prepared transaction for the current batch.
///
/// `before_bytes` is the target's current bytes; `intended_after` is the
/// bytes about to be installed. Baseline counts cover the distinct write
/// digests in the before-image for identical-rendering verification.
fn build_prepared_transaction(
    writes: &[WriteItem],
    target_rel: &Path,
    before_bytes: &[u8],
    intended_after: &str,
    baseline: &std::collections::BTreeMap<String, usize>,
) -> TransactionFile {
    let entries = writes
        .iter()
        .map(|item| {
            let url = item
                .planned
                .note
                .url
                .clone()
                .filter(|url| !url.trim().is_empty());
            ImportEntry {
                id: item.planned.note.id.clone(),
                fp: item.fp.clone(),
                url,
                path: target_rel.to_string_lossy().replace('\\', "/"),
                block_digest: imports::block_digest(&item.markdown),
                intended: Some(item.markdown.clone()),
                state: EntryState::Prepared,
                dest_digest: None,
            }
        })
        .collect();
    TransactionFile {
        schema_version: imports::SCHEMA_VERSION,
        transaction_id: imports::new_transaction_id(),
        destination: target_rel.to_string_lossy().replace('\\', "/"),
        before_sha256: imports::sha256_hex(before_bytes),
        after_sha256: Some(imports::sha256_hex(intended_after.as_bytes())),
        baseline_counts: baseline.clone(),
        entries,
    }
}

/// Resolve unfinished transactions under the vault lock.
///
/// Returns the fresh store listing after recovery. Each boundary:
/// - prepared + target still equals before-image: no import happened;
///   the stale prepared file is removed so the batch safely retries.
/// - target installed + record still prepared: parse-verify the recorded
///   intended result and finalize without appending again.
/// - verified + commit pending: the caller commits before any archive
///   (handled by the commit step, not here).
/// - target edited/moved and the intended result cannot be proven: stop
///   with a precise diagnostic, retaining evidence and Keep content.
fn resolve_unfinished(
    bob_dir: &Path,
    target_rel: &Path,
    settings: &note_tasks::NoteTaskSettings,
) -> Result<Vec<(PathBuf, TransactionFile)>, GkeepError> {
    let mut files = imports::read_all(bob_dir).map_err(|error| {
        GkeepError::runtime(
            "vault",
            format!("read the gkeep import store: {error}"),
        )
    })?;
    // Work on a snapshot of unfinished paths; mutation updates `files`.
    let unfinished_paths: Vec<PathBuf> = imports::unfinished(&files)
        .iter()
        .map(|(path, _)| (*path).clone())
        .collect();
    for tx_path in unfinished_paths {
        let index = files.iter().position(|(path, _)| *path == tx_path);
        let Some(index) = index else {
            continue;
        };
        let file = files[index].1.clone();
        // Only transactions for this destination participate; other
        // destinations (migration batches for other notes) are left for
        // their own command.
        if Path::new(&file.destination) != target_rel
            && file.destination
                != target_rel.to_string_lossy().replace('\\', "/")
        {
            continue;
        }
        let dest_path = bob_dir.join(&file.destination);
        let current_bytes = match fs::read(&dest_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(GkeepError::runtime(
                    "vault",
                    format!(
                        "unresolved gkeep import {}: destination {} is missing ({}); evidence retained at {}; Keep content untouched",
                        file.transaction_id,
                        file.destination,
                        error,
                        tx_path.display()
                    ),
                ));
            }
            Err(error) => {
                return Err(GkeepError::runtime(
                    "vault",
                    format!("read {}: {error}", dest_path.display()),
                ));
            }
        };
        let current_sha = imports::sha256_hex(&current_bytes);
        if current_sha == file.before_sha256 {
            // No import happened; safely retry with current inputs.
            if let Err(error) = fs::remove_file(&tx_path) {
                return Err(GkeepError::runtime(
                    "vault",
                    format!(
                        "remove stale gkeep import {}: {error}",
                        tx_path.display()
                    ),
                ));
            }
            files.remove(index);
            continue;
        }
        // Target installed but record still prepared: verify recorded
        // intended blocks at task boundaries without appending again.
        let current_contents =
            String::from_utf8_lossy(&current_bytes).into_owned();
        let mut block_by_digest = std::collections::BTreeMap::new();
        for entry in &file.entries {
            if entry.state != EntryState::Prepared {
                continue;
            }
            let Some(intended) = entry.intended.as_deref() else {
                continue;
            };
            block_by_digest
                .entry(entry.block_digest.clone())
                .or_insert_with(|| intended.to_string());
        }
        if block_by_digest.is_empty() {
            continue;
        }
        let current_counts =
            task_boundary_counts(&current_contents, settings, &block_by_digest);
        // Group prepared indices by digest for multiplicity.
        let mut need: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for entry in &file.entries {
            if entry.state == EntryState::Prepared {
                *need.entry(entry.block_digest.clone()).or_default() += 1;
            }
        }
        let mut all_ok = true;
        for (digest, want) in &need {
            let base = file.baseline_counts.get(digest).copied().unwrap_or(0);
            let found = current_counts.get(digest).copied().unwrap_or(0);
            if found != base + *want {
                all_ok = false;
                break;
            }
        }
        if all_ok {
            // Finalize without appending: only successful entries become
            // verified receipts; intended text is removed.
            let mut finalized = file.clone();
            for entry in &mut finalized.entries {
                if entry.state == EntryState::Prepared {
                    entry.state = EntryState::Verified;
                    entry.intended = None;
                    entry.dest_digest = Some(current_sha.clone());
                }
            }
            finalized.after_sha256 = Some(current_sha.clone());
            if let Err(error) = imports::persist_verified(&tx_path, &finalized)
            {
                return Err(GkeepError::runtime(
                    "vault",
                    format!("finalize the gkeep import store: {error}"),
                ));
            }
            files[index].1 = finalized;
            continue;
        }
        // Prepared target was edited or moved and the intended result
        // cannot be proven: stop, retain evidence and Keep content. Never
        // overwrite edits or guess that the URL proves the write.
        return Err(GkeepError::runtime(
            "vault",
            format!(
                "unresolved gkeep import {}: destination {} changed and the intended task blocks cannot be proven (expected baseline-plus-inserted multiplicity not found); evidence retained at {}; resolve manually and re-run `bob gkeep pull`",
                file.transaction_id,
                file.destination,
                tx_path.display()
            ),
        ));
    }
    Ok(files)
}

/// Commit outstanding receipt batches and their recorded destinations
/// with the existing scoped helper. Other dirty or staged files remain
/// untouched. Paths already present unchanged in HEAD are skipped so a
/// repeat pull cannot create an empty commit. Extra paths (the current
/// write's target and new receipt) are always included.
fn commit_outstanding_imports(
    bob_dir: &Path,
    child_env: &ob::ChildEnv,
    message: &str,
    store: &[(PathBuf, TransactionFile)],
    plan: &Plan,
    extra: &[PathBuf],
) -> Result<Option<String>, GkeepError> {
    let mut check = extra.to_vec();
    for planned in &plan.notes {
        let fp = planned.note.content.fingerprint();
        let Some((tx_path, file)) =
            imports::verified_file_for(store, &planned.note.id, &fp)
        else {
            continue;
        };
        let receipt_rel = imports::vault_rel(bob_dir, tx_path);
        let dest_rel = PathBuf::from(&file.destination);
        let receipt_committed =
            match imports::matches_head(bob_dir, child_env, &receipt_rel) {
                Ok(value) => value,
                Err(error) => {
                    return Err(GkeepError::runtime("commit", error));
                }
            };
        if receipt_committed {
            continue;
        }
        let dest_path = bob_dir.join(&dest_rel);
        if !dest_path.is_file() {
            return Err(GkeepError::runtime(
                "vault",
                format!(
                    "cannot finish GKeep import {}: destination {} is missing; evidence retained at {}; Keep content untouched",
                    file.transaction_id,
                    file.destination,
                    tx_path.display()
                ),
            ));
        }
        if !check.iter().any(|path| path == &receipt_rel) {
            check.push(receipt_rel);
        }
        if !check.iter().any(|path| path == &dest_rel) {
            check.push(dest_rel);
        }
    }
    check.sort();
    check.dedup();
    if check.is_empty() {
        return Ok(None);
    }
    if let Err(error) = imports::preflight_trackable(bob_dir, child_env, &check)
    {
        return Err(error.into_gkeep());
    }
    match ob::commit_paths(bob_dir, child_env, message, &check) {
        Ok(sha) => Ok(sha),
        Err(error) => Err(GkeepError::runtime(
            "commit",
            format!("commit the vault: {error}"),
        )),
    }
}

struct VerifyOutcome {
    ok: Vec<bool>,
}

/// Count task-boundary occurrences of each distinct block digest.
///
/// Only open top-level tasks count: the block's lines must exactly match
/// the file lines starting at a top-level open task. This proves complete
/// parsed blocks at task boundaries, not URL/substring/fingerprint
/// presence.
fn task_boundary_counts(
    file_contents: &str,
    settings: &note_tasks::NoteTaskSettings,
    block_by_digest: &std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, usize> {
    let normalized = file_contents.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    let scan = note_tasks::scan(file_contents, settings);
    // Index open top-level tasks by line.
    let open_tops: std::collections::BTreeSet<usize> = scan
        .tasks()
        .iter()
        .filter(|task| {
            task.indentation.is_empty() && task.status_type.is_open()
        })
        .map(|task| task.line_index)
        .collect();
    // Pre-split distinct blocks (owned strings so lifetimes hold).
    let normalized_blocks: Vec<(String, String)> = block_by_digest
        .iter()
        .map(|(digest, block)| (digest.clone(), block.replace("\r\n", "\n")))
        .collect();
    let split: Vec<(String, Vec<String>)> = normalized_blocks
        .iter()
        .map(|(digest, normalized)| {
            (
                digest.clone(),
                normalized.lines().map(str::to_string).collect::<Vec<_>>(),
            )
        })
        .collect();
    let mut counts: std::collections::BTreeMap<String, usize> = block_by_digest
        .keys()
        .map(|digest| (digest.clone(), 0))
        .collect();
    for line_index in open_tops {
        for (digest, block_lines) in &split {
            if line_index + block_lines.len() > lines.len() {
                continue;
            }
            if lines[line_index..line_index + block_lines.len()]
                .iter()
                .zip(block_lines.iter())
                .all(|(file, want)| *file == want.as_str())
            {
                *counts.get_mut(digest).expect("digest present") += 1;
            }
        }
    }
    counts
}

/// Baseline occurrence counts for the distinct write digests in the
/// before-image, needed to verify identical-rendering insertion.
fn baseline_counts(
    before_contents: &str,
    settings: &note_tasks::NoteTaskSettings,
    writes: &[WriteItem],
) -> std::collections::BTreeMap<String, usize> {
    let mut block_by_digest = std::collections::BTreeMap::new();
    for item in writes {
        let digest = imports::block_digest(&item.markdown);
        block_by_digest
            .entry(digest)
            .or_insert_with(|| item.markdown.clone());
    }
    task_boundary_counts(before_contents, settings, &block_by_digest)
}

fn verify_writes(
    file_contents: &str,
    writes: &[WriteItem],
    settings: &note_tasks::NoteTaskSettings,
    baseline: &std::collections::BTreeMap<String, usize>,
) -> VerifyOutcome {
    use std::collections::BTreeMap;
    // Group write indices by block digest: identical Markdown (distinct
    // Keep ids, especially URL-less) shares one multiplicity check. One
    // preexisting identical task must not prove two new imports.
    let mut by_digest: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut block_by_digest: BTreeMap<String, String> = BTreeMap::new();
    for (index, item) in writes.iter().enumerate() {
        let digest = imports::block_digest(&item.markdown);
        by_digest.entry(digest.clone()).or_default().push(index);
        block_by_digest
            .entry(digest)
            .or_insert_with(|| item.markdown.clone());
    }
    let current =
        task_boundary_counts(file_contents, settings, &block_by_digest);
    let mut ok = vec![false; writes.len()];
    for (digest, indices) in &by_digest {
        let base = baseline.get(digest).copied().unwrap_or(0);
        let inserted = indices.len();
        let found = current.get(digest).copied().unwrap_or(0);
        if found == base + inserted {
            for index in indices {
                ok[*index] = true;
            }
        }
    }
    VerifyOutcome { ok }
}

/// Debug-only fault injection for transaction/recovery tests.
///
/// When the named env var is set to `1`, the caller fails with a
/// deterministic error before doing the step, so a retry can prove no
/// duplicate append and no lost history.
fn fault_injected(name: &str) -> bool {
    #[cfg(debug_assertions)]
    {
        bob_env::var(name)
            .map(|value| value.trim() == "1")
            .unwrap_or(false)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = name;
        false
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_with_archive(
    args: &PullArgs,
    config: &GkeepConfig,
    client: &AdapterClient,
    creds: &Credentials,
    target_rel: &Path,
    plan: &Plan,
    verified: &[WriteItem],
    commit_sha: Option<String>,
    clips: &ClipSet,
    styler: &Styler,
) -> i32 {
    let is_json = args.format.is_json();
    // Archive set: every verified write plus every pending/revised note.
    // Pending notes were already verified by the ledger/journal lookup.
    // Successful clips join with their attachment counts for the guard.
    let mut archive_notes: Vec<(String, KeepContent, usize)> = Vec::new();
    let mut archive_for: Vec<String> = Vec::new();
    for item in verified {
        archive_notes.push((
            item.planned.note.id.clone(),
            item.planned.note.content.clone(),
            item.planned.note.attachments.len(),
        ));
        archive_for.push(item.planned.note.id.clone());
    }
    for planned in &plan.notes {
        if matches!(planned.action, PlanAction::ArchiveOnly) {
            let id = planned.note.id.clone();
            if !archive_for.contains(&id) {
                archive_notes.push((
                    id.clone(),
                    planned.note.content.clone(),
                    planned.note.attachments.len(),
                ));
                archive_for.push(id);
            }
        }
        // Revised notes without a verified write (verify failed) stay out.
        // Revised notes with a verified write are already in the set.
    }
    for (id, content, attachments) in &clips.archives {
        if !archive_for.contains(id) {
            archive_notes.push((id.clone(), content.clone(), *attachments));
            archive_for.push(id.clone());
        }
    }
    // Nothing to archive and nothing written: the "nothing to pull" path.
    // Archive unless --no-archive.
    let mut archive_status: std::collections::BTreeMap<
        String,
        (ArchiveStatus, Option<String>),
    > = std::collections::BTreeMap::new();
    let mut adapter_failed: Option<GkeepError> = None;
    if !args.no_archive && !archive_notes.is_empty() {
        let spinner_label = if is_json || args.quiet {
            None
        } else {
            Some("Archiving Google Keep notes")
        };
        match client.archive(creds, &archive_notes, spinner_label) {
            Ok(results) => {
                for (id, _, _) in &archive_notes {
                    let hit = results.iter().find(|row| row.id == *id);
                    match hit {
                        Some(row) => {
                            archive_status.insert(
                                id.clone(),
                                (row.status, row.detail.clone()),
                            );
                        }
                        None => {
                            archive_status.insert(
                                id.clone(),
                                (
                                    ArchiveStatus::Error,
                                    Some(
                                        "adapter omitted the note".to_string(),
                                    ),
                                ),
                            );
                        }
                    }
                }
            }
            Err(error) => adapter_failed = Some(error),
        }
    }
    if let Some(error) = adapter_failed {
        // Every note due for archive gets `archive: error` with the
        // adapter message as detail, so both modes report one failure
        // per note.
        let mut failed_status = archive_status;
        for (id, _, _) in &archive_notes {
            failed_status.insert(
                id.clone(),
                (ArchiveStatus::Error, Some(error.message().to_string())),
            );
        }
        let failed_markdown = if verified.is_empty() {
            None
        } else {
            Some(
                verified
                    .iter()
                    .map(|item| item.markdown.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        };
        // Writes are committed; journal the writes so the next pull can
        // archive them as pending with no duplicate write.
        append_journal(
            plan,
            verified,
            &failed_status,
            target_rel,
            commit_sha.clone(),
            true,
        );
        if is_json {
            // Exactly one JSON document on stdout, with `ok: false` and
            // a top-level `error`. Quiet still prints it only when the
            // run failed (it did): stdout gets the document, never empty.
            if args.quiet {
                eprintln!(
                    "bob gkeep pull: {}: {}",
                    error.kind(),
                    error.message()
                );
                if let Some(hint) = error.hint() {
                    eprintln!("  hint: {hint}");
                }
                println!(
                    "{}",
                    json_pull_report_with_error(
                        args,
                        config,
                        plan,
                        verified,
                        &failed_status,
                        failed_markdown.clone(),
                        commit_sha.clone(),
                        clips,
                        false,
                        Some(&error),
                    )
                );
                return 1;
            }
            println!(
                "{}",
                json_pull_report_with_error(
                    args,
                    config,
                    plan,
                    verified,
                    &failed_status,
                    failed_markdown.clone(),
                    commit_sha.clone(),
                    clips,
                    false,
                    Some(&error),
                )
            );
            return 1;
        }
        if !args.quiet {
            print_human_report(
                args,
                plan,
                verified,
                &failed_status,
                target_rel,
                commit_sha.clone(),
                clips,
                styler,
                false,
            );
        } else {
            // Quiet failures go to stderr only; stdout stays empty.
            for planned in &plan.notes {
                if failed_status
                    .get(&planned.note.id)
                    .is_some_and(|(status, _)| *status == ArchiveStatus::Error)
                {
                    eprintln!(
                        "bob gkeep pull: NOT archived {}: {}",
                        planned.note.id,
                        error.message()
                    );
                }
            }
        }
        ui::report_error("pull", &error, "human");
        return 1;
    }

    // Journal: written for verified writes, archived/refused for archives.
    append_journal(
        plan,
        verified,
        &archive_status,
        target_rel,
        commit_sha.clone(),
        false,
    );

    let failed = count_failed(plan, verified, &archive_status, clips);
    let ok = failed == 0;

    if is_json {
        if args.quiet && ok {
            return 0;
        }
        let inserted = if verified.is_empty() {
            None
        } else {
            Some(
                verified
                    .iter()
                    .map(|item| item.markdown.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        };
        println!(
            "{}",
            json_pull_report(
                args,
                config,
                plan,
                verified,
                &archive_status,
                inserted,
                commit_sha,
                clips,
                ok,
            )
        );
        return if ok { 0 } else { 1 };
    }

    if args.quiet {
        if ok {
            return 0;
        }
        // Quiet failures go to stderr only; stdout stays empty.
        for planned in &plan.notes {
            let id = planned.note.id.as_str();
            if matches!(planned.action, PlanAction::Skip) {
                continue;
            }
            if matches!(planned.action, PlanAction::CreateRef)
                && let Some(report) = clips.for_id(id)
                && let ClipOutcome::FailedRetryable { kind, message } =
                    &report.outcome
            {
                eprintln!(
                    "bob gkeep pull: clip failed for {id} ({kind}: {message}): left in Keep for the next pull"
                );
                continue;
            }
            let verified_ok = verified
                .iter()
                .any(|item| item.planned.note.id.as_str() == id);
            if !verified_ok
                && matches!(
                    planned.action,
                    PlanAction::Write | PlanAction::WriteRevision
                )
            {
                eprintln!(
                    "bob gkeep pull: NOT written: verification failed for {id}"
                );
                continue;
            }
            if let Some((status, detail)) = archive_status.get(id)
                && !status.is_success()
            {
                let extra = detail.clone().unwrap_or_default();
                if extra.is_empty() {
                    eprintln!(
                        "bob gkeep pull: NOT archived {id}: {}",
                        status.as_str()
                    );
                } else {
                    eprintln!("bob gkeep pull: NOT archived {id}: {extra}");
                }
            }
        }
        return 1;
    }
    // Human: nothing-to-do line when no actionable notes.
    let actionable = plan
        .notes
        .iter()
        .filter(|item| item.state.is_actionable())
        .count();
    if actionable == 0 && verified.is_empty() {
        let pinned = plan
            .notes
            .iter()
            .filter(|item| matches!(item.state, NoteState::Pinned))
            .count();
        let mut line = format!(
            "{} nothing to pull · Keep inbox is clear",
            styler.success_prefix(false)
        );
        if pinned > 0 {
            line.push_str(&format!(" · {pinned} pinned stays in Keep"));
        }
        println!("{line}");
        return 0;
    }
    print_human_report(
        args,
        plan,
        verified,
        &archive_status,
        target_rel,
        commit_sha,
        clips,
        styler,
        ok,
    );
    // Changed notes get the documented next-pull hint on stderr.
    let changed = archive_status
        .values()
        .filter(|entry| entry.0 == ArchiveStatus::Changed)
        .count();
    if changed > 0 {
        eprintln!(
            "warning {changed} note{} changed while pulling; {} in Keep and the next pull adds the revision",
            if changed == 1 { "" } else { "s" },
            if changed == 1 { "it stays" } else { "they stay" },
        );
    }
    if ok {
        0
    } else {
        1
    }
}

fn count_failed(
    plan: &Plan,
    verified: &[WriteItem],
    archive_status: &std::collections::BTreeMap<
        String,
        (ArchiveStatus, Option<String>),
    >,
    clips: &ClipSet,
) -> usize {
    // Writes planned but not verified.
    let mut failed = 0;
    let verified_ids: std::collections::BTreeSet<&str> = verified
        .iter()
        .map(|item| item.planned.note.id.as_str())
        .collect();
    for planned in &plan.notes {
        if matches!(
            planned.action,
            PlanAction::Write | PlanAction::WriteRevision
        ) && !verified_ids.contains(planned.note.id.as_str())
        {
            failed += 1;
        }
        if matches!(planned.action, PlanAction::CreateRef)
            && clips.for_id(&planned.note.id).is_some_and(|report| {
                matches!(report.outcome, ClipOutcome::FailedPermanent { .. })
            })
            && !verified_ids.contains(planned.note.id.as_str())
        {
            failed += 1;
        }
    }
    for (status, _) in archive_status.values() {
        if !status.is_success() {
            failed += 1;
        }
    }
    // Retryable clip failures stay in Keep and fail the run.
    failed += clips.failed_retryable();
    failed
}

fn append_journal(
    plan: &Plan,
    verified: &[WriteItem],
    archive_status: &std::collections::BTreeMap<
        String,
        (ArchiveStatus, Option<String>),
    >,
    target_rel: &Path,
    commit: Option<String>,
    adapter_crashed: bool,
) {
    let path = journal_path();
    let target = target_rel.to_string_lossy().into_owned();
    let ts = current_ts();
    let mut records = Vec::new();
    for item in verified {
        records.push(JournalRecord {
            ts: ts.clone(),
            event: JournalEvent::Written,
            id: item.planned.note.id.clone(),
            ref_: note_ref(&item.planned.note.id),
            fp: item.fp.clone(),
            path: target.clone(),
            commit: commit.clone(),
            status: None,
            url: None,
            parent: None,
        });
    }
    if !adapter_crashed {
        for (id, (status, _)) in archive_status {
            let fp = verified
                .iter()
                .find(|item| item.planned.note.id == *id)
                .map(|item| item.fp.clone())
                .or_else(|| {
                    plan.notes
                        .iter()
                        .find(|item| item.note.id == *id)
                        .map(|item| item.note.content.fingerprint())
                })
                .unwrap_or_default();
            let ref_ = note_ref(id);
            if status.is_success() {
                records.push(JournalRecord {
                    ts: ts.clone(),
                    event: JournalEvent::Archived,
                    id: id.clone(),
                    ref_,
                    fp,
                    path: target.clone(),
                    commit: commit.clone(),
                    status: Some(status.as_str().to_string()),
                    url: None,
                    parent: None,
                });
            } else {
                records.push(JournalRecord {
                    ts: ts.clone(),
                    event: JournalEvent::ArchiveRefused,
                    id: id.clone(),
                    ref_,
                    fp,
                    path: target.clone(),
                    commit: commit.clone(),
                    status: Some(status.as_str().to_string()),
                    url: None,
                    parent: None,
                });
            }
        }
        // Pending-only archives have no verified write to supply the fp;
        // look it up from the archive request would need the note. The
        // pending fp is filled by the caller path below when available.
    }
    if records.is_empty() {
        return;
    }
    if let Err(error) = Journal::append(&path, &records) {
        ui::warn(&format!("append the gkeep journal: {error}"));
    }
}

fn print_dry_run(
    args: &PullArgs,
    config: &GkeepConfig,
    plan: &Plan,
    writes: &[WriteItem],
    clips: &ClipSet,
    styler: &Styler,
) -> i32 {
    let is_json = args.format.is_json();
    if is_json {
        if args.quiet {
            return 0;
        }
        let inserted = if writes.is_empty() {
            None
        } else {
            Some(
                writes
                    .iter()
                    .map(|item| item.markdown.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        };
        let empty: std::collections::BTreeMap<
            String,
            (ArchiveStatus, Option<String>),
        > = std::collections::BTreeMap::new();
        println!(
            "{}",
            json_pull_report(
                args, config, plan, writes, &empty, inserted, None, clips,
                true,
            )
        );
        return 0;
    }
    if args.quiet {
        return 0;
    }
    let target = config.target();
    let actionable = plan
        .notes
        .iter()
        .filter(|item| item.state.is_actionable())
        .count();
    if actionable == 0 {
        let pinned = plan
            .notes
            .iter()
            .filter(|item| matches!(item.state, NoteState::Pinned))
            .count();
        let mut line = format!(
            "{} nothing to pull · Keep inbox is clear",
            styler.success_prefix(true)
        );
        if pinned > 0 {
            line.push_str(&format!(" · {pinned} pinned stays in Keep"));
        }
        println!("{line}");
        return 0;
    }
    println!("[dry-run] Google Keep → {target} · {actionable} to pull");
    for planned in &plan.notes {
        // URL-only rows lead with the display URL, not the note title.
        let title = if matches!(planned.action, PlanAction::CreateRef) {
            truncate_title(&report_display(planned))
        } else {
            truncate_title(&display_title(&planned.note))
        };
        let detail = match planned.action {
            PlanAction::Skip => format!(
                "skipped · {}",
                planned.skip_reason.as_deref().unwrap_or("skipped")
            ),
            PlanAction::Write | PlanAction::WriteRevision => {
                if args.no_archive {
                    "would write · left in Keep (--no-archive)".to_string()
                } else {
                    "would write · would archive".to_string()
                }
            }
            PlanAction::ArchiveOnly => {
                if args.no_archive {
                    "already in vault · left in Keep (--no-archive)".to_string()
                } else {
                    "would archive · already in vault".to_string()
                }
            }
            PlanAction::CreateRef => dry_ref_detail(planned, clips, args),
        };
        let glyph = styler.dim("·");
        println!("  {glyph} {title}  {detail}");
    }
    if !writes.is_empty() {
        let joined = writes
            .iter()
            .map(|item| item.markdown.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        println!("Markdown to insert under ## Tasks in {target}:");
        for line in joined.lines() {
            println!("{}", styler.dim(&format!("│ {line}")));
        }
    }
    let would_clip = clips.reports.len();
    if would_clip > 0 {
        println!(
            "{would_clip} link{} would be clipped into the reading queue",
            if would_clip == 1 { "" } else { "s" },
        );
    }
    let written = 0;
    let archived = if args.no_archive {
        0
    } else {
        plan.notes
            .iter()
            .filter(|item| item.state.is_actionable())
            .count()
    };
    let skipped = plan.notes.len() - actionable;
    println!(
        "{} {written} written · {archived} archived · {skipped} skipped",
        styler.success_prefix(true)
    );
    0
}

fn truncate_title(title: &str) -> String {
    let available = style::terminal_width().saturating_sub(40).max(20);
    style::truncate(title, available)
}

/// The dry-run detail for a URL-only note: the offline verdict for
/// library hits, else the would-clip line, always naming the
/// selected/planned parent.
fn dry_ref_detail(
    planned: &PlannedNote,
    clips: &ClipSet,
    args: &PullArgs,
) -> String {
    let archive_suffix = if args.no_archive {
        " · left in Keep (--no-archive)"
    } else {
        " · would archive"
    };
    let Some(report) = clips.for_id(&planned.note.id) else {
        return format!("would clip → reading queue{archive_suffix}");
    };
    let parent = report.parent.as_str();
    let ClipOutcome::WouldClip { verdict } = &report.outcome else {
        return format!("would clip → {parent}");
    };
    match verdict.verdict {
        crate::native::url_routing::Verdict::InLibrary => {
            let title = verdict.title.as_deref().unwrap_or("untitled");
            match verdict.reading_state.as_deref() {
                Some(state) => format!(
                    "already in library: {title} ({state}) → {parent}{archive_suffix}"
                ),
                None => {
                    format!(
                        "already in library: {title} → {parent}{archive_suffix}"
                    )
                }
            }
        }
        crate::native::url_routing::Verdict::InIntake => {
            let path = verdict.path.as_deref().unwrap_or("intake");
            format!("already queued · {path} → {parent}{archive_suffix}")
        }
        crate::native::url_routing::Verdict::Legacy => {
            let path = verdict.path.as_deref().unwrap_or("library");
            format!(
                "in your library as a legacy note ({path}) · a fresh copy would be clipped → {parent}{archive_suffix}"
            )
        }
        crate::native::url_routing::Verdict::Unknown => {
            let message = verdict.message.as_deref().unwrap_or("unreadable");
            format!(
                "would clip → {parent} · library check unavailable: {message}{archive_suffix}"
            )
        }
        crate::native::url_routing::Verdict::NotFound => {
            format!("would clip → {parent}{archive_suffix}")
        }
    }
}

/// Display URL for a planned ref note without a clip report.
fn report_display(planned: &PlannedNote) -> String {
    planned
        .ref_intent
        .as_ref()
        .map(|intent| intent.display.clone())
        .unwrap_or_else(|| display_title(&planned.note))
}

#[allow(clippy::too_many_arguments)]
fn print_human_report(
    args: &PullArgs,
    plan: &Plan,
    verified: &[WriteItem],
    archive_status: &std::collections::BTreeMap<
        String,
        (ArchiveStatus, Option<String>),
    >,
    target_rel: &Path,
    commit_sha: Option<String>,
    clips: &ClipSet,
    styler: &Styler,
    ok: bool,
) {
    let target = target_rel.to_string_lossy();
    let actionable = plan
        .notes
        .iter()
        .filter(|item| item.state.is_actionable())
        .count();
    println!("Google Keep → {target} · {actionable} to pull");
    println!();
    let verified_ids: std::collections::BTreeSet<&str> = verified
        .iter()
        .map(|item| item.planned.note.id.as_str())
        .collect();
    for planned in &plan.notes {
        let title = truncate_title(&display_title(&planned.note));
        let id = planned.note.id.as_str();
        if matches!(planned.action, PlanAction::Skip) {
            let reason = planned.skip_reason.as_deref().unwrap_or("skipped");
            println!("  {} {title}  skipped · {reason}", styler.dim("·"));
            continue;
        }
        if matches!(planned.action, PlanAction::ArchiveOnly) {
            let entry = archive_status.get(id).cloned();
            let detail = match entry {
                None if args.no_archive => {
                    "left in Keep (--no-archive)".to_string()
                }
                None => "would archive".to_string(),
                Some((status, _)) if status.is_success() => {
                    "archived · already in vault".to_string()
                }
                Some((ArchiveStatus::Changed, _)) => {
                    "NOT archived: edited in Keep during pull".to_string()
                }
                Some((ArchiveStatus::Missing, _)) => {
                    "NOT archived: note is gone".to_string()
                }
                Some((ArchiveStatus::Error, detail)) => {
                    let text = detail.unwrap_or_default();
                    if text.is_empty() {
                        "NOT archived: adapter error".to_string()
                    } else {
                        format!("NOT archived: {text}")
                    }
                }
                Some((_, _)) => "would archive".to_string(),
            };
            let glyph = if detail.starts_with("NOT") {
                styler.red("✗")
            } else if detail.starts_with("archived") {
                styler.green("✓")
            } else {
                styler.dim("·")
            };
            println!("  {glyph} {title}  {detail}");
            continue;
        }
        if matches!(planned.action, PlanAction::CreateRef) {
            print_human_ref_row(
                args,
                planned,
                clips,
                archive_status,
                &verified_ids,
                styler,
            );
            continue;
        }
        // Write or revision.
        if !verified_ids.contains(id) {
            println!(
                "  {} {title}  NOT written: verification failed",
                styler.red("✗")
            );
            continue;
        }
        let entry = archive_status.get(id).cloned();
        let detail = if args.no_archive {
            "written · left in Keep (--no-archive)".to_string()
        } else {
            match entry {
                Some((ArchiveStatus::Archived, _)) => {
                    "written · archived".to_string()
                }
                Some((ArchiveStatus::AlreadyArchived, _)) => {
                    "written · already archived".to_string()
                }
                Some((ArchiveStatus::Changed, _)) => {
                    "written · NOT archived: edited in Keep during pull"
                        .to_string()
                }
                Some((ArchiveStatus::Missing, _)) => {
                    "written · NOT archived: note is gone".to_string()
                }
                Some((ArchiveStatus::Error, extra)) => {
                    let text = extra.unwrap_or_default();
                    if text.is_empty() {
                        "written · NOT archived: adapter error".to_string()
                    } else {
                        format!("written · NOT archived: {text}")
                    }
                }
                None => "written · archiving skipped".to_string(),
            }
        };
        // `NOT archived: edited` uses the warning color; other `NOT`
        // failures use red; successes use green.
        let glyph = if detail.contains("NOT archived: edited") {
            styler.yellow("!")
        } else if detail.contains("NOT") {
            styler.red("✗")
        } else {
            styler.green("✓")
        };
        println!("  {glyph} {title}  {detail}");
    }
    println!();
    let written = verified.len();
    let clipped = clips.clipped();
    let archived = archive_status
        .values()
        .filter(|(status, _)| status.is_success())
        .count();
    let skipped = plan
        .notes
        .iter()
        .filter(|item| matches!(item.action, PlanAction::Skip))
        .count();
    let prefix = if ok {
        styler.success_prefix(false)
    } else {
        styler.warning_prefix()
    };
    let mut summary = format!("{prefix} {written} written");
    if clipped > 0 {
        summary.push_str(&format!(" · {clipped} clipped"));
    }
    summary.push_str(&format!(" · {archived} archived · {skipped} skipped"));
    if let Some(sha) = commit_sha {
        let short: String = sha.chars().take(7).collect();
        summary.push_str(&format!(" · vault commit {short}"));
    }
    println!("{summary}");
}

/// One human row for a URL-only note: the display URL, the clip
/// outcome, and the archive suffix.
fn print_human_ref_row(
    args: &PullArgs,
    planned: &PlannedNote,
    clips: &ClipSet,
    archive_status: &std::collections::BTreeMap<
        String,
        (ArchiveStatus, Option<String>),
    >,
    verified_ids: &std::collections::BTreeSet<&str>,
    styler: &Styler,
) {
    let display = truncate_title(&report_display(planned));
    let archive_suffix = |archived_word: &str| -> String {
        if args.no_archive {
            " · left in Keep (--no-archive)".to_string()
        } else {
            match archive_status.get(&planned.note.id) {
                Some((status, _)) if status.is_success() => {
                    format!(" · {archived_word}")
                }
                Some((ArchiveStatus::Changed, _)) => {
                    " · NOT archived: edited in Keep during pull".to_string()
                }
                Some((ArchiveStatus::Missing, _)) => {
                    " · NOT archived: note is gone".to_string()
                }
                Some((ArchiveStatus::Error, detail)) => {
                    let text = detail.clone().unwrap_or_default();
                    if text.is_empty() {
                        " · NOT archived: adapter error".to_string()
                    } else {
                        format!(" · NOT archived: {text}")
                    }
                }
                None | Some(_) => " · archiving skipped".to_string(),
            }
        }
    };
    let Some(report) = clips.for_id(&planned.note.id) else {
        println!("  {} {display}  clip skipped", styler.dim("·"),);
        return;
    };
    let parent = report.parent.as_str();
    match &report.outcome {
        ClipOutcome::Created { pdf } => {
            let suffix = archive_suffix("archived");
            println!(
                "  {} {display}  clipped → {pdf} → {parent}{suffix}",
                styler.green("✓"),
            );
        }
        ClipOutcome::AlreadyInLibrary { note } => {
            let suffix = archive_suffix("archived");
            println!(
                "  {} {display}  already in library · {note} → {parent}{suffix}",
                styler.green("✓"),
            );
        }
        ClipOutcome::AlreadyQueued { pdf } => {
            let suffix = archive_suffix("archived");
            println!(
                "  {} {display}  already queued · {pdf} → {parent}{suffix}",
                styler.green("✓"),
            );
        }
        ClipOutcome::FailedRetryable { kind, .. } => {
            println!(
                "  {} {display}  clip failed ({kind}) · left in Keep for the next pull",
                styler.yellow("!"),
            );
        }
        ClipOutcome::FailedPermanent { kind, .. } => {
            if !verified_ids.contains(planned.note.id.as_str()) {
                println!(
                    "  {} {display}  clip failed ({kind}) · NOT written: verification failed",
                    styler.red("✗"),
                );
                return;
            }
            let suffix = archive_suffix("archived");
            println!(
                "  {} {display}  clip failed ({kind}) · written as a task with a ⚠️ note{suffix}",
                styler.green("✓"),
            );
        }
        ClipOutcome::WouldClip { .. } => {
            println!("  {} {display}  would clip → {parent}", styler.dim("·"),);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn json_pull_report(
    args: &PullArgs,
    config: &GkeepConfig,
    plan: &Plan,
    verified: &[WriteItem],
    archive_status: &std::collections::BTreeMap<
        String,
        (ArchiveStatus, Option<String>),
    >,
    markdown: Option<String>,
    commit: Option<String>,
    clips: &ClipSet,
    ok: bool,
) -> serde_json::Value {
    json_pull_report_with_error(
        args,
        config,
        plan,
        verified,
        archive_status,
        markdown,
        commit,
        clips,
        ok,
        None,
    )
}

/// The per-note `clip` object for a URL-only note, with the
/// selected/planned parent additively (schema stays 1).
fn clip_json(report: &ClipReport) -> serde_json::Value {
    let mut value = json!({
        "url": report.intent.cleaned,
        "display": report.intent.display,
        "outcome": report.outcome.as_str(),
        "parent": report.parent,
    });
    match &report.outcome {
        ClipOutcome::Created { pdf } | ClipOutcome::AlreadyQueued { pdf } => {
            value["pdf"] = json!(pdf);
        }
        ClipOutcome::AlreadyInLibrary { note } => {
            value["existing"] = json!(note);
        }
        ClipOutcome::FailedRetryable { kind, message }
        | ClipOutcome::FailedPermanent { kind, message, .. } => {
            value["error"] = json!({
                "kind": kind,
                "message": message,
                "retryable": matches!(
                    report.outcome,
                    ClipOutcome::FailedRetryable { .. }
                ),
            });
        }
        ClipOutcome::WouldClip { .. } => {}
    }
    value
}

#[allow(clippy::too_many_arguments)]
fn json_pull_report_with_error(
    args: &PullArgs,
    config: &GkeepConfig,
    plan: &Plan,
    verified: &[WriteItem],
    archive_status: &std::collections::BTreeMap<
        String,
        (ArchiveStatus, Option<String>),
    >,
    markdown: Option<String>,
    commit: Option<String>,
    clips: &ClipSet,
    ok: bool,
    adapter_error: Option<&GkeepError>,
) -> serde_json::Value {
    let verified_ids: std::collections::BTreeSet<&str> = verified
        .iter()
        .map(|item| item.planned.note.id.as_str())
        .collect();
    // Dry runs write nothing: every note reports `written: false`.
    let dry = args.dry_run;
    let notes: Vec<serde_json::Value> = plan
        .notes
        .iter()
        .map(|planned| {
            let action = planned.action.as_str();
            let written =
                !dry && verified_ids.contains(planned.note.id.as_str());
            let (archive, detail) = match archive_status.get(&planned.note.id) {
                Some((status, detail)) => {
                    (status.as_str().to_string(), detail.clone())
                }
                None => (
                    if matches!(planned.action, PlanAction::Skip)
                        || args.no_archive
                    {
                        "not_requested".to_string()
                    } else {
                        "not_attempted".to_string()
                    },
                    None,
                ),
            };
            // `ref` stays the short selection id; the clip outcome for
            // URL-only notes rides alongside as `clip`.
            let mut note = json!({
                "id": planned.note.id,
                "ref": note_ref(&planned.note.id),
                "title": display_title(&planned.note),
                "state": planned.state.as_str(),
                "action": action,
                "skip_reason": planned.skip_reason,
                "written": written,
                "archive": archive,
                "detail": detail,
            });
            if matches!(planned.action, PlanAction::CreateRef)
                && let Some(report) = clips.for_id(&planned.note.id)
            {
                note["clip"] = clip_json(report);
            }
            note
        })
        .collect();
    let written = if dry { 0 } else { verified.len() };
    let archived = archive_status
        .values()
        .filter(|(status, _)| status.is_success())
        .count();
    let skipped = plan
        .notes
        .iter()
        .filter(|item| matches!(item.action, PlanAction::Skip))
        .count();
    let failed = count_failed(plan, verified, archive_status, clips);
    let mut document = json!({
        "schema_version": 1,
        "ok": ok,
        "dry_run": args.dry_run,
        "archive_enabled": !args.no_archive,
        "commit_enabled": !args.no_commit,
        "target": config.target(),
        "commit": commit,
        "notes": notes,
        "markdown": markdown,
        "summary": {
            "written": written,
            "archived": archived,
            "skipped": skipped,
            "failed": failed,
            "refs": {
                "clipped": clips.count("created"),
                "already_in_library": clips.count("already_in_library"),
                "already_queued": clips.count("already_queued"),
                "failed_retryable": clips.count("failed_retryable"),
                "failed_permanent": clips.count("failed_permanent"),
            },
        }
    });
    if let Some(error) = adapter_error {
        document["error"] = json!({
            "kind": error.kind(),
            "message": error.message(),
            "hint": error.hint(),
        });
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn prompt_accepts_default_reprompts_unknown_and_stops_on_eof() {
        let dir = tempfile::tempdir().expect("temp dir");
        let bob_dir = dir.path();
        std::fs::write(bob_dir.join("sase.md"), "---\ntype: [[area]]\n---\n")
            .expect("sase parent");
        // Enter accepts the sticky default.
        let mut enter = Cursor::new("\n");
        match prompt_keep_parent_from(
            bob_dir,
            &mut enter,
            "example.com/a",
            "gkeep_inbox",
        ) {
            PromptAnswer::Parent(parent) => assert_eq!(parent, "gkeep_inbox"),
            PromptAnswer::Eof => panic!("Enter should accept the default"),
        }
        // Unknown names re-prompt with hints, then accept the next answer.
        let mut retry = Cursor::new("nope\nsase\n");
        match prompt_keep_parent_from(
            bob_dir,
            &mut retry,
            "example.com/b",
            "gkeep_inbox",
        ) {
            PromptAnswer::Parent(parent) => assert_eq!(parent, "sase"),
            PromptAnswer::Eof => panic!("valid answer should resolve"),
        }
        // EOF uses the default without looping.
        let mut eof = Cursor::new("");
        match prompt_keep_parent_from(
            bob_dir,
            &mut eof,
            "example.com/c",
            "sase",
        ) {
            PromptAnswer::Parent(_) => panic!("EOF should not resolve"),
            PromptAnswer::Eof => {}
        }
    }

    #[test]
    fn git_ancestor_checks_dot_git_files_and_dirs() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        assert!(!has_git_ancestor(root));
        let sub = root.join("a").join("b");
        std::fs::create_dir_all(&sub).expect("mkdir");
        assert!(!has_git_ancestor(&sub));
        std::fs::create_dir_all(root.join(".git")).expect("mkdir .git");
        assert!(has_git_ancestor(root));
        assert!(has_git_ancestor(&sub));
        std::fs::remove_dir_all(root.join(".git")).expect("rm .git");
        std::fs::write(root.join(".git"), "gitdir: elsewhere\n")
            .expect("write .git file");
        assert!(has_git_ancestor(root));
        assert!(has_git_ancestor(&sub));
    }
}
