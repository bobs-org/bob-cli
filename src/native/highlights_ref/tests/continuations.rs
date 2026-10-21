//! Continuation-marker grammar, planner, seams, labels, and render tests.
use super::*;

const ELLIPSIS: char = '\u{2026}';
const EN_DASH: char = '\u{2013}';
const NBSP: char = '\u{00a0}';
const GAS_TOWN_HEAD_ID: &str = "h-ba93bec527bb";
const GAS_TOWN_FOLLOWER_ID: &str = "h-609a556c1cb0";
const GAS_TOWN_SIDECAR: &str = "\
#### [Page 11](highlights://gas_town_emergency_user_manual#page=11)

##### 2026-10-19:

> One last Crew tip: I have a new ad-hoc role that I’ve been using
> for the
past week, called the “PR Sheriff.One of my B
eads crew
> has a
permanent hook: b
d-4f43s: PR Sheriff Standing Orders. On
> every

***

#### [Page 12](highlights://gas_town_emergency_user_manual#page=12)

##### 2026-10-19:

> session startup, the sheriff checks open PRs and divides them
> into easy
wins and those needing human review, then slings the
> easy wins to
other crew for merging and cleanup

< PR Merge Master?!

***
";

fn highlight(
    order: usize,
    page_label: &str,
    page_index: Option<u32>,
    text: &str,
    task_source: Option<&str>,
) -> SidecarAnnotation {
    let comment = task_source.and_then(|source| {
        let comment = super::strip_comment_list_markers(source);
        (!comment.is_empty()).then_some(comment)
    });
    SidecarAnnotation {
        kind: SidecarAnnotationKind::Highlight,
        page_label: Some(page_label.to_string()),
        page_index,
        linked_page_style: false,
        text: text.to_string(),
        comment,
        task_source: task_source.map(str::to_string),
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
        comment: None,
        task_source: None,
        image: None,
        order,
        ordinal_on_page: order,
    }
}

fn render_annotations(
    bob_dir: &Path,
    pdf: &Path,
    ref_note: &Path,
    annotations: Vec<SidecarAnnotation>,
    note: &ParsedNote,
    strip_return_links: bool,
) -> RenderedHighlights {
    let config = test_config_for_bob_dir(bob_dir.to_path_buf());
    let sidecar = SidecarInput {
        path: bob_dir.join("lib/papers/example.md"),
        annotations,
    };
    super::render_sidecar_highlights(
        &config,
        pdf,
        ref_note,
        note,
        &sidecar,
        strip_return_links,
    )
    .expect("render continuations")
}

