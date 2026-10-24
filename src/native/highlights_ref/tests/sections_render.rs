//! Sectioned annotation rendering with stable IDs (bob-cli-bd.4).
use super::*;
use crate::native::highlights_ref::sections::*;

fn highlight(
    order: usize,
    page_label: &str,
    text: &str,
    comment: Option<&str>,
) -> SidecarAnnotation {
    SidecarAnnotation {
        kind: SidecarAnnotationKind::Highlight,
        page_label: Some(page_label.to_string()),
        page_index: None,
        linked_page_style: false,
        text: text.to_string(),
        section_heading: None,
        comment_section_heading: None,
        comment: comment.map(str::to_string),
        task_source: comment.map(str::to_string),
        image: None,
        order,
        ordinal_on_page: order,
    }
}

fn sticky(order: usize, page_label: &str, text: &str) -> SidecarAnnotation {
    SidecarAnnotation {
        kind: SidecarAnnotationKind::StandaloneNote,
        page_label: Some(page_label.to_string()),
        page_index: None,
        linked_page_style: false,
        text: text.to_string(),
        section_heading: None,
        comment_section_heading: None,
        comment: None,
        task_source: None,
        image: None,
        order,
        ordinal_on_page: order,
    }
}

fn declaration(
    order: usize,
    page_label: &str,
    heading: &str,
    description: &str,
) -> SidecarAnnotation {
    SidecarAnnotation {
        kind: SidecarAnnotationKind::StandaloneNote,
        page_label: Some(page_label.to_string()),
        page_index: None,
        linked_page_style: false,
        text: description.to_string(),
        section_heading: Some(heading.to_string()),
        comment_section_heading: None,
        comment: None,
        task_source: None,
        image: None,
        order,
        ordinal_on_page: order,
    }
}

fn render(
    bob_dir: &Path,
    pdf: &Path,
    ref_note: &Path,
    annotations: Vec<SidecarAnnotation>,
    note: &ParsedNote,
) -> RenderedHighlights {
    let config = test_config_for_bob_dir(bob_dir.to_path_buf());
    let input = SidecarInput {
        path: bob_dir.join("lib/papers/example.md"),
        annotations,
    };
    render_sidecar_highlights(&config, pdf, ref_note, note, &input, false)
        .expect("render sectioned highlights")
}

fn region_note(body: &str) -> ParsedNote {
    ParsedNote {
        frontmatter: Vec::new(),
        body: format!(
            "<!-- highlights:begin -->\n\n{body}\n<!-- highlights:end -->"
        ),
        original: None,
    }
}

fn live_ids(content: &str) -> BTreeSet<String> {
    parse_managed_region(content)
        .blocks
        .into_iter()
        .map(|block| block.block_id)
        .collect()
}

fn task_candidates(
    bob_dir: &Path,
    ref_note: &Path,
    pdf: &Path,
    annotations: &[SidecarAnnotation],
    rendered: &RenderedHighlights,
) -> Vec<AnnotationTaskCandidate> {
    let config = test_config_for_bob_dir(bob_dir.to_path_buf());
    let sidecar = SidecarInput {
        path: bob_dir.join("lib/papers/example.md"),
        annotations: annotations.to_vec(),
    };
    annotation_task_candidates(
        &config,
        ref_note,
        pdf,
        Some(&sidecar),
        Some(rendered),
        false,
    )
    .expect("extract tasks")
}

