//! List-capture unit vectors: split structure, execution items and
//! refusals, and editor parity for the same shapes.

use super::super::draft::*;
use super::super::editor_model::*;
use super::super::editor_parse::*;
use super::super::model::*;
use super::*;

fn options() -> CaptureParseOptions<'static> {
    CaptureParseOptions::routing_off()
}

fn parse_draft(raw: &str) -> Result<ParsedCaptureDraft, TemplateCodedError> {
    parse_capture_draft_with_options_coded(raw, None, None, &options())
}

fn list_of(item: &CaptureItem<'_>) -> ListContext {
    item.list.clone().expect("list context")
}

#[test]
fn split_task_list_yields_one_item_per_bullet() {
    let draft = split_capture_draft("@dev\n- Fix bank\n- Pay rent");
    assert_eq!(draft.items.len(), 2);
    let first = list_of(&draft.items[0]);
    assert_eq!(first.route, "dev");
    assert_eq!(first.kind, ListKind::Tasks);
    assert_eq!(first.section, None);
    assert_eq!((first.header_line, first.position, first.count), (1, 1, 2));
    assert_eq!((first.header_start, first.header_end), (0, 4));
    assert!(!first.ownerless);
    let second = list_of(&draft.items[1]);
    assert_eq!((second.position, second.count), (2, 2));
}

#[test]
fn split_section_list_files_notes() {
    let draft = split_capture_draft("@dev#Ideas\n- Big idea\n- Small idea");
    assert_eq!(draft.items.len(), 2);
    let first = list_of(&draft.items[0]);
    assert_eq!(first.kind, ListKind::Notes);
    assert_eq!(first.section.as_deref(), Some("Ideas"));
    assert_eq!((first.position, first.count), (1, 2));
}

#[test]
fn split_lone_header_is_an_empty_list() {
    for raw in ["@dev", "@dev#Ideas", "@dev\n- "] {
        let draft = split_capture_draft(raw);
        assert_eq!(draft.items.len(), 1, "{raw}");
        assert_eq!(list_of(&draft.items[0]).count, 0, "{raw}");
    }
}

#[test]
fn split_ignores_declarations_and_non_headers() {
    let declared = split_capture_draft("@@dev\n- A task");
    assert_eq!(declared.declarations.len(), 1);
    assert_eq!(declared.items.len(), 1);
    assert!(declared.items[0].list.is_none());

    for raw in [
        "Plan trip @dev\n- pack bags",
        "@dev+proj\n- A task",
        "Call bank @dev s:3\n- detail",
    ] {
        let draft = split_capture_draft(raw);
        assert!(draft.items.iter().all(|item| item.list.is_none()), "{raw}");
    }
}

#[test]
fn split_header_starts_item_without_blank_line() {
    let draft = split_capture_draft("@dev\n- A task\n@cash\n- B task");
    assert_eq!(draft.items.len(), 2);
    assert_eq!(list_of(&draft.items[0]).route, "dev");
    let second = list_of(&draft.items[1]);
    assert_eq!(second.route, "cash");
    assert_eq!(second.header_line, 3);
}

#[test]
fn execution_task_list_bullet_kinds_and_ids() {
    let draft =
        parse_draft("@dev\n- Plain\n- Named ^named-id\n- Linked :linked-id")
            .expect("task list parses");
    assert_eq!(draft.items.len(), 3);
    let kinds: Vec<&CaptureKind> =
        draft.items.iter().map(|item| &item.parsed.kind).collect();
    assert_eq!(kinds[0], &CaptureKind::Task);
    assert_eq!(
        kinds[1],
        &CaptureKind::TaskWithBlockId {
            block_id: "named-id".to_string(),
        }
    );
    assert!(matches!(
        kinds[2],
        CaptureKind::Pomodoro { block_id, .. } if block_id == "linked-id"
    ));
    for (index, item) in draft.items.iter().enumerate() {
        let list = item.list.clone().expect("list context");
        assert_eq!(list.route, "dev");
        assert_eq!(list.position, index + 1);
        assert_eq!(list.count, 3);
        assert_eq!(item.parsed.route.as_deref(), Some("dev"));
    }
    assert_eq!(draft.items[0].parsed.body, "Plain");
}

