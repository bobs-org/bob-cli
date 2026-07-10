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
        "ai_ref.md",
        "ref.md",
        "ref/Alpha.md",
        "ref/Beta.md",
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
                "idMeaning": {"type": "path"},
                "headers": ["status", "owner", "ready", "nullable", "missing"],
                "values": [
                    [
                        dataview_link("Projects/Alpha.md", None),
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
                        dataview_link("Projects/Beta.md", None),
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
                    [
                        dataview_link("Projects/Gamma.md", None),
                        null,
                        null,
                        true,
                        null,
                        null
                    ]
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

    assert_eq!(values[0][0], dataview_link("Projects/Alpha.md", None));
    assert_eq!(values[0][1], json!(["Alpha", "Project Alpha"]));
    assert_eq!(values[0][2], 8);
    assert_eq!(values[0][3], "2026-06-15");
    assert_eq!(values[0][4], "2026-06-01T09:30:00");
    assert_eq!(values[0][5], "PT2H");
    assert_eq!(values[0][6], "active");
    assert_eq!(values[0][7], 42);
    assert_eq!(values[0][8], dataview_link("People/Grace Hopper.md", None));
    assert_eq!(
        values[0][9],
        json!([dataview_link("Projects/Beta.md", None)])
    );

    assert_eq!(values[1][1], json!(["Beta Project"]));
    assert_eq!(values[1][3], "2026-07-01");
    assert_eq!(values[1][5], "P3D");
    assert!(values[2][3].is_null(), "blank YAML value should be null");
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

    assert_eq!(alpha[0], dataview_link("Projects/Alpha.md", None));
    assert_eq!(alpha[1], "Alpha");
    assert_eq!(alpha[2], "Projects");
    assert_eq!(alpha[3], "Projects/Alpha.md");
    assert_eq!(alpha[4], dataview_link("Projects/Alpha.md", None));
    assert_eq!(
        alpha[5],
        json!(["#project", "#project/active", "#task", "#task/project"])
    );
    assert_eq!(
        alpha[6],
        json!(["#project", "#project/active", "#task/project"])
    );
    assert_eq!(alpha[7], json!(["Alpha", "Project Alpha"]));
    assert_array_contains(&alpha[8], dataview_link("Links/Hub.md", None));
    assert_array_contains(&alpha[9], dataview_link("Origins/Origin.md", None));
    assert!(
        alpha[10].is_null(),
        "project notes should not have file.day"
    );
    assert_eq!(alpha[11], false);

    let daily = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"TABLE file.day, file.tags FROM "Daily""#,
    ]);
    assert_success(&daily);
    let daily_json = json_stdout(&daily);
    assert_eq!(daily_json["paths"], json!(["Daily/2026-06-03.md"]));
    assert_eq!(daily_json["result"]["values"][0][1], "2026-06-03");
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
    let tasks = row[1].as_array().expect("file.tasks");
    let lists = row[2].as_array().expect("file.lists");

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
fn dataview_native_source_expressions_match_fixture_goldens() {
    let cases = [
        (
            "tag",
            &["--source", "#project"][..],
            "Projects/Alpha.md\nProjects/Beta.md\nProjects/Gamma.md\nArchive/Old.md\n",
        ),
        (
            "folder",
            &["--source", r#""Projects""#],
            "Projects/Alpha.md\nProjects/Beta.md\nProjects/Gamma.md\n",
        ),
        (
            "file",
            &["--source", r#""Projects/Alpha.md""#],
            "Projects/Alpha.md\n",
        ),
        (
            "overlapping folder beats file",
            &["--source", r#""ref""#],
            "ref/Alpha.md\nref/Beta.md\n",
        ),
        (
            "overlapping file remains addressable",
            &["--source", r#""ref.md""#],
            "ref.md\n",
        ),
        (
            "incoming link",
            &["--source", "[[Links/Target]]"],
            "Projects/Beta.md\nLinks/Hub.md\n",
        ),
        (
            "outgoing link",
            &["--source", "outgoing([[Links/Hub]])"],
            "Projects/Alpha.md\nProjects/Beta.md\nLinks/Target.md\n",
        ),
        (
            "source algebra",
            &["--source", r#"(#project or "Daily") and -"Archive""#],
            "Projects/Alpha.md\nProjects/Beta.md\nProjects/Gamma.md\nDaily/2026-06-03.md\n",
        ),
    ];

    for (name, args, expected_stdout) in cases {
        let output = run_native_fixture(args);
        assert_success(&output);
        assert_eq!(
            stdout(&output),
            expected_stdout,
            "native source expression changed for {name}:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).is_empty(),
            "native source expression should keep stderr clean for {name}:\n{}",
            format_output(&output)
        );
    }

    let json =
        run_native_fixture(&["--format", "json", "--source", "#project"]);
    assert_success(&json);
    assert_json_stdout_eq(
        &json,
        json!({
            "engine": "native",
            "query_kind": "source",
            "format": "json",
            "paths": [
                "Projects/Alpha.md",
                "Projects/Beta.md",
                "Projects/Gamma.md",
                "Archive/Old.md"
            ],
            "warnings": []
        }),
    );
}

#[test]
fn dataview_native_dql_from_ref_prefers_folder_when_note_also_exists() {
    let output = run_native_fixture(&[
        "--format",
        "markdown",
        "--query",
        r#"LIST WITHOUT ID title + " (" + url + ")"
FROM "ref"
WHERE
  source_path AND url AND (
    parent = [[ai_ref]]
    OR parent.parent = [[ai_ref]]
    OR parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent.parent = [[ai_ref]]
  )
SORT title"#,
    ]);

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        concat!(
            "- Alpha Reference (https://example.test/alpha-reference)\n",
            "- Beta Reference (https://example.test/beta-reference)\n",
        ),
        "native DQL FROM \"ref\" should select folder rows, not ref.md:\n{}",
        format_output(&output)
    );
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
}

#[test]
fn dataview_native_source_smoke_handles_generated_vault_with_many_lists() {
    let temp = TempDir::new("bob-cli-dataview-native-source-smoke");
    let vault = temp.path().join("vault");
    let ref_dir = vault.join("ref");
    fs::create_dir_all(&ref_dir)
        .unwrap_or_else(|error| panic!("create smoke ref dir: {error}"));

    for page_index in 0..120 {
        let mut contents = format!(
            concat!(
                "---\n",
                "tags: [project, project/smoke]\n",
                "source_pdf: true\n",
                "parent: [[ai_ref]]\n",
                "aliases: [Smoke {page_index}]\n",
                "---\n\n",
                "# Smoke {page_index}\n",
                "status-inline:: active\n",
                "Links to [[hub]] and #project/smoke.\n",
            ),
            page_index = page_index,
        );
        for task_index in 0..40 {
            contents.push_str(&format!(
                "- [ ] Task {page_index}-{task_index} #task [due:: 2026-06-03]\n"
            ));
        }
        fs::write(ref_dir.join(format!("smoke-{page_index:03}.md")), contents)
            .unwrap_or_else(|error| panic!("write smoke note: {error}"));
    }

    let folder = run_bob_dataview(
        &vault,
        Some("native"),
        None,
        &["--source", r#""ref""#],
    );
    assert_success(&folder);
    assert_eq!(
        stdout(&folder).lines().count(),
        120,
        "folder source smoke should return generated ref notes:\n{}",
        format_output(&folder)
    );
    assert!(stderr(&folder).is_empty(), "{}", format_output(&folder));

    let tag = run_bob_dataview(
        &vault,
        Some("native"),
        None,
        &["--source", "#project"],
    );
    assert_success(&tag);
    assert_eq!(
        stdout(&tag).lines().count(),
        120,
        "tag source smoke should return generated project notes:\n{}",
        format_output(&tag)
    );
    assert!(stderr(&tag).is_empty(), "{}", format_output(&tag));
}

#[test]
fn dataview_native_dql_from_accepts_source_expressions() {
    let output = run_native_fixture(&[
        "--strict-paths",
        "--query",
        r#"LIST FROM (#project or "Daily") AND -"Archive""#,
    ]);

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "Projects/Alpha.md\nProjects/Beta.md\nProjects/Gamma.md\nDaily/2026-06-03.md\n",
        "native DQL FROM source expression changed:\n{}",
        format_output(&output)
    );
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
}

#[test]
fn dataview_native_expression_core_evaluates_table_and_list_values() {
    let table = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"TABLE score + metrics.points AS total, [status, ready] AS pair, {name: file.name, missing: missing.child} AS obj, nullable = null AS is_null FROM "Projects/Alpha.md""#,
    ]);

    assert_success(&table);
    assert!(stderr(&table).is_empty(), "{}", format_output(&table));
    assert_json_stdout_eq(
        &table,
        json!({
            "engine": "native",
            "query_kind": "dql",
            "format": "json",
            "paths": ["Projects/Alpha.md"],
            "result": {
                "type": "table",
                "idMeaning": {"type": "path"},
                "headers": ["total", "pair", "obj", "is_null"],
                "values": [[
                    dataview_link("Projects/Alpha.md", None),
                    15,
                    ["active", true],
                    {"missing": null, "name": "Alpha"},
                    true
                ]]
            },
            "warnings": []
        }),
    );

    let list = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"LIST status + ":" + file.name FROM "Projects/Alpha.md""#,
    ]);

    assert_success(&list);
    assert!(stderr(&list).is_empty(), "{}", format_output(&list));
    assert_json_stdout_eq(
        &list,
        json!({
            "engine": "native",
            "query_kind": "dql",
            "format": "json",
            "paths": ["Projects/Alpha.md"],
            "result": {
                "type": "list",
                "primaryMeaning": {"type": "path"},
                "values": [{
                    "$widget": "dataview:list-pair",
                    "key": dataview_link("Projects/Alpha.md", None),
                    "value": "active:Alpha"
                }]
            },
            "warnings": []
        }),
    );
}

