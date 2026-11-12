//! `bob albums update`: guarded album schema update execution.
//!
//! Dry run uses only the read-only planner: no lock, no Git or sync
//! commands, and no vault or cache writes. A schema no-op returns before
//! `run_locked` and triggers neither sync nor index repair. A changed
//! apply re-plans fresh inside `run_locked` and never applies preflight
//! bytes after the pre-sync.

use std::{
    cell::RefCell,
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;
use serde_json::{json, Value};
use similar::TextDiff;

use super::{
    ops::{OpError, OpResult},
    plan::{plan_update, UpdatePlan, UpdatePlanError},
    protocol::{parse_envelope, Envelope, Op, ProtocolError, Restoration},
    spans::RestoredOp,
};
use crate::native::{
    albums::{
        index::IndexState,
        registry::{AlbumRegistry, DiscoveryHealth},
        report::schema_fingerprint,
        write::plan_album_write,
        AlbumIssueSeverity,
    },
    capture::StagedTextFile,
    config,
    host_catalog::{self, CommitInfo, PlannedTxn, TxnResult},
    notebooks::snapshot::VaultSnapshot,
    ob,
    records::catalog::RecordCatalog,
    style::Styler,
    vault_rewrite::TxnFailure,
};

pub(crate) const SCHEMA_VERSION: u32 = 1;

/// Typed command failure: semantic refusals exit 2, runtime failures exit 1.
#[derive(Debug, Clone)]
pub(crate) struct UpdateFailure {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
    pub(crate) op_index: Option<usize>,
    pub(crate) field: Option<String>,
    pub(crate) choice: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) line: Option<usize>,
    pub(crate) key_path: Option<String>,
    pub(crate) affected: Option<usize>,
    pub(crate) exit: i32,
}

impl UpdateFailure {
    fn semantic(
        code: impl Into<String>,
        message: impl Into<String>,
        hint: Option<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint,
            op_index: None,
            field: None,
            choice: None,
            path: None,
            line: None,
            key_path: None,
            affected: None,
            exit: 2,
        }
    }

    fn runtime(
        code: impl Into<String>,
        message: impl Into<String>,
        hint: Option<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint,
            op_index: None,
            field: None,
            choice: None,
            path: None,
            line: None,
            key_path: None,
            affected: None,
            exit: 1,
        }
    }
}

/// What an update run planned and (unless a dry run) did.
pub(crate) struct UpdateReport {
    pub(crate) dry_run: bool,
    pub(crate) stem: String,
    pub(crate) path: String,
    pub(crate) changed: bool,
    pub(crate) before_fingerprint: String,
    pub(crate) after_fingerprint: String,
    pub(crate) consequence: String,
    pub(crate) group_before: Option<String>,
    pub(crate) group_after: Option<String>,
    pub(crate) per_op: Vec<OpResult>,
    pub(crate) derived_keys: Vec<Option<String>>,
    pub(crate) inverse_ops: Vec<RestoredOp>,
    pub(crate) changes: Vec<ChangedFile>,
    pub(crate) commit: Option<CommitInfo>,
    pub(crate) sync_pre: Option<&'static str>,
    pub(crate) sync_post: Option<&'static str>,
    pub(crate) warnings: Vec<String>,
    pub(crate) failure: Option<UpdateFailure>,
}

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

fn today() -> NaiveDate {
    crate::native::env::current_datetime().date()
}

fn protocol_failure(error: &ProtocolError) -> UpdateFailure {
    let mut failure = UpdateFailure::semantic(
        error.code,
        error.to_string(),
        error.hint.clone(),
    );
    failure.op_index = error.op_index;
    failure
}

fn op_failure(error: &OpError) -> UpdateFailure {
    let mut failure = UpdateFailure::semantic(
        error.code,
        error.to_string(),
        error.hint.clone(),
    );
    failure.op_index = error.op_index;
    failure.field = error.field.clone();
    failure.choice = error.choice.clone();
    failure.key_path = error.key_path.clone();
    failure.affected = error.affected;
    failure
}

