//! Sidecar discovery, parsing, and asset resolution.
use super::*;

pub(crate) fn is_wikilink(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("[[") && trimmed.ends_with("]]")
}

pub(super) fn simple_wikilink_target(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if !(trimmed.starts_with("[[") && trimmed.ends_with("]]")) {
        return None;
    }
    let target = &trimmed[2..trimmed.len() - 2];
    if target.trim().is_empty()
        || target.trim() != target
        || target.contains('|')
        || target.contains("#^")
        || target.contains("[[")
        || target.contains("]]")
    {
        return None;
    }
    Some(target)
}

pub(super) fn canonicalize_parent(
    projection: &mut Projection,
    source: &str,
) -> Result<()> {
    let Some(value) = projection.get_mut(FIELD_PARENT) else {
        return Ok(());
    };

    let canonical = match value {
        MarkerValue::String(value) | MarkerValue::Number(value) => {
            canonical_parent_target(value, source)?
        }
        MarkerValue::Bool(value) => {
            canonical_parent_target(&value.to_string(), source)?
        }
        MarkerValue::Null => return Err(empty_parent_error(source)),
        MarkerValue::List(_) => {
            return Err(CommandError::new(format!(
                "{source} parent must be a scalar note target; inline lists are not supported"
            )));
        }
    };

    *value = MarkerValue::String(canonical);
    Ok(())
}

pub(super) fn canonical_parent_target(
    value: &str,
    source: &str,
) -> Result<String> {
    let target = value.trim();
    if target.is_empty() {
        return Err(empty_parent_error(source));
    }
    if is_wikilink(target) {
        Ok(target.to_string())
    } else {
        Ok(format!("[[{target}]]"))
    }
}

pub(super) fn empty_parent_error(source: &str) -> CommandError {
    CommandError::new(format!(
        "{source} has an empty required marker key: {FIELD_PARENT}"
    ))
}

pub(super) fn read_sidecar_for_pdf(pdf: &Path) -> Result<Option<SidecarInput>> {
    let Some(path) = discover_sidecar_path(pdf)? else {
        return Ok(None);
    };
    let contents = fs::read_to_string(&path).map_err(|error| {
        CommandError::new(format!("read sidecar {}: {error}", path.display()))
    })?;
    Ok(Some(SidecarInput {
        path,
        annotations: parse_sidecar_markdown(&contents),
    }))
}

pub(super) fn discover_sidecar_path(pdf: &Path) -> Result<Option<PathBuf>> {
    let markdown = pdf.with_extension("md");
    if markdown.is_file() {
        return Ok(Some(markdown));
    }

    let textbundle = pdf.with_extension("textbundle");
    if !textbundle.exists() {
        return Ok(None);
    }
    if !textbundle.is_dir() {
        return Err(CommandError::new(format!(
            "unsupported sidecar {}: expected a .textbundle directory",
            textbundle.display()
        )));
    }

    for file_name in TEXTBUNDLE_TEXT_FILES {
        let text_path = textbundle.join(file_name);
        if text_path.is_file() {
            return Ok(Some(text_path));
        }
    }

    Err(CommandError::new(format!(
        "unsupported textbundle sidecar {}: expected text.md or text.markdown",
        textbundle.display()
    )))
}

pub(super) fn parse_sidecar_markdown(contents: &str) -> Vec<SidecarAnnotation> {
    let mut annotations = Vec::new();
    let mut chunk = Vec::new();
    let mut page_label = None;
    let mut linked_page_style = false;
    let mut order = 0usize;
    let mut page_ordinals: BTreeMap<String, usize> = BTreeMap::new();
    // Document preamble (a setext title, an author line, blank lines) before
    // the first page heading is never an annotation (bob-cli-4r). Sidecars
    // without any page heading have no preamble, so every chunk still parses.
    let mut seen_page_heading = !contents
        .lines()
        .any(|line| sidecar_page_heading_details(line).is_some());

    for line in contents.lines() {
        if let Some(next_page_heading) = sidecar_page_heading_details(line) {
            if seen_page_heading {
                flush_sidecar_chunk(
                    &mut annotations,
                    &mut chunk,
                    page_label.as_deref(),
                    linked_page_style,
                    &mut order,
                    &mut page_ordinals,
                );
            } else {
                chunk.clear();
            }
            seen_page_heading = true;
            page_label = Some(next_page_heading.label);
            linked_page_style = next_page_heading.linked_page_style;
            continue;
        }

        if is_horizontal_rule(line) {
            if seen_page_heading {
                flush_sidecar_chunk(
                    &mut annotations,
                    &mut chunk,
                    page_label.as_deref(),
                    linked_page_style,
                    &mut order,
                    &mut page_ordinals,
                );
            } else {
                chunk.clear();
            }
            continue;
        }

        chunk.push(line.to_string());
    }

    flush_sidecar_chunk(
        &mut annotations,
        &mut chunk,
        page_label.as_deref(),
        linked_page_style,
        &mut order,
        &mut page_ordinals,
    );
    annotations
}