#[test]
fn dataview_native_expression_core_supports_this_comparison_and_sorting() {
    let origin = run_native_fixture(&[
        "--origin",
        "Origins/Origin.md",
        "--query",
        r#"LIST FROM "Projects" WHERE owner = this.owner"#,
    ]);

    assert_success(&origin);
    assert_eq!(
        stdout(&origin),
        "Projects/Alpha.md\n",
        "native this/origin comparison changed:\n{}",
        format_output(&origin)
    );
    assert!(stderr(&origin).is_empty(), "{}", format_output(&origin));

    let sorted = run_native_fixture(&[
        "--query",
        r#"LIST FROM "Projects" WHERE due >= "2026-07-01" SORT due DESC"#,
    ]);

    assert_success(&sorted);
    assert_eq!(
        stdout(&sorted),
        "Projects/Beta.md\n",
        "native ordering comparison or SORT changed:\n{}",
        format_output(&sorted)
    );
    assert!(stderr(&sorted).is_empty(), "{}", format_output(&sorted));
}

#[test]
fn dataview_native_expression_core_supports_swizzling_and_lambdas() {
    let output = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"TABLE file.tasks.text AS texts, filter(file.tasks, (t) => !t.completed).text AS open, map(file.tasks, (t) => t.completed) AS done, any(file.tasks, (t) => !t.completed) AS has_open, all(file.tasks, (t) => t.completed) AS all_done, minby(file.tasks, (t) => t.line).text AS first_task FROM "Projects/Alpha.md""#,
    ]);

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    assert_json_stdout_eq(
        &output,
        json!({
            "engine": "native",
            "query_kind": "dql",
            "format": "json",
            "paths": ["Projects/Alpha.md"],
            "result": {
                "type": "table",
                "idMeaning": {"type": "path"},
                "headers": ["texts", "open", "done", "has_open", "all_done", "first_task"],
                "values": [[
                    dataview_link("Projects/Alpha.md", None),
                    [
                        "Kickoff #task/project",
                        "Prepare brief",
                        "Review with [[People/Ada Lovelace]]"
                    ],
                    [
                        "Kickoff #task/project",
                        "Review with [[People/Ada Lovelace]]"
                    ],
                    [false, true, false],
                    true,
                    false,
                    "Kickoff #task/project"
                ]]
            },
            "warnings": []
        }),
    );
}

