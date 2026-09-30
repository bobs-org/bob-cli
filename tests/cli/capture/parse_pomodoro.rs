//! Capture parse pomodoro adjust, shift, start.

use crate::support::*;

#[test]
fn capture_parse_pomodoro_adjust_protocol() {
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

    // Exact-unit recognition: `+5` is five 5-minute units, `-2` is two.
    let plus = parse("+5");
    assert_eq!(plus["schema_version"], 1);
    assert_eq!(plus["mode"], "pomodoro_adjust");
    assert_eq!(plus["body"], "+5");
    assert!(plus["route"].is_null());
    assert!(plus["section"].is_null());
    assert!(plus["block_id"].is_null());
    assert_eq!(plus["needs"], serde_json::json!([]));
    assert_eq!(
        plus["spans"],
        serde_json::json!([{ "start": 0, "end": 2, "kind": "pomodoro_adjust" }])
    );
    assert_eq!(plus["diagnostics"], serde_json::json!([]));
    assert_eq!(
        plus["pomodoro_adjust"],
        serde_json::json!({ "raw": "+5", "plus": true, "units": 5 })
    );
    assert!(plus.get("pomodoro_start").is_none(), "{plus}");
    assert!(plus.get("items").is_none(), "{plus}");

    let minus = parse("-2");
    assert_eq!(minus["mode"], "pomodoro_adjust");
    assert_eq!(
        minus["pomodoro_adjust"],
        serde_json::json!({ "raw": "-2", "plus": false, "units": 2 })
    );
    assert_eq!(
        minus["spans"],
        serde_json::json!([{ "start": 0, "end": 2, "kind": "pomodoro_adjust" }])
    );

    // Whitespace around the token is fine; the span covers only the token.
    let padded = parse("  +5  ");
    assert_eq!(padded["mode"], "pomodoro_adjust");
    assert_eq!(padded["body"], "+5");
    assert_eq!(
        padded["spans"],
        serde_json::json!([{ "start": 2, "end": 4, "kind": "pomodoro_adjust" }])
    );
    assert_eq!(
        padded["pomodoro_adjust"],
        serde_json::json!({ "raw": "+5", "plus": true, "units": 5 })
    );

    // Mixed drafts use blank lines; top level previews the first item.
    let mixed = parse_stdin("+5\n\nCall bank @Cash+\n");
    assert_eq!(mixed["mode"], "pomodoro_adjust");
    assert_eq!(
        mixed["pomodoro_adjust"],
        serde_json::json!({ "raw": "+5", "plus": true, "units": 5 })
    );
    assert_eq!(mixed["items"].as_array().expect("items").len(), 2);
    assert_eq!(mixed["items"][0]["mode"], "pomodoro_adjust");
    assert_eq!(
        mixed["items"][0]["pomodoro_adjust"],
        serde_json::json!({ "raw": "+5", "plus": true, "units": 5 })
    );
    assert_eq!(mixed["items"][1]["mode"], "incomplete");
    assert_eq!(mixed["items"][1]["route"], "cash");

    // A `@@` declaration routes ordinary items but never an adjustment.
    let declared = parse_stdin("@@work\nFirst task\n\n+5\n");
    assert_eq!(declared["global_destination"]["route"], "work");
    assert_eq!(declared["items"][0]["mode"], "task");
    assert_eq!(declared["items"][0]["route"], "work");
    assert_eq!(declared["items"][1]["mode"], "pomodoro_adjust");
    assert!(declared["items"][1]["route"].is_null());
    assert_eq!(
        declared["items"][1]["pomodoro_adjust"],
        serde_json::json!({ "raw": "+5", "plus": true, "units": 5 })
    );

    let lone_declared = parse_stdin("@@work\n+5\n");
    assert_eq!(lone_declared["mode"], "pomodoro_adjust");
    assert!(lone_declared["route"].is_null(), "{lone_declared}");
    assert_eq!(
        lone_declared["pomodoro_adjust"],
        serde_json::json!({ "raw": "+5", "plus": true, "units": 5 })
    );

    // Ordinary prose containing a count stays a task.
    let prose = parse("Plan +5");
    assert_eq!(prose["mode"], "task");
    assert!(prose.get("pomodoro_adjust").is_none(), "{prose}");
    assert_eq!(prose["diagnostics"], serde_json::json!([]));

    // Adjustment-first items with extra text, markers, or children are
    // invalid adjustments, never tasks.
    for text in ["+5 foo", "+5 s:1", "+5 p:1", "+5 %", "+5 @work"] {
        let value = parse(text);
        assert_eq!(value["mode"], "pomodoro_adjust", "{text}");
        assert!(value.get("pomodoro_adjust").is_none(), "{text}: {value}");
        assert_eq!(
            value["diagnostics"][0]["code"], "invalid_pomodoro_adjustment",
            "{text}: {value}"
        );
    }
    let child = parse_stdin("+5\n- detail\n");
    assert_eq!(child["mode"], "pomodoro_adjust");
    assert!(child.get("pomodoro_adjust").is_none(), "{child}");
    assert_eq!(
        child["diagnostics"][0]["code"], "invalid_pomodoro_adjustment",
        "{child}"
    );

    // Invalid standalone counts produce a diagnostic with a useful range.
    let zero = parse("+0");
    assert_eq!(zero["mode"], "pomodoro_adjust");
    assert!(zero.get("pomodoro_adjust").is_none(), "{zero}");
    assert_eq!(
        zero["diagnostics"][0]["code"],
        "invalid_pomodoro_adjustment"
    );
    assert_eq!(zero["diagnostics"][0]["range"], serde_json::json!([0, 2]));

    let overflow = parse("+99999999999999999999999");
    assert_eq!(overflow["mode"], "pomodoro_adjust");
    assert!(overflow.get("pomodoro_adjust").is_none(), "{overflow}");
    assert_eq!(
        overflow["diagnostics"][0]["code"], "invalid_pomodoro_adjustment",
        "{overflow}"
    );

    // A bare sign is one unit, never incomplete.
    for (text, plus) in [("+", true), ("-", false)] {
        let value = parse(text);
        assert_eq!(value["mode"], "pomodoro_adjust", "{text}");
        assert_eq!(value["needs"], serde_json::json!([]));
        assert_eq!(
            value["pomodoro_adjust"],
            serde_json::json!({ "raw": text, "plus": plus, "units": 1 }),
            "{text}"
        );
        assert_eq!(value["diagnostics"], serde_json::json!([]));
    }

    // JSON stays additive at schema version 1.
    assert_eq!(plus["schema_version"], 1);
    let ordinary = parse("Call bank @Cash+");
    assert!(ordinary.get("pomodoro_adjust").is_none(), "{ordinary}");
    let pomodoro = parse("Do work @dev:id#bugs");
    assert!(pomodoro.get("pomodoro_adjust").is_none(), "{pomodoro}");
    assert!(pomodoro.get("pomodoro_start").is_none(), "{pomodoro}");
}

