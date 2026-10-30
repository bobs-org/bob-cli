//! Editor spans, needs, and diagnostics for capture `t:<type>` markers.
//!
//! These tests pin the editor half of the masking invariant: every existing
//! claim sees the item with marker bytes blanked, and the marker itself
//! surfaces as one `task_type` span plus the shared teaching diagnostics.

use super::super::editor_model::{EditorMode, Need, SpanKind};
use super::super::type_marker::resolve_exact_type_name;
use super::*;

fn task_type_spans(parse: &EditorParse) -> Vec<(usize, usize)> {
    parse
        .spans
        .iter()
        .filter(|span| span.kind == SpanKind::TaskType)
        .map(|span| (span.start, span.end))
        .collect()
}

fn item_task_type_spans(parse: &EditorParse) -> Vec<(usize, usize)> {
    parse
        .items
        .iter()
        .flat_map(|item| item.spans.iter())
        .filter(|span| span.kind == SpanKind::TaskType)
        .map(|span| (span.start, span.end))
        .collect()
}

#[test]
fn trailing_marker_keeps_the_task_body_and_adds_a_whole_token_span() {
    let parse = editor("Fix it t:fix");
    assert_eq!(parse.body, "Fix it");
    assert_eq!(parse.mode, EditorMode::Task);
    assert!(parse.needs.is_empty());
    assert!(parse.diagnostics.is_empty());
    assert_eq!(task_type_spans(&parse), vec![(7, 12)]);
    // The first item carries the same span for per-item consumers.
    assert_eq!(parse.items[0].body, "Fix it");
    assert_eq!(item_task_type_spans(&parse), vec![(7, 12)]);
}

#[test]
fn leading_marker_and_child_line_markers_type_the_same_item() {
    let parse = editor("t:fix Fix it");
    assert_eq!(parse.body, "Fix it");
    assert_eq!(task_type_spans(&parse), vec![(0, 5)]);

    let parse = editor("Fix it @sase t:fix");
    assert_eq!(parse.body, "Fix it");
    assert_eq!(task_type_spans(&parse), vec![(13, 18)]);
    // The neighboring route span keeps its original range.
    assert!(parse.spans.iter().any(|span| {
        span.kind == SpanKind::Route && span.start == 7 && span.end == 12
    }));

    // A marker on a child line types the parent task and contributes no
    // semantic child body.
    let parse = editor("Fix it\n- check t:fix");
    assert_eq!(parse.body, "Fix it");
    assert_eq!(task_type_spans(&parse), vec![(15, 20)]);
    assert!(parse.diagnostics.is_empty());
}

#[test]
fn marker_offsets_survive_unicode_and_escaped_text() {
    let parse = editor("Fix 🧰 it t:fix");
    assert_eq!(parse.body, "Fix 🧰 it");
    assert_eq!(task_type_spans(&parse), vec![(12, 17)]);
    assert_eq!(&"Fix 🧰 it t:fix"[12..17], "t:fix");

    // An escaped marker keeps its visible text in the body with no span.
    let parse = editor("\\t:fix Fix it");
    assert_eq!(parse.body, "t:fix Fix it");
    assert!(task_type_spans(&parse).is_empty());
    assert!(parse.diagnostics.is_empty());

    // Mid-line markers and uppercase sigils stay prose.
    for raw in ["Ask about t:fix today", "Ask about T:FIX today"] {
        let parse = editor(raw);
        assert_eq!(parse.body, raw);
        assert!(task_type_spans(&parse).is_empty());
        assert!(parse.diagnostics.is_empty());
    }
}

#[test]
fn incomplete_marker_needs_a_task_type_without_a_diagnostic() {
    for raw in ["Fix it t:", "Fix it t:\"DEEP"] {
        let parse = editor(raw);
        assert_eq!(parse.body, "Fix it", "{raw:?}");
        assert_eq!(parse.mode, EditorMode::Incomplete, "{raw:?}");
        assert!(parse.needs.contains(&Need::TaskType), "{raw:?}");
        assert_eq!(task_type_spans(&parse).len(), 1, "{raw:?}");
        assert!(parse.diagnostics.is_empty(), "{raw:?}");
    }
}

