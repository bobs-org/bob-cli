//! `bob areas sync`: repair every generated `## Areas` catalog.
//!
//! Planning is the pure area-graph plus catalog planner over one snapshot.
//! A dry run only plans and prints diffs. A real standalone run wraps the
//! same plan in the locked, pre-synced, guarded, committed transaction;
//! nightly instead calls [`sync_with_existing_lock`], which assumes the
//! caller holds the maintenance lock and runs no extra vault-sync cycles.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{
    catalog::{plan_area_catalogs, CatalogPlan},
    graph::build_area_graph,
    report::CatalogRecord,
    slash, AreaIssue,
};
use crate::native::host_catalog::{
    apply_and_commit, CommitInfo, PlannedTxn, ReadDependencies, TxnResult,
};
use crate::native::{
    host_catalog::{color_diff, unified_diff},
    notebooks::snapshot::VaultSnapshot,
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
    pub(crate) findings: Vec<AreaIssue>,
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
            findings: Vec::new(),
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
            .filter(|record| {
                record.state == super::catalog::CatalogState::Damaged
            })
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
            "catalogs": self.catalogs.iter().map(record_json).collect::<Vec<_>>(),
            "findings": self.findings.iter().map(AreaIssue::to_json).collect::<Vec<_>>(),
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

fn record_json(record: &CatalogRecord) -> Value {
    json!({
        "host": record.host,
        "path": slash(&record.path),
        "kind": record.kind.as_str(),
        "state": record.state.as_str(),
        "rows": record.rows.iter().map(|row| json!({
            "path": slash(&row.path),
            "link": row.link,
            "depth": row.depth,
        })).collect::<Vec<_>>(),
        "issues": record.issues.iter().map(AreaIssue::to_json).collect::<Vec<_>>(),
    })
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
    let mut value = json!({
        "code": failure.code,
        "message": failure.message,
        "hint": failure.hint,
    });
    if let Some(kind) = failure.kind {
        value["kind"] = Value::String(kind.to_string());
    }
    value
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
    format!("areas: sync catalogs ({changed} notes)")
}

/// Context carried through the transaction for reporting.
#[derive(Debug)]
struct SyncContext {
    catalogs: Vec<CatalogRecord>,
    findings: Vec<AreaIssue>,
    changes: Vec<ChangedFile>,
}

fn catalog_records(plan: &CatalogPlan) -> Vec<CatalogRecord> {
    plan.hosts
        .iter()
        .map(|host| CatalogRecord {
            host: host.stem.clone(),
            path: host.path.clone(),
            kind: host.kind,
            state: host.state,
            rows: host.rows.clone(),
            issues: host.issues.clone(),
        })
        .collect()
}

fn merged_findings(
    snapshot: &VaultSnapshot,
    plan: &CatalogPlan,
) -> Vec<AreaIssue> {
    let graph = build_area_graph(snapshot);
    let mut findings = graph.findings.clone();
    for host in &plan.hosts {
        findings.extend(host.issues.iter().cloned());
    }
    findings.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.code.cmp(right.code))
    });
    findings
}

fn changes_of(plan: &CatalogPlan) -> Vec<ChangedFile> {
    plan.edits()
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
    plan: &CatalogPlan,
) -> PlannedTxn<SyncContext> {
    let edits = plan.edits();
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
            catalogs: catalog_records(plan),
            findings: merged_findings(snapshot, plan),
            changes: changes_of(plan),
        },
        kind: "area",
    }
}

/// Re-read the vault and report any affected host that is not `current`,
/// as a post-install verification. A same-stem note appearing mid-run can
/// flip a parent's resolution, so re-planning catches what preimage guards
/// alone would miss.
fn verify_hosts_current(
    vault: PathBuf,
    hosts: Vec<PathBuf>,
) -> Box<dyn FnOnce() -> Result<(), String>> {
    Box::new(move || {
        let snapshot = VaultSnapshot::load(&vault);
        if let Some(error) = snapshot.io_errors.first() {
            return Err(error.clone());
        }
        let graph = build_area_graph(&snapshot);
        let plan = plan_area_catalogs(&snapshot, &graph);
        for host in hosts {
            if let Some(planned) = plan.host(&host)
                && planned.edit.is_some()
            {
                let key = host.to_string_lossy().replace('\\', "/");
                return Err(format!(
                    "{key} is still not current after the write"
                ));
            }
        }
        Ok(())
    })
}

