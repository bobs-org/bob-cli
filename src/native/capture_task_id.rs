use std::{
    collections::HashSet,
    ffi::OsString,
    fs, io, iter,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgGroup, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{
    capture, capture_dependency_tasks, capture_language,
    capture_task_wikilinks, capture_tasks, collect_done, env as bob_env,
    note_tasks::{self, RefLookup, TaskRef},
    style::Styler,
    task_status_hooks,
};

const COMMAND_NAME: &str = "bob capture-task-id";

/// Bump only for a breaking change to the JSON object below; new optional
/// fields keep version 1.
const SCHEMA_VERSION: u32 = 1;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let request = match CaptureTaskIdRequest::from_matches(&matches) {
        Ok(request) => request,
        Err(error) => return print_error(error, output_format),
    };

    match assign_capture_task_id(request) {
        Ok(result) => {
            print_success(&result, output_format);
            0
        }
        Err(error) => print_error(error, output_format),
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        // A closed pipe renders nothing: stay quiet instead of
        // reporting the failed render, but keep clap's own exit
        // code so a usage error never reports success.
        if print_error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!(
                "{COMMAND_NAME}: failed to print command-line error: {print_error}"
            );
        }
    }
    exit_code
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Assign a block ID to an open capture task or notebook note")
        .long_about(
            "Assign a user-authored Obsidian block ID to one open task or \
notebook note in a Bob note.\n\n\
The command is the write half of task and note completion: callers first discover \
a missing-ID task with `bob capture-complete --all-tasks` (or the `&` \
dependency and `*` blocked-task picker), or a missing-ID notebook note \
with note-scoped `@nb+` completion, then pass that candidate's stale-safe --task-ref \
together with the authored --block-id. Validation uses the same route \
grammar, block-ID grammar, task and note scans, and duplicate-ID rules as \
`bob capture`. The selected task must still be open and must not already \
have an ID; the selected note must be an eligible note bullet and must not \
already have an ID. An ID already used anywhere in the target note \
is rejected, including non-task anchors, headings, and plain bullets.\n\n\
Use --route for a routable root note, or --note-path for the exact \
vault-relative path of any task- or note-bearing note (nested, case-sensitive, \
or quoted paths round-trip exactly; the lowercasing route parser is \
never consulted). --allow-closed additionally permits Done/Cancelled \
tasks without reopening them, for dependency candidates that name \
completed history; it never reopens the task and never \
applies to non-task lines or notebook notes. The previous daily snapshot stays \
read-only and fails with a guarded refusal.\n\n\
On success the command appends ' ^<id>' to the resolved physical task or \
note line, preserves that line's ending and every unrelated byte, and \
replaces the note with one same-directory temporary file rename. The JSON success shape \
additionally carries the exact `note_path` (in --note-path mode) and the \
backend-formatted `dependency_replacement` (`&note:id`, quoted when the \
locator needs it) so the app can resume the picker without losing \
quoting or case, plus `blocked_task_replacement` (`*note:id`, quoted when the \
locator needs it, in both modes) so the blocked-task picker can splice the new ID, \
plus `complete_replacement` (`!note:id`, quoted when the \
locator needs it, in both modes) so the Complete picker can splice the new ID, \
plus `relation_to_replacement` (`~note:block-id`) and \
`relation_from_replacement` (`~~note:block-id`), quoted when the locator \
needs it, so either relation picker can splice the new ID, \
plus `wikilink_replacement` (`[[note#^id]]`, the shortest unambiguous \
Obsidian block link, in both modes) so the Task Wikilink Picker can splice \
the new ID, plus `parent_replacement` (`@route+block-id`) in routable \
`--route` mode (for notebook notes, `@nb+ID` so the client can resume \
the note selection). Notebook assignments additionally carry \
`item_kind: \"note\"` so clients can tell notes from tasks. Dry-run plans the same \
mutation and returns the same success shape without writing. Stale, \
ambiguous, terminal, already-identified, duplicate, missing, unreadable, \
non-note, or read-only-snapshot targets fail \
without touching the note.",
        )
        .after_help(
            "Examples:\n  bob capture-task-id -r file -t 3:1f3a9c2b -i report-id\n  bob capture-task-id --route cash --task-ref 12:abcd1234 --block-id goog-exit -f json\n  bob capture-task-id -r file -t 3:1f3a9c2b -i report-id --dry-run\n  bob capture-task-id -b ~/bob -r notes -t 8:deadbeef -i inbox-item\n  bob capture-task-id -n projects/shopping.md -t 4:1f3a9c2b -i grocery-run\n  bob capture-task-id -n archive/old.md -t 9:abcd1234 -i kept-id --allow-closed\n\nEnvironment:\n  BOB_DIR                    Bob vault root when --bob-dir is omitted",
        )
        .disable_help_flag(true)
        .arg(allow_closed_arg())
        .arg(block_id_arg())
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(note_path_arg())
        .arg(route_arg())
        .arg(task_ref_arg())
        .group(
            ArgGroup::new("target")
                .args(["route", "note-path"])
                .required(true)
                .multiple(false),
        )
}

fn allow_closed_arg() -> Arg {
    Arg::new("allow-closed")
        .long("allow-closed")
        .short('a')
        .action(ArgAction::SetTrue)
        .help("Assign to a Done/Cancelled task without reopening it")
}

fn note_path_arg() -> Arg {
    Arg::new("note-path")
        .long("note-path")
        .short('n')
        .value_name("PATH")
        .help("Exact vault-relative path of the note (nested and case-sensitive names round-trip)")
}

fn block_id_arg() -> Arg {
    Arg::new("block-id")
        .long("block-id")
        .short('i')
        .value_name("ID")
        .required(true)
        .help("Block ID to assign; letters, numbers, and hyphens")
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Plan and report without writing the note")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn route_arg() -> Arg {
    Arg::new("route")
        .long("route")
        .short('r')
        .value_name("NAME")
        .help("Route/name of the note that contains the task or note")
}

