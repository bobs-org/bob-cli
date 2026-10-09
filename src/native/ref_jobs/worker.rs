//! Single-flight background clip worker: `bob ref jobs run`.
//!
//! The worker holds `worker.lock` (fs2, non-blocking) for the whole
//! pass: a second worker exits 0. It recovers stale `running/` jobs,
//! retries `stuck/` fallbacks without clipping again, then clips
//! `pending/` oldest first through the shared ingest. After releasing
//! the lock it re-checks `pending/` so a job queued mid-pass is never
//! stranded without another kick.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use crate::native::highlights_ref::ingest::{
    fallback_note_for, ingest_url, IngestError, IngestOutcome, IngestRequest,
};

use super::fallback::write_fallback;
use super::spool::{
    append_done, atomic_install, dir_rows, ensure_spool_dirs, move_job,
    DoneOutcome, JobFile, StoredError,
};

/// Exit codes: 0 when every processed job reached a terminal outcome,
/// 1 when any job is left in `stuck/`.
pub(crate) fn run_jobs(root: &Path, quiet: bool) -> i32 {
    let _ = ensure_spool_dirs(root);
    trim_worker_log(root);
    let mut outcomes = Vec::new();
    let mut acc_code = 0;
    let mut first = true;
    let code = loop {
        let Some(lock) = take_worker_lock(root) else {
            if first {
                if !quiet {
                    println!("another clip worker is running");
                }
                return 0;
            }
            break acc_code;
        };
        first = false;
        let mut pass = WorkerPass::new(root, quiet);
        pass.recover_running();
        pass.retry_stuck();
        let failed = pass.drain_pending();
        outcomes.extend(std::mem::take(&mut pass.outcomes));
        let code = pass.exit_code(root);
        acc_code = acc_code.max(code);
        drop(lock);
        // Lost-wakeup guard: a job queued after the drain started (its
        // kick found our lock held) must not wait for another kick.
        // When the drain skipped files (e.g. `running/` unwritable) those
        // stay in `pending/`; looping on them alone would spin the outer
        // pass forever, so only loop when a non-failed pending file
        // remains.
        if pending_count(root) == 0 {
            break acc_code;
        }
        if pending_paths(root).iter().all(|path| failed.contains(path)) {
            break acc_code;
        }
    };
    print_run_report(&outcomes, quiet);
    code
}

fn pending_count(root: &Path) -> usize {
    dir_rows(root, "pending").len()
}

fn pending_paths(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root.join("pending")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect()
}

/// One locked worker pass over the spool.
struct WorkerPass<'a> {
    root: &'a Path,
    quiet: bool,
    outcomes: Vec<RunOutcome>,
    stuck_now: bool,
}

struct RunOutcome {
    display: String,
    line: OutcomeLine,
}

enum OutcomeLine {
    Clipped {
        pdf: String,
    },
    AlreadyInLibrary {
        note: String,
    },
    AlreadyQueued {
        pdf: String,
    },
    FellBack {
        target: String,
        kind: String,
        message: String,
    },
}

impl<'a> WorkerPass<'a> {
    fn new(root: &'a Path, quiet: bool) -> Self {
        Self {
            root,
            quiet,
            outcomes: Vec::new(),
            stuck_now: false,
        }
    }

    fn exit_code(&self, root: &Path) -> i32 {
        if self.stuck_now || !dir_rows(root, "stuck").is_empty() {
            1
        } else {
            0
        }
    }

    /// Stale `running/` files belong to a dead worker: we hold the
    /// lock, so nobody is clipping them. Attempts increment first, then
    /// at 2+ they fail as `internal` without clipping again; otherwise
    /// back to pending.
    fn recover_running(&mut self) {
        for entry in dir_rows(self.root, "running") {
            let (job, path) = match entry {
                Ok(row) => row,
                Err(message) => {
                    park_unreadable(self.root, &message);
                    self.stuck_now = true;
                    continue;
                }
            };
            let pending =
                self.root.join("pending").join(format!("{}.json", job.id));
            if job.attempts + 1 >= 2 {
                let error = IngestError::internal(
                    "the clip worker stopped twice while clipping this link",
                );
                self.finish_failed(job, &path, &error);
            } else if let Err(error) = move_job(&path, &pending, false, true) {
                self.stuck_now = true;
                eprintln!("bob ref jobs: recover {}: {error}", job.id);
            }
        }
    }

