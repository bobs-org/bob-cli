//! Capture parse except pomodoro protocol.

use crate::support::*;
use std::fs;

#[test]
fn capture_parse_human_output_is_plain_and_concise() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("Call bank @Cash+")
        .output()
        .expect("run bob capture-parse human");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-parse stderr:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert_text_order(
        &out,
        &[
            "incomplete",
            "body",
            "Call bank",
            "route",
            "cash",
            "needs",
            "task",
            "sub_bullet_route",
            "interactive_placeholder",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_parse_json_output_is_stable_and_parseable() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Call bank @Cash+")
        .output()
        .expect("run bob capture-parse json");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-parse stderr:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], "Call bank @Cash+");
    assert_eq!(json["body"], "Call bank");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["route"], "cash");
    assert!(json["section"].is_null());
    assert!(json["block_id"].is_null());
    assert_eq!(json["needs"], serde_json::json!(["task"]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 10, "end": 15, "kind": "sub_bullet_route" },
            { "start": 15, "end": 16, "kind": "interactive_placeholder" },
        ])
    );
    assert_eq!(json["diagnostics"], serde_json::json!([]));
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_parse_json_reports_batch_items_with_global_ranges() {
    let draft = "First @cash\n- child\n\nSecond @notes#Ideas";
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg(draft)
        .output()
        .expect("run batch capture-parse json");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    let first_end = draft.find("\n\n").expect("separator");
    let second_start = first_end + 2;

    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["body"], "First");
    assert_eq!(json["route"], "cash");
    assert_eq!(json["sub_bullets"], serde_json::json!(["child"]));
    assert_eq!(json["items"].as_array().expect("items").len(), 2);
    assert_eq!(json["items"][0]["index"], 1);
    assert_eq!(
        json["items"][0]["range"],
        serde_json::json!({ "start": 0, "end": first_end })
    );
    assert_eq!(json["items"][0]["line_start"], 1);
    assert_eq!(json["items"][0]["line_end"], 2);
    assert_eq!(json["items"][0]["body"], "First");
    assert_eq!(json["items"][0]["route"], "cash");
    assert_eq!(
        json["items"][0]["sub_bullets"],
        serde_json::json!(["child"])
    );
    assert_eq!(json["items"][1]["index"], 2);
    assert_eq!(
        json["items"][1]["range"],
        serde_json::json!({ "start": second_start, "end": draft.len() })
    );
    assert_eq!(json["items"][1]["line_start"], 4);
    assert_eq!(json["items"][1]["line_end"], 4);
    assert_eq!(json["items"][1]["body"], "Second");
    assert_eq!(json["items"][1]["mode"], "bullet");
    assert_eq!(json["items"][1]["route"], "notes");
    assert_eq!(json["items"][1]["section"], "Ideas");
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 6, "end": 11, "kind": "route" },
            { "start": 28, "end": 34, "kind": "route" },
            { "start": 35, "end": 40, "kind": "section" },
        ])
    );
}

#[test]
fn capture_parse_json_reports_task_block_id_marker_spans_and_needs() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @Dev^")
        .output()
        .expect("run bob capture-parse task block ID marker");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["route"], "dev");
    assert!(json["block_id"].is_null());
    assert_eq!(json["needs"], serde_json::json!(["block_id"]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 8, "end": 12, "kind": "task_block_id_route" },
            { "start": 12, "end": 13, "kind": "interactive_placeholder" },
        ])
    );

    let invalid = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @Dev^bad.id")
        .output()
        .expect("run invalid task block ID parse");
    assert_success(&invalid);
    let json: serde_json::Value = serde_json::from_str(stdout(&invalid).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "task");
    assert_eq!(json["diagnostics"][0]["code"], "invalid_task_block_id");
}

