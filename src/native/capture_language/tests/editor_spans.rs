//! Editor span and diagnostic tests.

use super::super::editor_model::*;
use super::super::line::*;
use super::super::markers::*;
use super::super::model::*;
use super::*;

#[test]
fn tokenizer_records_half_open_byte_spans() {
    let tokens = tokenize_with_spans("  buy   milk  ");
    assert_eq!(
        tokens
            .iter()
            .map(|token| (token.text, token.start, token.end))
            .collect::<Vec<_>>(),
        vec![("buy", 2, 5), ("milk", 8, 12)]
    );
}

#[test]
fn tokenizer_keeps_multibyte_and_crlf_offsets_on_char_boundaries() {
    // "café" is 5 bytes, the emoji is 4, and the combining acute in
    // "e\u{301}" adds 2 more; every span must still slice cleanly.
    let raw = "caf\u{e9} \u{1f680}\r\ne\u{301}tude\ts:1";
    let tokens = tokenize_with_spans(raw);
    let observed: Vec<(&str, usize, usize)> = tokens
        .iter()
        .map(|token| (token.text, token.start, token.end))
        .collect();
    assert_eq!(
        observed,
        vec![
            ("caf\u{e9}", 0, 5),
            ("\u{1f680}", 6, 10),
            ("e\u{301}tude", 12, 19),
            ("s:1", 20, 23),
        ]
    );
    for token in &tokens {
        assert!(raw.is_char_boundary(token.start));
        assert!(raw.is_char_boundary(token.end));
        assert_eq!(&raw[token.start..token.end], token.text);
    }
    // Spans stay ordered and never overlap.
    for pair in tokens.windows(2) {
        assert!(pair[0].end <= pair[1].start);
    }
}

#[test]
fn editor_spans_use_original_byte_offsets_after_multibyte_text() {
    let raw = "caf\u{e9} run \u{1f680} @Cash+goog-exit";
    let parse = editor(raw);
    assert_eq!(parse.body, "caf\u{e9} run \u{1f680}");
    assert_eq!(parse.mode, EditorMode::SubBullet);
    assert_eq!(
        ranges(&parse),
        vec![
            (15, 20, SpanKind::SubBulletRoute),
            (21, 30, SpanKind::SubBulletBlockId),
        ]
    );
    for span in &parse.spans {
        assert!(raw.is_char_boundary(span.start));
        assert!(raw.is_char_boundary(span.end));
    }
}

#[test]
fn plan_worked_example_matches_documented_offsets() {
    let raw = "Call bank @Cash+";
    let parse = editor(raw);
    assert_eq!(parse.body, "Call bank");
    assert_eq!(parse.mode, EditorMode::Incomplete);
    assert_eq!(parse.route.as_deref(), Some("cash"));
    assert_eq!(parse.needs, vec![Need::Task]);
    assert_eq!(
        ranges(&parse),
        vec![
            (10, 15, SpanKind::SubBulletRoute),
            (15, 16, SpanKind::InteractivePlaceholder),
        ]
    );
    assert_eq!(&raw[10..15], "@Cash");
    assert_eq!(&raw[15..16], "+");
}

#[test]
fn editor_reports_terminal_marker_spans() {
    let raw = "body p:2 s:1 % @groceries";
    let parse = editor(raw);
    assert_eq!(parse.body, "body");
    assert_eq!(parse.mode, EditorMode::Task);
    assert_eq!(parse.route.as_deref(), Some("groceries"));
    assert_eq!(
        ranges(&parse),
        vec![
            (5, 8, SpanKind::Priority),
            (9, 12, SpanKind::Schedule),
            (13, 14, SpanKind::Clipboard),
            (15, 25, SpanKind::Route),
        ]
    );
}

