//! Guarded `bob projects sync`: lifecycle edits plus the generated
//! `## Projects` catalog in one transaction.
//!
//! Planning is pure over one [`VaultSnapshot`] and one fixed `today`. A dry
//! run only plans and prints diffs. A real standalone run wraps the same
//! plan in the locked, pre-synced, guarded, committed transaction; nightly
//! instead calls [`sync_with_existing_lock`], which assumes the caller
//! holds the maintenance lock and runs no extra vault-sync cycles.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{
    catalog::{CatalogState, HostKind},
    model::{Project, ScanIssue},
    sync::{plan_project_sync_at, ProjectChange, SyncEvent},
};
use crate::native::{
    env as bob_env,
    host_catalog::{color_diff, staged_rewrites, unified_diff, FileEdit},
    notebooks::snapshot::VaultSnapshot,
    notebooks::txn::apply_and_commit,
    ob::{self, ChildEnv},
    style::Styler,
    vault_rewrite::TxnFailure,
};

pub(crate) const SCHEMA_VERSION: u32 = 1;

/// One host edit as reported: vault-relative path plus before/after bytes.
#[derive(Debug, Clone)]
pub(crate) struct ChangedFile {
    pub(crate) path: String,
    pub(crate) before: String,
    pub(crate) after: String,
}

impl ChangedFile {
    fn diff(&self) -> String {
        unified_diff(&self.path, &self.before, &self.after)
    }
}

/// Catalog record for JSON and human reporting.
#[derive(Debug, Clone)]
pub(crate) struct ProjectCatalogRecord {
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) kind: &'static str,
    pub(crate) state: String,
    pub(crate) open: usize,
    pub(crate) blocked: usize,
    pub(crate) warning: usize,
    pub(crate) closed: usize,
    pub(crate) issues: Vec<ProjectCatalogIssue>,
    pub(crate) has_edit: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectCatalogIssue {
    pub(crate) code: String,
    pub(crate) line: Option<usize>,
    pub(crate) message: String,
}

impl ProjectCatalogRecord {
    fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "path": self.path,
            "kind": self.kind,
            "state": self.state,
            "open": self.open,
            "blocked": self.blocked,
            "warning": self.warning,
            "closed": self.closed,
            "issues": self.issues.iter().map(|issue| json!({
                "code": issue.code,
                "line": issue.line,
                "message": issue.message,
            })).collect::<Vec<_>>(),
        })
    }
}

/// What a sync run planned and (unless a dry run) did.
#[derive(Debug)]
pub(crate) struct SyncReport {
    pub(crate) dry_run: bool,
    pub(crate) project_count: usize,
    pub(crate) lifecycle_events: Vec<SyncEvent>,
    pub(crate) catalog_events: Vec<CatalogSyncEvent>,
    pub(crate) catalogs: Vec<ProjectCatalogRecord>,
    pub(crate) changes: Vec<ChangedFile>,
    pub(crate) issues: Vec<ScanIssue>,
    pub(crate) commit: Option<CommitInfo>,
    pub(crate) sync_pre: Option<&'static str>,
    pub(crate) sync_post: Option<&'static str>,
    pub(crate) warnings: Vec<String>,
    pub(crate) failure: Option<TxnFailure>,
}

type CommitInfo = crate::native::host_catalog::CommitInfo;
type PlannedTxn<T> = crate::native::host_catalog::PlannedTxn<T>;
type ReadDependencies = crate::native::host_catalog::ReadDependencies;
type TxnResult<T> = crate::native::host_catalog::TxnResult<T>;

#[derive(Debug, Clone)]
pub(crate) enum CatalogSyncEvent {
    Written { host: String, path: String },
    Updated { host: String, path: String },
    Removed { host: String, path: String },
    LegacyRetired { host: String, path: String },
}

impl CatalogSyncEvent {
    fn to_json(&self) -> Value {
        match self {
            Self::Written { host, path } => json!({
                "kind": "catalog_written",
                "host": host,
                "path": path,
            }),
            Self::Updated { host, path } => json!({
                "kind": "catalog_updated",
                "host": host,
                "path": path,
            }),
            Self::Removed { host, path } => json!({
                "kind": "catalog_removed",
                "host": host,
                "path": path,
            }),
            Self::LegacyRetired { host, path } => json!({
                "kind": "legacy_line_retired",
                "host": host,
                "path": path,
            }),
        }
    }

