//! Notebook section-bullet scanner, claim, editor, and seed plan.

use super::super::section_bullet::{
    plan_create_item, plan_list_item, scan_first_level_line, scan_leading,
    NotebookBodyOutcome, SectionBulletHit,
};
use super::*;
use crate::native::capture_language::notebook::{
    staged_notebook_sections, NotebookRouteInfo, NotebookRoutePolicy,
    NotebookSection, NotebookSectionProvenance,
};

fn policy() -> NotebookRoutePolicy {
    let mut policy = NotebookRoutePolicy::default();
    policy.insert(
        "dev".into(),
        NotebookRouteInfo {
            title: "Developer tools".into(),
            summary: Some("Tools".into()),
            owner: Some("dev".into()),
            breadcrumb: vec!["dev".into(), "Developer tools".into()],
            sections: vec![NotebookSection {
                title: "Git".into(),
                line_index: 6,
            }],
        },
    );
    policy
}

fn strict_options(
    notebook_policy: Option<&NotebookRoutePolicy>,
) -> CaptureParseOptions<'_> {
    CaptureParseOptions {
        notebook_policy,
        ..CaptureParseOptions::routing_off()
    }
}

fn editor_options(
    notebook_policy: Option<&NotebookRoutePolicy>,
) -> EditorParseOptions<'_> {
    EditorParseOptions {
        notebook_policy,
        ..EditorParseOptions::routing_off()
    }
}

fn draft(raw: &str) -> Result<ParsedCaptureDraft, String> {
    let policy = policy();
    parse_capture_draft_with_options(
        raw,
        None,
        None,
        &strict_options(Some(&policy)),
    )
}

fn editor_parse(raw: &str) -> EditorItemParse {
    let policy = policy();
    parse_for_editor_with(raw, &editor_options(Some(&policy)))
        .items
        .remove(0)
}

fn editor_full(raw: &str) -> EditorParse {
    let policy = policy();
    parse_for_editor_with(raw, &editor_options(Some(&policy)))
}

fn complete_hit(
    body: &str,
) -> (String, bool, String, std::ops::Range<usize>, String) {
    match scan_leading(body, 0).expect(body) {
        SectionBulletHit::Complete {
            selector,
            exact,
            leftover,
            span,
            authored,
        } => (selector, exact, leftover, span, authored),
        other => panic!("{body}: {other:?}"),
    }
}

#[test]
fn scanner_hits_and_opener_exceptions() {
    let (selector, exact, leftover, span, authored) = complete_hit("#Git");
    assert_eq!(selector, "Git");
    assert!(!exact);
    assert!(leftover.is_empty());
    assert_eq!(span, 0..4);
    assert_eq!(authored, "#Git");

    let (selector, exact, ..) = complete_hit("#API-keys");
    assert_eq!(selector, "API-keys");
    assert!(!exact);

    let (selector, exact, ..) = complete_hit("#coc.nvim");
    assert_eq!(selector, "coc.nvim");
    assert!(!exact);

    let (selector, exact, leftover, span, _) = complete_hit("#\"API Design\"");
    assert_eq!(selector, "API Design");
    assert!(exact);
    assert!(leftover.is_empty());
    assert_eq!(span, 0..13);

    let (selector, exact, ..) = complete_hit("#“API Design”");
    assert_eq!(selector, "API Design");
    assert!(exact);

    assert!(scan_leading("#1", 0).is_none());
    assert!(scan_leading("##heading", 0).is_none());
    assert!(scan_leading("#-dash", 0).is_none());
    assert!(matches!(
        scan_leading("#", 0),
        Some(SectionBulletHit::Incomplete { .. })
    ));
    assert!(matches!(
        scan_leading("\\#git", 0),
        Some(SectionBulletHit::Escaped { .. })
    ));
    let hit = scan_first_level_line("- #Git", 0).expect("first-level");
    let SectionBulletHit::Complete { selector, span, .. } = hit else {
        panic!("{hit:?}");
    };
    assert_eq!(selector, "Git");
    assert_eq!(span, 2..6);
}

