//! Requirement bullets: the shared position resolver, the lexical level
//! gate, and both parsers (`bob capture` items and `capture-parse`).

use super::super::line::{
    empty_child_after_markers_error, invalid_child_line_error,
};
use super::super::req_bullets::{
    checkbox_range, code_for_message, invalid_child_line_message,
    is_deep_placeholder_row, resolve, LevelGate, ReqBullet, ReqHost, ReqRole,
    FREE_LEVELS, LIST_FREE_LEVELS,
};
use super::*;
use crate::native::task_reqs::copy;

fn bullet(level: u8, mark: Option<char>) -> ReqBullet {
    ReqBullet {
        level,
        mark,
        header: false,
        empty: false,
        line_number: usize::from(level) + 1,
    }
}

fn req(level: u8) -> ReqBullet {
    bullet(level, Some(' '))
}

fn note(level: u8) -> ReqBullet {
    bullet(level, None)
}

fn header(level: u8) -> ReqBullet {
    ReqBullet {
        header: true,
        ..note(level)
    }
}

fn roles(host: ReqHost, bullets: &[ReqBullet]) -> Vec<ReqRole> {
    resolve(host, None, FREE_LEVELS, bullets).roles
}

fn codes_for(
    host: ReqHost,
    item_mark: Option<char>,
    bullets: &[ReqBullet],
) -> Vec<(Option<usize>, &'static str)> {
    resolve(host, item_mark, FREE_LEVELS, bullets)
        .refusals
        .iter()
        .map(|refusal| (refusal.bullet, refusal.code))
        .collect()
}

const fn depth(depth: u8) -> ReqRole {
    ReqRole::Req { depth }
}

// ---------------------------------------------------------------------------
// The roles table, one row per position.
// ---------------------------------------------------------------------------

#[test]
fn first_level_checkboxes_under_a_task_are_depth_one_requirements() {
    for host in [ReqHost::NewTask, ReqHost::ExistingTask] {
        assert_eq!(
            roles(host, &[req(1), note(1), header(1), note(2)]),
            vec![
                depth(1),
                ReqRole::Note,
                ReqRole::DrawerHeader,
                ReqRole::DrawerEntry
            ],
            "{host:?}"
        );
    }
}

#[test]
fn a_checkbox_under_a_drawer_header_is_req_in_drawer() {
    let bullets = [header(1), req(2), note(2)];
    assert_eq!(
        roles(ReqHost::NewTask, &bullets),
        vec![
            ReqRole::DrawerHeader,
            ReqRole::Refused,
            ReqRole::DrawerEntry
        ]
    );
    assert_eq!(
        codes_for(ReqHost::NewTask, None, &bullets),
        vec![(Some(1), "req_in_drawer")]
    );
}

#[test]
fn requirements_nest_and_plain_bullets_under_them_are_notes() {
    let bullets = [req(1), req(2), req(3), note(4), note(5), req(4)];
    assert_eq!(
        roles(ReqHost::NewTask, &bullets),
        vec![
            depth(1),
            depth(2),
            depth(3),
            ReqRole::Note,
            ReqRole::Note,
            depth(4)
        ]
    );
    assert!(codes_for(ReqHost::NewTask, None, &bullets).is_empty());
}

#[test]
fn a_checkbox_under_a_note_is_req_parent() {
    let bullets = [note(1), req(2), note(2)];
    assert_eq!(
        roles(ReqHost::NewTask, &bullets),
        vec![ReqRole::Note, ReqRole::Refused, ReqRole::Note]
    );
    assert_eq!(
        codes_for(ReqHost::NewTask, None, &bullets),
        vec![(Some(1), "req_parent")]
    );
}

#[test]
fn the_same_line_form_makes_the_item_line_a_requirement() {
    let bullets = [req(1), note(1), req(2), note(2)];
    let resolution =
        resolve(ReqHost::SameLineReq, Some(' '), FREE_LEVELS, &bullets);
    assert_eq!(resolution.item_line, Some(1));
    assert_eq!(
        resolution.roles,
        vec![depth(2), ReqRole::Note, ReqRole::Refused, ReqRole::Note]
    );
    // A checkbox under the item requirement's note is `req_parent`.
    assert_eq!(
        codes_for(ReqHost::SameLineReq, Some(' '), &bullets),
        vec![(Some(2), "req_parent")]
    );
}

