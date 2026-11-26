//! Capture snooze grammar contract (`z:<when>` and `@route:id!`): spans,
//! diagnostics, strict/editor agreement, and executable dry-run intents.

use crate::support::*;
use std::fs;

const SNOOZE_NOW: &str = "2026-11-25 13:00:00";

fn parse_json(draft: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-parse json");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

fn dry_run(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    draft: &str,
) -> std::process::Output {
    bob_command()
        .arg("capture")
        .arg("--dry-run")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", SNOOZE_NOW)
        .output()
        .expect("run bob capture dry-run")
}

fn dry_error(output: &std::process::Output) -> String {
    let json: serde_json::Value = serde_json::from_str(stdout(output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(output))
        });
    assert_eq!(json["ok"], false);
    json["error"]
        .as_str()
        .expect("dry-run error text")
        .to_string()
}

fn snooze_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &vault.join("job.md"),
        "---\ntype: \"[[area]]\"\n---\n\n# Job\n",
    );
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    (temp, vault, day_file)
}

#[test]
fn capture_parse_reports_snooze_spans_for_existing_and_new_tasks() {
    let existing = parse_json("@job:email-dan-eod z:1530");
    assert_eq!(existing["grammar"], 34);
    assert_eq!(existing["mode"], "pomodoro_link");
    assert_eq!(existing["body"], "");
    assert_eq!(existing["route"], "job");
    assert_eq!(existing["block_id"], "email-dan-eod");
    assert_eq!(existing["diagnostics"], serde_json::json!([]));
    assert_eq!(
        existing["spans"],
        serde_json::json!([
            { "start": 0, "end": 4, "kind": "pomodoro_route" },
            { "start": 5, "end": 18, "kind": "pomodoro_block_id" },
            { "start": 19, "end": 25, "kind": "snooze" },
        ])
    );

    let caret = parse_json("^job:email-dan-eod z:1530");
    assert_eq!(caret["mode"], "pomodoro_link");
    assert_eq!(
        caret["spans"],
        serde_json::json!([
            { "start": 0, "end": 4, "kind": "active_task_route" },
            { "start": 5, "end": 18, "kind": "active_task_block_id" },
            { "start": 19, "end": 25, "kind": "snooze" },
        ])
    );

    let new_task = parse_json("Fix sink @home z:19");
    assert_eq!(new_task["mode"], "task");
    assert_eq!(new_task["body"], "Fix sink");
    assert_eq!(new_task["route"], "home");
    assert_eq!(new_task["diagnostics"], serde_json::json!([]));

    let reversed = parse_json("Fix sink z:19 @home");
    assert_eq!(reversed["mode"], "task");
    assert_eq!(reversed["body"], "Fix sink");
    assert_eq!(reversed["route"], "home");
    assert_eq!(
        reversed["spans"],
        serde_json::json!([
            { "start": 9, "end": 13, "kind": "snooze" },
            { "start": 14, "end": 19, "kind": "route" },
        ])
    );

    let authored = parse_json("Fix sink @home:fix-sink z:19");
    assert_eq!(authored["mode"], "pomodoro_task");
    assert_eq!(authored["body"], "Fix sink");
    assert_eq!(authored["block_id"], "fix-sink");
    assert_eq!(authored["diagnostics"], serde_json::json!([]));
}

#[test]
fn capture_parse_reports_drop_span_and_keeps_the_toggle_distinct() {
    let drop = parse_json("@job:email-dan-eod!");
    assert_eq!(drop["grammar"], 34);
    assert_eq!(drop["mode"], "pomodoro_link");
    assert_eq!(drop["body"], "");
    assert_eq!(drop["diagnostics"], serde_json::json!([]));
    assert_eq!(
        drop["spans"],
        serde_json::json!([
            { "start": 0, "end": 4, "kind": "pomodoro_route" },
            { "start": 5, "end": 18, "kind": "pomodoro_block_id" },
            { "start": 18, "end": 19, "kind": "snooze" },
        ])
    );

    let toggle = parse_json("@job+email-dan!");
    assert_eq!(toggle["mode"], "task_toggle");
    assert_eq!(toggle["diagnostics"], serde_json::json!([]));
    assert!(
        toggle["spans"]
            .as_array()
            .expect("toggle spans")
            .iter()
            .all(|span| span["kind"] != "snooze"),
        "toggle carries no snooze span: {toggle}"
    );
}