#[test]
fn capture_parse_pomodoro_adjust_human_and_help() {
    let human = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("+5")
        .output()
        .expect("run capture-parse human");
    assert_success(&human);
    let out = stdout(&human);
    assert!(out.contains("pomodoro_adjust"), "{out}");
    assert!(out.contains("+5 (25m, 5 units)"), "{out}");
    assert!(out.contains("pomodoro_adjust"), "{out}");
    assert_stdout_has_no_ansi(&human);

    let minus = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("-2")
        .output()
        .expect("run minus human");
    assert_success(&minus);
    assert!(
        stdout(&minus).contains("-2 (10m, 2 units)"),
        "{}",
        stdout(&minus)
    );

    let plain = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("Do work @dev:id#bugs")
        .output()
        .expect("run plain human");
    assert_success(&plain);
    assert!(!stdout(&plain).contains("adjust"), "{}", stdout(&plain));

    let parse_help = bob_command()
        .arg("capture-parse")
        .arg("--help")
        .output()
        .expect("run capture-parse help");
    assert_success(&parse_help);
    let help = stdout(&parse_help);
    assert!(
        help.contains("pomodoro_adjust")
            && help.contains("+5")
            && help.contains("-2")
            && help.contains("+5\\n\\nCall bank"),
        "expected adjustment help:\n{help}"
    );

    let capture_help = bob_command()
        .arg("capture")
        .arg("--help")
        .output()
        .expect("run capture help");
    assert_success(&capture_help);
    let capture = stdout(&capture_help);
    assert!(
        capture.contains("+5")
            && capture.contains("-2")
            && capture.contains("+5\\n\\nCall bank"),
        "expected capture adjustment help:\n{capture}"
    );
}

