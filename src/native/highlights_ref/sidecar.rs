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

/// Normalize optional source-note provenance like `parent`, while keeping
/// diagnostics tied to the field that supplied an invalid value.
pub(super) fn canonicalize_source_note(
    projection: &mut Projection,
    source: &str,
) -> Result<()> {
    let Some(value) = projection.get_mut(FIELD_SOURCE_NOTE) else {
        return Ok(());
    };
    let scalar = match value {
        MarkerValue::String(value) | MarkerValue::Number(value) => {
            value.clone()
        }
        MarkerValue::Bool(value) => value.to_string(),
        MarkerValue::Null => {
            return Err(CommandError::new(format!(
                "{source} {FIELD_SOURCE_NOTE} must be a scalar note target; null is not supported"
            )));
        }
        MarkerValue::List(_) => {
            return Err(CommandError::new(format!(
                "{source} {FIELD_SOURCE_NOTE} must be a scalar note target; inline lists are not supported"
            )));
        }
    };
    let target = scalar.trim();
    if target.is_empty() {
        return Err(CommandError::new(format!(
            "{source} has an empty optional marker field: {FIELD_SOURCE_NOTE}"
        )));
    }
    let (canonical, route) = if target.starts_with("[[")
        && target.ends_with("]]")
    {
        let inner = &target[2..target.len() - 2];
        if inner.trim().is_empty()
            || inner.trim() != inner
            || inner.contains('|')
            || inner.contains('#')
            || inner.contains("[[")
            || inner.contains("]]")
        {
            return Err(CommandError::new(format!(
                "{source} {FIELD_SOURCE_NOTE} must be a bare note target or a simple wikilink"
            )));
        }
        (target.to_string(), inner)
    } else {
        if target.starts_with(['!', '"', '\''])
            || target.contains("[[")
            || target.contains("]]")
            || target.contains('|')
            || target.contains('#')
            || target.starts_with('[')
            || target.ends_with(']')
        {
            return Err(CommandError::new(format!(
                "{source} {FIELD_SOURCE_NOTE} must be a bare note target or a simple wikilink"
            )));
        }
        (format!("[[{target}]]"), target)
    };
    if route.to_ascii_lowercase().ends_with(".md") {
        return Err(CommandError::new(format!(
            "{source} {FIELD_SOURCE_NOTE} must omit the .md suffix"
        )));
    }
    *value = MarkerValue::String(canonical);
    Ok(())
}