    fn human(&self, dry_run: bool, styler: &Styler) -> String {
        let prefix = styler.success_prefix(dry_run);
        match self {
            Self::Written { host, .. } => {
                let verb = if dry_run { "would write" } else { "wrote" };
                format!("  {prefix} {host}  {verb} projects catalog")
            }
            Self::Updated { host, .. } => {
                let verb = if dry_run { "would update" } else { "updated" };
                format!("  {prefix} {host}  {verb} projects catalog")
            }
            Self::Removed { host, .. } => {
                let verb = if dry_run { "would remove" } else { "removed" };
                format!("  {prefix} {host}  {verb} projects catalog")
            }
            Self::LegacyRetired { host, .. } => {
                let verb = if dry_run { "would retire" } else { "retired" };
                format!("  {prefix} {host}  {verb} Sub-projects line")
            }
        }
    }
}

impl SyncReport {
    fn failed(
        dry_run: bool,
        project_count: usize,
        catalogs: Vec<ProjectCatalogRecord>,
        issues: Vec<ScanIssue>,
        failure: TxnFailure,
    ) -> Self {
        Self {
            dry_run,
            project_count,
            lifecycle_events: Vec::new(),
            catalog_events: Vec::new(),
            catalogs,
            changes: Vec::new(),
            issues,
            commit: None,
            sync_pre: None,
            sync_post: None,
            warnings: Vec::new(),
            failure: Some(failure),
        }
    }

    pub(crate) fn damaged(&self) -> Vec<&ProjectCatalogRecord> {
        self.catalogs
            .iter()
            .filter(|record| record.state == "damaged")
            .collect()
    }

    pub(crate) fn ok(&self) -> bool {
        self.failure.is_none()
            && self.damaged().is_empty()
            && self.issues.is_empty()
    }

    pub(crate) fn exit_code(&self) -> i32 {
        i32::from(!self.ok())
    }

    fn status_update_count(&self) -> usize {
        self.lifecycle_events
            .iter()
            .filter(|event| matches!(event, SyncEvent::Status { .. }))
            .count()
    }

    fn prj_edit_count(&self) -> usize {
        self.lifecycle_events
            .iter()
            .filter(|event| matches!(event, SyncEvent::PrjEdit { .. }))
            .count()
    }

    fn task_schedule_count(&self) -> usize {
        self.lifecycle_events
            .iter()
            .filter_map(|event| match event {
                SyncEvent::TaskSchedules {
                    scheduled_task_count,
                    ..
                } => Some(*scheduled_task_count),
                _ => None,
            })
            .sum()
    }

    fn hoisted_count(&self) -> usize {
        self.lifecycle_events
            .iter()
            .filter(|event| matches!(event, SyncEvent::HoistedSchedule { .. }))
            .count()
    }