#[test]
fn dataview_native_function_library_supports_constructors_and_utilities() {
    let output = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"TABLE object("name", file.name).name AS obj, list(1,2,3) AS listed, date("2026-06-15") = due AS parsed_due, dur("2 hours") = estimate AS parsed_dur, number("score: 9") AS parsed_num, string(ready) AS ready_text, typeof(owner) AS owner_type, link("People/Ada Lovelace", "Ada") = owner AS link_eq, meta([[Projects/Alpha#Next Actions]]).subpath AS subpath, meta(embed(link("Projects/Alpha"))).embed AS embedded, default(missing, "fallback") AS fallback, choice(ready, "yes", "no") AS chosen, striptime(started) AS started_day, dateformat(due, "yyyy-MM-dd") AS due_text, durationformat(estimate, "h") AS estimate_hours, currencyformat(budget, "USD") AS budget_text, typeof(hash("seed", file.name)) AS hash_type FROM "Projects/Alpha.md""#,
    ]);

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json = json_stdout(&output);
    assert_eq!(json["paths"], json!(["Projects/Alpha.md"]));
    assert_eq!(
        json["result"]["values"][0],
        json!([
            dataview_link("Projects/Alpha.md", None),
            "Alpha",
            [1, 2, 3],
            true,
            true,
            9,
            "true",
            "link",
            true,
            "Next Actions",
            true,
            "fallback",
            "yes",
            "2026-06-01",
            "2026-06-15",
            "2",
            "$42.00",
            "number"
        ])
    );
}

