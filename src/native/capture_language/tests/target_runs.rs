//! Shared target-run selection tests: lexical selection, terminal
//! extraction across runs, refusal metadata, and byte-preserving member
//! views and public execution/editor behavior.

use super::super::draft::*;
use super::super::editor_model::*;
use super::super::editor_parse::*;
use super::super::model::*;
use super::super::target_runs::*;

fn active() -> CaptureParseOptions<'static> {
    CaptureParseOptions::routing_off()
}

fn default_options() -> CaptureParseOptions<'static> {
    CaptureParseOptions::routing_off()
}

fn editor_active() -> EditorParseOptions<'static> {
    EditorParseOptions {
        url_routing: None,
        has_global_destination: false,
        record_policy: None,
        notebook_policy: None,
        global_route: None,
    }
}

fn parse_editor_active(raw: &str) -> EditorParse {
    parse_for_editor_with(raw, &editor_active())
}

fn parse_editor_default(raw: &str) -> EditorParse {
    parse_for_editor_with(raw, &EditorParseOptions::routing_off())
}

fn execute_active(raw: &str) -> Result<ParsedCaptureText, String> {
    parse_capture_text_with_options(raw, None, None, &active())
}

fn draft_active(
    raw: &str,
) -> Result<ParsedCaptureDraft, super::super::draft::TemplateCodedError> {
    parse_capture_draft_with_options_coded(raw, None, None, &active())
}

fn parse_singleton(raw: &str) -> ParsedCaptureText {
    parse_capture_draft_with_options_coded(raw, None, None, &active())
        .expect("singleton parses")
        .items
        .into_iter()
        .next()
        .expect("one item")
        .parsed
}

fn ignore_absolute_spans(parsed: &mut ParsedCaptureText) {
    if let Some(marker) = parsed.type_marker.as_mut() {
        marker.span = 0..0;
        marker.name_span = 0..0;
    }
    for dependency in &mut parsed.dependencies {
        dependency.span = None;
    }
    for dependent in &mut parsed.blocked_tasks {
        dependent.span = None;
    }
    for relation in &mut parsed.relations {
        relation.span = None;
    }
}

fn tokens_of(text: &str) -> Vec<Token<'_>> {
    tokenize_with_spans(text)
}

#[test]
fn leading_run_selects_two_members() {
    let tokens = tokens_of("@a+x @b+y note");
    let selection =
        select_target_run(&tokens, true, true).expect("leading run");
    assert_eq!(selection.position, TargetRunPosition::Leading);
    assert_eq!(selection.members.len(), 2);
    assert_eq!(selection.members[0].marker_text, "@a+x");
    assert_eq!(selection.members[1].marker_text, "@b+y");
}

#[test]
fn trailing_run_selects_two_members() {
    let tokens = tokens_of("note @a+x @b+y");
    let selection =
        select_target_run(&tokens, true, true).expect("trailing run");
    assert_eq!(selection.position, TargetRunPosition::Trailing);
    assert_eq!(selection.members.len(), 2);
}

#[test]
fn whole_line_selects_all_members() {
    let tokens = tokens_of("@a+x @b+y @c+z");
    let selection = select_target_run(&tokens, true, true).expect("whole line");
    assert_eq!(selection.position, TargetRunPosition::WholeLine);
    assert_eq!(selection.members.len(), 3);
}

#[test]
fn three_members_with_mixed_suffixes() {
    let tokens = tokens_of("@a+x @b+y! @c+z#sec note");
    let selection =
        select_target_run(&tokens, true, true).expect("leading run");
    assert_eq!(selection.members.len(), 3);
    assert_eq!(selection.members[1].marker_text, "@b+y!");
    assert_eq!(selection.members[2].marker_text, "@c+z#sec");
}

#[test]
fn between_prose_words_stays_literal() {
    let tokens = tokens_of("hello @a+x @b+y world");
    assert!(select_target_run(&tokens, true, true).is_none());
    assert!(matches!(
        detect_target_refusals(&tokens, true, true),
        TargetRefusal::None
    ));
}

#[test]
fn non_target_tokens_end_the_run() {
    for raw in ["@a+x @b:y note", "@a+x @bob hi"] {
        let tokens = tokens_of(raw);
        // Single leading member only; the plain/pomodoro token ends the run.
        let selection = select_target_run(&tokens, true, true).expect("single");
        assert_eq!(selection.members.len(), 1, "{raw}");
        assert_eq!(selection.members[0].marker_text, "@a+x", "{raw}");
        assert!(matches!(
            detect_target_refusals(&tokens, true, true),
            TargetRefusal::None
        ));
        // The maximal run stops at this token; the item remains single-target.
        let parsed = execute_active(raw).expect("singleton parses");
        assert_eq!(parsed.route.as_deref(), Some("a"), "{raw}");
    }
}

