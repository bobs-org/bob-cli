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

    let daily = fs::read_to_string(vault.join("Daily/2026-07-10.md"))
        .expect("read daily-note fixture");
    assert_eq!(daily.matches("```tasks").count(), 1, "{daily}");
    for instruction in [
        "( status.type is TODO ) OR ( status.type is IN_PROGRESS )",
        "limit groups to 3 tasks",
        "sort by function task.lineNumber",
        "hide toolbar",
    ] {
        assert!(
            daily.contains(instruction),
            "missing daily-note instruction {instruction}"
        );
    }
}

#[test]
fn path_qualified_dependency_ids_keep_duplicate_block_fragments_distinct() {
    let temp = TempDir::new("bob-cli-tasks-qualified-dependencies");
    let vault = temp.path();
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        include_str!(
            "fixtures/tasks_parity/vault/.obsidian/plugins/obsidian-tasks-plugin/data.json"
        ),
    );
    write_file(
        &vault.join("Alpha.md"),
        include_str!("fixtures/tasks_dependency_identity/Alpha.md"),
    );
    write_file(
        &vault.join("Beta.md"),
        include_str!("fixtures/tasks_dependency_identity/Beta.md"),
    );
    write_file(
        &vault.join("Dependents.md"),
        include_str!("fixtures/tasks_dependency_identity/Dependents.md"),
    );

    let output = run_tasks(vault, &["--format", "json", "--tasks", ""]);
    assert_success(&output);
    let value = json_stdout(&output);
    let tasks = value["result"]["tasks"].as_array().unwrap();
    let alpha = find_task(tasks, "#task Alpha review");
    let beta = find_task(tasks, "#task Beta review");
    assert_eq!(alpha["blockId"], "review");
    assert_eq!(beta["blockId"], "review");
    assert_eq!(alpha["id"], "alpha__review");
    assert_eq!(beta["id"], "beta__review");
    assert_eq!(
        find_task(tasks, "#task Blocked only by Alpha")["isBlocked"],
        true
    );
    assert_eq!(
        find_task(tasks, "#task Ready after Beta")["isBlocked"],
        false
    );
}