pub(super) fn flush_sidecar_chunk(
    annotations: &mut Vec<SidecarAnnotation>,
    chunk: &mut Vec<String>,
    page_label: Option<&str>,
    linked_page_style: bool,
    order: &mut usize,
    page_ordinals: &mut BTreeMap<String, usize>,
) {
    for mut annotation in
        parse_sidecar_chunk(chunk, page_label, linked_page_style)
    {
        *order += 1;
        annotation.order = *order;
        let page_key = annotation.page_label.clone().unwrap_or_default();
        let ordinal = page_ordinals.entry(page_key).or_insert(0);
        *ordinal += 1;
        annotation.ordinal_on_page = *ordinal;
        annotations.push(annotation);
    }
    chunk.clear();
}

pub(super) fn parse_sidecar_chunk(
    chunk: &[String],
    page_label: Option<&str>,
    linked_page_style: bool,
) -> Vec<SidecarAnnotation> {
    let lines = trim_blank_lines(chunk);
    if lines.is_empty() || lines.iter().all(|line| is_markdown_heading(line)) {
        return Vec::new();
    }

    if let Some(blockquote_index) = lines
        .iter()
        .position(|line| line.trim_start().starts_with('>'))
    {
        let mut quote_lines = Vec::new();
        let mut index = blockquote_index;
        let mut last_quote_line_was_blank = false;
        while index < lines.len() {
            let trimmed = lines[index].trim_start();
            if trimmed.starts_with('>') {
                let quote_line = strip_blockquote_marker(trimmed);
                last_quote_line_was_blank = quote_line.trim().is_empty();
                quote_lines.push(quote_line);
                index += 1;
                continue;
            }
            if trimmed.is_empty() && should_keep_blank_quote_line(&lines, index)
            {
                quote_lines.push(String::new());
                last_quote_line_was_blank = true;
                index += 1;
                continue;
            }
            if !last_quote_line_was_blank
                && linked_page_style
                && is_quote_continuation_line(&lines[index])
            {
                quote_lines.push(trimmed.to_string());
                last_quote_line_was_blank = false;
                index += 1;
                continue;
            }
            break;
        }

        let text = normalize_annotation_text(&quote_lines);
        if text.is_empty() {
            return Vec::new();
        }
        let comment_lines = lines[index..]
            .iter()
            .filter(|line| !is_markdown_heading(line))
            .cloned()
            .collect::<Vec<_>>();
        let comment_source = normalize_annotation_text(&comment_lines);
        let comment = strip_comment_label(&comment_source);

        return vec![SidecarAnnotation {
            kind: SidecarAnnotationKind::Highlight,
            page_label: page_label.map(str::to_string),
            linked_page_style,
            text,
            comment: (!comment.is_empty()).then_some(comment),
            task_source: (!comment_source.is_empty())
                .then(|| strip_comment_label_only(&comment_source)),
            image: None,
            order: 0,
            ordinal_on_page: 0,
        }];
    }

    let non_heading_lines = lines
        .iter()
        .filter(|line| !is_markdown_heading(line))
        .cloned()
        .collect::<Vec<_>>();
    let image_annotations = parse_image_sidecar_annotations(
        &non_heading_lines,
        page_label,
        linked_page_style,
    );
    if !image_annotations.is_empty() {
        return image_annotations;
    }

    let note_lines = non_heading_lines
        .iter()
        .map(|line| strip_standalone_note_marker(line))
        .collect::<Vec<_>>();
    let text = normalize_annotation_text(&note_lines);
    if text.is_empty() {
        Vec::new()
    } else {
        vec![SidecarAnnotation {
            kind: SidecarAnnotationKind::StandaloneNote,
            page_label: page_label.map(str::to_string),
            linked_page_style,
            text,
            comment: None,
            task_source: None,
            image: None,
            order: 0,
            ordinal_on_page: 0,
        }]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MarkdownImage {
    pub(super) target: String,
    pub(super) alt_text: Option<String>,
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingImageAnnotation {
    pub(super) image: SidecarImage,
    pub(super) comment_lines: Vec<String>,
}

pub(super) fn parse_image_sidecar_annotations(
    lines: &[String],
    page_label: Option<&str>,
    linked_page_style: bool,
) -> Vec<SidecarAnnotation> {
    let mut images = Vec::<PendingImageAnnotation>::new();
    let mut prefix_comment_lines = Vec::<String>::new();

    for line in lines {
        let markdown_images = markdown_images_in_line(line);
        if markdown_images.is_empty() {
            push_image_comment_line(
                &mut images,
                &mut prefix_comment_lines,
                strip_standalone_note_marker(line),
            );
            continue;
        }

        let mut cursor = 0usize;
        for markdown_image in markdown_images {
            let before = line[cursor..markdown_image.start].trim();
            if !before.is_empty() {
                push_image_comment_line(
                    &mut images,
                    &mut prefix_comment_lines,
                    strip_standalone_note_marker(before),
                );
            }

            let mut pending = PendingImageAnnotation {
                image: SidecarImage {
                    target: markdown_image.target,
                    alt_text: markdown_image.alt_text,
                },
                comment_lines: Vec::new(),
            };
            if images.is_empty() && !prefix_comment_lines.is_empty() {
                pending.comment_lines.append(&mut prefix_comment_lines);
            }
            images.push(pending);
            cursor = markdown_image.end;
        }

        let after = line[cursor..].trim();
        if !after.is_empty() {
            push_image_comment_line(
                &mut images,
                &mut prefix_comment_lines,
                strip_standalone_note_marker(after),
            );
        }
    }

    images
        .into_iter()
        .filter_map(|pending| {
            let comment_source =
                normalize_annotation_text(&pending.comment_lines);
            let comment = strip_comment_label(&comment_source);
            let text = pending
                .image
                .alt_text
                .clone()
                .unwrap_or_else(|| pending.image.target.clone());
            (!pending.image.target.is_empty()).then(|| SidecarAnnotation {
                kind: SidecarAnnotationKind::Image,
                page_label: page_label.map(str::to_string),
                linked_page_style,
                text,
                comment: (!comment.is_empty()).then_some(comment),
                task_source: (!comment_source.is_empty())
                    .then(|| strip_comment_label_only(&comment_source)),
                image: Some(pending.image),
                order: 0,
                ordinal_on_page: 0,
            })
        })
        .collect()
}

pub(super) fn push_image_comment_line(
    images: &mut [PendingImageAnnotation],
    prefix_comment_lines: &mut Vec<String>,
    line: String,
) {
    if let Some(image) = images.last_mut() {
        image.comment_lines.push(line);
    } else {
        prefix_comment_lines.push(line);
    }
}

pub(super) fn markdown_images_in_line(line: &str) -> Vec<MarkdownImage> {
    let mut images = Vec::new();
    let mut search_start = 0usize;
    while let Some(relative_start) = line[search_start..].find("![") {
        let start = search_start + relative_start;
        let alt_start = start + 2;
        let Some(label_end_relative) = line[alt_start..].find("](") else {
            break;
        };
        let label_end = alt_start + label_end_relative;
        let target_start = label_end + 2;
        let Some(target_end_relative) = line[target_start..].find(')') else {
            break;
        };
        let target_end = target_start + target_end_relative;
        let end = target_end + 1;
        if let Some(target) =
            markdown_image_target(&line[target_start..target_end])
        {
            let alt = line[alt_start..label_end].trim();
            images.push(MarkdownImage {
                target,
                alt_text: (!alt.is_empty()).then(|| alt.to_string()),
                start,
                end,
            });
        }
        search_start = end;
    }
    images
}

pub(super) fn markdown_image_target(destination: &str) -> Option<String> {
    let trimmed = destination.trim();
    if trimmed.is_empty() {
        return None;
    }

    let target = if let Some(rest) = trimmed.strip_prefix('<') {
        let end = rest.find('>')?;
        &rest[..end]
    } else if image_target_has_supported_extension(trimmed) {
        trimmed
    } else {
        trimmed.split_whitespace().next()?
    };

    image_target_has_supported_extension(target)
        .then(|| target.trim().to_string())
}

pub(super) fn image_target_has_supported_extension(target: &str) -> bool {
    let clean_target = target.split(['?', '#']).next().unwrap_or(target).trim();
    let extension = Path::new(clean_target)
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);
    matches!(
        extension.as_deref(),
        Some(
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "bmp"
                | "svg"
                | "avif"
                | "heic"
        )
    )
}

pub(super) fn resolve_sidecar_image_assets(
    config: &Config,
    pdf: &Path,
    ref_note_path: &Path,
    sidecar: &SidecarInput,
) -> Result<BTreeMap<usize, ImageAssetWrite>> {
    let mut assets = BTreeMap::new();
    for annotation in &sidecar.annotations {
        if annotation.kind != SidecarAnnotationKind::Image {
            continue;
        }
        let image = annotation.image.as_ref().ok_or_else(|| {
            CommandError::new("image annotation is missing image metadata")
        })?;
        let source_path =
            sidecar_image_source_path(&sidecar.path, &image.target)?;
        let source_bytes = fs::read(&source_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CommandError::new(format!(
                    "image asset not found: {} - export the sidecar as a TextBundle so images are included",
                    image.target
                ))
            } else {
                CommandError::new(format!(
                    "read image asset {}: {error}",
                    source_path.display()
                ))
            }
        })?;
        let source_sha256 = hex::encode(Sha256::digest(&source_bytes));
        let block_id = image_annotation_block_id(config, pdf, &source_sha256);
        let extension =
            image_target_extension(&image.target).ok_or_else(|| {
                CommandError::new(format!(
                    "image asset target has no supported extension: {}",
                    image.target
                ))
            })?;
        let dest_path =
            image_asset_dest_path(ref_note_path, &block_id, &extension)?;
        let action = image_asset_action(&dest_path, &source_sha256)?;
        assets.insert(
            annotation.order,
            ImageAssetWrite {
                annotation_order: annotation.order,
                source_path,
                dest_path: dest_path.clone(),
                vault_relative_dest_path: PathBuf::from(
                    vault_relative_path_value(config, &dest_path),
                ),
                source_sha256,
                block_id,
                action,
            },
        );
    }
    Ok(assets)
}

