//! One report for human and JSON rendering.
//!
//! Execution fills a [`Report`] — the plan, the Git outcome, and the exit
//! record — and rendering reads only that report. Planned moves and applied
//! changes are separate facts: `moves` is what the plan intends, `exit` says
//! what actually happened on disk.
use super::*;

use serde_json::{json, Map, Value};

use git::{GitOutcome, GitReport};

/// JSON contract version for `bob task archive -f json`.
pub(super) const SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    /// A dry run planned and reported; nothing was written.
    Preview,
    /// The plan was empty: nothing to write, Git skipped.
    Noop,
    /// Every output was installed (and Git completed when it applied).
    Applied,
    /// The run stopped; `reason` and `message` say why.
    Failed,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Noop => "noop",
            Self::Applied => "applied",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExitReport {
    pub(super) code: i32,
    pub(super) outcome: Outcome,
    pub(super) reason: Option<&'static str>,
    pub(super) message: Option<String>,
    /// Vault-relative paths actually installed.
    pub(super) applied_paths: Vec<PathBuf>,
    /// Vault-relative paths planned but not installed.
    pub(super) deferred_paths: Vec<PathBuf>,
    pub(super) recovery_directory: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Report {
    pub(super) vault: PathBuf,
    pub(super) threshold: usize,
    pub(super) dry_run: bool,
    pub(super) plan: Option<CollectionPlan>,
    pub(super) git: GitReport,
    pub(super) exit: ExitReport,
}

impl Report {
    pub(super) fn new(vault: PathBuf, threshold: usize, dry_run: bool) -> Self {
        Self {
            vault,
            threshold,
            dry_run,
            plan: None,
            git: GitReport::not_run(if dry_run {
                "dry_run"
            } else {
                "incomplete"
            }),
            exit: ExitReport {
                code: 1,
                outcome: Outcome::Failed,
                reason: Some("incomplete"),
                message: Some("the run did not complete".to_string()),
                applied_paths: Vec::new(),
                deferred_paths: Vec::new(),
                recovery_directory: None,
            },
        }
    }

    pub(super) fn exit_code(&self) -> i32 {
        self.exit.code
    }

    pub(super) fn preview(&mut self) {
        self.git = GitReport::not_run("dry_run");
        self.exit = ExitReport {
            code: 0,
            outcome: Outcome::Preview,
            reason: None,
            message: None,
            applied_paths: Vec::new(),
            deferred_paths: Vec::new(),
            recovery_directory: None,
        };
        self.flag_duplicate_holds();
    }

    pub(super) fn noop(&mut self) {
        self.git = GitReport::no_changes();
        self.exit = ExitReport {
            code: 0,
            outcome: Outcome::Noop,
            reason: None,
            message: None,
            applied_paths: Vec::new(),
            deferred_paths: Vec::new(),
            recovery_directory: None,
        };
        self.flag_duplicate_holds();
    }

    pub(super) fn applied(
        &mut self,
        applied_paths: Vec<PathBuf>,
        recovery_directory: PathBuf,
        git: GitReport,
    ) {
        let git_failure = git.failure.clone();
        self.git = git;
        self.exit = ExitReport {
            code: 0,
            outcome: Outcome::Applied,
            reason: None,
            message: None,
            applied_paths,
            deferred_paths: Vec::new(),
            recovery_directory: Some(recovery_directory),
        };
        if let Some(failure) = git_failure {
            self.exit.code = failure.exit_code;
            self.exit.outcome = Outcome::Failed;
            self.exit.reason = Some("git_failed");
            self.exit.message = Some(format!(
                "vault changes were applied, but {} (the archive was not rerun)",
                failure.message
            ));
        } else {
            self.flag_duplicate_holds();
        }
    }

    fn flag_duplicate_holds(&mut self) {
        let Some(plan) = &self.plan else {
            return;
        };
        let mut notes: Vec<String> = plan
            .holds
            .iter()
            .filter(|hold| hold.reason == "duplicate_block_id")
            .map(|hold| path_text(&hold.relative_path))
            .collect();
        if notes.is_empty() {
            return;
        }
        notes.sort();
        notes.dedup();
        self.exit.code = 1;
        self.exit.reason = Some("duplicate_block_id");
        self.exit.message = Some(format!(
            "held blocks with duplicate ids in {}; fix duplicates and rerun",
            notes.join(", ")
        ));
    }

    pub(super) fn fail(&mut self, reason: &'static str, message: String) {
        self.exit = ExitReport {
            code: 1,
            outcome: Outcome::Failed,
            reason: Some(reason),
            message: Some(message),
            applied_paths: Vec::new(),
            deferred_paths: Vec::new(),
            recovery_directory: None,
        };
        self.git = GitReport::not_run("apply_incomplete");
    }

    pub(super) fn fail_with_paths(
        &mut self,
        reason: &'static str,
        message: String,
        applied_paths: Vec<PathBuf>,
        deferred_paths: Vec<PathBuf>,
        recovery_directory: Option<PathBuf>,
    ) {
        self.fail(reason, message);
        self.exit.applied_paths = applied_paths;
        self.exit.deferred_paths = deferred_paths;
        self.exit.recovery_directory = recovery_directory;
    }

    fn archive_counts(&self) -> (usize, usize) {
        let Some(plan) = &self.plan else {
            return (0, 0);
        };
        let created = plan
            .files
            .iter()
            .filter(|file| file.writes_archive() && file.archive_created)
            .count();
        let updated = plan
            .files
            .iter()
            .filter(|file| file.writes_archive() && !file.archive_created)
            .count();
        (created, updated)
    }
}

/// Vault-relative path with `/` separators, independent of the platform.
pub(super) fn path_text(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn path_list(paths: &[PathBuf]) -> Value {
    Value::Array(
        paths
            .iter()
            .map(|path| Value::String(path_text(path)))
            .collect(),
    )
}

// ---------------------------------------------------------------- JSON

pub(super) fn render_json(report: &Report) -> String {
    let plan = report.plan.as_ref();
    let value = json!({
        "schema_version": SCHEMA_VERSION,
        "dry_run": report.dry_run,
        "vault": report.vault.display().to_string(),
        "threshold": report.threshold,
        "scan": plan.map(scan_json).unwrap_or(Value::Null),
        "moves": plan.map(moves_json).unwrap_or_else(|| json!([])),
        "holds": plan.map(holds_json).unwrap_or_else(|| json!([])),
        "skips": plan.map(skips_json).unwrap_or_else(|| json!({})),
        "grandfathered": plan
            .map(|plan| path_list(&plan.grandfathered))
            .unwrap_or_else(|| json!([])),
        "link_repairs": plan
            .map(link_repairs_json)
            .unwrap_or_else(empty_link_repairs_json),
        "git": git_json(&report.git),
        "exit": exit_json(&report.exit),
    });
    value.to_string()
}

fn scan_json(plan: &CollectionPlan) -> Value {
    json!({
        "markdown_files": plan.scanned_files,
        "files_meeting_threshold": plan.task_move_file_count(),
        "task_blocks": plan.total_task_count(),
        "source_done_tasks_updates": plan.source_metadata_update_count(),
        "archive_metadata_repairs": plan.archive_metadata_update_count(),
        "moved_block_ids": plan.moved_block_id_count(),
        "ambiguous_moved_block_ids": plan.ambiguous_moved_block_id_count(),
        "moved_block_id_renames": plan.moved_block_id_rename_count(),
        "planned_bytes": plan.planned_bytes(),
    })
}

fn sorted_files(plan: &CollectionPlan) -> Vec<&FilePlan> {
    let mut files: Vec<&FilePlan> = plan.files.iter().collect();
    files.sort_by(|left, right| {
        left.relative_source_path.cmp(&right.relative_source_path)
    });
    files
}

fn root_json(line: usize, block_id: &Option<String>) -> Value {
    json!({"line": line, "block_id": block_id})
}

fn moves_json(plan: &CollectionPlan) -> Value {
    Value::Array(
        sorted_files(plan)
            .into_iter()
            .map(|file| {
                let roots: Vec<Value> = file
                    .moved_roots
                    .iter()
                    .map(|record| root_json(record.source_line, &record.root_id))
                    .collect();
                let replays: Vec<Value> = file
                    .moved_roots
                    .iter()
                    .filter(|record| record.replayed)
                    .map(|record| root_json(record.source_line, &record.root_id))
                    .collect();
                let block_ids: Vec<&String> = file
                    .moved_roots
                    .iter()
                    .flat_map(|record| record.final_ids.iter())
                    .collect();
                let id_renames: Vec<Value> = file
                    .moved_block_id_final_ids
                    .iter()
                    .filter(|(from, to)| from != to)
                    .map(|(from, to)| json!({"from": from, "to": to}))
                    .collect();
                json!({
                    "source": path_text(&file.relative_source_path),
                    "archive": path_text(&file.relative_archive_path),
                    "task_count": file.task_count,
                    "writes_source": file.writes_source(),
                    "writes_archive": file.writes_archive(),
                    "creates_archive": file.writes_archive() && file.archive_created,
                    "roots": roots,
                    "block_ids": block_ids,
                    "id_renames": id_renames,
                    "replays": replays,
                })
            })
            .collect(),
    )
}

fn holds_json(plan: &CollectionPlan) -> Value {
    Value::Array(
        plan.holds
            .iter()
            .map(|hold| {
                json!({
                    "path": path_text(&hold.relative_path),
                    "line": hold.line,
                    "block_id": hold.block_id,
                    "reason": hold.reason,
                })
            })
            .collect(),
    )
}

fn skips_json(plan: &CollectionPlan) -> Value {
    let mut map = Map::new();
    for (reason, count) in &plan.skip_counts {
        map.insert(reason.clone(), json!(count));
    }
    Value::Object(map)
}

fn link_repairs_json(plan: &CollectionPlan) -> Value {
    let files: Vec<PathBuf> = plan.link_repair_files().into_iter().collect();
    json!({
        "files": path_list(&files),
        "link_count": plan.link_repair_count(),
        "dependency_metadata_count": plan.dependency_metadata_repair_count(),
    })
}

fn empty_link_repairs_json() -> Value {
    json!({"files": [], "link_count": 0, "dependency_metadata_count": 0})
}

fn git_json(git: &GitReport) -> Value {
    json!({
        "outcome": git.outcome.as_str(),
        "skip_reason": git.skip_reason,
        "warning": git.warning,
        "committed": git.committed,
        "commit_message": git.commit_message,
        "pushed": git.pushed,
        "paths": path_list(&git.paths),
        "failure": git.failure.as_ref().map(|failure| json!({
            "step": failure.step,
            "message": failure.message,
            "exit_code": failure.exit_code,
        })),
    })
}

fn exit_json(exit: &ExitReport) -> Value {
    json!({
        "code": exit.code,
        "outcome": exit.outcome.as_str(),
        "reason": exit.reason,
        "message": exit.message,
        "applied_paths": path_list(&exit.applied_paths),
        "deferred_paths": path_list(&exit.deferred_paths),
        "recovery_directory": exit
            .recovery_directory
            .as_ref()
            .map(|path| path.display().to_string()),
    })
}

// --------------------------------------------------------------- human

/// Lines printed before planning starts, so a long run shows its target.
pub(super) fn human_header(report: &Report) -> String {
    let mut text = String::from("Move done tasks\n");
    if report.dry_run {
        text.push_str("mode: dry run (no vault changes will be made)\n");
    }
    text.push_str(&format!("vault: {}\n", report.vault.display()));
    text.push_str(&format!("threshold: {}\n", report.threshold));
    text
}

/// Labeled sections: scan, holds, skips, grandfathered, moves, git, summary.
pub(super) fn render_human(report: &Report) -> String {
    let mut out = String::new();
    let Some(plan) = &report.plan else {
        out.push_str("summary:\n");
        push_failure(&mut out, report);
        return out;
    };

    push_scan(&mut out, plan);
    push_selection(&mut out, plan);
    push_scan_counts(&mut out, plan);

    out.push_str("moves:\n");
    if plan.is_empty() {
        out.push_str("  none\n");
    } else {
        for file in &plan.files {
            out.push_str(&format!("  {}\n", move_line(file)));
        }
        for repair in &plan.link_repairs {
            out.push_str(&format!(
                "  {} ({} Obsidian link repairs, {} dependency metadata repairs)\n",
                repair.relative_path.display(),
                repair.link_count,
                repair.dependency_metadata_count
            ));
        }
    }

    push_git(&mut out, report);

    out.push_str("summary:\n");
    match report.exit.outcome {
        Outcome::Failed => push_failure(&mut out, report),
        Outcome::Noop => out.push_str(
            "  no task blocks met the threshold; no vault changes made.\n",
        ),
        Outcome::Preview if plan.is_empty() => out.push_str(
            "  dry run: no task blocks met the threshold; no vault changes would be made.\n",
        ),
        Outcome::Preview | Outcome::Applied => {
            push_summary_counts(&mut out, report, plan)
        }
    }
    out
}

fn push_scan(out: &mut String, plan: &CollectionPlan) {
    out.push_str("scan:\n");
    out.push_str(&format!("  markdown files: {}\n", plan.scanned_files));
    out.push_str(&format!(
        "  files meeting threshold: {}\n",
        plan.task_move_file_count()
    ));
    out.push_str(&format!("  task blocks: {}\n", plan.total_task_count()));
}

fn push_selection(out: &mut String, plan: &CollectionPlan) {
    out.push_str("holds:\n");
    if plan.holds.is_empty() {
        out.push_str("  none\n");
    } else {
        for hold in &plan.holds {
            out.push_str(&format!(
                "  {}:{} {}\n",
                hold.relative_path.display(),
                hold.line,
                hold.reason
            ));
        }
    }
    out.push_str("skips:\n");
    if plan.skip_counts.is_empty() {
        out.push_str("  none\n");
    } else {
        for (reason, count) in &plan.skip_counts {
            out.push_str(&format!("  {reason}: {count}\n"));
        }
    }
    out.push_str("grandfathered:\n");
    if plan.grandfathered.is_empty() {
        out.push_str("  none\n");
    } else {
        for path in &plan.grandfathered {
            out.push_str(&format!("  {}\n", path.display()));
        }
    }
}

fn push_scan_counts(out: &mut String, plan: &CollectionPlan) {
    out.push_str(&format!(
        "  source done_tasks updates: {}\n",
        plan.source_metadata_update_count()
    ));
    out.push_str(&format!(
        "  archive metadata repairs: {}\n",
        plan.archive_metadata_update_count()
    ));
    out.push_str(&format!(
        "  moved block ids: {}\n",
        plan.moved_block_id_count()
    ));
    out.push_str(&format!(
        "  ambiguous moved block ids: {}\n",
        plan.ambiguous_moved_block_id_count()
    ));
    out.push_str(&format!(
        "  moved block id renames: {}\n",
        plan.moved_block_id_rename_count()
    ));
    out.push_str(&format!(
        "  Obsidian links repaired: {}\n",
        plan.link_repair_count()
    ));
    out.push_str(&format!(
        "  dependency metadata repaired: {}\n",
        plan.dependency_metadata_repair_count()
    ));
    out.push_str(&format!(
        "  link-repair files: {}\n",
        plan.link_repair_file_count()
    ));
    out.push_str(&format!("  planned bytes: {}\n", plan.planned_bytes()));
}

fn move_line(file: &FilePlan) -> String {
    if file.writes_archive() {
        if file.task_count > 0 {
            format!(
                "{} -> {} ({} task blocks)",
                file.relative_source_path.display(),
                file.relative_archive_path.display(),
                file.task_count
            )
        } else if file.source_metadata_updated {
            format!(
                "{} -> {} (source/archive metadata)",
                file.relative_source_path.display(),
                file.relative_archive_path.display()
            )
        } else if file.archive_link_repair_count > 0 {
            format!(
                "{} ({} Obsidian link repairs)",
                file.relative_archive_path.display(),
                file.archive_link_repair_count
            )
        } else {
            format!(
                "{} -> {} (archive metadata)",
                file.relative_source_path.display(),
                file.relative_archive_path.display()
            )
        }
    } else if file.source_link_repair_count > 0 && !file.source_metadata_updated
    {
        format!(
            "{} ({} Obsidian link repairs)",
            file.relative_source_path.display(),
            file.source_link_repair_count
        )
    } else {
        format!(
            "{} -> {} (done_tasks metadata)",
            file.relative_source_path.display(),
            file.relative_archive_path.display()
        )
    }
}

fn push_git(out: &mut String, report: &Report) {
    let git = &report.git;
    out.push_str("git:\n");
    match git.outcome {
        GitOutcome::NotRun => match git.skip_reason {
            Some("dry_run") => {
                out.push_str("  not run: dry run (Git is never touched)\n")
            }
            _ => out
                .push_str("  not run: vault changes did not finish applying\n"),
        },
        GitOutcome::Skipped => match git.skip_reason {
            Some("no_changes") => out.push_str("  skipped: no vault changes\n"),
            Some("nothing_to_commit") => {
                out.push_str("  detected: git worktree\n");
                if let Some(staged) = git.staged_paths {
                    out.push_str(&format!("  staged paths: {staged}\n"));
                }
                out.push_str("  skipped: no collection changes to commit\n");
            }
            _ => {
                if let Some(warning) = &git.warning {
                    out.push_str(&format!("  warning: {warning}\n"));
                }
            }
        },
        GitOutcome::Pushed | GitOutcome::Failed => {
            if git.detected_worktree {
                out.push_str("  detected: git worktree\n");
            }
            if let Some(staged) = git.staged_paths {
                out.push_str(&format!("  staged paths: {staged}\n"));
            }
            if let Some(message) = &git.commit_message {
                out.push_str(&format!("  committed: {message}\n"));
            }
            if git.pushed {
                out.push_str("  pushed\n");
            }
            if let Some(failure) = &git.failure {
                out.push_str(&format!("  failed: {}\n", failure.message));
            }
        }
    }
}

fn push_failure(out: &mut String, report: &Report) {
    let exit = &report.exit;
    out.push_str(&format!(
        "  failed: {}: {}\n",
        exit.reason.unwrap_or("failed"),
        exit.message
            .as_deref()
            .unwrap_or("the run did not complete")
    ));
    if !exit.applied_paths.is_empty() {
        out.push_str("  applied paths:\n");
        for path in &exit.applied_paths {
            out.push_str(&format!("    {}\n", path_text(path)));
        }
    }
    if !exit.deferred_paths.is_empty() {
        out.push_str("  deferred paths:\n");
        for path in &exit.deferred_paths {
            out.push_str(&format!("    {}\n", path_text(path)));
        }
    }
    if let Some(recovery) = &exit.recovery_directory {
        out.push_str(&format!(
            "  recovery directory: {}\n",
            recovery.display()
        ));
    }
}

fn push_summary_counts(
    out: &mut String,
    report: &Report,
    plan: &CollectionPlan,
) {
    let (created, updated) = report.archive_counts();
    if report.dry_run {
        out.push_str("  dry run: no vault changes made\n");
    }
    // (applied label, preview label, count)
    let rows = [
        (
            "moved task blocks",
            "task blocks to move",
            plan.total_task_count(),
        ),
        (
            "source files updated",
            "source files to update",
            plan.source_file_update_count(),
        ),
        (
            "source done_tasks updated",
            "source done_tasks to update",
            plan.source_metadata_update_count(),
        ),
        (
            "archive metadata repaired",
            "archive metadata to repair",
            plan.archive_metadata_update_count(),
        ),
        (
            "moved block ids",
            "block ids to move",
            plan.moved_block_id_count(),
        ),
        (
            "moved block id renames",
            "block id renames",
            plan.moved_block_id_rename_count(),
        ),
        (
            "Obsidian links repaired",
            "Obsidian links to repair",
            plan.link_repair_count(),
        ),
        (
            "dependency metadata repaired",
            "dependency metadata to repair",
            plan.dependency_metadata_repair_count(),
        ),
        (
            "link-repair files updated",
            "link-repair files to update",
            plan.link_repair_file_count(),
        ),
        ("archive files created", "archive files to create", created),
        ("archive files updated", "archive files to update", updated),
    ];
    for (applied, preview, count) in rows {
        let label = if report.dry_run { preview } else { applied };
        out.push_str(&format!("  {label}: {count}\n"));
    }
}