#[test]
fn editor_reports_invalid_components_as_diagnostics() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "Body @bad.route+id",
            "invalid_sub_bullet_route",
            SUB_BULLET_ROUTE_ERROR,
        ),
        (
            "Body @dev+bad.id",
            "invalid_sub_bullet_block_id",
            SUB_BULLET_BLOCK_ID_ERROR,
        ),
        (
            "Body @dev+id#bad_id",
            "invalid_sub_bullet_section",
            SUB_BULLET_SECTION_ERROR,
        ),
        (
            "Body @dev+id#req^x",
            "invalid_sub_bullet_section",
            SUB_BULLET_SECTION_ERROR,
        ),
        (
            "Body @dev+id#req:x",
            "invalid_sub_bullet_section",
            SUB_BULLET_SECTION_ERROR,
        ),
        (
            "Body @dev+id#req+x",
            "invalid_sub_bullet_section",
            SUB_BULLET_SECTION_ERROR,
        ),
        (
            "Body @dev+id#req#x",
            "invalid_sub_bullet_section",
            SUB_BULLET_SECTION_ERROR,
        ),
        (
            "Body @bad.route^id",
            "invalid_task_block_id_route",
            TASK_BLOCK_ID_ROUTE_ERROR,
        ),
        (
            "Body @dev^bad.id",
            "invalid_task_block_id",
            TASK_BLOCK_ID_ERROR,
        ),
        (
            "Body @dev^bad.id+",
            "invalid_task_block_id",
            TASK_BLOCK_ID_ERROR,
        ),
        (
            "Body @dev^id+!",
            "invalid_task_block_id",
            TASK_BLOCK_ID_ERROR,
        ),
        (
            "Body @dev^focus-123#bugs+",
            "invalid_project_note_marker",
            "put the project-note `+` right after the block ID: `@dev^focus-123+#bugs` (a `+` after `#bugs` would be part of the Pomodoro name)",
        ),
        (
            "Body @dev^focus-123#bugs",
            "invalid_project_note_marker",
            "`#bugs` after `@dev^focus-123` needs the project-note `+` (`@dev^focus-123+#bugs`); to link a task under a Pomodoro, use `@dev:focus-123#bugs`",
        ),
        (
            "Body @bad.route:id",
            "invalid_pomodoro_route",
            POMODORO_ROUTE_ERROR,
        ),
        (
            "Body @dev:bad.id",
            "invalid_pomodoro_block_id",
            POMODORO_BLOCK_ID_ERROR,
        ),
        (
            "Body @dev:id#bad_id",
            "invalid_pomodoro_name",
            POMODORO_NAME_ERROR,
        ),
        (
            "Body @dev:id#req^x",
            "invalid_pomodoro_name",
            POMODORO_NAME_ERROR,
        ),
        (
            "Body @dev:bad.id+",
            "retired_project_note_marker",
            "`@dev:bad.id+` is retired: a project note never links its own `^prj` task. Write `@dev^bad.id+` and end each task bullet you want in the Pomodoro with ` :<id>`",
        ),
        (
            "Body @cash:goog-exit+#bugs",
            "retired_project_note_marker",
            "`@cash:goog-exit+#bugs` is retired: a project note never links its own `^prj` task. Write `@cash^goog-exit+#bugs` and end each task bullet you want in the Pomodoro with ` :<id>`",
        ),
        (
            "Body @dev:id+!",
            "invalid_pomodoro_block_id",
            POMODORO_BLOCK_ID_ERROR,
        ),
    ];

    for (raw, code, message) in cases {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Task, "{raw}");
        assert_eq!(parse.body, "Body", "{raw}");
        assert!(parse.needs.is_empty(), "{raw}");
        assert_eq!(parse.diagnostics.len(), 1, "{raw}");
        let diagnostic = &parse.diagnostics[0];
        assert_eq!(diagnostic.severity, Severity::Error, "{raw}");
        assert_eq!(diagnostic.code, *code, "{raw}");
        assert_eq!(diagnostic.message, *message, "{raw}");
        assert_eq!(diagnostic.range, Some((5, raw.len())), "{raw}");
    }
}