#[test]
fn capture_parse_pomodoro_shift_protocol() {
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

    // Exact-unit recognition: `++3` is three 5-minute units later.
    let later = parse("++3");
    assert_eq!(later["schema_version"], 1);
    assert_eq!(later["mode"], "pomodoro_shift");
    assert_eq!(later["body"], "++3");
    assert!(later["route"].is_null());
    assert!(later["section"].is_null());
    assert!(later["block_id"].is_null());
    assert_eq!(later["needs"], serde_json::json!([]));
    assert_eq!(
        later["spans"],
        serde_json::json!([{ "start": 0, "end": 3, "kind": "pomodoro_shift" }])
    );
    assert_eq!(later["diagnostics"], serde_json::json!([]));
    assert_eq!(
        later["pomodoro_shift"],
        serde_json::json!({ "raw": "++3", "later": true, "units": 3 })
    );
    assert!(later.get("pomodoro_adjust").is_none(), "{later}");
    assert!(later.get("pomodoro_start").is_none(), "{later}");
    assert!(later.get("items").is_none(), "{later}");

    let earlier = parse("--2");
    assert_eq!(earlier["mode"], "pomodoro_shift");
    assert_eq!(
        earlier["pomodoro_shift"],
        serde_json::json!({ "raw": "--2", "later": false, "units": 2 })
    );
    assert_eq!(
        earlier["spans"],
        serde_json::json!([{ "start": 0, "end": 3, "kind": "pomodoro_shift" }])
    );
    assert!(earlier.get("pomodoro_adjust").is_none(), "{earlier}");

    // A bare doubled sign is one unit, never incomplete.
    for (text, later) in [("++", true), ("--", false)] {
        let value = parse(text);
        assert_eq!(value["mode"], "pomodoro_shift", "{text}");
        assert_eq!(value["needs"], serde_json::json!([]));
        assert_eq!(
            value["pomodoro_shift"],
            serde_json::json!({ "raw": text, "later": later, "units": 1 }),
            "{text}"
        );
        assert_eq!(value["diagnostics"], serde_json::json!([]));
    }

    // Whitespace around the token is fine; the span covers only the token.
    let padded = parse("  ++3  ");
    assert_eq!(padded["mode"], "pomodoro_shift");
    assert_eq!(padded["body"], "++3");
    assert_eq!(
        padded["spans"],
        serde_json::json!([{ "start": 2, "end": 5, "kind": "pomodoro_shift" }])
    );
    assert_eq!(
        padded["pomodoro_shift"],
        serde_json::json!({ "raw": "++3", "later": true, "units": 3 })
    );

    // Mixed drafts use blank lines; top level previews the first item.
    let mixed = parse_stdin("++3\n\nCall bank @Cash+\n");
    assert_eq!(mixed["mode"], "pomodoro_shift");
    assert_eq!(
        mixed["pomodoro_shift"],
        serde_json::json!({ "raw": "++3", "later": true, "units": 3 })
    );
    assert_eq!(mixed["items"].as_array().expect("items").len(), 2);
    assert_eq!(mixed["items"][0]["mode"], "pomodoro_shift");
    assert_eq!(
        mixed["items"][0]["pomodoro_shift"],
        serde_json::json!({ "raw": "++3", "later": true, "units": 3 })
    );
    assert_eq!(mixed["items"][1]["mode"], "incomplete");
    assert_eq!(mixed["items"][1]["route"], "cash");

    // A `@@` declaration routes ordinary items but never a shift.
    let declared = parse_stdin("@@work\nFirst task\n\n++3\n");
    assert_eq!(declared["global_destination"]["route"], "work");
    assert_eq!(declared["items"][0]["mode"], "task");
    assert_eq!(declared["items"][0]["route"], "work");
    assert_eq!(declared["items"][1]["mode"], "pomodoro_shift");
    assert!(declared["items"][1]["route"].is_null());
    assert_eq!(
        declared["items"][1]["pomodoro_shift"],
        serde_json::json!({ "raw": "++3", "later": true, "units": 3 })
    );

    let lone_declared = parse_stdin("@@work\n++3\n");
    assert_eq!(lone_declared["mode"], "pomodoro_shift");
    assert!(lone_declared["route"].is_null(), "{lone_declared}");
    assert_eq!(
        lone_declared["pomodoro_shift"],
        serde_json::json!({ "raw": "++3", "later": true, "units": 3 })
    );

    // Prose lookalikes stay ordinary tasks with no diagnostics.
    for text in [
        "- foo", "-- aside", "++ plan", "+++", "---", "+-", "-+3", "Plan ++3",
        "C++",
    ] {
        let value = parse(text);
        assert_eq!(value["mode"], "task", "{text}");
        assert!(value.get("pomodoro_shift").is_none(), "{text}: {value}");
        assert!(value.get("pomodoro_adjust").is_none(), "{text}: {value}");
        assert_eq!(value["diagnostics"], serde_json::json!([]), "{text}");
    }

    // Operator-first items with extra text, markers, or children are
    // invalid shifts, never tasks; the range covers the item.
    for text in ["++3 more", "++3@work", "--2 s:1", "++3++"] {
        let value = parse(text);
        assert_eq!(value["mode"], "pomodoro_shift", "{text}");
        assert!(value.get("pomodoro_shift").is_none(), "{text}: {value}");
        assert_eq!(
            value["diagnostics"][0]["code"], "invalid_pomodoro_shift",
            "{text}: {value}"
        );
    }
    let shape = parse("++3 more");
    assert_eq!(
        shape["diagnostics"][0]["range"],
        serde_json::json!([0, 8]),
        "{shape}"
    );
    let child = parse_stdin("++3\n- detail\n");
    assert_eq!(child["mode"], "pomodoro_shift");
    assert!(child.get("pomodoro_shift").is_none(), "{child}");
    assert_eq!(
        child["diagnostics"][0]["code"], "invalid_pomodoro_shift",
        "{child}"
    );

    // Invalid standalone counts produce a diagnostic on the token range.
    for text in ["++0", "--0"] {
        let value = parse(text);
        assert_eq!(value["mode"], "pomodoro_shift", "{text}");
        assert!(value.get("pomodoro_shift").is_none(), "{text}: {value}");
        assert_eq!(
            value["diagnostics"][0]["code"], "invalid_pomodoro_shift",
            "{text}: {value}"
        );
        assert_eq!(
            value["diagnostics"][0]["range"],
            serde_json::json!([0, 3]),
            "{text}: {value}"
        );
    }
    let overflow = parse("++99999999999999999999999");
    assert_eq!(overflow["mode"], "pomodoro_shift");
    assert!(overflow.get("pomodoro_shift").is_none(), "{overflow}");
    assert_eq!(
        overflow["diagnostics"][0]["code"], "invalid_pomodoro_shift",
        "{overflow}"
    );

    // JSON stays additive at schema version 1.
    assert_eq!(later["schema_version"], 1);
    let ordinary = parse("Call bank @Cash+");
    assert!(ordinary.get("pomodoro_shift").is_none(), "{ordinary}");
    assert!(ordinary.get("pomodoro_adjust").is_none(), "{ordinary}");
    let adjust = parse("+5");
    assert!(adjust.get("pomodoro_shift").is_none(), "{adjust}");
    let pomodoro = parse("Do work @dev:id#bugs");
    assert!(pomodoro.get("pomodoro_shift").is_none(), "{pomodoro}");
    assert!(pomodoro.get("pomodoro_start").is_none(), "{pomodoro}");
}

