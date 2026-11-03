//! Capture grammar/routing unit tests.
use super::*;

#[test]
fn normalizes_whitespace() {
    assert_eq!(
        normalize_task_text(" \n buy\t  milk \r\n @groceries  "),
        "buy milk @groceries"
    );
}

#[test]
fn parses_schedule_tokens() {
    assert_eq!(parse_schedule_token("s:0"), Some(0));
    assert_eq!(parse_schedule_token("s:1"), Some(1));
    assert_eq!(parse_schedule_token("s:42"), Some(42));

    for token in [
        "s:",
        "s:abc",
        "s:-1",
        "s:1.5",
        "S:1",
        "sx:1",
        "s:18446744073709551616",
    ] {
        assert_eq!(parse_schedule_token(token), None, "{token}");
    }
}

#[test]
fn parses_priority_tokens() {
    assert_eq!(parse_priority_token("$"), Some(1));
    assert_eq!(parse_priority_token("$1"), Some(1));
    assert_eq!(parse_priority_token("$4"), Some(4));
    assert_eq!(parse_priority_token("$0"), Some(0));
    assert_eq!(parse_priority_token("$12"), Some(12));

    for token in [
        "$$",
        "$1.5",
        "$x",
        "$-1",
        "$1k",
        "p:1",
        "p:",
        "$99999999999999999999",
        "$18446744073709551616",
    ] {
        assert_eq!(parse_priority_token(token), None, "{token}");
    }
}

#[test]
fn parses_retired_priority_tokens() {
    assert_eq!(parse_retired_priority_token("p:1"), Some(1));
    assert_eq!(parse_retired_priority_token("p:12"), Some(12));
    for token in ["p:", "p:abc", "p:-1", "p:1.5", "P:1", "px:1", "$1"] {
        assert_eq!(parse_retired_priority_token(token), None, "{token}");
    }
}

#[test]
fn extracts_priority_markers_from_terminal_region() {
    let mut tokens = vec!["buy", "milk", "$2"];
    assert_eq!(
        extract_terminal_markers(&mut tokens).0.priority_level,
        Some(2)
    );
    assert_eq!(tokens, vec!["buy", "milk"]);

    let mut tokens = vec!["buy", "milk", "$2", "@groceries"];
    assert_eq!(
        extract_terminal_markers(&mut tokens).0.priority_level,
        Some(2)
    );
    assert_eq!(tokens, vec!["buy", "milk", "@groceries"]);

    let mut tokens = vec!["buy", "milk", "@groceries", "$3"];
    assert_eq!(
        extract_terminal_markers(&mut tokens).0.priority_level,
        Some(3)
    );
    assert_eq!(tokens, vec!["buy", "milk", "@groceries"]);

    let mut tokens = vec!["set", "$2", "priority"];
    assert_eq!(extract_terminal_markers(&mut tokens).0.priority_level, None);
    assert_eq!(tokens, vec!["set", "$2", "priority"]);

    let mut tokens = vec!["buy", "$1", "$2"];
    assert_eq!(
        extract_terminal_markers(&mut tokens).0.priority_level,
        Some(2)
    );
    assert_eq!(tokens, vec!["buy", "$1"]);

    let mut tokens = vec!["buy", "milk", "$"];
    assert_eq!(
        extract_terminal_markers(&mut tokens).0.priority_level,
        Some(1)
    );
    assert_eq!(tokens, vec!["buy", "milk"]);
}

#[test]
fn extracts_retired_priority_tokens_from_the_terminal_region() {
    let mut tokens = vec!["buy", "milk", "p:2"];
    let markers = extract_terminal_markers(&mut tokens).0;
    assert_eq!(
        markers
            .retired_priority
            .as_ref()
            .map(|retired| retired.token.as_str()),
        Some("p:2")
    );
    assert!(markers.priority_level.is_none());
    assert_eq!(tokens, vec!["buy", "milk"]);

    let mut tokens = vec!["buy", "milk", "p:2", "@bob"];
    let markers = extract_terminal_markers(&mut tokens).0;
    assert_eq!(
        markers
            .retired_priority
            .as_ref()
            .map(|retired| retired.token.as_str()),
        Some("p:2")
    );
    assert_eq!(tokens, vec!["buy", "milk", "@bob"]);

    let mut tokens = vec!["set", "p:2", "priority"];
    assert!(extract_terminal_markers(&mut tokens)
        .0
        .retired_priority
        .is_none());
    assert_eq!(tokens, vec!["set", "p:2", "priority"]);

    let mut tokens = vec!["buy", "p:x"];
    assert!(extract_terminal_markers(&mut tokens)
        .0
        .retired_priority
        .is_none());
    assert_eq!(tokens, vec!["buy", "p:x"]);
}