/// Read-only plan: what a sync would change.
pub(crate) fn dry_run(vault: &Path) -> SyncReport {
    let snapshot = match load_complete(vault) {
        Ok(snapshot) => snapshot,
        Err(failure) => return SyncReport::failed(true, failure),
    };
    let graph = build_area_graph(&snapshot);
    let plan = plan_area_catalogs(&snapshot, &graph);
    SyncReport {
        dry_run: true,
        catalogs: catalog_records(&plan),
        findings: merged_findings(&snapshot, &plan),
        changes: changes_of(&plan),
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
                .any(|change| change.path == slash(&record.path))
            {
                record.state = super::catalog::CatalogState::Current;
                record.issues.clear();
            }
        }
    }
    SyncReport {
        dry_run: false,
        catalogs,
        findings: result.context.findings,
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
    let graph = build_area_graph(&snapshot);
    let plan = plan_area_catalogs(&snapshot, &graph);
    let planned = planned_txn(vault, &snapshot, &plan);
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
            report.catalogs = catalog_records(&plan);
            report.findings = merged_findings(&snapshot, &plan);
            report
        }
    }
}

/// Standalone real run: identity, lock, pre-sync, plan, guarded write,
/// one scoped commit, post-sync.
pub(crate) fn run_standalone(vault: &Path, json_mode: bool) -> SyncReport {
    let child_env = ob::child_env();
    let result = crate::native::host_catalog::run_locked(
        vault,
        &child_env,
        json_mode,
        "area",
        || {
            let snapshot = load_complete(vault)?;
            let graph = build_area_graph(&snapshot);
            let plan = plan_area_catalogs(&snapshot, &graph);
            Ok(planned_txn(vault, &snapshot, &plan))
        },
    );
    match result {
        Ok(result) => report_of(result),
        Err(failure) => SyncReport::failed(false, failure),
    }
}

// ---------------------------------------------------------------- output

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Human rendering for `bob areas sync` (and the nightly step).
pub(crate) fn print_human(report: &SyncReport) {
    let styler = Styler::detect();
    if let Some(failure) = &report.failure {
        eprintln!("{} {}", styler.red("✗"), failure.message);
        if let Some(hint) = &failure.hint {
            eprintln!("  {}", styler.dim(hint));
        }
    }
    for record in &report.catalogs {
        if record.state == super::catalog::CatalogState::Damaged {
            for issue in &record.issues {
                let line = issue
                    .line
                    .map(|line| format!(":{line}"))
                    .unwrap_or_default();
                println!(
                    "{} {}{line} [{}] {}",
                    styler.red("✗"),
                    slash(&record.path),
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
            println!("{} area catalogs are current", styler.green("✓"));
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
            println!("{} area catalogs are current", styler.green("✓"));
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

/// Nightly step adapter: run the lock-free core once and print the
/// human summary. A damaged host or a failed write fails the step; a
/// current vault writes and commits nothing.
pub(crate) fn nightly_step(vault: &Path, child_env: &ChildEnv) -> i32 {
    let report = sync_with_existing_lock(vault, child_env);
    print_human(&report);
    report.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, name: &str, contents: &str) {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn area(parent: &str) -> String {
        format!(
            "---\ntype: \"[[area]]\"\nparent: \"[[{parent}]]\"\n---\n\n# Area\n"
        )
    }

    #[test]
    fn stale_preimage_is_refused_without_a_commit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = dir.path();
        write(vault, "area.md", "# Area root\n");
        write(vault, "cash.md", &area("area"));
        write(vault, "garden.md", &area("cash"));
        let snapshot = VaultSnapshot::load(vault);
        assert!(snapshot.io_errors.is_empty());
        let graph = build_area_graph(&snapshot);
        let plan = plan_area_catalogs(&snapshot, &graph);
        assert!(!plan.edits().is_empty(), "catalog write must be planned");

        // Mutate one planned host after planning but before install. The
        // guarded writer must refuse with a stale preimage and commit
        // nothing.
        let planned_path = plan.edits()[0].path.clone();
        let planned_abs = vault.join(&planned_path);
        let before = fs::read_to_string(&planned_abs).unwrap();
        fs::write(&planned_abs, format!("{before}\nBryan's note.\n")).unwrap();

        let planned = planned_txn(vault, &snapshot, &plan);
        let child_env = ob::child_env();
        let result = apply_and_commit(vault, &child_env, planned);
        let failure = result.expect_err("stale preimage must fail");
        assert!(
            failure.message.contains("changed on disk after planning"),
            "unexpected message: {}",
            failure.message
        );
    }
}
