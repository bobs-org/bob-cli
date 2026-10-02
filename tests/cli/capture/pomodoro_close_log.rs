//! Pomodoro close `=x` Work Log bullet integration tests.
//!
//! Work Log entries are child bullets under the close (`- [<n>] <text>`
//! with two-space `  - <detail>` details). Unnumbered bullets log in order
//! to the close's worked tasks; a leading number is always a task number.

use super::pomodoro_close::{
    close_worked_vault, run_close_expect_error, run_close_json,
};
use crate::support::*;
use std::fs;
use std::path::PathBuf;

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
    // `=x` plus `- 1 wired the lexer`: the entry lands under link 1, then
    // the unchanged close writes it to task 1's Work Log.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-1");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x\n- 1 wired the lexer"],
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

    // `=x1,2` plus an entry with a nested detail plus an entry under link
    // 2: byte-exact day file and Work Log post-images.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-2");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x1,2\n- 1 wired the lexer\n  - chose a hand-rolled lexer\n- 2 sketched the URL parser"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([
            {
                "index": 1,
                "text": "wired the lexer",
                "details": ["chose a hand-rolled lexer"],
            },
            { "index": 2, "text": "sketched the URL parser" },
        ])
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "\t- \u{1F345} [[bob#^capture-stop]]\n",
            "- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n",
            "\t- \u{1F345} [[bob#^capture-stop]]\n",
            "\t\t- Designed the `=x` grammar\n",
            "\t\t\t- chose `x` for done\n",
            "\t\t- Wrote the plan\n",
            "\t\t- wired the lexer\n",
            "\t\t\t- chose a hand-rolled lexer\n",
            "\t- \u{1F345} [[bob#^web-capture]]\n",
            "\t\t- sketched the URL parser\n",
            "\t- ~~[[sase#^axe-restart]]~~\n",
            "\t\t- Restarted axe\n",
            "\t- quick note\n",
            "- [ ] () — CAPTURE\n",
            "\t- [[bob#^capture-stop]]\n",
            "\t- [[bob#^web-capture]]\n",
            "- [ ] () — SASE\n",
            "\t- [[sase#^recovery-panel]]\n",
        )
    );
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("read bob");
    assert!(
        bob_after.contains(
            "\t\t- *2026-09-28* — wired the lexer\n\t\t\t- chose a hand-rolled lexer\n"
        ),
        "{bob_after}"
    );
    assert!(
        bob_after.contains("\t\t- *2026-09-28* — sketched the URL parser\n"),
        "{bob_after}"
    );
    let tasks = json["pomodoro_close"]["tasks"].as_array().expect("tasks");
    let row1 = tasks.iter().find(|task| task["index"] == 1).expect("row 1");
    assert_eq!(
        row1["typed_work_log"],
        serde_json::json!(["*2026-09-28* — wired the lexer"]),
        "{row1}"
    );
    assert_eq!(
        row1["typed_work_log_details"],
        serde_json::json!([["chose a hand-rolled lexer"]]),
        "{row1}"
    );
    // Row 2 has no details, so its aligned array is omitted.
    let row2 = tasks.iter().find(|task| task["index"] == 2).expect("row 2");
    assert_eq!(
        row2["typed_work_log"],
        serde_json::json!(["*2026-09-28* — sketched the URL parser"]),
        "{row2}"
    );
    assert!(row2.get("typed_work_log_details").is_none(), "{row2}");

    // `=x` plus two entries under link 1: typed order kept.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-3");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x\n- 1 wrote docs\n- 1 opened the PR"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([
            { "index": 1, "text": "wrote docs" },
            { "index": 1, "text": "opened the PR" },
        ])
    );

    // `=x1` plus `- 1 fixed 3 bugs`: only the first token is an index, so
    // no escape is needed.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-4");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x1\n- 1 fixed 3 bugs"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "fixed 3 bugs" }])
    );

    // Backslashes stay literal: no escape.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-5");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x\n- 1 fixed \\3 bugs"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "fixed \\3 bugs" }])
    );

    // Literal text: markers stay text, so no destination is set and no
    // schedule is parsed.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-7");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x\n- 1 moved @@inbox s:3"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "moved @@inbox s:3" }])
    );
    assert_eq!(json["kind"], "pomodoro_close");

    // Plain wikilinks are allowed and keep their text.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-8");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x\n- 1 see [[Design notes]]"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "see [[Design notes]]" }])
    );

    // A placeholder row is ignored: plain `=x`.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-9");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x\n- "]);
    assert_eq!(json["kind"], "pomodoro_close");
    assert!(json["pomodoro_close"].get("log").is_none(), "{json}");
}