#[test]
fn extracts_escaped_priority_tokens_from_the_terminal_region() {
    let mut tokens = tokenize_with_spans("Pay Bob \\$3");
    let markers = extract_terminal_markers(&mut tokens).0;
    assert!(markers.priority_level.is_none());
    assert_eq!(markers.priority_escapes, vec![(8, 9)]);
    assert_eq!(
        tokens.iter().map(|token| token.text).collect::<Vec<_>>(),
        vec!["Pay", "Bob", "\\$3"]
    );

    let mut tokens = tokenize_with_spans("Pay Bob \\$3 $2 @cash");
    let markers = extract_terminal_markers(&mut tokens).0;
    assert_eq!(markers.priority_level, Some(2));
    assert_eq!(markers.priority_escapes, vec![(8, 9)]);
    assert_eq!(
        tokens.iter().map(|token| token.text).collect::<Vec<_>>(),
        vec!["Pay", "Bob", "\\$3", "@cash"]
    );

    let mut tokens = tokenize_with_spans("cost \\$3 today");
    let markers = extract_terminal_markers(&mut tokens).0;
    assert!(markers.priority_level.is_none());
    assert!(markers.priority_escapes.is_empty());
    assert_eq!(
        tokens.iter().map(|token| token.text).collect::<Vec<_>>(),
        vec!["cost", "\\$3", "today"]
    );
}

#[test]
fn extracts_trailing_schedule_from_terminal_region() {
    let mut tokens = vec!["buy", "milk", "s:1"];
    assert_eq!(extract_trailing_schedule(&mut tokens), Some(1));
    assert_eq!(tokens, vec!["buy", "milk"]);

    let mut tokens = vec!["buy", "milk", "s:2", "@groceries"];
    assert_eq!(extract_trailing_schedule(&mut tokens), Some(2));
    assert_eq!(tokens, vec!["buy", "milk", "@groceries"]);

    let mut tokens = vec!["buy", "milk", "@groceries", "s:3"];
    assert_eq!(extract_trailing_schedule(&mut tokens), Some(3));
    assert_eq!(tokens, vec!["buy", "milk", "@groceries"]);

    let mut tokens = vec!["take", "s:1", "pill"];
    assert_eq!(extract_trailing_schedule(&mut tokens), None);
    assert_eq!(tokens, vec!["take", "s:1", "pill"]);

    let mut tokens = vec!["buy", "s:1", "s:2"];
    assert_eq!(extract_trailing_schedule(&mut tokens), Some(2));
    assert_eq!(tokens, vec!["buy", "s:1"]);

    let mut tokens = vec!["buy", "s:abc"];
    assert_eq!(extract_trailing_schedule(&mut tokens), None);
    assert_eq!(tokens, vec!["buy", "s:abc"]);
}