#[test]
fn forced_destinations_keep_targets_literal() {
    let tokens = tokens_of("@a+x @b+y note");
    assert!(select_target_run(&tokens, true, false).is_none());
    assert!(matches!(
        detect_target_refusals(&tokens, true, false),
        TargetRefusal::None
    ));
    let forced = parse_capture_text_with_options(
        "@a+x @b+y note",
        Some("work"),
        None,
        &active(),
    )
    .expect("forced parses");
    // Forced route keeps `@tokens` literal: the body retains them.
    assert!(forced.body.contains("@a+x"), "{}", forced.body);
    assert!(forced.body.contains("@b+y"), "{}", forced.body);
}

#[test]
fn partial_and_malformed_members_are_selected() {
    let tokens = tokens_of("@a+ @b+y note");
    let selection =
        select_target_run(&tokens, true, true).expect("partial selected");
    assert_eq!(selection.members.len(), 2);
    assert_eq!(selection.members[0].marker_text, "@a+");
    // Malformed members receive the existing strict error, never prose.
    let error = execute_active("@a+ @b+y note").unwrap_err();
    assert!(
        error.contains("sub-bullet capture"),
        "unexpected error: {error}"
    );
}

#[test]
fn unicode_offsets_are_preserved() {
    let raw = "café note @a+x @b+y";
    let tokens = tokens_of(raw);
    let selection =
        select_target_run(&tokens, true, true).expect("trailing run");
    assert_eq!(selection.members.len(), 2);
    // `é` is two bytes: member starts must account for it.
    let first_at = raw.find("@a+x").expect("first");
    let second_at = raw.find("@b+y").expect("second");
    assert_eq!(selection.members[0].start, first_at);
    assert_eq!(selection.members[1].start, second_at);
    assert_eq!(
        &raw[selection.members[0].start..selection.members[0].end],
        "@a+x"
    );
    // Masking preserves byte length and other offsets.
    let masked = masked_source_for_member(raw, &selection.members, 0);
    assert_eq!(masked.len(), raw.len());
    assert_eq!(&masked[first_at..first_at + 4], "@a+x");
    assert_eq!(&masked[second_at..second_at + 4], "    ");
    assert_eq!(&masked[second_at + 4..], &raw[second_at + 4..]);
}

#[test]
fn terminal_extraction_crosses_the_run() {
    let variants = [
        "note s:3 @a+x @b+y",
        "note @a+x @b+y s:3",
        "note @a+x s:3 @b+y",
    ];
    for raw in variants {
        let parsed = execute_active(raw).expect("parses");
        assert_eq!(parsed.scheduled_offset, Some(3), "{raw}");
        assert_eq!(parsed.body, "note", "{raw}");
    }
    let priorities = [
        "note $2 @a+x @b+y",
        "note @a+x @b+y $2",
        "note @a+x $2 @b+y",
    ];
    for raw in priorities {
        let parsed = execute_active(raw).expect("parses");
        assert_eq!(parsed.priority_level, Some(2), "{raw}");
        assert_eq!(parsed.body, "note", "{raw}");
    }
}

#[test]
fn duplicate_identity_ignores_case_drawer_and_toggle() {
    for raw in [
        "@work+a @work+a note",
        "@Work+a @work+a note",
        "@WORK+a @work+a note",
        "@work+a @work+a! note",
        "@work+a @work+a#sec note",
    ] {
        let error = draft_active(raw).unwrap_err();
        assert_eq!(error.code, Some("duplicate_capture_target"), "{raw}");
        assert!(
            error.message.contains("twice on this line"),
            "{raw}: {}",
            error.message
        );
    }
    // Separate draft items may name the same task.
    let both = draft_active("@work+a note\n\n@work+a note")
        .expect("separate items share tasks");
    assert_eq!(both.items.len(), 2);
}

#[test]
fn duplicate_message_uses_repeated_token_and_block_id() {
    let tokens = tokens_of("@work+a @dev+b @work+a note");
    let selection =
        select_target_run(&tokens, true, true).expect("leading run");
    let duplicate =
        find_duplicate_target(&selection.members).expect("duplicate");
    assert_eq!(duplicate.repeated_text, "@work+a");
    assert_eq!(duplicate.block_id, "a");
    let message =
        duplicate_target_message(&duplicate.repeated_text, &duplicate.block_id);
    assert_eq!(
        message,
        "`@work+a` names ^a twice on this line; name each task once"
    );
}

#[test]
fn split_edges_refuse_with_trailing_range() {
    let raw = "@a+x note @b+y";
    let tokens = tokens_of(raw);
    assert!(select_target_run(&tokens, true, true).is_none());
    let refusal = detect_target_refusals(&tokens, true, true);
    let TargetRefusal::Split(split) = refusal else {
        panic!("expected split, got {refusal:?}");
    };
    assert_eq!(split.leading_text, "@a+x");
    assert_eq!(split.trailing_text, "@b+y");
    let message =
        split_targets_message(&split.leading_text, &split.trailing_text);
    assert_eq!(
        message,
        "`@a+x` and `@b+y` are at opposite ends of the line; put every `@route+id` target in one run at the start or end"
    );
    let error = draft_active(raw).unwrap_err();
    assert_eq!(error.code, Some("split_capture_targets"));
    assert!(error.message.contains(&message), "{raw}: {}", error.message);
    // Editor shares the same evidence.
    let diagnostic = editor_refusal_diagnostic(&TargetRefusal::Split(split))
        .expect("diagnostic");
    assert_eq!(diagnostic.code, "split_capture_targets");
    assert_eq!(diagnostic.message, message);
    assert!(diagnostic.range.is_some());
}