#[test]
fn invalid_markers_carry_the_shared_teaching_copy() {
    for (raw, needle) in [
        ("Fix it t:fix.", "is not a task type"),
        ("Fix it t:bob+decks", "names two types"),
        ("Fix it t:\"BUILD + FIX\"", "names two types"),
        ("Fix it t:“BUILD + FIX”", "names two types"),
    ] {
        let parse = editor(raw);
        assert_eq!(parse.body, "Fix it", "{raw:?}");
        assert_eq!(task_type_spans(&parse).len(), 1, "{raw:?}");
        assert_eq!(parse.diagnostics.len(), 1, "{raw:?}");
        let diagnostic = &parse.diagnostics[0];
        assert_eq!(diagnostic.code, "invalid_type_marker");
        assert!(
            diagnostic.message.contains(needle),
            "{raw:?}: {}",
            diagnostic.message
        );
        // An invalid name fabricates no resolved type and no new need.
        assert!(!parse.needs.contains(&Need::TaskType), "{raw:?}");
    }
    // Execution reports the same copy for the same inputs.
    assert!(execute("Fix it t:fix.")
        .unwrap_err()
        .contains("is not a task type"));
    assert!(execute("Fix it t:bob+decks")
        .unwrap_err()
        .contains("names two types"));
}

#[test]
fn duplicate_markers_share_the_execution_duplicate_diagnostic() {
    let parse = editor("Fix it t:fix t:bug");
    let codes = codes(&parse);
    assert_eq!(codes, vec!["duplicate_capture_marker"]);
    assert_eq!(
        parse.diagnostics[0].message,
        execute("Fix it t:fix t:bug").unwrap_err()
    );
    // Both marker tokens still highlight.
    assert_eq!(task_type_spans(&parse), vec![(7, 12), (13, 18)]);
}

#[test]
fn a_marker_alone_needs_a_task() {
    let parse = editor("t:fix");
    assert_eq!(parse.body, "");
    assert!(parse.needs.contains(&Need::Task));
    assert_eq!(codes(&parse), vec!["type_marker_needs_task"]);
    assert_eq!(parse.diagnostics[0].message, execute("t:fix").unwrap_err());
}

#[test]
fn non_task_subjects_are_refused_with_the_shared_copy() {
    // (draft, message needle)
    for (raw, needle) in [
        ("@notes#Ideas text t:fix", "writes a plain bullet"),
        ("@sase+fix-x t:fix", "changes an existing task's status"),
        ("@sase:close-1=x t:fix", "closes a Pomodoro"),
        ("x t:fix", "closes a Pomodoro"),
        ("=3 t:fix", "starts a Pomodoro"),
        ("+5 t:fix", "adjusts a Pomodoro duration"),
        (
            "^sase:fix-x t:fix",
            "retyping it with `t:` is not supported yet",
        ),
        (
            "@sase:fix-x t:fix",
            "retyping it with `t:` is not supported yet",
        ),
        ("!sase:fix-x t:fix", "completes an existing task"),
    ] {
        let parse = editor(raw);
        assert_eq!(
            codes(&parse),
            vec!["type_marker_not_applicable"],
            "{raw:?}"
        );
        assert!(
            parse.diagnostics[0].message.contains(needle),
            "{raw:?}: {}",
            parse.diagnostics[0].message
        );
        assert_eq!(
            parse.diagnostics[0].message,
            execute(raw).unwrap_err(),
            "{raw:?}"
        );
    }
}

#[test]
fn global_existing_task_inheritance_rechecks_the_marker_subject() {
    let raw = "@@sase+parent\n\nA child t:fix";
    let parse = editor(raw);
    assert_eq!(parse.mode, EditorMode::SubBullet);
    assert_eq!(codes(&parse), vec!["type_marker_not_applicable"]);
    assert_eq!(parse.diagnostics[0].message, execute(raw).unwrap_err());
}

#[test]
fn bodiless_forms_need_a_task_like_their_masked_parse() {
    // These masked forms never parse, so execution fails them with missing
    // text and the editor needs a task instead of refusing a capture.
    for raw in ["@notes#Ideas t:fix", "@cash^new+ t:fix", "@sase# t:fix"] {
        let parse = editor(raw);
        assert_eq!(codes(&parse), vec!["type_marker_needs_task"], "{raw:?}");
        assert_eq!(
            parse.diagnostics[0].message,
            execute(raw).unwrap_err(),
            "{raw:?}"
        );
        assert!(parse.needs.contains(&Need::Task), "{raw:?}");
    }
}