fn gas_town_paths(bob_dir: &Path) -> (PathBuf, PathBuf, Config) {
    let pdf = bob_dir.join("lib/blogs/gas_town_emergency_user_manual.pdf");
    let ref_note = bob_dir.join("ref/blogs/gas_town_emergency_user_manual.md");
    let config = test_config_for_bob_dir(bob_dir.to_path_buf());
    (pdf, ref_note, config)
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

fn marker_task(source: &str) -> Option<String> {
    match super::scan_continuation_marker(source) {
        super::MarkerScan::Marker { task_source, .. } => task_source,
        other => panic!("expected marker, got {other:?} for {source:?}"),
    }
}

#[test]
fn continuation_grammar_table() {
    assert!(matches!(
        super::scan_continuation_marker("<"),
        super::MarkerScan::Marker {
            token: super::ContinuationToken::Join,
            task_source: None,
        }
    ));
    assert_eq!(marker_task("< x").as_deref(), Some("x"));
    assert_eq!(marker_task("- < x").as_deref(), Some("- x"));
    assert_eq!(marker_task("* < x").as_deref(), Some("* x"));
    assert_eq!(marker_task(&format!("<{NBSP}x")).as_deref(), Some("x"));
    assert_eq!(marker_task("<\tx").as_deref(), Some("x"));
    assert_eq!(
        marker_task(&format!("<{ELLIPSIS} why skip this?")).as_deref(),
        Some("why skip this?")
    );
    assert_eq!(
        marker_task("<... why skip this?").as_deref(),
        Some("why skip this?")
    );
    assert!(matches!(
        super::scan_continuation_marker(&format!("<{ELLIPSIS}")),
        super::MarkerScan::Marker {
            token: super::ContinuationToken::Gap,
            task_source: None,
        }
    ));
    assert!(matches!(
        super::scan_continuation_marker("<..."),
        super::MarkerScan::Marker {
            token: super::ContinuationToken::Gap,
            task_source: None,
        }
    ));

    match super::scan_continuation_marker("\\< x") {
        super::MarkerScan::Escaped { task_source } => {
            assert_eq!(task_source, "< x");
        }
        other => panic!("expected escape, got {other:?}"),
    }
    match super::scan_continuation_marker(&format!("\\<{ELLIPSIS} x")) {
        super::MarkerScan::Escaped { task_source } => {
            assert_eq!(task_source, format!("<{ELLIPSIS} x"));
        }
        other => panic!("expected gap escape, got {other:?}"),
    }
    assert!(matches!(
        super::scan_continuation_marker("\\<div>"),
        super::MarkerScan::None
    ));

    for ordinary in [
        "<div>",
        "<-",
        "<=",
        "<3",
        "<https://x.org>",
        "</tmp/x.png>",
        "keep going\n< later",
        "…see above",
    ] {
        assert!(
            matches!(
                super::scan_continuation_marker(ordinary),
                super::MarkerScan::None
            ),
            "{ordinary:?}"
        );
    }

    match super::scan_continuation_marker("< x\r\nsecond") {
        super::MarkerScan::Marker {
            token: super::ContinuationToken::Join,
            task_source: Some(source),
        } => assert_eq!(source, "x\r\nsecond"),
        other => panic!("expected CRLF marker, got {other:?}"),
    }
    assert_eq!(marker_task("- < #task x").as_deref(), Some("- #task x"));
    assert!(matches!(
        super::scan_continuation_marker("<"),
        super::MarkerScan::Marker {
            task_source: None,
            ..
        }
    ));
}

#[test]
fn continuation_planner_chains_and_skips() {
    let a = highlight(1, "Page 1", Some(1), "A", None);
    let b = highlight(2, "Page 2", Some(2), "B", Some("<"));
    let c = highlight(3, "Page 3", Some(3), "C", Some("<"));
    let plan = super::plan_continuations(&[a.clone(), b.clone(), c.clone()]);
    assert_eq!(plan.head_of.get(&2), Some(&1));
    assert_eq!(plan.head_of.get(&3), Some(&1));
    assert_eq!(
        plan.followers.get(&1).map(Vec::as_slice),
        Some([(2, super::Seam::Join), (3, super::Seam::Join),].as_slice())
    );
    assert!(plan.dangling.is_empty());

    let mirror = sticky(2, "Page 1", "- status: wip\n- parent: obsidian");
    let sticky_note = sticky(2, "Page 1", "a personal note");
    let follower = highlight(3, "Page 2", Some(2), "B", Some("<"));
    let with_sticky =
        super::plan_continuations(&[a.clone(), sticky_note, follower.clone()]);
    assert_eq!(with_sticky.head_of.get(&3), Some(&1));
    let with_mirror = super::plan_continuations(&[a.clone(), mirror, follower]);
    assert_eq!(with_mirror.head_of.get(&3), Some(&1));
}

#[test]
fn continuation_dangling_and_literal_notes() {
    let first = highlight(1, "Page 1", Some(1), "A", Some("<"));
    let plan = super::plan_continuations(&[first.clone()]);
    assert_eq!(plan.dangling, vec![1]);
    assert!(plan.head_of.is_empty());

    let image = SidecarAnnotation {
        kind: SidecarAnnotationKind::Image,
        page_label: Some("Page 1".to_string()),
        page_index: Some(1),
        linked_page_style: false,
        text: "Figure".to_string(),
        comment: None,
        task_source: Some("< not a marker".to_string()),
        image: Some(SidecarImage {
            target: "assets/figure.png".to_string(),
            alt_text: Some("Figure".to_string()),
        }),
        order: 1,
        ordinal_on_page: 1,
    };
    let dangling_after_image = highlight(2, "Page 2", Some(2), "B", Some("<"));
    let plan = super::plan_continuations(&[image, dangling_after_image]);
    assert_eq!(plan.dangling, vec![2]);

    let sticky_literal = sticky(1, "Page 1", "< keep this");
    let caption = SidecarAnnotation {
        kind: SidecarAnnotationKind::Image,
        page_label: Some("Page 1".to_string()),
        page_index: Some(1),
        linked_page_style: false,
        text: "Figure".to_string(),
        comment: Some("< caption".to_string()),
        task_source: Some("< caption".to_string()),
        image: Some(SidecarImage {
            target: "assets/figure.png".to_string(),
            alt_text: Some("Figure".to_string()),
        }),
        order: 2,
        ordinal_on_page: 2,
    };
    let plan = super::plan_continuations(&[sticky_literal, caption]);
    assert!(plan.head_of.is_empty());
    assert!(plan.dangling.is_empty());
    assert!(plan.effective.is_empty());

    let bob_dir = temp_bob_dir("continuation-dangling-render");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let rendered = render_annotations(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![first],
        &ParsedNote::empty(),
        false,
    );
    assert_eq!(rendered.count, 1);
    assert_eq!(rendered.continuation_count, 0);
    assert_eq!(
        rendered.dangling_continuations,
        vec![super::DanglingContinuation {
            page_label: Some("Page 1".to_string()),
        }]
    );
    assert!(rendered.content.contains("<"), "{}", rendered.content);
}

#[test]
fn skipped_items_render_after_the_group() {
    let bob_dir = temp_bob_dir("continuation-skip");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let rendered = render_annotations(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![
            highlight(1, "Page 1", Some(1), "Head quote", None),
            sticky(2, "Page 1", "a personal note"),
            highlight(3, "Page 2", Some(2), "Follower quote", Some("<")),
        ],
        &ParsedNote::empty(),
        false,
    );
    let head_id = rendered.block_ids_by_annotation_order[&1].clone();
    let sticky_id = rendered.block_ids_by_annotation_order[&2].clone();
    let head_pos = rendered.content.find(&format!("^{head_id}")).unwrap();
    let sticky_pos = rendered.content.find(&format!("^{sticky_id}")).unwrap();
    assert!(head_pos < sticky_pos, "{}", rendered.content);
    assert!(
        rendered.content.contains("### Pages 1–2"),
        "{}",
        rendered.content
    );
    assert!(
        rendered
            .content
            .contains("### Page 1\n\n> [!note] a personal note")
            || rendered.content.contains("> [!note] a personal note"),
        "{}",
        rendered.content
    );
}

#[test]
fn continuation_seams_and_quote_assembly() {
    let join = super::assemble_group_quote(&[
        ("imple-", super::Seam::Join),
        ("mentation across", super::Seam::Join),
    ]);
    assert_eq!(join, "implementation across");

    let same_page = super::assemble_group_quote(&[
        ("left words", super::Seam::Join),
        ("right words", super::Seam::Gap),
    ]);
    assert_eq!(same_page, format!("left words [{ELLIPSIS}] right words"));

    let a = highlight(1, "Page 10", Some(10), "left words", None);
    let same = highlight(2, "Page 10", Some(10), "right words", Some("<"));
    let skipped = highlight(2, "Page 12", Some(12), "right words", Some("<"));
    let unknown = highlight(2, "Page 11", None, "right words", Some("<"));
    assert_eq!(
        super::plan_continuations(&[a.clone(), same]).followers[&1][0].1,
        super::Seam::Gap
    );
    assert_eq!(
        super::plan_continuations(&[a.clone(), skipped]).followers[&1][0].1,
        super::Seam::Gap
    );
    assert_eq!(
        super::plan_continuations(&[a, unknown]).followers[&1][0].1,
        super::Seam::Gap
    );

    let forced = super::plan_continuations(&[
        highlight(1, "Page 11", Some(11), "left", None),
        highlight(
            2,
            "Page 12",
            Some(12),
            "right",
            Some(&format!("<{ELLIPSIS}")),
        ),
    ]);
    assert_eq!(forced.followers[&1][0].1, super::Seam::Gap);

    let existing_gap = super::assemble_group_quote(&[
        (&format!("already [{ELLIPSIS}]"), super::Seam::Join),
        ("more", super::Seam::Gap),
    ]);
    assert_eq!(existing_gap, format!("already [{ELLIPSIS}] more"));
    let existing_dots = super::assemble_group_quote(&[
        ("already [...]", super::Seam::Join),
        ("more", super::Seam::Gap),
    ]);
    assert_eq!(existing_dots, "already [...] more");

    let stripped_left = super::strip_return_link_glyphs("hello ↩ p. 1ᵈ");
    let joined = super::assemble_group_quote(&[
        (stripped_left.as_str(), super::Seam::Join),
        ("world", super::Seam::Join),
    ]);
    assert_eq!(joined, "hello world");
    assert!(!joined.contains('↩'), "{joined}");
}

#[test]
fn continuation_group_page_labels() {
    assert_eq!(
        super::group_page_label(&[Some("Page 11"), Some("Page 12")]).as_deref(),
        Some("Pages 11–12")
    );
    assert_eq!(
        super::group_page_label(&[
            Some("Page 11"),
            Some("Page 12"),
            Some("Page 13"),
        ])
        .as_deref(),
        Some("Pages 11–13")
    );
    assert_eq!(
        super::group_page_label(&[Some("Page 11"), Some("Page 14")]).as_deref(),
        Some("Pages 11, 14")
    );
    assert_eq!(
        super::group_page_label(&[
            Some("Page 11"),
            Some("Page 12"),
            Some("Page 14"),
        ])
        .as_deref(),
        Some("Pages 11–12, 14")
    );
    assert_eq!(
        super::group_page_label(&[Some("p. 11"), Some("p. 12")]).as_deref(),
        Some("pp. 11–12")
    );
    assert_eq!(
        super::group_page_label(&[Some("Page 10"), Some("Page 10")]).as_deref(),
        Some("Page 10")
    );
    assert_eq!(
        super::group_page_label(&[Some("Appendix A"), Some("Page 2")])
            .as_deref(),
        Some("Appendix A")
    );
    assert_eq!(
        super::group_page_label(&[Some("Page 11")]).as_deref(),
        Some("Page 11")
    );
    assert!(super::group_page_label(&[Some("Page 11"), Some("Page 12")])
        .unwrap()
        .contains(EN_DASH));
}

#[test]
fn continuation_page_index_parsing() {
    let linked = parse_sidecar_markdown(
        "#### [Page 12](highlights://gas_town_emergency_user_manual#page=12)\n\n> quote\n",
    );
    assert_eq!(linked[0].page_index, Some(12));
    assert_eq!(linked[0].page_label.as_deref(), Some("Page 12"));

    let disagree = parse_sidecar_markdown(
        "#### [Page 12](highlights://example#page=99)\n\n> quote\n",
    );
    assert_eq!(disagree[0].page_index, Some(99));
    assert_eq!(disagree[0].page_label.as_deref(), Some("Page 12"));

    let unlinked = parse_sidecar_markdown("## Page 7\n\n> quote\n");
    assert_eq!(unlinked[0].page_index, Some(7));

    let no_number = parse_sidecar_markdown("## Page foo\n\n> quote\n");
    assert_eq!(no_number[0].page_index, None);
    assert_eq!(no_number[0].page_label.as_deref(), Some("Page foo"));
}

#[test]
fn gas_town_golden_merge_stub_and_idempotent_rerender() {
    let bob_dir = temp_bob_dir("gas-town-golden");
    let (pdf, ref_note, config) = gas_town_paths(&bob_dir);
    let annotations = parse_sidecar_markdown(GAS_TOWN_SIDECAR);
    assert_eq!(annotations.len(), 2);
    let head_id = super::annotation_block_id(&config, &pdf, &annotations[0]);
    let follower_id =
        super::annotation_block_id(&config, &pdf, &annotations[1]);
    assert_eq!(head_id, GAS_TOWN_HEAD_ID);
    assert_eq!(follower_id, GAS_TOWN_FOLLOWER_ID);

    let sidecar = SidecarInput {
        path: bob_dir.join("lib/blogs/gas_town_emergency_user_manual.md"),
        annotations: annotations.clone(),
    };
    let first = super::render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &sidecar,
        false,
    )
    .expect("render gas town");
    assert!(
        first.content.contains("### Pages 11–12"),
        "{}",
        first.content
    );
    assert!(
        first.content.contains("On every session startup,"),
        "{}",
        first.content
    );
    assert!(
        first.content.contains("PR Merge Master?!"),
        "{}",
        first.content
    );
    assert!(
        !first.content.contains("< PR Merge Master"),
        "{}",
        first.content
    );
    assert_eq!(first.count, 1);
    assert_eq!(first.continuation_count, 1);
    assert!(first.dangling_continuations.is_empty());
    assert!(
        !first.content.contains(&format!("^{follower_id}")),
        "{}",
        first.content
    );

    let prior = region_note(&format!(
        "### Page 11\n\n> [!quote] old head\n\n^{head_id}\n\n### Page 12\n\n> [!quote] old follower\n\n^{follower_id}\n"
    ));
    let with_prior = super::render_sidecar_highlights(
        &config, &pdf, &ref_note, &prior, &sidecar, false,
    )
    .expect("render with prior region");
    assert!(
        with_prior.content.contains("[!info] Merged highlight"),
        "{}",
        with_prior.content
    );
    assert!(
        with_prior.content.contains(&format!("^{follower_id}")),
        "{}",
        with_prior.content
    );
    assert!(
        !with_prior.content.contains("[!warning] Removed highlight"),
        "{}",
        with_prior.content
    );

    let rerender_note = region_note(&with_prior.content);
    let second = super::render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &rerender_note,
        &sidecar,
        false,
    )
    .expect("rerender");
    assert_eq!(second.content, with_prior.content);
}

