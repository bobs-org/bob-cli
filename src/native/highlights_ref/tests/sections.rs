//! Annotation section declaration and membership grammar.
use super::*;
use crate::native::highlights_ref::sections::*;

fn annotation(
    order: usize,
    kind: SidecarAnnotationKind,
    text: &str,
    task_source: Option<&str>,
    section_heading: Option<&str>,
) -> SidecarAnnotation {
    SidecarAnnotation {
        kind,
        page_label: Some(format!("Page {order}")),
        page_index: Some(order as u32),
        linked_page_style: false,
        text: text.to_string(),
        section_heading: section_heading.map(str::to_string),
        comment: task_source.map(str::to_string),
        task_source: task_source.map(str::to_string),
        image: None,
        order,
        ordinal_on_page: order,
    }
}

fn declaration(
    order: usize,
    heading: &str,
    description: &str,
) -> SidecarAnnotation {
    annotation(
        order,
        SidecarAnnotationKind::StandaloneNote,
        description,
        None,
        Some(heading),
    )
}

fn plan(annotations: &[SidecarAnnotation]) -> SectionPlan {
    let folds = plan_annotation_folds(annotations, false);
    plan_sections(annotations, &folds, "lib/papers/example.pdf")
}

fn diagnostic_kinds(plan: &SectionPlan) -> Vec<SectionDiagnosticKind> {
    plan.diagnostics
        .iter()
        .map(|diagnostic| diagnostic.kind)
        .collect()
}

#[test]
fn sticky_declarations_keep_description_headings_and_empty_declarations() {
    let parsed = parse_sidecar_chunk(
        &[
            "##### 2026-10-19:".to_string(),
            "Note: # Memory @m".to_string(),
            "A description.".to_string(),
            "## Subheading".to_string(),
        ],
        Some("Page 1"),
        Some(1),
        false,
    );
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].section_heading.as_deref(), Some("Memory @m"));
    assert_eq!(parsed[0].text, "A description.\n\\## Subheading");

    let empty = parse_sidecar_chunk(
        &["# Memory".to_string()],
        Some("Page 2"),
        Some(2),
        false,
    );
    assert_eq!(empty.len(), 1);
    assert_eq!(empty[0].section_heading.as_deref(), Some("Memory"));
    assert!(empty[0].text.is_empty());
}

#[test]
fn ordinary_stickies_and_comment_hash_lines_keep_their_grammar() {
    let ordinary = parse_sidecar_chunk(
        &[
            "Personal note".to_string(),
            "## Not a declaration".to_string(),
        ],
        Some("Page 1"),
        Some(1),
        false,
    );
    assert_eq!(ordinary.len(), 1);
    assert_eq!(ordinary[0].section_heading, None);
    assert_eq!(ordinary[0].text, "Personal note");

    let comment = parse_sidecar_chunk(
        &[
            "> quoted text".to_string(),
            "# Memory is literal here".to_string(),
        ],
        Some("Page 3"),
        Some(3),
        false,
    );
    assert_eq!(
        comment[0].comment.as_deref(),
        Some("\\# Memory is literal here")
    );

    let image_comment = parse_sidecar_chunk(
        &[
            "![Figure](assets/figure.png)".to_string(),
            "# Memory is literal here".to_string(),
        ],
        Some("Page 4"),
        Some(4),
        false,
    );
    assert_eq!(image_comment.len(), 1);
    assert_eq!(
        image_comment[0].comment.as_deref(),
        Some("\\# Memory is literal here")
    );
}

#[test]
fn declaration_scanner_normalizes_titles_aliases_and_reserved_names() {
    let parsed =
        scan_section_declaration("Open [[questions]] | ^ # @Q @open_questions")
            .expect("valid title and aliases");
    assert_eq!(parsed.title, "Open questions");
    assert_eq!(parsed.aliases, vec!["Q", "open_questions"]);
    assert_eq!(parsed.tag, "@q");
    assert_eq!(
        parsed.keys,
        BTreeSet::from(["openquestions".into(), "q".into()])
    );
    assert_eq!(section_key("記憶 A_1"), "記憶a1");
    assert_eq!(scan_section_declaration("!!!"), Err(Rejection::TitleEmpty));
    assert_eq!(
        scan_section_declaration("Other highlights @misc"),
        Err(Rejection::ReservedTitle)
    );
}

