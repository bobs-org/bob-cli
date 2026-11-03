//! Move bullets (`- ^<id>` / `- :<id>`): lexer, execution/editor agreement,
//! and capture-parse shape.

use super::super::move_bullet::{
    scan_move_bullet, MoveBulletHit, MoveBulletInvalid,
};
use super::*;

fn scan(body: &str) -> Option<MoveBulletHit> {
    scan_move_bullet(body, 0, body)
}

fn scan_at(body: &str, base: usize, source: &str) -> Option<MoveBulletHit> {
    scan_move_bullet(body, base, source)
}

#[test]
fn complete_move_bullet_claims_sigil_and_id_with_spans() {
    for (body, sigil, id) in [
        ("^equity-docs", '^', "equity-docs"),
        (":call-ms", ':', "call-ms"),
        ("^1", '^', "1"),
        ("  ^spaced", '^', "spaced"),
    ] {
        let base = 40;
        let hit = scan_at(body, base, body).expect("complete move");
        let MoveBulletHit::Complete {
            sigil: hit_sigil,
            id: hit_id,
            sigil_span,
            id_span,
            span,
        } = hit
        else {
            panic!("expected a complete move for {body:?}: {hit:?}");
        };
        assert_eq!(hit_sigil, sigil);
        assert_eq!(hit_id, id);
        let body_start = body.find(sigil).expect("sigil byte");
        assert_eq!(sigil_span, (base + body_start, base + body_start + 1));
        assert_eq!(
            id_span,
            (base + body_start + 1, base + body_start + 1 + id.len())
        );
        assert_eq!(span, sigil_span.0..id_span.1);
    }
}

#[test]
fn lone_sigil_is_an_unfinished_move() {
    for body in ["^", ":", "^ ", ":\t", "  ^  "] {
        let hit = scan(body).expect("unfinished move");
        let MoveBulletHit::Unfinished { sigil, span } = hit else {
            panic!("expected an unfinished move for {body:?}: {hit:?}");
        };
        assert!(matches!(sigil, '^' | ':'));
        let start = body.find(sigil).expect("sigil byte");
        assert_eq!(span, start..start + 1);
    }
}

#[test]
fn lone_sigil_followed_by_words_stays_prose() {
    for body in ["^ tweak", ": query", "^  spaced words", "  : more words"] {
        assert_eq!(scan(body), None, "{body:?} stays prose");
    }
}

#[test]
fn words_after_the_token_are_a_teaching_error() {
    for body in ["^a tweak", ":a tweak it", "^a-b c"] {
        let hit = scan(body).expect("move-shaped");
        let MoveBulletHit::Invalid { code, kind, .. } = hit else {
            panic!("expected invalid for {body:?}: {hit:?}");
        };
        assert_eq!(code, "move_bullet_text");
        assert!(matches!(kind, MoveBulletInvalid::Text { .. }), "{kind:?}");
    }
}

#[test]
fn markers_after_the_token_name_the_marker() {
    // `%c` and `#fix` never reach the move lexer: directives and type
    // markers are masked before the child loop, so both parsers refuse
    // those first (covered below). `#9` stays prose and reaches the move.
    for (body, marker) in [
        ("^a s:2", "s:2"),
        ("^a $3", "$3"),
        ("^a p:2", "p:2"),
        ("^a #9", "#9"),
        ("^a @other", "@other"),
        (":a s:2 then more", "s:2"),
    ] {
        let hit = scan(body).expect("move-shaped");
        let MoveBulletHit::Invalid { code, kind, .. } = hit else {
            panic!("expected invalid for {body:?}: {hit:?}");
        };
        assert_eq!(code, "move_bullet_marker");
        let MoveBulletInvalid::Marker { token } = kind else {
            panic!("expected a marker for {body:?}: {kind:?}");
        };
        assert_eq!(token, marker);
    }
}

