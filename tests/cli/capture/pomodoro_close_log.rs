//! Pomodoro close `=x` Work Log tail integration tests.

use super::pomodoro_close::{
    close_worked_vault, run_close_expect_error, run_close_json,
};
use crate::support::*;
use std::fs;

fn parse_json(text: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .output()
        .expect("run capture-parse");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("parse JSON")
}

#[test]
fn capture_pomodoro_close_log_worked_table() {
    // `=x 1 wired the lexer`: entry under link 1, then the unchanged close
    // writes it to task 1's Work Log.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-1");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x 1 wired the lexer"],
    );
    let close = &json["pomodoro_close"];
    assert_eq!(
        close["log"],
        serde_json::json!([{ "index": 1, "text": "wired the lexer" }])
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(day_after.contains("\t\t- wired the lexer\n"), "{day_after}");
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("read bob");
    assert!(
        bob_after.contains("*2026-09-28* — wired the lexer"),
        "{bob_after}"
    );
    let tasks = close["tasks"].as_array().expect("tasks");
    let row1 = tasks.iter().find(|task| task["index"] == 1).expect("row 1");
    assert!(
        row1["typed_work_log"]
            .as_array()
            .expect("typed")
            .iter()
            .any(|entry| entry
                .as_str()
                .unwrap_or_default()
                .contains("wired the lexer")),
        "{row1}"
    );

    // `=x1,2 2 sketched the URL parser`: both links in progress, entry
    // under link 2.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-2");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x1,2 2 sketched the URL parser"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 2, "text": "sketched the URL parser" }])
    );

    // `=x 1 wrote docs 1 opened the PR`: two sub-bullets under link 1.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-3");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x 1 wrote docs 1 opened the PR"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([
            { "index": 1, "text": "wrote docs" },
            { "index": 1, "text": "opened the PR" },
        ])
    );

    // `=x1 1 fixed 3 bugs`: 3 is not loggable under `=x1`, so it stays text.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-4");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x1 1 fixed 3 bugs"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "fixed 3 bugs" }])
    );

    // Escapes: `\3` writes `3`, `\=` writes `=`.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-5");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x 1 fixed \\3 bugs"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "fixed 3 bugs" }])
    );
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-6");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x 1 foo \\="],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "foo =" }])
    );

    // Literal text: no global destination.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-7");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x 1 moved @@inbox"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "moved @@inbox" }])
    );
}

#[test]
fn capture_pomodoro_close_log_complete_target() {
    // `=x1!2 2 shipped it`: 2 completes and its entry lands in the
    // completed task.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-c");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x1!2 2 shipped it"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 2, "text": "shipped it" }])
    );
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("read bob");
    assert!(bob_after.contains("shipped it"), "{bob_after}");
}

#[test]
fn capture_pomodoro_close_log_execution_errors() {
    // Out of range at execution suggests the escape.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-e1");
    let before = fs::read_to_string(&day_file).expect("read");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x 1 fixed 3 bugs"],
    );
    // With no `<N>` both 1 and 3 are loggable, so 3 is parsed as a second
    // entry and fails at execution as out of range.
    assert!(
        error.contains("task 3")
            || error.contains("out of range")
            || error.contains("numbered Task Links"),
        "{error}"
    );
    assert!(error.contains("\\3") || error.contains("write"), "{error}");
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // Deferred target without `<N>`.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-e2");
    let before = fs::read_to_string(&day_file).expect("read");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x 2 looked at it"],
    );
    assert!(error.contains("deferred"), "{error}");
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // Lexical: not worked, dropped, tail-start, block link, fence, empty.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-e3");
    let before = fs::read_to_string(&day_file).expect("read");
    for (args, phrase) in [
        (vec!["=x1 2 foo"], "isn't worked by `=x1`"),
        (vec!["=x~2 2 foo"], "is dropped by `~2`"),
        (vec!["=x more"], "write a task number"),
        (
            vec!["=x 1 see [[bob#^web-capture]]"],
            "can't contain the block link",
        ),
        (vec!["=x 1 ```rust"], "can't start with a code fence"),
        (vec!["=x 1 2 foo"], "type Work Log text after task 1"),
    ] {
        let error = run_close_expect_error(
            &vault,
            &day_file,
            "2026-09-28 09:37:00",
            &args,
        );
        assert!(error.contains(phrase), "{args:?}: {error}");
    }
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // Dangling index stays incomplete.
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x 1"],
    );
    assert!(
        error.contains("is incomplete: type the Work Log text after task 1"),
        "{error}"
    );
}

