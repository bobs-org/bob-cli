//! Merge-in (`<`) scanner, execution resolver, and editor contract tests.
//!
//! Covers MI7 (context table), MI8 (lexical claim table), escapes, MI13
//! parse (task-list bullets), and MI14 lexical (source-also-marked).

use super::super::editor_model::Need;
use super::super::merge_in::*;
use super::super::model::RawLine;
use super::super::SpanKind;

fn parent_line(text: &str) -> RawLine<'_> {
    RawLine {
        text,
        start: 0,
        end: text.len(),
    }
}

fn scan(text: &str) -> MergeInScan {
    scan_merge_in_line(&parent_line(text), text, false)
}

#[test]
fn valid_merge_in_claims_two_sources_in_order() {
    let scan = scan("New outcome @foo^great < @foo+f @bar+b");
    let MergeInScan::Valid { sources, arrow } = scan else {
        panic!("expected valid, got {scan:?}");
    };
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0].marker, "@foo+f");
    assert_eq!(sources[0].route, "foo");
    assert_eq!(sources[0].block_id, "f");
    assert_eq!(sources[1].marker, "@bar+b");
    assert_eq!(arrow.start, "New outcome @foo^great ".len());
}

#[test]
fn terminal_arrow_is_incomplete() {
    let scan = scan("New outcome @foo^great <");
    let MergeInScan::Incomplete { sources, .. } = scan else {
        panic!("expected incomplete, got {scan:?}");
    };
    assert!(sources.is_empty());
}

#[test]
fn bare_route_and_plus_are_incomplete() {
    for input in [
        "New outcome < @foo",
        "New outcome < @foo+",
        "New outcome < +fla",
        "New outcome < +",
        "New outcome < @foo+f @bar+",
    ] {
        let scan = scan(input);
        assert!(
            matches!(scan, MergeInScan::Incomplete { .. }),
            "expected incomplete for {input:?}, got {scan:?}"
        );
    }
}

#[test]
fn prose_comparisons_stay_unclaimed() {
    for input in [
        "Latency < 50ms @foo",
        "a < b",
        "x <= y",
        "a << b",
        "Fix <https://example.com>",
    ] {
        assert!(
            matches!(scan(input), MergeInScan::Unclaimed),
            "expected unclaimed for {input:?}"
        );
    }
}

#[test]
fn later_real_clause_claims_after_prose_arrow() {
    // First `<` is prose (`b` not claimable), later `< @a+x` claims.
    let scan = scan("a < b < @a+x");
    assert!(
        matches!(scan, MergeInScan::Valid { .. }),
        "expected valid, got {scan:?}"
    );
}

#[test]
fn protected_markup_does_not_claim() {
    // Code span containing a merge-like clause stays prose when the whole
    // line is protected. (Per-line protection is checked via the vault
    // link index; here we at least pin the unprotected valid case.)
    let scan = scan("New task < @a+x");
    assert!(matches!(scan, MergeInScan::Valid { .. }));
}

#[test]
fn glued_arrows_claim_spacing_error() {
    for input in ["text<@a+x", "text <@a+x", "text< @a+x"] {
        let scan = scan(input);
        let MergeInScan::Invalid { code, rewrite, .. } = scan else {
            panic!("expected invalid for {input:?}, got {scan:?}");
        };
        assert_eq!(code, MERGE_IN_ARROW_SPACING_CODE);
        let rewrite = rewrite.expect("spacing rewrite");
        assert_eq!(rewrite.label, "Add spaces");
    }
}

#[test]
fn glued_non_sources_stay_prose() {
    for input in ["a <= b", "a => b", "x<3", "a << b"] {
        assert!(
            matches!(scan(input), MergeInScan::Unclaimed),
            "expected unclaimed for {input:?}"
        );
    }
}

#[test]
fn duplicate_sources_refuse() {
    let scan = scan("New outcome < @foo+f @foo+f");
    let MergeInScan::Invalid { code, .. } = scan else {
        panic!("expected invalid, got {scan:?}");
    };
    assert_eq!(code, "duplicate_capture_target");
}

#[test]
fn source_marker_and_suffix_reuse_transfer_codes() {
    for (input, code) in [
        ("New outcome < @foo:f", "transfer_source_marker"),
        ("New outcome < @foo^f", "transfer_source_marker"),
        ("New outcome < @foo+f#d", "transfer_source_suffix"),
        ("New outcome < @foo+f!", "transfer_source_suffix"),
    ] {
        let scan = scan(input);
        let MergeInScan::Invalid { code: actual, .. } = scan else {
            panic!("expected invalid for {input:?}, got {scan:?}");
        };
        assert_eq!(actual, code, "wrong code for {input:?}");
    }
}