#[test]
fn capture_parse_pomodoro_shift_human_and_help() {
    let human = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("++3")
        .output()
        .expect("run capture-parse human");
    assert_success(&human);
    let out = stdout(&human);
    assert!(out.contains("pomodoro_shift"), "{out}");
    assert!(out.contains("++3 (15m later, 3 units)"), "{out}");
    assert_stdout_has_no_ansi(&human);

    let bare = run_with_stdin(bob_command().arg("capture-parse"), "--\n");
    assert_success(&bare);
    assert!(
        stdout(&bare).contains("-- (5m earlier, 1 unit)"),
        "{}",
        stdout(&bare)
    );

    let singular = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("-")
        .output()
        .expect("run singular human");
    assert_success(&singular);
    assert!(
        stdout(&singular).contains("- (5m, 1 unit)"),
        "{}",
        stdout(&singular)
    );

    let plain = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("Do work @dev:id#bugs")
        .output()
        .expect("run plain human");
    assert_success(&plain);
    assert!(!stdout(&plain).contains("shift"), "{}", stdout(&plain));

    let parse_help = bob_command()
        .arg("capture-parse")
        .arg("--help")
        .output()
        .expect("run capture-parse help");
    assert_success(&parse_help);
    let help = stdout(&parse_help);
    assert!(
        help.contains("pomodoro_shift")
            && help.contains("++3")
            && help.contains("'--'")
            && help.contains("Modes:")
            && help.contains("pomodoro_shift, pomodoro_link"),
        "expected shift help:\n{help}"
    );

    let capture_help = bob_command()
        .arg("capture")
        .arg("--help")
        .output()
        .expect("run capture help");
    assert_success(&capture_help);
    let capture = stdout(&capture_help);
    assert!(
        capture.contains("++3")
            && capture.contains("--1")
            && capture.contains("bob capture -- --2"),
        "expected capture shift help:\n{capture}"
    );

    let complete_help = bob_command()
        .arg("capture-complete")
        .arg("--help")
        .output()
        .expect("run capture-complete help");
    assert_success(&complete_help);
    assert!(
        stdout(&complete_help).contains("Pomodoro shift"),
        "{}",
        stdout(&complete_help)
    );
}

