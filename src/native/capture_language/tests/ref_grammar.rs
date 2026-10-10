//! Reference-item claim and URL-list split tests (phase grammar).
//!
//! The language layer can produce reference items when routing is on,
//! while every routing-off caller keeps today's task behavior. Only the
//! lexical URL-list split is live without routing.

use super::super::editor_model::*;
use super::super::model::*;
use super::*;
use crate::native::url_routing::UrlRoutingPolicy;

fn open_policy() -> UrlRoutingPolicy {
    UrlRoutingPolicy {
        capture: true,
        gkeep: true,
        exclude_hosts: Vec::new(),
    }
}

fn routing_options(
    policy: Option<&UrlRoutingPolicy>,
) -> CaptureParseOptions<'_> {
    CaptureParseOptions {
        parse_clip_markers: true,
        url_routing: policy,
        explicit_destination: false,
        has_global_destination: false,
    }
}

fn execute_routing(
    raw: &str,
    policy: Option<&UrlRoutingPolicy>,
) -> Result<ParsedCaptureText, String> {
    parse_capture_text_with_clip_control(
        raw,
        None,
        None,
        &routing_options(policy),
    )
}

fn draft_routing(
    raw: &str,
    policy: Option<&UrlRoutingPolicy>,
) -> Result<ParsedCaptureDraft, String> {
    parse_capture_draft_with_clip_control(
        raw,
        None,
        None,
        &routing_options(policy),
    )
}

fn editor_routing(raw: &str, policy: Option<&UrlRoutingPolicy>) -> EditorParse {
    parse_for_editor_with(
        raw,
        &EditorParseOptions {
            url_routing: policy,
            has_global_destination: false,
        },
    )
}

fn assert_task(raw: &str, policy: Option<&UrlRoutingPolicy>) {
    let parsed = execute_routing(raw, policy)
        .unwrap_or_else(|error| panic!("{raw}: {error}"));
    assert!(
        matches!(parsed.kind, CaptureKind::Task),
        "{raw}: expected a task, got {:?}",
        parsed.kind
    );
}

fn assert_not_ref(raw: &str, policy: Option<&UrlRoutingPolicy>) {
    match execute_routing(raw, policy) {
        Ok(parsed) => assert!(
            !matches!(parsed.kind, CaptureKind::Ref(_)),
            "{raw}: must not claim a reference item, got {:?}",
            parsed.kind
        ),
        Err(_) => {}
    }
}

#[test]
fn bare_url_claims_reference_item_when_routing_is_on() {
    let policy = open_policy();
    let parsed = execute_routing("https://example.com/post", Some(&policy))
        .expect("bare URL claims");
    let CaptureKind::Ref(intent) = &parsed.kind else {
        panic!("expected a reference item, got {:?}", parsed.kind);
    };
    assert_eq!(parsed.body, "https://example.com/post");
    assert_eq!(intent.cleaned, "https://example.com/post");
    assert_eq!(intent.display, "example.com/post");
    assert!(parsed.route.is_none());
    assert!(parsed.sub_bullets.is_empty());
}

#[test]
fn claim_accepts_wrapper_case_and_hints() {
    let policy = open_policy();
    for raw in [
        "HTTPS://Example.com/Post",
        "<https://x.org/a>",
        "https://example.com/paper.pdf",
        "https://arxiv.org/abs/2602.16844v2",
    ] {
        let parsed = execute_routing(raw, Some(&policy))
            .unwrap_or_else(|error| panic!("{raw}: {error}"));
        assert!(
            matches!(parsed.kind, CaptureKind::Ref(_)),
            "{raw}: expected a reference item, got {:?}",
            parsed.kind
        );
        assert_eq!(parsed.body, raw);
    }
    // The `<>` wrapper stays in the body: it is the original token.
    let wrapped = execute_routing("<https://x.org/a>", Some(&policy))
        .expect("wrapped URL claims");
    assert_eq!(wrapped.body, "<https://x.org/a>");
}

#[test]
fn routing_off_keeps_bare_url_a_task() {
    let policy = open_policy();
    let routed = execute_routing("https://example.com/post", Some(&policy))
        .expect("routing on claims");
    assert!(matches!(routed.kind, CaptureKind::Ref(_)));
    let plain = execute("https://example.com/post").expect("routing off");
    assert!(matches!(plain.kind, CaptureKind::Task));
    assert_eq!(plain.body, "https://example.com/post");
    let explicit_none =
        execute_routing("https://example.com/post", None).expect("no policy");
    assert!(matches!(explicit_none.kind, CaptureKind::Task));
}

