//! `bob albums create`: dry run, guarded write, and output.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{json, Value};
use similar::TextDiff;

use super::{
    catalog::{self, AlbumCatalogRecord},
    create::{
        plan_create, validate_inputs, CreatePlan, CreateRefusal, CreateRequest,
    },
    registry::AlbumRegistry,
    report::AlbumIssue,
    AlbumIssueSeverity,
};
use crate::native::{
    capture::StagedTextFile,
    host_catalog::{self, CommitInfo, PlannedTxn},
    notebooks::snapshot::VaultSnapshot,
    ob,
    style::Styler,
    vault_rewrite::TxnFailure,
};

pub(crate) const SCHEMA_VERSION: u32 = 1;

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
            Some(failure) if failure.code.starts_with("album_") => 2,
            Some(_) => 1,
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        let mut payload = json!({
            "schema_version": SCHEMA_VERSION,
            "ok": self.failure.is_none(),
            "command": "create",
            "dry_run": self.dry_run,
            "album": Value::Null,
            "changed": [],
            "changes": [],
            "catalogs": [],
            "findings": [],
            "commit": commit_json(self.commit.as_ref()),
            "sync": sync_json(self.sync_pre, self.sync_post),
            "warnings": self.warnings,
        });
        if let Some(plan) = &self.plan {
            payload["album"] = json!({
                "path": slash(&plan.path),
                "stem": plan.stem,
                "title": plan.title,
                "summary": plan.summary,
                "parent": plan.parent_stem,
                "noun": plan.noun,
                "label": plan.label,
            });
            payload["changed"] = Value::Array(
                changed_paths(plan).iter().map(|path| json!(path)).collect(),
            );
            payload["changes"] = Value::Array(changes_json(plan, self.dry_run));
            payload["catalogs"] = Value::Array(
                plan.catalogs
                    .iter()
                    .map(AlbumCatalogRecord::to_json)
                    .collect(),
            );
            payload["findings"] = Value::Array(
                plan.catalogs
                    .iter()
                    .flat_map(|record| {
                        record.issues.iter().map(AlbumIssue::to_json)
                    })
                    .collect(),
            );
        }
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

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn changed_paths(plan: &CreatePlan) -> Vec<String> {
    let mut paths = vec![slash(&plan.path), slash(&plan.base_path)];
    for edit in &plan.edits {
        paths.push(slash(&edit.path));
    }
    paths
}

fn changes_json(plan: &CreatePlan, dry_run: bool) -> Vec<Value> {
    let mut changes = Vec::new();
    for (path, contents) in [
        (&plan.path, &plan.contents),
        (&plan.base_path, &plan.base_contents),
    ] {
        let path = slash(path);
        let mut entry = json!({"path": path, "action": "create"});
        if dry_run {
            entry["diff"] = Value::String(new_file_diff(&path, contents));
        }
        changes.push(entry);
    }
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

fn new_file_diff(path: &str, contents: &str) -> String {
    TextDiff::from_lines("", contents)
        .unified_diff()
        .context_radius(3)
        .header("/dev/null", &format!("b/{path}"))
        .to_string()
}

fn unified_diff(path: &str, before: &str, after: &str) -> String {
    TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

fn refusal_failure(refusal: CreateRefusal) -> TxnFailure {
    TxnFailure::new(refusal.code, refusal.message, refusal.hint)
}

/// Load the vault and refuse a partial view: an unreadable file must never
/// read as an empty subtree and drop catalog rows.
fn load_complete(vault: &Path) -> Result<VaultSnapshot, TxnFailure> {
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
            Some("fix the unreadable paths, then retry; nothing was created"),
        ));
    }
    Ok(snapshot)
}

