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
        page_index: None,
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
    assert!(parsed.sections.is_empty());
    assert!(parsed.removed_sections.is_empty());
    let block = &parsed.blocks[0];
    assert_eq!(block.kind, super::RegionBlockKind::Highlight);
    assert_eq!(block.section, None);
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
fn region_parses_three_tucked_pieces_with_their_own_notes() {
    let parsed = super::parse_managed_region(
        "### Pages 6–8\n\n> [!quote|Page 6] Head words\n>\n> > [!note] Comment Head note\n>\n> > [!quote|Page 7] Middle words\n> >\n> > ![[ref/example.assets/middle.png]]\n>\n> > [!quote|Page 8] Tail words\n> >\n> > > [!note] Comment Tail note\n\n^h-ba93bec527bb\n",
    );
    assert!(parsed.unparsed.is_empty(), "{parsed:?}");
    assert_eq!(parsed.blocks.len(), 1);
    let block = &parsed.blocks[0];
    assert_eq!(block.pieces.len(), 3);
    assert_eq!(
        block
            .pieces
            .iter()
            .map(|piece| piece.page_label.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("Page 6"), Some("Page 7"), Some("Page 8")]
    );
    assert_eq!(block.pieces[0].quote, "Head words");
    assert_eq!(block.pieces[1].quote, "Middle words");
    assert_eq!(block.pieces[2].quote, "Tail words");
    assert_eq!(block.pieces[0].comment.as_deref(), Some("Head note"));
    assert_eq!(block.pieces[1].comment, None);
    assert_eq!(
        block.pieces[1].images,
        vec!["ref/example.assets/middle.png"]
    );
    assert_eq!(block.pieces[2].comment.as_deref(), Some("Tail note"));
    assert_eq!(
        block.quote.as_deref(),
        Some("Head words\n\nMiddle words\n\nTail words")
    );
    assert_eq!(block.comment.as_deref(), Some("Head note\n\nTail note"));
    assert_eq!(block.images, vec!["ref/example.assets/middle.png"]);
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
                page_index: None,
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
                page_index: None,
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
            page_index: None,
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
            page_index: None,
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

#[test]
fn region_round_trip_image_notes() {
    let bob_dir = temp_bob_dir("region-image-notes");
    let source = bob_dir.join("shot.png");
    write_test_bytes(&source, b"png-bytes");
    let missing = bob_dir.join("missing.png");
    let config = test_config_for_bob_dir(bob_dir.clone());
    let pdf = bob_dir.join("lib/papers/region.pdf");
    let ref_note = bob_dir.join("ref/papers/region.md");
    let sidecar = SidecarInput {
        path: bob_dir.join("lib/papers/region.md"),
        annotations: vec![
            highlight_annotation(
                0,
                Some("Page 1"),
                "Latency is not throughput.",
                Some(source.to_str().expect("utf8")),
            ),
            highlight_annotation(
                1,
                Some("Page 1"),
                "A missing shot.",
                Some(missing.to_str().expect("utf8")),
            ),
            SidecarAnnotation {
                kind: SidecarAnnotationKind::StandaloneNote,
                page_label: Some("Page 1".to_string()),
                page_index: None,
                linked_page_style: false,
                text: source.to_str().expect("utf8").to_string(),
                comment: None,
                task_source: None,
                image: None,
                order: 2,
                ordinal_on_page: 2,
            },
        ],
    };
    let rendered = super::render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &sidecar,
        false,
    )
    .expect("render image-note region");
    let parsed = super::parse_managed_region(&rendered.content);
    assert!(parsed.unparsed.is_empty(), "{:?}", parsed.unparsed);
    assert_eq!(parsed.blocks.len(), 3);
    assert_eq!(parsed.blocks[0].kind, super::RegionBlockKind::Highlight);
    assert_eq!(parsed.blocks[0].images.len(), 1);
    assert!(parsed.blocks[0].unresolved_images.is_empty());
    assert_eq!(parsed.blocks[0].comment, None);
    assert_eq!(parsed.blocks[1].unresolved_images.len(), 1);
    assert_eq!(
        parsed.blocks[1].unresolved_image_states,
        vec![super::UnresolvedImageNoteState::NotFound]
    );
    assert_eq!(parsed.blocks[2].kind, super::RegionBlockKind::Note);
    assert_eq!(parsed.blocks[2].images.len(), 1);
}