#[test]
fn claim_off_cases_stay_tasks() {
    let policy = open_policy();
    // `URL @route` now claims a reference (explicit parent); it no longer
    // stays a task.
    for raw in [
        "https://example.com/post s:3",
        "https://example.com/post p:1",
        "https://example.com/post %",
        "https://example.com/post is worth reading",
        "https://example.com/post https://example.org/other",
        "[post](https://example.com/post)",
        "http://go/x",
        "http://localhost:8080/a",
        "http://10.0.0.1/a",
        "ftp://example.com/a",
        "example.com",
    ] {
        assert_task(raw, Some(&policy));
    }
    // Marker-shaped text keeps today's errors, never a reference item.
    for raw in [
        "https://example.com/post #Ideas",
        "https://example.com/post &notes:abc",
        "",
    ] {
        assert_not_ref(raw, Some(&policy));
    }
    assert!(execute_routing("", Some(&policy)).is_err());
}

#[test]
fn excluded_hosts_stay_tasks() {
    let defaults = UrlRoutingPolicy::default();
    assert_task("https://youtube.com/watch?v=1", Some(&defaults));
    assert_task("https://m.youtube.com/watch?v=1", Some(&defaults));
    // The same host claims when nothing excludes it.
    let open = open_policy();
    let parsed = execute_routing("https://youtube.com/watch?v=1", Some(&open))
        .expect("open policy claims");
    assert!(matches!(parsed.kind, CaptureKind::Ref(_)));
}

#[test]
fn forced_destination_global_and_clip_keep_tasks() {
    let policy = open_policy();
    // Non-route forced flags still keep the item a task.
    let options = CaptureParseOptions {
        explicit_destination: true,
        ..routing_options(Some(&policy))
    };
    let forced_blocked = parse_capture_text_with_clip_control(
        "https://example.com/post",
        Some("notes"),
        None,
        &options,
    )
    .expect("blocked forced route stays a task");
    assert!(matches!(forced_blocked.kind, CaptureKind::Task));

    let explicit = CaptureParseOptions {
        explicit_destination: true,
        ..routing_options(Some(&policy))
    };
    let clipped = parse_capture_text_with_clip_control(
        "https://example.com/post",
        None,
        None,
        &explicit,
    )
    .expect("explicit destination stays a task");
    assert!(matches!(clipped.kind, CaptureKind::Task));

    // A forced `-r` route alone claims an explicit-parent reference.
    let forced_ref = parse_capture_text_with_clip_control(
        "https://example.com/post",
        Some("notes"),
        None,
        &routing_options(Some(&policy)),
    )
    .expect("forced -r claims");
    assert!(matches!(forced_ref.kind, CaptureKind::Ref(_)));
    assert_eq!(forced_ref.route.as_deref(), Some("notes"));
    assert_eq!(forced_ref.body, "https://example.com/post");

    // A plain `@@notes` declaration routes a default ref globally.
    let draft =
        draft_routing("@@notes\nhttps://example.com/post", Some(&policy))
            .expect("global draft parses");
    assert_eq!(draft.items.len(), 1);
    assert!(
        matches!(draft.items[0].parsed.kind, CaptureKind::Ref(_)),
        "got {:?}",
        draft.items[0].parsed.kind
    );
    assert_eq!(draft.items[0].parsed.route.as_deref(), Some("notes"));
    assert_eq!(draft.items[0].ref_source, Some(RefParentSource::Global));

    // `@@notes+block-id` keeps task/sub-bullet semantics (never a ref).
    let task_global =
        draft_routing("@@notes+abc\nhttps://example.com/post", Some(&policy))
            .expect("task global parses");
    assert!(
        !matches!(task_global.items[0].parsed.kind, CaptureKind::Ref(_)),
        "got {:?}",
        task_global.items[0].parsed.kind
    );

    // A child bullet keeps the item a task.
    let child = draft_routing(
        "https://example.com/post\n- why it matters",
        Some(&policy),
    )
    .expect("child draft parses");
    assert_eq!(child.items.len(), 1);
    assert!(matches!(child.items[0].parsed.kind, CaptureKind::Task));
}

