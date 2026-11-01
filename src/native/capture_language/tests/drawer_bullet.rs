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