#[test]
fn diagnostic_vocabulary_strings_are_stable() {
    assert_eq!(SectionDiagnosticKind::Sections.as_str(), "sections");
    assert_eq!(
        SectionDiagnosticKind::TagUnknown.as_str(),
        "section_tag_unknown"
    );
    assert_eq!(SectionDiagnosticKind::Conflict.as_str(), "section_conflict");
    assert_eq!(
        SectionDiagnosticKind::Duplicate.as_str(),
        "section_duplicate"
    );
    assert_eq!(
        SectionDiagnosticKind::ReservedTitle.as_str(),
        "section_reserved_title"
    );
    assert_eq!(
        SectionDiagnosticKind::TitleEmpty.as_str(),
        "section_title_empty"
    );
    assert_eq!(
        SectionDiagnosticKind::DeclarationInComment.as_str(),
        "section_declaration_in_comment"
    );
    assert_eq!(
        SectionDiagnosticKind::TagOnlyNote.as_str(),
        "section_tag_only_note"
    );
    assert_eq!(
        SectionDiagnosticKind::TagOnTask.as_str(),
        "section_tag_on_task"
    );
}

#[test]
fn no_declaration_output_has_no_consumption_or_section_layout() {
    let bob_dir = temp_bob_dir("sections-no-decl");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let annotations = vec![
        highlight(
            1,
            "Page 2",
            "POTENTIAL QUOTES: a point",
            Some("Keep @chat and @agent!"),
        ),
        highlight(2, "Page 2", "second quote", Some("@memory stays visible")),
        sticky(3, "Page 3", "ordinary note @memory"),
        highlight(4, "Page 4", "task quote", Some("- #task Draft @sase")),
    ];
    let rendered = render(
        &bob_dir,
        &pdf,
        &ref_note,
        annotations.clone(),
        &ParsedNote::empty(),
    );
    assert!(rendered.sections.is_empty());
    assert!(!rendered.content.contains("[!summary|section]"));
    assert!(!rendered.content.contains("### Other highlights"));
    assert!(!rendered.content.contains("^s-"));
    // Tags stay visible without declarations.
    assert!(rendered.content.contains("@memory"));
    assert!(rendered.content.contains("@chat"));
    assert!(rendered.content.contains("### Page 2"));
    // Repeated page labels stream without duplication.
    let first_page2 = rendered.content.find("### Page 2").expect("page 2");
    assert!(rendered.content[first_page2..].contains("^h-"));
    // Report data is empty.
    assert!(rendered.section_diagnostics.is_empty());
    // Rerender is identical (no churn).
    let second = render(
        &bob_dir,
        &pdf,
        &ref_note,
        annotations,
        &region_note(&rendered.content),
    );
    assert_eq!(second.content, rendered.content);
}

#[test]
fn memory_example_sections_first_with_other_last() {
    let bob_dir = temp_bob_dir("sections-memory");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let annotations = vec![
        declaration(
            1,
            "Page 1",
            "Memory",
            "Everything that belongs in the blog's memory section.",
        ),
        highlight(2, "Page 4", "Make a point about memory", Some("@memory")),
        highlight(
            3,
            "Page 5",
            "Mention memory every wrong way",
            Some("Compare with the intro @memory"),
        ),
        highlight(4, "Page 2", "POTENTIAL QUOTES: leftover", None),
    ];
    let rendered =
        render(&bob_dir, &pdf, &ref_note, annotations, &ParsedNote::empty());
    assert_eq!(rendered.sections.len(), 1);
    assert_eq!(rendered.sections[0].title, "Memory");
    assert_eq!(rendered.sections[0].tag, "@memory");
    assert_eq!(rendered.sections[0].block_id, "s-memory");
    assert_eq!(rendered.sections[0].count, 2);
    // Section comes before Other, Removed is absent.
    let memory_pos = rendered.content.find("### Memory").expect("memory h3");
    let other_pos = rendered
        .content
        .find("### Other highlights")
        .expect("other h3");
    assert!(memory_pos < other_pos);
    assert!(!rendered.content.contains("### Removed highlights"));
    // Card caption with count and pages.
    assert!(rendered
        .content
        .contains("> [!summary|section] @memory · 2 highlights · Pages 4–5"));
    assert!(rendered.content.contains("^s-memory"));
    // Members use H4 captions, tags consumed.
    assert!(rendered.content.contains("#### Page 4"));
    assert!(rendered.content.contains("#### Page 5"));
    assert!(!rendered.content.contains("Compare with the intro @memory"));
    assert!(rendered.content.contains("Compare with the intro"));
    // Ungrouped block under Other with H4.
    let other_slice = &rendered.content[other_pos..];
    assert!(other_slice.contains("#### Page 2"));
    assert!(other_slice.contains("POTENTIAL QUOTES"));
    // Count excludes cards.
    assert_eq!(rendered.count, 3);
    // Round trip: cards and homes parse.
    let parsed = parse_managed_region(&rendered.content);
    assert_eq!(parsed.sections.len(), 1);
    assert_eq!(parsed.sections[0].title, "Memory");
    assert_eq!(parsed.sections[0].tag, "@memory");
    assert!(parsed.unparsed.is_empty());
    let sectioned: Vec<_> = parsed
        .blocks
        .iter()
        .filter(|b| b.section.as_deref() == Some("memory"))
        .collect();
    assert_eq!(sectioned.len(), 2);
    let ungrouped: Vec<_> = parsed
        .blocks
        .iter()
        .filter(|b| b.section.is_none())
        .collect();
    assert_eq!(ungrouped.len(), 1);
    // Rerender identical.
    let second = render(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![
            declaration(
                1,
                "Page 1",
                "Memory",
                "Everything that belongs in the blog's memory section.",
            ),
            highlight(
                2,
                "Page 4",
                "Make a point about memory",
                Some("@memory"),
            ),
            highlight(
                3,
                "Page 5",
                "Mention memory every wrong way",
                Some("Compare with the intro @memory"),
            ),
            highlight(4, "Page 2", "POTENTIAL QUOTES: leftover", None),
        ],
        &region_note(&rendered.content),
    );
    assert_eq!(second.content, rendered.content);
}

