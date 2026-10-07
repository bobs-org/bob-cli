//! Sidecar parse/render, assets, and text cleanup tests.
use super::*;

#[test]
fn sidecar_parser_extracts_image_annotations_and_leaves_non_images_as_notes() {
    let annotations = parse_sidecar_markdown(
        "\
## Page 7

![Figure 1](assets/figure.png)

Comment: Compare this figure with the appendix.
- #task Follow up on the figure.

---

![Diagram](assets/one.svg)
![Table](assets/two.webp)

---

![Not an image](assets/paper.pdf)
",
    );

    assert_eq!(annotations.len(), 4);
    assert_eq!(annotations[0].kind, SidecarAnnotationKind::Image);
    assert_eq!(
        annotations[0]
            .image
            .as_ref()
            .map(|image| image.target.as_str()),
        Some("assets/figure.png")
    );
    assert_eq!(
        annotations[0]
            .image
            .as_ref()
            .and_then(|image| image.alt_text.as_deref()),
        Some("Figure 1")
    );
    assert_eq!(
        annotations[0].comment.as_deref(),
        Some(
            "Compare this figure with the appendix.\n- #task Follow up on the figure."
        )
    );
    assert_eq!(annotations[1].kind, SidecarAnnotationKind::Image);
    assert_eq!(annotations[2].kind, SidecarAnnotationKind::Image);
    assert_eq!(annotations[3].kind, SidecarAnnotationKind::StandaloneNote);
    assert_eq!(annotations[3].text, "![Not an image](assets/paper.pdf)");
}

#[test]
fn render_sidecar_highlights_renders_image_assets_and_tasks() {
    let bob_dir = temp_bob_dir("image-render");
    let sidecar_path = bob_dir.join("lib/books/figures.textbundle/text.md");
    let asset_path =
        bob_dir.join("lib/books/figures.textbundle/assets/figure.png");
    write_test_file(&asset_path, "synthetic image bytes");
    let annotations = parse_sidecar_markdown(
        "\
## Page 4

- status: wip
- parent: obsidian

---

![Figure](assets/figure.png)

Comment: Compare this figure.
- #task Revisit this figure.
",
    );
    let sidecar = super::SidecarInput {
        path: sidecar_path,
        annotations,
    };
    let config = test_config_for_bob_dir(bob_dir.clone());
    let pdf = bob_dir.join("lib/books/figures.pdf");
    let ref_note = bob_dir.join("ref/books/figures.md");
    let note = super::ParsedNote::empty();

    let rendered = super::render_sidecar_highlights(
        &config, &pdf, &ref_note, &note, &sidecar,
    )
    .expect("render image selection");

    assert_eq!(rendered.count, 1);
    assert_eq!(rendered.image_count, 1);
    assert_eq!(rendered.image_assets.len(), 1);
    let image_asset = &rendered.image_assets[0];
    assert_eq!(image_asset.action, super::ImageAssetAction::Copy);
    assert_eq!(image_asset.source_path, asset_path);
    assert_eq!(
        image_asset.vault_relative_dest_path.parent(),
        Some(Path::new("ref/books/figures.assets"))
    );
    assert!(
        rendered.content.contains(&format!(
            "> [!quote] Image ![[{}]]\n",
            super::display_path(&image_asset.vault_relative_dest_path)
        )),
        "{}",
        rendered.content
    );
    assert!(
        rendered
            .content
            .contains("> > [!note] Comment Compare this figure."),
        "{}",
        rendered.content
    );
    assert!(
        rendered
            .content
            .contains(&format!("^{}\n", image_asset.block_id)),
        "{}",
        rendered.content
    );

    let candidates = super::annotation_task_candidates(
        &config,
        &ref_note,
        &pdf,
        Some(&sidecar),
        Some(&rendered),
    )
    .expect("extract image comment task");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].task_text, "#task Revisit this figure.");
    assert_eq!(candidates[0].source_block_id, image_asset.block_id);
}