#[test]
fn region_existing_fixtures_have_no_image_notes() {
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
    assert!(parsed.blocks[0].images.is_empty());
    assert!(parsed.blocks[0].unresolved_images.is_empty());
}

const SECTIONED_MEMORY_HEAD: &str = "496b1ca10117";
const SECTIONED_MEMORY_TAIL: &str = "6bbab22c999c";
const SECTIONED_OTHER: &str = "38a082b00de9";
const SECTIONED_REMOVED: &str = "0123456789ab";

fn sectioned_region_with_other_and_removed() -> String {
    format!(
        "### Memory\n\
         \n\
         > [!summary|section] @memory · 2 highlights · Pages 4–5\n\
         > Everything that belongs in the blog's memory section.\n\
         \n\
         ^s-memory\n\
         \n\
         #### Page 4\n\
         \n\
         > [!quote] Make a point about how if I were building AI from scratch right now, …\n\
         \n\
         ^h-{SECTIONED_MEMORY_HEAD}\n\
         \n\
         #### Page 5\n\
         \n\
         > [!quote] Mention that you've implemented project memory every wrong way possible …\n\
         >\n\
         > > [!note] Comment Compare with the intro\n\
         \n\
         ^h-{SECTIONED_MEMORY_TAIL}\n\
         \n\
         ### Other highlights\n\
         \n\
         #### Page 2\n\
         \n\
         > [!quote] POTENTIAL QUOTES: …\n\
         \n\
         ^h-{SECTIONED_OTHER}\n\
         \n\
         ### Removed highlights\n\
         \n\
         > [!info] Removed highlight\n\
         >\n\
         > Dropped from the sidecar.\n\
         \n\
         ^h-{SECTIONED_REMOVED}\n\
         \n\
         > [!info] Removed section\n\
         >\n\
         > “Old” is no longer declared in the Highlights notes.\n\
         \n\
         ^s-old\n"
    )
}

#[test]
fn region_parses_sections_other_and_removed() {
    let region = sectioned_region_with_other_and_removed();
    let parsed = super::parse_managed_region(&region);
    assert!(parsed.unparsed.is_empty(), "{:?}", parsed.unparsed);
    assert_eq!(parsed.sections.len(), 1);
    let section = &parsed.sections[0];
    assert_eq!(section.id, "memory");
    assert_eq!(section.block_id, "s-memory");
    assert_eq!(section.title, "Memory");
    assert_eq!(section.tag, "@memory");
    assert_eq!(
        section.description,
        "Everything that belongs in the blog's memory section."
    );
    assert!(section.images.is_empty());
    assert!(section.unresolved_images.is_empty());
    assert_eq!(parsed.blocks.len(), 3);
    assert_eq!(
        parsed.blocks[0].block_id,
        format!("h-{SECTIONED_MEMORY_HEAD}")
    );
    assert_eq!(parsed.blocks[0].section.as_deref(), Some("memory"));
    assert_eq!(parsed.blocks[0].page_label.as_deref(), Some("Page 4"));
    assert_eq!(parsed.blocks[0].kind, super::RegionBlockKind::Highlight);
    assert!(!parsed.blocks[0].in_preamble);
    assert_eq!(
        parsed.blocks[1].block_id,
        format!("h-{SECTIONED_MEMORY_TAIL}")
    );
    assert_eq!(parsed.blocks[1].section.as_deref(), Some("memory"));
    assert_eq!(parsed.blocks[1].page_label.as_deref(), Some("Page 5"));
    assert_eq!(
        parsed.blocks[1].comment.as_deref(),
        Some("Compare with the intro")
    );
    assert_eq!(parsed.blocks[2].block_id, format!("h-{SECTIONED_OTHER}"));
    assert_eq!(parsed.blocks[2].section, None);
    assert_eq!(parsed.blocks[2].page_label.as_deref(), Some("Page 2"));
    assert_eq!(parsed.removed, vec![format!("h-{SECTIONED_REMOVED}")]);
    assert_eq!(parsed.removed_sections, vec!["s-old".to_string()]);
}