#[test]
fn declaration_ordering_merged_empty_and_other_omitted() {
    let bob_dir = temp_bob_dir("sections-order");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    // Two declarations sharing a key merge; empty scaffold still renders.
    let annotations = vec![
        declaration(1, "Page 1", "Memory", "First description."),
        declaration(2, "Page 9", "Memory", "Second description."),
        declaration(3, "Page 1", "Visuals", ""),
        highlight(4, "Page 4", "memory quote", Some("@memory")),
    ];
    let rendered =
        render(&bob_dir, &pdf, &ref_note, annotations, &ParsedNote::empty());
    assert_eq!(rendered.sections.len(), 2);
    assert_eq!(rendered.sections[0].title, "Memory");
    assert_eq!(rendered.sections[0].count, 1);
    assert_eq!(rendered.sections[1].title, "Visuals");
    assert_eq!(rendered.sections[1].count, 0);
    // Order follows first declaration.
    let memory_pos = rendered.content.find("### Memory").expect("memory");
    let visuals_pos = rendered.content.find("### Visuals").expect("visuals");
    assert!(memory_pos < visuals_pos);
    // Merged descriptions concatenate.
    assert!(rendered.content.contains("First description."));
    assert!(rendered.content.contains("Second description."));
    // Empty scaffold caption.
    assert!(rendered.content.contains("@visuals · nothing tagged yet"));
    // All blocks grouped, so Other is omitted.
    assert!(!rendered.content.contains("### Other highlights"));
    // Diagnostics record the merge.
    assert!(rendered
        .section_diagnostics
        .iter()
        .any(|d| d.detail.contains("merged into")));
}

#[test]
fn rejected_declarations_render_literally() {
    let bob_dir = temp_bob_dir("sections-rejected");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    // Reserved title is rejected; with no valid sections it renders as ordinary note.
    let annotations = vec![
        declaration(1, "Page 1", "Other highlights", "Reserved description."),
        sticky(2, "Page 2", "plain note"),
    ];
    let rendered =
        render(&bob_dir, &pdf, &ref_note, annotations, &ParsedNote::empty());
    assert!(rendered.sections.is_empty());
    assert!(!rendered.content.contains("[!summary|section]"));
    assert!(rendered.content.contains("\\# Other highlights"));
    assert!(rendered.content.contains("Reserved description."));
    // With a valid section, rejected still renders under Other.
    let mixed = vec![
        declaration(1, "Page 1", "Memory", "Desc."),
        declaration(2, "Page 1", "Other highlights", "Reserved."),
        highlight(3, "Page 4", "q", Some("@memory")),
    ];
    let rendered =
        render(&bob_dir, &pdf, &ref_note, mixed, &ParsedNote::empty());
    assert_eq!(rendered.sections.len(), 1);
    assert!(rendered.content.contains("\\# Other highlights"));
    assert!(rendered.content.contains("### Other highlights"));
}

