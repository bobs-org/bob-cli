//! Grammar and marker parsing tests.

use super::super::completion::*;
use super::super::draft::*;
use super::super::editor_classify::*;
use super::super::editor_model::*;
use super::super::markers::*;
use super::super::model::*;
use super::super::project_tasks::*;
use super::super::tokens::*;
use super::*;

#[test]
fn lua_parses_all_four_canonical_pomodoro_forms() {
    let complete = editor("Do work @Dev:focus-123");
    assert_eq!(complete.mode, EditorMode::PomodoroTask);
    assert_eq!(complete.body, "Do work");
    assert_eq!(complete.route.as_deref(), Some("dev"));
    assert_eq!(complete.block_id.as_deref(), Some("focus-123"));
    assert!(complete.needs.is_empty());

    let named = editor("Do work @Dev:focus-123#bugs");
    assert_eq!(named.mode, EditorMode::PomodoroTask);
    assert_eq!(named.section.as_deref(), Some("bugs"));
    assert!(named.needs.is_empty());

    let needs_name = editor("Do work @Dev:focus-123#");
    assert_eq!(needs_name.needs, vec![Need::PomodoroName]);
    assert_eq!(needs_name.block_id.as_deref(), Some("focus-123"));

    let needs_id = editor("Do work @Dev:");
    assert_eq!(needs_id.route.as_deref(), Some("dev"));
    assert_eq!(needs_id.needs, vec![Need::PomodoroId]);

    let needs_target = editor("Do work @:focus-123");
    assert_eq!(needs_target.block_id.as_deref(), Some("focus-123"));
    assert_eq!(needs_target.needs, vec![Need::Route]);

    let needs_both = editor("Do work @:");
    assert_eq!(needs_both.needs, vec![Need::Route, Need::PomodoroId]);
}

#[test]
fn lua_accepts_legacy_boundary_aliases() {
    // Delta from Lua: the Lua fixture used `@!Dev:old_id`, but Bob's
    // block IDs have never accepted `_`, so that exact input is a
    // diagnostic here and the hyphenated form is the passing case.
    let complete = editor("Do work @!Dev:old-id");
    assert_eq!(complete.mode, EditorMode::PomodoroTask);
    assert_eq!(complete.route.as_deref(), Some("dev"));
    assert_eq!(complete.block_id.as_deref(), Some("old-id"));

    let underscored = editor("Do work @!Dev:old_id");
    assert_eq!(codes(&underscored), vec!["invalid_pomodoro_block_id"]);

    let route_only = editor("Do work @!Dev");
    assert_eq!(route_only.route.as_deref(), Some("dev"));
    assert_eq!(route_only.needs, vec![Need::PomodoroId]);

    let neither = editor("Do work @!");
    assert_eq!(neither.needs, vec![Need::Route, Need::PomodoroId]);
}

#[test]
fn lua_parses_all_four_canonical_sub_bullet_forms() {
    let complete = editor("Add context @Dev+focus-123");
    assert_eq!(complete.mode, EditorMode::SubBullet);
    assert_eq!(complete.body, "Add context");
    assert_eq!(complete.route.as_deref(), Some("dev"));
    assert_eq!(complete.block_id.as_deref(), Some("focus-123"));
    assert!(complete.needs.is_empty());

    let needs_task = editor("Add context @Dev+");
    assert_eq!(needs_task.route.as_deref(), Some("dev"));
    assert_eq!(needs_task.needs, vec![Need::Task]);

    let needs_target = editor("Add context @+focus-123");
    assert_eq!(needs_target.block_id.as_deref(), Some("focus-123"));
    assert_eq!(needs_target.needs, vec![Need::Route]);

    let needs_both = editor("Add context @+");
    assert_eq!(needs_both.needs, vec![Need::Route, Need::Task]);

    let with_section = editor("Add context @Dev+focus-123#req");
    assert_eq!(with_section.mode, EditorMode::SubBullet);
    assert_eq!(with_section.block_id.as_deref(), Some("focus-123"));
    assert_eq!(with_section.section.as_deref(), Some("req"));
    assert!(with_section.needs.is_empty());

    let needs_section = editor("Add context @Dev+focus-123#");
    assert_eq!(needs_section.needs, vec![Need::TaskSection]);
    assert_eq!(needs_section.block_id.as_deref(), Some("focus-123"));
}

#[test]
fn a_bare_sub_bullet_marker_becomes_a_task_toggle() {
    let bare = editor("@Cash+Goog-Exit");
    assert_eq!(bare.mode, EditorMode::TaskToggle);
    assert_eq!(bare.body, "");
    assert_eq!(bare.route.as_deref(), Some("cash"));
    assert_eq!(bare.block_id.as_deref(), Some("Goog-Exit"));
    assert!(bare.section.is_none());
    assert!(bare.needs.is_empty());
    assert!(bare.diagnostics.is_empty());
    assert_eq!(
        span_kinds(&bare),
        vec![SpanKind::TaskToggleRoute, SpanKind::TaskToggleBlockId]
    );

    let named = editor("@Cash+Goog-Exit#deep+work");
    assert_eq!(named.mode, EditorMode::TaskToggle);
    assert_eq!(named.section.as_deref(), Some("deep+work"));
    assert!(named.diagnostics.is_empty());
    assert_eq!(
        span_kinds(&named),
        vec![
            SpanKind::TaskToggleRoute,
            SpanKind::TaskToggleBlockId,
            SpanKind::TaskTogglePomodoroName,
        ]
    );

    // A trailing bare `#` on an otherwise-empty item is one keystroke
    // from a toggle: stay `incomplete`, but ask for a Pomodoro name
    // instead of a task section.
    let incomplete = editor("@cash+goog-exit#");
    assert_eq!(incomplete.mode, EditorMode::Incomplete);
    assert_eq!(incomplete.needs, vec![Need::PomodoroName]);
    assert_eq!(
        span_kinds(&incomplete),
        vec![
            SpanKind::TaskToggleRoute,
            SpanKind::TaskToggleBlockId,
            SpanKind::InteractivePlaceholder,
        ]
    );

    // The same trailing bare `#` keeps its task-section meaning once the
    // item has body text -- unaffected by the toggle rule.
    let with_body = editor("note @cash+goog-exit#");
    assert_eq!(with_body.mode, EditorMode::Incomplete);
    assert_eq!(with_body.needs, vec![Need::TaskSection]);

    // Any authored child bullet disqualifies the item from toggling, so
    // it still just needs text.
    let with_child = editor("@cash+goog-exit\n- detail");
    assert_ne!(with_child.mode, EditorMode::TaskToggle);
    assert_eq!(with_child.mode, EditorMode::SubBullet);

    // A schedule/priority/clip marker on the item also disqualifies it.
    let with_schedule = editor("@cash+goog-exit s:2");
    assert_ne!(with_schedule.mode, EditorMode::TaskToggle);
}

#[test]
fn a_terminal_bang_on_a_marker_only_toggle_is_explicit_toggle() {
    let parse = editor("@dev+id!");
    assert_eq!(parse.mode, EditorMode::TaskToggle);
    assert_eq!(parse.body, "");
    assert_eq!(parse.route.as_deref(), Some("dev"));
    assert_eq!(parse.block_id.as_deref(), Some("id"));
    assert!(parse.section.is_none());
    assert!(parse.needs.is_empty());
    assert!(parse.diagnostics.is_empty());
    assert_eq!(
        span_kinds(&parse),
        vec![
            SpanKind::TaskToggleRoute,
            SpanKind::TaskToggleBlockId,
            SpanKind::TaskToggleExplicitToggle,
        ]
    );
    assert_eq!(
        ranges(&parse),
        vec![
            (0, 4, SpanKind::TaskToggleRoute),
            (5, 7, SpanKind::TaskToggleBlockId),
            (7, 8, SpanKind::TaskToggleExplicitToggle),
        ]
    );

    let executed = execute("@dev+id!").expect("parse");
    assert_eq!(executed.body, "");
    assert_eq!(executed.route.as_deref(), Some("dev"));
    assert_eq!(
        executed.kind,
        CaptureKind::TaskToggle {
            block_id: "id".to_string(),
            pomodoro_name: None,
            intent: TaskToggleIntent::Toggle,
        }
    );

    let café = editor("café @dev+id!");
    assert_eq!(codes(&café), vec!["unsupported_explicit_toggle"]);
    assert!(
        café.diagnostics[0]
            .message
            .contains("marker-only `@<route>+<block-id>!`"),
        "{:?}",
        café.diagnostics[0].message
    );
    let bang = "café @dev+id!".rfind('!').expect("bang");
    assert_eq!(café.diagnostics[0].range, Some((bang, bang + 1)));
}