#[test]
fn editor_reports_retired_double_colon_as_migration_guidance() {
    for raw in [
        "Body @dev::focus-123",
        "Body @dev::",
        "Body @::focus-123",
        "Body @::",
        "Body @dev::bad.id",
    ] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Task, "{raw}");
        assert_eq!(parse.body, "Body", "{raw}");
        assert_eq!(parse.route, None, "{raw}");
        assert!(parse.needs.is_empty(), "{raw}");
        assert_eq!(
            codes(&parse),
            vec!["retired_task_block_id_marker"],
            "{raw}"
        );
        assert_eq!(
            parse.diagnostics[0].message, RETIRED_DOUBLE_COLON_ERROR,
            "{raw}"
        );
        assert_eq!(parse.diagnostics[0].range, Some((5, raw.len())), "{raw}");
    }
}

#[test]
fn mixed_separators_keep_the_first_family_and_do_not_steal_section_suffixes() {
    let bullet_plus = editor("Jot @notes#time+box");
    assert_eq!(bullet_plus.mode, EditorMode::Bullet);
    assert_eq!(bullet_plus.section.as_deref(), Some("time+box"));
    assert!(bullet_plus.diagnostics.is_empty());

    let bullet_caret = editor("Jot @notes#time^box");
    assert_eq!(bullet_caret.mode, EditorMode::Bullet);
    assert_eq!(bullet_caret.section.as_deref(), Some("time^box"));
    assert!(bullet_caret.diagnostics.is_empty());

    let bullet_colons = editor("Jot @notes#time::box");
    assert_eq!(bullet_colons.mode, EditorMode::Bullet);
    assert_eq!(bullet_colons.section.as_deref(), Some("time::box"));
    assert!(bullet_colons.diagnostics.is_empty());

    let plus_then_hash = editor("Add context @route+bad#section");
    assert_eq!(plus_then_hash.mode, EditorMode::SubBullet);
    assert_eq!(plus_then_hash.block_id.as_deref(), Some("bad"));
    assert_eq!(plus_then_hash.section.as_deref(), Some("section"));
    assert!(plus_then_hash.diagnostics.is_empty());

    let plus_then_colon = editor("Add context @route+bad:id");
    assert_eq!(codes(&plus_then_colon), vec!["invalid_sub_bullet_block_id"]);

    let caret_then_colon = editor("Do work @route^bad:id");
    assert_eq!(codes(&caret_then_colon), vec!["invalid_task_block_id"]);

    let plus_then_caret = editor("Add context @route+id^x");
    assert_eq!(codes(&plus_then_caret), vec!["invalid_sub_bullet_block_id"]);

    let caret_then_plus = editor("Do work @route^id+x");
    assert_eq!(codes(&caret_then_plus), vec!["invalid_task_block_id"]);

    let colon_then_plus = editor("Do work @route:id+x");
    assert_eq!(codes(&colon_then_plus), vec!["invalid_pomodoro_block_id"]);

    let colon_then_caret = editor("Do work @route:id^x");
    assert_eq!(codes(&colon_then_caret), vec!["invalid_pomodoro_block_id"]);
}

#[test]
fn editor_reports_legacy_bullet_markers_without_failing() {
    let parse = editor("Some note #bar");
    assert_eq!(codes(&parse), vec!["legacy_bullet_marker"]);
    assert_eq!(parse.diagnostics[0].range, Some((10, 14)));
    assert_eq!(parse.mode, EditorMode::Task);
    assert_eq!(parse.body, "Some note #bar");

    let parse = editor("Some note #bar @foo");
    assert_eq!(codes(&parse), vec!["legacy_bullet_marker"]);
    assert_eq!(parse.diagnostics[0].range, Some((10, 14)));
    // The trailing route still resolves so the editor can keep painting.
    assert_eq!(parse.route.as_deref(), Some("foo"));
}

#[test]
fn editor_leading_marker_wins_over_trailing_marker() {
    let parse = editor("@work buy milk @home");
    assert_eq!(parse.route.as_deref(), Some("work"));
    assert_eq!(parse.body, "buy milk @home");
    assert_eq!(ranges(&parse), vec![(0, 5, SpanKind::Route)]);

    // An invalid prefix is not route-shaped, so the suffix still wins --
    // exactly like `parse_capture_text_with_clip_control`.
    let parse = editor("@bad! body @Good");
    assert_eq!(parse.route.as_deref(), Some("good"));
    assert_eq!(parse.body, "@bad! body");
}