#[test]
fn all_target_line_is_not_split() {
    let raw = "@a+x @b+y";
    let tokens = tokens_of(raw);
    let selection = select_target_run(&tokens, true, true).expect("whole line");
    assert_eq!(selection.position, TargetRunPosition::WholeLine);
    assert!(matches!(
        detect_target_refusals(&tokens, true, true),
        TargetRefusal::None
    ));
}

#[test]
fn trailing_plain_route_stays_literal() {
    let raw = "@a+x note @work";
    let tokens = tokens_of(raw);
    // Trailing `@work` is a plain route, not a member: leading single only.
    let selection =
        select_target_run(&tokens, true, true).expect("leading single");
    assert_eq!(selection.members.len(), 1);
    assert!(matches!(
        detect_target_refusals(&tokens, true, true),
        TargetRefusal::None
    ));
}

#[test]
fn child_trailing_run_needs_body_before_it() {
    let with_body = tokens_of("child note @a+x @b+y");
    let selection =
        select_target_run(&with_body, false, true).expect("child trailing");
    assert_eq!(selection.position, TargetRunPosition::Trailing);
    assert_eq!(selection.members.len(), 2);
    // Whole child line with no body falls back to the singleton path.
    let whole = tokens_of("@a+x @b+y");
    let child_whole = select_target_run(&whole, false, true).expect("whole");
    assert_eq!(child_whole.position, TargetRunPosition::WholeLine);
}

#[test]
fn member_views_match_singleton_equivalence() {
    let raw = "@work+a @dev+b note";
    let tokens = tokens_of(raw);
    let selection =
        select_target_run(&tokens, true, true).expect("leading run");
    assert_eq!(selection.members.len(), 2);
    for (index, member) in selection.members.iter().enumerate() {
        let masked = masked_source_for_member(raw, &selection.members, index);
        assert_eq!(masked.len(), raw.len());
        // The kept member stays; the other becomes spaces.
        assert!(masked.contains(&member.marker_text));
        for (other_index, other) in selection.members.iter().enumerate() {
            if other_index != index {
                assert!(
                    !masked.contains(&other.marker_text)
                        || other.marker_text == member.marker_text,
                    "{masked}"
                );
            }
        }
        // Each masked view parses like the singleton with only that target.
        let masked_parsed = parse_capture_text_with_options(
            &masked,
            None,
            None,
            &default_options(),
        )
        .expect("masked parses as singleton");
        let singleton_raw = format!("{} note", member.marker_text);
        let singleton = parse_capture_text_with_options(
            &singleton_raw,
            None,
            None,
            &default_options(),
        )
        .expect("singleton parses");
        assert_eq!(masked_parsed.body, singleton.body, "{masked}");
        assert_eq!(masked_parsed.body, "note", "{masked}");
        assert_eq!(masked_parsed.route, singleton.route, "{masked}");
    }
}

#[test]
fn public_execution_fans_out_runs_and_reports_refusal_codes() {
    for raw in ["@work+a @dev+b note", "note @work+a @dev+b"] {
        let draft = draft_active(raw).expect("public run parses");
        assert_eq!(draft.items.len(), 2, "{raw}");
        assert_eq!(draft.items[0].parsed.body, "note", "{raw}");
        assert_eq!(draft.items[1].parsed.body, "note", "{raw}");
        assert_eq!(draft.items[0].fanout.as_ref().unwrap().position, 1);
        assert_eq!(draft.items[1].fanout.as_ref().unwrap().position, 2);
    }
    for (raw, code) in [
        ("@work+a @work+a note", "duplicate_capture_target"),
        ("@a+x note @b+y", "split_capture_targets"),
    ] {
        let error = draft_active(raw).expect_err("target refusal");
        assert_eq!(error.code, Some(code), "{raw}: {}", error.message);
    }
}

#[test]
fn list_bullets_keep_list_marker() {
    let error = draft_active("@dev\n- A @cash+a").unwrap_err();
    assert_eq!(error.code, Some("list_bullet_marker"));
}