#[test]
fn marker_only_task_toggle_spellings_are_a_three_way_intent_matrix() {
    let plain = execute("@dev+id").expect("plain ensure-Next");
    assert_eq!(
        plain.kind,
        CaptureKind::TaskToggle {
            block_id: "id".to_string(),
            pomodoro_name: None,
            intent: TaskToggleIntent::EnsureNext,
        }
    );
    let plain_editor = editor("@dev+id");
    assert_eq!(plain_editor.mode, EditorMode::TaskToggle);
    assert_eq!(
        span_kinds(&plain_editor),
        vec![SpanKind::TaskToggleRoute, SpanKind::TaskToggleBlockId]
    );

    let named = execute("@dev+id#deep+work").expect("named ensure-Next");
    assert_eq!(
        named.kind,
        CaptureKind::TaskToggle {
            block_id: "id".to_string(),
            pomodoro_name: Some("deep+work".to_string()),
            intent: TaskToggleIntent::EnsureNext,
        }
    );
    assert_eq!(named.route.as_deref(), Some("dev"));
    let named_editor = editor("@dev+id#deep+work");
    assert_eq!(named_editor.mode, EditorMode::TaskToggle);
    assert_eq!(
        span_kinds(&named_editor),
        vec![
            SpanKind::TaskToggleRoute,
            SpanKind::TaskToggleBlockId,
            SpanKind::TaskTogglePomodoroName,
        ]
    );
    assert!(!span_kinds(&named_editor)
        .contains(&SpanKind::TaskToggleExplicitToggle));

    let bang = execute("@dev+id!").expect("explicit toggle");
    assert_eq!(
        bang.kind,
        CaptureKind::TaskToggle {
            block_id: "id".to_string(),
            pomodoro_name: None,
            intent: TaskToggleIntent::Toggle,
        }
    );
    let bang_editor = editor("@dev+id!");
    assert_eq!(
        span_kinds(&bang_editor),
        vec![
            SpanKind::TaskToggleRoute,
            SpanKind::TaskToggleBlockId,
            SpanKind::TaskToggleExplicitToggle,
        ]
    );
}

#[test]
fn explicit_toggle_near_misses_have_focused_diagnostics() {
    let named = editor("@dev+id#now!");
    assert_eq!(codes(&named), vec!["unsupported_explicit_toggle"]);
    assert!(
        named.diagnostics[0].message.contains("#<pomodoro>"),
        "{}",
        named.diagnostics[0].message
    );

    let hash_bang = editor("@dev+id#!");
    assert_eq!(codes(&hash_bang), vec!["unsupported_explicit_toggle"]);
    assert!(
        hash_bang.diagnostics[0].message.contains("#<pomodoro>"),
        "{}",
        hash_bang.diagnostics[0].message
    );

    let repeated = editor("@dev+id!!");
    assert_eq!(codes(&repeated), vec!["unsupported_explicit_toggle"]);
    assert!(
        repeated.diagnostics[0].message.contains("repeated"),
        "{}",
        repeated.diagnostics[0].message
    );

    let global = editor("@@dev+id!\nTask");
    assert_eq!(codes(&global), vec!["unsupported_explicit_toggle"]);
    assert!(
        global.diagnostics[0].message.contains("`@@`"),
        "{}",
        global.diagnostics[0].message
    );

    for raw in [
        "@dev+id! s:1",
        "@dev+id! p:2",
        "@dev+id! %",
        "@dev+id!\n- child",
        "body @dev+id!",
    ] {
        let parse = editor(raw);
        assert!(
            codes(&parse).contains(&"unsupported_explicit_toggle"),
            "{raw}: {:?}",
            parse.diagnostics
        );
        let executed = execute(raw).expect_err(raw);
        assert!(
            executed.contains("marker-only `@<route>+<block-id>!`"),
            "{raw}: {executed}"
        );
    }

    let executed_named = execute("@dev+id#now!").expect_err("named");
    assert!(executed_named.contains("#<pomodoro>"), "{executed_named}");
    let executed_repeated = execute("@dev+id!!").expect_err("repeated");
    assert!(
        executed_repeated.contains("repeated"),
        "{executed_repeated}"
    );
    let executed_global = parse_capture_draft_with_clip_control(
        "@@dev+id!\nTask",
        None,
        None,
        true,
    )
    .expect_err("global");
    assert!(executed_global.contains("`@@`"), "{executed_global}");

    let literal = editor("Wow! @dev");
    assert!(literal.diagnostics.is_empty(), "{:?}", literal.diagnostics);
    assert_eq!(literal.body, "Wow!");
    let prose = execute("ship it!").expect("literal bang");
    assert_eq!(prose.body, "ship it!");
    assert_eq!(prose.kind, CaptureKind::Task);
}

#[test]
fn a_toggle_with_body_text_stays_a_sub_bullet_marker() {
    // A `+`-name is only valid Pomodoro syntax for a genuine toggle; a
    // sub-bullet capture with real body text still enforces the
    // stricter task-section charset and reports the sub-bullet code,
    // even though the relaxed parse-time check let it through.
    let rejected = editor("Add context @sase+goog-exit#a+b");
    assert_eq!(codes(&rejected), vec!["invalid_sub_bullet_section"]);
    assert_eq!(rejected.mode, EditorMode::Task);
    assert_eq!(rejected.body, "Add context");
    assert!(rejected.route.is_none());
    assert!(rejected.needs.is_empty());
    assert!(
        span_kinds(&rejected).iter().all(|kind| !matches!(
            kind,
            SpanKind::SubBulletRoute
                | SpanKind::SubBulletBlockId
                | SpanKind::SubBulletSection
                | SpanKind::TaskToggleRoute
                | SpanKind::TaskToggleBlockId
                | SpanKind::TaskTogglePomodoroName
        )),
        "{:?}",
        rejected.spans
    );
}

#[test]
fn lua_parses_all_four_canonical_task_block_id_forms() {
    let complete = editor("Do work @Dev^focus-123");
    assert_eq!(complete.mode, EditorMode::Task);
    assert_eq!(complete.body, "Do work");
    assert_eq!(complete.route.as_deref(), Some("dev"));
    assert_eq!(complete.block_id.as_deref(), Some("focus-123"));
    assert!(complete.needs.is_empty());

    let needs_id = editor("Do work @Dev^");
    assert_eq!(needs_id.route.as_deref(), Some("dev"));
    assert_eq!(needs_id.needs, vec![Need::BlockId]);

    let needs_target = editor("Do work @^focus-123");
    assert_eq!(needs_target.block_id.as_deref(), Some("focus-123"));
    assert_eq!(needs_target.needs, vec![Need::Route]);

    let needs_both = editor("Do work @^");
    assert_eq!(needs_both.needs, vec![Need::Route, Need::BlockId]);
}

#[test]
fn lua_gives_sub_bullet_markers_precedence_over_pomodoro_markers() {
    let malformed = editor("Add context @route+bad:id");
    assert_eq!(codes(&malformed), vec!["invalid_sub_bullet_block_id"]);
    assert!(
        malformed.diagnostics[0].message.contains("sub-bullet"),
        "{:?}",
        malformed.diagnostics
    );

    let pomodoro = editor("Do work @route:id");
    assert_eq!(pomodoro.mode, EditorMode::PomodoroTask);
    assert_eq!(pomodoro.block_id.as_deref(), Some("id"));
}