#[test]
fn continuation_identity_toggle_is_stable() {
    let bob_dir = temp_bob_dir("continuation-identity");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let head = highlight(1, "Page 11", Some(11), "On every", None);
    let plain_follower =
        highlight(2, "Page 12", Some(12), "session startup", None);
    let merged_follower =
        highlight(2, "Page 12", Some(12), "session startup", Some("<"));
    let config = test_config_for_bob_dir(bob_dir.clone());
    let head_id = super::annotation_block_id(&config, &pdf, &head);
    let follower_id =
        super::annotation_block_id(&config, &pdf, &plain_follower);
    assert_eq!(head_id, super::annotation_block_id(&config, &pdf, &head));
    assert_eq!(
        follower_id,
        super::annotation_block_id(&config, &pdf, &merged_follower)
    );

    let pre = render_annotations(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![head.clone(), plain_follower.clone()],
        &ParsedNote::empty(),
        false,
    );
    assert_eq!(pre.block_ids_by_annotation_order[&1], head_id);
    assert_eq!(pre.block_ids_by_annotation_order[&2], follower_id);

    let merged = render_annotations(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![head.clone(), merged_follower.clone()],
        &region_note(&pre.content),
        false,
    );
    assert_eq!(merged.block_ids_by_annotation_order[&1], head_id);
    assert_eq!(merged.block_ids_by_annotation_order[&2], head_id);
    assert!(merged.content.contains("[!info] Merged highlight"));
    assert!(!merged.content.contains("[!warning] Removed highlight"));

    let unmerged = render_annotations(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![head, plain_follower],
        &region_note(&merged.content),
        false,
    );
    assert_eq!(unmerged.content, pre.content);
    assert!(!unmerged.content.contains("[!warning] Removed highlight"));
    assert!(!unmerged.content.contains("[!info] Merged highlight"));
}