#[test]
fn membership_covers_notes_aliases_conflicts_and_escapes() {
    let bob_dir = temp_bob_dir("sections-membership");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let annotations = vec![
        declaration(1, "Page 1", "Open questions @q", "Desc."),
        // Alias and forward reference (declaration is first, tag later still works).
        sticky(2, "Page 2", "a sticky @q"),
        // Conflict: first home kept, second stays visible.
        highlight(3, "Page 3", "conflict quote", Some("@q @visuals")),
        // Unknown tags stay visible.
        highlight(4, "Page 4", "unknown quote", Some("@memry")),
        // Escaped tags never resolve and render without slash.
        highlight(5, "Page 5", "escaped quote", Some("\\@q")),
        // Tags-only sticky does not join.
        sticky(6, "Page 6", "@q"),
        // Mid-sentence tags are prose.
        sticky(7, "Page 7", "Use @q mid-sentence here"),
    ];
    // Add the second section for the conflict case.
    let mut with_visuals = vec![declaration(8, "Page 1", "Visuals", "")];
    with_visuals.extend(annotations.clone());
    // Reorder so Visuals declaration is second; keep orders unique by rebuilding.
    let annotations = vec![
        declaration(1, "Page 1", "Open questions @q", "Desc."),
        declaration(2, "Page 1", "Visuals", ""),
        sticky(3, "Page 2", "a sticky @q"),
        highlight(4, "Page 3", "conflict quote", Some("@q @visuals")),
        highlight(5, "Page 4", "unknown quote", Some("@memry")),
        highlight(6, "Page 5", "escaped quote", Some("\\@q")),
        sticky(7, "Page 6", "@q"),
        sticky(8, "Page 7", "Use @q mid-sentence here"),
    ];
    let rendered =
        render(&bob_dir, &pdf, &ref_note, annotations, &ParsedNote::empty());
    assert_eq!(rendered.sections.len(), 2);
    // Alias joined.
    assert!(!rendered.content.contains("a sticky @q"));
    // Conflict keeps first home but leaves second visible.
    assert!(rendered.content.contains("@visuals"));
    // Unknown stays visible.
    assert!(rendered.content.contains("@memry"));
    // Escaped renders without slash.
    assert!(rendered.content.contains("@q"));
    assert!(!rendered.content.contains("\\@q"));
    // Tags-only stays visible under Other.
    let other_pos = rendered
        .content
        .find("### Other highlights")
        .expect("other");
    let other_slice = &rendered.content[other_pos..];
    assert!(other_slice.contains("@q"));
    // Mid-sentence stays prose under Other.
    assert!(other_slice.contains("Use @q mid-sentence here"));
    let _ = with_visuals;
}

#[test]
fn page_captions_reset_at_each_section_and_handle_labels() {
    let bob_dir = temp_bob_dir("sections-pages");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let annotations = vec![
        declaration(1, "Page 1", "Memory", ""),
        declaration(2, "Page 1", "Visuals", ""),
        highlight(3, "Page 4", "memory one", Some("@memory")),
        highlight(4, "Page 4", "visual one", Some("@visuals")),
        highlight(5, "Page 4", "memory two", Some("@memory")),
    ];
    let rendered =
        render(&bob_dir, &pdf, &ref_note, annotations, &ParsedNote::empty());
    // Each section repeats its Page 4 caption (run resets at H3).
    let memory_pos = rendered.content.find("### Memory").expect("memory");
    let visuals_pos = rendered.content.find("### Visuals").expect("visuals");
    let memory_slice = &rendered.content[memory_pos..visuals_pos];
    let visuals_slice = &rendered.content[visuals_pos..];
    assert!(memory_slice.contains("#### Page 4"));
    assert!(visuals_slice.contains("#### Page 4"));
    // Grouped pages use the group label on the card.
    assert!(rendered.content.contains("@memory · 2 highlights · Page 4"));
}