#[test]
fn marker_only_continuation_lines_contribute_no_child_body() {
    let parse = editor("Fix it\n- t:fix");
    assert_eq!(parse.body, "Fix it");
    assert_eq!(task_type_spans(&parse), vec![(9, 14)]);
    assert!(parse.diagnostics.is_empty());
    assert!(parse.sub_bullets.is_empty());
}

/// Assert the masking invariant for one baseline/variant pair: the variant
/// parses exactly like the baseline once marker metadata is removed.
/// `insert_at` is the byte offset where `inserted` was spliced in, so
/// ordinary spans translate by its length when they sit at or after it.
fn check_masking_parity(
    baseline: &str,
    variant: &str,
    insert_at: usize,
    inserted: &str,
) {
    let base = editor(baseline);
    assert!(
        base.diagnostics.is_empty(),
        "baseline must be clean: {baseline:?}"
    );
    assert!(
        base.needs.is_empty(),
        "baseline must be clean: {baseline:?}"
    );
    let var = editor(variant);
    assert_eq!(var.body, base.body, "{variant:?}");
    assert_eq!(var.mode, base.mode, "{variant:?}");
    assert_eq!(var.route, base.route, "{variant:?}");
    assert_eq!(var.section, base.section, "{variant:?}");
    assert_eq!(var.block_id, base.block_id, "{variant:?}");
    assert_eq!(var.needs, base.needs, "{variant:?}");
    assert_eq!(var.diagnostics, base.diagnostics, "{variant:?}");
    assert_eq!(var.sub_bullets, base.sub_bullets, "{variant:?}");
    let mut expected: Vec<_> = base
        .spans
        .iter()
        .map(|span| {
            let shift = if span.start >= insert_at {
                inserted.len()
            } else {
                0
            };
            (span.start + shift, span.end + shift, span.kind.label())
        })
        .collect();
    expected.sort();
    let mut actual: Vec<_> = var
        .spans
        .iter()
        .filter(|span| span.kind != SpanKind::TaskType)
        .map(|span| (span.start, span.end, span.kind.label()))
        .collect();
    actual.sort();
    assert_eq!(actual, expected, "{variant:?}");
    // Exactly one task_type span covers the inserted token.
    let marker_spans = task_type_spans(&var);
    assert_eq!(marker_spans.len(), 1, "{variant:?}");
    assert_eq!(
        &variant[marker_spans[0].0..marker_spans[0].1],
        inserted.trim()
    );
}

#[test]
fn masked_items_match_their_baseline_across_grammar_families() {
    // Plain task, leading and trailing.
    check_masking_parity("Fix the parser", "t:fix Fix the parser", 0, "t:fix ");
    check_masking_parity(
        "Fix the parser",
        "Fix the parser t:fix",
        15,
        " t:fix",
    );
    // Routed, Pomodoro-linked, and terminal-marker neighbors.
    check_masking_parity("Fix it @sase", "t:fix Fix it @sase", 0, "t:fix ");
    check_masking_parity("Fix it @sase", "Fix it @sase t:fix", 11, " t:fix");
    check_masking_parity(
        "Fix it @sase:abc#bugs",
        "Fix it @sase:abc#bugs t:fix",
        21,
        " t:fix",
    );
    check_masking_parity(
        "Fix it s:2 @sase",
        "Fix it s:2 @sase t:fix",
        15,
        " t:fix",
    );
    // A marker between body and transparent markers still sits at an edge.
    check_masking_parity(
        "Fix it s:2 @sase",
        "Fix it t:fix s:2 @sase",
        7,
        "t:fix ",
    );
    // Dependency and blocked-task modifiers stay intact beside a marker.
    check_masking_parity(
        "Fix it &notes:block",
        "Fix it &notes:block t:fix",
        19,
        " t:fix",
    );
    check_masking_parity(
        "Fix it *notes:block",
        "Fix it *notes:block t:fix",
        19,
        " t:fix",
    );
    // Unicode bytes move every later offset.
    check_masking_parity("Fix 🧰 it", "t:fix Fix 🧰 it", 0, "t:fix ");
    // Markers on child lines type the parent item.
    check_masking_parity(
        "Fix it\n- check details",
        "Fix it\n- check details t:fix",
        22,
        " t:fix",
    );
    check_masking_parity(
        "Fix it\n- check details",
        "Fix it\n- t:fix check details",
        9,
        "t:fix ",
    );
    // A batch keeps every item's parse; later items shift by the
    // inserted marker length while the marked item gains one span.
    let base = editor("First task\n\nSecond @sase");
    let var = editor("First task t:fix\n\nSecond @sase");
    assert_eq!(var.items.len(), base.items.len());
    assert_eq!(var.items[0].body, "First task");
    assert_eq!(var.items[1].body, base.items[1].body);
    let shifted: Vec<_> = base.items[1]
        .spans
        .iter()
        .map(|span| super::super::editor_model::Span {
            start: span.start + " t:fix".len(),
            end: span.end + " t:fix".len(),
            kind: span.kind,
        })
        .collect();
    assert_eq!(var.items[1].spans, shifted);
}

