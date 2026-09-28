//! Draft splitting and authored-child tests.

use super::super::draft::*;
use super::super::editor_model::*;
use super::super::model::*;
use super::*;

#[test]
fn split_physical_lines_treats_lf_crlf_and_bare_cr_as_terminators() {
    assert_eq!(line_texts("a\nb"), vec!["a", "b"]);
    assert_eq!(line_texts("a\r\nb"), vec!["a", "b"]);
    assert_eq!(line_texts("a\rb"), vec!["a", "b"]);
    assert_eq!(line_texts("a\r\n\rb\n\r\nc"), vec!["a", "", "b", "", "c"]);
}

#[test]
fn split_physical_lines_drops_only_one_trailing_terminator() {
    assert_eq!(line_texts("a\n"), vec!["a"]);
    assert_eq!(line_texts("a\r\n"), vec!["a"]);
    assert_eq!(line_texts("a\n\n"), vec!["a", ""]);
    assert_eq!(line_texts(""), Vec::<&str>::new());
}

#[test]
fn split_physical_lines_reports_byte_offsets_excluding_terminators() {
    let raw = "ab\r\ncd\nef";
    let lines = split_physical_lines(raw);
    assert_eq!(
        lines
            .iter()
            .map(|line| (line.text, line.start, line.end))
            .collect::<Vec<_>>(),
        vec![("ab", 0, 2), ("cd", 4, 6), ("ef", 7, 9)]
    );
    for line in &lines {
        assert_eq!(&raw[line.start..line.end], line.text);
    }
}

