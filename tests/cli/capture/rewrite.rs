//! Capture rewrite and complete-and-rewrite.

use crate::support::*;

#[test]
fn capture_rewrite_help_describes_only_the_remaining_rules() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("--help")
        .output()
        .expect("run bob capture-rewrite help");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("It implements five rules."),
        "capture-rewrite help miscounts its rules:\n{out}"
    );
    assert!(
        out.contains("lowercase 'x' or 's'")
            || out.contains("x' or 's'"),
        "capture-rewrite help should describe the x/s close-outcome alias:\n{out}"
    );
    assert!(
        out.contains("glued '.'") && out.contains("switches between"),
        "capture-rewrite help should describe the glued-dot toggle:\n{out}"
    );
    assert!(
        !out.contains("Template start"),
        "capture-rewrite help still documents the template start:\n{out}"
    );
    assert!(
        !output.stdout.contains(&b'\r'),
        "capture-rewrite help contains a raw CR byte"
    );
}

#[test]
fn capture_rewrite_human_output_is_plain_and_concise() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("16")
        .arg("--")
        .arg("Buy milk @dev @@")
        .output()
        .expect("run bob capture-rewrite human");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-rewrite stderr:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert_text_order(
        &out,
        &[
            "absorb_local_marker",
            "before",
            "after",
            "Moved @dev into @@dev",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_rewrite_json_absorbs_a_local_marker() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("16")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Buy milk @dev @@")
        .output()
        .expect("run bob capture-rewrite json");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-rewrite stderr:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], "Buy milk @dev @@");
    assert_eq!(json["text"], "Buy milk @@dev");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], 14);
    assert_eq!(json["rule"], "absorb_local_marker");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": 9, "end": 14 }, "replacement": "" },
            { "range": { "start": 14, "end": 16 }, "replacement": "@@dev" },
        ])
    );
    assert_eq!(json["summary"], "Moved @dev into @@dev");
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_rewrite_json_expands_a_route_sigil_alias() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("9")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@errands.")
        .output()
        .expect("run bob capture-rewrite sigil alias");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], "@errands.");
    assert_eq!(json["text"], "@errands^");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], 9);
    assert_eq!(json["rule"], "expand_sigil_alias");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": 8, "end": 9 }, "replacement": "^" },
        ])
    );
    assert_eq!(json["summary"], "Typed ^ for .");
}

#[test]
fn capture_rewrite_json_expands_a_relation_equals_alias() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("8")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Fix it =")
        .output()
        .expect("run bob capture-rewrite relation equals alias");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], "Fix it =");
    assert_eq!(json["text"], "Fix it ~");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], 8);
    assert_eq!(json["rule"], "expand_sigil_alias");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": 7, "end": 8 }, "replacement": "~" },
        ])
    );
    assert_eq!(json["summary"], "Typed ~ for =");
}

#[test]
fn capture_rewrite_json_expands_a_relation_flip_alias() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("9")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Fix it ~=")
        .output()
        .expect("run bob capture-rewrite relation flip alias");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], "Fix it ~=");
    assert_eq!(json["text"], "Fix it ~~");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], 9);
    assert_eq!(json["rule"], "expand_sigil_alias");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": 8, "end": 9 }, "replacement": "~" },
        ])
    );
    assert_eq!(json["summary"], "Typed ~ for =");
}

#[test]
fn capture_rewrite_json_expands_a_relation_reverse_flip_alias() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("10")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Fix it ~~=")
        .output()
        .expect("run bob capture-rewrite reverse relation flip alias");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], "Fix it ~~=");
    assert_eq!(json["text"], "Fix it ~");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], 8);
    assert_eq!(json["rule"], "expand_sigil_alias");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": 8, "end": 10 }, "replacement": "" },
        ])
    );
    assert_eq!(json["summary"], "Typed ~ for ~~=");
}

#[test]
fn capture_rewrite_json_toggles_complete_relation_direction() {
    let draft = "Fix it ~tasks:target=";
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-rewrite relation direction toggle");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], draft);
    assert_eq!(json["text"], "Fix it ~~tasks:target");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], 21);
    assert_eq!(json["rule"], "toggle_relation_direction");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": 8, "end": 8 }, "replacement": "~" },
            { "range": { "start": 20, "end": 21 }, "replacement": "" },
        ])
    );
    assert_eq!(json["summary"], "Flipped to Linked From");
}

#[test]
fn capture_rewrite_keeps_relation_dash_literal() {
    let draft = "Fix it -";
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-rewrite with literal dash");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["input"], draft);
    assert_eq!(json["text"], draft);
    assert_eq!(json["changed"], false);
    assert!(json["rule"].is_null());
    assert_eq!(json["edits"], serde_json::json!([]));
}

#[test]
fn capture_rewrite_json_expands_glued_target_comma_with_one_edit() {
    let draft = "@cash+goog-exit,";
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-rewrite target continuation alias");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], draft);
    assert_eq!(json["text"], "@cash+goog-exit +");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], draft.len() + 1);
    assert_eq!(json["rule"], "expand_sigil_alias");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            {
                "range": { "start": draft.len() - 1, "end": draft.len() },
                "replacement": " +"
            },
        ])
    );
    assert_eq!(json["summary"], "Typed + for ,");
}

#[test]
fn capture_rewrite_json_expands_a_move_bullet_sigil_alias() {
    let draft = "@cash:goog-exit@\n- ;";
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-rewrite move sigil alias");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["input"], draft);
    assert_eq!(json["text"], "@cash:goog-exit@\n- :");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], draft.len());
    assert_eq!(json["rule"], "expand_sigil_alias");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": draft.len() - 1, "end": draft.len() }, "replacement": ":" },
        ])
    );
    assert_eq!(json["summary"], "Typed : for ;");
}