#[test]
fn image_block_id_is_stable_across_asset_renames() {
    let bob_dir = temp_bob_dir("image-id");
    let config = test_config_for_bob_dir(bob_dir.clone());
    let pdf = bob_dir.join("lib/books/figures.pdf");
    let ref_note = bob_dir.join("ref/books/figures.md");
    let note = super::ParsedNote::empty();

    let first_sidecar_path = bob_dir.join("lib/books/first.textbundle/text.md");
    write_test_file(
        &bob_dir.join("lib/books/first.textbundle/assets/a.png"),
        "same image bytes",
    );
    let first_sidecar = super::SidecarInput {
        path: first_sidecar_path,
        annotations: parse_sidecar_markdown(
            "## Page 1\n\n![A](assets/a.png)\n",
        ),
    };
    let second_sidecar_path =
        bob_dir.join("lib/books/second.textbundle/text.md");
    write_test_file(
        &bob_dir.join("lib/books/second.textbundle/assets/renamed.png"),
        "same image bytes",
    );
    let second_sidecar = super::SidecarInput {
        path: second_sidecar_path,
        annotations: parse_sidecar_markdown(
            "## Page 1\n\n![A](assets/renamed.png)\n",
        ),
    };

    let first = super::render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &note,
        &first_sidecar,
    )
    .expect("render first image sidecar");
    let second = super::render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &note,
        &second_sidecar,
    )
    .expect("render renamed image sidecar");

    assert_eq!(
        first.image_assets[0].block_id,
        second.image_assets[0].block_id
    );
    assert_eq!(
        first.image_assets[0].vault_relative_dest_path,
        second.image_assets[0].vault_relative_dest_path
    );
}

#[test]
fn missing_image_asset_error_points_at_textbundle_export() {
    let bob_dir = temp_bob_dir("image-missing");
    let config = test_config_for_bob_dir(bob_dir.clone());
    let pdf = bob_dir.join("lib/books/missing.pdf");
    let ref_note = bob_dir.join("ref/books/missing.md");
    let sidecar = super::SidecarInput {
        path: bob_dir.join("lib/books/missing.textbundle/text.md"),
        annotations: parse_sidecar_markdown(
            "## Page 1\n\n![Missing](assets/missing.png)\n",
        ),
    };

    let error = super::render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &super::ParsedNote::empty(),
        &sidecar,
    )
    .expect_err("missing image asset should fail planning");
    assert!(
        error.to_string().contains("image asset not found")
            && error.to_string().contains("TextBundle"),
        "{error}"
    );
}

#[test]
fn pdf_text_artifact_cleanup_normalizes_extraction_noise() {
    assert_eq!(
        super::clean_pdf_text_artifacts(
            "A\u{00a0}\u{2007}\u{202f}B\t\tC \u{fb00}\u{fb01}\u{fb02}\u{fb03}\u{fb04}\u{fb05}\u{fb06}\u{00ad}\u{200b}\u{feff}"
        ),
        "A B C fffiflffifflftst"
    );
}

#[test]
fn beautify_annotation_text_reflows_and_dehyphenates() {
    assert_eq!(
        super::beautify_annotation_text(
            "\
Confusing latency and through-
put leads to mis-sized capa-
city plans for Marie-
Curie and soft\u{00ad}
ware.

Next paragraph keeps a
blank line.
"
        ),
        "\
Confusing latency and throughput leads to mis-sized capacity plans for Marie-Curie and software.

Next paragraph keeps a blank line."
    );
}

#[test]
fn beautify_annotation_text_preserves_list_structure() {
    assert_eq!(
        super::beautify_annotation_text(
            "\
- #task Follow the first wrapped
  continuation line.
* Keep the second item
wrapped too.
+ Plain plus item.
"
        ),
        "\
- #task Follow the first wrapped continuation line.
* Keep the second item wrapped too.
+ Plain plus item."
    );
}

#[test]
fn rendered_annotation_blocks_do_not_include_source_task_anchors() {
    let annotations = parse_sidecar_markdown(
        "\
## Page 2

- status: wip
- parent: obsidian

---

> Quote with a task comment.

- #task Review the contradiction.
- #task Review the contradiction.
- Ordinary comment bullet.

---

Note:
- #task Follow up on the standalone note.
- [x] #task Preserve accepted source checkboxes.
- Untagged standalone bullet.
",
    );
    let sidecar = super::SidecarInput {
        path: PathBuf::from("example.md"),
        annotations,
    };
    let config = test_config();
    let pdf = Path::new("/tmp/bob/lib/example.pdf");
    let ref_note = Path::new("/tmp/bob/ref/example.md");
    let note = super::ParsedNote::empty();

    let rendered = super::render_sidecar_highlights(
        &config, pdf, ref_note, &note, &sidecar,
    )
    .expect("render annotation blocks");

    assert_eq!(super::generated_block_ids(&rendered.content).len(), 2);
    assert!(!rendered.content.contains(" ^ht-"), "{}", rendered.content);
    let review_lines = rendered
        .content
        .lines()
        .filter(|line| line.contains("#task Review the contradiction."))
        .collect::<Vec<_>>();
    assert_eq!(review_lines.len(), 2, "{}", rendered.content);
    assert_eq!(
        review_lines
            .iter()
            .filter(|line| line.contains(" ^ht-"))
            .count(),
        0,
        "{}",
        rendered.content
    );
    assert!(
        rendered
            .content
            .contains("> [!note] - #task Follow up on the standalone note.\n"),
        "{}",
        rendered.content
    );
    assert!(
        rendered
            .content
            .contains("> - Untagged standalone bullet.\n"),
        "{}",
        rendered.content
    );
}

