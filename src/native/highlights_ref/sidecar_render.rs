//! Sidecar highlight rendering and line classifiers.
use super::*;

pub(super) fn render_sidecar_highlights(
    config: &Config,
    pdf: &Path,
    ref_note_path: &Path,
    note: &ParsedNote,
    sidecar: &SidecarInput,
    strip_return_links: bool,
) -> Result<RenderedHighlights> {
    let existing_ids = note.generated_block_ids()?;
    let image_assets_by_order =
        resolve_sidecar_image_assets(config, pdf, ref_note_path, sidecar)?;
    let mut image_asset_writes_by_dest =
        BTreeMap::<PathBuf, ImageAssetWrite>::new();
    let mut current_ids = BTreeSet::new();
    let mut block_ids_by_annotation_order = BTreeMap::new();
    let mut standalone_ids_by_annotation_order = BTreeMap::new();
    let mut rendered = String::new();
    let mut current_page = None;
    let mut image_count = 0usize;
    let mut image_note_count = 0usize;
    let mut image_notes_unresolved = Vec::new();
    let silently_dropped_ids = silently_dropped_block_ids(note);
    let folds = plan_annotation_folds(&sidecar.annotations, strip_return_links);
    let plan = &folds.continuations;
    let annotations_by_order: BTreeMap<usize, &SidecarAnnotation> = sidecar
        .annotations
        .iter()
        .map(|ann| (ann.order, ann))
        .collect();
    let id_plan = plan_block_ids(
        config,
        pdf,
        sidecar,
        note,
        &folds,
        &annotations_by_order,
    )?;
    let absorbed_ids = id_plan.absorbed_standalone_ids.clone();

    for image_asset in image_assets_by_order.values() {
        image_asset_writes_by_dest
            .entry(image_asset.dest_path.clone())
            .or_insert_with(|| image_asset.clone());
    }

    for annotation in &sidecar.annotations {
        if is_sidecar_marker_mirror(annotation) {
            continue;
        }
        if plan.head_of.contains_key(&annotation.order) {
            continue;
        }
        if folds.underlines.host_of.contains_key(&annotation.order) {
            continue;
        }

        match annotation.kind {
            SidecarAnnotationKind::Highlight => {
                let followers = plan
                    .followers
                    .get(&annotation.order)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let mut members = Vec::with_capacity(followers.len() + 1);
                members.push(annotation);
                for (order, _) in followers {
                    members.push(annotations_by_order[order]);
                }
                let Some(block_id) =
                    id_plan.effective_by_order.get(&annotation.order).cloned()
                else {
                    return Err(CommandError::new(
                        "ambiguous highlight identity: no planned block ID for highlight head",
                    ));
                };
                for member in &members {
                    block_ids_by_annotation_order
                        .insert(member.order, block_id.clone());
                    if let Some(standalone) =
                        id_plan.standalone_by_order.get(&member.order)
                    {
                        standalone_ids_by_annotation_order
                            .insert(member.order, standalone.clone());
                    }
                }
                let member_index_by_order = members
                    .iter()
                    .enumerate()
                    .map(|(index, member)| (member.order, index))
                    .collect::<BTreeMap<_, _>>();
                for (&underline_order, &host_order) in &folds.underlines.host_of
                {
                    if member_index_by_order.contains_key(&host_order) {
                        block_ids_by_annotation_order
                            .insert(underline_order, block_id.clone());
                        if let Some(standalone) =
                            id_plan.standalone_by_order.get(&underline_order)
                        {
                            standalone_ids_by_annotation_order
                                .insert(underline_order, standalone.clone());
                        } else {
                            standalone_ids_by_annotation_order.insert(
                                underline_order,
                                id_plan
                                    .effective_by_order
                                    .get(&underline_order)
                                    .cloned()
                                    .unwrap_or_else(|| block_id.clone()),
                            );
                        }
                    }
                }
                if !current_ids.insert(block_id.clone()) {
                    return Err(CommandError::new(
                        "ambiguous highlight identity: duplicate block ID for distinct highlights",
                    ));
                }

                let labels: Vec<Option<&str>> = members
                    .iter()
                    .map(|member| member.page_label.as_deref())
                    .collect();
                let heading_label = group_page_label(&labels)
                    .or_else(|| annotation.page_label.clone());
                push_page_heading(
                    &mut rendered,
                    &mut current_page,
                    heading_label.as_deref(),
                );

                let mut piece_texts = Vec::new();
                for member in &members {
                    if strip_return_links {
                        piece_texts
                            .push(strip_return_link_glyphs(&member.text));
                    } else {
                        piece_texts.push(member.text.clone());
                    }
                }
                let mut rendered_pieces = Vec::with_capacity(members.len());
                for (member_index, member) in members.iter().enumerate() {
                    let member_text = &piece_texts[member_index];
                    let standalone_underline =
                        folds.underlines.standalone.contains(&member.order);
                    let mut quote = if standalone_underline {
                        let inner = underline_inner(&member.text)
                            .expect("planned underline has an inner quote");
                        standalone_underline_quote(&clean_for_matching(
                            &inner,
                            strip_return_links,
                        ))
                    } else {
                        beautify_annotation_text(member_text)
                    };
                    let member_text_key = beautify_annotation_text(member_text);
                    let mut underline_ranges = Vec::new();
                    let mut underline_contributions = Vec::new();
                    for (&underline_order, &host_order) in
                        &folds.underlines.host_of
                    {
                        if host_order != member.order {
                            continue;
                        }
                        let Some(underline) =
                            annotations_by_order.get(&underline_order).copied()
                        else {
                            continue;
                        };
                        let Some(inner) = underline_inner(&underline.text)
                        else {
                            continue;
                        };
                        let cleaned_inner =
                            clean_for_matching(&inner, strip_return_links);
                        let host_key_start = *folds
                            .underlines
                            .host_key_start
                            .get(&underline_order)
                            .expect(
                                "absorbed underline records its host key start",
                            );
                        let ranges = emphasis_ranges(
                            &quote,
                            &[member_text_key.clone()],
                            0,
                            &cleaned_inner,
                            host_key_start,
                        );
                        underline_ranges.extend(ranges.iter().cloned());
                        underline_contributions.push(UnderlineContribution {
                            order: underline_order,
                            ranges,
                            inner: cleaned_inner,
                        });
                    }
                    let quote_chars = quote.chars().collect::<Vec<_>>();
                    let merged_run_count = merge_emphasis_ranges(
                        &quote_chars,
                        underline_ranges.clone(),
                    )
                    .len();
                    quote = apply_emphasis(&quote, &underline_ranges);

                    let mut image_note_renders = Vec::new();
                    let mut comment_parts =
                        Vec::<(String, Option<String>)>::new();
                    let effective = plan.effective_annotation(member);
                    let source = effective
                        .task_source
                        .as_deref()
                        .or(effective.comment.as_deref())
                        .unwrap_or("");
                    let (image_note_refs, remaining_text) =
                        split_image_note_lines(source);
                    append_resolved_image_notes(
                        config,
                        ref_note_path,
                        member.order,
                        &block_id,
                        &image_note_refs,
                        &mut image_note_renders,
                        &mut image_note_count,
                        &mut image_notes_unresolved,
                        &mut image_asset_writes_by_dest,
                    )?;
                    let comment = strip_comment_label(
                        &beautify_annotation_text(&remaining_text),
                    );
                    if !comment.trim().is_empty() {
                        comment_parts.push((comment, None));
                    }

                    underline_contributions.sort_by_key(|contribution| {
                        (
                            contribution
                                .ranges
                                .first()
                                .map_or(usize::MAX, |range| range.start),
                            contribution.order,
                        )
                    });
                    for contribution in underline_contributions {
                        let underline =
                            annotations_by_order[&contribution.order];
                        let source = effective_underline_source(underline);
                        let (image_note_refs, remaining_text) =
                            split_image_note_lines(&source);
                        append_resolved_image_notes(
                            config,
                            ref_note_path,
                            contribution.order,
                            &block_id,
                            &image_note_refs,
                            &mut image_note_renders,
                            &mut image_note_count,
                            &mut image_notes_unresolved,
                            &mut image_asset_writes_by_dest,
                        )?;
                        let comment = strip_comment_label(
                            &beautify_annotation_text(&remaining_text),
                        );
                        if !comment.trim().is_empty() {
                            let anchor = contribution
                                .ranges
                                .first()
                                .map(|range| {
                                    quote_chars[range.clone()]
                                        .iter()
                                        .copied()
                                        .collect::<String>()
                                })
                                .unwrap_or(contribution.inner);
                            comment_parts.push((comment, Some(anchor)));
                        }
                    }
                    let add_anchors =
                        comment_parts.len() >= 2 || merged_run_count >= 2;
                    let comment_parts = comment_parts
                        .into_iter()
                        .map(|(comment, anchor)| match (add_anchors, anchor) {
                            (true, Some(anchor)) => {
                                format!(
                                    "{}{}",
                                    emphasis_anchor(&anchor),
                                    comment
                                )
                            }
                            _ => comment,
                        })
                        .collect();
                    rendered_pieces.push(RenderedPiece {
                        page_label: member.page_label.clone(),
                        quote,
                        image_notes: image_note_renders,
                        comment_parts,
                    });
                }
                rendered.push_str(&render_highlight_group_block(
                    &rendered_pieces,
                    &block_id,
                ));
            }
            SidecarAnnotationKind::Image
            | SidecarAnnotationKind::StandaloneNote => {
                let image_asset = image_assets_by_order.get(&annotation.order);
                let block_id = match annotation.kind {
                    SidecarAnnotationKind::Image => image_asset
                        .map(|asset| asset.block_id.clone())
                        .ok_or_else(|| {
                            CommandError::new(
                                "image annotation was not resolved",
                            )
                        })?,
                    SidecarAnnotationKind::StandaloneNote => id_plan
                        .effective_by_order
                        .get(&annotation.order)
                        .cloned()
                        .ok_or_else(|| {
                            CommandError::new(
                                "ambiguous highlight identity: no planned block ID for standalone note",
                            )
                        })?,
                    SidecarAnnotationKind::Highlight => {
                        unreachable!("highlights render through the group path")
                    }
                };
                block_ids_by_annotation_order
                    .insert(annotation.order, block_id.clone());
                if annotation.kind == SidecarAnnotationKind::Image {
                    standalone_ids_by_annotation_order
                        .insert(annotation.order, block_id.clone());
                } else if let Some(standalone) =
                    id_plan.standalone_by_order.get(&annotation.order)
                {
                    standalone_ids_by_annotation_order
                        .insert(annotation.order, standalone.clone());
                } else {
                    standalone_ids_by_annotation_order
                        .insert(annotation.order, block_id.clone());
                }
                if !current_ids.insert(block_id.clone()) {
                    return Err(CommandError::new(
                        "ambiguous highlight identity: duplicate block ID for distinct annotations",
                    ));
                }
                if annotation.kind == SidecarAnnotationKind::Image {
                    image_count += 1;
                }

                let image_note_source = match annotation.kind {
                    SidecarAnnotationKind::Image => annotation
                        .task_source
                        .as_deref()
                        .or(annotation.comment.as_deref())
                        .unwrap_or(""),
                    SidecarAnnotationKind::StandaloneNote => {
                        annotation.text.as_str()
                    }
                    SidecarAnnotationKind::Highlight => "",
                };
                let (image_note_refs, remaining_text) =
                    split_image_note_lines(image_note_source);
                let mut image_note_renders = Vec::new();
                append_resolved_image_notes(
                    config,
                    ref_note_path,
                    annotation.order,
                    &block_id,
                    &image_note_refs,
                    &mut image_note_renders,
                    &mut image_note_count,
                    &mut image_notes_unresolved,
                    &mut image_asset_writes_by_dest,
                )?;
                push_page_heading(
                    &mut rendered,
                    &mut current_page,
                    annotation.page_label.as_deref(),
                );
                rendered.push_str(&render_annotation_block(
                    config,
                    ref_note_path,
                    annotation,
                    &block_id,
                    image_asset,
                    &image_note_renders,
                    &remaining_text,
                    strip_return_links,
                ));
            }
        }
    }

    // Blocks the existing region marks as mirror-shaped or preamble are
    // previously leaked content (bob-cli-4r): drop them without a tombstone.
    // Absorbed continuation followers and underlines never tombstone.
    let removed_ids = existing_ids
        .difference(&current_ids)
        .filter(|block_id| {
            !silently_dropped_ids.contains(block_id.as_str())
                && !absorbed_ids.contains(block_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    let continuation_stubs = continuation_redirect_stubs(
        plan,
        &annotations_by_order,
        &id_plan,
        &existing_ids,
    );
    let previously_removed = note
        .managed_region()?
        .map(parse_managed_region)
        .map(|region| region.removed.into_iter().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let newly_absorbed_published = folds
        .underlines
        .host_of
        .keys()
        .filter_map(|order| id_plan.standalone_by_order.get(order).cloned())
        .filter(|block_id| {
            existing_ids.contains(block_id)
                && !previously_removed.contains(block_id)
        })
        .collect::<BTreeSet<_>>();
    let linked_absorbed =
        crate::native::vault_links::markdown_links_to_block_ids(
            &config.bob_dir,
            &newly_absorbed_published,
        )
        .map_err(|error| {
            CommandError::new(format!(
                "could not scan vault Markdown for underline links: {error}"
            ))
        })?;
    let emphasis_stubs = underline_redirect_stubs(
        &folds,
        &annotations_by_order,
        &id_plan,
        &existing_ids,
        &previously_removed,
        &linked_absorbed,
    );
    if !continuation_stubs.is_empty()
        || !emphasis_stubs.is_empty()
        || !removed_ids.is_empty()
    {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(REMOVED_HIGHLIGHTS_HEADING);
        rendered.push_str("\n\n");
        for stub in continuation_stubs {
            let body = format!(
                "\n\nNow shown in [[#^{}|{}]].",
                stub.head_id, stub.alias
            );
            push_callout_block(
                &mut rendered,
                1,
                "[!info] Grouped highlight",
                &body,
            );
            rendered.push('\n');
            rendered.push('^');
            rendered.push_str(&stub.follower_id);
            rendered.push_str("\n\n");
        }
        for stub in emphasis_stubs {
            let body = format!(
                "\nNow emphasized in [[#^{}|{}]].",
                stub.host_id, stub.alias
            );
            push_callout_block(
                &mut rendered,
                1,
                "[!info] Merged emphasis",
                &body,
            );
            rendered.push('\n');
            rendered.push('^');
            rendered.push_str(&stub.underline_id);
            rendered.push_str("\n\n");
        }
        for block_id in removed_ids {
            push_callout_block(
                &mut rendered,
                1,
                "[!warning] Removed highlight",
                "This annotation is no longer present in the Highlights sidecar.",
            );
            rendered.push('\n');
            rendered.push('^');
            rendered.push_str(&block_id);
            rendered.push_str("\n\n");
        }
    }

    for (&order, id) in &id_plan.effective_by_order {
        block_ids_by_annotation_order
            .entry(order)
            .or_insert_with(|| id.clone());
        standalone_ids_by_annotation_order
            .entry(order)
            .or_insert_with(|| {
                id_plan
                    .standalone_by_order
                    .get(&order)
                    .cloned()
                    .unwrap_or_else(|| id.clone())
            });
    }
    for (&order, id) in &id_plan.standalone_by_order {
        standalone_ids_by_annotation_order
            .entry(order)
            .or_insert_with(|| id.clone());
    }
    for (order, asset) in &image_assets_by_order {
        if let Some(id) = block_ids_by_annotation_order.get(order) {
            standalone_ids_by_annotation_order
                .entry(*order)
                .or_insert_with(|| id.clone());
        } else {
            let _ = asset;
        }
    }

    Ok(RenderedHighlights {
        content: rendered,
        count: current_ids.len(),
        image_count,
        image_note_count,
        image_notes_unresolved,
        continuation_count: plan.head_of.len(),
        dangling_continuations: plan
            .dangling
            .iter()
            .map(|order| DanglingContinuation {
                page_label: annotations_by_order
                    .get(order)
                    .and_then(|annotation| annotation.page_label.clone()),
            })
            .collect(),
        underlines_merged: folds.underlines.host_of.len(),
        standalone_underlines: folds
            .underlines
            .standalone_reasons
            .iter()
            .map(|(order, reason)| StandaloneUnderline {
                page_label: annotations_by_order
                    .get(order)
                    .and_then(|annotation| annotation.page_label.clone()),
                reason: reason.clone(),
            })
            .collect(),
        image_assets: image_asset_writes_by_dest.into_values().collect(),
        block_ids_by_annotation_order,
        standalone_ids_by_annotation_order,
    })
}

struct UnderlineContribution {
    order: usize,
    ranges: Vec<std::ops::Range<usize>>,
    inner: String,
}

fn effective_underline_source(annotation: &SidecarAnnotation) -> String {
    let source = annotation
        .task_source
        .as_deref()
        .or(annotation.comment.as_deref())
        .unwrap_or("");
    match scan_continuation_marker(source) {
        MarkerScan::Escaped { task_source } => task_source,
        _ => source.to_string(),
    }
}

fn emphasis_anchor(run: &str) -> String {
    let words = run.split_whitespace().collect::<Vec<_>>();
    let anchor = words.iter().take(6).copied().collect::<Vec<_>>().join(" ");
    let ellipsis = if words.len() > 6 { "…" } else { "" };
    format!("“{anchor}{ellipsis}” — ")
}

struct ContinuationStub {
    follower_id: String,
    head_id: String,
    alias: String,
}

struct EmphasisStub {
    underline_id: String,
    host_id: String,
    alias: String,
}

fn underline_redirect_stubs(
    folds: &AnnotationFolds,
    annotations_by_order: &BTreeMap<usize, &SidecarAnnotation>,
    id_plan: &BlockIdPlan,
    existing_ids: &BTreeSet<String>,
    previously_removed: &BTreeSet<String>,
    linked_absorbed: &BTreeSet<String>,
) -> Vec<EmphasisStub> {
    let mut stubs = Vec::new();
    for (&underline_order, &host_order) in &folds.underlines.host_of {
        let Some(underline_id) =
            id_plan.standalone_by_order.get(&underline_order).cloned()
        else {
            continue;
        };
        if !existing_ids.contains(&underline_id)
            || (!previously_removed.contains(&underline_id)
                && !linked_absorbed.contains(&underline_id))
        {
            continue;
        }
        let head_order = folds
            .continuations
            .head_of
            .get(&host_order)
            .copied()
            .unwrap_or(host_order);
        let Some(head) = annotations_by_order.get(&head_order).copied() else {
            continue;
        };
        let mut members = vec![head];
        if let Some(followers) = folds.continuations.followers.get(&head_order)
        {
            for (order, _) in followers {
                if let Some(member) = annotations_by_order.get(order).copied() {
                    members.push(member);
                }
            }
        }
        let labels = members
            .iter()
            .map(|member| member.page_label.as_deref())
            .collect::<Vec<_>>();
        let alias = group_page_label(&labels)
            .map(|label| format!("the {label} highlight"))
            .unwrap_or_else(|| "the merged highlight".to_string());
        let Some(host_id) =
            id_plan.effective_by_order.get(&head_order).cloned()
        else {
            continue;
        };
        stubs.push(EmphasisStub {
            underline_id,
            host_id,
            alias,
        });
    }
    stubs
}

fn continuation_redirect_stubs(
    plan: &ContinuationPlan,
    annotations_by_order: &BTreeMap<usize, &SidecarAnnotation>,
    id_plan: &BlockIdPlan,
    existing_ids: &BTreeSet<String>,
) -> Vec<ContinuationStub> {
    let mut stubs = Vec::new();
    for annotation in annotations_by_order.values().copied() {
        let Some(&head_order) = plan.head_of.get(&annotation.order) else {
            continue;
        };
        let Some(follower_id) =
            id_plan.standalone_by_order.get(&annotation.order).cloned()
        else {
            continue;
        };
        if !existing_ids.contains(&follower_id) {
            continue;
        }
        let Some(head) = annotations_by_order.get(&head_order).copied() else {
            continue;
        };
        let mut members = vec![head];
        if let Some(followers) = plan.followers.get(&head_order) {
            for (order, _) in followers {
                if let Some(member) = annotations_by_order.get(order).copied() {
                    members.push(member);
                }
            }
        }
        let labels: Vec<Option<&str>> = members
            .iter()
            .map(|member| member.page_label.as_deref())
            .collect();
        let alias = match group_page_label(&labels) {
            Some(label) => format!("the {label} group"),
            None => "the grouped highlight".to_string(),
        };
        let Some(head_id) =
            id_plan.effective_by_order.get(&head_order).cloned()
        else {
            continue;
        };
        stubs.push(ContinuationStub {
            follower_id,
            head_id,
            alias,
        });
    }
    stubs
}

fn push_page_heading(
    rendered: &mut String,
    current_page: &mut Option<String>,
    page_label: Option<&str>,
) {
    if page_label == current_page.as_deref() {
        return;
    }
    if let Some(page_label) = page_label {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str("### ");
        rendered.push_str(page_label);
        rendered.push_str("\n\n");
    }
    *current_page = page_label.map(str::to_string);
}

fn append_resolved_image_notes(
    config: &Config,
    ref_note_path: &Path,
    annotation_order: usize,
    block_id: &str,
    image_note_refs: &[ImageNoteRef],
    image_note_renders: &mut Vec<ImageNoteRender>,
    image_note_count: &mut usize,
    image_notes_unresolved: &mut Vec<UnresolvedImageNote>,
    image_asset_writes_by_dest: &mut BTreeMap<PathBuf, ImageAssetWrite>,
) -> Result<()> {
    for image_note in image_note_refs {
        *image_note_count += 1;
        match resolve_image_note(
            config,
            ref_note_path,
            image_note,
            annotation_order,
            block_id,
        )? {
            ImageNoteResolution::Embedded {
                vault_path,
                width,
                write,
            } => {
                if let Some(write) = write {
                    image_asset_writes_by_dest
                        .entry(write.dest_path.clone())
                        .or_insert(write);
                }
                image_note_renders
                    .push(ImageNoteRender::Embedded { vault_path, width });
            }
            ImageNoteResolution::Unresolved { state } => {
                image_notes_unresolved.push(UnresolvedImageNote {
                    source_text: image_note.source_text.clone(),
                    state,
                });
                image_note_renders.push(ImageNoteRender::Unresolved {
                    source_text: image_note.source_text.clone(),
                    state,
                });
            }
        }
    }
    Ok(())
}

struct RenderedPiece {
    page_label: Option<String>,
    quote: String,
    image_notes: Vec<ImageNoteRender>,
    comment_parts: Vec<String>,
}

fn render_highlight_group_block(
    pieces: &[RenderedPiece],
    block_id: &str,
) -> String {
    let mut rendered = String::new();
    let is_group = pieces.len() > 1;
    for (index, piece) in pieces.iter().enumerate() {
        let depth = if index == 0 { 1 } else { 2 };
        if index > 0 {
            push_blockquote_spacer(&mut rendered, 1);
        }
        let header = if is_group {
            match piece.page_label.as_deref() {
                Some(label)
                    if !label.chars().any(|character| {
                        matches!(character, ']' | '|' | '\n' | '\r')
                    }) =>
                {
                    format!("[!quote|{label}]")
                }
                _ => "[!quote]".to_string(),
            }
        } else {
            "[!quote]".to_string()
        };
        push_callout_block(&mut rendered, depth, &header, &piece.quote);
        push_image_note_renders(&mut rendered, &piece.image_notes, depth);
        if !piece.comment_parts.is_empty() {
            let comment = piece.comment_parts.join("\n\n");
            push_blockquote_spacer(&mut rendered, depth);
            push_callout_block(
                &mut rendered,
                depth + 1,
                "[!note] Comment",
                &comment,
            );
        }
    }
    rendered.push('\n');
    rendered.push('^');
    rendered.push_str(block_id);
    rendered.push_str("\n\n");
    rendered
}

/// Normalized semantic text for stable highlight identity (bob-cli-9o).
///
/// Presentation-only decoration never changes identity: return-link pill/tag
/// glyphs are stripped, PDF text is beautified (reflow, hyphen healing,
/// ligatures), Markdown emphasis (`**`) added by underline absorption or
/// standalone healing is removed, and whitespace is collapsed. Comments and
/// task sources are never part of the quote identity; see
/// [`identity_comment_key`] for the duplicate tiebreaker.
pub(super) fn identity_quote_key(text: &str) -> String {
    let stripped = strip_return_link_glyphs(text);
    let beautified = beautify_annotation_text(&stripped);
    let no_emphasis = beautified.replace("**", "");
    normalized_identity_text(&no_emphasis)
}

/// Stable identity for a rendered continuation group. Separating normalized
/// piece keys with a newline keeps each piece boundary significant while
/// retaining the whitespace prefix relation used by legacy ID matching.
fn group_identity_text(pieces: &[&str]) -> String {
    match pieces {
        [] => String::new(),
        [piece] => identity_quote_key(piece),
        _ => pieces
            .iter()
            .map(|piece| identity_quote_key(piece))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Normalized comment key used only as a duplicate tiebreaker within an
/// identical-quote group (bob-cli-9o). Comment edits alone never change the
/// block ID: when the tiebreaker no longer matches, the primary quote key
/// still preserves the existing ID one-to-one in order.
pub(super) fn identity_comment_key(text: &str) -> String {
    let stripped = strip_return_link_glyphs(text);
    let beautified = beautify_annotation_text(&stripped);
    let no_emphasis = beautified.replace("**", "");
    normalized_identity_text(&no_emphasis)
}

fn stable_base_block_id(
    config: &Config,
    pdf: &Path,
    kind_str: &str,
    page_label: Option<&str>,
    normalized_text: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_pdf_value(config, pdf));
    hasher.update([0]);
    hasher.update(kind_str);
    hasher.update([0]);
    hasher.update(page_label.unwrap_or_default());
    hasher.update([0]);
    hasher.update(normalized_text);
    let digest = hex::encode(hasher.finalize());
    format!("h-{}", &digest[..12])
}

fn stable_block_id_with_index(
    config: &Config,
    pdf: &Path,
    kind_str: &str,
    page_label: Option<&str>,
    normalized_text: &str,
    index: usize,
) -> String {
    if index == 0 {
        return stable_base_block_id(
            config,
            pdf,
            kind_str,
            page_label,
            normalized_text,
        );
    }
    let mut hasher = Sha256::new();
    hasher.update(source_pdf_value(config, pdf));
    hasher.update([0]);
    hasher.update(kind_str);
    hasher.update([0]);
    hasher.update(page_label.unwrap_or_default());
    hasher.update([0]);
    hasher.update(normalized_text);
    hasher.update([0]);
    hasher.update(format!("#{index}"));
    let digest = hex::encode(hasher.finalize());
    format!("h-{}", &digest[..12])
}

pub(super) fn annotation_block_id(
    config: &Config,
    pdf: &Path,
    annotation: &SidecarAnnotation,
) -> String {
    debug_assert_ne!(annotation.kind, SidecarAnnotationKind::Image);
    stable_base_block_id(
        config,
        pdf,
        annotation.kind.as_str(),
        annotation.page_label.as_deref(),
        &identity_quote_key(&annotation.text),
    )
}

pub(super) fn image_annotation_block_id(
    config: &Config,
    pdf: &Path,
    image_sha256: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_pdf_value(config, pdf));
    hasher.update([0]);
    hasher.update(SidecarAnnotationKind::Image.as_str());
    hasher.update([0]);
    hasher.update(image_sha256);
    let digest = hex::encode(hasher.finalize());
    format!("h-{}", &digest[..12])
}

fn legacy_ordinal_block_id(
    config: &Config,
    pdf: &Path,
    annotation: &SidecarAnnotation,
) -> String {
    debug_assert_ne!(annotation.kind, SidecarAnnotationKind::Image);
    let mut hasher = Sha256::new();
    hasher.update(source_pdf_value(config, pdf));
    hasher.update([0]);
    hasher.update(annotation.kind.as_str());
    hasher.update([0]);
    hasher.update(annotation.page_label.as_deref().unwrap_or_default());
    hasher.update([0]);
    hasher.update(annotation.ordinal_on_page.to_string());
    hasher.update([0]);
    hasher.update(normalized_identity_text(&annotation.text));
    let digest = hex::encode(hasher.finalize());
    format!("h-{}", &digest[..12])
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct IdentityPrimary {
    kind: String,
    page: Option<String>,
    text: String,
}

#[derive(Debug, Clone)]
struct CurrentHead {
    order: usize,
    kind_str: String,
    head_page: Option<String>,
    head_text: String,
    head_comment: String,
    full_page: Option<String>,
    full_text: String,
    legacy_full_text: String,
    full_comment: String,
    members: Vec<usize>,
}

#[derive(Debug, Clone)]
struct ExistingLive {
    block_id: String,
    kind_str: String,
    page: Option<String>,
    text: String,
    comment: String,
    note_index: usize,
}

fn region_kind_str(kind: RegionBlockKind) -> &'static str {
    match kind {
        RegionBlockKind::Highlight => "highlight",
        RegionBlockKind::Note => "note",
        RegionBlockKind::Image => "image",
    }
}

fn first_number_in_label(label: &str) -> Option<u32> {
    let mut current = String::new();
    for character in label.chars() {
        if character.is_ascii_digit() {
            current.push(character);
        } else if !current.is_empty() {
            break;
        }
    }
    if current.is_empty() {
        None
    } else {
        current.parse().ok()
    }
}

fn pages_compatible_for_prefix(
    head_page: Option<&str>,
    existing_page: Option<&str>,
) -> bool {
    if head_page == existing_page {
        return true;
    }
    let (Some(head), Some(existing)) = (head_page, existing_page) else {
        return false;
    };
    match (first_number_in_label(head), first_number_in_label(existing)) {
        (Some(head_number), Some(existing_number)) => {
            head_number == existing_number
        }
        _ => false,
    }
}

struct BlockIdPlan {
    effective_by_order: BTreeMap<usize, String>,
    standalone_by_order: BTreeMap<usize, String>,
    absorbed_standalone_ids: BTreeSet<String>,
}

#[allow(clippy::too_many_arguments)]
fn plan_block_ids(
    config: &Config,
    pdf: &Path,
    sidecar: &SidecarInput,
    note: &ParsedNote,
    folds: &AnnotationFolds,
    annotations_by_order: &BTreeMap<usize, &SidecarAnnotation>,
) -> Result<BlockIdPlan> {
    let existing_ids = note.generated_block_ids()?;
    let silently_dropped = silently_dropped_block_ids(note);
    let managed_region = note.managed_region()?;
    let parsed = managed_region.map(parse_managed_region);
    let mut existing_live: Vec<ExistingLive> = Vec::new();
    if let Some(region) = parsed.as_ref() {
        for (index, block) in region.blocks.iter().enumerate() {
            if block.mirror || block.in_preamble {
                continue;
            }
            if silently_dropped.contains(&block.block_id) {
                continue;
            }
            match block.kind {
                RegionBlockKind::Image => continue,
                RegionBlockKind::Highlight => {
                    let quote = block.quote.clone().unwrap_or_default();
                    let text = if block.pieces.is_empty() {
                        identity_quote_key(&quote)
                    } else {
                        group_identity_text(
                            &block
                                .pieces
                                .iter()
                                .map(|piece| piece.quote.as_str())
                                .collect::<Vec<_>>(),
                        )
                    };
                    existing_live.push(ExistingLive {
                        block_id: block.block_id.clone(),
                        kind_str: region_kind_str(block.kind).to_string(),
                        page: block.page_label.clone(),
                        text,
                        comment: identity_comment_key(
                            block.comment.as_deref().unwrap_or_default(),
                        ),
                        note_index: index,
                    });
                }
                RegionBlockKind::Note => {
                    let text = block.comment.clone().unwrap_or_default();
                    existing_live.push(ExistingLive {
                        block_id: block.block_id.clone(),
                        kind_str: region_kind_str(block.kind).to_string(),
                        page: block.page_label.clone(),
                        text: identity_quote_key(&text),
                        comment: String::new(),
                        note_index: index,
                    });
                }
            }
        }
    }

    let plan_continuations = &folds.continuations;
    let mut heads: Vec<CurrentHead> = Vec::new();
    let mut absorbed_orders: Vec<usize> = Vec::new();
    for annotation in &sidecar.annotations {
        if is_sidecar_marker_mirror(annotation) {
            continue;
        }
        if annotation.kind == SidecarAnnotationKind::Image {
            continue;
        }
        if plan_continuations.head_of.contains_key(&annotation.order) {
            absorbed_orders.push(annotation.order);
            continue;
        }
        if folds.underlines.host_of.contains_key(&annotation.order) {
            absorbed_orders.push(annotation.order);
            continue;
        }
        match annotation.kind {
            SidecarAnnotationKind::Highlight => {
                let followers = plan_continuations
                    .followers
                    .get(&annotation.order)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let mut members: Vec<usize> = vec![annotation.order];
                for (order, _) in followers {
                    members.push(*order);
                }
                let member_refs: Vec<&SidecarAnnotation> = members
                    .iter()
                    .filter_map(|order| {
                        annotations_by_order.get(order).copied()
                    })
                    .collect();
                let head_effective =
                    plan_continuations.effective_annotation(annotation);
                let head_comment_raw = head_effective
                    .task_source
                    .as_deref()
                    .or(head_effective.comment.as_deref())
                    .unwrap_or("");
                let head_effective_comment = if head_comment_raw.is_empty() {
                    head_effective.comment.clone().unwrap_or_default()
                } else {
                    format!(
                        "{}\n{}",
                        head_effective.comment.clone().unwrap_or_default(),
                        head_effective.task_source.clone().unwrap_or_default()
                    )
                };
                let head_text = identity_quote_key(&annotation.text);
                let head_comment =
                    identity_comment_key(&head_effective_comment);
                let head_page = annotation.page_label.clone();
                let labels: Vec<Option<&str>> = member_refs
                    .iter()
                    .map(|member| member.page_label.as_deref())
                    .collect();
                let full_page = group_page_label(&labels)
                    .or_else(|| annotation.page_label.clone());
                let mut piece_texts: Vec<String> = Vec::new();
                for member in &member_refs {
                    piece_texts.push(strip_return_link_glyphs(&member.text));
                }
                let mut pieces: Vec<(&str, Seam)> = Vec::new();
                if !piece_texts.is_empty() {
                    pieces.push((piece_texts[0].as_str(), Seam::Join));
                    for (index, (_, seam)) in followers.iter().enumerate() {
                        if let Some(text) = piece_texts.get(index + 1) {
                            pieces.push((text.as_str(), *seam));
                        }
                    }
                }
                let full_text = group_identity_text(
                    &piece_texts.iter().map(String::as_str).collect::<Vec<_>>(),
                );
                let combined = assemble_group_quote(&pieces);
                let legacy_full_text = identity_quote_key(&combined);
                let mut full_comment_parts: Vec<String> = Vec::new();
                for member in &member_refs {
                    let effective =
                        plan_continuations.effective_annotation(member);
                    if let Some(comment) = effective.comment.as_deref() {
                        if !comment.trim().is_empty() {
                            full_comment_parts.push(comment.to_string());
                        }
                    }
                    if let Some(task_source) = effective.task_source.as_deref()
                    {
                        if !task_source.trim().is_empty() {
                            full_comment_parts.push(task_source.to_string());
                        }
                    }
                }
                let full_comment =
                    identity_comment_key(&full_comment_parts.join("\n"));
                heads.push(CurrentHead {
                    order: annotation.order,
                    kind_str: annotation.kind.as_str().to_string(),
                    head_page,
                    head_text,
                    head_comment,
                    full_page,
                    full_text,
                    legacy_full_text,
                    full_comment,
                    members,
                });
            }
            SidecarAnnotationKind::StandaloneNote => {
                let text = identity_quote_key(&annotation.text);
                let page = annotation.page_label.clone();
                heads.push(CurrentHead {
                    order: annotation.order,
                    kind_str: annotation.kind.as_str().to_string(),
                    head_page: page.clone(),
                    head_text: text.clone(),
                    head_comment: String::new(),
                    full_page: page,
                    full_text: text,
                    legacy_full_text: String::new(),
                    full_comment: String::new(),
                    members: vec![annotation.order],
                });
            }
            SidecarAnnotationKind::Image => {}
        }
    }

    let mut effective_by_order: BTreeMap<usize, String> = BTreeMap::new();
    let mut standalone_by_order: BTreeMap<usize, String> = BTreeMap::new();
    let mut used_ids: BTreeSet<String> = existing_ids.clone();
    let mut reused_existing: BTreeSet<String> = BTreeSet::new();

    let mut existing_by_full: BTreeMap<IdentityPrimary, Vec<usize>> =
        BTreeMap::new();
    for (index, live) in existing_live.iter().enumerate() {
        existing_by_full
            .entry(IdentityPrimary {
                kind: live.kind_str.clone(),
                page: live.page.clone(),
                text: live.text.clone(),
            })
            .or_default()
            .push(index);
    }

    let mut heads_by_full: BTreeMap<IdentityPrimary, Vec<usize>> =
        BTreeMap::new();
    for (head_index, head) in heads.iter().enumerate() {
        heads_by_full
            .entry(IdentityPrimary {
                kind: head.kind_str.clone(),
                page: head.full_page.clone(),
                text: head.full_text.clone(),
            })
            .or_default()
            .push(head_index);
    }

    let mut head_effective_assigned: Vec<Option<String>> =
        vec![None; heads.len()];
    let mut full_matched_existing: BTreeSet<usize> = BTreeSet::new();

    for (primary, head_indices) in heads_by_full.iter() {
        let Some(existing_indices) = existing_by_full.get(primary) else {
            continue;
        };
        let mut existing_by_secondary: BTreeMap<String, Vec<usize>> =
            BTreeMap::new();
        for &existing_index in existing_indices {
            if full_matched_existing.contains(&existing_index) {
                continue;
            }
            let secondary = existing_live[existing_index].comment.clone();
            existing_by_secondary
                .entry(secondary)
                .or_default()
                .push(existing_index);
        }
        let mut current_by_secondary: BTreeMap<String, Vec<usize>> =
            BTreeMap::new();
        for &head_index in head_indices {
            let secondary = heads[head_index].full_comment.clone();
            current_by_secondary
                .entry(secondary)
                .or_default()
                .push(head_index);
        }
        for (secondary, current_list) in current_by_secondary.iter() {
            if let Some(existing_list) = existing_by_secondary.get(secondary) {
                let count = current_list.len().min(existing_list.len());
                for position in 0..count {
                    let head_index = current_list[position];
                    let existing_index = existing_list[position];
                    if head_effective_assigned[head_index].is_some()
                        || full_matched_existing.contains(&existing_index)
                    {
                        continue;
                    }
                    let id = existing_live[existing_index].block_id.clone();
                    head_effective_assigned[head_index] = Some(id.clone());
                    full_matched_existing.insert(existing_index);
                    reused_existing.insert(id);
                }
            }
        }
        let mut remaining_existing: Vec<usize> = existing_indices
            .iter()
            .copied()
            .filter(|index| !full_matched_existing.contains(index))
            .collect();
        remaining_existing
            .sort_by_key(|index| existing_live[*index].note_index);
        let mut remaining_current: Vec<usize> = head_indices
            .iter()
            .copied()
            .filter(|index| head_effective_assigned[*index].is_none())
            .collect();
        remaining_current.sort_by_key(|index| heads[*index].order);
        let count = remaining_current.len().min(remaining_existing.len());
        for position in 0..count {
            let head_index = remaining_current[position];
            let existing_index = remaining_existing[position];
            let id = existing_live[existing_index].block_id.clone();
            head_effective_assigned[head_index] = Some(id.clone());
            full_matched_existing.insert(existing_index);
            reused_existing.insert(id);
        }
    }

    // Existing fused groups have no piece boundaries, so their parsed quote
    // matches the pre-tucked assembly key. Reuse those IDs after the new
    // per-piece identity pass has had first claim on all exact matches.
    let mut legacy_heads_by_full: BTreeMap<IdentityPrimary, Vec<usize>> =
        BTreeMap::new();
    for (head_index, head) in heads.iter().enumerate() {
        if head.members.len() < 2
            || head_effective_assigned[head_index].is_some()
        {
            continue;
        }
        legacy_heads_by_full
            .entry(IdentityPrimary {
                kind: head.kind_str.clone(),
                page: head.full_page.clone(),
                text: head.legacy_full_text.clone(),
            })
            .or_default()
            .push(head_index);
    }
    for (primary, head_indices) in legacy_heads_by_full.iter() {
        let Some(existing_indices) = existing_by_full.get(primary) else {
            continue;
        };
        let mut existing_by_secondary: BTreeMap<String, Vec<usize>> =
            BTreeMap::new();
        for &existing_index in existing_indices {
            if full_matched_existing.contains(&existing_index) {
                continue;
            }
            existing_by_secondary
                .entry(existing_live[existing_index].comment.clone())
                .or_default()
                .push(existing_index);
        }
        let mut current_by_secondary: BTreeMap<String, Vec<usize>> =
            BTreeMap::new();
        for &head_index in head_indices {
            current_by_secondary
                .entry(heads[head_index].full_comment.clone())
                .or_default()
                .push(head_index);
        }
        for (secondary, current_list) in current_by_secondary.iter() {
            if let Some(existing_list) = existing_by_secondary.get(secondary) {
                let count = current_list.len().min(existing_list.len());
                for position in 0..count {
                    let head_index = current_list[position];
                    let existing_index = existing_list[position];
                    if head_effective_assigned[head_index].is_some()
                        || full_matched_existing.contains(&existing_index)
                    {
                        continue;
                    }
                    let id = existing_live[existing_index].block_id.clone();
                    head_effective_assigned[head_index] = Some(id.clone());
                    full_matched_existing.insert(existing_index);
                    reused_existing.insert(id);
                }
            }
        }
        let mut remaining_existing: Vec<usize> = existing_indices
            .iter()
            .copied()
            .filter(|index| !full_matched_existing.contains(index))
            .collect();
        remaining_existing
            .sort_by_key(|index| existing_live[*index].note_index);
        let mut remaining_current: Vec<usize> = head_indices
            .iter()
            .copied()
            .filter(|index| head_effective_assigned[*index].is_none())
            .collect();
        remaining_current.sort_by_key(|index| heads[*index].order);
        let count = remaining_current.len().min(remaining_existing.len());
        for position in 0..count {
            let head_index = remaining_current[position];
            let existing_index = remaining_existing[position];
            let id = existing_live[existing_index].block_id.clone();
            head_effective_assigned[head_index] = Some(id.clone());
            full_matched_existing.insert(existing_index);
            reused_existing.insert(id);
        }
    }

    let mut heads_by_head: BTreeMap<IdentityPrimary, Vec<usize>> =
        BTreeMap::new();
    for (head_index, head) in heads.iter().enumerate() {
        if head_effective_assigned[head_index].is_some() {
            continue;
        }
        if head.full_page == head.head_page && head.full_text == head.head_text
        {
            continue;
        }
        heads_by_head
            .entry(IdentityPrimary {
                kind: head.kind_str.clone(),
                page: head.head_page.clone(),
                text: head.head_text.clone(),
            })
            .or_default()
            .push(head_index);
    }
    if !heads_by_head.is_empty() {
        let mut existing_head_map: BTreeMap<IdentityPrimary, Vec<usize>> =
            BTreeMap::new();
        for (existing_index, live) in existing_live.iter().enumerate() {
            if full_matched_existing.contains(&existing_index) {
                continue;
            }
            existing_head_map
                .entry(IdentityPrimary {
                    kind: live.kind_str.clone(),
                    page: live.page.clone(),
                    text: live.text.clone(),
                })
                .or_default()
                .push(existing_index);
        }
        for (primary, head_indices) in heads_by_head.iter() {
            let Some(existing_indices) = existing_head_map.get(primary) else {
                continue;
            };
            let mut existing_by_secondary: BTreeMap<String, Vec<usize>> =
                BTreeMap::new();
            for &existing_index in existing_indices {
                if full_matched_existing.contains(&existing_index) {
                    continue;
                }
                existing_by_secondary
                    .entry(existing_live[existing_index].comment.clone())
                    .or_default()
                    .push(existing_index);
            }
            let mut current_by_secondary: BTreeMap<String, Vec<usize>> =
                BTreeMap::new();
            for &head_index in head_indices {
                current_by_secondary
                    .entry(heads[head_index].head_comment.clone())
                    .or_default()
                    .push(head_index);
            }
            for (secondary, current_list) in current_by_secondary.iter() {
                if let Some(existing_list) =
                    existing_by_secondary.get(secondary)
                {
                    let count = current_list.len().min(existing_list.len());
                    for position in 0..count {
                        let head_index = current_list[position];
                        let existing_index = existing_list[position];
                        if head_effective_assigned[head_index].is_some()
                            || full_matched_existing.contains(&existing_index)
                        {
                            continue;
                        }
                        let id = existing_live[existing_index].block_id.clone();
                        head_effective_assigned[head_index] = Some(id.clone());
                        full_matched_existing.insert(existing_index);
                        reused_existing.insert(id);
                    }
                }
            }
            let mut remaining_existing: Vec<usize> = existing_indices
                .iter()
                .copied()
                .filter(|index| !full_matched_existing.contains(index))
                .collect();
            remaining_existing
                .sort_by_key(|index| existing_live[*index].note_index);
            let mut remaining_current: Vec<usize> = head_indices
                .iter()
                .copied()
                .filter(|index| head_effective_assigned[*index].is_none())
                .collect();
            remaining_current.sort_by_key(|index| heads[*index].order);
            let count = remaining_current.len().min(remaining_existing.len());
            for position in 0..count {
                let head_index = remaining_current[position];
                let existing_index = remaining_existing[position];
                let id = existing_live[existing_index].block_id.clone();
                head_effective_assigned[head_index] = Some(id.clone());
                full_matched_existing.insert(existing_index);
                reused_existing.insert(id);
            }
        }
    }

    for (head_index, head) in heads.iter().enumerate() {
        if head_effective_assigned[head_index].is_some() {
            continue;
        }
        if head.kind_str != "highlight" {
            continue;
        }
        let head_norm = head.head_text.clone();
        if head_norm.is_empty() {
            continue;
        }
        let mut candidates: Vec<usize> = Vec::new();
        for (existing_index, live) in existing_live.iter().enumerate() {
            if full_matched_existing.contains(&existing_index) {
                continue;
            }
            if live.kind_str != head.kind_str {
                continue;
            }
            if !pages_compatible_for_prefix(
                head.head_page.as_deref(),
                live.page.as_deref(),
            ) {
                continue;
            }
            let existing_norm = live.text.clone();
            if existing_norm == head_norm {
                continue;
            }
            if existing_norm.starts_with(&format!("{head_norm} "))
                || existing_norm.starts_with(&format!("{head_norm} ["))
                || existing_norm.starts_with(&head_norm)
                    && existing_norm[head_norm.len()..]
                        .chars()
                        .next()
                        .is_none_or(|character| {
                            character.is_whitespace() || character == '['
                        })
            {
                candidates.push(existing_index);
            }
        }
        if candidates.len() == 1 {
            let existing_index = candidates[0];
            let conflicting =
                heads.iter().enumerate().any(|(other_index, other)| {
                    other_index != head_index
                        && head_effective_assigned[other_index].is_none()
                        && other.kind_str == head.kind_str
                        && other.head_text == head_norm
                        && pages_compatible_for_prefix(
                            other.head_page.as_deref(),
                            existing_live[existing_index].page.as_deref(),
                        )
                });
            if !conflicting {
                let id = existing_live[existing_index].block_id.clone();
                head_effective_assigned[head_index] = Some(id.clone());
                full_matched_existing.insert(existing_index);
                reused_existing.insert(id);
            } else {
                return Err(CommandError::new(
                    "ambiguous highlight identity: a new highlight is a prefix of multiple existing blocks; rerun after editing the note to disambiguate",
                ));
            }
        } else if candidates.len() > 1 {
            return Err(CommandError::new(
                "ambiguous highlight identity: a new highlight is a prefix of multiple existing blocks; rerun after editing the note to disambiguate",
            ));
        }
    }

    let mut legacy_by_id: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for annotation in &sidecar.annotations {
        if is_sidecar_marker_mirror(annotation) {
            continue;
        }
        if annotation.kind == SidecarAnnotationKind::Image {
            continue;
        }
        let legacy = legacy_ordinal_block_id(config, pdf, annotation);
        if existing_ids.contains(&legacy) && !reused_existing.contains(&legacy)
        {
            legacy_by_id
                .entry(legacy)
                .or_default()
                .push(annotation.order);
        }
    }

    for (head_index, head) in heads.iter().enumerate() {
        if head_effective_assigned[head_index].is_some() {
            continue;
        }
        let Some(annotation) = annotations_by_order.get(&head.order).copied()
        else {
            continue;
        };
        let legacy = legacy_ordinal_block_id(config, pdf, annotation);
        if existing_ids.contains(&legacy) && !reused_existing.contains(&legacy)
        {
            let users = legacy_by_id.get(&legacy).map_or(0, Vec::len);
            if users == 1 {
                head_effective_assigned[head_index] = Some(legacy.clone());
                reused_existing.insert(legacy.clone());
                continue;
            } else if users > 1 {
                return Err(CommandError::new(
                    "ambiguous highlight identity: multiple annotations share one legacy block ID; rerun after editing the note to disambiguate",
                ));
            }
        }
        let stable = stable_base_block_id(
            config,
            pdf,
            &head.kind_str,
            head.head_page.as_deref(),
            &head.head_text,
        );
        if existing_ids.contains(&stable) && !reused_existing.contains(&stable)
        {
            head_effective_assigned[head_index] = Some(stable.clone());
            reused_existing.insert(stable);
        }
    }

    let mut primary_groups: BTreeMap<IdentityPrimary, Vec<usize>> =
        BTreeMap::new();
    for (head_index, head) in heads.iter().enumerate() {
        if head_effective_assigned[head_index].is_some() {
            continue;
        }
        primary_groups
            .entry(IdentityPrimary {
                kind: head.kind_str.clone(),
                page: head.head_page.clone(),
                text: head.head_text.clone(),
            })
            .or_default()
            .push(head_index);
    }
    for head_indices in primary_groups.values() {
        let mut sorted = head_indices.clone();
        sorted.sort_by_key(|index| heads[*index].order);
        for &head_index in &sorted {
            let head = &heads[head_index];
            let mut index = 0usize;
            loop {
                let candidate = stable_block_id_with_index(
                    config,
                    pdf,
                    &head.kind_str,
                    head.head_page.as_deref(),
                    &head.head_text,
                    index,
                );
                if !used_ids.contains(&candidate)
                    && !reused_existing.contains(&candidate)
                {
                    let conflicting_primary =
                        existing_live.iter().any(|live| {
                            live.block_id == candidate
                                && (live.kind_str != head.kind_str
                                    || live.page != head.head_page
                                    || live.text != head.head_text)
                        });
                    if conflicting_primary {
                        return Err(CommandError::new(
                            "ambiguous highlight identity: a new block ID collides with an existing ID for different content; rerun after editing the note to disambiguate",
                        ));
                    }
                    head_effective_assigned[head_index] =
                        Some(candidate.clone());
                    used_ids.insert(candidate);
                    break;
                }
                index += 1;
                if index > 1000 {
                    return Err(CommandError::new(
                        "ambiguous highlight identity: could not allocate a fresh block ID",
                    ));
                }
            }
        }
    }

    for (head_index, head) in heads.iter().enumerate() {
        let Some(id) = head_effective_assigned[head_index].clone() else {
            return Err(CommandError::new(
                "ambiguous highlight identity: no block ID planned",
            ));
        };
        for &order in &head.members {
            effective_by_order.insert(order, id.clone());
            if standalone_by_order.contains_key(&order) {
                return Err(CommandError::new(
                    "ambiguous highlight identity: duplicate annotation order",
                ));
            }
            standalone_by_order.insert(order, id.clone());
        }
        for (&underline_order, &host_order) in &folds.underlines.host_of {
            if head.members.contains(&host_order) {
                effective_by_order.insert(underline_order, id.clone());
            }
        }
    }

    let mut absorbed_standalone_ids: BTreeSet<String> = BTreeSet::new();
    let mut standalone_primary_groups: BTreeMap<IdentityPrimary, Vec<usize>> =
        BTreeMap::new();
    for &order in &absorbed_orders {
        if let Some(annotation) = annotations_by_order.get(&order).copied() {
            if annotation.kind == SidecarAnnotationKind::Image {
                continue;
            }
            standalone_primary_groups
                .entry(IdentityPrimary {
                    kind: annotation.kind.as_str().to_string(),
                    page: annotation.page_label.clone(),
                    text: identity_quote_key(&annotation.text),
                })
                .or_default()
                .push(order);
        }
    }
    for orders in standalone_primary_groups.values() {
        let mut sorted = orders.clone();
        sorted.sort();
        for &order in &sorted {
            let Some(annotation) = annotations_by_order.get(&order).copied()
            else {
                continue;
            };
            let legacy = legacy_ordinal_block_id(config, pdf, annotation);
            if existing_ids.contains(&legacy)
                && !reused_existing.contains(&legacy)
            {
                let users = legacy_by_id.get(&legacy).map_or(0, Vec::len);
                if users == 1 {
                    standalone_by_order.insert(order, legacy.clone());
                    absorbed_standalone_ids.insert(legacy.clone());
                    reused_existing.insert(legacy.clone());
                    continue;
                } else if users > 1 {
                    return Err(CommandError::new(
                        "ambiguous highlight identity: multiple absorbed annotations share one legacy block ID",
                    ));
                }
            }
            let stable = stable_base_block_id(
                config,
                pdf,
                annotation.kind.as_str(),
                annotation.page_label.as_deref(),
                &identity_quote_key(&annotation.text),
            );
            if existing_ids.contains(&stable)
                && !reused_existing.contains(&stable)
            {
                standalone_by_order.insert(order, stable.clone());
                absorbed_standalone_ids.insert(stable.clone());
                reused_existing.insert(stable);
                continue;
            }
            let mut matched: Option<String> = None;
            for live in &existing_live {
                if reused_existing.contains(&live.block_id) {
                    continue;
                }
                if live.kind_str == annotation.kind.as_str()
                    && live.page == annotation.page_label
                    && live.text == identity_quote_key(&annotation.text)
                {
                    matched = Some(live.block_id.clone());
                    break;
                }
            }
            if let Some(id) = matched {
                standalone_by_order.insert(order, id.clone());
                absorbed_standalone_ids.insert(id.clone());
                reused_existing.insert(id.clone());
                used_ids.insert(id);
                continue;
            }
            let mut index = 0usize;
            loop {
                let candidate = stable_block_id_with_index(
                    config,
                    pdf,
                    annotation.kind.as_str(),
                    annotation.page_label.as_deref(),
                    &identity_quote_key(&annotation.text),
                    index,
                );
                if !used_ids.contains(&candidate)
                    && !reused_existing.contains(&candidate)
                {
                    standalone_by_order.insert(order, candidate.clone());
                    absorbed_standalone_ids.insert(candidate.clone());
                    used_ids.insert(candidate);
                    break;
                }
                index += 1;
                if index > 1000 {
                    return Err(CommandError::new(
                        "ambiguous highlight identity: could not allocate a fresh absorbed block ID",
                    ));
                }
            }
        }
    }

    for annotation in &sidecar.annotations {
        if is_sidecar_marker_mirror(annotation) {
            continue;
        }
        if annotation.kind != SidecarAnnotationKind::StandaloneNote {
            continue;
        }
        if standalone_by_order.contains_key(&annotation.order) {
            continue;
        }
        let legacy = legacy_ordinal_block_id(config, pdf, annotation);
        if existing_ids.contains(&legacy)
            && !reused_existing.contains(&legacy)
            && !used_ids.contains(&legacy)
        {
            standalone_by_order.insert(annotation.order, legacy.clone());
            effective_by_order.insert(annotation.order, legacy.clone());
            reused_existing.insert(legacy);
            continue;
        }
    }

    Ok(BlockIdPlan {
        effective_by_order,
        standalone_by_order,
        absorbed_standalone_ids,
    })
}

pub(super) fn render_annotation_block(
    config: &Config,
    ref_note_path: &Path,
    annotation: &SidecarAnnotation,
    block_id: &str,
    image_asset: Option<&ImageAssetWrite>,
    image_notes: &[ImageNoteRender],
    remaining_text: &str,
    strip_return_links: bool,
) -> String {
    let mut rendered = String::new();
    match annotation.kind {
        SidecarAnnotationKind::Highlight => {
            // Return-link tags and pill fragments are render-only cleanup
            // for PDFs stamped with `return_links: true`: block IDs and
            // task sources keep using the raw sidecar text below.
            let stripped;
            let raw = if strip_return_links {
                stripped = strip_return_link_glyphs(&annotation.text);
                stripped.as_str()
            } else {
                annotation.text.as_str()
            };
            let text = beautify_annotation_text(raw);
            push_callout_block(&mut rendered, 1, "[!quote]", &text);
            push_image_note_renders(&mut rendered, image_notes, 1);
            push_remaining_comment_callout(&mut rendered, remaining_text);
        }
        SidecarAnnotationKind::Image => {
            let embed_path = image_asset
                .map(|asset| display_path(&asset.vault_relative_dest_path))
                .unwrap_or_else(|| {
                    vault_relative_note_link(config, ref_note_path)
                });
            let embed = format!("![[{embed_path}]]");
            push_callout_block(&mut rendered, 1, "[!quote] Image", &embed);
            push_image_note_renders(&mut rendered, image_notes, 1);
            push_remaining_comment_callout(&mut rendered, remaining_text);
        }
        SidecarAnnotationKind::StandaloneNote => {
            let text = beautify_annotation_text(remaining_text);
            push_callout_block(&mut rendered, 1, "[!note]", &text);
            push_image_note_renders(&mut rendered, image_notes, 1);
        }
    }
    rendered.push('\n');
    rendered.push('^');
    rendered.push_str(block_id);
    rendered.push_str("\n\n");
    rendered
}

pub(super) fn push_remaining_comment_callout(
    rendered: &mut String,
    remaining_text: &str,
) {
    let comment =
        strip_comment_label(&beautify_annotation_text(remaining_text));
    if comment.trim().is_empty() {
        return;
    }
    rendered.push_str(">\n");
    push_callout_block(rendered, 2, "[!note] Comment", &comment);
}

pub(super) fn push_image_note_renders(
    rendered: &mut String,
    image_notes: &[ImageNoteRender],
    base_depth: usize,
) {
    for image_note in image_notes {
        push_blockquote_spacer(rendered, base_depth);
        let prefix = blockquote_prefix(base_depth);
        match image_note {
            ImageNoteRender::Embedded { vault_path, width } => {
                rendered.push_str(&prefix);
                rendered
                    .push_str(&image_note_embed_markdown(vault_path, *width));
                rendered.push('\n');
            }
            ImageNoteRender::Unresolved { source_text, state } => {
                let body = match state {
                    UnresolvedImageNoteState::Unsupported => format!(
                        "\n`{source_text}` — use PNG, JPEG, GIF, WebP, or SVG"
                    ),
                    UnresolvedImageNoteState::NotFound
                    | UnresolvedImageNoteState::Unreadable => {
                        format!("\n`{source_text}`")
                    }
                };
                push_callout_block(
                    rendered,
                    base_depth + 1,
                    &format!("[!warning] {}", state.warning_title()),
                    &body,
                );
            }
        }
    }
}

fn push_blockquote_spacer(rendered: &mut String, depth: usize) {
    rendered.push_str(blockquote_prefix(depth).trim_end());
    rendered.push('\n');
}

pub(super) fn push_callout_block(
    rendered: &mut String,
    depth: usize,
    header: &str,
    text: &str,
) {
    debug_assert!(depth > 0);
    let prefix = blockquote_prefix(depth);
    let mut lines = text.lines();

    rendered.push_str(&prefix);
    rendered.push_str(header);
    if let Some(first_line) = lines.next()
        && !first_line.is_empty()
    {
        rendered.push(' ');
        rendered.push_str(first_line);
    }
    rendered.push('\n');

    for line in lines {
        push_prefixed_blockquote_line(rendered, &prefix, line);
    }
}

pub(super) fn blockquote_prefix(depth: usize) -> String {
    let mut prefix = String::new();
    for level in 0..depth {
        if level > 0 {
            prefix.push(' ');
        }
        prefix.push('>');
    }
    prefix.push(' ');
    prefix
}

pub(super) fn push_prefixed_blockquote_line(
    rendered: &mut String,
    prefix: &str,
    line: &str,
) {
    if line.is_empty() {
        rendered.push_str(prefix.trim_end());
    } else {
        rendered.push_str(prefix);
        rendered.push_str(line);
    }
    rendered.push('\n');
}

impl SidecarAnnotationKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            SidecarAnnotationKind::Highlight => "highlight",
            SidecarAnnotationKind::Image => "image",
            SidecarAnnotationKind::StandaloneNote => "note",
        }
    }
}

#[cfg(test)]
pub(super) fn sidecar_page_heading(line: &str) -> Option<String> {
    sidecar_page_heading_details(line).map(|heading| heading.label)
}

pub(super) fn sidecar_page_heading_details(
    line: &str,
) -> Option<SidecarPageHeading> {
    let trimmed = line.trim();
    if !trimmed.starts_with('#') {
        return None;
    }
    let level = trimmed
        .chars()
        .take_while(|character| *character == '#')
        .count();
    let heading = trimmed.trim_start_matches('#').trim();
    let (label, linked_page_style) = match markdown_link_label(heading) {
        Some(label) => (label, true),
        None => (heading, false),
    };
    if is_sidecar_page_label(label) {
        let page_index = markdown_link_parts(heading)
            .and_then(|(_, destination)| destination_page_index(destination))
            .or_else(|| label_page_number(label));
        Some(SidecarPageHeading {
            label: label.to_string(),
            linked_page_style,
            page_index,
            level,
        })
    } else {
        None
    }
}

pub(super) fn markdown_link_label(text: &str) -> Option<&str> {
    markdown_link_parts(text).map(|(label, _)| label)
}

pub(super) fn markdown_link_parts(text: &str) -> Option<(&str, &str)> {
    if !text.starts_with('[') {
        return None;
    }
    let (label, destination) = text[1..].split_once("](")?;
    let destination = destination.strip_suffix(')')?;
    Some((label, destination))
}

pub(super) fn is_sidecar_page_label(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.starts_with("page ")
        || lower.starts_with("page:")
        || lower.starts_with("p. ")
        || lower.starts_with("p ")
}

/// Page number after a sidecar page-label prefix (`Page `, `page:`, `p. `,
/// `p `), parsed as a decimal integer.
pub(super) fn label_page_number(label: &str) -> Option<u32> {
    let lower = label.to_ascii_lowercase();
    let rest = if let Some(rest) = lower.strip_prefix("page ") {
        rest
    } else if let Some(rest) = lower.strip_prefix("page:") {
        rest
    } else if let Some(rest) = lower.strip_prefix("p. ") {
        rest
    } else if let Some(rest) = lower.strip_prefix("p ") {
        rest
    } else {
        return None;
    };
    let prefix_len = label.len() - rest.len();
    label[prefix_len..].trim().parse().ok()
}

fn destination_page_index(destination: &str) -> Option<u32> {
    let rest = destination.split_once("#page=")?.1;
    let digits = rest.split_once('&').map_or(rest, |(digits, _)| digits);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Content-based marker-mirror detection shared with the region parser:
/// a standalone note whose text is a `status`/`parent` marker list (optionally
/// prefixed by a color-grouped setext title in raw two-line or rendered
/// single-line form), or a linked-page highlight whose comment is one, is the
/// PDF marker mirrored into the sidecar — never one of Bryan's annotations
/// (bob-cli-4r). Real notes never look like a marker list, so every
/// mirror-shaped annotation is skipped no matter where it sits in the sidecar.
/// Color groups stay page-less; no page labels are manufactured.
pub(super) fn is_sidecar_marker_mirror(annotation: &SidecarAnnotation) -> bool {
    match annotation.kind {
        SidecarAnnotationKind::StandaloneNote => {
            is_marker_mirror_text(&annotation.text)
        }
        SidecarAnnotationKind::Highlight => {
            if !annotation.linked_page_style {
                return false;
            }
            annotation
                .comment
                .as_deref()
                .is_some_and(is_marker_mirror_text)
        }
        SidecarAnnotationKind::Image => false,
    }
}

/// Block IDs in the note's existing managed region that must disappear
/// without a tombstone: mirror-shaped blocks and preamble blocks are
/// previously leaked content (bob-cli-4r), not annotations the sidecar lost.
pub(super) fn silently_dropped_block_ids(
    note: &ParsedNote,
) -> BTreeSet<String> {
    let Ok(Some(region)) = note.managed_region() else {
        return BTreeSet::new();
    };
    parse_managed_region(region)
        .blocks
        .into_iter()
        .filter(|block| block.mirror || block.in_preamble)
        .map(|block| block.block_id)
        .collect()
}

/// Immediate nonblank lines after a `>` quote stay in the quote, including
/// heading-shaped wrap lines such as `# Set default agent` (bob-cli-ba).
/// Page headings, color-group headings, and annotation-date headers still
/// bound the quote.
pub(super) fn is_quote_continuation_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty()
        && sidecar_page_heading_details(line).is_none()
        && !is_annotation_date_header(line)
        && !is_color_group_heading(line)
        && !is_comment_label_line(trimmed)
        && !is_marker_list_line(trimmed)
}

pub(super) fn is_comment_label_line(line: &str) -> bool {
    ["Comment:", "comment:", "Note:", "note:"]
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

pub(super) fn is_marker_list_line(line: &str) -> bool {
    let Some(item) =
        line.strip_prefix("- ").or_else(|| line.strip_prefix("* "))
    else {
        return false;
    };
    let Some((key, _)) = item.split_once(':') else {
        return false;
    };
    !normalize_key(key).is_empty()
}

pub(super) fn should_keep_blank_quote_line(
    lines: &[String],
    index: usize,
) -> bool {
    lines[index + 1..]
        .iter()
        .any(|line| line.trim_start().starts_with('>'))
}

pub(super) fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() < 3 {
        return false;
    }
    trimmed.chars().all(|character| character == '-')
        || trimmed.chars().all(|character| character == '*')
        || trimmed.chars().all(|character| character == '_')
}

pub(super) fn trim_blank_lines(lines: &[String]) -> Vec<String> {
    let mut start = 0usize;
    let mut end = lines.len();
    while start < end && lines[start].trim().is_empty() {
        start += 1;
    }
    while end > start && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    lines[start..end].to_vec()
}

pub(super) fn markdown_heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let hash_count = trimmed
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if (1..=6).contains(&hash_count)
        && trimmed
            .as_bytes()
            .get(hash_count)
            .is_some_and(u8::is_ascii_whitespace)
    {
        Some(hash_count)
    } else {
        None
    }
}