#[test]
fn editor_keeps_middle_and_time_tokens_literal() {
    for raw in [
        "Email @home soon",
        "call dentist @5:30pm",
        "standup @10:00",
        "Discuss @dev:id later",
        "Discuss @dev+id later",
        "Discuss @dev^id later",
        "Discuss @dev::id later",
    ] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Task, "{raw}");
        assert_eq!(parse.body, raw, "{raw}");
        assert_eq!(parse.route, None, "{raw}");
        assert!(parse.spans.is_empty(), "{raw}");
        assert!(parse.diagnostics.is_empty(), "{raw}");
    }
}

#[test]
fn editor_accepts_marker_only_input_with_an_empty_body() {
    for (raw, mode) in [
        ("@dev:id", EditorMode::PomodoroLink),
        ("@:", EditorMode::Incomplete),
        ("@dev+id", EditorMode::TaskToggle),
        ("@dev+id!", EditorMode::TaskToggle),
        ("@dev+id#req", EditorMode::TaskToggle),
        ("@+", EditorMode::Incomplete),
        ("@dev^id", EditorMode::Task),
        ("@^", EditorMode::Incomplete),
        ("@", EditorMode::Incomplete),
    ] {
        let parse = editor(raw);
        assert_eq!(parse.body, "", "{raw}");
        assert_eq!(parse.mode, mode, "{raw}");
    }
}

#[test]
fn editor_reports_solo_at_pomodoro_links() {
    let parse = editor("@sase:deep-fix");
    assert_eq!(parse.mode, EditorMode::PomodoroLink);
    assert_eq!(parse.body, "");
    assert_eq!(parse.route.as_deref(), Some("sase"));
    assert_eq!(parse.block_id.as_deref(), Some("deep-fix"));
    assert_eq!(parse.section, None);
    assert!(parse.needs.is_empty());
    assert!(parse.pomodoro_start.is_none());
    assert_eq!(
        ranges(&parse),
        vec![
            (0, 5, SpanKind::PomodoroRoute),
            (6, 14, SpanKind::PomodoroBlockId),
        ]
    );
    assert!(parse.diagnostics.is_empty());

    let named = editor("@SASE:deep-fix#bugs=3");
    assert_eq!(named.mode, EditorMode::PomodoroLink);
    assert_eq!(named.route.as_deref(), Some("sase"));
    assert_eq!(named.section.as_deref(), Some("bugs"));
    let start = named.pomodoro_start.clone().expect("start suffix");
    assert_eq!(start.raw, "3");
    assert_eq!(
        ranges(&named),
        vec![
            (0, 5, SpanKind::PomodoroRoute),
            (6, 14, SpanKind::PomodoroBlockId),
            (15, 19, SpanKind::PomodoroName),
            (19, 21, SpanKind::PomodoroStart),
        ]
    );

    // Body-bearing markers keep their new-task meaning.
    let task = editor("Do work @sase:deep-fix");
    assert_eq!(task.mode, EditorMode::PomodoroTask);
    assert_eq!(task.body, "Do work");

    // Anything else on a solo item is an `invalid_pomodoro_link`
    // conflict, not a new task.
    for (raw, message) in [
        (
            "@sase:deep-fix\n- child",
            "Pomodoro link capture cannot be combined with authored child bullets",
        ),
        (
            "@sase:deep-fix s:2",
            "Pomodoro link capture cannot be combined with s:<N>",
        ),
        (
            "@sase:deep-fix p:2",
            "Pomodoro link capture cannot be combined with p:<N>",
        ),
        (
            "@sase:deep-fix %",
            "Pomodoro link capture cannot be combined with % clipboard markers",
        ),
    ] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::PomodoroLink, "{raw}");
        assert_eq!(codes(&parse), vec!["invalid_pomodoro_link"], "{raw}");
        assert_eq!(parse.diagnostics[0].message, message, "{raw}");
    }
}

