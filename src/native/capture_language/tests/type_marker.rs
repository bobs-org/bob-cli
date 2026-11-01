use super::super::model::RawLine;
use super::super::type_marker::{
    completion_at_raw, masked_line, scan_line, TypeMarkerOccurrence,
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
    let raw = "@sase #\"DEEP WORK\" Fix 🧰 it s:2 $3";
    let parsed = execute(raw).expect("literal name is a type marker");
    assert_eq!(parsed.body, "Fix 🧰 it");
    let marker = parsed.type_marker.expect("type marker");
    assert_eq!(marker.name, "DEEP WORK");
    assert!(marker.literal);
    assert_eq!(&raw[marker.span], "#\"DEEP WORK\"");
    assert_eq!(&raw[marker.name_span], "DEEP WORK");

    let ranged = scan_line("Fix it &note:block #fix s:2 @sase", 40);
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
fn attached_route_type_completion_keeps_the_marker_attached() {
    let raw = "@cash+task-id #fix";
    let completion = completion_at_raw(raw, raw.len())
        .expect("separate Ensure Next type marker");
    assert_eq!(completion.query, "fix");
    assert!(completion.leading);

    let project_task = "@cash^task-id #fix";
    let completion = completion_at_raw(project_task, project_task.len())
        .expect("separate explicit-ID task type marker");
    assert_eq!(completion.query, "fix");
    assert!(completion.leading);
}

#[test]
fn incomplete_invalid_and_duplicate_markers_have_teaching_errors() {
    assert!(execute("Fix the parser #\"DEEP")
        .unwrap_err()
        .contains("`#` is incomplete"));
    let trailing = scan_line("jot idea #", 0);
    assert!(matches!(
        trailing.as_slice(),
        [TypeMarkerOccurrence::Incomplete { .. }]
    ));
    assert_eq!(&"jot idea #"[trailing[0].span().clone()], "#");
    let error = execute("jot idea #").unwrap_err();
    assert!(
        error.contains("`#` is incomplete: add a task type (`#fix`)"),
        "{error}"
    );
    assert!(
        error.contains("or write `\\#` to keep a literal `#`"),
        "{error}"
    );
    let heading = execute("# Heading text").unwrap_err();
    assert!(heading.contains("`#` is incomplete"), "{heading}");
    let escaped = execute("\\# Heading text").unwrap();
    assert_eq!(escaped.body, "# Heading text");
    assert!(escaped.type_marker.is_none());
    assert!(execute("Fix the parser #fix.")
        .unwrap_err()
        .contains("is not a task type"));
    assert!(execute("Fix the parser #\"DEEP WORK\".")
        .unwrap_err()
        .contains("is not a task type"));
    assert!(execute("Fix the parser #bob+decks")
        .unwrap_err()
        .contains("names two types"));
    assert!(execute("Fix the parser #fix #bug")
        .unwrap_err()
        .contains("type marker (#<type>)"));
    assert!(execute("Fix the parser #fix\n- check\n  - #bug")
        .unwrap_err()
        .contains("type marker (#<type>)"));
}

#[test]
fn digit_heading_dash_and_midline_hashes_stay_prose() {
    for raw in [
        "issue #123",
        "#123 issue",
        "## Heading",
        "#-not-a-type",
        "Ask about # in the middle",
        "glued#hash",
    ] {
        assert!(scan_line(raw, 0).is_empty(), "{raw:?}");
        let parsed = execute(raw).expect("unclaimed hash stays ordinary prose");
        assert_eq!(parsed.body, raw, "{raw:?}");
        assert!(parsed.type_marker.is_none(), "{raw:?}");
    }
}

#[test]
fn escaped_midline_uppercase_and_protected_tokens_remain_body_text() {
    let escaped = execute("\\#fix Fix the parser").unwrap();
    assert_eq!(escaped.body, "#fix Fix the parser");
    assert!(escaped.type_marker.is_none());

    for raw in [
        "Ask about #fix today",
        "Ask about \\#fix today",
        "Ask about T:FIX today",
        "Write `#fix` in the docs",
        "Read [[#fix]] later",
    ] {
        let parsed = execute(raw).expect("unclaimed text stays ordinary prose");
        assert_eq!(parsed.body, raw);
        assert!(parsed.type_marker.is_none(), "{raw:?}");
    }

    let escaped_mask = masked_line(&RawLine {
        text: "\\#fix Fix it",
        start: 0,
        end: 12,
    });
    assert_eq!(escaped_mask, " #fix Fix it");

    assert!(scan_line("@dev:focus-x#\"context #fix\"", 0).is_empty());
    assert!(scan_line("&\"note #fix\":block", 0).is_empty());
    assert!(scan_line("&\"note #fix", 0).is_empty());
    assert!(scan_line("~\"note #fix\":block", 0).is_empty());
}

#[test]
fn only_task_subjects_accept_markers() {
    assert!(execute("#fix\n- child task")
        .unwrap_err()
        .contains("needs a task"));
    assert!(execute("Write a note @notes#Ideas #fix")
        .unwrap_err()
        .contains("writes a plain bullet"));
    // Existing targets accept markers now: whole-item links, Ensure Next,
    // toggles, and relationship-only owners retype.
    assert!(execute("^sase:fix-x #fix").is_ok());
    assert!(execute("@sase+fix-x #fix").is_ok());
    assert!(execute("@sase+fix-x! #fix").is_ok());
    // Link-close suffixes still refuse as closes.
    assert!(execute("@sase:fix-x=x #fix")
        .unwrap_err()
        .contains("closes a Pomodoro"));
}

#[test]
fn marker_only_continuations_stay_with_the_original_item() {
    for input in ["Fix parser\n#fix", "Fix parser\n- check\n  - #fix"] {
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
    let input = "Fix {0} #fix %t\n\nparser\n";
    let draft = execute_draft(input).expect("template row");
    let item = &draft.items[0];
    assert_eq!(item.parsed.body, "Fix parser");
    let marker = item.parsed.type_marker.as_ref().unwrap();
    assert_eq!(marker.name, "fix");
    assert_eq!("Fix parser #fix".get(marker.span.clone()), Some("#fix"));
}

#[test]
fn url_lists_and_operator_chains_probe_the_masked_line() {
    let urls = "https://example.test/one\nhttps://example.test/two #fix";
    let url_items = split_capture_draft(urls).items;
    assert_eq!(url_items.len(), 2);
    assert!(url_items[1].end > "https://example.test/two".len());

    let chain = "+2 x #fix";
    let chain_items = split_capture_draft(chain).items;
    assert_eq!(chain_items.len(), 2);
    assert_eq!(&chain[chain_items[1].start..chain_items[1].end], "x #fix");
    assert!(execute_draft(chain)
        .unwrap_err()
        .contains("closes a Pomodoro"));

    let inline_close = "+1 x wired the lexer #fix";
    let close_items = split_capture_draft(inline_close).items;
    assert_eq!(close_items.len(), 2);
    assert_eq!(
        &inline_close[close_items[1].start..close_items[1].end],
        "x wired the lexer #fix"
    );
    assert!(execute_draft(inline_close)
        .unwrap_err()
        .contains("closes a Pomodoro"));
}

#[test]
fn malformed_masked_operators_keep_their_existing_errors() {
    let plain = execute("+0").unwrap_err();
    let marked = execute("+0 #fix").unwrap_err();
    assert_eq!(marked, plain);
}

#[test]
fn marker_edge_transparency_includes_clip_directives() {
    let raw = "Fix the parser #fix %c";
    assert_eq!(scan_line(raw, 0).len(), 1);

    let parsed = parse_capture_text_with_options(
        raw,
        None,
        None,
        &CaptureParseOptions::routing_off(),
    )
    .expect("clip directives stay transparent next to type markers");
    assert_eq!(parsed.body, "Fix the parser");
    assert!(parsed.type_marker.is_some());
    assert_eq!(parsed.clip, Some(ClipRequest::Current));
}

#[test]
fn unicode_and_crlf_offsets_index_the_original_draft() {
    let raw = "Fix 🧰 parser\r\n- check 🧪 #fix\r\n";
    let draft = execute_draft(raw).expect("CRLF draft");
    let marker = draft.items[0].parsed.type_marker.as_ref().unwrap();
    assert_eq!(&raw[marker.span.clone()], "#fix");
    assert_eq!(draft.items[0].line_end, 2);
    assert_eq!(draft.items[0].parsed.sub_bullets[0].body, "check 🧪");
}