#[test]
fn capture_parse_json_reports_project_note_markers() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @cash^goog-exit+")
        .output()
        .expect("run bob capture-parse project-note caret marker");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "project_note");
    assert_eq!(json["body"], "Do work");
    assert_eq!(json["route"], "cash");
    assert_eq!(json["block_id"], "goog-exit");
    assert!(json["section"].is_null());
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 8, "end": 13, "kind": "task_block_id_route" },
            { "start": 14, "end": 23, "kind": "task_block_id" },
            { "start": 23, "end": 24, "kind": "project_note_marker" },
        ])
    );
    assert!(json["diagnostics"].as_array().unwrap().is_empty());

    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @cash^goog-exit+#bugs")
        .output()
        .expect("run bob capture-parse project-note pomodoro marker");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "project_note");
    assert_eq!(json["body"], "Do work");
    assert_eq!(json["route"], "cash");
    assert_eq!(json["block_id"], "goog-exit");
    assert_eq!(json["section"], "bugs");
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 8, "end": 13, "kind": "task_block_id_route" },
            { "start": 14, "end": 23, "kind": "task_block_id" },
            { "start": 23, "end": 24, "kind": "project_note_marker" },
            { "start": 25, "end": 29, "kind": "pomodoro_name" },
        ])
    );
    assert_eq!(
        json["diagnostics"],
        serde_json::json!([
            {
                "severity": "error",
                "code": "unused_project_note_pomodoro",
                "message": "`#bugs` picks the Pomodoro for ` :<id>` task links, but no task bullet ends with ` :<id>`; add one or remove `#bugs`",
                "range": [24, 29],
            }
        ])
    );

    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @^focus-123+")
        .output()
        .expect("run bob capture-parse routeless project-note marker");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "project_note");
    assert!(json["route"].is_null());
    assert_eq!(json["block_id"], "focus-123");
    assert_eq!(json["needs"], serde_json::json!(["route"]));

    let invalid = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @cash:goog-exit+#bugs")
        .output()
        .expect("run retired colon project-note marker");
    assert_success(&invalid);
    let json: serde_json::Value = serde_json::from_str(stdout(&invalid).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "task");
    assert_eq!(
        json["diagnostics"][0]["code"],
        "retired_project_note_marker"
    );
    assert!(
        json["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| {
                message.contains("is retired")
                    && message.contains("@cash^goog-exit+#bugs")
            }),
        "{json}"
    );

    let global = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@@cash^goog-exit+")
        .output()
        .expect("run project-note global declaration parse");
    assert_success(&global);
    let json: serde_json::Value = serde_json::from_str(stdout(&global).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["diagnostics"][0]["code"], "invalid_global_destination");
}

#[test]
fn capture_parse_json_reports_retired_double_colon_as_migration_guidance() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Follow up @file::new-id")
        .output()
        .expect("run bob capture-parse retired double colon");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "task");
    assert_eq!(
        json["diagnostics"][0]["code"],
        "retired_task_block_id_marker"
    );
    assert!(
        json["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| {
                message.contains("'@<route>::<block-id>' is no longer accepted")
                    && message.contains("@<route>^<block-id>")
            }),
        "{json}"
    );
}

#[test]
fn capture_parse_json_reports_pomodoro_note_mode_and_span() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note #")
        .output()
        .expect("run bob capture-parse on a bare #");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "pomodoro_note");
    assert_eq!(json["body"], "note");
    assert!(json["route"].is_null());
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["spans"],
        serde_json::json!([{ "start": 5, "end": 6, "kind": "pomodoro_note" }])
    );
    assert!(json["diagnostics"].as_array().unwrap().is_empty());

    let conflict = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note # @work")
        .output()
        .expect("run bob capture-parse on a conflicting # and @route");

    assert_success(&conflict);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&conflict).trim())
            .expect("capture-parse JSON");
    assert_eq!(json["diagnostics"][0]["code"], "pomodoro_note_conflict");
}

