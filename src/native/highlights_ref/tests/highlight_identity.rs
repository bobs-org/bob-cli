//! Stable highlight identity across insertion and underline absorption (bob-cli-9o).
use super::*;

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

#[test]
fn highlight_identity_survives_insertion_and_underline_absorption() {
    let bob_dir = temp_bob_dir("highlight-identity-insert");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let config = test_config_for_bob_dir(bob_dir.clone());

    let first_sidecar = SidecarInput {
        path: bob_dir.join("lib/papers/example.md"),
        annotations: parse_sidecar_markdown(
            "## Page 9\n\n> first quote\n\n- #task Review first.\n\n---\n\n> second quote\n\n- #task Review second.\n\n---\n\n> third quote\n\n- #task Review third.\n",
        ),
    };
    let first = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &first_sidecar,
        false,
    )
    .expect("render three highlights");
    assert_eq!(first.count, 3);
    assert!(!first.content.contains("Removed highlights"));
    let first_ids: Vec<String> = first
        .block_ids_by_annotation_order
        .values()
        .cloned()
        .collect();
    assert_eq!(first_ids.len(), 3);
    let first_live = live_ids(&first.content);
    assert_eq!(first_live.len(), 3);
    for id in &first_ids {
        assert!(first_live.contains(id));
    }

    let inserted_sidecar = SidecarInput {
        path: first_sidecar.path.clone(),
        annotations: parse_sidecar_markdown(
            "## Page 9\n\n> new top quote\n\n- #task Review new top.\n\n---\n\n> first quote\n\n- #task Review first.\n\n---\n\n> second quote\n\n- #task Review second.\n\n---\n\n> third quote\n\n- #task Review third.\n",
        ),
    };
    let second = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &region_note(&first.content),
        &inserted_sidecar,
        false,
    )
    .expect("rescan after insertion above");
    assert_eq!(second.count, 4);
    assert!(
        !second.content.contains("Removed highlights"),
        "{}",
        second.content
    );
    for id in &first_ids {
        assert!(
            second.content.contains(&format!("^{id}")),
            "preserved {id} in {}",
            second.content
        );
    }
    let second_live = live_ids(&second.content);
    assert_eq!(second_live.len(), 4);
    for id in &first_ids {
        assert!(second_live.contains(id));
    }
    let new_id = second
        .block_ids_by_annotation_order
        .get(&1)
        .expect("new top order")
        .clone();
    assert!(!first_ids.contains(&new_id));

    let candidates = annotation_task_candidates(
        &config,
        &ref_note,
        &pdf,
        Some(&inserted_sidecar),
        Some(&second),
        false,
    )
    .expect("tasks after insertion");
    assert!(!candidates.is_empty());
    for candidate in &candidates {
        assert!(
            second_live.contains(&candidate.source_block_id),
            "task {} links to live {}",
            candidate.task_text,
            candidate.source_block_id
        );
        assert!(
            !second.content.contains("Removed highlights"),
            "no false tombstone for {}",
            candidate.task_text
        );
    }

    let underline_sidecar = SidecarInput {
        path: first_sidecar.path.clone(),
        annotations: parse_sidecar_markdown(
            "## Page 9\n\n> new top quote\n\n- #task Review new top.\n\n---\n\n> first quote\n\n- #task Review first.\n\n---\n\n> **second**\n\n- #task Emphasize second.\n\n---\n\n> second quote\n\n- #task Review second.\n\n---\n\n> third quote\n\n- #task Review third.\n",
        ),
    };
    let third = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &region_note(&second.content),
        &underline_sidecar,
        false,
    )
    .expect("rescan after absorbed underline above host");
    assert!(
        !third.content.contains("[!warning] Removed highlight"),
        "{}",
        third.content
    );
    for id in &first_ids {
        assert!(
            third.content.contains(&format!("^{id}")),
            "preserved {id} after underline in {}",
            third.content
        );
    }
    assert!(third.content.contains(&format!("^{new_id}")));
    let third_live = live_ids(&third.content);
    for id in &first_ids {
        assert!(third_live.contains(id));
    }
    assert!(third_live.contains(&new_id));

    let underline_candidates = annotation_task_candidates(
        &config,
        &ref_note,
        &pdf,
        Some(&underline_sidecar),
        Some(&third),
        false,
    )
    .expect("tasks after underline absorption");
    assert!(!underline_candidates.is_empty());
    for candidate in &underline_candidates {
        assert!(
            third_live.contains(&candidate.source_block_id),
            "task {} links to live {}",
            candidate.task_text,
            candidate.source_block_id
        );
    }

    let rerender = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &region_note(&third.content),
        &underline_sidecar,
        false,
    )
    .expect("idempotent rescan");
    assert_eq!(rerender.content, third.content);
}

