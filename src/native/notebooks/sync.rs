//! `bob notebooks sync`: repair every generated `## Notebooks` catalog.
//!
//! Planning is the pure catalog planner over one snapshot. A dry run only
//! plans and prints diffs. A real standalone run wraps the same plan in the
//! locked, pre-synced, guarded, committed transaction; nightly instead calls
//! [`sync_with_existing_lock`], which assumes the caller holds the
//! maintenance lock and runs no extra vault-sync cycles.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use similar::TextDiff;

use super::{
    model::{CatalogRecord, CatalogState},
    scan::{analyze, Analysis},
    snapshot::VaultSnapshot,
    txn::{
        self, apply_and_commit, verify_hosts_current, CommitInfo, PlannedTxn,
        ReadDependencies, TxnResult,
    },
};
use crate::native::{
    ob::{self, ChildEnv},
    style::Styler,
    vault_rewrite::TxnFailure,
};

pub(super) const SCHEMA_VERSION: u32 = 1;

/// One host edit as reported: path plus before/after bytes.
#[derive(Debug, Clone)]
pub(crate) struct ChangedFile {
    pub(crate) path: String,
    pub(crate) before: String,
    pub(crate) after: String,
}

impl ChangedFile {
    pub(super) fn diff(&self) -> String {
        unified_diff(&self.path, &self.before, &self.after)
    }
}

/// What a sync run planned and (unless a dry run) did.
#[derive(Debug)]
pub(crate) struct SyncReport {
    pub(crate) dry_run: bool,
    pub(crate) catalogs: Vec<CatalogRecord>,
    pub(crate) changes: Vec<ChangedFile>,
    pub(crate) commit: Option<CommitInfo>,
    pub(crate) sync_pre: Option<&'static str>,
    pub(crate) sync_post: Option<&'static str>,
    pub(crate) warnings: Vec<String>,
    pub(crate) failure: Option<TxnFailure>,
}

impl SyncReport {
    fn failed(dry_run: bool, failure: TxnFailure) -> Self {
        Self {
            dry_run,
            catalogs: Vec::new(),
            changes: Vec::new(),
            commit: None,
            sync_pre: None,
            sync_post: None,
            warnings: Vec::new(),
            failure: Some(failure),
        }
    }

    pub(crate) fn damaged(&self) -> Vec<&CatalogRecord> {
        self.catalogs
            .iter()
            .filter(|record| record.state == CatalogState::Damaged)
            .collect()
    }

    pub(crate) fn ok(&self) -> bool {
        self.failure.is_none() && self.damaged().is_empty()
    }

    pub(crate) fn exit_code(&self) -> i32 {
        i32::from(!self.ok())
    }

    pub(crate) fn to_json(&self) -> Value {
        let mut payload = json!({
            "schema_version": SCHEMA_VERSION,
            "ok": self.ok(),
            "command": "sync",
            "dry_run": self.dry_run,
            "changed": self.changes.iter().map(|change| change.path.clone()).collect::<Vec<_>>(),
            "changes": self.changes.iter().map(|change| {
                let mut entry = json!({"path": change.path, "action": "update"});
                if self.dry_run {
                    entry["diff"] = Value::String(change.diff());
                }
                entry
            }).collect::<Vec<_>>(),
            "catalogs": self.catalogs.iter().map(CatalogRecord::to_json).collect::<Vec<_>>(),
            "findings": findings_json(&self.catalogs),
            "commit": commit_json(self.commit.as_ref()),
            "sync": sync_json(self.sync_pre, self.sync_post),
            "warnings": self.warnings,
        });
        if let Some(failure) = &self.failure {
            payload["error"] = failure_json(failure);
        }
        payload
    }
}

pub(super) fn findings_json(catalogs: &[CatalogRecord]) -> Vec<Value> {
    catalogs
        .iter()
        .flat_map(|record| {
            record.issues.iter().map(move |issue| {
                json!({
                    "host": record.host,
                    "path": record.path,
                    "code": issue.code,
                    "severity": issue.severity.as_str(),
                    "line": issue.line,
                    "message": issue.message,
                })
            })
        })
        .collect()
}

