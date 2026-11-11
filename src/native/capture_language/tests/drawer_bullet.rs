//! First-level `#` drawer shortcuts: lexer, execution, and capture-parse.

use super::super::drawer_bullet::{
    scan_first_level_line, scan_leading, DrawerBulletHit,
};
use super::super::type_marker::{completion_at_raw, scan_line};
use super::*;

#[test]
fn leading_hash_on_a_first_level_bullet_is_a_drawer_shortcut() {
    let hit = scan_first_level_line("- #future-work", 0)
        .expect("first-level shortcut");
    let DrawerBulletHit::Complete {
        title,
        key,
        leftover,
        selector,
        ..
    } = hit
    else {
        panic!("expected a complete shortcut: {hit:?}");
    };
    assert_eq!(title, "FUTURE WORK");
    assert_eq!(key, "FUTURE WORK");
    assert!(leftover.is_empty());
    assert!(!selector.exact);
    assert_eq!(selector.text, "future-work");
}

#[test]
fn quoted_literal_digit_heading_and_escaped_hashes() {
    let quoted = scan_leading("#\"NON-GOALS\"", 0).expect("quoted");
    let DrawerBulletHit::Complete {
        title, selector, ..
    } = quoted
    else {
        panic!("{quoted:?}");
    };
    assert_eq!(title, "NON-GOALS");
    assert!(selector.exact);

    assert!(scan_leading("#1", 0).is_none());
    assert!(scan_leading("##heading", 0).is_none());
    assert!(scan_leading("#-dash", 0).is_none());
    assert!(matches!(
        scan_leading("\\#idea", 0),
        Some(DrawerBulletHit::Escaped { .. })
    ));
    assert!(matches!(
        scan_leading("#", 0),
        Some(DrawerBulletHit::Incomplete { .. })
    ));
}

#[test]
fn leftover_text_on_a_shortcut_is_drawer_bullet_text() {
    let hit = scan_leading("#requirements Postgres 17", 0).expect("leftover");
    let DrawerBulletHit::Complete { leftover, .. } = hit else {
        panic!("{hit:?}");
    };
    assert_eq!(leftover, "Postgres 17");
    let error = execute("Ship it\n- #requirements Postgres 17").unwrap_err();
    assert!(
        error.contains("names a drawer, so it takes no text"),
        "{error}"
    );
}

#[test]
fn childless_shortcut_and_escaped_literal_hash() {
    let error = execute("Ship it\n- #future-work").unwrap_err();
    assert!(error.contains("has no notes beneath it"), "{error}");
    let parsed = execute("Ship it\n- \\#idea").expect("escaped stays prose");
    assert_eq!(sub_bullet_bodies(&parsed.sub_bullets), vec!["#idea"]);
    assert!(parsed.sub_bullets[0].drawer.is_none());
}

#[test]
fn nested_hash_still_types_and_first_level_does_not() {
    assert!(scan_line("- #future-work", 0).is_empty());
    let nested = scan_line("  - #fix", 0);
    assert!(
        matches!(nested.as_slice(), [TypeMarkerOccurrence::Complete(_)]),
        "{nested:?}"
    );
    assert!(
        completion_at_raw("Ship it\n- #fu", "Ship it\n- #fu".len()).is_none()
    );
    let nested_type = "Ship it\n- check\n  - #fi";
    assert!(completion_at_raw(nested_type, nested_type.len()).is_some());
}

#[test]
fn editor_reports_drawer_needs_and_all_caps_spans() {
    let incomplete = editor("Ship it\n- #");
    assert_eq!(incomplete.mode, EditorMode::Incomplete);
    assert!(incomplete.needs.contains(&Need::Drawer), "{incomplete:?}");
    assert!(span_kinds(&incomplete).contains(&SpanKind::InteractivePlaceholder));

    let notes = editor("Ship it\n- #future-work");
    assert_eq!(notes.mode, EditorMode::Incomplete);
    assert!(notes.needs.contains(&Need::DrawerNotes), "{notes:?}");
    assert!(notes.diagnostics.is_empty(), "{notes:?}");
    assert!(span_kinds(&notes).contains(&SpanKind::Drawer));

    let caps = editor("Ship it\n- FUTURE WORK\n  - try replication");
    assert!(
        span_kinds(&caps).contains(&SpanKind::DrawerBullet),
        "{:?}",
        span_kinds(&caps)
    );
    assert_eq!(caps.sub_bullets[0].body, "FUTURE WORK");
    assert!(caps.sub_bullets[0].drawer.as_ref().is_some_and(|drawer| {
        drawer.exact && drawer.text == "FUTURE WORK"
    }));

    let childless_caps = editor("Plan trip\n- NYC");
    assert!(!span_kinds(&childless_caps).contains(&SpanKind::DrawerBullet));
    assert!(childless_caps.sub_bullets[0].drawer.is_none());
}

