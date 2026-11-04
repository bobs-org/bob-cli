//! CLI shell for the pure task-merge planner.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    iter,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use clap::{Arg, ArgAction, Command as ClapCommand};
use serde_json::{json, Value};

use crate::native::{
    capture_drawers,
    capture_language::DrawerSelector,
    collect_done::link_repair_markdown_files,
    config, env as bob_env, note_tasks, pomodoro,
    style::Styler,
    task_command_shell::{self, TaskCommandShell},
    task_complete::SuccessorSelection,
    task_locator::TaskLocator,
    task_move::{transplant::RepairedFile, VaultSnapshot},
    task_status_hooks_write::{
        apply_plan, capture_required, planned_write, ApplyOutcome,
        ApplySession, InputKind, InputSnapshot, ReasonCode, WritePlan,
    },
    vault_links::NoteIndex,
};

use super::{
    plan::plan_task_merge,
    types::{
        DailyNoteReadState, MergeContainer, MergeContext, MergeError,
        MergeGesture, MergeOutcome, MergePlan, MergeRelationPolicy,
        MergeRequest,
    },
};

const COMMAND_NAME: &str = "bob task merge";
const TOOL_NAME: &str = "task-merge";
const DEFAULT_RETRY_TIMEOUT_SECS: u64 = 60;
const MERGE_LABEL_FALLBACK: &str = "merged into";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Human,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PullIn {
    All,
    None,
    Tokens {
        numbers: Vec<usize>,
        keys: Vec<String>,
    },
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
    plan: Option<MergePlan>,
    pull_choice: Option<String>,
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
        .about("Fold tasks into a task, drawer, or note section")
        .long_about("Merge one or more source tasks into a destination-shaped container: another task, one of its drawers, or a note's H2 section. Sources retire as cancelled tombstones with a merged-into link; the destination owns the moved notes. A live run uses the task maintenance lock, guarded writes, a scoped commit, and vault sync; --dry-run prints the same plan without changing the vault.")
        .after_help("Examples:\n  bob task merge -i db:upgrade inbox:bench\n                                 Fold one task into ^upgrade\n  bob task merge -i db:upgrade -D 'future work' inbox:bench\n                                 Fold into ^upgrade's FUTURE WORK drawer\n  bob task merge -i dev -s 'Future Work' mac_inbox:ws-proto\n                                 Fold into dev.md's ## Future Work\n  bob task merge -d -f json -i db:upgrade inbox:bench\n                                 Preview the same merge as JSON\n\nA task locator is note:block-id or [[note#^block-id]]. Quote capture drafts that use >: bob capture '@a+x > @c'.")
        .disable_help_flag(true)
        .arg(Arg::new("into").long("into").short('i').value_name("TASK|NOTE").required(true).help("A task, or a note when --section is given"))
        .arg(Arg::new("drawer").long("drawer").short('D').value_name("TITLE").conflicts_with("section").help("T's drawer, exact title; created if missing"))
        .arg(Arg::new("section").long("section").short('s').value_name("TITLE").conflicts_with("drawer").help("The note's H2, created if missing"))
        .arg(Arg::new("dry-run").long("dry-run").short('d').action(ArgAction::SetTrue).help("Plan and print only; never lock, sync, create, or write"))
        .arg(Arg::new("format").long("format").short('f').value_name("FORMAT").value_parser(["human", "json"]).default_value("human").help("Output format: human or json"))
        .arg(Arg::new("help").long("help").short('h').action(ArgAction::Help).help("Show help"))
        .arg(Arg::new("offline").long("offline").short('o').action(ArgAction::SetTrue).help("Skip both sync cycles; commit locally without pushing"))
        .arg(Arg::new("pull-in").long("pull-in").short('p').value_name("PICKS").help("all, none, or a comma list of offer numbers and keys"))
        .arg(Arg::new("retry-timeout").long("retry-timeout").short('r').value_name("SECONDS").value_parser(clap::value_parser!(u64)).default_value("60").help("Lock wait and re-plan budget after concurrent edits; 0 fails fast"))
        .arg(Arg::new("sources").value_name("TASK").required(true).num_args(1..).help("Source task locators in typed order"))
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
        pull_choice: None,
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
    let pull_in = match matches
        .get_one::<String>("pull-in")
        .map(|raw| parse_pull_in_picks(raw))
    {
        None => None,
        Some(Ok(picks)) => {
            report.pull_choice = Some(pull_choice_name(&picks));
            Some(picks)
        }
        Some(Err(message)) => {
            report.error =
                Some(("usage".into(), "merge_pull_in_invalid".into(), message));
            print_report(&report);
            return 2;
        }
    };
    let into_raw = matches
        .get_one::<String>("into")
        .cloned()
        .unwrap_or_default();
    let drawer_raw = matches.get_one::<String>("drawer").cloned();
    let section_raw = matches.get_one::<String>("section").cloned();
    let source_texts: Vec<String> = matches
        .get_many::<String>("sources")
        .map(|values| values.cloned().collect())
        .unwrap_or_default();
    if dry_run {
        match plan_once(
            &report.vault,
            &into_raw,
            drawer_raw.as_deref(),
            section_raw.as_deref(),
            &source_texts,
            pull_in.as_ref(),
        ) {
            Ok((plan, choice)) => {
                report.plan = Some(plan);
                if report.pull_choice.is_none() {
                    report.pull_choice = Some(choice);
                }
            }
            Err((stage, code, message)) => {
                report.error = Some((stage, code, message));
            }
        }
        print_report(&report);
        return if report.error.is_some() { 1 } else { 0 };
    }
    execute_live(
        &mut report,
        &into_raw,
        drawer_raw.as_deref(),
        section_raw.as_deref(),
        &source_texts,
        pull_in.as_ref(),
        retry,
    )
}

