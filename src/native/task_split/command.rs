//! CLI shell for the pure task-split planner.

use std::{
    ffi::OsString,
    iter,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use clap::{Arg, ArgAction, ArgGroup, Command as ClapCommand};
use serde_json::{json, Value};

use crate::native::{
    capture::{self},
    collect_done::link_repair_markdown_files,
    config, env as bob_env,
    note_tasks::{self, BlockIdLookup},
    style::Styler,
    task_command_shell::{self, TaskCommandShell},
    task_move::VaultSnapshot,
    task_status_hooks_write::{
        apply_plan, capture_required, planned_write, ApplyOutcome,
        ApplySession, InputKind, InputSnapshot, ReasonCode, WritePlan,
    },
};

use super::plan::{
    list_task_split_candidates, plan_task_split, SplitCandidate, SplitPlan,
    SplitRequest, SplitSelection, SplitSettings,
};

const COMMAND_NAME: &str = "bob task split";
const TOOL_NAME: &str = "task-split";
const DEFAULT_RETRY_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Human,
    Json,
}

struct Scan {
    snapshot: VaultSnapshot,
    inputs: Vec<InputSnapshot>,
    scan_paths: Vec<PathBuf>,
}

struct Report {
    vault: PathBuf,
    format: Format,
    dry_run: bool,
    offline: bool,
    plan: Option<SplitPlan>,
    candidates: Vec<SplitCandidate>,
    applied: Vec<String>,
    deferred: Vec<String>,
    recovery: Option<String>,
    commit: Option<task_command_shell::CommitResult>,
    pre_sync: Option<Value>,
    post_sync: Option<Value>,
    mode: &'static str,
    error: Option<(String, String, String)>,
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Promote task notes into linked tasks")
        .long_about("Split selected authored bullets from a task into new tasks. The parent keeps an in-place task link to each new task, and the selected bullets' children move with them. A live run uses the task maintenance lock, guarded writes, a scoped commit, and vault sync; --dry-run prints the same plan without changing the vault.")
        .after_help("Examples:\n  bob task split --dry-run db:upgrade -n 1\n                                 Preview the first splittable note\n  bob task split db:upgrade -m 'rollback runbook'\n                                 Split the unique matching note\n  bob task split dev -s 'Future Work'\n                                 Promote every bullet in a note section\n  bob task split -f json db:upgrade\n                                 List numbered candidates as JSON\n\nA task locator is note:block-id or [[note#^block-id]].")
        .disable_help_flag(true)
        .arg(Arg::new("owner").value_name("TASK|NOTE").required(true).index(1).help("Task locator, or note path when --section is given"))
        .arg(Arg::new("dry-run").long("dry-run").short('d').action(ArgAction::SetTrue).help("Plan and print only; do not lock, sync, or write"))
        .arg(Arg::new("format").long("format").short('f').value_name("FORMAT").value_parser(["human", "json"]).default_value("human").help("Output format: human or json"))
        .arg(Arg::new("help").long("help").short('h').action(ArgAction::Help).help("Show help"))
        .arg(Arg::new("match").long("match").short('m').value_name("TEXT").action(ArgAction::Append).help("Select a unique splittable bullet containing this text; repeatable"))
        .arg(Arg::new("nth").long("nth").short('n').value_name("N").value_parser(clap::value_parser!(usize)).action(ArgAction::Append).help("Select the Nth candidate; repeatable and 1-based"))
        .arg(Arg::new("offline").long("offline").short('o').action(ArgAction::SetTrue).help("Skip both sync cycles; commit locally"))
        .arg(Arg::new("retry-timeout").long("retry-timeout").short('r').value_name("SECONDS").value_parser(clap::value_parser!(u64)).default_value("60").help("Lock wait and re-plan budget"))
        .arg(Arg::new("section").long("section").short('s').value_name("TITLE").conflicts_with_all(["match", "nth", "bullet-ref"]).help("Select section bullets in this note; created Tasks intake receives the new tasks"))
        .arg(Arg::new("bullet-ref").long("bullet-ref").value_name("LINE:DIGEST").hide(true).conflicts_with_all(["match", "nth"]))
        .group(ArgGroup::new("selector").args(["match", "nth", "bullet-ref"]).multiple(false))
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    let format = if matches.get_one::<String>("format").map(String::as_str)
        == Some("json")
    {
        Format::Json
    } else {
        Format::Human
    };
    let dry_run = matches.get_flag("dry-run");
    let offline = matches.get_flag("offline");
    let retry = matches
        .get_one::<u64>("retry-timeout")
        .copied()
        .unwrap_or(DEFAULT_RETRY_TIMEOUT_SECS);
    let mut report = Report {
        vault: bob_env::bob_dir(),
        format,
        dry_run,
        offline,
        plan: None,
        candidates: Vec::new(),
        applied: Vec::new(),
        deferred: Vec::new(),
        recovery: None,
        commit: None,
        pre_sync: None,
        post_sync: None,
        mode: if dry_run {
            "dry_run"
        } else if offline {
            "offline"
        } else {
            "sync"
        },
        error: None,
    };
    let owner_text = matches.get_one::<String>("owner").unwrap();
    let section = matches.get_one::<String>("section").cloned();
    let owner_ref = match parse_locator(owner_text, section.is_some()) {
        Ok(locator) => locator,
        Err(message) => {
            report.error =
                Some(("plan".into(), "split_owner_invalid".into(), message));
            print_report(&report);
            return 2;
        }
    };
    let task_settings = note_tasks::read_settings(&report.vault);
    let relation_settings =
        config::load_relations_config(&config::config_path())
            .unwrap_or_default();
    let split_settings = SplitSettings {
        linked_from_lines: relation_settings.linked_from_lines,
        linked_from_mentions: relation_settings.linked_from_mentions,
        inbox_route: capture::INBOX_FILE.to_string(),
    };
    let selection = if let Some(section) = section {
        SplitSelection::Section { title: section }
    } else if let Some(values) = matches.get_many::<String>("match") {
        SplitSelection::Match(values.cloned().collect())
    } else if let Some(values) = matches.get_many::<usize>("nth") {
        SplitSelection::Nth(values.copied().collect())
    } else if let Some(reference) = matches.get_one::<String>("bullet-ref") {
        SplitSelection::BulletRef(reference.clone())
    } else {
        SplitSelection::Cursor { line: 0 }
    };
    if matches.get_many::<String>("match").is_none()
        && matches.get_many::<usize>("nth").is_none()
        && matches.get_one::<String>("bullet-ref").is_none()
    {
        return list_candidates(
            &mut report,
            &owner_ref,
            selection,
            &task_settings,
        );
    }
    if dry_run {
        let scan = match scan_vault(&report.vault) {
            Ok(scan) => scan,
            Err(error) => {
                report.error =
                    Some(("plan".into(), "split_scan_failed".into(), error));
                print_report(&report);
                return 1;
            }
        };
        let request = match resolve_request(
            &scan.snapshot,
            &owner_ref,
            selection,
            &task_settings,
        ) {
            Ok(request) => request,
            Err(error) => {
                report.error =
                    Some(("plan".into(), error.code.into(), error.message));
                print_report(&report);
                return 1;
            }
        };
        match plan_task_split(
            &scan.snapshot,
            &request,
            &bob_env::current_datetime().date().to_string(),
            &split_settings,
            &task_settings,
        ) {
            Ok(plan) => report.plan = Some(plan),
            Err(error) => {
                report.error =
                    Some(("plan".into(), error.code.into(), error.message))
            }
        }
        print_report(&report);
        return if report.error.is_some() { 1 } else { 0 };
    }
    execute_live(
        &mut report,
        owner_ref,
        selection,
        retry,
        &split_settings,
        &task_settings,
    )
}