/// The reserved-name diagnostics of one parse, as `(code, text)` pairs
/// where `text` is the exact byte range the diagnostic points at.
fn reserved_ranges(raw: &str) -> Vec<(&'static str, String)> {
    let parse = editor(raw);
    parse
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "reserved_drawer")
        .map(|diagnostic| {
            let (start, end) = diagnostic.range.expect("ranged diagnostic");
            (diagnostic.code, raw[start..end].to_string())
        })
        .collect()
}

#[test]
fn editor_flags_reserved_shortcuts_and_literals_under_a_task() {
    for (raw, flagged) in [
        ("Ship it\n- #tasks\n  - note", "#tasks"),
        ("Ship it\n- TASKS\n  - note", "TASKS"),
        ("Ship it\n- #schedule-log\n  - note", "#schedule-log"),
        (
            "Ship it\n- #\"Schedule Log\"\n  - note",
            "#\"Schedule Log\"",
        ),
        ("Ship it\n- SCHEDULE LOG\n  - note", "SCHEDULE LOG"),
        ("Ship it\n- #links\n  - note", "#links"),
        ("@foo+bar\n- #tasks\n  - note", "#tasks"),
        ("@foo+bar\n- TASKS\n  - note", "TASKS"),
    ] {
        assert_eq!(
            reserved_ranges(raw),
            vec![("reserved_drawer", flagged.to_string())],
            "{raw:?}"
        );
        let parse = editor(raw);
        let diagnostic = &parse.diagnostics[0];
        assert!(
            diagnostic.message.contains("is managed by Bob")
                && diagnostic.message.contains("something else"),
            "{diagnostic:?}"
        );
    }
}

#[test]
fn reserved_diagnostic_message_matches_the_commit_resolver() {
    let parse = editor("Ship it\n- #schedule-log\n  - note");
    let commit = crate::native::capture_drawers::resolve_drawer_for_write(
        &[],
        "schedule-log",
        false,
        &Default::default(),
        "",
        None,
        crate::native::capture_drawers::DrawerScope::Task,
    )
    .expect_err("the resolver refuses a managed name");
    assert_eq!(commit.code, "reserved_drawer");
    assert_eq!(parse.diagnostics[0].code, commit.code);
    assert_eq!(parse.diagnostics[0].message, commit.message);
}

#[test]
fn reserved_ranges_are_utf8_byte_offsets() {
    // A multibyte parent line shifts every later byte offset.
    let raw = "Ship caf\u{e9} \u{1f680}\n- #tasks\n  - note";
    assert_eq!(
        reserved_ranges(raw),
        vec![("reserved_drawer", "#tasks".to_string())]
    );
    let literal = "Ship caf\u{e9} \u{1f680}\n- TASKS\n  - note";
    assert_eq!(
        reserved_ranges(literal),
        vec![("reserved_drawer", "TASKS".to_string())]
    );
}

#[test]
fn legal_controls_and_vault_dependent_names_stay_unflagged() {
    for raw in [
        // An ordinary drawer, shortcut and literal.
        "Ship it\n- #future-work\n  - note",
        "Ship it\n- FUTURE WORK\n  - note",
        // A childless literal ALL-CAPS bullet is a plain note, even when
        // the word is a managed name.
        "Ship it\n- TASKS",
        "Ship it\n- NYC",
        // An escaped hash and a nested task-type marker are not drawers.
        "Ship it\n- \\#tasks\n  - note",
        "Ship it\n- check\n  - #tasks",
        // A `^prj` structural heading is only reserved on a project note,
        // which only the vault can tell.
        "@foo+bar\n- #pomodoros\n  - note",
        // A same-line note is not an under-task position at all.
        "@foo+bar a note\n- TASKS\n  - note",
    ] {
        assert!(reserved_ranges(raw).is_empty(), "{raw:?}");
    }
}