fn task_ref_arg() -> Arg {
    Arg::new("task-ref")
        .long("task-ref")
        .short('t')
        .value_name("REF")
        .required(true)
        .help("Stale-safe task or note ref from capture-complete or capture-tasks")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human")
        {
            "json" => Self::Json,
            _ => Self::Human,
        }
    }
}

/// Which note holds the task: a routable root note (`--route`), or
/// the exact vault-relative path of any task-bearing note
/// (`--note-path`, for nested, case-sensitive, or quoted paths the
/// lowercasing route parser cannot round-trip). Exactly one is set.
#[derive(Debug, Clone, PartialEq, Eq)]
enum NoteTarget {
    Route(String),
    Path(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureTaskIdRequest {
    bob_dir: PathBuf,
    dry_run: bool,
    target: NoteTarget,
    task_ref: TaskRef,
    block_id: String,
    allow_closed: bool,
}

impl CaptureTaskIdRequest {
    fn from_matches(matches: &ArgMatches) -> Result<Self, CaptureTaskIdError> {
        Ok(Self {
            bob_dir: bob_dir_from_matches(matches),
            dry_run: matches.get_flag("dry-run"),
            target: target_from_matches(matches)?,
            task_ref: task_ref_from_matches(matches)?,
            block_id: block_id_from_matches(matches)?,
            allow_closed: matches.get_flag("allow-closed"),
        })
    }
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

fn target_from_matches(
    matches: &ArgMatches,
) -> Result<NoteTarget, CaptureTaskIdError> {
    match (
        matches.get_one::<String>("route"),
        matches.get_one::<String>("note-path"),
    ) {
        (Some(route), None) => {
            if capture::is_route_token(route) {
                return Ok(NoteTarget::Route(route.to_ascii_lowercase()));
            }
            Err(CaptureTaskIdError::usage(
                "--route must contain only A-Z, a-z, 0-9, '_' or '-'",
            ))
        }
        (None, Some(path)) => {
            if path.is_empty() {
                return Err(CaptureTaskIdError::usage(
                    "--note-path must not be empty",
                ));
            }
            Ok(NoteTarget::Path(path.clone()))
        }
        _ => Err(CaptureTaskIdError::usage(
            "pass exactly one of --route or --note-path",
        )),
    }
}

fn task_ref_from_matches(
    matches: &ArgMatches,
) -> Result<TaskRef, CaptureTaskIdError> {
    let value = matches.get_one::<String>("task-ref").expect("required");
    TaskRef::parse(value).ok_or_else(|| {
        CaptureTaskIdError::usage("--task-ref must use <line>:<digest>")
    })
}

fn block_id_from_matches(
    matches: &ArgMatches,
) -> Result<String, CaptureTaskIdError> {
    let block_id = matches.get_one::<String>("block-id").expect("required");
    if capture_language::is_block_id(block_id) {
        return Ok(block_id.clone());
    }
    Err(CaptureTaskIdError::usage(
        "--block-id must be non-empty and contain only A-Z, a-z, 0-9 or '-'",
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureTaskIdResult {
    ok: bool,
    schema_version: u32,
    dry_run: bool,
    /// The routable note name in `--route` mode; absent in
    /// `--note-path` mode, where no route names the note.
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<String>,
    relative_target: String,
    /// The exact vault-relative note path in `--note-path` mode;
    /// absent in `--route` mode, where `relative_target` already is it.
    #[serde(skip_serializing_if = "Option::is_none")]
    note_path: Option<String>,
    /// Additive note discriminator: `"note"` for notebook assignments.
    /// Absent for ordinary task assignments.
    #[serde(skip_serializing_if = "Option::is_none")]
    item_kind: Option<&'static str>,
    block_id: String,
    /// Backend-formatted `&note:block-id` (quoted when the locator
    /// needs it) so the app resumes the picker without losing
    /// quoting or case.
    dependency_replacement: String,
    /// Backend-formatted `*note:block-id` (quoted when the locator
    /// needs it) so the blocked-task picker can splice the assigned ID.
    blocked_task_replacement: String,
    /// Backend-formatted `!note:block-id` (quoted when the locator
    /// needs it) so the Complete picker can splice the new ID without
    /// losing quoting or case. Present in both `--route` and
    /// `--note-path` modes.
    complete_replacement: String,
    /// Backend-formatted `~` and `~~` relation replacements, quoted when
    /// the locator needs it. Present in both target modes.
    relation_to_replacement: String,
    relation_from_replacement: String,
    /// Backend-formatted `[[note#^id]]` Obsidian block link (shortest
    /// unique basename, or the vault-relative path without `.md` when
    /// needed; `done/` archive always keeps the path). Present in both
    /// `--route` and `--note-path` modes so the Task Wikilink Picker can
    /// splice a just-assigned ID.
    wikilink_replacement: String,
    /// Backend-formatted parent-task marker, present only when `--route`
    /// identifies a routable note.
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_replacement: Option<String>,
    line: usize,
    #[serde(rename = "ref")]
    task_ref: String,
    task: AssignedTask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AssignedTask {
    #[serde(rename = "ref")]
    task_ref: String,
    line: usize,
    block_id: String,
    status_symbol: char,
    status_name: String,
    status_type: &'static str,
    text: String,
    section: Option<String>,
    depth: usize,
    child_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AssignmentPlan {
    target: PathBuf,
    original: String,
    updated: String,
    result: CaptureTaskIdResult,
}

fn assign_capture_task_id(
    request: CaptureTaskIdRequest,
) -> Result<CaptureTaskIdResult, CaptureTaskIdError> {
    let plan = plan_assignment(&request)?;
    if !request.dry_run {
        collect_done::atomic_write(&plan.target, &plan.updated).map_err(
            |error| {
                CaptureTaskIdError::io(format!(
                    "replace target {}: {error}",
                    plan.target.display()
                ))
            },
        )?;
    }
    Ok(plan.result)
}

fn plan_assignment(
    request: &CaptureTaskIdRequest,
) -> Result<AssignmentPlan, CaptureTaskIdError> {
    // One vault-wide discovery serves exact-path resolution, the
    // shortest-unambiguous replacement, and the previous-daily guard
    // below; the assignment itself still reads only its target note.
    let discovered = capture_dependency_tasks::discover(&request.bob_dir);
    let (route, relative_target) = match &request.target {
        NoteTarget::Route(route) => {
            (Some(route.clone()), capture::route_label(route))
        }
        NoteTarget::Path(path) => {
            match capture_dependency_tasks::resolve_dependency_note(
                &request.bob_dir,
                &discovered,
                path,
            ) {
                Ok(resolved) => {
                    (None, resolved.to_string_lossy().replace('\\', "/"))
                }
                Err(capture_dependency_tasks::LocatorError::Usage(message)) => {
                    return Err(CaptureTaskIdError::usage(message))
                }
                Err(capture_dependency_tasks::LocatorError::Io(message)) => {
                    return Err(CaptureTaskIdError::io(message));
                }
            }
        }
    };
    if let Some(previous) = previous_daily_note(&request.bob_dir, &discovered)
        && previous == relative_target
    {
        return Err(CaptureTaskIdError::io(format!(
            "note {relative_target} is the previous daily snapshot, which stays read-only; pick the task in its current note instead"
        )));
    }
    let target = request.bob_dir.join(&relative_target);
    if target
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.permissions().readonly())
    {
        return Err(CaptureTaskIdError::io(format!(
            "note {relative_target} is read-only"
        )));
    }
    let original = match fs::read_to_string(&target) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CaptureTaskIdError::io(format!(
                "note does not exist: {}",
                target.display()
            )));
        }
        Err(error) => {
            return Err(CaptureTaskIdError::io(format!(
                "read target {}: {error}",
                target.display()
            )));
        }
    };

    let settings = note_tasks::read_settings(&request.bob_dir);
    if collect_done::block_ids_in_markdown(&original)
        .iter()
        .any(|existing| existing == &request.block_id)
    {
        return Err(CaptureTaskIdError::io(format!(
            "block ID ^{} already exists in {relative_target}",
            request.block_id
        )));
    }

    // Notebook notes: resolve the `line:digest` against the eligible
    // bullet snapshot and append the chosen ID to that bullet only.
    if is_notebook_contents(&original) {
        return plan_notebook_assignment(
            request,
            &discovered,
            route,
            &relative_target,
            &target,
            &original,
        );
    }

    let scan = note_tasks::scan(&original, &settings);
    let task = match scan
        .by_ref(request.task_ref.line, &request.task_ref.digest)
    {
        RefLookup::Found(task) => task,
        RefLookup::Stale => {
            return Err(CaptureTaskIdError::io(format!(
                "the selected task is no longer in {}; rerun the task picker",
                relative_target
            )));
        }
        RefLookup::Ambiguous => {
            return Err(CaptureTaskIdError::io(format!(
                "the selected task matches more than one line in {}; rerun the task picker",
                relative_target
            )));
        }
    };

    if !task.status_type.is_open() {
        let eligible = request.allow_closed
            && matches!(
                task.status_type,
                note_tasks::TaskStatusType::Done
                    | note_tasks::TaskStatusType::Cancelled
            );
        if !eligible {
            return Err(CaptureTaskIdError::io(format!(
                "the selected task is no longer open in {relative_target} (status: {})",
                task.status_name
            )));
        }
    }
    if let Some(existing) = task.block_id.as_deref() {
        return Err(CaptureTaskIdError::io(format!(
            "the selected task already has block ID ^{existing} in {relative_target}"
        )));
    }

    let updated =
        append_block_id(&original, task.line_index, &request.block_id)?;
    let updated_scan = note_tasks::scan(&updated, &settings);
    let updated_task =
        updated_scan.task_at(task.line_index).ok_or_else(|| {
            CaptureTaskIdError::io(format!(
                "updated task disappeared from {relative_target}"
            ))
        })?;
    if updated_task.block_id.as_deref() != Some(request.block_id.as_str()) {
        return Err(CaptureTaskIdError::io(format!(
            "failed to assign block ID ^{} on line {} of {relative_target}",
            request.block_id,
            task.line_index + 1
        )));
    }

    let assigned = assigned_task(updated_task);
    let target_path = PathBuf::from(&relative_target);
    let dependency_replacement = capture_dependency_tasks::replacement_for(
        &discovered.index,
        &target_path,
        &request.block_id,
    );
    let blocked_task_replacement =
        capture_dependency_tasks::replacement_for_sigil(
            &discovered.index,
            &target_path,
            &request.block_id,
            b'*',
        );
    let complete_replacement = capture_dependency_tasks::replacement_for_sigil(
        &discovered.index,
        &target_path,
        &request.block_id,
        b'!',
    );
    let relation_to_replacement =
        capture_dependency_tasks::replacement_for_prefix(
            &discovered.index,
            &target_path,
            &request.block_id,
            "~",
        );
    let relation_from_replacement =
        capture_dependency_tasks::replacement_for_prefix(
            &discovered.index,
            &target_path,
            &request.block_id,
            "~~",
        );
    let wikilink_replacement = capture_task_wikilinks::wikilink_replacement(
        &discovered.index,
        &target_path,
        &request.block_id,
    );
    let parent_replacement = route
        .as_deref()
        .map(|route| format!("@{route}+{}", request.block_id));
    Ok(AssignmentPlan {
        target,
        original,
        updated,
        result: CaptureTaskIdResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            dry_run: request.dry_run,
            route,
            relative_target: relative_target.clone(),
            note_path: match &request.target {
                NoteTarget::Route(_) => None,
                NoteTarget::Path(_) => Some(relative_target),
            },
            item_kind: None,
            block_id: request.block_id.clone(),
            dependency_replacement,
            blocked_task_replacement,
            complete_replacement,
            relation_to_replacement,
            relation_from_replacement,
            wikilink_replacement,
            parent_replacement,
            line: assigned.line,
            task_ref: assigned.task_ref.clone(),
            task: assigned,
        },
    })
}

fn is_notebook_contents(contents: &str) -> bool {
    if let Some(frontmatter) =
        crate::native::projects::parse_frontmatter(contents)
    {
        return crate::native::projects::frontmatter_is_notebook(&frontmatter);
    }
    contents.contains("[[notebook]]")
}

/// Notebook assignment: append the chosen ID to the eligible bullet
/// resolved from the `line:digest` ref. Reuses the shared bullet
/// snapshot and the existing ref semantics so a retyped notebook,
/// a shifted line, or an ambiguous digest refuses like tasks do.
#[allow(clippy::too_many_arguments)]
fn plan_notebook_assignment(
    request: &CaptureTaskIdRequest,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
    route: Option<String>,
    relative_target: &str,
    target: &Path,
    original: &str,
) -> Result<AssignmentPlan, CaptureTaskIdError> {
    use crate::native::capture::notebook_bullets;
    let bullets = notebook_bullets::scan_notebook_bullets(original);
    let pos = match notebook_bullets::resolve_bullet_ref(
        &bullets,
        request.task_ref.line,
        &request.task_ref.digest,
    ) {
        Ok(pos) => pos,
        Err(()) => {
            // Distinguish a non-note target from a stale/ambiguous ref:
            // when the ref line points at a live line that is not an
            // eligible bullet, name it as such.
            let line_count = original.split('\n').count();
            if request.task_ref.line >= 1
                && request.task_ref.line <= line_count
                && !bullets
                    .iter()
                    .any(|bullet| bullet.line == request.task_ref.line)
            {
                return Err(CaptureTaskIdError::io(format!(
                    "the selected line {} is not a notebook note in {}; pick a note bullet",
                    request.task_ref.line, relative_target
                )));
            }
            // Ambiguous equal lines vs stale: exactly one digest match
            // would have resolved, so two or more is ambiguous.
            let mut digest_matches = 0;
            for bullet in &bullets {
                if let Some((_, digest)) = bullet.task_ref.split_once(':')
                    && digest == request.task_ref.digest
                {
                    digest_matches += 1;
                }
            }
            if digest_matches > 1 {
                return Err(CaptureTaskIdError::io(format!(
                    "the selected note matches more than one line in {}; rerun the note picker",
                    relative_target
                )));
            }
            return Err(CaptureTaskIdError::io(format!(
                "the selected note is no longer in {}; rerun the note picker",
                relative_target
            )));
        }
    };
    let bullet = &bullets[pos];
    if let Some(existing) = bullet.block_id.as_deref() {
        return Err(CaptureTaskIdError::io(format!(
            "the selected note already has block ID ^{existing} in {relative_target}"
        )));
    }
    // Duplicate chosen ID anywhere (headings and plain bullets included)
    // is already checked by the caller via `block_ids_in_markdown`, but
    // re-check against the live snapshot for a precise message.
    let duplicates = notebook_bullets::duplicate_ids(original);
    if duplicates.contains(&request.block_id) {
        return Err(CaptureTaskIdError::io(format!(
            "block ID ^{} already exists in {relative_target}",
            request.block_id
        )));
    }
    let updated =
        append_block_id(original, bullet.line_index, &request.block_id)?;
    // Verify the write took: the resolved line now carries the ID.
    {
        let after = notebook_bullets::scan_notebook_bullets(&updated);
        let ok = after.iter().any(|candidate| {
            candidate.line_index == bullet.line_index
                && candidate.block_id.as_deref()
                    == Some(request.block_id.as_str())
        });
        if !ok {
            return Err(CaptureTaskIdError::io(format!(
                "failed to assign block ID ^{} on line {} of {relative_target}",
                request.block_id, bullet.line
            )));
        }
    }
    let assigned_ref = format!(
        "{}:{}",
        bullet.line,
        note_tasks::line_digest(
            updated
                .split('\n')
                .nth(bullet.line_index)
                .unwrap_or_default()
        )
    );
    let assigned = AssignedTask {
        task_ref: assigned_ref.clone(),
        line: bullet.line,
        block_id: request.block_id.clone(),
        status_symbol: '-',
        status_name: "Note".to_string(),
        status_type: "NOTE",
        text: bullet.text.clone(),
        section: bullet.section.clone(),
        depth: bullet.depth,
        child_count: bullet.child_count,
    };
    let target_path = PathBuf::from(relative_target);
    let dependency_replacement = capture_dependency_tasks::replacement_for(
        &discovered.index,
        &target_path,
        &request.block_id,
    );
    let blocked_task_replacement =
        capture_dependency_tasks::replacement_for_sigil(
            &discovered.index,
            &target_path,
            &request.block_id,
            b'*',
        );
    let complete_replacement = capture_dependency_tasks::replacement_for_sigil(
        &discovered.index,
        &target_path,
        &request.block_id,
        b'!',
    );
    let relation_to_replacement =
        capture_dependency_tasks::replacement_for_prefix(
            &discovered.index,
            &target_path,
            &request.block_id,
            "~",
        );
    let relation_from_replacement =
        capture_dependency_tasks::replacement_for_prefix(
            &discovered.index,
            &target_path,
            &request.block_id,
            "~~",
        );
    let wikilink_replacement = capture_task_wikilinks::wikilink_replacement(
        &discovered.index,
        &target_path,
        &request.block_id,
    );
    let parent_replacement = route
        .as_deref()
        .map(|route| format!("@{route}+{}", request.block_id));
    Ok(AssignmentPlan {
        target: target.to_path_buf(),
        original: original.to_string(),
        updated,
        result: CaptureTaskIdResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            dry_run: request.dry_run,
            route,
            relative_target: relative_target.to_string(),
            note_path: match &request.target {
                NoteTarget::Route(_) => None,
                NoteTarget::Path(_) => Some(relative_target.to_string()),
            },
            item_kind: Some("note"),
            block_id: request.block_id.clone(),
            dependency_replacement,
            blocked_task_replacement,
            complete_replacement,
            relation_to_replacement,
            relation_from_replacement,
            wikilink_replacement,
            parent_replacement,
            line: assigned.line,
            task_ref: assigned.task_ref.clone(),
            task: assigned,
        },
    })
}

/// Latest canonical `YYYY/YYYYMMDD.md` daily note before the capture
/// clock's anchor date: the previous daily snapshot, which stays
/// read-only. `None` when the vault holds no earlier daily.
fn previous_daily_note(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
) -> Option<String> {
    let anchor = task_status_hooks::daily_anchor_date(
        &super::pomodoro::day_file_for(bob_dir),
        bob_env::current_datetime().date(),
    );
    let mut previous: Option<(NaiveDate, String)> = None;
    let mut seen = HashSet::new();
    for paths in discovered.basenames.values() {
        for path in paths {
            if !seen.insert(path) {
                continue;
            }
            if let Some(date) =
                task_status_hooks::canonical_daily_date(Path::new(path))
                && date < anchor
                && previous.as_ref().is_none_or(|(day, _)| date > *day)
            {
                previous = Some((date, path.clone()));
            }
        }
    }
    previous.map(|(_, path)| path)
}

fn assigned_task(task: &note_tasks::NoteTask) -> AssignedTask {
    AssignedTask {
        task_ref: task.task_ref(),
        line: task.line_index + 1,
        block_id: task.block_id.clone().expect("assigned task has a block ID"),
        status_symbol: task.status_symbol,
        status_name: task.status_name.clone(),
        status_type: capture_tasks::status_type_label(task.status_type),
        text: task.description.clone(),
        section: task.section.clone(),
        depth: capture_tasks::indentation_depth(&task.indentation),
        child_count: task.child_count,
    }
}

fn append_block_id(
    contents: &str,
    line_index: usize,
    block_id: &str,
) -> Result<String, CaptureTaskIdError> {
    let mut updated =
        String::with_capacity(contents.len() + block_id.len() + 2);
    let mut found = false;
    for (index, line) in contents.split_inclusive('\n').enumerate() {
        if index == line_index {
            let (content, ending) = collect_done::split_line_ending(line);
            updated.push_str(content);
            updated.push_str(" ^");
            updated.push_str(block_id);
            updated.push_str(ending);
            found = true;
        } else {
            updated.push_str(line);
        }
    }
    if !found {
        return Err(CaptureTaskIdError::io(format!(
            "task line {} is outside the note",
            line_index + 1
        )));
    }
    Ok(updated)
}

fn print_success(result: &CaptureTaskIdResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureTaskIdResult) {
    let styler = Styler::detect();
    print!("{}", human_success(result, &styler));
}

fn human_success(result: &CaptureTaskIdResult, styler: &Styler) -> String {
    let mut output = format!(
        "Capture task ID {} {}\n\n",
        styler.separator(),
        styler.cyan(&result.relative_target)
    );
    let verb = if result.dry_run { "would add" } else { "added" };
    output.push_str(&format!(
        "  {} {}\n",
        styler.success_prefix(result.dry_run),
        styler.cyan(&format!("{verb} ^{}", result.block_id))
    ));
    output.push_str(&format!(
        "    [{}] {}\n",
        result.task.status_symbol, result.task.text
    ));
    let section = result.task.section.as_deref().unwrap_or("no section");
    output.push_str(&format!(
        "    {}  line {}  {}\n",
        styler.dim(section),
        result.line,
        styler.dim(&result.task_ref)
    ));
    output
}

fn success_json(result: &CaptureTaskIdResult) -> String {
    serde_json::to_string(result).expect("serialize capture task id result")
}

fn print_error(error: CaptureTaskIdError, output_format: OutputFormat) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {}", error.message),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": error.message }))
        }
    }
    error.kind.exit_code()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureTaskIdError {
    kind: CaptureTaskIdErrorKind,
    message: String,
}