pub(super) fn commit_json(commit: Option<&CommitInfo>) -> Value {
    match commit {
        Some(commit) => json!({"sha": commit.sha, "subject": commit.subject}),
        None => Value::Null,
    }
}

pub(super) fn sync_json(
    pre: Option<&'static str>,
    post: Option<&'static str>,
) -> Value {
    match (pre, post) {
        (Some(pre), Some(post)) => json!({"pre": pre, "post": post}),
        _ => Value::Null,
    }
}

pub(super) fn failure_json(failure: &TxnFailure) -> Value {
    json!({
        "code": failure.code,
        "message": failure.message,
        "hint": failure.hint,
    })
}

pub(super) fn unified_diff(path: &str, before: &str, after: &str) -> String {
    TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

/// Load the vault and refuse a partial view: an unreadable file must never
/// read as an empty subtree and delete catalogs.
pub(super) fn load_complete(vault: &Path) -> Result<VaultSnapshot, TxnFailure> {
    let snapshot = VaultSnapshot::load(vault);
    if let Some(first) = snapshot.io_errors.first() {
        let more = snapshot.io_errors.len() - 1;
        let suffix = if more > 0 {
            format!(" (+{more} more)")
        } else {
            String::new()
        };
        return Err(TxnFailure::new(
            "io",
            format!("the vault scan was incomplete: {first}{suffix}"),
            Some(
                "fix the unreadable paths, then retry; no catalog was written",
            ),
        ));
    }
    Ok(snapshot)
}

fn subject(changed: usize) -> String {
    format!("notebooks: sync catalogs ({changed} notes)")
}

/// Context carried through the transaction for reporting.
#[derive(Debug)]
struct SyncContext {
    catalogs: Vec<CatalogRecord>,
    changes: Vec<ChangedFile>,
}

fn changes_of(analysis: &Analysis) -> Vec<ChangedFile> {
    analysis
        .plan
        .edits()
        .into_iter()
        .map(|edit| ChangedFile {
            path: edit.path.to_string_lossy().replace('\\', "/"),
            before: edit.before.clone(),
            after: edit.after.clone(),
        })
        .collect()
}

fn planned_txn(
    vault: &Path,
    snapshot: &VaultSnapshot,
    analysis: &Analysis,
) -> PlannedTxn<SyncContext> {
    let edits = analysis.plan.edits();
    let hosts: Vec<PathBuf> =
        edits.iter().map(|edit| edit.path.clone()).collect();
    let files = crate::native::host_catalog::staged_rewrites(vault, edits);
    PlannedTxn {
        subject: subject(files.len()),
        files,
        appends: Vec::new(),
        deps: ReadDependencies::from_snapshot(snapshot),
        verify: verify_hosts_current(vault.to_path_buf(), hosts),
        context: SyncContext {
            catalogs: analysis.plan.records(),
            changes: changes_of(analysis),
        },
        kind: "notebook",
    }
}

/// Read-only plan: what a sync would change.
pub(crate) fn dry_run(vault: &Path) -> SyncReport {
    let snapshot = match load_complete(vault) {
        Ok(snapshot) => snapshot,
        Err(failure) => return SyncReport::failed(true, failure),
    };
    let analysis = analyze(&snapshot);
    SyncReport {
        dry_run: true,
        catalogs: analysis.plan.records(),
        changes: changes_of(&analysis),
        commit: None,
        sync_pre: None,
        sync_post: None,
        warnings: Vec::new(),
        failure: None,
    }
}

/// A committed run reports the state after the write: every changed host
/// is now current and its stale warning is resolved.
fn report_of(result: TxnResult<SyncContext>) -> SyncReport {
    let mut catalogs = result.context.catalogs;
    if result.commit.is_some() {
        for record in &mut catalogs {
            if result
                .context
                .changes
                .iter()
                .any(|change| change.path == record.path)
            {
                record.state = CatalogState::Current;
                record.issues.clear();
            }
        }
    }
    SyncReport {
        dry_run: false,
        catalogs,
        changes: result.context.changes,
        commit: result.commit,
        sync_pre: Some(result.sync_pre),
        sync_post: Some(result.sync_post),
        warnings: result.post_sync_warning.into_iter().collect(),
        failure: None,
    }
}

/// The lock-free core: plan from the current vault, guard, apply, and
/// commit. The caller holds the maintenance lock and owns any sync cycles.
pub(crate) fn sync_with_existing_lock(
    vault: &Path,
    child_env: &ChildEnv,
) -> SyncReport {
    let snapshot = match load_complete(vault) {
        Ok(snapshot) => snapshot,
        Err(failure) => return SyncReport::failed(false, failure),
    };
    let analysis = analyze(&snapshot);
    let planned = planned_txn(vault, &snapshot, &analysis);
    match apply_and_commit(vault, child_env, planned) {
        Ok((commit, context)) => report_of(TxnResult {
            commit,
            sync_pre: "skipped",
            sync_post: "skipped",
            post_sync_warning: None,
            context,
        }),
        Err(failure) => {
            let mut report = SyncReport::failed(false, failure);
            report.catalogs = analysis.plan.records();
            report
        }
    }
}

/// Standalone real run: identity, lock, pre-sync, plan, guarded write,
/// one scoped commit, post-sync.
pub(crate) fn run_standalone(vault: &Path, json_mode: bool) -> SyncReport {
    let child_env = ob::child_env();
    let result = txn::run_locked(vault, &child_env, json_mode, || {
        let snapshot = load_complete(vault)?;
        let analysis = analyze(&snapshot);
        Ok(planned_txn(vault, &snapshot, &analysis))
    });
    match result {
        Ok(result) => report_of(result),
        Err(failure) => SyncReport::failed(false, failure),
    }
}

// ---------------------------------------------------------------- output

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Human rendering for `bob notebooks sync` (and the nightly step).
pub(crate) fn print_human(report: &SyncReport) {
    let styler = Styler::detect();
    if let Some(failure) = &report.failure {
        eprintln!("{} {}", styler.red("✗"), failure.message);
        if let Some(hint) = &failure.hint {
            eprintln!("  {}", styler.dim(hint));
        }
    }
    for record in &report.catalogs {
        if record.state == CatalogState::Damaged {
            for issue in &record.issues {
                let line = issue
                    .line
                    .map(|line| format!(":{line}"))
                    .unwrap_or_default();
                println!(
                    "{} {}{line} [{}] {}",
                    styler.red("✗"),
                    record.path,
                    issue.code,
                    issue.message
                );
            }
        }
    }
    if report.dry_run {
        for change in &report.changes {
            print!("{}", color_diff(&change.diff(), &styler));
        }
        let changed = report.changes.len();
        if changed == 0 && report.failure.is_none() {
            println!("{} notebook catalogs are current", styler.green("✓"));
        } else if changed > 0 {
            println!(
                "{}",
                styler.yellow(&format!(
                    "dry run: {} would change; nothing was written",
                    plural(changed, "catalog", "catalogs")
                ))
            );
        }
        return;
    }
    if report.failure.is_some() {
        return;
    }
    match &report.commit {
        Some(commit) => {
            println!(
                "{} synced {} ({})",
                styler.green("✓"),
                plural(report.changes.len(), "catalog", "catalogs"),
                short(&commit.sha)
            );
            for change in &report.changes {
                println!("  {}", change.path);
            }
        }
        None if report.damaged().is_empty() => {
            println!("{} notebook catalogs are current", styler.green("✓"));
        }
        None => {}
    }
    for warning in &report.warnings {
        println!("{} {warning}", styler.yellow("!"));
    }
}

pub(super) fn short(sha: &str) -> &str {
    &sha[..sha.len().min(9)]
}

pub(super) fn color_diff(diff: &str, styler: &Styler) -> String {
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
            line.to_string()
        };
        out.push_str(&painted);
        out.push('\n');
    }
    out
}

/// Nightly step adapter: run the lock-free core once and print the
/// human summary. A damaged host or a failed write fails the step; a
/// current vault writes and commits nothing.
pub(crate) fn nightly_step(vault: &Path, child_env: &ChildEnv) -> i32 {
    let report = sync_with_existing_lock(vault, child_env);
    print_human(&report);
    report.exit_code()
}