#[test]
fn continuation_tasks_keep_piece_identity() {
    let bob_dir = temp_bob_dir("continuation-tasks");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let config = test_config_for_bob_dir(bob_dir.clone());
    let head = highlight(1, "Page 11", Some(11), "On every", None);
    let follower = highlight(
        2,
        "Page 12",
        Some(12),
        "session startup",
        Some("- < #task Check the sheriff"),
    );
    let sidecar = SidecarInput {
        path: bob_dir.join("lib/papers/example.md"),
        annotations: vec![head.clone(), follower.clone()],
    };
    let rendered = super::render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &sidecar,
        false,
    )
    .expect("render task group");
    let head_id = super::annotation_block_id(&config, &pdf, &head);
    let follower_id = super::annotation_block_id(&config, &pdf, &follower);
    let merged = super::annotation_task_candidates(
        &config,
        &ref_note,
        &pdf,
        Some(&sidecar),
        Some(&rendered),
        false,
    )
    .expect("merged tasks");
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].task_text, "#task Check the sheriff");
    assert_eq!(merged[0].source_block_id, head_id);
    let merged_processed = merged[0].processed_id.clone();

    let unmerged_follower = highlight(
        2,
        "Page 12",
        Some(12),
        "session startup",
        Some("- #task Check the sheriff"),
    );
    let unmerged_sidecar = SidecarInput {
        path: sidecar.path.clone(),
        annotations: vec![head, unmerged_follower],
    };
    let unmerged = super::annotation_task_candidates(
        &config,
        &ref_note,
        &pdf,
        Some(&unmerged_sidecar),
        None,
        false,
    )
    .expect("unmerged tasks");
    assert_eq!(unmerged[0].processed_id, merged_processed);
    assert_eq!(unmerged[0].source_block_id, follower_id);

    let sticky_note = sticky(1, "Page 1", "- #task Sticky follow-up");
    let sticky_sidecar = SidecarInput {
        path: sidecar.path,
        annotations: vec![sticky_note.clone()],
    };
    let sticky_candidates = super::annotation_task_candidates(
        &config,
        &ref_note,
        &pdf,
        Some(&sticky_sidecar),
        None,
        false,
    )
    .expect("sticky tasks");
    let sticky_id = super::annotation_block_id(&config, &pdf, &sticky_note);
    assert_eq!(sticky_candidates[0].source_block_id, sticky_id);
    assert_eq!(
        sticky_candidates[0].processed_id,
        super::annotation_task_processed_id(
            &config,
            &ref_note,
            &sticky_id,
            "#task Sticky follow-up",
        )
    );
}