#[test]
fn a_new_project_note_keeps_its_structural_sections() {
    for raw in [
        "Parent @cash^p1@\n- #tasks\n  - merged\n- Real",
        "Parent @cash^p2@\n- TASKS\n  - merged\n- Real",
        "Parent @cash^p3@\n- #schedule-log\n  - merged\n- Real",
    ] {
        assert!(reserved_ranges(raw).is_empty(), "{raw:?}");
        assert_eq!(editor(raw).mode, EditorMode::ProjectNote, "{raw:?}");
    }
}

#[test]
fn reserved_childless_shortcut_keeps_its_drawer_notes_need() {
    let parse = editor("Ship it\n- #tasks");
    assert_eq!(parse.mode, EditorMode::Incomplete);
    assert!(parse.needs.contains(&Need::DrawerNotes), "{parse:?}");
    assert_eq!(
        reserved_ranges("Ship it\n- #tasks"),
        vec![("reserved_drawer", "#tasks".to_string())]
    );
}

#[test]
fn each_reserved_bullet_is_flagged_once_and_a_glued_drawer_stays_quiet() {
    let raw = "Ship it\n- #tasks\n  - a\n- #future-work\n  - b\n- LINKS\n  - c";
    assert_eq!(
        reserved_ranges(raw),
        vec![
            ("reserved_drawer", "#tasks".to_string()),
            ("reserved_drawer", "LINKS".to_string()),
        ]
    );
    // Every bullet of a glued drawer item already lives in that drawer, so
    // a drawer-looking bullet there is not a drawer position.
    assert!(
        reserved_ranges("@foo+bar#future-work\n- TASKS\n  - note").is_empty()
    );
}

#[test]
fn execution_files_a_shortcut_under_a_new_task() {
    let parsed = execute("Ship it\n- #future-work\n  - try replication")
        .expect("drawer group");
    assert_eq!(parsed.sub_bullets[0].body, "FUTURE WORK");
    assert!(parsed.sub_bullets[0].drawer.is_some());
    assert_eq!(parsed.sub_bullets[1].body, "try replication");
    assert_eq!(parsed.sub_bullets[1].depth, AuthoredDepth::Nested);
}

// ---------------------------------------------------------------------------
// Completion at a first-level bullet's leading `#`.
// ---------------------------------------------------------------------------

use super::super::completion::drawer_bullet_completion_at;
use super::super::drawer_bullet::DrawerBulletTarget;

#[test]
fn bullet_start_hash_claims_a_drawer_field_not_a_type_field() {
    let raw = "@foo+bar\n- #";
    let existing = field(raw, raw.len()).expect("drawer field");
    assert_eq!(existing.context, CompletionContext::Drawer);
    assert_eq!(existing.route.as_deref(), Some("foo"));
    assert_eq!(existing.block_id.as_deref(), Some("bar"));
    assert_eq!(existing.query, "");
    assert_eq!(existing.replacement, (raw.len() - 1, raw.len()));

    let typed = "Ship it\n- #future";
    let fresh = field(typed, typed.len()).expect("drawer field");
    assert_eq!(fresh.context, CompletionContext::Drawer);
    assert_eq!(fresh.query, "future");
    // A new task has no block ID; its route is the inbox by default.
    assert_eq!(fresh.route.as_deref(), Some("mac_inbox"));
    assert_eq!(fresh.block_id, None);
}

#[test]
fn bullet_drawer_target_follows_the_item_shape() {
    let at = |raw: &str| {
        drawer_bullet_completion_at(raw, raw.len()).map(|bullet| bullet.target)
    };
    assert_eq!(
        at("@foo+bar\n- #"),
        Some(DrawerBulletTarget::ExistingTask {
            route: "foo".to_string(),
            block_id: "bar".to_string(),
        })
    );
    assert_eq!(
        at("Ship it\n- #"),
        Some(DrawerBulletTarget::NewTask {
            route: None,
            text: "Ship it".to_string(),
        })
    );
    assert_eq!(
        at("Ship it @foo\n- #"),
        Some(DrawerBulletTarget::NewTask {
            route: Some("foo".to_string()),
            text: "Ship it".to_string(),
        })
    );
    assert_eq!(
        at("@@foo\nShip it\n- #"),
        Some(DrawerBulletTarget::NewTask {
            route: Some("foo".to_string()),
            text: "Ship it".to_string(),
        })
    );
    assert_eq!(
        at("@foo^fresh@ Plan\n- #"),
        Some(DrawerBulletTarget::NewProjectNote {
            route: "foo".to_string(),
            block_id: "fresh".to_string(),
            text: "Plan".to_string(),
        })
    );
    // A drawer bullet may already stand among the item's bullets.
    assert_eq!(
        at("@foo+bar\n- #future\n  - note\n- #"),
        Some(DrawerBulletTarget::ExistingTask {
            route: "foo".to_string(),
            block_id: "bar".to_string(),
        })
    );
}

