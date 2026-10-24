//! Underline classification, matching, emphasis, and render integration.
use super::*;
use std::ops::Range;

fn highlight(
    order: usize,
    page_label: &str,
    page_index: Option<u32>,
    text: &str,
    task_source: Option<&str>,
) -> SidecarAnnotation {
    SidecarAnnotation {
        kind: SidecarAnnotationKind::Highlight,
        page_label: Some(page_label.to_string()),
        page_index,
        linked_page_style: false,
        text: text.to_string(),
        section_heading: None,
        comment: task_source.map(str::to_string),
        task_source: task_source.map(str::to_string),
        image: None,
        order,
        ordinal_on_page: order,
    }
}

fn sidecar(annotations: Vec<SidecarAnnotation>) -> SidecarInput {
    SidecarInput {
        path: PathBuf::from("/tmp/bob/lib/papers/example.md"),
        annotations,
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
    let mut input = sidecar(annotations);
    input.path = bob_dir.join("lib/papers/example.md");
    render_sidecar_highlights(&config, pdf, ref_note, note, &input, false)
        .expect("render highlights")
}

fn note_with_region(region: &str) -> ParsedNote {
    ParsedNote {
        frontmatter: Vec::new(),
        body: format!(
            "<!-- highlights:begin -->\n\n{region}\n<!-- highlights:end -->"
        ),
        original: None,
    }
}

#[test]
fn underline_classifier_accepts_only_fully_bold_rows() {
    assert_eq!(underline_inner("**one phrase**"), Some("one phrase".into()));
    assert_eq!(
        underline_inner("**first\nline**\n**second**"),
        Some("first\nline\nsecond".into())
    );
    assert_eq!(
        underline_inner("  **first**\n\n**second**  "),
        Some("first\n\nsecond".into())
    );
    assert_eq!(underline_inner("**** **words**"), Some(" words".into()));
    assert_eq!(underline_inner("ordinary **partial** text"), None);
    assert_eq!(underline_inner("**unpaired"), None);
    assert_eq!(underline_inner("****"), None);
    assert_eq!(underline_inner("**one** other **two**"), None);
}

#[test]
fn underline_match_key_and_host_rules_fail_closed() {
    let cases = [
        ("The well-being result is useful.", "**well being**", true),
        ("The embed-\nding system works.", "**embedding**", false),
        ("An embed\u{00ad}ding works.", "**embedding**", true),
        ("A ﬁne result.", "**fine**", true),
        ("Read this returnᵃ now.", "**returnᵇ now**", true),
    ];
    for (host_text, underline_text, strip_return_links) in cases {
        let host = highlight(1, "Page 1", Some(1), host_text, None);
        let underline = highlight(2, "Page 1", Some(1), underline_text, None);
        let plan = plan_underlines(&[host, underline], strip_return_links);
        assert_eq!(plan.host_of.get(&2), Some(&1), "{host_text:?}");
    }

    let host = highlight(1, "Page 1", Some(1), "A distinct phrase here.", None);
    let before = highlight(2, "Page 1", Some(1), "**distinct phrase**", None);
    assert_eq!(
        plan_underlines(&[before, host], false).host_of.get(&2),
        Some(&1)
    );

    let host = highlight(1, "Page 1", Some(1), "A distinct phrase here.", None);
    let underline =
        highlight(2, "Page 2", Some(2), "**distinct phrase**", None);
    assert!(plan_underlines(&[host, underline], false)
        .host_of
        .is_empty());

    let first =
        highlight(1, "Page 1", Some(1), "A distinct phrase here.", None);
    let second =
        highlight(2, "Page 1", Some(1), "The distinct phrase repeats.", None);
    let underline =
        highlight(3, "Page 1", Some(1), "**distinct phrase**", None);
    let plan = plan_underlines(&[first, second, underline], false);
    assert!(plan.host_of.is_empty());
    assert!(plan.standalone.contains(&3));

    let host = highlight(
        1,
        "Page 1",
        Some(1),
        "A phrase repeats: phrase, phrase.",
        None,
    );
    let underline = highlight(2, "Page 1", Some(1), "**phrase**", None);
    let plan = plan_underlines(&[host, underline], false);
    assert!(plan.host_of.is_empty());
    assert!(plan.standalone.contains(&2));

    let host = highlight(1, "Page 1", Some(1), "It approves the change.", None);
    let underline = highlight(2, "Page 1", Some(1), "**proves**", None);
    let plan = plan_underlines(&[host, underline], false);
    assert!(plan.host_of.is_empty());
    assert!(plan.standalone.contains(&2));

    let host = highlight(1, "Page 1", Some(1), "It approves the change.", None);
    let underline = highlight(2, "Page 1", Some(1), "**approves**", None);
    assert_eq!(
        plan_underlines(&[host, underline], false).host_of.get(&2),
        Some(&1)
    );

    let host = highlight(1, "Page 1", None, "A phrase on the same page.", None);
    let underline = highlight(2, "Page 1", Some(1), "**phrase**", None);
    assert_eq!(
        plan_underlines(&[host, underline], false).host_of.get(&2),
        Some(&1)
    );

    let mut marker_mirror = highlight(
        1,
        "Page 1",
        Some(1),
        "**status: wip parent: obsidian**",
        Some("- status: wip\n- parent: obsidian"),
    );
    marker_mirror.linked_page_style = true;
    let plan = plan_underlines(&[marker_mirror], false);
    assert!(plan.host_of.is_empty());
    assert!(plan.standalone.is_empty());
}

#[test]
fn continuation_markers_are_respected_and_absorbed_rows_do_not_block_them() {
    let host = highlight(1, "Page 11", Some(11), "The important phrase", None);
    let underline = highlight(
        2,
        "Page 11",
        Some(11),
        "**important phrase**",
        Some("- < Keep this explicit"),
    );
    let marker_plan = plan_annotation_folds(&[host.clone(), underline], false);
    assert!(marker_plan.underlines.host_of.is_empty());
    assert!(marker_plan.underlines.standalone.is_empty());

    let escaped = highlight(
        2,
        "Page 11",
        Some(11),
        "**important phrase**",
        Some("- \\< ordinary note"),
    );
    let escaped_plan = plan_annotation_folds(&[host.clone(), escaped], false);
    assert_eq!(escaped_plan.underlines.host_of.get(&2), Some(&1));

    let host = highlight(1, "Page 11", Some(11), "The important phrase", None);
    let underline =
        highlight(2, "Page 11", Some(11), "**important phrase**", None);
    let follower = highlight(
        3,
        "Page 12",
        Some(12),
        "continues into the next section",
        Some("- < Follow the thread"),
    );
    let folds = plan_annotation_folds(&[host, underline, follower], false);
    assert_eq!(folds.underlines.host_of.get(&2), Some(&1));
    assert_eq!(folds.continuations.followers[&1][0].0, 3);

    let host = highlight(1, "Page 1", Some(1), "secret passage", None);
    let follower = highlight(
        2,
        "Page 1",
        Some(1),
        "secret passage ends here",
        Some("- < marker"),
    );
    let underline = highlight(3, "Page 1", Some(1), "**ends here**", None);
    let folds = plan_annotation_folds(&[host, follower, underline], false);
    assert_eq!(folds.underlines.host_of.get(&3), Some(&2));
    assert_eq!(folds.continuations.head_of.get(&2), Some(&1));
}

#[test]
fn image_selections_and_sticky_notes_cannot_host_underlines() {
    let underline =
        highlight(3, "Page 1", Some(1), "**a unique phrase**", None);
    let mut sticky = highlight(1, "Page 1", Some(1), "a unique phrase", None);
    sticky.kind = SidecarAnnotationKind::StandaloneNote;
    let mut image = highlight(2, "Page 1", Some(1), "a unique phrase", None);
    image.kind = SidecarAnnotationKind::Image;
    let plan = plan_underlines(&[sticky, image, underline], false);
    assert!(plan.host_of.is_empty());
    assert!(plan.standalone.contains(&3));
}

fn unique_emphasis_ranges(quote: &str, underline: &str) -> Vec<Range<usize>> {
    let start =
        unique_key_start(quote, underline).expect("unique emphasis key");
    emphasis_ranges(quote, &[quote.to_string()], 0, underline, start)
}

#[test]
fn emphasis_snaps_splits_flanks_and_merges_ranges() {
    let quote = "We didn’t finish the migration.";
    let ranges = unique_emphasis_ranges(quote, "didn’");
    assert_eq!(
        apply_emphasis(quote, &ranges),
        "We **didn’t** finish the migration."
    );
    let quote = "what didn’t change";
    let ranges = unique_emphasis_ranges(quote, "what didn’");
    assert_eq!(apply_emphasis(quote, &ranges), "**what didn’t** change");

    let quote = "integrate the result";
    let ranges = unique_emphasis_ranges(quote, "tegr");
    assert_eq!(apply_emphasis(quote, &ranges), "**integrate** the result");

    let quote = "lead-in words before the marked span";
    let ranges = unique_emphasis_ranges(quote, "mark");
    assert_eq!(
        apply_emphasis(quote, &ranges),
        "lead-in words before the **marked** span"
    );

    let quote = "word( text";
    let ranges = unique_emphasis_ranges(quote, "( text");
    assert_eq!(apply_emphasis(quote, &ranges), "**word( text**");

    let quote = "foo!bar";
    let ranges = unique_emphasis_ranges(quote, "foo!");
    assert_eq!(apply_emphasis(quote, &ranges), "**foo!bar**");

    let quote = "one two\nthree four";
    let ranges = unique_emphasis_ranges(quote, "two three");
    assert_eq!(
        apply_emphasis(quote, &ranges),
        "one **two**\n**three** four"
    );

    let quote = "alpha beta gamma delta";
    let mut ranges = unique_emphasis_ranges(quote, "beta");
    ranges.extend(unique_emphasis_ranges(quote, "delta"));
    assert_eq!(
        apply_emphasis(quote, &ranges),
        "alpha **beta** gamma **delta**"
    );
    assert_eq!(
        apply_emphasis("alpha beta gamma", &[0..5, 6..10]),
        "**alpha beta** gamma"
    );
    assert_eq!(
        apply_emphasis("whole quote", &[0..5, 4..11]),
        "**whole quote**"
    );
    let already_strong = "before **strong phrase** after";
    let strong_start = already_strong.find("strong").expect("strong start");
    let strong_end = strong_start + "strong phrase".len();
    assert_eq!(
        apply_emphasis(already_strong, &[strong_start..strong_end]),
        already_strong
    );
    assert_eq!(
        standalone_underline_quote("alpha beta\n\ngamma"),
        "**alpha beta**\n\n**gamma**"
    );
}

#[test]
fn absorbed_underline_renders_comment_image_and_task_on_host() {
    let bob_dir = temp_bob_dir("underline-render");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let image_path = bob_dir.join("lib/papers/figure.png");
    write_test_bytes(&image_path, &test_png_bytes(100, 100, None));

    let host = highlight(
        1,
        "Page 1",
        Some(1),
        "The decision matters because it keeps work moving.",
        Some("Host context."),
    );
    let underline = highlight(
        2,
        "Page 1",
        Some(1),
        "**decision matters**",
        Some(&format!(
            "Comment: Keep this close.\n- {}\n- #task Track the decision.",
            image_path.display()
        )),
    );
    let underline_id = annotation_block_id(
        &test_config_for_bob_dir(bob_dir.clone()),
        &pdf,
        &underline,
    );
    let sidecar = sidecar(vec![host.clone(), underline.clone()]);
    let config = test_config_for_bob_dir(bob_dir.clone());
    let rendered = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &sidecar,
        false,
    )
    .expect("render underline fold");
    let host_id = annotation_block_id(&config, &pdf, &host);
    assert_eq!(rendered.count, 1);
    assert_eq!(rendered.block_ids_by_annotation_order[&2], host_id);
    assert!(
        rendered
            .content
            .contains("The **decision matters** because it keeps work moving."),
        "{}",
        rendered.content
    );
    assert!(
        rendered
            .content
            .contains("“decision matters” — Keep this close."),
        "{}",
        rendered.content
    );
    assert!(rendered.content.contains("![[ref/papers/example.assets/"));
    assert_eq!(rendered.image_note_count, 1);

    let candidates = annotation_task_candidates(
        &config,
        &ref_note,
        &pdf,
        Some(&sidecar),
        Some(&rendered),
        false,
    )
    .expect("underline task candidate");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].source_block_id, host_id);
    assert_eq!(
        candidates[0].processed_id,
        annotation_task_processed_id(
            &config,
            &ref_note,
            &underline_id,
            &candidates[0].identity
        )
    );
}

