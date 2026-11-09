//! Notebook-draft-language claim tests.
//!
//! List, global, and forced composition, `#section^id`, teaching refusals,
//! `%c`/`%l`/`%n`/`%t`, and ordinary-route named bullets. Fail-closed
//! execution stays with the CLI suite until the writer phase.

use super::super::draft::TemplateCodedError;
use super::*;
use crate::native::capture_language::notebook::{
    candidate_notebook_routes, NotebookRouteInfo, NotebookRoutePolicy,
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

fn strict(raw: &str) -> Result<ParsedCaptureText, String> {
    let policy = policy();
    parse_capture_text_with_options(
        raw,
        None,
        None,
        &strict_options(Some(&policy)),
    )
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

fn draft_coded(raw: &str) -> Result<ParsedCaptureDraft, TemplateCodedError> {
    let policy = policy();
    parse_capture_draft_with_options_coded(
        raw,
        None,
        None,
        &strict_options(Some(&policy)),
    )
}

fn draft_code(raw: &str) -> &'static str {
    draft_coded(raw)
        .expect_err("notebook refusal")
        .code
        .expect("coded notebook refusal")
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

fn assert_note(
    raw: &str,
    body: &str,
    section: Option<&str>,
    exact: bool,
    block_id: Option<&str>,
) {
    let parsed = strict(raw).unwrap_or_else(|error| panic!("{raw}: {error}"));
    let CaptureKind::NotebookNote {
        section: got_section,
        exact_section,
        block_id: got_id,
    } = &parsed.kind
    else {
        panic!("{raw}: not a notebook note: {:?}", parsed.kind);
    };
    assert_eq!(parsed.body, body, "{raw}");
    assert_eq!(parsed.route.as_deref(), Some("dev"), "{raw}");
    assert_eq!(got_section.as_deref(), section, "{raw}");
    assert_eq!(*exact_section, exact, "{raw}");
    assert_eq!(got_id.as_deref(), block_id, "{raw}");
    let editor = editor_parse(raw);
    assert_eq!(editor.mode, EditorMode::NotebookNote, "{raw}");
    assert_eq!(editor.body, body, "{raw}");
    assert_eq!(editor.route.as_deref(), Some("dev"), "{raw}");
    assert_eq!(editor.section.as_deref(), section, "{raw}");
    assert_eq!(editor.block_id.as_deref(), block_id, "{raw}");
    assert!(
        editor.diagnostics.is_empty(),
        "{raw}: {:?}",
        editor.diagnostics
    );
}

fn assert_sub_note(raw: &str, body: &str, parent: &str) {
    let parsed = strict(raw).unwrap_or_else(|error| panic!("{raw}: {error}"));
    let CaptureKind::NotebookSubNote { parent_block_id } = &parsed.kind else {
        panic!("{raw}: not a notebook sub-note: {:?}", parsed.kind);
    };
    assert_eq!(parsed.body, body, "{raw}");
    assert_eq!(parent_block_id, parent, "{raw}");
    let editor = editor_parse(raw);
    assert_eq!(editor.mode, EditorMode::NotebookSubNote, "{raw}");
    assert_eq!(editor.body, body, "{raw}");
    assert_eq!(editor.block_id.as_deref(), Some(parent), "{raw}");
}

fn assert_code(raw: &str, code: &str) {
    assert_eq!(draft_code(raw), code, "{raw}");
    let editor = editor_parse(raw);
    assert_eq!(
        editor.diagnostics.first().map(|diagnostic| diagnostic.code),
        Some(code),
        "{raw}: {:?}",
        editor.diagnostics
    );
}

#[test]
fn grammar_revision_is_twenty_three() {
    assert_eq!(GRAMMAR_REVISION, 23);
}

#[test]
fn individual_forms_claim_notes_and_sub_notes() {
    assert_note("A note @dev", "A note", None, false, None);
    assert_note("@dev A note", "A note", None, false, None);
    assert_note(
        "Keep `#task` as prose @dev#Ideas",
        "Keep `#task` as prose",
        Some("Ideas"),
        false,
        None,
    );
    assert_note(
        "An authored note @dev^idea-id",
        "An authored note",
        None,
        false,
        Some("idea-id"),
    );
    assert_sub_note("Child note @dev+parent-id", "Child note", "parent-id");
}

#[test]
fn section_plus_id_splits_unquoted_and_quoted_carets() {
    assert_note(
        "Try mergiraf @dev#git^mergiraf",
        "Try mergiraf",
        Some("git"),
        false,
        Some("mergiraf"),
    );
    assert_note(
        "@dev#git^mergiraf Try mergiraf",
        "Try mergiraf",
        Some("git"),
        false,
        Some("mergiraf"),
    );
    assert_note(
        r#"Keep @dev#"Title^literal""#,
        "Keep",
        Some("Title^literal"),
        true,
        None,
    );
    assert_note(
        r#"Keep @dev#"Title"^nid"#,
        "Keep",
        Some("Title"),
        true,
        Some("nid"),
    );
    assert_note(
        r#"@dev#"Title"^nid Keep"#,
        "Keep",
        Some("Title"),
        true,
        Some("nid"),
    );
}

#[test]
fn incomplete_section_and_id_refuse_instead_of_falling_back() {
    assert_eq!(draft_code("Keep @dev#^id"), "notebook_section_required");
    assert_eq!(draft_code("Keep @dev^"), "notebook_block_id_required");
    assert_eq!(draft_code(r#"Keep @dev#""#), "notebook_section_required");
    let dangling = editor_parse("Keep @dev^");
    assert_eq!(dangling.mode, EditorMode::NotebookNote);
    assert!(dangling.needs.contains(&Need::BlockId), "{dangling:?}");
    assert!(dangling.diagnostics.is_empty(), "{dangling:?}");
    let missing_section = editor_parse("Keep @dev#^id");
    assert!(
        missing_section.needs.contains(&Need::Section),
        "{missing_section:?}"
    );
    let quoted = editor_parse(r#"Keep @dev#""#);
    assert!(quoted.needs.contains(&Need::Section), "{quoted:?}");
}

#[test]
fn notebook_list_files_notes_and_strips_bullet_prefixes() {
    let parsed = draft("@dev\n- first idea\n- second idea")
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(parsed.items.len(), 2);
    for (item, body) in parsed.items.iter().zip(["first idea", "second idea"]) {
        let CaptureKind::NotebookNote {
            section,
            exact_section,
            block_id,
        } = &item.parsed.kind
        else {
            panic!(
                "list member is not a notebook note: {:?}",
                item.parsed.kind
            );
        };
        assert_eq!(item.parsed.body, body);
        assert_eq!(item.parsed.route.as_deref(), Some("dev"));
        assert!(section.is_none());
        assert!(!*exact_section);
        assert!(block_id.is_none());
        let list = item.list.as_ref().expect("list context");
        assert_eq!(list.kind, ListKind::Tasks);
        assert_eq!(list.count, 2);
    }
    let editor = editor_full("@dev\n- first idea\n- second idea");
    assert_eq!(editor.items.len(), 2);
    assert_eq!(editor.items[0].mode, EditorMode::NotebookNote);
    assert_eq!(editor.items[0].body, "first idea");
    assert_eq!(editor.items[1].body, "second idea");
}

#[test]
fn notebook_section_list_inherits_the_header_section() {
    let parsed = draft("@dev#Ideas\n- Big idea\n- Small idea")
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(parsed.items.len(), 2);
    for item in &parsed.items {
        let CaptureKind::NotebookNote { section, .. } = &item.parsed.kind
        else {
            panic!("expected notebook note: {:?}", item.parsed.kind);
        };
        assert_eq!(section.as_deref(), Some("Ideas"));
        assert_eq!(item.list.as_ref().expect("list").kind, ListKind::Notes);
    }
}

#[test]
fn empty_unsectioned_notebook_list_holds_no_tasks() {
    assert_code("@dev", "notebook_holds_no_tasks");
    assert_code("@dev\n- ", "notebook_holds_no_tasks");
}

#[test]
fn empty_notebook_notes_list_is_empty_list() {
    assert_code("@dev#Ideas", "empty_list");
    assert_code("@dev#", "empty_list");
}

#[test]
fn named_section_header_is_not_a_list() {
    let parsed = strict("@dev#git^mergiraf\n- nested detail")
        .unwrap_or_else(|error| panic!("{error}"));
    let CaptureKind::NotebookNote {
        section, block_id, ..
    } = &parsed.kind
    else {
        panic!("expected a single named note: {:?}", parsed.kind);
    };
    assert_eq!(section.as_deref(), Some("git"));
    assert_eq!(block_id.as_deref(), Some("mergiraf"));
    assert_eq!(parsed.sub_bullets.len(), 1);
    assert_eq!(parsed.sub_bullets[0].body, "nested detail");
    let editor = editor_parse("@dev#git^mergiraf\n- nested detail");
    assert_eq!(editor.mode, EditorMode::NotebookNote);
    assert_eq!(editor.sub_bullets.len(), 1);
}

#[test]
fn plus_id_with_bullets_fans_out_sub_notes() {
    let parsed = draft("@dev+parent-id\n- child a\n- child b")
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(parsed.items.len(), 2);
    for (index, (item, body)) in
        parsed.items.iter().zip(["child a", "child b"]).enumerate()
    {
        let CaptureKind::NotebookSubNote { parent_block_id } =
            &item.parsed.kind
        else {
            panic!("expected sub-note: {:?}", item.parsed.kind);
        };
        assert_eq!(parent_block_id, "parent-id");
        assert_eq!(item.parsed.body, body);
        let fanout = item.fanout.as_ref().expect("fan-out context");
        assert_eq!(fanout.position, index + 1);
        assert_eq!(fanout.count, 2);
        assert_eq!(fanout.marker, "@dev+parent-id");
    }
}

#[test]
fn ordinary_plus_list_does_not_fan_out() {
    let parsed = parse_capture_draft_with_options(
        "@work+parent-id\n- child a",
        None,
        None,
        &strict_options(Some(&policy())),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(parsed.items.len(), 1);
    assert!(
        !matches!(
            parsed.items[0].parsed.kind,
            CaptureKind::NotebookSubNote { .. }
        ),
        "{:?}",
        parsed.items[0].parsed.kind
    );
    assert!(parsed.items[0].fanout.is_none());
}

#[test]
fn global_destination_inherits_until_a_local_marker() {
    let inherited =
        draft("@@dev\nA shared note").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(inherited.items.len(), 1);
    assert!(matches!(
        inherited.items[0].parsed.kind,
        CaptureKind::NotebookNote { .. }
    ));
    assert_eq!(inherited.items[0].parsed.body, "A shared note");
    assert_eq!(inherited.items[0].parsed.route.as_deref(), Some("dev"));
    let editor = editor_parse("@@dev\nA shared note");
    assert_eq!(editor.mode, EditorMode::NotebookNote);
    assert_eq!(editor.body, "A shared note");

    let item_level =
        strict("A shared note @@dev").unwrap_or_else(|error| panic!("{error}"));
    assert!(matches!(item_level.kind, CaptureKind::NotebookNote { .. }));
    assert_eq!(item_level.body, "A shared note");

    let local = draft("@@dev\nBuy milk @work")
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(matches!(local.items[0].parsed.kind, CaptureKind::Task));
    assert_eq!(local.items[0].parsed.route.as_deref(), Some("work"));
}

#[test]
fn global_place_suffixes_are_invalid_destinations() {
    for token in ["@@dev#Ideas", "@@dev^id", "@@dev+id"] {
        let raw = format!("{token}\nA note");
        let error = draft_coded(&raw).expect_err("invalid global");
        assert_eq!(
            error.message,
            super::super::line::invalid_global_destination_place_error(token),
            "{raw}"
        );
        let editor = editor_full(&raw);
        assert!(
            editor
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code
                    == "invalid_global_destination"),
            "{raw}: {:?}",
            editor.diagnostics
        );
    }
}

#[test]
fn forced_route_claims_notebook_and_keeps_at_tokens_literal() {
    let policy = policy();
    let options = CaptureParseOptions {
        recognize_lists: false,
        ..strict_options(Some(&policy))
    };
    let parsed = parse_capture_text_with_options(
        "A note @work",
        Some("dev"),
        None,
        &options,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(matches!(parsed.kind, CaptureKind::NotebookNote { .. }));
    assert_eq!(parsed.body, "A note @work");
    assert_eq!(parsed.route.as_deref(), Some("dev"));

    let sectioned = parse_capture_text_with_options(
        "A note",
        Some("dev"),
        Some("Ideas"),
        &options,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let CaptureKind::NotebookNote {
        section,
        exact_section,
        ..
    } = sectioned.kind
    else {
        panic!("forced section: {:?}", sectioned.kind);
    };
    assert_eq!(section.as_deref(), Some("Ideas"));
    assert!(exact_section);

    let mut sub_options = options;
    sub_options.forced_task = Some("parent-id");
    let child = parse_capture_text_with_options(
        "Child",
        Some("dev"),
        None,
        &sub_options,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let CaptureKind::NotebookSubNote { parent_block_id } = child.kind else {
        panic!("forced sub-note: {:?}", child.kind);
    };
    assert_eq!(parent_block_id, "parent-id");

    let mut drawer_options = options;
    drawer_options.forced_drawer = Some("req");
    let error = parse_capture_draft_with_options_coded(
        "A note",
        Some("dev"),
        None,
        &drawer_options,
    )
    .expect_err("forced drawer");
    assert_eq!(error.code, Some("notebook_marker_unsupported"));
}

#[test]
fn teaching_refusals_name_the_marker() {
    assert_code("#fix Keep @dev", "notebook_marker_unsupported");
    assert_code("Keep #fix @dev", "notebook_marker_unsupported");
    assert_code("Keep @dev #fix", "notebook_marker_unsupported");
    assert_code("Keep @dev s:3", "notebook_marker_unsupported");
    assert_code("Keep @dev $2", "notebook_marker_unsupported");
    assert_code("Keep @dev %l", "notebook_marker_unsupported");
    assert_code("Keep @dev %n", "pomodoro_note_conflict");
    assert_code("Keep @dev !", "notebook_marker_unsupported");
    assert_code("Keep @dev =x", "notebook_marker_unsupported");
    assert_code("Keep @dev:abc", "notebook_holds_no_tasks");
    assert_code("Keep @dev &work:abc", "notebook_holds_no_tasks");
    assert_code("Keep @dev *work:abc", "notebook_holds_no_tasks");
    assert_code("Keep @dev ~work:abc", "notebook_holds_no_tasks");
    assert_code("Keep @dev+parent#req", "notebook_marker_unsupported");
    assert_code("Keep @dev^idea@", "notebook_project_seal");
    assert_code("@work+abc > @dev", "notebook_holds_no_tasks");
    assert_code("@dev+abc > @work", "notebook_holds_no_tasks");
    assert_code("New task < @dev+abc", "notebook_holds_no_tasks");
}

#[test]
fn url_on_a_notebook_is_a_plain_note() {
    let routing = crate::native::url_routing::UrlRoutingPolicy {
        capture: true,
        gkeep: true,
        exclude_hosts: Vec::new(),
    };
    let policy = policy();
    let options = CaptureParseOptions {
        url_routing: Some(&routing),
        ..strict_options(Some(&policy))
    };
    let parsed = parse_capture_text_with_options(
        "https://example.com/post @dev",
        None,
        None,
        &options,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        matches!(parsed.kind, CaptureKind::NotebookNote { .. }),
        "{:?}",
        parsed.kind
    );
    assert_eq!(parsed.body, "https://example.com/post");
    let editor = parse_for_editor_with(
        "https://example.com/post @dev",
        &EditorParseOptions {
            url_routing: Some(&routing),
            ..editor_options(Some(&policy))
        },
    )
    .items
    .remove(0);
    assert_eq!(editor.mode, EditorMode::NotebookNote);
    assert_eq!(editor.body, "https://example.com/post");
}

#[test]
fn clip_directive_is_recorded_on_the_note() {
    let parsed =
        strict("Keep @dev %c").unwrap_or_else(|error| panic!("{error}"));
    assert!(matches!(parsed.kind, CaptureKind::NotebookNote { .. }));
    assert_eq!(parsed.clip, Some(ClipRequest::Current));
    assert_eq!(parsed.body, "Keep");
    let editor = editor_parse("Keep @dev %c");
    assert!(editor.diagnostics.is_empty(), "{:?}", editor.diagnostics);
    assert_eq!(editor.mode, EditorMode::NotebookNote);
}

#[test]
fn template_expansion_sees_instance_notebook_routes() {
    assert_eq!(
        candidate_notebook_routes("Note {0} %t\n\n@dev hello\n"),
        vec!["dev"]
    );
    let parsed = draft("@dev {0} %t\n\nhello\n")
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(parsed.items.len(), 1);
    assert!(matches!(
        parsed.items[0].parsed.kind,
        CaptureKind::NotebookNote { .. }
    ));
    assert_eq!(parsed.items[0].parsed.body, "hello");
}

#[test]
fn ordinary_named_bullet_keeps_block_id_without_a_notebook_policy() {
    let parsed = parse_capture_text_with_options(
        "Jot @notes#time^box",
        None,
        None,
        &CaptureParseOptions::routing_off(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let CaptureKind::Bullet {
        section_prefix,
        block_id,
        exact,
    } = parsed.kind
    else {
        panic!("expected ordinary bullet: {:?}", parsed.kind);
    };
    assert_eq!(section_prefix.as_deref(), Some("time"));
    assert_eq!(block_id.as_deref(), Some("box"));
    assert!(!exact);
    let editor = parse_for_editor("Jot @notes#time^box");
    assert_eq!(editor.mode, EditorMode::Bullet);
    assert_eq!(editor.section.as_deref(), Some("time"));
    assert_eq!(editor.block_id.as_deref(), Some("box"));
}

#[test]
fn unicode_and_crlf_offsets_stay_on_char_boundaries() {
    let raw = "café @dev#git^nid";
    let editor = editor_parse(raw);
    for span in &editor.spans {
        assert!(raw.is_char_boundary(span.start), "{span:?}");
        assert!(raw.is_char_boundary(span.end), "{span:?}");
    }
    let route = editor
        .spans
        .iter()
        .find(|span| span.kind == SpanKind::NotebookRoute)
        .expect("route span");
    assert_eq!(&raw[route.start..route.end], "@dev");
    let crlf = strict("A note @dev\r\n- detail")
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(crlf.body, "A note");
    assert_eq!(crlf.sub_bullets.len(), 1);
    assert_eq!(crlf.sub_bullets[0].body, "detail");
}

#[test]
fn unknown_notebook_routes_stay_on_the_established_path() {
    let parsed =
        strict("A task @work").unwrap_or_else(|error| panic!("{error}"));
    assert!(matches!(parsed.kind, CaptureKind::Task));
    assert_eq!(parsed.route.as_deref(), Some("work"));
    let absent = parse_capture_text_with_options(
        "A note @dev",
        None,
        None,
        &strict_options(None),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(matches!(absent.kind, CaptureKind::Task));
}
