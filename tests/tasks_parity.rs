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
    assert_eq!(
        actual["result"],
        json!({
            "type": "tasks",
            "count": 25,
            "tasks": [
                task("Daily/2026-07-10.md", " ", "#task Daily note task [scheduled:: 2026-07-10]"),
                task("Tasks/Dependencies.md", " ", "#task Blocking root [id:: root]"),
                task("Tasks/Dependencies.md", " ", "#task Blocked child [id:: blocked] [dependsOn:: root]"),
                task("Tasks/Dependencies.md", "x", "#task Done dependency [id:: done-root]"),
                task("Tasks/Dependencies.md", " ", "#task Ready after done dependency [dependsOn:: done-root]"),
                task("Tasks/Dependencies.md", " ", "#task Mixed dependencies [id:: mixed] [dependsOn:: root, done-root]"),
                task("Tasks/MetadataDataview.md", " ", "#task Complete dataview metadata #hide [due:: 2026-07-12] [scheduled:: 2026-07-10] [start:: 2026-07-08] [created:: 2026-07-01] [completion:: 2026-07-09] [cancelled:: 2026-07-11] [priority:: high] [repeat:: every week] [id:: dv-all] [dependsOn:: done-root] [onCompletion:: keep] ^dataview-metadata"),
                task("Tasks/MetadataDataview.md", " ", "#task Invalid dataview dates [due:: not-a-date] [scheduled:: 2026-99-99] [start:: invalid] [created:: never] [completion:: nope] [cancelled:: ???] [priority:: low]"),
                task("Tasks/MetadataEmoji.md", " ", "#task Complete emoji metadata #hide 🔺 🔁 every week 🛫 2026-07-08 ⏳ 2026-07-10 📅 2026-07-12 ➕ 2026-07-01 ✅ 2026-07-09 ❌ 2026-07-11 🆔 emoji-all ⛔ done-root 🏁 keep ^emoji-metadata"),
                task("Tasks/MetadataEmoji.md", " ", "#task Low emoji priority 🔽"),
                task("Tasks/Nested.md", " ", "#task Parent task ^parent-task"),
                task("Tasks/Nested.md", " ", "#task Child task ^child-task"),
                task("Tasks/Nested.md", "x", "#task Done grandchild ^grandchild-task"),
                task("Tasks/Nested.md", " ", "#task Child under a non-task list item ^non-task-parent-child"),
                task("Tasks/Statuses.md", " ", "#task Todo status"),
                task("Tasks/Statuses.md", "/", "#task In Progress status"),
                task("Tasks/Statuses.md", "*", "#task Next status"),
                task("Tasks/Statuses.md", "x", "#task Done status"),
                task("Tasks/Statuses.md", "-", "#task Canceled status"),
                task("Tasks/Statuses.md", "?", "#task Unknown status becomes TODO"),
                task("_generated/Generated.md", " ", "#task Generated task is indexed"),
                task("_templates/Task.md", " ", "#task Template task is indexed unless a query excludes this folder"),
                task("dash.md", "/", "#task Dashboard WIP [scheduled:: 2026-07-10]"),
                task("dash.md", "*", "#task Dashboard NEXT"),
                task("dash.md", " ", "#task Dashboard READY"),
            ]
        })
    );

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
    assert_eq!(actual["settings"]["taskFormat"], "emoji");
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

fn task(path: &str, status: &str, text: &str) -> Value {
    json!({"path": path, "status": status, "text": text})
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
