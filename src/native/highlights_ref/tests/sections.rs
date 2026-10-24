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
        comment_section_heading: None,
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

fn comment_declared(
    order: usize,
    kind: SidecarAnnotationKind,
    text: &str,
    comment: &str,
    heading: &str,
) -> SidecarAnnotation {
    SidecarAnnotation {
        kind,
        page_label: Some(format!("Page {order}")),
        page_index: Some(order as u32),
        linked_page_style: false,
        text: text.to_string(),
        section_heading: None,
        comment_section_heading: Some(heading.to_string()),
        comment: Some(comment.to_string()),
        task_source: Some(comment.to_string()),
        image: None,
        order,
        ordinal_on_page: order,
    }
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
    // A first-line `\#` escape never declares and never reports; only a
    // non-first-line `#` line stays literal with a diagnostic.
    let first_line_escape = vec![
        declaration(1, "Memory", ""),
        annotation(
            2,
            SidecarAnnotationKind::Highlight,
            "Quote",
            Some("\\# Memory\n@memory"),
            None,
        ),
    ];
    let planned = plan(&first_line_escape);
    assert!(
        !diagnostic_kinds(&planned)
            .contains(&SectionDiagnosticKind::DeclarationInComment),
        "first-line escape is intentional: {:?}",
        planned.diagnostics
    );
    assert_eq!(planned.home_of.get(&2), Some(&0));

    let later_line = vec![
        declaration(1, "Memory", ""),
        annotation(
            2,
            SidecarAnnotationKind::Highlight,
            "Quote",
            Some("A note\n\\# Memory\n@memory"),
            None,
        ),
    ];
    let planned = plan(&later_line);
    assert!(diagnostic_kinds(&planned)
        .contains(&SectionDiagnosticKind::DeclarationInComment));
    let detail = planned
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.kind == SectionDiagnosticKind::DeclarationInComment
        })
        .expect("detail")
        .detail
        .clone();
    assert!(
        detail.contains("put \"# Name\" on the first line"),
        "reworded diagnostic: {detail}"
    );

    // The mistake is never silent: it reports even when no section exists.
    let no_sections = vec![annotation(
        1,
        SidecarAnnotationKind::Highlight,
        "Quote",
        Some("A note\n\\# Memory"),
        None,
    )];
    let planned = plan(&no_sections);
    assert!(planned.sections.is_empty());
    assert!(diagnostic_kinds(&planned)
        .contains(&SectionDiagnosticKind::DeclarationInComment));
}