#[derive(Debug)]
struct OwnerRef {
    path: PathBuf,
    block_id: Option<String>,
}

fn parse_locator(raw: &str, allow_note: bool) -> Result<OwnerRef, String> {
    let text = raw
        .strip_prefix("[[")
        .and_then(|s| s.strip_suffix("]]"))
        .unwrap_or(raw);
    if let Some((path, block)) = text.rsplit_once("#^") {
        return Ok(OwnerRef {
            path: with_md(path),
            block_id: Some(block.to_string()),
        });
    }
    if let Some((path, block)) = text.rsplit_once(':')
        && !block.is_empty()
    {
        return Ok(OwnerRef {
            path: with_md(path),
            block_id: Some(block.trim_start_matches('^').to_string()),
        });
    }
    if allow_note {
        return Ok(OwnerRef {
            path: with_md(text),
            block_id: None,
        });
    }
    Err(
        "use a task locator such as db:upgrade, or name a note with --section"
            .to_string(),
    )
}

fn resolve_request(
    snapshot: &VaultSnapshot,
    owner: &OwnerRef,
    selection: SplitSelection,
    task_settings: &note_tasks::NoteTaskSettings,
) -> Result<SplitRequest, super::plan::SplitError> {
    let contents = snapshot.notes.get(&owner.path).ok_or_else(|| {
        super::plan::SplitError {
            code: "split_owner_missing",
            message: format!(
                "{} does not exist in the vault",
                owner.path.display()
            ),
        }
    })?;
    let line = if let Some(id) = &owner.block_id {
        match note_tasks::scan(contents, task_settings).by_block_id(id) {
            BlockIdLookup::Found(task) => task.line_index + 1,
            BlockIdLookup::Missing => {
                return Err(super::plan::SplitError {
                    code: "split_owner_missing",
                    message: format!(
                        "no task with block ID ^{id} in {}",
                        owner.path.display()
                    ),
                })
            }
            BlockIdLookup::Duplicate(count) => {
                return Err(super::plan::SplitError {
                    code: "split_owner_ambiguous",
                    message: format!(
                        "block ID ^{id} appears {count} times in {}",
                        owner.path.display()
                    ),
                })
            }
            BlockIdLookup::NotATask { line_index, .. } => {
                return Err(super::plan::SplitError {
                    code: "split_owner_not_task",
                    message: format!(
                        "^{id} at line {} is not a task",
                        line_index + 1
                    ),
                })
            }
        }
    } else {
        1
    };
    Ok(SplitRequest {
        owner_path: owner.path.clone(),
        owner_line: line,
        selection,
    })
}

