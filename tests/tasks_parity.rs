use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};

const BOB_BIN: &str = env!("CARGO_BIN_EXE_bob");
const LIVE_PARITY_ENV: &str = "BOB_TASKS_PARITY_LIVE";
const LIVE_PARITY_VAULT_ENV: &str = "BOB_TASKS_PARITY_VAULT";

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX_EPOCH")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "{prefix}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("create temp dir: {error}"));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn tasks_parity_fixture_vault_covers_phase1_contract() {
    let vault = fixture_vault_path();
    for relative in [
        ".obsidian/plugins/obsidian-tasks-plugin/data.json",
        ".trash/Hidden.md",
        "Daily/2026-07-10.md",
        "Tasks/Dependencies.md",
        "Tasks/MetadataDataview.md",
        "Tasks/MetadataEmoji.md",
        "Tasks/MissingGlobalFilter.md",
        "Tasks/Nested.md",
        "Tasks/Statuses.md",
        "_generated/Generated.md",
        "_templates/Task.md",
        "dash.md",
    ] {
        assert!(
            vault.join(relative).is_file(),
            "missing Tasks parity fixture file: {relative}"
        );
    }

    let settings = read_json(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
    );
    assert_eq!(settings["globalFilter"], "#task");
    assert_eq!(settings["globalQuery"], "");
    assert_eq!(settings["taskFormat"], "dataview");
    assert_eq!(
        settings["statusSettings"]["customStatuses"],
        json!([
            {
                "symbol": "/",
                "name": "In Progress",
                "nextStatusSymbol": "x",
                "availableAsCommand": true,
                "type": "IN_PROGRESS"
            },
            {
                "symbol": "*",
                "name": "Next",
                "nextStatusSymbol": "x",
                "availableAsCommand": true,
                "type": "ON_HOLD"
            },
            {
                "symbol": "-",
                "name": "Canceled",
                "nextStatusSymbol": " ",
                "availableAsCommand": true,
                "type": "CANCELLED"
            }
        ])
    );

    let statuses = fs::read_to_string(vault.join("Tasks/Statuses.md"))
        .expect("read status fixtures");
    for marker in ["- [ ]", "* [/]", "+ [*]", "1. [x]", "2. [-]"] {
        assert!(
            statuses.contains(marker),
            "missing list/status marker {marker}"
        );
    }

    let dataview = fs::read_to_string(vault.join("Tasks/MetadataDataview.md"))
        .expect("read dataview metadata fixtures");
    for marker in [
        "[due::",
        "[scheduled::",
        "[start::",
        "[created::",
        "[completion::",
        "[cancelled::",
        "[priority::",
        "[repeat::",
        "[id::",
        "[dependsOn::",
        "[onCompletion::",
        "#hide",
        "^dataview-metadata",
        "not-a-date",
    ] {
        assert!(
            dataview.contains(marker),
            "missing dataview marker {marker}"
        );
    }

    let emoji = fs::read_to_string(vault.join("Tasks/MetadataEmoji.md"))
        .expect("read emoji metadata fixtures");
    for marker in [
        "📅", "⏳", "🛫", "➕", "✅", "❌", "🔺", "🔽", "🔁", "🆔", "⛔", "🏁",
    ] {
        assert!(emoji.contains(marker), "missing emoji marker {marker}");
    }

    let dependencies = fs::read_to_string(vault.join("Tasks/Dependencies.md"))
        .expect("read dependency fixtures");
    for marker in [
        "Missing dependency is ignored",
        "Self dependency",
        "Duplicate id done instance",
        "Duplicate id open instance",
        "Canceled dependency",
    ] {
        assert!(
            dependencies.contains(marker),
            "missing dependency edge case {marker}"
        );
    }

    let dash = fs::read_to_string(vault.join("dash.md"))
        .expect("read dashboard fixture");
    assert!(dash.contains("TQ_extra_instructions:"), "{dash}");
    assert_eq!(dash.matches("```tasks").count(), 3, "{dash}");
    for query in [
        "status.type is IN_PROGRESS",
        "status.name includes Next",
        "status.type is TODO",
    ] {
        assert!(dash.contains(query), "missing dashboard query {query}");
    }
}

#[test]
fn tasks_native_filterless_paths_golden_includes_underscore_folders() {
    let output = run_fixture(&["--tasks", ""]);

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        concat!(
            "Daily/2026-07-10.md\n",
            "Tasks/Dependencies.md\n",
            "Tasks/MetadataDataview.md\n",
            "Tasks/MetadataEmoji.md\n",
            "Tasks/Nested.md\n",
            "Tasks/Statuses.md\n",
            "_generated/Generated.md\n",
            "_templates/Task.md\n",
            "dash.md\n",
        ),
        "filterless Tasks paths golden changed:\n{}",
        format_output(&output)
    );
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
}