pub(super) fn sidecar_image_source_path(
    sidecar_path: &Path,
    target: &str,
) -> Result<PathBuf> {
    let filesystem_target =
        target.split(['?', '#']).next().unwrap_or(target).trim();
    let target_path = Path::new(filesystem_target);
    if target_path.is_absolute()
        || target_path.components().any(|component| {
            matches!(component, Component::Prefix(_) | Component::ParentDir)
        })
    {
        return Err(CommandError::new(format!(
            "image asset target must be relative to the sidecar: {target}"
        )));
    }
    let sidecar_dir = sidecar_path.parent().ok_or_else(|| {
        CommandError::new(format!(
            "sidecar has no parent directory: {}",
            sidecar_path.display()
        ))
    })?;
    Ok(sidecar_dir.join(target_path))
}

pub(super) fn image_target_extension(target: &str) -> Option<String> {
    let clean_target = target.split(['?', '#']).next().unwrap_or(target).trim();
    Path::new(clean_target)
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .filter(|extension| {
            image_target_has_supported_extension(&format!("x.{extension}"))
        })
}

pub(super) fn image_asset_dest_path(
    ref_note_path: &Path,
    block_id: &str,
    extension: &str,
) -> Result<PathBuf> {
    let assets_dir = ref_note_assets_dir(ref_note_path)?;
    Ok(assets_dir.join(format!("{block_id}.{extension}")))
}

pub(super) fn ref_note_assets_dir(ref_note_path: &Path) -> Result<PathBuf> {
    let stem = ref_note_path.file_stem().ok_or_else(|| {
        CommandError::new(format!(
            "reference note path has no file stem: {}",
            ref_note_path.display()
        ))
    })?;
    let mut dir_name = OsString::from(stem);
    dir_name.push(".assets");
    Ok(ref_note_path.with_file_name(dir_name))
}

pub(super) fn image_asset_action(
    dest_path: &Path,
    source_sha256: &str,
) -> Result<ImageAssetAction> {
    match fs::read(dest_path) {
        Ok(bytes) => {
            let dest_sha256 = hex::encode(Sha256::digest(bytes));
            if dest_sha256 == source_sha256 {
                Ok(ImageAssetAction::None)
            } else {
                Ok(ImageAssetAction::Copy)
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ImageAssetAction::Copy)
        }
        Err(error) => Err(CommandError::new(format!(
            "read image asset destination {}: {error}",
            dest_path.display()
        ))),
    }
}