#[test]
fn ordinary_draft_fans_out_in_typed_order_with_shared_source_identity() {
    let parsed = draft_active("first\n\n@work+a @dev+b note\n\nlast")
        .expect("run parses");
    assert_eq!(parsed.items.len(), 4);
    assert_eq!(
        parsed
            .items
            .iter()
            .map(|item| item.parsed.body.as_str())
            .collect::<Vec<_>>(),
        ["first", "note", "note", "last"]
    );
    let first = &parsed.items[1];
    let second = &parsed.items[2];
    assert_eq!(first.parsed.route.as_deref(), Some("work"));
    assert_eq!(second.parsed.route.as_deref(), Some("dev"));
    assert_eq!(first.index, second.index);
    assert_eq!((first.start, first.end), (second.start, second.end));
    assert_eq!(
        (first.line_start, first.line_end),
        (second.line_start, second.line_end)
    );
    assert_eq!(first.fanout.as_ref().map(|meta| meta.line), Some(3));
    assert_eq!(
        first.fanout,
        Some(FanoutContext {
            line: 3,
            position: 1,
            count: 2,
            marker: "@work+a".to_string(),
        })
    );
    assert_eq!(
        second.fanout,
        Some(FanoutContext {
            line: 3,
            position: 2,
            count: 2,
            marker: "@dev+b".to_string(),
        })
    );
    assert_eq!(parsed.items[0].fanout, None);
    assert_eq!(parsed.items[3].fanout, None);
}

#[test]
fn each_member_reuses_singleton_semantics_for_toggles_drawers_and_schedule() {
    for (run, members) in [
        ("@a+x! @b+y", vec!["@a+x!", "@b+y"]),
        (
            "@a+x#first @b+y#second note",
            vec!["@a+x#first note", "@b+y#second note"],
        ),
        ("note s:3 @a+x @b+y", vec!["note s:3 @a+x", "note s:3 @b+y"]),
        ("note $2 @a+x @b+y", vec!["note $2 @a+x", "note $2 @b+y"]),
        (
            "note @a+x @b+y #bugs",
            vec!["note @a+x #bugs", "note @b+y #bugs"],
        ),
        ("note @a+x @b+y %c", vec!["note @a+x %c", "note @b+y %c"]),
        (
            "@a+x @b+y\n- child note",
            vec!["@a+x\n- child note", "@b+y\n- child note"],
        ),
        (
            "note @a+x @b+y *foo:bar",
            vec!["note @a+x *foo:bar", "note @b+y *foo:bar"],
        ),
        (
            "note @a+x @b+y ~foo:bar",
            vec!["note @a+x ~foo:bar", "note @b+y ~foo:bar"],
        ),
        (
            "note @a+x @b+y ~~foo:bar",
            vec!["note @a+x ~~foo:bar", "note @b+y ~~foo:bar"],
        ),
    ] {
        let actual = draft_active(run).expect("run parses");
        assert_eq!(actual.items.len(), members.len(), "{run}");
        for (actual, singleton) in actual.items.iter().zip(members) {
            let mut actual = actual.parsed.clone();
            let mut singleton = parse_singleton(singleton);
            ignore_absolute_spans(&mut actual);
            ignore_absolute_spans(&mut singleton);
            assert_eq!(actual, singleton, "{run}");
        }
    }
}

#[test]
fn type_marker_spans_remain_at_original_draft_offsets_for_each_member() {
    let raw = "note @a+x @b+y #bugs";
    let parsed = draft_active(raw).expect("typed run parses");
    let start = raw.find("#bugs").expect("type marker");
    for item in &parsed.items {
        let marker = item.parsed.type_marker.as_ref().expect("type marker");
        assert_eq!(marker.name, "bugs");
        assert_eq!(marker.span, start..start + "#bugs".len());
        assert_eq!(marker.name_span, start + 1..start + "#bugs".len());
    }
}

#[test]
fn child_line_runs_and_dependency_ownership_are_member_local() {
    let child_run = "Parent task\n- child note @a+x @b+y";
    let parsed = draft_active(child_run).expect("child run parses");
    assert_eq!(parsed.items.len(), 2);
    assert_eq!(parsed.items[0].parsed.route.as_deref(), Some("a"));
    assert_eq!(parsed.items[1].parsed.route.as_deref(), Some("b"));
    assert_eq!(
        parsed.items[0].parsed,
        parse_singleton("Parent task\n- child note @a+x")
    );
    assert_eq!(
        parsed.items[1].parsed,
        parse_singleton("Parent task\n- child note @b+y")
    );
    assert_eq!(parsed.items[0].fanout.as_ref().unwrap().line, 2);

    let dependency_run = "note @a+x @b+y &foo:bar";
    let parsed = draft_active(dependency_run).expect("dependency run parses");
    assert_eq!(parsed.items.len(), 2);
    let span_start = dependency_run.find("&foo:bar").unwrap();
    for item in &parsed.items {
        assert_eq!(
            item.parsed.dependencies[0].span,
            Some((span_start, span_start + "&foo:bar".len()))
        );
    }
    let mut first = parsed.items[0].parsed.clone();
    let mut first_singleton = parse_singleton("note @a+x &foo:bar");
    ignore_absolute_spans(&mut first);
    ignore_absolute_spans(&mut first_singleton);
    assert_eq!(first, first_singleton);
    let mut second = parsed.items[1].parsed.clone();
    let mut second_singleton = parse_singleton("note @b+y &foo:bar");
    ignore_absolute_spans(&mut second);
    ignore_absolute_spans(&mut second_singleton);
    assert_eq!(second, second_singleton);
}