#[test]
fn round_trip_preserves_identities_pages_and_homes() {
    let bob_dir = temp_bob_dir("sections-roundtrip");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let annotations = vec![
        declaration(1, "Page 1", "Memory", "Memory description."),
        highlight(2, "Page 4", "first memory quote", Some("@memory")),
        highlight(3, "Page 5", "second memory quote", Some("note @memory")),
        sticky(4, "Page 6", "ungrouped sticky"),
    ];
    let rendered = render(
        &bob_dir,
        &pdf,
        &ref_note,
        annotations.clone(),
        &ParsedNote::empty(),
    );
    let parsed = parse_managed_region(&rendered.content);
    assert!(parsed.unparsed.is_empty());
    assert_eq!(parsed.sections.len(), 1);
    // True page labels, not section titles.
    for block in &parsed.blocks {
        assert!(block.page_label.as_deref() != Some("Memory"));
    }
    // Section homes.
    let memory_blocks: Vec<_> = parsed
        .blocks
        .iter()
        .filter(|b| b.section.as_deref() == Some("memory"))
        .collect();
    assert_eq!(memory_blocks.len(), 2);
    // Annotation counts unchanged (cards never count).
    assert_eq!(parsed.blocks.len(), 3);
    assert_eq!(rendered.count, 3);
    // Rerender identical bytes.
    let second = render(
        &bob_dir,
        &pdf,
        &ref_note,
        annotations,
        &region_note(&rendered.content),
    );
    assert_eq!(second.content, rendered.content);
    assert_eq!(live_ids(&second.content), live_ids(&rendered.content));
}

#[test]
fn identity_survives_tag_toggles_and_declaration_changes() {
    let bob_dir = temp_bob_dir("sections-identity");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let base = vec![
        declaration(1, "Page 1", "Memory", ""),
        highlight(2, "Page 4", "stable quote", Some("plain comment")),
        sticky(3, "Page 2", "sticky keeps id"),
    ];
    let first = render(
        &bob_dir,
        &pdf,
        &ref_note,
        base.clone(),
        &ParsedNote::empty(),
    );
    let first_ids = live_ids(&first.content);
    assert_eq!(first_ids.len(), 2);
    // Add a tag: same h-ID set, no tombstones.
    let tagged = vec![
        declaration(1, "Page 1", "Memory", ""),
        highlight(2, "Page 4", "stable quote", Some("plain comment @memory")),
        sticky(3, "Page 2", "sticky keeps id @memory"),
    ];
    let second = render(
        &bob_dir,
        &pdf,
        &ref_note,
        tagged,
        &region_note(&first.content),
    );
    assert_eq!(live_ids(&second.content), first_ids);
    assert!(!second.content.contains("Removed highlights"));
    // Remove the tag again: IDs still stable.
    let third = render(
        &bob_dir,
        &pdf,
        &ref_note,
        base.clone(),
        &region_note(&second.content),
    );
    assert_eq!(live_ids(&third.content), first_ids);
    // Remove the declaration: members fall back to Other with tags visible, same IDs.
    let no_decl = vec![
        highlight(2, "Page 4", "stable quote", Some("plain comment @memory")),
        sticky(3, "Page 2", "sticky keeps id @memory"),
    ];
    let fourth = render(
        &bob_dir,
        &pdf,
        &ref_note,
        no_decl,
        &region_note(&third.content),
    );
    assert_eq!(live_ids(&fourth.content), first_ids);
    assert!(fourth.content.contains("@memory"));
    assert!(!fourth.content.contains("[!summary|section]"));
    // Old published sticky ending in @word keeps its ID through primary matching.
    let published = vec![sticky(1, "Page 2", "ends with tag @memory")];
    let first_published = render(
        &bob_dir,
        &pdf,
        &ref_note,
        published.clone(),
        &ParsedNote::empty(),
    );
    let published_ids = live_ids(&first_published.content);
    let with_decl = vec![
        declaration(9, "Page 1", "Memory", ""),
        sticky(1, "Page 2", "ends with tag @memory"),
    ];
    let second_published = render(
        &bob_dir,
        &pdf,
        &ref_note,
        with_decl,
        &region_note(&first_published.content),
    );
    assert_eq!(live_ids(&second_published.content), published_ids);
}