#[test]
fn a_pre_checked_same_line_item_is_req_state_and_silences_its_bullets() {
    let bullets = [req(1), bullet(1, Some('x'))];
    let resolution =
        resolve(ReqHost::SameLineReq, Some('x'), FREE_LEVELS, &bullets);
    assert_eq!(resolution.item_line, None);
    assert_eq!(resolution.roles, vec![ReqRole::Refused, ReqRole::Refused]);
    assert_eq!(
        codes_for(ReqHost::SameLineReq, Some('x'), &bullets),
        vec![(None, "req_state")]
    );
}

#[test]
fn single_note_bullets_refuse_checkboxes_as_req_parent() {
    let bullets = [req(1), note(1), note(2)];
    assert_eq!(
        roles(ReqHost::SingleNote, &bullets),
        vec![ReqRole::Refused, ReqRole::Note, ReqRole::Note]
    );
    assert_eq!(
        codes_for(ReqHost::SingleNote, None, &bullets),
        vec![(Some(0), "req_parent")]
    );
}

#[test]
fn drawer_items_refuse_every_checkbox_including_the_same_line_one() {
    let bullets = [note(1), req(1), req(2)];
    assert_eq!(
        codes_for(ReqHost::Drawer, Some(' '), &bullets),
        vec![(None, "req_in_drawer"), (Some(1), "req_in_drawer")]
    );
    assert_eq!(
        roles(ReqHost::Drawer, &bullets),
        vec![ReqRole::DrawerEntry, ReqRole::Refused, ReqRole::Refused]
    );
}

#[test]
fn project_notes_keep_rows_and_take_requirements_under_task_rows() {
    let bullets = [
        note(1),
        req(2),
        req(3),
        note(2),
        bullet(1, Some('x')),
        req(2),
        header(1),
        req(2),
        note(2),
    ];
    assert_eq!(
        roles(ReqHost::ProjectNote, &bullets),
        vec![
            ReqRole::ProjectRow,
            depth(1),
            depth(2),
            ReqRole::Note,
            ReqRole::ProjectRow,
            depth(1),
            ReqRole::DrawerHeader,
            ReqRole::Refused,
            ReqRole::DrawerEntry,
        ]
    );
    assert_eq!(
        codes_for(ReqHost::ProjectNote, None, &bullets),
        vec![(Some(7), "req_in_drawer")]
    );
}

#[test]
fn ineligible_hosts_keep_checkboxes_as_notes_and_refuse_a_third_level() {
    let bullets = [req(1), req(2), req(3)];
    let resolution = resolve(ReqHost::Ineligible, None, FREE_LEVELS, &bullets);
    assert_eq!(
        resolution.roles,
        vec![ReqRole::Note, ReqRole::Note, ReqRole::Refused]
    );
    assert_eq!(resolution.refusals.len(), 1);
    assert_eq!(resolution.refusals[0].code, "invalid_child_line");
    assert_eq!(
        resolution.refusals[0].message,
        invalid_child_line_error(4),
        "ineligible hosts keep today's copy"
    );
}

// ---------------------------------------------------------------------------
// Levels and checks.
// ---------------------------------------------------------------------------

#[test]
fn a_requirement_note_keeps_exactly_one_nested_note_level() {
    let bullets = [req(1), note(2), note(3), note(4)];
    assert_eq!(
        roles(ReqHost::NewTask, &bullets),
        vec![depth(1), ReqRole::Note, ReqRole::Note, ReqRole::Refused]
    );
    let resolution = resolve(ReqHost::NewTask, None, FREE_LEVELS, &bullets);
    assert_eq!(resolution.refusals[0].code, "invalid_child_line");
    assert_eq!(
        resolution.refusals[0].message,
        invalid_child_line_message(bullets[3].line_number, true)
    );
}

#[test]
fn a_third_level_under_plain_notes_and_level_jumps_stay_invalid() {
    for bullets in [
        vec![note(1), note(2), note(3)],
        vec![req(1), req(3)],
        vec![note(1), req(2), req(3)],
    ] {
        let found = codes_for(ReqHost::NewTask, None, &bullets);
        assert!(
            found.iter().any(|(_, code)| *code == "invalid_child_line")
                || found.iter().any(|(_, code)| *code == "req_parent"),
            "{bullets:?}: {found:?}"
        );
    }
    assert_eq!(
        codes_for(ReqHost::NewTask, None, &[note(1), note(2), note(3)]),
        vec![(Some(2), "invalid_child_line")]
    );
    assert_eq!(
        codes_for(ReqHost::NewTask, None, &[req(1), req(3)]),
        vec![(Some(1), "invalid_child_line")]
    );
    // The refused `req_parent` bullet's subtree reports nothing further.
    assert_eq!(
        codes_for(ReqHost::NewTask, None, &[note(1), req(2), req(3)]),
        vec![(Some(1), "req_parent")]
    );
}

