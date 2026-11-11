//! `bob notebooks create`: dry run, guarded write, and output.

use std::path::Path;

use serde_json::{json, Value};

use super::{
    create::{
        plan_create, validate_inputs, CreatePlan, CreateRefusal, CreateRequest,
    },
    model::CatalogRecord,
    sync::{
        color_diff, commit_json, failure_json, findings_json, load_complete,
        short, sync_json, unified_diff, SCHEMA_VERSION,
    },
    txn::{self, verify_hosts_current, CommitInfo, PlannedTxn},
};
use crate::native::{
    capture::StagedTextFile, ob, style::Styler, vault_rewrite::TxnFailure,
};

/// What a create run planned and (unless a dry run) did.
#[derive(Debug)]
pub(crate) struct CreateReport {
    pub(crate) dry_run: bool,
    pub(crate) plan: Option<CreatePlan>,
    pub(crate) commit: Option<CommitInfo>,
    pub(crate) sync_pre: Option<&'static str>,
    pub(crate) sync_post: Option<&'static str>,
    pub(crate) warnings: Vec<String>,
    pub(crate) failure: Option<TxnFailure>,
}

impl CreateReport {
    fn failed(dry_run: bool, failure: TxnFailure) -> Self {
        Self {
            dry_run,
            plan: None,
            commit: None,
            sync_pre: None,
            sync_post: None,
            warnings: Vec::new(),
            failure: Some(failure),
        }
    }