#[test]
fn capture_parse_pomodoro_start_protocol() {
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

    // Exact starts: bare and counted suffixes mirror `se<X>` timing, with
    // `raw` excluding `=`.
    for (text, raw, duration, offset) in [
        ("=", "", 5, 0),
        ("=3", "3", 3, 0),
        ("=-", "-", 5, 1),
        ("=-2", "-2", 5, 2),
        ("=3-", "3-", 3, 1),
        ("=2-1", "2-1", 2, 1),
        ("=0", "0", 0, 0),
        ("=03", "03", 3, 0),
    ] {
        let value = parse(text);
        assert_eq!(value["schema_version"], 1, "{text}");
        assert_eq!(value["mode"], "pomodoro_start", "{text}");
        assert_eq!(value["body"], text, "{text}");
        assert!(value["route"].is_null(), "{text}: {value}");
        assert!(value["section"].is_null(), "{text}: {value}");
        assert!(value["block_id"].is_null(), "{text}: {value}");
        assert_eq!(value["needs"], serde_json::json!([]), "{text}");
        assert_eq!(
            value["spans"],
            serde_json::json!([{
                "start": 0,
                "end": text.len(),
                "kind": "pomodoro_start"
            }]),
            "{text}: {value}"
        );
        assert_eq!(value["diagnostics"], serde_json::json!([]), "{text}");
        assert_eq!(
            value["pomodoro_start"],
            serde_json::json!({
                "raw": raw,
                "duration_units": duration,
                "offset_units": offset,
            }),
            "{text}: {value}"
        );
        assert!(value.get("pomodoro_adjust").is_none(), "{text}: {value}");
        assert!(value.get("pomodoro_shift").is_none(), "{text}: {value}");
        assert!(value.get("pomodoro_close").is_none(), "{text}: {value}");
        assert!(value.get("items").is_none(), "{text}: {value}");
    }

    // Whitespace around the token is fine; the span covers only the token.
    let padded = parse("  =3  ");
    assert_eq!(padded["mode"], "pomodoro_start");
    assert_eq!(padded["body"], "=3");
    assert_eq!(
        padded["spans"],
        serde_json::json!([{ "start": 2, "end": 4, "kind": "pomodoro_start" }])
    );
    assert_eq!(
        padded["pomodoro_start"],
        serde_json::json!({
            "raw": "3",
            "duration_units": 3,
            "offset_units": 0,
        })
    );

    // Mixed drafts use blank lines; top level previews the first item.
    let mixed = parse_stdin("=3\n\nSecond task\n");
    assert_eq!(mixed["mode"], "pomodoro_start");
    assert_eq!(
        mixed["pomodoro_start"],
        serde_json::json!({
            "raw": "3",
            "duration_units": 3,
            "offset_units": 0,
        })
    );
    assert_eq!(mixed["items"].as_array().expect("items").len(), 2);
    assert_eq!(mixed["items"][0]["mode"], "pomodoro_start");
    assert_eq!(
        mixed["items"][0]["pomodoro_start"],
        serde_json::json!({
            "raw": "3",
            "duration_units": 3,
            "offset_units": 0,
        })
    );
    assert_eq!(mixed["items"][1]["mode"], "task");

    // A `@@` declaration routes ordinary items but never a start.
    let declared = parse_stdin("@@work\nFirst task\n\n=3\n");
    assert_eq!(declared["global_destination"]["route"], "work");
    assert_eq!(declared["items"][0]["mode"], "task");
    assert_eq!(declared["items"][0]["route"], "work");
    assert_eq!(declared["items"][1]["mode"], "pomodoro_start");
    assert!(declared["items"][1]["route"].is_null());
    assert_eq!(
        declared["items"][1]["pomodoro_start"],
        serde_json::json!({
            "raw": "3",
            "duration_units": 3,
            "offset_units": 0,
        })
    );

    let lone_declared = parse_stdin("@@work\n=3\n");
    assert_eq!(lone_declared["mode"], "pomodoro_start");
    assert!(lone_declared["route"].is_null(), "{lone_declared}");
    assert_eq!(
        lone_declared["pomodoro_start"],
        serde_json::json!({
            "raw": "3",
            "duration_units": 3,
            "offset_units": 0,
        })
    );

    // Counted tokens with extra text are invalid starts on the extra text,
    // never tasks.
    for (text, start, end) in [
        ("=3 more", 3, 7),
        ("=-2 @work", 4, 9),
        ("=3x", 2, 3),
        ("=2-1-", 4, 5),
    ] {
        let value = parse(text);
        assert_eq!(value["mode"], "pomodoro_start", "{text}");
        assert!(value.get("pomodoro_start").is_none(), "{text}: {value}");
        assert_eq!(
            value["diagnostics"][0]["code"], "invalid_pomodoro_start",
            "{text}: {value}"
        );
        assert_eq!(
            value["diagnostics"][0]["range"],
            serde_json::json!([start, end]),
            "{text}: {value}"
        );
    }
    let child = parse_stdin("=3\n- detail\n");
    assert_eq!(child["mode"], "pomodoro_start");
    assert!(child.get("pomodoro_start").is_none(), "{child}");
    assert_eq!(
        child["diagnostics"][0]["code"], "invalid_pomodoro_start",
        "{child}"
    );
    let bare_child = parse_stdin("=\n- child\n");
    assert_eq!(bare_child["mode"], "pomodoro_start");
    assert!(bare_child.get("pomodoro_start").is_none(), "{bare_child}");
    assert_eq!(
        bare_child["diagnostics"][0]["code"], "invalid_pomodoro_start",
        "{bare_child}"
    );

    // Oversized values fail on the token range.
    let overflow = parse("=99999999999999999999999");
    assert_eq!(overflow["mode"], "pomodoro_start");
    assert!(overflow.get("pomodoro_start").is_none(), "{overflow}");
    assert_eq!(
        overflow["diagnostics"][0]["code"], "invalid_pomodoro_start",
        "{overflow}"
    );
    assert_eq!(
        overflow["diagnostics"][0]["range"],
        serde_json::json!([0, 24]),
        "{overflow}"
    );

    // Prose lookalikes stay ordinary tasks with no diagnostics.
    for text in [
        "= foo", "=- foo", "==", "=-)", "Plan =3", "a=3", "=xx", "=xa",
        "Plan =x", "Plan =x1",
    ] {
        let value = parse(text);
        assert_eq!(value["mode"], "task", "{text}");
        assert!(value.get("pomodoro_start").is_none(), "{text}: {value}");
        assert!(value.get("pomodoro_close").is_none(), "{text}: {value}");
        assert_eq!(value["diagnostics"], serde_json::json!([]), "{text}");
    }

    // JSON stays additive at schema version 1: other modes omit the spec.
    let ordinary = parse("Call bank @Cash+");
    assert!(ordinary.get("pomodoro_start").is_none(), "{ordinary}");
    let adjust = parse("+5");
    assert!(adjust.get("pomodoro_start").is_none(), "{adjust}");
    let shift = parse("++3");
    assert!(shift.get("pomodoro_start").is_none(), "{shift}");
    let close = parse("=x");
    assert!(close.get("pomodoro_start").is_none(), "{close}");
    let pomodoro = parse("Do work @dev:id#bugs");
    assert!(pomodoro.get("pomodoro_start").is_none(), "{pomodoro}");
}