#[test]
fn dataview_native_function_library_supports_numeric_and_container_functions() {
    let output = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r##"TABLE round(16.555, 2) AS rounded, trunc(-93.333) AS truncated, floor(-0.837) AS floored, ceil(12.1) AS ceiled, min([3,1,2]) AS minv, max(1,2,3) AS maxv, sum([1,2,3]) AS summed, product([1,2,3]) AS multiplied, reduce([100,20,3], "-") AS reduced, average([1,2,3]) AS averaged, contains(file, "ctime") AS has_ctime, contains(file.tags, "#project") AS has_project, icontains(status, "ACT") AS active_ci, econtains(aliases, "Alpha") AS exact_alias, containsword("Hello there chaps!", "chaps") AS word, extract(file, "name", "path").name AS extracted, sort([3,1,2]) AS sorted, reverse(["a","b"]) AS reversed, length(file.aliases) AS alias_count, nonnull([null,false]) AS nonnulls, firstvalue([null,owner]) AS first, all(true, ready) AS all_ready, any(false, ready) AS any_ready, none([false,false]) AS none_true, join(["a","b"], ":") AS joined, unique([1,3,1]) AS uniquev, flat([1,[2,[3]]], 2) AS flattened, slice([1,2,3,4], -2) AS sliced FROM "Projects/Alpha.md""##,
    ]);

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json = json_stdout(&output);
    assert_eq!(
        json["result"]["values"][0],
        json!([
            dataview_link("Projects/Alpha.md", None),
            16.56,
            -93,
            -1,
            13,
            1,
            3,
            6,
            6,
            77,
            2,
            true,
            true,
            true,
            true,
            true,
            "Alpha",
            [1, 2, 3],
            ["b", "a"],
            2,
            [false],
            dataview_link("People/Ada Lovelace.md", Some("Ada")),
            true,
            true,
            true,
            "a:b",
            [1, 3],
            [1, 2, 3],
            [3, 4]
        ])
    );
}

#[test]
fn dataview_native_function_library_supports_string_functions() {
    let output = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"TABLE regextest("\\w+", file.name) AS regex_test, regexmatch("Alpha", file.name) AS regex_match, regexreplace("Suite 1000", "\\d+", "-") AS regex_replaced, replace("what", "wh", "h") AS replaced, lower(priority) AS lowered, upper(status) AS uppered, split("hello world", " ") AS splitv, startswith(file.path, "Projects/") AS starts, endswith(file.path, "Alpha.md") AS ends, padleft("yes", 5, "!") AS leftpadded, padright("yes", 5, "!") AS rightpadded, substring("hello", 2) AS substr, truncate("Hello there!", 8) AS truncated FROM "Projects/Alpha.md""#,
    ]);

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json = json_stdout(&output);
    assert_eq!(
        json["result"]["values"][0],
        json!([
            dataview_link("Projects/Alpha.md", None),
            true,
            true,
            "Suite -",
            "hat",
            "high",
            "ACTIVE",
            ["hello", "world"],
            true,
            true,
            "!!yes",
            "yes!!",
            "llo",
            "Hello..."
        ])
    );
}