fn plan_failure(error: &UpdatePlanError) -> UpdateFailure {
    // Infrastructure guards exit 1; every other planner refusal exits 2.
    const RUNTIME: &[&str] = &[
        "discovery_incomplete",
        "dependency_read_failed",
        "album_overlay_missing",
        "vault_mismatch",
        "lock_busy",
        "not_a_worktree",
        "sync_failed",
        "io",
        "verify_failed",
        "stale_note",
        "dirty_paths",
        "changed_after_planning",
        "read_failed",
    ];
    let exit = if RUNTIME.contains(&error.code) { 1 } else { 2 };
    UpdateFailure {
        code: error.code.to_owned(),
        message: error.to_string(),
        hint: error.hint.clone(),
        op_index: error.op_index,
        field: error.field.clone(),
        choice: error.choice.clone(),
        path: error.path.clone(),
        line: error.line,
        key_path: error.key_path.clone(),
        affected: error.affected,
        exit,
    }
}

fn txn_failure(
    error: &TxnFailure,
    typed: Option<&UpdatePlanError>,
) -> UpdateFailure {
    if let Some(plan) = typed {
        return plan_failure(plan);
    }
    const RUNTIME: &[&str] = &[
        "vault_mismatch",
        "lock_busy",
        "not_a_worktree",
        "sync_failed",
        "io",
        "verify_failed",
        "stale_note",
        "dirty_paths",
    ];
    let exit = if RUNTIME.contains(&error.code) { 1 } else { 2 };
    UpdateFailure {
        code: error.code.to_owned(),
        message: error.message.clone(),
        hint: error.hint.clone(),
        op_index: None,
        field: None,
        choice: None,
        path: None,
        line: None,
        key_path: None,
        affected: None,
        exit,
    }
}

/// Read the JSON envelope: `-i FILE`, `-i -` for stdin, omitted means stdin.
pub(crate) fn read_envelope_text(
    input: Option<&OsString>,
) -> Result<String, UpdateFailure> {
    match input {
        None => read_stdin(),
        Some(raw) if raw == "-" => read_stdin(),
        Some(raw) => {
            let path = PathBuf::from(raw);
            fs::read_to_string(&path).map_err(|error| {
                UpdateFailure::semantic(
                    "envelope_unreadable",
                    format!("could not read input {}: {error}", path.display()),
                    Some("pass a JSON file with -i FILE or pipe the envelope on stdin".to_owned()),
                )
            })
        }
    }
}

fn read_stdin() -> Result<String, UpdateFailure> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).map_err(|error| {
        UpdateFailure::semantic(
            "envelope_unreadable",
            format!("could not read the envelope from stdin: {error}"),
            Some("pass a JSON file with -i FILE or pipe the envelope on stdin".to_owned()),
        )
    })?;
    Ok(text)
}

fn load_complete(
    vault: &Path,
) -> Result<(VaultSnapshot, AlbumRegistry), UpdateFailure> {
    let config_path = config::config_path();
    let snapshot = VaultSnapshot::load_with_config(vault, &config_path);
    if let Some(first) = snapshot.io_errors.first() {
        let more = snapshot.io_errors.len() - 1;
        let suffix = if more > 0 {
            format!(" (+{more} more)")
        } else {
            String::new()
        };
        return Err(UpdateFailure::runtime(
            "io",
            format!("the vault scan was incomplete: {first}{suffix}"),
            Some(
                "fix the unreadable paths, then retry; nothing was written"
                    .to_owned(),
            ),
        ));
    }
    let registry = AlbumRegistry::from_snapshot(vault, &config_path, &snapshot);
    if !matches!(registry.health, DiscoveryHealth::Complete) {
        return Err(UpdateFailure::runtime(
            "discovery_incomplete",
            "the vault scan is incomplete".to_owned(),
            Some("resolve the read errors, then retry the preview".to_owned()),
        ));
    }
    Ok((snapshot, registry))
}

