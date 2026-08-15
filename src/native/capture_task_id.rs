use std::{ffi::OsString, fs, io, iter, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{
    capture, capture_language, capture_tasks, collect_done, env as bob_env,
    note_tasks::{self, RefLookup, TaskRef},
    style::Styler,
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
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Assign a block ID to an open capture task")
        .long_about(
            "Assign a user-authored Obsidian block ID to one open task in a \
routed Bob note.\n\n\
The command is the write half of '@route+' completion: callers first discover \
a missing-ID task with `bob capture-complete --all-tasks`, then pass that \
candidate's stale-safe --task-ref together with the authored --block-id. \
Validation uses the same route grammar, block-ID grammar, task scan, and \
duplicate-ID rules as `bob capture`. The selected task must still be open and \
must not already have an ID. An ID already used anywhere in the routed note \
is rejected, including non-task anchors.\n\n\
On success the command appends ' ^<id>' to the resolved physical task line, \
preserves that line's ending and every unrelated byte, and replaces the note \
with one same-directory temporary file rename. Dry-run plans the same \
mutation and returns the same success shape without writing. Stale, \
ambiguous, terminal, already-identified, missing, or unreadable tasks fail \
without touching the note.",
        )
        .after_help(
            "Examples:\n  bob capture-task-id -r file -t 3:1f3a9c2b -i report-id\n  bob capture-task-id --route cash --task-ref 12:abcd1234 --block-id goog-exit -f json\n  bob capture-task-id -r file -t 3:1f3a9c2b -i report-id --dry-run\n  bob capture-task-id -b ~/bob -r notes -t 8:deadbeef -i inbox-item\n\nEnvironment:\n  BOB_DIR                    Bob vault root when --bob-dir is omitted",
        )
        .disable_help_flag(true)
        .arg(block_id_arg())
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(route_arg())
        .arg(task_ref_arg())
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
        .required(true)
        .help("Route/name of the note that contains the task")
}

fn task_ref_arg() -> Arg {
    Arg::new("task-ref")
        .long("task-ref")
        .short('t')
        .value_name("REF")
        .required(true)
        .help("Stale-safe task ref from capture-complete or capture-tasks")
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureTaskIdRequest {
    bob_dir: PathBuf,
    dry_run: bool,
    route: String,
    task_ref: TaskRef,
    block_id: String,
}

impl CaptureTaskIdRequest {
    fn from_matches(matches: &ArgMatches) -> Result<Self, CaptureTaskIdError> {
        Ok(Self {
            bob_dir: bob_dir_from_matches(matches),
            dry_run: matches.get_flag("dry-run"),
            route: route_from_matches(matches)?,
            task_ref: task_ref_from_matches(matches)?,
            block_id: block_id_from_matches(matches)?,
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

fn route_from_matches(
    matches: &ArgMatches,
) -> Result<String, CaptureTaskIdError> {
    let route = matches.get_one::<String>("route").expect("required");
    if capture::is_route_token(route) {
        return Ok(route.to_ascii_lowercase());
    }
    Err(CaptureTaskIdError::usage(
        "--route must contain only A-Z, a-z, 0-9, '_' or '-'",
    ))
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
    route: String,
    relative_target: String,
    block_id: String,
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
    let relative_target = capture::route_label(&request.route);
    let target = request.bob_dir.join(&relative_target);
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
    let scan = note_tasks::scan(&original, &settings);
    if collect_done::block_ids_in_markdown(&original)
        .iter()
        .any(|existing| existing == &request.block_id)
    {
        return Err(CaptureTaskIdError::io(format!(
            "block ID ^{} already exists in {relative_target}",
            request.block_id
        )));
    }

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
        return Err(CaptureTaskIdError::io(format!(
            "the selected task is no longer open in {relative_target} (status: {})",
            task.status_name
        )));
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
    Ok(AssignmentPlan {
        target,
        original,
        updated,
        result: CaptureTaskIdResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            dry_run: request.dry_run,
            route: request.route.clone(),
            relative_target,
            block_id: request.block_id.clone(),
            line: assigned.line,
            task_ref: assigned.task_ref.clone(),
            task: assigned,
        },
    })
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
            route: route.to_string(),
            task_ref,
            block_id: block_id.to_string(),
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
        assert_eq!(result.route, "file");
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
            assert!(error.message.contains("read target"), "{}", error.message);
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
            route: "file".to_string(),
            relative_target: "file.md".to_string(),
            block_id: "report-id".to_string(),
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