fn pull_choice_name(picks: &PullIn) -> String {
    match picks {
        PullIn::All => "all".to_string(),
        PullIn::None => "none".to_string(),
        PullIn::Tokens { .. } => "subset".to_string(),
    }
}

fn parse_pull_in_picks(raw: &str) -> Result<PullIn, String> {
    if raw == "all" {
        return Ok(PullIn::All);
    }
    if raw == "none" {
        return Ok(PullIn::None);
    }
    let mut numbers = Vec::new();
    let mut keys = Vec::new();
    for token in raw.split(',') {
        let token = token.trim();
        if token.is_empty() {
            return Err(
                "--pull-in has an empty token; use all, none, or a comma list of offer numbers and keys"
                    .to_string(),
            );
        }
        if token.bytes().all(|byte| byte.is_ascii_digit()) {
            let number: usize = token.parse().map_err(|_| {
                format!(
                    "--pull-in offer number `{token}` is not a valid number"
                )
            })?;
            numbers.push(number);
        } else {
            keys.push(token.to_string());
        }
    }
    Ok(PullIn::Tokens { numbers, keys })
}

fn parse_task_locator(raw: &str) -> Option<TaskLocator> {
    TaskLocator::parse(raw)
}

fn merge_container(
    into_raw: &str,
    drawer_raw: Option<&str>,
    section_raw: Option<&str>,
) -> Result<MergeContainer, MergeError> {
    if let Some(drawer) = drawer_raw {
        let Some(locator) = parse_task_locator(into_raw) else {
            return Err(MergeError::new(
                "merge_into_note_without_section",
                "a note holds tasks; use `bob task move -t dev`, or name a section with -s",
            ));
        };
        return Ok(MergeContainer::Drawer(
            locator,
            DrawerSelector {
                text: drawer.to_string(),
                exact: true,
            },
        ));
    }
    if let Some(section) = section_raw {
        let note = note_query(into_raw);
        if note.is_empty() {
            return Err(MergeError::new(
                "task_not_found",
                format!("no note matches `{into_raw}`"),
            ));
        }
        let exact = is_quoted_literal(into_raw);
        return Ok(MergeContainer::Section {
            note,
            selector: DrawerSelector {
                text: section.to_string(),
                exact,
            },
        });
    }
    let Some(locator) = parse_task_locator(into_raw) else {
        return Err(MergeError::new(
            "merge_into_note_without_section",
            "a note holds tasks; use `bob task move -t dev`, or name a section with -s",
        ));
    };
    Ok(MergeContainer::Task(locator))
}

