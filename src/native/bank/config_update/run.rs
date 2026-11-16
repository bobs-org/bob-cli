//! Dry-run and apply for `bob bank config update`.

use std::{
    cell::RefCell,
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::ops::{inverse_to_json, OpError};
use super::plan::{self, UpdatePlan};
use super::protocol::{
    parse_envelope, Envelope, ProtocolError, SCHEMA_VERSION,
};
use crate::native::bank::config;
use crate::native::capture::StagedTextFile;
use crate::native::host_catalog::{
    self, CommitInfo, PlannedTxn, ReadDependencies, TxnResult,
};
use crate::native::vault_rewrite::TxnFailure;
use crate::native::{ob, style::Styler};

#[derive(Debug, Clone)]
pub(crate) struct UpdateFailure {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
    pub(crate) op_index: Option<usize>,
    pub(crate) key_path: Option<String>,
    pub(crate) line: Option<usize>,
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
            key_path: None,
            line: None,
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
            key_path: None,
            line: None,
            affected: None,
            exit: 1,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ChangedFile {
    pub(crate) path: String,
    pub(crate) before: String,
    pub(crate) after: String,
}

pub(crate) struct UpdateReport {
    pub(crate) dry_run: bool,
    pub(crate) changed: bool,
    pub(crate) before_fingerprint: String,
    pub(crate) after_fingerprint: String,
    pub(crate) consequence: String,
    pub(crate) per_op: Vec<super::ops::OpResult>,
    pub(crate) inverse_ops: Vec<super::ops::InverseOp>,
    pub(crate) shifted: Vec<super::plan::HotkeyShift>,
    pub(crate) free: Vec<char>,
    pub(crate) categories: Value,
    pub(crate) changes: Vec<ChangedFile>,
    pub(crate) commit: Option<CommitInfo>,
    pub(crate) sync_pre: Option<&'static str>,
    pub(crate) sync_post: Option<&'static str>,
    pub(crate) warnings: Vec<String>,
    pub(crate) failure: Option<UpdateFailure>,
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
            "ok": self.failure.is_none(),
            "schema_version": SCHEMA_VERSION,
            "command": "update",
            "dry_run": self.dry_run,
            "changed": self.changed,
            "before_fingerprint": self.before_fingerprint,
            "after_fingerprint": self.after_fingerprint,
            "consequence": self.consequence,
            "per_op": self.per_op.iter().map(|result| {
                json!({
                    "index": result.op_index,
                    "op": result.op_name,
                    "summary": result.summary,
                    "key": result.key,
                    "hotkey": result.hotkey.map(|hotkey| hotkey.to_string()),
                    "rows": result.rows,
                    "rule_refs": result.rule_refs,
                    "journal_events": result.journal_events,
                })
            }).collect::<Vec<_>>(),
            "hotkeys": {
                "shifted": self.shifted.iter().map(|shift| {
                    json!({
                        "key": shift.key,
                        "from": shift.from.map(|ch| ch.to_string()),
                        "to": shift.to.map(|ch| ch.to_string()),
                    })
                }).collect::<Vec<_>>(),
                "free": self.free.iter().map(char::to_string).collect::<Vec<_>>(),
            },
            "categories": self.categories,
            "inverse_ops": self.inverse_ops.iter().map(inverse_to_json).collect::<Vec<_>>(),
            "changes": self.changes.iter().map(|change| {
                let mut entry = json!({"path": change.path});
                entry["diff"] = json!(host_catalog::unified_diff(
                    &change.path,
                    &change.before,
                    &change.after,
                ));
                entry
            }).collect::<Vec<_>>(),
            "commit": match &self.commit {
                Some(commit) => json!({"sha": commit.sha, "subject": commit.subject}),
                None => Value::Null,
            },
            "sync": match (self.sync_pre, self.sync_post) {
                (Some(pre), Some(post)) => json!({"pre": pre, "post": post}),
                _ => Value::Null,
            },
            "warnings": self.warnings,
            "failure": Value::Null,
        });
        if let Some(failure) = &self.failure {
            payload["failure"] = json!({
                "code": failure.code,
                "message": failure.message,
                "hint": failure.hint,
                "op_index": failure.op_index,
                "key_path": failure.key_path,
                "line": failure.line,
                "affected": failure.affected,
            });
            payload["ok"] = json!(false);
        }
        payload
    }
}

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
                    Some(
                        "pass a JSON file with -i FILE or pipe the envelope on stdin"
                            .to_owned(),
                    ),
                )
            })
        }
    }
}