    /// Validation and refusal exit 2; infrastructure failures exit 1.
    pub(crate) fn exit_code(&self) -> i32 {
        match &self.failure {
            None => 0,
            Some(failure) if failure.code.starts_with("notebook_") => 2,
            Some(_) => 1,
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        let mut payload = json!({
            "schema_version": SCHEMA_VERSION,
            "ok": self.failure.is_none(),
            "command": "create",
            "dry_run": self.dry_run,
            "notebook": Value::Null,
            "changes": [],
            "catalogs": [],
            "findings": [],
            "commit": commit_json(self.commit.as_ref()),
            "sync": sync_json(self.sync_pre, self.sync_post),
            "warnings": self.warnings,
        });
        if let Some(plan) = &self.plan {
            payload["notebook"] = json!({
                "path": slash(&plan.path),
                "stem": plan.stem,
                "title": plan.title,
                "summary": plan.summary,
                "parent": plan.parent_stem,
            });
            payload["changes"] = Value::Array(changes_json(plan, self.dry_run));
            payload["catalogs"] = Value::Array(
                plan.catalogs.iter().map(CatalogRecord::to_json).collect(),
            );
            payload["findings"] = Value::Array(findings_json(&plan.catalogs));
        }
        if let Some(failure) = &self.failure {
            payload["error"] = failure_json(failure);
        }
        payload
    }
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn changes_json(plan: &CreatePlan, dry_run: bool) -> Vec<Value> {
    let mut changes = Vec::new();
    let mut created = json!({"path": slash(&plan.path), "action": "create"});
    if dry_run {
        created["diff"] = Value::String(new_file_diff(plan));
    }
    changes.push(created);
    for edit in &plan.edits {
        let path = slash(&edit.path);
        let mut entry = json!({"path": path, "action": "update"});
        if dry_run {
            entry["diff"] =
                Value::String(unified_diff(&path, &edit.before, &edit.after));
        }
        changes.push(entry);
    }
    changes
}

fn new_file_diff(plan: &CreatePlan) -> String {
    similar::TextDiff::from_lines("", plan.contents.as_str())
        .unified_diff()
        .context_radius(3)
        .header("/dev/null", &format!("b/{}", slash(&plan.path)))
        .to_string()
}

fn refusal_failure(refusal: CreateRefusal) -> TxnFailure {
    TxnFailure::new(refusal.code, refusal.message, refusal.hint)
}

/// Read-only plan: the new file plus every host diff.
pub(crate) fn dry_run(vault: &Path, request: &CreateRequest) -> CreateReport {
    if let Err(refusal) =
        validate_inputs(&request.id, &request.summary, request.title.as_deref())
    {
        return CreateReport::failed(true, refusal_failure(refusal));
    }
    let snapshot = match load_complete(vault) {
        Ok(snapshot) => snapshot,
        Err(failure) => return CreateReport::failed(true, failure),
    };
    match plan_create(&snapshot, request) {
        Ok(plan) => CreateReport {
            dry_run: true,
            warnings: plan.warnings.clone(),
            plan: Some(plan),
            commit: None,
            sync_pre: None,
            sync_post: None,
            failure: None,
        },
        Err(refusal) => CreateReport::failed(true, refusal_failure(refusal)),
    }
}

/// Stage the new file and affected hosts as one guarded transaction.
fn planned_txn(vault: &Path, plan: CreatePlan) -> PlannedTxn<CreatePlan> {
    let mut files = vec![StagedTextFile {
        target: vault.join(&plan.path),
        target_existed: false,
        original_target: String::new(),
        updated_target: plan.contents.clone(),
    }];
    files.extend(crate::native::host_catalog::staged_rewrites(
        vault,
        &plan.edits,
    ));
    let hosts = plan.edits.iter().map(|edit| edit.path.clone()).collect();
    PlannedTxn {
        subject: format!("notebooks: create {}", slash(&plan.path)),
        files,
        deps: plan.deps.clone(),
        verify: verify_hosts_current(vault.to_path_buf(), hosts),
        context: plan,
        kind: "notebook",
    }
}

/// Standalone real run: refusals that need no vault fail before any lock.
pub(crate) fn run_standalone(
    vault: &Path,
    request: &CreateRequest,
    json_mode: bool,
) -> CreateReport {
    if let Err(refusal) =
        validate_inputs(&request.id, &request.summary, request.title.as_deref())
    {
        return CreateReport::failed(false, refusal_failure(refusal));
    }
    let child_env = ob::child_env();
    let result = txn::run_locked(vault, &child_env, json_mode, || {
        let snapshot = load_complete(vault)?;
        let plan = plan_create(&snapshot, request).map_err(refusal_failure)?;
        Ok(planned_txn(vault, plan))
    });
    match result {
        Ok(result) => {
            let plan = result.context;
            let mut warnings = plan.warnings.clone();
            warnings.extend(result.post_sync_warning);
            CreateReport {
                dry_run: false,
                plan: Some(plan),
                commit: result.commit,
                sync_pre: Some(result.sync_pre),
                sync_post: Some(result.sync_post),
                warnings,
                failure: None,
            }
        }
        Err(failure) => CreateReport::failed(false, failure),
    }
}

/// Human rendering for `bob notebooks create`.
pub(crate) fn print_human(report: &CreateReport) {
    let styler = Styler::detect();
    if let Some(failure) = &report.failure {
        eprintln!("{} {}", styler.red("✗"), failure.message);
        if let Some(hint) = &failure.hint {
            eprintln!("  {}", styler.dim(hint));
        }
        return;
    }
    let Some(plan) = &report.plan else {
        return;
    };
    let path = slash(&plan.path);
    if report.dry_run {
        print!("{}", color_diff(&new_file_diff(plan), &styler));
        for edit in &plan.edits {
            print!(
                "{}",
                color_diff(
                    &unified_diff(
                        &slash(&edit.path),
                        &edit.before,
                        &edit.after
                    ),
                    &styler
                )
            );
        }
        println!(
            "{}",
            styler.yellow(&format!(
                "dry run: would create {path} under {} and update {} catalog(s); nothing was written",
                plan.parent_stem,
                plan.edits.len()
            ))
        );
    } else {
        let sha = report
            .commit
            .as_ref()
            .map(|commit| format!(" ({})", short(&commit.sha)))
            .unwrap_or_default();
        println!(
            "{} created {} in {}{sha}",
            styler.green("✓"),
            styler.bold(&path),
            plan.parent_stem
        );
        for edit in &plan.edits {
            println!("  updated {}", slash(&edit.path));
        }
    }
    for warning in &report.warnings {
        println!("{} {warning}", styler.yellow("!"));
    }
}