#[test]
fn plus_in_a_pomodoro_name_does_not_select_the_sub_bullet_family() {
    assert!(is_pomodoro_selector_component("c++"));
    assert!(is_pomodoro_selector_component("bob+sase"));
    assert!(is_pomodoro_selector_component("a+b"));
    assert!(!is_selector_component("c++"));
    assert!(!is_selector_component("a+b"));

    let parsed = parse_capture_text_with_clip_control(
        "Do work @sase:deep-fix#c++",
        None,
        None,
        true,
    )
    .expect("pomodoro with plus name");
    assert_eq!(parsed.route.as_deref(), Some("sase"));
    assert_eq!(
        parsed.kind,
        CaptureKind::Pomodoro {
            block_id: "deep-fix".to_string(),
            pomodoro_name: Some("c++".to_string()),
            start: None,
            close: None,
        }
    );

    let infix = parse_capture_text_with_clip_control(
        "Do work @sase:deep-fix#bob+sase",
        None,
        None,
        true,
    )
    .expect("pomodoro with infix plus");
    assert_eq!(
        infix.kind,
        CaptureKind::Pomodoro {
            block_id: "deep-fix".to_string(),
            pomodoro_name: Some("bob+sase".to_string()),
            start: None,
            close: None,
        }
    );

    let sub_bullet = parse_capture_text_with_clip_control(
        "Add context @sase+goog-exit#a+b",
        None,
        None,
        true,
    );
    assert_eq!(sub_bullet, Err(SUB_BULLET_SECTION_ERROR.to_string()));

    let parse = editor("Do work @sase:deep-fix#c++");
    assert_eq!(parse.mode, EditorMode::PomodoroTask);
    assert_eq!(parse.section.as_deref(), Some("c++"));
    assert!(parse.diagnostics.is_empty());
    assert_eq!(
        span_kinds(&parse),
        vec![
            SpanKind::PomodoroRoute,
            SpanKind::PomodoroBlockId,
            SpanKind::PomodoroName,
        ]
    );
    let name_span = parse
        .spans
        .iter()
        .find(|span| span.kind == SpanKind::PomodoroName)
        .expect("pomodoro_name span");
    assert_eq!(
        &"Do work @sase:deep-fix#c++"[name_span.start..name_span.end],
        "c++"
    );

    let rejected = editor("Add context @sase+goog-exit#a+b");
    assert_eq!(codes(&rejected), vec!["invalid_sub_bullet_section"]);

    let incomplete = editor("Body @:#a+b");
    assert_eq!(incomplete.mode, EditorMode::Incomplete);
    assert_eq!(incomplete.section.as_deref(), Some("a+b"));
    assert!(incomplete.diagnostics.is_empty());
    assert_eq!(incomplete.needs, vec![Need::Route, Need::PomodoroId]);

    let raw = "Do work @sase:deep-fix#c+";
    let hash = raw.find('#').expect("hash");
    let completion = field(raw, raw.len()).expect("pomodoro name");
    assert_eq!(completion.context, CompletionContext::PomodoroName);
    assert_eq!(completion.query, "c+");
    assert_eq!(completion.replacement, (hash + 1, raw.len()));

    let inside = "Do work @sase:deep-fix#c++";
    let inside_hash = inside.find('#').expect("hash");
    let inside_field =
        field(inside, inside_hash + 2).expect("cursor inside c++");
    assert_eq!(inside_field.context, CompletionContext::PomodoroName);
    assert_eq!(inside_field.query, "c");
    assert_eq!(inside_field.replacement, (inside_hash + 1, inside.len()));
    assert_eq!(
        &inside[inside_field.replacement.0..inside_field.replacement.1],
        "c++"
    );
}

#[test]
fn lua_rejects_invalid_sub_bullet_and_pomodoro_components() {
    let cases = [
        ("Add context @bad.route+id", "invalid_sub_bullet_route"),
        ("Add context @route+bad.id", "invalid_sub_bullet_block_id"),
        ("Add context @route+bad_id", "invalid_sub_bullet_block_id"),
        ("Add context @route+id#bad_id", "invalid_sub_bullet_section"),
        ("Do work @bad.route:id", "invalid_pomodoro_route"),
        ("Do work @route:bad.id", "invalid_pomodoro_block_id"),
        ("Do work @route:id:extra", "invalid_pomodoro_block_id"),
        ("Do work @route:id#bad_id", "invalid_pomodoro_name"),
        ("Do work @!:id", "invalid_pomodoro_route"),
    ];

    for (raw, code) in cases {
        let parse = editor(raw);
        assert_eq!(codes(&parse), vec![code], "{raw}");
        assert_eq!(parse.diagnostics[0].severity, Severity::Error, "{raw}");
    }
}

#[test]
fn lua_keeps_middle_markers_literal_and_marker_only_bodies_empty() {
    let parse = editor("Discuss @dev:id later");
    assert_eq!(parse.mode, EditorMode::Task);
    assert_eq!(parse.body, "Discuss @dev:id later");

    let parse = editor("Discuss @dev+id later");
    assert_eq!(parse.mode, EditorMode::Task);
    assert_eq!(parse.body, "Discuss @dev+id later");

    let parse = editor("Discuss @dev^id later");
    assert_eq!(parse.mode, EditorMode::Task);
    assert_eq!(parse.body, "Discuss @dev^id later");

    for raw in ["@dev:id", "@:", "@dev+id", "@+", "@dev^id", "@^"] {
        assert_eq!(editor(raw).body, "", "{raw}");
    }
}

#[test]
fn lua_composes_clipboard_terminal_markers_around_every_picker_token() {
    // (token, mode, route, section, block_id, needs)
    let cases: &[MarkerCase] = &[
        (
            "@",
            EditorMode::Incomplete,
            None,
            None,
            None,
            &[Need::Route],
        ),
        (
            "@#",
            EditorMode::Incomplete,
            None,
            None,
            None,
            &[Need::Route],
        ),
        (
            "@#Ideas",
            EditorMode::Incomplete,
            None,
            Some("Ideas"),
            None,
            &[Need::Route],
        ),
        (
            "@Notes#",
            EditorMode::Bullet,
            Some("notes"),
            None,
            None,
            &[Need::Section],
        ),
        (
            "@Dev:focus-123",
            EditorMode::PomodoroTask,
            Some("dev"),
            None,
            Some("focus-123"),
            &[],
        ),
        (
            "@Dev:",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            None,
            &[Need::PomodoroId],
        ),
        (
            "@:focus-123",
            EditorMode::Incomplete,
            None,
            None,
            Some("focus-123"),
            &[Need::Route],
        ),
        (
            "@:",
            EditorMode::Incomplete,
            None,
            None,
            None,
            &[Need::Route, Need::PomodoroId],
        ),
        (
            "@!Dev:focus-123",
            EditorMode::PomodoroTask,
            Some("dev"),
            None,
            Some("focus-123"),
            &[],
        ),
        (
            "@!Dev",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            None,
            &[Need::PomodoroId],
        ),
        (
            "@!",
            EditorMode::Incomplete,
            None,
            None,
            None,
            &[Need::Route, Need::PomodoroId],
        ),
        (
            "@Dev+focus-123",
            EditorMode::SubBullet,
            Some("dev"),
            None,
            Some("focus-123"),
            &[],
        ),
        (
            "@Dev+",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            None,
            &[Need::Task],
        ),
        (
            "@+focus-123",
            EditorMode::Incomplete,
            None,
            None,
            Some("focus-123"),
            &[Need::Route],
        ),
        (
            "@+",
            EditorMode::Incomplete,
            None,
            None,
            None,
            &[Need::Route, Need::Task],
        ),
        (
            "@Dev+focus-123#req",
            EditorMode::SubBullet,
            Some("dev"),
            Some("req"),
            Some("focus-123"),
            &[],
        ),
        (
            "@Dev^focus-123",
            EditorMode::Task,
            Some("dev"),
            None,
            Some("focus-123"),
            &[],
        ),
        (
            "@Dev^",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            None,
            &[Need::BlockId],
        ),
        (
            "@^focus-123",
            EditorMode::Incomplete,
            None,
            None,
            Some("focus-123"),
            &[Need::Route],
        ),
        (
            "@^",
            EditorMode::Incomplete,
            None,
            None,
            None,
            &[Need::Route, Need::BlockId],
        ),
    ];

    for (token, mode, route, section, block_id, needs) in cases {
        for marker in ["%", "%03", "%build_log"] {
            for raw in [
                format!("Body {marker} {token}"),
                format!("Body {token} {marker}"),
            ] {
                let parse = editor(&raw);
                assert_eq!(parse.mode, *mode, "{raw}");
                assert_eq!(parse.route.as_deref(), *route, "{raw}");
                assert_eq!(parse.section.as_deref(), *section, "{raw}");
                assert_eq!(parse.block_id.as_deref(), *block_id, "{raw}");
                assert_eq!(parse.needs, needs.to_vec(), "{raw}");
                assert!(parse.diagnostics.is_empty(), "{raw}");
            }
        }
    }
}

