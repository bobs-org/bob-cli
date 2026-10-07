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
    assert_eq!(parse_priority_token("p:1"), Some(1));
    assert_eq!(parse_priority_token("p:4"), Some(4));
    assert_eq!(parse_priority_token("p:12"), Some(12));

    for token in [
        "p:",
        "p:abc",
        "p:-1",
        "p:1.5",
        "P:1",
        "px:1",
        "p:18446744073709551616",
    ] {
        assert_eq!(parse_priority_token(token), None, "{token}");
    }
}

#[test]
fn extracts_priority_markers_from_terminal_region() {
    let mut tokens = vec!["buy", "milk", "p:2"];
    assert_eq!(
        extract_terminal_markers(&mut tokens, false)
            .0
            .priority_level,
        Some(2)
    );
    assert_eq!(tokens, vec!["buy", "milk"]);

    let mut tokens = vec!["buy", "milk", "p:2", "@groceries"];
    assert_eq!(
        extract_terminal_markers(&mut tokens, false)
            .0
            .priority_level,
        Some(2)
    );
    assert_eq!(tokens, vec!["buy", "milk", "@groceries"]);

    let mut tokens = vec!["buy", "milk", "@groceries", "p:3"];
    assert_eq!(
        extract_terminal_markers(&mut tokens, false)
            .0
            .priority_level,
        Some(3)
    );
    assert_eq!(tokens, vec!["buy", "milk", "@groceries"]);

    let mut tokens = vec!["set", "p:2", "priority"];
    assert_eq!(
        extract_terminal_markers(&mut tokens, false)
            .0
            .priority_level,
        None
    );
    assert_eq!(tokens, vec!["set", "p:2", "priority"]);

    let mut tokens = vec!["buy", "p:1", "p:2"];
    assert_eq!(
        extract_terminal_markers(&mut tokens, false)
            .0
            .priority_level,
        Some(2)
    );
    assert_eq!(tokens, vec!["buy", "p:1"]);
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
        (
            "body %",
            "body",
            None,
            None,
            ClipRequest::Current { header: None },
            None,
        ),
        (
            "body %20",
            "body",
            None,
            None,
            ClipRequest::History {
                count: NonZeroUsize::new(20).expect("nonzero"),
            },
            None,
        ),
        (
            "body %log @notes",
            "body",
            Some("notes"),
            None,
            ClipRequest::Current {
                header: Some("log".to_string()),
            },
            None,
        ),
        (
            "body s:1 % @groceries",
            "body",
            Some("groceries"),
            Some(1),
            ClipRequest::Current { header: None },
            None,
        ),
        (
            "body % s:1 @groceries",
            "body",
            Some("groceries"),
            Some(1),
            ClipRequest::Current { header: None },
            None,
        ),
        (
            "body @groceries s:1 %log",
            "body",
            Some("groceries"),
            Some(1),
            ClipRequest::Current {
                header: Some("log".to_string()),
            },
            None,
        ),
        (
            "body %log @groceries s:1",
            "body",
            Some("groceries"),
            Some(1),
            ClipRequest::Current {
                header: Some("log".to_string()),
            },
            None,
        ),
        (
            "body s:1 @groceries %log",
            "body",
            Some("groceries"),
            Some(1),
            ClipRequest::Current {
                header: Some("log".to_string()),
            },
            None,
        ),
        (
            "@groceries body %foo_bar",
            "body",
            Some("groceries"),
            None,
            ClipRequest::Current {
                header: Some("foo_bar".to_string()),
            },
            None,
        ),
        (
            "body %log @dev:blockid",
            "body",
            Some("dev"),
            None,
            ClipRequest::Current {
                header: Some("log".to_string()),
            },
            None,
        ),
        (
            "body %log @notes#Ideas",
            "body",
            Some("notes"),
            None,
            ClipRequest::Current {
                header: Some("log".to_string()),
            },
            None,
        ),
        (
            "body %3 s:2 @groceries",
            "body",
            Some("groceries"),
            Some(2),
            ClipRequest::History {
                count: NonZeroUsize::new(3).expect("nonzero"),
            },
            None,
        ),
        (
            "body @groceries %3 s:2",
            "body",
            Some("groceries"),
            Some(2),
            ClipRequest::History {
                count: NonZeroUsize::new(3).expect("nonzero"),
            },
            None,
        ),
        (
            "body %2 @notes#Ideas",
            "body",
            Some("notes"),
            None,
            ClipRequest::History {
                count: NonZeroUsize::new(2).expect("nonzero"),
            },
            None,
        ),
        (
            "body @dev:blockid %2",
            "body",
            Some("dev"),
            None,
            ClipRequest::History {
                count: NonZeroUsize::new(2).expect("nonzero"),
            },
            None,
        ),
        (
            "body p:2 s:1 % @groceries",
            "body",
            Some("groceries"),
            Some(1),
            ClipRequest::Current { header: None },
            Some(2),
        ),
        (
            "body @groceries %log p:3 s:4",
            "body",
            Some("groceries"),
            Some(4),
            ClipRequest::Current {
                header: Some("log".to_string()),
            },
            Some(3),
        ),
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
fn clip_markers_are_terminal_forgiving_and_can_be_disabled() {
    for raw in ["save % now", "body %bad!", "body 50%", "body 100%"] {
        let parsed = parse_capture_text(raw, None).expect("literal text");
        assert_eq!(parsed.body, raw, "{raw}");
        assert_eq!(parsed.clip, None, "{raw}");
    }

    let parsed = super::parse_capture_text_with_clip_control(
        "body %log",
        None,
        None,
        &CaptureParseOptions::routing_off(false),
    )
    .expect("disabled clip marker");
    assert_eq!(parsed.body, "body %log");
    assert_eq!(parsed.clip, None);

    let parsed =
        super::super::parse_capture_text("body %log", Some("work"), None)
            .expect("forced route still extracts marker");
    assert_eq!(parsed.body, "body");
    assert_eq!(parsed.route.as_deref(), Some("work"));
    assert_eq!(
        parsed.clip,
        Some(ClipRequest::Current {
            header: Some("log".to_string())
        })
    );

    let parsed = super::super::parse_capture_text(
        "body %section_clip",
        Some("notes"),
        Some("Ideas"),
    )
    .expect("forced section still extracts marker");
    assert_eq!(parsed.body, "body");
    assert_eq!(
        parsed.clip,
        Some(ClipRequest::Current {
            header: Some("section_clip".to_string())
        })
    );
    assert!(matches!(
        parsed.kind,
        CaptureKind::Bullet { exact: true, .. }
    ));

    let parsed = parse_capture_text("body %first %second", None)
        .expect("one marker extracted");
    assert_eq!(parsed.body, "body %first");
    assert_eq!(
        parsed.clip,
        Some(ClipRequest::Current {
            header: Some("second".to_string())
        })
    );

    let parsed = parse_capture_text("body %2 %3", None)
        .expect("one numeric marker extracted");
    assert_eq!(parsed.body, "body %2");
    assert_eq!(
        parsed.clip,
        Some(ClipRequest::History {
            count: NonZeroUsize::new(3).expect("nonzero")
        })
    );

    for raw in ["body %0", "body %184467440737095516160"] {
        let parsed = parse_capture_text(raw, None).expect("literal numeric");
        assert_eq!(parsed.body, raw, "{raw}");
        assert_eq!(parsed.clip, None, "{raw}");
    }
    for (raw, count) in [("body %1", 1), ("body %01", 1), ("body %3", 3)] {
        let parsed = parse_capture_text(raw, None).expect("history marker");
        assert_eq!(parsed.body, "body", "{raw}");
        assert_eq!(
            parsed.clip,
            Some(ClipRequest::History {
                count: NonZeroUsize::new(count).expect("nonzero")
            }),
            "{raw}"
        );
    }

    let error = parse_capture_text("%", None)
        .expect_err("marker-only capture has no parent text");
    assert_eq!(error.kind, CaptureErrorKind::Usage);
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
        ("@route", "@route", None, "bare route stays literal"),
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
        ("Do thing p:2 @Dev:Foo-Bar#bugs", "Do thing", None, "bugs"),
        (
            "Do thing @!Dev:Foo-Bar#after-tui-fix",
            "Do thing",
            None,
            "after-tui-fix",
        ),
        ("Do thing %log @Dev:Foo-Bar#Q&A", "Do thing", None, "Q&A"),
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
        ("@Cash+Goog-Exit Called today", "Called today", None, None),
        ("Called today @Cash+Goog-Exit", "Called today", None, None),
        (
            "Called today s:1 @Cash+Goog-Exit",
            "Called today",
            Some(1),
            None,
        ),
        (
            "Called today @Cash+Goog-Exit s:1",
            "Called today",
            Some(1),
            None,
        ),
        (
            "Called today %log @Cash+Goog-Exit",
            "Called today",
            None,
            Some("log"),
        ),
        (
            "Called today @Cash+Goog-Exit %log",
            "Called today",
            None,
            Some("log"),
        ),
    ];

    for (raw, body, scheduled_offset, clip_header) in cases {
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
            clip_header.map(|header| ClipRequest::Current {
                header: Some(header.to_string())
            }),
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
        .expect("hash after plus is a task-section selector");
    assert_eq!(
        parsed.kind,
        CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId("bad".to_string()),
            section: Some(capture_language::TaskSectionSelector {
                text: "section".to_string(),
                exact: false,
            }),
        }
    );
    for raw in ["body @foo^bad:id", "body @foo^bad#section"] {
        let error = parse_capture_text(raw, None)
            .expect_err("caret must take precedence");
        assert!(
            error.message.contains("task block-ID")
                || error.message.contains("project-note `+`"),
            "{raw}: {error:?}"
        );
    }
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
        (
            "body @cash+#req",
            "requires a block ID before the task section",
        ),
        ("body @cash+id#", "requires a task section"),
        ("body @cash+id#bad_id", "section must contain"),
        ("body @cash+id#req^x", "section must contain"),
        ("body @cash+id#req+x", "section must contain"),
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

/// A bare `@route+block-id[#name]` marker with no other text is a task
/// toggle operation, not a "task text is required" error.
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
            pomodoro_name: None,
            intent: TaskToggleIntent::EnsureNext,
        }
    );

    let named = parse_capture_text("@cash+id#deep+work", None)
        .expect("bare marker with a Pomodoro name ensures Next");
    assert_eq!(
        named.kind,
        CaptureKind::TaskToggle {
            block_id: "id".to_string(),
            pomodoro_name: Some("deep+work".to_string()),
            intent: TaskToggleIntent::EnsureNext,
        }
    );

    for raw in ["@cash+id s:2", "@cash+id\n- child text"] {
        let error = parse_capture_text(raw, None)
            .expect_err(&format!("{raw} should still need text"));
        assert!(
            error.message.contains("task text is required"),
            "{raw}: {error:?}"
        );
    }

    let explicit = parse_capture_text("@cash+id!", None)
        .expect("bare explicit-toggle marker");
    assert_eq!(explicit.body, "");
    assert_eq!(
        explicit.kind,
        CaptureKind::TaskToggle {
            block_id: "id".to_string(),
            pomodoro_name: None,
            intent: TaskToggleIntent::Toggle,
        }
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