#[test]
fn extra_text_has_move_before_rewrite() {
    let scan = scan("New outcome < @foo+f s:3");
    let MergeInScan::Invalid { code, rewrite, .. } = scan else {
        panic!("expected invalid, got {scan:?}");
    };
    assert_eq!(code, MERGE_IN_EXTRA_TEXT_CODE);
    let rewrite = rewrite.expect("extra-text rewrite");
    assert_eq!(rewrite.label, "Move before <");
    // Re-parsing the rewrite yields a valid clause.
    let reparsed = scan_merge_in_line(
        &parent_line(&rewrite.replacement),
        &rewrite.replacement,
        false,
    );
    assert!(
        matches!(reparsed, MergeInScan::Valid { .. }),
        "rewrite did not fix: {reparsed:?}"
    );
}

#[test]
fn second_arrow_is_extra_text() {
    let scan = scan("New < @a+x < @b+y");
    assert!(
        matches!(
            scan,
            MergeInScan::Invalid {
                code: MERGE_IN_EXTRA_TEXT_CODE,
                ..
            }
        ),
        "got {scan:?}"
    );
}

#[test]
fn escape_keeps_tail_literal() {
    let scan = scan("Escaped \\< @foo+f @work");
    assert!(matches!(scan, MergeInScan::Escaped { .. }), "got {scan:?}");
    // Execution keeps the literal tail in the body.
    let parsed = super::super::parse_capture_draft_with_options_coded(
        "Escaped \\< @foo+f @work",
        None,
        None,
        &super::super::CaptureParseOptions::routing_off(),
    )
    .expect("escape parses");
    assert_eq!(parsed.items.len(), 1);
    assert!(
        parsed.items[0].parsed.body.contains("< @foo+f"),
        "body was {:?}",
        parsed.items[0].parsed.body
    );
    assert!(parsed.items[0].parsed.merge_in.is_none());
}

#[test]
fn execution_attaches_clause_to_single_new_task() {
    let parsed = super::super::parse_capture_draft_with_options_coded(
        "New outcome @foo^great < @foo+f @bar+b",
        None,
        None,
        &super::super::CaptureParseOptions::routing_off(),
    )
    .expect("valid merge-in parses");
    assert_eq!(parsed.items.len(), 1);
    let clause = parsed.items[0]
        .parsed
        .merge_in
        .as_ref()
        .expect("clause attached");
    assert_eq!(clause.sources.len(), 2);
    assert_eq!(clause.sources[0].marker, "@foo+f");
}

#[test]
fn execution_incomplete_uses_approved_code() {
    let error = super::super::parse_capture_draft_with_options_coded(
        "New outcome @foo^great <",
        None,
        None,
        &super::super::CaptureParseOptions::routing_off(),
    )
    .expect_err("terminal < refuses");
    assert_eq!(error.code, Some(INCOMPLETE_MERGE_IN_CODE));
}

#[test]
fn execution_needs_new_task_for_existing_destination() {
    let error = super::super::parse_capture_draft_with_options_coded(
        "@foo+t < @a+x @b+y",
        None,
        None,
        &super::super::CaptureParseOptions::routing_off(),
    )
    .expect_err("existing task refuses");
    assert_eq!(error.code, Some(MERGE_IN_NEEDS_NEW_TASK_CODE));
}

#[test]
fn execution_needs_new_task_for_empty_left() {
    let error = super::super::parse_capture_draft_with_options_coded(
        "< @a+x",
        None,
        None,
        &super::super::CaptureParseOptions::routing_off(),
    )
    .expect_err("empty left refuses");
    assert_eq!(error.code, Some(MERGE_IN_NEEDS_NEW_TASK_CODE));
}

#[test]
fn execution_source_also_marked_refuses() {
    let error = super::super::parse_capture_draft_with_options_coded(
        "New task &a:x < @a+x",
        None,
        None,
        &super::super::CaptureParseOptions::routing_off(),
    )
    .expect_err("source also marked refuses");
    assert_eq!(error.code, Some(MERGE_IN_SOURCE_ALSO_MARKED_CODE));
    for (input, present) in [
        ("New task *a:x < @a+x", true),
        ("New task ~a:x < @a+x", true),
        ("New task ~~a:x < @a+x", true),
        ("New task &b:y < @a+x", false),
    ] {
        let result = super::super::parse_capture_draft_with_options_coded(
            input,
            None,
            None,
            &super::super::CaptureParseOptions::routing_off(),
        );
        if present {
            let error = result.expect_err("should refuse");
            assert_eq!(
                error.code,
                Some(MERGE_IN_SOURCE_ALSO_MARKED_CODE),
                "wrong code for {input:?}"
            );
        } else {
            assert!(result.is_ok(), "should allow {input:?}");
        }
    }
}