#[test]
fn checks_run_position_then_state_then_depth() {
    assert_eq!(
        codes_for(ReqHost::NewTask, None, &[bullet(1, Some('x'))]),
        vec![(Some(0), "req_state")]
    );
    assert_eq!(
        codes_for(ReqHost::NewTask, None, &[note(1), bullet(2, Some('x'))]),
        vec![(Some(1), "req_parent")]
    );
    assert_eq!(
        codes_for(ReqHost::NewTask, None, &[header(1), bullet(2, Some('-'))]),
        vec![(Some(1), "req_in_drawer")]
    );
}

#[test]
fn requirements_nest_sixteen_deep_and_refuse_the_seventeenth() {
    let sixteen: Vec<ReqBullet> = (1..=16).map(req).collect();
    assert!(codes_for(ReqHost::NewTask, None, &sixteen).is_empty());
    assert_eq!(roles(ReqHost::NewTask, &sixteen)[15], depth(16));
    let seventeen: Vec<ReqBullet> = (1..=17).map(req).collect();
    assert_eq!(
        codes_for(ReqHost::NewTask, None, &seventeen),
        vec![(Some(16), "req_depth")]
    );
    // The same-line form starts at depth 1, so its bullets reach 16 one
    // level sooner.
    let fifteen: Vec<ReqBullet> = (1..=15).map(req).collect();
    assert!(codes_for(ReqHost::SameLineReq, Some(' '), &fifteen).is_empty());
    let sixteen_under_item: Vec<ReqBullet> = (1..=16).map(req).collect();
    assert_eq!(
        codes_for(ReqHost::SameLineReq, Some(' '), &sixteen_under_item),
        vec![(Some(15), "req_depth")]
    );
}

#[test]
fn an_empty_requirement_is_a_placeholder_unless_it_has_children() {
    let empty = ReqBullet {
        empty: true,
        ..req(1)
    };
    let bullets = [empty, note(1)];
    let resolution = resolve(ReqHost::NewTask, None, FREE_LEVELS, &bullets);
    assert!(resolution.refusals.is_empty());
    assert!(resolution.is_placeholder(&bullets, 0));
    let bullets = [empty, note(2)];
    let resolution = resolve(ReqHost::NewTask, None, FREE_LEVELS, &bullets);
    assert_eq!(resolution.refusals[0].code, "empty_child_after_markers");
    assert!(!resolution.is_placeholder(&bullets, 0));
}

#[test]
fn list_levels_count_after_the_shift() {
    // After the shift, a list bullet's first nested level is free; a
    // second only nests beneath a requirement chain.
    let resolution = resolve(
        ReqHost::NewTask,
        None,
        LIST_FREE_LEVELS,
        &[note(1), note(2)],
    );
    assert_eq!(resolution.refusals[0].code, "invalid_child_line");
    let resolution = resolve(
        ReqHost::NewTask,
        None,
        LIST_FREE_LEVELS,
        &[req(1), note(2), note(3), req(2)],
    );
    assert!(resolution.refusals.is_empty(), "{:?}", resolution.refusals);
}

#[test]
fn hosts_map_from_kinds_and_modes_alike() {
    let existing = CaptureKind::SubBullet {
        target: SubBulletTarget::BlockId("ex".to_string()),
        section: None,
    };
    let drawer = CaptureKind::SubBullet {
        target: SubBulletTarget::BlockId("ex".to_string()),
        section: Some(DrawerSelector {
            text: "notes".to_string(),
            exact: false,
        }),
    };
    let link = CaptureKind::PomodoroLink {
        block_id: "ex".to_string(),
        pomodoro_name: None,
        pomodoro_name_is_literal: false,
        start: None,
        close: None,
        spelling: PomodoroLinkSpelling::At,
    };
    for (kind, mode, body, section, host) in [
        (
            CaptureKind::Task,
            EditorMode::Task,
            "Fix",
            None,
            ReqHost::NewTask,
        ),
        (
            existing.clone(),
            EditorMode::SubBullet,
            "",
            None,
            ReqHost::ExistingTask,
        ),
        (
            link,
            EditorMode::PomodoroLink,
            "",
            None,
            ReqHost::ExistingTask,
        ),
        (
            existing.clone(),
            EditorMode::SubBullet,
            "[ ] Handles CRLF",
            None,
            ReqHost::SameLineReq,
        ),
        (
            existing,
            EditorMode::SubBullet,
            "Call Morgan",
            None,
            ReqHost::SingleNote,
        ),
        (
            drawer,
            EditorMode::SubBullet,
            "",
            Some("notes"),
            ReqHost::Drawer,
        ),
        (
            CaptureKind::PomodoroNote,
            EditorMode::PomodoroNote,
            "x",
            None,
            ReqHost::Ineligible,
        ),
    ] {
        assert_eq!(ReqHost::from_kind(&kind, body), host, "{kind:?}");
        assert_eq!(ReqHost::from_mode(mode, body, section), host, "{mode:?}");
    }
}