#[test]
fn capture_rewrite_json_expands_a_dot_route_alias() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("10")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Buy milk .")
        .output()
        .expect("run bob capture-rewrite dot route alias");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], "Buy milk .");
    assert_eq!(json["text"], "Buy milk @");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], 10);
    assert_eq!(json["rule"], "expand_sigil_alias");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": 9, "end": 10 }, "replacement": "@" },
        ])
    );
    assert_eq!(json["summary"], "Typed @ for .");
}

#[test]
fn capture_rewrite_json_expands_a_dot_dependency_alias() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("7")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@home .")
        .output()
        .expect("run bob capture-rewrite . alias");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], "@home .");
    assert_eq!(json["text"], "@home &");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], 7);
    assert_eq!(json["rule"], "expand_sigil_alias");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": 6, "end": 7 }, "replacement": "&" },
        ])
    );
    assert_eq!(json["summary"], "Typed & for .");
}

#[test]
fn capture_rewrite_json_dot_prefers_dependency_once_routed() {
    let draft = "Buy milk @foo:bar .";
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-rewrite routed dot alias");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], draft);
    assert_eq!(json["text"], "Buy milk @foo:bar &");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], draft.len());
    assert_eq!(json["rule"], "expand_sigil_alias");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": draft.len() - 1, "end": draft.len() }, "replacement": "&" },
        ])
    );
    assert_eq!(json["summary"], "Typed & for .");
    assert_eq!(json["alias"]["id"], "dependency");
}

#[test]
fn capture_rewrite_json_expands_a_semicolon_blocked_task_alias() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("10")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Buy milk ;")
        .output()
        .expect("run bob capture-rewrite ; alias");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], "Buy milk ;");
    assert_eq!(json["text"], "Buy milk *");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], 10);
    assert_eq!(json["rule"], "expand_sigil_alias");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": 9, "end": 10 }, "replacement": "*" },
        ])
    );
    assert_eq!(json["summary"], "Typed * for ;");
}

#[test]
fn capture_rewrite_json_leaves_a_third_at_alone() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("12")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Read {0} @@@")
        .output()
        .expect("run capture-rewrite with a third at");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["text"], "Read {0} @@@");
    assert_eq!(json["changed"], false);
    assert!(json["rule"].is_null(), "{json}");
}

#[test]
fn capture_rewrite_json_leaves_template_drafts_untouched() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("18")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @file:{0}^ %t\n\nrow")
        .output()
        .expect("run capture-rewrite on a template draft");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["text"], "Do work @file:{0}^ %t\n\nrow");
    assert_eq!(json["changed"], false);
}

#[test]
fn capture_rewrite_toggles_task_bullet_ids_in_lists_and_project_notes() {
    for (before, expected) in [
        ("@dev\n- Task :task-id^", "@dev\n- Task ^task-id"),
        ("@dev\n- Task ^task-id:", "@dev\n- Task :task-id"),
        ("@dev\n- Task :task-id.", "@dev\n- Task ^task-id"),
        ("@dev\n- Task ^task-id.", "@dev\n- Task :task-id"),
        (
            "Project @cash^project-id@\n- Task :task-id^",
            "Project @cash^project-id@\n- Task ^task-id",
        ),
        (
            "Project @cash^project-id@\n- Task ^task-id:",
            "Project @cash^project-id@\n- Task :task-id",
        ),
        (
            "Project @cash^project-id@\n- Task :task-id.",
            "Project @cash^project-id@\n- Task ^task-id",
        ),
        (
            "Project @cash^project-id@\n- Task ^task-id.",
            "Project @cash^project-id@\n- Task :task-id",
        ),
    ] {
        let output = bob_command()
            .arg("capture-rewrite")
            .arg("-c")
            .arg(before.len().to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(before)
            .output()
            .expect("run task ID toggle");

        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("capture-rewrite JSON");
        assert_eq!(json["changed"], true, "{before}");
        assert_eq!(json["text"], expected, "{before}");
        assert_eq!(json["cursor"], expected.len(), "{before}");
        assert_eq!(json["rule"], "switch_block_id_separator", "{before}");
    }
}

#[test]
fn capture_rewrite_json_reports_a_rule_a5_notice_without_changing_text() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note @notes#Ideas @@")
        .output()
        .expect("run bob capture-rewrite notice");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["changed"], false);
    assert_eq!(json["text"], "note @notes#Ideas @@");
    assert!(json.get("rule").is_none(), "{json}");
    assert_eq!(json["notices"].as_array().expect("notices").len(), 1);
    assert!(json["notices"][0]
        .as_str()
        .expect("notice")
        .contains("cannot take a section"));
}

#[test]
fn capture_rewrite_json_reports_no_rewrite_without_a_bare_at_at() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Buy milk @dev")
        .output()
        .expect("run bob capture-rewrite no-op");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["changed"], false);
    assert_eq!(json["text"], "Buy milk @dev");
    assert!(json.get("cursor").is_none(), "{json}");
    assert_eq!(json["edits"], serde_json::json!([]));
}

#[test]
fn capture_rewrite_reads_stdin_when_text_is_omitted() {
    let output = run_with_stdin(
        bob_command().arg("capture-rewrite").arg("-f").arg("json"),
        "Buy milk @dev @@",
    );

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["text"], "Buy milk @@dev");
}