#[test]
fn absorbed_identity_and_link_aware_stub_are_reversible() {
    let bob_dir = temp_bob_dir("underline-identity");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let config = test_config_for_bob_dir(bob_dir.clone());
    let host = highlight(1, "Page 1", Some(1), "A unique passage here.", None);
    let underline = highlight(2, "Page 1", Some(1), "**unique passage**", None);
    let host_id = annotation_block_id(&config, &pdf, &host);
    let underline_id = annotation_block_id(&config, &pdf, &underline);
    let old_region = format!(
        "### Page 1\n\n> [!quote] **unique passage**\n\n^{underline_id}"
    );
    let old_note = note_with_region(&old_region);

    let baseline = render(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![host.clone()],
        &ParsedNote::empty(),
    );
    let merged = render(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![host.clone(), underline.clone()],
        &old_note,
    );
    assert_eq!(baseline.block_ids_by_annotation_order[&1], host_id);
    assert_eq!(merged.block_ids_by_annotation_order[&1], host_id);
    assert!(!merged.content.contains(&underline_id));
    assert!(!merged.content.contains("Removed highlight"));

    write_test_file(
        &bob_dir.join("sase.md"),
        &format!("A task bookmark: [[ref/example#^{underline_id}|🔖]].\n"),
    );
    let linked = render(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![host.clone(), underline.clone()],
        &old_note,
    );
    assert!(linked.content.contains("[!info] Merged emphasis"));
    assert!(linked.content.contains(&format!(
        "Now emphasized in [[#^{host_id}|the Page 1 highlight]]."
    )));
    assert!(linked.content.contains(&format!("^{underline_id}")));
    let round_trip = parse_managed_region(&linked.content);
    assert_eq!(round_trip.blocks.len(), 1, "{round_trip:?}");
    assert!(round_trip.removed.contains(&underline_id));
    assert!(round_trip.unparsed.is_empty(), "{round_trip:?}");

    let same_scan = render(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![host.clone(), underline.clone()],
        &note_with_region(&linked.content),
    );
    assert_eq!(linked.content, same_scan.content);

    let persisted_note = note_with_region(&linked.content);
    fs::remove_file(bob_dir.join("sase.md")).expect("remove task link");
    let persisted = render(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![host.clone(), underline.clone()],
        &persisted_note,
    );
    assert!(persisted.content.contains("[!info] Merged emphasis"));

    let deleted = render(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![host.clone()],
        &persisted_note,
    );
    assert!(deleted.content.contains("[!warning] Removed highlight"));
    assert!(deleted.content.contains(&format!("^{underline_id}")));

    let host_deleted = render(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![underline],
        &note_with_region(&linked.content),
    );
    assert!(host_deleted
        .content
        .contains("> [!quote] **unique passage**"));
    assert!(host_deleted.content.contains(&format!("^{underline_id}")));
}