#[test]
fn editor_reports_caret_pomodoro_links() {
    let parse = editor("^sase:deep-fix");
    assert_eq!(parse.mode, EditorMode::PomodoroLink);
    assert_eq!(parse.body, "");
    assert_eq!(parse.route.as_deref(), Some("sase"));
    assert_eq!(parse.block_id.as_deref(), Some("deep-fix"));
    assert_eq!(parse.section, None);
    assert!(parse.needs.is_empty());
    assert!(parse.pomodoro_start.is_none());
    assert_eq!(
        ranges(&parse),
        vec![
            (0, 5, SpanKind::ActiveTaskRoute),
            (6, 14, SpanKind::ActiveTaskBlockId),
        ]
    );
    assert!(parse.diagnostics.is_empty());

    let named = editor("^SASE:deep-fix#bugs=3");
    assert_eq!(named.mode, EditorMode::PomodoroLink);
    assert_eq!(named.route.as_deref(), Some("sase"));
    assert_eq!(named.section.as_deref(), Some("bugs"));
    let start = named.pomodoro_start.clone().expect("start suffix");
    assert_eq!(start.raw, "3");
    assert_eq!(
        ranges(&named),
        vec![
            (0, 5, SpanKind::ActiveTaskRoute),
            (6, 14, SpanKind::ActiveTaskBlockId),
            (15, 19, SpanKind::PomodoroName),
            (19, 21, SpanKind::PomodoroStart),
        ]
    );
    assert!(named.diagnostics.is_empty());

    let plain_start = editor("^sase:deep-fix=");
    assert_eq!(plain_start.mode, EditorMode::PomodoroLink);
    assert!(plain_start.pomodoro_start.is_some());
}

#[test]
fn editor_reports_caret_partials_as_incomplete() {
    for raw in ["^", "^frag", "^sase:"] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Incomplete, "{raw}");
        assert_eq!(parse.body, "", "{raw}");
        assert_eq!(parse.needs, vec![Need::ActiveTask], "{raw}");
        assert_eq!(
            ranges(&parse),
            vec![(0, raw.len(), SpanKind::InteractivePlaceholder)],
            "{raw}"
        );
        assert!(parse.diagnostics.is_empty(), "{raw}");
    }
    let routed = editor("^sase:");
    assert_eq!(routed.route.as_deref(), Some("sase"));

    let named = editor("^sase:deep-fix#");
    assert_eq!(named.mode, EditorMode::Incomplete);
    assert_eq!(named.needs, vec![Need::PomodoroName]);
    assert_eq!(named.route.as_deref(), Some("sase"));
    assert_eq!(named.block_id.as_deref(), Some("deep-fix"));
    assert_eq!(
        ranges(&named),
        vec![
            (0, 5, SpanKind::ActiveTaskRoute),
            (6, 14, SpanKind::ActiveTaskBlockId),
            (14, 15, SpanKind::InteractivePlaceholder),
        ]
    );
}

#[test]
fn editor_reports_caret_near_misses_and_conflicts() {
    for (raw, message) in [
        (
            "^sase:deep-fix extra",
            "`^route:block-id` must be the whole capture item",
        ),
        (
            "^sase:deep-fix+",
            "`^route:block-id+` is not a capture form",
        ),
        (
            "^sase:deep-fix!",
            "Pomodoro link `^route:block-id!` is a task toggle",
        ),
        (
            "^sase:deep-fix\n- child",
            "Pomodoro link capture cannot be combined with authored child bullets",
        ),
        (
            "^sase:deep-fix s:2",
            "Pomodoro link capture cannot be combined with s:<N>",
        ),
        (
            "^sase:deep-fix p:1",
            "Pomodoro link capture cannot be combined with p:<N>",
        ),
        (
            "^sase:deep-fix %",
            "Pomodoro link capture cannot be combined with % clipboard markers",
        ),
        (
            "^sase:deep-fix==",
            "Pomodoro start suffix must mirror se<X>",
        ),
    ] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::PomodoroLink, "{raw}");
        assert_eq!(codes(&parse), vec!["invalid_pomodoro_link"], "{raw}");
        assert!(
            parse.diagnostics[0].message.starts_with(message),
            "{raw}: {}",
            parse.diagnostics[0].message
        );
        let token_len =
            raw.split_whitespace().next().expect("token").len();
        assert_eq!(
            parse.diagnostics[0].range,
            Some((0, token_len)),
            "{raw}"
        );
    }
}