#[test]
fn continuation_image_notes_embed_under_the_group() {
    let bob_dir = temp_bob_dir("continuation-image-notes");
    let source = bob_dir.join("shot.png");
    write_test_bytes(&source, b"png-bytes");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let rendered = render_annotations(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![
            highlight(1, "Page 11", Some(11), "On every", None),
            highlight(
                2,
                "Page 12",
                Some(12),
                "session startup",
                Some(&format!("<\n{}", source.display())),
            ),
        ],
        &ParsedNote::empty(),
        false,
    );
    assert!(
        rendered.content.contains("### Pages 11–12"),
        "{}",
        rendered.content
    );
    assert_eq!(rendered.image_note_count, 1);
    assert!(rendered.content.contains("![["), "{}", rendered.content);
    let quote_pos = rendered.content.find("[!quote]").unwrap();
    let image_pos = rendered.content.find("![[").unwrap();
    let id_pos = rendered.content.find('^').unwrap();
    assert!(
        quote_pos < image_pos && image_pos < id_pos,
        "{}",
        rendered.content
    );
}

#[test]
fn continuation_round_trip_group_and_stub() {
    let bob_dir = temp_bob_dir("continuation-round-trip");
    let (pdf, ref_note, config) = gas_town_paths(&bob_dir);
    let annotations = parse_sidecar_markdown(GAS_TOWN_SIDECAR);
    let head_id = super::annotation_block_id(&config, &pdf, &annotations[0]);
    let follower_id =
        super::annotation_block_id(&config, &pdf, &annotations[1]);
    let sidecar = SidecarInput {
        path: bob_dir.join("lib/blogs/gas_town_emergency_user_manual.md"),
        annotations,
    };
    let prior = region_note(&format!(
        "### Page 11\n\n> [!quote] old head\n\n^{head_id}\n\n### Page 12\n\n> [!quote] old follower\n\n^{follower_id}\n"
    ));
    let rendered = super::render_sidecar_highlights(
        &config, &pdf, &ref_note, &prior, &sidecar, false,
    )
    .expect("render");
    let parsed = super::parse_managed_region(&rendered.content);
    assert!(parsed.unparsed.is_empty(), "{parsed:?}");
    assert_eq!(parsed.removed, vec![follower_id.clone()]);
    let highlights: Vec<_> = parsed
        .blocks
        .iter()
        .filter(|block| block.kind == super::RegionBlockKind::Highlight)
        .collect();
    assert_eq!(highlights.len(), 1);
    assert_eq!(highlights[0].page_label.as_deref(), Some("Pages 11–12"));
    assert!(
        highlights[0]
            .quote
            .as_deref()
            .is_some_and(|quote| quote.contains("On every session startup,")),
        "{:?}",
        highlights[0].quote
    );
    assert_eq!(highlights[0].comment.as_deref(), Some("PR Merge Master?!"));
}