#[test]
fn execution_and_editor_agree_on_typed_task_bodies() {
    for variant in [
        "t:fix Fix the parser",
        "Fix the parser t:fix",
        "Fix it @sase t:fix",
        "Fix 🧰 it t:\"DEEP WORK\"",
        "Fix it\n- check details t:fix",
    ] {
        let parsed = execute(variant).expect("valid typed task");
        parsed.type_marker.expect("type marker");
        let edited = editor(variant);
        assert_eq!(edited.body, parsed.body, "{variant:?}");
        assert_eq!(edited.items[0].body, parsed.body, "{variant:?}");
    }
    let parsed = execute("Fix the parser t:fix").expect("valid typed task");
    assert_eq!(parsed.type_marker.expect("marker").name, "fix");
    let parsed =
        execute("Fix 🧰 it t:\"DEEP WORK\"").expect("valid typed task");
    assert_eq!(parsed.type_marker.expect("marker").name, "DEEP WORK");
}

#[test]
fn template_rows_keep_markers_out_of_the_body() {
    use super::super::template;
    let raw = "@@template:\n\nTask {0} t:fix\n\nvalue\n";
    let expansion = template::expand(raw).expect("valid template").unwrap();
    assert!(expansion.text.contains("t:fix"));
    // The expanded row parses like its unmarked baseline.
    let parse = editor(&expansion.text);
    assert_eq!(parse.body, "Task value");
    assert_eq!(task_type_spans(&parse).len(), 1);
    assert!(parse.diagnostics.is_empty());
    let base = editor("Task value\n");
    assert_eq!(parse.body, base.body);
    assert_eq!(parse.mode, base.mode);
}

fn configured(
    key: &str,
    name: &str,
) -> crate::native::config::themes::ConfiguredTheme {
    crate::native::config::themes::ConfiguredTheme {
        key: key.to_string(),
        name: name.to_string(),
        color: None,
    }
}

#[test]
fn exact_name_resolution_prefers_config_then_catalog() {
    use std::collections::BTreeMap;
    let vocab = vec![
        configured("fix", "Fix"),
        configured("deep work", "Deep Work"),
    ];
    let mut catalog = BTreeMap::new();
    catalog.insert("fix".to_string(), "FIX".to_string());
    catalog.insert("deep work".to_string(), "DEEP WORK".to_string());
    catalog.insert("build".to_string(), "BUILD".to_string());
    let lookup = |key: &str| catalog.get(key).cloned();

    // Configured spellings win ties with the catalog.
    assert_eq!(
        resolve_exact_type_name(Some(&vocab), &lookup, "fix", false)
            .expect("configured slug")
            .name,
        "Fix"
    );
    // A slug finds a spaced type through the hyphen fallback.
    assert_eq!(
        resolve_exact_type_name(Some(&vocab), &lookup, "deep-work", false)
            .expect("hyphen slug")
            .name,
        "Deep Work"
    );
    // Without a vocabulary the catalog spelling applies.
    assert_eq!(
        resolve_exact_type_name(None, &lookup, "deep-work", false)
            .expect("catalog hyphen")
            .name,
        "DEEP WORK"
    );
    // A literal matches by key equality only, never the hyphen fallback.
    assert_eq!(
        resolve_exact_type_name(None, &lookup, "BUILD", true)
            .expect("literal")
            .name,
        "BUILD"
    );
    assert!(
        resolve_exact_type_name(None, &lookup, "DEEP-WORK", true).is_none(),
        "literals never substitute spaces for hyphens"
    );
    // Prefixes never resolve.
    assert!(
        resolve_exact_type_name(Some(&vocab), &lookup, "fi", false).is_none()
    );
    assert!(resolve_exact_type_name(None, &lookup, "BU", true).is_none());
    // Unknown names resolve nowhere.
    assert!(
        resolve_exact_type_name(Some(&vocab), &lookup, "nope", false).is_none()
    );
}