#[test]
fn heading_shaped_quote_wrap_remints_highlight_id() {
    let bob_dir = temp_bob_dir("highlight-quote-heading-remint");
    let pdf = bob_dir.join("lib/papers/quote-heading.pdf");
    let ref_note = bob_dir.join("ref/papers/quote-heading.md");
    let config = test_config_for_bob_dir(bob_dir.clone());

    let truncated_sidecar = SidecarInput {
        path: bob_dir.join("lib/papers/quote-heading.md"),
        annotations: parse_sidecar_markdown(
            "\
#### [Page 8](highlights://quote-heading#page=8)

##### 2026-06-15:

> # Set custom agent command
gt config agent set claude-glm
",
        ),
    };
    let truncated = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &truncated_sidecar,
        false,
    )
    .expect("render truncated quote");
    assert_eq!(truncated.count, 1);
    let old_id = truncated
        .block_ids_by_annotation_order
        .values()
        .next()
        .expect("truncated id")
        .clone();
    assert!(
        !truncated.content.contains("# Set default agent"),
        "{}",
        truncated.content
    );

    let wrapped_sidecar = SidecarInput {
        path: truncated_sidecar.path.clone(),
        annotations: parse_sidecar_markdown(
            "\
#### [Page 8](highlights://quote-heading#page=8)

##### 2026-06-15:

> # Set custom agent command
gt config agent set claude-glm
# Set default agent
gt config default-agent
",
        ),
    };
    let wrapped = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &region_note(&truncated.content),
        &wrapped_sidecar,
        false,
    )
    .expect("remint after quote text changes");
    assert_eq!(wrapped.count, 1);
    let new_id = wrapped
        .block_ids_by_annotation_order
        .values()
        .next()
        .expect("wrapped id")
        .clone();
    assert_ne!(old_id, new_id);
    assert!(
        wrapped.content.contains(&format!("^{new_id}")),
        "{}",
        wrapped.content
    );
    assert!(
        wrapped.content.contains("# Set default agent")
            && wrapped.content.contains("gt config default-agent"),
        "{}",
        wrapped.content
    );
    assert!(
        !wrapped
            .content
            .contains("[!note] Comment gt config default-agent"),
        "{}",
        wrapped.content
    );
    assert!(
        wrapped.content.contains("[!warning] Removed highlight")
            && wrapped.content.contains(&format!("^{old_id}")),
        "changed quote text remints and tombstones {old_id}: {}",
        wrapped.content
    );
    let live = live_ids(&wrapped.content);
    assert!(live.contains(&new_id));
    assert!(!live.contains(&old_id));
}

