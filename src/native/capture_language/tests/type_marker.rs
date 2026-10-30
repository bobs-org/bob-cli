use super::super::model::RawLine;
use super::super::type_marker::{
    masked_line, scan_line, scan_line_with_clip_control, TypeMarkerOccurrence,
};
use super::*;

#[test]
fn fixture_vectors_preserve_the_task_body_and_attach_one_type() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/capture_type_marker/vectors.json"
    ))
    .expect("type-marker fixture vectors are valid JSON");
    for vector in vectors.as_array().expect("vector list") {
        let input = vector["input"].as_str().expect("input");
        let parsed = execute(input).expect("valid typed task");
        assert_eq!(parsed.body, vector["body"].as_str().unwrap(), "{input:?}");
        let marker = parsed.type_marker.expect("type marker");
        assert_eq!(marker.name, vector["type"].as_str().unwrap(), "{input:?}");
    }
}

#[test]
fn slug_literal_and_marker_run_keep_original_byte_ranges() {
    let raw = "@sase t:\"DEEP WORK\" Fix 🧰 it s:2 $3";
    let parsed = execute(raw).expect("literal name is a type marker");
    assert_eq!(parsed.body, "Fix 🧰 it");
    let marker = parsed.type_marker.expect("type marker");
    assert_eq!(marker.name, "DEEP WORK");
    assert!(marker.literal);
    assert_eq!(&raw[marker.span], "t:\"DEEP WORK\"");
    assert_eq!(&raw[marker.name_span], "DEEP WORK");

    let ranged = scan_line("Fix it &note:block t:fix s:2 @sase", 40);
    assert!(matches!(
        ranged.as_slice(),
        [TypeMarkerOccurrence::Complete(_)]
    ));
    let TypeMarkerOccurrence::Complete(marker) = &ranged[0] else {
        unreachable!();
    };
    assert_eq!(marker.span.start, 40 + "Fix it &note:block ".len());
}

#[test]
fn incomplete_invalid_and_duplicate_markers_have_teaching_errors() {
    assert!(execute("Fix the parser t:")
        .unwrap_err()
        .contains("`t:` is incomplete"));
    assert!(execute("Fix the parser t:\"DEEP")
        .unwrap_err()
        .contains("`t:` is incomplete"));
    assert!(execute("Fix the parser t:fix.")
        .unwrap_err()
        .contains("is not a task type"));
    assert!(execute("Fix the parser t:\"DEEP WORK\".")
        .unwrap_err()
        .contains("is not a task type"));
    assert!(execute("Fix the parser t:bob+decks")
        .unwrap_err()
        .contains("names two types"));
    assert!(execute("Fix the parser t:fix\n- t:bug")
        .unwrap_err()
        .contains("type marker (t:<type>)"));
}

#[test]
fn escaped_midline_uppercase_and_protected_tokens_remain_body_text() {
    let escaped = execute("\\t:fix Fix the parser").unwrap();
    assert_eq!(escaped.body, "t:fix Fix the parser");
    assert!(escaped.type_marker.is_none());

    for raw in [
        "Ask about t:fix today",
        "Ask about \\t:fix today",
        "Ask about T:FIX today",
        "Write `t:fix` in the docs",
        "Read [[t:fix]] later",
    ] {
        let parsed = execute(raw).expect("unclaimed text stays ordinary prose");
        assert_eq!(parsed.body, raw);
        assert!(parsed.type_marker.is_none(), "{raw:?}");
    }

    let escaped_mask = masked_line(&RawLine {
        text: "\\t:fix Fix it",
        start: 0,
        end: 13,
    });
    assert_eq!(escaped_mask, " t:fix Fix it");

    assert!(scan_line("@dev:focus-x#\"context t:fix\"", 0).is_empty());
    assert!(scan_line("&\"note t:fix\":block", 0).is_empty());
    assert!(scan_line("&\"note t:fix", 0).is_empty());
    assert!(scan_line("~\"note t:fix\":block", 0).is_empty());
}

