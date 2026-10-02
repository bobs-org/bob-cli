//! Capture parse pomodoro close and drop selections.

use crate::support::*;

#[test]
fn capture_parse_pomodoro_close_protocol() {
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
    let parse_stdin = |draft: &str| {
        let output = run_with_stdin(
            bob_command().arg("capture-parse").arg("-f").arg("json"),
            draft,
        );
        assert_success(&output);
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("capture-parse JSON")
    };

    // Whole-item `=x`: close mode, token span, additive spec.
    let close = parse("=x");
    assert_eq!(close["schema_version"], 1);
    assert_eq!(close["mode"], "pomodoro_close");
    assert_eq!(close["body"], "=x");
    assert!(close["route"].is_null());
    assert!(close["section"].is_null());
    assert!(close["block_id"].is_null());
    assert_eq!(close["needs"], serde_json::json!([]));
    assert_eq!(
        close["spans"],
        serde_json::json!([{ "start": 0, "end": 2, "kind": "pomodoro_close" }])
    );
    assert_eq!(close["diagnostics"], serde_json::json!([]));
    assert_eq!(
        close["pomodoro_close"],
        serde_json::json!({ "raw": "=x", "in_progress": null, "complete": [] })
    );

    // `=X` keeps its case in the typed raw.
    let upper = parse("=X");
    assert_eq!(upper["mode"], "pomodoro_close");
    assert_eq!(
        upper["pomodoro_close"],
        serde_json::json!({ "raw": "=X", "in_progress": null, "complete": [] })
    );

    // A bare `=` is now a whole-item start, not an incomplete close.
    let start = parse("=");
    assert_eq!(start["mode"], "pomodoro_start", "{start}");

    // An inline entry is a valid close with its log, never a diagnostic.
    let more = parse("=x more");
    assert_eq!(more["mode"], "pomodoro_close", "{more}");
    assert_eq!(
        more["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "more" }]),
        "{more}"
    );
    assert_eq!(more["diagnostics"], serde_json::json!([]), "{more}");
    assert_eq!(more["body"], "=x", "{more}");

    // Link forms keep their mode with a `pomodoro_close` suffix span and
    // the typed raw.
    let caret = parse("^r:id=x");
    assert_eq!(caret["mode"], "pomodoro_link");
    assert_eq!(
        caret["pomodoro_close"],
        serde_json::json!({ "raw": "=x", "in_progress": null, "complete": [] })
    );
    assert!(caret.get("pomodoro_start").is_none(), "{caret}");
    assert_eq!(
        caret["spans"],
        serde_json::json!([
            { "start": 0, "end": 2, "kind": "active_task_route" },
            { "start": 3, "end": 5, "kind": "active_task_block_id" },
            { "start": 5, "end": 7, "kind": "pomodoro_close" },
        ])
    );
    assert_eq!(caret["diagnostics"], serde_json::json!([]));

    let body = parse("Text @r:id=x");
    assert_eq!(body["mode"], "pomodoro_task");
    assert_eq!(body["body"], "Text");
    assert_eq!(
        body["pomodoro_close"],
        serde_json::json!({ "raw": "=x", "in_progress": null, "complete": [] })
    );
    assert_eq!(
        body["spans"],
        serde_json::json!([
            { "start": 5, "end": 7, "kind": "pomodoro_route" },
            { "start": 8, "end": 10, "kind": "pomodoro_block_id" },
            { "start": 10, "end": 12, "kind": "pomodoro_close" },
        ])
    );

    // `#name=x` diagnoses the `#name` component.
    let named = parse("^r:id#n=x");
    assert_eq!(named["mode"], "pomodoro_link");
    assert!(named.get("pomodoro_close").is_none(), "{named}");
    assert_eq!(
        named["diagnostics"][0]["code"], "invalid_pomodoro_close",
        "{named}"
    );
    assert_eq!(
        named["diagnostics"][0]["range"],
        serde_json::json!([5, 7]),
        "{named}"
    );

    // Mixed drafts preview the first item and keep per-item specs.
    let mixed = parse_stdin("+5\n\n=x\n");
    assert_eq!(mixed["mode"], "pomodoro_adjust");
    assert_eq!(mixed["items"].as_array().expect("items").len(), 2);
    assert_eq!(mixed["items"][0]["mode"], "pomodoro_adjust");
    assert_eq!(mixed["items"][1]["mode"], "pomodoro_close");
    assert_eq!(
        mixed["items"][1]["pomodoro_close"],
        serde_json::json!({ "raw": "=x", "in_progress": null, "complete": [] })
    );
}

#[test]
fn capture_parse_pomodoro_close_selection_protocol() {
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
    let parse_stdin = |draft: &str| {
        let output = run_with_stdin(
            bob_command().arg("capture-parse").arg("-f").arg("json"),
            draft,
        );
        assert_success(&output);
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("capture-parse JSON")
    };

    // Valid selections: additive spec plus the three span kinds.
    for (text, in_progress, complete) in [
        ("=x1", serde_json::json!([1]), serde_json::json!([])),
        ("=x1,3", serde_json::json!([1, 3]), serde_json::json!([])),
        ("=x3,1", serde_json::json!([1, 3]), serde_json::json!([])),
        ("=x!2", serde_json::json!(null), serde_json::json!([2])),
        ("=x1,3!2", serde_json::json!([1, 3]), serde_json::json!([2])),
        ("=X1!2", serde_json::json!([1]), serde_json::json!([2])),
        ("=x0", serde_json::json!([]), serde_json::json!([])),
        ("=x0!2", serde_json::json!([]), serde_json::json!([2])),
    ] {
        let value = parse(text);
        assert_eq!(value["mode"], "pomodoro_close", "{text}");
        assert_eq!(value["pomodoro_close"]["raw"], text, "{text}");
        assert_eq!(
            value["pomodoro_close"]["in_progress"], in_progress,
            "{text}"
        );
        assert_eq!(value["pomodoro_close"]["complete"], complete, "{text}");
        assert_eq!(value["diagnostics"], serde_json::json!([]), "{text}");
    }
    let both = parse("=x1,3!2");
    assert_eq!(
        both["spans"],
        serde_json::json!([
            { "start": 0, "end": 2, "kind": "pomodoro_close" },
            { "start": 2, "end": 5, "kind": "pomodoro_close_in_progress" },
            { "start": 5, "end": 7, "kind": "pomodoro_close_complete" },
        ])
    );

    // Every lexical row: precise `invalid_pomodoro_close` range, no spec.
    for (text, leading, range) in [
        ("=x1,1", "task 1 is listed twice", [4, 5]),
        (
            "=x1!1",
            "task 1 cannot both stay in progress and complete",
            [4, 5],
        ),
        (
            "=x1,2!2",
            "task 2 cannot both stay in progress and complete",
            [6, 7],
        ),
        ("=x0,2", "`0` means no task stays in progress", [2, 3]),
        ("=x2,0", "`0` means no task stays in progress", [4, 5]),
        ("=x0,", "`0` means no task stays in progress", [2, 3]),
        ("=x00", "`0` means no task stays in progress", [2, 4]),
        ("=x!0", "task numbers start at 1", [3, 4]),
        ("=x,1", "expected a task number before", [2, 3]),
        ("=x1,,2", "expected a task number before", [4, 5]),
        ("=x1,,", "expected a task number before", [4, 5]),
        ("=x1!2,,", "expected a task number before", [6, 7]),
        ("=x1,!2", "expected a task number after", [3, 4]),
        ("=x!,1", "expected a task number before", [3, 4]),
        ("=x1!2!3", "use one `!` list", [5, 6]),
        ("=x1a", "`=x1a` is not a task list", [3, 4]),
        ("=x1;2", "`=x1;2` is not a task list", [3, 5]),
        ("=x!2x", "`=x!2x` is not a task list", [4, 5]),
        (
            "=x99999999999",
            "task number 99999999999 is too large",
            [2, 13],
        ),
    ] {
        let value = parse(text);
        assert_eq!(value["mode"], "pomodoro_close", "{text}");
        assert!(value.get("pomodoro_close").is_none(), "{text}: {value}");
        assert_eq!(
            value["diagnostics"][0]["code"], "invalid_pomodoro_close",
            "{text}"
        );
        assert!(
            value["diagnostics"][0]["message"]
                .as_str()
                .expect("message")
                .starts_with(leading),
            "{text}: {}",
            value["diagnostics"][0]["message"]
        );
        assert_eq!(
            value["diagnostics"][0]["range"],
            serde_json::json!(range),
            "{text}"
        );
    }

    // Spaced lists get the no-spaces hint when the first token is not a
    // plain number (`=x 1,3` means `=x1,3`). A lone number is the dangling
    // inline entry instead.
    for (text, range) in [("=x 1,3", [3, 6]), ("=x1 !2", [4, 6])] {
        let value = parse(text);
        assert_eq!(
            value["diagnostics"][0]["code"], "invalid_pomodoro_close",
            "{text}"
        );
        assert!(
            value["diagnostics"][0]["message"]
                .as_str()
                .expect("message")
                .starts_with("write the task numbers right after"),
            "{text}: {}",
            value["diagnostics"][0]["message"]
        );
        assert_eq!(
            value["diagnostics"][0]["range"],
            serde_json::json!(range),
            "{text}"
        );
    }
    for (text, start, end) in [
        ("=x 1", 3, 4),
        ("=x1 1", 4, 5),
        ("=x1,3 3", 6, 7),
        ("=x 2", 3, 4),
    ] {
        let value = parse(text);
        assert_eq!(value["mode"], "incomplete", "{text}");
        assert_eq!(
            value["needs"],
            serde_json::json!(["pomodoro_close_log_text"]),
            "{text}"
        );
        assert!(
            value["spans"]
                .as_array()
                .expect("spans")
                .iter()
                .any(|span| {
                    span["kind"] == "interactive_placeholder"
                        && span["start"] == start
                        && span["end"] == end
                }),
            "{text}: {value}"
        );
    }

    // A dangling close index with a space (`=x1, 3`) is an editing state for
    // the close token itself, not the no-spaces hint.
    let spaced_incomplete = parse("=x1, 3");
    assert_eq!(spaced_incomplete["mode"], "incomplete", "=x1, 3");
    assert_eq!(
        spaced_incomplete["needs"],
        serde_json::json!(["pomodoro_close_task"]),
        "=x1, 3"
    );

    // A close with a dangling Work Log bullet is an editing state needing
    // `pomodoro_close_log_text`, with a placeholder over the number.
    let dangling = parse("=x\n- 1");
    assert_eq!(dangling["mode"], "incomplete");
    assert_eq!(
        dangling["needs"],
        serde_json::json!(["pomodoro_close_log_text"])
    );
    assert!(
        dangling["spans"]
            .as_array()
            .expect("spans")
            .iter()
            .any(|span| {
                span["kind"] == "interactive_placeholder"
                    && span["start"] == 5
                    && span["end"] == 6
            }),
        "{dangling}"
    );

    // Other extra text keeps an `invalid_pomodoro_close` diagnostic on the
    // first tail token.
    for (text, range) in [("=x - wired", [3, 4])] {
        let value = parse(text);
        assert_eq!(
            value["diagnostics"][0]["code"], "invalid_pomodoro_close",
            "{text}"
        );
        assert_eq!(
            value["diagnostics"][0]["range"],
            serde_json::json!(range),
            "{text}"
        );
    }

    // A `^` link close ending in `!` is incomplete, not a toggle.
    for text in ["^r:id=x!", "^r:id=x1!", "^r:id=x0!", "^r:id=x1,3!"] {
        let value = parse(text);
        assert_eq!(value["mode"], "incomplete", "{text}");
        assert_eq!(
            value["needs"],
            serde_json::json!(["pomodoro_close_task"]),
            "{text}"
        );
    }
    let caret_toggle = parse("^r:id!");
    assert_eq!(
        caret_toggle["diagnostics"][0]["code"],
        "invalid_pomodoro_link"
    );

    // An inline entry is valid with its log and index span.
    let more = parse("=x1 more");
    assert_eq!(more["mode"], "pomodoro_close", "{more}");
    assert_eq!(
        more["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 1, "text": "more" }]),
        "{more}"
    );
    let echo = parse("=x2,3 2 foo bar baz");
    assert_eq!(echo["mode"], "pomodoro_close", "{echo}");
    assert_eq!(
        echo["pomodoro_close"]["log"],
        serde_json::json!([{ "index": 2, "text": "foo bar baz" }]),
        "{echo}"
    );
    let child = parse_stdin("=x1\n- detail\n");
    assert_eq!(child["mode"], "pomodoro_close");
    assert_eq!(
        child["diagnostics"][0]["code"], "invalid_pomodoro_close",
        "{child}"
    );
    assert!(
        child["diagnostics"][0]["message"]
            .as_str()
            .expect("message")
            .starts_with("start each Work Log bullet"),
        "{child}"
    );

    // Dangling separators are editing states, never mistakes.
    for (text, in_progress, complete, spans) in [
        (
            "=x1,",
            serde_json::json!([1]),
            serde_json::json!([]),
            serde_json::json!([
                { "start": 0, "end": 2, "kind": "pomodoro_close" },
                { "start": 2, "end": 3, "kind": "pomodoro_close_in_progress" },
                { "start": 3, "end": 4, "kind": "interactive_placeholder" },
            ]),
        ),
        (
            "=x!",
            serde_json::json!(null),
            serde_json::json!([]),
            serde_json::json!([
                { "start": 0, "end": 2, "kind": "pomodoro_close" },
                { "start": 2, "end": 3, "kind": "interactive_placeholder" },
            ]),
        ),
        (
            "=x1!",
            serde_json::json!([1]),
            serde_json::json!([]),
            serde_json::json!([
                { "start": 0, "end": 2, "kind": "pomodoro_close" },
                { "start": 2, "end": 3, "kind": "pomodoro_close_in_progress" },
                { "start": 3, "end": 4, "kind": "interactive_placeholder" },
            ]),
        ),
        (
            "=x!2,",
            serde_json::json!(null),
            serde_json::json!([2]),
            serde_json::json!([
                { "start": 0, "end": 2, "kind": "pomodoro_close" },
                { "start": 2, "end": 4, "kind": "pomodoro_close_complete" },
                { "start": 4, "end": 5, "kind": "interactive_placeholder" },
            ]),
        ),
    ] {
        let value = parse(text);
        assert_eq!(value["mode"], "incomplete", "{text}");
        assert_eq!(
            value["needs"],
            serde_json::json!(["pomodoro_close_task"]),
            "{text}"
        );
        assert_eq!(value["pomodoro_close"]["raw"], text, "{text}");
        assert_eq!(
            value["pomodoro_close"]["in_progress"], in_progress,
            "{text}"
        );
        assert_eq!(value["pomodoro_close"]["complete"], complete, "{text}");
        assert_eq!(value["spans"], spans, "{text}");
        assert_eq!(value["diagnostics"], serde_json::json!([]), "{text}");
    }

    // Other `=` shapes and mid-body tokens stay ordinary prose.
    for text in ["=xx", "=xa", "Plan =x1"] {
        let value = parse(text);
        assert_eq!(value["mode"], "task", "{text}");
        assert!(value.get("pomodoro_close").is_none(), "{text}: {value}");
        assert_eq!(value["diagnostics"], serde_json::json!([]), "{text}");
    }

    // Link forms carry the suffix spec and spans; `#name` still conflicts.
    let at = parse("@r:id=x1!2");
    assert_eq!(at["mode"], "pomodoro_link");
    assert_eq!(
        at["pomodoro_close"],
        serde_json::json!({ "raw": "=x1!2", "in_progress": [1], "complete": [2] })
    );
    assert_eq!(
        at["spans"],
        serde_json::json!([
            { "start": 0, "end": 2, "kind": "pomodoro_route" },
            { "start": 3, "end": 5, "kind": "pomodoro_block_id" },
            { "start": 5, "end": 7, "kind": "pomodoro_close" },
            { "start": 7, "end": 8, "kind": "pomodoro_close_in_progress" },
            { "start": 8, "end": 10, "kind": "pomodoro_close_complete" },
        ])
    );
    let caret = parse("^r:id=x1");
    assert_eq!(caret["mode"], "pomodoro_link");
    assert_eq!(
        caret["pomodoro_close"],
        serde_json::json!({ "raw": "=x1", "in_progress": [1], "complete": [] })
    );
    let body = parse("Text @r:id=x!1");
    assert_eq!(body["mode"], "pomodoro_task");
    assert_eq!(
        body["pomodoro_close"],
        serde_json::json!({ "raw": "=x!1", "in_progress": null, "complete": [1] })
    );
    let named = parse("^r:id#n=x1");
    assert!(named.get("pomodoro_close").is_none(), "{named}");
    assert_eq!(
        named["diagnostics"][0]["code"], "invalid_pomodoro_close",
        "{named}"
    );
    let scheduled = parse("Text @r:id=x1 s:2");
    assert_eq!(
        scheduled["diagnostics"][0]["code"], "invalid_pomodoro_close",
        "{scheduled}"
    );

    // A dangling link suffix is incomplete too.
    let dangling = parse("^r:id=x1,");
    assert_eq!(dangling["mode"], "incomplete");
    assert_eq!(
        dangling["needs"],
        serde_json::json!(["pomodoro_close_task"])
    );
    assert_eq!(
        dangling["pomodoro_close"],
        serde_json::json!({ "raw": "=x1,", "in_progress": [1], "complete": [] })
    );

    // A multi-item draft mixes an adjustment, a selection close, and a task.
    let mixed = parse_stdin("+5\n\n=x1!2\n\nCall bank @Cash+\n");
    assert_eq!(mixed["items"].as_array().expect("items").len(), 3);
    assert_eq!(mixed["items"][1]["mode"], "pomodoro_close");
    assert_eq!(
        mixed["items"][1]["pomodoro_close"],
        serde_json::json!({ "raw": "=x1!2", "in_progress": [1], "complete": [2] })
    );

    // A `@@` draft never routes a selection-bearing close.
    let declared = parse_stdin("@@foo\nFirst\n\n=x1\n");
    assert_eq!(declared["items"][0]["route"], "foo");
    assert_eq!(declared["items"][1]["mode"], "pomodoro_close");
    assert!(declared["items"][1]["route"].is_null());

    // The human `close` line summarizes the selection.
    let human = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("=x1,3!2")
        .output()
        .expect("run capture-parse human");
    assert_success(&human);
    let out = stdout(&human);
    assert!(
        out.contains(
            "=x1,3!2 (in progress 1, 3 · complete 2 · defer the rest)"
        ),
        "{out}"
    );
    let none = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("=x0")
        .output()
        .expect("run capture-parse human");
    assert_success(&none);
    assert!(stdout(&none).contains("=x0 (in progress none"), "{none:?}");
}

#[test]
fn capture_parse_pomodoro_close_drop_protocol() {
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

    // Valid drop selections: additive spec plus the drop span, in either
    // `!`/`~` order.
    for (text, spec, spans) in [
        (
            "=x~2",
            serde_json::json!({ "raw": "=x~2", "in_progress": null, "complete": [], "drop": [2] }),
            serde_json::json!([
                { "start": 0, "end": 2, "kind": "pomodoro_close" },
                { "start": 2, "end": 4, "kind": "pomodoro_close_drop" },
            ]),
        ),
        (
            "=x1~2",
            serde_json::json!({ "raw": "=x1~2", "in_progress": [1], "complete": [], "drop": [2] }),
            serde_json::json!([
                { "start": 0, "end": 2, "kind": "pomodoro_close" },
                { "start": 2, "end": 3, "kind": "pomodoro_close_in_progress" },
                { "start": 3, "end": 5, "kind": "pomodoro_close_drop" },
            ]),
        ),
        (
            "=x1!2~3",
            serde_json::json!({ "raw": "=x1!2~3", "in_progress": [1], "complete": [2], "drop": [3] }),
            serde_json::json!([
                { "start": 0, "end": 2, "kind": "pomodoro_close" },
                { "start": 2, "end": 3, "kind": "pomodoro_close_in_progress" },
                { "start": 3, "end": 5, "kind": "pomodoro_close_complete" },
                { "start": 5, "end": 7, "kind": "pomodoro_close_drop" },
            ]),
        ),
        (
            "=x1~3!2",
            serde_json::json!({ "raw": "=x1~3!2", "in_progress": [1], "complete": [2], "drop": [3] }),
            serde_json::json!([
                { "start": 0, "end": 2, "kind": "pomodoro_close" },
                { "start": 2, "end": 3, "kind": "pomodoro_close_in_progress" },
                { "start": 3, "end": 5, "kind": "pomodoro_close_drop" },
                { "start": 5, "end": 7, "kind": "pomodoro_close_complete" },
            ]),
        ),
    ] {
        let value = parse(text);
        assert_eq!(value["mode"], "pomodoro_close", "{text}");
        assert_eq!(value["pomodoro_close"], spec, "{text}");
        assert_eq!(value["spans"], spans, "{text}");
        assert_eq!(value["diagnostics"], serde_json::json!([]), "{text}");
    }

    // A dangling `~` is an editing state needing `pomodoro_close_task`.
    let tilde = parse("=x1~");
    assert_eq!(tilde["mode"], "incomplete");
    assert_eq!(tilde["needs"], serde_json::json!(["pomodoro_close_task"]));
    assert_eq!(tilde["pomodoro_close"]["raw"], "=x1~");
    assert_eq!(
        tilde["pomodoro_close"]["in_progress"],
        serde_json::json!([1])
    );
    assert_eq!(tilde["pomodoro_close"]["complete"], serde_json::json!([]));
    assert!(tilde["pomodoro_close"].get("drop").is_none());
    assert_eq!(
        tilde["spans"],
        serde_json::json!([
            { "start": 0, "end": 2, "kind": "pomodoro_close" },
            { "start": 2, "end": 3, "kind": "pomodoro_close_in_progress" },
            { "start": 3, "end": 4, "kind": "interactive_placeholder" },
        ])
    );

    // Drop overlaps and range errors report precise ranges with no spec.
    for (text, leading, range) in [
        (
            "=x1~1",
            "task 1 cannot both stay in progress and drop",
            [4, 5],
        ),
        ("=x!2~2", "task 2 cannot both complete and drop", [5, 6]),
        ("=x~0", "task numbers start at 1", [3, 4]),
        ("=x1~2~3", "use one `~` list", [5, 6]),
    ] {
        let value = parse(text);
        assert_eq!(value["mode"], "pomodoro_close", "{text}");
        assert!(value.get("pomodoro_close").is_none(), "{text}: {value}");
        assert_eq!(
            value["diagnostics"][0]["code"], "invalid_pomodoro_close",
            "{text}"
        );
        assert!(
            value["diagnostics"][0]["message"]
                .as_str()
                .expect("message")
                .starts_with(leading),
            "{text}: {}",
            value["diagnostics"][0]["message"]
        );
        assert_eq!(
            value["diagnostics"][0]["range"],
            serde_json::json!(range),
            "{text}"
        );
    }

    // Link forms carry the drop spec and spans.
    let at = parse("@r:id=x1~2");
    assert_eq!(at["mode"], "pomodoro_link");
    assert_eq!(
        at["pomodoro_close"],
        serde_json::json!({ "raw": "=x1~2", "in_progress": [1], "complete": [], "drop": [2] })
    );
    assert_eq!(
        at["spans"],
        serde_json::json!([
            { "start": 0, "end": 2, "kind": "pomodoro_route" },
            { "start": 3, "end": 5, "kind": "pomodoro_block_id" },
            { "start": 5, "end": 7, "kind": "pomodoro_close" },
            { "start": 7, "end": 8, "kind": "pomodoro_close_in_progress" },
            { "start": 8, "end": 10, "kind": "pomodoro_close_drop" },
        ])
    );

    // The human `close` line summarizes the drop list.
    let human = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("=x1,3!2~4")
        .output()
        .expect("run capture-parse human");
    assert_success(&human);
    assert!(
        stdout(&human).contains(
            "=x1,3!2~4 (in progress 1, 3 · complete 2 · drop 4 · defer the rest)"
        ),
        "{}",
        stdout(&human)
    );
}