#[test]
fn checkbox_before_the_token_is_a_teaching_error() {
    for (body, status) in [("[x] ^a", 'x'), ("[ ] ^a", ' '), ("[x] :a", 'x')] {
        let hit = scan(body).expect("move-shaped");
        let MoveBulletHit::Invalid { code, kind, .. } = hit else {
            panic!("expected invalid for {body:?}: {hit:?}");
        };
        assert_eq!(code, "move_bullet_checkbox");
        let MoveBulletInvalid::Checkbox { status: hit_status } = kind else {
            panic!("expected a checkbox for {body:?}: {kind:?}");
        };
        assert_eq!(hit_status, status);
    }
}

#[test]
fn cross_note_shapes_point_at_another_note() {
    for (body, sigil, id) in [
        ("^r:a", '^', "a"),
        (":r:a", ':', "a"),
        ("^my-note:some-id", '^', "some-id"),
    ] {
        let hit = scan(body).expect("move-shaped");
        let MoveBulletHit::Invalid { code, kind, .. } = hit else {
            panic!("expected invalid for {body:?}: {hit:?}");
        };
        assert_eq!(code, "move_bullet_cross_note");
        let MoveBulletInvalid::CrossNote {
            sigil: hit_sigil,
            id: hit_id,
        } = kind
        else {
            panic!("expected cross-note for {body:?}: {kind:?}");
        };
        assert_eq!(hit_sigil, sigil);
        assert_eq!(hit_id, id);
    }
}

#[test]
fn reserved_prj_never_moves() {
    for body in ["^prj", ":prj", "^PRJ", ":Prj"] {
        let hit = scan(body).expect("move-shaped");
        let MoveBulletHit::Invalid { code, kind, .. } = hit else {
            panic!("expected invalid for {body:?}: {hit:?}");
        };
        assert_eq!(code, "move_bullet_reserved");
        assert!(
            matches!(kind, MoveBulletInvalid::Reserved { .. }),
            "{kind:?}"
        );
    }
}

#[test]
fn bad_charset_uses_the_existing_charset_error() {
    let hit = scan("^a_b").expect("move-shaped");
    let MoveBulletHit::Invalid { code, kind, .. } = hit else {
        panic!("expected invalid: {hit:?}");
    };
    assert_eq!(code, "invalid_project_task_id");
    let MoveBulletInvalid::BadId { raw } = kind else {
        panic!("expected a charset error: {kind:?}");
    };
    assert_eq!(raw, "a_b");
}

#[test]
fn escapes_and_protected_markup_keep_prose() {
    assert_eq!(scan("\\^a"), None);
    assert_eq!(scan("\\:a"), None);
    assert_eq!(scan("[x] \\^a"), None);
    // The sigil byte itself sits inside a wikilink.
    assert_eq!(scan_at("^a", 2, "[[^a]]"), None);
    // The sigil byte itself sits inside inline code.
    assert_eq!(scan_at("^a", 1, "`^a`"), None);
}

#[test]
fn non_sigil_bodies_stay_prose() {
    for body in [
        "tweak ^a",
        "Draft the memo",
        ":)",
        "^)",
        "::x",
        "^:a",
        "^-x",
        "^_x",
        "[x]",
        "[x] hello",
        "",
        "   ",
    ] {
        assert_eq!(scan(body), None, "{body:?} stays prose");
    }
}