#[test]
fn tasks_native_filterless_json_golden_reads_settings_and_tasks() {
    let output = run_fixture(&["--format", "json", "--tasks", ""]);

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let actual = json_stdout(&output);
    assert_eq!(actual["engine"], "native");
    assert_eq!(actual["query_kind"], "tasks");
    assert_eq!(actual["format"], "json");
    assert_eq!(actual["warnings"], json!([]));
    assert_eq!(
        actual["paths"],
        json!([
            "Daily/2026-07-10.md",
            "Tasks/Dependencies.md",
            "Tasks/MetadataDataview.md",
            "Tasks/MetadataEmoji.md",
            "Tasks/Nested.md",
            "Tasks/Statuses.md",
            "_generated/Generated.md",
            "_templates/Task.md",
            "dash.md"
        ])
    );
    assert_eq!(actual["result"]["type"], "tasks");
    assert_eq!(actual["result"]["count"], 33);
    let tasks = actual["result"]["tasks"]
        .as_array()
        .expect("Tasks result array");
    assert_eq!(tasks.len(), 33);
    for task in tasks {
        for field in [
            "file",
            "path",
            "lineNumber",
            "heading",
            "originalMarkdown",
            "description",
            "displayDescription",
            "descriptionWithoutTags",
            "status",
            "priority",
            "created",
            "start",
            "scheduled",
            "due",
            "done",
            "cancelled",
            "recurrenceRule",
            "onCompletion",
            "id",
            "dependsOn",
            "tags",
            "blockId",
            "parentLineNumber",
            "childTaskLineNumbers",
            "isBlocked",
            "isBlocking",
            "urgency",
        ] {
            assert!(task.get(field).is_some(), "missing {field} in {task}");
        }
    }

    let full = find_task(tasks, "#task Complete dataview metadata #hide");
    assert_eq!(full["path"], "Tasks/MetadataDataview.md");
    assert_eq!(full["file"]["folder"], "Tasks/");
    assert_eq!(full["file"]["filename"], "MetadataDataview.md");
    assert_eq!(full["heading"], "Dataview Task Metadata");
    assert_eq!(full["lineNumber"], 2);
    assert_eq!(full["status"]["type"], "TODO");
    assert_eq!(full["priority"], "High");
    assert_eq!(full["priorityNumber"], 1);
    assert_eq!(full["due"]["value"], "2026-07-12");
    assert_eq!(full["scheduled"]["value"], "2026-07-10");
    assert_eq!(full["start"]["value"], "2026-07-08");
    assert_eq!(full["created"]["value"], "2026-07-01");
    assert_eq!(full["done"]["value"], "2026-07-09");
    assert_eq!(full["cancelled"]["value"], "2026-07-11");
    assert_eq!(full["recurrenceRule"], "every week");
    assert_eq!(full["onCompletion"], "keep");
    assert_eq!(full["id"], "dv-all");
    assert_eq!(full["dependsOn"], json!(["done-root"]));
    assert_eq!(full["tags"], json!(["#hide"]));
    assert_eq!(full["blockId"], "dataview-metadata");
    assert_eq!(full["isBlocked"], false);
    assert!(
        (full["urgency"].as_f64().unwrap() - 18.88571428571429).abs() < 0.00001
    );

    let blocked = find_task(tasks, "#task Blocked child");
    assert_eq!(blocked["dependsOn"], json!(["root"]));
    assert_eq!(blocked["isBlocked"], true);
    assert_eq!(find_task(tasks, "#task Blocking root")["isBlocking"], true);
    assert_eq!(
        find_task(tasks, "#task Ready after done dependency")["isBlocked"],
        false
    );
    assert_eq!(
        find_task(tasks, "#task Missing dependency is ignored")["isBlocked"],
        false
    );
    let self_dependency = find_task(tasks, "#task Self dependency");
    assert_eq!(self_dependency["isBlocked"], true);
    assert_eq!(self_dependency["isBlocking"], true);
    assert_eq!(
        find_task(tasks, "#task Duplicate id done instance")["isBlocking"],
        false
    );
    assert_eq!(
        find_task(tasks, "#task Duplicate id open instance")["isBlocking"],
        true
    );
    assert_eq!(
        find_task(tasks, "#task Duplicate id dependent")["isBlocked"],
        true
    );
    assert_eq!(
        find_task(tasks, "#task Ready after canceled dependency")["isBlocked"],
        false
    );

    let invalid_date =
        find_task(tasks, "#task Syntactically valid but nonexistent date");
    assert_eq!(invalid_date["scheduled"]["raw"], "2026-99-99");
    assert_eq!(invalid_date["scheduled"]["valid"], false);
    assert!(invalid_date["scheduled"]["value"].is_null());

    let child = find_task(tasks, "#task Child task");
    assert_eq!(child["parentLineNumber"], 2);
    assert_eq!(child["parentTaskLineNumber"], 2);
    assert_eq!(child["childTaskLineNumbers"], json!([4]));
    let non_task_child =
        find_task(tasks, "#task Child under a non-task list item");
    assert_eq!(non_task_child["parentLineNumber"], 5);
    assert!(non_task_child["parentTaskLineNumber"].is_null());

    let unknown = find_task(tasks, "#task Unknown status becomes TODO");
    assert_eq!(unknown["status"]["symbol"], "?");
    assert_eq!(unknown["status"]["name"], "Unknown");
    assert_eq!(unknown["status"]["type"], "TODO");
    assert_eq!(unknown["status"]["nextSymbol"], "x");
    assert_eq!(unknown["status"]["availableAsCommand"], false);

    // The configured task format is Dataview, so emoji signifiers remain
    // description text. The emoji parser itself is covered by unit tests.
    let emoji = find_task(tasks, "#task Complete emoji metadata #hide 🔺");
    assert_eq!(emoji["priority"], "Normal");
    assert!(emoji["due"].is_null());
    assert_eq!(emoji["id"], "");

    let settings = &actual["settings"];
    assert_eq!(settings["globalFilter"], "#task");
    assert_eq!(settings["globalQuery"], "");
    assert_eq!(settings["removeGlobalFilter"], false);
    assert_eq!(settings["taskFormat"], "dataview");
    assert_eq!(
        settings["statusSettings"]["coreStatuses"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        settings["statusSettings"]["customStatuses"]
            .as_array()
            .map(Vec::len),
        Some(3)
    );
    assert_eq!(
        settings["presets"].as_object().map(serde_json::Map::len),
        Some(8)
    );
}

#[test]
fn tasks_settings_have_stable_defaults_when_plugin_data_is_absent() {
    let temp = TempDir::new("bob-cli-tasks-default-settings");
    write_file(
        &temp.path().join("All.md"),
        "- [ ] Checkbox without a global filter\n",
    );

    let output = run_tasks(temp.path(), &["--format", "json", "--tasks", ""]);
    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let actual = json_stdout(&output);
    assert_eq!(actual["paths"], json!(["All.md"]));
    assert_eq!(actual["result"]["count"], 1);
    assert_eq!(actual["settings"]["globalFilter"], "");
    assert_eq!(actual["settings"]["globalQuery"], "");
    assert_eq!(actual["settings"]["taskFormat"], "tasksPluginEmoji");
    assert_eq!(
        actual["settings"]["statusSettings"]["coreStatuses"],
        json!([
            {
                "symbol": " ",
                "name": "Todo",
                "nextStatusSymbol": "x",
                "availableAsCommand": true,
                "type": "TODO"
            },
            {
                "symbol": "x",
                "name": "Done",
                "nextStatusSymbol": " ",
                "availableAsCommand": true,
                "type": "DONE"
            }
        ])
    );
    assert_eq!(
        actual["settings"]["statusSettings"]["customStatuses"],
        json!([
            {
                "symbol": "/",
                "name": "In Progress",
                "nextStatusSymbol": "x",
                "availableAsCommand": true,
                "type": "IN_PROGRESS"
            },
            {
                "symbol": "-",
                "name": "Cancelled",
                "nextStatusSymbol": " ",
                "availableAsCommand": true,
                "type": "CANCELLED"
            }
        ])
    );
}

#[test]
fn tasks_plugin_emoji_setting_selects_emoji_metadata_parser() {
    let temp = TempDir::new("bob-cli-tasks-emoji-settings");
    write_file(
        &temp
            .path()
            .join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
  "globalFilter": "#task",
  "taskFormat": "tasksPluginEmoji"
}"##,
    );
    write_file(
        &temp.path().join("Emoji.md"),
        "# Emoji\n\n- [ ] #task Emoji metadata 🔺 🔁 every Sunday 📅 2026-07-12 🆔 emoji-id ⛔ missing 🏁 delete ^emoji-block\n",
    );

    let output = run_tasks(temp.path(), &["--format", "json", "--tasks", ""]);
    assert_success(&output);
    let actual = json_stdout(&output);
    let tasks = actual["result"]["tasks"].as_array().unwrap();
    let task = find_task(tasks, "#task Emoji metadata");
    assert_eq!(actual["settings"]["taskFormat"], "tasksPluginEmoji");
    assert_eq!(task["description"], "#task Emoji metadata");
    assert_eq!(task["priority"], "Highest");
    assert_eq!(task["recurrenceRule"], "every week on Sunday");
    assert_eq!(task["due"]["value"], "2026-07-12");
    assert_eq!(task["id"], "emoji-id");
    assert_eq!(task["dependsOn"], json!(["missing"]));
    assert_eq!(task["onCompletion"], "delete");
    assert_eq!(task["blockId"], "emoji-block");
}