#[test]
fn expanded_members_share_one_inline_global_declaration() {
    let parsed = draft_active("@a+x @b+y @@global note")
        .expect("one declaration is shared by both members");
    assert_eq!(parsed.items.len(), 2);
    assert_eq!(
        parsed.global.as_ref().map(|global| global.route.as_str()),
        Some("global")
    );
    assert_eq!(parsed.items[0].parsed.body, "note");
    assert_eq!(parsed.items[1].parsed.body, "note");
    assert_eq!(parsed.items[0].parsed.route.as_deref(), Some("a"));
    assert_eq!(parsed.items[1].parsed.route.as_deref(), Some("b"));
}

#[test]
fn template_row_fans_out_without_changing_row_identity() {
    let parsed = draft_active("Apply {0} to @work+a @dev+b %t\n\ntask\n")
        .expect("template fan-out parses");
    assert_eq!(parsed.items.len(), 2);
    assert_eq!(parsed.items[0].parsed.route.as_deref(), Some("work"));
    assert_eq!(
        parsed.items[1].parsed.route.as_deref(),
        Some("dev"),
        "{:#?}",
        parsed.items
    );
    assert_eq!(parsed.items[0].parsed.body, "Apply task to");
    assert_eq!(parsed.items[0].parsed.body, parsed.items[1].parsed.body);
    assert_eq!(parsed.items[0].index, parsed.items[1].index);
    assert_eq!(parsed.items[0].start, parsed.items[1].start);
    assert_eq!(parsed.items[0].end, parsed.items[1].end);
    assert_eq!(parsed.items[0].template_row, parsed.items[1].template_row);
    assert_eq!(parsed.items[0].template_row.as_ref().unwrap().row, 1);
    assert_eq!(parsed.items[0].line_start, parsed.items[1].line_start);
    assert_eq!(parsed.items[0].line_end, parsed.items[1].line_end);
    assert_eq!(parsed.items[0].fanout.as_ref().unwrap().position, 1);
    assert_eq!(parsed.items[1].fanout.as_ref().unwrap().position, 2);
    assert_eq!(parsed.items[0].fanout.as_ref().unwrap().line, 3);
    assert_eq!(parsed.items[1].fanout.as_ref().unwrap().line, 3);
}

#[test]
fn template_fanout_applies_cross_run_schedule_to_each_member() {
    let parsed = draft_active("Apply {0} to @work+a s:3 @dev+b %t\n\ntask\n")
        .expect("scheduled template run parses");
    assert_eq!(parsed.items.len(), 2);
    for (item, route) in parsed.items.iter().zip(["work", "dev"]) {
        assert_eq!(item.parsed.route.as_deref(), Some(route));
        assert_eq!(item.parsed.body, "Apply task to");
        assert_eq!(item.parsed.scheduled_offset, Some(3));
        assert_eq!(item.template_row.as_ref().unwrap().row, 1);
    }
}

#[test]
fn template_clipboard_refusal_keeps_its_coded_error() {
    let error = draft_active("Apply {0} to @a+x @b+y %c %t\n\ntask\n")
        .expect_err("template clipboard remains refused");
    assert_eq!(error.code, Some("template_clipboard"));
}

#[test]
fn fanout_keeps_coded_refusals_for_later_members_and_templates() {
    for raw in [
        "@a+x @a+x note",
        "@a+x note @b+y",
        "Task {0} @a+x @a+x %t\n\nvalue\n",
    ] {
        let error = draft_active(raw).expect_err("refusal stays an error");
        let expected = if raw.contains("note @b") {
            "split_capture_targets"
        } else {
            "duplicate_capture_target"
        };
        assert_eq!(error.code, Some(expected), "{raw}: {}", error.message);
    }
}

#[test]
fn malformed_later_member_keeps_its_existing_strict_error() {
    let error = draft_active("@a+x @b+ note")
        .expect_err("malformed member rejects the full draft");
    assert_eq!(error.code, None);
    assert!(!error.message.is_empty());
}

#[test]
fn toggle_member_keeps_relationship_refusal() {
    let error = draft_active("@a+x! @b+y *foo:bar")
        .expect_err("a toggle cannot own a relationship");
    assert!(
        error.message.contains("relationship-only '@a+x!'"),
        "{}",
        error.message
    );
}