#[test]
fn underline_report_lines_explain_standalone_reasons() {
    let standalone = vec![
        StandaloneUnderline {
            page_label: Some("Page 2".into()),
            reason: underline::StandaloneUnderlineReason::NoHost,
        },
        StandaloneUnderline {
            page_label: None,
            reason: underline::StandaloneUnderlineReason::AmbiguousHost {
                hosts: 2,
            },
        },
        StandaloneUnderline {
            page_label: Some("Page 4".into()),
            reason: underline::StandaloneUnderlineReason::RepeatedInHost,
        },
    ];
    assert_eq!(
        underline_report_lines(3, &standalone),
        vec![
            "underlines_merged: 3",
            "underline_standalone: Page 2 (no highlight around it)",
            "underline_standalone: no page (inside 2 highlights)",
            "underline_standalone: Page 4 (text repeats inside its highlight)",
        ]
    );
    assert_eq!(
        underline_report_lines(0, &standalone)[0],
        "underlines_merged: 0"
    );
}

#[test]
fn accepted_whole_word_occurrence_renders_inside_the_host() {
    let quote = "It approves and proves the change.";
    let host = highlight(1, "Page 1", Some(1), quote, None);
    let underline = highlight(2, "Page 1", Some(1), "**proves**", None);
    let plan = plan_underlines(&[host.clone(), underline.clone()], false);
    let start = *plan.host_key_start.get(&2).expect("accepted occurrence");
    assert_eq!(plan.host_of.get(&2), Some(&1));
    assert_eq!(unique_key_start(quote, "proves"), None);
    let ranges =
        emphasis_ranges(quote, &[quote.to_string()], 0, "proves", start);
    assert_eq!(
        apply_emphasis(quote, &ranges),
        "It approves and **proves** the change."
    );

    let bob_dir = temp_bob_dir("underline-whole-word");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let rendered = render(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![host, underline],
        &ParsedNote::empty(),
    );
    assert_eq!(rendered.count, 1);
    assert_eq!(rendered.underlines_merged, 1);
    assert!(rendered.standalone_underlines.is_empty());
    assert!(
        rendered
            .content
            .contains("It approves and **proves** the change."),
        "{}",
        rendered.content
    );
    assert_eq!(
        rendered.content.matches("**proves**").count(),
        1,
        "{}",
        rendered.content
    );
}