#[test]
fn only_task_subjects_accept_markers() {
    assert!(execute("t:fix\n- child task")
        .unwrap_err()
        .contains("needs a task"));
    assert!(execute("Write a note @notes#Ideas t:fix")
        .unwrap_err()
        .contains("writes a plain bullet"));
    // Existing targets accept markers now: whole-item links, Ensure Next,
    // toggles, and relationship-only owners retype.
    assert!(execute("^sase:fix-x t:fix").is_ok());
    assert!(execute("@sase+fix-x t:fix").is_ok());
    assert!(execute("@sase+fix-x! t:fix").is_ok());
    // Link-close suffixes still refuse as closes.
    assert!(execute("@sase:fix-x=x t:fix")
        .unwrap_err()
        .contains("closes a Pomodoro"));
}

#[test]
fn marker_only_continuations_stay_with_the_original_item() {
    for input in ["Fix parser\nt:fix", "Fix parser\n- t:fix"] {
        let draft = execute_draft(input).expect("one task item");
        assert_eq!(draft.items.len(), 1, "{input:?}");
        assert_eq!(draft.items[0].parsed.body, "Fix parser");
        assert_eq!(
            draft.items[0].parsed.type_marker.as_ref().unwrap().name,
            "fix"
        );
        assert_eq!(&input[draft.items[0].start..draft.items[0].end], input);
    }
}

#[test]
fn template_rows_keep_type_metadata_and_template_local_offsets() {
    let input = "@@template:\nFix {0} t:fix\n\nparser\n";
    let draft = execute_draft(input).expect("template row");
    let item = &draft.items[0];
    assert_eq!(item.parsed.body, "Fix parser");
    let marker = item.parsed.type_marker.as_ref().unwrap();
    assert_eq!(marker.name, "fix");
    assert_eq!("Fix parser t:fix".get(marker.span.clone()), Some("t:fix"));
}

#[test]
fn url_lists_and_operator_chains_probe_the_masked_line() {
    let urls = "https://example.test/one\nhttps://example.test/two t:fix";
    let url_items = split_capture_draft(urls).items;
    assert_eq!(url_items.len(), 2);
    assert!(url_items[1].end > "https://example.test/two".len());

    let chain = "+2 x t:fix";
    let chain_items = split_capture_draft(chain).items;
    assert_eq!(chain_items.len(), 2);
    assert_eq!(&chain[chain_items[1].start..chain_items[1].end], "x t:fix");
    assert!(execute_draft(chain)
        .unwrap_err()
        .contains("closes a Pomodoro"));

    let inline_close = "+1 x wired the lexer t:fix";
    let close_items = split_capture_draft(inline_close).items;
    assert_eq!(close_items.len(), 2);
    assert_eq!(
        &inline_close[close_items[1].start..close_items[1].end],
        "x wired the lexer t:fix"
    );
    assert!(execute_draft(inline_close)
        .unwrap_err()
        .contains("closes a Pomodoro"));
}

#[test]
fn malformed_masked_operators_keep_their_existing_errors() {
    let plain = execute("+0").unwrap_err();
    let marked = execute("+0 t:fix").unwrap_err();
    assert_eq!(marked, plain);
}

#[test]
fn marker_edge_transparency_follows_clip_parsing_options() {
    let raw = "Fix the parser t:fix %";
    assert_eq!(scan_line(raw, 0).len(), 1);
    assert!(scan_line_with_clip_control(raw, 0, false).is_empty());

    let parsed = parse_capture_text_with_clip_control(
        raw,
        None,
        None,
        &CaptureParseOptions::routing_off(false),
    )
    .expect("without clip parsing, percent remains task text");
    assert_eq!(parsed.body, raw);
    assert!(parsed.type_marker.is_none());
}

#[test]
fn unicode_and_crlf_offsets_index_the_original_draft() {
    let raw = "Fix 🧰 parser\r\n- check 🧪 t:fix\r\n";
    let draft = execute_draft(raw).expect("CRLF draft");
    let marker = draft.items[0].parsed.type_marker.as_ref().unwrap();
    assert_eq!(&raw[marker.span.clone()], "t:fix");
    assert_eq!(draft.items[0].line_end, 2);
    assert_eq!(draft.items[0].parsed.sub_bullets[0].body, "check 🧪");
}
