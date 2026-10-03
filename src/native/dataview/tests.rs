//! Dataview unit tests: parser surface and path extraction.

use serde_json::json;

use super::*;

#[test]
fn native_source_parser_accepts_phase3_source_surface() {
    assert!(matches!(
        NativeSourceExpr::parse("").expect("empty source parses"),
        NativeSourceExpr::All
    ));

    let source =
        NativeSourceExpr::parse(r#"(#project or "Daily") and -"Archive""#)
            .expect("source algebra parses");
    assert!(matches!(source, NativeSourceExpr::And(_, _)));

    let outgoing = NativeSourceExpr::parse("outgoing([[Links/Hub]])")
        .expect("outgoing source parses");
    assert!(matches!(outgoing, NativeSourceExpr::OutgoingLink(_)));
}

#[test]
fn native_dql_parser_accepts_phase3_command_surface() {
    let query = NativeQuery::parse(
        r#"
TABLE WITHOUT ID owner AS Owner, choice(ready, "yes", "no") AS Readiness
FROM (#project OR "Daily") AND -"Archive"
WHERE ready AND owner = [[People/Ada Lovelace]]
SORT due DESC
GROUP BY status AS Status
FLATTEN aliases AS alias
LIMIT 5
"#,
    )
    .expect("phase 3 DQL surface parses");

    match query.kind {
        NativeQueryKind::Table {
            columns,
            without_id,
        } => {
            assert!(without_id);
            assert_eq!(columns.len(), 2);
            assert_eq!(columns[0].header(), "Owner");
            assert_eq!(columns[1].header(), "Readiness");
        }
        other => panic!("expected table query, got {other:?}"),
    }
    assert_eq!(query.commands.len(), 6);
    assert!(matches!(query.commands[0], NativeDataCommand::From(_)));
    assert!(matches!(query.commands[1], NativeDataCommand::Where(_)));
    assert!(matches!(query.commands[2], NativeDataCommand::Sort { .. }));
    assert!(matches!(
        query.commands[3],
        NativeDataCommand::GroupBy { .. }
    ));
    assert!(matches!(
        query.commands[4],
        NativeDataCommand::Flatten { .. }
    ));
    assert!(matches!(query.commands[5], NativeDataCommand::Limit(5)));
}

#[test]
fn native_dql_parser_reports_representative_invalid_queries() {
    let table = native_error_message(
        NativeQuery::parse("TABLE FROM #project")
            .expect_err("missing table expression should fail"),
    );
    assert!(table.contains("expected DQL expression"), "{table}");

    let source = native_error_message(
        NativeQuery::parse("LIST FROM (#project or")
            .expect_err("unfinished source should fail"),
    );
    assert!(
        source.contains("expected Dataview source expression"),
        "{source}"
    );

    let outgoing = native_error_message(
        NativeSourceExpr::parse(r#"outgoing("Projects")"#)
            .expect_err("outgoing source requires wikilink"),
    );
    assert!(outgoing.contains("expected wikilink"), "{outgoing}");
}

#[test]
fn source_paths_are_normalized_and_deduplicated() {
    let paths = vec![
        "Projects\\Alpha".to_string(),
        "Projects/Alpha.md".to_string(),
        "./Inbox/Waiting.md#Task".to_string(),
    ];

    let extraction = extract_source_paths(&paths, false)
        .expect("source paths should extract");

    assert_eq!(
        extraction.paths,
        vec!["Projects/Alpha.md", "Inbox/Waiting.md"]
    );
    assert!(extraction.warnings.is_empty(), "{extraction:?}");
}

#[test]
fn dql_list_paths_use_list_pair_identity() {
    let result = json!({
        "type": "list",
        "primaryMeaning": { "type": "path" },
        "values": [
            {
                "$widget": "dataview:list-pair",
                "key": { "type": "link", "path": "Projects/Alpha.md", "display": null, "embed": false },
                "value": "active"
            },
            {
                "$widget": "dataview:list-pair",
                "key": { "type": "link", "path": "Projects/Alpha.md", "display": null, "embed": false },
                "value": "duplicate"
            },
            { "type": "link", "path": "Inbox/Waiting", "display": null, "embed": false }
        ]
    });

    let extraction =
        extract_dql_paths(&result, false).expect("list paths should extract");

    assert_eq!(
        extraction.paths,
        vec!["Projects/Alpha.md", "Inbox/Waiting.md"]
    );
    assert!(extraction.warnings.is_empty(), "{extraction:?}");
}

#[test]
fn dql_table_paths_use_first_identity_column() {
    let result = json!({
        "type": "table",
        "idMeaning": { "type": "path" },
        "headers": ["File", "Status"],
        "values": [
            [
                { "type": "link", "path": "Areas\\Odd Name.md", "display": null, "embed": false },
                "active"
            ],
            [
                { "path": "Root Note" },
                "waiting"
            ]
        ]
    });

    let extraction =
        extract_dql_paths(&result, false).expect("table paths should extract");

    assert_eq!(extraction.paths, vec!["Areas/Odd Name.md", "Root Note.md"]);
    assert!(extraction.warnings.is_empty(), "{extraction:?}");
}

#[test]
fn dql_task_paths_resolve_grouped_task_source_notes() {
    let result = json!({
        "type": "task",
        "values": [
            {
                "key": "open",
                "rows": [
                    { "path": "Tasks/Source.md", "text": "first" },
                    { "path": "Tasks/Source.md", "text": "duplicate" },
                    { "link": { "path": "Tasks/Other.md" }, "text": "fallback" }
                ]
            }
        ]
    });

    let extraction =
        extract_dql_paths(&result, false).expect("task paths should extract");

    assert_eq!(extraction.paths, vec!["Tasks/Source.md", "Tasks/Other.md"]);
    assert!(extraction.warnings.is_empty(), "{extraction:?}");
}

#[test]
fn dql_grouped_table_rows_warn_and_fail_when_strict() {
    let result = json!({
        "type": "table",
        "idMeaning": {
            "type": "group",
            "name": "status",
            "on": { "type": "path" }
        },
        "values": [["active", 3], ["waiting", 1]]
    });

    let non_strict = extract_dql_paths(&result, false)
        .expect("grouped paths should be best effort");
    assert!(non_strict.paths.is_empty(), "{non_strict:?}");
    assert_eq!(non_strict.warnings.len(), 2, "{non_strict:?}");

    let strict = extract_dql_paths(&result, true)
        .expect_err("grouped identity should fail in strict mode");
    assert!(
        matches!(strict, DataviewError::StrictPaths { .. }),
        "{strict:?}"
    );
}

#[test]
fn dql_missing_table_identities_warn_per_row() {
    let result = json!({
        "type": "table",
        "idMeaning": { "type": "path" },
        "values": [
            [],
            [{ "path": "Projects/Alpha.md" }, "active"],
            [{}]
        ]
    });

    let extraction = extract_dql_paths(&result, false)
        .expect("non-strict missing identities should warn");

    assert_eq!(extraction.paths, vec!["Projects/Alpha.md"]);
    assert_eq!(extraction.warnings.len(), 2, "{extraction:?}");
    assert!(
        extraction.warnings[0].contains("DQL table row 1")
            && extraction.warnings[1].contains("DQL table row 3"),
        "{extraction:?}"
    );
}

fn native_error_message(error: DataviewError) -> String {
    match error {
        DataviewError::NativeQuery { message } => message,
        other => panic!("expected native query error, got {other:?}"),
    }
}

struct VaultFixture {
    _dir: tempfile::TempDir,
    vault: NativeVault,
}

fn vault_with(files: &[(&str, &str)]) -> VaultFixture {
    let dir = tempfile::tempdir().expect("temp vault");
    for (path, contents) in files {
        let full = dir.path().join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&full, contents).expect("write fixture note");
    }
    let vault =
        NativeVault::read(dir.path(), None).expect("index fixture vault");
    VaultFixture { _dir: dir, vault }
}

fn parse_query(source: &str) -> NativeQuery {
    NativeQuery::parse(source)
        .unwrap_or_else(|error| panic!("parse query:\n{source}\n{error:?}"))
}

fn evaluate_json(vault: &NativeVault, source: &str) -> serde_json::Value {
    let query = parse_query(source);
    vault
        .evaluate(&query)
        .unwrap_or_else(|error| panic!("evaluate {source}: {error:?}"))
        .result
}

fn evaluate_json_limit(
    vault: &NativeVault,
    source: &str,
    limit: usize,
) -> Result<serde_json::Value, DataviewError> {
    let query = parse_query(source);
    vault
        .evaluate_with_flatten_limit(&query, limit)
        .map(|output| output.result)
}

fn table_rows(result: &serde_json::Value) -> Vec<Vec<serde_json::Value>> {
    let drop_identity = result["idMeaning"]["type"] == "path";
    result["values"]
        .as_array()
        .unwrap_or_else(|| panic!("table values: {result}"))
        .iter()
        .map(|row| {
            let mut cells = row
                .as_array()
                .unwrap_or_else(|| panic!("table row: {row}"))
                .clone();
            if drop_identity && !cells.is_empty() {
                cells.remove(0);
            }
            cells
        })
        .collect()
}

fn nested_task_note() -> &'static str {
    r#"# Nested

- [ ] Parent task
  - [x] Completed child
  - [-] Canceled child
- [ ] Sibling task with block id ^sibling-task
"#
}

#[test]
fn flatten_aliases_empty_null_and_scalar_values() {
    let fixture = vault_with(&[
        ("Empty.md", "---\nitems: []\n---\n# Empty\n"),
        ("Null.md", "---\nitems: null\n---\n# Null\n"),
        ("Scalar.md", "---\nitems: 7\n---\n# Scalar\n"),
        ("Values.md", "---\nitems: [a, b]\n---\n# Values\n"),
    ]);

    let empty = evaluate_json(
        &fixture.vault,
        r#"TABLE WITHOUT ID items FROM "Empty.md" FLATTEN items"#,
    );
    assert!(table_rows(&empty).is_empty(), "{empty}");

    let null = evaluate_json(
        &fixture.vault,
        r#"TABLE WITHOUT ID items FROM "Null.md" FLATTEN items AS item"#,
    );
    assert_eq!(table_rows(&null), vec![vec![serde_json::Value::Null]]);

    let scalar = evaluate_json(
        &fixture.vault,
        r#"TABLE WITHOUT ID items FROM "Scalar.md" FLATTEN items"#,
    );
    assert_eq!(table_rows(&scalar), vec![vec![serde_json::json!(7)]]);

    let aliased = evaluate_json(
        &fixture.vault,
        r#"TABLE WITHOUT ID alias FROM "Values.md" FLATTEN items AS alias"#,
    );
    assert_eq!(
        table_rows(&aliased),
        vec![vec![serde_json::json!("a")], vec![serde_json::json!("b")]]
    );
}

#[test]
fn flatten_retains_file_metadata_and_nested_task_children() {
    let fixture = vault_with(&[("Tasks/Nested.md", nested_task_note())]);
    let result = evaluate_json(
        &fixture.vault,
        r#"TABLE WITHOUT ID t.text, t.children.text, file.name, file.tasks.text FROM "Tasks/Nested.md" FLATTEN file.tasks AS t"#,
    );
    let rows = table_rows(&result);
    assert_eq!(rows.len(), 4, "{result}");
    assert!(
        rows.iter().all(|row| row[2] == serde_json::json!("Nested")),
        "{result}"
    );
    assert!(
        rows.iter().all(|row| {
            row[3].as_array().is_some_and(|tasks| tasks.len() == 4)
        }),
        "{result}"
    );
    let parent = rows
        .iter()
        .find(|row| row[0] == serde_json::json!("Parent task"))
        .expect("parent row");
    assert_eq!(
        parent[1],
        serde_json::json!(["Completed child", "Canceled child"])
    );
}

#[test]
fn group_by_preserves_first_seen_order_and_member_access() {
    let fixture = vault_with(&[("Tasks/Nested.md", nested_task_note())]);
    let result = evaluate_json(
        &fixture.vault,
        r#"
TABLE WITHOUT ID Status, length(rows) AS Count, rows.t.status
FROM "Tasks/Nested.md"
FLATTEN file.tasks AS t
GROUP BY t.status AS Status
"#,
    );
    assert_eq!(
        table_rows(&result),
        vec![
            serde_json::json!([" ", 2, [" ", " "]])
                .as_array()
                .unwrap()
                .clone(),
            serde_json::json!(["x", 1, ["x"]])
                .as_array()
                .unwrap()
                .clone(),
            serde_json::json!(["-", 1, ["-"]])
                .as_array()
                .unwrap()
                .clone(),
        ]
    );
}

#[test]
fn grouped_sorting_repeated_grouping_and_lambda_shadowing() {
    let fixture = vault_with(&[("Tasks/Nested.md", nested_task_note())]);
    let sorted = evaluate_json(
        &fixture.vault,
        r#"
TABLE WITHOUT ID Status, length(rows) AS Count
FROM "Tasks/Nested.md"
FLATTEN file.tasks AS t
GROUP BY t.status AS Status
SORT Status ASC
"#,
    );
    assert_eq!(
        table_rows(&sorted),
        vec![
            vec![serde_json::json!(" "), serde_json::json!(2)],
            vec![serde_json::json!("-"), serde_json::json!(1)],
            vec![serde_json::json!("x"), serde_json::json!(1)],
        ]
    );

    let repeated = evaluate_json(
        &fixture.vault,
        r#"
TABLE WITHOUT ID Count, length(rows) AS Groups
FROM "Tasks/Nested.md"
FLATTEN file.tasks AS t
GROUP BY t.status AS Status
GROUP BY length(rows) AS Count
SORT Count ASC
"#,
    );
    assert_eq!(
        table_rows(&repeated),
        vec![
            vec![serde_json::json!(1), serde_json::json!(2)],
            vec![serde_json::json!(2), serde_json::json!(1)],
        ]
    );

    let shadowed = evaluate_json(
        &fixture.vault,
        r#"
TABLE WITHOUT ID Status, length(filter(rows, (t) => t.t.blockId = "sibling-task")) AS Hits
FROM "Tasks/Nested.md"
FLATTEN file.tasks AS t
GROUP BY t.status AS Status
"#,
    );
    let rows = table_rows(&shadowed);
    let space = rows
        .iter()
        .find(|row| row[0] == serde_json::json!(" "))
        .expect("space status group");
    assert_eq!(space[1], serde_json::json!(1), "{shadowed}");
}

#[test]
fn flatten_alias_and_collection_builtins_do_not_mutate_siblings() {
    let fixture = vault_with(&[(
        "Note.md",
        "---\nitems: [keep, mutate]\n---\n# Note\n- [ ] one\n- [ ] two\n",
    )]);

    let flattened = evaluate_json(
        &fixture.vault,
        r#"TABLE WITHOUT ID length(file.tasks), length(items), t.text FROM "Note.md" FLATTEN file.tasks AS t"#,
    );
    let rows = table_rows(&flattened);
    assert_eq!(rows.len(), 2, "{flattened}");
    assert!(
        rows.iter().all(|row| {
            row[0] == serde_json::json!(2) && row[1] == serde_json::json!(2)
        }),
        "{flattened}"
    );

    let reversed = evaluate_json(
        &fixture.vault,
        r#"TABLE WITHOUT ID length(file.tasks), reverse(file.tasks).text, file.tasks.text FROM "Note.md""#,
    );
    let row = &table_rows(&reversed)[0];
    assert_eq!(row[0], serde_json::json!(2));
    assert_eq!(row[1], serde_json::json!(["two", "one"]));
    assert_eq!(row[2], serde_json::json!(["one", "two"]));

    let added = evaluate_json(
        &fixture.vault,
        r#"TABLE WITHOUT ID items + list("extra"), items FROM "Note.md""#,
    );
    let row = &table_rows(&added)[0];
    assert_eq!(row[0], serde_json::json!(["keep", "mutate", "extra"]));
    assert_eq!(row[1], serde_json::json!(["keep", "mutate"]));

    let shared = DataviewValue::array(vec![
        DataviewValue::String("keep".into()),
        DataviewValue::String("mutate".into()),
    ]);
    let cloned = shared.clone();
    let mut items = shared.into_vec().expect("array into_vec");
    items.push(DataviewValue::String("extra".into()));
    assert_eq!(
        cloned.as_array().map(<[DataviewValue]>::len),
        Some(2),
        "into_vec must copy a shared array before mutation"
    );
}

#[test]
fn flatten_lists_include_nested_children() {
    let fixture = vault_with(&[(
        "Lists.md",
        "# Lists\n\n- parent\n  - child a\n  - child b\n- sibling\n",
    )]);
    let result = evaluate_json(
        &fixture.vault,
        r#"
TABLE WITHOUT ID Kind, length(rows) AS Count
FROM "Lists.md"
FLATTEN file.lists AS item
GROUP BY choice(typeof(item.parent) = "number", "child", "top") AS Kind
SORT Kind ASC
"#,
    );
    assert_eq!(
        table_rows(&result),
        vec![
            vec![serde_json::json!("child"), serde_json::json!(2)],
            vec![serde_json::json!("top"), serde_json::json!(2)],
        ]
    );
}

#[test]
fn flatten_budget_accepts_exact_limit_and_rejects_overflow() {
    assert_eq!(checked_flatten_len(0, 3, 3), Ok(3));
    assert_eq!(
        checked_flatten_len(2, 2, 3),
        Err(FlattenOverflow::Limit { attempted: 4 })
    );
    assert_eq!(
        checked_flatten_len(usize::MAX, 1, usize::MAX),
        Err(FlattenOverflow::CheckedAdd)
    );

    let fixture = vault_with(&[(
        "Budget.md",
        "---\nleft: [1, 2]\nright: [a, b, c]\n---\n# Budget\n",
    )]);

    let exact = evaluate_json_limit(
        &fixture.vault,
        r#"TABLE WITHOUT ID left FROM "Budget.md" FLATTEN left AS left"#,
        2,
    )
    .expect("exact limit succeeds");
    assert_eq!(table_rows(&exact).len(), 2, "{exact}");

    let overflow = evaluate_json_limit(
        &fixture.vault,
        r#"TABLE WITHOUT ID left FROM "Budget.md" FLATTEN left AS t"#,
        1,
    )
    .expect_err("one over the limit fails");
    let message = native_error_message(overflow);
    assert!(
        message
            .contains("native FLATTEN left AS t would exceed the 1-row limit"),
        "{message}"
    );
    assert!(message.contains("at least 2 rows"), "{message}");
    assert!(message.contains("DQL TASK query"), "{message}");

    let cartesian_ok = evaluate_json_limit(
        &fixture.vault,
        r#"TABLE WITHOUT ID left, right FROM "Budget.md" FLATTEN left AS left FLATTEN right AS right"#,
        6,
    )
    .expect("repeated expansion at limit succeeds");
    assert_eq!(table_rows(&cartesian_ok).len(), 6, "{cartesian_ok}");

    let cartesian_over = evaluate_json_limit(
        &fixture.vault,
        r#"TABLE WITHOUT ID left, right FROM "Budget.md" FLATTEN left AS left FLATTEN right AS right"#,
        5,
    )
    .expect_err("repeated expansion over the limit fails");
    let message = native_error_message(cartesian_over);
    assert!(message.contains("FLATTEN right AS right"), "{message}");
    assert!(message.contains("at least 6 rows"), "{message}");
}

#[test]
fn flatten_budget_honors_earlier_filters_not_later_limits() {
    let fixture = vault_with(&[(
        "Budget.md",
        "---\nitems: [1, 2, 3]\nkeep: false\n---\n# Budget\n",
    )]);

    let filtered = evaluate_json_limit(
        &fixture.vault,
        r#"TABLE WITHOUT ID items FROM "Budget.md" WHERE keep FLATTEN items"#,
        1,
    )
    .expect("WHERE before FLATTEN can drop the page");
    assert!(table_rows(&filtered).is_empty(), "{filtered}");

    let later_limit = evaluate_json_limit(
        &fixture.vault,
        r#"TABLE WITHOUT ID items FROM "Budget.md" FLATTEN items LIMIT 1"#,
        2,
    )
    .expect_err("LIMIT after FLATTEN cannot hide overflow");
    let message = native_error_message(later_limit);
    assert!(
        message.contains("would exceed the 2-row limit"),
        "{message}"
    );
}