fn report_of_plan(dry_run: bool, plan: &UpdatePlan) -> UpdateReport {
    let changes = plan
        .edit
        .as_ref()
        .map(|edit| ChangedFile {
            path: slash(&plan.path),
            before: edit.before.clone(),
            after: edit.after.clone(),
        })
        .into_iter()
        .collect();
    UpdateReport {
        dry_run,
        stem: plan.stem.clone(),
        path: slash(&plan.path),
        changed: plan.changed,
        before_fingerprint: plan.before_fingerprint.clone(),
        after_fingerprint: plan.after_fingerprint.clone(),
        consequence: plan.op_outcome.consequence.clone(),
        group_before: plan.op_outcome.group_before.clone(),
        group_after: plan.op_outcome.group_after.clone(),
        per_op: plan.op_outcome.per_op.clone(),
        derived_keys: plan.op_outcome.derived_keys.clone(),
        inverse_ops: plan.schema_edit.inverse_ops.clone(),
        changes,
        commit: None,
        sync_pre: None,
        sync_post: None,
        warnings: Vec::new(),
        failure: None,
    }
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Read-only plan: no lock, no Git or sync commands, no vault writes.
pub(crate) fn dry_run(
    vault: &Path,
    stem: &str,
    envelope_text: &str,
) -> UpdateReport {
    let envelope = match parse_envelope(envelope_text) {
        Ok(envelope) => envelope,
        Err(error) => {
            return failed_report(true, stem, protocol_failure(&error));
        }
    };
    let (snapshot, registry) = match load_complete(vault) {
        Ok(loaded) => loaded,
        Err(failure) => return failed_report(true, stem, failure),
    };
    let config_path = config::config_path();
    match plan_update(
        vault,
        &config_path,
        &snapshot,
        &registry,
        stem,
        &envelope,
        today(),
    ) {
        Ok(plan) => report_of_plan(true, &plan),
        Err(error) => failed_report(true, stem, plan_failure(&error)),
    }
}

fn failed_report(
    dry_run: bool,
    stem: &str,
    failure: UpdateFailure,
) -> UpdateReport {
    UpdateReport {
        dry_run,
        stem: stem.to_owned(),
        path: format!("{stem}.md"),
        changed: false,
        before_fingerprint: String::new(),
        after_fingerprint: String::new(),
        consequence: String::new(),
        group_before: None,
        group_after: None,
        per_op: Vec::new(),
        derived_keys: Vec::new(),
        inverse_ops: Vec::new(),
        changes: Vec::new(),
        commit: None,
        sync_pre: None,
        sync_post: None,
        warnings: Vec::new(),
        failure: Some(failure),
    }
}

fn short_summary(ops: &[Op], per_op: &[OpResult]) -> String {
    if ops.is_empty() {
        return "no-op".to_owned();
    }
    if ops.len() == 1 {
        return short_op(&ops[0], per_op.first());
    }
    let first = short_op(&ops[0], per_op.first());
    format!("{} ops ({first} +{})", ops.len(), ops.len() - 1)
}

fn short_op(op: &Op, result: Option<&OpResult>) -> String {
    let _ = result;
    match op {
        Op::AlbumSet { .. } => "album.set".to_owned(),
        Op::FieldAdd { label, .. } => format!("field.add {label}"),
        Op::FieldSet { field, .. } => format!("field.set {field}"),
        Op::FieldMove { field, .. } => format!("field.move {field}"),
        Op::FieldRemove { field } => format!("field.remove {field}"),
        Op::ChoiceAdd { field, label, .. } => {
            format!("choice.add {field}/{label}")
        }
        Op::ChoiceSet { field, value, .. } => {
            format!("choice.set {field}/{value}")
        }
        Op::ChoiceMove { field, value, .. } => {
            format!("choice.move {field}/{value}")
        }
        Op::ChoiceRemove { field, value } => {
            format!("choice.remove {field}/{value}")
        }
    }
}

struct ApplyContext {
    stem: String,
    path: PathBuf,
    before_fingerprint: String,
    after_fingerprint: String,
    consequence: String,
    group_before: Option<String>,
    group_after: Option<String>,
    per_op: Vec<OpResult>,
    derived_keys: Vec<Option<String>>,
    inverse_ops: Vec<RestoredOp>,
    changes: Vec<ChangedFile>,
    changed: bool,
}

#[allow(dead_code)]
fn planned_txn(_vault: &Path, _plan: &UpdatePlan) -> PlannedTxn<ApplyContext> {
    unreachable!("changed applies build their transaction inside run_locked");
}

fn staged_subject(stem: &str, ops: &[Op], per_op: &[OpResult]) -> String {
    format!("albums: update {stem}.md ({})", short_summary(ops, per_op))
}

/// Re-read the vault and report anything the install did not leave exact:
/// the edited schema parses, its fingerprint matches, and the regenerated
/// index is current. A mismatch rolls the transaction back.
fn verify_installed(
    vault: &Path,
    relative: &Path,
    expected_after: &str,
    expected_fingerprint: &str,
) -> Result<(), String> {
    let actual = fs::read_to_string(vault.join(relative)).map_err(|error| {
        format!("could not re-read {}: {error}", slash(relative))
    })?;
    if actual != *expected_after {
        return Err(format!(
            "{} differs from the plan after the write",
            slash(relative)
        ));
    }
    let config_path = config::config_path();
    let snapshot = VaultSnapshot::load_with_config(vault, &config_path);
    if let Some(error) = snapshot.io_errors.first() {
        return Err(error.clone());
    }
    let registry = AlbumRegistry::from_snapshot(vault, &config_path, &snapshot);
    let Some(entry) = registry
        .albums
        .iter()
        .find(|entry| Path::new(&entry.declaration.path) == relative)
    else {
        return Err(format!("{} is missing after the write", slash(relative)));
    };
    if entry
        .findings
        .iter()
        .any(|finding| finding.severity == AlbumIssueSeverity::Error)
    {
        return Err(format!(
            "{} has registry errors after the write",
            slash(relative)
        ));
    }
    let Some(record_type) = entry.declaration.record_type.as_ref() else {
        return Err(format!(
            "{} has no valid record schema after the write",
            slash(relative)
        ));
    };
    let fingerprint =
        schema_fingerprint(record_type, entry.declaration.plural.as_deref());
    if fingerprint != *expected_fingerprint {
        return Err(format!(
            "{} fingerprint changed after the write (expected {expected_fingerprint}, found {fingerprint})",
            slash(relative)
        ));
    }
    let mut catalog = RecordCatalog::new(vault);
    let folder = catalog.folder(&entry.declaration.stem);
    let index = plan_album_write(&entry.declaration, &actual, folder, &[]);
    if index.state == IndexState::Damaged {
        return Err(format!(
            "{} index is damaged after the write",
            slash(relative)
        ));
    }
    if index.state.as_str() != "current" {
        return Err(format!(
            "{} index is not current after the write",
            slash(relative)
        ));
    }
    Ok(())
}

/// Standalone real run. A schema no-op returns before `run_locked` and
/// triggers neither sync nor index repair.
pub(crate) fn run_standalone(
    vault: &Path,
    stem: &str,
    envelope_text: &str,
    json_mode: bool,
) -> UpdateReport {
    let envelope: Envelope = match parse_envelope(envelope_text) {
        Ok(envelope) => envelope,
        Err(error) => {
            return failed_report(false, stem, protocol_failure(&error))
        }
    };
    // Preflight: surface invalid requests and semantic refusals without a
    // lock, and detect the no-op path before any sync lifecycle.
    let preflight = match load_complete(vault) {
        Ok((snapshot, registry)) => {
            let config_path = config::config_path();
            plan_update(
                vault,
                &config_path,
                &snapshot,
                &registry,
                stem,
                &envelope,
                today(),
            )
        }
        Err(failure) => return failed_report(false, stem, failure),
    };
    let preflight = match preflight {
        Ok(plan) => plan,
        Err(error) => return failed_report(false, stem, plan_failure(&error)),
    };
    if !preflight.changed {
        return report_of_plan(false, &preflight);
    }
    let expect = envelope.expect.clone();
    let ops_for_subject = envelope.ops.clone();
    let typed: RefCell<Option<UpdatePlanError>> = RefCell::new(None);
    let child_env = ob::child_env();
    let result = host_catalog::run_locked(
        vault,
        &child_env,
        json_mode,
        "album",
        || {
            let config_path = config::config_path();
            let snapshot = VaultSnapshot::load_with_config(vault, &config_path);
            if let Some(first) = snapshot.io_errors.first() {
                return Err(TxnFailure::new(
                "io",
                format!("the vault scan was incomplete: {first}"),
                Some("fix the unreadable paths, then retry; nothing was written".to_owned()),
            ));
            }
            let registry =
                AlbumRegistry::from_snapshot(vault, &config_path, &snapshot);
            let fresh = UpdatePlanForTxn::fresh(
                vault,
                &config_path,
                &snapshot,
                &registry,
                stem,
                &envelope,
            );
            match fresh {
                Ok(plan) => {
                    let subject = staged_subject(
                        &plan.stem,
                        &ops_for_subject,
                        &plan.op_outcome.per_op,
                    );
                    let files = plan
                        .edit
                        .as_ref()
                        .map(|edit| StagedTextFile {
                            target: vault.join(&plan.path),
                            target_existed: true,
                            original_target: edit.before.clone(),
                            updated_target: edit.after.clone(),
                        })
                        .into_iter()
                        .collect::<Vec<_>>();
                    let context = ApplyContext {
                        stem: plan.stem.clone(),
                        path: plan.path.clone(),
                        before_fingerprint: plan.before_fingerprint.clone(),
                        after_fingerprint: plan.after_fingerprint.clone(),
                        consequence: plan.op_outcome.consequence.clone(),
                        group_before: plan.op_outcome.group_before.clone(),
                        group_after: plan.op_outcome.group_after.clone(),
                        per_op: plan.op_outcome.per_op.clone(),
                        derived_keys: plan.op_outcome.derived_keys.clone(),
                        inverse_ops: plan.schema_edit.inverse_ops.clone(),
                        changes: plan
                            .edit
                            .as_ref()
                            .map(|edit| ChangedFile {
                                path: slash(&plan.path),
                                before: edit.before.clone(),
                                after: edit.after.clone(),
                            })
                            .into_iter()
                            .collect(),
                        changed: plan.changed,
                    };
                    if !plan.changed {
                        return Ok(PlannedTxn {
                            subject,
                            files: Vec::new(),
                            appends: Vec::new(),
                            deps: plan.deps.clone(),
                            verify: Box::new(|| Ok(())),
                            context,
                            kind: "album",
                        });
                    }
                    // Guard against a preflight/replan mismatch: the fresh plan
                    // must still expect the caller's fingerprint.
                    if plan.before_fingerprint != expect {
                        *typed.borrow_mut() = Some(UpdatePlanError {
                        code: "stale_schema",
                        message: format!(
                            "schema fingerprint changed: expected {expect}, current {}",
                            plan.before_fingerprint
                        ),
                        hint: Some("refresh the album report and preview the operation again".to_owned()),
                        path: None,
                        line: None,
                        key_path: None,
                        affected: None,
                        op_index: None,
                        field: None,
                        choice: None,
                    });
                        let snapshot = typed.borrow();
                        let error = snapshot.as_ref().expect("typed error");
                        return Err(TxnFailure::new(
                            error.code,
                            error.message.clone(),
                            error.hint.clone(),
                        ));
                    }
                    let after = plan.after.clone();
                    let after_fingerprint = plan.after_fingerprint.clone();
                    let relative = plan.path.clone();
                    let vault_path = vault.to_path_buf();
                    Ok(PlannedTxn {
                        subject,
                        files,
                        appends: Vec::new(),
                        deps: plan.deps.clone(),
                        verify: Box::new(move || {
                            verify_installed(
                                &vault_path,
                                &relative,
                                &after,
                                &after_fingerprint,
                            )
                        }),
                        context,
                        kind: "album",
                    })
                }
                Err(error) => {
                    let failure = TxnFailure::new(
                        error.code,
                        error.message.clone(),
                        error.hint.clone(),
                    );
                    *typed.borrow_mut() = Some(error);
                    Err(failure)
                }
            }
        },
    );
    match result {
        Ok(result) => report_of_txn(false, &expect, result),
        Err(failure) => {
            let typed_error = typed.borrow().clone();
            let mut report = failed_report(
                false,
                stem,
                txn_failure(&failure, typed_error.as_ref()),
            );
            // Preserve the preflight identity for refusal output.
            report.before_fingerprint = preflight.before_fingerprint.clone();
            report.after_fingerprint = preflight.after_fingerprint.clone();
            report
        }
    }
}

struct UpdatePlanForTxn;

impl UpdatePlanForTxn {
    fn fresh(
        vault: &Path,
        config_path: &Path,
        snapshot: &VaultSnapshot,
        registry: &AlbumRegistry,
        stem: &str,
        envelope: &Envelope,
    ) -> Result<UpdatePlan, UpdatePlanError> {
        plan_update(
            vault,
            config_path,
            snapshot,
            registry,
            stem,
            envelope,
            today(),
        )
    }
}

fn report_of_txn(
    dry_run: bool,
    _expect: &str,
    result: TxnResult<ApplyContext>,
) -> UpdateReport {
    let context = result.context;
    let mut warnings = Vec::new();
    if let Some(warning) = result.post_sync_warning {
        warnings.push(warning);
    }
    UpdateReport {
        dry_run,
        stem: context.stem.clone(),
        path: slash(&context.path),
        changed: context.changed,
        before_fingerprint: context.before_fingerprint.clone(),
        after_fingerprint: context.after_fingerprint.clone(),
        consequence: context.consequence.clone(),
        group_before: context.group_before.clone(),
        group_after: context.group_after.clone(),
        per_op: context.per_op.clone(),
        derived_keys: context.derived_keys.clone(),
        inverse_ops: context.inverse_ops.clone(),
        changes: context.changes.clone(),
        commit: result.commit,
        sync_pre: Some(result.sync_pre),
        sync_post: Some(result.sync_post),
        warnings,
        failure: None,
    }
}

fn op_to_json(op: &Op) -> Value {
    let name = op.name();
    match op {
        Op::AlbumSet { set, unset } => {
            let mut payload = json!({"op": name});
            if !set.is_empty() {
                payload["set"] = Value::Object(
                    set.iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                );
            }
            if !unset.is_empty() {
                payload["unset"] = Value::Array(
                    unset
                        .iter()
                        .map(|item| Value::String(item.clone()))
                        .collect(),
                );
            }
            payload
        }
        Op::FieldAdd {
            label,
            kind,
            list,
            suggest,
            position,
            choices,
            restore_key,
        } => {
            let mut payload = json!({"op": name, "label": label});
            if kind != "text" {
                payload["kind"] = Value::String(kind.clone());
            }
            if *list {
                payload["list"] = Value::Bool(true);
            }
            if *suggest {
                payload["suggest"] = Value::Bool(true);
            }
            if let Some(position) = position {
                payload["position"] = Value::from(*position as u64);
            }
            if !choices.is_empty() {
                payload["choices"] = Value::Array(
                    choices
                        .iter()
                        .map(|choice| {
                            let mut entry = json!({"label": choice.label});
                            if let Some(emoji) = &choice.emoji {
                                entry["emoji"] = Value::String(emoji.clone());
                            }
                            entry
                        })
                        .collect(),
                );
            }
            if let Some(key) = restore_key {
                payload["restore_key"] = Value::String(key.clone());
            }
            payload
        }
        Op::FieldSet { field, set, unset } => {
            let mut payload = json!({"op": name, "field": field});
            if !set.is_empty() {
                payload["set"] = Value::Object(
                    set.iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                );
            }
            if !unset.is_empty() {
                payload["unset"] = Value::Array(
                    unset
                        .iter()
                        .map(|item| Value::String(item.clone()))
                        .collect(),
                );
            }
            payload
        }
        Op::FieldMove { field, position } => {
            json!({"op": name, "field": field, "position": position})
        }
        Op::FieldRemove { field } => {
            json!({"op": name, "field": field})
        }
        Op::ChoiceAdd {
            field,
            label,
            emoji,
            position,
            restore_value,
        } => {
            let mut payload =
                json!({"op": name, "field": field, "label": label});
            if let Some(emoji) = emoji {
                payload["emoji"] = Value::String(emoji.clone());
            }
            if let Some(position) = position {
                payload["position"] = Value::from(*position as u64);
            }
            if let Some(value) = restore_value {
                payload["restore_value"] = Value::String(value.clone());
            }
            payload
        }
        Op::ChoiceSet { field, value, set } => {
            let mut payload =
                json!({"op": name, "field": field, "value": value});
            payload["set"] = Value::Object(
                set.iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            );
            payload
        }
        Op::ChoiceMove {
            field,
            value,
            position,
        } => {
            json!({"op": name, "field": field, "value": value, "position": position})
        }
        Op::ChoiceRemove { field, value } => {
            json!({"op": name, "field": field, "value": value})
        }
    }
}

fn restored_op_to_json(op: &RestoredOp) -> Value {
    let mut payload = op_to_json(&op.op);
    if let Some(restoration) = &op.restore {
        payload["restore"] = json!({
            "spans": restoration.spans.iter().map(|span| {
                json!({
                    "key_path": span.key_path,
                    "expected": span.expected.clone().map(Value::String).unwrap_or(Value::Null),
                    "prior": span.prior.clone().map(Value::String).unwrap_or(Value::Null),
                })
            }).collect::<Vec<_>>(),
        });
    }
    payload
}

fn restoration_to_json(restoration: &Restoration) -> Value {
    json!({
        "spans": restoration.spans.iter().map(|span| {
            json!({
                "key_path": span.key_path,
                "expected": span.expected.clone().map(Value::String).unwrap_or(Value::Null),
                "prior": span.prior.clone().map(Value::String).unwrap_or(Value::Null),
            })
        }).collect::<Vec<_>>(),
    })
}

#[allow(dead_code)]
fn envelope_inverse_to_json(envelope: &Envelope) -> Value {
    let _ = restoration_to_json;
    let _ = envelope.restorations.len();
    Value::Null
}

impl UpdateReport {
    pub(crate) fn exit_code(&self) -> i32 {
        match &self.failure {
            None => 0,
            Some(failure) => failure.exit,
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        let mut payload = json!({
            "schema_version": SCHEMA_VERSION,
            "ok": self.failure.is_none(),
            "command": "update",
            "dry_run": self.dry_run,
            "stem": self.stem,
            "path": self.path,
            "changed": self.changed,
            "before_fingerprint": self.before_fingerprint,
            "after_fingerprint": self.after_fingerprint,
            "consequence": self.consequence,
            "group_before": self.group_before.clone().map(Value::String).unwrap_or(Value::Null),
            "group_after": self.group_after.clone().map(Value::String).unwrap_or(Value::Null),
            "derived_keys": self.derived_keys.iter().map(|key| key.clone().map(Value::String).unwrap_or(Value::Null)).collect::<Vec<_>>(),
            "per_op": self.per_op.iter().map(|result| {
                json!({
                    "index": result.op_index,
                    "op": result.op_name,
                    "summary": result.summary,
                    "derived_key": result.derived_key.clone().map(Value::String).unwrap_or(Value::Null),
                    "filled": result.filled.map(|count| Value::from(count as u64)).unwrap_or(Value::Null),
                    "count": result.count.map(|count| Value::from(count as u64)).unwrap_or(Value::Null),
                    "newly_invalid": result.newly_invalid,
                })
            }).collect::<Vec<_>>(),
            "changes": self.changes.iter().map(|change| {
                let mut entry = json!({"path": change.path, "action": "update"});
                if self.dry_run {
                    entry["diff"] = Value::String(change.diff());
                }
                entry
            }).collect::<Vec<_>>(),
            "inverse_ops": self.inverse_ops.iter().map(restored_op_to_json).collect::<Vec<_>>(),
            "commit": commit_json(self.commit.as_ref()),
            "sync": sync_json(self.sync_pre, self.sync_post),
            "warnings": self.warnings,
            "error": Value::Null,
        });
        if let Some(failure) = &self.failure {
            payload["error"] = json!({
                "code": failure.code,
                "message": failure.message,
                "hint": failure.hint.clone().map(Value::String).unwrap_or(Value::Null),
                "op_index": failure.op_index.map(|index| Value::from(index as u64)).unwrap_or(Value::Null),
                "field": failure.field.clone().map(Value::String).unwrap_or(Value::Null),
                "choice": failure.choice.clone().map(Value::String).unwrap_or(Value::Null),
                "path": failure.path.clone().map(Value::String).unwrap_or(Value::Null),
                "line": failure.line.map(|line| Value::from(line as u64)).unwrap_or(Value::Null),
                "key_path": failure.key_path.clone().map(Value::String).unwrap_or(Value::Null),
                "affected": failure.affected.map(|count| Value::from(count as u64)).unwrap_or(Value::Null),
            });
        }
        payload
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

/// Human rendering for `bob albums update`.
pub(crate) fn print_human(report: &UpdateReport) {
    let styler = Styler::detect();
    if let Some(failure) = &report.failure {
        eprintln!("{} {}", styler.red("✗"), failure.message);
        if let Some(hint) = &failure.hint {
            eprintln!("  {}", styler.dim(hint));
        }
        return;
    }
    if !report.changed {
        println!(
            "{} {} is already current ({})",
            styler.green("✓"),
            report.path,
            report.before_fingerprint
        );
        if !report.consequence.is_empty() {
            println!("  {}", styler.dim(&report.consequence));
        }
        return;
    }
    println!(
        "{} {} {}",
        styler.green("✓"),
        report.path,
        styler.dim(&report.consequence)
    );
    println!(
        "  {} → {}",
        report.before_fingerprint, report.after_fingerprint
    );
    for result in &report.per_op {
        println!(
            "  {} op {}: {}",
            styler.dim("•"),
            result.op_index,
            result.summary
        );
    }
    if report.dry_run {
        for change in &report.changes {
            print!("{}", color_diff(&change.diff(), &styler));
        }
        println!("{}", styler.dim("dry run: no files written"));
    } else {
        if let Some(commit) = &report.commit {
            println!("  {} {}", styler.dim("commit"), commit.sha);
            println!("  {} {}", styler.dim("subject"), commit.subject);
        }
        if report.sync_post == Some("failed") {
            println!(
                "{} saved locally; sync failed — run `bob vault-sync`",
                styler.red("!")
            );
        }
    }
    for warning in &report.warnings {
        let saved_locally = warning.contains("committed locally")
            || warning.contains("post-sync failed");
        if saved_locally {
            eprintln!("{} saved locally; sync failed — run `bob vault-sync` ({warning})", styler.red("!"));
        } else {
            eprintln!("{} {warning}", styler.dim("warning:"));
        }
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

/// Describe one envelope op for docs and test goldens.
#[allow(dead_code)]
pub(crate) fn describe_envelope(envelope: &Envelope) -> Vec<Value> {
    envelope.ops.iter().map(op_to_json).collect()
}

#[allow(dead_code)]
pub(crate) fn op_result_failure(error: &OpError) -> UpdateFailure {
    op_failure(error)
}