#[test]
fn color_grouped_quote_fix_remints_highlight_id() {
    // bob-cli-bf with color_grouped_quote_id = remint: when the
    // color-grouped quote fix changes a live highlight's quote text, the id
    // re-mints and the old id tombstones, as batch 5 did for linked-page
    // quotes.
    let bob_dir = temp_bob_dir("highlight-color-grouped-remint");
    let pdf = bob_dir.join("lib/papers/color-grouped.pdf");
    let ref_note = bob_dir.join("ref/papers/color-grouped.md");
    let config = test_config_for_bob_dir(bob_dir.clone());

    let truncated_sidecar = SidecarInput {
        path: bob_dir.join("lib/papers/color-grouped.md"),
        annotations: parse_sidecar_markdown(
            "#### Yellow\n\n##### 2026-06-06:\n\n> Execution host first. Put the always-online server on a private\n> tailnet and run the\n",
        ),
    };
    let truncated = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &truncated_sidecar,
        false,
    )
    .expect("render truncated color-grouped quote");
    assert_eq!(truncated.count, 1);
    let old_id = truncated
        .block_ids_by_annotation_order
        .values()
        .next()
        .expect("truncated id")
        .clone();
    assert!(
        !truncated.content.contains("mobile gateway"),
        "{}",
        truncated.content
    );

    let wrapped_sidecar = SidecarInput {
        path: truncated_sidecar.path.clone(),
        annotations: parse_sidecar_markdown(
            "#### Yellow\n\n##### 2026-06-06:\n\n> Execution host first. Put the always-online server on a private\n> tailnet and run the\nexisting SASE mobile gateway there.\n",
        ),
    };
    let wrapped = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &region_note(&truncated.content),
        &wrapped_sidecar,
        false,
    )
    .expect("remint after color-grouped quote text changes");
    assert_eq!(wrapped.count, 1);
    let new_id = wrapped
        .block_ids_by_annotation_order
        .values()
        .next()
        .expect("wrapped id")
        .clone();
    assert_ne!(old_id, new_id);
    assert!(
        wrapped.content.contains(&format!("^{new_id}")),
        "{}",
        wrapped.content
    );
    assert!(
        wrapped
            .content
            .contains("existing SASE mobile gateway there."),
        "{}",
        wrapped.content
    );
    assert!(
        wrapped.content.contains("[!warning] Removed highlight")
            && wrapped.content.contains(&format!("^{old_id}")),
        "changed quote text remints and tombstones {old_id}: {}",
        wrapped.content
    );
    let live = live_ids(&wrapped.content);
    assert!(live.contains(&new_id));
    assert!(!live.contains(&old_id));
}