#[test]
fn lua_clipboard_composition_body_follows_bob_terminal_extraction() {
    // Delta from Lua: Hammerspoon left `%`/`s:`/`p:` in the body for
    // `bob capture` to interpret, while this module extracts them into
    // structured spans. A clipboard marker that precedes an incomplete
    // marker still stays in the body, because `is_route_marker` (shared
    // with execution) only recognizes complete route tokens.
    assert_eq!(editor("Body % @Dev:focus-123").body, "Body");
    assert_eq!(editor("Body @Dev:focus-123 %").body, "Body");
    assert_eq!(editor("Body @Dev+ %build_log").body, "Body");
    assert_eq!(editor("Body % @Dev+").body, "Body %");
    assert_eq!(editor("Body @Dev^ %build_log").body, "Body");
    assert_eq!(editor("Body % @Dev^").body, "Body %");
}

#[test]
fn lua_preserves_crossed_clipboard_and_schedule_markers() {
    let cases: &[CrossedMarkerCase] = &[
        (
            "Body @Notes# % s:2",
            EditorMode::Bullet,
            Some("notes"),
            None,
            "Body",
        ),
        (
            "Body @Notes# s:2 %03",
            EditorMode::Bullet,
            Some("notes"),
            None,
            "Body",
        ),
        (
            "Body % @Notes# s:2",
            EditorMode::Bullet,
            Some("notes"),
            None,
            "Body",
        ),
        (
            "Body s:2 @Notes# %build_log",
            EditorMode::Bullet,
            Some("notes"),
            None,
            "Body",
        ),
        (
            "Body @:focus-123 % s:0",
            EditorMode::Incomplete,
            None,
            Some("focus-123"),
            "Body",
        ),
        (
            "Body @Dev+ s:10 %build_log",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            "Body",
        ),
        (
            "Body @Dev^ s:10 %build_log",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            "Body",
        ),
        (
            "Body @Notes# p:2 s:1",
            EditorMode::Bullet,
            Some("notes"),
            None,
            "Body",
        ),
        (
            "Body p:2 @Notes# s:1",
            EditorMode::Bullet,
            Some("notes"),
            None,
            "Body",
        ),
        (
            "Body @:focus-123 p:3 s:0",
            EditorMode::Incomplete,
            None,
            Some("focus-123"),
            "Body",
        ),
        (
            "Body @Dev+ p:4 s:10",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            "Body",
        ),
        (
            "Body @Dev^ p:4 s:10",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            "Body",
        ),
    ];

    for (raw, mode, route, block_id, body) in cases {
        let parse = editor(raw);
        assert_eq!(parse.mode, *mode, "{raw}");
        assert_eq!(parse.route.as_deref(), *route, "{raw}");
        assert_eq!(parse.block_id.as_deref(), *block_id, "{raw}");
        assert_eq!(parse.body, *body, "{raw}");
    }

    // Delta from Lua: a leading `p:N` that sits before an incomplete
    // marker is not part of the terminal region for either parser, so it
    // stays in the body here just as `bob capture` would keep it.
    let parse = editor("Body p:3 @:focus-123 s:0");
    assert_eq!(parse.body, "Body p:3");
    assert_eq!(parse.block_id.as_deref(), Some("focus-123"));
}

#[test]
fn lua_preserves_existing_note_and_section_descriptors() {
    let parse = editor("Task @");
    assert_eq!(parse.mode, EditorMode::Incomplete);
    assert_eq!(parse.body, "Task");
    assert_eq!(parse.needs, vec![Need::Route]);

    let parse = editor("Idea @#");
    assert_eq!(parse.mode, EditorMode::Incomplete);
    assert_eq!(parse.body, "Idea");
    assert_eq!(parse.section, None);

    let parse = editor("Idea @#Ideas");
    assert_eq!(parse.mode, EditorMode::Incomplete);
    assert_eq!(parse.section.as_deref(), Some("Ideas"));

    let parse = editor("Idea @Notes#");
    assert_eq!(parse.mode, EditorMode::Bullet);
    assert_eq!(parse.route.as_deref(), Some("notes"));
    assert_eq!(parse.needs, vec![Need::Section]);

    // Delta from Lua: Hammerspoon left `@route#prefix` and plain
    // `@route` tokens to `bob capture`, so it reported mode "none".
    // Bob's own grammar resolves both, and this endpoint is
    // authoritative, so it reports the resolved capture instead.
    let parse = editor("Idea @notes#time:box");
    assert_eq!(parse.mode, EditorMode::Bullet);
    assert_eq!(parse.section.as_deref(), Some("time:box"));
    assert_eq!(parse.body, "Idea");

    let parse = editor("Idea @notes#Ideas");
    assert_eq!(parse.mode, EditorMode::Bullet);
    assert_eq!(parse.section.as_deref(), Some("Ideas"));

    let parse = editor("Task @dev");
    assert_eq!(parse.mode, EditorMode::Task);
    assert_eq!(parse.route.as_deref(), Some("dev"));
    assert_eq!(parse.body, "Task");
}

#[test]
fn lua_leaves_invalid_or_unsupported_terminal_regions_to_bob_capture() {
    // These stay literal for both parsers: the terminal token is not a
    // marker Bob accepts, so nothing is extracted and nothing routes.
    for raw in [
        "Idea @Notes# %0",
        "Idea @Notes# %bad.header",
        "Idea @Notes# %18446744073709551616",
        "Idea @Notes# s:18446744073709551616",
        "Idea @Notes# p:18446744073709551616",
    ] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Task, "{raw}");
        assert_eq!(parse.body, raw, "{raw}");
        assert_eq!(parse.route, None, "{raw}");
    }

    // Delta from Lua: Bob still consumes the first valid marker of each
    // kind before it stops at the duplicate or non-marker, so the body
    // loses that token while the route stays unresolved.
    for (raw, body) in [
        ("Idea @Notes# % %3", "Idea @Notes# %"),
        ("Idea @Notes# s:1 s:2", "Idea @Notes# s:1"),
        ("Idea @Notes# % s:1 %build_log", "Idea @Notes# %"),
        ("Idea @Notes# middle %", "Idea @Notes# middle"),
        ("Idea @Notes# p:1 p:2", "Idea @Notes# p:1"),
    ] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Task, "{raw}");
        assert_eq!(parse.body, body, "{raw}");
        assert_eq!(parse.route, None, "{raw}");
    }

    // Delta from Lua: Bob resolves plain `@route` and `@route#prefix`
    // tokens itself, so these are complete captures rather than "none".
    let parse = editor("Task @dev %");
    assert_eq!(parse.mode, EditorMode::Task);
    assert_eq!(parse.route.as_deref(), Some("dev"));
    assert_eq!(parse.body, "Task");

    let parse = editor("Idea @notes#Ideas %");
    assert_eq!(parse.mode, EditorMode::Bullet);
    assert_eq!(parse.section.as_deref(), Some("Ideas"));
    assert_eq!(parse.body, "Idea");
}

