//! Sidecar highlight rendering and line classifiers.
use super::*;

pub(super) fn render_sidecar_highlights(
    config: &Config,
    pdf: &Path,
    ref_note_path: &Path,
    note: &ParsedNote,
    sidecar: &SidecarInput,
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
    let mut skipped_marker_note = false;
    let mut image_count = 0usize;

    for image_asset in image_assets_by_order.values() {
        image_asset_writes_by_dest
            .entry(image_asset.dest_path.clone())
            .or_insert_with(|| image_asset.clone());
    }

    for annotation in &sidecar.annotations {
        if !skipped_marker_note && is_sidecar_marker_mirror(annotation) {
            skipped_marker_note = true;
            continue;
        }

        let image_asset = image_assets_by_order.get(&annotation.order);
        let block_id = match annotation.kind {
            SidecarAnnotationKind::Image => {
                image_asset.map(|asset| asset.block_id.clone()).ok_or_else(
                    || CommandError::new("image annotation was not resolved"),
                )?
            }
            SidecarAnnotationKind::Highlight
            | SidecarAnnotationKind::StandaloneNote => {
                annotation_block_id(config, pdf, annotation)
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

        if annotation.page_label != current_page {
            if let Some(page_label) = &annotation.page_label {
                if !rendered.is_empty() {
                    rendered.push('\n');
                }
                rendered.push_str("### ");
                rendered.push_str(page_label);
                rendered.push_str("\n\n");
            }
            current_page = annotation.page_label.clone();
        }

        rendered.push_str(&render_annotation_block(
            config,
            ref_note_path,
            annotation,
            &block_id,
            image_asset,
        ));
    }

    let removed_ids = existing_ids
        .difference(&current_ids)
        .cloned()
        .collect::<Vec<_>>();
    if !removed_ids.is_empty() {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(REMOVED_HIGHLIGHTS_HEADING);
        rendered.push_str("\n\n");
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
        image_assets: image_asset_writes_by_dest.into_values().collect(),
        block_ids_by_annotation_order,
    })
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
) -> String {
    let mut rendered = String::new();
    match annotation.kind {
        SidecarAnnotationKind::Highlight => {
            let text = beautify_annotation_text(&annotation.text);
            push_callout_block(&mut rendered, 1, "[!quote]", &text);
            push_annotation_comment_callout(&mut rendered, annotation);
        }
        SidecarAnnotationKind::Image => {
            let embed_path = image_asset
                .map(|asset| display_path(&asset.vault_relative_dest_path))
                .unwrap_or_else(|| {
                    vault_relative_note_link(config, ref_note_path)
                });
            let embed = format!("![[{embed_path}]]");
            push_callout_block(&mut rendered, 1, "[!quote] Image", &embed);
            push_annotation_comment_callout(&mut rendered, annotation);
        }
        SidecarAnnotationKind::StandaloneNote => {
            let text = beautify_annotation_text(&annotation.text);
            push_callout_block(&mut rendered, 1, "[!note]", &text);
        }
    }
    rendered.push('\n');
    rendered.push('^');
    rendered.push_str(block_id);
    rendered.push_str("\n\n");
    rendered
}

pub(super) fn push_annotation_comment_callout(
    rendered: &mut String,
    annotation: &SidecarAnnotation,
) {
    if let Some(comment) = &annotation.comment {
        let comment_source =
            annotation.task_source.as_deref().unwrap_or(comment);
        let comment =
            strip_comment_label(&beautify_annotation_text(comment_source));
        rendered.push_str(">\n");
        push_callout_block(rendered, 2, "[!note] Comment", &comment);
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
        Some(SidecarPageHeading {
            label: label.to_string(),
            linked_page_style,
        })
    } else {
        None
    }
}

pub(super) fn markdown_link_label(text: &str) -> Option<&str> {
    if !text.starts_with('[') {
        return None;
    }
    let (label, destination) = text[1..].split_once("](")?;
    destination.ends_with(')').then_some(label)
}

pub(super) fn is_sidecar_page_label(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.starts_with("page ")
        || lower.starts_with("page:")
        || lower.starts_with("p. ")
        || lower.starts_with("p ")
}

pub(super) fn is_sidecar_marker_mirror(annotation: &SidecarAnnotation) -> bool {
    if annotation.kind == SidecarAnnotationKind::StandaloneNote {
        return true;
    }

    if annotation.kind != SidecarAnnotationKind::Highlight {
        return false;
    }
    if !annotation.linked_page_style {
        return false;
    }

    let Some(comment) = &annotation.comment else {
        return false;
    };
    parse_marker(comment).is_ok()
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