/// Read-only plan: the new files plus every host diff.
pub(crate) fn dry_run(vault: &Path, request: &CreateRequest) -> CreateReport {
    if let Err(refusal) = validate_inputs(
        &request.stem,
        &request.noun,
        &request.summary,
        request.title.as_deref(),
        request.label.as_deref(),
        request.emoji.as_deref(),
    ) {
        return CreateReport::failed(true, refusal_failure(refusal));
    }
    let snapshot = match load_complete(vault) {
        Ok(snapshot) => snapshot,
        Err(failure) => return CreateReport::failed(true, failure),
    };
    match plan_create(vault, &snapshot, request) {
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

/// Re-read the vault and report anything the install did not leave exact:
/// the new declaration's valid claim/schema, the starter Base, and every
/// affected host catalog. A mismatch rolls the transaction back.
fn verify_create(
    vault: PathBuf,
    plan: CreatePlan,
    hosts: Vec<PathBuf>,
) -> Box<dyn FnOnce() -> Result<(), String>> {
    Box::new(move || verify_installed_create(&vault, &plan, &hosts))
}

pub(crate) fn verify_installed_create(
    vault: &Path,
    plan: &CreatePlan,
    hosts: &[PathBuf],
) -> Result<(), String> {
    let snapshot = VaultSnapshot::load(vault);
    if let Some(error) = snapshot.io_errors.first() {
        return Err(error.clone());
    }
    let registry = AlbumRegistry::from_snapshot(vault, &snapshot);
    let Some(entry) = registry
        .albums
        .iter()
        .find(|entry| Path::new(&entry.declaration.path) == plan.path)
    else {
        return Err(format!(
            "{} is missing after the write",
            slash(&plan.path)
        ));
    };
    if entry
        .findings
        .iter()
        .any(|finding| finding.severity == AlbumIssueSeverity::Error)
    {
        return Err(format!(
            "{} has registry errors after the write",
            slash(&plan.path)
        ));
    }
    if entry.usable_type.is_none() {
        return Err(format!(
            "{} is not a usable album after the write",
            slash(&plan.path)
        ));
    }
    if entry.parent_path.is_none()
        || entry.owner.is_none()
        || entry.root.is_none()
    {
        return Err(format!(
            "{} has no owner chain after the write",
            slash(&plan.path)
        ));
    }
    let catalogs = catalog::plan_catalogs(&snapshot, &registry);
    for host in hosts {
        let key = slash(host);
        if !catalog::required_host_catalog_is_current(catalogs.host(host)) {
            return Err(format!("{key} is still not current after the write"));
        }
    }
    for (relative, expected) in [
        (&plan.path, &plan.contents),
        (&plan.base_path, &plan.base_contents),
    ] {
        match fs::read_to_string(vault.join(relative)) {
            Ok(actual) if actual == *expected => {}
            Ok(_) => {
                return Err(format!(
                    "{} differs from the plan after the write",
                    slash(relative)
                ));
            }
            Err(error) => {
                return Err(format!(
                    "could not re-read {}: {error}",
                    slash(relative)
                ));
            }
        }
    }
    Ok(())
}

/// Stage the new album, its Base, and affected hosts as one guarded
/// transaction.
fn planned_txn(vault: &Path, plan: CreatePlan) -> PlannedTxn<CreatePlan> {
    let hosts = plan.edits.iter().map(|edit| edit.path.clone()).collect();
    let mut files = vec![
        StagedTextFile {
            target: vault.join(&plan.path),
            target_existed: false,
            original_target: String::new(),
            updated_target: plan.contents.clone(),
        },
        StagedTextFile {
            target: vault.join(&plan.base_path),
            target_existed: false,
            original_target: String::new(),
            updated_target: plan.base_contents.clone(),
        },
    ];
    files.extend(host_catalog::staged_rewrites(vault, &plan.edits));
    PlannedTxn {
        subject: format!("albums: create {}", slash(&plan.path)),
        files,
        appends: Vec::new(),
        deps: plan.deps.clone(),
        verify: verify_create(vault.to_path_buf(), plan.clone(), hosts),
        context: plan,
        kind: "album",
    }
}

/// Standalone real run: refusals that need no vault fail before any lock.
pub(crate) fn run_standalone(
    vault: &Path,
    request: &CreateRequest,
    json_mode: bool,
) -> CreateReport {
    if let Err(refusal) = validate_inputs(
        &request.stem,
        &request.noun,
        &request.summary,
        request.title.as_deref(),
        request.label.as_deref(),
        request.emoji.as_deref(),
    ) {
        return CreateReport::failed(false, refusal_failure(refusal));
    }
    let child_env = ob::child_env();
    let result =
        host_catalog::run_locked(vault, &child_env, json_mode, "album", || {
            let snapshot = load_complete(vault)?;
            let plan = plan_create(vault, &snapshot, request)
                .map_err(refusal_failure)?;
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

/// Human rendering for `bob albums create`.
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
        for (relative, contents) in [
            (&plan.path, &plan.contents),
            (&plan.base_path, &plan.base_contents),
        ] {
            print!(
                "{}",
                color_diff(&new_file_diff(&slash(relative), contents), &styler)
            );
        }
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
                "dry run: would create {path} and {} under {} and update {} catalog(s); nothing was written",
                slash(&plan.base_path),
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
        println!("  created {}", slash(&plan.base_path));
        for edit in &plan.edits {
            println!("  updated {}", slash(&edit.path));
        }
        println!();
        println!(
            "Add fields under `record:` when the first values settle, for example:"
        );
        println!();
        println!("  record:");
        println!("    noun: {}", plan.noun);
        println!("    fields:");
        println!("      status:");
        println!("        values: {{to_try: To try, liked: Liked}}");
        println!();
        println!("Run `bob albums check` to lint the new album.");
    }
    for warning in &report.warnings {
        println!("{} {warning}", styler.yellow("!"));
    }
}

fn short(sha: &str) -> &str {
    sha.get(..9.min(sha.len())).unwrap_or(sha)
}
