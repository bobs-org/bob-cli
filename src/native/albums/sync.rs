//! `bob albums sync`: rebuild stale records indexes and owner catalogs
//! through one guarded host-catalog transaction.
//!
//! Planning composes the album-index projection with the owner-catalog
//! planner from one snapshot. Standalone execution uses the shared
//! lock/pre-sync/commit/post-sync lifecycle; nightly calls the lock-free
//! core while its outer runner owns those steps.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde_json::{json, Value};
use similar::TextDiff;

use super::{
    catalog::{self, AlbumCatalogRecord, CatalogState},
    index::IndexState,
    registry::{AlbumRegistry, DiscoveryHealth},
    report::{self, AlbumIssue, AlbumReportEntry},
};
use crate::native::{
    config,
    host_catalog::{
        self, apply_and_commit, CommitInfo, FileEdit, PlannedTxn,
        ReadDependencies, TxnResult,
    },
    notebooks::snapshot::VaultSnapshot,
    ob::{self, ChildEnv},
    records::RecordCatalog,
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

/// Shared album-index plan exported for the owner-catalog phase. It carries
/// the same edits and input guards used by standalone and nightly sync.
#[derive(Debug, Clone)]
pub(crate) struct IndexSyncPlan {
    pub(crate) albums: Vec<AlbumReportEntry>,
    pub(crate) edits: Vec<FileEdit>,
    pub(crate) dependencies: ReadDependencies,
    pub(crate) io_errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CombinedSyncPlan {
    pub(crate) albums: Vec<AlbumReportEntry>,
    pub(crate) catalogs: Vec<AlbumCatalogRecord>,
    pub(crate) changes: Vec<ChangedFile>,
    pub(crate) edits: Vec<FileEdit>,
    pub(crate) dependencies: ReadDependencies,
    pub(crate) io_errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct SyncContext {
    pub(crate) albums: Vec<AlbumReportEntry>,
    pub(crate) catalogs: Vec<AlbumCatalogRecord>,
    pub(crate) changes: Vec<ChangedFile>,
    pub(crate) io_errors: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct SyncReport {
    pub(crate) dry_run: bool,
    pub(crate) albums: Vec<AlbumReportEntry>,
    pub(crate) catalogs: Vec<AlbumCatalogRecord>,
    pub(crate) changes: Vec<ChangedFile>,
    pub(crate) commit: Option<CommitInfo>,
    pub(crate) sync_pre: Option<&'static str>,
    pub(crate) sync_post: Option<&'static str>,
    pub(crate) warnings: Vec<String>,
    pub(crate) io_errors: Vec<String>,
    pub(crate) failure: Option<TxnFailure>,
}

impl SyncReport {
    fn failed(dry_run: bool, failure: TxnFailure) -> Self {
        Self {
            dry_run,
            albums: Vec::new(),
            catalogs: Vec::new(),
            changes: Vec::new(),
            commit: None,
            sync_pre: None,
            sync_post: None,
            warnings: Vec::new(),
            io_errors: Vec::new(),
            failure: Some(failure),
        }
    }

    fn damaged(&self) -> bool {
        self.albums
            .iter()
            .any(|album| album.index == IndexState::Damaged)
            || self
                .catalogs
                .iter()
                .any(|catalog| catalog.state == CatalogState::Damaged)
    }

    fn ok(&self) -> bool {
        self.failure.is_none() && self.io_errors.is_empty() && !self.damaged()
    }

    pub(crate) fn exit_code(&self) -> i32 {
        i32::from(!self.ok())
    }

    pub(crate) fn to_json(&self) -> Value {
        let findings = self
            .albums
            .iter()
            .flat_map(|album| album.issues.iter().map(AlbumIssue::to_json))
            .chain(self.catalogs.iter().flat_map(|catalog| {
                catalog.issues.iter().map(AlbumIssue::to_json)
            }))
            .collect::<Vec<_>>();
        let records =
            self.albums.iter().map(|album| album.records).sum::<usize>();
        let roots = self
            .albums
            .iter()
            .filter_map(|album| album.root.as_deref())
            .collect::<BTreeSet<_>>()
            .len();
        let mut errors = 0usize;
        let mut warnings = 0usize;
        for issue in self
            .albums
            .iter()
            .flat_map(|album| album.issues.iter())
            .chain(
                self.catalogs
                    .iter()
                    .flat_map(|catalog| catalog.issues.iter()),
            )
        {
            match issue.severity {
                super::AlbumIssueSeverity::Error => errors += 1,
                super::AlbumIssueSeverity::Warning => warnings += 1,
                super::AlbumIssueSeverity::Info => {}
            }
        }
        let mut payload = json!({
            "schema_version": SCHEMA_VERSION,
            "ok": self.ok(),
            "command": "sync",
            "dry_run": self.dry_run,
            "changed": self.changes.iter().map(|change| change.path.clone()).collect::<Vec<_>>(),
            "changes": self.changes.iter().map(|change| {
                let mut item = json!({"path": change.path, "action": "update"});
                if self.dry_run {
                    item["diff"] = Value::String(change.diff());
                }
                item
            }).collect::<Vec<_>>(),
            // The entries are the same deterministic index/catalog report used
            // by `albums list` and `albums check`.
            "albums": self.albums.iter().map(AlbumReportEntry::to_json).collect::<Vec<_>>(),
            "catalogs": self.catalogs.iter().map(AlbumCatalogRecord::to_json).collect::<Vec<_>>(),
            "counts": {
                "albums": self.albums.len(),
                "records": records,
                "roots": roots,
                "damaged": self.albums.iter().filter(|album| album.index == IndexState::Damaged).count(),
                "errors": errors,
                "warnings": warnings,
            },
            "findings": findings,
            "io_errors": self.io_errors,
            "commit": commit_json(self.commit.as_ref()),
            "sync": sync_json(self.sync_pre, self.sync_post),
            "warnings": self.warnings,
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
}

/// Build the shared index plan from one coherent album registry and vault
/// snapshot. Every declaration is guarded by its full preimage; record
/// members are guarded by frontmatter plus direct Markdown folder membership.
pub(crate) fn plan_indexes(
    vault: &Path,
    registry: &AlbumRegistry,
    snapshot: &VaultSnapshot,
) -> Result<IndexSyncPlan, TxnFailure> {
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
                "fix unreadable vault paths, then retry; no index was written",
            ),
        ));
    }

    let mut catalog = RecordCatalog::new(vault);
    let mut dependencies = ReadDependencies::default();
    let mut albums = Vec::with_capacity(registry.albums.len());
    let mut edits = Vec::new();
    let mut io_errors = Vec::new();
    for album in &registry.albums {
        if let Some(contents) = &album.contents {
            dependencies.insert(
                PathBuf::from(&album.declaration.path),
                Some(contents.clone()),
            );
        }
        let folder = catalog.folder(&album.declaration.stem);
        if folder.diagnostics().is_empty() {
            let members = folder.membership();
            dependencies.insert_directory_membership(
                PathBuf::from(folder.directory()),
                folder.present().then_some(members),
            );
        }
        for note in folder.notes() {
            if !note.diagnostics.iter().any(|diagnostic| {
                diagnostic.code
                    == crate::native::records::RecordDiagnosticCode::NoteRead
            }) {
                dependencies.insert_frontmatter(
                    note.relative_path.clone(),
                    note.frontmatter.clone(),
                );
            }
        }
        let projection = report::project_album_from_folder(album, folder);
        io_errors.extend(projection.io_errors);
        if let Some(edit) = projection.index_plan.edit {
            edits.push(edit);
        }
        albums.push(projection.entry);
    }
    albums.sort_by(|left, right| left.path.cmp(&right.path));
    edits.sort_by(|left, right| left.path.cmp(&right.path));

    // The snapshot is the discovery source for album declarations; require
    // it to contain each declaration preimage used above.
    for album in &registry.albums {
        let path = Path::new(&album.declaration.path);
        if snapshot.contents(path).is_none() {
            return Err(TxnFailure::new(
                "io",
                format!(
                    "{} was not retained for index planning",
                    album.declaration.path
                ),
                Some("repair the unreadable album note, then retry"),
            ));
        }
    }

    Ok(IndexSyncPlan {
        albums,
        edits,
        dependencies,
        io_errors,
    })
}

/// Compose index and catalog plans from one snapshot/registry. Edits share
/// one final preimage and postimage per path.
pub(crate) fn plan_sync(
    vault: &Path,
    registry: &AlbumRegistry,
    snapshot: &VaultSnapshot,
) -> Result<CombinedSyncPlan, TxnFailure> {
    let indexes = plan_indexes(vault, registry, snapshot)?;
    let catalogs = catalog::plan_catalogs(snapshot, registry);
    let mut dependencies = indexes.dependencies;
    dependencies.extend(catalog::catalog_read_dependencies(snapshot, registry));
    let catalog_edits = catalogs
        .edits()
        .into_iter()
        .cloned()
        .collect::<Vec<FileEdit>>();
    let edits = merge_edits(indexes.edits, catalog_edits);
    let changes = edits
        .iter()
        .map(|edit| ChangedFile {
            path: slash(&edit.path),
            before: edit.before.clone(),
            after: edit.after.clone(),
        })
        .collect();
    Ok(CombinedSyncPlan {
        albums: indexes.albums,
        catalogs: catalogs.records(),
        changes,
        edits,
        dependencies,
        io_errors: indexes.io_errors,
    })
}

fn merge_edits(
    indexes: Vec<FileEdit>,
    catalogs: Vec<FileEdit>,
) -> Vec<FileEdit> {
    let mut by_path = BTreeMap::new();
    for edit in indexes.into_iter().chain(catalogs) {
        match by_path.remove(&edit.path) {
            None => {
                by_path.insert(edit.path.clone(), edit);
            }
            Some(existing) => {
                let merged = if existing.after == edit.before {
                    FileEdit {
                        path: existing.path,
                        before: existing.before,
                        after: edit.after,
                    }
                } else if edit.after == existing.before {
                    FileEdit {
                        path: existing.path,
                        before: edit.before,
                        after: existing.after,
                    }
                } else {
                    FileEdit {
                        path: existing.path,
                        before: existing.before,
                        after: edit.after,
                    }
                };
                by_path.insert(merged.path.clone(), merged);
            }
        }
    }
    by_path.into_values().collect()
}

fn planned_txn(
    vault: &Path,
    plan: CombinedSyncPlan,
) -> PlannedTxn<SyncContext> {
    let hosts = plan.edits.iter().map(|edit| edit.path.clone()).collect();
    let files = host_catalog::staged_rewrites(vault, &plan.edits);
    let count = files.len();
    let verify_vault = vault.to_path_buf();
    let verify = Box::new(move || verify_current_hosts(&verify_vault, hosts));
    PlannedTxn {
        files,
        appends: Vec::new(),
        deps: plan.dependencies,
        subject: format!("albums: sync ({count} notes)"),
        verify,
        context: SyncContext {
            albums: plan.albums,
            catalogs: plan.catalogs,
            changes: plan.changes,
            io_errors: plan.io_errors,
        },
        kind: "album",
    }
}

fn verify_current_hosts(
    vault: &Path,
    hosts: Vec<PathBuf>,
) -> Result<(), String> {
    let snapshot = VaultSnapshot::load(vault);
    if let Some(error) = snapshot.io_errors.first() {
        return Err(error.clone());
    }
    let registry =
        AlbumRegistry::from_snapshot(vault, &config::config_path(), &snapshot);
    let report = report::build_report(&registry, &snapshot);
    let catalogs = catalog::plan_catalogs(&snapshot, &registry);
    for host in hosts {
        let key = host.to_string_lossy().replace('\\', "/");
        if let Some(album) =
            report.albums.iter().find(|album| album.path == key)
        {
            if album.index != IndexState::Current {
                return Err(format!(
                    "{key} is still not current after the write"
                ));
            }
            continue;
        }
        if !catalog::host_catalog_is_current(catalogs.host(Path::new(&key))) {
            return Err(format!("{key} is still not current after the write"));
        }
    }
    Ok(())
}

/// Read-only combined plan. It takes no lock and runs no Git or sync command.
pub(crate) fn dry_run(vault: &Path) -> SyncReport {
    let snapshot = VaultSnapshot::load(vault);
    let registry =
        AlbumRegistry::from_snapshot(vault, &config::config_path(), &snapshot);
    let plan = match plan_sync(vault, &registry, &snapshot) {
        Ok(plan) => plan,
        Err(failure) => return SyncReport::failed(true, failure),
    };
    SyncReport {
        dry_run: true,
        albums: plan.albums,
        catalogs: plan.catalogs,
        changes: plan.changes,
        commit: None,
        sync_pre: None,
        sync_post: None,
        warnings: Vec::new(),
        io_errors: plan.io_errors,
        failure: None,
    }
}

fn report_of(result: TxnResult<SyncContext>) -> SyncReport {
    let mut albums = result.context.albums;
    let mut catalogs = result.context.catalogs;
    if result.commit.is_some() {
        for album in &mut albums {
            if result
                .context
                .changes
                .iter()
                .any(|change| change.path == album.path)
            {
                album.index = IndexState::Current;
                album
                    .issues
                    .retain(|issue| issue.code != "album_index_stale");
            }
        }
        for catalog in &mut catalogs {
            if result
                .context
                .changes
                .iter()
                .any(|change| change.path == catalog.path)
            {
                catalog.state = CatalogState::Current;
                catalog
                    .issues
                    .retain(|issue| issue.code != "album_catalog_stale");
            }
        }
    }
    SyncReport {
        dry_run: false,
        albums,
        catalogs,
        changes: result.context.changes,
        commit: result.commit,
        sync_pre: Some(result.sync_pre),
        sync_post: Some(result.sync_post),
        warnings: result.post_sync_warning.into_iter().collect(),
        io_errors: result.context.io_errors,
        failure: None,
    }
}

/// Lock-free core for nightly. The caller holds the shared vault lock and
/// owns the surrounding vault-sync cycles.
pub(crate) fn sync_with_existing_lock(
    vault: &Path,
    child_env: &ChildEnv,
) -> SyncReport {
    let snapshot = VaultSnapshot::load(vault);
    let registry =
        AlbumRegistry::from_snapshot(vault, &config::config_path(), &snapshot);
    let plan = match plan_sync(vault, &registry, &snapshot) {
        Ok(plan) => plan,
        Err(failure) => return SyncReport::failed(false, failure),
    };
    match apply_and_commit(vault, child_env, planned_txn(vault, plan)) {
        Ok((commit, context)) => report_of(TxnResult {
            commit,
            sync_pre: "skipped",
            sync_post: "skipped",
            post_sync_warning: None,
            context,
        }),
        Err(failure) => SyncReport::failed(false, failure),
    }
}

/// Standalone real execution: shared lock, pre-sync, guarded transaction,
/// one scoped commit, then post-sync only when an index or catalog changed.
pub(crate) fn run_standalone(vault: &Path, json_mode: bool) -> SyncReport {
    let child_env = ob::child_env();
    let captured: RefCell<Option<SyncContext>> = RefCell::new(None);
    let result =
        host_catalog::run_locked(vault, &child_env, json_mode, "album", || {
            let snapshot = VaultSnapshot::load(vault);
            let registry = AlbumRegistry::from_snapshot(
                vault,
                &config::config_path(),
                &snapshot,
            );
            let plan = plan_sync(vault, &registry, &snapshot)?;
            *captured.borrow_mut() = Some(SyncContext {
                albums: plan.albums.clone(),
                catalogs: plan.catalogs.clone(),
                changes: plan.changes.clone(),
                io_errors: plan.io_errors.clone(),
            });
            Ok(planned_txn(vault, plan))
        });
    match result {
        Ok(result) => report_of(result),
        Err(failure) => {
            let mut report = SyncReport::failed(false, failure);
            if let Some(context) = captured.into_inner() {
                report.albums = context.albums;
                report.catalogs = context.catalogs;
                report.changes = context.changes;
                report.io_errors = context.io_errors;
            }
            report
        }
    }
}

fn commit_json(commit: Option<&CommitInfo>) -> Value {
    match commit {
        Some(commit) => json!({"sha": commit.sha, "subject": commit.subject}),
        None => Value::Null,
    }
}

fn sync_json(pre: Option<&'static str>, post: Option<&'static str>) -> Value {
    match (pre, post) {
        (Some(pre), Some(post)) => json!({"pre": pre, "post": post}),
        _ => Value::Null,
    }
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn print_human(report: &SyncReport) {
    let styler = Styler::detect();
    if let Some(failure) = &report.failure {
        eprintln!("{} {}", styler.red("✗"), failure.message);
        if let Some(hint) = &failure.hint {
            eprintln!("  {}", styler.dim(hint));
        }
    }
    for error in &report.io_errors {
        eprintln!("{} {error}", styler.red("✗"));
    }
    for album in &report.albums {
        if album.index == IndexState::Damaged {
            for issue in &album.issues {
                if issue.severity == super::AlbumIssueSeverity::Error {
                    println!(
                        "{} {} [{}] {}",
                        styler.red("✗"),
                        issue.path,
                        issue.code,
                        issue.message
                    );
                }
            }
        }
    }
    for catalog in &report.catalogs {
        if catalog.state == CatalogState::Damaged {
            for issue in &catalog.issues {
                if issue.severity == super::AlbumIssueSeverity::Error {
                    let line = issue
                        .line
                        .map(|line| format!(":{line}"))
                        .unwrap_or_default();
                    println!(
                        "{} {}{line} [{}] {}",
                        styler.red("✗"),
                        issue.path,
                        issue.code,
                        issue.message
                    );
                }
            }
        }
    }
    if report.dry_run {
        for change in &report.changes {
            print!("{}", color_diff(&change.diff(), &styler));
        }
        if report.changes.is_empty()
            && report.failure.is_none()
            && !report.damaged()
        {
            println!(
                "{} album indexes and catalogs are current",
                styler.green("✓")
            );
        } else if !report.changes.is_empty() {
            println!(
                "{} dry run: {} {} would change; nothing was written",
                styler.yellow("!"),
                report.changes.len(),
                if report.changes.len() == 1 {
                    "note"
                } else {
                    "notes"
                }
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
                "{} synced {} {} ({})",
                styler.green("✓"),
                report.changes.len(),
                if report.changes.len() == 1 {
                    "note"
                } else {
                    "notes"
                },
                &commit.sha[..commit.sha.len().min(9)]
            );
            for change in &report.changes {
                println!("  {}", change.path);
            }
        }
        None if !report.damaged() => {
            println!(
                "{} album indexes and catalogs are current",
                styler.green("✓")
            );
        }
        None => {}
    }
    for warning in &report.warnings {
        println!("{} {warning}", styler.yellow("!"));
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

pub(crate) fn nightly_step(vault: &Path, child_env: &ChildEnv) -> i32 {
    let report = sync_with_existing_lock(vault, child_env);
    print_human(&report);
    report.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const ALBUM: &str = "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Places.\nrecord:\n  noun: restaurant\n  fields:\n    status:\n      values: {to_try: To try, liked: Liked}\n---\n\n# Restaurants\n";
    const RECORD: &str = "---\ntype: [[record]]\nparent: \"[[eat]]\"\ntitle: One\nstatus: to_try\n---\n\n# One\n";

    fn planned(vault: &Path) -> CombinedSyncPlan {
        let snapshot = VaultSnapshot::load(vault);
        let registry = AlbumRegistry::from_snapshot(
            vault,
            &config::config_path(),
            &snapshot,
        );
        plan_sync(vault, &registry, &snapshot).expect("combined plan")
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let vault = temp.path().join("vault");
        fs::create_dir_all(vault.join("eat")).expect("eat directory");
        fs::write(
            vault.join("body.md"),
            "---\ntype: [[area]]\n---\n\n# Body\n",
        )
        .expect("owner");
        fs::write(vault.join("eat.md"), ALBUM).expect("album");
        fs::write(vault.join("eat/one.md"), RECORD).expect("record");
        (temp, vault)
    }

    #[test]
    fn record_frontmatter_is_guarded_but_its_body_is_not() {
        let (_temp, vault) = fixture();
        let plan = planned(&vault);
        fs::write(
            vault.join("eat/one.md"),
            RECORD.replace("# One", "A longer body\n# One"),
        )
        .expect("body edit");
        assert!(plan.dependencies.validate(&vault).is_ok());

        fs::write(
            vault.join("eat/one.md"),
            RECORD.replace("status: to_try", "status: liked"),
        )
        .expect("frontmatter edit");
        let error = plan.dependencies.validate(&vault).unwrap_err();
        assert_eq!(
            error.typed,
            Some(crate::native::capture::CaptureFailureKind::NoteChanged)
        );
    }

    #[test]
    fn album_preimage_and_folder_membership_are_guarded() {
        let (_temp, vault) = fixture();
        let plan = planned(&vault);
        fs::write(
            vault.join("eat.md"),
            ALBUM.replace("Places.", "New places."),
        )
        .expect("album edit");
        assert!(plan.dependencies.validate(&vault).is_err());

        let plan = planned(&vault);
        fs::write(vault.join("eat/two.md"), RECORD.replace("One", "Two"))
            .expect("new member");
        assert!(plan.dependencies.validate(&vault).is_err());

        let plan = planned(&vault);
        fs::remove_file(vault.join("eat/one.md")).expect("remove member");
        let error = plan.dependencies.validate(&vault).unwrap_err();
        assert_eq!(
            error.typed,
            Some(crate::native::capture::CaptureFailureKind::NoteChanged)
        );
    }

    #[test]
    fn owner_preimage_and_new_album_membership_are_guarded() {
        let (_temp, vault) = fixture();
        let plan = planned(&vault);
        fs::write(
            vault.join("body.md"),
            "---\ntype: [[area]]\n---\n\n# Body\n\nAuthored.\n",
        )
        .expect("owner edit");
        assert!(plan.dependencies.validate(&vault).is_err());

        let plan = planned(&vault);
        fs::write(
            vault.join("movies.md"),
            "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Films.\nrecord:\n  noun: movie\n---\n\n# Movies\n",
        )
        .expect("new album");
        let error = plan.dependencies.validate(&vault).unwrap_err();
        assert_eq!(
            error.typed,
            Some(crate::native::capture::CaptureFailureKind::NoteChanged)
        );
    }

    #[test]
    fn combined_plan_includes_the_owner_catalog_edit() {
        let (_temp, vault) = fixture();
        let plan = planned(&vault);
        let paths = plan
            .changes
            .iter()
            .map(|change| change.path.as_str())
            .collect::<Vec<_>>();
        assert!(paths.contains(&"eat.md"), "{paths:?}");
        assert!(paths.contains(&"body.md"), "{paths:?}");
        let body = plan
            .changes
            .iter()
            .find(|change| change.path == "body.md")
            .unwrap();
        assert!(body.after.contains("> [!albums]"), "{}", body.after);
        assert!(body.after.contains("[[eat|Restaurants]]"), "{}", body.after);
    }
}