fn list_candidates(
    report: &mut Report,
    owner: &OwnerRef,
    selection: SplitSelection,
    task_settings: &note_tasks::NoteTaskSettings,
) -> i32 {
    let scan = match scan_vault(&report.vault) {
        Ok(scan) => scan,
        Err(error) => {
            report.error =
                Some(("plan".into(), "split_scan_failed".into(), error));
            print_report(report);
            return 1;
        }
    };
    let request = match resolve_request(
        &scan.snapshot,
        owner,
        selection,
        task_settings,
    ) {
        Ok(request) => request,
        Err(error) => {
            report.error =
                Some(("plan".into(), error.code.into(), error.message));
            print_report(report);
            return 1;
        }
    };
    match list_task_split_candidates(&scan.snapshot, &request, task_settings) {
        Ok(candidates) => report.candidates = candidates,
        Err(error) => {
            report.error =
                Some(("plan".into(), error.code.into(), error.message));
            print_report(report);
            return 1;
        }
    }
    print_report(report);
    2
}

fn execute_live(
    report: &mut Report,
    owner: OwnerRef,
    selection: SplitSelection,
    retry: u64,
    settings: &SplitSettings,
    task_settings: &note_tasks::NoteTaskSettings,
) -> i32 {
    let shell = match TaskCommandShell::start(
        &report.vault,
        Duration::from_secs(retry),
        report.offline,
        report.format == Format::Json,
    ) {
        Ok(shell) => shell,
        Err(error) => {
            report.error =
                Some(("lock".into(), "maintenance_lock".into(), error));
            print_report(report);
            return 1;
        }
    };
    if let Some(warning) = shell.warning.as_ref() {
        eprintln!("warning: {warning}");
        report.mode = "not_a_worktree";
    }
    if let Some(cycle) = &shell.pre_sync {
        report.pre_sync = Some(
            json!({"ok":cycle.ok,"files_committed":cycle.files_committed,"error":cycle.error}),
        );
        if !cycle.ok {
            report.error = Some((
                "pre_sync".into(),
                "vault_sync_failed".into(),
                cycle
                    .error
                    .clone()
                    .unwrap_or_else(|| "pre-sync failed".into()),
            ));
            print_report(report);
            return 1;
        }
    }
    let timeout = Duration::from_secs(retry);
    let started = Instant::now();
    let mut applied = Vec::new();
    let mut partial = false;
    loop {
        let scan = match scan_vault(&report.vault) {
            Ok(scan) => scan,
            Err(error) => {
                report.error =
                    Some(("plan".into(), "split_scan_failed".into(), error));
                break;
            }
        };
        let request = match resolve_request(
            &scan.snapshot,
            &owner,
            selection.clone(),
            task_settings,
        ) {
            Ok(request) => request,
            Err(error) => {
                report.error =
                    Some(("plan".into(), error.code.into(), error.message));
                break;
            }
        };
        let plan = match plan_task_split(
            &scan.snapshot,
            &request,
            &bob_env::current_datetime().date().to_string(),
            settings,
            task_settings,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                report.error =
                    Some(("plan".into(), error.code.into(), error.message));
                break;
            }
        };
        let write_plan = match build_write_plan(&report.vault, &scan, &plan) {
            Ok(plan) => plan,
            Err(error) => {
                report.error =
                    Some(("apply".into(), "write_plan_failed".into(), error));
                break;
            }
        };
        report.plan = Some(plan.clone());
        let rescan_root = report.vault.clone();
        let mut session = ApplySession::production(Box::new(move || {
            link_repair_markdown_files(&rescan_root)
        }));
        session.tool = TOOL_NAME;
        match apply_plan(&write_plan, &session) {
            Ok(ApplyOutcome::NoOp) => break,
            Ok(ApplyOutcome::Applied {
                applied_files,
                recovery_directory,
            }) => {
                applied = applied_files;
                report.recovery =
                    Some(recovery_directory.display().to_string());
                break;
            }
            Err(error) => {
                let retryable = matches!(
                    error.reason,
                    ReasonCode::VaultChanged
                        | ReasonCode::QuietPeriod
                        | ReasonCode::UnstableRead
                ) && error.applied_files.is_empty();
                if retryable && started.elapsed() < timeout {
                    continue;
                }
                applied = error.applied_files;
                report.applied = display_paths(&report.vault, &applied);
                report.deferred =
                    display_paths(&report.vault, &error.deferred_files);
                report.recovery = error
                    .recovery_directory
                    .map(|path| path.display().to_string());
                if applied.is_empty() {
                    report.error = Some((
                        "apply".into(),
                        "guarded_write_failed".into(),
                        error.message,
                    ));
                } else {
                    partial = true;
                    report.error = Some((
                        "apply".into(),
                        "partial_write".into(),
                        error.message,
                    ));
                }
                break;
            }
        }
    }
    report.applied = display_paths(&report.vault, &applied);
    if !applied.is_empty() && shell.worktree {
        let subject = report
            .plan
            .as_ref()
            .map(split_subject)
            .unwrap_or_else(|| "bob task split".into());
        match shell.commit(&report.vault, &applied, &subject) {
            Ok(commit) => report.commit = commit,
            Err(error) => {
                report.error =
                    Some(("commit".into(), "git_commit_failed".into(), error))
            }
        }
    }
    if shell.worktree && !report.offline && (report.error.is_none() || partial)
    {
        let cycle = shell.post_sync();
        report.post_sync = Some(
            json!({"ok":cycle.ok,"pushed":cycle.pushed,"conflicts":cycle.conflicts,"error":cycle.error}),
        );
        if !cycle.ok && report.error.is_none() {
            report.error = Some((
                "post_sync".into(),
                "vault_sync_failed".into(),
                cycle.error.unwrap_or_else(|| "post-sync failed".into()),
            ));
        }
    }
    print_report(report);
    if report.error.is_some() {
        1
    } else {
        0
    }
}

