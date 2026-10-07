//! Managed-region and note-anatomy parser tests: every block kind
//! round-tripped against the renderer, plus hand-written mirror, tombstone,
//! and unknown-callout shapes.
use super::*;

fn highlight_annotation(
    order: usize,
    page_label: Option<&str>,
    text: &str,
    comment: Option<&str>,
) -> SidecarAnnotation {
    SidecarAnnotation {
        kind: SidecarAnnotationKind::Highlight,
        page_label: page_label.map(str::to_string),
        linked_page_style: false,
        text: text.to_string(),
        comment: comment.map(str::to_string),
        task_source: None,
        image: None,
        order,
        ordinal_on_page: 0,
    }
}

fn render_annotations(
    annotations: Vec<SidecarAnnotation>,
    note: &ParsedNote,
) -> (PathBuf, RenderedHighlights) {
    let bob_dir = temp_bob_dir("region-round-trip");
    let config = test_config_for_bob_dir(bob_dir.clone());
    let pdf = bob_dir.join("lib/papers/region.pdf");
    let ref_note = bob_dir.join("ref/papers/region.md");
    let sidecar = SidecarInput {
        path: bob_dir.join("lib/papers/region.textbundle/text.md"),
        annotations,
    };
    let rendered = super::render_sidecar_highlights(
        &config, &pdf, &ref_note, note, &sidecar, false,
    )
    .expect("render region fixtures");
    (bob_dir, rendered)
}

#[test]
fn region_round_trip_highlight_with_multiline_comment() {
    let note = ParsedNote::empty();
    let (_bob_dir, rendered) = render_annotations(
        vec![highlight_annotation(
            0,
            Some("Page 2"),
            "The first line\nThe second line",
            Some("My first note\nMy second note"),
        )],
        &note,
    );

    let parsed = super::parse_managed_region(&rendered.content);
    assert_eq!(parsed.blocks.len(), 1);
    assert!(parsed.removed.is_empty());
    assert!(parsed.unparsed.is_empty());
    let block = &parsed.blocks[0];
    assert_eq!(block.kind, super::RegionBlockKind::Highlight);
    assert_eq!(block.page_label.as_deref(), Some("Page 2"));
    assert_eq!(
        block.quote.as_deref(),
        Some("The first line The second line")
    );
    assert_eq!(
        block.comment.as_deref(),
        Some("My first note My second note")
    );
    assert_eq!(block.asset, None);
    assert_eq!(block.block_id, rendered.block_ids_by_annotation_order[&0]);
    assert!(!block.mirror);
    assert!(!block.in_preamble);
}

#[test]
fn region_round_trip_standalone_note() {
    // The renderer skips standalone notes whose text is a status/parent
    // marker list, so the fixture carries a realistic mirror first: it is
    // dropped by content, while the genuine personal note renders.
    let note = ParsedNote::empty();
    let (_bob_dir, rendered) = render_annotations(
        vec![
            highlight_annotation(0, Some("Page 1"), "A quote", None),
            SidecarAnnotation {
                kind: SidecarAnnotationKind::StandaloneNote,
                page_label: Some("Page 1".to_string()),
                linked_page_style: false,
                text: "- status: wip\n- parent: obsidian".to_string(),
                comment: None,
                task_source: None,
                image: None,
                order: 1,
                ordinal_on_page: 1,
            },
            SidecarAnnotation {
                kind: SidecarAnnotationKind::StandaloneNote,
                page_label: Some("Page 1".to_string()),
                linked_page_style: false,
                text: "A personal note".to_string(),
                comment: None,
                task_source: None,
                image: None,
                order: 2,
                ordinal_on_page: 2,
            },
        ],
        &note,
    );

    let parsed = super::parse_managed_region(&rendered.content);
    assert_eq!(parsed.blocks.len(), 2);
    assert!(parsed.unparsed.is_empty());
    let block = &parsed.blocks[1];
    assert_eq!(block.kind, super::RegionBlockKind::Note);
    assert_eq!(block.quote, None);
    assert_eq!(block.comment.as_deref(), Some("A personal note"));
    assert_eq!(block.block_id, rendered.block_ids_by_annotation_order[&2]);
    assert!(!block.mirror);
}