#[test]
fn capture_parse_named_pomodoro_start_protocol() {
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

    // Exact named starts: `section` carries the typed selector, the spec
    // mirrors `se<X>` timing, and `#` sits in no span.
    for (text, section, raw, duration, offset, prefix_end, name_end) in [
        ("=#bugs", "bugs", "", 5, 0, 1, 6),
        ("=3#bugs", "bugs", "3", 3, 0, 2, 7),
        ("=-2#bugs", "bugs", "-2", 5, 2, 3, 8),
        ("=#deep-work", "deep-work", "", 5, 0, 1, 11),
    ] {
        let value = parse(text);
        assert_eq!(value["schema_version"], 1, "{text}");
        assert_eq!(value["mode"], "pomodoro_start", "{text}");
        assert_eq!(value["body"], text, "{text}");
        assert_eq!(value["section"], section, "{text}: {value}");
        assert_eq!(value["needs"], serde_json::json!([]), "{text}");
        assert_eq!(
            value["spans"],
            serde_json::json!([
                { "start": 0, "end": prefix_end, "kind": "pomodoro_start" },
                {
                    "start": prefix_end + 1,
                    "end": name_end,
                    "kind": "pomodoro_name"
                },
            ]),
            "{text}: {value}"
        );
        assert_eq!(value["diagnostics"], serde_json::json!([]), "{text}");
        assert_eq!(
            value["pomodoro_start"],
            serde_json::json!({
                "raw": raw,
                "duration_units": duration,
                "offset_units": offset,
            }),
            "{text}: {value}"
        );
        assert!(value.get("items").is_none(), "{text}: {value}");
    }

    // `=<X>#` with no name is incomplete, needing the Pomodoro name.
    for (text, raw) in [("=#", ""), ("=3#", "3")] {
        let value = parse(text);
        assert_eq!(value["mode"], "incomplete", "{text}");
        assert_eq!(
            value["needs"],
            serde_json::json!(["pomodoro_name"]),
            "{text}"
        );
        assert!(value["section"].is_null(), "{text}: {value}");
        assert_eq!(
            value["spans"],
            serde_json::json!([
                {
                    "start": 0,
                    "end": 1 + raw.len(),
                    "kind": "pomodoro_start"
                },
                {
                    "start": 1 + raw.len(),
                    "end": 2 + raw.len(),
                    "kind": "interactive_placeholder"
                },
            ]),
            "{text}: {value}"
        );
        assert_eq!(value["diagnostics"], serde_json::json!([]), "{text}");
    }

    // Near misses keep `pomodoro_start` mode with one diagnostic.
    let order = parse("=#bugs=3");
    assert_eq!(order["mode"], "pomodoro_start");
    assert_eq!(order["diagnostics"][0]["code"], "invalid_pomodoro_start");
    assert_eq!(order["diagnostics"][0]["range"], serde_json::json!([2, 8]));
    assert!(
        order["diagnostics"][0]["message"]
            .as_str()
            .expect("message")
            .contains("`=3#bugs` instead of `=#bugs=3`"),
        "{order}"
    );

    let multiword = parse("=#deep work");
    assert_eq!(
        multiword["diagnostics"][0]["code"],
        "invalid_pomodoro_start"
    );
    assert_eq!(
        multiword["diagnostics"][0]["range"],
        serde_json::json!([7, 11])
    );

    let close_hash = parse("=x#bugs");
    assert_eq!(close_hash["mode"], "pomodoro_close");
    assert_eq!(
        close_hash["diagnostics"][0]["code"],
        "invalid_pomodoro_close"
    );
    assert_eq!(
        close_hash["diagnostics"][0]["range"],
        serde_json::json!([2, 7])
    );

    // Chains split per token with absolute ranges.
    let chain = parse_stdin("=x =#bugs\n");
    assert_eq!(chain["items"].as_array().expect("items").len(), 2);
    assert_eq!(chain["items"][0]["mode"], "pomodoro_close");
    assert_eq!(chain["items"][1]["mode"], "pomodoro_start");
    assert_eq!(chain["items"][1]["section"], "bugs");
    assert_eq!(chain["items"][1]["body"], "=#bugs");
    assert_eq!(
        chain["items"][1]["range"],
        serde_json::json!({ "start": 3, "end": 9 })
    );

    // A `@@` declaration never applies to an incomplete named start.
    let declared = parse_stdin("@@work\nFirst task\n\n=#\n");
    assert_eq!(declared["items"][1]["mode"], "incomplete");
    assert!(declared["items"][1]["route"].is_null());
    assert!(declared["items"][1]["pomodoro_start"].is_object());
}

