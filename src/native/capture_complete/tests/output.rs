use super::{
    super::{
        cli::build_cli,
        model::{
            Candidates, CaptureCompleteResult, Replacement, RouteCandidate,
            SCHEMA_VERSION,
        },
        pomodoros::pomodoro_name_candidates_from_entries,
        render::print_human_success_with_styler,
    },
    active_tasks::active_task_fixture,
    day_file_guard, result, with_env, write_file, TempDir,
};
use crate::native::{
    capture_language::CompletionContext, capture_pomodoros,
    capture_targets::CaptureTargetKind, style::Styler,
};
#[test]
fn build_cli_renders_without_panicking() {
    build_cli().debug_assert();
}

#[test]
fn empty_completion_has_no_context_and_a_zero_length_replacement() {
    let temp = TempDir::new("bob-cli-capture-complete-empty");
    let value = result(temp.path(), "buy milk", 4);
    assert_eq!(value.cursor, 4);
    assert_eq!(value.context, None);
    assert_eq!(value.replacement, Replacement { start: 4, end: 4 });
    assert_eq!(value.candidates.len(), 0);
}

#[test]
fn trailing_hash_fragment_requests_no_completion() {
    let temp = TempDir::new("bob-cli-capture-complete-now-tag");
    // `#now` is retired: a trailing `#n` is ordinary text with no
    // completion field, exactly like any other `#tag`.
    let raw = "Fix it #n";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, None);
    assert_eq!(value.candidates.len(), 0);
}

#[test]
fn close_items_and_suffixes_request_no_completion() {
    let _guard = day_file_guard();
    let temp = TempDir::new("bob-cli-capture-complete-close");
    let day_file = active_task_fixture(temp.path());
    // Whole-item `=x`/`=` are actions: empty success everywhere.
    for (raw, cursor) in [("=x", 2), ("=", 1), ("=x more", 3)] {
        let empty = with_env("BOB_DAY_FILE", &day_file, || {
            result(temp.path(), raw, cursor)
        });
        assert_eq!(empty.context, None, "{raw}");
        assert_eq!(empty.candidates.len(), 0, "{raw}");
    }
    // Inside a `=x` suffix there is no completion field.
    for raw in ["^sase:deep-fix=x", "Text @sase:deep-fix=x"] {
        let empty = with_env("BOB_DAY_FILE", &day_file, || {
            result(temp.path(), raw, raw.len())
        });
        assert_eq!(empty.context, None, "{raw}");
        assert_eq!(empty.candidates.len(), 0, "{raw}");
    }
    // Before the `=` the link still completes.
    let raw = "^sase:deep-fix=x";
    let link =
        with_env("BOB_DAY_FILE", &day_file, || result(temp.path(), raw, 14));
    assert_eq!(link.context, Some(CompletionContext::ActiveTask));
}

#[test]
fn close_bullet_lines_suppress_wikilink_block_but_keep_note() {
    let temp = TempDir::new("bob-cli-capture-complete-close-bullet");
    write_file(&temp.path().join("Design notes.md"), "line ^web-capture\n");
    // Note completion keeps working on a Work Log bullet line.
    let note = result(temp.path(), "=x\n- 1 see [[Design", 17);
    assert_eq!(note.context, Some(CompletionContext::WikilinkNote));
    assert_ne!(note.candidates.len(), 0);
    // Note completion keeps working inside an inline entry too.
    let inline_note = result(temp.path(), "=x see [[Design", 15);
    assert_eq!(inline_note.context, Some(CompletionContext::WikilinkNote));
    assert_ne!(inline_note.candidates.len(), 0);
    // Block completion is suppressed on a Work Log bullet line, on a
    // plain close and on a chain close alike, and inside inline entries.
    for raw in [
        "=x\n- 1 see [[Design notes#^web",
        "=x =\n- 1 see [[Design notes#^web",
        "=x see [[Design notes#^web",
    ] {
        let block = result(temp.path(), raw, raw.len());
        assert_eq!(block.context, None, "{raw}");
        assert_eq!(block.candidates.len(), 0, "{raw}");
    }
    // Named-start completion on a trail token keeps working because the
    // owner's line ends at the entry.
    let raw = "=x wired it =#bu";
    let trail = result(temp.path(), raw, raw.len());
    assert_ne!(trail.context, None, "{raw}");
    // Marker completion (including `@@`) stays suppressed on bullet
    // lines and inline entries: entry text is literal.
    for raw in ["=x\n- 1 @@", "=x @@"] {
        let markers = result(temp.path(), raw, raw.len());
        assert_eq!(markers.context, None, "{raw}");
        assert_eq!(markers.candidates.len(), 0, "{raw}");
    }
}