#[test]
fn render_sidecar_highlights_beautifies_callout_text() {
    let annotations = parse_sidecar_markdown(
        "\
## Page 2

- status: wip
- parent: obsidian

---

> Confusing latency and through-
> put leads to mis-sized capa-
> city plans with \u{fb01}les.

Comment: Compare this with SLO notes.
",
    );
    let sidecar = super::SidecarInput {
        path: PathBuf::from("example.md"),
        annotations,
    };
    let config = test_config();
    let pdf = Path::new("/tmp/bob/lib/example.pdf");
    let ref_note = Path::new("/tmp/bob/ref/example.md");
    let note = super::ParsedNote::empty();

    let rendered = super::render_sidecar_highlights(
        &config, pdf, ref_note, &note, &sidecar,
    )
    .expect("render beautified annotation block");

    assert!(
        rendered.content.contains(
            "> [!quote] Confusing latency and throughput leads to mis-sized capacity plans with files.\n"
        ),
        "{}",
        rendered.content
    );
    assert!(
        rendered
            .content
            .contains("> > [!note] Comment Compare this with SLO notes.\n"),
        "{}",
        rendered.content
    );
    assert_eq!(super::generated_block_ids(&rendered.content).len(), 1);
}

#[test]
fn annotation_block_id_is_stable_across_space_wrapping() {
    let config = test_config();
    let pdf = Path::new("/tmp/bob/lib/example.pdf");
    let mut first = super::SidecarAnnotation {
        kind: SidecarAnnotationKind::Highlight,
        page_label: Some("Page 2".to_string()),
        linked_page_style: true,
        text: "Stable quoted text continues across\nphysical lines."
            .to_string(),
        comment: None,
        task_source: None,
        image: None,
        order: 0,
        ordinal_on_page: 0,
    };
    let mut second = first.clone();
    second.text =
        "Stable quoted text\ncontinues across physical lines.".to_string();

    assert_eq!(
        super::annotation_block_id(&config, pdf, &first),
        super::annotation_block_id(&config, pdf, &second)
    );

    first.comment = Some("Comment changes do not affect IDs.".to_string());
    assert_eq!(
        super::annotation_block_id(&config, pdf, &first),
        super::annotation_block_id(&config, pdf, &second)
    );
}

#[test]
fn sidecar_page_heading_extracts_linked_page_label() {
    assert_eq!(
        sidecar_page_heading(
            "#### [Page 1](highlights://highlights-ref-sync#page=1)"
        )
        .as_deref(),
        Some("Page 1")
    );
    assert_eq!(sidecar_page_heading("## p. 12").as_deref(), Some("p. 12"));
    assert_eq!(sidecar_page_heading("# Systems Performance"), None);
    assert_eq!(sidecar_page_heading("##### 2026-06-03:"), None);
}

#[test]
fn marker_content_decoder_preserves_pdfdoc_line_separators() {
    let contents = lopdf::Object::String(
        b"- status: wip\n- parent: obsidian\r\n- title: Obsidian Docs\r"
            .to_vec(),
        lopdf::StringFormat::Literal,
    );

    assert_eq!(
        decode_marker_contents(&contents)
            .expect("decode literal marker contents"),
        "- status: wip\n- parent: obsidian\n- title: Obsidian Docs\n"
    );
}