#[test]
fn capture_parse_json_reports_wikilink_semantic_spans() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("See [[sase#Design|Spec]]")
        .output()
        .expect("run bob capture-parse wikilink json");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["body"], "See [[sase#Design|Spec]]");
    assert_eq!(
        json["spans"]
            .as_array()
            .expect("spans array")
            .iter()
            .map(|span| span["kind"].as_str().expect("kind"))
            .collect::<Vec<_>>(),
        vec![
            "wikilink_delimiter",
            "wikilink_target",
            "wikilink_delimiter",
            "wikilink_heading",
            "wikilink_delimiter",
            "wikilink_alias",
            "wikilink_delimiter",
        ]
    );
}

#[test]
fn capture_parse_reports_diagnostics_without_failing() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note @dev+bad.id")
        .output()
        .expect("run bob capture-parse diagnostic");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["mode"], "task");
    assert_eq!(
        json["diagnostics"][0]["code"],
        "invalid_sub_bullet_block_id"
    );
    assert_eq!(json["diagnostics"][0]["severity"], "error");
    assert_eq!(json["diagnostics"][0]["range"], serde_json::json!([5, 16]));
}

#[test]
fn capture_parse_reads_one_stdin_line_when_text_is_omitted() {
    let output = run_with_stdin(
        bob_command().arg("capture-parse").arg("-f").arg("json"),
        "jot idea @notes#Ideas\n",
    );

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "bullet");
    assert_eq!(json["route"], "notes");
    assert_eq!(json["section"], "Ideas");
    assert_eq!(json["body"], "jot idea");
}

#[test]
fn capture_parse_missing_text_is_a_usage_error() {
    let human = run_with_stdin(bob_command().arg("capture-parse"), "   \n");
    assert_eq!(human.status.code(), Some(2));
    assert!(
        stdout(&human).is_empty()
            && stderr(&human).contains("task text is required"),
        "unexpected capture-parse missing-text output:\n{}",
        format_output(&human)
    );

    let json_output = run_with_stdin(
        bob_command().arg("capture-parse").arg("-f").arg("json"),
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
            .expect("capture-parse failure JSON");
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .expect("error string")
            .contains("task text is required"),
        "unexpected capture-parse failure JSON: {json}"
    );
}

#[test]
fn capture_parse_reports_utf8_byte_offsets() {
    let text = "caf\u{e9} run \u{1f680} @Cash+goog-exit";
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .output()
        .expect("run bob capture-parse utf8");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "sub_bullet");
    assert_eq!(json["body"], "caf\u{e9} run \u{1f680}");
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 15, "end": 20, "kind": "sub_bullet_route" },
            { "start": 21, "end": 30, "kind": "sub_bullet_block_id" },
        ])
    );
    assert_eq!(&text[15..20], "@Cash");
    assert_eq!(&text[21..30], "goog-exit");
}

#[test]
fn capture_parse_json_reports_sub_bullet_section_and_incomplete_needs() {
    let complete = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Postgres 17 minimum @foo+bar#req")
        .output()
        .expect("run complete three-component parse");
    assert_success(&complete);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&complete).trim()).expect("json");
    assert_eq!(json["mode"], "sub_bullet");
    assert_eq!(json["body"], "Postgres 17 minimum");
    assert_eq!(json["route"], "foo");
    assert_eq!(json["block_id"], "bar");
    assert_eq!(json["section"], "req");
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 20, "end": 24, "kind": "sub_bullet_route" },
            { "start": 25, "end": 28, "kind": "sub_bullet_block_id" },
            { "start": 29, "end": 32, "kind": "sub_bullet_section" },
        ])
    );

    let incomplete = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Postgres 17 minimum @foo+bar#")
        .output()
        .expect("run incomplete section parse");
    assert_success(&incomplete);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&incomplete).trim()).expect("json");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["route"], "foo");
    assert_eq!(json["block_id"], "bar");
    assert!(json["section"].is_null());
    assert_eq!(json["needs"], serde_json::json!(["task_section"]));

    let needs_task = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note @foo+#req")
        .output()
        .expect("run empty block-id parse");
    assert_success(&needs_task);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&needs_task).trim()).expect("json");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["needs"], serde_json::json!(["task"]));
    assert_eq!(json["section"], "req");

    let both = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note @foo+#")
        .output()
        .expect("run empty block-id and section parse");
    assert_success(&both);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&both).trim()).expect("json");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["needs"], serde_json::json!(["task", "task_section"]));

    let invalid = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note @foo+bar#bad_id")
        .output()
        .expect("run invalid section parse");
    assert_success(&invalid);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&invalid).trim()).expect("json");
    assert_eq!(json["diagnostics"][0]["code"], "invalid_sub_bullet_section");

    let note_bullet = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Some note @foo#Ideas")
        .output()
        .expect("run note-bullet parse");
    assert_success(&note_bullet);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&note_bullet).trim()).expect("json");
    assert_eq!(json["mode"], "bullet");
    assert_eq!(json["section"], "Ideas");
    assert!(json["block_id"].is_null());

    let pomodoro_note = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("remembered to bump the timeout #")
        .output()
        .expect("run bare hash parse");
    assert_success(&pomodoro_note);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&pomodoro_note).trim()).expect("json");
    assert_eq!(json["mode"], "pomodoro_note");
}