#[test]
fn wikilink_note_completion_returns_alias_metadata_and_cursor_after() {
    let temp = TempDir::new("bob-cli-capture-complete-link-note");
    write_file(
        &temp.path().join("Artificial Intelligence.md"),
        "---\naliases: [AI]\n---\n",
    );

    let value = result(temp.path(), "[[AI", 4);
    assert_eq!(value.context, Some(CompletionContext::WikilinkNote));
    assert_eq!(value.replacement, Replacement { start: 2, end: 4 });
    let Candidates::WikilinkNote(notes) = &value.candidates else {
        panic!("expected wikilink note candidates");
    };
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].replacement, "Artificial Intelligence|AI]]");
    assert_eq!(notes[0].cursor_after, 30);
    assert_eq!(notes[0].path, "Artificial Intelligence.md");
    assert_eq!(notes[0].alias.as_deref(), Some("AI"));
}

#[test]
fn wikilink_completion_takes_precedence_over_marker_text_inside_link() {
    let temp = TempDir::new("bob-cli-capture-complete-link-precedence");
    write_file(&temp.path().join("Project Dev.md"), "");

    let value = result(temp.path(), "[[Project @d", 11);
    assert_eq!(value.context, Some(CompletionContext::WikilinkNote));
    assert_eq!(value.replacement, Replacement { start: 2, end: 12 });
}

#[test]
fn wikilink_same_note_heading_uses_capture_route_then_inbox_fallback() {
    let temp = TempDir::new("bob-cli-capture-complete-link-heading");
    write_file(&temp.path().join("sase.md"), "# Design\n");
    write_file(&temp.path().join("mac_inbox.md"), "# Inbox\n");

    let routed = result(temp.path(), "@sase task [[#De", 16);
    assert_eq!(routed.context, Some(CompletionContext::WikilinkHeading));
    let Candidates::WikilinkHeading(headings) = &routed.candidates else {
        panic!("expected heading candidates");
    };
    assert_eq!(headings[0].replacement, "Design]]");
    assert_eq!(headings[0].path, "sase.md");

    let fallback = result(temp.path(), "[[#In", 5);
    let Candidates::WikilinkHeading(headings) = &fallback.candidates else {
        panic!("expected heading candidates");
    };
    assert_eq!(headings[0].path, "mac_inbox.md");
}

#[test]
fn wikilink_same_note_heading_uses_the_cursor_item_route() {
    let temp = TempDir::new("bob-cli-capture-complete-batch-link-heading");
    write_file(&temp.path().join("work.md"), "# Work\n");
    write_file(&temp.path().join("sase.md"), "# Design\n");

    let draft = "@work first\n\n@sase second [[#De";
    let value = result(temp.path(), draft, draft.len());
    assert_eq!(value.context, Some(CompletionContext::WikilinkHeading));
    let Candidates::WikilinkHeading(headings) = &value.candidates else {
        panic!("expected heading candidates");
    };
    assert_eq!(headings[0].replacement, "Design]]");
    assert_eq!(headings[0].path, "sase.md");
}

#[test]
fn wikilink_completion_surfaces_bounded_index_warnings() {
    let temp = TempDir::new("bob-cli-capture-complete-link-warnings");
    write_file(&temp.path().join("Good.md"), "");
    write_file(&temp.path().join("Bad.md"), "---\naliases: [\n---\n");

    let value = result(temp.path(), "[[G", 3);
    assert_eq!(value.context, Some(CompletionContext::WikilinkNote));
    assert_eq!(value.warnings.len(), 1);
    assert!(value.warnings[0].contains("parse aliases in Bad.md"));
}