#[test]
fn execution_ordinary_single_line_capture_has_no_sub_bullets() {
    let parsed = execute("buy milk @groceries").expect("parse");
    assert!(parsed.sub_bullets.is_empty());
}

#[test]
fn execution_retired_double_colon_is_a_usage_error() {
    for raw in [
        "Do thing @Dev::Foo-Bar",
        "@Dev::Foo-Bar Do thing",
        "body @cash::",
        "body @::id",
    ] {
        let error = execute(raw).expect_err(raw);
        assert_eq!(error, RETIRED_DOUBLE_COLON_ERROR, "{raw}");
    }
}

#[test]
fn execution_parses_equals_family_starts_alongside_close() {
    // Exact starts: bare and counted suffixes mirror `se<X>` timing.
    for (raw, suffix, duration, offset) in [
        ("=", "", 5, 0),
        ("  =  ", "", 5, 0),
        ("=-", "-", 5, 1),
        ("=3", "3", 3, 0),
        ("=-2", "-2", 5, 2),
        ("=3-", "3-", 3, 1),
        ("=2-1", "2-1", 2, 1),
        ("=0", "0", 0, 0),
        ("=03", "03", 3, 0),
    ] {
        let parsed =
            execute(raw).unwrap_or_else(|error| panic!("{raw}: {error}"));
        assert_eq!(parsed.body, raw.trim(), "{raw}");
        match parsed.kind {
            CaptureKind::PomodoroStart { spec, .. } => {
                assert_eq!(spec.raw, suffix, "{raw}");
                assert_eq!(spec.duration_units, duration, "{raw}");
                assert_eq!(spec.offset_units, offset, "{raw}");
            }
            other => panic!("{raw}: expected start, got {other:?}"),
        }
    }
    // A counted token with extra text, markers, or child lines echoes
    // the typed token in its shape error; an exact token with child
    // lines fails bare or counted. Never a task.
    for (raw, token, suffix) in [
        ("=3 more", "=3", "3"),
        ("=-2 @work", "=-2", "-2"),
        ("=3s:1", "=3", "3"),
        ("=2-1-", "=2-1", "2-1"),
        ("=3x", "=3", "3"),
        ("=\n- child", "=", ""),
        ("=2\n- child", "=2", "2"),
    ] {
        let error = execute(raw).expect_err(raw);
        assert_eq!(
            error,
            format!(
                "Pomodoro start `{token}` must be the whole capture item; remove extra text, markers, or child lines (to start a task's session instead, use `^route:block-id={suffix}`)"
            ),
            "{raw}"
        );
    }
    // Oversized values fail the shared start overflow before any write.
    let overflow = execute("=99999999999999999999999").expect_err("overflow");
    assert_eq!(overflow, POMODORO_START_OVERFLOW_ERROR, "{overflow}");
    // Bare tokens with prose, other close shapes, and mid-body tokens
    // stay ordinary tasks.
    for raw in ["= foo", "=- foo", "==", "=-)", "Plan =3", "a=3"] {
        let parsed =
            execute(raw).unwrap_or_else(|error| panic!("{raw}: {error}"));
        assert_eq!(parsed.kind, CaptureKind::Task, "{raw}");
    }
    // A forced route rejects an exact start with the start copy.
    let forced =
        parse_capture_text_with_clip_control("=3", Some("work"), None, true)
            .expect_err("forced");
    assert_eq!(forced, POMODORO_START_FORCED_ERROR, "{forced}");
    // Close shapes keep today's meaning.
    let close = execute("=x").expect("close");
    assert!(
        matches!(close.kind, CaptureKind::PomodoroClose { .. }),
        "=x"
    );
    let close_shape = execute("=x more").expect_err("close shape");
    assert!(
        close_shape.contains("`=x` must be the whole"),
        "{close_shape}"
    );
    for raw in ["=xx", "=xa"] {
        let parsed =
            execute(raw).unwrap_or_else(|error| panic!("{raw}: {error}"));
        assert_eq!(parsed.kind, CaptureKind::Task, "{raw}");
    }
    // A dangling separator is an editing state: strict execution rejects
    // it instead of guessing.
    let dangling = execute("=x!").expect_err("dangling close");
    assert!(
        dangling.contains("`=x!` is incomplete: type a task number after `!`"),
        "{dangling}"
    );
}

#[test]
fn execution_plus_sub_bullet_does_not_conflict_with_authored_plus_child() {
    let parsed =
        execute("parent line\n+ authored child @dev+focus-123\n+ second child")
            .expect("parse");
    assert_eq!(parsed.body, "parent line");
    assert_eq!(parsed.route.as_deref(), Some("dev"));
    assert_eq!(
        parsed.kind,
        CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId("focus-123".to_string()),
            section: None,
        }
    );
    assert_eq!(
        sub_bullet_bodies(&parsed.sub_bullets),
        vec!["authored child", "second child"]
    );
}

#[test]
fn execution_parses_three_component_sub_bullet_markers() {
    let cases = [
        (
            "Postgres 17 minimum @foo+bar#requirements",
            "Postgres 17 minimum",
            "requirements",
            None,
            None,
        ),
        (
            "@foo+bar#requirements Postgres 17 minimum",
            "Postgres 17 minimum",
            "requirements",
            None,
            None,
        ),
        (
            "note @foo+bar#future-work s:1",
            "note",
            "future-work",
            Some(1),
            None,
        ),
        (
            "note s:1 @foo+bar#future-work",
            "note",
            "future-work",
            Some(1),
            None,
        ),
        ("note p:2 @foo+bar#Q&A", "note", "Q&A", None, Some(2)),
        ("note @foo+bar#Q&A p:2", "note", "Q&A", None, Some(2)),
        (
            "note %log @foo+bar#non-goals",
            "note",
            "non-goals",
            None,
            None,
        ),
        (
            "note @foo+bar#non-goals %log",
            "note",
            "non-goals",
            None,
            None,
        ),
    ];
    for (raw, body, section, scheduled, priority) in cases {
        let parsed =
            execute(raw).unwrap_or_else(|error| panic!("{raw}: {error}"));
        assert_eq!(parsed.body, body, "{raw}");
        assert_eq!(parsed.route.as_deref(), Some("foo"), "{raw}");
        assert_eq!(parsed.scheduled_offset, scheduled, "{raw}");
        assert_eq!(parsed.priority_level, priority, "{raw}");
        assert_eq!(
            parsed.kind,
            CaptureKind::SubBullet {
                target: SubBulletTarget::BlockId("bar".to_string()),
                section: Some(TaskSectionSelector {
                    text: section.to_string(),
                    exact: false,
                }),
            },
            "{raw}"
        );
    }
}

#[test]
fn execution_three_component_marker_composes_on_multiline_first_line_only() {
    let parsed = execute(
        "@foo+bar#requirements parent line\n- first child\n- second child",
    )
    .expect("parse");
    assert_eq!(parsed.body, "parent line");
    assert_eq!(
        parsed.kind,
        CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId("bar".to_string()),
            section: Some(TaskSectionSelector {
                text: "requirements".to_string(),
                exact: false,
            }),
        }
    );
    assert_eq!(
        sub_bullet_bodies(&parsed.sub_bullets),
        vec!["first child", "second child"]
    );

    let parsed = execute(
        "parent line\n- first child @foo+bar#requirements\n- second child",
    )
    .expect("trailing child marker");
    assert_eq!(parsed.body, "parent line");
    assert_eq!(parsed.route.as_deref(), Some("foo"));
    assert_eq!(
        sub_bullet_bodies(&parsed.sub_bullets),
        vec!["first child", "second child"]
    );

    let parsed = execute("parent @foo+bar#req later\n- child")
        .expect("mid-text stays literal");
    assert_eq!(parsed.body, "parent @foo+bar#req later");
    assert_eq!(parsed.kind, CaptureKind::Task);
    assert_eq!(parsed.route, None);
}

