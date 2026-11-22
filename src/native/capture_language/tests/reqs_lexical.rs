//! Lexical capture helpers on requirement-chain lines: the shared
//! admission helper, and the completion, parent-selector, and typing-alias
//! readers that treat an admitted deeper line exactly like a level-2 line.

use super::super::req_bullets::admitted_authored_lines;
use super::super::tokens::parent_task_selector_tokens;
use super::*;

/// The effective level of every line of the draft's first item, `None`
/// where the item's parsers would not admit the line.
fn admitted_levels(raw: &str) -> Vec<Option<u8>> {
    let draft = split_capture_draft(raw);
    admitted_authored_lines(&draft.items[0])
        .iter()
        .map(|line| line.map(|line| line.level))
        .collect()
}

/// The completion field at the end of `raw`, with its replacement made
/// relative to the trailing `token`.
fn trailing_field(
    raw: &str,
    token: &str,
) -> Option<(CompletionContext, String, (usize, usize))> {
    let base = raw.len() - token.len();
    field(raw, raw.len()).map(|field| {
        (
            field.context,
            field.query,
            (field.replacement.0 - base, field.replacement.1 - base),
        )
    })
}

const LEVEL_2: &str = "@work T\n- [ ] r1\n  - [ ] r2 ";
const LEVEL_3: &str = "@work T\n- [ ] r1\n  - [ ] r2\n    - [ ] r3 ";
const LEVEL_4: &str =
    "@work T\n- [ ] r1\n  - [ ] r2\n    - [ ] r3\n      - [ ] r4 ";

#[test]
fn admitted_authored_lines_follow_the_requirement_gate() {
    for (raw, expected) in [
        ("Task\n- a\n  - b", vec![None, Some(1), Some(2)]),
        (
            "Task\n- [ ] a\n  - [ ] b\n    - [ ] c\n      - [ ] d",
            vec![None, Some(1), Some(2), Some(3), Some(4)],
        ),
        // A requirement's note, and that note's one nested note.
        (
            "Task\n- [ ] a\n  - [ ] b\n    - note",
            vec![None, Some(1), Some(2), Some(3)],
        ),
        (
            "Task\n- [ ] a\n  - note\n    - deep",
            vec![None, Some(1), Some(2), Some(3)],
        ),
        // A note under a note, and a level jump, stay refused.
        (
            "Task\n- a\n  - b\n    - c",
            vec![None, Some(1), Some(2), None],
        ),
        ("Task\n- [ ] a\n      - jump", vec![None, Some(1), None]),
        // Placeholders and malformed indentation are never admitted.
        ("Task\n- \n   - odd\n\t- tab", vec![None, None, None, None]),
        // A refused line does not poison a later requirement chain.
        (
            "Task\n- a\n  - b\n    - c\n- [ ] r\n  - [ ] s\n    - [ ] t",
            vec![None, Some(1), Some(2), None, Some(1), Some(2), Some(3)],
        ),
        // Task-list bullets count levels after the shift.
        (
            "@work\n- Task A\n  - [ ] req\n    - [ ] deeper\n      - note",
            vec![None, Some(1), Some(2), Some(3)],
        ),
        (
            "@work\n- Task A\n  - note\n    - deep",
            vec![None, Some(1), None],
        ),
    ] {
        assert_eq!(admitted_levels(raw), expected, "{raw:?}");
    }
}

#[test]
fn deep_requirement_lines_complete_like_level_two() {
    for token in ["&wo", "*wo", "+Exi", "@wo", "~wo", "#fi"] {
        let level_2 = format!("{LEVEL_2}{token}");
        let expected = trailing_field(&level_2, token)
            .unwrap_or_else(|| panic!("{level_2:?} completes"));
        for raw in [format!("{LEVEL_3}{token}"), format!("{LEVEL_4}{token}")] {
            assert_eq!(
                trailing_field(&raw, token).as_ref(),
                Some(&expected),
                "{raw:?}"
            );
        }
    }
    // A requirement's note at level 3 completes too.
    let raw = "@work T\n- [ ] r1\n  - [ ] r2\n    - seen in March &wo";
    assert_eq!(
        trailing_field(raw, "&wo").map(|field| field.0),
        Some(CompletionContext::TaskDependency)
    );
}