#[test]
fn extracts_clip_and_schedule_markers_from_terminal_region() {
    let cases = [
        ("body %c", "body", None, None, ClipRequest::Current, None),
        (
            "body %c20",
            "body",
            None,
            None,
            ClipRequest::History {
                count: NonZeroUsize::new(20).expect("nonzero"),
            },
            None,
        ),
        (
            "body %c @notes",
            "body",
            Some("notes"),
            None,
            ClipRequest::Current,
            None,
        ),
        (
            "body s:1 %c @groceries",
            "body",
            Some("groceries"),
            Some(1),
            ClipRequest::Current,
            None,
        ),
        (
            "body %c s:1 @groceries",
            "body",
            Some("groceries"),
            Some(1),
            ClipRequest::Current,
            None,
        ),
        (
            "body @groceries s:1 %c",
            "body",
            Some("groceries"),
            Some(1),
            ClipRequest::Current,
            None,
        ),
        (
            "body %c @groceries s:1",
            "body",
            Some("groceries"),
            Some(1),
            ClipRequest::Current,
            None,
        ),
        (
            "body s:1 @groceries %c",
            "body",
            Some("groceries"),
            Some(1),
            ClipRequest::Current,
            None,
        ),
        (
            "@groceries body %c",
            "body",
            Some("groceries"),
            None,
            ClipRequest::Current,
            None,
        ),
        (
            "body %c @dev:blockid",
            "body",
            Some("dev"),
            None,
            ClipRequest::Current,
            None,
        ),
        (
            "body %c @notes#Ideas",
            "body",
            Some("notes"),
            None,
            ClipRequest::Current,
            None,
        ),
        (
            "body %c3 s:2 @groceries",
            "body",
            Some("groceries"),
            Some(2),
            ClipRequest::History {
                count: NonZeroUsize::new(3).expect("nonzero"),
            },
            None,
        ),
        (
            "body @groceries %c3 s:2",
            "body",
            Some("groceries"),
            Some(2),
            ClipRequest::History {
                count: NonZeroUsize::new(3).expect("nonzero"),
            },
            None,
        ),
        (
            "body %c2 @notes#Ideas",
            "body",
            Some("notes"),
            None,
            ClipRequest::History {
                count: NonZeroUsize::new(2).expect("nonzero"),
            },
            None,
        ),
        (
            "body @dev:blockid %c2",
            "body",
            Some("dev"),
            None,
            ClipRequest::History {
                count: NonZeroUsize::new(2).expect("nonzero"),
            },
            None,
        ),
        (
            "body $2 s:1 %c @groceries",
            "body",
            Some("groceries"),
            Some(1),
            ClipRequest::Current,
            Some(2),
        ),
        (
            "body @groceries %c $3 s:4",
            "body",
            Some("groceries"),
            Some(4),
            ClipRequest::Current,
            Some(3),
        ),
        ("body %c1", "body", None, None, ClipRequest::Current, None),
    ];

    for (raw, body, route, scheduled, clip, priority) in cases {
        let parsed = parse_capture_text(raw, None)
            .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
        assert_eq!(parsed.body, body, "{raw}");
        assert_eq!(parsed.route.as_deref(), route, "{raw}");
        assert_eq!(parsed.scheduled_offset, scheduled, "{raw}");
        assert_eq!(parsed.clip, Some(clip), "{raw}");
        assert_eq!(parsed.priority_level, priority, "{raw}");
    }
}

#[test]
fn clip_directives_claim_edges_and_leave_prose_alone() {
    for raw in ["save % now", "body 50%", "body 100%", "https://ex.com/a%25"] {
        let parsed = parse_capture_text(raw, None).expect("literal text");
        assert_eq!(parsed.body, raw, "{raw}");
        assert_eq!(parsed.clip, None, "{raw}");
    }

    let parsed = parse_capture_text("body \\%c", None).expect("escaped clip");
    assert_eq!(parsed.body, "body %c");
    assert_eq!(parsed.clip, None);

    let parsed =
        super::super::parse_capture_text("body %c", Some("work"), None)
            .expect("forced route still extracts marker");
    assert_eq!(parsed.body, "body");
    assert_eq!(parsed.route.as_deref(), Some("work"));
    assert_eq!(parsed.clip, Some(ClipRequest::Current));

    let parsed = super::super::parse_capture_text(
        "body %c",
        Some("notes"),
        Some("Ideas"),
    )
    .expect("forced section still extracts marker");
    assert_eq!(parsed.body, "body");
    assert_eq!(parsed.clip, Some(ClipRequest::Current));
    assert!(matches!(
        parsed.kind,
        CaptureKind::Bullet { exact: true, .. }
    ));

    let error = parse_capture_text("body %c %c", None)
        .expect_err("duplicate clip directives");
    assert!(error.message.contains("directive (%c)"), "{error:?}");

    let error = parse_capture_text("body %", None).expect_err("incomplete");
    assert!(error.message.contains("`%` is incomplete"), "{error:?}");

    for raw in ["body %log", "body %3", "body %C", "body %x"] {
        let error = parse_capture_text(raw, None).expect_err("unknown");
        assert!(
            error.message.contains("is not a capture directive"),
            "{raw}: {error:?}"
        );
    }
    for raw in ["body %c0", "body %c01"] {
        let error = parse_capture_text(raw, None).expect_err("invalid count");
        assert!(error.message.contains("is invalid"), "{raw}: {error:?}");
    }
}

#[test]
fn parses_auto_routes_like_hammerspoon() {
    let cases = [
        (
            "@Groceries Buy Milk",
            "Buy Milk",
            Some("groceries"),
            "prefix route wins and lower-cases",
        ),
        (
            "Buy Milk @Groceries",
            "Buy Milk",
            Some("groceries"),
            "suffix route lower-cases",
        ),
        ("a @b @C", "a @b", Some("c"), "last suffix token wins"),
        (
            "@Work buy milk @home",
            "buy milk @home",
            Some("work"),
            "prefix wins before suffix",
        ),
        (
            "Email @home soon",
            "Email @home soon",
            None,
            "middle @token stays literal",
        ),
        (
            "@bad! body @Good",
            "@bad! body",
            Some("good"),
            "invalid prefix can still use suffix",
        ),
    ];

    for (raw, body, route, label) in cases {
        let parsed = parse_capture_text(raw, None).unwrap_or_else(|error| {
            panic!("{label}: unexpected error: {error:?}")
        });
        assert_eq!(parsed.body, body, "{label}");
        assert_eq!(parsed.route.as_deref(), route, "{label}");
    }

    // A lone header starts a list, so it refuses instead of staying a
    // literal inbox task.
    let error =
        parse_capture_text("@route", None).expect_err("lone header refuses");
    assert!(
        error.message.contains("heads a list but has no bullets"),
        "lone header: {error:?}"
    );
}