#[test]
fn dataview_native_function_library_works_in_where_sort_and_list() {
    let output = run_native_fixture(&[
        "--query",
        r##"LIST FROM "Projects" WHERE contains(file.tags, "#project") AND default(status, "missing") != "waiting" SORT lower(file.name) DESC LIMIT 2"##,
    ]);

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "Projects/Gamma.md\nProjects/Alpha.md\n",
        "native function WHERE/SORT paths changed:\n{}",
        format_output(&output)
    );
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));

    let list = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"LIST join(sort(file.aliases), "|") FROM "Projects/Alpha.md""#,
    ]);
    assert_success(&list);
    assert!(stderr(&list).is_empty(), "{}", format_output(&list));
    let json = json_stdout(&list);
    assert_eq!(
        json["result"]["values"],
        json!([{
            "$widget": "dataview:list-pair",
            "key": dataview_link("Projects/Alpha.md", None),
            "value": "Alpha|Project Alpha"
        }])
    );
}

#[test]
fn dataview_native_dql_execution_supports_phase6_result_shapes() {
    let task = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"TASK FROM "Tasks""#,
    ]);
    assert_success(&task);
    assert!(stderr(&task).is_empty(), "{}", format_output(&task));
    let task_json = json_stdout(&task);
    assert_eq!(task_json["paths"], json!(["Tasks/Nested.md"]));
    assert_eq!(task_json["result"]["type"], "task");
    let tasks = task_json["result"]["values"]
        .as_array()
        .expect("task values");
    assert_eq!(tasks.len(), 2, "TASK should emit top-level tasks");
    assert_eq!(tasks[0]["text"], "Parent task #project");
    assert_eq!(
        tasks[0]["children"]
            .as_array()
            .expect("task children")
            .len(),
        2
    );
    assert_eq!(tasks[1]["blockId"], "sibling-task");

    let calendar = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"CALENDAR due FROM "Projects""#,
    ]);
    assert_success(&calendar);
    assert!(stderr(&calendar).is_empty(), "{}", format_output(&calendar));
    assert_json_stdout_eq(
        &calendar,
        json!({
            "engine": "native",
            "query_kind": "dql",
            "format": "json",
            "paths": ["Projects/Alpha.md", "Projects/Beta.md"],
            "result": {
                "type": "calendar",
                "values": [
                    {
                        "date": "2026-06-15",
                        "link": dataview_link("Projects/Alpha.md", None),
                        "value": "Alpha"
                    },
                    {
                        "date": "2026-07-01",
                        "link": dataview_link("Projects/Beta.md", None),
                        "value": "Beta"
                    }
                ]
            },
            "warnings": []
        }),
    );

    let grouped = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"TABLE status FROM "Projects" GROUP BY status"#,
    ]);
    assert_success(&grouped);
    let grouped_json = json_stdout(&grouped);
    assert_eq!(grouped_json["paths"], json!([]));
    assert_eq!(
        grouped_json["result"],
        json!({
            "type": "table",
            "idMeaning": {"type": "group"},
            "headers": ["status"],
            "values": [["active"], ["waiting"], [null]]
        })
    );
    assert!(
        stderr(&grouped).contains("DQL table row 1 uses grouped identity"),
        "{}",
        format_output(&grouped)
    );

    let grouped_strict = run_native_fixture(&[
        "--strict-paths",
        "--query",
        r#"TABLE status FROM "Projects" GROUP BY status"#,
    ]);
    assert!(
        !grouped_strict.status.success(),
        "strict grouped paths should fail:\n{}",
        format_output(&grouped_strict)
    );
    assert!(
        stdout(&grouped_strict).is_empty(),
        "{}",
        format_output(&grouped_strict)
    );
    assert!(
        stderr(&grouped_strict)
            .contains("paths output could not derive clean note paths"),
        "{}",
        format_output(&grouped_strict)
    );

    let flattened = run_native_fixture(&[
        "--format",
        "json",
        "--query",
        r#"TABLE aliases FROM "Projects" FLATTEN aliases"#,
    ]);
    assert_success(&flattened);
    assert!(
        stderr(&flattened).is_empty(),
        "{}",
        format_output(&flattened)
    );
    let flattened_json = json_stdout(&flattened);
    assert_eq!(
        flattened_json["paths"],
        json!(["Projects/Alpha.md", "Projects/Beta.md", "Projects/Gamma.md"])
    );
    assert_eq!(
        flattened_json["result"]["values"],
        json!([
            [dataview_link("Projects/Alpha.md", None), "Alpha"],
            [dataview_link("Projects/Alpha.md", None), "Project Alpha"],
            [dataview_link("Projects/Beta.md", None), "Beta Project"],
            [dataview_link("Projects/Gamma.md", None), null],
        ])
    );

    let flattened_paths = run_native_fixture(&[
        "--strict-paths",
        "--query",
        r#"TABLE aliases FROM "Projects" FLATTEN aliases"#,
    ]);
    assert_success(&flattened_paths);
    assert_eq!(
        stdout(&flattened_paths),
        "Projects/Alpha.md\nProjects/Beta.md\nProjects/Gamma.md\n"
    );
    assert!(
        stderr(&flattened_paths).is_empty(),
        "{}",
        format_output(&flattened_paths)
    );
}