#[test]
fn twin_quotes_keep_association_and_stay_unique() {
    let bob_dir = temp_bob_dir("sections-twins");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    // Identical quotes on one page in different sections, distinct stripped comments.
    let annotations = vec![
        declaration(1, "Page 1", "Memory", ""),
        declaration(2, "Page 1", "Visuals", ""),
        highlight(3, "Page 4", "same quote", Some("first @memory")),
        highlight(4, "Page 4", "same quote", Some("second @visuals")),
    ];
    let first = render(
        &bob_dir,
        &pdf,
        &ref_note,
        annotations.clone(),
        &ParsedNote::empty(),
    );
    let first_highlight_ids = [
        first
            .block_ids_by_annotation_order
            .get(&3)
            .cloned()
            .expect("head 3"),
        first
            .block_ids_by_annotation_order
            .get(&4)
            .cloned()
            .expect("head 4"),
    ];
    assert_ne!(first_highlight_ids[0], first_highlight_ids[1]);
    assert_eq!(live_ids(&first.content).len(), 2);
    // Reorder sections: distinct stripped comments preserve association.
    let reordered = vec![
        declaration(1, "Page 1", "Visuals", ""),
        declaration(2, "Page 1", "Memory", ""),
        highlight(3, "Page 4", "same quote", Some("first @memory")),
        highlight(4, "Page 4", "same quote", Some("second @visuals")),
    ];
    let second = render(
        &bob_dir,
        &pdf,
        &ref_note,
        reordered,
        &region_note(&first.content),
    );
    // Both IDs survive and stay unique; no annotation lost.
    assert_eq!(live_ids(&second.content).len(), 2);
    // Equal stripped comments: IDs remain unique.
    let equal = vec![
        declaration(1, "Page 1", "Memory", ""),
        declaration(2, "Page 1", "Visuals", ""),
        highlight(3, "Page 4", "same quote", Some("same @memory")),
        highlight(4, "Page 4", "same quote", Some("same @visuals")),
    ];
    let third = render(&bob_dir, &pdf, &ref_note, equal, &ParsedNote::empty());
    assert_eq!(live_ids(&third.content).len(), 2);
    // Accepted residual: otherwise indistinguishable twins may swap IDs/links by order.
    // We document the order-based fallback without promising stability here.
    let parsed = parse_managed_region(&third.content);
    assert_eq!(parsed.blocks.len(), 2);
}