#[test]
fn time_tokens_stay_literal_and_leading_route_wins() {
    for raw in ["call dentist @5:30pm", "standup @10:00"] {
        let parsed = parse_capture_text(raw, None).expect("time literal");
        assert_eq!(parsed.body, raw);
        assert_eq!(parsed.route, None);
        assert_eq!(parsed.kind, CaptureKind::Task);
    }
    let parsed =
        parse_capture_text("task @dev:foo", None).expect("valid marker");
    assert_eq!(parsed.route.as_deref(), Some("dev"));
    let parsed = parse_capture_text("@groceries ping @x:", None)
        .expect("leading route wins");
    assert_eq!(parsed.route.as_deref(), Some("groceries"));
    assert_eq!(parsed.body, "ping @x:");
}

#[test]
fn parses_scheduled_offsets_with_routes() {
    let cases = [
        ("Buy Milk s:1", "Buy Milk", None, Some(1)),
        (
            "Buy Milk s:2 @Groceries",
            "Buy Milk",
            Some("groceries"),
            Some(2),
        ),
        (
            "Buy Milk @Groceries s:2",
            "Buy Milk",
            Some("groceries"),
            Some(2),
        ),
        (
            "@Groceries Buy Milk s:3",
            "Buy Milk",
            Some("groceries"),
            Some(3),
        ),
        ("take s:1 pill", "take s:1 pill", None, None),
        ("Buy Milk s:1 s:2", "Buy Milk s:1", None, Some(2)),
        ("Buy Milk s:abc", "Buy Milk s:abc", None, None),
        ("Buy Milk S:1", "Buy Milk S:1", None, None),
    ];

    for (raw, body, route, offset) in cases {
        let parsed = parse_capture_text(raw, None)
            .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
        assert_eq!(parsed.body, body, "{raw}");
        assert_eq!(parsed.route.as_deref(), route, "{raw}");
        assert_eq!(parsed.scheduled_offset, offset, "{raw}");
    }

    let error = parse_capture_text("s:1", None).expect_err("schedule only");
    assert_eq!(error.kind, CaptureErrorKind::Usage);
}

#[test]
fn parses_pomodoro_routes_in_terminal_positions_with_schedules() {
    let cases = [
        ("@Dev:Foo-Bar Do thing", "Do thing", None),
        ("Do thing @Dev:Foo-Bar", "Do thing", None),
        ("Do thing s:2 @Dev:Foo-Bar", "Do thing", Some(2)),
        ("Do thing @Dev:Foo-Bar s:2", "Do thing", Some(2)),
        ("@Dev:Foo-Bar Do thing s:2", "Do thing", Some(2)),
        ("@!Dev:Foo-Bar Do thing", "Do thing", None),
        ("Do thing @!Dev:Foo-Bar", "Do thing", None),
        ("Do thing @!Dev:Foo-Bar s:2", "Do thing", Some(2)),
    ];

    for (raw, body, scheduled_offset) in cases {
        let parsed = parse_capture_text(raw, None)
            .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
        assert_eq!(parsed.body, body, "{raw}");
        assert_eq!(parsed.route.as_deref(), Some("dev"), "{raw}");
        assert_eq!(parsed.scheduled_offset, scheduled_offset, "{raw}");
        assert_eq!(
            parsed.kind,
            CaptureKind::Pomodoro {
                pomodoro_name_is_literal: false,
                block_id: "Foo-Bar".to_string(),
                pomodoro_name: None,
                start: None,
                close: None,
            },
            "{raw}"
        );
    }
}