#[test]
fn dataview_native_markdown_goldens_cover_supported_exports() {
    let list = run_native_fixture(&[
        "--format",
        "markdown",
        "--query",
        r#"LIST FROM "Projects""#,
    ]);
    assert_success(&list);
    assert_eq!(
        stdout(&list),
        concat!(
            "- [[Projects/Alpha.md|Alpha]]\n",
            "- [[Projects/Beta.md|Beta]]\n",
            "- [[Projects/Gamma.md|Gamma]]\n",
        ),
        "native list markdown changed:\n{}",
        format_output(&list)
    );
    assert!(stderr(&list).is_empty(), "{}", format_output(&list));

    let table = run_native_fixture(&[
        "--format",
        "markdown",
        "--query",
        r#"TABLE status, owner FROM "Projects""#,
    ]);
    assert_success(&table);
    assert_eq!(
        stdout(&table),
        concat!(
            "| File                         | status  | owner                                    |\n",
            "| ---------------------------- | ------- | ---------------------------------------- |\n",
            "| [[Projects/Alpha.md\\|Alpha]] | active  | [[People/Ada Lovelace.md\\|Ada]]          |\n",
            "| [[Projects/Beta.md\\|Beta]]   | waiting | [[People/Grace Hopper.md\\|Grace Hopper]] |\n",
            "| [[Projects/Gamma.md\\|Gamma]] | -       | -                                        |\n",
        ),
        "native table markdown changed:\n{}",
        format_output(&table)
    );
    assert!(stderr(&table).is_empty(), "{}", format_output(&table));

    let task = run_native_fixture(&[
        "--format",
        "markdown",
        "--query",
        r#"TASK FROM "Tasks""#,
    ]);
    assert_success(&task);
    assert_eq!(
        stdout(&task),
        concat!(
            "- [ ] Parent task #project\n",
            "  - [x] Completed child\n",
            "  - [-] Canceled child\n",
            "- [ ] Sibling task with block id\n",
        ),
        "native task markdown changed:\n{}",
        format_output(&task)
    );
    assert!(stderr(&task).is_empty(), "{}", format_output(&task));
}

#[test]
fn dataview_native_calendar_markdown_fails_cleanly() {
    let output = run_native_fixture(&[
        "--format",
        "markdown",
        "--query",
        r#"CALENDAR due FROM "Projects""#,
    ]);

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
        .arg("query")
        .arg("--engine")
        .arg("obsidian")
        .args(args)
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .output()
        .unwrap_or_else(|error| {
            panic!("run bob query with Obsidian protocol stub: {error}")
        })
}

fn run_bob_dataview(
    vault: &Path,
    engine: Option<&str>,
    obsidian_vault: Option<&str>,
    args: &[&str],
) -> Output {
    let mut command = bob_command();
    command.arg("query").arg("--bob-dir").arg(vault);
    if let Some(engine) = engine {
        command.arg("--engine").arg(engine);
    }
    if let Some(obsidian_vault) = obsidian_vault {
        command.arg("--vault").arg(obsidian_vault);
    } else {
        command.env_remove("BOB_DATAVIEW_VAULT");
    }
    command.args(args).output().unwrap_or_else(|error| {
        panic!("run bob query against {}: {error}", vault.display())
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