#[test]
fn editor_leaves_caret_lookalikes_and_prose_literal() {
    for raw in [
        "^_^",
        "^^",
        "^.",
        "^ text",
        "^frag rest",
        "^sase: rest",
        "note ^sase:deep-fix",
        "^sase:deep-fix#\n- child",
    ] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Task, "{raw}");
        assert_eq!(codes(&parse), Vec::<&str>::new(), "{raw}");
        assert!(
            span_kinds(&parse).iter().all(|kind| !matches!(
                kind,
                SpanKind::ActiveTaskRoute | SpanKind::ActiveTaskBlockId
            )),
            "{raw}"
        );
    }
    // `^` is only recognized as the first token of an item's first
    // line: later lines and later items stay prose.
    let child = editor("parent\n- ^sase:deep-fix");
    assert_eq!(child.mode, EditorMode::Task);
    let multi = editor("^sase:deep-fix\n\nsecond item");
    assert_eq!(multi.items.len(), 2);
    assert_eq!(multi.items[0].mode, EditorMode::PomodoroLink);
    assert_eq!(multi.items[1].mode, EditorMode::Task);
}

#[test]
fn editor_never_applies_global_destination_to_caret_items() {
    let parse = editor("@@foo\n^sase:deep-fix");
    assert_eq!(parse.items.len(), 1);
    assert_eq!(parse.items[0].mode, EditorMode::PomodoroLink);
    assert_eq!(parse.items[0].route.as_deref(), Some("sase"));

    let partial = editor("@@foo\n^frag");
    assert_eq!(partial.items.len(), 1);
    assert_eq!(partial.items[0].mode, EditorMode::Incomplete);
    assert_eq!(partial.items[0].needs, vec![Need::ActiveTask]);
}

#[test]
fn editor_normalizes_intra_line_whitespace_like_execution() {
    // Only horizontal whitespace collapses within a physical line now;
    // `\n`/`\r` are line terminators, not normalized-away whitespace.
    let parse = editor(" \t buy\t  milk \t @groceries  ");
    assert_eq!(parse.body, "buy milk");
    assert_eq!(parse.route.as_deref(), Some("groceries"));
}

#[test]
fn normalize_task_text_still_collapses_newlines_as_whitespace() {
    // `normalize_task_text` is a general-purpose whitespace collapser
    // reused for each physical line's own text; called directly on a
    // string that still has embedded newlines, it keeps collapsing them
    // exactly like `split_whitespace` always has.
    assert_eq!(
        normalize_task_text(" \n buy\t  milk \r\n @groceries  "),
        "buy milk @groceries"
    );
}

#[test]
fn editor_serializes_snake_case_vocabulary() {
    let parse = editor("Call bank @Cash+");
    let value = serde_json::json!({
        "mode": parse.mode,
        "needs": parse.needs,
        "spans": parse.spans,
    });
    assert_eq!(value["mode"], "incomplete");
    assert_eq!(value["needs"][0], "task");
    assert_eq!(value["spans"][0]["kind"], "sub_bullet_route");
    assert_eq!(value["spans"][0]["start"], 10);
    assert_eq!(value["spans"][1]["kind"], "interactive_placeholder");
}

