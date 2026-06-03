use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde_json::{json, Value};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

const BOB_BIN: &str = env!("CARGO_BIN_EXE_bob");
const LIVE_PARITY_ENV: &str = "BOB_DATAVIEW_PARITY_LIVE";
const LIVE_PARITY_VAULT_ENV: &str = "BOB_DATAVIEW_PARITY_VAULT";

struct TempDir {
    path: PathBuf,
}

struct NativeFailureCase {
    name: &'static str,
    args: &'static [&'static str],
    markers: &'static [&'static str],
}

struct LiveCase {
    name: &'static str,
    args: &'static [&'static str],
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
fn dataview_parity_fixture_vault_covers_contract_surface() {
    let vault = fixture_vault_path();
    for relative in [
        "Projects/Alpha.md",
        "Projects/Beta.md",
        "Projects/Gamma.md",
        "Archive/Old.md",
        "Daily/2026-06-03.md",
        "Links/Hub.md",
        "Links/Target.md",
        "Origins/Origin.md",
        "People/Ada Lovelace.md",
        "People/Grace Hopper.md",
        "Tasks/Nested.md",
        ".obsidian/plugins/dataview/data.json",
    ] {
        assert!(
            vault.join(relative).is_file(),
            "missing Dataview parity fixture file: {relative}"
        );
    }

    let alpha = fs::read_to_string(vault.join("Projects/Alpha.md"))
        .expect("read Alpha");
    assert!(alpha.contains("aliases:"), "{alpha}");
    assert!(alpha.contains("metrics:"), "{alpha}");
    assert!(alpha.contains("nullable: null"), "{alpha}");
    assert!(alpha.contains("2026-06-01T09:30:00"), "{alpha}");
    assert!(alpha.contains("estimate: 2 hours"), "{alpha}");
    assert!(alpha.contains("status-inline:: active"), "{alpha}");
    assert!(alpha.contains("#project/active"), "{alpha}");
    assert!(alpha.contains("[[Projects/Beta]]"), "{alpha}");
    assert!(alpha.contains("- [ ] Kickoff"), "{alpha}");
    assert!(alpha.contains("[completion:: 2026-06-02]"), "{alpha}");

    let tasks =
        fs::read_to_string(vault.join("Tasks/Nested.md")).expect("read tasks");
    assert!(tasks.contains("  - [x] Completed child"), "{tasks}");
    assert!(tasks.contains("^sibling-task"), "{tasks}");

    let origin = fs::read_to_string(vault.join("Origins/Origin.md"))
        .expect("read origin");
    assert!(origin.contains("current_project:"), "{origin}");
}

#[test]
fn dataview_native_current_paths_golden_uses_fixture_vault() {
    let output = run_native_fixture(&[
        "--strict-paths",
        "--query",
        r#"LIST FROM "Projects" WHERE ready"#,
    ]);

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "Projects/Alpha.md\nProjects/Gamma.md\n",
        "native paths output changed:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "native paths golden should keep stderr clean:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_native_current_json_golden_uses_bob_wrapper_shape() {
    let output = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"TABLE status, owner, ready, nullable, missing FROM "Projects""#,
    ]);

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "native JSON golden should keep stderr clean:\n{}",
        format_output(&output)
    );
    assert_json_stdout_eq(
        &output,
        json!({
            "engine": "native",
            "query_kind": "dql",
            "format": "json",
            "paths": [
                "Projects/Alpha.md",
                "Projects/Beta.md",
                "Projects/Gamma.md"
            ],
            "result": {
                "type": "table",
                "headers": ["status", "owner", "ready", "nullable", "missing"],
                "values": [
                    [
                        "active",
                        {
                            "type": "link",
                            "path": "People/Ada Lovelace.md",
                            "display": "Ada",
                            "embed": false
                        },
                        true,
                        null,
                        null
                    ],
                    [
                        "waiting",
                        {
                            "type": "link",
                            "path": "People/Grace Hopper.md",
                            "display": null,
                            "embed": false
                        },
                        false,
                        null,
                        null
                    ],
                    [null, null, true, null, null]
                ]
            },
            "warnings": []
        }),
    );
}