#[test]
fn capture_parse_reports_numeric_route_drop_like_strict_execution() {
    for raw in ["@20261125:here!", "@2026:here!"] {
        let drop = parse_json(raw);
        assert_eq!(drop["grammar"], 34, "{raw}");
        assert_eq!(drop["mode"], "pomodoro_link", "{raw}");
        assert_eq!(drop["body"], "", "{raw}");
        assert_eq!(
            drop["route"],
            raw[1..raw.len() - 6].to_lowercase(),
            "{raw}"
        );
        assert_eq!(drop["block_id"], "here", "{raw}");
        assert_eq!(drop["diagnostics"], serde_json::json!([]), "{raw}");
        let route_end = 1 + drop["route"].as_str().expect("route").len();
        let block_end = raw.len() - 1;
        assert_eq!(
            drop["spans"],
            serde_json::json!([
                { "start": 0, "end": route_end, "kind": "pomodoro_route" },
                { "start": route_end + 1, "end": block_end, "kind": "pomodoro_block_id" },
                { "start": block_end, "end": raw.len(), "kind": "snooze" },
            ]),
            "{raw}"
        );
    }
}

#[test]
fn capture_dry_run_executes_recognized_snooze_intents_without_writing() {
    let (_temp, vault, day_file) =
        snooze_vault("bob-cli-capture-snooze-preview");
    write_file(
        &vault.join("job.md"),
        "- [*] #task Email Dan ^email-dan-eod\n",
    );
    write_file(&vault.join("home.md"), "# Home\n");
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n\n## Snoozed\n\n- **1500** [[job#^email-dan-eod]]\n");
    let before = fs::read_to_string(&day_file).expect("read day");
    for draft in [
        "@job:email-dan-eod z:1530",
        "^job:email-dan-eod z:1530",
        "Fix sink @home z:19",
        "Fix sink @home:fix-sink z:19",
        "@job:email-dan-eod!",
    ] {
        let output = dry_run(&vault, &day_file, draft);
        assert_success(&output);
    }
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), before);
}

#[test]
fn capture_dry_run_and_parse_agree_on_snooze_refusals() {
    let (_temp, vault, day_file) =
        snooze_vault("bob-cli-capture-snooze-refusals");
    // Each draft must fail strict execution with the exact message
    // `capture-parse` reports, and leave the vault untouched.
    let before = fs::read_to_string(&day_file).expect("read day");
    let drafts = [
        "@job:email-dan-eod z:",
        "@job:email-dan-eod s:2 z:1530",
        "Fix sink @home s:2 z:19",
        "@job:email-dan-eod $2 z:1530",
        "@job:email-dan-eod#night z:1530",
        "@job:email-dan-eod=3 z:1530",
        "^job:email-dan-eod=3 z:1530",
        "@job:email-dan-eod=x z:1530",
        "@job:email-dan-eod z:1530\n- kid notes",
    ];
    for draft in drafts {
        let parsed = parse_json(draft);
        let diagnostics =
            parsed["diagnostics"].as_array().expect("diagnostics array");
        assert_eq!(diagnostics.len(), 1, "{draft}: {parsed}");
        let message = diagnostics[0]["message"]
            .as_str()
            .expect("diagnostic message");

        let output = dry_run(&vault, &day_file, draft);
        let error = dry_error(&output);
        let suffix = error
            .strip_prefix("capture item 1 starting on line 1: ")
            .unwrap_or(&error);
        assert_eq!(suffix, message, "{draft}");
    }
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        before,
        "refused intents stage nothing"
    );
}

#[test]
fn capture_parse_reports_incomplete_and_duplicate_snooze() {
    let bare = parse_json("@job:email-dan-eod z:");
    assert_eq!(bare["mode"], "pomodoro_link");
    assert_eq!(bare["diagnostics"].as_array().expect("array").len(), 1);
    assert_eq!(bare["diagnostics"][0]["code"], "incomplete_snooze");
    assert_eq!(
        bare["spans"][2],
        serde_json::json!({ "start": 19, "end": 21, "kind": "snooze" })
    );

    // A duplicate stops terminal parsing: the last token wins and the
    // earlier one stays literal body text.
    let duplicate = parse_json("Fix @home z:19 z:20");
    assert_eq!(duplicate["body"], "Fix @home z:19");
    assert!(duplicate["route"].is_null());
    assert_eq!(duplicate["diagnostics"], serde_json::json!([]));
    assert_eq!(
        duplicate["spans"],
        serde_json::json!([{ "start": 15, "end": 19, "kind": "snooze" }])
    );
}