    /// Retry the fallback write for `stuck/` jobs without clipping
    /// again. A success appends `fell_back` and removes the job; a
    /// repeat failure leaves it parked.
    fn retry_stuck(&mut self) {
        for entry in dir_rows(self.root, "stuck") {
            let (job, path) = match entry {
                Ok(row) => row,
                Err(_) => {
                    self.stuck_now = true;
                    continue;
                }
            };
            let Some(error) = job.error.clone() else {
                self.stuck_now = true;
                continue;
            };
            let note =
                error_note(&error, &job.cleaned_url, job.effective_parent());
            match write_fallback(
                &job.bob_dir,
                &job.fallback.relative_target,
                &job.fallback.task_line,
                &note,
            ) {
                Ok(_) => {
                    self.record_done(
                        &job,
                        DoneOutcome::FellBack,
                        None,
                        None,
                        Some(error),
                        Some(job.fallback.relative_target.clone()),
                    );
                    let _ = fs::remove_file(&path);
                    self.outcomes.push(RunOutcome {
                        display: job.display.clone(),
                        line: OutcomeLine::FellBack {
                            target: job.fallback.relative_target.clone(),
                            kind: "stuck".to_string(),
                            message: "fallback write retried".to_string(),
                        },
                    });
                }
                Err(_) => {
                    self.stuck_now = true;
                }
            }
        }
    }

    /// Clip `pending/`, oldest `created_at` first, then by `id`.
    ///
    /// A failing `move_job` into `running/` (or a failed `stuck/` park)
    /// leaves the file in `pending/`; remembered paths are skipped so one
    /// bad file cannot spin the drain forever. Returns the skipped paths
    /// so the lost-wakeup guard does not loop on them alone.
    fn drain_pending(&mut self) -> HashSet<PathBuf> {
        let mut failed: HashSet<PathBuf> = HashSet::new();
        loop {
            let next =
                dir_rows(self.root, "pending").into_iter().find(|entry| {
                    match entry {
                        Ok((_, path)) => !failed.contains(path),
                        Err(message) => {
                            let path_text =
                                message.split(": ").next().unwrap_or("");
                            !failed.contains(&PathBuf::from(path_text))
                        }
                    }
                });
            let Some(entry) = next else {
                return failed;
            };
            let (job, path) = match entry {
                Ok(row) => row,
                Err(message) => {
                    let path_text =
                        message.split(": ").next().unwrap_or("").to_string();
                    park_unreadable(self.root, &message);
                    if Path::new(&path_text).exists() {
                        failed.insert(PathBuf::from(path_text));
                    }
                    self.stuck_now = true;
                    continue;
                }
            };
            let running =
                self.root.join("running").join(format!("{}.json", job.id));
            let job = match move_job(&path, &running, true, false) {
                Ok(job) => job,
                Err(error) => {
                    failed.insert(path.clone());
                    self.stuck_now = true;
                    eprintln!("bob ref jobs: claim {}: {error}", job.id);
                    continue;
                }
            };
            self.clip_one(&job, &running);
        }
    }