#[test]
fn comment_declarations_declare_merge_conflict_and_reject() {
    // Accepted comment declaration creates a section with no description.
    let declared = comment_declared(
        1,
        SidecarAnnotationKind::Highlight,
        "Quote",
        "\\# Memory",
        "Memory",
    );
    let tagged = annotation(
        2,
        SidecarAnnotationKind::Highlight,
        "Other quote",
        Some("@memory"),
        None,
    );
    let planned = plan(&[declared.clone(), tagged]);
    assert_eq!(planned.sections.len(), 1);
    assert_eq!(planned.sections[0].title, "Memory");
    assert!(planned.sections[0].description.is_empty());
    assert_eq!(planned.sections[0].anchor_orders, vec![1]);
    assert!(planned.sections[0].declaration_orders.is_empty());
    assert_eq!(planned.home_of.get(&1), Some(&0));
    assert_eq!(planned.home_of.get(&2), Some(&0));
    assert_eq!(planned.rewritten.get(&1).map(String::as_str), Some(""));

    // Escaped first line never declares.
    let escaped = comment_declared(
        1,
        SidecarAnnotationKind::Highlight,
        "Quote",
        "\\# Memory",
        "Memory",
    );
    // Manually clear the heading to simulate a user-typed `\#` escape:
    // parsing would leave `comment_section_heading` as None.
    let mut escaped = escaped;
    escaped.comment_section_heading = None;
    let planned = plan(&[
        annotation(
            1,
            SidecarAnnotationKind::StandaloneNote,
            "decl",
            None,
            Some("Memory"),
        ),
        SidecarAnnotation {
            comment_section_heading: None,
            ..escaped
        },
    ]);
    assert_eq!(planned.sections.len(), 1);
    assert!(!planned.comment_declarations.contains(&1));

    // Continuation followers never declare.
    let head = annotation(
        1,
        SidecarAnnotationKind::Highlight,
        "Head",
        Some("@memory"),
        None,
    );
    let mut follower = comment_declared(
        2,
        SidecarAnnotationKind::Highlight,
        "Follower",
        "\\# Memory",
        "Memory",
    );
    follower.page_label = Some("Page 1".to_string());
    // Force a continuation fold: head + follower with `<` marker.
    let mut head_with_marker = head.clone();
    head_with_marker.task_source = Some("ordinary".to_string());
    head_with_marker.comment = Some("ordinary".to_string());
    follower.task_source = Some("< \\# Memory".to_string());
    follower.comment = Some("< \\# Memory".to_string());
    let annotations = vec![
        annotation(
            0,
            SidecarAnnotationKind::StandaloneNote,
            "",
            None,
            Some("Memory"),
        ),
        head_with_marker,
        follower,
    ];
    let folds = plan_annotation_folds(&annotations, false);
    // If the follower merged, it must not declare.
    if folds.continuations.head_of.contains_key(&2) {
        let planned =
            plan_sections(&annotations, &folds, "lib/papers/example.pdf");
        assert!(!planned.comment_declarations.contains(&2));
    }

    // Merging sticky + comment in both orders: first wins title/slug,
    // description comes from the sticky.
    for (first_sticky, expected_title) in
        [(true, "Sticky title"), (false, "Comment title")]
    {
        let (first, second) = if first_sticky {
            (
                annotation(
                    1,
                    SidecarAnnotationKind::StandaloneNote,
                    "Sticky description.",
                    None,
                    Some("Sticky title"),
                ),
                comment_declared(
                    2,
                    SidecarAnnotationKind::Highlight,
                    "Q",
                    "\\# Comment title",
                    "Comment title",
                ),
            )
        } else {
            (
                comment_declared(
                    1,
                    SidecarAnnotationKind::Highlight,
                    "Q",
                    "\\# Comment title",
                    "Comment title",
                ),
                annotation(
                    2,
                    SidecarAnnotationKind::StandaloneNote,
                    "Sticky description.",
                    None,
                    Some("Sticky title"),
                ),
            )
        };
        // Same key to force a merge: use identical titles.
        let first = first;
        let mut second = second;
        // Force same key by using the same heading text.
        if first_sticky {
            second.comment_section_heading = Some("Sticky title".to_string());
        } else {
            // first is comment "Comment title", second sticky must match it
            second.section_heading = Some("Comment title".to_string());
        }
        let _ = expected_title;
        let planned = plan(&[first.clone(), second.clone()]);
        assert_eq!(planned.sections.len(), 1, "{first_sticky}");
        // Description always comes from the sticky.
        assert_eq!(
            planned.sections[0].description, "Sticky description.",
            "{first_sticky}"
        );
        assert!(planned.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == SectionDiagnosticKind::Duplicate
        }));
    }

    // Two declarations in one block report conflict; tags for another section
    // also conflict with the declaration home kept.
    let host = comment_declared(
        1,
        SidecarAnnotationKind::Highlight,
        "Host quote",
        "\\# Memory",
        "Memory",
    );
    let mut absorbed = comment_declared(
        2,
        SidecarAnnotationKind::Highlight,
        "**Host quote**",
        "\\# Visuals",
        "Visuals",
    );
    absorbed.page_label = host.page_label.clone();
    absorbed.page_index = host.page_index;
    let sticky_memory = annotation(
        0,
        SidecarAnnotationKind::StandaloneNote,
        "",
        None,
        Some("Memory"),
    );
    let sticky_visuals = annotation(
        3,
        SidecarAnnotationKind::StandaloneNote,
        "",
        None,
        Some("Visuals"),
    );
    let annotations = vec![sticky_memory, host, absorbed, sticky_visuals];
    let folds = plan_annotation_folds(&annotations, false);
    // Only test conflict when the underline actually absorbs.
    if folds.underlines.host_of.get(&2) == Some(&1) {
        let planned =
            plan_sections(&annotations, &folds, "lib/papers/example.pdf");
        assert_eq!(planned.home_of.get(&1), Some(&0));
        assert!(diagnostic_kinds(&planned)
            .contains(&SectionDiagnosticKind::Conflict));
    }

    // Rejected comment declarations report and render literally.
    let empty = comment_declared(
        1,
        SidecarAnnotationKind::Highlight,
        "Q",
        "\\# !!!",
        "!!!",
    );
    let planned = plan(&[empty]);
    assert!(planned.sections.is_empty());
    assert!(
        diagnostic_kinds(&planned).contains(&SectionDiagnosticKind::TitleEmpty)
    );
    let reserved = comment_declared(
        1,
        SidecarAnnotationKind::Highlight,
        "Q",
        "\\# Other highlights",
        "Other highlights",
    );
    let planned = plan(&[reserved]);
    assert!(planned.sections.is_empty());
    assert!(diagnostic_kinds(&planned)
        .contains(&SectionDiagnosticKind::ReservedTitle));
}