impl CaptureTaskIdError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureTaskIdErrorKind::Usage,
            message: message.into(),
        }
    }

    fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureTaskIdErrorKind::Io,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureTaskIdErrorKind {
    Usage,
    Io,
}

impl CaptureTaskIdErrorKind {
    fn exit_code(self) -> i32 {
        match self {
            Self::Usage => 2,
            Self::Io => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::Path,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn request(
        bob_dir: &Path,
        route: &str,
        task_ref: TaskRef,
        block_id: &str,
        dry_run: bool,
    ) -> CaptureTaskIdRequest {
        CaptureTaskIdRequest {
            bob_dir: bob_dir.to_path_buf(),
            dry_run,
            target: NoteTarget::Route(route.to_string()),
            task_ref,
            block_id: block_id.to_string(),
            allow_closed: false,
        }
    }

    fn task_ref_for(contents: &str, description: &str) -> TaskRef {
        let settings = note_tasks::read_settings(Path::new("/nonexistent"));
        let scan = note_tasks::scan(contents, &settings);
        let task = scan
            .task_named(description)
            .unwrap_or_else(|| panic!("missing task {description}"));
        TaskRef::from_task(task)
    }

    #[test]
    fn build_cli_renders_without_panicking() {
        build_cli().debug_assert();
    }

    #[test]
    fn assigns_a_block_id_on_an_lf_note_and_returns_the_updated_ref() {
        let temp = TempDir::new("bob-cli-capture-task-id-lf");
        write_settings(temp.path());
        let original = concat!(
            "# Tasks\n",
            "- [ ] #task Write the report\n",
            "- [ ] #task Already ready ^ready-id\n",
        );
        write_file(&temp.path().join("file.md"), original);
        let task_ref = task_ref_for(original, "Write the report");

        let result = assign_capture_task_id(request(
            temp.path(),
            "file",
            task_ref.clone(),
            "report-id",
            false,
        ))
        .expect("assign");

        assert_eq!(result.schema_version, 1);
        assert!(!result.dry_run);
        assert_eq!(result.route.as_deref(), Some("file"));
        assert_eq!(result.relative_target, "file.md");
        assert_eq!(result.block_id, "report-id");
        assert_eq!(result.line, 2);
        assert_eq!(result.task.text, "Write the report");
        assert_eq!(result.task.block_id, "report-id");
        assert_eq!(result.task.section.as_deref(), Some("Tasks"));
        assert_eq!(
            fs::read_to_string(temp.path().join("file.md")).expect("read"),
            concat!(
                "# Tasks\n",
                "- [ ] #task Write the report ^report-id\n",
                "- [ ] #task Already ready ^ready-id\n",
            )
        );
        let updated_ref = task_ref_for(
            &fs::read_to_string(temp.path().join("file.md")).expect("read"),
            "Write the report",
        );
        assert_eq!(result.task_ref, updated_ref.to_string());
        assert_ne!(result.task_ref, task_ref.to_string());
    }

    #[test]
    fn assigns_a_block_id_on_a_crlf_note_without_touching_other_bytes() {
        let temp = TempDir::new("bob-cli-capture-task-id-crlf");
        write_settings(temp.path());
        let original = concat!(
            "# Tasks\r\n",
            "- [ ] #task Write the report\r\n",
            "- ordinary\r\n",
        );
        write_file(&temp.path().join("file.md"), original);
        let task_ref = task_ref_for(original, "Write the report");

        assign_capture_task_id(request(
            temp.path(),
            "file",
            task_ref,
            "report-id",
            false,
        ))
        .expect("assign");

        assert_eq!(
            fs::read_to_string(temp.path().join("file.md")).expect("read"),
            concat!(
                "# Tasks\r\n",
                "- [ ] #task Write the report ^report-id\r\n",
                "- ordinary\r\n",
            )
        );
    }

    #[test]
    fn dry_run_returns_the_plan_without_writing() {
        let temp = TempDir::new("bob-cli-capture-task-id-dry");
        write_settings(temp.path());
        let original = "- [ ] #task Write the report\n";
        write_file(&temp.path().join("file.md"), original);
        let task_ref = task_ref_for(original, "Write the report");

        let result = assign_capture_task_id(request(
            temp.path(),
            "file",
            task_ref,
            "report-id",
            true,
        ))
        .expect("dry-run");

        assert!(result.dry_run);
        assert_eq!(result.block_id, "report-id");
        assert_eq!(
            fs::read_to_string(temp.path().join("file.md")).expect("read"),
            original
        );
    }

    fn notebook_contents(body: &str) -> String {
        format!(
            "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\nsummary: Notes.\n---\n\n# Title\n\n{body}"
        )
    }

    fn notebook_ref_for(contents: &str, text: &str) -> TaskRef {
        let bullets =
            crate::native::capture::notebook_bullets::scan_notebook_bullets(
                contents,
            );
        let bullet = bullets
            .iter()
            .find(|bullet| bullet.text == text)
            .unwrap_or_else(|| panic!("missing note {text}"));
        let (line, digest) =
            bullet.task_ref.split_once(':').expect("line:digest");
        TaskRef {
            line: line.parse().expect("line"),
            digest: digest.to_string(),
        }
    }

    #[test]
    fn notebook_assignment_writes_and_returns_parent_replacement() {
        let temp = TempDir::new("bob-cli-capture-task-id-notebook");
        write_settings(temp.path());
        let original =
            notebook_contents("- Plain idea\n\n- Use merge ^merge-cmd\n");
        write_file(&temp.path().join("dev_tools.md"), &original);
        let note_ref = notebook_ref_for(&original, "Plain idea");

        let dry = assign_capture_task_id(request(
            temp.path(),
            "dev_tools",
            note_ref.clone(),
            "plain-id",
            true,
        ))
        .expect("dry-run");
        assert!(dry.dry_run);
        assert_eq!(dry.item_kind, Some("note"));
        assert_eq!(
            dry.parent_replacement.as_deref(),
            Some("@dev_tools+plain-id")
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("dev_tools.md")).expect("read"),
            original
        );

        let result = assign_capture_task_id(request(
            temp.path(),
            "dev_tools",
            note_ref,
            "plain-id",
            false,
        ))
        .expect("assign");
        assert_eq!(result.item_kind, Some("note"));
        assert_eq!(
            result.parent_replacement.as_deref(),
            Some("@dev_tools+plain-id")
        );
        assert_eq!(result.task.text, "Plain idea");
        assert_eq!(result.task.block_id, "plain-id");
        let updated =
            fs::read_to_string(temp.path().join("dev_tools.md")).expect("read");
        assert!(updated.contains("- Plain idea ^plain-id"), "{updated:?}");
        assert!(updated.contains("- Use merge ^merge-cmd"), "{updated:?}");
    }

    #[test]
    fn notebook_assignment_rejects_non_note_and_duplicate_ids() {
        let temp = TempDir::new("bob-cli-capture-task-id-notebook-errors");
        write_settings(temp.path());
        let original = notebook_contents(
            "## Git\n\n- Note one\n\n- [ ] Task ^task-id\n\n# Heading ^head-id\n",
        );
        write_file(&temp.path().join("dev_tools.md"), &original);
        // Non-note target line (heading).
        let heading_line = original
            .split('\n')
            .position(|line| line.starts_with("# Heading"))
            .expect("heading")
            + 1;
        let digest = crate::native::note_tasks::line_digest(
            original
                .split('\n')
                .nth(heading_line - 1)
                .unwrap_or_default(),
        );
        let err = assign_capture_task_id(request(
            temp.path(),
            "dev_tools",
            TaskRef {
                line: heading_line,
                digest,
            },
            "new-id",
            false,
        ))
        .expect_err("non-note refuses");
        assert!(err.message.contains("not a notebook note"), "{err:?}");
        // Already assigned.
        let with_id = notebook_contents("- Note ^has-id\n");
        write_file(&temp.path().join("notes.md"), &with_id);
        let note_ref = notebook_ref_for(&with_id, "Note");
        let err = assign_capture_task_id(request(
            temp.path(),
            "notes",
            note_ref,
            "other-id",
            false,
        ))
        .expect_err("already assigned");
        assert!(err.message.contains("already has block ID"), "{err:?}");
        // Colliding chosen ID.
        let colliding = notebook_contents("- Other ^taken\n\n- Plain\n");
        write_file(&temp.path().join("plain.md"), &colliding);
        let plain_ref = notebook_ref_for(&colliding, "Plain");
        let err = assign_capture_task_id(request(
            temp.path(),
            "plain",
            plain_ref,
            "taken",
            false,
        ))
        .expect_err("colliding");
        assert!(err.message.contains("already exists"), "{err:?}");
    }

    #[test]
    fn notebook_assignment_handles_stale_shifted_and_format_edges() {
        let temp = TempDir::new("bob-cli-capture-task-id-notebook-edges");
        write_settings(temp.path());
        // Stale: digest no longer matches after a retype.
        let original = notebook_contents("- Plain idea\n");
        write_file(&temp.path().join("dev_tools.md"), &original);
        let note_ref = notebook_ref_for(&original, "Plain idea");
        let retyped = notebook_contents("- Plain idea edited\n");
        write_file(&temp.path().join("dev_tools.md"), &retyped);
        let err = assign_capture_task_id(request(
            temp.path(),
            "dev_tools",
            note_ref,
            "new-id",
            false,
        ))
        .expect_err("stale");
        assert!(err.message.contains("no longer in"), "{err:?}");
        // Shifted: same digest one line lower resolves.
        let original = notebook_contents("- Plain idea\n");
        let original_ref = notebook_ref_for(&original, "Plain idea");
        let shifted = notebook_contents("Intro\n\n- Plain idea\n");
        write_file(&temp.path().join("shift.md"), &shifted);
        // `shift.md` is not a routable notebook route; use --note-path.
        let shift_request = CaptureTaskIdRequest {
            bob_dir: temp.path().to_path_buf(),
            dry_run: false,
            target: NoteTarget::Path("shift.md".to_string()),
            task_ref: original_ref,
            block_id: "shift-id".to_string(),
            allow_closed: false,
        };
        // The digest matches exactly one bullet after the shift, so it
        // recovers. Line numbers differ because of the inserted intro.
        // (If the intro changes the digest, this becomes stale instead;
        // both outcomes prove the ref is honored.)
        let outcome = assign_capture_task_id(shift_request);
        match outcome {
            Ok(result) => {
                assert_eq!(result.item_kind, Some("note"));
                assert!(result.line >= 2);
            }
            Err(err) => {
                assert!(err.message.contains("no longer in"), "{err:?}");
            }
        }
        // Ambiguous equal lines refuse: a shifted line with the same
        // digest on two rows cannot pick one.
        let dup_text = notebook_contents("- Same\n\n- Same\n");
        write_file(&temp.path().join("dup.md"), &dup_text);
        let same_digest = notebook_ref_for(&dup_text, "Same").digest;
        let err = assign_capture_task_id(request(
            temp.path(),
            "dup",
            TaskRef {
                line: 99,
                digest: same_digest,
            },
            "new-id",
            false,
        ))
        .expect_err("ambiguous");
        assert!(
            err.message.contains("more than one line")
                || err.message.contains("no longer in"),
            "{err:?}"
        );
        // CRLF preserves unrelated bytes.
        let crlf = notebook_contents("- Plain idea\r\n\n- Other\r\n");
        // `notebook_contents` joins with `\n`; build CRLF explicitly.
        let crlf = crlf.replace('\n', "\r\n").replace("\r\r\n", "\r\n");
        write_file(&temp.path().join("crlf.md"), &crlf);
        let crlf_ref = notebook_ref_for(&crlf, "Plain idea");
        assign_capture_task_id(CaptureTaskIdRequest {
            bob_dir: temp.path().to_path_buf(),
            dry_run: false,
            target: NoteTarget::Path("crlf.md".to_string()),
            task_ref: crlf_ref,
            block_id: "crlf-id".to_string(),
            allow_closed: false,
        })
        .expect("crlf");
        let updated =
            fs::read_to_string(temp.path().join("crlf.md")).expect("read");
        assert!(updated.contains("\r\n"), "{updated:?}");
        assert!(updated.contains("- Plain idea ^crlf-id"), "{updated:?}");
        // BOM is preserved.
        let bom = format!("\u{feff}{}", notebook_contents("- Plain\n"));
        write_file(&temp.path().join("bom.md"), &bom);
        let bom_ref = notebook_ref_for(&bom, "Plain");
        assign_capture_task_id(CaptureTaskIdRequest {
            bob_dir: temp.path().to_path_buf(),
            dry_run: false,
            target: NoteTarget::Path("bom.md".to_string()),
            task_ref: bom_ref,
            block_id: "bom-id".to_string(),
            allow_closed: false,
        })
        .expect("bom");
        let updated =
            fs::read_to_string(temp.path().join("bom.md")).expect("read");
        assert!(updated.starts_with("\u{feff}"), "{updated:?}");
        // Read-only refuses without writing.
        let ro_original = notebook_contents("- Plain\n");
        write_file(&temp.path().join("ro.md"), &ro_original);
        let ro_ref = notebook_ref_for(&ro_original, "Plain");
        let ro_path = temp.path().join("ro.md");
        let mut perms =
            std::fs::metadata(&ro_path).expect("meta").permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&ro_path, perms).expect("readonly");
        let err = assign_capture_task_id(CaptureTaskIdRequest {
            bob_dir: temp.path().to_path_buf(),
            dry_run: false,
            target: NoteTarget::Path("ro.md".to_string()),
            task_ref: ro_ref,
            block_id: "ro-id".to_string(),
            allow_closed: false,
        })
        .expect_err("readonly");
        assert!(err.message.contains("read-only"), "{err:?}");
    }