#[test]
fn tasks_native_filterless_paths_golden_includes_underscore_folders() {
    let output = run_fixture(&["--tasks", ""]);

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        concat!(
            "dash.md\n",
            "Tasks/Statuses.md\n",
            "Tasks/MetadataDataview.md\n",
            "Daily/2026-07-10.md\n",
            "Tasks/Dependencies.md\n",
            "Tasks/MetadataEmoji.md\n",
            "Tasks/Nested.md\n",
            "_generated/Generated.md\n",
            "_templates/Task.md\n",
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
            "dash.md",
            "Tasks/Statuses.md",
            "Tasks/MetadataDataview.md",
            "Daily/2026-07-10.md",
            "Tasks/Dependencies.md",
            "Tasks/MetadataEmoji.md",
            "Tasks/Nested.md",
            "_generated/Generated.md",
            "_templates/Task.md"
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
        7,
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
            &["--tasks", "", "--engine", "obsidian"],
            "--engine obsidian does not support Tasks queries",
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
        (&["--tasks", "spaghetti"][..], "do not understand query"),
        (
            &["--tasks", "description regex matches apple sauce"][..],
            "Regular expressions must look like",
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
fn tasks_note_executes_every_dashboard_block_with_origin_defaults() {
    let output = run_fixture(&["--format", "json", "--tasks-note", "dash.md"]);
    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));

    let actual = json_stdout(&output);
    assert_eq!(actual["engine"], "native");
    assert_eq!(actual["query_kind"], "tasks_note");
    assert_eq!(actual["format"], "json");
    assert_eq!(actual["note"], "dash.md");
    assert_eq!(actual["warnings"], json!([]));
    let blocks = actual["blocks"].as_array().expect("tasks note blocks");
    assert_eq!(blocks.len(), 3);

    for (index, heading, query, count) in [
        (1, "WIP", "status.type is IN_PROGRESS", 1),
        (2, "NEXT", "status.name includes Next", 1),
        (3, "READY", "status.type is TODO", 14),
    ] {
        let block = &blocks[index - 1];
        assert_eq!(block["index"], index);
        assert_eq!(block["heading"], heading);
        assert_eq!(block["query"], query);
        assert_eq!(block["result"]["count"], count);
        assert_eq!(
            block["parsedQuery"]["filters"].as_array().map(Vec::len),
            Some(6),
            "query-file defaults were not composed for {heading}: {block}"
        );
        assert!(block["result"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|task| task["path"] != "dash.md"));
    }

    let inline = run_fixture(&[
        "--format",
        "json",
        "--origin",
        "dash.md",
        "--tasks",
        "status.type is IN_PROGRESS",
    ]);
    assert_success(&inline);
    assert_eq!(
        json_stdout(&inline)["result"]["tasks"],
        blocks[0]["result"]["tasks"],
        "--origin and --tasks-note must apply identical note context/defaults"
    );
}

#[test]
fn tasks_note_identifies_blocks_in_paths_and_markdown_output() {
    let paths = run_fixture(&["--tasks-note", "dash.md"]);
    assert_success(&paths);
    let paths = stdout(&paths);
    for marker in [
        "[dash.md#WIP (block 1, line 20)]",
        "[dash.md#NEXT (block 2, line 26)]",
        "[dash.md#READY (block 3, line 32)]",
    ] {
        assert!(paths.contains(marker), "missing {marker:?}:\n{paths}");
    }

    let markdown =
        run_fixture(&["--format", "markdown", "--tasks-note", "dash.md"]);
    assert_success(&markdown);
    let markdown = stdout(&markdown);
    assert!(markdown.contains("## WIP (block 1)\n"), "{markdown}");
    assert!(markdown.contains("## NEXT (block 2)\n"), "{markdown}");
    assert!(markdown.contains("## READY (block 3)\n"), "{markdown}");
    assert!(markdown.contains("#task In Progress status"), "{markdown}");
    assert!(!markdown.contains("Dashboard WIP"), "{markdown}");
}

#[test]
fn tasks_note_and_origin_normalize_leading_current_directory_components() {
    let note = run_fixture(&["--format", "json", "--tasks-note", "./dash.md"]);
    assert_success(&note);
    let note = json_stdout(&note);
    assert_eq!(note["note"], "dash.md");

    let inline = run_fixture(&[
        "--format",
        "json",
        "--origin",
        "./dash.md",
        "--tasks",
        "status.type is IN_PROGRESS",
    ]);
    assert_success(&inline);
    assert_eq!(
        json_stdout(&inline)["result"]["tasks"],
        note["blocks"][0]["result"]["tasks"]
    );
}

#[test]
fn tasks_note_reports_the_failing_block_context() {
    let temp = TempDir::new("bob-cli-tasks-note-error");
    write_file(
        &temp.path().join("Queries.md"),
        "# Queries\n\n## Broken\n\n```tasks\nspaghetti\n```\n",
    );
    let output = run_tasks(temp.path(), &["--tasks-note", "Queries.md"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(stdout(&output).is_empty(), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("Queries.md#Broken (block 1, line 5)"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn tasks_query_parser_composes_dash_defaults_and_serializes_the_ast() {
    let output = run_fixture(&[
        "--format",
        "json",
        "--origin",
        "dash.md",
        "--tasks",
        "status.type is IN_PROGRESS",
    ]);

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let actual = json_stdout(&output);
    let query = &actual["query"];
    assert_eq!(query["filters"].as_array().map(Vec::len), Some(6));
    assert_eq!(query["sorting"].as_array().map(Vec::len), Some(2));
    assert_eq!(query["grouping"], json!([]));
    assert_eq!(query["ignoreGlobalQuery"], false);
    assert_eq!(query["sorting"][0]["key"], "function");
    assert_eq!(query["sorting"][0]["function"], "task.file.path");
    assert_eq!(query["sorting"][1]["function"], "task.lineNumber");
    assert_eq!(query["filters"][5]["type"], "statusType");
    assert_eq!(query["filters"][5]["value"], "IN_PROGRESS");
    assert!(query["statements"]
        .as_array()
        .unwrap()
        .iter()
        .any(|statement| {
            statement["source"] == "queryFileDefaults"
                && statement["instruction"]
                    == "folder does not include _templates"
        }));
}

#[test]
fn tasks_query_parser_accepts_the_daily_note_query_surface() {
    let query = concat!(
        "# comment\n",
        "( status.type is TODO ) OR ( status.type is IN_PROGRESS )\n",
        "is not blocked\n",
        "filter by function task.file.path !== query.file.path\n",
        "filter by function !task.scheduled.moment || \\\n         task.scheduled.moment.isSameOrBefore(moment(query.file.filenameWithoutExtension, \"YYYYMMDD\"), \"day\")\n",
        "filter by function !task.tags.includes(\"#hide\")\n",
        "group by path\n",
        "limit groups to 3 tasks\n",
        "sort by function task.file.path\n",
        "sort by function task.lineNumber\n",
        "short mode\n",
        "hide toolbar\n",
    );
    let output = run_fixture(&[
        "--format",
        "json",
        "--origin",
        "Daily/2026-07-10.md",
        "--tasks",
        query,
    ]);

    assert_success(&output);
    let actual = json_stdout(&output);
    let query = &actual["query"];
    assert_eq!(query["filters"].as_array().map(Vec::len), Some(5));
    assert_eq!(query["sorting"].as_array().map(Vec::len), Some(2));
    assert_eq!(query["grouping"].as_array().map(Vec::len), Some(1));
    assert_eq!(query["limitGroups"], 3);
    assert_eq!(query["layout"]["shortMode"], true);
    assert_eq!(query["layout"]["showToolbar"], false);
}

#[test]
fn tasks_by_function_exposes_tasks_query_context_and_real_moment_dates() {
    let query = concat!(
        "filter by function task.descriptionWithoutTags === \"Complete dataview metadata\"\n",
        "filter by function task.status.type === \"TODO\" && task.priorityName === \"High\" && task.priorityNumber === 1\n",
        "filter by function task.due.moment.isSame(moment(\"2026-07-12\"), \"day\") && task.scheduled.formatAsDate() === \"2026-07-10\"\n",
        "filter by function task.file.path === \"Tasks/MetadataDataview.md\" && task.file.folder === \"Tasks/\" && task.heading === \"Dataview Task Metadata\" && task.lineNumber === 2\n",
        "filter by function task.isRecurring && task.id === \"dv-all\" && task.dependsOn.includes(\"done-root\") && !task.isBlocked(query.allTasks)\n",
        "filter by function query.allTasks.length === 33 && query.file.path === \"Daily/2026-07-10.md\" && query.file.hasProperty(\"date\") && query.file.property(\"date\") === \"2026-07-10\"\n",
        "filter by function moment().format(\"YYYY-MM-DD HH:mm\") === \"2026-07-10 12:00\"\n",
    );
    let output = run_fixture(&[
        "--format",
        "json",
        "--origin",
        "Daily/2026-07-10.md",
        "--tasks",
        query,
    ]);

    assert_success(&output);
    let actual = json_stdout(&output);
    assert_eq!(actual["result"]["count"], 1);
    assert_eq!(
        actual["result"]["tasks"][0]["description"],
        "#task Complete dataview metadata #hide"
    );
}

#[test]
fn tasks_by_function_runs_dash_filters_and_stacked_sorts() {
    let query = concat!(
        "filter by function task.file.path !== query.file.path\n",
        "filter by function !task.scheduled.moment || task.scheduled.moment.isSameOrBefore(moment(), \"day\")\n",
        "filter by function !task.tags.includes(\"#hide\")\n",
        "sort by function task.file.path\n",
        "sort by function task.lineNumber\n",
    );
    let output = run_fixture(&[
        "--format", "json", "--origin", "dash.md", "--tasks", query,
    ]);

    assert_success(&output);
    let actual = json_stdout(&output);
    let tasks = actual["result"]["tasks"].as_array().unwrap();
    assert!(tasks.iter().all(|task| task["path"] != "dash.md"));
    assert!(tasks.iter().all(|task| {
        !task["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tag| tag == "#hide")
    }));
    let keys = tasks
        .iter()
        .map(|task| {
            (
                task["path"].as_str().unwrap(),
                task["lineNumber"].as_u64().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert!(keys.windows(2).all(|pair| pair[0] <= pair[1]), "{keys:?}");
}

#[test]
fn tasks_group_by_function_reports_array_keys_and_runtime_errors() {
    let output = run_fixture(&[
        "--format",
        "json",
        "--tasks",
        "group by function task.tags.length ? task.tags : [\"untagged\"]",
    ]);
    assert_success(&output);
    let actual = json_stdout(&output);
    let entries = actual["result"]["functionGroups"][0]["tasks"]
        .as_array()
        .unwrap();
    let tagged = entries
        .iter()
        .find(|entry| {
            entry["path"] == "Tasks/MetadataDataview.md"
                && entry["lineNumber"] == 2
        })
        .unwrap();
    assert_eq!(tagged["groups"], json!(["#hide"]));
    assert!(entries
        .iter()
        .any(|entry| entry["groups"] == json!(["untagged"])));

    let error = run_fixture(&[
        "--format",
        "json",
        "--tasks",
        "group by function task.missing.property",
    ]);
    assert_success(&error);
    let error = json_stdout(&error);
    let groups = &error["result"]["functionGroups"][0]["tasks"][0]["groups"];
    assert!(
        groups[0]
            .as_str()
            .unwrap()
            .contains("Failed calculating expression"),
        "{groups}"
    );
}

#[test]
fn tasks_by_function_surfaces_parse_return_type_and_sort_errors() {
    for query in [
        "filter by function task.description.length",
        "filter by function task.missing(",
        "sort by function task.missing",
    ] {
        let output = run_fixture(&["--format", "json", "--tasks", query]);
        assert!(
            !output.status.success(),
            "{query}\n{}",
            format_output(&output)
        );
        let error = stderr(&output);
        assert!(error.contains("Tasks query failed"), "{query}: {error}");
        assert!(
            error.contains("JavaScript")
                || error.contains("filtering function"),
            "{query}: {error}"
        );
    }

    let interrupted = run_fixture(&[
        "--format",
        "json",
        "--tasks",
        "filter by function while (true) {} return true",
    ]);
    assert!(
        !interrupted.status.success(),
        "{}",
        format_output(&interrupted)
    );
    assert!(
        stderr(&interrupted).contains("interrupted"),
        "{}",
        format_output(&interrupted)
    );
}

#[test]
fn tasks_result_pipeline_sorts_groups_and_limits_like_tasks_v8() {
    let output = run_fixture(&[
        "--format",
        "json",
        "--tasks",
        concat!(
            "folder includes Tasks/\n",
            "sort by status.name\n",
            "sort by function task.lineNumber\n",
            "group by status.type\n",
            "group by path\n",
            "limit groups 2",
        ),
    ]);
    assert_success(&output);
    let actual = json_stdout(&output);

    assert_eq!(actual["result"]["countBeforeLimit"], 27);
    assert_eq!(actual["result"]["count"], 18);
    assert_eq!(actual["result"]["countText"], "18 of 27 tasks");
    assert_eq!(
        actual["paths"],
        json!([
            "Tasks/Statuses.md",
            "Tasks/Dependencies.md",
            "Tasks/MetadataDataview.md",
            "Tasks/MetadataEmoji.md",
            "Tasks/Nested.md"
        ])
    );

    let groups = actual["result"]["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 12);
    assert_eq!(
        groups[0]["names"],
        json!(["%%1%%IN_PROGRESS", "Tasks/Statuses"])
    );
    assert_eq!(
        groups[0]["headings"],
        json!([
            {"level": 0, "name": "%%1%%IN_PROGRESS"},
            {"level": 1, "name": "Tasks/Statuses"}
        ])
    );
    let limited = groups
        .iter()
        .find(|group| {
            group["names"] == json!(["%%2%%TODO", "Tasks/Dependencies"])
        })
        .unwrap();
    assert_eq!(limited["count"], 2);
    assert_eq!(limited["countBeforeLimit"], 9);
    assert_eq!(limited["countText"], "2 of 9 tasks");
    assert_eq!(
        limited["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|task| task["description"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["#task Blocking root", "#task Blocked child"]
    );
}

#[test]
fn tasks_markdown_honors_tree_layout_fields_counts_and_explain() {
    let tree = run_fixture(&[
        "--format",
        "markdown",
        "--tasks",
        concat!(
            "description includes Parent task\n",
            "show tree\n",
            "hide backlink\n",
            "hide task count\n",
            "hide id\n",
            "hide tags",
        ),
    ]);
    assert_success(&tree);
    assert_eq!(
        stdout(&tree),
        concat!(
            "- [ ] Parent task ^parent-task\n",
            "    - [ ] Child task ^child-task\n",
            "        - [x] Done grandchild ^grandchild-task\n",
        )
    );

    let explained = run_fixture(&[
        "--format",
        "markdown",
        "--tasks",
        concat!(
            "description includes Dashboard WIP\n",
            "hide backlink\n",
            "short mode\n",
            "explain",
        ),
    ]);
    assert_success(&explained);
    assert_eq!(
        stdout(&explained),
        concat!(
            "Only tasks containing the global filter '#task'.\n",
            "\n",
            "Explanation of this Tasks code block query:\n",
            "\n",
            "  description includes Dashboard WIP\n",
            "\n",
            "  hide backlink\n",
            "\n",
            "  short mode\n",
            "\n",
            "- [/] #task Dashboard WIP [scheduled::]\n",
            "\n",
            "1 task\n",
        )
    );
}

#[test]
fn tasks_every_native_sort_and_group_key_executes() {
    for key in [
        "cancelled",
        "created",
        "description",
        "done",
        "due",
        "filename",
        "happens",
        "heading",
        "id",
        "path",
        "priority",
        "random",
        "recurring",
        "scheduled",
        "start",
        "status",
        "status.name",
        "status.type",
        "tag",
        "urgency",
    ] {
        let query = format!("sort by {key} reverse\nlimit 3");
        let output = run_fixture(&["--format", "json", "--tasks", &query]);
        assert_success(&output);
        assert_eq!(json_stdout(&output)["result"]["count"], 3, "{key}");
    }

    for key in [
        "backlink",
        "cancelled",
        "created",
        "done",
        "due",
        "filename",
        "folder",
        "happens",
        "heading",
        "id",
        "path",
        "priority",
        "recurrence",
        "recurring",
        "root",
        "scheduled",
        "start",
        "status",
        "status.name",
        "status.type",
        "tags",
        "urgency",
    ] {
        let query = format!("group by {key} reverse\nlimit groups 1");
        let output = run_fixture(&["--format", "json", "--tasks", &query]);
        assert_success(&output);
        assert!(
            !json_stdout(&output)["result"]["groups"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{key}"
        );
    }
}

#[test]
fn tasks_native_filter_families_match_fixture_goldens() {
    for (query, expected) in [
        (
            "status.type is IN_PROGRESS",
            &["#task In Progress status", "#task Dashboard WIP"][..],
        ),
        (
            "status.name includes next",
            &["#task Next status", "#task Dashboard NEXT"][..],
        ),
        (
            "description regex matches /COMPLETE DATAVIEW/i",
            &["#task Complete dataview metadata #hide"][..],
        ),
        (
            "tag includes hide",
            &[
                "#task Complete dataview metadata #hide",
                "#task Complete emoji metadata #hide 🔺",
            ][..],
        ),
        (
            "priority is above medium",
            &["#task Complete dataview metadata #hide"][..],
        ),
        (
            "recurrence includes week",
            &["#task Complete dataview metadata #hide"][..],
        ),
        (
            "is blocked",
            &[
                "#task Blocked child",
                "#task Mixed dependencies",
                "#task Self dependency",
                "#task Duplicate id dependent",
            ][..],
        ),
        (
            "is blocking",
            &[
                "#task Blocking root",
                "#task Self dependency",
                "#task Duplicate id open instance",
            ][..],
        ),
        ("exclude sub-items", &[]),
    ] {
        let actual = filtered_descriptions(query);
        if query == "exclude sub-items" {
            assert!(
                !actual.iter().any(|description| {
                    description.starts_with("#task Child task")
                        || description.starts_with(
                            "#task Child under a non-task list item",
                        )
                }),
                "{query}: {actual:?}"
            );
            assert!(
                actual
                    .iter()
                    .any(|description| description == "#task Parent task"),
                "{query}: {actual:?}"
            );
        } else {
            assert_description_prefixes(query, &actual, expected);
        }
    }
}

#[test]
fn tasks_native_date_filters_match_pinned_range_boundaries() {
    for (query, expected) in [
        (
            "scheduled today",
            &[
                "#task Daily note task",
                "#task Complete dataview metadata #hide",
                "#task Dashboard WIP",
            ][..],
        ),
        (
            "due this week",
            &["#task Complete dataview metadata #hide"][..],
        ),
        (
            "scheduled date is invalid",
            &["#task Syntactically valid but nonexistent date"][..],
        ),
        (
            "happens on 2026-07-10",
            &[
                "#task Daily note task",
                "#task Complete dataview metadata #hide",
                "#task Dashboard WIP",
            ][..],
        ),
        (
            "starts after today",
            &[
                // Tasks v8 intentionally treats a missing start date as a
                // match for every starts comparison.
                "#task Daily note task",
            ][..],
        ),
    ] {
        let actual = filtered_descriptions(query);
        if query == "starts after today" {
            assert!(
                actual
                    .iter()
                    .any(|description| description == "#task Daily note task"),
                "{query}: {actual:?}"
            );
            assert!(
                !actual.iter().any(|description| {
                    description.starts_with("#task Complete dataview metadata")
                }),
                "{query}: {actual:?}"
            );
        } else {
            assert_description_prefixes(query, &actual, expected);
        }
    }
}

#[test]
fn tasks_native_boolean_and_implicit_and_filters_match_goldens() {
    let boolean = filtered_descriptions(
        "(status.type is TODO) AND NOT ((is blocked) OR (tag includes #hide))",
    );
    assert!(
        boolean
            .iter()
            .any(|description| description == "#task Todo status"),
        "{boolean:?}"
    );
    assert!(
        !boolean.iter().any(|description| {
            description.starts_with("#task Blocked child")
                || description.starts_with("#task Complete dataview metadata")
        }),
        "{boolean:?}"
    );

    assert_eq!(
        filtered_descriptions("status.type is TODO\nfolder includes Tasks/"),
        filtered_descriptions(
            "(status.type is TODO) AND (folder includes Tasks/)",
        )
    );
    assert!(filtered_descriptions("description includes #task").is_empty());
}

#[test]
fn tasks_native_dashboard_defaults_run_function_filters_with_pinned_moment() {
    let output = run_fixture(&[
        "--format",
        "json",
        "--origin",
        "dash.md",
        "--tasks",
        "status.type is TODO",
    ]);
    assert_success(&output);
    let actual = json_stdout(&output);
    let descriptions = result_descriptions(&actual);
    assert_eq!(actual["result"]["count"], 14);
    for excluded in [
        "#task Blocked child",
        "#task Mixed dependencies",
        "#task Self dependency",
        "#task Duplicate id dependent",
        "#task Template task is indexed",
        "#task Complete dataview metadata #hide",
        "#task Complete emoji metadata #hide",
        "#task Dashboard READY",
    ] {
        assert!(
            !descriptions
                .iter()
                .any(|description| description.starts_with(excluded)),
            "unexpected {excluded:?} in {descriptions:?}"
        );
    }
    assert!(
        descriptions
            .iter()
            .any(|description| description == "#task Daily note task"),
        "{descriptions:?}"
    );
}

#[test]
fn tasks_live_obsidian_parity_harness_renders_and_scrapes_tasks_blocks() {
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
    let query = "status.type is IN_PROGRESS";
    let Some(oracle) = run_live_tasks_oracle(&vault, "dash.md", query) else {
        eprintln!(
            "Tasks live oracle skipped: Obsidian is unavailable or is not \
             running with vault {vault:?} open"
        );
        return;
    };

    assert!(
        oracle["errors"].as_array().is_some_and(Vec::is_empty),
        "live Tasks renderer errors: {oracle}"
    );
    assert!(oracle["groupHeadings"].is_array(), "{oracle}");
    let rows = oracle["tasks"].as_array().expect("oracle task rows");
    assert!(
        !rows.is_empty(),
        "live Tasks oracle returned no rows: {oracle}"
    );

    let native = run_fixture(&[
        "--format", "json", "--origin", "dash.md", "--tasks", query,
    ]);
    assert_success(&native);
    let native = json_stdout(&native);
    let expected = native["result"]["tasks"]
        .as_array()
        .expect("native task rows");
    assert_eq!(rows.len(), expected.len(), "live/native count mismatch");
    for task in expected {
        let description = task["displayDescription"].as_str().unwrap();
        let symbol = task["status"]["symbol"].as_str().unwrap();
        assert!(
            rows.iter().any(|row| {
                row["description"]
                    .as_str()
                    .is_some_and(|actual| actual.contains(description))
                    && row["statusSymbol"] == symbol
            }),
            "live oracle did not contain native task {description:?}: {oracle}"
        );
    }
}

fn run_live_tasks_oracle(
    vault: &str,
    source_path: &str,
    query: &str,
) -> Option<Value> {
    let request = json!({
        "markdown": format!("```tasks\n{query}\n```"),
        "sourcePath": source_path,
    });
    let request =
        serde_json::to_string(&request).expect("serialize oracle request");
    let prefix = "BOB_TASKS_RESULT\t";
    let prefix_json = serde_json::to_string(prefix).unwrap();
    let script = format!(
        r#"
(async () => {{
  const request = {request};
  const prefix = {prefix_json};
  const {{ MarkdownRenderer, Component }} = require("obsidian");
  const root = document.createElement("div");
  root.style.display = "none";
  document.body.appendChild(root);
  const component = new Component();
  component.load();
  try {{
    await MarkdownRenderer.render(
      app,
      request.markdown,
      root,
      request.sourcePath,
      component,
    );
    let previous = "";
    let stable = 0;
    for (let attempt = 0; attempt < 100; attempt += 1) {{
      const signature = root.innerHTML;
      stable = signature === previous ? stable + 1 : 0;
      previous = signature;
      if (stable >= 3 && root.querySelector("li.task-list-item, .tasks-error, .plugin-tasks-query-result")) break;
      await new Promise(resolve => setTimeout(resolve, 50));
    }}
    const tasks = [...root.querySelectorAll("li.task-list-item")].map(row => {{
      const descriptionNode = row.querySelector(".task-description");
      const clone = (descriptionNode ?? row).cloneNode(true);
      clone.querySelectorAll(
        "input, button, ul, ol, .tasks-backlink, .task-extras, .task-list-item-checkbox"
      ).forEach(node => node.remove());
      const checkbox = row.querySelector("input[type=checkbox]");
      const backlink = row.querySelector(
        ".tasks-backlink a.internal-link, a.tasks-backlink, a.internal-link[data-href]"
      );
      return {{
        description: (clone.textContent ?? "").replace(/\s+/g, " ").trim(),
        statusSymbol: row.getAttribute("data-task")
          ?? checkbox?.getAttribute("data-task")
          ?? (checkbox?.checked ? "x" : " "),
        backlinkTarget: backlink?.getAttribute("data-href")
          ?? backlink?.getAttribute("href")
          ?? null,
      }};
    }});
    const groupHeadings = [...root.querySelectorAll(
      ".task-group > h4, .task-group > h5, .task-group > h6, .tasks-group-heading"
    )].map(node => (node.textContent ?? "").replace(/\s+/g, " ").trim());
    const errors = [...root.querySelectorAll(".tasks-error")]
      .map(node => (node.textContent ?? "").replace(/\s+/g, " ").trim());
    console.log(prefix + JSON.stringify({{ tasks, groupHeadings, errors }}));
  }} catch (error) {{
    console.log(prefix + JSON.stringify({{
      tasks: [],
      groupHeadings: [],
      errors: [String(error?.stack ?? error)],
    }}));
  }} finally {{
    component.unload();
    root.remove();
  }}
}})();
"#
    );

    let command = env::var_os("BOB_DATAVIEW_OBSIDIAN_COMMAND")
        .unwrap_or_else(|| "obsidian".into());
    let output = match Command::new(command)
        .arg(format!("vault={vault}"))
        .arg("eval")
        .arg(format!("code={script}"))
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return None;
        }
        Err(error) => panic!("run Obsidian Tasks live oracle: {error}"),
    };
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let payloads = stdout
        .lines()
        .filter_map(|line| line.strip_prefix(prefix))
        .collect::<Vec<_>>();
    assert_eq!(payloads.len(), 1, "malformed live oracle output: {stdout}");
    Some(serde_json::from_str(payloads[0]).expect("parse live oracle JSON"))
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

fn filtered_descriptions(query: &str) -> Vec<String> {
    let query = format!(
        "{query}\nsort by function task.file.path\nsort by function task.lineNumber"
    );
    let output = run_fixture(&["--format", "json", "--tasks", &query]);
    assert_success(&output);
    result_descriptions(&json_stdout(&output))
}

fn result_descriptions(value: &Value) -> Vec<String> {
    value["result"]["tasks"]
        .as_array()
        .expect("Tasks result array")
        .iter()
        .map(|task| {
            task["description"]
                .as_str()
                .expect("task description")
                .to_string()
        })
        .collect()
}

fn assert_description_prefixes(
    query: &str,
    actual: &[String],
    expected: &[&str],
) {
    assert_eq!(actual.len(), expected.len(), "{query}: {actual:?}");
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            actual.starts_with(expected),
            "{query}: expected {expected:?}, got {actual:?}"
        );
    }
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