#[test]
fn continuation_page_index_boundary_never_joins_on_overflow() {
    let head = highlight(1, "Page 4294967295", Some(u32::MAX), "head", None);
    let wrap = highlight(2, "Page 0", Some(0), "wrapped", Some("<"));
    let plan = super::plan_continuations(&[head.clone(), wrap]);
    assert_eq!(
        plan.followers.get(&1).map(Vec::as_slice),
        Some([(2, super::Seam::Gap)].as_slice())
    );

    let same_max =
        highlight(1, "Page 4294967295", Some(u32::MAX), "head", None);
    let same_max_next =
        highlight(2, "Page 4294967295", Some(u32::MAX), "same", Some("<"));
    let plan = super::plan_continuations(&[same_max, same_max_next]);
    assert_eq!(
        plan.followers.get(&1).map(Vec::as_slice),
        Some([(2, super::Seam::Gap)].as_slice())
    );

    let before_max =
        highlight(1, "Page 4294967294", Some(u32::MAX - 1), "head", None);
    let at_max =
        highlight(2, "Page 4294967295", Some(u32::MAX), "tail", Some("<"));
    let plan = super::plan_continuations(&[before_max, at_max]);
    assert_eq!(
        plan.followers.get(&1).map(Vec::as_slice),
        Some([(2, super::Seam::Join)].as_slice())
    );

    let linked = parse_sidecar_markdown(
        "#### [Page 4294967295](highlights://example#page=4294967295)\n\n> head text\n\n***\n\n#### [Page 0](highlights://example#page=0)\n\n> tail text\n\n< joining note\n\n***\n",
    );
    assert_eq!(linked.len(), 2);
    assert_eq!(linked[0].page_index, Some(u32::MAX));
    assert_eq!(linked[1].page_index, Some(0));
    let plan = super::plan_continuations(&linked);
    let head_order = linked[0].order;
    let tail_order = linked[1].order;
    assert_eq!(
        plan.followers.get(&head_order).map(Vec::as_slice),
        Some([(tail_order, super::Seam::Gap)].as_slice())
    );
}