fn build_write_plan(
    vault: &Path,
    scan: &Scan,
    plan: &SplitPlan,
) -> Result<WritePlan, String> {
    let destination = plan.tasks.first().map(|task| PathBuf::from(&task.path));
    let mut postimages = plan.postimages.iter().collect::<Vec<_>>();
    postimages.sort_by(|(left, _), (right, _)| {
        let rank = |path: &Path| {
            if destination.as_deref() == Some(path) {
                0
            } else if path == plan.owner_path {
                2
            } else {
                1
            }
        };
        rank(left).cmp(&rank(right)).then_with(|| left.cmp(right))
    });
    let mut outputs = Vec::new();
    for (relative, contents) in postimages {
        let path = vault.join(relative);
        let input = scan
            .inputs
            .iter()
            .find(|input| input.path == path)
            .ok_or_else(|| {
                format!(
                    "{} was not in the planning read-set",
                    relative.display()
                )
            })?;
        outputs.push(
            planned_write(path, input, contents.as_bytes().to_vec(), true)
                .map_err(|error| error.message())?,
        );
    }
    Ok(WritePlan {
        vault_canonical: vault
            .canonicalize()
            .unwrap_or_else(|_| vault.to_path_buf()),
        inputs: scan.inputs.clone(),
        scan_paths: scan.scan_paths.clone(),
        outputs,
    })
}

