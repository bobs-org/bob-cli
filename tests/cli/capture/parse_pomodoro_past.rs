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
fn past_parse_reports_full_contract_and_grammar_14() {
    // Single unnamed past.
    let one = parse("x=3");
    assert_eq!(one["schema_version"], 1);
    assert_eq!(one["grammar"], 26);
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
    assert!(one["pomodoro_past"]["close"].get("size").is_none());
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

    let explicit_complete = parse_human("x=fix=!2");
    assert!(
        explicit_complete.contains("close: complete 2 · defer the rest"),
        "{explicit_complete}"
    );
    let explicit_drop = parse_human("x=fix=~2");
    assert!(
        explicit_drop.contains("close: drop 2 · defer the rest"),
        "{explicit_drop}"
    );

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
        ("x=#fix@3", "x=fix3"),
        ("x=#deep-work@3", "x=3#deep-work"),
        ("x=3#fix", "x=3fix"),
        ("x=#fix", "x=fix"),
        ("x=3#fix@2", "x=3fix"),
        ("x=3#fix@2", "x=fix2"),
        ("x=3@2", "x=3"),
        ("x=3@2", "x=2"),
        ("x=build@3~2", "x=build3~2"),
        ("x=3fix2", "x=3fix"),
        ("x=3fix2", "x=fix2"),
        ("x=deep-work", "x=#deep-work"),
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
fn past_parse_segment_suffix_json_spans_and_runs() {
    // Valid suffix shapes report segment, pin, and lead blocks.
    let tail = parse("x=~");
    assert_eq!(
        tail["pomodoro_past"]["segment"],
        serde_json::json!({"raw": "~", "kind": "tail", "edge": "start"})
    );
    let head = parse("x=~~");
    assert_eq!(
        head["pomodoro_past"]["segment"],
        serde_json::json!({"raw": "~~", "kind": "head", "edge": "end"})
    );
    let indexed = parse("x=fix~2");
    assert_eq!(
        indexed["pomodoro_past"]["segment"],
        serde_json::json!({"raw": "~2", "kind": "index", "index": 2, "edge": "start"})
    );
    let indexed_end = parse("x=fix2~2+");
    assert_eq!(
        indexed_end["pomodoro_past"]["segment"],
        serde_json::json!({"raw": "~2+", "kind": "index", "index": 2, "edge": "end"})
    );
    // Segment span covers the suffix bytes: `x=fix~2` names 5..7.
    let kinds: Vec<(i64, i64, &str)> = indexed["spans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["start"].as_i64().unwrap(),
                s["end"].as_i64().unwrap(),
                s["kind"].as_str().unwrap(),
            )
        })
        .collect();
    assert!(
        kinds.contains(&(5, 7, "pomodoro_past_segment")),
        "{kinds:?}"
    );
    assert!(kinds.contains(&(2, 5, "pomodoro_name")), "{kinds:?}");
    // Repair split is unchanged: `x=2~1!=!1` keeps `E=2~1`.
    let split = parse("x=2~1!=!1");
    assert_eq!(
        split["pomodoro_past"]["segment"],
        serde_json::json!({"raw": "~1", "kind": "index", "index": 1, "edge": "start"})
    );
    assert_eq!(split["pomodoro_past"]["repair"], true);
    assert_eq!(
        split["pomodoro_past"]["close"]["complete"],
        serde_json::json!([1])
    );
    let split_kinds: Vec<&str> = split["spans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["kind"].as_str().unwrap())
        .collect();
    assert!(split_kinds.contains(&"pomodoro_past_segment"));

    // Pinned runs carry pin and lead blocks.
    let pinned = parse("x=a~2 x=b");
    assert_eq!(pinned["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        pinned["items"][0]["pomodoro_past"]["stack"]["pin"],
        serde_json::json!("start")
    );
    assert_eq!(
        pinned["items"][0]["pomodoro_past"]["stack"]["lead_blocks"],
        serde_json::json!(0)
    );
    assert_eq!(
        pinned["items"][1]["pomodoro_past"]["stack"]["pin"],
        serde_json::json!("start")
    );
    assert_eq!(
        pinned["items"][1]["pomodoro_past"]["stack"]["lead_blocks"],
        serde_json::json!(5)
    );

    // Misplaced start suffix reports the run rule with a moved rewrite.
    let misplaced = parse("x=a x=b~2");
    let misplaced_text = serde_json::to_string(&misplaced).unwrap();
    assert!(
        misplaced_text.contains("can't place this run"),
        "{misplaced_text}"
    );
    assert!(misplaced_text.contains("x=a~2 x=b"), "{misplaced_text}");

    // A second suffix in one run reports the second place.
    let double = parse("x=a~2 x=b~1");
    let double_text = serde_json::to_string(&double).unwrap();
    assert!(double_text.contains("adds a second place"), "{double_text}");

    // Unsuffixed JSON stays byte-identical: no segment, pin, or lead blocks.
    let plain = parse("x=3");
    assert!(plain["pomodoro_past"].get("segment").is_none(), "{plain}");
    assert!(
        plain["pomodoro_past"]["stack"].get("pin").is_none(),
        "{plain}"
    );
    assert!(
        plain["pomodoro_past"]["stack"].get("lead_blocks").is_none(),
        "{plain}"
    );
    let plain_run = parse("x=build2 x=fix3");
    for item in plain_run["items"].as_array().unwrap() {
        assert!(item["pomodoro_past"].get("segment").is_none());
        assert!(item["pomodoro_past"]["stack"].get("pin").is_none());
        assert!(item["pomodoro_past"]["stack"].get("lead_blocks").is_none());
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

#[test]
fn past_parse_named_later_offset_teaches_the_hash_spelling() {
    for (text, literal) in [
        ("x=fix+", "x=#fix+"),
        ("x=fix2+", "x=#fix2+"),
        ("x=bugs+", "x=#bugs+"),
        ("x=c++", "x=#c++"),
    ] {
        let value = parse(text);
        assert_eq!(value["mode"], "pomodoro_past", "{text}");
        assert_eq!(
            value["diagnostics"][0]["code"], "invalid_pomodoro_past",
            "{text}: {value}"
        );
        let message = value["diagnostics"][0]["message"]
            .as_str()
            .expect("message");
        assert!(
            message.contains("records finished work"),
            "{text}: {message}"
        );
        assert!(
            message.contains(&format!(
                "spell a name that ends in `+` with `#` (`{literal}`)"
            )),
            "{text}: {message}"
        );
    }
    let legal = parse("x=#c++");
    assert_eq!(legal["mode"], "pomodoro_past");
    assert_eq!(legal["diagnostics"], serde_json::json!([]));
    assert_eq!(legal["pomodoro_past"]["name"], "C++");
}