#[test]
fn capture_pomodoro_close_log_complete_target() {
    // `=x1!2` plus `- 2 shipped it`: 2 completes and its entry lands in the
    // completed task.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-c");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x1!2\n- 2 shipped it"],
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
    // Out of range at execution quotes the bullet head, with no escape
    // advice.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-e1");
    let before = fs::read_to_string(&day_file).expect("read");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x\n- 5 foo"],
    );
    assert!(error.contains("`- 5` logs to task 5"), "{error}");
    assert!(error.contains("2 numbered Task Links"), "{error}");
    assert!(!error.contains("\\5"), "{error}");
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // Deferred target without `<N>`.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-e2");
    let before = fs::read_to_string(&day_file).expect("read");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x\n- 2 looked at it"],
    );
    assert!(error.contains("deferred"), "{error}");
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // Lexical bullet errors: not worked, dropped, missing number (and its
    // orphaned nested line), block links in entries and details, fence.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-e3");
    let before = fs::read_to_string(&day_file).expect("read");
    for (args, phrase) in [
        (vec!["=x1\n- 2 foo"], "isn't worked by `=x1`"),
        (vec!["=x~2\n- 2 foo"], "is dropped by `~2`"),
        (vec!["=x0\n- 0 foo"], "task numbers start at 1"),
        (
            vec!["=x\n- 1 ok\n- wired the lexer"],
            "start each Work Log bullet",
        ),
        (vec!["=x\n  - orphan"], "has no preceding"),
        (
            vec!["=x\n- 1 see [[bob#^web-capture]]"],
            "can't contain the block link",
        ),
        (
            vec!["=x\n- 1 ok\n  - ![[bob#^x]]"],
            "can't contain the block link",
        ),
        (vec!["=x\n- 1 ```rust"], "can't start with a code fence"),
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

    // A dangling bullet stays incomplete, quoting its bullet head.
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x\n- 1"],
    );
    assert!(
        error.contains(
            "`- 1` is incomplete: type the Work Log text after task 1"
        ),
        "{error}"
    );

    // An inline entry plus child bullets is the mixing error, echoing the
    // resolved bullet.
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x more\n- 1 foo"],
    );
    assert!(
        error.contains("can't be combined with Work Log bullets"),
        "{error}"
    );
    assert!(error.contains("`- 1 more`"), "{error}");
}

#[test]
fn capture_pomodoro_close_log_retired_tail() {
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-t");
    let before = fs::read_to_string(&day_file).expect("read");
    // The retired tail is now the inline entry: a leading number names the
    // task and executes like its bullet form.
    for args in [vec!["=x1,2 2 foo bar baz"], vec!["=x more"]] {
        let output = bob_command()
            .arg("capture")
            .arg("--dry-run")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(&args[0])
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-28 09:37:00")
            .output()
            .expect("dry run");
        assert_success(&output);
    }
    // A stray leading marker still teaches the fix.
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x - 2 foo"],
    );
    assert!(error.contains("drop the stray `-`"), "{error}");
    // A trailing Task Link is its own item, never entry text.
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x ^bob:ready="],
    );
    assert!(error.contains("can't end with the Task Link"), "{error}");
    // `=x 1` is now the dangling inline number with the `=x1` hint.
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x 1"],
    );
    assert!(
        error.contains("type the Work Log text after task 1"),
        "{error}"
    );
    assert!(error.contains("`=x1`"), "{error}");
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
}

