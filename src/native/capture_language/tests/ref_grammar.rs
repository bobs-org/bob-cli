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
    for raw in [
        "https://example.com/post @notes",
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
    // Production callers mark forced flags as an explicit destination.
    let options = CaptureParseOptions {
        explicit_destination: true,
        ..routing_options(Some(&policy))
    };
    let forced = parse_capture_text_with_clip_control(
        "https://example.com/post",
        Some("notes"),
        None,
        &options,
    )
    .expect("forced route stays a task");
    assert!(matches!(forced.kind, CaptureKind::Task));

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

    // A `@@` declaration keeps the item a task.
    let draft =
        draft_routing("@@notes\nhttps://example.com/post", Some(&policy))
            .expect("global draft parses");
    assert_eq!(draft.items.len(), 1);
    assert!(
        matches!(draft.items[0].parsed.kind, CaptureKind::Task),
        "got {:?}",
        draft.items[0].parsed.kind
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

    // Routing off, extra text, and `@@` all stay tasks.
    assert_eq!(
        editor_routing("https://example.com/post", None).mode,
        EditorMode::Task
    );
    assert_eq!(
        editor_routing("https://example.com/post extra", Some(&policy)).mode,
        EditorMode::Task
    );
    assert_eq!(
        editor_routing("@@notes\nhttps://example.com/post", Some(&policy))
            .items[0]
            .mode,
        EditorMode::Task
    );
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