#[test]
fn capture_rewrite_missing_text_is_a_usage_error() {
    let human = run_with_stdin(bob_command().arg("capture-rewrite"), "   \n");
    assert_eq!(human.status.code(), Some(2));
    assert!(
        stdout(&human).is_empty()
            && stderr(&human).contains("task text is required"),
        "unexpected capture-rewrite missing-text output:\n{}",
        format_output(&human)
    );

    let json_output = run_with_stdin(
        bob_command().arg("capture-rewrite").arg("-f").arg("json"),
        "   \n",
    );
    assert_eq!(json_output.status.code(), Some(2));
    assert!(
        stderr(&json_output).is_empty(),
        "JSON failures keep stderr clean:\n{}",
        format_output(&json_output)
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&json_output).trim())
            .expect("capture-rewrite failure JSON");
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .expect("error string")
            .contains("task text is required"),
        "unexpected capture-rewrite failure JSON: {json}"
    );
}

#[test]
fn capture_rewrite_rejects_a_cursor_off_a_char_boundary() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("2")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("\u{1f680} @dev @@")
        .output()
        .expect("run bob capture-rewrite bad cursor");

    assert_eq!(output.status.code(), Some(2));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite failure JSON");
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .expect("error string")
            .contains("UTF-8 byte boundary"),
        "unexpected capture-rewrite failure JSON: {json}"
    );
}

#[test]
fn capture_rewrite_never_touches_the_vault_or_clipboard() {
    let temp = TempDir::new("bob-cli-capture-rewrite-no-io");
    let missing = temp.path().join("missing-vault");
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Buy milk @dev @@")
        .env("BOB_DIR", &missing)
        .output()
        .expect("run bob capture-rewrite without a vault");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert!(
        !missing.exists(),
        "capture-rewrite must not create the vault directory"
    );

    let toggle = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("17")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @file:id^")
        .env("BOB_DIR", &missing)
        .output()
        .expect("run bob capture-rewrite toggle without a vault");
    assert_success(&toggle);
    let json: serde_json::Value = serde_json::from_str(stdout(&toggle).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["rule"], "switch_block_id_separator");
    assert_eq!(json["text"], "Do work @file^id");
    assert!(
        !missing.exists(),
        "capture-rewrite must not create the vault directory"
    );
}

#[test]
fn capture_rewrite_json_switches_a_block_id_separator() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("17")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @file:id^")
        .output()
        .expect("run bob capture-rewrite toggle json");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], "Do work @file:id^");
    assert_eq!(json["text"], "Do work @file^id");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], 16);
    assert_eq!(json["rule"], "switch_block_id_separator");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": 8, "end": 17 }, "replacement": "@file^id" },
        ])
    );
    assert_eq!(json["summary"], "Changed @file:id to @file^id");

    let again = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("16")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @file^id")
        .output()
        .expect("run bob capture-rewrite idempotent");
    assert_success(&again);
    let json: serde_json::Value = serde_json::from_str(stdout(&again).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["changed"], false);
    assert_eq!(json["text"], "Do work @file^id");
}

#[test]
fn capture_rewrite_json_dot_switches_a_block_id_separator() {
    let input = "Create sase-gallery! @sase:sase-gallery.";
    let expected = "Create sase-gallery! @sase^sase-gallery";
    let marker_start = input.find("@sase:").expect("marker start");
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg(input.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(input)
        .output()
        .expect("run bob capture-rewrite glued-dot toggle");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["changed"], true);
    assert_eq!(json["rule"], "switch_block_id_separator");
    assert_eq!(json["text"], expected);
    assert_eq!(json["cursor"], expected.len());
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": marker_start, "end": input.len() }, "replacement": "@sase^sase-gallery" },
        ])
    );
    assert_eq!(
        json["summary"],
        "Changed @sase:sase-gallery to @sase^sase-gallery"
    );
    assert!(json.get("alias").is_none(), "{json}");
    assert!(json.get("sigil_alias_tip").is_none(), "{json}");
}

#[test]
fn capture_rewrite_json_dot_switches_a_plus_marker_to_caret() {
    let input = "Create sase-gallery! @sase+sase-gallery.";
    let expected = "Create sase-gallery! @sase^sase-gallery";
    let marker_start = input.find("@sase+").expect("marker start");
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg(input.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(input)
        .output()
        .expect("run bob capture-rewrite glued-dot plus switch");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["changed"], true);
    assert_eq!(json["rule"], "switch_block_id_separator");
    assert_eq!(json["text"], expected);
    assert_eq!(json["cursor"], expected.len());
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": marker_start, "end": input.len() }, "replacement": "@sase^sase-gallery" },
        ])
    );
    assert_eq!(
        json["summary"],
        "Changed @sase+sase-gallery to @sase^sase-gallery"
    );
    assert!(json.get("alias").is_none(), "{json}");
    assert!(json.get("sigil_alias_tip").is_none(), "{json}");

    let again_input = format!("{expected}.");
    let again = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg(again_input.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(&again_input)
        .output()
        .expect("run bob capture-rewrite glued-dot second toggle");
    assert_success(&again);
    let json: serde_json::Value = serde_json::from_str(stdout(&again).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["text"], "Create sase-gallery! @sase:sase-gallery");
}

