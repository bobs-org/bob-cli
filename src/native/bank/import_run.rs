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
    deps.insert_directory(
        PathBuf::from("eat"),
        Some("md"),
        restaurant_dir_entries,
    );
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
    let mut dir_entries = Vec::new();
    if let Some(kind) = kind {
        let record_type = kind.record_type();
        let directory = &record_type.dir;
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
                entries.push(RestaurantEntry {
                    title: title.clone(),
                    stem: record.stem.clone(),
                    emoji: record_type.emoji.clone(),
                });
            }
        }
    }
    Ok((
        entries,
        deps,
        registry_dependencies,
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
    failure(
        "bank_import_plan",
        refusal.message(),
        "resolve the listed conflict and retry",
    )
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

    fn prepared(label: &str, order: usize, contents: &str) -> PreparedSource {
        PreparedSource {
            label: label.to_owned(),
            statement: source::parse_export(label, contents.as_bytes(), order)
                .expect("synthetic CSV parses"),
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
