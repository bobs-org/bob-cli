//! The typing-tip reverse check: positive rows, a negative corpus, and the
//! fixture coverage that keeps every alias id classified and exercised.

use std::collections::BTreeSet;

use super::*;

/// One forward fixture per alias id: typing the unshifted key at the end of
/// `raw` fires exactly that alias. Every positive fixture also runs the
/// reverse check through `assert_alias_applied`.
const FORWARD_FIXTURES: &[(&str, AliasId)] = &[
    ("Buy milk .", AliasId::RouteAfterText),
    ("Buy milk /", AliasId::HashMenu),
    ("@errands\\", AliasId::HashMenu),
    ("@errands.", AliasId::BlockIdAfterRoute),
    ("@errands;", AliasId::LinkAfterRoute),
    (";", AliasId::TaskLinkItem),
    ("@cash^goog-exit@\n- .", AliasId::MoveBulletCaret),
    ("@cash:goog-exit@\n- ;", AliasId::MoveBulletColon),
    ("@cash,", AliasId::ParentTask),
    ("Buy milk ,", AliasId::ParentTask),
    ("Fix it =", AliasId::Relation),
    ("@home .", AliasId::Dependency),
    ("Buy milk ;", AliasId::BlockedTask),
    // Untipped ids still need a stamped forward fixture.
    ("Fix it ~=", AliasId::RelationFlip),
    ("Fix it ~~=", AliasId::RelationFlip),
    ("@cash^goog-exit@\n- ^.", AliasId::MoveChoice),
    ("@cash+goog-exit,", AliasId::NextParent),
    ("Create task < @cash+line,", AliasId::NextParent),
    ("x=2-", AliasId::PastSegment),
    ("xx", AliasId::CloseComplete),
    ("xs", AliasId::ClosePark),
];

#[test]
fn every_alias_id_has_a_stamped_forward_fixture() {
    let mut seen = BTreeSet::new();
    for (raw, id) in FORWARD_FIXTURES {
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(assert_alias_applied(raw, &rewrite), *id, "{raw:?}");
        seen.insert(id.code());
    }
    for id in AliasId::ALL {
        assert!(seen.contains(id.code()), "no forward fixture for {id:?}");
    }
}

#[test]
fn alias_id_codes_are_unique_snake_case_wire_spellings() {
    let codes: BTreeSet<&str> =
        AliasId::ALL.iter().map(|id| id.code()).collect();
    assert_eq!(codes.len(), AliasId::ALL.len());
    for code in codes {
        assert!(
            code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
            "{code}"
        );
    }
}

#[test]
fn the_tipped_classification_matches_the_v1_lessons() {
    let tipped: BTreeSet<&str> = AliasId::ALL
        .iter()
        .filter(|id| id.tipped())
        .map(|id| id.code())
        .collect();
    assert_eq!(
        tipped,
        BTreeSet::from([
            "route_after_text",
            "hash_menu",
            "block_id_after_route",
            "link_after_route",
            "task_link_item",
            "move_bullet_caret",
            "move_bullet_colon",
            "parent_task",
            "relation",
            "dependency",
            "blocked_task",
        ])
    );
}

fn tip_at_end(raw: &str) -> Option<SigilAliasTip> {
    sigil_alias_tip(raw, raw.len())
}

#[test]
fn sigil_alias_tip_proves_each_positive_row() {
    for (raw, id, keys, sigil, position) in [
        (
            "Buy milk @",
            AliasId::RouteAfterText,
            &["."][..],
            '@',
            Some("after a word"),
        ),
        (
            "- [ ] Buy @",
            AliasId::RouteAfterText,
            &["."],
            '@',
            Some("after a word"),
        ),
        (
            "@errands^",
            AliasId::BlockIdAfterRoute,
            &["."],
            '^',
            Some("right after a route"),
        ),
        (
            "@errands:",
            AliasId::LinkAfterRoute,
            &[";"],
            ':',
            Some("right after a route"),
        ),
        (
            ":",
            AliasId::TaskLinkItem,
            &[";"],
            ':',
            Some("at an item start"),
        ),
        (
            "@cash^goog-exit@\n- ^",
            AliasId::MoveBulletCaret,
            &["."],
            '^',
            Some("on a move bullet"),
        ),
        (
            "@cash:goog-exit@\n- :",
            AliasId::MoveBulletColon,
            &[";"],
            ':',
            Some("on a move bullet"),
        ),
        (
            "@cash+",
            AliasId::ParentTask,
            &[","],
            '+',
            Some("right after a route"),
        ),
        (
            "Buy milk +",
            AliasId::ParentTask,
            &[","],
            '+',
            Some("after a word"),
        ),
        (
            "Buy milk #",
            AliasId::HashMenu,
            &["/", "\\"],
            '#',
            Some("after a word"),
        ),
        (
            "@errands#",
            AliasId::HashMenu,
            &["/", "\\"],
            '#',
            Some("right after a route"),
        ),
        (
            "Fix it ~",
            AliasId::Relation,
            &["="],
            '~',
            Some("after a word"),
        ),
        (
            "@home &",
            AliasId::Dependency,
            &["."],
            '&',
            Some("once routed"),
        ),
        (
            "@sase Buy milk &",
            AliasId::Dependency,
            &["."],
            '&',
            Some("once routed"),
        ),
        (
            "Buy milk *",
            AliasId::BlockedTask,
            &[";"],
            '*',
            Some("after a word"),
        ),
    ] {
        let tip = tip_at_end(raw).unwrap_or_else(|| panic!("{raw:?}: no tip"));
        assert_eq!(tip.id, id, "{raw:?}");
        assert_eq!(tip.keys, keys, "{raw:?}");
        assert_eq!(tip.sigil, sigil, "{raw:?}");
        assert_eq!(tip.position, position, "{raw:?}");
        assert_eq!(tip.range, (raw.len() - 1, raw.len()), "{raw:?}");
    }
}