#[test]
fn tasks_short_flags_files_stdin_and_comments_reach_filterless_slice() {
    let temp = TempDir::new("bob-cli-tasks-inputs");
    let query_file = temp.path().join("filterless.tasks");
    write_file(&query_file, "# comment-only query\n");

    for (flag, value) in [
        ("-t", "# inline comment"),
        ("-T", query_file.to_str().expect("UTF-8 query path")),
    ] {
        let output = run_fixture(&[flag, value]);
        assert_success(&output);
        assert_eq!(
            stdout(&output).lines().count(),
            9,
            "{}",
            format_output(&output)
        );
    }

    let origin = run_fixture(&["-o", "dash.md", "-t", ""]);
    assert_success(&origin);
    assert_eq!(
        stdout(&origin).lines().count(),
        9,
        "{}",
        format_output(&origin)
    );

    let mut child = Command::new(BOB_BIN)
        .arg("query")
        .arg("-b")
        .arg(fixture_vault_path())
        .arg("-T")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdin Tasks query");
    use std::io::Write;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(b"# stdin comment\n")
        .expect("write Tasks stdin");
    let output = child
        .wait_with_output()
        .expect("wait for stdin Tasks query");
    assert_success(&output);
    assert_eq!(
        stdout(&output).lines().count(),
        9,
        "{}",
        format_output(&output)
    );
}