#[test]
fn region_round_trip_image() {
    let bob_dir = temp_bob_dir("region-image");
    let sidecar_path = bob_dir.join("lib/books/fig.textbundle/text.md");
    write_test_file(
        &bob_dir.join("lib/books/fig.textbundle/assets/figure.png"),
        "synthetic image bytes",
    );
    let config = test_config_for_bob_dir(bob_dir.clone());
    let pdf = bob_dir.join("lib/books/fig.pdf");
    let ref_note = bob_dir.join("ref/books/fig.md");
    let sidecar = SidecarInput {
        path: sidecar_path,
        annotations: vec![SidecarAnnotation {
            kind: SidecarAnnotationKind::Image,
            page_label: None,
            linked_page_style: false,
            text: String::new(),
            comment: Some("Look closer".to_string()),
            task_source: None,
            image: Some(SidecarImage {
                target: "assets/figure.png".to_string(),
                alt_text: Some("Figure".to_string()),
            }),
            order: 0,
            ordinal_on_page: 0,
        }],
    };
    let note = ParsedNote::empty();
    let rendered = super::render_sidecar_highlights(
        &config, &pdf, &ref_note, &note, &sidecar, false,
    )
    .expect("render image fixture");

    let parsed = super::parse_managed_region(&rendered.content);
    assert_eq!(parsed.blocks.len(), 1);
    assert!(parsed.unparsed.is_empty());
    let block = &parsed.blocks[0];
    assert_eq!(block.kind, super::RegionBlockKind::Image);
    assert_eq!(block.quote, None);
    assert_eq!(block.comment.as_deref(), Some("Look closer"));
    let asset = &rendered.image_assets[0];
    assert_eq!(
        block.asset.as_deref(),
        Some(super::display_path(&asset.vault_relative_dest_path).as_str())
    );
    assert_eq!(block.block_id, asset.block_id);
    assert!(!block.mirror);
    assert!(!block.in_preamble);
}

#[test]
fn region_round_trip_pages_preamble_and_tombstones() {
    let note = ParsedNote {
        frontmatter: Vec::new(),
        body: "<!-- highlights:begin -->\n\n> [!quote] Stale\n\n^h-0123456789ab\n\n<!-- highlights:end -->".to_string(),
        original: None,
    };
    let (_bob_dir, rendered) = render_annotations(
        vec![
            highlight_annotation(0, None, "Before any page", None),
            highlight_annotation(1, Some("Page 1"), "On page one", None),
            highlight_annotation(2, Some("Page 2"), "On page two", None),
        ],
        &note,
    );

    let parsed = super::parse_managed_region(&rendered.content);
    assert_eq!(parsed.blocks.len(), 3);
    assert_eq!(parsed.removed, vec!["h-0123456789ab".to_string()]);
    assert!(parsed.unparsed.is_empty());
    // Every field and every ID round-trips on every block.
    let expectations = [
        (None, "Before any page", 0),
        (Some("Page 1"), "On page one", 1),
        (Some("Page 2"), "On page two", 2),
    ];
    for (block, (page, quote, order)) in parsed.blocks.iter().zip(expectations)
    {
        assert_eq!(block.kind, super::RegionBlockKind::Highlight);
        assert_eq!(block.page_label.as_deref(), page);
        assert_eq!(block.quote.as_deref(), Some(quote));
        assert_eq!(block.comment, None);
        assert_eq!(
            block.block_id,
            rendered.block_ids_by_annotation_order[&order]
        );
        assert!(!block.mirror);
    }
    assert!(parsed.blocks[0].in_preamble);
    assert!(!parsed.blocks[1].in_preamble);
    assert!(!parsed.blocks[2].in_preamble);
}

#[test]
fn region_parses_leaked_mirror_like_ea_graph() {
    let region = "### Page 1\n\n> [!note] - status: ready\n> - parent: sase_ref\n> - url: https://arxiv.org/pdf/2608.04278\n> - title: EA-Graph: Artifact-Anchored Verification Memory for Coding Agents under Upstream Drift\n\n^h-da83ea24debb\n";
    let parsed = super::parse_managed_region(region);
    assert_eq!(parsed.blocks.len(), 1);
    assert!(parsed.unparsed.is_empty());
    let block = &parsed.blocks[0];
    assert_eq!(block.kind, super::RegionBlockKind::Note);
    assert_eq!(block.quote, None);
    assert_eq!(
        block.comment.as_deref(),
        Some("- status: ready\n- parent: sase_ref\n- url: https://arxiv.org/pdf/2608.04278\n- title: EA-Graph: Artifact-Anchored Verification Memory for Coding Agents under Upstream Drift")
    );
    assert_eq!(block.block_id, "h-da83ea24debb");
    assert_eq!(block.page_label.as_deref(), Some("Page 1"));
    assert!(block.mirror);
    assert!(!super::is_marker_mirror_text("Just a personal note"));
}