#[test]
fn execution_keeps_pomodoro_note_and_other_families_unchanged() {
    let parsed =
        execute("remembered to bump the timeout #").expect("bare hash");
    assert_eq!(parsed.kind, CaptureKind::PomodoroNote);
    assert_eq!(parsed.body, "remembered to bump the timeout");

    let parsed = execute("Some note @foo#Ideas").expect("note bullet");
    assert!(matches!(
        parsed.kind,
        CaptureKind::Bullet {
            section_prefix: Some(ref prefix),
            exact: false,
        } if prefix == "Ideas"
    ));

    let parsed = execute("Do thing @foo^id").expect("caret");
    assert!(matches!(parsed.kind, CaptureKind::TaskWithBlockId { .. }));

    let parsed = execute("Do thing @foo:id").expect("colon");
    assert!(matches!(parsed.kind, CaptureKind::Pomodoro { .. }));

    let error = execute("Do thing @foo::id").expect_err("retired");
    assert_eq!(error, RETIRED_DOUBLE_COLON_ERROR);
}

#[test]
fn project_note_markers_stay_in_their_families() {
    // (token, sub-bullet, task-block-ID, pomodoro)
    let cases: &[(&str, bool, bool, bool)] = &[
        ("@cash^goog-exit+", false, true, false),
        ("@cash^goog-exit+#bugs", false, true, false),
        ("@cash^goog-exit#bugs+", false, true, false),
        ("@cash:goog-exit+", false, false, true),
        ("@cash:goog-exit+#bugs", false, false, true),
        // A `+` inside the Pomodoro name is name charset, not a sigil.
        ("@sase:deep-fix#bugs+", false, false, true),
        ("@cash+goog-exit", true, false, false),
        ("@cash^goog-exit", false, true, false),
        ("@cash:goog-exit", false, false, true),
        ("@cash:goog-exit#bugs", false, false, true),
    ];
    for (token, sub_bullet, task_block_id, pomodoro) in cases {
        assert_eq!(
            is_sub_bullet_marker_candidate(token),
            *sub_bullet,
            "{token}"
        );
        assert_eq!(
            is_task_block_id_marker_candidate(token),
            *task_block_id,
            "{token}"
        );
        assert_eq!(is_pomodoro_marker_candidate(token), *pomodoro, "{token}");
    }
}

#[test]
fn execution_parses_project_note_markers() {
    let parsed = execute("Finish it @cash^goog-exit+").expect("caret plus");
    assert_eq!(parsed.route.as_deref(), Some("cash"));
    assert_eq!(parsed.body, "Finish it");
    assert_eq!(
        parsed.kind,
        CaptureKind::ProjectNote {
            block_id: "goog-exit".to_string(),
            pomodoro_name: None,
        }
    );

    // A named project-note marker parses at the token level; the item
    // level rejects it as unused until task bullets can name IDs.
    let token = parse_task_block_id_route_token("@cash^goog-exit+#bugs")
        .expect("named project note");
    assert_eq!(token.route.as_deref(), Some("cash"));
    assert_eq!(
        token.kind,
        CaptureKind::ProjectNote {
            block_id: "goog-exit".to_string(),
            pomodoro_name: Some("bugs".to_string()),
        }
    );

    // A `+` inside the Pomodoro name is name charset, not a sigil.
    let token = parse_task_block_id_route_token("@cash^goog-exit+#c++")
        .expect("plus in project-note pomodoro name");
    assert_eq!(
        token.kind,
        CaptureKind::ProjectNote {
            block_id: "goog-exit".to_string(),
            pomodoro_name: Some("c++".to_string()),
        }
    );

    // The retired `:` project-note forms fail with a teaching error.
    let error =
        execute("Finish it @cash:goog-exit+").expect_err("retired colon plus");
    assert_eq!(
        error,
        retired_project_note_marker_error(
            "@cash:goog-exit+",
            "cash",
            "goog-exit",
            None
        )
    );
    let error = execute("Finish it @cash:goog-exit+#bugs")
        .expect_err("retired named colon plus");
    assert_eq!(
        error,
        retired_project_note_marker_error(
            "@cash:goog-exit+#bugs",
            "cash",
            "goog-exit",
            Some("bugs")
        )
    );

    // A trailing `+` on a `:` Pomodoro name is not a sigil.
    let parsed = execute("Finish it @sase:deep-fix#bugs+")
        .expect("plus in pomodoro name");
    assert_eq!(
        parsed.kind,
        CaptureKind::Pomodoro {
            block_id: "deep-fix".to_string(),
            pomodoro_name: Some("bugs+".to_string()),
            start: None,
            close: None,
        }
    );
}

#[test]
fn execution_rejects_project_note_shape_errors() {
    // A `#pomodoro` name with no ` :` task is unused.
    let error = execute("Finish it @cash^goog-exit+#bugs")
        .expect_err("unused pomodoro name");
    assert_eq!(error, unused_project_note_pomodoro_error("bugs"));

    // A `+` after `#name` is part of the Pomodoro name: the `+` belongs
    // right after the block ID.
    let error = execute("Finish it @cash^goog-exit#bugs+")
        .expect_err("misordered plus");
    assert_eq!(
        error,
        project_note_misordered_error("cash", "goog-exit", "bugs")
    );

    // A `#name` without the project-note `+` names the fix.
    let error = execute("Finish it @cash^goog-exit#bugs")
        .expect_err("name without plus");
    assert_eq!(
        error,
        project_note_name_without_plus_error("cash", "goog-exit", "bugs")
    );

    // An empty name asks for one.
    let error = execute("Finish it @cash^goog-exit+#")
        .expect_err("empty pomodoro name");
    assert_eq!(error, project_note_name_required_error("cash", "goog-exit"));

    // Session suffixes stay rejected on project notes.
    let error = execute("Finish it @cash^goog-exit+=3")
        .expect_err("start suffix on project note");
    assert_eq!(error, POMODORO_START_PROJECT_NOTE_ERROR);
    let error = execute("Finish it @cash^goog-exit+#bugs=x")
        .expect_err("close suffix on project note");
    assert_eq!(error, POMODORO_CLOSE_PROJECT_NOTE_ERROR);

    let error = execute("Finish it @cash^+").expect_err("empty caret block ID");
    assert_eq!(error, TASK_BLOCK_ID_ERROR);

    let error = execute("Finish it @cash:+").expect_err("empty colon block ID");
    assert_eq!(error, POMODORO_BLOCK_ID_ERROR);

    let error = execute("Finish it @cash:+#bugs")
        .expect_err("empty block ID before pomodoro name");
    assert!(
        error.contains("requires a block ID before the Pomodoro name"),
        "{error}"
    );

    // `!` stays reserved for the explicit sub-bullet toggle: these keep
    // their existing block-ID errors and never become toggles.
    let error =
        execute("Finish it @cash^goog-exit+!").expect_err("caret plus bang");
    assert_eq!(error, TASK_BLOCK_ID_ERROR);
    let error =
        execute("Finish it @cash:goog-exit+!").expect_err("colon plus bang");
    assert_eq!(error, POMODORO_BLOCK_ID_ERROR);
    assert!(exact_explicit_toggle_prefix("@cash^goog-exit+!").is_none());
    assert!(exact_explicit_toggle_prefix("@cash:goog-exit+!").is_none());
}