#[test]
fn whole_word_emphasis_ignores_subword_order_and_sidecar_order() {
    let bob_dir = temp_bob_dir("underline-order");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");

    let host = highlight(
        2,
        "Page 1",
        Some(1),
        "It proves and approves the change.",
        None,
    );
    let underline = highlight(1, "Page 1", Some(1), "**proves**", None);
    let rendered = render(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![underline, host],
        &ParsedNote::empty(),
    );
    assert_eq!(rendered.underlines_merged, 1);
    assert!(
        rendered
            .content
            .contains("It **proves** and approves the change."),
        "{}",
        rendered.content
    );
    assert!(
        !rendered.content.contains("**approves**"),
        "{}",
        rendered.content
    );
}

#[test]
fn repeated_whole_word_and_inside_word_stay_standalone() {
    let bob_dir = temp_bob_dir("underline-standalone");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");

    let repeated_host =
        highlight(1, "Page 1", Some(1), "It proves then proves again.", None);
    let repeated_underline =
        highlight(2, "Page 1", Some(1), "**proves**", None);
    let repeated = render(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![repeated_host, repeated_underline],
        &ParsedNote::empty(),
    );
    assert_eq!(repeated.underlines_merged, 0);
    assert_eq!(repeated.standalone_underlines.len(), 1);
    assert_eq!(
        repeated.standalone_underlines[0].reason,
        StandaloneUnderlineReason::RepeatedInHost
    );
    assert!(
        repeated
            .content
            .contains("> [!quote] It proves then proves again."),
        "{}",
        repeated.content
    );
    assert!(
        repeated.content.contains("> [!quote] **proves**"),
        "{}",
        repeated.content
    );

    let inside_host =
        highlight(1, "Page 1", Some(1), "It approves the change.", None);
    let inside_underline = highlight(2, "Page 1", Some(1), "**proves**", None);
    let inside = render(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![inside_host, inside_underline],
        &ParsedNote::empty(),
    );
    assert_eq!(inside.underlines_merged, 0);
    assert_eq!(inside.standalone_underlines.len(), 1);
    assert_eq!(
        inside.standalone_underlines[0].reason,
        StandaloneUnderlineReason::NoHost
    );
    assert!(
        inside
            .content
            .contains("> [!quote] It approves the change."),
        "{}",
        inside.content
    );
    assert!(
        inside.content.contains("> [!quote] **proves**"),
        "{}",
        inside.content
    );
}