pub(super) fn source_note_target(value: &str, source: &str) -> Result<String> {
    let Some(target) = simple_wikilink_target(value) else {
        return Err(CommandError::new(format!(
            "{source} {FIELD_SOURCE_NOTE} must be a simple canonical wikilink"
        )));
    };
    if target.is_empty() || target.contains('#') {
        return Err(CommandError::new(format!(
            "{source} {FIELD_SOURCE_NOTE} must be a simple canonical wikilink"
        )));
    }
    Ok(target.to_string())
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

pub(super) fn sidecar_has_user_annotations(sidecar: &SidecarInput) -> bool {
    sidecar
        .annotations
        .iter()
        .any(|annotation| !is_sidecar_marker_mirror(annotation))
}

pub(super) fn read_sidecar_for_sync(
    pdf: &Path,
    options: SyncOptions,
) -> Result<Option<SidecarInput>> {
    let sidecar = read_sidecar_for_pdf(pdf)?;
    if options.ignore_unannotated_sidecar
        && !sidecar.as_ref().is_some_and(sidecar_has_user_annotations)
    {
        return Ok(None);
    }
    Ok(sidecar)
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
    let mut page_index = None;
    let mut linked_page_style = false;
    let mut order = 0usize;
    let mut page_ordinals: BTreeMap<String, usize> = BTreeMap::new();
    // Document preamble (a setext title, an author line, blank lines) before
    // the first page heading is never an annotation (bob-cli-4r). Sidecars
    // without any page heading have no preamble, so every chunk still parses.
    // The sidecar's page-heading level is the level of its first page-shaped
    // heading: simple page-shaped lines at any other level (a `# P values`
    // sticky in a `####`-paged sidecar) never split the page stream and stay
    // in their chunk, where the heading filter still applies. A linked
    // `[Page N](highlights://…)` heading is a page break at any level.
    let page_heading_level = contents
        .lines()
        .filter_map(sidecar_page_heading_details)
        .next()
        .map(|heading| heading.level);
    let mut seen_page_heading = page_heading_level.is_none();
    // Color-grouped Highlights exports have no page headings at all; their
    // hard-wrapped quotes use the same bare-line continuation shape as
    // linked-page exports (bob-cli-bf). Simple `## Page N` sidecars always
    // have page headings, so they never qualify.
    let color_grouped_style = is_color_grouped_sidecar_contents(contents);

    for line in contents.lines() {
        let next_page_heading =
            match (&page_heading_level, sidecar_page_heading_details(line)) {
                (Some(expected), Some(heading))
                    if heading.linked_page_style
                        || heading.level == *expected =>
                {
                    Some(heading)
                }
                _ => None,
            };
        if let Some(next_page_heading) = next_page_heading {
            if seen_page_heading {
                flush_sidecar_chunk(
                    &mut annotations,
                    &mut chunk,
                    page_label.as_deref(),
                    page_index,
                    linked_page_style,
                    color_grouped_style,
                    &mut order,
                    &mut page_ordinals,
                );
            } else {
                chunk.clear();
            }
            seen_page_heading = true;
            page_label = Some(next_page_heading.label);
            page_index = next_page_heading.page_index;
            linked_page_style = next_page_heading.linked_page_style;
            continue;
        }

        if is_horizontal_rule(line) {
            if seen_page_heading {
                flush_sidecar_chunk(
                    &mut annotations,
                    &mut chunk,
                    page_label.as_deref(),
                    page_index,
                    linked_page_style,
                    color_grouped_style,
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
        page_index,
        linked_page_style,
        color_grouped_style,
        &mut order,
        &mut page_ordinals,
    );
    annotations
}

pub(super) fn flush_sidecar_chunk(
    annotations: &mut Vec<SidecarAnnotation>,
    chunk: &mut Vec<String>,
    page_label: Option<&str>,
    page_index: Option<u32>,
    linked_page_style: bool,
    color_grouped_style: bool,
    order: &mut usize,
    page_ordinals: &mut BTreeMap<String, usize>,
) {
    for mut annotation in parse_sidecar_chunk(
        chunk,
        page_label,
        page_index,
        linked_page_style,
        color_grouped_style,
    ) {
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
    page_index: Option<u32>,
    linked_page_style: bool,
    color_grouped_style: bool,
) -> Vec<SidecarAnnotation> {
    let lines = trim_blank_lines(chunk);
    if lines.is_empty() {
        return Vec::new();
    }

    let section_declaration = standalone_section_declaration(&lines);

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
                && (linked_page_style || color_grouped_style)
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
        let raw_comment_lines = lines[index..].to_vec();
        let comment_section_heading =
            comment_section_declaration(&raw_comment_lines);
        let comment_lines = lines[index..]
            .iter()
            .filter_map(|line| {
                if is_section_comment_heading(line) {
                    Some(escape_markdown_heading(line))
                } else if is_markdown_heading(line) {
                    None
                } else {
                    Some(line.clone())
                }
            })
            .collect::<Vec<_>>();
        let comment_source = normalize_annotation_text(&comment_lines);
        let comment = strip_comment_label(&comment_source);

        return vec![SidecarAnnotation {
            kind: SidecarAnnotationKind::Highlight,
            page_label: page_label.map(str::to_string),
            page_index,
            linked_page_style,
            text,
            section_heading: None,
            comment_section_heading,
            comment: (!comment.is_empty()).then_some(comment),
            task_source: (!comment_source.is_empty())
                .then(|| strip_comment_label_only(&comment_source)),
            image: None,
            order: 0,
            ordinal_on_page: 0,
        }];
    }

    let image_annotations = parse_image_sidecar_annotations(
        &lines,
        page_label,
        page_index,
        linked_page_style,
    );
    if !image_annotations.is_empty() {
        return image_annotations;
    }

    let (section_heading, note_lines) =
        if let Some((index, heading)) = section_declaration {
            let description = lines[index + 1..]
                .iter()
                .map(|line| {
                    if is_markdown_heading(line) {
                        escape_markdown_heading(line)
                    } else {
                        strip_standalone_note_marker(line)
                    }
                })
                .collect::<Vec<_>>();
            (Some(heading), description)
        } else {
            let note_lines = lines
                .iter()
                .filter(|line| !is_markdown_heading(line))
                .map(|line| strip_standalone_note_marker(line))
                .collect::<Vec<_>>();
            (None, note_lines)
        };
    let text = normalize_annotation_text(&note_lines);
    if text.is_empty() && section_heading.is_none() {
        Vec::new()
    } else {
        vec![SidecarAnnotation {
            kind: SidecarAnnotationKind::StandaloneNote,
            page_label: page_label.map(str::to_string),
            page_index,
            linked_page_style,
            text,
            section_heading,
            comment_section_heading: None,
            comment: None,
            task_source: None,
            image: None,
            order: 0,
            ordinal_on_page: 0,
        }]
    }
}

/// Return the first syntactic section declaration after Highlights' dated or
/// page-heading lines. The returned heading is the raw text after `#`.
fn standalone_section_declaration(lines: &[String]) -> Option<(usize, String)> {
    for (index, line) in lines.iter().enumerate() {
        if let Some(level) = markdown_heading_level(line)
            && (2..=6).contains(&level)
        {
            continue;
        }
        let candidate = strip_standalone_note_marker(line);
        let Some(heading) = section_heading_text(&candidate) else {
            return None;
        };
        return Some((index, heading));
    }
    None
}

fn section_heading_text(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('#')?;
    if rest.starts_with('#') {
        return None;
    }
    let first = rest.chars().next()?;
    if !first.is_whitespace() {
        return None;
    }
    let heading = rest.trim_start();
    (!heading.trim().is_empty()).then(|| heading.trim_end().to_string())
}

fn is_section_comment_heading(line: &str) -> bool {
    section_heading_text(line).is_some()
}

/// First-line `# Name` comment declaration, recorded before escaping.
/// Takes the raw comment lines (after the quote, before escaping/filtering),
/// skips blanks and level 2-6 headings (filtered from the comment source),
/// strips an optional `Comment:`/`Note:` label, and returns the heading text
/// when the author typed exactly one `#` plus whitespace. A user-typed
/// `\# Name` never declares.
fn comment_section_declaration(raw_lines: &[String]) -> Option<String> {
    for line in raw_lines {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(level) = markdown_heading_level(line)
            && (2..=6).contains(&level)
        {
            continue;
        }
        // Image markdown lines are not comment text.
        if !markdown_images_in_line(line).is_empty() {
            // A line that is only an image carries no declaration; a line
            // with text around an image keeps that text as the candidate.
            // For simplicity, skip pure-image lines here; the per-image
            // splitter below handles mixed lines.
            let without_images = strip_image_markdown_spans(line);
            if without_images.trim().is_empty() {
                continue;
            }
        }
        let trimmed = line.trim();
        let mut candidate = trimmed;
        for label in ["Comment:", "comment:", "Note:", "note:"] {
            if let Some(rest) = trimmed.strip_prefix(label) {
                let rest = rest.trim_start();
                if rest.is_empty() {
                    candidate = "";
                } else {
                    candidate = rest;
                    // Only strip one label; a `Comment: Note: # X` keeps
                    // `Note: # X` as the candidate, which then fails to
                    // declare, matching the tag label handling.
                }
                break;
            }
        }
        if candidate.trim().is_empty() {
            continue;
        }
        // Strip image spans from a mixed line before checking the heading.
        let candidate = strip_image_markdown_spans(candidate);
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        return section_heading_text(candidate);
    }
    None
}

fn strip_image_markdown_spans(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0usize;
    for image in markdown_images_in_line(line) {
        output.push_str(&line[cursor..image.start]);
        cursor = image.end;
    }
    output.push_str(&line[cursor..]);
    output
}

fn escape_markdown_heading(line: &str) -> String {
    let leading = line.len() - line.trim_start().len();
    format!("{}\\{}", &line[..leading], &line[leading..])
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
    pub(super) comment_raw_lines: Vec<String>,
}

pub(super) fn parse_image_sidecar_annotations(
    lines: &[String],
    page_label: Option<&str>,
    page_index: Option<u32>,
    linked_page_style: bool,
) -> Vec<SidecarAnnotation> {
    let mut images = Vec::<PendingImageAnnotation>::new();
    let mut prefix_comment_lines = Vec::<String>::new();
    let mut prefix_raw_lines = Vec::<String>::new();

    for line in lines {
        let markdown_images = markdown_images_in_line(line);
        if markdown_images.is_empty() {
            // Preserve blank separators for the declaration blank-line rule.
            if line.trim().is_empty() {
                push_image_comment_line(
                    &mut images,
                    &mut prefix_comment_lines,
                    &mut prefix_raw_lines,
                    String::new(),
                    String::new(),
                );
                continue;
            }
            let stripped = strip_standalone_note_marker(line);
            if is_section_comment_heading(line) {
                push_image_comment_line(
                    &mut images,
                    &mut prefix_comment_lines,
                    &mut prefix_raw_lines,
                    escape_markdown_heading(&stripped),
                    stripped,
                );
            } else if is_markdown_heading(line) {
                // Level 2-6 headings are filtered from the display source but
                // kept for first-line detection (the helper skips them).
                push_image_comment_raw_only(
                    &mut images,
                    &mut prefix_raw_lines,
                    stripped,
                );
            } else {
                push_image_comment_line(
                    &mut images,
                    &mut prefix_comment_lines,
                    &mut prefix_raw_lines,
                    stripped.clone(),
                    stripped,
                );
            }
            continue;
        }

        let mut cursor = 0usize;
        for markdown_image in markdown_images {
            let before = line[cursor..markdown_image.start].trim();
            if !before.is_empty() {
                let stripped = strip_standalone_note_marker(before);
                let before_is_heading = is_section_comment_heading(before);
                let before_is_md = is_markdown_heading(before);
                // A `# Name` fragment before an image is a declaration
                // candidate; escape it for display like the pure-line path.
                if before_is_md && !before_is_heading {
                    // Filtered from display; keep raw only.
                    push_image_comment_raw_only(
                        &mut images,
                        &mut prefix_raw_lines,
                        stripped,
                    );
                } else {
                    let display = if before_is_heading {
                        escape_markdown_heading(&stripped)
                    } else {
                        stripped.clone()
                    };
                    push_image_comment_line(
                        &mut images,
                        &mut prefix_comment_lines,
                        &mut prefix_raw_lines,
                        display,
                        stripped,
                    );
                }
            }

            let mut pending = PendingImageAnnotation {
                image: SidecarImage {
                    target: markdown_image.target,
                    alt_text: markdown_image.alt_text,
                },
                comment_lines: Vec::new(),
                comment_raw_lines: Vec::new(),
            };
            if images.is_empty() {
                if !prefix_comment_lines.is_empty() {
                    pending.comment_lines.append(&mut prefix_comment_lines);
                }
                if !prefix_raw_lines.is_empty() {
                    pending.comment_raw_lines.append(&mut prefix_raw_lines);
                }
            }
            images.push(pending);
            cursor = markdown_image.end;
        }

        let after = line[cursor..].trim();
        if !after.is_empty() {
            let stripped = strip_standalone_note_marker(after);
            let after_is_heading = is_section_comment_heading(after);
            let after_is_md = is_markdown_heading(after);
            if after_is_md && !after_is_heading {
                push_image_comment_raw_only(
                    &mut images,
                    &mut prefix_raw_lines,
                    stripped,
                );
            } else {
                let display = if after_is_heading {
                    escape_markdown_heading(&stripped)
                } else {
                    stripped.clone()
                };
                push_image_comment_line(
                    &mut images,
                    &mut prefix_comment_lines,
                    &mut prefix_raw_lines,
                    display,
                    stripped,
                );
            }
        }
    }

    images
        .into_iter()
        .filter_map(|pending| {
            let comment_section_heading =
                comment_section_declaration(&pending.comment_raw_lines);
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
                page_index,
                linked_page_style,
                text,
                section_heading: None,
                comment_section_heading,
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
    prefix_raw_lines: &mut Vec<String>,
    display: String,
    raw: String,
) {
    if let Some(image) = images.last_mut() {
        image.comment_lines.push(display);
        image.comment_raw_lines.push(raw);
    } else {
        prefix_comment_lines.push(display);
        prefix_raw_lines.push(raw);
    }
}

fn push_image_comment_raw_only(
    images: &mut [PendingImageAnnotation],
    prefix_raw_lines: &mut Vec<String>,
    raw: String,
) {
    if let Some(image) = images.last_mut() {
        image.comment_raw_lines.push(raw);
    } else {
        prefix_raw_lines.push(raw);
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