#[test]
fn execution_case_folds_routes_for_also_marked() {
    let error = super::super::parse_capture_draft_with_options_coded(
        "New task &A:x < @a+x",
        None,
        None,
        &super::super::CaptureParseOptions::routing_off(),
    )
    .expect_err("case-folded match refuses");
    assert_eq!(error.code, Some(MERGE_IN_SOURCE_ALSO_MARKED_CODE));
}

#[test]
fn task_list_bullets_claim_per_bullet() {
    let parsed = super::super::parse_capture_draft_with_options_coded(
        "@work\n- first < @a+x\n- second < @b+y\n",
        None,
        None,
        &super::super::CaptureParseOptions::routing_off(),
    )
    .expect("list bullets parse");
    assert_eq!(parsed.items.len(), 2);
    for item in &parsed.items {
        assert!(
            item.parsed.merge_in.is_some(),
            "bullet missing clause: {item:?}"
        );
    }
    // Absolute source ranges index the exact draft.
    let first = parsed.items[0]
        .parsed
        .merge_in
        .as_ref()
        .expect("first clause");
    assert_eq!(first.sources.len(), 1);
    let (start, end) = (first.sources[0].start, first.sources[0].end);
    assert_eq!(
        &"@work\n- first < @a+x\n- second < @b+y\n"[start..end],
        "@a+x"
    );
}

#[test]
fn editor_valid_keeps_task_mode_with_spans_and_model() {
    let parsed = super::super::parse_for_editor("New task < @a+x @b+y");
    assert_eq!(parsed.mode, super::super::EditorMode::Task);
    assert!(parsed.needs.is_empty());
    assert!(parsed.diagnostics.is_empty());
    let kinds = parsed
        .spans
        .iter()
        .map(|span| span.kind)
        .collect::<Vec<_>>();
    assert!(kinds.contains(&SpanKind::MergeInArrow));
    assert!(kinds.contains(&SpanKind::MergeInSource));
    assert!(!kinds.contains(&super::super::SpanKind::TransferSource));
    let merge_in = parsed.merge_in.expect("editor merge_in");
    assert_eq!(merge_in.sources.len(), 2);
    assert_eq!(merge_in.sources[0].marker, "@a+x");
}

#[test]
fn editor_pending_has_need_and_placeholder_without_diagnostic() {
    let parsed = super::super::parse_for_editor("New task < @foo");
    assert!(parsed.needs.contains(&Need::MergeInSource));
    assert!(parsed.spans.iter().any(
        |span| span.kind == super::super::SpanKind::InteractivePlaceholder
    ));
    assert!(parsed.diagnostics.is_empty());
    // Complete sources are kept on pending input.
    let pending = super::super::parse_for_editor("New task < @a+x @foo");
    let merge_in = pending.merge_in.expect("pending merge_in");
    assert_eq!(merge_in.sources.len(), 1);
    assert_eq!(merge_in.sources[0].marker, "@a+x");
}

#[test]
fn editor_needs_new_task_carries_use_rewrite() {
    let parsed = super::super::parse_for_editor("@foo+t < @a+x @b+y");
    assert_eq!(parsed.diagnostics.len(), 1);
    let diagnostic = &parsed.diagnostics[0];
    assert_eq!(diagnostic.code, MERGE_IN_NEEDS_NEW_TASK_CODE);
    let rewrite = diagnostic.rewrite.as_ref().expect("Use > rewrite");
    assert_eq!(rewrite.label, "Use >");
    assert!(
        rewrite.replacement.contains(">"),
        "replacement was {:?}",
        rewrite.replacement
    );
}

#[test]
fn utf8_and_bulk_offsets_are_absolute() {
    let input = "Café ☕ task < @a+x\n\nSecond < @b+y\n";
    let parsed = super::super::parse_capture_draft_with_options_coded(
        input,
        None,
        None,
        &super::super::CaptureParseOptions::routing_off(),
    )
    .expect("utf8 parses");
    assert_eq!(parsed.items.len(), 2);
    for item in &parsed.items {
        let clause = item.parsed.merge_in.as_ref().expect("clause attached");
        for source in &clause.sources {
            assert!(input.is_char_boundary(source.start));
            assert!(input.is_char_boundary(source.end));
            assert_eq!(&input[source.start..source.end], source.marker);
        }
    }
    let editor = super::super::parse_for_editor(input);
    assert_eq!(editor.items.len(), 2);
    assert!(editor.items.iter().all(|item| item.merge_in.is_some()));
}