#[test]
fn parses_named_pomodoro_routes_in_terminal_positions() {
    let cases = [
        ("@Dev:Foo-Bar#bugs Do thing", "Do thing", None, "bugs"),
        ("Do thing @Dev:Foo-Bar#bugs", "Do thing", None, "bugs"),
        (
            "Do thing s:2 @Dev:Foo-Bar#bugs",
            "Do thing",
            Some(2),
            "bugs",
        ),
        (
            "Do thing @Dev:Foo-Bar#bugs s:2",
            "Do thing",
            Some(2),
            "bugs",
        ),
        ("Do thing $2 @Dev:Foo-Bar#bugs", "Do thing", None, "bugs"),
        (
            "Do thing @!Dev:Foo-Bar#after-tui-fix",
            "Do thing",
            None,
            "after-tui-fix",
        ),
        ("Do thing %c @Dev:Foo-Bar#Q&A", "Do thing", None, "Q&A"),
    ];

    for (raw, body, scheduled_offset, name) in cases {
        let parsed = parse_capture_text(raw, None)
            .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
        assert_eq!(parsed.body, body, "{raw}");
        assert_eq!(parsed.route.as_deref(), Some("dev"), "{raw}");
        assert_eq!(parsed.scheduled_offset, scheduled_offset, "{raw}");
        assert_eq!(
            parsed.kind,
            CaptureKind::Pomodoro {
                pomodoro_name_is_literal: false,
                block_id: "Foo-Bar".to_string(),
                pomodoro_name: Some(name.to_string()),
                start: None,
                close: None,
            },
            "{raw}"
        );
    }

    let parsed = parse_capture_text("body @foo#sec:x", None)
        .expect("hash before colon stays a bullet");
    assert!(matches!(parsed.kind, CaptureKind::Bullet { .. }));
    let parsed = parse_capture_text("body @foo+id#sec", None)
        .expect("plus family keeps the section");
    assert!(matches!(parsed.kind, CaptureKind::SubBullet { .. }));
}

#[test]
fn malformed_named_pomodoro_markers_are_usage_errors() {
    for (raw, expected) in [
        (
            "body @dev:#bugs",
            "requires a block ID before the Pomodoro name",
        ),
        ("body @dev:id#", "requires a Pomodoro name"),
        ("body @dev:id#bad_id", "name must contain"),
    ] {
        let error = parse_capture_text(raw, None)
            .expect_err(&format!("{raw} should fail"));
        assert_eq!(error.kind, CaptureErrorKind::Usage, "{raw}");
        assert!(error.message.contains(expected), "{raw}: {error:?}");
    }
}

#[test]
fn parses_task_block_id_routes_in_terminal_positions_with_schedules() {
    let cases = [
        ("@Dev^Foo-Bar Do thing", "Do thing", None),
        ("Do thing @Dev^Foo-Bar", "Do thing", None),
        ("Do thing s:2 @Dev^Foo-Bar", "Do thing", Some(2)),
        ("Do thing @Dev^Foo-Bar s:2", "Do thing", Some(2)),
        ("@Dev^Foo-Bar Do thing s:2", "Do thing", Some(2)),
    ];

    for (raw, body, scheduled_offset) in cases {
        let parsed = parse_capture_text(raw, None)
            .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
        assert_eq!(parsed.body, body, "{raw}");
        assert_eq!(parsed.route.as_deref(), Some("dev"), "{raw}");
        assert_eq!(parsed.scheduled_offset, scheduled_offset, "{raw}");
        assert_eq!(
            parsed.kind,
            CaptureKind::TaskWithBlockId {
                block_id: "Foo-Bar".to_string(),
            },
            "{raw}"
        );
    }
}