#[test]
fn region_round_trip_highlight_comment_mirror() {
    let note = ParsedNote::empty();
    let (_bob_dir, rendered) = render_annotations(
        vec![SidecarAnnotation {
            kind: SidecarAnnotationKind::Highlight,
            page_label: Some("Page 1".to_string()),
            // Plain highlights render even with a marker-shaped comment; the
            // renderer's marker-mirror skip only applies to linked-page
            // highlights and standalone notes.
            linked_page_style: false,
            text: "A quote".to_string(),
            comment: Some("- status: ready\n- parent: sase_ref".to_string()),
            task_source: None,
            image: None,
            order: 0,
            ordinal_on_page: 0,
        }],
        &note,
    );

    let parsed = super::parse_managed_region(&rendered.content);
    assert_eq!(parsed.blocks.len(), 1);
    assert!(parsed.blocks[0].mirror);
}

#[test]
fn region_unknown_callout_is_unparsed() {
    let region = "> [!summary] Something new\n\n^h-abcdef123456\n";
    let parsed = super::parse_managed_region(region);
    assert!(parsed.blocks.is_empty());
    assert_eq!(parsed.unparsed.len(), 1);
    assert!(parsed.unparsed[0].contains("^h-abcdef123456"));
}

#[test]
fn note_anatomy_full_body() {
    let body = "# Title\n\n- [x] #task #ref [[lib/papers/x.pdf]] #hide ^ref\n\nMy own note paragraph.\n\n![[lib/papers/x.mp3]]\n\n## Tasks\n\n- [ ] Do thing [[#^h-abc123def456|🔖]] [h:: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef] [created::2026-10-01]\n- [x] Done thing\n\n## Highlights\n\n<!-- highlights:begin -->\n\n### Page 1\n\n> [!quote] Something worth keeping.\n\n^h-abc123def456\n\n<!-- highlights:end -->\n";
    let parts = super::split_note_body(body);
    assert_eq!(parts.h1.as_deref(), Some("Title"));
    assert_eq!(
        parts.tracker_line.as_deref(),
        Some("- [x] #task #ref [[lib/papers/x.pdf]] #hide ^ref")
    );
    assert_eq!(
        parts.region.as_deref(),
        Some("### Page 1\n\n> [!quote] Something worth keeping.\n\n^h-abc123def456")
    );
    assert_eq!(parts.own_notes, "My own note paragraph.");
    assert_eq!(parts.tasks.len(), 2);
    assert!(!parts.tasks[0].checked);
    assert_eq!(parts.tasks[0].mark, ' ');
    assert_eq!(parts.tasks[0].text, "Do thing");
    assert_eq!(parts.tasks[0].block_id, "h-abc123def456");
    assert!(parts.tasks[1].checked);
    assert_eq!(parts.tasks[1].mark, 'x');
    assert_eq!(parts.tasks[1].text, "Done thing");
    assert_eq!(parts.tasks[1].block_id, "");
}

#[test]
fn note_anatomy_legacy_body_passes_through_verbatim() {
    let body = "# Old title\n\nSome migrated text.\n\n- [ ] a legacy task\n";
    let parts = super::split_note_body(body);
    assert_eq!(parts.region, None);
    assert_eq!(parts.own_notes, body);
    assert_eq!(parts.h1.as_deref(), Some("Old title"));
}

#[test]
fn note_anatomy_broken_markers_yield_no_region() {
    let body =
        "# Title\n\n<!-- highlights:begin -->\n\n> [!quote] Oops, no end.\n";
    let parts = super::split_note_body(body);
    assert_eq!(parts.region, None);
    assert_eq!(parts.h1.as_deref(), Some("Title"));
}
