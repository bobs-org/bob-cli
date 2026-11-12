//! Snapshot loading, dry-run reporting, and guarded application for
//! `bob bank import`.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use chrono::{Local, NaiveDate, TimeZone, Utc};
use serde_json::{json, Value};

use super::{
    config::{self, AccountStatus, BankConfig, Category, ConfigParse},
    import_plan::{
        self, BatchPlan, MonthEdit, PlanInput, PlanRefusal, ReadDep,
        RestaurantEntry,
    },
    journal, scan,
    source::{self, ParsedStatement, ResolvedStatement, SourceError},
};
use crate::native::{
    albums::registry::{DiscoveryHealth, RecordTypeSource},
    bank::model,
    capture::StagedTextFile,
    host_catalog::{
        CommitInfo, PlannedAppend, PlannedTxn, ReadDependencies, TxnResult,
    },
    ob, pref_journal,
    records::{
        catalog::RecordCatalog,
        context::{RecordContext, RecordKindResolution},
        schema::RecordDiagnosticCode,
    },
    style::Styler,
    vault_rewrite::TxnFailure,
};

/// A source parsed before the command enters the maintenance path. Its bytes
/// and digest stay fixed for the lifetime of this invocation.
#[derive(Debug, Clone)]
pub(crate) struct PreparedSource {
    pub(crate) label: String,
    pub(crate) statement: ParsedStatement,
}

#[derive(Debug)]
pub(crate) struct ImportReport {
    pub(crate) dry_run: bool,
    pub(crate) today: NaiveDate,
    pub(crate) config: Option<BankConfig>,
    pub(crate) plan: Option<BatchPlan>,
    pub(crate) commit: Option<CommitInfo>,
    pub(crate) sync_pre: Option<&'static str>,
    pub(crate) sync_post: Option<&'static str>,
    pub(crate) warnings: Vec<String>,
    pub(crate) failure: Option<TxnFailure>,
}

impl ImportReport {
    pub(crate) fn failed(
        dry_run: bool,
        today: NaiveDate,
        failure: TxnFailure,
    ) -> Self {
        Self {
            dry_run,
            today,
            config: None,
            plan: None,
            commit: None,
            sync_pre: None,
            sync_post: None,
            warnings: Vec::new(),
            failure: Some(failure),
        }
    }