#[test]
fn capture_parse_never_touches_the_vault_or_clipboard() {
    let temp = TempDir::new("bob-cli-capture-parse-no-io");
    let missing = temp.path().join("missing-vault");
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("save this %build_log @dev:focus-1")
        .env("BOB_DIR", &missing)
        .env("BOB_CLIPBOARD_CMD", "/nonexistent/clipboard-command")
        .output()
        .expect("run bob capture-parse without a vault");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-parse stderr:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["mode"], "pomodoro_task");
    assert_eq!(json["route"], "dev");
    assert_eq!(json["block_id"], "focus-1");
    assert_eq!(json["body"], "save this");
    assert!(
        !missing.exists(),
        "capture-parse must not create the vault directory"
    );

    // Contrast: the same input makes `bob capture` reach for the clipboard.
    let captured = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&missing)
        .arg("--")
        .arg("save this %build_log @dev:focus-1")
        .env("BOB_CLIPBOARD_CMD", "/nonexistent/clipboard-command")
        .output()
        .expect("run bob capture without a vault");
    assert!(
        !captured.status.success(),
        "expected bob capture to fail without clipboard access:\n{}",
        format_output(&captured)
    );
}

#[test]
fn capture_parse_json_reports_explicit_toggle_span_and_plain_ensure_next() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit!")
        .output()
        .expect("run explicit-toggle parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["mode"], "task_toggle");
    assert_eq!(json["body"], "");
    assert_eq!(json["route"], "cash");
    assert_eq!(json["block_id"], "goog-exit");
    assert!(json["section"].is_null());
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 0, "end": 5, "kind": "task_toggle_route" },
            { "start": 6, "end": 15, "kind": "task_toggle_block_id" },
            { "start": 15, "end": 16, "kind": "task_toggle_explicit_toggle" },
        ])
    );
    assert!(json["diagnostics"].as_array().unwrap().is_empty());

    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit")
        .output()
        .expect("run plain ensure-Next parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["mode"], "task_toggle");
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 0, "end": 5, "kind": "task_toggle_route" },
            { "start": 6, "end": 15, "kind": "task_toggle_block_id" },
        ])
    );

    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit#deep+work")
        .output()
        .expect("run named ensure-Next parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["mode"], "task_toggle");
    assert_eq!(json["section"], "deep+work");
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 0, "end": 5, "kind": "task_toggle_route" },
            { "start": 6, "end": 15, "kind": "task_toggle_block_id" },
            { "start": 16, "end": 25, "kind": "task_toggle_pomodoro_name" },
        ])
    );
}

