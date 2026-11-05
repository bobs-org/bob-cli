//! Capture-parse `x=` past grammar (phase `bob-cli-h2.1`).

use crate::support::*;
use std::fs;

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
    serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
        .expect("capture-parse JSON")
}

fn parse_human(text: &str) -> String {
    let output = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg(text)
        .output()
        .expect("run capture-parse human");
    assert_success(&output);
    stdout(&output).trim().to_string()
}

#[test]
fn past_parse_reports_full_contract_and_grammar_13() {
    // Single unnamed past.
    let one = parse("x=3");
    assert_eq!(one["schema_version"], 1);
    assert_eq!(one["grammar"], 13);
    assert_eq!(one["mode"], "pomodoro_past");
    assert_eq!(one["body"], "x=3");
    assert_eq!(one["section"], serde_json::Value::Null);
    assert_eq!(
        one["pomodoro_past"],
        serde_json::json!({
            "raw": "x=3",
            "name": null,
            "name_is_literal": false,
            "blocks": 3,
            "minutes": 15,
            "repair": false,
            "stack": { "index": 1, "size": 1, "offset_blocks": 0 },
            "close": { "raw": "", "in_progress": null, "complete": [] },
        })
    );
    // Spans: `x=` plus leading count, no second past, no name/repair/close.
    assert_eq!(
        one["spans"],
        serde_json::json!([{ "start": 0, "end": 3, "kind": "pomodoro_past" }])
    );
    assert_eq!(one["diagnostics"], serde_json::json!([]));
    let human = parse_human("x=3");
    assert!(human.contains("past"), "{human}");
    assert!(human.contains("x=3"), "{human}");
    assert!(human.contains("15m"), "{human}");

    // Named with close selection, repair, and stack of two.
    let two = parse("x=fix3!=*3");
    assert_eq!(two["mode"], "pomodoro_past");
    assert_eq!(two["section"], "FIX");
    assert_eq!(two["pomodoro_past"]["raw"], "x=fix3!=*3");
    assert_eq!(two["pomodoro_past"]["name"], "FIX");
    assert_eq!(two["pomodoro_past"]["blocks"], 3);
    assert_eq!(two["pomodoro_past"]["minutes"], 15);
    assert_eq!(two["pomodoro_past"]["repair"], true);
    assert_eq!(
        two["pomodoro_past"]["close"]["park"],
        serde_json::json!([3])
    );
    // Both `pomodoro_past` spans: `x=` and the trailing `3`, plus
    // name/repair/close.
    let kinds: Vec<&str> = two["spans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["kind"].as_str().unwrap())
        .collect();
    for kind in [
        "pomodoro_past",
        "pomodoro_name",
        "pomodoro_past_repair",
        "pomodoro_close",
        "pomodoro_close_park",
    ] {
        assert!(kinds.contains(&kind), "{kinds:?}");
    }
    let human = parse_human("x=fix3!=*3");
    assert!(human.contains("past"), "{human}");
    assert!(human.contains("x=fix3!=*3"), "{human}");
    assert!(human.contains("FIX"), "{human}");
    assert!(human.contains("repair"), "{human}");

    // Both spellings give byte-identical vault semantics (raw differs).
    let a = parse("x=3fix=*3");
    let b = parse("x=fix3=*3");
    assert_eq!(a["pomodoro_past"]["blocks"], b["pomodoro_past"]["blocks"]);
    assert_eq!(a["pomodoro_past"]["name"], b["pomodoro_past"]["name"]);
    assert_ne!(a["pomodoro_past"]["raw"], b["pomodoro_past"]["raw"]);

    // Stacks across one line.
    let stack = parse("x=build2 x=fix3");
    assert_eq!(stack["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        stack["items"][0]["pomodoro_past"]["stack"],
        serde_json::json!({ "index": 1, "size": 2, "offset_blocks": 3 })
    );
    assert_eq!(
        stack["items"][1]["pomodoro_past"]["stack"],
        serde_json::json!({ "index": 2, "size": 2, "offset_blocks": 0 })
    );

    // `X=3` and mid-body prose stay prose.
    for text in ["X=3", "Plan x=3"] {
        let prose = parse(text);
        assert_ne!(prose["mode"], "pomodoro_past", "{text}");
        assert!(prose.get("pomodoro_past").is_none(), "{text}");
    }
}

#[test]
fn past_parse_teaching_errors_and_incomplete() {
    let cases = vec![
        ("x=3-1", "x=3"),
        ("x=-2", "x="),
        ("x=3-", "x=3"),
        ("x=fix-", "x=fix"),
        ("x=fix3-1", "x=fix3"),
        ("x=3-1fix", "x=3fix"),
        ("x=fix@3", "x=fix3"),
        ("x=build@", "x=build3"),
        ("x=3#fix", "x=3fix"),
        ("x=#fix", "x=fix"),
        ("x=3fix2", "x=3fix"),
        ("x=3fix2", "x=fix2"),
        ("x=deep-work", "x=#deep-work"),
        ("x=~2", "x==~2"),
        ("x=*3", "x==*3"),
        ("x=0", "x=1"),
        ("x=fix0", "x=fix1"),
        ("x=1!2", "x==1!2"),
        ("x=fix!2", "x=fix=!2"),
        ("x=3x", "x=3#"),
    ];
    for (typed, rewrite) in cases {
        let value = parse(typed);
        assert_eq!(value["mode"], "pomodoro_past", "{typed}");
        assert_eq!(
            value["diagnostics"].as_array().unwrap().len(),
            1,
            "{typed}"
        );
        assert_eq!(
            value["diagnostics"][0]["code"], "invalid_pomodoro_past",
            "{typed}"
        );
        let message = value["diagnostics"][0]["message"].as_str().unwrap();
        assert!(message.contains(typed), "{typed}: {message}");
        assert!(message.contains(rewrite), "{typed}: {message}");
    }
    for typed in ["x=#", "x=3#"] {
        let value = parse(typed);
        assert_eq!(value["mode"], "incomplete", "{typed}");
        assert!(!value["needs"].as_array().unwrap().is_empty(), "{typed}");
    }
    for typed in ["x=3=1,", "x==~"] {
        let value = parse(typed);
        assert_eq!(value["mode"], "incomplete", "{typed}");
        assert!(
            value["needs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n == "pomodoro_close_task"),
            "{typed}"
        );
    }
}

#[test]
fn past_capture_records_with_nothing_lost() {
    // Executable records replaced the temporary refusal: a valid `x=`
    // now records instead of refusing, and leaves no stray bytes.
    let temp = TempDir::new("bob-cli-past-records");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day = vault.join("day.md");
    write_file(&day, "## Pomodoros\n- [ ] ()\n");
    let output = bob_command()
        .env("BOB_NOW", "2026-11-04 09:21:00")
        .env("BOB_DAY_FILE", &day)
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("x=3")
        .output()
        .expect("run bob capture");
    assert_success(&output);
    let value: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(value["kind"], "pomodoro_past");
    assert_eq!(value["placement"], "recorded");
    let after = fs::read_to_string(&day).expect("read day after");
    assert!(after.contains("(**0910-0925**"), "{after}");
}