    fn clip_one(&mut self, job: &JobFile, running: &Path) {
        say(self.quiet, format!("⟳ clipping {}…", job.display));
        let progress = |line: &str| {
            if !self.quiet {
                println!("  {line}");
            }
        };
        // A job without a parent uses its source's inbox (jobs written
        // by an older `bob` carry none).
        let request = IngestRequest {
            bob_dir: &job.bob_dir,
            url: &job.cleaned_url,
            parent: job.effective_parent(),
            progress: Some(&progress),
        };
        match ingest_url(&request) {
            Ok(IngestOutcome::Created { pdf, .. }) => {
                self.record_done(
                    job,
                    DoneOutcome::Created,
                    Some(pdf.clone()),
                    None,
                    None,
                    None,
                );
                let _ = fs::remove_file(running);
                self.outcomes.push(RunOutcome {
                    display: job.display.clone(),
                    line: OutcomeLine::Clipped { pdf },
                });
            }
            Ok(IngestOutcome::AlreadyInLibrary { note }) => {
                self.record_done(
                    job,
                    DoneOutcome::AlreadyInLibrary,
                    None,
                    Some(note.clone()),
                    None,
                    None,
                );
                let _ = fs::remove_file(running);
                self.outcomes.push(RunOutcome {
                    display: job.display.clone(),
                    line: OutcomeLine::AlreadyInLibrary { note },
                });
            }
            Ok(IngestOutcome::AlreadyQueued { pdf }) => {
                self.record_done(
                    job,
                    DoneOutcome::AlreadyQueued,
                    Some(pdf.clone()),
                    None,
                    None,
                    None,
                );
                let _ = fs::remove_file(running);
                self.outcomes.push(RunOutcome {
                    display: job.display.clone(),
                    line: OutcomeLine::AlreadyQueued { pdf },
                });
            }
            Err(error) => {
                self.finish_failed(job.clone(), running, &error);
            }
        }
    }

    /// Write the fallback task for a failed clip. A fallback that
    /// itself fails parks the job in `stuck/` with both errors.
    fn finish_failed(
        &mut self,
        job: JobFile,
        running: &Path,
        error: &IngestError,
    ) {
        let stored = StoredError {
            kind: error.kind.as_str().to_string(),
            message: first_line(&error.message),
            retryable: error.retryable(),
        };
        // The fallback task lands in the job's parent note (staged as
        // `<parent>.md` by capture) with a retry command naming `-P`.
        let note =
            error.fallback_note(&job.cleaned_url, job.effective_parent());
        match write_fallback(
            &job.bob_dir,
            &job.fallback.relative_target,
            &job.fallback.task_line,
            &note,
        ) {
            Ok(_) => {
                self.record_done(
                    &job,
                    DoneOutcome::FellBack,
                    None,
                    None,
                    Some(stored),
                    Some(job.fallback.relative_target.clone()),
                );
                let _ = fs::remove_file(running);
                self.outcomes.push(RunOutcome {
                    display: job.display.clone(),
                    line: OutcomeLine::FellBack {
                        target: job.fallback.relative_target.clone(),
                        kind: error.kind.as_str().to_string(),
                        message: first_line(&error.message),
                    },
                });
            }
            Err(fallback_error) => {
                self.stuck_now = true;
                let stuck =
                    self.root.join("stuck").join(format!("{}.json", job.id));
                let mut parked = job.clone();
                parked.error = Some(stored);
                parked.fallback_error = Some(fallback_error);
                let bytes =
                    serde_json::to_vec_pretty(&parked).unwrap_or_default();
                if !bytes.is_empty() {
                    let _ = atomic_install(&stuck, &bytes);
                }
                let _ = fs::remove_file(running);
                eprintln!(
                    "bob ref jobs: fallback write failed for {}: parked in stuck/",
                    job.display
                );
            }
        }
    }

    fn record_done(
        &self,
        job: &JobFile,
        outcome: DoneOutcome,
        pdf: Option<String>,
        note: Option<String>,
        error: Option<StoredError>,
        fallback_target: Option<String>,
    ) {
        if let Err(error) = append_done(
            self.root,
            job,
            outcome,
            pdf,
            note,
            error,
            fallback_target,
        ) {
            eprintln!("bob ref jobs: {}", error);
        }
    }
}

/// Print one worker line unless quiet.
fn say(quiet: bool, line: String) {
    if !quiet {
        println!("{line}");
    }
}

fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or_default().to_string()
}