#[test]
fn execution_list_nested_children_shift_up_one_level() {
    // The bullet line is the item's text line, so its nested lines parse
    // as first-level children: exactly as `<text> @route` plus those lines
    // would. This is the equivalence invariant.
    let draft = parse_draft("@dev\n- Owner\n  - nested child")
        .expect("nested list parses");
    assert_eq!(draft.items.len(), 1);
    let children = &draft.items[0].parsed.sub_bullets;
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].body, "nested child");
    assert_eq!(children[0].depth, AuthoredDepth::First);
}

#[test]
fn execution_section_list_bullet_kind() {
    let draft = parse_draft("@dev#Ideas\n- Big idea").expect("section list");
    assert_eq!(draft.items.len(), 1);
    assert!(matches!(
        draft.items[0].parsed.kind,
        CaptureKind::Bullet { .. }
    ));
    assert_eq!(draft.items[0].parsed.route.as_deref(), Some("dev"));
}

#[test]
fn execution_list_refusals_carry_stable_codes() {
    for (raw, code) in [
        ("@dev", "empty_list"),
        ("@dev\n- ", "empty_list"),
        ("@dev s:3\n- Call the bank", "list_header_marker"),
        ("@dev\n- A @cash", "list_bullet_marker"),
        ("@dev\n- A +query", "list_bullet_marker"),
        ("@dev\n- A %n", "list_bullet_marker"),
        ("@dev\n- A %l", "list_bullet_marker"),
        ("@dev\n- A !cash:x", "list_bullet_marker"),
        ("@dev\n- [x] Done", "list_bullet_checkbox"),
        ("@dev\n- #fix stuff", "drawer_bullet_position"),
        (
            "@dev\n- Owner\n  - detail :owned",
            "misplaced_project_task_id",
        ),
        ("@dev\n- Owner :", "invalid_project_task_id"),
        ("@dev\n- Owner\n  - detail :", "invalid_project_task_id"),
        ("@dev\n- Bad :bad!id", "invalid_project_task_id"),
        ("@dev\n- Bad :prj", "invalid_project_task_id"),
        ("@dev\n- :orphan", "invalid_project_task_id"),
        ("@dev\n- ^dev:foo", "invalid_project_task_id"),
        ("@dev %t\n- detail {0}\n\nFoo", "template_list"),
    ] {
        let error = parse_draft(raw).unwrap_err();
        assert_eq!(error.code, Some(code), "{raw}");
    }

    let dup = parse_draft("@dev\n- Alpha ^same\n- Beta ^same").unwrap_err();
    assert_eq!(dup.code, Some("duplicate_project_task_id"));
    assert!(dup.message.contains("line 2"));
    assert!(dup.message.contains("line 3"));
}

#[test]
fn execution_list_template_rows_never_split() {
    // A header-shaped template row is an ordinary row value, not a list:
    // the row expands before any header could be recognized.
    let draft = parse_draft("Do {0} @work %t\n\nCall bank")
        .expect("template with plain row");
    assert!(draft.template.is_some());
}

#[test]
fn editor_task_list_reports_items_modes_and_spans() {
    let parse = editor("@dev\n- Plain\n- Linked :linked-id");
    assert_eq!(parse.mode, EditorMode::Task);
    assert_eq!(parse.body, "Plain");
    assert_eq!(parse.route.as_deref(), Some("dev"));
    assert!(parse.needs.is_empty());
    assert_eq!(parse.items.len(), 2);

    let second = &parse.items[1];
    assert_eq!(second.mode, EditorMode::PomodoroTask);
    assert_eq!(second.block_id.as_deref(), Some("linked-id"));
    let kinds: Vec<SpanKind> =
        second.spans.iter().map(|span| span.kind).collect();
    assert!(kinds.contains(&SpanKind::ProjectTaskLinkMarker));
    assert!(kinds.contains(&SpanKind::ProjectTaskBlockId));

    let list = second.list.clone().expect("list context");
    assert_eq!((list.position, list.count), (2, 2));
}