#[test]
fn capture_pomodoro_close_log_dry_run_and_blocks() {
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-dr");
    let before_day = fs::read_to_string(&day_file).expect("read");
    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x\n- 1 wired the lexer\n  - chose a hand-rolled lexer"],
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
        &serde_json::json!([
            {
                "index": 1,
                "text": "wired the lexer",
                "details": ["chose a hand-rolled lexer"],
            },
        ])
    );
    // The blocks cover both the inserted entry and its detail line.
    let rendered =
        serde_json::to_string(&json["pomodoro_blocks"]).expect("json");
    assert!(rendered.contains("wired the lexer"), "{rendered}");
    assert!(rendered.contains("chose a hand-rolled lexer"), "{rendered}");
}

#[test]
fn capture_pomodoro_close_log_batch_rollback() {
    // A bullet error rolls the whole batch back.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-rb");
    let before = fs::read_to_string(&day_file).expect("read");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=x\n- 1 wired the lexer")
        .arg("=x1\n- 2 foo")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("run batch");
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
}

#[test]
fn capture_pomodoro_close_log_chains() {
    // `-2 =x` plus a bullet: shorten, then close with the entry.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-ch1");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("-2 =x\n- 1 wired the lexer")
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

    // `=x =` plus a bullet: close with the entry, then start the next
    // Pomodoro.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-ch2");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=x =\n- 1 wired the lexer")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("run chain");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["captures"][0]["kind"], "pomodoro_close");
    assert_eq!(
        json["captures"][0]["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "wired the lexer" }])
    );
    assert_eq!(json["captures"][1]["kind"], "pomodoro_start");

    // A blank line ends the close item: close with the entry, then start.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-ch3");
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-28 09:37:00"),
        "=x\n- 1 wired the lexer\n\n=\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["captures"][0]["kind"], "pomodoro_close");
    assert_eq!(
        json["captures"][0]["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "wired the lexer" }])
    );
    assert_eq!(json["captures"][1]["kind"], "pomodoro_start");

    // Inline entries chain: `-2 =x 1 foo` shortens then closes with the
    // entry, and `=x 1 wired the lexer =` closes with the entry then starts.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-ch4");
    for args in ["-2 =x 1 foo", "=x 1 wired the lexer ="] {
        let output = bob_command()
            .arg("capture")
            .arg("--dry-run")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(args)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-28 09:37:00")
            .output()
            .expect("dry run");
        assert_success(&output);
    }
}

#[test]
fn capture_parse_pomodoro_close_log_protocol() {
    // Valid bullets: spec plus an index span per entry number.
    let value = parse_json("=x\n- 1 wired the lexer");
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
                    && span["start"] == 5
                    && span["end"] == 6
            }),
        "{value}"
    );

    // Details ride the spec with no new spans.
    let detail =
        parse_json("=x\n- 1 wired the lexer\n  - chose a hand-rolled lexer");
    assert_eq!(
        detail["pomodoro_close"]["log"],
        serde_json::json!([{
            "index": 1,
            "text": "wired the lexer",
            "details": ["chose a hand-rolled lexer"],
        }])
    );

    // Chain: two items, and the close item's range contains the start
    // token's range.
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=x =\n- 1 wired it")
        .output()
        .expect("parse chain");
    assert_success(&output);
    let chain: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    let items = chain["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["mode"], "pomodoro_close");
    assert_eq!(items[1]["mode"], "pomodoro_start");
    assert_eq!(
        items[0]["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "wired it" }])
    );
    let (close_start, close_end) = (
        items[0]["range"]["start"].as_u64().expect("start"),
        items[0]["range"]["end"].as_u64().expect("end"),
    );
    let (start_start, start_end) = (
        items[1]["range"]["start"].as_u64().expect("start"),
        items[1]["range"]["end"].as_u64().expect("end"),
    );
    assert!(close_start < start_start);
    assert!(close_end > start_end);

    // Dangling bullets: a final one and a mid-draft one each get a
    // placeholder instead of an index span, with the complete entries kept
    // in the spec.
    let incomplete = parse_json("=x\n- 1");
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
                    && span["start"] == 5
                    && span["end"] == 6
            }),
        "{incomplete}"
    );
    let mid = parse_json("=x\n- 2 a\n- 1");
    assert_eq!(mid["mode"], "incomplete");
    assert_eq!(mid["needs"], serde_json::json!(["pomodoro_close_log_text"]));
    let spans = mid["spans"].as_array().expect("spans");
    assert!(
        spans.iter().any(|span| {
            span["kind"] == "pomodoro_close_log_index"
                && span["start"] == 5
                && span["end"] == 6
        }),
        "{mid}"
    );
    assert!(
        spans.iter().any(|span| {
            span["kind"] == "interactive_placeholder"
                && span["start"] == 11
                && span["end"] == 12
        }),
        "{mid}"
    );
    assert_eq!(
        mid["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 2, "text": "a" }])
    );

    // A dangling inline number is an editing state with the log-text need.
    let dangling_inline = parse_json("=x 1");
    assert_eq!(dangling_inline["mode"], "incomplete");
    assert_eq!(
        dangling_inline["needs"],
        serde_json::json!(["pomodoro_close_log_text"])
    );

    // Invalid drafts report precise diagnostics.
    for (text, phrase) in [
        ("=x - wired", "drop the stray `-`"),
        ("=x = wired", "then the session operators"),
        (
            "=x\n- 1 ok\n- wired the lexer",
            "start each Work Log bullet",
        ),
        ("=x1\n- 2 foo", "isn't worked by"),
        (
            "=x\n- 1 see [[bob#^web-capture]]",
            "can't contain the block link",
        ),
        (
            "=x\n- 1 ok\n  - ![[bob#^x]]",
            "can't contain the block link",
        ),
        ("=x\n- 1 ```fence", "can't start with a code fence"),
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

fn close_four_link_vault(name: &str) -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260928.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n",
            "\t- [[bob#^task-one]]\n",
            "\t- [[bob#^task-two]]\n",
            "\t- [[bob#^task-three]]\n",
            "\t- [[bob#^task-four]]\n",
        ),
    );
    write_file(
        &vault.join("bob.md"),
        concat!(
            "## Tasks\n",
            "\n",
            "- [*] #task Task one [created::2026-09-26] ^task-one\n",
            "- [*] #task Task two [created::2026-09-26] ^task-two\n",
            "- [*] #task Task three [created::2026-09-26] ^task-three\n",
            "- [*] #task Task four [created::2026-09-26] ^task-four\n",
        ),
    );
    (temp, vault, day_file)
}