#[test]
fn parses_sub_bullet_routes_with_precedence_and_terminal_markers() {
    let cases = [
        ("@Cash+Goog-Exit Called today", "Called today", None, false),
        ("Called today @Cash+Goog-Exit", "Called today", None, false),
        (
            "Called today s:1 @Cash+Goog-Exit",
            "Called today",
            Some(1),
            false,
        ),
        (
            "Called today @Cash+Goog-Exit s:1",
            "Called today",
            Some(1),
            false,
        ),
        (
            "Called today %c @Cash+Goog-Exit",
            "Called today",
            None,
            true,
        ),
        (
            "Called today @Cash+Goog-Exit %c",
            "Called today",
            None,
            true,
        ),
    ];

    for (raw, body, scheduled_offset, has_clip) in cases {
        let parsed = parse_capture_text(raw, None)
            .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
        assert_eq!(parsed.body, body, "{raw}");
        assert_eq!(parsed.route.as_deref(), Some("cash"), "{raw}");
        assert_eq!(parsed.scheduled_offset, scheduled_offset, "{raw}");
        assert_eq!(
            parsed.kind,
            CaptureKind::SubBullet {
                target: SubBulletTarget::BlockId("Goog-Exit".to_string()),
                section: None,
            },
            "{raw}"
        );
        assert_eq!(
            parsed.clip,
            has_clip.then_some(ClipRequest::Current),
            "{raw}"
        );
    }

    let error = parse_capture_text("body @foo+bad:id", None)
        .expect_err("colon stays inside the plus family");
    assert!(
        error.message.contains("sub-bullet"),
        "body @foo+bad:id: {error:?}"
    );
    let parsed = parse_capture_text("body @foo+bad#section", None)
        .expect("hash after plus is a drawer selector");
    assert_eq!(
        parsed.kind,
        CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId("bad".to_string()),
            section: Some(capture_language::DrawerSelector {
                text: "section".to_string(),
                exact: false,
            }),
        }
    );
    let error = parse_capture_text("body @foo^bad:id", None)
        .expect_err("colon after caret must take precedence");
    assert!(
        error.message.contains("task block-ID")
            || error.message.contains("project-note `+`"),
        "{error:?}"
    );
    let typed = parse_capture_text("body @foo^bad #section", None)
        .expect("hash after a caret task ID is a task-type marker");
    assert!(matches!(typed.kind, CaptureKind::TaskWithBlockId { .. }));
    assert_eq!(
        typed
            .type_marker
            .as_ref()
            .map(|marker| marker.name.as_str()),
        Some("section")
    );
    let error = parse_capture_text("body @foo::id", None)
        .expect_err("retired double colon is not ID-only or Pomodoro");
    assert!(
        error
            .message
            .contains("'@<route>::<block-id>' is no longer accepted"),
        "{error:?}"
    );
    let parsed = parse_capture_text("body @foo^id", None)
        .expect("caret is ordinary task-with-ID");
    assert!(matches!(parsed.kind, CaptureKind::TaskWithBlockId { .. }));
    let parsed = parse_capture_text("body @foo:id", None)
        .expect("colon remains Pomodoro");
    assert!(matches!(parsed.kind, CaptureKind::Pomodoro { .. }));
    let parsed = parse_capture_text("body @foo#section", None)
        .expect("hash remains bullet");
    assert!(matches!(parsed.kind, CaptureKind::Bullet { .. }));
}

#[test]
fn malformed_sub_bullet_markers_are_usage_errors() {
    for (raw, expected) in [
        ("body @cash+", "requires a block ID"),
        ("body @+id", "must use @<route>+<block-id>"),
        ("body @+id#req", "must use @<route>+<block-id>"),
        ("body @bad.route+id", "route must contain"),
        ("body @cash+bad.id", "block ID must be"),
        ("body @cash+bad:id", "block ID must be"),
        ("body @cash+#req", "requires a block ID before the drawer"),
        ("body @cash+id#", "incomplete"),
        ("body @cash+id#bad_id", "invalid drawer"),
        ("body @cash+id#req^x", "invalid drawer"),
        ("body @cash+id#req+x", "invalid drawer"),
    ] {
        let error = parse_capture_text(raw, None)
            .expect_err(&format!("{raw} should fail"));
        assert_eq!(error.kind, CaptureErrorKind::Usage, "{raw}");
        assert!(error.message.contains(expected), "{raw}: {error:?}");
    }

    let parsed = parse_capture_text("Discuss @cash+id later", None)
        .expect("mid-text marker remains literal");
    assert_eq!(parsed.body, "Discuss @cash+id later");
    assert_eq!(parsed.kind, CaptureKind::Task);
}

/// A bare `@route+block-id[#<type>]` marker with no other text is a task
/// toggle operation, not a "task text is required" error. First-level child
/// bullets switch the item to the multi-note form instead.
#[test]
fn bare_sub_bullet_markers_toggle_instead_of_erroring() {
    let parsed = parse_capture_text("@cash+id", None)
        .expect("bare block-ID marker toggles");
    assert_eq!(parsed.body, "");
    assert_eq!(parsed.route.as_deref(), Some("cash"));
    assert_eq!(
        parsed.kind,
        CaptureKind::TaskToggle {
            block_id: "id".to_string(),
            intent: TaskToggleIntent::EnsureNext,
        }
    );

    let typed = parse_capture_text("@cash+id #fix", None)
        .expect("bare marker with a task type ensures Next");
    assert_eq!(
        typed.kind,
        CaptureKind::TaskToggle {
            block_id: "id".to_string(),
            intent: TaskToggleIntent::EnsureNext,
        }
    );
    assert_eq!(
        typed
            .type_marker
            .as_ref()
            .map(|marker| marker.name.as_str()),
        Some("fix")
    );

    let error = parse_capture_text("@cash+id s:2", None)
        .expect_err("@cash+id s:2 should still need text");
    assert!(
        error.message.contains("task text is required"),
        "@cash+id s:2: {error:?}"
    );

    let multi = parse_capture_text("@cash+id\n- child text", None)
        .expect("note-less marker with bullets is multi-note");
    assert_eq!(multi.body, "");
    assert_eq!(multi.route.as_deref(), Some("cash"));
    assert_eq!(
        multi.kind,
        CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId("id".to_string()),
            section: None,
        }
    );
    assert_eq!(multi.sub_bullets.len(), 1);
    assert_eq!(multi.sub_bullets[0].body, "child text");
    assert!(multi.sub_bullets[0].task_id.is_none());
    assert!(multi.sub_bullets[0].drawer.is_none());

    let explicit = parse_capture_text("@cash+id!", None)
        .expect("bare explicit-toggle marker");
    assert_eq!(explicit.body, "");
    assert_eq!(
        explicit.kind,
        CaptureKind::TaskToggle {
            block_id: "id".to_string(),
            intent: TaskToggleIntent::Toggle,
        }
    );
}