#[test]
fn continuation_member_keeps_emphasis_and_comments_with_each_piece() {
    let bob_dir = temp_bob_dir("underline-continuation-window");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let host = highlight(
        1,
        "Page 1",
        Some(1),
        "It approves and proves the change.",
        None,
    );
    let follower = highlight(
        2,
        "Page 3",
        Some(3),
        "Later it proves nothing else.",
        Some("- < continue"),
    );
    let head_underline = highlight(3, "Page 1", Some(1), "**approves**", None);
    let follower_underline = highlight(
        4,
        "Page 3",
        Some(3),
        "**proves nothing else**",
        Some("Comment: follower note"),
    );
    let rendered = render(
        &bob_dir,
        &pdf,
        &ref_note,
        vec![host, follower, head_underline, follower_underline],
        &ParsedNote::empty(),
    );
    assert_eq!(rendered.count, 1);
    assert_eq!(rendered.underlines_merged, 2);
    assert_eq!(rendered.continuation_count, 1);
    let parsed = parse_managed_region(&rendered.content);
    assert!(parsed.unparsed.is_empty(), "{parsed:?}");
    assert_eq!(parsed.blocks.len(), 1);
    assert_eq!(parsed.blocks[0].pieces.len(), 2);
    assert!(
        parsed.blocks[0].pieces[0]
            .quote
            .contains("It **approves** and proves the change."),
        "{}",
        rendered.content
    );
    assert!(
        parsed.blocks[0].pieces[1]
            .quote
            .contains("Later it **proves nothing else**."),
        "{}",
        rendered.content
    );
    assert_eq!(
        parsed.blocks[0].pieces[1].comment.as_deref(),
        Some("continue\n\n“proves nothing else” — follower note")
    );
}