#[test]
fn capture_pomodoro_close_log_positional_acceptance() {
    // The user's example: `=x3,4` plus unnumbered bullets writes exactly
    // what the numbered form writes, with the same `bob capture` JSON.
    let (_temp, vault, day_file) =
        close_four_link_vault("bob-cli-close-log-numbered");
    let numbered = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x3,4\n- 3 foo bar\n- 4 baz bam"],
    );
    let (_temp, vault, day_file) =
        close_four_link_vault("bob-cli-close-log-unnumbered");
    let unnumbered = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x3,4\n- foo bar\n- baz bam"],
    );
    let expected = serde_json::json!([
        { "index": 3, "text": "foo bar" },
        { "index": 4, "text": "baz bam" },
    ]);
    assert_eq!(numbered["pomodoro_close"]["log"], expected);
    assert_eq!(unnumbered["pomodoro_close"]["log"], expected);
    assert_eq!(
        numbered["pomodoro_close"]["tasks"],
        unnumbered["pomodoro_close"]["tasks"]
    );
    let (_temp, vault, day_file) =
        close_four_link_vault("bob-cli-close-log-read-numbered");
    run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x3,4\n- 3 foo bar\n- 4 baz bam"],
    );
    let numbered_day = fs::read_to_string(&day_file).expect("read day");
    let numbered_bob =
        fs::read_to_string(vault.join("bob.md")).expect("read bob");
    let (_temp, vault, day_file) =
        close_four_link_vault("bob-cli-close-log-read-unnumbered");
    run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x3,4\n- foo bar\n- baz bam"],
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        numbered_day
    );
    assert_eq!(
        fs::read_to_string(vault.join("bob.md")).expect("read bob"),
        numbered_bob
    );
}

