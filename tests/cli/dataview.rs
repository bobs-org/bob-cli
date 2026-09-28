//! Dataview behavior tests.

use crate::support::*;
use std::ffi::OsString;
use std::fs;
use std::path::Path;

#[test]
fn dataview_short_options_are_accepted() {
    let temp = TempDir::new("bob-cli-dataview-short-options");
    let vault = temp.path().join("vault");
    let query_file = temp.path().join("projects.dql");
    let obsidian = temp.path().join("obsidian");
    let log = temp.path().join("commands.log");

    write_file(&vault.join("Home.md"), "---\n---\n");
    write_file(&vault.join("Projects/Alpha.md"), "# Alpha\n#project\n");
    write_file(&query_file, "LIST FROM #project");

    let output = bob_command()
        .arg("query")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("-o")
        .arg("Home.md")
        .arg("-Q")
        .arg(&query_file)
        .output()
        .expect("run bob query with short query-file options");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["format"], "json");
    assert_eq!(json["paths"][0], "Projects/Alpha.md");

    let output = bob_command()
        .arg("query")
        .arg("-b")
        .arg(&vault)
        .arg("-S")
        .arg("-q")
        .arg("LIST FROM #project")
        .output()
        .expect("run bob query with short strict-paths/query options");

    assert_success(&output);
    assert_eq!(stdout(&output), "Projects/Alpha.md\n");

    write_obsidian_success_stub(
        &obsidian,
        r##"{"status":"ok","kind":"source_paths","paths":["Projects/Alpha.md"],"warnings":[]}"##,
    );
    let output = bob_command()
        .arg("query")
        .arg("-e")
        .arg("obsidian")
        .arg("-s")
        .arg("#project")
        .arg("-v")
        .arg("Bob")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("STUB_LOG", &log)
        .output()
        .expect("run bob query with short obsidian options");

    assert_success(&output);
    assert_eq!(stdout(&output), "Projects/Alpha.md\n");
    let log_text = fs::read_to_string(&log).expect("read obsidian argv log");
    assert!(log_text.contains("ARG:vault=Bob"), "{log_text}");
}