#[test]
fn the_level_gate_lets_deeper_lines_through_only_under_checkbox_chains() {
    let mut gate = LevelGate::new(FREE_LEVELS);
    assert!(gate.admits(1));
    gate.push(1, true);
    assert!(gate.admits(2));
    gate.push(2, false);
    assert!(gate.admits(3), "a note whose parent is a checkbox");
    gate.push(3, false);
    assert!(!gate.admits(4), "a note of a note");
    assert!(!gate.admits(5), "a jump");
    gate.push(1, false);
    gate.push(2, false);
    assert!(!gate.admits(3));
}

#[test]
fn spans_codes_and_placeholder_rows() {
    assert_eq!(checkbox_range("[ ] text", 10), Some((10, 14)));
    assert_eq!(checkbox_range("[x]\ttext", 0), Some((0, 4)));
    assert_eq!(checkbox_range("[ ]", 5), Some((5, 8)));
    assert_eq!(checkbox_range("[link] text", 0), None);
    assert_eq!(checkbox_range("[[x]] text", 0), None);
    assert!(is_deep_placeholder_row("    - "));
    assert!(is_deep_placeholder_row("      -"));
    assert!(!is_deep_placeholder_row("  - "));
    assert!(!is_deep_placeholder_row("     - "));
    assert_eq!(code_for_message(copy::REQ_IN_DRAWER), Some("req_in_drawer"));
    assert_eq!(code_for_message(copy::REQ_PARENT), Some("req_parent"));
    assert_eq!(code_for_message(copy::REQ_STATE), Some("req_state"));
    assert_eq!(code_for_message(copy::REQ_DEPTH), Some("req_depth"));
    assert_eq!(
        code_for_message(&invalid_child_line_message(4, true)),
        Some("invalid_child_line")
    );
    assert_eq!(
        code_for_message(&invalid_child_line_message(4, false)),
        None
    );
}

// ---------------------------------------------------------------------------
// Both parsers.
// ---------------------------------------------------------------------------

fn req_depths(parsed: &ParsedCaptureText) -> Vec<Option<u8>> {
    parsed
        .sub_bullets
        .iter()
        .map(|bullet| bullet.req.map(|info| info.depth))
        .collect()
}

fn marker_depths(parse: &EditorParse) -> Vec<(usize, usize, u8)> {
    parse
        .items
        .iter()
        .flat_map(|item| item.req_markers.iter())
        .map(|marker| (marker.start, marker.end, marker.depth))
        .collect()
}

#[test]
fn a_new_task_admits_three_requirement_levels_and_a_note_chain() {
    let raw = "Fix export\n- [ ] Handles CRLF\n  - [ ] Windows\n    - [ ] Mixed\n      - seen in March\n        - twice\n- see the ticket";
    let parsed = execute(raw).expect("requirements parse");
    assert_eq!(
        sub_bullet_depths(&parsed.sub_bullets),
        vec![1, 2, 3, 4, 5, 1]
    );
    assert_eq!(
        req_depths(&parsed),
        vec![Some(1), Some(2), Some(3), None, None, None]
    );
    assert_eq!(parsed.sub_bullets[0].body, "[ ] Handles CRLF");

    let editor = editor(raw);
    assert!(editor.diagnostics.is_empty(), "{:?}", editor.diagnostics);
    let first = raw.find("[ ] Handles").expect("first");
    let second = raw.find("[ ] Windows").expect("second");
    let third = raw.find("[ ] Mixed").expect("third");
    assert_eq!(
        marker_depths(&editor),
        vec![
            (first, first + 4, 1),
            (second, second + 4, 2),
            (third, third + 4, 3)
        ]
    );
    assert_eq!(
        span_kinds(&editor)
            .iter()
            .filter(|kind| **kind == SpanKind::ReqMarker)
            .count(),
        3
    );
}

