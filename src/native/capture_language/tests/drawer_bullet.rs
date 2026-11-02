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