#[test]
fn dataview_rejects_invalid_argument_combinations() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["query", "--source", "#project", "--query", "LIST"],
            "cannot be used with",
        ),
        (
            &["query", "--source", "#project", "--format", "markdown"],
            "--format markdown requires a DQL query",
        ),
        (
            &["query", "--vault", "Bob", "--query", "LIST FROM #project"],
            "--vault can only be used with --engine obsidian",
        ),
        (
            &[
                "query",
                "--query",
                "LIST FROM #project",
                "--format",
                "json",
                "--strict-paths",
            ],
            "--strict-paths can only be used with --format paths",
        ),
    ];

    for (args, marker) in cases {
        let output = bob_command()
            .args(*args)
            .output()
            .unwrap_or_else(|error| panic!("run bob {args:?}: {error}"));

        assert_eq!(
            output.status.code(),
            Some(2),
            "invalid query args should fail with usage:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(marker),
            "expected `{marker}` in query validation error:\n{}",
            format_output(&output)
        );
        assert!(
            !stderr(&output).contains("engine execution is not implemented"),
            "validation must fail before execution:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn dataview_obsidian_source_uses_path_command_and_sentinel_protocol() {
    let temp = TempDir::new("bob-cli-dataview-source");
    let stub_bin = temp.path().join("bin");
    let log = temp.path().join("commands.log");
    fs::create_dir_all(&stub_bin).expect("create stub bin");

    write_obsidian_success_stub(
        &stub_bin.join("obsidian"),
        r##"{"status":"ok","kind":"source_paths","paths":["Projects/alpha.md","Inbox/waiting.md"],"warnings":["cold start"]}"##,
    );

    let output = bob_command()
        .arg("query")
        .arg("--engine")
        .arg("obsidian")
        .arg("--source")
        .arg("#project")
        .arg("--vault")
        .arg("Bob")
        .env_remove("BOB_DATAVIEW_OBSIDIAN_COMMAND")
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("STUB_LOG", &log)
        .output()
        .expect("run bob query source query");

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "Projects/alpha.md\nInbox/waiting.md\n",
        "source paths should be printed cleanly:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("warning: cold start"),
        "protocol warnings should go to stderr:\n{}",
        format_output(&output)
    );

    let log_text = fs::read_to_string(&log).expect("read obsidian argv log");
    assert_text_order(&log_text, &["ARG:vault=Bob", "ARG:eval", "ARG:code="]);
    assert!(
        log_text.contains(r##""query":{"kind":"source","source":"#project"}"##)
            && log_text.contains("api.pagePaths")
            && log_text.contains("BOB_DATAVIEW_RESULT"),
        "expected generated source-query JavaScript in obsidian argv:\n{log_text}"
    );
}

#[test]
fn dataview_obsidian_dql_paths_extracts_and_deduplicates_note_paths() {
    let temp = TempDir::new("bob-cli-dataview-dql-paths");
    let obsidian = temp.path().join("obsidian");
    let log = temp.path().join("commands.log");
    write_obsidian_success_stub(
        &obsidian,
        r##"{"status":"ok","kind":"dql_json","result":{"type":"table","idMeaning":{"type":"path"},"headers":["File","Status"],"values":[[{"type":"link","path":"Projects/alpha.md","display":null,"embed":false},"active"],[{"type":"link","path":"Projects/alpha.md","display":null,"embed":false},"duplicate"],[{"path":"Inbox\\waiting"},"waiting"]]},"warnings":[]}"##,
    );

    let output = bob_command()
        .arg("query")
        .arg("--engine")
        .arg("obsidian")
        .arg("--query")
        .arg("TABLE status FROM #project")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("STUB_LOG", &log)
        .output()
        .expect("run bob query dql paths query");

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "Projects/alpha.md\nInbox/waiting.md\n",
        "DQL paths should be printed cleanly:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "unexpected query stderr:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_obsidian_dql_paths_warn_or_fail_for_missing_identities() {
    let temp = TempDir::new("bob-cli-dataview-dql-strict-paths");
    let obsidian = temp.path().join("obsidian");
    let log = temp.path().join("commands.log");
    write_obsidian_success_stub(
        &obsidian,
        r##"{"status":"ok","kind":"dql_json","result":{"type":"table","idMeaning":{"type":"path"},"headers":["File","Status"],"values":[[],[{"path":"Projects/alpha.md"},"active"]]},"warnings":[]}"##,
    );

    let output = bob_command()
        .arg("query")
        .arg("--engine")
        .arg("obsidian")
        .arg("--query")
        .arg("TABLE status FROM #project")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("STUB_LOG", &log)
        .output()
        .expect("run non-strict bob query dql paths query");

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "Projects/alpha.md\n",
        "non-strict DQL paths should print best-effort paths:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("warning: DQL table row 1"),
        "non-strict DQL paths should warn about missing identities:\n{}",
        format_output(&output)
    );

    let output = bob_command()
        .arg("query")
        .arg("--engine")
        .arg("obsidian")
        .arg("--query")
        .arg("TABLE status FROM #project")
        .arg("--strict-paths")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("STUB_LOG", &log)
        .output()
        .expect("run strict bob query dql paths query");

    assert_eq!(
        output.status.code(),
        Some(1),
        "strict DQL paths should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).is_empty(),
        "strict DQL paths must keep stdout clean:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("paths output could not derive clean note paths")
            && err.contains("DQL table row 1")
            && err.contains("--format json"),
        "strict DQL paths should explain how to inspect raw results:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_obsidian_dql_json_reads_query_file_and_forwards_env_vault() {
    let temp = TempDir::new("bob-cli-dataview-dql-json");
    let obsidian = temp.path().join("obsidian");
    let query_file = temp.path().join("projects.dql");
    let log = temp.path().join("commands.log");
    write_file(&query_file, "LIST FROM #project");
    write_obsidian_success_stub(
        &obsidian,
        r##"{"status":"ok","kind":"dql_json","result":{"type":"list","values":[{"path":"Projects/alpha.md"}]},"warnings":[]}"##,
    );

    let output = bob_command()
        .arg("query")
        .arg("--engine")
        .arg("obsidian")
        .arg("--format")
        .arg("json")
        .arg("--origin")
        .arg("Home.md")
        .arg("--query-file")
        .arg(&query_file)
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env("BOB_DATAVIEW_VAULT", "Bob")
        .env("STUB_LOG", &log)
        .output()
        .expect("run bob query dql json query");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected query stderr:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["engine"], "obsidian");
    assert_eq!(json["query_kind"], "dql");
    assert_eq!(json["format"], "json");
    assert_eq!(json["paths"][0], "Projects/alpha.md");
    assert_eq!(json["result"]["type"], "list");
    assert_eq!(json["result"]["values"][0]["path"], "Projects/alpha.md");

    let log_text = fs::read_to_string(&log).expect("read obsidian argv log");
    assert_text_order(&log_text, &["ARG:vault=Bob", "ARG:eval", "ARG:code="]);
    assert!(
        log_text.contains(r##""origin":"Home.md""##)
            && log_text.contains(
                r##""query":{"kind":"dql","query":"LIST FROM #project"}"##
            )
            && log_text.contains(
                "api.tryQuery(request.query.query, origin, { forceId: true })"
            ),
        "expected generated DQL JavaScript in obsidian argv:\n{log_text}"
    );
}

#[test]
fn dataview_obsidian_markdown_prints_rendered_markdown() {
    let temp = TempDir::new("bob-cli-dataview-markdown");
    let obsidian = temp.path().join("obsidian");
    let log = temp.path().join("commands.log");
    write_obsidian_success_stub(
        &obsidian,
        r##"{"status":"ok","kind":"markdown","markdown":"| File |\n| --- |\n| Alpha |\n","warnings":[]}"##,
    );

    let output = bob_command()
        .arg("query")
        .arg("--engine")
        .arg("obsidian")
        .arg("--format")
        .arg("markdown")
        .arg("--query")
        .arg("TABLE file.name FROM #project")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("STUB_LOG", &log)
        .output()
        .expect("run bob query markdown query");

    assert_success(&output);
    assert_eq!(stdout(&output), "| File |\n| --- |\n| Alpha |\n");
    assert!(
        stderr(&output).is_empty(),
        "unexpected query stderr:\n{}",
        format_output(&output)
    );
    let log_text = fs::read_to_string(&log).expect("read obsidian argv log");
    assert!(
        !log_text.contains("ARG:vault=")
            && log_text.contains("api.tryQueryMarkdown"),
        "markdown query should not forward an unset vault:\n{log_text}"
    );
}

#[test]
fn dataview_obsidian_reports_protocol_errors() {
    let cases = [
        (
            "missing-dataview",
            r##"{"status":"error","code":"DATAVIEW_MISSING","message":"Dataview plugin not loaded"}"##,
            "Dataview is disabled, missing, or not ready",
            "Dataview plugin not loaded",
        ),
        (
            "query-error",
            r##"{"status":"error","code":"DATAVIEW_QUERY_ERROR","message":"Expected one of FROM, WHERE"}"##,
            "Dataview query failed",
            "Expected one of FROM, WHERE",
        ),
    ];

    for (name, payload, marker, detail) in cases {
        let temp = TempDir::new(&format!("bob-cli-dataview-{name}"));
        let obsidian = temp.path().join("obsidian");
        let log = temp.path().join("commands.log");
        write_obsidian_success_stub(&obsidian, payload);

        let output = bob_command()
            .arg("query")
            .arg("--engine")
            .arg("obsidian")
            .arg("--format")
            .arg("json")
            .arg("--query")
            .arg("LIST FROM #project")
            .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
            .env_remove("BOB_DATAVIEW_VAULT")
            .env("STUB_LOG", &log)
            .output()
            .unwrap_or_else(|error| {
                panic!("run bob query protocol error {name}: {error}")
            });

        assert_eq!(
            output.status.code(),
            Some(1),
            "protocol error should fail:\n{}",
            format_output(&output)
        );
        assert!(
            stdout(&output).is_empty(),
            "protocol errors must keep stdout clean:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(marker)
                && stderr(&output).contains(detail),
            "expected protocol error report for {name}:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn dataview_obsidian_reports_missing_and_malformed_sentinel() {
    let cases = [
        (
            "missing-sentinel",
            "#!/bin/sh\nprintf 'plugin log only\\n'\n",
            "missing Obsidian protocol response",
            "plugin log only",
        ),
        (
            "malformed-sentinel",
            "#!/bin/sh\nprintf 'BOB_DATAVIEW_RESULT\\t{not-json}\\n'\n",
            "malformed Obsidian protocol response",
            "invalid sentinel JSON",
        ),
    ];

    for (name, script, marker, detail) in cases {
        let temp = TempDir::new(&format!("bob-cli-dataview-{name}"));
        let obsidian = temp.path().join("obsidian");
        write_executable(&obsidian, script);

        let output = bob_command()
            .arg("query")
            .arg("--engine")
            .arg("obsidian")
            .arg("--format")
            .arg("json")
            .arg("--query")
            .arg("LIST FROM #project")
            .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
            .env_remove("BOB_DATAVIEW_VAULT")
            .output()
            .unwrap_or_else(|error| {
                panic!("run bob query sentinel case {name}: {error}")
            });

        assert_eq!(
            output.status.code(),
            Some(1),
            "sentinel protocol failure should exit 1:\n{}",
            format_output(&output)
        );
        assert!(
            stdout(&output).is_empty(),
            "sentinel protocol failures must keep stdout clean:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(marker)
                && stderr(&output).contains(detail),
            "expected sentinel protocol error for {name}:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn dataview_obsidian_reports_missing_command_without_query_blob() {
    let temp = TempDir::new("bob-cli-dataview-missing-command");
    let missing = temp.path().join("missing-obsidian");

    let output = bob_command()
        .arg("query")
        .arg("--engine")
        .arg("obsidian")
        .arg("--format")
        .arg("json")
        .arg("--query")
        .arg("LIST FROM #project WHERE status = \"active\"")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &missing)
        .env_remove("BOB_DATAVIEW_VAULT")
        .output()
        .expect("run bob query with missing obsidian");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing obsidian command should fail:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        stdout(&output).is_empty()
            && err.contains("Obsidian command not found")
            && err.contains("BOB_DATAVIEW_OBSIDIAN_COMMAND")
            && !err.contains("LIST FROM #project")
            && !err.contains("code="),
        "missing-command error should be actionable without query/code leak:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_obsidian_reports_not_running_without_javascript_blob() {
    let temp = TempDir::new("bob-cli-dataview-not-running");
    let obsidian = temp.path().join("obsidian");
    write_executable(
        &obsidian,
        "#!/bin/sh\nprintf 'The CLI is unable to find Obsidian. Please make sure Obsidian is running and try again. %s\\n' \"$*\" >&2\nexit 1\n",
    );

    let output = bob_command()
        .arg("query")
        .arg("--engine")
        .arg("obsidian")
        .arg("--format")
        .arg("json")
        .arg("--query")
        .arg("LIST FROM #project WHERE status = \"active\"")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .output()
        .expect("run bob query when Obsidian is not running");

    assert_eq!(
        output.status.code(),
        Some(1),
        "not-running obsidian should fail:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        stdout(&output).is_empty()
            && err.contains("Obsidian is not running")
            && err.contains("<generated JavaScript>")
            && !err.contains("LIST FROM #project")
            && !err.contains("forceId"),
        "not-running error should redact generated JavaScript:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_obsidian_query_does_not_run_ob_command() {
    let temp = TempDir::new("bob-cli-dataview-no-sync");
    let vault = temp.path().join("vault");
    let ob = temp.path().join("ob");
    let obsidian = temp.path().join("obsidian");
    let ob_log = temp.path().join("ob.log");
    let obsidian_log = temp.path().join("obsidian.log");
    fs::create_dir_all(&vault).expect("create vault");
    write_executable(
        &ob,
        r#"#!/bin/sh
printf 'ob %s\n' "$*" >> "$OB_LOG"
printf 'ob should not run\n' >&2
exit 99
"#,
    );
    write_obsidian_success_stub(
        &obsidian,
        r##"{"status":"ok","kind":"source_paths","paths":["Projects/alpha.md"],"warnings":[]}"##,
    );

    let output = bob_command()
        .arg("query")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("--engine")
        .arg("obsidian")
        .arg("--source")
        .arg("#project")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("OB_COMMAND", &ob)
        .env("OB_LOG", &ob_log)
        .env("STUB_LOG", &obsidian_log)
        .output()
        .expect("run bob query with failing ob stub");

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "Projects/alpha.md\n",
        "paths output should stay clean without sync logs:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "query should not surface ob output:\n{}",
        format_output(&output)
    );
    assert!(
        !ob_log.exists(),
        "bob query must not run OB_COMMAND:\n{}",
        fs::read_to_string(&ob_log).unwrap_or_default()
    );
}

#[test]
fn dataview_rejects_removed_sync_option() {
    let temp = TempDir::new("bob-cli-dataview-sync-rejected");
    let ob = temp.path().join("ob");
    let ob_log = temp.path().join("ob.log");
    write_executable(
        &ob,
        "#!/bin/sh\nprintf 'ob %s\\n' \"$*\" >> \"$OB_LOG\"\nexit 99\n",
    );

    let output = bob_command()
        .arg("query")
        .arg("--sync")
        .arg("--source")
        .arg("#project")
        .env("OB_COMMAND", &ob)
        .env("OB_LOG", &ob_log)
        .output()
        .expect("run bob query with removed sync flag");

    assert_eq!(
        output.status.code(),
        Some(2),
        "removed --sync flag should be a usage failure:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).is_empty(),
        "usage failure must keep stdout clean:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("unexpected argument") && err.contains("--sync"),
        "removed --sync flag should fail visibly:\n{}",
        format_output(&output)
    );
    assert!(
        !ob_log.exists(),
        "usage rejection must not run OB_COMMAND:\n{}",
        fs::read_to_string(&ob_log).unwrap_or_default()
    );
}

#[test]
fn dataview_rejects_unsafe_origin_and_missing_bob_dir() {
    let temp = TempDir::new("bob-cli-dataview-path-validation");
    let vault = temp.path().join("vault");
    let missing_vault = temp.path().join("missing-vault");
    fs::create_dir_all(&vault).expect("create vault");
    let cases = [
        (
            vec![
                OsString::from("query"),
                OsString::from("--bob-dir"),
                vault.clone().into_os_string(),
                OsString::from("--origin"),
                OsString::from("../Secret.md"),
                OsString::from("--query"),
                OsString::from("LIST FROM #project"),
            ],
            "invalid --origin",
            ".. traversal",
        ),
        (
            vec![
                OsString::from("query"),
                OsString::from("--bob-dir"),
                vault.into_os_string(),
                OsString::from("--origin"),
                temp.path().join("absolute.md").into_os_string(),
                OsString::from("--query"),
                OsString::from("LIST FROM #project"),
            ],
            "invalid --origin",
            "absolute paths are not allowed",
        ),
        (
            vec![
                OsString::from("query"),
                OsString::from("--bob-dir"),
                missing_vault.into_os_string(),
                OsString::from("--query"),
                OsString::from("LIST FROM #project"),
            ],
            "--bob-dir must name an existing Bob vault directory",
            "missing-vault",
        ),
    ];

    for (args, marker, detail) in cases {
        let output = bob_command()
            .args(args)
            .output()
            .expect("run invalid query path case");

        assert_eq!(
            output.status.code(),
            Some(2),
            "path validation errors should be usage failures:\n{}",
            format_output(&output)
        );
        assert!(
            stdout(&output).is_empty()
                && stderr(&output).contains(marker)
                && stderr(&output).contains(detail),
            "expected path validation error:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn dataview_native_dql_paths_walks_parent_frontmatter_headlessly() {
    let temp = TempDir::new("bob-cli-dataview-native-parents");
    let vault = temp.path().join("vault");
    write_native_parent_chain_fixture(&vault);
    let query = r#"
LIST
FROM "ref"
WHERE source_pdf
  AND (
    parent = [[ai_ref]]
    OR parent.parent = [[ai_ref]]
    OR parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent.parent = [[ai_ref]]
  )
"#;

    let output = bob_command()
        .arg("query")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("--strict-paths")
        .arg("--query")
        .arg(query)
        .output()
        .expect("run native query parent query");

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "ref/papers/direct_ai.md\nref/papers/memory_os.md\n",
        "native parent query should only include source PDFs under ai_ref:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "native engine should keep stderr clean for supported queries:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_native_table_paths_match_list_rows_headlessly() {
    let temp = TempDir::new("bob-cli-dataview-native-table-paths");
    let vault = temp.path().join("vault");
    write_native_parent_chain_fixture(&vault);
    let query_tail = r#"
FROM "ref"
WHERE source_pdf
  AND (
    parent = [[ai_ref]]
    OR parent.parent = [[ai_ref]]
    OR parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent.parent = [[ai_ref]]
  )
"#;
    let list_query = format!("LIST\n{query_tail}");
    let table_query =
        format!("TABLE status, parent, source_path\n{query_tail}");

    let list_output = bob_command()
        .arg("query")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("--engine")
        .arg("native")
        .arg("--strict-paths")
        .arg("--query")
        .arg(&list_query)
        .output()
        .expect("run native query list parent query");
    let table_output = bob_command()
        .arg("query")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("--engine")
        .arg("native")
        .arg("--strict-paths")
        .arg("--query")
        .arg(&table_query)
        .output()
        .expect("run native query table parent query");

    assert_success(&list_output);
    assert_success(&table_output);
    assert_eq!(
        stdout(&table_output),
        stdout(&list_output),
        "native TABLE paths should match equivalent LIST rows:\nLIST:\n{}\nTABLE:\n{}",
        format_output(&list_output),
        format_output(&table_output)
    );
    assert_eq!(
        stdout(&table_output),
        "ref/papers/direct_ai.md\nref/papers/memory_os.md\n"
    );
    assert!(
        stderr(&list_output).is_empty() && stderr(&table_output).is_empty(),
        "native table/list paths should keep stderr clean:\nLIST:\n{}\nTABLE:\n{}",
        format_output(&list_output),
        format_output(&table_output)
    );
}

#[test]
fn dataview_native_table_json_projects_frontmatter_rows() {
    let temp = TempDir::new("bob-cli-dataview-native-table-json");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("ref/alpha.md"),
        "---\nstatus: active\nparent: \"[[memory_ref]]\"\nready: true\n---\n",
    );
    write_file(
        &vault.join("ref/beta.md"),
        "---\nparent: null\nready: false\n---\n",
    );

    let output = bob_command()
        .arg("query")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("--engine")
        .arg("native")
        .arg("--format")
        .arg("json")
        .arg("--query")
        .arg("TABLE status, parent, ready, missing FROM \"ref\"")
        .output()
        .expect("run native query table JSON query");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "native JSON TABLE query should keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["engine"], "native");
    assert_eq!(json["query_kind"], "dql");
    assert_eq!(json["format"], "json");
    assert_eq!(
        json["paths"],
        serde_json::json!(["ref/alpha.md", "ref/beta.md"])
    );
    assert_eq!(json["result"]["type"], "table");
    assert_eq!(
        json["result"]["idMeaning"],
        serde_json::json!({"type": "path"})
    );
    assert_eq!(
        json["result"]["headers"],
        serde_json::json!(["status", "parent", "ready", "missing"])
    );
    let values = json["result"]["values"]
        .as_array()
        .expect("table values array");
    assert_eq!(values.len(), 2, "expected two table rows:\n{json}");
    assert_eq!(
        values[0][0],
        serde_json::json!({
            "type": "link",
            "path": "ref/alpha.md",
            "display": null,
            "embed": false
        })
    );
    assert_eq!(values[0][1], "active");
    assert_eq!(
        values[0][2],
        serde_json::json!({
            "type": "link",
            "path": "memory_ref.md",
            "display": null,
            "embed": false
        })
    );
    assert_eq!(values[0][3], true);
    assert!(values[0][4].is_null(), "missing field should be null");
    assert_eq!(
        values[1][0],
        serde_json::json!({
            "type": "link",
            "path": "ref/beta.md",
            "display": null,
            "embed": false
        })
    );
    assert!(values[1][1].is_null(), "missing field should be null");
    assert!(values[1][2].is_null(), "frontmatter null should be null");
    assert_eq!(values[1][3], false);
}

#[test]
fn dataview_native_where_false_returns_no_rows() {
    let temp = TempDir::new("bob-cli-dataview-native-false");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("ref/papers/memory_os.md"),
        "---\nsource_pdf: lib/papers/memory-os.pdf\nparent: \"[[ai_ref]]\"\n---\n",
    );

    let output = bob_command()
        .arg("query")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("--engine")
        .arg("native")
        .arg("--query")
        .arg("LIST FROM \"ref\" WHERE false")
        .output()
        .expect("run native query false query");

    assert_success(&output);
    assert!(
        stdout(&output).is_empty() && stderr(&output).is_empty(),
        "WHERE false should not return fallback rows:\n{}",
        format_output(&output)
    );
}

fn write_obsidian_success_stub(path: &Path, payload: &str) {
    let sentinel_line =
        shell_single_quote(&format!("BOB_DATAVIEW_RESULT\t{payload}"));
    write_executable(
        path,
        &format!(
            "#!/bin/sh\n\
             : > \"$STUB_LOG\"\n\
             for arg in \"$@\"; do printf 'ARG:%s\\n' \"$arg\" >> \"$STUB_LOG\"; done\n\
             printf 'plugin log before\\n'\n\
             printf '%s\\n' {sentinel_line}\n\
             printf 'plugin log after\\n'\n"
        ),
    );
}

fn write_native_parent_chain_fixture(vault: &Path) {
    write_file(&vault.join("ai_ref.md"), "---\n---\n");
    write_file(&vault.join("sase.md"), "---\nparent: \"[[tools]]\"\n---\n");
    write_file(
        &vault.join("agent_ref.md"),
        "---\nparent: \"[[ai_ref]]\"\n---\n",
    );
    write_file(
        &vault.join("memory_ref.md"),
        "---\nparent: \"[[agent_ref]]\"\n---\n",
    );
    write_file(
        &vault.join("ref/papers/direct_ai.md"),
        "---\nsource_pdf: lib/papers/direct-ai.pdf\nsource_path: lib/papers/direct-ai.pdf\nstatus: direct\nparent: \"[[ai_ref]]\"\n---\n",
    );
    write_file(
        &vault.join("ref/papers/memory_os.md"),
        "---\nsource_pdf: lib/papers/memory-os.pdf\nsource_path: lib/papers/memory-os.pdf\nstatus: inherited\nparent: \"[[memory_ref]]\"\n---\n",
    );
    write_file(
        &vault.join("ref/papers/log_is_the_agent.md"),
        "---\nsource_pdf: lib/papers/log-is-the-agent.pdf\nsource_path: lib/papers/log-is-the-agent.pdf\nstatus: other\nparent: \"[[sase]]\"\n---\n",
    );
    write_file(
        &vault.join("ref/chat/obsidian-note-refactor.md"),
        "---\nsource_pdf: lib/chat/obsidian-note-refactor.pdf\nsource_path: lib/chat/obsidian-note-refactor.pdf\nstatus: other\nparent: \"[[obsidian]]\"\n---\n",
    );
    write_file(
        &vault.join("ref/papers/not_a_pdf.md"),
        "---\nstatus: inherited\nparent: \"[[memory_ref]]\"\n---\n",
    );
}