fn note_query(raw: &str) -> String {
    let text = raw.trim();
    let inner = text
        .strip_prefix("[[")
        .and_then(|s| s.strip_suffix("]]"))
        .unwrap_or(text);
    if let Some((note, _)) = inner.split_once("#^") {
        return note.trim().to_string();
    }
    if let Some((note, fragment)) = inner.rsplit_once('#')
        && !fragment.is_empty()
    {
        let _ = fragment;
        return note.trim().to_string();
    }
    inner.trim().to_string()
}

fn is_quoted_literal(raw: &str) -> bool {
    let text = raw.trim();
    text.starts_with('"') && text.ends_with('"') && text.len() >= 2
}

fn parse_sources(raws: &[String]) -> Result<Vec<TaskLocator>, MergeError> {
    let mut sources = Vec::with_capacity(raws.len());
    for raw in raws {
        match parse_task_locator(raw) {
            Some(locator) => sources.push(locator),
            None => {
                return Err(MergeError::new(
                    "task_not_found",
                    format!(
                        "`{raw}` is not a task locator; use note:block-id or [[note#^block-id]]"
                    ),
                ));
            }
        }
    }
    Ok(sources)
}

fn initial_selection(pull_in: Option<&PullIn>) -> SuccessorSelection {
    match pull_in {
        None | Some(PullIn::None) => SuccessorSelection::None,
        Some(PullIn::All) => SuccessorSelection::All,
        Some(PullIn::Tokens { keys, .. }) => {
            SuccessorSelection::Keys(keys.iter().cloned().collect())
        }
    }
}