/// The `stuck/` retry note reuses the clip's own fallback text,
/// recomputed from the stored kind and message.
fn error_note(error: &StoredError, cleaned_url: &str, parent: &str) -> String {
    fallback_note_for(&error.kind, &error.message, cleaned_url, parent)
}

/// Move an unreadable spool file to `stuck/` preserving its bytes, so
/// `list` can show it and the pending loop cannot spin on it.
fn park_unreadable(root: &Path, message: &str) {
    let Some(path_text) = message.split(": ").next() else {
        return;
    };
    let from = Path::new(path_text);
    let Some(name) = from.file_name() else {
        return;
    };
    let to = root.join("stuck").join(name);
    if from != to {
        let _ = fs::rename(from, &to);
    }
}

/// Non-blocking single-flight guard: `None` when another worker holds
/// `worker.lock`. The returned file keeps the lock until dropped.
fn take_worker_lock(root: &Path) -> Option<fs::File> {
    let _ = fs::create_dir_all(root);
    let path = root.join("worker.lock");
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    {
        use fs2::FileExt;
        file.try_lock_exclusive().ok()?;
    }
    Some(file)
}

/// At startup, trim `worker.log` to its last 256 KiB once it exceeds
/// 1 MiB.
fn trim_worker_log(root: &Path) {
    const LIMIT: u64 = 1024 * 1024;
    const KEEP: u64 = 256 * 1024;
    let path = root.join("worker.log");
    let Ok(metadata) = fs::metadata(&path) else {
        return;
    };
    if metadata.len() <= LIMIT {
        return;
    }
    let Ok(bytes) = fs::read(&path) else {
        return;
    };
    let start = bytes.len().saturating_sub(KEEP as usize);
    // Keep whole lines: start after the first newline in the window.
    let mut keep = start;
    while keep < bytes.len() && bytes[keep] != b'\n' {
        keep += 1;
    }
    if keep < bytes.len() {
        keep += 1;
    }
    let _ = fs::write(&path, &bytes[keep.min(bytes.len())..]);
}

/// Print the human `run` report: one line per job, then a summary.
fn print_run_report(outcomes: &[RunOutcome], quiet: bool) {
    use crate::native::style::Styler;
    let styler = Styler::detect();
    if !quiet {
        for outcome in outcomes {
            println!("{}", render_outcome_line(outcome, styler));
        }
    }
    if outcomes.is_empty() {
        println!("nothing to do");
        return;
    }
    let mut counts: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    for outcome in outcomes {
        let key = match &outcome.line {
            OutcomeLine::Clipped { .. } => "clipped",
            OutcomeLine::AlreadyInLibrary { .. } => "already in library",
            OutcomeLine::AlreadyQueued { .. } => "already queued",
            OutcomeLine::FellBack { .. } => "fell back",
        };
        *counts.entry(key).or_insert(0) += 1;
    }
    let summary = counts
        .iter()
        .map(|(key, count)| format!("{count} {key}"))
        .collect::<Vec<_>>()
        .join(" · ");
    println!("{} {summary}", styler.green("ok"));
}

fn render_outcome_line(
    outcome: &RunOutcome,
    styler: crate::native::style::Styler,
) -> String {
    let display = styler.cyan(&outcome.display);
    match &outcome.line {
        OutcomeLine::Clipped { pdf } => {
            format!(
                "{} clipped {display} → {}",
                styler.green("✓"),
                styler.cyan(pdf)
            )
        }
        OutcomeLine::AlreadyInLibrary { note } => {
            format!(
                "{} already in library {display} · {}",
                styler.green("✓"),
                styler.cyan(note)
            )
        }
        OutcomeLine::AlreadyQueued { pdf } => {
            format!(
                "{} already queued {display} · {}",
                styler.green("✓"),
                styler.cyan(pdf)
            )
        }
        OutcomeLine::FellBack {
            target,
            kind,
            message,
        } => {
            format!(
                "{} fell back {display} → {} ({}: {})",
                styler.yellow("↩"),
                styler.cyan(target),
                kind,
                styler.dim(message)
            )
        }
    }
}