#[test]
fn editor_leading_run_combines_into_one_item_with_targets() {
    let raw = "@work+a @dev+b note";
    let parse = parse_editor_active(raw);
    assert_eq!(parse.items.len(), 1, "{raw}");
    let item = &parse.items[0];
    assert_eq!(item.targets.len(), 2, "{raw}");
    assert_eq!(item.body, "note");
    assert_eq!(item.mode, EditorMode::SubBullet);
    assert_eq!(item.route.as_deref(), Some("work"));
    assert_eq!(item.block_id.as_deref(), Some("a"));
    assert!(item.needs.is_empty(), "{:?}", item.needs);
    let first_at = raw.find("@work+a").unwrap();
    let second_at = raw.find("@dev+b").unwrap();
    assert_eq!(
        (item.targets[0].start, item.targets[0].end),
        (first_at, first_at + "@work+a".len())
    );
    assert_eq!(
        (item.targets[1].start, item.targets[1].end),
        (second_at, second_at + "@dev+b".len())
    );
    assert_eq!(&raw[item.targets[0].start..item.targets[0].end], "@work+a");
    assert_eq!(&raw[item.targets[1].start..item.targets[1].end], "@dev+b");
    assert_eq!(item.targets[0].mode, EditorMode::SubBullet);
    assert_eq!(item.targets[1].mode, EditorMode::SubBullet);
    // Both members' spans are kept at absolute offsets, sorted.
    let mut spans = item
        .spans
        .iter()
        .map(|span| (span.start, span.end, span.kind))
        .collect::<Vec<_>>();
    spans.sort_by_key(|span| (span.0, span.1));
    assert!(spans.contains(&(
        first_at,
        first_at + "@work".len(),
        SpanKind::SubBulletRoute
    )));
    assert!(spans.contains(&(
        second_at,
        second_at + "@dev".len(),
        SpanKind::SubBulletRoute
    )));
    // The run claims its route once: no cross-line duplicate.
    assert!(
        !item
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "duplicate_capture_marker"),
        "{:?}",
        item.diagnostics
    );
    // First-item projection mirrors the combined item.
    assert_eq!(parse.targets, item.targets);
    assert_eq!(parse.body, item.body);
    assert_eq!(parse.route, item.route);
    // The ordinary public editor entry point activates the same run.
    let default = parse_editor_default(raw);
    assert_eq!(default.items[0].targets.len(), 2);
    assert_eq!(default.targets.len(), 2);
    assert_eq!(default.items[0].body, "note");
}

#[test]
fn editor_trailing_and_whole_line_runs_keep_body_and_offsets() {
    let trailing = "note @a+x @b+y";
    let parse = parse_editor_active(trailing);
    let item = &parse.items[0];
    assert_eq!(item.targets.len(), 2);
    assert_eq!(item.body, "note");
    assert_eq!(item.route.as_deref(), Some("a"));

    let whole = "@a+x @b+y";
    let parse = parse_editor_active(whole);
    let item = &parse.items[0];
    assert_eq!(item.targets.len(), 2, "{whole}");
    assert!(item.body.is_empty(), "{}", item.body);
}

#[test]
fn editor_mixed_toggle_and_drawer_spans_are_all_kept() {
    let raw = "@a+x! @b+y";
    let parse = parse_editor_active(raw);
    let item = &parse.items[0];
    assert_eq!(item.targets.len(), 2);
    assert_eq!(item.targets[0].marker, "@a+x!");
    assert!(
        item.spans
            .iter()
            .any(|span| span.kind == SpanKind::TaskToggleExplicitToggle),
        "{:?}",
        item.spans
    );

    let raw = "@a+x#first @b+y#second note";
    let parse = parse_editor_active(raw);
    let item = &parse.items[0];
    assert_eq!(item.targets.len(), 2);
    assert_eq!(item.targets[0].section.as_deref(), Some("first"));
    assert_eq!(item.targets[1].section.as_deref(), Some("second"));
    let first_at = raw.find("@a+x#first").unwrap();
    let second_at = raw.find("@b+y#second").unwrap();
    assert_eq!(&raw[first_at..first_at + "@a+x#first".len()], "@a+x#first");
    assert_eq!(
        &raw[second_at..second_at + "@b+y#second".len()],
        "@b+y#second"
    );
    assert_eq!(item.body, "note");
}

#[test]
fn editor_partial_members_union_needs_without_prose() {
    let raw = "@a+ @b+y note";
    let parse = parse_editor_active(raw);
    let item = &parse.items[0];
    assert_eq!(item.targets.len(), 2);
    assert_eq!(item.body, "note");
    assert!(item.needs.contains(&Need::Task), "{:?}", item.needs);
    assert_eq!(item.targets[0].mode, EditorMode::Incomplete);
    assert_eq!(item.targets[1].mode, EditorMode::SubBullet);
    assert_eq!(parse.needs, item.needs);

    let raw = "@a+x @b+ note";
    let parse = parse_editor_active(raw);
    let item = &parse.items[0];
    assert_eq!(item.targets.len(), 2);
    assert!(item.needs.contains(&Need::Task), "{:?}", item.needs);
    assert_eq!(item.targets[1].mode, EditorMode::Incomplete);
}