fn read_stdin() -> Result<String, UpdateFailure> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|error| {
            UpdateFailure::semantic(
            "envelope_unreadable",
            format!("could not read the envelope from stdin: {error}"),
            Some(
                "pass a JSON file with -i FILE or pipe the envelope on stdin"
                    .to_owned(),
            ),
        )
        })?;
    Ok(text)
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
    const RUNTIME: &[&str] = &[
        "io",
        "lock_busy",
        "not_a_worktree",
        "sync_failed",
        "verify_failed",
        "vault_mismatch",
        "stale_note",
        "dirty_paths",
    ];
    let mut failure = if RUNTIME.contains(&error.code) {
        UpdateFailure::runtime(
            error.code,
            error.message.clone(),
            error.hint.clone(),
        )
    } else {
        UpdateFailure::semantic(
            error.code,
            error.message.clone(),
            error.hint.clone(),
        )
    };
    failure.op_index = error.op_index;
    failure.key_path = error.key_path.clone();
    failure.line = error.line;
    failure.affected = error.affected;
    failure
}

fn failed_report(dry_run: bool, failure: UpdateFailure) -> UpdateReport {
    UpdateReport {
        dry_run,
        changed: false,
        before_fingerprint: String::new(),
        after_fingerprint: String::new(),
        consequence: String::new(),
        per_op: Vec::new(),
        inverse_ops: Vec::new(),
        shifted: Vec::new(),
        free: Vec::new(),
        categories: Value::Array(Vec::new()),
        changes: Vec::new(),
        commit: None,
        sync_pre: None,
        sync_post: None,
        warnings: Vec::new(),
        failure: Some(failure),
    }
}

fn report_of_plan(dry_run: bool, plan: &UpdatePlan) -> UpdateReport {
    let changes = if plan.changed {
        vec![ChangedFile {
            path: "bank.md".to_owned(),
            before: plan.before.clone(),
            after: plan.after.clone(),
        }]
    } else {
        Vec::new()
    };
    UpdateReport {
        dry_run,
        changed: plan.changed,
        before_fingerprint: plan.before_fingerprint.clone(),
        after_fingerprint: plan.after_fingerprint.clone(),
        consequence: plan.consequence.clone(),
        per_op: plan.per_op.clone(),
        inverse_ops: plan.inverse_ops.clone(),
        shifted: plan.shifted.clone(),
        free: plan.free.clone(),
        categories: plan.categories.clone(),
        changes,
        commit: None,
        sync_pre: dry_run.then_some("skipped"),
        sync_post: dry_run.then_some("skipped"),
        warnings: plan.warnings.clone(),
        failure: None,
    }
}

pub(crate) fn dry_run(vault: &Path, envelope_text: &str) -> UpdateReport {
    let envelope = match parse_envelope(envelope_text) {
        Ok(envelope) => envelope,
        Err(error) => return failed_report(true, protocol_failure(&error)),
    };
    let scan = match plan::load_scan(vault) {
        Ok(scan) => scan,
        Err(error) => return failed_report(true, op_failure(&error)),
    };
    let journal = plan::load_journal(vault);
    let subscription_refs = plan::load_subscription_refs(vault);
    match plan::plan_update(
        &scan,
        &journal,
        &subscription_refs,
        &envelope,
        today(),
    ) {
        Ok(plan) => report_of_plan(true, &plan),
        Err(error) => failed_report(true, op_failure(&error)),
    }
}