    pub(crate) fn exit_code(&self) -> i32 {
        match &self.failure {
            None => 0,
            Some(failure) if failure.code.starts_with("bank_import_") => 2,
            Some(failure) if failure.code == "stale_note" => 2,
            Some(_) => 1,
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        let config = self.config.as_ref();
        let accounts = config.map(config::account_statuses).unwrap_or_default();
        let categories = config
            .map(|config| config.categories.as_slice())
            .unwrap_or_default();
        let findings = self
            .plan
            .as_ref()
            .map(|plan| {
                plan.findings
                    .iter()
                    .map(|finding| {
                        json!({
                            "code": finding.code,
                            "path": finding.path,
                            "line": finding.line,
                            "message": finding.message,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let changed = self.plan.as_ref().is_some_and(|plan| plan.changed);
        let files = self
            .plan
            .as_ref()
            .map(|plan| plan.files.iter().map(file_json).collect::<Vec<_>>())
            .unwrap_or_default();
        let rows = self
            .plan
            .as_ref()
            .map(|plan| {
                plan.rows
                    .iter()
                    .map(|row| model::row_json(row, "checking"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let changes = self
            .plan
            .as_ref()
            .map(|plan| changes_json(plan, self.dry_run))
            .unwrap_or_default();
        let mut payload = json!({
            "schema_version": 1,
            "ok": self.failure.is_none(),
            "today": self.today.format("%Y-%m-%d").to_string(),
            "categories": categories.iter().map(category_json).collect::<Vec<_>>(),
            "accounts": accounts.iter().map(account_json).collect::<Vec<_>>(),
            "issues": {
                "errors": if self.failure.is_some() { 1 } else { 0 },
                "warnings": findings.len(),
            },
            "dry_run": self.dry_run,
            "changed": changed,
            "changes": changes,
            "findings": findings,
            "commit": commit_json(self.commit.as_ref()),
            "sync": {
                "pre": self.sync_pre,
                "post": self.sync_post,
            },
            "warnings": self.warnings,
            "files": files,
            "rows": rows,
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

fn category_json(category: &Category) -> Value {
    json!({
        "key": category.key,
        "emoji": category.emoji,
        "label": category.label,
        "builtin": category.builtin,
    })
}

fn account_json(account: &AccountStatus) -> Value {
    json!({
        "alias": account.alias,
        "label": account.label,
        "kind": account.kind.as_str(),
        "default": account.default,
        "start": account.start.map(|day| day.format("%Y-%m-%d").to_string()),
        "status": account.status,
        "last_import": Value::Null,
        "balance_cents": Value::Null,
        "balance_asof": Value::Null,
        "delta_cents": Value::Null,
        "hint": account.hint,
    })
}

fn file_json(file: &import_plan::PlannedFile) -> Value {
    json!({
        "file": file.file,
        "account": file.account,
        "source": file.source.as_str(),
        "start": file.start.map(|day| day.format("%Y-%m-%d").to_string()),
        "end": file.end.map(|day| day.format("%Y-%m-%d").to_string()),
        "new": file.counts.new,
        "matched": file.counts.matched,
        "seen": file.counts.seen,
        "ambiguous": file.counts.ambiguous,
        "before_start": file.counts.before_start,
        "out_cents": file.out_cents,
        "in_cents": file.in_cents,
        "balance_cents": file.balance_cents,
        "balance_asof": file.balance_asof.map(|day| day.format("%Y-%m-%d").to_string()),
        "status": if file.new_evidence { "new_evidence" } else { "seen" },
    })
}

fn commit_json(commit: Option<&CommitInfo>) -> Value {
    commit.map_or(
        Value::Null,
        |commit| json!({ "sha": commit.sha, "subject": commit.subject }),
    )
}

fn changes_json(plan: &BatchPlan, dry_run: bool) -> Vec<Value> {
    let mut changes = Vec::new();
    for edit in &plan.month_edits {
        let mut value = json!({ "path": edit.path });
        if dry_run {
            value["diff"] = Value::String(text_diff(
                &edit.path,
                edit.before.as_deref().unwrap_or_default(),
                &edit.after,
                edit.before.is_none(),
            ));
        }
        changes.push(value);
    }
    if let Some((path, suffix)) = journal_change(plan) {
        let mut value = json!({ "path": path });
        if dry_run {
            value["diff"] = Value::String(journal_diff(&suffix));
        }
        changes.push(value);
    }
    changes
}

fn text_diff(path: &str, before: &str, after: &str, created: bool) -> String {
    let from = if created {
        "/dev/null".to_owned()
    } else {
        format!("a/{path}")
    };
    let to = format!("b/{path}");
    similar::TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(&from, &to)
        .to_string()
}

fn journal_change(plan: &BatchPlan) -> Option<(String, Vec<u8>)> {
    if plan.journal_events.is_empty() {
        return None;
    }
    let host = plan
        .journal_events
        .first()
        .and_then(|event| serde_json::from_str::<Value>(event).ok())
        .and_then(|event| event.get("host")?.as_str().map(str::to_owned))
        .unwrap_or_else(pref_journal::host_label);
    let path = format!(".bob/bank/{host}.jsonl");
    let mut suffix = Vec::new();
    if plan
        .read_deps
        .iter()
        .find(|dep| dep.path == path)
        .and_then(|dep| dep.preimage.as_ref())
        .is_some_and(|bytes| !bytes.is_empty() && !bytes.ends_with(b"\n"))
    {
        suffix.push(b'\n');
    }
    for event in &plan.journal_events {
        suffix.extend_from_slice(event.as_bytes());
    }
    Some((path, suffix))
}

fn journal_diff(suffix: &[u8]) -> String {
    let text = String::from_utf8_lossy(suffix);
    let mut diff = String::from("--- journal append\n+++ journal append\n");
    for line in text.split_inclusive('\n') {
        diff.push('+');
        diff.push_str(line);
        if !line.ends_with('\n') {
            diff.push('\n');
        }
    }
    diff
}

fn failure(
    code: &'static str,
    message: impl Into<String>,
    hint: &str,
) -> TxnFailure {
    TxnFailure::new(code, message, Some(hint.to_owned()))
}

/// Parse every source before a write command enters the shared maintenance
/// path. A malformed member refuses the full batch.
pub(crate) fn prepare_sources(
    paths: &[PathBuf],
) -> Result<Vec<PreparedSource>, SourceError> {
    let mut prepared = Vec::with_capacity(paths.len());
    for (order, path) in paths.iter().enumerate() {
        let label = path.to_string_lossy().into_owned();
        let bytes = fs::read(path).map_err(|error| SourceError {
            file: label.clone(),
            record: None,
            field: None,
            message: format!("could not read source: {error}"),
            hint: None,
        })?;
        let statement = source::parse_export(&label, &bytes, order)?;
        prepared.push(PreparedSource { label, statement });
    }
    Ok(prepared)
}

/// Build one fresh pure plan from the current vault snapshot.
pub(crate) fn plan_import(
    vault: &Path,
    sources: &[PreparedSource],
    requested_account: Option<&str>,
    now_stamp: &str,
    host: &str,
) -> Result<
    (
        BatchPlan,
        BankConfig,
        ReadDependencies,
        Vec<StagedTextFile>,
        Vec<PlannedAppend>,
        Vec<(PathBuf, Vec<u8>)>,
    ),
    TxnFailure,
> {
    let scan = scan::scan_bank(vault);
    if !scan.io_errors.is_empty() {
        return Err(failure(
            "bank_import_incomplete_read",
            format!(
                "refusing unsafe import; unreadable: {}",
                scan.io_errors.join(", ")
            ),
            "fix the listed bank files and retry",
        ));
    }
    let hub = scan.hub.as_ref().ok_or_else(|| {
        failure(
            "bank_import_config",
            "no bank.md configuration; create a bank hub before importing",
            "see docs/bank.md for the bank mapping",
        )
    })?;
    let config = match config::parse_bank_config(&hub.contents) {
        ConfigParse::Ok(config) => config,
        ConfigParse::MissingMapping => {
            return Err(failure(
                "bank_import_config",
                "bank.md has no usable `bank:` mapping",
                "fix bank.md before importing",
            ));
        }
        ConfigParse::Invalid { diagnostics, .. } => {
            let detail = diagnostics
                .first()
                .map(|diagnostic| {
                    format!(
                        "bank.md{}{}: {}",
                        diagnostic
                            .line
                            .map_or(String::new(), |line| format!(":{line}")),
                        if diagnostic.key_path.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", diagnostic.key_path)
                        },
                        diagnostic.message
                    )
                })
                .unwrap_or_else(|| {
                    "bank.md has invalid bank configuration".to_owned()
                });
            return Err(failure(
                "bank_import_config",
                detail,
                "fix bank.md before importing",
            ));
        }
    };

    let journal = journal::load(vault).map_err(|error| {
        failure(
            "bank_import_incomplete_read",
            format!("could not read the bank journal: {error}"),
            "fix journal permissions and retry",
        )
    })?;
    if !journal.io_errors.is_empty() {
        return Err(failure(
            "bank_import_incomplete_read",
            format!(
                "refusing unsafe import; unreadable: {}",
                journal.io_errors.join(", ")
            ),
            "fix the listed bank journal files and retry",
        ));
    }

    let mut resolved = Vec::<ResolvedStatement>::with_capacity(sources.len());
    for source in sources {
        let statement = source::resolve_account(
            source.statement.clone(),
            &config,
            requested_account,
            &source.label,
        )
        .map_err(source_error_failure)?;
        resolved.push(statement);
    }

    let (
        restaurants,
        restaurant_deps,
        registry_dependencies,
        restaurant_directory,
        restaurant_dir_entries,
        root_md_entries,
    ) = load_restaurants(vault)?;
    let plan = import_plan::plan_batch(PlanInput {
        config: Some(&config),
        hub: ReadDep {
            path: hub.path.clone(),
            preimage: Some(hub.contents.as_bytes().to_vec()),
        },
        scan: &scan,
        journal: &journal,
        statements: resolved,
        file_labels: sources
            .iter()
            .map(|source| source.label.clone())
            .collect(),
        restaurants,
        restaurant_deps,
        now_stamp: now_stamp.to_owned(),
        host: host.to_owned(),
    })
    .map_err(plan_refusal_failure)?;

    let mut deps = ReadDependencies::default();
    for read in &plan.read_deps {
        deps.insert_bytes(PathBuf::from(&read.path), read.preimage.clone());
    }
    deps.insert_directory(
        PathBuf::from("bank"),
        Some("md"),
        scan.month_entries.clone(),
    );
    deps.insert_directory(
        PathBuf::from(".bob/bank"),
        Some("jsonl"),
        journal.directory_entries.clone(),
    );
    deps.insert_directory(PathBuf::from("."), Some("md"), root_md_entries);
    if let Some(directory) = restaurant_directory {
        deps.insert_directory(
            PathBuf::from(directory),
            Some("md"),
            restaurant_dir_entries,
        );
    }
    for (path, preimage) in registry_dependencies {
        deps.insert_bytes(path, preimage);
    }

    let staged = plan
        .month_edits
        .iter()
        .map(|edit| StagedTextFile {
            target: vault.join(&edit.path),
            target_existed: edit.before.is_some(),
            original_target: edit.before.clone().unwrap_or_default(),
            updated_target: edit.after.clone(),
        })
        .collect::<Vec<_>>();
    let appends =
        if plan.journal_events.is_empty() {
            Vec::new()
        } else {
            let path = format!(".bob/bank/{host}.jsonl");
            let mut suffix = Vec::new();
            if journal.preimages.get(&path).is_some_and(|bytes| {
                !bytes.is_empty() && !bytes.ends_with(b"\n")
            }) {
                suffix.push(b'\n');
            }
            for event in &plan.journal_events {
                suffix.extend_from_slice(event.as_bytes());
            }
            vec![PlannedAppend {
                target: vault.join(&path),
                original_target: journal.preimages.get(&path).cloned(),
                suffix,
            }]
        };
    let mut verify = staged
        .iter()
        .map(|file| {
            (file.target.clone(), file.updated_target.as_bytes().to_vec())
        })
        .collect::<Vec<_>>();
    verify.extend(appends.iter().map(|append| {
        let mut bytes = append.original_target.clone().unwrap_or_default();
        bytes.extend_from_slice(&append.suffix);
        (append.target.clone(), bytes)
    }));
    Ok((plan, config, deps, staged, appends, verify))
}

fn load_restaurants(
    vault: &Path,
) -> Result<
    (
        Vec<RestaurantEntry>,
        Vec<ReadDep>,
        Vec<(PathBuf, Option<Vec<u8>>)>,
        Option<String>,
        Vec<String>,
        Vec<String>,
    ),
    TxnFailure,
> {
    let config_path = crate::native::config::config_path();
    let config_preimage = optional_file_bytes(&config_path)?;
    let root_md_entries = directory_names(vault, Some("md"))?;
    let context = RecordContext::discover(vault, &config_path);
    if optional_file_bytes(&config_path)? != config_preimage
        || directory_names(vault, Some("md"))? != root_md_entries
    {
        return Err(failure(
            "bank_import_registry",
            "the records registry changed while it was being read",
            "re-run the import against the latest registry",
        ));
    }
    let mut registry_dependencies = Vec::new();
    registry_dependencies.push((config_path.clone(), config_preimage));
    for album in &context.registry().albums {
        let path = vault.join(&album.declaration.path);
        let bytes = fs::read(&path).map_err(|error| {
            failure(
                "bank_import_registry",
                format!("could not read {}: {error}", album.declaration.path),
                "repair the album declaration and retry",
            )
        })?;
        if album
            .contents
            .as_ref()
            .is_some_and(|contents| contents.as_bytes() != bytes)
        {
            return Err(failure(
                "bank_import_registry",
                format!(
                    "{} changed while the records registry was read",
                    album.declaration.path
                ),
                "re-run the import against the latest registry",
            ));
        }
        registry_dependencies
            .push((PathBuf::from(&album.declaration.path), Some(bytes)));
    }

    if let DiscoveryHealth::Partial(errors) = &context.registry().health {
        return Err(failure(
            "bank_import_registry",
            format!(
                "could not read the records registry: {}",
                errors.join(", ")
            ),
            "repair the records registry and retry",
        ));
    }

    let kind = match context.resolve("eat") {
        RecordKindResolution::Valid { kind, .. } => {
            if let RecordTypeSource::Config { path } = &kind.sourced.source
                && path != &config_path
            {
                registry_dependencies
                    .push((path.clone(), optional_file_bytes(path)?));
            }
            Some(kind)
        }
        RecordKindResolution::DiscoveryIncomplete { errors } => {
            return Err(failure(
                "bank_import_registry",
                format!(
                    "could not read the restaurant registry: {}",
                    errors.join(", ")
                ),
                "repair the records registry and retry",
            ));
        }
        _ => None,
    };
    let mut entries = Vec::new();
    let mut deps = Vec::new();
    let mut restaurant_directory = None;
    let mut dir_entries = Vec::new();
    if let Some(kind) = kind {
        let record_type = kind.record_type();
        let directory = &record_type.dir;
        restaurant_directory = Some(directory.clone());
        let record_directory = vault.join(directory);
        let before_entries = directory_names(&record_directory, Some("md"))?;
        let mut catalog = RecordCatalog::new(vault);
        let folder = catalog.folder(directory);
        dir_entries = directory_names(&record_directory, Some("md"))?;
        if dir_entries != before_entries {
            return Err(failure(
                "bank_import_registry",
                format!("{directory}/ changed while its records were read"),
                "re-run the import against the latest registry",
            ));
        }
        for record in folder.notes() {
            let full_path = vault.join(&record.relative_path);
            let bytes = fs::read(&full_path).map_err(|error| {
                failure(
                    "bank_import_registry",
                    format!(
                        "could not read {}: {error}",
                        record.relative_path.display()
                    ),
                    "repair the restaurant note and retry",
                )
            })?;
            deps.push(ReadDep {
                path: record.relative_path.to_string_lossy().replace('\\', "/"),
                preimage: Some(bytes.clone()),
            });
            if record.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == RecordDiagnosticCode::NoteRead
            }) {
                return Err(failure(
                    "bank_import_registry",
                    format!(
                        "could not read {} from the restaurant registry",
                        record.relative_path.display()
                    ),
                    "repair the restaurant note and retry",
                ));
            }
            let markdown = std::str::from_utf8(&bytes).map_err(|error| {
                failure(
                    "bank_import_registry",
                    format!(
                        "{} is not readable UTF-8: {error}",
                        record.relative_path.display()
                    ),
                    "repair the restaurant note and retry",
                )
            })?;
            let current = crate::native::records::staged_record(
                record.relative_path.clone(),
                record.stem.clone(),
                markdown,
            );
            if current.title != record.title {
                return Err(failure(
                    "bank_import_registry",
                    format!(
                        "{} changed while its title was read",
                        record.relative_path.display()
                    ),
                    "re-run the import against the latest registry",
                ));
            }
            if let Some(title) = &record.title {
                let mut link_path = record.relative_path.clone();
                link_path.set_extension("");
                entries.push(RestaurantEntry {
                    title: title.clone(),
                    path: link_path.to_string_lossy().replace('\\', "/"),
                    emoji: record_type.emoji.clone(),
                });
            }
        }
    }
    Ok((
        entries,
        deps,
        registry_dependencies,
        restaurant_directory,
        dir_entries,
        root_md_entries,
    ))
}

fn optional_file_bytes(path: &Path) -> Result<Option<Vec<u8>>, TxnFailure> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failure(
            "bank_import_registry",
            format!("could not read {}: {error}", path.display()),
            "fix the records registry and retry",
        )),
    }
}

fn directory_names(
    directory: &Path,
    extension: Option<&str>,
) -> Result<Vec<String>, TxnFailure> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new())
        }
        Err(error) => {
            return Err(failure(
                "bank_import_incomplete_read",
                format!("could not read {}: {error}", directory.display()),
                "fix the directory permissions and retry",
            ))
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            failure(
                "bank_import_incomplete_read",
                format!("could not read {}: {error}", directory.display()),
                "fix the directory and retry",
            )
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if extension.is_none_or(|extension| {
            Path::new(&name)
                .extension()
                .is_some_and(|actual| actual == extension)
        }) {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

pub(crate) fn source_error_failure(error: SourceError) -> TxnFailure {
    failure(
        "bank_import_source",
        error.display(),
        error
            .hint
            .as_deref()
            .unwrap_or("fix the source file and retry"),
    )
}

fn plan_refusal_failure(refusal: PlanRefusal) -> TxnFailure {
    let (code, hint) = match &refusal {
        PlanRefusal::IdentityConflict { .. } => (
            "bank_import_identity_conflict",
            "use a consistent export for each transaction identity; no rows were written",
        ),
        PlanRefusal::DuplicateUnjournaledId { .. } => (
            "bank_import_duplicate_id",
            "resolve the duplicate bank IDs in the listed month notes, then retry",
        ),
        PlanRefusal::MissingConfig => (
            "bank_import_config",
            "fix bank.md before importing",
        ),
        PlanRefusal::IncompleteRead { .. } => (
            "bank_import_incomplete_read",
            "fix the listed bank files and retry",
        ),
        PlanRefusal::IdCollision { .. } => (
            "bank_import_id_collision",
            "resolve the row ID collision and retry",
        ),
    };
    failure(code, refusal.message(), hint)
}

fn text_diff_for(edit: &MonthEdit) -> String {
    text_diff(
        &edit.path,
        edit.before.as_deref().unwrap_or_default(),
        &edit.after,
        edit.before.is_none(),
    )
}

/// Build the transaction returned to the shared, lock-owning lifecycle.
pub(crate) fn planned_txn(
    vault: &Path,
    plan: BatchPlan,
    deps: ReadDependencies,
    files: Vec<StagedTextFile>,
    appends: Vec<PlannedAppend>,
    verify_files: Vec<(PathBuf, Vec<u8>)>,
) -> PlannedTxn<BatchPlan> {
    let aliases = plan
        .files
        .iter()
        .map(|file| file.account.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(",");
    let subject = format!("bank: import {} rows ({aliases})", plan.counts.new);
    let _ = vault;
    PlannedTxn {
        files,
        appends,
        deps,
        subject,
        verify: Box::new(move || {
            for (path, expected) in verify_files {
                let actual = fs::read(&path).map_err(|error| {
                    format!("read {}: {error}", path.display())
                })?;
                if actual != expected {
                    return Err(format!(
                        "{} did not match the planned after-image",
                        path.display()
                    ));
                }
            }
            Ok(())
        }),
        context: plan,
        kind: "bank import",
    }
}

/// Dry-run plan with no lock, sync, Git, directory creation, or writes.
pub(crate) fn dry_run(
    vault: &Path,
    sources: &[PreparedSource],
    requested_account: Option<&str>,
    now_stamp: &str,
    today: NaiveDate,
) -> ImportReport {
    match plan_import(
        vault,
        sources,
        requested_account,
        now_stamp,
        &pref_journal::host_label(),
    ) {
        Ok((plan, config, _, _, _, _)) => ImportReport {
            dry_run: true,
            today,
            config: Some(config),
            plan: Some(plan),
            commit: None,
            sync_pre: Some("skipped"),
            sync_post: Some("skipped"),
            warnings: Vec::new(),
            failure: None,
        },
        Err(failure) => ImportReport::failed(true, today, failure),
    }
}

/// Execute under the shared maintenance lock and sync lifecycle. The plan
/// closure runs only after pre-sync and reads the fresh vault snapshot.
pub(crate) fn apply(
    vault: &Path,
    sources: &[PreparedSource],
    requested_account: Option<&str>,
    now_stamp: &str,
    today: NaiveDate,
    json_mode: bool,
) -> ImportReport {
    let child_env = ob::child_env();
    let host = pref_journal::host_label();
    let result: Result<TxnResult<ApplyContext>, TxnFailure> =
        crate::native::host_catalog::run_locked(
            vault,
            &child_env,
            json_mode,
            "bank import",
            || {
                let (plan, config, deps, files, appends, verify_files) =
                    plan_import(
                        vault,
                        sources,
                        requested_account,
                        now_stamp,
                        &host,
                    )?;
                if !appends.is_empty() {
                    match crate::native::vault_git::check_ignore_quiet(
                        vault,
                        &child_env,
                        Path::new(&format!(".bob/bank/{host}.jsonl")),
                    ) {
                        Ok(crate::native::vault_git::IgnoreQuiet::Ignored) => {
                            return Err(failure(
                                "bank_import_journal_ignored",
                                "refusing import; Git ignores the bank journal destination",
                                "add `!/.bob/bank/*.jsonl` to the vault .gitignore allowlist, then retry",
                            ));
                        }
                        Ok(
                            crate::native::vault_git::IgnoreQuiet::Trackable,
                        ) => {}
                        Err(error) => {
                            return Err(failure(
                                "bank_import_journal_ignore_check",
                                format!("cannot verify bank journal Git policy: {error}"),
                                "check the vault worktree and .gitignore rules, then retry",
                            ));
                        }
                    }
                }
                let txn = planned_txn(
                    vault,
                    plan,
                    deps,
                    files,
                    appends,
                    verify_files,
                );
                Ok(PlannedTxn {
                    files: txn.files,
                    appends: txn.appends,
                    deps: txn.deps,
                    subject: txn.subject,
                    verify: txn.verify,
                    context: ApplyContext {
                        plan: txn.context,
                        config,
                    },
                    kind: txn.kind,
                })
            },
        );
    match result {
        Ok(result) => ImportReport {
            dry_run: false,
            today,
            config: Some(result.context.config),
            plan: Some(result.context.plan),
            commit: result.commit,
            sync_pre: Some(result.sync_pre),
            sync_post: Some(result.sync_post),
            warnings: result.post_sync_warning.into_iter().collect(),
            failure: None,
        },
        Err(failure) => ImportReport::failed(false, today, failure),
    }
}

struct ApplyContext {
    plan: BatchPlan,
    config: BankConfig,
}

pub(crate) fn print_human(report: &ImportReport) {
    let styler = Styler::detect();
    if let Some(failure) = &report.failure {
        eprintln!("{} bob bank import: {}", styler.red("✗"), failure.message);
        if let Some(hint) = &failure.hint {
            eprintln!("  {}", styler.dim(hint));
        }
        return;
    }
    let Some(plan) = &report.plan else {
        return;
    };
    if report.dry_run {
        println!("{}", styler.dim("Dry run — nothing written"));
    }
    for file in &plan.files {
        let range = match (file.start, file.end) {
            (Some(start), Some(end)) if start == end => start.to_string(),
            (Some(start), Some(end)) => format!("{start}–{end}"),
            _ => "date range unavailable".to_owned(),
        };
        println!("{} · {} · {range}", file.file, file.account);
        println!(
            "  New {} · matched {} · seen {} · ambiguous {} · before start {}",
            file.counts.new,
            file.counts.matched,
            file.counts.seen,
            file.counts.ambiguous,
            file.counts.before_start,
        );
        println!(
            "  Out {} · In {}",
            super::grammar::render_amount(-file.out_cents),
            super::grammar::render_amount(file.in_cents),
        );
        if let Some(balance) = file.balance_cents {
            let asof = file
                .balance_asof
                .map_or_else(String::new, |day| format!(" on {day}"));
            println!(
                "  Ledger balance {}{} · unverified",
                super::grammar::render_amount(balance),
                asof
            );
        }
    }
    for finding in &plan.findings {
        println!(
            "{}:{}: warning: {}",
            finding.path,
            finding.line.unwrap_or(0),
            finding.message
        );
    }
    let mut paths = plan
        .month_edits
        .iter()
        .map(|edit| edit.path.clone())
        .collect::<Vec<_>>();
    if let Some((path, _)) = journal_change(plan) {
        paths.push(path);
    }
    paths.sort();
    paths.dedup();
    if !paths.is_empty() {
        println!("Affected: {}", paths.join(", "));
    }
    if report.dry_run {
        for edit in &plan.month_edits {
            println!("{}", text_diff_for(edit));
        }
        if let Some((path, suffix)) = journal_change(plan) {
            println!("{}\n{}", path, journal_diff(&suffix));
        }
    } else if let Some(commit) = &report.commit {
        println!("Committed {} ({})", commit.subject, commit.sha);
        println!(
            "Sync: pre {}, post {}",
            report.sync_pre.unwrap_or("unknown"),
            report.sync_post.unwrap_or("unknown")
        );
    } else {
        println!("No changes; no import commit.");
    }
    for warning in &report.warnings {
        println!("warning: {warning}");
    }
    if plan.counts.new > 0 {
        println!("Next: review the waiting rows with `bob bank list`.");
    } else {
        println!("Next: check the import summary with `bob bank report`.");
    }
}

pub(crate) fn now_stamp(datetime: chrono::NaiveDateTime) -> String {
    let timestamp =
        if let Some(timezone) = crate::native::env::configured_timezone() {
            timezone
                .from_local_datetime(&datetime)
                .single()
                .or_else(|| timezone.from_local_datetime(&datetime).earliest())
                .map(|timestamp| timestamp.with_timezone(&Utc))
        } else {
            Local
                .from_local_datetime(&datetime)
                .single()
                .or_else(|| Local.from_local_datetime(&datetime).earliest())
                .map(|timestamp| timestamp.with_timezone(&Utc))
        }
        .unwrap_or_else(|| datetime.and_utc());
    timestamp.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, process::Command};

    use super::*;
    use crate::native::{
        bank::journal::TxnOutcome, env::TestEnvGuard,
        host_catalog::apply_and_commit,
    };

    const HUB: &str = "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-01\n  categories:\n    groceries: 🛒 Groceries\n    dining: 🍽️ Dining\n---\n# Bank\n";
    const CSV: &str = "11/12/2026,-13.71,*,,\"SMASHBURGER CARD 1234\"\n";

    struct TestVault {
        _temp: tempfile::TempDir,
        path: PathBuf,
    }

    fn environment() -> TestEnvGuard {
        TestEnvGuard::set(&[
            (
                "BOB_CONFIG_FILE",
                Some(OsStr::new("/nonexistent-bob-bank-import-test.yml")),
            ),
            ("BOB_NOW", Some(OsStr::new("2026-11-13T12:00:00"))),
        ])
    }

    fn git(path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn vault() -> TestVault {
        let temp = tempfile::tempdir().expect("temporary vault");
        let path = temp.path().join("vault");
        fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q"]);
        git(&path, &["config", "user.email", "test@example.invalid"]);
        git(&path, &["config", "user.name", "Bank Import Test"]);
        fs::write(path.join("bank.md"), HUB).unwrap();
        git(&path, &["add", "bank.md"]);
        git(&path, &["commit", "-qm", "fixture"]);
        TestVault { _temp: temp, path }
    }

    fn syncable_vault() -> TestVault {
        let vault = vault();
        let remote = vault._temp.path().join("remote.git");
        let output = Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&remote)
            .output()
            .expect("initialize local sync remote");
        assert!(output.status.success());
        let remote = remote.to_string_lossy().into_owned();
        let branch = git(&vault.path, &["branch", "--show-current"]);
        git(&vault.path, &["remote", "add", "origin", &remote]);
        git(
            &vault.path,
            &["push", "--quiet", "--set-upstream", "origin", &branch],
        );
        git(&vault.path, &["remote", "set-head", "origin", &branch]);
        vault
    }

    fn sync_environment(vault: &TestVault) -> TestEnvGuard {
        let lock = vault._temp.path().join("sync.lock");
        let state = vault._temp.path().join("sync-state.json");
        TestEnvGuard::set(&[
            ("BOB_DIR", Some(vault.path.as_os_str())),
            (
                "BOB_CONFIG_FILE",
                Some(OsStr::new("/nonexistent-bob-bank-import-test.yml")),
            ),
            ("BOB_NOW", Some(OsStr::new("2026-11-13T12:00:00"))),
            ("BOB_VAULT_SYNC_LOCK_FILE", Some(lock.as_os_str())),
            ("BOB_VAULT_SYNC_STATE_FILE", Some(state.as_os_str())),
            ("BOB_VAULT_SYNC_NOTIFY", Some(OsStr::new("0"))),
        ])
    }

    fn prepared(label: &str, order: usize, contents: &str) -> PreparedSource {
        PreparedSource {
            label: label.to_owned(),
            statement: source::parse_export(label, contents.as_bytes(), order)
                .expect("synthetic CSV parses"),
        }
    }

    fn prepared_ofx(
        label: &str,
        order: usize,
        amount: &str,
        bank_id: &str,
    ) -> PreparedSource {
        let contents = format!(
            "<OFX><STMTRS><CURDEF>USD</CURDEF><BANKACCTFROM><ACCTTYPE>CHECKING</ACCTTYPE></BANKACCTFROM>\
<BANKTRANLIST><DTSTART>20261101</DTSTART><DTEND>20261113</DTEND><STMTTRN>\
<DTPOSTED>20261112</DTPOSTED><TRNAMT>{amount}</TRNAMT><FITID>{bank_id}</FITID>\
<NAME>TEST CAFE</NAME></STMTTRN></BANKTRANLIST></STMTRS></OFX>"
        );
        PreparedSource {
            label: label.to_owned(),
            statement: source::parse_export(label, contents.as_bytes(), order)
                .expect("synthetic OFX parses"),
        }
    }

    fn planned(
        vault: &Path,
        sources: &[PreparedSource],
    ) -> PlannedTxn<BatchPlan> {
        let (plan, _config, deps, files, appends, verify) = plan_import(
            vault,
            sources,
            Some("checking"),
            "2026-11-13T17:00:00+00:00",
            "test-host",
        )
        .expect("import plan");
        planned_txn(vault, plan, deps, files, appends, verify)
    }

    #[test]
    fn row_and_journal_land_in_one_scoped_import_commit() {
        let _guard = environment();
        let vault = vault();
        let input = prepared("checking.csv", 0, CSV);
        let txn = planned(&vault.path, std::slice::from_ref(&input));
        let dry_after = txn.files[0].updated_target.clone();
        assert_eq!(txn.appends.len(), 1);
        let journal_path = txn.appends[0].target.clone();
        let (commit, plan) = apply_and_commit(&vault.path, &Vec::new(), txn)
            .expect("guarded import");
        let commit = commit.expect("one import commit");
        assert_eq!(commit.subject, "bank: import 1 rows (checking)");
        assert_eq!(
            fs::read_to_string(vault.path.join("bank/2026-11.md")).unwrap(),
            dry_after
        );
        let journal = fs::read_to_string(&journal_path).unwrap();
        assert!(journal.contains("\"kind\":\"bank.import\""));
        assert!(journal.contains("\"kind\":\"bank.txn\""));
        assert!(!journal.contains("1234"), "masked data only: {journal}");
        assert_eq!(plan.counts.new, 1);
        let paths =
            git(&vault.path, &["show", "--name-only", "--format=", "HEAD"]);
        assert!(paths.lines().any(|path| path == "bank/2026-11.md"));
        assert!(paths
            .lines()
            .any(|path| path == ".bob/bank/test-host.jsonl"));
    }

    #[test]
    fn apply_refuses_ignored_journal_before_rows_and_commits_when_allowed() {
        let vault = syncable_vault();
        let _guard = sync_environment(&vault);
        let ignore = "/.bob/bank/*.jsonl\n";
        fs::write(vault.path.join(".gitignore"), ignore).unwrap();
        git(&vault.path, &["add", ".gitignore"]);
        git(&vault.path, &["commit", "-qm", "ignore bank journal"]);
        git(&vault.path, &["push", "--quiet"]);

        let input = prepared("checking.csv", 0, CSV);
        let today = NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
        let refused = apply(
            &vault.path,
            std::slice::from_ref(&input),
            Some("checking"),
            "2026-11-13T17:00:00+00:00",
            today,
            true,
        );
        assert_eq!(
            refused.failure.as_ref().map(|error| error.code),
            Some("bank_import_journal_ignored")
        );
        assert_eq!(refused.exit_code(), 2);
        assert!(!vault.path.join("bank/2026-11.md").exists());
        assert!(!vault.path.join(".bob/bank").exists());
        assert_eq!(
            fs::read_to_string(vault.path.join(".gitignore")).unwrap(),
            ignore
        );

        fs::write(vault.path.join(".gitignore"), "!/.bob/bank/*.jsonl\n")
            .unwrap();
        git(&vault.path, &["add", ".gitignore"]);
        git(&vault.path, &["commit", "-qm", "allow bank journal"]);
        git(&vault.path, &["push", "--quiet"]);
        let applied = apply(
            &vault.path,
            std::slice::from_ref(&input),
            Some("checking"),
            "2026-11-13T17:00:00+00:00",
            today,
            true,
        );
        assert!(applied.failure.is_none(), "{:?}", applied.failure);
        let commit = applied.commit.expect("one scoped import commit");
        assert_eq!(commit.subject, "bank: import 1 rows (checking)");
        let paths =
            git(&vault.path, &["show", "--name-only", "--format=", "HEAD"]);
        assert!(paths.lines().any(|path| path == "bank/2026-11.md"));
        let journal = format!(".bob/bank/{}.jsonl", pref_journal::host_label());
        assert!(paths.lines().any(|path| path == journal));
    }

    #[test]
    fn malformed_cli_source_returns_before_any_sync_or_write() {
        let vault = syncable_vault();
        let _guard = sync_environment(&vault);
        let source_path = vault._temp.path().join("truncated.qfx");
        fs::write(
            &source_path,
            "<OFX><STMTRS><BANKTRANLIST><DTSTART>20261101</DTSTART>\
<DTEND>20261112</DTEND><STMTTRN><TRNAMT>-1.00</TRNAMT><FITID>x</FITID>",
        )
        .unwrap();
        let state = vault._temp.path().join("sync-state.json");
        assert!(!state.exists());
        let code = super::super::cli::run_import(vec![
            "-b".into(),
            vault.path.as_os_str().to_owned(),
            "-f".into(),
            "json".into(),
            source_path.as_os_str().to_owned(),
        ]);
        assert_eq!(code, 2);
        assert!(!state.exists(), "malformed input must not enter vault sync");
        assert!(!vault.path.join("bank/2026-11.md").exists());
        assert!(!vault.path.join(".bob/bank").exists());
        assert_eq!(git(&vault.path, &["status", "--porcelain"]), "");
    }

    #[test]
    fn conflicting_multi_file_apply_refuses_without_rows_or_journal() {
        let vault = syncable_vault();
        let _guard = sync_environment(&vault);
        let sources = [
            prepared_ofx("first.qfx", 0, "-10.00", "same-id"),
            prepared_ofx("second.qfx", 1, "-20.00", "same-id"),
        ];
        let report = apply(
            &vault.path,
            &sources,
            Some("checking"),
            "2026-11-13T17:00:00+00:00",
            NaiveDate::from_ymd_opt(2026, 11, 13).unwrap(),
            true,
        );
        assert_eq!(report.exit_code(), 2);
        let failure = report
            .failure
            .as_ref()
            .expect("conflicting identity refuses");
        assert_eq!(failure.code, "bank_import_identity_conflict");
        assert!(failure.message.contains("second.qfx record 1 field TRNAMT"));
        assert!(!failure.message.contains("same-id"));
        assert!(!vault.path.join("bank/2026-11.md").exists());
        assert!(!vault.path.join(".bob/bank").exists());
        assert_eq!(git(&vault.path, &["status", "--porcelain"]), "");
    }

    #[test]
    fn lock_refusal_precedes_sync_and_planning() {
        let vault = syncable_vault();
        let _guard = sync_environment(&vault);
        let held = crate::native::ob::acquire_lock_quiet_if_held()
            .expect("lock check")
            .expect("test holds the isolated lock");
        let report = apply(
            &vault.path,
            &[prepared("checking.csv", 0, CSV)],
            Some("checking"),
            "2026-11-13T17:00:00+00:00",
            NaiveDate::from_ymd_opt(2026, 11, 13).unwrap(),
            true,
        );
        drop(held);
        assert_eq!(
            report.failure.as_ref().map(|error| error.code),
            Some("lock_busy")
        );
        assert!(!vault.path.join("bank/2026-11.md").exists());
        assert!(!vault.path.join(".bob/bank").exists());
        assert!(!vault._temp.path().join("sync-state.json").exists());
    }

    #[test]
    fn apply_after_presync_uses_the_remote_bank_config() {
        let vault = syncable_vault();
        let _guard = sync_environment(&vault);
        let remote = vault._temp.path().join("remote.git");
        let clone = vault._temp.path().join("remote-writer");
        let output = Command::new("git")
            .args(["clone", "--quiet"])
            .arg(&remote)
            .arg(&clone)
            .output()
            .expect("clone sync remote");
        assert!(output.status.success());
        git(&clone, &["config", "user.email", "test@example.invalid"]);
        git(&clone, &["config", "user.name", "Bank Import Test"]);
        let bank = fs::read_to_string(clone.join("bank.md")).unwrap();
        let updated = bank.replace(
            "    dining: 🍽️ Dining\n---",
            "    dining: 🍽️ Dining\n  rules:\n    - match: SMASHBURGER\n      payee: Synced Cafe\n---",
        );
        assert_ne!(updated, bank);
        fs::write(clone.join("bank.md"), updated).unwrap();
        git(&clone, &["add", "bank.md"]);
        git(&clone, &["commit", "-qm", "update bank naming rule"]);
        git(&clone, &["push", "--quiet"]);

        let report = apply(
            &vault.path,
            &[prepared("checking.csv", 0, CSV)],
            Some("checking"),
            "2026-11-13T17:00:00+00:00",
            NaiveDate::from_ymd_opt(2026, 11, 13).unwrap(),
            true,
        );
        assert!(report.failure.is_none(), "{:?}", report.failure);
        assert_eq!(report.plan.as_ref().unwrap().rows[0].payee, "Synced Cafe");
        assert!(fs::read_to_string(vault.path.join("bank.md"))
            .unwrap()
            .contains("payee: Synced Cafe"));
    }

    #[test]
    fn fixed_clock_dry_run_and_apply_write_identical_month_and_journal_bytes() {
        let vault = syncable_vault();
        let _guard = sync_environment(&vault);
        let source = prepared("checking.csv", 0, CSV);
        let today = NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
        let preview = dry_run(
            &vault.path,
            std::slice::from_ref(&source),
            Some("checking"),
            "2026-11-13T17:00:00+00:00",
            today,
        );
        let preview_plan = preview.plan.expect("dry-run plan");
        let month = preview_plan.month_edits[0].after.clone();
        let journal = preview_plan.journal_events.concat();
        let applied = apply(
            &vault.path,
            std::slice::from_ref(&source),
            Some("checking"),
            "2026-11-13T17:00:00+00:00",
            today,
            true,
        );
        assert!(applied.failure.is_none(), "{:?}", applied.failure);
        assert_eq!(
            fs::read_to_string(vault.path.join("bank/2026-11.md")).unwrap(),
            month
        );
        let journal_path = vault
            .path
            .join(format!(".bob/bank/{}.jsonl", pref_journal::host_label()));
        assert_eq!(fs::read_to_string(journal_path).unwrap(), journal);
    }

    #[test]
    fn post_sync_failure_keeps_the_local_commit_and_journal() {
        use std::os::unix::fs::PermissionsExt;

        let vault = syncable_vault();
        let _guard = sync_environment(&vault);
        let hook = vault._temp.path().join("remote.git/hooks/pre-receive");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

        let report = apply(
            &vault.path,
            &[prepared("checking.csv", 0, CSV)],
            Some("checking"),
            "2026-11-13T17:00:00+00:00",
            NaiveDate::from_ymd_opt(2026, 11, 13).unwrap(),
            true,
        );
        assert!(report.failure.is_none(), "{:?}", report.failure);
        assert!(report.commit.is_some());
        assert_eq!(report.sync_post, Some("failed"));
        assert!(report.warnings.iter().any(|warning| {
            warning.contains("committed locally")
                && warning.contains("run bob vault-sync")
        }));
        assert!(vault.path.join("bank/2026-11.md").exists());
        assert!(vault
            .path
            .join(format!(".bob/bank/{}.jsonl", pref_journal::host_label()))
            .exists());
        assert!(git(&vault.path, &["log", "-1", "--format=%s"])
            .starts_with("bank: import 1 rows (checking)"));
    }

    #[test]
    fn parse_refusal_precedes_any_vault_change() {
        let _guard = environment();
        let vault = vault();
        let good = vault._temp.path().join("good.csv");
        let bad = vault._temp.path().join("bad.csv");
        fs::write(&good, CSV).unwrap();
        fs::write(&bad, "not,a,wells,fargo,profile,extra\n").unwrap();
        let error =
            prepare_sources(&[good, bad]).expect_err("whole batch refuses");
        assert!(error.display().contains("bad.csv"));
        assert!(!vault.path.join("bank").exists());
        assert!(!vault.path.join(".bob/bank").exists());
        assert_eq!(git(&vault.path, &["status", "--porcelain"]), "");
    }

    #[test]
    fn identical_replay_has_no_second_commit_or_journal_event() {
        let _guard = environment();
        let vault = vault();
        let input = prepared("checking.csv", 0, CSV);
        let first = planned(&vault.path, std::slice::from_ref(&input));
        apply_and_commit(&vault.path, &Vec::new(), first)
            .expect("first import");
        let head = git(&vault.path, &["rev-parse", "HEAD"]);

        let replay = planned(&vault.path, std::slice::from_ref(&input));
        assert!(replay.files.is_empty());
        assert!(replay.appends.is_empty());
        assert!(!replay.context.changed);
        let (commit, replayed) =
            apply_and_commit(&vault.path, &Vec::new(), replay)
                .expect("replay is safe");
        assert!(commit.is_none());
        assert_eq!(git(&vault.path, &["rev-parse", "HEAD"]), head);
        assert_eq!(replayed.counts.seen, 1);
    }

    #[test]
    fn retry_after_row_only_crash_backfills_journal_without_duplicate_row() {
        let _guard = environment();
        let vault = vault();
        let input = prepared("checking.csv", 0, CSV);
        let first = planned(&vault.path, std::slice::from_ref(&input));
        let row_id = first
            .context
            .rows
            .first()
            .and_then(|row| row.id.clone())
            .expect("new row has a reserved ID");
        let row_after = first.files[0].updated_target.clone();
        // Model a process exit after the guarded note install but before the
        // host journal append: only the month row reaches Git.
        fs::create_dir_all(vault.path.join("bank")).unwrap();
        fs::write(vault.path.join("bank/2026-11.md"), row_after).unwrap();
        git(&vault.path, &["add", "bank/2026-11.md"]);
        git(&vault.path, &["commit", "-qm", "interrupted row install"]);

        let retry = planned(&vault.path, std::slice::from_ref(&input));
        assert!(retry.files.is_empty(), "recovery must not rewrite the row");
        assert_eq!(retry.context.counts.new, 0);
        assert_eq!(retry.context.counts.seen, 1);
        assert!(retry.context.month_edits.is_empty());
        assert!(retry.context.journal_events.iter().any(|event| {
            event.contains("\"kind\":\"bank.txn\"")
                && event.contains(&format!("\"id\":\"{row_id}\""))
        }));
        let (commit, _) = apply_and_commit(&vault.path, &Vec::new(), retry)
            .expect("recovery appends journal only");
        assert!(commit.is_some());
        let note =
            fs::read_to_string(vault.path.join("bank/2026-11.md")).unwrap();
        assert_eq!(note.matches(&format!("^bank-{row_id}")).count(), 1);
        let snapshot = journal::load(&vault.path).expect("journal loads");
        assert_eq!(snapshot.transactions.len(), 1);
        assert_eq!(
            snapshot.transactions.values().next().unwrap().outcome,
            TxnOutcome::Written
        );
    }

    #[test]
    fn newly_appeared_registry_note_refuses_before_import_writes() {
        let _guard = environment();
        let vault = vault();
        let input = prepared("checking.csv", 0, CSV);
        let txn = planned(&vault.path, std::slice::from_ref(&input));
        fs::write(vault.path.join("new-record.md"), "---\ntitle: Late\n---\n")
            .unwrap();

        let error = apply_and_commit(&vault.path, &Vec::new(), txn)
            .expect_err("registry membership changed");
        assert_eq!(error.code, "stale_note");
        assert!(!vault.path.join("bank").exists());
        assert!(!vault.path.join(".bob/bank").exists());
        assert_eq!(
            git(&vault.path, &["status", "--porcelain"]),
            "?? new-record.md"
        );
    }

    #[test]
    fn nonlegacy_restaurant_directory_links_and_guards_its_inputs() {
        let vault = vault();
        let config_path = vault._temp.path().join("config.yml");
        fs::write(
            &config_path,
            "records:\n  eat:\n    dir: restaurants\n    label: Restaurant\n    emoji: 🍽️\n    fields: {}\n",
        )
        .unwrap();
        let _guard = TestEnvGuard::set(&[
            ("BOB_CONFIG_FILE", Some(config_path.as_os_str())),
            ("BOB_NOW", Some(OsStr::new("2026-11-13T12:00:00"))),
        ]);
        fs::create_dir_all(vault.path.join("restaurants")).unwrap();
        fs::write(
            vault.path.join("restaurants/test_cafe.md"),
            "---\ntitle: Test Cafe\n---\n",
        )
        .unwrap();

        let first =
            prepared("cafe.csv", 0, "11/12/2026,-13.71,*,,TST*TEST CAFE\n");
        let txn = planned(&vault.path, std::slice::from_ref(&first));
        assert_eq!(
            txn.context.rows[0].links,
            vec!["[[restaurants/test_cafe|🍽️]]".to_owned()]
        );
        let (commit, _) = apply_and_commit(&vault.path, &Vec::new(), txn)
            .expect("unchanged configured restaurant registry applies");
        assert!(commit.is_some());
        let note =
            fs::read_to_string(vault.path.join("bank/2026-11.md")).unwrap();
        assert!(note.contains("[[restaurants/test_cafe|🍽️]]"));

        let changed_source = prepared(
            "another-cafe.csv",
            0,
            "11/13/2026,-4.25,*,,TST*NEW CAFE\n",
        );
        let stale = planned(&vault.path, std::slice::from_ref(&changed_source));
        let original_rows =
            fs::read(vault.path.join("bank/2026-11.md")).unwrap();
        fs::write(
            vault.path.join("restaurants/test_cafe.md"),
            "---\ntitle: Renamed Cafe\n---\n",
        )
        .unwrap();
        let error = apply_and_commit(&vault.path, &Vec::new(), stale)
            .expect_err("changed restaurant input makes the plan stale");
        assert_eq!(error.code, "stale_note");
        assert_eq!(
            fs::read(vault.path.join("bank/2026-11.md")).unwrap(),
            original_rows
        );

        let stale_membership =
            planned(&vault.path, std::slice::from_ref(&changed_source));
        fs::write(
            vault.path.join("restaurants/new_cafe.md"),
            "---\ntitle: New Cafe\n---\n",
        )
        .unwrap();
        let error =
            apply_and_commit(&vault.path, &Vec::new(), stale_membership)
                .expect_err("new restaurant membership makes the plan stale");
        assert_eq!(error.code, "stale_note");
        assert_eq!(
            fs::read(vault.path.join("bank/2026-11.md")).unwrap(),
            original_rows
        );
    }

    #[test]
    fn malformed_journal_is_reported_without_echoing_its_bytes() {
        let _guard = environment();
        let vault = vault();
        let journal_dir = vault.path.join(".bob/bank");
        fs::create_dir_all(&journal_dir).unwrap();
        fs::write(
            journal_dir.join("other-host.jsonl"),
            b"raw-private-fitid-987654321\n",
        )
        .unwrap();
        let input = prepared("checking.csv", 0, CSV);
        let report = dry_run(
            &vault.path,
            &[input],
            Some("checking"),
            "2026-11-13T17:00:00+00:00",
            NaiveDate::from_ymd_opt(2026, 11, 13).unwrap(),
        );
        let value = report.to_json();
        assert!(value["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| { finding["code"] == "bank_journal_malformed" }));
        assert!(!value.to_string().contains("raw-private-fitid-987654321"));
        assert_eq!(value["issues"]["warnings"], 1);
    }

    #[test]
    fn json_contract_has_additive_import_summary_and_exact_dry_run_diff() {
        let _guard = environment();
        let vault = vault();
        let source = prepared("checking.csv", 0, CSV);
        let report = dry_run(
            &vault.path,
            &[source],
            Some("checking"),
            "2026-11-13T17:00:00+00:00",
            NaiveDate::from_ymd_opt(2026, 11, 13).unwrap(),
        );
        let value = report.to_json();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["dry_run"], true);
        assert_eq!(value["changed"], true);
        assert_eq!(value["files"][0]["source"], "csv");
        assert_eq!(value["files"][0]["new"], 1);
        assert!(value["changes"].as_array().unwrap().iter().any(|change| {
            change["path"] == "bank/2026-11.md"
                && change["diff"]
                    .as_str()
                    .is_some_and(|diff| diff.contains("^bank-"))
        }));
        let journal_path =
            format!(".bob/bank/{}.jsonl", pref_journal::host_label());
        assert!(value["changes"].as_array().unwrap().iter().any(|change| {
            change["path"] == journal_path
                && change["diff"]
                    .as_str()
                    .is_some_and(|diff| diff.contains("bank.txn"))
        }));
        assert!(!vault.path.join("bank").exists());
        assert!(!vault.path.join(".bob/bank").exists());
    }
}