#[test]
fn sigil_alias_tip_keeps_only_the_keys_the_forward_rewrite_accepts() {
    // `/` stays literal inside a `=` split and a wikilink note path, so only
    // `\` would have typed `#`: the tip never names `/` there.
    for raw in ["=*fix#", "=#", "[[folder#"] {
        let tip = tip_at_end(raw).unwrap_or_else(|| panic!("{raw:?}: no tip"));
        assert_eq!(tip.id, AliasId::HashMenu, "{raw:?}");
        assert_eq!(tip.keys, ["\\"], "{raw:?}");
        assert_eq!(tip.position, None, "{raw:?}");
    }
}

#[test]
fn sigil_alias_tip_uses_the_forward_pipeline_for_a_spaced_next_parent() {
    // The glued `@cash+goog-exit,` -> ` +` next-parent alias is untipped, but
    // typing `,` after the existing space is the ordinary `parent_task`
    // alias, so the reverse check proves that true lesson instead.
    let tip = tip_at_end("@cash+goog-exit +").expect("parent task tip");
    assert_eq!(tip.id, AliasId::ParentTask);
    assert_eq!(tip.keys, [","]);
    assert_eq!(tip.position, Some("after a word"));
}

#[test]
fn sigil_alias_tip_reports_utf8_byte_offsets() {
    let raw = "Caf\u{e9} run @";
    let tip = tip_at_end(raw).expect("utf-8 tip");
    assert_eq!(tip.id, AliasId::RouteAfterText);
    assert_eq!(raw.len(), 11);
    assert_eq!(tip.range, (10, 11));
    assert!(raw.is_char_boundary(tip.range.0));
    assert!(raw.is_char_boundary(tip.range.1));

    let multi = "\u{1f680} \u{e9} @";
    assert_eq!(tip_at_end(multi).expect("emoji tip").range.1, multi.len());
}

#[test]
fn sigil_alias_tip_only_looks_at_the_sigil_ending_at_the_cursor() {
    // A later line does not matter, but text after the sigil on its own line
    // is not something the forward alias fires on, so nothing is taught.
    let raw = "Buy milk @\nlater";
    let cursor = "Buy milk @".len();
    let tip = sigil_alias_tip(raw, cursor).expect("tip before a later line");
    assert_eq!(tip.id, AliasId::RouteAfterText);
    assert_eq!(tip.range, (cursor - 1, cursor));
    assert!(sigil_alias_tip("Buy milk @ later", cursor).is_none());
    assert!(sigil_alias_tip(raw, raw.len()).is_none());
    assert!(sigil_alias_tip(raw, 0).is_none());
    assert!(sigil_alias_tip("Buy milk @", 99).is_none());
    // A cursor inside a multi-byte character is not a boundary.
    assert!(sigil_alias_tip("caf\u{e9}", 4).is_none());
}

#[test]
fn sigil_alias_tip_stays_silent_for_the_negative_corpus() {
    for (raw, cursor) in [
        ("@", 1),
        ("  @", 3),
        ("- @", 3),
        ("- [ ] @", 7),
        ("@cash Buy milk @", 16),
        ("foo@", 4),
        ("Buy `x @", 8),
        ("See [[note @", 12),
        ("#", 1),
        ("Buy milk\n#", 10),
        ("=/", 2),
        ("[[folder/", 9),
        ("Buy #\"foo bar #", 15),
        ("Fix it ~~", 9),
        ("@cash^goog-exit@\n- ^^", 21),
        ("*", 1),
        ("!", 1),
        ("x*", 2),
        ("x!", 2),
        ("x=2~", 4),
        ("Buy milk &", 10),
        ("Buy milk .", 10),
        ("Buy milk", 8),
        // A template draft never rewrites, so it never teaches.
        ("Do work @file:{0}^ %t\n\nrow", "Do work @file:{0}^".len()),
    ] {
        assert!(raw.len() >= cursor, "{raw:?}");
        assert!(sigil_alias_tip(raw, cursor).is_none(), "{raw:?}");
    }
}

#[test]
fn sigil_alias_tip_agrees_with_the_forward_rewrite_for_every_prefix() {
    // Whatever bob teaches must really work: swapping the first proved key in
    // and rewriting yields the draft again, byte for byte.
    for raw in [
        "Buy milk @",
        "@errands^",
        "@errands:",
        ":",
        "@cash+",
        "Buy milk #",
        "Fix it ~",
        "@home &",
        "Buy milk *",
        "@cash^goog-exit@\n- ^",
    ] {
        let tip = tip_at_end(raw).unwrap_or_else(|| panic!("{raw:?}: no tip"));
        let mut typed = raw.to_string();
        typed.replace_range(tip.range.0..tip.range.1, &tip.keys[0]);
        let rewrite = rewrite_draft(&typed, Some(raw.len()));
        assert_eq!(
            rewrite.rule,
            Some(RewriteRule::ExpandSigilAlias),
            "{raw:?}"
        );
        assert_eq!(rewrite.text, raw, "{raw:?}");
        assert_eq!(rewrite.cursor, Some(raw.len()), "{raw:?}");
    }
}