#[test]
fn json_shape_is_stable() {
    let scan =
        capture_pomodoros::scan("## Pomodoros\n- [ ] (1205-1230) — MEMORY\n");
    let pomodoro_name =
        pomodoro_name_candidates_from_entries(&scan.entries, "mem").remove(0);
    let pomodoro_json = serde_json::to_value(CaptureCompleteResult {
        ok: true,
        schema_version: SCHEMA_VERSION,
        cursor: 10,
        replacement: Replacement { start: 9, end: 10 },
        context: Some(CompletionContext::PomodoroName),
        candidates: Candidates::PomodoroName(vec![pomodoro_name]),
        block_id: None,
        warnings: Vec::new(),
        query: None,
        owner: None,
        picker: None,
    })
    .expect("pomodoro json");

    assert_eq!(pomodoro_json["context"], "pomodoro_name");
    assert_eq!(pomodoro_json["candidates"][0]["replacement"], "memory");
    assert_eq!(pomodoro_json["candidates"][0]["name"], "MEMORY");
    assert_eq!(pomodoro_json["candidates"][0]["requires_name"], false);
    assert!(pomodoro_json["candidates"][0]
        .get("creates_pomodoro")
        .is_none());
    assert_eq!(pomodoro_json["candidates"][0]["line"], 2);
    assert_eq!(pomodoro_json["candidates"][0]["state"], "open");
    assert_eq!(pomodoro_json["candidates"][0]["status_symbol"], " ");
    assert_eq!(pomodoro_json["candidates"][0]["time_range"], "1205-1230");
    assert_eq!(pomodoro_json["candidates"][0]["placeholder"], false);
    assert_eq!(pomodoro_json["candidates"][0]["is_current"], true);
    assert_eq!(pomodoro_json["candidates"][0]["child_count"], 0);
    assert_eq!(pomodoro_json["candidates"][0]["match_count"], 1);
    assert!(pomodoro_json["candidates"][0]["ref"]
        .as_str()
        .expect("ref")
        .contains(':'));

    let value = serde_json::to_value(CaptureCompleteResult {
        ok: true,
        schema_version: SCHEMA_VERSION,
        cursor: 3,
        replacement: Replacement { start: 1, end: 3 },
        context: Some(CompletionContext::Route),
        candidates: Candidates::Route(vec![RouteCandidate {
            replacement: "cash".to_string(),
            route: "cash".to_string(),
            label: "cash.md".to_string(),
            kind: CaptureTargetKind::Area,
            status: None,
        }]),
        block_id: None,
        warnings: Vec::new(),
        query: None,
        owner: None,
        picker: None,
    })
    .expect("json");

    assert_eq!(value["ok"], true);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["cursor"], 3);
    assert_eq!(value["replacement"]["start"], 1);
    assert_eq!(value["replacement"]["end"], 3);
    assert_eq!(value["context"], "route");
    assert_eq!(value["candidates"][0]["replacement"], "cash");
    assert_eq!(value["candidates"][0]["route"], "cash");
    assert_eq!(value["candidates"][0]["kind"], "area");
    assert!(value["candidates"][0]["status"].is_null());
}

#[test]
fn empty_json_context_is_null() {
    let value =
        serde_json::to_value(CaptureCompleteResult::empty(4)).expect("json");
    assert!(value["context"].is_null());
    assert_eq!(value["candidates"], serde_json::json!([]));
}

#[test]
fn human_output_is_plain_without_color() {
    let styler = Styler::plain();
    assert!(!styler.is_color());
    print_human_success_with_styler(&CaptureCompleteResult::empty(0), &styler);
    print_human_success_with_styler(
        &CaptureCompleteResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            cursor: 3,
            replacement: Replacement { start: 1, end: 3 },
            context: Some(CompletionContext::Route),
            candidates: Candidates::Route(vec![RouteCandidate {
                replacement: "cash".to_string(),
                route: "cash".to_string(),
                label: "cash.md".to_string(),
                kind: CaptureTargetKind::Area,
                status: None,
            }]),
            block_id: None,
            warnings: Vec::new(),
            query: None,
            owner: None,
            picker: None,
        },
        &styler,
    );
}