#[test]
fn dataview_native_index_values_cover_yaml_inline_dates_and_links() {
    let output = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"TABLE aliases, metrics.points, due, started, estimate, status-inline, budget, reviewer, related FROM "Projects""#,
    ]);

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json = json_stdout(&output);
    assert_eq!(
        json["paths"],
        json!(["Projects/Alpha.md", "Projects/Beta.md", "Projects/Gamma.md"])
    );
    let values = json["result"]["values"].as_array().expect("table values");

    assert_eq!(values[0][0], json!(["Alpha", "Project Alpha"]));
    assert_eq!(values[0][1], 8);
    assert_eq!(values[0][2], "2026-06-15");
    assert_eq!(values[0][3], "2026-06-01T09:30:00");
    assert_eq!(values[0][4], "PT2H");
    assert_eq!(values[0][5], "active");
    assert_eq!(values[0][6], 42);
    assert_eq!(values[0][7], dataview_link("People/Grace Hopper.md", None));
    assert_eq!(
        values[0][8],
        json!([dataview_link("Projects/Beta.md", None)])
    );

    assert_eq!(values[1][0], json!(["Beta Project"]));
    assert_eq!(values[1][2], "2026-07-01");
    assert_eq!(values[1][4], "P3D");
    assert!(values[2][2].is_null(), "blank YAML value should be null");
}

#[test]
fn dataview_native_index_builds_file_metadata_and_link_graph() {
    let output = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"TABLE file.name, file.folder, file.path, file.link, file.tags, file.etags, file.aliases, file.outlinks, file.inlinks, file.day, file.starred FROM "Projects""#,
    ]);

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json = json_stdout(&output);
    let values = json["result"]["values"].as_array().expect("table values");
    let alpha = values[0].as_array().expect("alpha row");

    assert_eq!(alpha[0], "Alpha");
    assert_eq!(alpha[1], "Projects");
    assert_eq!(alpha[2], "Projects/Alpha.md");
    assert_eq!(alpha[3], dataview_link("Projects/Alpha.md", None));
    assert_eq!(
        alpha[4],
        json!(["#project", "#project/active", "#task", "#task/project"])
    );
    assert_eq!(
        alpha[5],
        json!(["#project", "#project/active", "#task/project"])
    );
    assert_eq!(alpha[6], json!(["Alpha", "Project Alpha"]));
    assert_array_contains(&alpha[7], dataview_link("Links/Hub.md", None));
    assert_array_contains(&alpha[8], dataview_link("Origins/Origin.md", None));
    assert!(alpha[9].is_null(), "project notes should not have file.day");
    assert_eq!(alpha[10], false);

    let daily = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"TABLE file.day, file.tags FROM "Daily""#,
    ]);
    assert_success(&daily);
    let daily_json = json_stdout(&daily);
    assert_eq!(daily_json["paths"], json!(["Daily/2026-06-03.md"]));
    assert_eq!(daily_json["result"]["values"][0][0], "2026-06-03");
}

#[test]
fn dataview_native_index_builds_task_and_list_objects() {
    let output = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"TABLE file.tasks, file.lists FROM "Tasks""#,
    ]);

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json = json_stdout(&output);
    let row = json["result"]["values"][0].as_array().expect("task row");
    let tasks = row[0].as_array().expect("file.tasks");
    let lists = row[1].as_array().expect("file.lists");

    assert_eq!(tasks.len(), 4, "task list should be flat");
    assert_eq!(lists.len(), 4, "list index should include task list items");

    let parent = tasks[0].as_object().expect("parent task");
    assert_eq!(parent["text"], "Parent task #project");
    assert_eq!(parent["due"], "2026-06-08");
    assert_eq!(parent["status"], " ");
    assert_eq!(parent["completed"], false);
    assert_eq!(
        parent["owner"],
        dataview_link("People/Grace Hopper.md", None)
    );
    assert_eq!(parent["tags"], json!(["#project"]));
    assert_eq!(
        parent["children"]
            .as_array()
            .expect("parent children")
            .len(),
        2
    );

    let completed_child = tasks[1].as_object().expect("completed child");
    assert_eq!(completed_child["text"], "Completed child");
    assert_eq!(completed_child["completion"], "2026-06-01");
    assert_eq!(completed_child["status"], "x");
    assert_eq!(completed_child["completed"], true);

    let sibling = tasks[3].as_object().expect("sibling task");
    assert_eq!(sibling["blockId"], "sibling-task");
    assert_eq!(
        sibling["link"],
        dataview_link("Tasks/Nested.md#^sibling-task", None)
    );
}