fn finalize_selection(
    pull_in: Option<&PullIn>,
    plan: &MergePlan,
) -> Result<SuccessorSelection, MergeError> {
    let Some(PullIn::Tokens { numbers, keys }) = pull_in else {
        return Ok(initial_selection(pull_in));
    };
    if numbers.is_empty() {
        return Ok(SuccessorSelection::Keys(keys.iter().cloned().collect()));
    }
    let offers: Vec<_> = plan
        .records
        .iter()
        .flat_map(|record| &record.offers)
        .collect();
    let mut mapped = keys.clone();
    let mut bad = Vec::new();
    for number in numbers {
        match offers.iter().find(|offer| offer.n == *number) {
            Some(offer) => mapped.push(offer.key.clone()),
            None => bad.push(number.to_string()),
        }
    }
    if !bad.is_empty() {
        return Err(MergeError::new(
            "unknown_pull_in_pick",
            format!(
                "unknown offer number{}: {}; valid offers are {}",
                if bad.len() == 1 { "" } else { "s" },
                bad.join(", "),
                if offers.is_empty() {
                    "none".to_string()
                } else {
                    offers
                        .iter()
                        .map(|offer| offer.n.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
        ));
    }
    let surviving: BTreeSet<String> =
        offers.iter().map(|offer| offer.key.clone()).collect();
    let unknown_keys: Vec<String> = mapped
        .iter()
        .filter(|key| !surviving.contains(*key))
        .cloned()
        .collect();
    if !unknown_keys.is_empty() {
        return Err(MergeError::new(
            "unknown_pull_in_pick",
            format!(
                "unknown offer key{}: {}",
                if unknown_keys.len() == 1 { "" } else { "s" },
                unknown_keys.join(", ")
            ),
        ));
    }
    Ok(SuccessorSelection::Keys(mapped.into_iter().collect()))
}

fn merge_context<'a>(
    snapshot: &'a VaultSnapshot,
    vault: &Path,
    task_settings: &'a note_tasks::NoteTaskSettings,
    note_index: &'a NoteIndex,
    drawer_catalog: &'a BTreeMap<String, capture_drawers::DrawerCatalogRecord>,
    relation_policy: &'a MergeRelationPolicy,
    successor_labels: &'a [config::relations::CancelLabel],
) -> (MergeContext<'a>, PathBuf) {
    let daily_abs = pomodoro::day_file_for(vault);
    let daily_rel = daily_abs
        .strip_prefix(vault)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| daily_abs.clone());
    let today = bob_env::current_datetime().date();
    let context = MergeContext {
        today,
        daily_path: daily_rel.clone(),
        daily_read: DailyNoteReadState::NotRead,
        task_settings,
        note_index,
        drawer_catalog,
        relation_policy: *relation_policy,
        successor_labels,
    };
    let _ = snapshot;
    (context, daily_rel)
}

fn plan_once(
    vault: &Path,
    into_raw: &str,
    drawer_raw: Option<&str>,
    section_raw: Option<&str>,
    source_texts: &[String],
    pull_in: Option<&PullIn>,
) -> Result<(MergePlan, String), (String, String, String)> {
    let scan = scan_vault(vault).map_err(|error| {
        ("plan".to_string(), "merge_scan_failed".to_string(), error)
    })?;
    let container = merge_container(into_raw, drawer_raw, section_raw)
        .map_err(|error| {
            ("plan".to_string(), error.code.to_string(), error.message)
        })?;
    let sources = parse_sources(source_texts).map_err(|error| {
        ("plan".to_string(), error.code.to_string(), error.message)
    })?;
    let task_settings = note_tasks::read_settings(vault);
    let relation_settings =
        config::load_relations_config(&config::config_path())
            .unwrap_or_default();
    let relation_policy = MergeRelationPolicy {
        linked_from_lines: relation_settings.linked_from_lines,
        linked_from_mentions: relation_settings.linked_from_mentions,
    };
    let drawer_catalog =
        capture_drawers::load_drawer_catalog(vault).unwrap_or_default();
    let note_index = NoteIndex::from_paths(scan.snapshot.notes.keys().cloned());
    let (context, _) = merge_context(
        &scan.snapshot,
        vault,
        &task_settings,
        &note_index,
        &drawer_catalog,
        &relation_policy,
        &relation_settings.successor_labels,
    );
    let request = MergeRequest {
        sources,
        target: container,
        label: MERGE_LABEL_FALLBACK.to_string(),
        gesture: MergeGesture::Explicit,
        selection: initial_selection(pull_in),
    };
    let probe = plan_task_merge(&scan.snapshot, &request, &context).map_err(
        |error| ("plan".to_string(), error.code.to_string(), error.message),
    )?;
    let selection = finalize_selection(pull_in, &probe).map_err(|error| {
        ("plan".to_string(), error.code.to_string(), error.message)
    })?;
    if selection == request.selection {
        let choice = pull_choice_for_report(pull_in);
        return Ok((probe, choice));
    }
    let final_request = MergeRequest {
        selection,
        ..request
    };
    let plan = plan_task_merge(&scan.snapshot, &final_request, &context)
        .map_err(|error| {
            ("plan".to_string(), error.code.to_string(), error.message)
        })?;
    let choice = pull_choice_for_report(pull_in);
    Ok((plan, choice))
}

fn pull_choice_for_report(pull_in: Option<&PullIn>) -> String {
    match pull_in {
        None => "none".to_string(),
        Some(picks) => pull_choice_name(picks),
    }
}

fn execute_live(
    report: &mut Report,
    into_raw: &str,
    drawer_raw: Option<&str>,
    section_raw: Option<&str>,
    source_texts: &[String],
    pull_in: Option<&PullIn>,
    retry: u64,
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
                    Some(("plan".into(), "merge_scan_failed".into(), error));
                break;
            }
        };
        let container = match merge_container(into_raw, drawer_raw, section_raw)
        {
            Ok(container) => container,
            Err(error) => {
                report.error =
                    Some(("plan".into(), error.code.into(), error.message));
                break;
            }
        };
        let sources = match parse_sources(source_texts) {
            Ok(sources) => sources,
            Err(error) => {
                report.error =
                    Some(("plan".into(), error.code.into(), error.message));
                break;
            }
        };
        let task_settings = note_tasks::read_settings(&report.vault);
        let relation_settings =
            config::load_relations_config(&config::config_path())
                .unwrap_or_default();
        let relation_policy = MergeRelationPolicy {
            linked_from_lines: relation_settings.linked_from_lines,
            linked_from_mentions: relation_settings.linked_from_mentions,
        };
        let drawer_catalog =
            capture_drawers::load_drawer_catalog(&report.vault)
                .unwrap_or_default();
        let note_index =
            NoteIndex::from_paths(scan.snapshot.notes.keys().cloned());
        let (context, _) = merge_context(
            &scan.snapshot,
            &report.vault,
            &task_settings,
            &note_index,
            &drawer_catalog,
            &relation_policy,
            &relation_settings.successor_labels,
        );
        let initial = MergeRequest {
            sources,
            target: container,
            label: MERGE_LABEL_FALLBACK.to_string(),
            gesture: MergeGesture::Explicit,
            selection: initial_selection(pull_in),
        };
        let probe = match plan_task_merge(&scan.snapshot, &initial, &context) {
            Ok(plan) => plan,
            Err(error) => {
                report.error =
                    Some(("plan".into(), error.code.into(), error.message));
                break;
            }
        };
        let selection = match finalize_selection(pull_in, &probe) {
            Ok(selection) => selection,
            Err(error) => {
                report.error =
                    Some(("plan".into(), error.code.into(), error.message));
                break;
            }
        };
        let plan = if selection == initial.selection {
            probe
        } else {
            let final_request = MergeRequest {
                selection,
                ..initial
            };
            match plan_task_merge(&scan.snapshot, &final_request, &context) {
                Ok(plan) => plan,
                Err(error) => {
                    report.error =
                        Some(("plan".into(), error.code.into(), error.message));
                    break;
                }
            }
        };
        report.pull_choice = Some(pull_choice_for_report(pull_in));
        let write_plan = match build_write_plan(&report.vault, &scan, &plan) {
            Ok(plan) => plan,
            Err(error) => {
                report.error =
                    Some(("apply".into(), "write_plan_failed".into(), error));
                break;
            }
        };
        if write_plan.outputs.is_empty() {
            report.plan = Some(plan);
            break;
        }
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
            .map(merge_subject)
            .unwrap_or_else(|| "bob task merge".into());
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
    plan: &MergePlan,
) -> Result<WritePlan, String> {
    let order: BTreeMap<&PathBuf, usize> = plan
        .write_order
        .iter()
        .enumerate()
        .map(|(index, path)| (path, index))
        .collect();
    let mut postimages = plan.postimages.iter().collect::<Vec<_>>();
    postimages.sort_by(|(left, _), (right, _)| {
        order
            .get(*left)
            .copied()
            .unwrap_or(usize::MAX)
            .cmp(&order.get(*right).copied().unwrap_or(usize::MAX))
            .then_with(|| left.cmp(right))
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

fn merge_subject(plan: &MergePlan) -> String {
    let target = plan
        .records
        .first()
        .map(|record| {
            let fallback = record.target.path.to_string_lossy().into_owned();
            let stem = Path::new(&record.target.path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(&fallback);
            if record.target.kind == "section" {
                let section = record.target.section.as_deref().unwrap_or("");
                format!("{stem}#{section}")
            } else if let Some(block) = record.target.block_id.as_deref() {
                format!("{stem}#^{block}")
            } else {
                stem.to_string()
            }
        })
        .unwrap_or_else(|| "merge".to_string());
    let count = plan.records.len();
    format!(
        "bob task merge -> {target} ({count} {})",
        if count == 1 { "task" } else { "tasks" }
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

fn display_relative(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => {
                Some(part.to_string_lossy().into_owned())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn status_name(status: char) -> &'static str {
    match status {
        ' ' => "Todo",
        '*' => "Next",
        '/' => "In Progress",
        '?' => "Ready",
        '-' => "Cancelled",
        'x' | 'X' => "Done",
        _ => "Closed",
    }
}

fn print_report(report: &Report) {
    if report.format == Format::Json {
        println!("{}", json_document(report));
        return;
    }
    let styler = Styler::detect();
    if let Some((_, _, message)) = &report.error
        && report.plan.is_none()
    {
        eprintln!("{} plan: {message}", styler.red("✗"));
        print_git_footer(report, &styler);
        return;
    }
    let Some(plan) = &report.plan else {
        println!("No merge planned.");
        return;
    };
    let action = if report.dry_run {
        "Would merge"
    } else {
        "Merged"
    };
    let target_label = plan
        .records
        .first()
        .map(|record| {
            if record.target.kind == "section" {
                record.target.section.as_deref().unwrap_or("")
            } else {
                record.target.title.as_deref().unwrap_or("")
            }
        })
        .unwrap_or("");
    let target_path = plan
        .records
        .first()
        .map(|record| display_relative(&record.target.path))
        .unwrap_or_default();
    let container_suffix = plan
        .records
        .first()
        .and_then(|record| {
            if record.target.kind == "drawer" {
                record
                    .target
                    .drawer
                    .as_deref()
                    .map(|drawer| format!(" › {drawer}"))
            } else if record.target.kind == "section" {
                record
                    .target
                    .section
                    .as_deref()
                    .map(|section| format!(" › {section}"))
            } else {
                None
            }
        })
        .unwrap_or_default();
    let count = plan.records.len();
    println!(
        "{}",
        styler.bold(&format!(
            "⤵ {action} {count} {} into “{target_label}” · {target_path}{container_suffix}",
            if count == 1 { "task" } else { "tasks" }
        ))
    );
    for record in &plan.records {
        let outcome = match record.outcome {
            MergeOutcome::Merged => {
                status_name(record.previous_status.unwrap_or(' '))
            }
            MergeOutcome::Unchanged => "Unchanged",
            MergeOutcome::AlreadyClosed => "Already closed",
        };
        let moved = if record.moved_notes > 0 {
            format!(
                " · {} {} moved",
                record.moved_notes,
                if record.moved_notes == 1 {
                    "note"
                } else {
                    "notes"
                }
            )
        } else if record.moved_lines > 0 {
            " · title only".to_string()
        } else {
            String::new()
        };
        println!(
            "  ⊘ {}   {}:{} · {outcome} → Cancelled{moved}",
            record.source.title,
            display_relative(&record.source.path),
            record.source.line
        );
        for dependent in &record.dependents_repointed {
            println!("  ⛓ {dependent}   now waits on “{target_label}”");
        }
        if !record.prerequisites_left.is_empty() {
            println!(
                "  ⚠ “{}” waited on {}; “{target_label}” does not · add &note:id",
                record.source.title,
                record.prerequisites_left.join(", ")
            );
        }
        if !record.successors_orphaned.is_empty() {
            println!(
                "  ⚠ {} task(s) start after “{}”; they won't be offered when “{target_label}” finishes · relink with ~",
                record.successors_orphaned.len(),
                record.source.title
            );
        }
        for offer in &record.offers {
            if offer.selected {
                println!(
                    "  ◌ {} link removed  offer {}: link “{target_label}” under {}",
                    offer.entry_name, offer.n, offer.entry_name
                );
            } else {
                println!(
                    "  ◌ {} link removed  offer {}: link “{target_label}” under {} (-p {})",
                    offer.entry_name, offer.n, offer.entry_name, offer.n
                );
            }
        }
    }
    if !plan.repaired_files.is_empty() {
        let links: usize =
            plan.repaired_files.iter().map(|file| file.link_count).sum();
        let notes = plan.repaired_files.len();
        println!("  ↻ repaired {links} link(s) in {notes} note(s)");
    }
    let unpicked: Vec<_> = plan
        .records
        .iter()
        .flat_map(|record| &record.offers)
        .filter(|offer| !offer.selected)
        .collect();
    if !report.dry_run
        && !unpicked.is_empty()
        && let Some(record) = plan.records.first()
    {
        let stem = Path::new(&record.target.path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("");
        if let Some(block) = record.target.block_id.as_deref() {
            println!("  hint: link “{target_label}” later with bob capture '@{stem}:{block}'");
        } else {
            println!("  hint: link “{target_label}” later with bob capture '@{stem}'");
        }
    }
    print_git_footer(report, &styler);
    if let Some((stage, _, message)) = &report.error {
        eprintln!("{} {stage}: {message}", styler.red("✗"));
    }
}

fn print_git_footer(report: &Report, styler: &Styler) {
    if report.dry_run {
        return;
    }
    if let Some(commit) = &report.commit {
        let sha = commit
            .sha
            .as_deref()
            .map(short_sha)
            .unwrap_or_else(|| "no changes".to_string());
        println!(
            "  {} Committed {}  {}",
            styler.green("✓"),
            sha,
            commit.subject
        );
    }
    if !report.applied.is_empty() {
        println!(
            "  Applied {} file(s): {}",
            report.applied.len(),
            report.applied.join(", ")
        );
    }
}

fn json_document(report: &Report) -> Value {
    let merges = report
        .plan
        .as_ref()
        .map(|plan| {
            plan.records
                .iter()
                .map(|record| {
                    json!({
                        "source": {"path": display_relative(&record.source.path), "block_id": record.source.block_id, "line": record.source.line, "title": record.source.title},
                        "target": record.target,
                        "label": record.label,
                        "outcome": record.outcome,
                        "previous_status": record.previous_status.map(|status| status.to_string()),
                        "moved_lines": record.moved_lines,
                        "moved_notes": record.moved_notes,
                        "dependents_repointed": record.dependents_repointed,
                        "prerequisites_left": record.prerequisites_left,
                        "successors_orphaned": record.successors_orphaned,
                        "links_pruned": record.links_pruned,
                        "offers": record.offers,
                        "message": record.message,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let repaired: Vec<Value> = report
        .plan
        .as_ref()
        .map(|plan| {
            plan.repaired_files
                .iter()
                .map(|file: &RepairedFile| {
                    json!({
                        "path": display_relative(&file.path),
                        "link_count": file.link_count,
                        "dependency_token_count": file.dependency_token_count,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let (offer_count, picked_count) = report
        .plan
        .as_ref()
        .map(|plan| {
            let offers = plan
                .records
                .iter()
                .flat_map(|record| &record.offers)
                .collect::<Vec<_>>();
            (
                offers.len(),
                offers.iter().filter(|offer| offer.selected).count(),
            )
        })
        .unwrap_or((0, 0));
    json!({
        "schema_version": 1,
        "ok": report.error.is_none(),
        "dry_run": report.dry_run,
        "merges": merges,
        "pull_in": {"choice": report.pull_choice, "offer_count": offer_count, "picked_count": picked_count},
        "warnings": [],
        "repaired_files": repaired,
        "git": {
            "mode": report.mode,
            "pre_sync": report.pre_sync,
            "commit": report.commit.as_ref().map(|commit| json!({"sha": commit.sha, "subject": commit.subject, "paths": commit.paths})),
            "post_sync": report.post_sync,
        },
        "applied_paths": report.applied,
        "deferred_paths": report.deferred,
        "recovery_directory": report.recovery,
        "error": report.error.as_ref().map(|(stage, code, message)| json!({"stage": stage, "code": code, "message": message, "hint": null})),
    })
}

fn print_clap_error(error: clap::Error) -> i32 {
    let code = error.exit_code();
    let _ = error.print();
    code
}