#[test]
fn capture_parse_named_pomodoro_reports_incomplete_need() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@dev:some-id#")
        .output()
        .expect("run capture-parse named pomodoro");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["route"], "dev");
    assert_eq!(json["block_id"], "some-id");
    assert_eq!(json["needs"][0], "pomodoro_name");
}

#[test]
fn capture_parse_reports_global_destination_metadata() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("First task\n\nSecond @@foo\n\nThird @bar")
        .output()
        .expect("run global capture-parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["route"], "foo");
    assert_eq!(json["global_destination"]["route"], "foo");
    assert_eq!(json["global_destination"]["mode"], "task");
    assert_eq!(json["global_destination"]["line"], 3);
    assert_eq!(json["items"][0]["route"], "foo");
    assert_eq!(json["items"][1]["route"], "foo");
    assert_eq!(json["items"][1]["body"], "Second");
    assert_eq!(json["items"][2]["route"], "bar");
    assert_eq!(json["spans"][0]["kind"], "global_route");
}

#[test]
fn capture_parse_reports_duplicate_global_destination_diagnostics() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("@@foo\nFirst @@bar\n\nSecond @@baz")
        .output()
        .expect("run duplicate global capture-parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["global_destination"]["route"], "foo");
    let diagnostics = json["diagnostics"].as_array().expect("diagnostics");
    let duplicate_diagnostics: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic["code"] == "duplicate_global_destination"
        })
        .collect();
    assert_eq!(duplicate_diagnostics.len(), 2, "{json}");
    assert_eq!(duplicate_diagnostics[0]["severity"], "error");
    assert!(duplicate_diagnostics[0]["message"]
        .as_str()
        .unwrap()
        .contains("line 2"));
    assert!(duplicate_diagnostics[1]["message"]
        .as_str()
        .unwrap()
        .contains("line 4"));
}

#[test]
fn capture_parse_and_capture_report_shadowed_global_destination_warnings() {
    let draft = "Buy milk @dev @@groceries\n\nOther";

    let parse_output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg(draft)
        .output()
        .expect("run shadowed global capture-parse");
    assert_success(&parse_output);
    let parse_json: serde_json::Value =
        serde_json::from_str(stdout(&parse_output).trim()).expect("parse JSON");
    assert_eq!(parse_json["items"][0]["route"], "dev");
    assert_eq!(parse_json["items"][1]["route"], "groceries");
    let diagnostic = parse_json["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|diagnostic| diagnostic["code"] == "global_destination_shadowed")
        .expect("shadow warning");
    assert_eq!(diagnostic["severity"], "warning");
    assert!(diagnostic["message"].as_str().unwrap().contains("@dev"));
    assert!(diagnostic["message"]
        .as_str()
        .unwrap()
        .contains("@@groceries"));

    let temp = TempDir::new("bob-cli-capture-global-shadowed");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let json_output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run shadowed global capture json");
    assert_success(&json_output);
    assert!(
        stderr(&json_output).is_empty(),
        "json capture should keep warnings in JSON:\n{}",
        format_output(&json_output)
    );
    let capture_json: serde_json::Value =
        serde_json::from_str(stdout(&json_output).trim())
            .expect("capture JSON");
    assert_eq!(capture_json["warnings"][0], diagnostic["message"]);

    let human_output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run shadowed global capture human");
    assert_success(&human_output);
    assert!(stderr(&human_output).contains("warning"));
    assert!(stderr(&human_output).contains("@@groceries"));
}

#[test]
fn capture_parse_reports_sub_bullets_for_a_multiline_draft() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("@work parent line\n- first child\n- second child @work")
        .output()
        .expect("run multiline capture-parse");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["body"], "parent line");
    assert_eq!(json["route"], "work");
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!(["first child", "second child"])
    );
    assert_eq!(json["sub_bullet_depths"], serde_json::json!([1, 1]));
}