#[test]
fn global_declaration_rejects_project_note_shapes() {
    for token in [
        "@@cash^goog-exit+",
        "@@cash^goog-exit+#bugs",
        "@@cash:goog-exit+",
    ] {
        let declaration = Token {
            text: token,
            start: 0,
            end: token.len(),
        };
        match classify_global_token(&declaration) {
            TokenParse::Invalid(diagnostic) => {
                assert_eq!(
                    diagnostic.code, "invalid_global_destination",
                    "{token}"
                );
            }
            TokenParse::Marker(_) => panic!("{token} must not parse"),
        }
    }
}

#[test]
fn execution_accepts_project_task_ids_and_strips_them_from_bodies() {
    let parsed = execute(
        "Finish the Google exit packet! @cash^goog-exit+#admin\n\
         - Draft the resignation memo :draft-memo\n\
         \x20 - keep it short\n\
         - Call Morgan Stanley about the 401k :call-ms\n\
         - Collect the equity paperwork ^equity-docs\n\
         - FUTURE WORK\n\
         \x20 - Revisit the severance terms",
    )
    .expect("worked example");
    assert_eq!(parsed.body, "Finish the Google exit packet!");
    assert_eq!(
        parsed.kind,
        CaptureKind::ProjectNote {
            block_id: "goog-exit".to_string(),
            pomodoro_name: Some("admin".to_string()),
        }
    );
    assert_eq!(
        sub_bullet_bodies(&parsed.sub_bullets),
        vec![
            "Draft the resignation memo",
            "keep it short",
            "Call Morgan Stanley about the 401k",
            "Collect the equity paperwork",
            "FUTURE WORK",
            "Revisit the severance terms",
        ]
    );
    assert_eq!(
        sub_bullet_depths(&parsed.sub_bullets),
        vec![1, 2, 1, 1, 1, 2]
    );
    assert_eq!(
        parsed
            .sub_bullets
            .iter()
            .map(|sub| sub.task_id.clone())
            .collect::<Vec<_>>(),
        vec![
            Some(ProjectTaskId {
                block_id: "draft-memo".to_string(),
                link: true,
            }),
            None,
            Some(ProjectTaskId {
                block_id: "call-ms".to_string(),
                link: true,
            }),
            Some(ProjectTaskId {
                block_id: "equity-docs".to_string(),
                link: false,
            }),
            None,
            None,
        ]
    );

    // A project note without a `#pomodoro` name still accepts task IDs.
    let parsed = execute("Finish it @cash^x+\n- Draft the memo :draft-memo")
        .expect("link without pomodoro name");
    assert_eq!(
        parsed.sub_bullets[0].task_id,
        Some(ProjectTaskId {
            block_id: "draft-memo".to_string(),
            link: true,
        })
    );

    // The project-note marker may sit on a child line: the trailing
    // `@route…` marker is set aside before the last word is lexed.
    let parsed =
        execute("Finish it\n- Draft :d @cash^x+").expect("child-line marker");
    assert_eq!(parsed.route.as_deref(), Some("cash"));
    assert_eq!(parsed.body, "Finish it");
    assert_eq!(sub_bullet_bodies(&parsed.sub_bullets), vec!["Draft"]);
    assert_eq!(
        parsed.sub_bullets[0].task_id,
        Some(ProjectTaskId {
            block_id: "d".to_string(),
            link: true,
        })
    );

    // Trailing item-wide markers are set aside before the last word is
    // lexed, so both spellings below name `draft-memo`.
    let parsed = execute("Finish it @cash^x+\n- Draft memo :draft-memo s:2")
        .expect("schedule marker after the ID");
    assert_eq!(parsed.scheduled_offset, Some(2));
    assert_eq!(sub_bullet_bodies(&parsed.sub_bullets), vec!["Draft memo"]);
    assert!(parsed.sub_bullets[0].task_id.is_some());
    let parsed =
        execute("Finish it @cash^x+\n- Draft memo :draft-memo @cash^x+")
            .expect_err("duplicate route marker");
    assert!(parsed.contains("may appear on only one line"), "{parsed}");
}

#[test]
fn execution_rejects_project_task_id_rule_violations_verbatim() {
    // The parent line's own task is always `^prj` and is never linked.
    let error = execute("Finish it :foo @cash^x+").expect_err("parent task ID");
    assert_eq!(error, misplaced_parent_task_id_error(":foo"));

    // Only first-level child bullets can be named.
    let error = execute("Finish it @cash^x+\n- Draft\n  - nested :foo")
        .expect_err("nested task ID");
    assert_eq!(error, misplaced_nested_task_id_error(":foo"));

    // Shape and charset come before reserved, placement, empty body,
    // checkbox, and duplicate checks.
    let error =
        execute("Finish it @cash^x+\n- Draft :a_b").expect_err("charset");
    assert_eq!(error, invalid_project_task_id_charset_error("a_b"));

    // `prj` is reserved in any letter case, even on a nested bullet
    // (reserved precedes placement).
    for token in [":PRJ", "^prj"] {
        let error = execute(&format!("Finish it @cash^x+\n- Draft {token}"))
            .expect_err("reserved");
        assert_eq!(error, reserved_project_task_id_error(&token[1..]));
    }
    let error = execute("Finish it @cash^x+\n- Draft\n  - nested :PRJ")
        .expect_err("reserved on nested");
    assert_eq!(error, reserved_project_task_id_error("PRJ"));

    // An empty remaining body names no task.
    let error = execute("Finish it @cash^x+\n- :foo").expect_err("empty body");
    assert_eq!(error, empty_project_task_body_error(2));

    // A `:` task takes no authored checkbox.
    let error =
        execute("Finish it @cash^x+\n- [x] Foo :foo").expect_err("checkbox");
    assert_eq!(error, checkbox_project_task_id_error("foo", 'x'));

    // A `^` task keeps its authored checkbox.
    let parsed =
        execute("Finish it @cash^x+\n- [x] Foo ^foo").expect("caret checkbox");
    assert_eq!(sub_bullet_bodies(&parsed.sub_bullets), vec!["[x] Foo"]);

    // Duplicate IDs compare exact and case-sensitive and name both draft
    // lines; `:` and `^` share the namespace.
    let error = execute("Finish it @cash^x+\n- One :same\n- Two :same")
        .expect_err("duplicate");
    assert_eq!(error, duplicate_project_task_id_error("same", 2, 3));
    let error = execute("Finish it @cash^x+\n- One :same\n- Two ^same")
        .expect_err("cross-sigil duplicate");
    assert_eq!(error, duplicate_project_task_id_error("same", 2, 3));
    let parsed = execute("Finish it @cash^x+\n- One :Same\n- Two :same")
        .expect("case-sensitive distinct");
    assert!(parsed.sub_bullets[1].task_id.is_some());

    // Checkbox precedes duplicate: the first error is deterministic.
    let error = execute("Finish it @cash^x+\n- [x] Foo :same\n- Bar :same")
        .expect_err("checkbox before duplicate");
    assert_eq!(error, checkbox_project_task_id_error("same", 'x'));

    // A lone sigil ending a first-level bullet is unfinished.
    let error =
        execute("Finish it @cash^x+\n- Draft :").expect_err("lone colon");
    assert_eq!(error, unfinished_project_task_id_error(':'));
    let error =
        execute("Finish it @cash^x+\n- Draft ^").expect_err("lone caret");
    assert_eq!(error, unfinished_project_task_id_error('^'));

    // A lone sigil on the parent or a nested bullet stays literal.
    let parsed = execute("Finish it : @cash^x+").expect("lone parent sigil");
    assert_eq!(parsed.body, "Finish it :");
    let parsed = execute("Finish it @cash^x+\n- Draft\n  - nested :")
        .expect("lone nested sigil");
    assert_eq!(
        sub_bullet_bodies(&parsed.sub_bullets),
        vec!["Draft", "nested :"]
    );

    // A `^`-only project note with a `#pomodoro` name is still unused.
    let error = execute("Finish it @cash^x+#bugs\n- Doc ^docs")
        .expect_err("unused pomodoro");
    assert_eq!(error, unused_project_note_pomodoro_error("bugs"));
}