pub(super) fn is_markdown_heading(line: &str) -> bool {
    markdown_heading_level(line).is_some()
}

/// Dated metadata heading Highlights inserts above each annotation,
/// e.g. `##### 2026-06-03:`. These bound quotes; they are never quote text.
pub(super) fn is_annotation_date_header(line: &str) -> bool {
    if !is_markdown_heading(line) {
        return false;
    }
    let heading = line.trim_start().trim_start_matches('#').trim();
    let date = heading.strip_suffix(':').unwrap_or(heading).trim();
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
}

/// Color-group heading Highlights inserts above each color group in
/// color-grouped exports, e.g. `#### Yellow` (bob-cli-bf). These bound
/// quotes; they are never quote text.
pub(super) fn is_color_group_heading(line: &str) -> bool {
    if markdown_heading_level(line).is_none() {
        return false;
    }
    if sidecar_page_heading_details(line).is_some() {
        return false;
    }
    if is_annotation_date_header(line) {
        return false;
    }
    let heading = line.trim_start().trim_start_matches('#').trim();
    let name = heading.strip_suffix(':').unwrap_or(heading).trim();
    if name.is_empty() || name.contains(char::is_whitespace) {
        return false;
    }
    matches!(
        name.to_ascii_lowercase().as_str(),
        "yellow"
            | "green"
            | "blue"
            | "pink"
            | "red"
            | "orange"
            | "purple"
            | "gray"
            | "grey"
    )
}