#[test]
fn capture_parse_reports_nested_sub_bullets_and_depths() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("parent line\n- first child\n  - first detail @work\n- second child")
        .output()
        .expect("run nested multiline capture-parse");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["body"], "parent line");
    assert_eq!(json["route"], "work");
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!(["first child", "first detail", "second child"])
    );
    assert_eq!(json["sub_bullet_depths"], serde_json::json!([1, 2, 1]));
}

#[test]
fn capture_parse_reports_orphaned_nested_bullet_diagnostic() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("parent line\n  - orphan @work")
        .output()
        .expect("run orphaned nested capture-parse");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert!(json.get("sub_bullets").is_none(), "{json}");
    assert!(json.get("sub_bullet_depths").is_none(), "{json}");
    assert_eq!(json["route"], serde_json::Value::Null);
    assert_eq!(json["diagnostics"][0]["code"], "orphaned_nested_bullet");
}

#[test]
fn capture_parse_reports_start_suffix_spans_and_batch_offsets() {
    let parse = |text: &str| {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run capture-parse");
        assert_success(&output);
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("capture-parse JSON")
    };

    let named = parse("Do work @sase:outline#deep=-2");
    assert_eq!(named["schema_version"], 1);
    assert_eq!(named["mode"], "pomodoro_task");
    assert_eq!(named["section"], "deep");
    assert_eq!(
        named["pomodoro_start"],
        serde_json::json!({
            "raw": "-2",
            "duration_units": 5,
            "offset_units": 2,
        })
    );
    assert_eq!(
        named["spans"],
        serde_json::json!([
            { "start": 8, "end": 13, "kind": "pomodoro_route" },
            { "start": 14, "end": 21, "kind": "pomodoro_block_id" },
            { "start": 22, "end": 26, "kind": "pomodoro_name" },
            { "start": 26, "end": 29, "kind": "pomodoro_start" },
        ])
    );

    let block_only = parse("Do work @sase:outline=3");
    assert_eq!(
        block_only["pomodoro_start"],
        serde_json::json!({
            "raw": "3",
            "duration_units": 3,
            "offset_units": 0,
        })
    );

    // A partial marker still typing its block ID already reports the start.
    let partial = parse("Do work @sase:=3");
    assert_eq!(partial["mode"], "incomplete");
    assert_eq!(partial["needs"], serde_json::json!(["pomodoro_id"]));
    assert_eq!(
        partial["pomodoro_start"],
        serde_json::json!({
            "raw": "3",
            "duration_units": 3,
            "offset_units": 0,
        })
    );

    let invalid = parse("Do work @sase:outline=abc");
    assert!(invalid.get("pomodoro_start").is_none(), "{invalid}");
    assert_eq!(invalid["diagnostics"][0]["code"], "invalid_pomodoro_start");

    // Older marker shapes keep the version-1 shape with no start field.
    let plain = parse("Do work @dev:id#bugs");
    assert!(plain.get("pomodoro_start").is_none(), "{plain}");

    let batch = parse("First @sase:one=3\n\nSecond @sase:two#deep=-");
    assert_eq!(batch["items"].as_array().expect("items").len(), 2);
    assert_eq!(
        batch["items"][0]["pomodoro_start"],
        serde_json::json!({
            "raw": "3",
            "duration_units": 3,
            "offset_units": 0,
        })
    );
    assert_eq!(
        batch["items"][1]["pomodoro_start"],
        serde_json::json!({
            "raw": "-",
            "duration_units": 5,
            "offset_units": 1,
        })
    );
}

#[test]
fn capture_parse_human_reports_start_suffix() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("Do work @sase:outline=3")
        .output()
        .expect("run capture-parse human");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("start"), "{out}");
    assert!(out.contains("=3 (15m, offset 0u)"), "{out}");
    assert!(out.contains("pomodoro_start"), "{out}");
    assert_stdout_has_no_ansi(&output);

    let plain = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("Do work @dev:id#bugs")
        .output()
        .expect("run plain capture-parse human");
    assert_success(&plain);
    assert!(!stdout(&plain).contains("start"), "{}", stdout(&plain));
}