#[test]
fn section_tasks_use_section_links_and_keep_processed_ids() {
    let bob_dir = temp_bob_dir("sections-tasks");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let annotations = vec![
        declaration(1, "Page 1", "Memory", "- #task Review memory."),
        highlight(2, "Page 4", "q", Some("@memory")),
    ];
    let rendered = render(
        &bob_dir,
        &pdf,
        &ref_note,
        annotations.clone(),
        &ParsedNote::empty(),
    );
    // Declaration maps to the section in both maps.
    assert_eq!(
        rendered
            .block_ids_by_annotation_order
            .get(&1)
            .map(String::as_str),
        Some("s-memory")
    );
    assert_eq!(
        rendered
            .standalone_ids_by_annotation_order
            .get(&1)
            .map(String::as_str),
        Some("s-memory")
    );
    let candidates =
        task_candidates(&bob_dir, &ref_note, &pdf, &annotations, &rendered);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].source_block_id, "s-memory");
    // Rendered source link uses the section block.
    let link = annotation_task_source_link(
        &test_config_for_bob_dir(bob_dir.clone()),
        &candidates[0],
    );
    assert!(link.contains("#^s-memory"), "{link}");
    assert!(link.contains("🔖"), "{link}");
    // Editing surrounding prose preserves the processed ID.
    let edited = vec![
        declaration(
            1,
            "Page 1",
            "Memory",
            "Intro line.\n- #task Review memory.",
        ),
        highlight(2, "Page 4", "q", Some("@memory")),
    ];
    let second = render(
        &bob_dir,
        &pdf,
        &ref_note,
        edited.clone(),
        &region_note(&rendered.content),
    );
    let second_candidates =
        task_candidates(&bob_dir, &ref_note, &pdf, &edited, &second);
    assert_eq!(second_candidates.len(), 1);
    assert_eq!(
        second_candidates[0].processed_id,
        candidates[0].processed_id
    );
}

#[test]
fn comment_declaration_groups_bryans_note_and_keeps_ids() {
    let bob_dir = temp_bob_dir("sections-comment-bryan");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let sidecar = "\
#### [Page 2](highlights://example#page=2)

##### 2026-10-24:

> Unrelated quote on page two.

---

#### [Page 4](highlights://example#page=4)

##### 2026-10-24:

> First quote on page four.

# what is it

---

#### [Page 4](highlights://example#page=4)

##### 2026-10-24:

> Second quote on page four.

@what-is-it

---

#### [Page 5](highlights://example#page=5)

##### 2026-10-24:

> Quote on page five.

This is an interesting foobar… @what-is-it
";
    let annotations = parse_sidecar_markdown(sidecar);
    assert_eq!(annotations.len(), 4, "{annotations:?}");
    assert_eq!(
        annotations[1].comment_section_heading.as_deref(),
        Some("what is it"),
        "highlight comment declares: {annotations:?}"
    );
    let rendered = render(
        &bob_dir,
        &pdf,
        &ref_note,
        annotations.clone(),
        &ParsedNote::empty(),
    );
    assert_eq!(rendered.sections.len(), 1);
    assert_eq!(rendered.sections[0].title, "what is it");
    assert_eq!(rendered.sections[0].tag, "@what-is-it");
    assert_eq!(rendered.sections[0].count, 3);
    assert!(
        rendered.content.contains("### what is it"),
        "{}",
        rendered.content
    );
    assert!(
        rendered.content.contains(
            "> [!summary|section] @what-is-it · 3 highlights · Pages 4–5"
        ),
        "{}",
        rendered.content
    );
    assert!(
        rendered.content.contains("^s-what-is-it"),
        "{}",
        rendered.content
    );
    let page4_pos = rendered.content.find("#### Page 4").expect("page 4");
    let page5_pos = rendered.content.find("#### Page 5").expect("page 5");
    let other_pos = rendered
        .content
        .find("### Other highlights")
        .expect("other");
    assert!(page4_pos < page5_pos && page5_pos < other_pos);
    // Both page-4 quotes render with no Comment card.
    let section_slice = &rendered.content[page4_pos..page5_pos];
    assert!(section_slice.contains("First quote on page four."));
    assert!(section_slice.contains("Second quote on page four."));
    assert!(
        !section_slice.contains("[!note] Comment"),
        "declaring and tagged page-4 blocks have no Comment: {section_slice}"
    );
    let page5_slice = &rendered.content[page5_pos..other_pos];
    assert!(page5_slice.contains("This is an interesting foobar…"));
    assert!(!page5_slice.contains("@what-is-it"));
    let other_slice = &rendered.content[other_pos..];
    assert!(other_slice.contains("#### Page 2"));
    assert!(other_slice.contains("Unrelated quote on page two."));
    // IDs equal the same sidecar without the declaration line.
    let plain = "\
#### [Page 2](highlights://example#page=2)

##### 2026-10-24:

> Unrelated quote on page two.

---

#### [Page 4](highlights://example#page=4)

##### 2026-10-24:

> First quote on page four.

---

#### [Page 4](highlights://example#page=4)

##### 2026-10-24:

> Second quote on page four.

@what-is-it

---

#### [Page 5](highlights://example#page=5)

##### 2026-10-24:

> Quote on page five.

This is an interesting foobar… @what-is-it
";
    let plain_annotations = parse_sidecar_markdown(plain);
    let plain_rendered = render(
        &bob_dir,
        &pdf,
        &ref_note,
        plain_annotations,
        &ParsedNote::empty(),
    );
    let ids: BTreeSet<String> = rendered
        .block_ids_by_annotation_order
        .values()
        .cloned()
        .collect();
    let plain_ids: BTreeSet<String> = plain_rendered
        .block_ids_by_annotation_order
        .values()
        .cloned()
        .collect();
    assert_eq!(ids, plain_ids, "declaring keeps ^h- IDs");
    // Second render against the new region is byte-identical.
    let second = render(
        &bob_dir,
        &pdf,
        &ref_note,
        annotations,
        &region_note(&rendered.content),
    );
    assert_eq!(second.content, rendered.content);
    // Removing the declaration restores the original layout.
    let third = render(
        &bob_dir,
        &pdf,
        &ref_note,
        parse_sidecar_markdown(plain),
        &region_note(&rendered.content),
    );
    assert!(!third.content.contains("### what is it"));
    assert!(third.content.contains("@what-is-it"));
    assert_eq!(
        third.content, plain_rendered.content,
        "removing the declaration restores the plain render"
    );
    assert_eq!(
        third
            .block_ids_by_annotation_order
            .values()
            .cloned()
            .collect::<BTreeSet<_>>(),
        ids,
        "removing the declaration keeps IDs"
    );
    // Round trip parses.
    let parsed = parse_managed_region(&rendered.content);
    assert_eq!(parsed.sections.len(), 1);
    assert!(parsed.unparsed.is_empty());
}