#[test]
fn capture_rewrite_json_comma_switches_a_block_id_separator_to_plus() {
    let input = "Create sase-gallery! @sase:sase-gallery,";
    let expected = "Create sase-gallery! @sase+sase-gallery";
    let marker_start = input.find("@sase:").expect("marker start");
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg(input.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(input)
        .output()
        .expect("run bob capture-rewrite glued-comma switch");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["changed"], true);
    assert_eq!(json["rule"], "switch_block_id_separator");
    assert_eq!(json["text"], expected);
    assert_eq!(json["cursor"], expected.len());
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": marker_start, "end": input.len() }, "replacement": "@sase+sase-gallery" },
        ])
    );
    assert_eq!(
        json["summary"],
        "Changed @sase:sase-gallery to @sase+sase-gallery"
    );
    assert!(json.get("alias").is_none(), "{json}");
    assert!(json.get("sigil_alias_tip").is_none(), "{json}");
}

#[test]
fn capture_rewrite_json_switches_a_colon_marker_to_plus() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("17")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @file:id+")
        .output()
        .expect("run capture-rewrite plus toggle json");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], "Do work @file:id+");
    assert_eq!(json["text"], "Do work @file+id");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], 16);
    assert_eq!(json["rule"], "switch_block_id_separator");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": 8, "end": 17 }, "replacement": "@file+id" },
        ])
    );
    assert_eq!(json["summary"], "Changed @file:id to @file+id");

    let again = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("16")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @file+id")
        .output()
        .expect("run capture-rewrite idempotent plus toggle");
    assert_success(&again);
    let json: serde_json::Value = serde_json::from_str(stdout(&again).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["changed"], false);
    assert_eq!(json["text"], "Do work @file+id");
}

#[test]
fn capture_parse_keeps_project_note_plus_literal() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@work^prep@")
        .output()
        .expect("run capture-parse on project-note marker");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "project_note");
}

#[test]
fn capture_parse_does_not_apply_the_separator_toggle() {
    for input in [
        "Do work @file:id^",
        "Do work @file:id.",
        "Do work @file:id,",
    ] {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(input)
            .output()
            .expect("run bob capture-parse unrewritten toggle");

        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("capture-parse JSON");
        assert_eq!(json["input"], input);
        assert_eq!(json["body"], "Do work");
        assert!(json["block_id"].is_null(), "{json}");
        let codes: Vec<&str> = json["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .filter_map(|diagnostic| diagnostic["code"].as_str())
            .collect();
        assert!(codes.contains(&"invalid_pomodoro_block_id"), "{json}");
    }

    let input = "Do work @file+id.";
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(input)
        .output()
        .expect("run bob capture-parse unrewritten plus dot");
    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["input"], input);
    assert_eq!(json["body"], "Do work");
    assert!(json["block_id"].is_null(), "{json}");
    let codes: Vec<&str> = json["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str())
        .collect();
    assert!(codes.contains(&"invalid_sub_bullet_block_id"), "{json}");
}

#[test]
fn capture_complete_and_rewrite_ignore_adjustments() {
    let complete = bob_command()
        .arg("capture-complete")
        .arg("-c")
        .arg("1")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("+5")
        .output()
        .expect("run capture-complete");
    assert_success(&complete);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&complete).trim()).expect("complete JSON");
    assert!(json["context"].is_null(), "{json}");
    assert_eq!(json["candidates"], serde_json::json!([]));

    let rewrite = bob_command()
        .arg("capture-rewrite")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("+5")
        .output()
        .expect("run capture-rewrite");
    assert_success(&rewrite);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&rewrite).trim()).expect("rewrite JSON");
    assert_eq!(json["changed"], false);
    assert_eq!(json["text"], "+5");
}

#[test]
fn capture_complete_and_rewrite_ignore_shifts() {
    for (text, cursor) in [("++3", 2), ("--", 1), ("++", 1)] {
        let complete = bob_command()
            .arg("capture-complete")
            .arg("-c")
            .arg(cursor.to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run capture-complete");
        assert_success(&complete);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&complete).trim())
                .expect("complete JSON");
        assert!(json["context"].is_null(), "{text}: {json}");
        assert_eq!(json["candidates"], serde_json::json!([]), "{text}");
    }

    for text in ["++3", "--"] {
        let rewrite = bob_command()
            .arg("capture-rewrite")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run capture-rewrite");
        assert_success(&rewrite);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&rewrite).trim())
                .expect("rewrite JSON");
        assert_eq!(json["changed"], false, "{text}");
        assert_eq!(json["text"], text, "{text}");
    }

    // A `@@` declaration leaves a shift item alone, like an adjustment.
    let declared = run_with_stdin(
        bob_command().arg("capture-rewrite").arg("-f").arg("json"),
        "@@foo\n++3",
    );
    assert_success(&declared);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&declared).trim()).expect("rewrite JSON");
    assert_eq!(json["changed"], false, "{json}");
    assert_eq!(json["text"], "@@foo\n++3", "{json}");
}
#[test]
fn capture_rewrite_pomodoro_close_protocol() {
    // `x` is never rewritten, on whole items or link forms.
    for text in [
        "x",
        "^r:id=x",
        "Text @r:id=x",
        "x1,3!2",
        "x0",
        "x1,",
        "^r:id=x1",
        "Text @r:id=x!1",
    ] {
        let output = bob_command()
            .arg("capture-rewrite")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run capture-rewrite");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("rewrite JSON");
        assert_eq!(json["input"], text, "{text}");
        assert_eq!(json["text"], text, "{text}");
        assert_eq!(json["changed"], false, "{text}");
        assert_eq!(json["edits"], serde_json::json!([]), "{text}");
    }
}