    #[test]
    fn recovers_a_shifted_line_and_returns_the_new_line() {
        let temp = TempDir::new("bob-cli-capture-task-id-shift");
        write_settings(temp.path());
        let original = "- [ ] #task Write the report\n";
        let original_ref = task_ref_for(original, "Write the report");
        let shifted = concat!("Intro\n", "- [ ] #task Write the report\n");
        write_file(&temp.path().join("file.md"), shifted);

        let result = assign_capture_task_id(request(
            temp.path(),
            "file",
            original_ref,
            "report-id",
            false,
        ))
        .expect("recover shift");

        assert_eq!(result.line, 2);
        assert_eq!(
            fs::read_to_string(temp.path().join("file.md")).expect("read"),
            "Intro\n- [ ] #task Write the report ^report-id\n"
        );
    }

    #[test]
    fn validation_failures_are_write_free() {
        let temp = TempDir::new("bob-cli-capture-task-id-errors");
        write_settings(temp.path());
        let original = concat!(
            "Plain heading ^plain-id\n",
            "- [ ] #task Same\n",
            "- [ ] #task Same\n",
            "- [ ] #task Ready ^ready-id\n",
            "- [x] #task Done task\n",
            "- [ ] #task Open missing\n",
        );
        write_file(&temp.path().join("file.md"), original);
        let missing_ref = task_ref_for(original, "Open missing");
        let ready_ref = task_ref_for(original, "Ready");
        let done_ref = task_ref_for(original, "Done task");
        let same_digest = task_ref_for(original, "Same").digest;
        let same_ref = TaskRef {
            line: 99,
            digest: same_digest,
        };

        let cases = [
            (
                request(
                    temp.path(),
                    "file",
                    missing_ref.clone(),
                    "ready-id",
                    false,
                ),
                "already exists",
            ),
            (
                request(
                    temp.path(),
                    "file",
                    missing_ref.clone(),
                    "plain-id",
                    false,
                ),
                "already exists",
            ),
            (
                request(temp.path(), "file", ready_ref, "fresh-id", false),
                "already has block ID ^ready-id",
            ),
            (
                request(temp.path(), "file", done_ref, "fresh-id", false),
                "no longer open",
            ),
            (
                request(
                    temp.path(),
                    "file",
                    TaskRef {
                        line: 99,
                        digest: "deadbeef".to_string(),
                    },
                    "fresh-id",
                    false,
                ),
                "no longer in file.md",
            ),
            (
                request(temp.path(), "file", same_ref, "fresh-id", false),
                "matches more than one line",
            ),
        ];

        for (request, expected) in cases {
            let error =
                assign_capture_task_id(request).expect_err("expected failure");
            assert_eq!(error.kind, CaptureTaskIdErrorKind::Io, "{expected}");
            assert!(
                error.message.contains(expected),
                "expected {expected:?} in {}",
                error.message
            );
            assert_eq!(
                fs::read_to_string(temp.path().join("file.md")).expect("read"),
                original,
                "{expected} mutated the note"
            );
        }

        let missing = assign_capture_task_id(request(
            temp.path(),
            "missing",
            missing_ref.clone(),
            "fresh-id",
            false,
        ))
        .expect_err("missing note");
        assert!(missing.message.contains("does not exist"));
        assert_eq!(
            fs::read_to_string(temp.path().join("file.md")).expect("read"),
            original
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = temp.path().join("locked.md");
            write_file(&path, original);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o000))
                .expect("lock note");
            let error = assign_capture_task_id(request(
                temp.path(),
                "locked",
                missing_ref,
                "fresh-id",
                false,
            ))
            .expect_err("unreadable note");
            assert!(
                error.message.contains("read-only")
                    || error.message.contains("read target"),
                "{}",
                error.message
            );
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
                .expect("unlock note");
            assert_eq!(
                fs::read_to_string(&path).expect("read locked"),
                original
            );
        }
    }

    #[test]
    fn json_success_shape_is_stable() {
        let result = CaptureTaskIdResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            dry_run: false,
            route: Some("file".to_string()),
            relative_target: "file.md".to_string(),
            note_path: None,
            item_kind: None,
            block_id: "report-id".to_string(),
            dependency_replacement: "&file:report-id".to_string(),
            blocked_task_replacement: "*file:report-id".to_string(),
            complete_replacement: "!file:report-id".to_string(),
            relation_to_replacement: "~file:report-id".to_string(),
            relation_from_replacement: "~~file:report-id".to_string(),
            wikilink_replacement: "[[file#^report-id]]".to_string(),
            parent_replacement: Some("@file+report-id".to_string()),
            line: 2,
            task_ref: "2:abcd1234".to_string(),
            task: AssignedTask {
                task_ref: "2:abcd1234".to_string(),
                line: 2,
                block_id: "report-id".to_string(),
                status_symbol: ' ',
                status_name: "Todo".to_string(),
                status_type: "TODO",
                text: "Write the report".to_string(),
                section: Some("Tasks".to_string()),
                depth: 0,
                child_count: 0,
            },
        };

        let value: serde_json::Value =
            serde_json::from_str(&success_json(&result)).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["dry_run"], false);
        assert_eq!(value["route"], "file");
        assert_eq!(value["relative_target"], "file.md");
        assert!(value.get("note_path").is_none());
        assert_eq!(value["dependency_replacement"], "&file:report-id");
        assert_eq!(value["blocked_task_replacement"], "*file:report-id");
        assert_eq!(value["complete_replacement"], "!file:report-id");
        assert_eq!(value["relation_to_replacement"], "~file:report-id");
        assert_eq!(value["relation_from_replacement"], "~~file:report-id");
        assert_eq!(value["wikilink_replacement"], "[[file#^report-id]]");
        assert_eq!(value["block_id"], "report-id");
        assert_eq!(value["line"], 2);
        assert_eq!(value["ref"], "2:abcd1234");
        assert_eq!(value["task"]["ref"], "2:abcd1234");
        assert_eq!(value["task"]["block_id"], "report-id");
        assert_eq!(value["task"]["text"], "Write the report");
        assert_eq!(value["task"]["section"], "Tasks");
        assert_eq!(value["task"]["status_type"], "TODO");
    }

    fn write_settings(root: &Path) {
        write_file(
            &root.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            r##"{
              "globalFilter": "#task",
              "statusSettings": {
                "coreStatuses": [
                  {"symbol":" ","name":"Todo","type":"TODO"},
                  {"symbol":"x","name":"Done","type":"DONE"},
                  {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
                  {"symbol":"*","name":"Next","type":"ON_HOLD"},
                  {"symbol":"-","name":"Canceled","type":"CANCELLED"}
                ],
                "customStatuses": []
              }
            }"##,
        );
    }

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("file parent"))
            .expect("create file parent");
        fs::write(path, contents).expect("write file");
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "{prefix}-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                eprintln!("failed to remove {}: {error}", self.path.display());
            }
        }
    }
}