#[test]
fn clipboard_notes_parse_as_sub_bullet_without_text() {
    for raw in ["@cash+id %c", "%c @cash+id", "@cash+id %c3"] {
        let parsed = parse_capture_text(raw, None).unwrap_or_else(|error| {
            panic!("{raw} should parse as clipboard notes: {error:?}")
        });
        assert_eq!(parsed.body, "", "{raw}");
        assert_eq!(parsed.route.as_deref(), Some("cash"), "{raw}");
        assert_eq!(
            parsed.kind,
            CaptureKind::SubBullet {
                target: SubBulletTarget::BlockId("id".to_string()),
                section: None,
            },
            "{raw}"
        );
        assert!(parsed.sub_bullets.is_empty(), "{raw}");
        if raw.contains("%c3") {
            assert_eq!(
                parsed.clip,
                Some(ClipRequest::History {
                    count: NonZeroUsize::new(3).expect("nonzero"),
                }),
                "{raw}"
            );
        } else {
            assert_eq!(parsed.clip, Some(ClipRequest::Current), "{raw}");
        }
    }

    let drawer = parse_capture_text("@cash+id#requirements %c", None)
        .expect("drawer clipboard notes parse");
    assert_eq!(drawer.body, "");
    assert_eq!(
        drawer.kind,
        CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId("id".to_string()),
            section: Some(DrawerSelector {
                text: "requirements".to_string(),
                exact: false,
            }),
        },
        "drawer clipboard notes kind"
    );
    assert_eq!(drawer.clip, Some(ClipRequest::Current));

    for raw in ["@cash+id %c s:2", "@cash+id %c $2"] {
        let error = parse_capture_text(raw, None)
            .expect_err(&format!("{raw} should refuse single-note markers"));
        assert_eq!(error.kind, CaptureErrorKind::Usage, "{raw}");
        assert!(
            error.message.contains("decorates a single note"),
            "{raw}: {error:?}"
        );
    }

    // Unchanged refusals.
    let parsed = parse_capture_text("@cash+id", None)
        .expect("bare marker still toggles");
    assert!(matches!(
        parsed.kind,
        CaptureKind::TaskToggle {
            intent: TaskToggleIntent::EnsureNext,
            ..
        }
    ));
    let error = parse_capture_text("@cash+id s:2", None)
        .expect_err("@cash+id s:2 should still need text");
    assert!(
        error.message.contains("task text is required"),
        "@cash+id s:2: {error:?}"
    );
    let error = parse_capture_text("@cash+id! %c", None)
        .expect_err("@cash+id! %c should refuse explicit toggle");
    assert!(
        error.message.contains("explicit toggle"),
        "@cash+id! %c: {error:?}"
    );
    let error = parse_capture_text("@cash+id %c\n- a", None)
        .expect_err("bullets plus %c should refuse");
    assert!(
        error.message.contains("decorates a single note"),
        "bullets plus %c: {error:?}"
    );
}

#[test]
fn malformed_task_block_id_markers_are_usage_errors() {
    for (raw, expected) in [
        ("body @cash^", "block ID must be"),
        ("body @^id", "route must contain"),
        ("body @bad.route^id", "route must contain"),
        ("body @cash^bad.id", "block ID must be"),
        ("@cash^id", "task text is required"),
    ] {
        let error = parse_capture_text(raw, None)
            .expect_err(&format!("{raw} should fail"));
        assert_eq!(error.kind, CaptureErrorKind::Usage, "{raw}");
        assert!(error.message.contains(expected), "{raw}: {error:?}");
    }

    let parsed = parse_capture_text("Discuss @cash^id later", None)
        .expect("mid-text caret marker remains literal");
    assert_eq!(parsed.body, "Discuss @cash^id later");
    assert_eq!(parsed.kind, CaptureKind::Task);
}

