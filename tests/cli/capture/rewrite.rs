//! Capture rewrite and complete-and-rewrite.

use crate::support::*;

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
fn capture_parse_does_not_apply_the_separator_toggle() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @file:id^")
        .output()
        .expect("run bob capture-parse unrewritten toggle");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["input"], "Do work @file:id^");
    assert_eq!(json["body"], "Do work");
    assert!(json["block_id"].is_null(), "{json}");
    let codes: Vec<&str> = json["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str())
        .collect();
    assert!(
        codes
            .iter()
            .any(|code| *code == "invalid_pomodoro_block_id"),
        "{json}"
    );
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
    // `=x` is never rewritten, on whole items or link forms.
    for text in [
        "=x",
        "^r:id=x",
        "Text @r:id=x",
        "=x1,3!2",
        "=x0",
        "=x1,",
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
    for text in ["=x\n- 1 moved @@inbox", "=x moved @@inbox"] {
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
