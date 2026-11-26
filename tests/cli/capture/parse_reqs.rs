//! `bob capture-parse` requirement bullets: `req_marker` spans with a
//! depth, the shared refusal codes on each bullet's line, and authored
//! levels past two beneath requirement chains.

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
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

/// `(start, end, depth)` for every `req_marker` span.
fn req_markers(value: &serde_json::Value) -> Vec<(u64, u64, u64)> {
    value["spans"]
        .as_array()
        .expect("spans")
        .iter()
        .filter(|span| span["kind"] == "req_marker")
        .map(|span| {
            (
                span["start"].as_u64().expect("start"),
                span["end"].as_u64().expect("end"),
                span["depth"].as_u64().expect("req_marker carries depth"),
            )
        })
        .collect()
}

fn diagnostics(value: &serde_json::Value) -> Vec<(String, u64, u64)> {
    value["diagnostics"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item["code"].as_str().expect("code").to_string(),
                        item["range"][0].as_u64().expect("start"),
                        item["range"][1].as_u64().expect("end"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn at(text: &str, needle: &str) -> u64 {
    text.find(needle).expect("needle in draft") as u64
}

#[test]
fn nested_requirements_report_req_marker_spans_with_depths() {
    let text = "Fix export\n- [ ] Handles CRLF\n  - [ ] Windows\n    - [ ] Mixed files\n      - seen in March\n        - twice\n- see the ticket";
    let value = parse(text);
    assert_eq!(value["grammar"], 33, "{value}");
    assert_eq!(value["mode"], "task", "{value}");
    assert!(diagnostics(&value).is_empty(), "{value}");
    let first = at(text, "[ ] Handles");
    let second = at(text, "[ ] Windows");
    let third = at(text, "[ ] Mixed");
    assert_eq!(
        req_markers(&value),
        vec![
            (first, first + 4, 1),
            (second, second + 4, 2),
            (third, third + 4, 3)
        ],
        "{value}"
    );
    assert_eq!(
        value["sub_bullet_depths"],
        serde_json::json!([1, 2, 3, 4, 5, 1]),
        "{value}"
    );
    // Only `req_marker` spans carry a depth.
    for span in value["spans"].as_array().expect("spans") {
        assert_eq!(
            span.get("depth").is_some(),
            span["kind"] == "req_marker",
            "{span}"
        );
    }
}

#[test]
fn each_refusal_reports_its_code_on_the_bullets_whole_line() {
    for (text, code, line) in [
        ("Fix\n- note\n  - [ ] bad", "req_parent", "  - [ ] bad"),
        ("Fix\n- [x] done", "req_state", "- [x] done"),
        (
            "Fix\n- #future-work\n  - [ ] bad",
            "req_in_drawer",
            "  - [ ] bad",
        ),
        ("@work+ex#notes\n- [ ] bad", "req_in_drawer", "- [ ] bad"),
        ("@work+ex Call Morgan\n- [ ] bad", "req_parent", "- [ ] bad"),
        (
            "Fix\n- note\n  - nested\n    - deep",
            "invalid_child_line",
            "    - deep",
        ),
    ] {
        let value = parse(text);
        let start = at(text, line);
        assert_eq!(
            diagnostics(&value),
            vec![(code.to_string(), start, start + line.len() as u64)],
            "{text:?}: {value}"
        );
        assert!(req_markers(&value).is_empty(), "{text:?}: {value}");
    }
}

#[test]
fn every_problem_is_reported_not_only_the_first() {
    let text = "Fix\n- [x] done\n- note\n  - [ ] bad";
    let value = parse(text);
    let codes: Vec<String> = diagnostics(&value)
        .into_iter()
        .map(|(code, ..)| code)
        .collect();
    assert_eq!(codes, vec!["req_state", "req_parent"], "{value}");
}

#[test]
fn the_depth_bound_and_the_empty_placeholder() {
    let mut text = String::from("Deep");
    for level in 0..17 {
        text.push('\n');
        text.push_str(&"  ".repeat(level));
        text.push_str(&format!("- [ ] level {}", level + 1));
    }
    let value = parse(&text);
    let codes: Vec<String> = diagnostics(&value)
        .into_iter()
        .map(|(code, ..)| code)
        .collect();
    assert_eq!(codes, vec!["req_depth"], "{value}");
    assert_eq!(req_markers(&value).len(), 16, "{value}");

    let text = "Fix\n- [ ] \n- note";
    let value = parse(text);
    assert!(diagnostics(&value).is_empty(), "{value}");
    assert_eq!(req_markers(&value), vec![(6, 10, 1)], "{value}");
    assert_eq!(value["sub_bullets"], serde_json::json!(["note"]), "{value}");
}

#[test]
fn the_same_line_form_marks_the_item_line() {
    let text = "@work+ex [ ] Handles CRLF\n- [ ] sub\n- plain";
    let value = parse(text);
    assert_eq!(value["mode"], "sub_bullet", "{value}");
    let item = at(text, "[ ] Handles");
    let sub = at(text, "[ ] sub");
    assert_eq!(
        req_markers(&value),
        vec![(item, item + 4, 1), (sub, sub + 4, 2)],
        "{value}"
    );
    // The route spans stay beside the marker.
    let kinds: Vec<&str> = value["spans"]
        .as_array()
        .expect("spans")
        .iter()
        .map(|span| span["kind"].as_str().expect("kind"))
        .collect();
    assert!(kinds.contains(&"sub_bullet_route"), "{value}");
}

#[test]
fn task_lists_count_levels_after_the_shift() {
    let text = "@work\n- Task A\n  - [ ] req\n    - [ ] deeper\n      - note\n- Task B";
    let value = parse(text);
    assert!(diagnostics(&value).is_empty(), "{value}");
    assert_eq!(
        value["items"][0]["sub_bullet_depths"],
        serde_json::json!([1, 2, 3])
    );
    let req = at(text, "[ ] req");
    let deeper = at(text, "[ ] deeper");
    assert_eq!(
        req_markers(&value),
        vec![(req, req + 4, 1), (deeper, deeper + 4, 2)],
        "{value}"
    );

    let value = parse("@work\n- Task A\n  - note\n    - deep");
    let codes: Vec<String> = diagnostics(&value)
        .into_iter()
        .map(|(code, ..)| code)
        .collect();
    assert_eq!(codes, vec!["invalid_child_line"], "{value}");
}

#[test]
fn sealed_project_notes_mark_requirements_under_task_rows_only() {
    let text = "@work^ship@ Ship it\n- [ ] Row one\n  - [ ] req\n    - [ ] deeper\n- #notes\n  - [ ] in section";
    let value = parse(text);
    let req = at(text, "[ ] req");
    let deeper = at(text, "[ ] deeper");
    assert_eq!(
        req_markers(&value),
        vec![(req, req + 4, 1), (deeper, deeper + 4, 2)],
        "{value}"
    );
    let line = "  - [ ] in section";
    let start = at(text, line);
    assert_eq!(
        diagnostics(&value),
        vec![(
            "req_in_drawer".to_string(),
            start,
            start + line.len() as u64
        )],
        "{value}"
    );
}

#[test]
fn ineligible_items_keep_checkboxes_as_text() {
    for text in [
        "%n Standup\n- [ ] agenda",
        "@work#Ideas\n- [ ] idea",
        "x\n- [ ] 1 wired the lexer",
    ] {
        let value = parse(text);
        assert!(req_markers(&value).is_empty(), "{text:?}: {value}");
    }
}

#[test]
fn human_output_lists_req_marker_spans() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("Fix\n- [ ] Handles CRLF")
        .output()
        .expect("run capture-parse");
    assert_success(&output);
    assert!(
        stdout(&output).contains("req_marker"),
        "{}",
        stdout(&output)
    );
}