/// A sidecar is color-grouped when it has no page headings at all but
/// contains a color heading or an annotation-date header (bob-cli-bf).
/// Simple `## Page N` sidecars always have page headings, so they never
/// qualify; linked-page sidecars qualify through `linked_page_style` instead.
pub(super) fn is_color_grouped_sidecar_contents(contents: &str) -> bool {
    if contents
        .lines()
        .any(|line| sidecar_page_heading_details(line).is_some())
    {
        return false;
    }
    contents.lines().any(|line| {
        is_annotation_date_header(line) || is_color_group_heading(line)
    })
}

pub(super) fn strip_blockquote_marker(line: &str) -> String {
    line.trim_start()
        .strip_prefix('>')
        .unwrap_or(line)
        .strip_prefix(' ')
        .unwrap_or_else(|| {
            line.trim_start()
                .strip_prefix('>')
                .expect("line starts with blockquote marker")
        })
        .to_string()
}

pub(super) fn strip_standalone_note_marker(line: &str) -> String {
    let trimmed = line.trim_start();
    for prefix in ["Note:", "note:", "[note]", "[Note]"] {
        if let Some(value) = trimmed.strip_prefix(prefix) {
            return value.trim_start().to_string();
        }
    }
    line.to_string()
}

pub(super) fn strip_comment_label(text: &str) -> String {
    strip_comment_list_markers(&strip_comment_label_only(text))
}