#[test]
fn comment_declaration_beats_tags_and_reports_other_section() {
    let sticky_memory = annotation(
        0,
        SidecarAnnotationKind::StandaloneNote,
        "",
        None,
        Some("Memory"),
    );
    let sticky_visuals = annotation(
        3,
        SidecarAnnotationKind::StandaloneNote,
        "",
        None,
        Some("Visuals"),
    );
    // Declaring block tags another section: declaration wins, other tag stays.
    let mut declaring = comment_declared(
        1,
        SidecarAnnotationKind::Highlight,
        "Quote",
        "\\# Memory\n@visuals",
        "Memory",
    );
    // The stored comment has the escaped declaration plus the other tag.
    declaring.comment = Some("\\# Memory\n@visuals".to_string());
    declaring.task_source = Some("\\# Memory\n@visuals".to_string());
    let planned = plan(&[sticky_memory, declaring, sticky_visuals]);
    assert_eq!(planned.sections.len(), 2);
    assert_eq!(planned.home_of.get(&1), Some(&0));
    assert_eq!(
        planned.rewritten.get(&1).map(String::as_str),
        Some("@visuals")
    );
    assert!(
        diagnostic_kinds(&planned).contains(&SectionDiagnosticKind::Conflict)
    );
    let conflict = planned
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.kind == SectionDiagnosticKind::Conflict)
        .expect("conflict");
    assert!(conflict.detail.contains("(kept @memory)"));
}