#[test]
fn editor_terminal_extraction_matches_execution_across_the_run() {
    for raw in [
        "note s:3 @a+x @b+y",
        "note @a+x @b+y s:3",
        "note @a+x s:3 @b+y",
    ] {
        let parse = parse_editor_active(raw);
        let item = &parse.items[0];
        assert_eq!(item.targets.len(), 2, "{raw}");
        assert_eq!(item.body, "note", "{raw}");
        assert!(
            item.spans
                .iter()
                .any(|span| span.kind == SpanKind::Schedule),
            "{raw}: {:?}",
            item.spans
        );
    }
    for raw in [
        "note $2 @a+x @b+y",
        "note @a+x @b+y $2",
        "note @a+x $2 @b+y",
    ] {
        let parse = parse_editor_active(raw);
        let item = &parse.items[0];
        assert_eq!(item.targets.len(), 2, "{raw}");
        assert!(
            item.spans
                .iter()
                .any(|span| span.kind == SpanKind::Priority),
            "{raw}: {:?}",
            item.spans
        );
    }
}

#[test]
fn editor_duplicate_and_split_diagnostics_share_strict_evidence() {
    let raw = "@work+a @work+a note";
    let parse = parse_editor_active(raw);
    let item = &parse.items[0];
    assert_eq!(item.targets.len(), 2, "{raw}");
    let diagnostic = item
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "duplicate_capture_target")
        .expect("duplicate diagnostic");
    assert_eq!(
        diagnostic.message,
        "`@work+a` names ^a twice on this line; name each task once"
    );
    let repeated_at = raw.rfind("@work+a").unwrap();
    assert_eq!(
        diagnostic.range,
        Some((repeated_at, repeated_at + "@work+a".len()))
    );

    let raw = "@a+x note @b+y";
    let parse = parse_editor_active(raw);
    let item = &parse.items[0];
    assert!(item.targets.is_empty(), "{:?}", item.targets);
    let diagnostic = item
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "split_capture_targets")
        .expect("split diagnostic");
    assert!(diagnostic.message.contains("at opposite ends of the line"));
    let trailing_at = raw.rfind("@b+y").unwrap();
    assert_eq!(
        diagnostic.range,
        Some((trailing_at, trailing_at + "@b+y".len()))
    );

    // The ordinary public editor entry point reports the same coded evidence.
    for raw in ["@work+a @work+a note", "@a+x note @b+y"] {
        let default = parse_editor_default(raw);
        assert!(
            default.items[0].diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "duplicate_capture_target"
                    || diagnostic.code == "split_capture_targets"
            }),
            "{raw}: {:?}",
            default.items[0].diagnostics
        );
    }
}

#[test]
fn editor_second_item_offsets_and_unicode_stay_absolute() {
    let raw = "first\n\nnote @a+x @b+y";
    let parse = parse_editor_active(raw);
    assert_eq!(parse.items.len(), 2);
    let item = &parse.items[1];
    assert_eq!(item.targets.len(), 2);
    for target in &item.targets {
        assert_eq!(&raw[target.start..target.end], target.marker);
    }
    assert_eq!(parse.targets, parse.items[0].targets);

    let raw = "café note @a+x @b+y";
    let parse = parse_editor_active(raw);
    let item = &parse.items[0];
    assert_eq!(item.targets.len(), 2);
    let first_at = raw.find("@a+x").unwrap();
    let second_at = raw.find("@b+y").unwrap();
    assert_eq!(
        (item.targets[0].start, item.targets[0].end),
        (first_at, first_at + 4)
    );
    assert_eq!(
        (item.targets[1].start, item.targets[1].end),
        (second_at, second_at + 4)
    );
    for span in &item.spans {
        assert!(raw.is_char_boundary(span.start));
        assert!(raw.is_char_boundary(span.end));
    }
}

#[test]
fn editor_separate_route_lines_still_refuse_duplicates() {
    // A parent route plus a child trailing route is not a target run: the
    // existing cross-line duplicate stands, and no targets are emitted.
    let raw = "@a+x note\n- child @b+y";
    let parse = parse_editor_active(raw);
    let item = &parse.items[0];
    assert!(item.targets.is_empty(), "{:?}", item.targets);
    assert!(
        item.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "duplicate_capture_marker"),
        "{:?}",
        item.diagnostics
    );
}

#[test]
fn refusal_detection_checks_only_the_selected_run() {
    let literal = "hello @a+x middle @a+x goodbye";
    let parsed = draft_active(literal).expect("middle target prose is literal");
    assert_eq!(parsed.items.len(), 1);
    assert_eq!(parsed.items[0].parsed.body, literal);
    assert!(parsed.items[0].parsed.route.is_none());
    assert!(parse_editor_active(literal).items[0].targets.is_empty());

    let leading = "@a+x @b+y note @a+x later";
    let parsed =
        draft_active(leading).expect("later target-shaped prose is literal");
    assert_eq!(parsed.items.len(), 2);
    assert_eq!(parsed.items[0].parsed.body, "note @a+x later");
    assert_eq!(parsed.items[0].parsed.route.as_deref(), Some("a"));
    let editor = parse_editor_active(leading);
    assert_eq!(editor.items[0].targets.len(), 2);
    assert_eq!(editor.items[0].body, "note @a+x later");

    let trailing = "hello @a+x middle @a+x @b+y";
    let parsed =
        draft_active(trailing).expect("duplicate prose before trailing run");
    assert_eq!(parsed.items.len(), 2);
    assert_eq!(parsed.items[0].parsed.body, "hello @a+x middle");
    assert_eq!(parsed.items[0].parsed.route.as_deref(), Some("a"));
    let editor = parse_editor_active(trailing);
    assert_eq!(editor.items[0].targets.len(), 2);
    assert_eq!(editor.items[0].body, "hello @a+x middle");
}