#[test]
fn refused_deep_lines_stay_inert() {
    for raw in [
        "Task\n- note\n    - deep +abc",
        "Task\n- note\n    - deep &wo",
        "Task\n- note\n  - sub\n    - deep @wo",
        "Task\n- [ ] r1\n      - jump *wo",
        "@work\n- Task A\n  - note\n    - deep ~wo",
    ] {
        assert_eq!(field(raw, raw.len()), None, "{raw:?}");
        let draft = split_capture_draft(raw);
        assert!(
            parent_task_selector_tokens(&draft.items[0]).is_empty(),
            "{raw:?}"
        );
    }
    // The depth-model regression pins: a refused `+abc` stays an
    // `invalid_child_line` with no picker, and a refused `#fix` no type.
    let parsed = editor("Task\n- note\n    - deep +abc");
    assert_eq!(codes(&parsed), vec!["invalid_child_line"]);
    assert!(parsed.needs.is_empty(), "{:?}", parsed.needs);
    assert!(!span_kinds(&parsed).contains(&SpanKind::InteractivePlaceholder));
    let parsed = editor("Task\n- note\n    - #fix deep");
    assert_eq!(codes(&parsed), vec!["invalid_child_line"]);
    assert!(!span_kinds(&parsed).contains(&SpanKind::TaskType));
}

#[test]
fn parent_selectors_read_admitted_deep_lines() {
    let raw = format!("{LEVEL_3}+Exi");
    let draft = split_capture_draft(&raw);
    let selectors = parent_task_selector_tokens(&draft.items[0]);
    assert_eq!(selectors.len(), 1, "{raw:?}");
    assert_eq!(selectors[0].line_index, 3);
    assert_eq!(selectors[0].token.text, "+Exi");

    let parsed = editor(&raw);
    assert_eq!(parsed.needs, vec![Need::TaskParent]);
    assert!(span_kinds(&parsed).contains(&SpanKind::InteractivePlaceholder));
}

#[test]
fn typing_aliases_and_separator_toggles_work_on_deep_lines() {
    for prefix in [LEVEL_2, LEVEL_3, LEVEL_4] {
        let raw = format!("{prefix},");
        let rewrite = rewrite_draft(&raw, Some(raw.len()));
        assert_alias_applied(&raw, &rewrite);
        assert_eq!(rewrite.text, format!("{prefix}+"), "{raw:?}");

        let raw = format!("{prefix}@work .");
        let rewrite = rewrite_draft(&raw, Some(raw.len()));
        assert_alias_applied(&raw, &rewrite);
        assert_eq!(rewrite.text, format!("{prefix}@work &"), "{raw:?}");

        let raw = format!("{prefix}@work+ex^");
        let rewrite = rewrite_draft(&raw, Some(raw.len()));
        assert_eq!(
            rewrite.rule,
            Some(RewriteRule::SwitchBlockIdSeparator),
            "{raw:?}"
        );
        assert_eq!(rewrite.text, format!("{prefix}@work^ex"), "{raw:?}");
    }
    // A refused deeper line keeps every key literal.
    for raw in [
        "Task\n- note\n    - deep ,",
        "Task\n- note\n    - deep @work .",
    ] {
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(rewrite.rule, None, "{raw:?}");
        assert_eq!(rewrite.text, raw);
    }
}

#[test]
fn leading_markers_on_deep_lines_read_like_level_two() {
    for (raw, kind) in [
        ("@work T\n- [ ] r1\n  - #fix deep", SpanKind::TaskType),
        (
            "@work T\n- [ ] r1\n  - [ ] r2\n    - #fix deep",
            SpanKind::TaskType,
        ),
        ("@work T\n- [ ] r1\n  - %c deep", SpanKind::Directive),
        (
            "@work T\n- [ ] r1\n  - [ ] r2\n    - %c deep",
            SpanKind::Directive,
        ),
    ] {
        let parsed = editor(raw);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert!(span_kinds(&parsed).contains(&kind), "{raw:?}");
        assert_eq!(
            parsed.sub_bullets.last().map(|bullet| bullet.body.as_str()),
            Some("deep"),
            "{raw:?}"
        );
    }
}