/// Every lexical refusal agrees byte for byte: the execution parser's
/// message is the editor diagnostic's message, with the same code.
#[test]
fn execution_and_editor_agree_on_every_lexical_refusal() {
    let cases = [
        ("Finish @cash^x@\n- ^a tweak", "move_bullet_text"),
        ("Finish @cash^x@\n- :a tweak", "move_bullet_text"),
        ("Finish @cash^x@\n- ^a s:2", "move_bullet_marker"),
        ("Finish @cash^x@\n- ^a $3", "move_bullet_marker"),
        ("Finish @cash^x@\n- ^a #9", "move_bullet_marker"),
        ("Finish @cash^x@\n- ^a @other", "move_bullet_marker"),
        ("Finish @cash^x@\n- [x] ^a", "move_bullet_checkbox"),
        ("Finish @cash^x@\n- [ ] :a", "move_bullet_checkbox"),
        ("Finish @cash^x@\n- ^a\n  - detail", "move_bullet_nested"),
        ("Finish @cash^x@\n- ^a\n- ^a", "move_bullet_duplicate"),
        (
            "Finish @cash^x@\n- ^a\n- tweak ^a",
            "move_bullet_id_collision",
        ),
        (
            "Finish @cash^x@\n- tweak ^a\n- ^a",
            "move_bullet_id_collision",
        ),
        ("Finish @cash^x@\n- ^r:a", "move_bullet_cross_note"),
        ("Finish @cash^x@\n- :r:a", "move_bullet_cross_note"),
        ("Finish @cash^x@\n- ^prj", "move_bullet_reserved"),
        ("Finish @cash^x@\n- :PRJ", "move_bullet_reserved"),
        ("Finish @cash^x@\n- ^a_b", "invalid_project_task_id"),
    ];
    for (raw, code) in cases {
        let message = execute(raw).expect_err("execution refuses");
        let parsed = editor(raw);
        assert_eq!(parsed.diagnostics.len(), 1, "{raw:?}");
        assert_eq!(parsed.diagnostics[0].code, code, "{raw:?}");
        assert_eq!(parsed.diagnostics[0].message, message, "{raw:?}");
    }
}

#[test]
fn unfinished_moves_agree_on_the_need_without_a_diagnostic() {
    for (raw, sigil) in
        [("Finish @cash^x@\n- ^", '^'), ("Finish @cash^x@\n- :", ':')]
    {
        let message = execute(raw).expect_err("execution refuses");
        assert_eq!(
            message,
            super::super::move_bullet::move_bullet_unfinished_error(
                sigil, "cash"
            ),
            "{raw:?}"
        );
        let parsed = editor(raw);
        assert_eq!(parsed.mode, EditorMode::Incomplete);
        assert_eq!(parsed.needs, vec![Need::ProjectTaskMove]);
        assert!(parsed.diagnostics.is_empty());
        assert_eq!(parsed.sub_bullets.len(), 1);
        assert_eq!(parsed.sub_bullets[0].body, "");
        let move_bullet =
            parsed.sub_bullets[0].move_task.as_ref().expect("move");
        assert_eq!(move_bullet.sigil, sigil);
        assert_eq!(move_bullet.block_id, None);
    }
}

#[test]
fn accepted_moves_record_empty_bodies_with_spans_in_both_parsers() {
    let parsed = editor("Finish @cash^x@\n- Draft memo :a\n- ^b");
    assert_eq!(parsed.mode, EditorMode::PomodoroProjectNote);
    assert!(parsed.diagnostics.is_empty());
    assert_eq!(parsed.sub_bullets.len(), 2);
    assert_eq!(parsed.sub_bullets[0].body, "Draft memo");
    assert_eq!(parsed.sub_bullets[1].body, "");
    let move_bullet = parsed.sub_bullets[1].move_task.as_ref().expect("move");
    assert_eq!(move_bullet.sigil, '^');
    assert_eq!(move_bullet.block_id.as_deref(), Some("b"));

    let executed = execute("Finish @cash^x@\n- Draft memo :a\n- ^b")
        .expect("execution accepts");
    assert_eq!(executed.sub_bullets.len(), 2);
    assert_eq!(executed.sub_bullets[0].body, "Draft memo");
    assert_eq!(executed.sub_bullets[1].body, "");
    assert_eq!(executed.sub_bullets[1].task_id, None);
    assert_eq!(
        executed.sub_bullets[1].move_task,
        parsed.sub_bullets[1].move_task
    );

    // The offer seal takes moves the same way.
    let offer = editor("Finish @cash:x@\n- ^a");
    assert_eq!(offer.mode, EditorMode::PomodoroProjectNote);
    assert!(offer.diagnostics.is_empty());
    let offer_move = offer.sub_bullets[0].move_task.as_ref().expect("move");
    assert_eq!(offer_move.block_id.as_deref(), Some("a"));
    let offer_executed =
        execute("Finish @cash:x@\n- ^a").expect("execution accepts");
    assert_eq!(
        offer_executed.sub_bullets[0].move_task,
        offer.sub_bullets[0].move_task
    );
}