#[test]
fn caret_inside_quotes_is_title_and_outside_is_leftover() {
    let (selector, exact, leftover, ..) = complete_hit("#\"Title^literal\"");
    assert_eq!(selector, "Title^literal");
    assert!(exact);
    assert!(leftover.is_empty());

    let (selector, _, leftover, ..) = complete_hit("#Git^id");
    assert_eq!(selector, "Git");
    assert_eq!(leftover, "^id");
}

#[test]
fn utf8_byte_ranges_follow_the_source() {
    let raw = "- #\"café\"";
    let hit = scan_first_level_line(raw, 0).expect("utf8");
    let SectionBulletHit::Complete { selector, span, .. } = hit else {
        panic!("{hit:?}");
    };
    assert_eq!(selector, "café");
    assert_eq!(&raw[span.clone()], "#\"café\"");
}

#[test]
fn route_only_list_fans_out_nested_notes() {
    let parsed = draft(
        "@dev\n- #Git\n  - Use worktrees ^worktrees\n  - Pin hooks\n- Prefer fd over find",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(parsed.items.len(), 3);
    let kinds: Vec<_> = parsed
        .items
        .iter()
        .map(|item| match &item.parsed.kind {
            CaptureKind::NotebookNote {
                section, block_id, ..
            } => (
                item.parsed.body.as_str(),
                section.as_deref(),
                block_id.as_deref(),
            ),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("Use worktrees", Some("Git"), Some("worktrees")),
            ("Pin hooks", Some("Git"), None),
            ("Prefer fd over find", None, None),
        ]
    );
    let first = parsed.items[0].list.as_ref().expect("list");
    assert_eq!((first.position, first.count), (1, 3));
    let fanout = parsed.items[0].fanout.as_ref().expect("fanout");
    assert_eq!(fanout.position, 1);
    assert_eq!(fanout.count, 2);
    assert_eq!(fanout.marker, "#Git");
    assert_eq!(parsed.items[1].fanout.as_ref().expect("second").position, 2);
    assert!(parsed.items[2].fanout.is_none());
}

#[test]
fn escaped_hash_stays_a_top_note() {
    let parsed =
        draft("@dev\n- \\#git").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(parsed.items.len(), 1);
    assert_eq!(parsed.items[0].parsed.body, "#git");
    let CaptureKind::NotebookNote { section, .. } =
        &parsed.items[0].parsed.kind
    else {
        panic!("{:?}", parsed.items[0].parsed.kind);
    };
    assert!(section.is_none());
}

#[test]
fn all_caps_in_a_list_stays_a_plain_note() {
    let parsed = draft("@dev\n- KEYBINDS\n  - leader is space")
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(parsed.items.len(), 1);
    assert_eq!(parsed.items[0].parsed.body, "KEYBINDS");
    assert_eq!(parsed.items[0].parsed.sub_bullets.len(), 1);
    let CaptureKind::NotebookNote { section, .. } =
        &parsed.items[0].parsed.kind
    else {
        panic!("{:?}", parsed.items[0].parsed.kind);
    };
    assert!(section.is_none());
}

#[test]
fn refusals_match_the_contract_table() {
    let cases = [
        ("@dev\n- #", "notebook_section_required"),
        ("@dev\n- #Git", "empty_section_bullet"),
        ("@dev\n- #Git leftover", "section_bullet_text"),
        ("@dev#Git\n- #Docker\n  - x", "section_bullet_position"),
        (
            "@dev\n- #Git\n  - one line\n    - too deep",
            "section_bullet_depth",
        ),
    ];
    for (raw, code) in cases {
        let error = parse_capture_draft_with_options_coded(
            raw,
            None,
            None,
            &strict_options(Some(&policy())),
        )
        .expect_err(raw);
        assert_eq!(error.code, Some(code), "{raw}: {}", error.message);
    }
}

#[test]
fn editor_bare_hash_and_empty_notes_are_incomplete() {
    let bare = editor_parse("@dev\n- #");
    assert_eq!(bare.mode, EditorMode::Incomplete);
    assert_eq!(bare.needs, vec![Need::Section]);
    assert!(bare.diagnostics.is_empty());
    assert!(bare
        .spans
        .iter()
        .any(|span| span.kind == SpanKind::InteractivePlaceholder));

    let empty = editor_parse("@dev\n- #Git");
    assert_eq!(empty.mode, EditorMode::Incomplete);
    assert_eq!(empty.needs, vec![Need::SectionNotes]);
    assert!(empty.diagnostics.is_empty());
    let section = empty
        .spans
        .iter()
        .find(|span| span.kind == SpanKind::NotebookSection)
        .expect("section span");
    assert_eq!(section.title.as_deref(), Some("Git"));
    assert_eq!(section.section_action, Some("existing"));

    let glued = editor_parse("Note @dev#");
    assert_eq!(glued.mode, EditorMode::Incomplete);
    assert!(glued.needs.contains(&Need::Section));
    assert!(glued.diagnostics.is_empty());
}

#[test]
fn header_section_plus_section_bullet_is_position_error() {
    let editor = editor_parse("@dev#Git\n- #Docker\n  - x");
    assert_eq!(
        editor.diagnostics.first().map(|diagnostic| diagnostic.code),
        Some("section_bullet_position")
    );
    assert!(editor
        .diagnostics
        .first()
        .is_some_and(|diagnostic| diagnostic.message.contains("## Git")));
}

#[test]
fn create_seed_plan_puts_plain_siblings_first() {
    let raw = "Notes @dev^nvim@ %n\n- #Git\n  - Use worktrees\n- Prefer fd\n  - Keep this";
    let draft = split_capture_draft(raw);
    let item = &draft.items[0];
    match plan_create_item(item, "dev_nvim") {
        NotebookBodyOutcome::Ready(plan) => {
            assert_eq!(plan.top_notes.len(), 1);
            assert_eq!(plan.top_notes[0].text, "Prefer fd");
            assert_eq!(plan.sections.len(), 1);
            assert_eq!(plan.sections[0].selector, "Git");
            assert_eq!(plan.sections[0].notes[0].text, "Use worktrees");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn create_all_caps_and_hash_merge() {
    let raw = "Notes @dev^nvim@ %n\n- KEYMAPS\n  - x\n- #Keymaps\n  - y";
    let draft = split_capture_draft(raw);
    match plan_create_item(&draft.items[0], "dev_nvim") {
        NotebookBodyOutcome::Ready(plan) => {
            assert_eq!(plan.sections.len(), 1);
            assert_eq!(plan.sections[0].selector, "Keymaps");
            assert_eq!(plan.sections[0].notes.len(), 2);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn staged_create_sees_earlier_hash_bullets_before_the_caret() {
    let raw = "Notes @dev^nvim@ %n\n- #Git\n  - x\n- #";
    let draft = split_capture_draft(raw);
    let create = &draft.items[0];
    let hash = raw.rfind('#').expect("bare hash");
    let staged = staged_notebook_sections(&policy(), raw, "dev_nvim", hash);
    let titles: Vec<&str> = staged
        .iter()
        .map(|section| section.title.as_str())
        .collect();
    assert!(titles.contains(&"Git"), "{titles:?}");
    assert_eq!(
        staged
            .iter()
            .find(|section| section.title == "Git")
            .map(|section| section.provenance),
        Some(NotebookSectionProvenance::Draft)
    );
    let _ = create;
}

#[test]
fn drawer_position_without_notebook_policy_is_unchanged() {
    let error = parse_capture_draft_with_options_coded(
        "@dev\n- #fix stuff",
        None,
        None,
        &CaptureParseOptions::routing_off(),
    )
    .expect_err("drawer");
    assert_eq!(error.code, Some("drawer_bullet_position"));
}

#[test]
fn list_item_plan_ignores_all_caps() {
    let raw = "@dev\n- KEYBINDS\n  - leader is space";
    let draft = split_capture_draft(raw);
    match plan_list_item(&draft.items[0], "dev") {
        NotebookBodyOutcome::Ready(plan) => {
            assert!(plan.sections.is_empty());
            assert_eq!(plan.top_notes.len(), 1);
            assert_eq!(plan.top_notes[0].text, "KEYBINDS");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn editor_create_section_notes_need() {
    let parse = editor_full("Notes @dev^nvim@ %n\n- #Git");
    let item = &parse.items[0];
    assert_eq!(item.mode, EditorMode::Incomplete);
    assert_eq!(item.needs, vec![Need::SectionNotes]);
    assert!(item.diagnostics.is_empty());
}

#[test]
fn editor_keeps_one_item_per_section_bullet() {
    let parse = editor_full("@dev\n- #Git\n  - a\n  - b");
    assert_eq!(parse.items.len(), 1);
    let item = &parse.items[0];
    assert_eq!(item.mode, EditorMode::NotebookNote);
    assert_eq!(item.section.as_deref(), Some("Git"));
    assert!(item.drawer_bullet_target.is_none());
    assert!(item.body.is_empty());
    let section = item
        .spans
        .iter()
        .find(|span| span.kind == SpanKind::NotebookSection)
        .expect("section");
    assert_eq!(section.section_action, Some("existing"));
}

#[test]
fn hash_under_a_single_notebook_note_stays_opaque() {
    let parsed = draft("A note @dev\n- #Git\n  - nested")
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(parsed.items.len(), 1);
    assert_eq!(parsed.items[0].parsed.body, "A note");
    assert_eq!(parsed.items[0].parsed.sub_bullets[0].body, "#Git");
    let editor = editor_parse("A note @dev\n- #Git\n  - nested");
    assert_eq!(editor.mode, EditorMode::NotebookNote);
    assert!(editor.drawer_bullet_target.is_none());
    assert!(editor.diagnostics.is_empty());
}

#[test]
fn list_near_miss_and_create_near_miss_refuse() {
    let error = parse_capture_draft_with_options_coded(
        "@dev\n- #Gi\n  - x",
        None,
        None,
        &strict_options(Some(&policy())),
    )
    .expect_err("near miss");
    assert_eq!(error.code, Some("notebook_section_near_miss"));

    let error = parse_capture_draft_with_options_coded(
        "Notes @dev^nvim@ %n\n- #Git\n  - x\n- #Gi\n  - y",
        None,
        None,
        &strict_options(Some(&policy())),
    )
    .expect_err("create near miss");
    assert_eq!(error.code, Some("notebook_section_near_miss"));
}

#[test]
fn staged_percent_n_hash_is_visible_to_a_later_glued_selector() {
    let raw = "Notes @dev^nvim@ %n\n- #Git\n  - x\n\nMore @dev_nvim#";
    let hash = raw.rfind('#').expect("later hash");
    let titles: Vec<String> =
        staged_notebook_sections(&policy(), raw, "dev_nvim", hash)
            .into_iter()
            .map(|section| section.title)
            .collect();
    assert!(titles.iter().any(|title| title == "Git"), "{titles:?}");
}

#[test]
fn create_plan_puts_siblings_between_and_after_sections_on_top() {
    let raw =
        "Notes @dev^nvim@ %n\n- first\n- #Git\n  - g\n- middle\n- #Docker\n  - d\n- last";
    let draft = split_capture_draft(raw);
    match plan_create_item(&draft.items[0], "dev_nvim") {
        NotebookBodyOutcome::Ready(plan) => {
            let tops: Vec<&str> = plan
                .top_notes
                .iter()
                .map(|note| note.text.as_str())
                .collect();
            assert_eq!(tops, ["first", "middle", "last"]);
            let sections: Vec<&str> = plan
                .sections
                .iter()
                .map(|section| section.selector.as_str())
                .collect();
            assert_eq!(sections, ["Git", "Docker"]);
        }
        other => panic!("{other:?}"),
    }
}