#[test]
fn linked_sidecar_parser_keeps_wrapped_quotes_and_marker_mirror() {
    let annotations = parse_sidecar_markdown(
        "\
# Highlights Reference Note Sync

#### [Page 1](highlights://highlights-ref-sync#page=1)

##### 2026-06-03:

> Highlights Reference Note Sync

- status: wip
- parent: obsidian

***

#### [Page 2](highlights://highlights-ref-sync#page=2)

##### 2026-06-03:

> It only writes the PDF marker when frontmatter is the selected
source and --write-pdf is supplied.

***

#### [Page 6](highlights://highlights-ref-sync#page=6)

##### 2026-06-03:

> Comment: Compare this with SLO notes.

Some note...

***
",
    );

    assert_eq!(annotations.len(), 3);
    assert!(is_sidecar_marker_mirror(&annotations[0]));
    assert_eq!(annotations[0].page_label.as_deref(), Some("Page 1"));
    assert!(annotations[0].linked_page_style);
    assert_eq!(
        annotations[0].comment.as_deref(),
        Some("- status: wip\n- parent: obsidian")
    );

    assert_eq!(annotations[1].kind, SidecarAnnotationKind::Highlight);
    assert_eq!(annotations[1].page_label.as_deref(), Some("Page 2"));
    assert!(annotations[1].linked_page_style);
    assert_eq!(
        annotations[1].text,
        "It only writes the PDF marker when frontmatter is the selected\nsource and --write-pdf is supplied."
    );
    assert_eq!(annotations[1].comment, None);

    assert_eq!(annotations[2].page_label.as_deref(), Some("Page 6"));
    assert_eq!(annotations[2].text, "Comment: Compare this with SLO notes.");
    assert_eq!(annotations[2].comment.as_deref(), Some("Some note..."));
}

#[test]
fn linked_sidecar_parser_strips_comment_bullet_markers() {
    let annotations = parse_sidecar_markdown(
        "\
# Highlights Reference Note Sync

#### [Page 2](highlights://highlights-ref-sync#page=2)

##### 2026-06-03:

> A determinism contract keeps replayable tool calls stable.

- Support sase tool call replay?

***

#### [Page 3](highlights://highlights-ref-sync#page=3)

##### 2026-06-03:

> Multi-line bullet comments stay multiline.

- Preserve the first comment line.
- Preserve the second comment line.

***
",
    );

    assert_eq!(annotations.len(), 2);
    assert_eq!(annotations[0].page_label.as_deref(), Some("Page 2"));
    assert_eq!(
        annotations[0].text,
        "A determinism contract keeps replayable tool calls stable."
    );
    assert_eq!(
        annotations[0].comment.as_deref(),
        Some("Support sase tool call replay?")
    );

    assert_eq!(annotations[1].page_label.as_deref(), Some("Page 3"));
    assert_eq!(
        annotations[1].text,
        "Multi-line bullet comments stay multiline."
    );
    assert_eq!(
        annotations[1].comment.as_deref(),
        Some(
            "Preserve the first comment line.\nPreserve the second comment line."
        )
    );
}

#[test]
fn setext_preamble_yields_no_annotations_and_keeps_genuine_ids_stable() {
    // The bob-cli-4r shape: a setext title plus an author line with a `---`
    // underline before the first page heading. The preamble is never an
    // annotation, and the genuine blocks render byte-identically with and
    // without it (no page-ordinal or ID shift).
    let with_preamble = "\
Memory Systems for AI Agents | Steve Kinney
=====================================================

Steve Kinney
---

#### [Page 1](highlights://steve_kinney_agent_memory#page=1)

##### 2026-06-03:

- status: wip
- parent: memory_ref
- url: https://stevekinney.com/writing/agent-memory-systems

***

#### [Page 2](highlights://steve_kinney_agent_memory#page=2)

##### 2026-06-03:

> A genuine highlight worth keeping.

Comment: Keep this.
";
    let without_preamble = with_preamble
        .lines()
        .skip_while(|line| super::sidecar_page_heading_details(line).is_none())
        .collect::<Vec<_>>()
        .join("\n");

    let preamble_annotations = super::parse_sidecar_markdown(with_preamble);
    assert_eq!(preamble_annotations.len(), 2);
    assert!(
        preamble_annotations
            .iter()
            .all(|annotation| annotation.page_label.is_some()),
        "preamble must not parse as an annotation: {preamble_annotations:?}"
    );
    assert!(
        !preamble_annotations
            .iter()
            .any(|annotation| annotation.text.contains("Steve Kinney")),
        "{preamble_annotations:?}"
    );

    let config = super::test_config();
    let pdf = Path::new("/tmp/bob/lib/example.pdf");
    let ref_note = Path::new("/tmp/bob/ref/example.md");
    let render = |contents: &str| {
        let sidecar = super::SidecarInput {
            path: PathBuf::from("example.md"),
            annotations: super::parse_sidecar_markdown(contents),
        };
        super::render_sidecar_highlights(
            &config,
            pdf,
            ref_note,
            &super::ParsedNote::empty(),
            &sidecar,
        )
        .expect("render preamble sidecar")
    };
    let with_rendered = render(with_preamble);
    let without_rendered = render(&without_preamble);

    assert_eq!(with_rendered.count, 1);
    assert_eq!(with_rendered.content, without_rendered.content);
    assert_eq!(
        with_rendered.block_ids_by_annotation_order,
        without_rendered.block_ids_by_annotation_order
    );
    assert!(
        with_rendered
            .content
            .contains("A genuine highlight worth keeping."),
        "{}",
        with_rendered.content
    );
    assert!(
        !with_rendered.content.contains("[!note] - status:"),
        "marker mirror must not render: {}",
        with_rendered.content
    );
}