fn scan_vault(vault: &Path) -> Result<Scan, String> {
    let files = link_repair_markdown_files(vault)
        .map_err(|error| format!("scan vault {}: {error}", vault.display()))?;
    let mut notes = Vec::new();
    let mut inputs = Vec::new();
    let mut scan_paths = Vec::new();
    for path in files {
        let input =
            capture_required(&path, InputKind::Note).map_err(|error| {
                format!("read {}: {}", path.display(), error.message())
            })?;
        let text = input
            .utf8_contents()
            .map_err(|error| error.message())?
            .ok_or_else(|| format!("{} disappeared", path.display()))?;
        let relative = path
            .strip_prefix(vault)
            .map_err(|error| {
                format!("{} is outside vault: {error}", path.display())
            })?
            .to_path_buf();
        notes.push((relative, text));
        scan_paths.push(path);
        inputs.push(input);
    }
    Ok(Scan {
        snapshot: VaultSnapshot::from_notes(notes),
        inputs,
        scan_paths,
    })
}

fn split_subject(plan: &SplitPlan) -> String {
    format!(
        "bob task split {} ({} {})",
        plan.owner_path.display(),
        plan.tasks.len(),
        if plan.tasks.len() == 1 {
            "task"
        } else {
            "tasks"
        }
    )
}
fn display_paths(vault: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| {
            path.strip_prefix(vault)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}
fn with_md(value: &str) -> PathBuf {
    let mut path = PathBuf::from(value);
    if path.extension().is_none() {
        path.set_extension("md");
    }
    path
}

fn print_report(report: &Report) {
    if report.format == Format::Json {
        println!("{}", json_document(report));
        return;
    }
    let styler = Styler::detect();
    if !report.candidates.is_empty() {
        println!("{}", styler.bold("Split candidates"));
        for candidate in &report.candidates {
            let title = match &candidate.drawer {
                Some(drawer) => format!("{drawer} › {}", candidate.title),
                None => candidate.title.clone(),
            };
            let children = if candidate.child_lines > 0 {
                format!(" (+{})", candidate.child_lines)
            } else {
                String::new()
            };
            println!("  {}  {}{}", candidate.ordinal, title, children);
        }
        println!("\nChoose with --nth N or --match TEXT.");
        return;
    }
    if report.error.is_none() && report.plan.is_none() {
        println!("No splittable bullets found.");
        return;
    }
    let action = if report.dry_run {
        "Would split"
    } else {
        "Split"
    };
    println!("{}", styler.bold(action));
    if let Some(plan) = &report.plan {
        println!("\n{}", plan.notice);
        for task in &plan.tasks {
            println!(
                "  ◯ {}  ^{} · {}:{}",
                task.title, task.block_id, task.path, task.line
            );
        }
        if !report.dry_run && !report.applied.is_empty() {
            println!(
                "\nApplied {} file(s): {}",
                report.applied.len(),
                report.applied.join(", ")
            );
        }
    }
    if let Some((stage, _, message)) = &report.error {
        eprintln!("{} {}: {}", styler.red("✗"), stage, message);
    }
}

fn json_document(report: &Report) -> Value {
    let splits=report.plan.as_ref().map(|plan|vec![json!({
        "owner":{"path":plan.owner_path,"line":plan.owner_line,"block_id":plan.owner_block_id,"minted_id":plan.owner_minted_id},
        "tasks":plan.tasks,"stubs":plan.stubs,"placement":plan.placement,"notice":plan.notice,
        "cursor":{"path":plan.cursor_path,"line":plan.cursor_line},
        "clamped":plan.clamped.map(|(requested,split)|json!({"requested":requested,"split":split})),
    })]).unwrap_or_default();
    json!({"schema_version":1,"ok":report.error.is_none(),"dry_run":report.dry_run,"candidates":report.candidates,"splits":splits,"warnings":[],"repaired_files":[],"git":{"mode":report.mode,"pre_sync":report.pre_sync,"commit":report.commit.as_ref().map(|commit|json!({"sha":commit.sha,"subject":commit.subject,"paths":commit.paths})),"post_sync":report.post_sync},"applied_paths":report.applied,"deferred_paths":report.deferred,"recovery_directory":report.recovery,"error":report.error.as_ref().map(|(stage,code,message)|json!({"stage":stage,"code":code,"message":message,"hint":null}))})
}

fn print_clap_error(error: clap::Error) -> i32 {
    let code = error.exit_code();
    let _ = error.print();
    code
}