#[test]
fn continuation_short_page_labels_merge_into_plural_group() {
    let bob_dir = temp_bob_dir("continuation-short-labels");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let rendered = render_annotations(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![
            highlight(1, "p. 11", Some(11), "On every", None),
            highlight(2, "p. 12", Some(12), "session startup", Some("<")),
        ],
        &ParsedNote::empty(),
        false,
    );
    assert!(
        rendered.content.contains("### pp. 11–12"),
        "{}",
        rendered.content
    );
    assert_eq!(rendered.count, 1);
    assert_eq!(rendered.continuation_count, 1);
    let parsed = super::parse_managed_region(&rendered.content);
    assert!(parsed.unparsed.is_empty(), "{parsed:?}");
    let highlights: Vec<_> = parsed
        .blocks
        .iter()
        .filter(|block| block.kind == super::RegionBlockKind::Highlight)
        .collect();
    assert_eq!(highlights.len(), 1);
    assert_eq!(highlights[0].page_label.as_deref(), Some("pp. 11–12"));
}

#[test]
fn continuation_report_lines_cover_merged_and_dangling() {
    assert!(super::continuation_report_lines(0, &[]).is_empty());
    assert_eq!(
        super::continuation_report_lines(1, &[]),
        vec!["continuations: 1".to_string()]
    );
    assert_eq!(
        super::continuation_report_lines(
            0,
            &[super::DanglingContinuation {
                page_label: Some("Page 11".to_string()),
            }]
        ),
        vec![
            "continuations: 0".to_string(),
            "continuation_dangling: Page 11 (no earlier text highlight)"
                .to_string(),
        ]
    );
    assert_eq!(
        super::continuation_report_lines(
            2,
            &[super::DanglingContinuation { page_label: None }]
        ),
        vec![
            "continuations: 2".to_string(),
            "continuation_dangling: no page (no earlier text highlight)"
                .to_string(),
        ]
    );
}