#[test]
fn capture_complete_and_rewrite_ignore_starts() {
    for (text, cursor) in [("=", 1), ("=3", 1), ("=3", 2), ("=-2", 2)] {
        let complete = bob_command()
            .arg("capture-complete")
            .arg("-c")
            .arg(cursor.to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run capture-complete");
        assert_success(&complete);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&complete).trim())
                .expect("complete JSON");
        assert_eq!(json["ok"], true, "{text}: {json}");
        assert!(json["context"].is_null(), "{text}: {json}");
        assert_eq!(json["candidates"], serde_json::json!([]), "{text}");
    }

    for text in ["=", "=3", "=-2"] {
        let rewrite = bob_command()
            .arg("capture-rewrite")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run capture-rewrite");
        assert_success(&rewrite);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&rewrite).trim())
                .expect("rewrite JSON");
        assert_eq!(json["changed"], false, "{text}");
        assert_eq!(json["text"], text, "{text}");
    }

    // A `@@` declaration leaves a start item alone, like a shift or close.
    let declared = run_with_stdin(
        bob_command().arg("capture-rewrite").arg("-f").arg("json"),
        "@@foo\n=3",
    );
    assert_success(&declared);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&declared).trim()).expect("rewrite JSON");
    assert_eq!(json["changed"], false, "{json}");
    assert_eq!(json["text"], "@@foo\n=3", "{json}");
    // A bare `@@` inside a Work Log entry is literal, on bullets and inline:
    // it is never absorbed and never treated as a declaration.
    for text in ["x\n- 1 moved @@inbox", "x moved @@inbox"] {
        let rewrite = bob_command()
            .arg("capture-rewrite")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("rewrite entry");
        assert_success(&rewrite);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&rewrite).trim())
                .expect("rewrite JSON");
        assert_eq!(json["changed"], false, "{text}: {json}");
        assert_eq!(json["text"], text, "{text}: {json}");
    }
}

#[test]
fn capture_rewrite_switches_a_token_that_becomes_a_run_member() {
    for (draft, cursor, expected) in [
        ("@work+a @dev:b+", "@work+a @dev:b+".len(), "@work+a @dev+b"),
        (
            "note @work:a+ @dev+b",
            "note @work:a+".len(),
            "note @work+a @dev+b",
        ),
        ("@work:a+ @dev+b", "@work:a+".len(), "@work+a @dev+b"),
        (
            "@work+a %c @dev:b+",
            "@work+a %c @dev:b+".len(),
            "@work+a %c @dev+b",
        ),
        (
            "note @work:a+ %c @dev+b",
            "note @work:a+".len(),
            "note @work+a %c @dev+b",
        ),
    ] {
        let output = bob_command()
            .arg("capture-rewrite")
            .arg("-c")
            .arg(cursor.to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(draft)
            .output()
            .expect("run capture-rewrite");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("rewrite JSON");
        assert_eq!(json["changed"], true, "{draft}: {json}");
        assert_eq!(json["text"], expected, "{draft}: {json}");
        assert_eq!(json["rule"], "switch_block_id_separator", "{draft}");
    }
}

#[test]
fn capture_rewrite_reports_a_run_for_bare_at_at() {
    let draft = "@work+a @dev+b @@";
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run capture-rewrite");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("rewrite JSON");
    assert_eq!(json["changed"], false, "{json}");
    assert_eq!(json["text"], draft, "{json}");
    let notices = json["notices"].as_array().expect("notices");
    assert_eq!(notices.len(), 1, "{json}");
    assert!(
        notices[0]
            .as_str()
            .expect("notice")
            .contains("existing task"),
        "{json}"
    );
}

#[test]
fn capture_rewrite_json_expands_a_backslash_directive_alias() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("10")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Buy milk \\")
        .output()
        .expect("run bob capture-rewrite backslash alias");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["input"], "Buy milk \\");
    assert_eq!(json["text"], "Buy milk %");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], 10);
    assert_eq!(json["rule"], "expand_sigil_alias");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": 9, "end": 10 }, "replacement": "%" },
        ])
    );
    assert_eq!(json["summary"], "Typed % for \\");
    assert_eq!(
        json["alias"],
        serde_json::json!({ "id": "directive_menu", "key": "\\", "sigil": "%" })
    );
}

#[test]
fn capture_rewrite_json_leaves_a_line_start_backslash_literal() {
    let draft = "Buy milk\n\\";
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-rewrite line-start backslash");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["changed"], false, "{json}");
    assert_eq!(json["text"], draft, "{json}");
}

#[test]
fn capture_rewrite_json_expands_a_slash_hash_alias() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("10")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Buy milk /")
        .output()
        .expect("run bob capture-rewrite slash alias");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["input"], "Buy milk /");
    assert_eq!(json["text"], "Buy milk #");
    assert_eq!(json["changed"], true);
    assert_eq!(json["cursor"], 10);
    assert_eq!(json["rule"], "expand_sigil_alias");
    assert_eq!(
        json["edits"],
        serde_json::json!([
            { "range": { "start": 9, "end": 10 }, "replacement": "#" },
        ])
    );
    assert_eq!(json["summary"], "Typed # for /");
}

#[test]
fn capture_rewrite_json_leaves_a_split_slash_literal() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("2")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=/")
        .output()
        .expect("run bob capture-rewrite split slash");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-rewrite JSON");
    assert_eq!(json["changed"], false, "{json}");
    assert_eq!(json["text"], "=/", "{json}");
}

