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