#[test]
fn highlight_identity_handles_duplicates_comments_legacy_and_folds() {
    let bob_dir = temp_bob_dir("highlight-identity-vectors");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let config = test_config_for_bob_dir(bob_dir.clone());

    let dup_sidecar = SidecarInput {
        path: bob_dir.join("lib/papers/example.md"),
        annotations: parse_sidecar_markdown(
            "## Page 1\n\n> same text\n\n---\n\n> same text\n\n",
        ),
    };
    let dup = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &dup_sidecar,
        false,
    )
    .expect("duplicate text on one page");
    assert_eq!(dup.count, 2);
    let dup_ids: Vec<String> = dup
        .block_ids_by_annotation_order
        .values()
        .cloned()
        .collect();
    assert_ne!(dup_ids[0], dup_ids[1]);

    let cross_sidecar = SidecarInput {
        path: dup_sidecar.path.clone(),
        annotations: parse_sidecar_markdown(
            "## Page 1\n\n> same text\n\n---\n\n## Page 2\n\n> same text\n\n",
        ),
    };
    let cross = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &cross_sidecar,
        false,
    )
    .expect("same text across pages");
    let cross_ids: Vec<String> = cross
        .block_ids_by_annotation_order
        .values()
        .cloned()
        .collect();
    assert_ne!(cross_ids[0], cross_ids[1]);

    let kind_sidecar = SidecarInput {
        path: dup_sidecar.path.clone(),
        annotations: parse_sidecar_markdown(
            "## Page 1\n\n> same text\n\n---\n\nsame text\n",
        ),
    };
    let kinds = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &kind_sidecar,
        false,
    )
    .expect("same text across kinds");
    let kind_ids: Vec<String> = kinds
        .block_ids_by_annotation_order
        .values()
        .cloned()
        .collect();
    assert_eq!(kind_ids.len(), 2);
    assert_ne!(kind_ids[0], kind_ids[1]);

    let comment_sidecar = SidecarInput {
        path: dup_sidecar.path.clone(),
        annotations: parse_sidecar_markdown(
            "## Page 3\n\n> keep me\n\nFirst comment.\n\n---\n\n> gone\n\n",
        ),
    };
    let commented = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &comment_sidecar,
        false,
    )
    .expect("comment baseline");
    let keep_id = commented
        .block_ids_by_annotation_order
        .get(&1)
        .expect("keep id")
        .clone();
    let edited_sidecar = SidecarInput {
        path: dup_sidecar.path.clone(),
        annotations: parse_sidecar_markdown(
            "## Page 3\n\n> keep me\n\nEdited comment.\n\n---\n\n> gone\n\n",
        ),
    };
    let edited = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &region_note(&commented.content),
        &edited_sidecar,
        false,
    )
    .expect("comment edit preserves ID");
    assert!(edited.content.contains(&format!("^{keep_id}")));
    assert!(!edited.content.contains("Removed highlights"));

    let removed_sidecar = SidecarInput {
        path: dup_sidecar.path.clone(),
        annotations: parse_sidecar_markdown(
            "## Page 3\n\n> keep me\n\nEdited comment.\n",
        ),
    };
    let removed = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &region_note(&edited.content),
        &removed_sidecar,
        false,
    )
    .expect("real removal tombstones");
    assert!(removed.content.contains("[!warning] Removed highlight"));
    assert!(removed.content.contains(&format!("^{keep_id}")));

    let rerender = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &region_note(&removed.content),
        &removed_sidecar,
        false,
    )
    .expect("idempotent after removal");
    assert_eq!(rerender.content, removed.content);

    let legacy_prior = region_note(&format!(
        "### Page 4\n\n> [!quote] legacy quote\n\n^h-aaaaaaaaaaaaaaaa\n"
    ));
    let legacy_sidecar = SidecarInput {
        path: dup_sidecar.path.clone(),
        annotations: parse_sidecar_markdown("## Page 4\n\n> legacy quote\n"),
    };
    let legacy = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &legacy_prior,
        &legacy_sidecar,
        false,
    )
    .expect("legacy adoption preserves existing ID");
    assert!(legacy.content.contains("^h-aaaaaaaaaaaaaaaa"));
    assert!(!legacy.content.contains("[!warning] Removed highlight"));

    let fold_sidecar = SidecarInput {
        path: dup_sidecar.path.clone(),
        annotations: parse_sidecar_markdown(
            "## Page 11\n\n> head text\n\n---\n\n## Page 12\n\n> tail text\n\n- <\n",
        ),
    };
    let folded = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &fold_sidecar,
        false,
    )
    .expect("continuation fold");
    assert_eq!(folded.count, 1);
    assert_eq!(folded.continuation_count, 1);
    let head_id = folded
        .block_ids_by_annotation_order
        .get(&1)
        .expect("fold head")
        .clone();
    assert_eq!(folded.block_ids_by_annotation_order.get(&2), Some(&head_id));
    let refold = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &region_note(&folded.content),
        &fold_sidecar,
        false,
    )
    .expect("fold idempotent");
    assert_eq!(refold.content, folded.content);

    let ambiguous_prior = region_note(
        "### Pages 11–12\n\n> [!quote] head text […] tail one\n\n^h-111111111111\n\n### Pages 11–12\n\n> [!quote] head text […] tail two\n\n^h-222222222222\n",
    );
    let ambiguous_sidecar = SidecarInput {
        path: dup_sidecar.path.clone(),
        annotations: parse_sidecar_markdown("## Page 11\n\n> head text\n"),
    };
    let ambiguous = render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ambiguous_prior,
        &ambiguous_sidecar,
        false,
    );
    assert!(
        ambiguous.is_err(),
        "ambiguous prefix must be write-free, got {:?}",
        ambiguous.map(|rendered| rendered.content)
    );
}