fn capture_rewrite_json(
    draft: &str,
    cursor: Option<usize>,
) -> serde_json::Value {
    let mut command = bob_command();
    command.arg("capture-rewrite").arg("-f").arg("json");
    if let Some(cursor) = cursor {
        command.arg("-c").arg(cursor.to_string());
    }
    let output = command
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-rewrite");
    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    serde_json::from_str(stdout(&output).trim()).expect("capture-rewrite JSON")
}

#[test]
fn capture_rewrite_json_expands_close_outcome_aliases() {
    for (draft, expected, replacement, summary) in [
        ("xx", "x!", "!", "Typed ! for x"),
        ("x=1=s", "x=1=*", "*", "Typed * for s"),
        ("@r:id=xx", "@r:id=x!", "!", "Typed ! for x"),
        ("+2 xs", "+2 x*", "*", "Typed * for s"),
        ("2xx", "2x!", "!", "Typed ! for x"),
        ("2x1s", "2x1*", "*", "Typed * for s"),
        ("^r:id=2xs", "^r:id=2x*", "*", "Typed * for s"),
        ("--1 2xs", "--1 2x*", "*", "Typed * for s"),
    ] {
        let json = capture_rewrite_json(draft, Some(draft.len()));
        assert_eq!(json["ok"], true, "{draft}");
        assert_eq!(json["schema_version"], 1, "{draft}");
        assert_eq!(json["input"], draft, "{draft}");
        assert_eq!(json["text"], expected, "{draft}");
        assert_eq!(json["changed"], true, "{draft}");
        assert_eq!(json["cursor"], draft.len(), "{draft}");
        assert_eq!(json["rule"], "expand_sigil_alias", "{draft}");
        assert_eq!(
            json["edits"],
            serde_json::json!([
                {
                    "range": { "start": draft.len() - 1, "end": draft.len() },
                    "replacement": replacement,
                },
            ]),
            "{draft}"
        );
        assert_eq!(json["summary"], summary, "{draft}");
    }
}

#[test]
fn capture_rewrite_json_leaves_close_outcome_letters_literal_without_cursor() {
    for draft in ["xx", "xs", "x=1=s", "@r:id=xx", "+2 xs", "2xx", "2x1s"] {
        let json = capture_rewrite_json(draft, None);
        assert_eq!(json["changed"], false, "{draft}: {json}");
        assert_eq!(json["text"], draft, "{draft}");
        assert!(json["rule"].is_null(), "{draft}: {json}");
    }
}

#[test]
fn capture_parse_and_capture_keep_xx_as_prose() {
    let parse = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("xx")
        .output()
        .expect("run bob capture-parse xx");
    assert_success(&parse);
    let json: serde_json::Value = serde_json::from_str(stdout(&parse).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "task", "{json}");
    assert_eq!(json["input"], "xx", "{json}");

    let temp = TempDir::new("bob-cli-xx-prose");
    let vault = temp.path().join("vault");
    std::fs::create_dir_all(&vault).expect("create vault");
    let dry = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("xx")
        .output()
        .expect("run bob capture --dry-run xx");
    assert_success(&dry);
    let json: serde_json::Value = serde_json::from_str(stdout(&dry).trim())
        .expect("capture dry-run JSON");
    assert_eq!(
        json["kind"], "task",
        "xx dry-run should stay a task, got {json}"
    );
}

#[test]
fn capture_rewrite_json_reports_a_typing_tip_for_a_hard_typed_sigil() {
    let json = capture_rewrite_json("Buy milk @", Some(10));
    assert_eq!(
        json,
        serde_json::json!({
            "ok": true,
            "schema_version": 1,
            "input": "Buy milk @",
            "text": "Buy milk @",
            "changed": false,
            "cursor": 10,
            "edits": [],
            "sigil_alias_tip": {
                "id": "route_after_text",
                "sigil": "@",
                "keys": ["."],
                "range": { "start": 9, "end": 10 },
                "where": "after a word",
            },
        })
    );
}

#[test]
fn capture_rewrite_json_tips_name_every_key_that_would_have_typed_the_sigil() {
    for (draft, id, keys, position) in [
        (
            "@errands^",
            "block_id_after_route",
            vec!["."],
            Some("right after a route"),
        ),
        (
            "@errands:",
            "link_after_route",
            vec![";"],
            Some("right after a route"),
        ),
        (
            "@cash+",
            "parent_task",
            vec![","],
            Some("right after a route"),
        ),
        ("Buy milk #", "hash_menu", vec!["/"], Some("after a word")),
        (
            "Buy milk %",
            "directive_menu",
            vec!["\\"],
            Some("after a word"),
        ),
        ("Buy milk\n- %", "directive_menu", vec!["\\"], None),
        ("Fix it ~", "relation", vec!["="], Some("after a word")),
        ("@home &", "dependency", vec!["."], Some("once routed")),
        (
            "Buy milk @foo:bar &",
            "dependency",
            vec!["."],
            Some("once routed"),
        ),
        (
            "Buy milk *",
            "blocked_task",
            vec![";"],
            Some("after a word"),
        ),
        (
            "@cash^goog-exit@\n- ^",
            "move_bullet_caret",
            vec!["."],
            Some("on a move bullet"),
        ),
    ] {
        let json = capture_rewrite_json(draft, Some(draft.len()));
        let tip = &json["sigil_alias_tip"];
        assert_eq!(json["changed"], false, "{draft:?}: {json}");
        assert_eq!(tip["id"], id, "{draft:?}: {json}");
        assert_eq!(tip["keys"], serde_json::json!(keys), "{draft:?}");
        assert_eq!(
            tip["range"],
            serde_json::json!({ "start": draft.len() - 1, "end": draft.len() }),
            "{draft:?}"
        );
        match position {
            Some(position) => assert_eq!(tip["where"], position, "{draft:?}"),
            None => assert!(
                tip.get("where").is_none(),
                "{draft:?} serialized an absent where: {json}"
            ),
        }
    }
}