#[test]
fn split_refusal_precedes_duplicate_identity_across_edges() {
    let raw = "@a+x note @a+x";
    let error = draft_active(raw).expect_err("opposite edges are split");
    assert_eq!(error.code, Some("split_capture_targets"));
    let parsed = parse_editor_active(raw);
    let diagnostic = parsed.items[0]
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "split_capture_targets")
        .expect("split diagnostic");
    let start = raw.rfind("@a+x").unwrap();
    assert_eq!(diagnostic.range, Some((start, start + "@a+x".len())));

    let duplicate = "note café @a+x @a+x";
    let error =
        draft_active(duplicate).expect_err("selected trailing duplicate");
    assert_eq!(error.code, Some("duplicate_capture_target"));
    let parsed = parse_editor_active(duplicate);
    let diagnostic = parsed.items[0]
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "duplicate_capture_target")
        .expect("duplicate diagnostic");
    let start = duplicate.rfind("@a+x").unwrap();
    assert_eq!(diagnostic.range, Some((start, start + "@a+x".len())));
}

#[test]
fn leading_run_terminals_reach_each_member_with_original_editor_spans() {
    for (raw, schedule, priority) in [
        ("@a+x s:3 @b+y note", Some(3), None),
        ("s:3 @a+x @b+y note", Some(3), None),
        ("@a+x $2 @b+y note", None, Some(2)),
        ("$2 @a+x @b+y note", None, Some(2)),
    ] {
        let parsed =
            draft_active(raw).expect("run with terminal marker parses");
        assert_eq!(parsed.items.len(), 2, "{raw}");
        for (item, (route, expected_block_id)) in
            parsed.items.iter().zip([("a", "x"), ("b", "y")])
        {
            assert_eq!(item.parsed.body, "note", "{raw}");
            assert_eq!(item.parsed.route.as_deref(), Some(route), "{raw}");
            let CaptureKind::SubBullet {
                target: SubBulletTarget::BlockId(actual_block_id),
                ..
            } = &item.parsed.kind
            else {
                panic!(
                    "{raw}: expected sub-bullet, got {:?}",
                    item.parsed.kind
                );
            };
            assert_eq!(actual_block_id, expected_block_id, "{raw}");
            assert_eq!(item.parsed.scheduled_offset, schedule, "{raw}");
            assert_eq!(item.parsed.priority_level, priority, "{raw}");
            let singleton = if schedule.is_some() {
                format!("@{route}+{expected_block_id} note s:3")
            } else {
                format!("@{route}+{expected_block_id} note $2")
            };
            assert_eq!(item.parsed, parse_singleton(&singleton), "{raw}");
        }

        let editor = parse_editor_active(raw);
        let item = &editor.items[0];
        assert_eq!(item.targets.len(), 2, "{raw}");
        assert_eq!(item.body, "note", "{raw}");
        let (marker, kind) = if schedule.is_some() {
            ("s:3", SpanKind::Schedule)
        } else {
            ("$2", SpanKind::Priority)
        };
        let start = raw.find(marker).unwrap();
        assert!(
            item.spans.iter().any(|span| {
                span.kind == kind
                    && span.start == start
                    && span.end == start + marker.len()
            }),
            "{raw}: {:?}",
            item.spans
        );
    }
}

#[test]
fn retired_priority_probe_preserves_singleton_and_selected_run_boundaries() {
    let singleton = execute_active("@a+x p:2 note").expect("literal singleton");
    assert_eq!(singleton.body, "p:2 note");
    let editor = parse_editor_active("@a+x p:2 note");
    assert_eq!(editor.items[0].body, "p:2 note");
    assert!(!editor.items[0]
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "retired_priority_marker"));

    for raw in ["@a+x p:2", "@a+x @b+y p:2 note", "p:2 @a+x @b+y note"] {
        let error = draft_active(raw).expect_err("retired terminal is refused");
        assert!(
            error.message.contains("is retired"),
            "{raw}: {}",
            error.message
        );
    }
    let terminal = "@a+x @b+y p:2 note";
    let editor = parse_editor_active(terminal);
    assert_eq!(editor.items[0].targets.len(), 2);
    let diagnostic = editor.items[0]
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "retired_priority_marker")
        .expect("run retired-priority diagnostic");
    let start = terminal.find("p:2").unwrap();
    assert_eq!(diagnostic.range, Some((start, start + 3)));
}