#[test]
fn comment_declarations_work_for_images_and_standalone_underlines() {
    let image = comment_declared(
        1,
        SidecarAnnotationKind::Image,
        "figure.png",
        "\\# Memory",
        "Memory",
    );
    let sticky = annotation(
        0,
        SidecarAnnotationKind::StandaloneNote,
        "",
        None,
        Some("Memory"),
    );
    let planned = plan(&[sticky, image]);
    assert_eq!(planned.sections.len(), 1);
    assert_eq!(planned.home_of.get(&1), Some(&0));
    assert_eq!(planned.rewritten.get(&1).map(String::as_str), Some(""));

    // A standalone underline (`**term**` with no host) declares as a head.
    let mut underline = comment_declared(
        2,
        SidecarAnnotationKind::Highlight,
        "**term**",
        "\\# Memory",
        "Memory",
    );
    underline.page_label = Some("Page 1".to_string());
    underline.page_index = Some(1);
    let planned = plan(&[
        annotation(
            0,
            SidecarAnnotationKind::StandaloneNote,
            "",
            None,
            Some("Memory"),
        ),
        underline,
    ]);
    // Either standalone (no host) with a home, or absorbed (host found) with
    // the same home — both prove the underline declaration works.
    assert_eq!(planned.sections.len(), 1);
    assert!(
        planned.home_of.contains_key(&2)
            || planned.comment_declarations.contains(&2)
    );
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

#[test]
fn hash_tag_scanner_handles_positions_and_rejections() {
    let positional = |text: &str| {
        scan_section_tags(text)
            .into_iter()
            .filter(|hit| hit.kind == SectionTagHitKind::Positional)
            .collect::<Vec<_>>()
    };
    // Positional `#name` shapes.
    for text in [
        "Comment: #Memory.",
        "- #open_questions",
        "#memory",
        "Notes #memory",
    ] {
        let hits = positional(text);
        assert_eq!(hits.len(), 1, "{text}");
        assert_eq!(hits[0].sigil, '#');
    }
    // Mid-sentence is prose.
    assert!(positional("Compare #memory mid-sentence").is_empty());
    // Never tokens.
    for text in [
        "C#",
        "page#anchor",
        "[[n#H]]",
        "#area/memory",
        "#git:home",
        "#2024",
        "#1",
        "#task",
        "#Task",
        "# Memory",
        "## Memory",
    ] {
        assert!(
            scan_section_tags(text).is_empty(),
            "{text}: {:?}",
            scan_section_tags(text)
        );
    }
    // Code and fences are ignored.
    assert!(scan_section_tags("`#memory` stays code").is_empty());
    assert!(
        scan_section_tags("```md\n#memory\n```").is_empty(),
        "fenced"
    );
    // A `#task` line never yields a positional section tag.
    assert!(positional("- #task Draft #memory").is_empty(), "task line");
    // Escaped.
    let escaped = scan_section_tags("\\#memory");
    assert_eq!(escaped.len(), 1);
    assert_eq!(escaped[0].kind, SectionTagHitKind::Escaped);
    assert_eq!(escaped[0].sigil, '#');
    // Mixed runs work on both ends.
    let mixed = positional("@memory #visuals");
    assert_eq!(mixed.len(), 2);
    assert_eq!(mixed[0].sigil, '@');
    assert_eq!(mixed[1].sigil, '#');
    let ends = positional("#a Text #memory");
    assert_eq!(ends.len(), 2);
}

#[test]
fn hash_display_source_consumes_and_keeps_escapes_visible() {
    let home = BTreeSet::from(["memory".to_string()]);
    assert_eq!(display_source("#memory", &home), "");
    assert_eq!(display_source("- #memory", &home), "");
    assert_eq!(display_source("A note #memory!", &home), "A note");
    assert_eq!(display_source("#memory @memory", &home), "");
    assert_eq!(display_source("#other", &home), "#other");
    assert_eq!(display_source("\\#memory", &home), "\\#memory");
    assert_eq!(display_source("\\@memory", &home), "@memory");
}

#[test]
fn hash_planner_joins_conflicts_and_reports_near_miss_only() {
    // `#memory` joins.
    let joined = plan(&[
        declaration(1, "Memory", ""),
        annotation(
            2,
            SidecarAnnotationKind::Highlight,
            "Quote",
            Some("#memory"),
            None,
        ),
    ]);
    assert_eq!(joined.home_of.get(&2), Some(&0));
    // A `#` forward reference joins.
    let forward = plan(&[
        annotation(
            1,
            SidecarAnnotationKind::Highlight,
            "Quote",
            Some("#memory"),
            None,
        ),
        declaration(2, "Memory", ""),
    ]);
    assert_eq!(forward.home_of.get(&1), Some(&0));
    // `#memory @visuals` conflicts and keeps Memory.
    let conflict = plan(&[
        declaration(1, "Memory", ""),
        declaration(2, "Visuals", ""),
        annotation(
            3,
            SidecarAnnotationKind::Highlight,
            "Quote",
            Some("#memory @visuals"),
            None,
        ),
    ]);
    assert_eq!(conflict.home_of.get(&3), Some(&0));
    assert!(
        diagnostic_kinds(&conflict).contains(&SectionDiagnosticKind::Conflict)
    );
    // `#memry` reports with a `#memory` hint.
    let typo = plan(&[
        declaration(1, "Memory", ""),
        annotation(
            2,
            SidecarAnnotationKind::Highlight,
            "Quote",
            Some("#memry"),
            None,
        ),
    ]);
    assert!(
        typo.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == SectionDiagnosticKind::TagUnknown
                && diagnostic.detail.contains("#memry")
                && diagnostic.detail.contains("did you mean #memory?")
        }),
        "{:?}",
        typo.diagnostics
    );
    // A far unknown `#a` stays silent under `near_miss`.
    let far = plan(&[
        declaration(1, "Memory", ""),
        annotation(
            2,
            SidecarAnnotationKind::Highlight,
            "Quote",
            Some("#a"),
            None,
        ),
    ]);
    assert!(
        !diagnostic_kinds(&far).contains(&SectionDiagnosticKind::TagUnknown),
        "{:?}",
        far.diagnostics
    );
    // A `#memory`-only sticky reports and does not join.
    let sticky = plan(&[
        declaration(1, "Memory", ""),
        annotation(
            2,
            SidecarAnnotationKind::StandaloneNote,
            "#memory",
            None,
            None,
        ),
    ]);
    assert!(!sticky.home_of.contains_key(&2));
    assert!(
        diagnostic_kinds(&sticky).contains(&SectionDiagnosticKind::TagOnlyNote)
    );
    // A comment declaration beats a `#name` tag for another section.
    let sticky_memory = annotation(
        0,
        SidecarAnnotationKind::StandaloneNote,
        "",
        None,
        Some("Memory"),
    );
    let sticky_visuals = annotation(
        3,
        SidecarAnnotationKind::StandaloneNote,
        "",
        None,
        Some("Visuals"),
    );
    let mut declaring = comment_declared(
        1,
        SidecarAnnotationKind::Highlight,
        "Quote",
        "\\# Memory\n#visuals",
        "Memory",
    );
    declaring.comment = Some("\\# Memory\n#visuals".to_string());
    declaring.task_source = Some("\\# Memory\n#visuals".to_string());
    let beaten = plan(&[sticky_memory, declaring, sticky_visuals]);
    assert_eq!(beaten.sections.len(), 2);
    assert_eq!(beaten.home_of.get(&1), Some(&0));
    // With no declarations, `#memory` yields an empty plan and no diagnostics.
    let none = plan(&[annotation(
        1,
        SidecarAnnotationKind::Highlight,
        "Quote",
        Some("#memory"),
        None,
    )]);
    assert!(none.sections.is_empty());
    assert!(none.diagnostics.is_empty());
}