#[test]
fn capture_rewrite_json_tip_ranges_are_utf8_byte_offsets() {
    let draft = "Caf\u{e9} run @";
    let json = capture_rewrite_json(draft, Some(draft.len()));
    assert_eq!(json["sigil_alias_tip"]["id"], "route_after_text");
    assert_eq!(
        json["sigil_alias_tip"]["range"],
        serde_json::json!({ "start": 10, "end": 11 })
    );
}

#[test]
fn capture_rewrite_json_sends_an_explicit_null_when_no_tip_is_proved() {
    // The explicit `null` is the capability marker an older bob never sends.
    for (draft, cursor) in [
        ("Buy milk &", 10),
        ("Buy milk", 8),
        ("@", 1),
        ("Do work @file:{0}^ %t\n\nrow", 18),
        ("xx", 1),
        // Under hash_fallback = drop no key would have typed `#` here.
        ("=*fix#", 6),
        ("=#", 2),
        ("[[folder#", 9),
    ] {
        let json = capture_rewrite_json(draft, Some(cursor));
        assert_eq!(json["changed"], false, "{draft:?}: {json}");
        assert_eq!(
            json.get("sigil_alias_tip"),
            Some(&serde_json::Value::Null),
            "{draft:?}: {json}"
        );
    }
}

#[test]
fn capture_rewrite_json_omits_the_tip_without_a_cursor_or_after_a_change() {
    let without_cursor = capture_rewrite_json("Buy milk @", None);
    assert_eq!(without_cursor["changed"], false);
    assert!(
        without_cursor.get("sigil_alias_tip").is_none(),
        "{without_cursor}"
    );

    let changed = capture_rewrite_json("Buy milk .", Some(10));
    assert_eq!(changed["changed"], true);
    assert!(changed.get("sigil_alias_tip").is_none(), "{changed}");

    let absorbed = capture_rewrite_json("Buy milk @dev @@", Some(16));
    assert_eq!(absorbed["changed"], true);
    assert!(absorbed.get("sigil_alias_tip").is_none(), "{absorbed}");
}

#[test]
fn capture_rewrite_json_stamps_the_alias_on_forward_sigil_aliases() {
    for (draft, id, key, sigil) in [
        ("Buy milk .", "route_after_text", ".", "@"),
        ("Buy milk /", "hash_menu", "/", "#"),
        ("Buy milk \\", "directive_menu", "\\", "%"),
        ("@errands.", "block_id_after_route", ".", "^"),
        ("@errands;", "link_after_route", ";", ":"),
        (";", "task_link_item", ";", ":"),
        ("@cash^goog-exit@\n- .", "move_bullet_caret", ".", "^"),
        ("@cash:goog-exit@\n- ;", "move_bullet_colon", ";", ":"),
        ("@cash^goog-exit@\n- ^.", "move_choice", ".", "^"),
        ("@cash,", "parent_task", ",", "+"),
        ("@cash+goog-exit,", "next_parent", ",", "+"),
        ("Fix it =", "relation", "=", "~"),
        ("Fix it ~=", "relation_flip", "=", "~"),
        ("Fix it ~~=", "relation_flip", "=", "~"),
        ("@home .", "dependency", ".", "&"),
        ("Buy milk ;", "blocked_task", ";", "*"),
        ("x=2-", "past_segment", "-", "~"),
        ("xx", "close_complete", "x", "!"),
        ("xs", "close_park", "s", "*"),
    ] {
        let json = capture_rewrite_json(draft, Some(draft.len()));
        assert_eq!(json["rule"], "expand_sigil_alias", "{draft:?}: {json}");
        assert_eq!(
            json["alias"],
            serde_json::json!({ "id": id, "key": key, "sigil": sigil }),
            "{draft:?}: {json}"
        );
        assert!(json.get("sigil_alias_tip").is_none(), "{draft:?}: {json}");
    }
}

#[test]
fn capture_rewrite_json_stamps_no_alias_on_other_rules_or_no_change() {
    for (draft, cursor) in [
        ("Buy milk @dev @@", Some(16)),
        ("Do work @file:id^", Some(17)),
        ("Fix trip ~work:foo |", Some(20)),
        ("Buy milk @dev", None),
        ("Buy milk @", Some(10)),
    ] {
        let json = capture_rewrite_json(draft, cursor);
        assert!(json.get("alias").is_none(), "{draft:?}: {json}");
    }
}

#[test]
fn capture_rewrite_human_output_adds_one_dim_tip_line_when_a_tip_is_proved() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("10")
        .arg("--")
        .arg("Buy milk @")
        .output()
        .expect("run bob capture-rewrite human tip");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    assert_eq!(
        stdout(&output),
        "no rewrite\ntip  next time . \u{2192} @ after a word\n"
    );
    assert_stdout_has_no_ansi(&output);

    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("10")
        .arg("--")
        .arg("Buy milk &")
        .output()
        .expect("run bob capture-rewrite human without a tip");
    assert_success(&output);
    assert_eq!(stdout(&output), "no rewrite\n");
}

#[test]
fn capture_rewrite_human_tip_line_omits_a_missing_position_phrase() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("12")
        .arg("--")
        .arg("Buy milk\n- %")
        .output()
        .expect("run bob capture-rewrite human tip without a phrase");
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "no rewrite\ntip  next time \\ \u{2192} %\n"
    );
}