#[test]
fn dataview_native_index_skips_hidden_directories() {
    let temp = TempDir::new("bob-cli-dataview-hidden-index");
    let vault = temp.path().join("vault");
    copy_dir_all(&fixture_vault_path(), &vault)
        .unwrap_or_else(|error| panic!("copy fixture vault: {error}"));
    fs::create_dir_all(vault.join(".hidden"))
        .unwrap_or_else(|error| panic!("create hidden directory: {error}"));
    fs::write(
        vault.join(".hidden/Secret.md"),
        "---\nready: true\n---\n# Secret\n",
    )
    .unwrap_or_else(|error| panic!("write hidden note: {error}"));

    let output = run_bob_dataview(
        &vault,
        Some("native"),
        None,
        &["--query", "LIST WHERE ready"],
    );

    assert_success(&output);
    assert!(
        !stdout(&output).contains(".hidden/Secret.md"),
        "hidden notes must not be indexed:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_native_expected_failures_record_future_parity_contract() {
    let cases = [
        NativeFailureCase {
            name: "source tag",
            args: &["--source", "#project"],
            markers: &["--engine native supports DQL queries only"],
        },
        NativeFailureCase {
            name: "source folder",
            args: &["--source", r#""Projects""#],
            markers: &["--engine native supports DQL queries only"],
        },
        NativeFailureCase {
            name: "source file",
            args: &["--source", r#""Projects/Alpha.md""#],
            markers: &["--engine native supports DQL queries only"],
        },
        NativeFailureCase {
            name: "source incoming link",
            args: &["--source", "[[Links/Target]]"],
            markers: &["--engine native supports DQL queries only"],
        },
        NativeFailureCase {
            name: "source algebra",
            args: &["--source", r#"(#project or "Daily") and -"Archive""#],
            markers: &["--engine native supports DQL queries only"],
        },
        NativeFailureCase {
            name: "tag source in DQL",
            args: &["--format", "json", "--query", "LIST FROM #project"],
            markers: &["native query failed", "unsupported token '#'"],
        },
        NativeFailureCase {
            name: "task JSON",
            args: &["--format", "json", "--query", r#"TASK FROM "Tasks""#],
            markers: &[
                "native query failed",
                "native engine supports LIST and limited TABLE queries",
            ],
        },
        NativeFailureCase {
            name: "calendar JSON",
            args: &[
                "--format",
                "json",
                "--query",
                r#"CALENDAR due FROM "Projects""#,
            ],
            markers: &[
                "native query failed",
                "native engine supports LIST and limited TABLE queries",
            ],
        },
        NativeFailureCase {
            name: "grouped paths",
            args: &["--query", r#"LIST FROM "Projects" GROUP BY status"#],
            markers: &["native query failed", "unexpected field name"],
        },
        NativeFailureCase {
            name: "flattened paths",
            args: &[
                "--query",
                r#"TABLE aliases FROM "Projects" FLATTEN aliases"#,
            ],
            markers: &["native query failed", "unexpected field name"],
        },
        NativeFailureCase {
            name: "list markdown",
            args: &[
                "--format",
                "markdown",
                "--query",
                r#"LIST FROM "Projects""#,
            ],
            markers: &["--format markdown requires the Obsidian engine"],
        },
        NativeFailureCase {
            name: "table markdown",
            args: &[
                "--format",
                "markdown",
                "--query",
                r#"TABLE status FROM "Projects""#,
            ],
            markers: &["--format markdown requires the Obsidian engine"],
        },
        NativeFailureCase {
            name: "task markdown",
            args: &["--format", "markdown", "--query", r#"TASK FROM "Tasks""#],
            markers: &["--format markdown requires the Obsidian engine"],
        },
        NativeFailureCase {
            name: "calendar markdown",
            args: &[
                "--format",
                "markdown",
                "--query",
                r#"CALENDAR due FROM "Projects""#,
            ],
            markers: &["--format markdown requires the Obsidian engine"],
        },
        NativeFailureCase {
            name: "origin and this",
            args: &[
                "--origin",
                "Origins/Origin.md",
                "--query",
                r#"LIST FROM "Projects" WHERE owner = this.owner"#,
            ],
            markers: &["native query failed", "expected comparison value"],
        },
    ];

    for case in cases {
        let output = run_native_fixture(case.args);
        assert!(
            !output.status.success(),
            "future native parity case unexpectedly passed: {}\n{}",
            case.name,
            format_output(&output)
        );
        let err = stderr(&output);
        for marker in case.markers {
            assert!(
                err.contains(marker),
                "expected marker `{marker}` for future native parity case `{}`:\n{}",
                case.name,
                format_output(&output)
            );
        }
        assert!(
            stdout(&output).is_empty(),
            "future native parity failures must keep stdout clean for `{}`:\n{}",
            case.name,
            format_output(&output)
        );
    }
}

#[test]
fn dataview_obsidian_source_goldens_cover_source_expression_contract() {
    let cases = [
        (
            "tag",
            &["--source", "#project"][..],
            r##"{"status":"ok","kind":"source_paths","paths":["Projects/Alpha.md","Projects/Beta.md","Projects/Gamma.md","Archive/Old.md"],"warnings":[]}"##,
            "Projects/Alpha.md\nProjects/Beta.md\nProjects/Gamma.md\nArchive/Old.md\n",
        ),
        (
            "folder",
            &["--source", r#""Projects""#],
            r##"{"status":"ok","kind":"source_paths","paths":["Projects/Alpha.md","Projects/Beta.md","Projects/Gamma.md"],"warnings":[]}"##,
            "Projects/Alpha.md\nProjects/Beta.md\nProjects/Gamma.md\n",
        ),
        (
            "file",
            &["--source", r#""Projects/Alpha.md""#],
            r##"{"status":"ok","kind":"source_paths","paths":["Projects/Alpha.md"],"warnings":[]}"##,
            "Projects/Alpha.md\n",
        ),
        (
            "incoming link",
            &["--source", "[[Links/Target]]"],
            r##"{"status":"ok","kind":"source_paths","paths":["Projects/Beta.md","Links/Hub.md"],"warnings":[]}"##,
            "Projects/Beta.md\nLinks/Hub.md\n",
        ),
        (
            "source algebra",
            &["--source", r#"(#project or "Daily") and -"Archive""#],
            r##"{"status":"ok","kind":"source_paths","paths":["Projects/Alpha.md","Projects/Beta.md","Projects/Gamma.md","Daily/2026-06-03.md"],"warnings":[]}"##,
            "Projects/Alpha.md\nProjects/Beta.md\nProjects/Gamma.md\nDaily/2026-06-03.md\n",
        ),
    ];

    for (name, args, payload, expected_stdout) in cases {
        let output = run_obsidian_stub(args, payload);
        assert_success(&output);
        assert_eq!(
            stdout(&output),
            expected_stdout,
            "source-expression golden changed for {name}:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).is_empty(),
            "source-expression golden should keep stderr clean for {name}:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn dataview_obsidian_dql_json_goldens_cover_result_shapes() {
    let output = run_obsidian_stub(
        &[
            "--format",
            "json",
            "--query",
            r#"LIST FROM "Projects" WHERE ready"#,
        ],
        r##"{"status":"ok","kind":"dql_json","result":{"type":"list","values":[{"type":"link","path":"Projects/Alpha.md","display":null,"embed":false},{"type":"link","path":"Projects/Gamma.md","display":null,"embed":false}]},"warnings":[]}"##,
    );
    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    assert_json_stdout_eq(
        &output,
        json!({
            "engine": "obsidian",
            "query_kind": "dql",
            "format": "json",
            "paths": ["Projects/Alpha.md", "Projects/Gamma.md"],
            "result": {
                "type": "list",
                "values": [
                    {
                        "type": "link",
                        "path": "Projects/Alpha.md",
                        "display": null,
                        "embed": false
                    },
                    {
                        "type": "link",
                        "path": "Projects/Gamma.md",
                        "display": null,
                        "embed": false
                    }
                ]
            },
            "warnings": []
        }),
    );

    let output = run_obsidian_stub(
        &[
            "--format",
            "json",
            "--query",
            r#"TABLE status, owner, ready FROM "Projects""#,
        ],
        r##"{"status":"ok","kind":"dql_json","result":{"type":"table","idMeaning":{"type":"path"},"headers":["status","owner","ready"],"values":[[{"type":"link","path":"Projects/Alpha.md","display":null,"embed":false},"active",{"type":"link","path":"People/Ada Lovelace.md","display":"Ada","embed":false},true],[{"type":"link","path":"Projects/Beta.md","display":null,"embed":false},"waiting",{"type":"link","path":"People/Grace Hopper.md","display":null,"embed":false},false],[{"type":"link","path":"Projects/Gamma.md","display":null,"embed":false},null,null,true]]},"warnings":[]}"##,
    );
    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    assert_json_stdout_eq(
        &output,
        json!({
            "engine": "obsidian",
            "query_kind": "dql",
            "format": "json",
            "paths": [
                "Projects/Alpha.md",
                "Projects/Beta.md",
                "Projects/Gamma.md"
            ],
            "result": {
                "type": "table",
                "idMeaning": {"type": "path"},
                "headers": ["status", "owner", "ready"],
                "values": [
                    [
                        {
                            "type": "link",
                            "path": "Projects/Alpha.md",
                            "display": null,
                            "embed": false
                        },
                        "active",
                        {
                            "type": "link",
                            "path": "People/Ada Lovelace.md",
                            "display": "Ada",
                            "embed": false
                        },
                        true
                    ],
                    [
                        {
                            "type": "link",
                            "path": "Projects/Beta.md",
                            "display": null,
                            "embed": false
                        },
                        "waiting",
                        {
                            "type": "link",
                            "path": "People/Grace Hopper.md",
                            "display": null,
                            "embed": false
                        },
                        false
                    ],
                    [
                        {
                            "type": "link",
                            "path": "Projects/Gamma.md",
                            "display": null,
                            "embed": false
                        },
                        null,
                        null,
                        true
                    ]
                ]
            },
            "warnings": []
        }),
    );

    let output = run_obsidian_stub(
        &["--format", "json", "--query", r#"TASK FROM "Tasks""#],
        r##"{"status":"ok","kind":"dql_json","result":{"type":"task","values":[{"text":"Parent task","path":"Tasks/Nested.md","line":8,"completed":false,"children":[{"text":"Completed child","path":"Tasks/Nested.md","line":9,"completed":true}]},{"text":"Sibling task with block id","link":{"type":"link","path":"Tasks/Nested.md#^sibling-task","display":null,"embed":false},"completed":false}]},"warnings":[]}"##,
    );
    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    assert_json_stdout_eq(
        &output,
        json!({
            "engine": "obsidian",
            "query_kind": "dql",
            "format": "json",
            "paths": ["Tasks/Nested.md"],
            "result": {
                "type": "task",
                "values": [
                    {
                        "text": "Parent task",
                        "path": "Tasks/Nested.md",
                        "line": 8,
                        "completed": false,
                        "children": [
                            {
                                "text": "Completed child",
                                "path": "Tasks/Nested.md",
                                "line": 9,
                                "completed": true
                            }
                        ]
                    },
                    {
                        "text": "Sibling task with block id",
                        "link": {
                            "type": "link",
                            "path": "Tasks/Nested.md#^sibling-task",
                            "display": null,
                            "embed": false
                        },
                        "completed": false
                    }
                ]
            },
            "warnings": []
        }),
    );

    let output = run_obsidian_stub(
        &[
            "--format",
            "json",
            "--query",
            r#"CALENDAR due FROM "Projects""#,
        ],
        r##"{"status":"ok","kind":"dql_json","result":{"type":"calendar","values":[{"date":"2026-06-15","link":{"type":"link","path":"Projects/Alpha.md","display":null,"embed":false},"value":"Alpha Project"},{"date":"2026-07-01","link":{"type":"link","path":"Projects/Beta.md","display":null,"embed":false},"value":"Beta Project"}]},"warnings":[]}"##,
    );
    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    assert_json_stdout_eq(
        &output,
        json!({
            "engine": "obsidian",
            "query_kind": "dql",
            "format": "json",
            "paths": ["Projects/Alpha.md", "Projects/Beta.md"],
            "result": {
                "type": "calendar",
                "values": [
                    {
                        "date": "2026-06-15",
                        "link": {
                            "type": "link",
                            "path": "Projects/Alpha.md",
                            "display": null,
                            "embed": false
                        },
                        "value": "Alpha Project"
                    },
                    {
                        "date": "2026-07-01",
                        "link": {
                            "type": "link",
                            "path": "Projects/Beta.md",
                            "display": null,
                            "embed": false
                        },
                        "value": "Beta Project"
                    }
                ]
            },
            "warnings": []
        }),
    );
}

#[test]
fn dataview_obsidian_paths_goldens_cover_grouped_and_flattened_warnings() {
    let output = run_obsidian_stub(
        &["--query", r#"TABLE status FROM "Projects" GROUP BY status"#],
        r##"{"status":"ok","kind":"dql_json","result":{"type":"table","idMeaning":{"type":"group"},"headers":["status"],"values":[["active"],["waiting"]]},"warnings":[]}"##,
    );
    assert_success(&output);
    assert!(
        stdout(&output).is_empty(),
        "grouped paths should not emit synthetic paths:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("DQL table row 1 uses grouped identity")
            && err.contains("DQL table row 2 uses grouped identity"),
        "grouped paths should warn for each row:\n{}",
        format_output(&output)
    );

    let output = run_obsidian_stub(
        &[
            "--query",
            r#"TABLE WITHOUT ID aliases FROM "Projects" FLATTEN aliases"#,
        ],
        r##"{"status":"ok","kind":"dql_json","result":{"type":"table","headers":["aliases"],"values":[[],[]]},"warnings":[]}"##,
    );
    assert_success(&output);
    assert!(
        stdout(&output).is_empty(),
        "flattened paths without identity should not emit paths:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("DQL table row 1 has no source note identity")
            && err.contains("DQL table row 2 has no source note identity"),
        "flattened paths should warn when identity is unavailable:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_obsidian_markdown_goldens_cover_supported_exports() {
    let cases = [
        (
            "list",
            &[
                "--format",
                "markdown",
                "--query",
                r#"LIST FROM "Projects""#,
            ][..],
            r##"{"status":"ok","kind":"markdown","markdown":"- Alpha Project\n- Beta Project\n- Gamma Project\n","warnings":[]}"##,
            "- Alpha Project\n- Beta Project\n- Gamma Project\n",
        ),
        (
            "table",
            &[
                "--format",
                "markdown",
                "--query",
                r#"TABLE status, owner FROM "Projects""#,
            ][..],
            r##"{"status":"ok","kind":"markdown","markdown":"| File | status | owner |\n| ---- | ------ | ----- |\n| Alpha | active | Ada |\n","warnings":[]}"##,
            "| File | status | owner |\n| ---- | ------ | ----- |\n| Alpha | active | Ada |\n",
        ),
        (
            "task",
            &[
                "--format",
                "markdown",
                "--query",
                r#"TASK FROM "Tasks""#,
            ][..],
            r##"{"status":"ok","kind":"markdown","markdown":"- [ ] Parent task\n  - [x] Completed child\n- [ ] Sibling task with block id\n","warnings":[]}"##,
            "- [ ] Parent task\n  - [x] Completed child\n- [ ] Sibling task with block id\n",
        ),
    ];

    for (name, args, payload, expected_stdout) in cases {
        let output = run_obsidian_stub(args, payload);
        assert_success(&output);
        assert_eq!(
            stdout(&output),
            expected_stdout,
            "markdown golden changed for {name}:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).is_empty(),
            "markdown golden should keep stderr clean for {name}:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn dataview_obsidian_calendar_markdown_golden_fails_cleanly() {
    let output = run_obsidian_stub(
        &[
            "--format",
            "markdown",
            "--query",
            r#"CALENDAR due FROM "Projects""#,
        ],
        r##"{"status":"error","code":"DATAVIEW_QUERY_ERROR","message":"Cannot render calendar queries to markdown"}"##,
    );

    assert_eq!(
        output.status.code(),
        Some(1),
        "calendar markdown should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).is_empty(),
        "calendar markdown failure must keep stdout clean:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("Dataview query failed")
            && err.contains("Cannot render calendar queries to markdown"),
        "calendar markdown failure should explain the Dataview error:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_live_obsidian_parity_harness_compares_supported_native_cases() {
    if env::var_os(LIVE_PARITY_ENV).is_none() {
        return;
    }

    let obsidian_vault = env::var(LIVE_PARITY_VAULT_ENV).unwrap_or_else(|_| {
        panic!(
            "{LIVE_PARITY_ENV}=1 requires {LIVE_PARITY_VAULT_ENV} to name an \
             Obsidian vault opened at {}",
            fixture_vault_path().display()
        )
    });

    let cases = [
        LiveCase {
            name: "ready project LIST paths",
            args: &[
                "--strict-paths",
                "--query",
                r#"LIST FROM "Projects" WHERE ready"#,
            ],
        },
        LiveCase {
            name: "project TABLE paths",
            args: &[
                "--strict-paths",
                "--query",
                r#"TABLE status, owner, ready, nullable FROM "Projects""#,
            ],
        },
    ];

    let fixture_vault = fixture_vault_path();
    for case in cases {
        let native =
            run_bob_dataview(&fixture_vault, Some("native"), None, case.args);
        assert_success(&native);

        let obsidian = run_bob_dataview(
            &fixture_vault,
            Some("obsidian"),
            Some(&obsidian_vault),
            case.args,
        );
        assert_success(&obsidian);

        assert_eq!(
            sorted_stdout_lines(&obsidian),
            sorted_stdout_lines(&native),
            "live Obsidian/native parity mismatch for {}:\nObsidian:\n{}\nNative:\n{}",
            case.name,
            format_output(&obsidian),
            format_output(&native)
        );
    }
}

fn run_native_fixture(args: &[&str]) -> Output {
    let temp = TempDir::new("bob-cli-dataview-parity");
    let vault = temp.path().join("vault");
    copy_dir_all(&fixture_vault_path(), &vault)
        .unwrap_or_else(|error| panic!("copy fixture vault: {error}"));
    run_bob_dataview(&vault, Some("native"), None, args)
}

fn run_obsidian_stub(args: &[&str], payload: &str) -> Output {
    let temp = TempDir::new("bob-cli-dataview-obsidian-golden");
    let obsidian = temp.path().join("obsidian");
    write_obsidian_protocol_stub(&obsidian, payload);
    bob_command()
        .arg("dataview")
        .args(args)
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .output()
        .unwrap_or_else(|error| {
            panic!("run bob dataview with Obsidian protocol stub: {error}")
        })
}

fn run_bob_dataview(
    vault: &Path,
    engine: Option<&str>,
    obsidian_vault: Option<&str>,
    args: &[&str],
) -> Output {
    let mut command = bob_command();
    command.arg("dataview").arg("--bob-dir").arg(vault);
    if let Some(engine) = engine {
        command.arg("--engine").arg(engine);
    }
    if let Some(obsidian_vault) = obsidian_vault {
        command.arg("--vault").arg(obsidian_vault);
    } else {
        command.env_remove("BOB_DATAVIEW_VAULT");
    }
    command.args(args).output().unwrap_or_else(|error| {
        panic!("run bob dataview against {}: {error}", vault.display())
    })
}

fn fixture_vault_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/dataview_parity/vault")
}

fn bob_command() -> Command {
    Command::new(BOB_BIN)
}

fn copy_dir_all(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&source, &target)?;
        } else if file_type.is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

fn write_obsidian_protocol_stub(path: &Path, payload: &str) {
    let sentinel_line =
        shell_single_quote(&format!("BOB_DATAVIEW_RESULT\t{payload}"));
    write_executable(
        path,
        &format!("#!/bin/sh\nprintf '%s\\n' {sentinel_line}\n"),
    );
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap_or_else(|error| {
        panic!("write executable stub {}: {error}", path.display())
    });
    set_mode(path, 0o755);
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    let permissions = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, permissions)
        .unwrap_or_else(|error| panic!("chmod {}: {error}", path.display()));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected command success:\n{}",
        format_output(output)
    );
}

fn assert_json_stdout_eq(output: &Output, expected: Value) {
    let actual = json_stdout(output);
    assert_eq!(
        actual,
        expected,
        "JSON stdout changed:\n{}",
        format_output(output)
    );
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!("stdout should be JSON: {error}\n{}", format_output(output))
    })
}

fn dataview_link(path: &str, display: Option<&str>) -> Value {
    json!({
        "type": "link",
        "path": path,
        "display": display,
        "embed": false,
    })
}

fn assert_array_contains(array: &Value, expected: Value) {
    let values = array.as_array().expect("JSON array");
    assert!(
        values.contains(&expected),
        "expected array to contain {expected}: {values:?}"
    );
}

fn sorted_stdout_lines(output: &Output) -> Vec<String> {
    let mut lines = stdout(output)
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    lines.sort();
    lines
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