struct ApplyContext {
    plan: UpdatePlan,
}

pub(crate) fn run_standalone(
    vault: &Path,
    envelope_text: &str,
    json_mode: bool,
) -> UpdateReport {
    let envelope: Envelope = match parse_envelope(envelope_text) {
        Ok(envelope) => envelope,
        Err(error) => return failed_report(false, protocol_failure(&error)),
    };
    let scan = match plan::load_scan(vault) {
        Ok(scan) => scan,
        Err(error) => return failed_report(false, op_failure(&error)),
    };
    let journal = plan::load_journal(vault);
    let subscription_refs = plan::load_subscription_refs(vault);
    let preflight = match plan::plan_update(
        &scan,
        &journal,
        &subscription_refs,
        &envelope,
        today(),
    ) {
        Ok(plan) => plan,
        Err(error) => return failed_report(false, op_failure(&error)),
    };
    if !preflight.changed {
        return report_of_plan(false, &preflight);
    }
    let expect = envelope.expect.clone();
    let typed: RefCell<Option<OpError>> = RefCell::new(None);
    let child_env = ob::child_env();
    let result = host_catalog::run_locked(
        vault,
        &child_env,
        json_mode,
        "bank",
        || {
            let scan = plan::load_scan(vault).map_err(|error| {
                *typed.borrow_mut() = Some(error.clone());
                TxnFailure::new(
                    error.code,
                    error.message.clone(),
                    error.hint.clone(),
                )
            })?;
            let journal = plan::load_journal(vault);
            let subscription_refs = plan::load_subscription_refs(vault);
            let plan = match plan::plan_update(
                &scan,
                &journal,
                &subscription_refs,
                &envelope,
                today(),
            ) {
                Ok(plan) => plan,
                Err(error) => {
                    let failure = TxnFailure::new(
                        error.code,
                        error.message.clone(),
                        error.hint.clone(),
                    );
                    *typed.borrow_mut() = Some(error);
                    return Err(failure);
                }
            };
            if plan.before_fingerprint != expect {
                let error = OpError::new(
                "stale_config",
                format!(
                    "config fingerprint changed: expected {expect}, current {}",
                    plan.before_fingerprint
                ),
                Some("refresh the setup report and preview the operation again".to_owned()),
            );
                let failure = TxnFailure::new(
                    error.code,
                    error.message.clone(),
                    error.hint.clone(),
                );
                *typed.borrow_mut() = Some(error);
                return Err(failure);
            }
            if !plan.changed {
                return Ok(PlannedTxn {
                    subject: plan::commit_subject(&plan),
                    files: Vec::new(),
                    appends: Vec::new(),
                    deps: ReadDependencies::default(),
                    verify: Box::new(|| Ok(())),
                    context: ApplyContext { plan },
                    kind: "bank",
                });
            }
            let files = vec![StagedTextFile {
                target: vault.join("bank.md"),
                target_existed: true,
                original_target: plan.before.clone(),
                updated_target: plan.after.clone(),
            }];
            let after = plan.after.clone();
            let after_fingerprint = plan.after_fingerprint.clone();
            let vault_path = vault.to_path_buf();
            Ok(PlannedTxn {
                subject: plan::commit_subject(&plan),
                files,
                appends: Vec::new(),
                deps: ReadDependencies::default(),
                verify: Box::new(move || {
                    verify_installed(&vault_path, &after, &after_fingerprint)
                }),
                context: ApplyContext { plan },
                kind: "bank",
            })
        },
    );
    match result {
        Ok(result) => report_of_txn(result),
        Err(failure) => {
            let typed_error = typed.borrow().clone();
            let mut report = failed_report(
                false,
                txn_failure(&failure, typed_error.as_ref()),
            );
            report.before_fingerprint = preflight.before_fingerprint.clone();
            report.after_fingerprint = preflight.after_fingerprint.clone();
            report
        }
    }
}