#[test]
fn colon_moves_count_as_links_for_the_unused_name_rule() {
    // A `:` move satisfies `#admin`.
    let linked = editor("Finish @cash^x@#admin\n- :a");
    assert_eq!(linked.mode, EditorMode::PomodoroProjectNote);
    assert!(linked.diagnostics.is_empty());

    // A `^` move does not.
    let unlinked = editor("Finish @cash^x@#admin\n- ^a");
    assert_eq!(
        codes(&unlinked),
        vec!["unused_project_note_pomodoro"],
        "{unlinked:?}"
    );
    let message = execute("Finish @cash^x@#admin\n- ^a").expect_err("refused");
    assert_eq!(unlinked.diagnostics[0].message, message);
}

#[test]
fn nested_and_foreign_lines_keep_todays_meaning() {
    // A nested `^` line is never a move: it keeps the trailing-ID meaning.
    let nested = editor("Finish @cash^x@\n- Draft\n  - ^a");
    assert_eq!(codes(&nested), vec!["misplaced_project_task_id"]);
    let message =
        execute("Finish @cash^x@\n- Draft\n  - ^a").expect_err("refused");
    assert_eq!(nested.diagnostics[0].message, message);

    // A nested lone sigil still opens the block-ID picker.
    let nested_lone = editor("Finish @cash^x@\n- Draft\n  - ^");
    assert_eq!(nested_lone.mode, EditorMode::Incomplete);
    assert_eq!(nested_lone.needs, vec![Need::BlockId]);

    // A `%c` directive or `#type` marker on the same line is refused
    // before the move post-pass in execution: directives and type
    // markers mask before the child loop. Both parsers report that
    // refusal first with the same message; the editor still lexes the
    // masked move underneath.
    for (raw, code) in [
        ("Finish @cash^x@\n- ^a %c", "invalid_project_note"),
        ("Finish @cash^x@\n- ^a #fix", "type_marker_not_applicable"),
    ] {
        let message = execute(raw).expect_err("execution refuses first");
        let parsed = editor(raw);
        assert!(!parsed.diagnostics.is_empty(), "{raw:?}");
        assert_eq!(parsed.diagnostics[0].code, code, "{raw:?}");
        assert_eq!(parsed.diagnostics[0].message, message, "{raw:?}");
    }
    // A bare `@route` word after the move names the marker in both
    // parsers: with no body text it never becomes a second destination.
    let routed = editor("Finish @cash^x@\n- ^a @other");
    assert_eq!(codes(&routed), vec!["move_bullet_marker"], "{routed:?}");

    // Outside project-note items a leading `^` stays literal prose.
    let foreign = editor("Do work @cash\n- ^a");
    assert!(foreign.diagnostics.is_empty());
    assert_eq!(foreign.sub_bullets.len(), 1);
    assert_eq!(foreign.sub_bullets[0].body, "^a");
    assert_eq!(foreign.sub_bullets[0].move_task, None);
    let foreign_executed =
        execute("Do work @cash\n- ^a").expect("foreign prose");
    assert_eq!(foreign_executed.sub_bullets[0].body, "^a");
    assert_eq!(foreign_executed.sub_bullets[0].move_task, None);

    // An escaped sigil stays a literal task in a project-note item.
    let escaped = editor("Finish @cash^x@\n- \\^a");
    assert!(escaped.diagnostics.is_empty());
    assert_eq!(escaped.sub_bullets[0].move_task, None);
}