#[test]
fn editor_rejects_a_trailing_now_tag_like_any_other_tag() {
    // `#now` is retired: a trailing tag is the ordinary legacy-marker
    // diagnostic with no tag span. The marker stays unresolved, so the
    // body keeps the whole line.
    let parse = editor("Fix it @sase^fix-it #now");
    assert_eq!(parse.mode, EditorMode::Task);
    assert_eq!(parse.body, "Fix it @sase^fix-it #now");
    assert_eq!(parse.route, None);
    assert_eq!(codes(&parse), vec!["legacy_bullet_marker"]);
    assert!(parse.spans.is_empty());

    // The pre-route slot matches too, exactly like any other `#tag`,
    // while the trailing route still resolves.
    let parse = editor("Fix it #now @sase");
    assert_eq!(parse.body, "Fix it #now");
    assert_eq!(parse.route.as_deref(), Some("sase"));
    assert_eq!(codes(&parse), vec!["legacy_bullet_marker"]);

    // Other trailing `#tag` shapes keep today's diagnostic.
    let parse = editor("Some note #bar");
    assert_eq!(codes(&parse), vec!["legacy_bullet_marker"]);

    // Authored child lines report the same diagnostic.
    let parse = editor("Fix it @sase\n- sub #now");
    assert_eq!(parse.body, "Fix it");
    assert_eq!(
        parse
            .sub_bullets
            .iter()
            .map(|item| item.body.as_str())
            .collect::<Vec<_>>(),
        vec!["sub #now"]
    );
    assert!(codes(&parse).contains(&"legacy_bullet_marker"));
}

#[test]
fn editor_rejects_a_partial_now_tag_like_any_other_tag() {
    for raw in ["Fix it @sase #n", "Fix it @sase #no"] {
        let parse = editor(raw);
        assert_eq!(codes(&parse), vec!["legacy_bullet_marker"], "{raw}");
    }
}

#[test]
fn editor_rejects_a_now_tag_without_task_text() {
    for raw in ["@sase:fix-it #now", "@sase+fix-it #now", "#now"] {
        let parse = editor(raw);
        assert_eq!(codes(&parse), vec!["legacy_bullet_marker"], "{raw}");
    }
    // Whole-item operators and links with a trailing tag report their
    // own shape diagnostics. An inline `=x #now` entry is literal text,
    // so it stays a valid close.
    for (raw, expected) in [
        ("^sase:fix-it #now", "invalid_pomodoro_link"),
        ("=3 #now", "invalid_pomodoro_start"),
        ("+5 #now", "invalid_pomodoro_adjustment"),
    ] {
        let parse = editor(raw);
        assert!(
            codes(&parse).contains(&expected),
            "{raw}: {:?}",
            codes(&parse)
        );
    }
    let inline = editor("=x #now");
    assert!(codes(&inline).is_empty(), "=x #now");
}

#[test]
fn task_link_query_spans_the_whole_token_as_a_placeholder() {
    // One `interactive_placeholder` span covers the token, sigil included.
    for (raw, range) in [
        (":", (0, 1)),
        (":dee", (0, 4)),
        ("  :dee", (2, 6)),
        (":déjà", (0, 7)),
    ] {
        let parse = editor(raw);
        assert_eq!(
            ranges(&parse),
            vec![(range.0, range.1, SpanKind::InteractivePlaceholder)],
            "{raw}"
        );
    }
    // In a batch the second item's span uses draft-absolute offsets.
    let batch = parse_for_editor("Buy milk\n\n:dee");
    assert_eq!(
        batch.items[1]
            .spans
            .iter()
            .map(|span| (span.start, span.end, span.kind))
            .collect::<Vec<_>>(),
        vec![(10, 14, SpanKind::InteractivePlaceholder)],
        "batch"
    );
}

#[test]
fn diagnostics_serialize_with_a_nullable_range_pair() {
    let parse = editor("Body @dev+bad.id");
    let value = serde_json::to_value(&parse.diagnostics).expect("json");
    assert_eq!(value[0]["severity"], "error");
    assert_eq!(value[0]["code"], "invalid_sub_bullet_block_id");
    assert_eq!(value[0]["range"][0], 5);
    assert_eq!(value[0]["range"][1], 16);
}