#[test]
fn capture_rewrite_help_documents_typing_tips() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("--help")
        .output()
        .expect("run bob capture-rewrite help");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("Typing tips"), "{out}");
    assert!(out.contains("sigil_alias_tip"), "{out}");
    assert!(out.contains("'Buy milk @'"), "{out}");
}

fn notebook_alias_vault(temp: &TempDir) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    std::fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("dev.md"),
        "---\ntype: \"[[area]]\"\n---\n# Dev\n",
    );
    write_file(
        &vault.join("dev_tools.md"),
        concat!(
            "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\nsummary: Tools.\n---\n\n",
            "# Developer tools\n\n",
            "## Git\n\n",
            "- Use worktrees for investigations [created::2026-11-08]\n",
        ),
    );
    vault
}

fn rewrite_with_vault_json(
    draft: &str,
    cursor: Option<usize>,
    vault: &std::path::Path,
) -> serde_json::Value {
    let mut command = bob_command();
    command
        .arg("capture-rewrite")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", vault);
    if let Some(cursor) = cursor {
        command.arg("-c").arg(cursor.to_string());
    }
    let output = command
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-rewrite");
    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    serde_json::from_str(stdout(&output).trim()).expect("capture-rewrite JSON")
}

#[test]
fn capture_rewrite_json_expands_slash_to_hash_in_a_notebook_list() {
    let temp = TempDir::new("bob-cli-rewrite-notebook-alias");
    let vault = notebook_alias_vault(&temp);
    let draft = "@dev_tools\n- /";
    let json = rewrite_with_vault_json(draft, Some(draft.len()), &vault);
    assert_eq!(json["changed"], true, "{json}");
    assert_eq!(json["text"], "@dev_tools\n- #", "{json}");
    assert_eq!(json["rule"], "expand_sigil_alias", "{json}");
    assert_eq!(
        json["alias"],
        serde_json::json!({ "id": "hash_menu", "key": "/", "sigil": "#" }),
        "{json}"
    );
}

#[test]
fn capture_rewrite_json_expands_slash_to_hash_in_a_notebook_create_item() {
    let temp = TempDir::new("bob-cli-rewrite-notebook-create-alias");
    let vault = notebook_alias_vault(&temp);
    let draft = "Parent @dev^nvim@ %n\n- /";
    let json = rewrite_with_vault_json(draft, Some(draft.len()), &vault);
    assert_eq!(json["changed"], true, "{json}");
    assert_eq!(json["text"], "Parent @dev^nvim@ %n\n- #", "{json}");
    assert_eq!(json["rule"], "expand_sigil_alias", "{json}");
    assert_eq!(
        json["alias"],
        serde_json::json!({ "id": "hash_menu", "key": "/", "sigil": "#" }),
        "{json}"
    );
}

#[test]
fn capture_rewrite_json_keeps_slash_literal_outside_notebook_bodies() {
    let temp = TempDir::new("bob-cli-rewrite-notebook-alias-negatives");
    let vault = notebook_alias_vault(&temp);
    for draft in [
        // An area task list keeps the drawer grammar: no section menu opens.
        "@dev\n- /",
        // A bullet under a notebook note is opaque note detail.
        "Text @dev_tools\n- /",
        // A fan-out bullet under a notebook task link is note detail too.
        "Text @dev_tools+abc\n- /",
        // A bullet under an `@nb#Section` header is already filed.
        "Note @dev_tools#git\n- /",
    ] {
        // Nested note detail keeps the lexical task-type menu exactly like
        // a typed `#` (it fires convergently); the unit preflight test pins
        // that no vault read happens there.
        let json = rewrite_with_vault_json(draft, Some(draft.len()), &vault);
        assert_eq!(json["changed"], false, "{draft:?}: {json}");
        assert_eq!(json["text"], draft, "{draft:?}: {json}");
    }
}

#[test]
fn capture_rewrite_json_reads_no_files_for_an_unrouted_slash_bullet() {
    let temp = TempDir::new("bob-cli-rewrite-unrouted-alias");
    let missing = temp.path().join("missing-vault");
    let draft = "- /";
    let json = rewrite_with_vault_json(draft, Some(draft.len()), &missing);
    assert_eq!(json["changed"], false, "{json}");
    assert_eq!(json["text"], draft, "{json}");
    assert!(
        !missing.exists(),
        "capture-rewrite must not create the vault directory"
    );
}

#[test]
fn capture_rewrite_json_teaches_slash_for_hash_in_a_notebook_body() {
    let temp = TempDir::new("bob-cli-rewrite-notebook-alias-tip");
    let vault = notebook_alias_vault(&temp);
    let draft = "@dev_tools\n- #";
    let json = rewrite_with_vault_json(draft, Some(draft.len()), &vault);
    assert_eq!(json["changed"], false, "{json}");
    assert_eq!(json["sigil_alias_tip"]["id"], "hash_menu", "{json}");
    assert_eq!(
        json["sigil_alias_tip"]["keys"],
        serde_json::json!(["/"]),
        "{json}"
    );

    // Under a notebook note the forward alias stays literal, so no tip.
    let draft = "Text @dev_tools\n- #";
    let json = rewrite_with_vault_json(draft, Some(draft.len()), &vault);
    assert_eq!(json["changed"], false, "{json}");
    assert_eq!(
        json.get("sigil_alias_tip"),
        Some(&serde_json::Value::Null),
        "{draft:?}: {json}"
    );
}