#[test]
fn split_capture_draft_reports_ranges_and_ignores_separator_runs() {
    let raw = " \nfirst\n- child\n\n\nsecond @work\r\n\r\nthird";
    let items = split_capture_draft(raw).items;
    let summaries = items
        .iter()
        .map(|item| {
            (
                item.index,
                item.line_start,
                item.line_end,
                &raw[item.start..item.end],
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        summaries,
        vec![
            (0, 2, 3, "first\n- child"),
            (1, 6, 6, "second @work"),
            (2, 8, 8, "third"),
        ]
    );
}

#[test]
fn authored_line_classifier_accepts_first_level_and_nested_items() {
    for (raw, body, depth, body_start) in [
        ("- body", "body", AuthoredDepth::First, 2),
        ("* body", "body", AuthoredDepth::First, 2),
        ("+ body", "body", AuthoredDepth::First, 2),
        ("-\tbody", "body", AuthoredDepth::First, 2),
        ("-   body", "body", AuthoredDepth::First, 4),
        ("  - nested", "nested", AuthoredDepth::Nested, 4),
        ("  * nested", "nested", AuthoredDepth::Nested, 4),
        ("  +\tnested", "nested", AuthoredDepth::Nested, 4),
    ] {
        let line = RawLine {
            text: raw,
            start: 10,
            end: 10 + raw.len(),
        };
        let AuthoredLineClass::Item(item) = classify_authored_line(line) else {
            panic!("expected item for {raw:?}");
        };
        assert_eq!(item.body, body, "{raw}");
        assert_eq!(item.depth, depth, "{raw}");
        assert_eq!(item.body_start, 10 + body_start, "{raw}");
    }
}

#[test]
fn authored_line_classifier_accepts_placeholders_without_items() {
    for raw in ["", "   ", "- ", "-\t", "-", " -", "  -", "  - "] {
        let line = RawLine {
            text: raw,
            start: 0,
            end: raw.len(),
        };
        assert_eq!(
            classify_authored_line(line),
            AuthoredLineClass::EmptyOrPlaceholder,
            "{raw:?}"
        );
    }
}

#[test]
fn authored_line_classifier_rejects_every_other_shape() {
    for raw in [
        "body",
        " - indented",
        "   - too deep",
        "\t- tabbed",
        "-body",
        "#body",
    ] {
        let line = RawLine {
            text: raw,
            start: 0,
            end: raw.len(),
        };
        assert_eq!(
            classify_authored_line(line),
            AuthoredLineClass::Invalid,
            "{raw:?}"
        );
    }
}

#[test]
fn execution_renders_authored_children_in_source_order() {
    let parsed = execute("@work parent line\n- first child\n- second child")
        .expect("parse");
    assert_eq!(parsed.body, "parent line");
    assert_eq!(parsed.route.as_deref(), Some("work"));
    assert_eq!(
        sub_bullet_bodies(&parsed.sub_bullets),
        vec!["first child", "second child"]
    );
    assert_eq!(sub_bullet_depths(&parsed.sub_bullets), vec![1, 1]);
}

#[test]
fn execution_tracks_nested_children_under_the_nearest_first_level_owner() {
    let parsed = execute(
        "@work parent line\n- first child\n  - first detail\n- second child\n  - second detail",
    )
    .expect("parse");
    assert_eq!(
        sub_bullet_bodies(&parsed.sub_bullets),
        vec![
            "first child",
            "first detail",
            "second child",
            "second detail"
        ]
    );
    assert_eq!(sub_bullet_depths(&parsed.sub_bullets), vec![1, 2, 1, 2]);
}

#[test]
fn execution_nested_placeholders_do_not_require_or_clear_an_owner() {
    let parsed =
        execute("parent\n  - \n- first child\n  -\n  - detail").expect("parse");
    assert_eq!(
        sub_bullet_bodies(&parsed.sub_bullets),
        vec!["first child", "detail"]
    );
    assert_eq!(sub_bullet_depths(&parsed.sub_bullets), vec![1, 2]);
}

#[test]
fn execution_treats_crlf_and_bare_cr_children_like_lf() {
    for raw in [
        "@work parent\n- child one\n- child two",
        "@work parent\r\n- child one\r\n- child two",
        "@work parent\r- child one\r- child two",
    ] {
        let parsed = execute(raw).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(parsed.body, "parent");
        assert_eq!(
            sub_bullet_bodies(&parsed.sub_bullets),
            vec!["child one", "child two"],
            "{raw}"
        );
        assert_eq!(sub_bullet_depths(&parsed.sub_bullets), vec![1, 1]);
    }
}

#[test]
fn execution_skips_placeholder_child_lines() {
    let parsed = execute("parent\n- real child\n- \n-\t\n").expect("parse");
    assert_eq!(sub_bullet_bodies(&parsed.sub_bullets), vec!["real child"]);
}

#[test]
fn execution_single_item_parser_rejects_blank_line_batches() {
    let error = execute("parent\n\nsecond item").unwrap_err();
    assert_eq!(
        error,
        "capture text contains multiple blank-line-separated items"
    );
}

#[test]
fn execution_batch_parser_prefixes_item_and_line_context() {
    let error = parse_capture_draft_with_clip_control(
        "parent\n\nsecond\n  - orphan",
        None,
        None,
        true,
    )
    .unwrap_err();
    assert_eq!(
        error,
        "capture item 2 starting on line 3: capture line 4 is a nested bullet but has no preceding first-level authored bullet to attach to"
    );
}

#[test]
fn execution_rejects_indented_or_deeper_child_lines() {
    let error = execute("parent\n   - too deep").unwrap_err();
    assert_eq!(
        error,
        "capture line 2 must be a column-zero bullet or a two-space nested \
bullet using \"-\", \"*\", or \"+\" followed by a space or tab, or be left \
blank"
    );
}

#[test]
fn execution_rejects_orphaned_nested_child_lines() {
    let error = execute("parent\n  - orphan").unwrap_err();
    assert_eq!(
        error,
        "capture line 2 is a nested bullet but has no preceding first-level \
authored bullet to attach to"
    );
}

#[test]
fn execution_rejects_nonbullet_continuation_prose() {
    let error =
        execute("parent\n- real child\ncontinuation prose").unwrap_err();
    assert!(error.contains("capture line 3"), "{error}");
}

#[test]
fn execution_rejects_a_child_emptied_by_marker_removal() {
    let error = execute("parent\n- s:1").unwrap_err();
    assert_eq!(
        error,
        "capture line 2 has no text left after its capture markers \
were removed"
    );

    let error = execute("parent\n- p:2\n- @work").unwrap_err();
    assert_eq!(
        error,
        "capture line 2 has no text left after its capture markers \
were removed"
    );
}

#[test]
fn execution_composes_a_trailing_marker_from_any_child_line() {
    let parsed = execute(
        "Prepare the launch review\n- Confirm the owner\n- Attach the checklist @work p:1 s:2",
    )
    .expect("parse");
    assert_eq!(parsed.body, "Prepare the launch review");
    assert_eq!(parsed.route.as_deref(), Some("work"));
    assert_eq!(parsed.priority_level, Some(1));
    assert_eq!(parsed.scheduled_offset, Some(2));
    assert_eq!(
        sub_bullet_bodies(&parsed.sub_bullets),
        vec!["Confirm the owner", "Attach the checklist"]
    );
    assert_eq!(sub_bullet_depths(&parsed.sub_bullets), vec![1, 1]);
}

#[test]
fn execution_rejects_duplicate_route_markers_across_lines() {
    let error = execute("@work parent\n- child @home").unwrap_err();
    assert!(error.contains("route/mode marker"), "{error}");
    assert!(error.contains("only one line"), "{error}");
}

#[test]
fn execution_rejects_duplicate_schedule_priority_and_clip_markers_across_lines()
{
    assert!(execute("@work parent s:1\n- child s:2")
        .unwrap_err()
        .contains("schedule marker"));
    assert!(execute("@work parent p:1\n- child p:2")
        .unwrap_err()
        .contains("priority marker"));
    assert!(execute("@work parent %\n- child %")
        .unwrap_err()
        .contains("clipboard marker"));
}

#[test]
fn execution_allows_the_same_marker_kind_once_across_the_whole_draft() {
    let parsed =
        execute("parent s:1\n- child one\n- child two p:3").expect("parse");
    assert_eq!(parsed.scheduled_offset, Some(1));
    assert_eq!(parsed.priority_level, Some(3));
}

#[test]
fn execution_preserves_unicode_child_bodies() {
    let parsed = execute("café parent\n- \u{1f680} launch\n- \u{e9}tude")
        .expect("parse");
    assert_eq!(parsed.body, "café parent");
    assert_eq!(
        sub_bullet_bodies(&parsed.sub_bullets),
        vec!["\u{1f680} launch", "\u{e9}tude"]
    );
}

#[test]
fn execution_forced_route_keeps_child_markers_literal() {
    let parsed = parse_capture_text_with_clip_control(
        "parent\n- child @home",
        Some("work"),
        None,
        true,
    )
    .expect("parse");
    assert_eq!(parsed.route.as_deref(), Some("work"));
    assert_eq!(sub_bullet_bodies(&parsed.sub_bullets), vec!["child @home"]);
}

#[test]
fn editor_reports_sub_bullets_for_a_multiline_draft() {
    let parse = editor("@work parent\n- first child\n- second child");
    assert_eq!(parse.body, "parent");
    assert_eq!(parse.route.as_deref(), Some("work"));
    assert_eq!(
        sub_bullet_bodies(&parse.sub_bullets),
        vec!["first child", "second child"]
    );
    assert_eq!(sub_bullet_depths(&parse.sub_bullets), vec![1, 1]);
    assert!(parse.diagnostics.is_empty());
}

#[test]
fn editor_reports_nested_sub_bullets_and_depths() {
    let parse = editor(
        "@work parent\n- first child\n  - first detail\n- second child\n  - second detail",
    );
    assert_eq!(
        sub_bullet_bodies(&parse.sub_bullets),
        vec![
            "first child",
            "first detail",
            "second child",
            "second detail"
        ]
    );
    assert_eq!(sub_bullet_depths(&parse.sub_bullets), vec![1, 2, 1, 2]);
    assert!(parse.diagnostics.is_empty());
}

#[test]
fn editor_diagnoses_an_invalid_child_line_without_failing() {
    let parse = editor("parent\n   - nested");
    assert_eq!(parse.body, "parent");
    assert!(parse.sub_bullets.is_empty());
    assert_eq!(codes(&parse), vec!["invalid_child_line"]);
    let raw = "parent\n   - nested";
    let expected_start = raw.find("   - nested").expect("line 2 offset");
    assert_eq!(
        parse.diagnostics[0].range,
        Some((expected_start, raw.len()))
    );
}

#[test]
fn editor_diagnoses_an_orphaned_nested_child_without_failing() {
    let raw = "parent\n  - orphan";
    let parse = editor(raw);
    assert_eq!(parse.body, "parent");
    assert!(parse.sub_bullets.is_empty());
    assert_eq!(codes(&parse), vec!["orphaned_nested_bullet"]);
    let expected_start = raw.find("  - orphan").expect("line 2 offset");
    assert_eq!(
        parse.diagnostics[0].range,
        Some((expected_start, raw.len()))
    );
}

#[test]
fn editor_diagnoses_a_child_emptied_by_marker_removal() {
    let raw = "parent\n- s:1";
    let parse = editor(raw);
    assert!(parse.sub_bullets.is_empty());
    assert_eq!(codes(&parse), vec!["empty_child_after_markers"]);
    let line2_start = raw.find("- s:1").expect("line 2 offset");
    assert_eq!(parse.diagnostics[0].range, Some((line2_start, raw.len())));
}

#[test]
fn editor_diagnoses_duplicate_markers_across_lines_but_keeps_the_first() {
    let parse = editor("@work parent\n- child @home");
    assert_eq!(parse.route.as_deref(), Some("work"));
    assert_eq!(codes(&parse), vec!["duplicate_capture_marker"]);
    assert!(
        parse.diagnostics[0].message.contains("route/mode marker"),
        "{:?}",
        parse.diagnostics
    );

    let parse = editor("parent s:1\n- child s:2");
    assert_eq!(codes(&parse), vec!["duplicate_capture_marker"]);
    assert!(parse.diagnostics[0].message.contains("schedule marker"));
}

#[test]
fn editor_child_line_markers_extend_spans_with_absolute_offsets() {
    let raw = "parent\n- child @work";
    let parse = editor(raw);
    assert_eq!(parse.route.as_deref(), Some("work"));
    let route_span = parse
        .spans
        .iter()
        .find(|span| span.kind == SpanKind::Route)
        .expect("route span");
    assert_eq!(&raw[route_span.start..route_span.end], "@work");
}

#[test]
fn editor_placeholder_child_lines_produce_no_sub_bullet_or_diagnostic() {
    let parse = editor("parent\n- real child\n- \n");
    assert_eq!(sub_bullet_bodies(&parse.sub_bullets), vec!["real child"]);
    assert!(parse.diagnostics.is_empty());
}

#[test]
fn editor_child_line_alone_can_resolve_the_capture_mode() {
    // The parent has no marker of its own; the child's trailing marker
    // becomes the whole capture's mode, exactly like execution.
    let parse = editor("plain parent\n- do it @dev:focus-1");
    assert_eq!(parse.mode, EditorMode::PomodoroTask);
    assert_eq!(parse.route.as_deref(), Some("dev"));
    assert_eq!(parse.block_id.as_deref(), Some("focus-1"));
    assert_eq!(sub_bullet_bodies(&parse.sub_bullets), vec!["do it"]);
}

#[test]
fn execution_rejects_duplicate_global_declarations_by_line() {
    let error = parse_capture_draft_with_clip_control(
        "@@foo\nBuy milk @@bar",
        None,
        None,
        true,
    )
    .unwrap_err();
    assert!(error.contains("duplicate global destination"), "{error}");
    assert!(error.contains("line 1"), "{error}");
    assert!(error.contains("line 2"), "{error}");
}
