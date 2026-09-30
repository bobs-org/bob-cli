//! `capture-parse` JSON for the `=[<X>][#name]~<K>` start-drop grammar:
//! modes, specs, spans, needs, placeholders, diagnostics and ranges, and
//! chain items.

use crate::support::*;

fn parse(text: &str) -> serde_json::Value {
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

fn parse_items(draft: &str) -> serde_json::Value {
    let output = run_with_stdin(
        bob_command().arg("capture-parse").arg("-f").arg("json"),
        draft,
    );
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("parse JSON")
}

#[test]
fn parse_start_drop_valid_spec_and_spans() {
    let parsed = parse("=~2");
    assert_eq!(parsed["mode"], "pomodoro_start", "{parsed}");
    assert_eq!(
        parsed["pomodoro_start"],
        serde_json::json!({
            "raw": "",
            "duration_units": 5,
            "offset_units": 0,
            "drop": [2],
        }),
        "{parsed}"
    );
    assert_eq!(
        parsed["spans"],
        serde_json::json!([
            { "start": 0, "end": 1, "kind": "pomodoro_start" },
            { "start": 1, "end": 3, "kind": "pomodoro_start_drop" },
        ]),
        "{parsed}"
    );
    assert_eq!(parsed["diagnostics"], serde_json::json!([]), "{parsed}");

    // Lists sort ascending; the human start line carries the drop summary.
    let parsed = parse("=3#bugs~3,1");
    assert_eq!(parsed["mode"], "pomodoro_start", "{parsed}");
    assert_eq!(parsed["section"], "bugs", "{parsed}");
    assert_eq!(
        parsed["pomodoro_start"]["drop"],
        serde_json::json!([1, 3]),
        "{parsed}"
    );
    let kinds: Vec<&str> = parsed["spans"]
        .as_array()
        .expect("spans")
        .iter()
        .map(|span| span["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(
        kinds,
        vec!["pomodoro_start", "pomodoro_name", "pomodoro_start_drop"],
        "{parsed}"
    );
}

#[test]
fn parse_start_drop_incomplete_needs_task() {
    let parsed = parse("=~");
    assert_eq!(parsed["mode"], "incomplete", "{parsed}");
    assert_eq!(
        parsed["needs"],
        serde_json::json!(["pomodoro_start_task"]),
        "{parsed}"
    );
    // An empty drop list stays omitted (additive-only contract).
    assert!(parsed["pomodoro_start"].get("drop").is_none(), "{parsed}");
    assert_eq!(
        parsed["spans"],
        serde_json::json!([
            { "start": 0, "end": 1, "kind": "pomodoro_start" },
            { "start": 1, "end": 2, "kind": "interactive_placeholder" },
        ]),
        "{parsed}"
    );

    let parsed = parse("=~2,");
    assert_eq!(parsed["mode"], "incomplete", "{parsed}");
    assert_eq!(
        parsed["needs"],
        serde_json::json!(["pomodoro_start_task"]),
        "{parsed}"
    );
    assert_eq!(
        parsed["pomodoro_start"]["drop"],
        serde_json::json!([2]),
        "{parsed}"
    );
    assert_eq!(
        parsed["spans"],
        serde_json::json!([
            { "start": 0, "end": 1, "kind": "pomodoro_start" },
            { "start": 1, "end": 3, "kind": "pomodoro_start_drop" },
            { "start": 3, "end": 4, "kind": "interactive_placeholder" },
        ]),
        "{parsed}"
    );
}

#[test]
fn parse_start_drop_diagnostics_match_execution() {
    // Messages match `bob capture` byte for byte; ranges point at the
    // offending bytes.
    let cases = [
        ("=~0", "task numbers start at 1", (2, 3)),
        ("=~2~3", "use one `~` list: `=~2,3`", (3, 4)),
        (
            "=~2a",
            "`=~2a` is not a drop list: write `~`, then comma-separated task numbers (for example `=~2,3`)",
            (3, 4),
        ),
    ];
    for (token, message, (start, end)) in cases {
        let parsed = parse(token);
        assert_eq!(parsed["mode"], "pomodoro_start", "{token}: {parsed}");
        let diagnostics =
            parsed["diagnostics"].as_array().expect("diags").clone();
        assert_eq!(diagnostics.len(), 1, "{token}: {parsed}");
        assert_eq!(
            diagnostics[0]["code"], "invalid_pomodoro_start",
            "{token}: {parsed}"
        );
        assert_eq!(diagnostics[0]["message"], message, "{token}: {parsed}");
        assert_eq!(
            diagnostics[0]["range"],
            serde_json::json!([start, end]),
            "{token}: {parsed}"
        );
        assert!(
            parsed.get("pomodoro_start").is_none()
                || parsed["pomodoro_start"].is_null(),
            "{token}: {parsed}"
        );
    }
}

#[test]
fn parse_start_drop_chain_items() {
    let parsed = parse_items("=x =~2");
    let items = parsed["items"].as_array().expect("items");
    assert_eq!(items.len(), 2, "{parsed}");
    assert_eq!(items[0]["mode"], "pomodoro_close", "{parsed}");
    assert_eq!(items[1]["mode"], "pomodoro_start", "{parsed}");
    assert_eq!(
        items[1]["pomodoro_start"]["drop"],
        serde_json::json!([2]),
        "{parsed}"
    );
}

#[test]
fn parse_start_drop_human_line() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("=3#bugs~1,3")
        .output()
        .expect("run capture-parse human");
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("=3#bugs~1,3 (15m, offset 0u · drop 1, 3)"),
        "{out}"
    );
}