#[test]
fn bullet_starts_outside_an_under_task_position_open_nothing() {
    for raw in [
        // The bullets nest under a same-line note.
        "@foo+bar fix\n- #",
        // A glued drawer already files every bullet into it.
        "@foo+bar#req\n- #",
        // A `@route#Section` heading bullet.
        "@foo#Ideas\n- #",
        // Pomodoro notes, closes, and an explicit toggle own their bullets.
        "%n\n- #",
        "x\n- #",
        "@foo+bar!\n- #",
        // An escaped hash is prose.
        "Ship it\n- \\#",
    ] {
        assert_eq!(field(raw, raw.len()), None, "{raw:?}");
        assert_eq!(
            drawer_bullet_completion_at(raw, raw.len()),
            None,
            "{raw:?}"
        );
    }
}

#[test]
fn nested_and_parent_line_hashes_still_complete_task_types() {
    for raw in ["Ship it\n- check\n  - #", "Ship it #"] {
        let type_field = field(raw, raw.len()).expect("type field");
        assert_eq!(type_field.context, CompletionContext::TaskType, "{raw:?}");
    }
}

#[test]
fn bullet_cursor_must_follow_the_hash() {
    let raw = "Ship it\n- #fu";
    let hash = raw.find('#').expect("hash");
    assert_eq!(field(raw, hash), None);
    assert!(field(raw, hash + 1).is_some());
    assert!(field(raw, raw.len()).is_some());
}

#[test]
fn bullet_continuation_needs_a_token_ending_the_line_without_a_nested_bullet() {
    let continues = |raw: &str, cursor: usize| {
        drawer_bullet_completion_at(raw, cursor)
            .expect("bullet field")
            .continuation
    };
    let end = "Ship it\n- #fu";
    assert!(continues(end, end.len()));
    assert!(continues("Ship it\n- #fu\n- other", end.len()));
    assert!(continues("Ship it\n- #fu\n\nSecond item", end.len()));
    assert!(!continues("Ship it\n- #fu\n  - note", end.len()));
    assert!(!continues("Ship it\n- #fu tail", end.len()));
    assert!(!continues("Ship it\n- #fu ", end.len()));
}

#[test]
fn notebook_note_bullet_hash_is_empty_success_with_policy() {
    let mut policy =
        crate::native::capture_language::notebook::NotebookRoutePolicy::default(
        );
    policy.insert(
        "dev_tools".into(),
        crate::native::capture_language::notebook::NotebookRouteInfo {
            title: "Developer tools".into(),
            summary: Some("Tools".into()),
            owner: Some("dev".into()),
            breadcrumb: vec!["dev".into(), "Developer tools".into()],
            sections: Vec::new(),
        },
    );
    let note = "Parent @dev_tools\n- #fu";
    assert!(
        completion_field_at_with(note, note.len(), Some(&policy)).is_none(),
        "notebook note detail opens no drawer"
    );
    let list = "@dev_tools\n- #fu";
    assert!(
        completion_field_at_with(list, list.len(), Some(&policy)).is_none(),
        "notebook list bullets open no drawer"
    );
    let lexical =
        completion_field_at(note, note.len()).expect("lexical drawer");
    assert_eq!(lexical.context, CompletionContext::Drawer);
}

#[test]
fn bullet_query_decodes_a_partial_and_a_closed_literal() {
    let query = |raw: &str, cursor: usize| {
        drawer_bullet_completion_at(raw, cursor)
            .expect("bullet field")
            .query
    };
    let partial = "Ship it\n- #\"FUT";
    assert_eq!(query(partial, partial.len()), "FUT");
    let closed = "Ship it\n- #\"FUTURE WORK\"";
    assert_eq!(query(closed, closed.len()), "FUTURE WORK");
    // A mid-token cursor queries only the text before it.
    assert_eq!(query("Ship it\n- #future", 13), "fu");
}