#[test]
fn editor_list_nested_children_shift_up_one_level() {
    let parse = editor("@dev\n- Owner\n  - nested child");
    assert_eq!(parse.sub_bullets.len(), 1);
    assert_eq!(parse.sub_bullets[0].depth, AuthoredDepth::First);
}

#[test]
fn editor_empty_list_needs_list_item() {
    for raw in ["@dev", "@dev#Ideas"] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Incomplete, "{raw}");
        assert_eq!(parse.needs, vec![Need::ListItem], "{raw}");
        assert!(parse.diagnostics.is_empty(), "{raw}");
    }
}

#[test]
fn editor_header_marker_points_at_the_stray_token() {
    for (raw, token) in [
        ("@dev s:3\n- Call the bank", "s:3"),
        ("@dev#Ideas s:3\n- Big idea", "s:3"),
    ] {
        let parse = editor(raw);
        assert_eq!(codes(&parse), vec!["list_header_marker"], "{raw}");
        let start = raw.find(token).expect("marker offset");
        assert_eq!(
            parse.diagnostics[0].range,
            Some((start, start + token.len())),
            "{raw}"
        );
    }
}

#[test]
fn editor_bare_header_reports_header_marker() {
    // A bare `@route` stays literal, so the editor sees the marker on the
    // header line itself and refuses with the same code as execution.
    let parse = editor("@dev s:3\n- Call the bank");
    assert_eq!(codes(&parse), vec!["list_header_marker"]);
}

#[test]
fn editor_duplicate_ids_land_on_the_second_bullet() {
    for sigil in [':', '^'] {
        let raw = format!("@dev\n- Alpha {sigil}same\n- Beta {sigil}same");
        let parse = editor(&raw);
        assert_eq!(codes(&parse), vec!["duplicate_project_task_id"], "{raw}");
        assert_eq!(parse.items.len(), 2);
        assert!(parse.items[0].diagnostics.is_empty());
        assert_eq!(
            parse.items[1]
                .diagnostics
                .iter()
                .map(|d| d.code)
                .collect::<Vec<_>>(),
            vec!["duplicate_project_task_id"],
            "{raw}"
        );
        let message = &parse.items[1].diagnostics[0].message;
        assert!(message.contains("line 2") && message.contains("line 3"));
    }
    // The `:` form underlines the whole token through its link marker.
    let parse = editor("@dev\n- Alpha :same\n- Beta :same");
    let id_start = "@dev\n- Alpha :same\n- Beta ".len();
    assert_eq!(
        parse.items[1].diagnostics[0].range,
        Some((id_start, id_start + ":same".len()))
    );
}

#[test]
fn editor_misplaced_and_unfinished_id_states() {
    let misplaced = editor("@dev\n- Owner\n  - detail :owned");
    assert_eq!(codes(&misplaced), vec!["misplaced_project_task_id"]);

    for raw in ["@dev\n- Owner :", "@dev\n- Owner\n  - detail :"] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Incomplete, "{raw}");
        assert_eq!(parse.needs, vec![Need::BlockId], "{raw}");
        assert!(parse.diagnostics.is_empty(), "{raw}");
        assert!(
            parse
                .spans
                .iter()
                .any(|span| span.kind == SpanKind::InteractivePlaceholder),
            "{raw}"
        );
    }
}

#[test]
fn editor_lone_at_still_needs_route() {
    // A lone `@` is not a list header: the editor keeps its long-standing
    // incomplete state needing `route`. Execution now refuses with
    // `incomplete_route` instead of filing literally.
    let parse = editor("@\n- A task");
    assert_eq!(parse.mode, EditorMode::Incomplete);
    assert_eq!(parse.needs, vec![Need::Route]);
}