#[test]
fn tag_scanner_handles_positions_labels_lists_codes_escapes_and_routes() {
    let text = concat!(
        "Comment: @Memory.\n",
        "- @open_questions\n",
        "Compare @Memory mid-sentence.\n",
        "mail me@example.com\n",
        "`@Memory` stays code\n",
        "\\@Memory is escaped\n",
        "- #task Draft @sase\n",
        "~/tmp/@Memory.png\n",
        "```md\n",
        "@Memory\n",
        "```\n",
    );
    let hits = scan_section_tags(text);
    assert_eq!(hits.len(), 4);
    assert_eq!(hits[0].raw, "@Memory.");
    assert_eq!(hits[0].key, "memory");
    assert_eq!(hits[0].kind, SectionTagHitKind::Positional);
    assert_eq!(hits[1].raw, "@open_questions");
    assert_eq!(hits[1].kind, SectionTagHitKind::Positional);
    assert_eq!(hits[2].raw, "@Memory");
    assert_eq!(hits[2].kind, SectionTagHitKind::Escaped);
    assert_eq!(hits[3].raw, "@sase");
    assert_eq!(hits[3].kind, SectionTagHitKind::OnTask);
    assert_eq!(
        hits.len(),
        4,
        "email, code, fences, and image paths are ignored"
    );
}

#[test]
fn source_rewrite_consumes_only_home_tags_and_keeps_escapes_visible() {
    let home = BTreeSet::from(["memory".to_string()]);
    assert_eq!(display_source("Comment: @memory", &home), "");
    assert_eq!(display_source("- @memory", &home), "");
    assert_eq!(
        display_source("A note @memory!\n@other", &home),
        "A note\n@other"
    );
    assert_eq!(display_source("\\@memory", &home), "@memory");
    assert_eq!(
        display_source("Use @memory mid-sentence", &home),
        "Use @memory mid-sentence"
    );
    assert_eq!(display_source("- @memory @memory", &home), "");
    assert_eq!(display_source("- @memory @other", &home), "- @other");
    assert_eq!(display_source("@memory `x  y`", &home), "`x  y`");
}

#[test]
fn planner_resolves_forward_aliases_merges_duplicates_and_rejects_invalid_titles(
) {
    let annotations = vec![
        annotation(
            1,
            SidecarAnnotationKind::Highlight,
            "Quote",
            Some("@open_questions"),
            None,
        ),
        declaration(2, "Open questions @q", "First description"),
        declaration(3, "Open questions @open_questions", "Second description"),
        declaration(4, "Other highlights", "reserved"),
        declaration(5, "!!!", "empty"),
    ];
    let plan = plan(&annotations);
    assert_eq!(plan.sections.len(), 1);
    assert_eq!(plan.sections[0].title, "Open questions");
    assert_eq!(plan.sections[0].tag, "@q");
    assert_eq!(plan.sections[0].declaration_orders, vec![2, 3]);
    assert_eq!(
        plan.sections[0].description,
        "First description\n\nSecond description"
    );
    assert_eq!(plan.home_of.get(&1), Some(&0));
    assert!(plan.rejected.contains_key(&4));
    assert!(plan.rejected.contains_key(&5));
    assert!(diagnostic_kinds(&plan).contains(&SectionDiagnosticKind::Duplicate));
    assert!(
        diagnostic_kinds(&plan).contains(&SectionDiagnosticKind::ReservedTitle)
    );
    assert!(
        diagnostic_kinds(&plan).contains(&SectionDiagnosticKind::TitleEmpty)
    );
    assert!(plan.diagnostics.iter().any(|diagnostic| {
        diagnostic.kind == SectionDiagnosticKind::Sections
            && diagnostic.detail == "1 (Open questions 1)"
    }));
    assert!(plan.diagnostics.iter().any(|diagnostic| {
        diagnostic.kind == SectionDiagnosticKind::Duplicate
            && diagnostic.detail.contains("Page 3")
            && diagnostic.detail.contains("Page 2")
    }));
}