pub(super) fn strip_comment_label_only(text: &str) -> String {
    let trimmed = text.trim();
    for prefix in ["Comment:", "comment:", "Note:", "note:"] {
        if let Some(value) = trimmed.strip_prefix(prefix) {
            return value.trim_start().to_string();
        }
    }
    text.to_string()
}

pub(super) fn strip_comment_list_markers(text: &str) -> String {
    if parse_marker(text).is_ok() {
        return text.to_string();
    }

    let mut saw_list_item = false;
    let mut stripped_lines = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            stripped_lines.push(String::new());
            continue;
        }

        let Some(item) = strip_unordered_list_marker(line) else {
            return text.to_string();
        };
        saw_list_item = true;
        stripped_lines.push(item.to_string());
    }

    if saw_list_item {
        stripped_lines.join("\n")
    } else {
        text.to_string()
    }
}

pub(super) fn strip_unordered_list_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let bytes = trimmed.as_bytes();
    if !bytes
        .first()
        .is_some_and(|byte| matches!(byte, b'-' | b'*' | b'+'))
    {
        return None;
    }
    if !bytes.get(1).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }

    let mut index = 2usize;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    Some(&trimmed[index..])
}

pub(super) fn normalize_annotation_text(lines: &[String]) -> String {
    let mut normalized = lines
        .iter()
        .map(|line| line.trim_end().to_string())
        .collect::<Vec<_>>();
    while normalized
        .first()
        .is_some_and(|line| line.trim().is_empty())
    {
        normalized.remove(0);
    }
    while normalized.last().is_some_and(|line| line.trim().is_empty()) {
        normalized.pop();
    }
    normalized.join("\n")
}