#[test]
fn url_list_block_splits_into_one_item_per_line() {
    let raw = "https://a.example/1\nhttps://b.example/2";
    let draft = split_capture_draft(raw);
    assert_eq!(draft.items.len(), 2);
    assert_eq!(
        &raw[draft.items[0].start..draft.items[0].end],
        "https://a.example/1"
    );
    assert_eq!(
        &raw[draft.items[1].start..draft.items[1].end],
        "https://b.example/2"
    );
    assert_eq!((draft.items[0].line_start, draft.items[0].line_end), (1, 1));
    assert_eq!((draft.items[1].line_start, draft.items[1].line_end), (2, 2));
    assert_eq!(draft.items[1].start, "https://a.example/1".len() + 1);

    // Five URLs split the same way.
    let five = (1..=5)
        .map(|n| format!("https://x.example/{n}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(split_capture_draft(&five).items.len(), 5);

    // Each split item classifies on its own.
    let policy = open_policy();
    let routed = draft_routing(raw, Some(&policy)).expect("list drafts parse");
    assert_eq!(routed.items.len(), 2);
    for item in &routed.items {
        assert!(
            matches!(item.parsed.kind, CaptureKind::Ref(_)),
            "got {:?}",
            item.parsed.kind
        );
    }
    let plain = draft_routing(raw, None).expect("routing-off list parses");
    for item in &plain.items {
        assert!(matches!(item.parsed.kind, CaptureKind::Task));
    }
}

#[test]
fn mixed_list_classifies_each_line_on_its_own() {
    let defaults = UrlRoutingPolicy::default();
    let raw = "https://example.com/a\nhttps://youtube.com/watch?v=1";
    let routed =
        draft_routing(raw, Some(&defaults)).expect("mixed list parses");
    assert_eq!(routed.items.len(), 2);
    assert!(matches!(routed.items[0].parsed.kind, CaptureKind::Ref(_)));
    assert!(matches!(routed.items[1].parsed.kind, CaptureKind::Task));
}

#[test]
fn url_list_requires_every_line() {
    // One non-URL line keeps today's shape: a single item whose child is
    // invalid.
    let raw = "https://a.example/1\nnot a url";
    assert_eq!(split_capture_draft(raw).items.len(), 1);
    let error = draft_routing(raw, None).expect_err("child line is invalid");
    assert!(error.contains("column-zero bullet"), "{error}");

    // A lone URL is one item, never a list.
    assert_eq!(split_capture_draft("https://a.example/1").items.len(), 1);

    // A blank line still separates items the usual way.
    let blank = "https://a.example/1\n\nhttps://b.example/2";
    assert_eq!(split_capture_draft(blank).items.len(), 2);

    // An indented line is not a list line.
    let indented = "https://a.example/1\n  https://b.example/2";
    assert_eq!(split_capture_draft(indented).items.len(), 1);

    // `<>`-wrapped lines split.
    let wrapped = "<https://a.example/1>\n<https://b.example/2>";
    let split = split_capture_draft(wrapped);
    assert_eq!(split.items.len(), 2);
    assert_eq!(
        &wrapped[split.items[0].start..split.items[0].end],
        "<https://a.example/1>"
    );
}

#[test]
fn url_list_split_handles_crlf_with_exact_ranges() {
    let raw = "https://a.example/1\r\nhttps://b.example/2";
    let split = split_capture_draft(raw);
    assert_eq!(split.items.len(), 2);
    assert_eq!(
        &raw[split.items[0].start..split.items[0].end],
        "https://a.example/1"
    );
    assert_eq!(
        &raw[split.items[1].start..split.items[1].end],
        "https://b.example/2"
    );
}

#[test]
fn editor_reports_ref_mode_with_one_exact_span() {
    let policy = open_policy();
    let parse = editor_routing("https://example.com/post", Some(&policy));
    assert_eq!(parse.mode, EditorMode::Ref);
    assert_eq!(parse.body, "https://example.com/post");
    assert!(parse.route.is_none());
    assert_eq!(
        parse
            .spans
            .iter()
            .map(|span| (span.start, span.end, span.kind))
            .collect::<Vec<_>>(),
        vec![(0, "https://example.com/post".len(), SpanKind::RefUrl)]
    );
    assert_eq!(parse.items.len(), 1);
    assert_eq!(parse.items[0].mode, EditorMode::Ref);

    // The span covers the whole token, `<>` included.
    let wrapped = editor_routing("<https://x.org/a>", Some(&policy));
    assert_eq!(wrapped.mode, EditorMode::Ref);
    assert_eq!(
        wrapped
            .spans
            .iter()
            .map(|span| (span.start, span.end, span.kind))
            .collect::<Vec<_>>(),
        vec![(0, "<https://x.org/a>".len(), SpanKind::RefUrl)]
    );

    // Routing off and extra text stay tasks; a plain `@@` routes the
    // ref globally instead.
    assert_eq!(
        editor_routing("https://example.com/post", None).mode,
        EditorMode::Task
    );
    assert_eq!(
        editor_routing("https://example.com/post extra", Some(&policy)).mode,
        EditorMode::Task
    );
    let global =
        editor_routing("@@notes\nhttps://example.com/post", Some(&policy));
    assert_eq!(global.items[0].mode, EditorMode::Ref);
    assert_eq!(global.items[0].route.as_deref(), Some("notes"));
}

#[test]
fn editor_url_list_items_each_report_ref() {
    let policy = open_policy();
    let parse = editor_routing(
        "https://a.example/1\nhttps://b.example/2",
        Some(&policy),
    );
    assert_eq!(parse.items.len(), 2);
    for item in &parse.items {
        assert_eq!(item.mode, EditorMode::Ref);
        assert_eq!(item.spans.len(), 1);
        assert_eq!(item.spans[0].kind, SpanKind::RefUrl);
    }
    let mut ranges = parse
        .spans
        .iter()
        .map(|span| (span.start, span.end))
        .collect::<Vec<_>>();
    ranges.sort();
    assert_eq!(ranges.len(), 2);
    assert!(ranges[0].1 <= ranges[1].0, "spans must not overlap");
}

#[test]
fn ref_parent_sources_cover_explicit_global_and_default() {
    let policy = open_policy();
    // Explicit trailing route.
    let trailing =
        execute_routing("https://example.com/post @sase", Some(&policy))
            .expect("trailing claims");
    assert!(matches!(trailing.kind, CaptureKind::Ref(_)));
    assert_eq!(trailing.body, "https://example.com/post");
    assert_eq!(trailing.route.as_deref(), Some("sase"));
    // Explicit leading route.
    let leading =
        execute_routing("@sase https://example.com/post", Some(&policy))
            .expect("leading claims");
    assert!(matches!(leading.kind, CaptureKind::Ref(_)));
    assert_eq!(leading.body, "https://example.com/post");
    assert_eq!(leading.route.as_deref(), Some("sase"));
    // Draft sources: explicit stays explicit under a global.
    let draft =
        draft_routing("@@other\nhttps://example.com/post @sase", Some(&policy))
            .expect("explicit under global");
    assert_eq!(draft.items[0].parsed.route.as_deref(), Some("sase"));
    assert_eq!(draft.items[0].ref_source, Some(RefParentSource::Explicit));
    // Default with no route.
    let default = draft_routing("https://example.com/post", Some(&policy))
        .expect("default parses");
    assert_eq!(default.items[0].parsed.route, None);
    assert_eq!(default.items[0].ref_source, Some(RefParentSource::Default));
    // Extra words, child lines, and `@@route+id` keep task behavior.
    assert_task("https://example.com/post @sase extra", Some(&policy));
    let child =
        draft_routing("https://example.com/post @sase\n- why", Some(&policy))
            .expect("child parses");
    assert!(matches!(child.items[0].parsed.kind, CaptureKind::Task));
    let task_global =
        draft_routing("@@sase+abc\nhttps://example.com/post", Some(&policy))
            .expect("task global parses");
    assert!(
        !matches!(task_global.items[0].parsed.kind, CaptureKind::Ref(_)),
        "got {:?}",
        task_global.items[0].parsed.kind
    );
}

#[test]
fn editor_ref_route_spans_use_byte_offsets_including_utf8() {
    let policy = open_policy();
    // Leading route: route span then URL span, both with exact offsets.
    let raw = "@sase https://example.com/é";
    let parse = editor_routing(raw, Some(&policy));
    assert_eq!(parse.mode, EditorMode::Ref);
    assert_eq!(parse.body, "https://example.com/é");
    assert_eq!(parse.route.as_deref(), Some("sase"));
    let mut kinds: Vec<SpanKind> =
        parse.spans.iter().map(|span| span.kind).collect();
    kinds.sort_by_key(|kind| format!("{kind:?}"));
    assert!(kinds.contains(&SpanKind::RefUrl));
    assert!(kinds.contains(&SpanKind::Route));
    for span in &parse.spans {
        assert!(raw.is_char_boundary(span.start));
        assert!(raw.is_char_boundary(span.end));
        assert_eq!(
            &raw[span.start..span.end].to_string(),
            &raw[span.start..span.end]
        );
    }
    // Trailing route keeps the ordinary route span.
    let trailing =
        editor_routing("https://example.com/post @sase", Some(&policy));
    assert_eq!(trailing.mode, EditorMode::Ref);
    assert_eq!(trailing.route.as_deref(), Some("sase"));
    assert!(trailing
        .spans
        .iter()
        .any(|span| span.kind == SpanKind::Route));
    assert!(trailing
        .spans
        .iter()
        .any(|span| span.kind == SpanKind::RefUrl));
}