#[test]
fn sidecar_without_page_headings_has_no_preamble() {
    let annotations = super::parse_sidecar_markdown(
        "\
# Empty Highlights Sidecar

This placeholder reserves the sidecar fixture path for later parser tests.
",
    );

    assert_eq!(annotations.len(), 1);
    assert_eq!(annotations[0].page_label, None);
}

#[test]
fn leaked_mirror_and_preamble_blocks_drop_silently_while_genuine_tombstones() {
    let note = super::ParsedNote {
        frontmatter: Vec::new(),
        body: "\
# Example

- [/] #task #ref [[lib/example.pdf]] #hide ^ref

## Highlights

<!-- highlights:begin -->

> [!note] - status: wip
> - parent: memory_ref
> - url: https://example.com/old

^h-aaaabbbbcccc

> [!quote] Preamble relic from an old export.

^h-111122223333

### Page 1

> [!quote] A genuine highlight.

^h-ddddeeeeffff

<!-- highlights:end -->
"
        .to_string(),
        original: None,
    };
    // The new sidecar carries only the marker mirror, so nothing renders:
    // the leaked mirror and the preamble relic vanish without tombstones,
    // while the genuinely lost highlight still tombstones.
    let sidecar = super::SidecarInput {
        path: PathBuf::from("example.md"),
        annotations: super::parse_sidecar_markdown(
            "## Page 1\n\n- status: wip\n- parent: memory_ref\n",
        ),
    };
    let config = super::test_config();
    let rendered = super::render_sidecar_highlights(
        &config,
        Path::new("/tmp/bob/lib/example.pdf"),
        Path::new("/tmp/bob/ref/example.md"),
        &note,
        &sidecar,
    )
    .expect("render against leaked region");

    assert_eq!(rendered.count, 0);
    assert!(
        !rendered.content.contains("^h-aaaabbbbcccc"),
        "leaked mirror must drop without a tombstone: {}",
        rendered.content
    );
    assert!(
        !rendered.content.contains("^h-111122223333"),
        "preamble relic must drop without a tombstone: {}",
        rendered.content
    );
    assert!(
        rendered.content.contains("### Removed highlights")
            && rendered.content.contains("^h-ddddeeeeffff"),
        "genuinely lost highlight must still tombstone: {}",
        rendered.content
    );
}

#[test]
fn sidecar_quote_continuation_does_not_capture_labeled_comment() {
    let annotations = parse_sidecar_markdown(
        "\
## Page 3

> Stable quoted text.
Comment: revised comment
",
    );

    assert_eq!(annotations.len(), 1);
    assert_eq!(annotations[0].text, "Stable quoted text.");
    assert_eq!(annotations[0].comment.as_deref(), Some("revised comment"));
}

#[test]
fn simple_sidecar_unlabeled_text_after_quote_remains_comment() {
    let annotations = parse_sidecar_markdown(
        "\
## Page 3

> Stable quoted text.
Unlabeled comment
",
    );

    assert_eq!(annotations.len(), 1);
    assert!(!annotations[0].linked_page_style);
    assert_eq!(annotations[0].text, "Stable quoted text.");
    assert_eq!(annotations[0].comment.as_deref(), Some("Unlabeled comment"));
}