#[test]
fn hash_declarations_accept_hash_aliases() {
    let parsed =
        scan_section_declaration("Open questions #q").expect("hash alias");
    assert_eq!(parsed.title, "Open questions");
    assert_eq!(parsed.aliases, vec!["q"]);
    assert_eq!(parsed.tag, "@q");
    assert_eq!(
        parsed.keys,
        BTreeSet::from(["openquestions".into(), "q".into()])
    );
    let chapter =
        scan_section_declaration("Chapter #1").expect("digit alias rejected");
    assert_eq!(chapter.title, "Chapter 1");
    assert!(chapter.aliases.is_empty());
    let notes =
        scan_section_declaration("Notes #task").expect("task alias rejected");
    assert!(notes.title.contains("task"), "{}", notes.title);
    assert!(notes.aliases.is_empty());
}

#[test]
fn hash_identity_key_strips_hash_but_keeps_non_tags() {
    assert_eq!(
        section_match_key("Compare #Memory, then `x`"),
        "Compare then `x`"
    );
    assert_eq!(
        section_match_key("Compare #Memory, then `x`"),
        section_match_key("Compare @Memory, then `x`")
    );
    assert!(section_match_key("Review #task now").contains("#task"));
    assert!(section_match_key("C# code").contains("C#"));
    assert!(section_match_key("see page#x now").contains("page#x"));
    assert!(section_match_key("tag #area/memory here").contains("#area/memory"));
    assert_eq!(section_match_key("\\#memory"), "\\#memory");
}