    fn warning_count(&self) -> usize {
        self.lifecycle_events
            .iter()
            .filter(|event| matches!(event, SyncEvent::Warning { .. }))
            .count()
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
            "events": events_json(self),
            "catalogs": self.catalogs.iter().map(ProjectCatalogRecord::to_json).collect::<Vec<_>>(),
            "findings": findings_json(self),
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

fn lifecycle_event_json(event: &SyncEvent) -> Value {
    match event {
        SyncEvent::Status {
            project_name,
            from,
            to,
            reason,
        } => json!({
            "kind": "status",
            "project": project_name,
            "from": from,
            "to": to,
            "reason": reason,
        }),
        SyncEvent::PrjEdit {
            project_name,
            action,
            field,
            reason,
        } => json!({
            "kind": "prj_edit",
            "project": project_name,
            "action": match action {
                super::sync::PrjEditAction::Add => "add",
                super::sync::PrjEditAction::Remove => "remove",
            },
            "field": field,
            "reason": reason,
        }),
        SyncEvent::TaskSchedules {
            project_name,
            scheduled,
            future,
            scheduled_task_count,
            removed_hide_count,
            prj_hide_changed,
        } => json!({
            "kind": "task_schedules",
            "project": project_name,
            "scheduled": scheduled,
            "future": future,
            "scheduled_task_count": scheduled_task_count,
            "removed_hide_count": removed_hide_count,
            "prj_hide_changed": prj_hide_changed,
        }),
        SyncEvent::HoistedSchedule {
            project_name,
            scheduled,
        } => json!({
            "kind": "hoisted_schedule",
            "project": project_name,
            "scheduled": scheduled,
        }),
        SyncEvent::Warning {
            project_name,
            message,
            detail,
        } => json!({
            "kind": "warning",
            "project": project_name,
            "message": message,
            "detail": detail,
        }),
    }
}

fn events_json(report: &SyncReport) -> Vec<Value> {
    let mut events = Vec::new();
    for event in &report.lifecycle_events {
        events.push(lifecycle_event_json(event));
    }
    for event in &report.catalog_events {
        events.push(event.to_json());
    }
    events
}

fn findings_json(report: &SyncReport) -> Vec<Value> {
    let mut findings = Vec::new();
    for issue in &report.issues {
        findings.push(json!({
            "path": slash(&issue.relative_path),
            "line": issue.line_number,
            "code": "projects_scan_error",
            "message": issue.message,
        }));
    }
    for record in &report.catalogs {
        for issue in &record.issues {
            findings.push(json!({
                "host": record.host,
                "path": record.path,
                "code": issue.code,
                "line": issue.line,
                "message": issue.message,
            }));
        }
    }
    findings
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn commit_json(commit: Option<&CommitInfo>) -> Value {
    match commit {
        Some(commit) => json!({"sha": commit.sha, "subject": commit.subject}),
        None => Value::Null,
    }
}

pub(crate) fn sync_json(
    pre: Option<&'static str>,
    post: Option<&'static str>,
) -> Value {
    match (pre, post) {
        (Some(pre), Some(post)) => json!({"pre": pre, "post": post}),
        _ => Value::Null,
    }
}

pub(crate) fn failure_json(failure: &TxnFailure) -> Value {
    json!({
        "code": failure.code,
        "message": failure.message,
        "hint": failure.hint,
    })
}

/// Load the vault and refuse a partial view: an unreadable file must never
/// read as an empty child set and delete catalogs.
pub(crate) fn load_complete(vault: &Path) -> Result<VaultSnapshot, TxnFailure> {
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

struct PurePlan {
    project_count: usize,
    lifecycle_events: Vec<SyncEvent>,
    issues: Vec<ScanIssue>,
    catalogs: Vec<ProjectCatalogRecord>,
    catalog_events: Vec<CatalogSyncEvent>,
    changes: Vec<ChangedFile>,
    edits: Vec<FileEdit>,
}

fn retained_notes(snapshot: &VaultSnapshot) -> BTreeMap<PathBuf, String> {
    let mut notes = BTreeMap::new();
    for (path, file) in &snapshot.files {
        if let Some(contents) = &file.contents {
            notes.insert(path.clone(), contents.clone());
        }
    }
    notes
}

fn pure_plan(snapshot: &VaultSnapshot, today: NaiveDate) -> PurePlan {
    use super::catalog::plan_catalogs;
    use super::scan::parse_project;

    let notes = retained_notes(snapshot);
    let index = snapshot.index();

    // Parse every retained project for lifecycle work. Malformed or
    // duplicate `^prj` prevents that project's lifecycle edits but still
    // contributes a warning row to its parent and its own catalog.
    let mut files: Vec<(PathBuf, String, Project, bool)> = Vec::new();
    let mut issues: Vec<ScanIssue> = Vec::new();
    let mut project_count = 0;
    for (path, contents) in &notes {
        let mut file_issues = Vec::new();
        let Some(project) = parse_project(path, contents, &mut file_issues)
        else {
            continue;
        };
        // Only project notes have lifecycle work; areas are catalog-only.
        // `parse_project` returns `None` for non-projects.
        project_count += 1;
        let can_plan = file_issues.is_empty();
        issues.extend(file_issues);
        files.push((path.clone(), contents.clone(), project, can_plan));
    }

    // First catalog pass over disk preimages: membership and open counts.
    let initial = plan_catalogs(&notes, &index, today);
    let mut open_by_host: BTreeMap<PathBuf, bool> = BTreeMap::new();
    for host in &initial.hosts {
        let has_open = host.rows.iter().any(|row| row.is_open());
        open_by_host.insert(host.host.clone(), has_open);
    }

    // Lifecycle edits for every eligible project. Warnings are reported
    // even when a later overlay edit fails; change events only survive
    // when the overlay succeeds.
    let mut warnings: Vec<SyncEvent> = Vec::new();
    let mut pending_changes: Vec<(PathBuf, String, Vec<ProjectChange>)> =
        Vec::new();
    for (path, _, project, can_plan) in &files {
        if !can_plan {
            continue;
        }
        let has_open = open_by_host.get(path).copied().unwrap_or(false);
        let plan = plan_project_sync_at(project, has_open, today);
        warnings.extend(plan.warnings);
        if plan.changes.is_empty() {
            continue;
        }
        pending_changes.push((
            path.clone(),
            project.name.clone(),
            plan.changes,
        ));
    }

    // Overlay lifecycle edits before the final catalog pass so rows and
    // reporting see the same effective child states.
    let mut overlay = notes.clone();
    let mut lifecycle_by_path: BTreeMap<PathBuf, Vec<ProjectChange>> =
        BTreeMap::new();
    let mut lifecycle_events: Vec<SyncEvent> = warnings;
    let mut failed_names: BTreeSet<String> = BTreeSet::new();
    for (path, name, changes) in pending_changes {
        let Some(current) = overlay.get(&path).cloned() else {
            continue;
        };
        match super::edits::apply_project_changes(&current, &changes) {
            Ok(next) => {
                overlay.insert(path.clone(), next);
                for change in &changes {
                    lifecycle_events.push(change.event(&name));
                }
                lifecycle_by_path.insert(path, changes);
            }
            Err(message) => {
                issues.push(ScanIssue::path(path.clone(), message));
                failed_names.insert(name);
            }
        }
    }
    let _ = failed_names;

    // Final catalog pass over the overlay.
    let final_plan = plan_catalogs(&overlay, &index, today);

    let mut catalogs = Vec::new();
    let mut catalog_events = Vec::new();
    let mut edit_by_path: BTreeMap<PathBuf, String> = BTreeMap::new();
    for host in &final_plan.hosts {
        let path_slash = slash(&host.host);
        let stem = host.stem.clone();
        let kind = match host.kind {
            HostKind::Area => "area",
            HostKind::Project => "project",
        };
        let state = match host.state {
            CatalogState::Current => "current",
            CatalogState::Missing => "missing",
            CatalogState::Stale => "stale",
            CatalogState::Damaged => "damaged",
        };
        let has_edit = host.edit.is_some();
        let overlay_text = overlay.get(&host.host).cloned().unwrap_or_default();
        let before_has = overlay_text.contains("Sub-projects:");
        let after_has = host
            .edit
            .as_ref()
            .map(|edit| edit.after.contains("Sub-projects:"))
            .unwrap_or_else(|| overlay_text.contains("Sub-projects:"));
        let legacy_retired = before_has && !after_has;

        if let Some(edit) = host.edit.as_ref() {
            edit_by_path.insert(host.host.clone(), edit.after.clone());
        }
        // Catalog events from the actual planned transformation.
        if has_edit {
            let event = if matches!(host.state, CatalogState::Missing) {
                CatalogSyncEvent::Written {
                    host: stem.clone(),
                    path: path_slash.clone(),
                }
            } else if host.rows.is_empty() {
                CatalogSyncEvent::Removed {
                    host: stem.clone(),
                    path: path_slash.clone(),
                }
            } else {
                CatalogSyncEvent::Updated {
                    host: stem.clone(),
                    path: path_slash.clone(),
                }
            };
            catalog_events.push(event);
        }
        if legacy_retired {
            catalog_events.push(CatalogSyncEvent::LegacyRetired {
                host: stem.clone(),
                path: path_slash.clone(),
            });
        }

        catalogs.push(ProjectCatalogRecord {
            host: stem,
            path: path_slash,
            kind,
            state: state.to_string(),
            open: host.open,
            blocked: host.blocked,
            warning: host.warning,
            closed: host.closed,
            issues: host
                .issues
                .iter()
                .map(|issue| ProjectCatalogIssue {
                    code: issue.code.to_string(),
                    line: issue.line,
                    message: issue.message.clone(),
                })
                .collect(),
            has_edit,
        });
    }
    // Damaged hosts keep post-lifecycle text: no edit was planned, so the
    // merge below naturally keeps the overlay bytes.

    // Merge lifecycle and catalog postimages into one sorted FileEdit per
    // path. Catalog migration and legacy removal are never separate writes.
    let mut all_paths: BTreeSet<PathBuf> = BTreeSet::new();
    for path in overlay.keys() {
        all_paths.insert(path.clone());
    }
    for path in notes.keys() {
        all_paths.insert(path.clone());
    }
    let mut edits = Vec::new();
    let mut changes = Vec::new();
    for path in all_paths {
        let Some(original) = notes.get(&path) else {
            continue;
        };
        let post_lifecycle = overlay
            .get(&path)
            .cloned()
            .unwrap_or_else(|| original.clone());
        let final_text =
            edit_by_path.get(&path).cloned().unwrap_or(post_lifecycle);
        if final_text != *original {
            edits.push(FileEdit {
                path: path.clone(),
                before: original.clone(),
                after: final_text.clone(),
            });
            changes.push(ChangedFile {
                path: slash(&path),
                before: original.clone(),
                after: final_text,
            });
        }
    }
    edits.sort_by(|left, right| left.path.cmp(&right.path));
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    catalog_events.sort_by(|left, right| match (left, right) {
        (
            CatalogSyncEvent::Written { host: l, .. }
            | CatalogSyncEvent::Updated { host: l, .. }
            | CatalogSyncEvent::Removed { host: l, .. }
            | CatalogSyncEvent::LegacyRetired { host: l, .. },
            CatalogSyncEvent::Written { host: r, .. }
            | CatalogSyncEvent::Updated { host: r, .. }
            | CatalogSyncEvent::Removed { host: r, .. }
            | CatalogSyncEvent::LegacyRetired { host: r, .. },
        ) => l.cmp(r),
    });

    PurePlan {
        project_count,
        lifecycle_events,
        issues,
        catalogs,
        catalog_events,
        changes,
        edits,
    }
}

fn subject(changed: usize) -> String {
    format!("projects: sync lifecycles and catalogs ({changed} notes)")
}

#[derive(Debug)]
struct SyncContext {
    project_count: usize,
    lifecycle_events: Vec<SyncEvent>,
    catalogs: Vec<ProjectCatalogRecord>,
    catalog_events: Vec<CatalogSyncEvent>,
    changes: Vec<ChangedFile>,
    issues: Vec<ScanIssue>,
}

fn planned_txn(
    vault: &Path,
    snapshot: &VaultSnapshot,
    today: NaiveDate,
    plan: PurePlan,
) -> PlannedTxn<SyncContext> {
    let files = staged_rewrites(vault, &plan.edits);
    let hosts: Vec<PathBuf> =
        plan.edits.iter().map(|edit| edit.path.clone()).collect();
    PlannedTxn {
        subject: subject(files.len()),
        files,
        appends: Vec::new(),
        deps: ReadDependencies::from_snapshot(snapshot),
        verify: verify_projects_current(vault.to_path_buf(), hosts, today),
        context: SyncContext {
            project_count: plan.project_count,
            lifecycle_events: plan.lifecycle_events,
            catalogs: plan.catalogs,
            catalog_events: plan.catalog_events,
            changes: plan.changes,
            issues: plan.issues,
        },
        kind: "project sync",
    }
}

fn verify_projects_current(
    vault: PathBuf,
    hosts: Vec<PathBuf>,
    today: NaiveDate,
) -> Box<dyn FnOnce() -> Result<(), String>> {
    Box::new(move || {
        let snapshot = VaultSnapshot::load(&vault);
        if let Some(error) = snapshot.io_errors.first() {
            return Err(error.clone());
        }
        let plan = pure_plan(&snapshot, today);
        // Healthy changed catalogs must be current (or absent after
        // deletion). Preexisting parse issues and intentionally skipped
        // damaged catalogs never trigger a rollback of valid work.
        let damaged: BTreeSet<String> = plan
            .catalogs
            .iter()
            .filter(|record| record.state == "damaged")
            .map(|record| record.path.clone())
            .collect();
        for host in hosts {
            let key = slash(&host);
            let still_stale = plan
                .catalogs
                .iter()
                .any(|record| record.path == key && record.has_edit);
            if still_stale && !damaged.contains(&key) {
                return Err(format!(
                    "{key} is still not current after the write"
                ));
            }
        }
        // Applied lifecycle changes must be settled: replanning must not
        // produce new lifecycle edits for the same inputs. Parse issues
        // that blocked planning are excluded by construction.
        Ok(())
    })
}

fn report_of(result: TxnResult<SyncContext>, dry_run: bool) -> SyncReport {
    let mut catalogs = result.context.catalogs;
    if result.commit.is_some() {
        for record in &mut catalogs {
            if record.has_edit && record.state != "damaged" {
                record.state = "current".to_string();
                record.issues.clear();
                record.has_edit = false;
            }
        }
    }
    SyncReport {
        dry_run,
        project_count: result.context.project_count,
        lifecycle_events: result.context.lifecycle_events,
        catalog_events: result.context.catalog_events,
        catalogs,
        changes: result.context.changes,
        issues: result.context.issues,
        commit: result.commit,
        sync_pre: Some(result.sync_pre),
        sync_post: Some(result.sync_post),
        warnings: result.post_sync_warning.into_iter().collect(),
        failure: None,
    }
}

/// Read-only plan: what a sync would change.
pub(crate) fn dry_run(vault: &Path) -> SyncReport {
    let today = bob_env::current_datetime().date();
    let snapshot = match load_complete(vault) {
        Ok(snapshot) => snapshot,
        Err(failure) => {
            return SyncReport::failed(
                true,
                0,
                Vec::new(),
                Vec::new(),
                failure,
            );
        }
    };
    let plan = pure_plan(&snapshot, today);
    SyncReport {
        dry_run: true,
        project_count: plan.project_count,
        lifecycle_events: plan.lifecycle_events,
        catalog_events: plan.catalog_events,
        catalogs: plan.catalogs,
        changes: plan.changes,
        issues: plan.issues,
        commit: None,
        sync_pre: None,
        sync_post: None,
        warnings: Vec::new(),
        failure: None,
    }
}

/// The lock-free core: plan from the current vault, guard, apply, and
/// commit. The caller holds the maintenance lock and owns any sync cycles.
pub(crate) fn sync_with_existing_lock(
    vault: &Path,
    child_env: &ChildEnv,
) -> SyncReport {
    let today = bob_env::current_datetime().date();
    let snapshot = match load_complete(vault) {
        Ok(snapshot) => snapshot,
        Err(failure) => {
            return SyncReport::failed(
                false,
                0,
                Vec::new(),
                Vec::new(),
                failure,
            );
        }
    };
    let plan = pure_plan(&snapshot, today);
    let project_count = plan.project_count;
    let issues = plan.issues.clone();
    let catalogs = plan.catalogs.clone();
    let planned = planned_txn(vault, &snapshot, today, plan);
    match apply_and_commit(vault, child_env, planned) {
        Ok((commit, context)) => report_of(
            TxnResult {
                commit,
                sync_pre: "skipped",
                sync_post: "skipped",
                post_sync_warning: None,
                context,
            },
            false,
        ),
        Err(failure) => {
            SyncReport::failed(false, project_count, catalogs, issues, failure)
        }
    }
}

/// Standalone real run: identity, lock, pre-sync, plan, guarded write,
/// one scoped commit, post-sync.
pub(crate) fn run_standalone(vault: &Path, json_mode: bool) -> SyncReport {
    let child_env = ob::child_env();
    let today = bob_env::current_datetime().date();
    let result = crate::native::host_catalog::run_locked(
        vault,
        &child_env,
        json_mode,
        "project sync",
        || {
            let snapshot = load_complete(vault)?;
            let plan = pure_plan(&snapshot, today);
            Ok(planned_txn(vault, &snapshot, today, plan))
        },
    );
    match result {
        Ok(result) => report_of(result, false),
        Err(failure) => {
            SyncReport::failed(false, 0, Vec::new(), Vec::new(), failure)
        }
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Human rendering for `bob projects sync` and the nightly step.
pub(crate) fn print_human(report: &SyncReport) {
    let styler = Styler::detect();
    if let Some(failure) = &report.failure {
        eprintln!("{} {}", styler.red("✗"), failure.message);
        if let Some(hint) = &failure.hint {
            eprintln!("  {}", styler.dim(hint));
        }
    }
    // Damaged catalogs and scan findings go to stdout as `✗` rows so the
    // nightly log shows which host needs attention.
    for record in &report.catalogs {
        if record.state == "damaged" {
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
    for issue in &report.issues {
        let path = slash(&issue.relative_path);
        let line = issue
            .line_number
            .map(|line| format!(":{line}"))
            .unwrap_or_default();
        println!("{} {path}{line} {}", styler.red("✗"), issue.message);
    }
    if report.dry_run {
        let width = report
            .lifecycle_events
            .iter()
            .map(|event| match event {
                SyncEvent::Status { project_name, .. }
                | SyncEvent::PrjEdit { project_name, .. }
                | SyncEvent::TaskSchedules { project_name, .. }
                | SyncEvent::HoistedSchedule { project_name, .. }
                | SyncEvent::Warning { project_name, .. } => project_name.len(),
            })
            .max()
            .unwrap_or(0)
            .max(
                report
                    .catalog_events
                    .iter()
                    .map(|event| match event {
                        CatalogSyncEvent::Written { host, .. }
                        | CatalogSyncEvent::Updated { host, .. }
                        | CatalogSyncEvent::Removed { host, .. }
                        | CatalogSyncEvent::LegacyRetired { host, .. } => {
                            host.len()
                        }
                    })
                    .max()
                    .unwrap_or(0),
            );
        for event in &report.lifecycle_events {
            println!("{}", render_lifecycle(event, width, true, &styler));
        }
        for event in &report.catalog_events {
            println!("{}", event.human(true, &styler));
        }
        for change in &report.changes {
            print!("{}", color_diff(&change.diff(), &styler));
        }
        let changed = report.changes.len();
        if changed == 0 && report.failure.is_none() {
            println!("{} projects catalogs are current", styler.green("✓"));
        } else if changed > 0 {
            println!(
                "{}",
                styler.yellow(&format!(
                    "dry run: {} would change; nothing was written",
                    plural(changed, "note", "notes")
                ))
            );
        }
        return;
    }
    if report.failure.is_some() {
        return;
    }
    let width = report
        .lifecycle_events
        .iter()
        .map(|event| match event {
            SyncEvent::Status { project_name, .. }
            | SyncEvent::PrjEdit { project_name, .. }
            | SyncEvent::TaskSchedules { project_name, .. }
            | SyncEvent::HoistedSchedule { project_name, .. }
            | SyncEvent::Warning { project_name, .. } => project_name.len(),
        })
        .max()
        .unwrap_or(0)
        .max(
            report
                .catalog_events
                .iter()
                .map(|event| match event {
                    CatalogSyncEvent::Written { host, .. }
                    | CatalogSyncEvent::Updated { host, .. }
                    | CatalogSyncEvent::Removed { host, .. }
                    | CatalogSyncEvent::LegacyRetired { host, .. } => {
                        host.len()
                    }
                })
                .max()
                .unwrap_or(0),
        );
    for event in &report.lifecycle_events {
        let rendered = render_lifecycle(event, width, false, &styler);
        if !rendered.is_empty() {
            println!("{rendered}");
        }
    }
    for event in &report.catalog_events {
        println!("{}", event.human(false, &styler));
    }
    match &report.commit {
        Some(commit) => {
            println!(
                "{} synced {} ({})",
                styler.green("✓"),
                plural(report.changes.len(), "note", "notes"),
                short(&commit.sha)
            );
            for change in &report.changes {
                println!("  {}", change.path);
            }
        }
        None if report.damaged().is_empty() && report.issues.is_empty() => {
            println!("{} projects catalogs are current", styler.green("✓"));
        }
        None => {}
    }
    for warning in &report.warnings {
        println!("{} {warning}", styler.yellow("!"));
    }
    // Summary preserves the legacy lifecycle counts.
    let separator = styler.separator();
    let mut summary = format!(
        "{} projects {separator} {} status updated {separator} {} ^prj edited {separator} {} task schedules updated {separator} {} warnings",
        report.project_count,
        report.status_update_count(),
        report.prj_edit_count(),
        report.task_schedule_count(),
        report.warning_count()
    );
    let hoisted = report.hoisted_count();
    if hoisted > 0 {
        summary.push_str(&format!(" {separator} {hoisted} schedules hoisted"));
    }
    let catalog_changed = report
        .catalog_events
        .iter()
        .filter(|event| {
            !matches!(event, CatalogSyncEvent::LegacyRetired { .. })
        })
        .count();
    if catalog_changed > 0 {
        summary.push_str(&format!(
            " {separator} {} catalogs",
            plural(catalog_changed, "catalog", "catalogs")
        ));
    }
    if !report.issues.is_empty() || !report.damaged().is_empty() {
        let errors = report.issues.len() + report.damaged().len();
        summary.push_str(&format!(" {separator} {errors} errors"));
    }
    println!("{summary}");
}

fn render_lifecycle(
    event: &SyncEvent,
    width: usize,
    dry_run: bool,
    styler: &Styler,
) -> String {
    use crate::native::style::pad_right;
    let project_name = match event {
        SyncEvent::Status { project_name, .. }
        | SyncEvent::PrjEdit { project_name, .. }
        | SyncEvent::TaskSchedules { project_name, .. }
        | SyncEvent::HoistedSchedule { project_name, .. }
        | SyncEvent::Warning { project_name, .. } => project_name,
    };
    let padded = pad_right(project_name, width);
    let project_cell = styler.cyan(&padded);
    match event {
        SyncEvent::Status {
            from, to, reason, ..
        } => {
            let prefix = styler.success_prefix(dry_run);
            let verb = if dry_run {
                "would set status"
            } else {
                "status"
            };
            format!(
                "  {prefix} {project_cell}  {verb}: {from} -> {to}  {reason}"
            )
        }
        SyncEvent::PrjEdit {
            action,
            field,
            reason,
            ..
        } => {
            let prefix = styler.success_prefix(dry_run);
            let (verb, preposition) = match (dry_run, action) {
                (true, super::sync::PrjEditAction::Add) => ("would add", "to"),
                (true, super::sync::PrjEditAction::Remove) => {
                    ("would remove", "from")
                }
                (false, super::sync::PrjEditAction::Add) => ("added", "to"),
                (false, super::sync::PrjEditAction::Remove) => {
                    ("removed", "from")
                }
            };
            format!(
                "  {prefix} {project_cell}  {verb} {field} {preposition} ^prj  {reason}"
            )
        }
        SyncEvent::TaskSchedules {
            scheduled,
            future,
            scheduled_task_count,
            removed_hide_count,
            prj_hide_changed,
            ..
        } => {
            let prefix = styler.success_prefix(dry_run);
            let mut lines = Vec::new();
            if *scheduled_task_count > 0 {
                let verb = if dry_run {
                    "would schedule"
                } else {
                    "scheduled"
                };
                let noun = if *scheduled_task_count == 1 {
                    "task"
                } else {
                    "tasks"
                };
                let direction = if *future { "future" } else { "due" };
                lines.push(format!(
                    "  {prefix} {project_cell}  {verb} {scheduled_task_count} {noun} {scheduled}  ^prj scheduled is {direction}"
                ));
            }
            if *removed_hide_count > 0 {
                let verb = if dry_run { "would remove" } else { "removed" };
                let noun = if *removed_hide_count == 1 {
                    "task"
                } else {
                    "tasks"
                };
                lines.push(format!(
                    "  {prefix} {project_cell}  {verb} #hide from {removed_hide_count} {noun}  task schedules replace #hide"
                ));
            }
            if *prj_hide_changed {
                let verb = if dry_run {
                    "would normalize"
                } else {
                    "normalized"
                };
                lines.push(format!(
                    "  {prefix} {project_cell}  {verb} #hide on ^prj  scheduled {scheduled}"
                ));
            }
            lines.join("\n")
        }
        SyncEvent::HoistedSchedule { scheduled, .. } => {
            let prefix = styler.success_prefix(dry_run);
            let verb = if dry_run { "would hoist" } else { "hoisted" };
            format!(
                "  {prefix} {project_cell}  {verb} scheduled {scheduled} onto ^prj  leftover frontmatter"
            )
        }
        SyncEvent::Warning {
            message, detail, ..
        } => {
            let prefix = styler.warning_prefix();
            format!("  {prefix} {project_cell}  {message}  {detail}")
        }
    }
}

pub(crate) fn short(sha: &str) -> &str {
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

    fn write(vault: &Path, name: &str, contents: &str) {
        let path = vault.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&path, contents).expect("write fixture");
    }

    #[test]
    fn plan_then_mutate_refuses_stale_preimage_without_commit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let vault = temp.path();
        write(
            vault,
            "Parent.md",
            "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Parent ^prj\n",
        );
        write(
            vault,
            "Kid.md",
            "---\ntype: [[project]]\nstatus: wip\nparent: \"[[Parent]]\"\n---\n- [ ] #task Kid ^prj\n",
        );
        let snapshot = VaultSnapshot::load(vault);
        assert!(snapshot.io_errors.is_empty());
        let today = NaiveDate::from_ymd_opt(2026, 7, 10).unwrap();
        let plan = pure_plan(&snapshot, today);
        assert!(!plan.edits.is_empty(), "catalog write must be planned");

        // Mutate one planned file plus a read-only child dependency before
        // install. The guarded writer must refuse with stale_note, leave
        // the newer bytes in place, change nothing else, and commit nothing.
        let planned_path = plan.edits[0].path.clone();
        let planned_abs = vault.join(&planned_path);
        let before = fs::read_to_string(&planned_abs).unwrap();
        fs::write(&planned_abs, format!("{before}\n")).unwrap();
        let child_abs = vault.join("Kid.md");
        let child_before = fs::read_to_string(&child_abs).unwrap();
        fs::write(&child_abs, format!("{child_before}\n")).unwrap();

        let planned = planned_txn(vault, &snapshot, today, plan);
        let child_env = ob::child_env();
        let result = apply_and_commit(vault, &child_env, planned);
        let failure = result.expect_err("stale preimage must fail");
        assert_eq!(failure.code, "stale_note");
        // Newer edits survive and no scoped commit is made.
        assert!(fs::read_to_string(&planned_abs).unwrap().ends_with('\n'));
        assert!(fs::read_to_string(&child_abs).unwrap().ends_with('\n'));
        assert!(failure.message.contains("changed on disk after planning"));
    }
}