#[test]
fn capture_parse_pomodoro_start_human_and_help() {
    let human = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("=3")
        .output()
        .expect("run capture-parse human");
    assert_success(&human);
    let out = stdout(&human);
    assert!(out.contains("pomodoro_start"), "{out}");
    assert!(out.contains("=3 (15m, offset 0u)"), "{out}");
    assert_stdout_has_no_ansi(&human);

    let bare = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("=")
        .output()
        .expect("run bare human");
    assert_success(&bare);
    assert!(
        stdout(&bare).contains("= (25m, offset 0u)"),
        "{}",
        stdout(&bare)
    );

    // A named start line shows the whole token; `section` keeps its own
    // line. A `@<route>:<block-id>[#<name>]=<X>` marker start keeps the
    // suffix-only line.
    let named = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("=3#bugs")
        .output()
        .expect("run named human");
    assert_success(&named);
    assert!(
        stdout(&named).contains("=3#bugs (15m, offset 0u)"),
        "{}",
        stdout(&named)
    );

    let marked = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("Do work @sase:outline#deep=-2")
        .output()
        .expect("run marked human");
    assert_success(&marked);
    assert!(
        stdout(&marked).contains("=-2 (25m, offset 2u)"),
        "{}",
        stdout(&marked)
    );
    assert!(!stdout(&marked).contains("=-2#deep"), "{}", stdout(&marked));

    let plain = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("Call bank @Cash+")
        .output()
        .expect("run plain human");
    assert_success(&plain);
    assert!(!stdout(&plain).contains("=3 (15m"), "{}", stdout(&plain));

    let parse_help = bob_command()
        .arg("capture-parse")
        .arg("--help")
        .output()
        .expect("run capture-parse help");
    assert_success(&parse_help);
    let help = stdout(&parse_help);
    assert!(
        help.contains("pomodoro_start")
            && help.contains("'='")
            && help.contains("'=3'")
            && help.contains("printf '=x")
            && help.contains("Modes:")
            && help.contains("pomodoro_close, pomodoro_start"),
        "expected start help:\n{help}"
    );

    let capture_help = bob_command()
        .arg("capture")
        .arg("--help")
        .output()
        .expect("run capture help");
    assert_success(&capture_help);
    let capture = stdout(&capture_help);
    assert!(
        capture.contains("bob capture '='")
            && capture.contains("bob capture '=3'")
            && capture.contains("printf '=x")
            && capture.contains("starts the next session")
            && capture.contains("Quote `=` items in zsh"),
        "expected capture start help:\n{capture}"
    );

    let complete_help = bob_command()
        .arg("capture-complete")
        .arg("--help")
        .output()
        .expect("run capture-complete help");
    assert_success(&complete_help);
    assert!(
        stdout(&complete_help).contains("`=`/`=<X>` start"),
        "{}",
        stdout(&complete_help)
    );
}