#[test]
fn comment_declaration_keeps_remaining_text_and_task_link() {
    let bob_dir = temp_bob_dir("sections-comment-remaining");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    // Declaring highlight with remaining comment text and a `#task` line.
    // Parsing records the declaration; rendering removes only the first line.
    let sidecar = "\
#### [Page 4](highlights://example#page=4)

##### 2026-10-24:

> A quote.

# Memory

Remaining comment text.

- #task Review memory.
";
    let annotations = parse_sidecar_markdown(sidecar);
    assert_eq!(annotations.len(), 1);
    assert_eq!(
        annotations[0].comment_section_heading.as_deref(),
        Some("Memory")
    );
    // Add the section declaration sticky so the comment declaration merges.
    // Actually the comment itself declares, so no sticky needed; but add a
    // sticky for another section to prove tags still work? Keep it minimal:
    // the comment declares Memory, so rendering should group it.
    let rendered = render(
        &bob_dir,
        &pdf,
        &ref_note,
        annotations.clone(),
        &ParsedNote::empty(),
    );
    assert_eq!(rendered.sections.len(), 1);
    assert_eq!(rendered.sections[0].title, "Memory");
    assert!(rendered.content.contains("### Memory"));
    assert!(rendered.content.contains("Remaining comment text."));
    assert!(!rendered.content.contains("\\# Memory"));
    // The `#task` line stays that block's task, linked to its `^h-` ID.
    let block_id = rendered
        .block_ids_by_annotation_order
        .get(&1)
        .cloned()
        .expect("h id");
    assert!(block_id.starts_with("h-"));
    let candidates =
        task_candidates(&bob_dir, &ref_note, &pdf, &annotations, &rendered);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].source_block_id, block_id);
}
