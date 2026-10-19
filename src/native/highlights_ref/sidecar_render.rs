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
    let absorbed_ids = plan
        .head_of
        .keys()
        .filter_map(|order| annotations_by_order.get(order).copied())
        .map(|ann| annotation_block_id(config, pdf, ann))
        .chain(
            folds
                .underlines
                .host_of
                .keys()
                .filter_map(|order| annotations_by_order.get(order).copied())
                .map(|ann| annotation_block_id(config, pdf, ann)),
        )
        .collect::<BTreeSet<_>>();

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
                let block_id = annotation_block_id(config, pdf, annotation);
                for member in &members {
                    block_ids_by_annotation_order
                        .insert(member.order, block_id.clone());
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
                    }
                }
                if !current_ids.insert(block_id.clone()) {
                    continue;
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
                let mut pieces = Vec::with_capacity(members.len());
                pieces.push((piece_texts[0].as_str(), Seam::Join));
                for (index, (_, seam)) in followers.iter().enumerate() {
                    pieces.push((piece_texts[index + 1].as_str(), *seam));
                }
                let standalone_underline = members.len() == 1
                    && followers.is_empty()
                    && folds.underlines.standalone.contains(&annotation.order);
                let quote = if standalone_underline {
                    let inner = underline_inner(&annotation.text)
                        .expect("planned underline has an inner quote");
                    standalone_underline_quote(&clean_for_matching(
                        &inner,
                        strip_return_links,
                    ))
                } else {
                    assemble_group_quote(&pieces)
                };
                let member_texts = piece_texts
                    .iter()
                    .map(|text| beautify_annotation_text(text))
                    .collect::<Vec<_>>();
                let mut underline_ranges = Vec::new();
                let mut underline_contributions = Vec::new();
                for (&underline_order, &host_order) in &folds.underlines.host_of
                {
                    let Some(&host_member) =
                        member_index_by_order.get(&host_order)
                    else {
                        continue;
                    };
                    let Some(underline) =
                        annotations_by_order.get(&underline_order).copied()
                    else {
                        continue;
                    };
                    let Some(inner) = underline_inner(&underline.text) else {
                        continue;
                    };
                    let cleaned_inner =
                        clean_for_matching(&inner, strip_return_links);
                    let ranges = emphasis_ranges(
                        &quote,
                        &member_texts,
                        host_member,
                        &cleaned_inner,
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
                let quote = apply_emphasis(&quote, &underline_ranges);

                let mut image_note_renders = Vec::new();
                let mut comment_parts = Vec::<(String, Option<String>)>::new();
                for member in &members {
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
                    let underline = annotations_by_order[&contribution.order];
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
                            format!("{}{}", emphasis_anchor(&anchor), comment)
                        }
                        _ => comment,
                    })
                    .collect::<Vec<_>>();
                rendered.push_str(&render_highlight_group_block(
                    &quote,
                    &image_note_renders,
                    &comment_parts,
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
                    SidecarAnnotationKind::StandaloneNote => {
                        annotation_block_id(config, pdf, annotation)
                    }
                    SidecarAnnotationKind::Highlight => {
                        unreachable!("highlights render through the group path")
                    }
                };
                block_ids_by_annotation_order
                    .insert(annotation.order, block_id.clone());
                if !current_ids.insert(block_id.clone()) {
                    continue;
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
        config,
        pdf,
        plan,
        &annotations_by_order,
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
        .filter_map(|order| annotations_by_order.get(order).copied())
        .map(|annotation| annotation_block_id(config, pdf, annotation))
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
        config,
        pdf,
        &folds,
        &annotations_by_order,
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
            let body =
                format!("\nNow part of [[#^{}|{}]].", stub.head_id, stub.alias);
            push_callout_block(
                &mut rendered,
                1,
                "[!info] Merged highlight",
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
    config: &Config,
    pdf: &Path,
    folds: &AnnotationFolds,
    annotations_by_order: &BTreeMap<usize, &SidecarAnnotation>,
    existing_ids: &BTreeSet<String>,
    previously_removed: &BTreeSet<String>,
    linked_absorbed: &BTreeSet<String>,
) -> Vec<EmphasisStub> {
    let mut stubs = Vec::new();
    for (&underline_order, &host_order) in &folds.underlines.host_of {
        let Some(underline) =
            annotations_by_order.get(&underline_order).copied()
        else {
            continue;
        };
        let underline_id = annotation_block_id(config, pdf, underline);
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
        stubs.push(EmphasisStub {
            underline_id,
            host_id: annotation_block_id(config, pdf, head),
            alias,
        });
    }
    stubs
}

fn continuation_redirect_stubs(
    config: &Config,
    pdf: &Path,
    plan: &ContinuationPlan,
    annotations_by_order: &BTreeMap<usize, &SidecarAnnotation>,
    existing_ids: &BTreeSet<String>,
) -> Vec<ContinuationStub> {
    let mut stubs = Vec::new();
    for annotation in annotations_by_order.values().copied() {
        let Some(&head_order) = plan.head_of.get(&annotation.order) else {
            continue;
        };
        let follower_id = annotation_block_id(config, pdf, annotation);
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
            Some(label) => format!("the {label} highlight"),
            None => "the merged highlight".to_string(),
        };
        stubs.push(ContinuationStub {
            follower_id,
            head_id: annotation_block_id(config, pdf, head),
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

fn render_highlight_group_block(
    quote: &str,
    image_notes: &[ImageNoteRender],
    comment_parts: &[String],
    block_id: &str,
) -> String {
    let mut rendered = String::new();
    push_callout_block(&mut rendered, 1, "[!quote]", quote);
    push_image_note_renders(&mut rendered, image_notes);
    if !comment_parts.is_empty() {
        let comment = comment_parts.join("\n\n");
        rendered.push_str(">\n");
        push_callout_block(&mut rendered, 2, "[!note] Comment", &comment);
    }
    rendered.push('\n');
    rendered.push('^');
    rendered.push_str(block_id);
    rendered.push_str("\n\n");
    rendered
}

pub(super) fn annotation_block_id(
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
            push_image_note_renders(&mut rendered, image_notes);
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
            push_image_note_renders(&mut rendered, image_notes);
            push_remaining_comment_callout(&mut rendered, remaining_text);
        }
        SidecarAnnotationKind::StandaloneNote => {
            let text = beautify_annotation_text(remaining_text);
            push_callout_block(&mut rendered, 1, "[!note]", &text);
            push_image_note_renders(&mut rendered, image_notes);
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
) {
    for image_note in image_notes {
        rendered.push_str(">\n");
        match image_note {
            ImageNoteRender::Embedded { vault_path, width } => {
                rendered.push_str("> ");
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
                    2,
                    &format!("[!warning] {}", state.warning_title()),
                    &body,
                );
            }
        }
    }
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
/// a standalone note whose text is a `status`/`parent` marker list, or a
/// linked-page highlight whose comment is one, is the PDF marker mirrored
/// into the sidecar — never one of Bryan's annotations (bob-cli-4r). Real
/// notes never look like a marker list, so every mirror-shaped annotation is
/// skipped no matter where it sits in the sidecar.
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

pub(super) fn is_quote_continuation_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty()
        && !is_markdown_heading(line)
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

pub(super) fn is_markdown_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let hash_count = trimmed
        .chars()
        .take_while(|character| *character == '#')
        .count();
    (1..=6).contains(&hash_count)
        && trimmed
            .as_bytes()
            .get(hash_count)
            .is_some_and(u8::is_ascii_whitespace)
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
