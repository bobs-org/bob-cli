//! Shared target-run selection tests: lexical selection, terminal
//! extraction across runs, refusal metadata, and byte-preserving member
//! views. Opt-in tests set `enable_target_runs`; default-path tests prove
//! public behavior is unchanged.

use super::super::draft::*;
use super::super::model::*;
use super::super::target_runs::*;

fn opt_in() -> CaptureParseOptions<'static> {
    CaptureParseOptions {
        enable_target_runs: true,
        ..CaptureParseOptions::routing_off()
    }
}

fn default_options() -> CaptureParseOptions<'static> {
    CaptureParseOptions::routing_off()
}

fn execute_opt_in(raw: &str) -> Result<ParsedCaptureText, String> {
    parse_capture_text_with_options(raw, None, None, &opt_in())
}

fn draft_opt_in(
    raw: &str,
) -> Result<ParsedCaptureDraft, super::super::draft::TemplateCodedError> {
    parse_capture_draft_with_options_coded(raw, None, None, &opt_in())
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
        // Opt-in still parses as a singleton (no new refusal).
        let parsed = execute_opt_in(raw).expect("singleton parses");
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
        &opt_in(),
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
    let error = execute_opt_in("@a+ @b+y note").unwrap_err();
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
        let parsed = execute_opt_in(raw).expect("parses");
        assert_eq!(parsed.scheduled_offset, Some(3), "{raw}");
        assert_eq!(parsed.body, "note", "{raw}");
    }
    let priorities = [
        "note $2 @a+x @b+y",
        "note @a+x @b+y $2",
        "note @a+x $2 @b+y",
    ];
    for raw in priorities {
        let parsed = execute_opt_in(raw).expect("parses");
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
        let error = draft_opt_in(raw).unwrap_err();
        assert_eq!(error.code, Some("duplicate_capture_target"), "{raw}");
        assert!(
            error.message.contains("twice on this line"),
            "{raw}: {}",
            error.message
        );
    }
    // Separate draft items may name the same task.
    let both = draft_opt_in("@work+a note\n\n@work+a note")
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
    let error = draft_opt_in(raw).unwrap_err();
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
fn default_path_stays_single_target() {
    // Public behavior is unchanged: the second target stays body text.
    let leading = parse_capture_text_with_options(
        "@work+a @dev+b note",
        None,
        None,
        &default_options(),
    )
    .expect("leading parses");
    assert_eq!(leading.route.as_deref(), Some("work"));
    assert!(leading.body.contains("@dev+b"), "{}", leading.body);
    let trailing = parse_capture_text_with_options(
        "note @work+a @dev+b",
        None,
        None,
        &default_options(),
    )
    .expect("trailing parses");
    assert_eq!(trailing.route.as_deref(), Some("dev"));
    assert!(trailing.body.contains("@work+a"), "{}", trailing.body);
    // No new codes on the default path, even for duplicates/splits.
    for raw in ["@work+a @work+a note", "@a+x note @b+y"] {
        let draft = parse_capture_draft_with_options_coded(
            raw,
            None,
            None,
            &default_options(),
        );
        if let Err(error) = draft {
            assert_ne!(error.code, Some("duplicate_capture_target"), "{raw}");
            assert_ne!(error.code, Some("split_capture_targets"), "{raw}");
        }
    }
}

#[test]
fn list_bullets_keep_list_marker_with_opt_in() {
    let error = draft_opt_in("@dev\n- A @cash+a").unwrap_err();
    assert_eq!(error.code, Some("list_bullet_marker"));
}