#[test]
fn capture_pomodoro_close_log_positional_runtime() {
    // Plain `=x` on `close_worked_vault` (link 1 plain, link 2 deferred)
    // logs the bullet to task 1 and reports the resolved index.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-log-positional");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x\n- wired the lexer"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "wired the lexer" }])
    );
    // `--dry-run` JSON reports resolved indices without writing.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-log-dry");
    let before = fs::read_to_string(&day_file).expect("read");
    let output = bob_command()
        .arg("capture")
        .arg("--dry-run")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=x\n- wired the lexer")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("dry run");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("close json");
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "wired the lexer" }])
    );
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
    // A runtime too-many error writes nothing and rolls the batch back.
    let (_temp, vault, day_file) =
        close_four_link_vault("bob-cli-close-log-too-many");
    let before = fs::read_to_string(&day_file).expect("read");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x\n- a\n- b\n- c\n- d\n- e"],
    );
    assert!(error.contains("unnumbered Work Log bullets"), "{error}");
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
    // `=x =` plus unnumbered bullets attaches them to the close.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-log-close-start");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x =\n- wired the lexer"],
    );
    assert_eq!(
        json["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "wired the lexer" }])
    );
}

#[test]
fn capture_pomodoro_close_log_positional_parse() {
    // `=x3,4` plus unnumbered bullets: indices resolve, with no
    // `pomodoro_close_log_index` spans.
    let value = parse_json("=x3,4\n- foo bar\n- baz bam");
    assert_eq!(value["mode"], "pomodoro_close");
    assert_eq!(
        value["pomodoro_close"]["log"],
        serde_json::json!([
            { "index": 3, "text": "foo bar" },
            { "index": 4, "text": "baz bam" },
        ])
    );
    assert!(
        value["spans"]
            .as_array()
            .expect("spans")
            .iter()
            .all(|span| span["kind"] != "pomodoro_close_log_index"),
        "{value}"
    );
    // `=x` plus an unnumbered bullet: mode `pomodoro_close` with no
    // `index` key until execution resolves it.
    let value = parse_json("=x\n- foo");
    assert_eq!(value["mode"], "pomodoro_close");
    assert!(
        value["pomodoro_close"]["log"][0].get("index").is_none(),
        "{value}"
    );
    assert_eq!(
        value["pomodoro_close"]["log"][0]["text"],
        serde_json::json!("foo")
    );
    // Error drafts report `invalid_pomodoro_close` on precise ranges.
    let value = parse_json("=x\n- a\n- 2 b");
    assert_eq!(
        value["diagnostics"][0]["code"], "invalid_pomodoro_close",
        "{value}"
    );
    assert_eq!(
        value["diagnostics"][0]["range"],
        serde_json::json!([9, 10]),
        "{value}"
    );
    let value = parse_json("=x3,4\n- a\n- b\n- c");
    assert_eq!(
        value["diagnostics"][0]["code"], "invalid_pomodoro_close",
        "{value}"
    );
    assert!(
        value["diagnostics"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("unnumbered Work Log bullets"),
        "{value}"
    );
}