#[test]
fn notes_between_requirement_branches_reset_the_chain() {
    let raw = "Fix\n- [ ] a\n  - [ ] b\n- note\n  - nested\n- [ ] c\n  - [ ] d";
    let parsed = execute(raw).expect("parse");
    assert_eq!(
        req_depths(&parsed),
        vec![Some(1), Some(2), None, None, Some(1), Some(2)]
    );
    assert!(editor(raw).diagnostics.is_empty());
}

#[test]
fn both_parsers_refuse_with_the_shared_codes_and_copy() {
    for (raw, code, message) in [
        ("Fix\n- note\n  - [ ] bad", "req_parent", copy::REQ_PARENT),
        ("Fix\n- [x] done", "req_state", copy::REQ_STATE),
        (
            "Fix\n- #future-work\n  - [ ] bad",
            "req_in_drawer",
            copy::REQ_IN_DRAWER,
        ),
        (
            "Fix\n- **NOTES**\n  - [ ] bad",
            "req_in_drawer",
            copy::REQ_IN_DRAWER,
        ),
        (
            "@work+ex Call Morgan\n- [ ] bad",
            "req_parent",
            copy::REQ_PARENT,
        ),
        (
            "@work+ex#notes\n- [ ] bad",
            "req_in_drawer",
            copy::REQ_IN_DRAWER,
        ),
        (
            "@work+ex#notes [ ] bad",
            "req_in_drawer",
            copy::REQ_IN_DRAWER,
        ),
        ("@work+ex [x] done", "req_state", copy::REQ_STATE),
    ] {
        let error = execute(raw).expect_err(raw);
        assert_eq!(error, message, "{raw:?}");
        assert_eq!(code_for_message(&error), Some(code), "{raw:?}");
        let parsed = editor(raw);
        assert_eq!(codes(&parsed), vec![code], "{raw:?}");
        assert!(
            !span_kinds(&parsed).contains(&SpanKind::ReqMarker),
            "{raw:?}: refused checkboxes get no span"
        );
    }
}

#[test]
fn the_depth_bound_holds_at_sixteen_and_refuses_seventeen() {
    let chain = |levels: usize| {
        let mut raw = String::from("Deep");
        for level in 0..levels {
            raw.push('\n');
            raw.push_str(&"  ".repeat(level));
            raw.push_str(&format!("- [ ] level {}", level + 1));
        }
        raw
    };
    let parsed = execute(&chain(16)).expect("sixteen");
    assert_eq!(req_depths(&parsed).last(), Some(&Some(16)));
    assert!(editor(&chain(16)).diagnostics.is_empty());
    assert_eq!(execute(&chain(17)).expect_err("seventeen"), copy::REQ_DEPTH);
    assert_eq!(codes(&editor(&chain(17))), vec!["req_depth"]);
}

#[test]
fn the_level_jump_and_a_third_level_note_keep_invalid_child_line() {
    for (raw, line) in [
        ("Fix\n- [ ] a\n      - jump", 3),
        ("Fix\n- note\n  - nested\n    - deep", 4),
    ] {
        let error = execute(raw).expect_err(raw);
        assert_eq!(error, invalid_child_line_message(line, true), "{raw:?}");
        assert_eq!(codes(&editor(raw)), vec!["invalid_child_line"], "{raw:?}");
    }
}

#[test]
fn an_empty_requirement_placeholder_writes_nothing_but_keeps_its_span() {
    let raw = "Fix\n- [ ] \n- note";
    let parsed = execute(raw).expect("placeholder");
    assert_eq!(sub_bullet_bodies(&parsed.sub_bullets), vec!["note"]);
    let parsed = editor(raw);
    assert!(parsed.diagnostics.is_empty());
    assert_eq!(marker_depths(&parsed), vec![(6, 10, 1)]);
    assert_eq!(parsed.sub_bullets.len(), 1);

    let raw = "Fix\n- [ ] \n  - child";
    assert_eq!(
        execute(raw).expect_err("empty with children"),
        empty_child_after_markers_error(2)
    );
    assert_eq!(codes(&editor(raw)), vec!["empty_child_after_markers"]);
}

#[test]
fn a_deeper_marker_only_row_is_a_placeholder_under_a_task() {
    let raw = "Fix\n- [ ] a\n  - [ ] b\n    - ";
    let parsed = execute(raw).expect("deep placeholder");
    assert_eq!(parsed.sub_bullets.len(), 2);
    assert!(editor(raw).diagnostics.is_empty());
}