#[test]
fn capture_pomodoro_close_log_dry_run_and_blocks() {
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-dr");
    let before_day = fs::read_to_string(&day_file).expect("read");
    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x 1 wired the lexer"],
    );
    let day_after = fs::read_to_string(&day_file).expect("read");
    assert_pomodoro_blocks_cover_changes(&before_day, &day_after, &json);
    let log = if json.get("pomodoro_close").is_some() {
        &json["pomodoro_close"]["log"]
    } else {
        &json["captures"][0]["pomodoro_close"]["log"]
    };
    assert_eq!(
        log,
        &serde_json::json!([{ "index": 1, "text": "wired the lexer" }])
    );
}

#[test]
fn capture_pomodoro_close_log_batch_rollback() {
    // An entry error rolls the whole batch back.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-rb");
    let before = fs::read_to_string(&day_file).expect("read");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=x 1 wired the lexer")
        .arg("=x1 2 foo")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("run batch");
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
}

#[test]
fn capture_pomodoro_close_log_chains() {
    // Leading operator runs first.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-ch1");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("-2 =x 1 wired the lexer")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("run chain");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["captures"][0]["kind"], "pomodoro_adjust");
    assert_eq!(json["captures"][1]["kind"], "pomodoro_close");
    assert_eq!(
        json["captures"][1]["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "wired the lexer" }])
    );

    // Trailing start run closes then starts.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-ch2");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=x 1 wired the lexer =")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("run chain");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["captures"][0]["kind"], "pomodoro_close");
    assert_eq!(json["captures"][1]["kind"], "pomodoro_start");

    // A trailing run not beginning with a start stays text.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-ch3");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x 1 bumped the version +1"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "bumped the version +1" }])
    );
}

#[test]
fn capture_parse_pomodoro_close_log_protocol() {
    // Valid tail: spec plus index span.
    let value = parse_json("=x 1 wired the lexer");
    assert_eq!(value["mode"], "pomodoro_close");
    assert_eq!(
        value["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "wired the lexer" }])
    );
    assert!(
        value["spans"]
            .as_array()
            .expect("spans")
            .iter()
            .any(|span| {
                span["kind"] == "pomodoro_close_log_index"
                    && span["start"] == 3
                    && span["end"] == 4
            }),
        "{value}"
    );

    // Chain: two items, close range covers its tail.
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=x 1 wired the lexer =")
        .output()
        .expect("parse chain");
    assert_success(&output);
    let chain: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    let items = chain["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["mode"], "pomodoro_close");
    assert_eq!(items[1]["mode"], "pomodoro_start");

    // Incomplete: dangling index needs log text with a placeholder.
    let incomplete = parse_json("=x 1");
    assert_eq!(incomplete["mode"], "incomplete");
    assert_eq!(
        incomplete["needs"],
        serde_json::json!(["pomodoro_close_log_text"])
    );
    assert!(
        incomplete["spans"]
            .as_array()
            .expect("spans")
            .iter()
            .any(|span| {
                span["kind"] == "interactive_placeholder"
                    && span["start"] == 3
                    && span["end"] == 4
            }),
        "{incomplete}"
    );

    // Invalid tails report precise diagnostics.
    for (text, phrase) in [
        ("=x more", "write a task number"),
        ("=x1 2 foo", "isn't worked by"),
        (
            "=x 1 see [[bob#^web-capture]]",
            "can't contain the block link",
        ),
    ] {
        let value = parse_json(text);
        assert_eq!(
            value["diagnostics"][0]["code"], "invalid_pomodoro_close",
            "{text}: {value}"
        );
        assert!(
            value["diagnostics"][0]["message"]
                .as_str()
                .unwrap_or_default()
                .contains(phrase),
            "{text}: {value}"
        );
    }
}