#[test]
fn region_parses_empty_scaffold_section() {
    let region = "### Related\n\n\
         > [!summary|section] @related · nothing tagged yet\n\
         \n\
         ^s-related\n";
    let parsed = super::parse_managed_region(region);
    assert!(parsed.unparsed.is_empty(), "{:?}", parsed.unparsed);
    assert!(parsed.blocks.is_empty());
    assert_eq!(parsed.sections.len(), 1);
    let section = &parsed.sections[0];
    assert_eq!(section.id, "related");
    assert_eq!(section.block_id, "s-related");
    assert_eq!(section.title, "Related");
    assert_eq!(section.tag, "@related");
    assert!(section.description.is_empty());
}

#[test]
fn region_parses_section_without_page_labels() {
    let region = "### Memory\n\n\
         > [!summary|section] @memory · 1 highlight\n\
         > A section with no page captions.\n\
         \n\
         ^s-memory\n\
         \n\
         > [!quote] Ungrouped by page\n\
         \n\
         ^h-aa11bb22cc33\n";
    let parsed = super::parse_managed_region(region);
    assert!(parsed.unparsed.is_empty(), "{:?}", parsed.unparsed);
    assert_eq!(parsed.blocks.len(), 1);
    assert_eq!(parsed.blocks[0].section.as_deref(), Some("memory"));
    assert_eq!(parsed.blocks[0].page_label, None);
    assert_eq!(parsed.blocks[0].quote.as_deref(), Some("Ungrouped by page"));
}

#[test]
fn region_parses_group_spanning_pages() {
    let region = "### Memory\n\n\
         > [!summary|section] @memory · 1 highlight · Pages 3–4\n\
         \n\
         ^s-memory\n\
         \n\
         #### Pages 3–4\n\
         \n\
         > [!quote] Spanning quote\n\
         \n\
         ^h-aa11bb22cc33\n";
    let parsed = super::parse_managed_region(region);
    assert!(parsed.unparsed.is_empty(), "{:?}", parsed.unparsed);
    assert_eq!(parsed.blocks.len(), 1);
    assert_eq!(parsed.blocks[0].section.as_deref(), Some("memory"));
    assert_eq!(parsed.blocks[0].page_label.as_deref(), Some("Pages 3–4"));
}

#[test]
fn region_parses_section_titled_page_turners() {
    let region = "### Page turners\n\n\
         > [!summary|section] @page-turners · 1 highlight · Page 1\n\
         \n\
         ^s-page-turners\n\
         \n\
         #### Page 1\n\
         \n\
         > [!quote] A turning point\n\
         \n\
         ^h-aa11bb22cc33\n";
    let parsed = super::parse_managed_region(region);
    assert!(parsed.unparsed.is_empty(), "{:?}", parsed.unparsed);
    assert_eq!(parsed.sections.len(), 1);
    assert_eq!(parsed.sections[0].title, "Page turners");
    assert_eq!(parsed.sections[0].id, "page-turners");
    assert_eq!(parsed.sections[0].tag, "@page-turners");
    assert_eq!(parsed.blocks.len(), 1);
    assert_eq!(parsed.blocks[0].section.as_deref(), Some("page-turners"));
    assert_eq!(parsed.blocks[0].page_label.as_deref(), Some("Page 1"));
}

#[test]
fn region_stray_non_card_h3_in_sectioned_region_is_a_page_label() {
    let region = "### Memory\n\n\
         > [!summary|section] @memory · 1 highlight · Page 1\n\
         \n\
         ^s-memory\n\
         \n\
         #### Page 1\n\
         \n\
         > [!quote] In the section\n\
         \n\
         ^h-aa11bb22cc33\n\
         \n\
         ### Page 2\n\
         \n\
         > [!quote] Stray page heading\n\
         \n\
         ^h-dd44ee55ff66\n";
    let parsed = super::parse_managed_region(region);
    assert!(parsed.unparsed.is_empty(), "{:?}", parsed.unparsed);
    assert_eq!(parsed.sections.len(), 1);
    assert_eq!(parsed.blocks.len(), 2);
    assert_eq!(parsed.blocks[0].section.as_deref(), Some("memory"));
    assert_eq!(parsed.blocks[0].page_label.as_deref(), Some("Page 1"));
    assert_eq!(parsed.blocks[1].section, None);
    assert_eq!(parsed.blocks[1].page_label.as_deref(), Some("Page 2"));
}

