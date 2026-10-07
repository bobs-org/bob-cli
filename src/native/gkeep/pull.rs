//! `bob gkeep pull`: guarded drain transaction.
//!
//! A Keep note is archived only when its current content fingerprint
//! matches a task block in the vault that was atomically written,
//! fsynced, re-read and parse-verified, and committed when the vault is
//! a Git worktree. A duplicate is always preferred over data loss.

use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use fs2::FileExt;
use serde_json::json;

use super::{
    adapter::{AdapterClient, Credentials},
    config::GkeepConfig,
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

/// One URL-only note's clip outcome from the pre-pass.
#[derive(Debug, Clone)]
struct ClipReport {
    id: String,
    intent: UrlIntent,
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
            eprintln!("bob gkeep pull: warning: URL routing is off: {error:?}");
            None
        }
    }
}

/// Undocumented test hook (debug builds only): a shell snippet run just
/// before each compare-and-swap re-read so tests can simulate Obsidian
/// racing the rename.
#[cfg(debug_assertions)]
fn maybe_run_before_rename_hook() {
    if let Ok(script) = std::env::var("BOB_GKEEP_TEST_BEFORE_RENAME")
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

    // Scan the ledger and journal before the vault lock: the clip
    // pre-pass below runs under the pull lock but outside the vault
    // lock, so long clips never block vault maintenance.
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

    let routing = load_gkeep_routing(args.no_ref);
    let plan_opts = PlanOptions {
        include_pinned: args.include_pinned,
        include_shared: args.include_shared,
        ids: args.id.clone(),
        limit: args.limit.map(|value| value as usize),
        routing,
    };
    let plan = super::plan::classify(&notes, &ledger, &journal, &plan_opts);

    // Clip URL-only notes before the target check and the vault lock,
    // under the pull lock. A dry run only computes offline verdicts.
    let clips =
        run_clip_pre_pass(&plan, &bob_dir, args.dry_run, args.quiet, is_json);

    // Dry runs stop at the plan: no locks, no clips, no writes, no
    // archive calls. The indent comes from the target when it reads,
    // else the default.
    if args.dry_run {
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
    // the target note. A pull whose notes all clip archives without
    // it; every other run keeps today's existence check.
    let needs_target = plan.notes.iter().any(|planned| {
        matches!(
            planned.action,
            PlanAction::Write | PlanAction::WriteRevision
        )
    }) || clips.reports.iter().any(|report| {
        matches!(report.outcome, ClipOutcome::FailedPermanent { .. })
    });
    let all_clipped = !needs_target && !clips.archives.is_empty();
    if !all_clipped && !target_path.is_file() {
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
    // to the guarded archive.
    if !needs_target {
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

    // Take `bob_sync.lock` before reading the target. (Obsidian ignores
    // the lock, so CAS still guards the write.)
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
    let indent =
        capture::dominant_indent_unit(&capture::line_spans(&target_contents))
            .unwrap_or("\t")
            .to_string();
    let mut writes: Vec<WriteItem> = build_writes(&plan, &clips, &indent);

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
        // The vault changed under us: delete the temp, re-plan once.
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
    // Verify: each block exactly once, open top-level #task, marker.
    let settings = note_tasks::read_settings(&bob_dir);
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
    let verify_outcome = verify_writes(&verified_contents, &writes, &settings);
    let verified: Vec<WriteItem> = writes
        .iter()
        .enumerate()
        .filter(|(index, _)| verify_outcome.ok[*index])
        .map(|(_, item)| item.clone())
        .collect();
    let verify_failed = writes.len() - verified.len();

    // Commit when the vault is a Git worktree, unless --no-commit.
    // When `git` cannot be started, only vaults that really are repos
    // fail: without a `.git` ancestor the vault simply is not a worktree.
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
            let count = verified.len();
            let message =
                format!("bob gkeep pull: {count} notes from Google Keep");
            match ob::commit_paths(
                &bob_dir,
                &child_env,
                &message,
                std::slice::from_ref(&target_rel),
            ) {
                Ok(sha) => commit_sha = sha,
                Err(error) => {
                    return ui::report_error(
                        "pull",
                        &GkeepError::runtime(
                            "commit",
                            format!("commit the vault: {error}"),
                        ),
                        format_name,
                    );
                }
            }
        }
    }
    drop(_vault_guard);

    if verify_failed > 0 {
        // Verified notes still archive; failures are reported with exit 1.
        let code = finish_with_archive(
            args,
            &config,
            &client,
            &creds,
            &target_rel,
            &plan,
            &verified,
            commit_sha.clone(),
            &clips,
            &styler,
        );
        if !args.quiet {
            for item in &writes {
                let idx = writes
                    .iter()
                    .position(|other| {
                        other.planned.note.id == item.planned.note.id
                    })
                    .unwrap_or(0);
                if !verify_outcome.ok[idx] {
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
        &plan,
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
/// fallbacks (rendered exactly as today, with the ⚠️ child just
/// before the `Source:` line).
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

/// Clip every `CreateRef` note sequentially, before the vault lock. A
/// dry run only computes offline verdicts and clips nothing. Terminal
/// outcomes (`Created`, `AlreadyInLibrary`, `AlreadyQueued`) append
/// one `ref_created` journal batch per clip immediately, so a crash
/// between clip and archive re-pulls as `ArchiveOnly`.
fn run_clip_pre_pass(
    plan: &Plan,
    bob_dir: &Path,
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
            progress,
        };
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
                    let fallback = error.fallback_note(&intent.cleaned);
                    ClipOutcome::FailedPermanent {
                        kind: error.kind.as_str().to_string(),
                        message: first_line(&error.message),
                        fallback,
                    }
                }
            }
        };
        // Terminal outcomes join the archive set and journal now, one
        // batch per clip.
        match &outcome {
            ClipOutcome::Created { pdf } => {
                append_ref_created(
                    &planned.note,
                    pdf,
                    Some(intent.cleaned.clone()),
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
            outcome,
        });
    }
    clips
}

/// Append one `ref_created` journal batch: `path` is the intake PDF or
/// the existing ref note, `url` the clipped URL.
fn append_ref_created(note: &KeepNote, path: &str, url: Option<String>) {
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

struct VerifyOutcome {
    ok: Vec<bool>,
}

fn verify_writes(
    file_contents: &str,
    writes: &[WriteItem],
    settings: &note_tasks::NoteTaskSettings,
) -> VerifyOutcome {
    let normalized = file_contents.replace("\r\n", "\n");
    let normalized_lines: Vec<&str> = normalized.lines().collect();
    let scan = note_tasks::scan(file_contents, settings);
    // Collect every marker in the file for the marker check.
    let mut file_markers = Vec::new();
    for line in normalized_lines.iter() {
        for pair in super::ledger::parse_markers(line) {
            file_markers.push(pair);
        }
    }
    let mut ok = Vec::with_capacity(writes.len());
    for item in writes {
        let block_norm = item.markdown.replace("\r\n", "\n");
        let count = normalized.matches(block_norm.as_str()).count();
        if count != 1 {
            ok.push(false);
            continue;
        }
        // First line of the block must be an open top-level #task.
        let first = block_norm.lines().next().unwrap_or_default();
        let line_index =
            normalized_lines.iter().position(|line| *line == first);
        let Some(line_index) = line_index else {
            ok.push(false);
            continue;
        };
        let task_ok = scan
            .tasks()
            .iter()
            .find(|task| task.line_index == line_index)
            .is_some_and(|task| {
                task.indentation.is_empty() && task.status_type.is_open()
            });
        if !task_ok {
            ok.push(false);
            continue;
        }
        if !file_markers
            .iter()
            .any(|(id, fp)| *id == item.planned.note.id && *fp == item.fp)
        {
            ok.push(false);
            continue;
        }
        ok.push(true);
    }
    VerifyOutcome { ok }
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
/// library hits, else the would-clip line.
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
    let ClipOutcome::WouldClip { verdict } = &report.outcome else {
        return "would clip → reading queue".to_string();
    };
    match verdict.verdict {
        crate::native::url_routing::Verdict::InLibrary => {
            let title = verdict.title.as_deref().unwrap_or("untitled");
            match verdict.reading_state.as_deref() {
                Some(state) => format!(
                    "already in library: {title} ({state}){archive_suffix}"
                ),
                None => {
                    format!("already in library: {title}{archive_suffix}")
                }
            }
        }
        crate::native::url_routing::Verdict::InIntake => {
            let path = verdict.path.as_deref().unwrap_or("intake");
            format!("already queued · {path}{archive_suffix}")
        }
        crate::native::url_routing::Verdict::Legacy => {
            let path = verdict.path.as_deref().unwrap_or("library");
            format!(
                "in your library as a legacy note ({path}) · a fresh copy would be clipped{archive_suffix}"
            )
        }
        crate::native::url_routing::Verdict::Unknown => {
            let message = verdict.message.as_deref().unwrap_or("unreadable");
            format!(
                "would clip → reading queue · library check unavailable: {message}{archive_suffix}"
            )
        }
        crate::native::url_routing::Verdict::NotFound => {
            format!("would clip → reading queue{archive_suffix}")
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
            print_human_ref_row(args, planned, clips, archive_status, styler);
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
    match &report.outcome {
        ClipOutcome::Created { pdf } => {
            let suffix = archive_suffix("archived");
            println!(
                "  {} {display}  clipped → {pdf}{suffix}",
                styler.green("✓"),
            );
        }
        ClipOutcome::AlreadyInLibrary { note } => {
            let suffix = archive_suffix("archived");
            println!(
                "  {} {display}  already in library · {note}{suffix}",
                styler.green("✓"),
            );
        }
        ClipOutcome::AlreadyQueued { pdf } => {
            let suffix = archive_suffix("archived");
            println!(
                "  {} {display}  already queued · {pdf}{suffix}",
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
            let suffix = archive_suffix("archived");
            println!(
                "  {} {display}  clip failed ({kind}) · written as a task with a ⚠️ note{suffix}",
                styler.green("✓"),
            );
        }
        ClipOutcome::WouldClip { .. } => {
            println!(
                "  {} {display}  would clip → reading queue",
                styler.dim("·"),
            );
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

/// The per-note `clip` object for a URL-only note.
fn clip_json(report: &ClipReport) -> serde_json::Value {
    let mut value = json!({
        "url": report.intent.cleaned,
        "display": report.intent.display,
        "outcome": report.outcome.as_str(),
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
