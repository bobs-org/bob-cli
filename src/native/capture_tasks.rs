use std::{
    ffi::OsString,
    fs, io, iter,
    path::{Path, PathBuf},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{
    capture, env as bob_env,
    note_tasks::{self, TaskStatusType},
    style::{display_width, pad_right, Styler},
};

const COMMAND_NAME: &str = "bob capture-tasks";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let request = match CaptureTasksRequest::from_matches(&matches) {
        Ok(request) => request,
        Err(error) => return print_tasks_error(error, output_format),
    };

    match list_capture_tasks(&request.bob_dir, &request.route) {
        Ok(result) => {
            print_success(&result, output_format);
            0
        }
        Err(error) => print_tasks_error(error, output_format),
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
        .about("List the open tasks of a capture note")
        .long_about(
            "List the open Obsidian Tasks entries in one routable Bob note.\n\n\
The command is read-only and reports tasks in document order with stable refs, \
status details, sections, nesting depth, and child counts for picker callers. \
Done and canceled tasks are omitted. Missing notes are not errors; they return \
an empty list.",
        )
        .after_help(
            "Examples:\n  bob capture-tasks --route cash\n  bob capture-tasks -r cash -f json\n  bob capture-tasks -b ~/bob -r project-alpha",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(route_arg())
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
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
        .help("Route/name of the capture note whose open tasks to list")
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
struct CaptureTasksRequest {
    bob_dir: PathBuf,
    route: String,
}

impl CaptureTasksRequest {
    fn from_matches(matches: &ArgMatches) -> Result<Self, CaptureTasksError> {
        Ok(Self {
            bob_dir: bob_dir_from_matches(matches),
            route: route_from_matches(matches)?,
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
) -> Result<String, CaptureTasksError> {
    let Some(route) = matches.get_one::<String>("route") else {
        return Err(CaptureTasksError::usage("--route is required"));
    };
    normalize_route(route)
}

fn normalize_route(route: &str) -> Result<String, CaptureTasksError> {
    if capture::is_route_token(route) {
        return Ok(route.to_ascii_lowercase());
    }

    Err(CaptureTasksError::usage(
        "--route must contain only A-Z, a-z, 0-9, '_' or '-'",
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureTasksResult {
    ok: bool,
    route: String,
    relative_target: String,
    count: usize,
    tasks: Vec<CaptureTask>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureTask {
    #[serde(rename = "ref")]
    task_ref: String,
    line: usize,
    block_id: Option<String>,
    status_symbol: char,
    status_name: String,
    status_type: &'static str,
    text: String,
    section: Option<String>,
    depth: usize,
    child_count: usize,
}

fn list_capture_tasks(
    bob_dir: &Path,
    route: &str,
) -> Result<CaptureTasksResult, CaptureTasksError> {
    let relative_target = capture::route_label(route);
    let target = bob_dir.join(&relative_target);
    let contents = match fs::read_to_string(&target) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(fs_error("read target", &target, error)),
    };
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let tasks = scan
        .open_tasks()
        .map(|task| CaptureTask {
            task_ref: format!("{}:{}", task.line_index + 1, task.digest),
            line: task.line_index + 1,
            block_id: task.block_id.clone(),
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            status_type: status_type_label(task.status_type),
            text: task.description.clone(),
            section: task.section.clone(),
            depth: indentation_depth(&task.indentation),
            child_count: task.child_count,
        })
        .collect::<Vec<_>>();

    Ok(CaptureTasksResult {
        ok: true,
        route: route.to_string(),
        relative_target,
        count: tasks.len(),
        tasks,
    })
}

pub(crate) fn status_type_label(status_type: TaskStatusType) -> &'static str {
    match status_type {
        TaskStatusType::Todo => "TODO",
        TaskStatusType::Done => "DONE",
        TaskStatusType::InProgress => "IN_PROGRESS",
        TaskStatusType::OnHold => "ON_HOLD",
        TaskStatusType::Cancelled => "CANCELLED",
        TaskStatusType::NonTask => "NON_TASK",
        TaskStatusType::Empty => "EMPTY",
    }
}

pub(crate) fn indentation_depth(indentation: &str) -> usize {
    let mut depth = 0usize;
    let mut spaces = 0usize;
    for character in indentation.chars() {
        match character {
            ' ' => spaces += 1,
            '\t' => {
                depth += spaces.div_ceil(4) + 1;
                spaces = 0;
            }
            _ => {
                depth += spaces.div_ceil(4) + 1;
                spaces = 0;
            }
        }
    }
    depth + spaces.div_ceil(4)
}

fn print_success(result: &CaptureTasksResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureTasksResult) {
    let styler = Styler::detect();
    print!("{}", human_success(result, &styler));
}

fn human_success(result: &CaptureTasksResult, styler: &Styler) -> String {
    let mut output = format!(
        "Capture tasks {} {}\n\n",
        styler.separator(),
        styler.cyan(&result.relative_target)
    );

    if result.tasks.is_empty() {
        output.push_str("  No open tasks found.\n");
    } else {
        let text_width = result
            .tasks
            .iter()
            .map(|task| display_width(&task.text))
            .max()
            .unwrap_or(0);
        let block_width = result
            .tasks
            .iter()
            .map(|task| task.block_id.as_ref().map_or(0, |id| id.len() + 1))
            .max()
            .unwrap_or(0);
        let mut previous_section: Option<&Option<String>> = None;

        for task in &result.tasks {
            if previous_section != Some(&task.section) {
                if previous_section.is_some() {
                    output.push('\n');
                }
                if let Some(section) = &task.section {
                    output.push_str(&format!("  {}\n", styler.dim(section)));
                }
                previous_section = Some(&task.section);
            }

            let marker = style_status_marker(task.status_symbol, styler);
            let text = pad_right(&task.text, text_width);
            let block_id = task
                .block_id
                .as_ref()
                .map(|id| format!("^{id}"))
                .unwrap_or_default();
            let block_id = styler.cyan(&pad_right(&block_id, block_width));
            let status_name = styler.dim(&task.status_name);
            output.push_str(&format!(
                "    {}{} {}  {}  {}\n",
                "  ".repeat(task.depth),
                marker,
                text,
                block_id,
                status_name
            ));
        }
    }

    output.push('\n');
    output.push_str(&status_summary(result, styler));
    output.push('\n');
    output
}

fn style_status_marker(symbol: char, styler: &Styler) -> String {
    let marker = format!("[{symbol}]");
    match symbol {
        '/' => styler.blue(&marker),
        '*' => styler.yellow(&marker),
        '?' => styler.red(&marker),
        _ => styler.dim(&marker),
    }
}

fn status_summary(result: &CaptureTasksResult, styler: &Styler) -> String {
    let mut counts = Vec::<(&str, usize)>::new();
    for task in &result.tasks {
        if let Some((_, count)) = counts
            .iter_mut()
            .find(|(name, _)| *name == task.status_name)
        {
            *count += 1;
        } else {
            counts.push((&task.status_name, 1));
        }
    }

    let mut parts = vec![format!(
        "{} {}",
        result.count,
        plural(result.count, "task", "tasks")
    )];
    parts.extend(
        counts
            .into_iter()
            .map(|(name, count)| format!("{count} {}", name.to_lowercase())),
    );
    parts.join(&format!(" {} ", styler.separator()))
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

fn success_json(result: &CaptureTasksResult) -> String {
    serde_json::to_string(result).expect("serialize capture tasks result")
}

fn print_tasks_error(
    error: CaptureTasksError,
    output_format: OutputFormat,
) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {}", error.message),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": error.message }))
        }
    }
    error.kind.exit_code()
}

fn fs_error(action: &str, path: &Path, error: io::Error) -> CaptureTasksError {
    CaptureTasksError::io(format!("{action} {}: {error}", path.display()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureTasksError {
    kind: CaptureTasksErrorKind,
    message: String,
}

impl CaptureTasksError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureTasksErrorKind::Usage,
            message: message.into(),
        }
    }

    fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureTasksErrorKind::Io,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureTasksErrorKind {
    Usage,
    Io,
}

impl CaptureTasksErrorKind {
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
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn route_validation_lowercases_valid_route() {
        assert_eq!(normalize_route("Cash-Flow").as_deref(), Ok("cash-flow"));

        let error = normalize_route("../bad").expect_err("invalid route");
        assert_eq!(error.kind, CaptureTasksErrorKind::Usage);
    }

    #[test]
    fn missing_file_returns_empty_tasks() {
        let temp = TempDir::new("bob-cli-capture-tasks-missing");
        let result = list_capture_tasks(temp.path(), "cash")
            .expect("missing note is not an error");

        assert_eq!(result.route, "cash");
        assert_eq!(result.relative_target, "cash.md");
        assert_eq!(result.count, 0);
        assert!(result.tasks.is_empty());
    }

    #[test]
    fn lists_only_open_tasks_in_document_order_with_sections_and_depth() {
        let temp = TempDir::new("bob-cli-capture-tasks-open");
        write_settings(temp.path());
        write_file(
            &temp.path().join("cash.md"),
            concat!(
                "- [ ] #task Above ^todo\n",
                "# Tasks\n",
                "- [x] #task Done\n",
                "\t- [/] #task Active [created:: 2026-07-31]\n",
                "- [*] #task Next ^next\n",
                "- [?] #task Blocked\n",
                "- [-] #task Canceled\n",
            ),
        );

        let result = list_capture_tasks(temp.path(), "cash").expect("tasks");
        assert_eq!(result.count, 4);
        assert_eq!(
            result
                .tasks
                .iter()
                .map(|task| task.text.as_str())
                .collect::<Vec<_>>(),
            ["Above", "Active", "Next", "Blocked"]
        );
        assert_eq!(result.tasks[0].section, None);
        assert_eq!(result.tasks[1].section.as_deref(), Some("Tasks"));
        assert_eq!(result.tasks[1].depth, 1);
        assert_eq!(result.tasks[1].status_type, "IN_PROGRESS");

        let human = human_success(&result, &Styler::plain());
        assert_text_order(
            &human,
            &["Above", "Tasks", "Active", "Next", "Blocked"],
        );
        assert!(human
            .contains("4 tasks - 1 todo - 1 in progress - 1 next - 1 blocked"));
    }

    #[test]
    fn json_success_shape_is_stable() {
        let result = CaptureTasksResult {
            ok: true,
            route: "cash".to_string(),
            relative_target: "cash.md".to_string(),
            count: 1,
            tasks: vec![CaptureTask {
                task_ref: "24:1f3a9c2b".to_string(),
                line: 24,
                block_id: Some("goog-exit".to_string()),
                status_symbol: '*',
                status_name: "Next".to_string(),
                status_type: "ON_HOLD",
                text: "Finish Google Exit Packet!".to_string(),
                section: Some("Tasks".to_string()),
                depth: 0,
                child_count: 1,
            }],
        };

        let value: serde_json::Value =
            serde_json::from_str(&success_json(&result)).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["route"], "cash");
        assert_eq!(value["relative_target"], "cash.md");
        assert_eq!(value["count"], 1);
        assert_eq!(value["tasks"][0]["ref"], "24:1f3a9c2b");
        assert_eq!(value["tasks"][0]["line"], 24);
        assert_eq!(value["tasks"][0]["block_id"], "goog-exit");
        assert_eq!(value["tasks"][0]["status_symbol"], "*");
        assert_eq!(value["tasks"][0]["status_name"], "Next");
        assert_eq!(value["tasks"][0]["status_type"], "ON_HOLD");
        assert_eq!(value["tasks"][0]["text"], "Finish Google Exit Packet!");
        assert_eq!(value["tasks"][0]["section"], "Tasks");
        assert_eq!(value["tasks"][0]["depth"], 0);
        assert_eq!(value["tasks"][0]["child_count"], 1);
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
                "customStatuses": [
                  {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
                ]
              }
            }"##,
        );
    }

    fn assert_text_order(text: &str, needles: &[&str]) {
        let mut last = 0;
        for needle in needles {
            let position = text[last..]
                .find(needle)
                .unwrap_or_else(|| panic!("missing {needle:?} in {text:?}"))
                + last;
            assert!(position >= last);
            last = position;
        }
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