#[test]
fn split_quoted_headers_decode_and_preserve_exact() {
    let single = split_capture_draft("@dev#\"Ideas\"\n- Big idea");
    assert_eq!(single.items.len(), 1);
    let first = list_of(&single.items[0]);
    assert_eq!(first.section.as_deref(), Some("Ideas"));
    assert!(first.exact);
    assert_eq!((first.header_start, first.header_end), (0, 12));

    let multi = split_capture_draft("@dev#\"Release Ideas\"\n- Big idea");
    assert_eq!(multi.items.len(), 1);
    let first = list_of(&multi.items[0]);
    assert_eq!(first.section.as_deref(), Some("Release Ideas"));
    assert!(first.exact);
    assert_eq!((first.header_start, first.header_end), (0, 20));

    let plain = split_capture_draft("@dev#Ideas\n- Big idea");
    let first = list_of(&plain.items[0]);
    assert_eq!(first.section.as_deref(), Some("Ideas"));
    assert!(!first.exact);

    // Incomplete literals and trailing bytes are not headers.
    for raw in [
        "@dev#\"Release\n- Big idea",
        "@dev#\"Ideas\"extra\n- Big idea",
        "@dev#Ideas extra\n- Big idea",
        "@dev s:3\n- Big idea",
    ] {
        let draft = split_capture_draft(raw);
        assert!(draft.items.iter().all(|item| item.list.is_none()), "{raw}");
    }

    // Forced/template splits never recognize headers.
    let forced = split_capture_draft_no_lists("@dev#\"Ideas\"\n- Big idea");
    assert!(forced.items.iter().all(|item| item.list.is_none()));
}

#[test]
fn execution_quoted_sections_carry_exact() {
    let quoted =
        parse_draft("@dev#\"Ideas\"\n- Big idea").expect("quoted parses");
    assert!(matches!(
        quoted.items[0].parsed.kind,
        CaptureKind::Bullet { exact: true, .. }
    ));
    let plain = parse_draft("@dev#Ideas\n- Big idea").expect("plain parses");
    assert!(matches!(
        plain.items[0].parsed.kind,
        CaptureKind::Bullet { exact: false, .. }
    ));
}

#[test]
fn execution_bare_at_with_bullets_refuses_incomplete_route() {
    let error = parse_draft("@\n- A task").unwrap_err();
    assert_eq!(error.code, Some("incomplete_route"));
}

#[test]
fn unterminated_section_literal_is_incomplete_never_a_mistake() {
    for raw in ["@sase#\"Fu", "boom @sase#\"Fu", "@sase#\"Fu\n- boom"] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Incomplete, "{raw}");
        assert_eq!(parse.needs, vec![Need::Section], "{raw}");
        assert!(parse.diagnostics.is_empty(), "{raw}");
        assert!(parse.section.is_none(), "{raw}");

        let error = parse_draft(raw).unwrap_err();
        assert_eq!(error.code, Some("incomplete_section"), "{raw}");
        assert!(
            error.message.contains("quoted section name"),
            "{raw}: {}",
            error.message
        );
    }

    let complete = editor(r#"jot @dev#"Ideas""#);
    assert_eq!(complete.mode, EditorMode::Bullet);
    assert_eq!(complete.section.as_deref(), Some("Ideas"));
    assert!(complete.needs.is_empty());
}

#[test]
fn editor_item_at_header_with_bullets_resolves_to_first_item() {
    let raw = "@sase#Fu\n- boom";
    let item = editor_item_at(raw, 8).expect("header item");
    assert_eq!(item.body, "boom");
    assert_eq!(item.route.as_deref(), Some("sase"));
    assert_eq!(item.section.as_deref(), Some("Fu"));
    assert_eq!(item.list.as_ref().map(|list| list.position), Some(1));
}

#[test]
fn split_quoted_headers_hold_utf8_and_crlf_offsets() {
    let utf = split_capture_draft("@dev#\"Caf\u{00e9}\"\n- Big idea");
    let first = list_of(&utf.items[0]);
    assert_eq!(first.section.as_deref(), Some("Caf\u{00e9}"));
    assert_eq!((first.header_start, first.header_end), (0, 12));

    let crlf = split_capture_draft("@dev#\"Release Ideas\"\r\n- Big idea");
    let first = list_of(&crlf.items[0]);
    assert_eq!((first.header_start, first.header_end), (0, 20));
    assert_eq!(crlf.items[0].start, 22);
}