#[test]
fn execution_evaluates_project_note_markers_parents_and_children_in_order() {
    // Step 1 (marker token) beats step 3 (children).
    let error = execute("Finish it @cash^goog-exit#bugs+\n- :foo")
        .expect_err("marker first");
    assert_eq!(
        error,
        project_note_misordered_error("cash", "goog-exit", "bugs")
    );

    // Step 2 (parent line) beats step 3 (children).
    let error =
        execute("Finish it :foo @cash^x+\n- :bar").expect_err("parent first");
    assert_eq!(error, misplaced_parent_task_id_error(":foo"));

    // Step 3 runs in source order: the first child's error wins.
    let error = execute("Finish it @cash^x+\n- Draft :a_b\n- Other :c_d")
        .expect_err("first child first");
    assert_eq!(error, invalid_project_task_id_charset_error("a_b"));

    // Step 4 (unused `#pomodoro`) runs last.
    let error = execute("Finish it @cash^x+#bugs\n- Draft :a_b")
        .expect_err("child before unused");
    assert_eq!(error, invalid_project_task_id_charset_error("a_b"));
}

#[test]
fn execution_keeps_task_id_lookalikes_literal_outside_project_notes() {
    // ` :1` stays prose in an ordinary task item.
    let parsed =
        execute("Fix @sase\n- ratio 3 :1").expect("non-project literal");
    assert_eq!(parsed.kind, CaptureKind::Task);
    assert_eq!(sub_bullet_bodies(&parsed.sub_bullets), vec!["ratio 3 :1"]);
    assert!(parsed.sub_bullets[0].task_id.is_none());

    // Smileys, times, and mid-body markers never lex, even in a project
    // note; a trailing `:1` still names the task.
    let parsed =
        execute("Finish it @cash^x+\n- ratio 3 :1\n- done 10:30\n- smile :-)")
            .expect("lookalikes");
    assert_eq!(
        sub_bullet_bodies(&parsed.sub_bullets),
        vec!["ratio 3", "done 10:30", "smile :-)"]
    );
    assert_eq!(
        parsed.sub_bullets[0].task_id,
        Some(ProjectTaskId {
            block_id: "1".to_string(),
            link: true,
        })
    );
    assert!(parsed.sub_bullets[1].task_id.is_none());
    assert!(parsed.sub_bullets[2].task_id.is_none());
}

#[test]
fn execution_forced_route_keeps_retired_and_special_markers_literal() {
    let parsed = parse_capture_text_with_clip_control(
        "Do thing @dev::id @dev+parent @dev^new-id",
        Some("work"),
        None,
        true,
    )
    .expect("parse");
    assert_eq!(parsed.route.as_deref(), Some("work"));
    assert_eq!(parsed.kind, CaptureKind::Task);
    assert_eq!(parsed.body, "Do thing @dev::id @dev+parent @dev^new-id");
}

#[test]
fn execution_keeps_equals_wording_off_caret_tokens_without_plus() {
    for raw in ["Do thing @dev^foo=3", "Do thing @dev^foo=x"] {
        let error = execute(raw).expect_err("task-block-ID error");
        assert_eq!(error, TASK_BLOCK_ID_ERROR, "{raw}");
    }
    let error = execute("Finish it @cash^goog-exit+=3")
        .expect_err("start suffix on project note");
    assert_eq!(error, POMODORO_START_PROJECT_NOTE_ERROR);
    let error = execute("Finish it @cash^goog-exit+#bugs=x")
        .expect_err("close suffix on project note");
    assert_eq!(error, POMODORO_CLOSE_PROJECT_NOTE_ERROR);
}

#[test]
fn execution_rejects_checkbox_only_project_task_ids() {
    for raw in [
        "Finish it @cash^x+\n- [x] ^foo",
        "Finish it @cash^x+\n- [ ] ^foo",
        "Finish it @cash^x+\n- [x] :foo",
    ] {
        let error = execute(raw).expect_err("empty body");
        assert_eq!(error, empty_project_task_body_error(2), "{raw}");
    }
}

#[test]
fn execution_rejects_route_less_retired_project_note_markers() {
    for (raw, token, block, name) in [
        ("Finish it @:goog-exit+", "@:goog-exit+", "goog-exit", None),
        (
            "Finish it @:goog-exit+#bugs",
            "@:goog-exit+#bugs",
            "goog-exit",
            Some("bugs"),
        ),
    ] {
        let error = execute(raw).expect_err("retired route-less");
        assert_eq!(
            error,
            retired_project_note_marker_error(token, "", block, name),
            "{raw}"
        );
        let parse = editor(raw);
        let expected =
            retired_project_note_marker_error(token, "", block, name);
        assert_eq!(
            parse
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>(),
            vec![expected],
            "{raw}"
        );
        assert_eq!(codes(&parse), vec!["retired_project_note_marker"], "{raw}");
    }
}

#[test]
fn execution_moves_a_trailing_now_tag_onto_the_body() {
    let parsed = execute("Fix it @sase^fix-it #now").expect("now tag");
    assert_eq!(parsed.body, "Fix it #now");
    assert_eq!(parsed.route.as_deref(), Some("sase"));
    let CaptureKind::TaskWithBlockId { block_id } = &parsed.kind else {
        panic!("expected a task block-ID capture: {:?}", parsed.kind);
    };
    assert_eq!(block_id, "fix-it");

    let parsed = execute("Fix it @sase:fix-it #now").expect("now tag");
    assert_eq!(parsed.body, "Fix it #now");
    assert_eq!(parsed.route.as_deref(), Some("sase"));
    assert!(
        matches!(parsed.kind, CaptureKind::Pomodoro { .. }),
        "{:?}",
        parsed.kind
    );

    // `#now` before the route never leaves the body.
    let parsed = execute("Fix it #now @sase").expect("now tag before route");
    assert_eq!(parsed.body, "Fix it #now");
    assert_eq!(parsed.route.as_deref(), Some("sase"));
    assert_eq!(parsed.kind, CaptureKind::Task);

    // Terminal markers still extract in front of a trailing tag.
    let parsed = execute("Fix it @sase s:3 #now").expect("now tag");
    assert_eq!(parsed.body, "Fix it #now");
    assert_eq!(parsed.route.as_deref(), Some("sase"));
    assert_eq!(parsed.scheduled_offset, Some(3));
}

#[test]
fn execution_rejects_a_now_tag_without_new_task_text() {
    for raw in [
        "@sase:fix-it #now",
        "^sase:fix-it #now",
        "@sase+fix-it #now",
        "@sase+fix-it! #now",
        "=x #now",
        "=3 #now",
        "+5 #now",
        "#now",
    ] {
        let error = execute(raw).expect_err(&format!("{raw} needs text"));
        assert_eq!(error, now_tag_body_error(), "{raw}");
    }
}

#[test]
fn execution_keeps_other_trailing_hash_tags_rejected() {
    // Every other trailing `#tag` keeps today's legacy-marker error, and
    // matching stays case-sensitive and whole-token.
    for raw in [
        "Some note #bar",
        "Some note #bar @foo",
        "Some note @foo #bar",
        "Some note #nowadays",
        "Some note #now/x",
        "Some note #NOW",
    ] {
        let error = execute(raw).expect_err(&format!("{raw} stays rejected"));
        assert!(error.contains("bullet section markers"), "{raw}: {error}");
    }

    // A `#now` mid-body stays literal task text.
    let parsed = execute("Some #now note").expect("middle tag");
    assert_eq!(parsed.body, "Some #now note");
    assert_eq!(parsed.kind, CaptureKind::Task);

    // With a forced route every `@...` token stays literal, so a trailing
    // `#now` stays literal body text there too.
    let parsed = parse_capture_text_with_clip_control(
        "Fix #now",
        Some("work"),
        None,
        true,
    )
    .expect("forced route");
    assert_eq!(parsed.body, "Fix #now");
    assert_eq!(parsed.route.as_deref(), Some("work"));
}
