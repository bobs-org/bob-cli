//! Template-aware capture-parse protocol tests.

use crate::support::*;
use serde_json::Value;

fn parse_json(draft: &str) -> Value {
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

#[test]
fn template_parse_reports_typed_spans_items_and_summary() {
    let draft = "@@template:\nApply to {0}! @route:{1}\n- **URL**: {2}\n\n\"Two Sigma\" role https://example.com/role\n\"Café 🚀\" job https://example.com/job\n";
    let value = parse_json(draft);

    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["body"], "Apply to {0}!");
    assert_eq!(value["mode"], "pomodoro_task");
    assert!(value.get("global_destination").is_none());
    assert_eq!(value["template"]["header"]["line"], 1);
    assert_eq!(value["template"]["field_count"], 3);
    assert_eq!(value["template"]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(value["template"]["rows"][0]["row"], 1);
    assert_eq!(value["template"]["rows"][0]["line"], 5);
    assert_eq!(
        value["template"]["rows"][0]["fields"][0]["value"],
        "Two Sigma"
    );
    assert_eq!(
        value["template"]["rows"][0]["fields"][0]["range"],
        serde_json::json!({
            "start": draft.find("\"Two Sigma\"").unwrap(),
            "end": draft.find("\"Two Sigma\"").unwrap() + "\"Two Sigma\"".len()
        })
    );
    assert_eq!(value["template"]["rows"][0]["mode"], "pomodoro_task");
    assert_eq!(value["template"]["rows"][0]["body"], "Apply to Two Sigma!");
    assert_eq!(value["items"].as_array().unwrap().len(), 3);
    assert_eq!(value["items"][0]["body"], "Apply to {0}!");
    assert_eq!(value["items"][0]["range"], value["template"]["range"]);
    assert_eq!(value["items"][1]["mode"], "template_row");
    assert_eq!(value["items"][1]["body"], "Apply to Two Sigma!");
    assert_eq!(value["items"][1]["needs"], serde_json::json!([]));
    assert_eq!(
        value["items"][1]["template_row"],
        value["template"]["rows"][0]
    );
    assert!(value["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| { entry["code"] != "invalid_global_destination" }));

    let spans = value["spans"].as_array().unwrap();
    assert!(spans.iter().any(|span| span["kind"] == "template_header"));
    assert!(spans.iter().any(|span| {
        span["kind"] == "template_placeholder" && span["field"] == 1
    }));
    assert!(spans.iter().any(|span| {
        span["kind"] == "template_field" && span["field"] == 0
    }));
    let mut previous_end = 0;
    for span in spans {
        let start = span["start"].as_u64().unwrap() as usize;
        let end = span["end"].as_u64().unwrap() as usize;
        assert!(
            start >= previous_end,
            "overlapping or unsorted span: {span}"
        );
        assert!(draft.is_char_boundary(start));
        assert!(draft.is_char_boundary(end));
        previous_end = end;
    }
}

#[test]
fn placeholders_suppress_per_row_picker_needs_but_constant_pickers_remain() {
    for draft in [
        "@@template:\n!{0}\n\nanything\n",
        "@@template:\n:{0}\n\nanything\n",
    ] {
        let value = parse_json(draft);
        assert_eq!(value["needs"], serde_json::json!([]), "{draft}");
        assert_eq!(value["items"][0]["needs"], serde_json::json!([]));
    }

    let value = parse_json("@@template:\nTask {0} &\n\nbody\n");
    assert!(value["needs"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("task_dependency")));

    let value = parse_json("@@template:\nTask @job:{0}\n\nitem\n");
    assert!(!value["needs"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("pomodoro_id")));
}

#[test]
fn template_row_need_and_arity_diagnostic_use_typed_ranges() {
    for draft in ["@@template:\n", "@@template:\nTask {0}\n\n"] {
        let value = parse_json(draft);
        assert!(value["needs"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("template_row")));
    }

    let draft = "@@template:\nTask {0} {1}\n\nonly-one\n";
    let value = parse_json(draft);
    let diagnostic = value["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["code"] == "template_row_arity")
        .expect("arity diagnostic");
    let start = draft.find("only-one").unwrap();
    assert_eq!(diagnostic["range"], serde_json::json!([start, start + 8]));

    let incomplete = parse_json("@@template:\n{0}\n\n:dee\n");
    let row = &incomplete["template"]["rows"][0];
    assert_eq!(row["mode"], "incomplete");
    assert!(incomplete["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["code"] == "template_row_incomplete"));
}

#[test]
fn expanded_row_diagnostics_map_bad_ids_to_fields_and_dedupe_literals() {
    let draft = "@@template:\nDo {0} @jobs:{1}\n\nbuild bad.id\n";
    let value = parse_json(draft);
    let field = value["template"]["rows"][0]["fields"][1]["range"].clone();
    let invalid = value["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["code"].as_str().unwrap().contains("block_id"))
        .expect("bad block-ID diagnostic");
    assert_eq!(invalid["range"][0], field["start"]);
    assert_eq!(invalid["range"][1], field["end"]);

    let repeated =
        parse_json("@@template:\nDo {0} @dev+bad.id\n\none\ntwo\nthree\n");
    let count = repeated["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["code"] == "invalid_sub_bullet_block_id")
        .count();
    assert_eq!(count, 1, "{repeated}");
}

#[test]
fn headerless_placeholder_suggests_template_without_changing_capture_items() {
    let draft = "Apply {0}\n\nNext item";
    let value = parse_json(draft);
    assert_eq!(value["items"].as_array().unwrap().len(), 2);
    assert_eq!(value["diagnostics"][0]["severity"], "warning");
    assert_eq!(value["diagnostics"][0]["code"], "template_header_missing");

    let ordinary = parse_json("Call bank @Cash+");
    assert!(ordinary.get("template").is_none());
    assert!(ordinary
        .get("diagnostics")
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(ordinary["body"], "Call bank");
}

#[test]
fn template_placeholders_split_capture_spans_and_refusals_are_reported() {
    let draft = "@@template:\nCall @rou{0}te+\n\n";
    let value = parse_json(draft);
    let placeholder = draft.find("{0}").unwrap();
    let route_spans = value["spans"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|span| span["kind"] == "sub_bullet_route")
        .collect::<Vec<_>>();
    assert!(route_spans.iter().any(|span| span["end"] == placeholder));
    assert!(route_spans
        .iter()
        .any(|span| span["start"] == placeholder + 3));

    let clipboard = parse_json("@@template:\nDo {0} %\n\nvalue\n");
    assert!(clipboard["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["code"] == "template_clipboard"));

    let session = parse_json("@@template:\n{0}\n\n+2\n");
    assert!(session["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["code"] == "template_session_operator"));

    let misplaced = parse_json("Task\n@@template:\n\nvalue\n");
    assert!(misplaced["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["code"] == "template_header_misplaced"));
    assert!(!misplaced["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["code"] == "invalid_global_destination"));
}

#[test]
fn template_session_suffixes_are_refused_with_typed_ranges() {
    for draft in [
        "@@template:\nTask {0} @job:{1}=3\n\na b\n",
        "@@template:\n^job:{0}=x\n\na\n",
        "@@template:\n^job:{0}=3\n\na\n",
        "@@template:\n{0}\n\n^job:a=x\n",
    ] {
        let value = parse_json(draft);
        let session = value["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["code"] == "template_session_operator")
            .unwrap_or_else(|| panic!("expected session refusal in {draft:?}"));
        let range = session["range"]
            .as_array()
            .unwrap_or_else(|| panic!("expected typed range in {draft:?}"));
        assert_eq!(range.len(), 2);
        let start = range[0].as_u64().unwrap() as usize;
        let end = range[1].as_u64().unwrap() as usize;
        assert!(start < end);
        assert!(draft.is_char_boundary(start));
        assert!(draft.is_char_boundary(end));
        assert!(end <= draft.len());
    }
}

#[test]
fn template_clipboard_follows_expanded_instances() {
    let listen = parse_json("@@template:\n{0} %\n\nhttps://example.com/a\n");
    assert!(
        !listen["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["code"] == "template_clipboard"),
        "URL % stays a Listen reference: {:?}",
        listen["diagnostics"]
    );

    let literal = "@@template:\n{0} task %\n\nvalue1\nvalue2\n";
    let value = parse_json(literal);
    let clipboard: Vec<_> = value["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["code"] == "template_clipboard")
        .collect();
    assert_eq!(clipboard.len(), 1);
    let percent = literal.find('%').unwrap();
    assert_eq!(
        clipboard[0]["range"],
        serde_json::json!([percent, percent + 1])
    );

    let row_clip = parse_json("@@template:\nTask {0}\n\na %\n");
    assert!(
        row_clip["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["code"] == "template_clipboard"),
        "a % typed in a row value is still refused"
    );
}

#[test]
fn human_output_adds_template_summary() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("@@template:\nTask {0} {1}\n\nA B\n")
        .output()
        .expect("run human capture-parse");
    assert_success(&output);
    assert!(stdout(&output).contains("template  2 fields · 1 rows"));
}