fn verify_installed(
    vault: &Path,
    expected: &str,
    fingerprint: &str,
) -> Result<(), String> {
    let path = vault.join("bank.md");
    let actual = fs::read_to_string(&path)
        .map_err(|error| format!("read bank.md: {error}"))?;
    if actual != expected {
        return Err("bank.md did not match the planned after-image".to_owned());
    }
    let found = config::config_fingerprint(&actual).ok_or_else(|| {
        "bank.md has no fingerprint after the write".to_owned()
    })?;
    if found != fingerprint {
        return Err(format!(
            "bank.md fingerprint changed after the write (expected {fingerprint}, found {found})"
        ));
    }
    match config::parse_bank_config(&actual) {
        crate::native::bank::config::ConfigParse::Ok(_) => Ok(()),
        _ => {
            Err("bank.md has no valid bank: mapping after the write".to_owned())
        }
    }
}

fn txn_failure(error: &TxnFailure, typed: Option<&OpError>) -> UpdateFailure {
    if let Some(plan) = typed {
        return op_failure(plan);
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
        key_path: None,
        line: None,
        affected: None,
        exit,
    }
}

fn report_of_txn(result: TxnResult<ApplyContext>) -> UpdateReport {
    let mut report = report_of_plan(false, &result.context.plan);
    report.commit = result.commit;
    report.sync_pre = Some(result.sync_pre);
    report.sync_post = Some(result.sync_post);
    if let Some(warning) = result.post_sync_warning {
        report.warnings.push(warning);
    }
    report
}

pub(crate) fn print_human(report: &UpdateReport) {
    let styler = Styler::detect();
    if let Some(failure) = &report.failure {
        eprintln!("{} {}", styler.red("✗"), failure.message);
        if let Some(hint) = &failure.hint {
            eprintln!("  {}", styler.dim(hint));
        }
        return;
    }
    if report.dry_run {
        println!("{}", styler.dim("Dry run — nothing written"));
    }
    if !report.changed {
        println!(
            "{} bank.md is already current ({})",
            styler.green("✓"),
            report.before_fingerprint
        );
        if !report.consequence.is_empty() {
            println!("  {}", styler.dim(&report.consequence));
        }
        for warning in &report.warnings {
            println!("  {}", styler.yellow(warning));
        }
        return;
    }
    println!("{} {}", styler.green("✓"), report.consequence);
    println!(
        "  {} → {}",
        styler.dim(&report.before_fingerprint),
        styler.dim(&report.after_fingerprint)
    );
    if !report.shifted.is_empty() {
        println!("  keys");
        for shift in &report.shifted {
            let from = shift
                .from
                .map(|ch| ch.to_ascii_uppercase().to_string())
                .unwrap_or_else(|| "·".to_owned());
            let to = shift
                .to
                .map(|ch| ch.to_ascii_uppercase().to_string())
                .unwrap_or_else(|| "·".to_owned());
            println!("    {}  {from}→{to}", shift.key);
        }
    }
    if !report.free.is_empty() {
        let keys = report
            .free
            .iter()
            .map(|ch| ch.to_ascii_uppercase().to_string())
            .collect::<Vec<_>>()
            .join(" ");
        println!("  free keys  {keys}");
    }
    for change in &report.changes {
        print!(
            "{}",
            host_catalog::color_diff(
                &host_catalog::unified_diff(
                    &change.path,
                    &change.before,
                    &change.after
                ),
                &styler,
            )
        );
    }
    if let Some(commit) = &report.commit {
        println!(
            "  commit  {}  {}",
            &commit.sha[..7.min(commit.sha.len())],
            commit.subject
        );
    }
    if let (Some(pre), Some(post)) = (report.sync_pre, report.sync_post) {
        println!("  sync  pre {pre} · post {post}");
    }
    for warning in &report.warnings {
        println!("  {}", styler.yellow(warning));
    }
}

pub(crate) fn failed_from_read(
    dry_run: bool,
    failure: UpdateFailure,
) -> UpdateReport {
    failed_report(dry_run, failure)
}