#[test]
fn tasks_cli_rejects_invalid_combinations_and_unsupported_surface() {
    let cases: &[(&[&str], &str)] = &[
        (&["--tasks", "", "--query", "LIST"], "cannot be used with"),
        (
            &["--tasks", "", "--format", "markdown"],
            "--format markdown is not available for Tasks queries yet",
        ),
        (
            &["--tasks", "", "--engine", "obsidian"],
            "--engine obsidian does not support Tasks queries yet",
        ),
        (
            &["--tasks-note", "dash.md", "--origin", "Home.md"],
            "--origin cannot be used with --tasks-note",
        ),
        (&["--tasks-note", "../dash.md"], "invalid --tasks-note"),
    ];

    for (args, marker) in cases {
        let output = run_fixture(args);
        assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
        assert!(
            stderr(&output).contains(marker),
            "expected {marker:?}:\n{}",
            format_output(&output)
        );
    }

    for (args, marker) in [
        (
            &["--tasks", "status.type is TODO"][..],
            "only an empty or comment-only Tasks query is supported yet",
        ),
        (
            &["-n", "dash.md"][..],
            "running Tasks code blocks from dash.md is not available yet",
        ),
    ] {
        let output = run_fixture(args);
        assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
        assert!(
            stderr(&output).contains(marker),
            "{}",
            format_output(&output)
        );
    }
}

#[test]
fn tasks_live_obsidian_parity_harness_scaffold_documents_render_oracle() {
    if env::var_os(LIVE_PARITY_ENV).is_none() {
        return;
    }

    let vault = env::var(LIVE_PARITY_VAULT_ENV).unwrap_or_else(|_| {
        panic!(
            "{LIVE_PARITY_ENV}=1 requires {LIVE_PARITY_VAULT_ENV} to name an \
             Obsidian vault opened at {}",
            fixture_vault_path().display()
        )
    });
    eprintln!(
        "Tasks live-oracle scaffold enabled for {vault}: Phase 7 will render \
         fenced tasks blocks with MarkdownRenderer.render, wait for async Tasks \
         output, and scrape task rows and group headings from the DOM"
    );
}

fn find_task<'a>(tasks: &'a [Value], description: &str) -> &'a Value {
    tasks
        .iter()
        .find(|task| {
            task["description"]
                .as_str()
                .is_some_and(|value| value.starts_with(description))
        })
        .unwrap_or_else(|| {
            panic!("missing task with description {description:?}")
        })
}

fn run_fixture(args: &[&str]) -> Output {
    run_tasks(&fixture_vault_path(), args)
}

fn run_tasks(vault: &Path, args: &[&str]) -> Output {
    Command::new(BOB_BIN)
        .arg("query")
        .arg("--bob-dir")
        .arg(vault)
        .args(args)
        .env("BOB_NOW", "2026-07-10 12:00:00")
        .output()
        .unwrap_or_else(|error| {
            panic!("run bob query against {}: {error}", vault.display())
        })
}

fn fixture_vault_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tasks_parity/vault")
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("create {}: {error}", parent.display())
        });
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn read_json(path: &Path) -> Value {
    let contents = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&contents)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", format_output(output));
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!("stdout should be JSON: {error}\n{}", format_output(output))
    })
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn format_output(output: &Output) -> String {
    format!(
        "status: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout(output),
        stderr(output)
    )
}
