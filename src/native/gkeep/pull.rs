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
    cli::HumanFormat,
    config::GkeepConfig,
    ledger::{Journal, JournalEvent, JournalRecord, Ledger},
    model::{note_ref, KeepContent},
    plan::{NoteState, Plan, PlanAction, PlanOptions, PlannedNote},
    render::{display_title, render_note},
    ui,
};
use super::{GkeepError, PullArgs};
use crate::native::{
    capture, env as bob_env, note_tasks, ob,
    style::{self, Styler},
};

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
    bob_env::current_datetime()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

pub(crate) fn run(args: &PullArgs) -> i32 {
    let is_json = args.format.is_json();
    let styler = Styler::detect();
    let format_name = args.error_format();

    // Guard: per-host pull lock, skipped for dry runs.
    let _pull_guard = if args.dry_run {
        None
    } else {
        match acquire_pull_lock() {
            Ok(guard) => Some(guard),
            Err(code) => {
                return ui::report_error(
                    "pull",
                    &GkeepError::runtime(
                        "lock",
                        "another bob gkeep pull is already running".to_string(),
                    ),
                    format_name,
                )
                .max(code);
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
    if !target_path.is_file() {
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

    let plan_opts = PlanOptions {
        include_pinned: args.include_pinned,
        include_shared: args.include_shared,
        ids: args.id.clone(),
        limit: args.limit.map(|value| value as usize),
    };
    let plan = super::plan::classify(&notes, &ledger, &journal, &plan_opts);

    // Render new and revised notes with the target indent unit.
    let indent =
        capture::dominant_indent_unit(&capture::line_spans(&target_contents))
            .unwrap_or("\t")
            .to_string();
    let mut writes: Vec<WriteItem> = Vec::new();
    for planned in &plan.notes {
        let revision = matches!(planned.state, NoteState::Revised);
        if !matches!(
            planned.action,
            PlanAction::Write | PlanAction::WriteRevision
        ) {
            continue;
        }
        let block = render_note(&planned.note, &indent, revision);
        writes.push(WriteItem {
            planned: planned.clone(),
            markdown: block.markdown,
            fp: planned.note.content.fingerprint(),
        });
    }

    let nothing_to_write = writes.is_empty();

    // Dry runs stop at the plan: no locks, no writes, no archive calls.
    if args.dry_run {
        return print_dry_run(args, &config, &plan, &writes, &styler);
    }

    // Nothing to write (only pending/skipped): skip lock, write, verify,
    // and commit; go straight to the guarded archive.
    if nothing_to_write {
        return finish_with_archive(
            args,
            &config,
            &client,
            &creds,
            &bob_dir,
            &target_rel,
            &plan,
            &[],
            None,
            &styler,
        );
    }

    // Lock the vault (Obsidian ignores it, so CAS still guards Verify).
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

    // Build the insertion once; CAS re-reads before the rename.
    let joined = writes
        .iter()
        .map(|item| item.markdown.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let (first_contents, _) =
        capture::insert_task_line(&target_contents, &joined);

    maybe_run_before_rename_hook();
    let current_bytes = match fs::read(&target_path) {
        Ok(bytes) => bytes,
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
    let (final_contents, final_bytes_snapshot) = if current_bytes
        == target_bytes
    {
        (first_contents, target_bytes.clone())
    } else {
        // The vault changed under us: re-plan the insertion once.
        let fresh = String::from_utf8_lossy(&current_bytes).into_owned();
        let fresh_indent =
            capture::dominant_indent_unit(&capture::line_spans(&fresh))
                .unwrap_or("\t")
                .to_string();
        let mut fresh_writes = Vec::with_capacity(writes.len());
        for item in &writes {
            let revision = matches!(item.planned.state, NoteState::Revised);
            let block =
                render_note(&item.planned.note, &fresh_indent, revision);
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
        maybe_run_before_rename_hook();
        let second = match fs::read(&target_path) {
            Ok(bytes) => bytes,
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
        if second != current_bytes {
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
        (replanned, current_bytes.clone())
    };

    // Durable write: temp + create_new, same-dir rename, dir fsync.
    if let Err(error) = durable_write(&target_path, &final_contents) {
        return ui::report_error(
            "pull",
            &GkeepError::runtime(
                "vault",
                format!("write {}: {error}", target_path.display()),
            ),
            format_name,
        );
    }
    let _ = final_bytes_snapshot;

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
    let mut commit_sha: Option<String> = None;
    if !args.no_commit {
        let child_env = ob::child_env();
        let worktree = match ob::detect_git_worktree(&bob_dir, &child_env) {
            Ok(inside) => inside,
            Err(error) => {
                ui::warn(&format!(
                    "git is unavailable ({error}); skipping commit"
                ));
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
            &bob_dir,
            &target_rel,
            &plan,
            &verified,
            commit_sha.clone(),
            &styler,
        );
        for item in &writes {
            let idx = writes
                .iter()
                .position(|other| other.planned.note.id == item.planned.note.id)
                .unwrap_or(0);
            if !verify_outcome.ok[idx] {
                eprintln!(
                    "bob gkeep pull: verification failed for {}",
                    item.planned.note.id
                );
            }
        }
        return code.max(1);
    }

    finish_with_archive(
        args,
        &config,
        &client,
        &creds,
        &bob_dir,
        &target_rel,
        &plan,
        &verified,
        commit_sha,
        &styler,
    )
}

#[derive(Debug, Clone)]
struct WriteItem {
    planned: PlannedNote,
    markdown: String,
    fp: String,
}

fn acquire_pull_lock() -> Result<File, i32> {
    let path = pull_lock_path();
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(error) = fs::create_dir_all(parent)
    {
        eprintln!("bob gkeep pull: create pull lock dir: {error}");
        return Err(1);
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| {
            eprintln!(
                "bob gkeep pull: open pull lock {}: {error}",
                path.display()
            );
            1
        })?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Err(1),
        Err(error) => {
            eprintln!("bob gkeep pull: lock {}: {error}", path.display());
            Err(1)
        }
    }
}

fn durable_write(target: &Path, contents: &str) -> io::Result<()> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let perms = fs::metadata(target)?.permissions();
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
        let file = File::open(&temp)?;
        file.sync_all()?;
        fs::set_permissions(&temp, perms)?;
        fs::rename(&temp, target)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&temp);
    }
    outcome
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
    bob_dir: &Path,
    target_rel: &Path,
    plan: &Plan,
    verified: &[WriteItem],
    commit_sha: Option<String>,
    styler: &Styler,
) -> i32 {
    let is_json = args.format.is_json();
    // Archive set: every verified write plus every pending/revised note.
    // Pending notes were already verified by the ledger/journal lookup.
    let mut archive_notes: Vec<(String, KeepContent)> = Vec::new();
    let mut archive_for: Vec<String> = Vec::new();
    for item in verified {
        archive_notes.push((
            item.planned.note.id.clone(),
            item.planned.note.content.clone(),
        ));
        archive_for.push(item.planned.note.id.clone());
    }
    for planned in &plan.notes {
        if matches!(planned.action, PlanAction::ArchiveOnly) {
            let id = planned.note.id.clone();
            if !archive_for.contains(&id) {
                archive_notes.push((id.clone(), planned.note.content.clone()));
                archive_for.push(id);
            }
        }
        // Revised notes without a verified write (verify failed) stay out.
        // Revised notes with a verified write are already in the set.
    }
    // Nothing to archive and nothing written: the "nothing to pull" path.
    // Archive unless --no-archive.
    let mut archive_status: std::collections::BTreeMap<
        String,
        (String, Option<String>),
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
                for (id, _) in &archive_notes {
                    let hit = results.iter().find(|row| row.id == *id);
                    match hit {
                        Some(row) => {
                            let status = match row.status {
                                super::model::ArchiveStatus::Archived => {
                                    "archived"
                                }
                                super::model::ArchiveStatus::AlreadyArchived => {
                                    "already_archived"
                                }
                                super::model::ArchiveStatus::Changed => {
                                    "changed"
                                }
                                super::model::ArchiveStatus::Missing => {
                                    "missing"
                                }
                                super::model::ArchiveStatus::Error => "error",
                            };
                            archive_status.insert(
                                id.clone(),
                                (status.to_string(), row.detail.clone()),
                            );
                        }
                        None => {
                            archive_status.insert(
                                id.clone(),
                                (
                                    "error".to_string(),
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
    } else {
        for (id, _) in &archive_notes {
            archive_status
                .insert(id.clone(), ("not_requested".to_string(), None));
        }
    }
    if let Some(error) = adapter_failed {
        // Writes are committed; journal the writes so the next pull can
        // archive them as pending with no duplicate write.
        append_journal(
            plan,
            verified,
            &archive_status,
            target_rel,
            commit_sha.clone(),
            true,
        );
        if is_json {
            if !args.quiet {
                println!(
                    "{}",
                    json_pull_report(
                        args,
                        config,
                        plan,
                        verified,
                        &archive_status,
                        None,
                        target_rel,
                        commit_sha.clone(),
                        false,
                    )
                );
            }
            return ui::report_error("pull", &error, args.error_format());
        }
        if !args.quiet {
            print_human_report(
                args,
                config,
                plan,
                verified,
                &archive_status,
                target_rel,
                commit_sha.clone(),
                styler,
                false,
            );
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

    let failed = count_failed(plan, verified, &archive_status);
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
                target_rel,
                commit_sha,
                ok,
            )
        );
        return if ok { 0 } else { 1 };
    }

    if args.quiet && ok {
        return 0;
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
        config,
        plan,
        verified,
        &archive_status,
        target_rel,
        commit_sha,
        styler,
        ok,
    );
    // Changed notes get the documented next-pull hint on stderr.
    let changed = archive_status
        .values()
        .filter(|(status, _)| *status == "changed")
        .count();
    if changed > 0 {
        eprintln!(
            "warning {changed} note{} changed while pulling; {} in Keep and the next pull adds the revision",
            if changed == 1 { "" } else { "s" },
            if changed == 1 { "it stays" } else { "they stay" },
        );
    }
    let _ = bob_dir;
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
        (String, Option<String>),
    >,
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
        if matches!(status.as_str(), "changed" | "missing" | "error") {
            failed += 1;
        }
    }
    failed
}

fn append_journal(
    plan: &Plan,
    verified: &[WriteItem],
    archive_status: &std::collections::BTreeMap<
        String,
        (String, Option<String>),
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
            match status.as_str() {
                "archived" | "already_archived" => {
                    records.push(JournalRecord {
                        ts: ts.clone(),
                        event: JournalEvent::Archived,
                        id: id.clone(),
                        ref_,
                        fp,
                        path: target.clone(),
                        commit: commit.clone(),
                        status: Some(status.clone()),
                    });
                }
                "changed" | "missing" | "error" => {
                    records.push(JournalRecord {
                        ts: ts.clone(),
                        event: JournalEvent::ArchiveRefused,
                        id: id.clone(),
                        ref_,
                        fp,
                        path: target.clone(),
                        commit: commit.clone(),
                        status: Some(status.clone()),
                    });
                }
                _ => {}
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
            (String, Option<String>),
        > = plan
            .notes
            .iter()
            .filter(|item| item.state.is_actionable())
            .map(|item| {
                (item.note.id.clone(), ("not_attempted".to_string(), None))
            })
            .collect();
        println!(
            "{}",
            json_pull_report(
                args,
                config,
                plan,
                writes,
                &empty,
                inserted,
                &PathBuf::from(config.target()),
                None,
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
        let title = truncate_title(&display_title(&planned.note), styler);
        // Leak-free: build the owned string for the skip arm.
        let detail_owned;
        let detail: &str = match planned.action {
            PlanAction::Skip => {
                detail_owned = format!(
                    "skipped · {}",
                    planned.skip_reason.as_deref().unwrap_or("skipped")
                );
                &detail_owned
            }
            PlanAction::Write | PlanAction::WriteRevision => {
                "would write · would archive"
            }
            PlanAction::ArchiveOnly => "would archive · already in vault",
        };
        let _ = detail;
        let glyph = match planned.action {
            PlanAction::Skip => styler.dim("·"),
            _ => styler.dim("·"),
        };
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
    let written = writes.len();
    let archived = plan
        .notes
        .iter()
        .filter(|item| item.state.is_actionable())
        .count();
    let skipped = plan.notes.len() - actionable;
    println!(
        "{} {written} written · {archived} archived · {skipped} skipped",
        styler.success_prefix(true)
    );
    0
}

fn truncate_title(title: &str, _styler: &Styler) -> String {
    let available = style::terminal_width().saturating_sub(40).max(20);
    style::truncate(title, available)
}

#[allow(clippy::too_many_arguments)]
fn print_human_report(
    args: &PullArgs,
    _config: &GkeepConfig,
    plan: &Plan,
    verified: &[WriteItem],
    archive_status: &std::collections::BTreeMap<
        String,
        (String, Option<String>),
    >,
    target_rel: &Path,
    commit_sha: Option<String>,
    styler: &Styler,
    _ok: bool,
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
        let title = truncate_title(&display_title(&planned.note), styler);
        let id = planned.note.id.as_str();
        if matches!(planned.action, PlanAction::Skip) {
            let reason = planned.skip_reason.as_deref().unwrap_or("skipped");
            println!("  {} {title}  skipped · {reason}", styler.dim("·"));
            continue;
        }
        if matches!(planned.action, PlanAction::ArchiveOnly) {
            let (status, _) = archive_status
                .get(id)
                .cloned()
                .unwrap_or(("not_requested".to_string(), None));
            let detail = match status.as_str() {
                "not_requested" if args.no_archive => {
                    "left in Keep (--no-archive)".to_string()
                }
                "archived" | "already_archived" => {
                    "archived · already in vault".to_string()
                }
                "changed" => {
                    "NOT archived: edited in Keep during pull".to_string()
                }
                "missing" => "NOT archived: note is gone".to_string(),
                "error" => {
                    let detail = archive_status
                        .get(id)
                        .and_then(|(_, detail)| detail.clone())
                        .unwrap_or_default();
                    if detail.is_empty() {
                        "NOT archived: adapter error".to_string()
                    } else {
                        format!("NOT archived: {detail}")
                    }
                }
                _ => "would archive".to_string(),
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
        // Write or revision.
        if !verified_ids.contains(id) {
            println!(
                "  {} {title}  NOT written: verification failed",
                styler.red("✗")
            );
            continue;
        }
        let (status, detail_opt) = archive_status
            .get(id)
            .cloned()
            .unwrap_or(("not_requested".to_string(), None));
        let detail = if args.no_archive {
            "written · left in Keep (--no-archive)".to_string()
        } else {
            match status.as_str() {
                "archived" => "written · archived".to_string(),
                "already_archived" => "written · already archived".to_string(),
                "changed" => {
                    "written · NOT archived: edited in Keep during pull"
                        .to_string()
                }
                "missing" => "written · NOT archived: note is gone".to_string(),
                "error" => {
                    let extra = detail_opt.unwrap_or_default();
                    if extra.is_empty() {
                        "written · NOT archived: adapter error".to_string()
                    } else {
                        format!("written · NOT archived: {extra}")
                    }
                }
                "not_requested" => {
                    "written · left in Keep (--no-archive)".to_string()
                }
                _ => "written · archiving skipped".to_string(),
            }
        };
        let glyph = if detail.contains("NOT") {
            styler.yellow("!")
        } else {
            styler.green("✓")
        };
        // `!` rows use the warning color; successes use green.
        let glyph = if detail.contains("NOT archived: edited") {
            styler.yellow("!")
        } else if detail.contains("NOT") {
            styler.red("✗")
        } else {
            glyph
        };
        println!("  {glyph} {title}  {detail}");
    }
    println!();
    let written = verified.len();
    let archived = archive_status
        .values()
        .filter(|(status, _)| {
            matches!(status.as_str(), "archived" | "already_archived")
        })
        .count();
    let skipped = plan
        .notes
        .iter()
        .filter(|item| matches!(item.action, PlanAction::Skip))
        .count();
    let mut summary = format!(
        "{} {written} written · {archived} archived · {skipped} skipped",
        styler.success_prefix(false)
    );
    if let Some(sha) = commit_sha {
        let short: String = sha.chars().take(7).collect();
        summary.push_str(&format!(" · vault commit {short}"));
    }
    println!("{summary}");
}

#[allow(clippy::too_many_arguments)]
fn json_pull_report(
    args: &PullArgs,
    config: &GkeepConfig,
    plan: &Plan,
    verified: &[WriteItem],
    archive_status: &std::collections::BTreeMap<
        String,
        (String, Option<String>),
    >,
    markdown: Option<String>,
    target_rel: &Path,
    commit: Option<String>,
    ok: bool,
) -> serde_json::Value {
    let verified_ids: std::collections::BTreeSet<&str> = verified
        .iter()
        .map(|item| item.planned.note.id.as_str())
        .collect();
    let notes: Vec<serde_json::Value> = plan
        .notes
        .iter()
        .map(|planned| {
            let action = match planned.action {
                PlanAction::Write => "write",
                PlanAction::WriteRevision => "write_revision",
                PlanAction::ArchiveOnly => "archive_only",
                PlanAction::Skip => "skip",
            };
            let written = verified_ids.contains(planned.note.id.as_str());
            let (archive, detail) =
                archive_status.get(&planned.note.id).cloned().unwrap_or((
                    if matches!(planned.action, PlanAction::Skip) {
                        "not_requested".to_string()
                    } else {
                        "not_attempted".to_string()
                    },
                    None,
                ));
            json!({
                "id": planned.note.id,
                "ref": note_ref(&planned.note.id),
                "title": display_title(&planned.note),
                "state": planned.state.as_str(),
                "action": action,
                "skip_reason": planned.skip_reason,
                "written": written,
                "archive": archive,
                "detail": detail,
            })
        })
        .collect();
    let written = verified.len();
    let archived = archive_status
        .values()
        .filter(|(status, _)| {
            matches!(status.as_str(), "archived" | "already_archived")
        })
        .count();
    let skipped = plan
        .notes
        .iter()
        .filter(|item| matches!(item.action, PlanAction::Skip))
        .count();
    let failed = notes.len() - written - skipped
        + archive_status
            .values()
            .filter(|(status, _)| {
                matches!(status.as_str(), "changed" | "missing" | "error")
            })
            .count();
    let _ = target_rel;
    json!({
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
        }
    })
}

#[allow(dead_code)]
fn human_format_of(args: &PullArgs) -> HumanFormat {
    args.format
}