#[test]
fn an_existing_task_takes_requirements_and_the_same_line_form() {
    let parsed =
        execute("@work+ex\n- [ ] a\n  - [ ] b\n    - note").expect("multi");
    assert_eq!(req_depths(&parsed), vec![Some(1), Some(2), None]);

    let raw = "@work+ex [ ] Handles CRLF\n- [ ] sub\n  - [ ] subsub\n- plain";
    let parsed = execute(raw).expect("same line");
    assert_eq!(parsed.body, "[ ] Handles CRLF");
    assert_eq!(req_depths(&parsed), vec![Some(2), Some(3), None]);
    let editor = editor(raw);
    assert!(editor.diagnostics.is_empty(), "{:?}", editor.diagnostics);
    let item = raw.find("[ ] Handles").expect("item checkbox");
    assert_eq!(marker_depths(&editor)[0], (item, item + 4, 1));
    assert_eq!(marker_depths(&editor).len(), 3);
}

#[test]
fn a_placeholder_only_existing_task_reads_like_ensure_next() {
    let parsed = execute("@work+ex\n- [ ] ").expect("toggle");
    assert!(
        matches!(parsed.kind, CaptureKind::TaskToggle { .. }),
        "{:?}",
        parsed.kind
    );
    let editor = editor("@work+ex\n- [ ] ");
    assert_eq!(editor.mode, EditorMode::TaskToggle);
}

#[test]
fn task_list_bullets_count_levels_after_the_shift() {
    let raw = "@work\n- Task A\n  - [ ] req\n    - [ ] deeper\n      - note\n- Task B";
    let draft = execute_draft(raw).expect("list");
    let first = &draft.items[0].parsed;
    assert_eq!(sub_bullet_depths(&first.sub_bullets), vec![1, 2, 3]);
    assert_eq!(req_depths(first), vec![Some(1), Some(2), None]);
    let parsed = editor(raw);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    assert_eq!(marker_depths(&parsed).len(), 2);

    // A plain note's nested note still refuses after the shift.
    let raw = "@work\n- Task A\n  - note\n    - deep";
    let error = execute_draft(raw).expect_err("note chain");
    assert!(
        error.ends_with(&invalid_child_line_message(4, true)),
        "{error}"
    );
}

#[test]
fn sealed_project_notes_take_requirements_under_task_rows() {
    let raw = "@work^ship@ Ship it\n- Row one\n  - [ ] req\n    - [ ] deeper\n      - note\n  - plain\n- [x] done row";
    let parsed = execute(raw).expect("project note");
    assert_eq!(
        sub_bullet_depths(&parsed.sub_bullets),
        vec![1, 2, 3, 4, 2, 1]
    );
    assert_eq!(
        req_depths(&parsed),
        vec![None, Some(1), Some(2), None, None, None]
    );
    let editor = editor(raw);
    assert!(editor.diagnostics.is_empty(), "{:?}", editor.diagnostics);
    assert_eq!(marker_depths(&editor).len(), 2);

    let raw = "@work^ship@ Ship it\n- Row one\n  - plain\n    - deep";
    assert_eq!(
        execute(raw).expect_err("row note chain"),
        invalid_child_line_message(4, true)
    );
}

#[test]
fn ineligible_items_keep_todays_checkbox_and_level_rules() {
    // A Pomodoro note keeps checkbox bullets as plain text and refuses a
    // third level with today's copy.
    let raw = "%n Standup\n- [ ] a\n  - [ ] b\n    - [ ] c";
    let error = execute(raw).expect_err("third level");
    assert_eq!(error, invalid_child_line_error(4));
    let parsed = editor(raw);
    assert!(!span_kinds(&parsed).contains(&SpanKind::ReqMarker));
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message == invalid_child_line_error(4)),
        "{:?}",
        parsed.diagnostics
    );
}

#[test]
fn a_prerequisite_on_a_deep_requirement_line_still_counts() {
    let raw = "@work Fix\n- [ ] a\n  - [ ] b\n    - [ ] c &work:ex";
    let parsed = execute(raw).expect("deep dependency");
    assert_eq!(parsed.dependencies.len(), 1, "{:?}", parsed.dependencies);
    assert_eq!(parsed.sub_bullets[2].body, "[ ] c");
    let editor = editor(raw);
    assert_eq!(editor.dependencies.len(), 1);
    assert!(span_kinds(&editor).contains(&SpanKind::DependencySigil));
}