#[test]
fn planner_consumes_follower_and_list_tags_and_reports_conflicts_and_unknowns()
{
    let annotations = vec![
        declaration(1, "Memory", ""),
        declaration(2, "Visuals", ""),
        annotation(
            3,
            SidecarAnnotationKind::Highlight,
            "Head",
            Some("ordinary"),
            None,
        ),
        annotation(
            4,
            SidecarAnnotationKind::Highlight,
            "Follower",
            Some("< @memory"),
            None,
        ),
        annotation(
            5,
            SidecarAnnotationKind::StandaloneNote,
            "- @memory @visuals",
            None,
            None,
        ),
        annotation(
            6,
            SidecarAnnotationKind::Image,
            "Image",
            Some("@memry"),
            None,
        ),
    ];
    let plan = plan(&annotations);
    assert_eq!(plan.home_of.get(&3), Some(&0));
    assert_eq!(plan.home_of.get(&5), Some(&0));
    assert_eq!(plan.rewritten.get(&4).map(String::as_str), Some(""));
    assert_eq!(
        plan.rewritten.get(&5).map(String::as_str),
        Some("- @visuals")
    );
    assert!(diagnostic_kinds(&plan).contains(&SectionDiagnosticKind::Conflict));
    assert!(
        diagnostic_kinds(&plan).contains(&SectionDiagnosticKind::TagUnknown)
    );
    assert!(plan.diagnostics.iter().any(|diagnostic| {
        diagnostic.kind == SectionDiagnosticKind::TagUnknown
            && diagnostic.detail.contains("did you mean @memory?")
    }));
}

#[test]
fn planner_does_not_join_tags_only_stickies_and_keeps_task_routes() {
    let annotations = vec![
        declaration(1, "Sase", ""),
        annotation(
            2,
            SidecarAnnotationKind::StandaloneNote,
            "@sase",
            None,
            None,
        ),
        annotation(
            3,
            SidecarAnnotationKind::Highlight,
            "Quote",
            Some("- #task Draft @sase"),
            None,
        ),
    ];
    let plan = plan(&annotations);
    assert!(!plan.home_of.contains_key(&2));
    assert!(
        diagnostic_kinds(&plan).contains(&SectionDiagnosticKind::TagOnlyNote)
    );
    assert!(diagnostic_kinds(&plan).contains(&SectionDiagnosticKind::TagOnTask));
    assert!(!plan.rewritten.contains_key(&3));
}

#[test]
fn planner_reports_literal_section_shaped_comment_lines() {
    let annotations = vec![
        declaration(1, "Memory", ""),
        annotation(
            2,
            SidecarAnnotationKind::Highlight,
            "Quote",
            Some("\\# Memory\n@memory"),
            None,
        ),
    ];
    let plan = plan(&annotations);
    assert!(diagnostic_kinds(&plan)
        .contains(&SectionDiagnosticKind::DeclarationInComment));
}

#[test]
fn planner_merges_underlines_by_their_display_head_and_keeps_repeated_home_tags(
) {
    let mut underline = annotation(
        3,
        SidecarAnnotationKind::Highlight,
        "**important**",
        Some("@Memory"),
        None,
    );
    underline.page_label = Some("Page 2".to_string());
    underline.page_index = Some(2);
    let annotations = vec![
        declaration(1, "Memory", ""),
        annotation(
            2,
            SidecarAnnotationKind::Highlight,
            "The **important** point",
            Some("@memory"),
            None,
        ),
        underline,
    ];
    let folds = plan_annotation_folds(&annotations, false);
    assert_eq!(folds.underlines.host_of.get(&3), Some(&2));
    let plan = plan_sections(&annotations, &folds, "lib/papers/example.pdf");
    assert_eq!(plan.home_of.get(&2), Some(&0));
    assert_eq!(plan.rewritten.get(&2).map(String::as_str), Some(""));
    assert_eq!(plan.rewritten.get(&3).map(String::as_str), Some(""));
}

#[test]
fn unicode_slugs_hash_fallback_and_collisions_are_stable() {
    let annotations = vec![
        declaration(1, "記憶", ""),
        declaration(2, "First @aé", ""),
        declaration(3, "Second @aö", ""),
    ];
    let plan = plan(&annotations);
    assert_eq!(plan.sections[0].tag, "@記憶");
    assert_eq!(plan.sections[0].slug.len(), 8);
    assert_eq!(plan.sections[1].slug, "a");
    assert_eq!(plan.sections[2].slug, "a-2");
}

#[test]
fn section_identity_key_ignores_tags_everywhere_but_keeps_an_all_tag_key() {
    assert_eq!(
        section_match_key("Compare @Memory, then `x`"),
        "Compare then `x`"
    );
    assert_eq!(section_match_key("\\@memory!"), "\\@memory!");
}

#[test]
fn no_declarations_produce_no_section_plan_or_tag_diagnostics() {
    let annotations = vec![annotation(
        1,
        SidecarAnnotationKind::Highlight,
        "Quote",
        Some("@unknown"),
        None,
    )];
    let plan = plan(&annotations);
    assert!(plan.sections.is_empty());
    assert!(plan.home_of.is_empty());
    assert!(plan.rewritten.is_empty());
    assert!(plan.diagnostics.is_empty());
}