#[test]
fn region_sectioned_annotation_counts_ignore_cards() {
    let region = sectioned_region_with_other_and_removed();
    let parsed = super::parse_managed_region(&region);
    let annotations = parsed
        .blocks
        .iter()
        .filter(|block| !block.mirror && !block.in_preamble)
        .count();
    assert_eq!(annotations, 3);
    assert_eq!(parsed.sections.len(), 1);
    let generated = super::generated_block_ids(&region);
    assert_eq!(
        generated,
        [
            format!("h-{SECTIONED_MEMORY_HEAD}"),
            format!("h-{SECTIONED_MEMORY_TAIL}"),
            format!("h-{SECTIONED_OTHER}"),
            format!("h-{SECTIONED_REMOVED}"),
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
    );
    assert!(!generated.iter().any(|id| id.starts_with("s-")));
}

#[test]
fn region_section_card_images_and_warnings() {
    let region = "### Visuals\n\n\
         > [!summary|section] @visuals · nothing tagged yet\n\
         > A picture-first section.\n\
         > ![[ref/notes/example.assets/shot.png]]\n\
         >\n\
         > > [!warning] Image not found\n\
         > >\n\
         > > `/missing.png`\n\
         \n\
         ^s-visuals\n";
    let parsed = super::parse_managed_region(region);
    assert!(parsed.unparsed.is_empty(), "{:?}", parsed.unparsed);
    assert_eq!(parsed.sections.len(), 1);
    let section = &parsed.sections[0];
    assert_eq!(section.description, "A picture-first section.");
    assert_eq!(
        section.images,
        vec!["ref/notes/example.assets/shot.png".to_string()]
    );
    assert_eq!(section.unresolved_images, vec!["/missing.png".to_string()]);
    assert_eq!(
        section.unresolved_image_states,
        vec![super::UnresolvedImageNoteState::NotFound]
    );
}

#[test]
fn region_sectioned_preamble_is_before_first_h3() {
    let region = "> [!quote] Before any section\n\n\
         ^h-aa11bb22cc33\n\
         \n\
         ### Memory\n\
         \n\
         > [!summary|section] @memory · 1 highlight\n\
         \n\
         ^s-memory\n\
         \n\
         > [!quote] Inside\n\
         \n\
         ^h-dd44ee55ff66\n";
    let parsed = super::parse_managed_region(region);
    assert!(parsed.unparsed.is_empty(), "{:?}", parsed.unparsed);
    assert_eq!(parsed.blocks.len(), 2);
    assert!(parsed.blocks[0].in_preamble);
    assert_eq!(parsed.blocks[0].section, None);
    assert!(!parsed.blocks[1].in_preamble);
    assert_eq!(parsed.blocks[1].section.as_deref(), Some("memory"));
}

#[test]
fn region_summary_section_without_s_id_is_not_sectioned() {
    let region = "### Memory\n\n\
         > [!summary|section] @memory · nothing tagged yet\n\
         \n\
         > [!quote] Still today's stream\n\
         \n\
         ^h-abcdef123456\n";
    let parsed = super::parse_managed_region(region);
    assert!(parsed.sections.is_empty());
    assert!(parsed.blocks.is_empty());
    assert_eq!(parsed.unparsed.len(), 1);
}

#[test]
fn region_unsectioned_h4_is_not_a_page_caption() {
    let region = "### Page 1\n\n\
         #### Pages 3–4\n\
         \n\
         > [!quote] Caption-shaped but unsectioned\n\
         \n\
         ^h-abcdef123456\n";
    let parsed = super::parse_managed_region(region);
    assert!(parsed.sections.is_empty());
    assert!(parsed.blocks.is_empty());
    assert_eq!(parsed.unparsed.len(), 1);
    assert!(parsed.unparsed[0].contains("#### Pages 3–4"));
}