#[test]
fn retired_double_colon_markers_are_usage_errors() {
    for raw in [
        "body @cash::id",
        "body @cash::",
        "body @::id",
        "@cash::id body",
    ] {
        let error = parse_capture_text(raw, None)
            .expect_err(&format!("{raw} should fail"));
        assert_eq!(error.kind, CaptureErrorKind::Usage, "{raw}");
        assert!(
            error
                .message
                .contains("'@<route>::<block-id>' is no longer accepted"),
            "{raw}: {error:?}"
        );
    }

    let parsed = parse_capture_text("Discuss @cash::id later", None)
        .expect("mid-text retired marker remains literal");
    assert_eq!(parsed.body, "Discuss @cash::id later");
    assert_eq!(parsed.kind, CaptureKind::Task);
}

#[test]
fn parses_picker_task_refs_strictly() {
    assert_eq!(
        parse_task_ref("24:1f3a9c2b").expect("valid ref"),
        SubBulletTarget::Ref {
            line: 24,
            digest: "1f3a9c2b".to_string()
        }
    );
    for value in ["", "0:1f3a9c2b", "24:ABCDEF12", "24:abc", "x:1f3a9c2b"] {
        let error = parse_task_ref(value).expect_err("invalid ref");
        assert_eq!(error.message, "--task-ref must use <line>:<digest>");
    }
}

#[test]
fn malformed_terminal_pomodoro_routes_are_usage_errors() {
    for raw in [
        "Do thing @dev:",
        "Do thing @dev:bad.id",
        "Do thing @bad/route:id",
        "Do thing @dev:id:extra",
        "Do thing @!",
        "Do thing @!dev",
        "Do thing @!:id",
        "Do thing @!dev:",
        "Do thing @!dev:bad.id",
        "Do thing @!bad/route:id",
        "Do thing @!dev:id:extra",
        "@!dev Do thing",
    ] {
        let error = parse_capture_text(raw, None)
            .expect_err(&format!("{raw} should fail"));
        assert_eq!(error.kind, CaptureErrorKind::Usage, "{raw}");
    }
}

#[test]
fn pomodoro_route_requires_a_body_and_stays_literal_in_middle_or_forced() {
    let parsed = parse_capture_text("@dev:id", None)
        .expect("solo link is a Pomodoro link");
    assert_eq!(parsed.body, "");
    assert_eq!(parsed.route.as_deref(), Some("dev"));
    assert!(matches!(parsed.kind, CaptureKind::PomodoroLink { .. }));

    let parsed = parse_capture_text("Discuss @dev:id later", None)
        .expect("middle marker stays literal");
    assert_eq!(parsed.body, "Discuss @dev:id later");
    assert_eq!(parsed.kind, CaptureKind::Task);

    let parsed = parse_capture_text("Do thing @dev:id", Some("Work"))
        .expect("forced route keeps marker literal");
    assert_eq!(parsed.body, "Do thing @dev:id");
    assert_eq!(parsed.route.as_deref(), Some("work"));
    assert_eq!(parsed.kind, CaptureKind::Task);

    let parsed = parse_capture_text("Jot @notes#time:box", None)
        .expect("a colon in a bullet prefix is not a Pomodoro marker");
    assert_eq!(parsed.body, "Jot");
    assert_eq!(parsed.route.as_deref(), Some("notes"));
    assert_eq!(
        parsed.kind,
        CaptureKind::Bullet {
            section_prefix: Some("time:box".to_string()),
            exact: false,
        }
    );
}

#[test]
fn forced_route_bypasses_auto_route_parsing() {
    let parsed = parse_capture_text("Buy milk @Groceries", Some("Work-Queue"))
        .expect("parse forced route");
    assert_eq!(parsed.body, "Buy milk @Groceries");
    assert_eq!(parsed.route.as_deref(), Some("work-queue"));
    assert_eq!(parsed.scheduled_offset, None);

    let parsed =
        parse_capture_text("Buy milk s:2 @Groceries", Some("Work-Queue"))
            .expect("parse forced route with schedule");
    assert_eq!(parsed.body, "Buy milk @Groceries");
    assert_eq!(parsed.route.as_deref(), Some("work-queue"));
    assert_eq!(parsed.scheduled_offset, Some(2));

    let error = parse_capture_text("Buy milk", Some("../bad"))
        .expect_err("invalid forced route must fail");
    assert_eq!(error.kind, CaptureErrorKind::Usage);
}
