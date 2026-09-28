//! Annotation task candidates, routes, and index.
use super::*;

pub(super) fn annotation_task_candidates(
    config: &Config,
    ref_note_path: &Path,
    pdf: &Path,
    sidecar: Option<&SidecarInput>,
    rendered_highlights: Option<&RenderedHighlights>,
) -> Result<Vec<AnnotationTaskCandidate>> {
    let Some(sidecar) = sidecar else {
        return Ok(Vec::new());
    };

    let mut skipped_marker_note = false;
    let mut candidates = Vec::new();
    for annotation in &sidecar.annotations {
        if !skipped_marker_note && is_sidecar_marker_mirror(annotation) {
            skipped_marker_note = true;
            continue;
        }

        match annotation.kind {
            SidecarAnnotationKind::Highlight => {
                let Some(source) = annotation.task_source.as_deref() else {
                    continue;
                };
                let block_id = annotation_block_id(config, pdf, annotation);
                candidates.extend(annotation_task_candidates_from_text(
                    config,
                    ref_note_path,
                    source,
                    &block_id,
                )?);
            }
            SidecarAnnotationKind::Image => {
                let Some(source) = annotation.task_source.as_deref() else {
                    continue;
                };
                let block_id = rendered_highlights
                    .and_then(|rendered| {
                        rendered
                            .block_ids_by_annotation_order
                            .get(&annotation.order)
                    })
                    .ok_or_else(|| {
                        CommandError::new(
                            "image annotation task source was not rendered",
                        )
                    })?;
                candidates.extend(annotation_task_candidates_from_text(
                    config,
                    ref_note_path,
                    source,
                    block_id,
                )?);
            }
            SidecarAnnotationKind::StandaloneNote => {
                let block_id = annotation_block_id(config, pdf, annotation);
                candidates.extend(annotation_task_candidates_from_text(
                    config,
                    ref_note_path,
                    &annotation.text,
                    &block_id,
                )?);
            }
        }
    }
    Ok(candidates)
}

pub(super) fn annotation_task_candidates_from_text(
    config: &Config,
    ref_note_path: &Path,
    text: &str,
    source_block_id: &str,
) -> Result<Vec<AnnotationTaskCandidate>> {
    let mut candidates = Vec::new();
    for line in text.lines() {
        if let Some(candidate) = annotation_task_candidate_from_source_line(
            config,
            ref_note_path,
            line,
            source_block_id,
        )? {
            candidates.push(candidate);
        }
    }
    Ok(candidates)
}

pub(super) fn annotation_task_candidate_from_source_line(
    config: &Config,
    ref_note_path: &Path,
    line: &str,
    source_block_id: &str,
) -> Result<Option<AnnotationTaskCandidate>> {
    let Some(source) = annotation_task_source_from_source_line(line) else {
        return Ok(None);
    };
    let target = match source.route_name {
        Some(route_name) => AnnotationTaskTarget::RoutedNote(
            route_name_to_note_path(config, &route_name)?,
        ),
        None => AnnotationTaskTarget::ReferenceNote,
    };
    let processed_id = annotation_task_processed_id(
        config,
        ref_note_path,
        source_block_id,
        &source.identity,
    );

    Ok(Some(AnnotationTaskCandidate {
        identity: source.identity,
        task_text: source.task_text,
        source_block_id: source_block_id.to_string(),
        target,
        source_ref_note_path: ref_note_path.to_path_buf(),
        processed_id,
    }))
}

pub(super) fn annotation_task_source_from_source_line(
    line: &str,
) -> Option<AnnotationTaskSource> {
    let item = strip_unordered_list_marker(line)?;
    annotation_task_source_from_item(item)
}

pub(super) fn annotation_task_source_from_item(
    item: &str,
) -> Option<AnnotationTaskSource> {
    let task_body = strip_optional_markdown_task_checkbox(item);
    let task_text_with_route = normalized_identity_text(
        &strip_created_task_properties(task_body.trim()),
    );
    let (task_text, route_name) =
        split_annotation_task_route_suffix(&task_text_with_route);
    if !contains_markdown_token(&task_text, PDF_TASK_TAG) {
        return None;
    }
    let identity = annotation_task_identity(&task_text)?;
    Some(AnnotationTaskSource {
        identity,
        task_text,
        route_name,
    })
}

pub(super) fn split_annotation_task_route_suffix(
    task_text: &str,
) -> (String, Option<String>) {
    let trimmed = task_text.trim();
    let Some((separator_index, separator)) = trimmed
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
    else {
        return (trimmed.to_string(), None);
    };
    let token_start = separator_index + separator.len_utf8();
    let before = &trimmed[..separator_index];
    let token = &trimmed[token_start..];
    let Some(route_name) = strict_annotation_task_route_name(token) else {
        return (trimmed.to_string(), None);
    };
    (before.trim_end().to_string(), Some(route_name))
}

pub(super) fn strict_annotation_task_route_name(token: &str) -> Option<String> {
    let name = token.strip_prefix('@')?;
    let mut characters = name.chars();
    let first = characters.next()?;
    if !first.is_ascii_alphanumeric() {
        return None;
    }
    characters
        .all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
        })
        .then(|| name.to_string())
}

pub(super) fn route_name_to_note_path(
    config: &Config,
    route_name: &str,
) -> Result<PathBuf> {
    let path = config.bob_dir.join(format!("{route_name}.md"));
    if path.is_dir() {
        return Err(CommandError::new(format!(
            "routed annotation task target is a directory: {}; create a root-level note named {route_name}.md first",
            path.display()
        )));
    }
    if !path.is_file() {
        return Err(CommandError::new(format!(
            "routed annotation task target does not exist: {}; create a root-level note named {route_name}.md first",
            path.display()
        )));
    }
    Ok(path)
}

pub(super) fn annotation_task_legacy_source_task_block_id(
    candidate: &AnnotationTaskCandidate,
) -> String {
    format!(
        "{}{}",
        SOURCE_TASK_BLOCK_ID_PREFIX,
        &candidate.processed_id[..12]
    )
}

pub(super) fn annotation_task_processed_id(
    config: &Config,
    ref_note_path: &Path,
    source_block_id: &str,
    identity: &str,
) -> String {
    annotation_task_source_digest(
        config,
        ref_note_path,
        source_block_id,
        identity,
    )
}

pub(super) fn annotation_task_source_digest(
    config: &Config,
    ref_note_path: &Path,
    source_block_id: &str,
    identity: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(HIGHLIGHT_TASK_ID_VERSION);
    hasher.update([0]);
    hasher.update(vault_relative_path_value(config, ref_note_path));
    hasher.update([0]);
    hasher.update(source_block_id);
    hasher.update([0]);
    hasher.update(identity);
    hex::encode(hasher.finalize())
}

pub(super) fn render_annotation_task_line(
    config: &Config,
    candidate: &AnnotationTaskCandidate,
    created_date: &str,
) -> String {
    let source_link = annotation_task_source_link(config, candidate);
    format!(
        "- [ ] {} {}[{}:: {}] [created::{}]",
        candidate.task_text,
        source_link,
        HIGHLIGHT_TASK_FIELD,
        candidate.processed_id,
        created_date
    )
}

pub(super) fn annotation_task_source_link(
    config: &Config,
    candidate: &AnnotationTaskCandidate,
) -> String {
    if candidate.source_block_id.is_empty() {
        return String::new();
    }

    let target = match candidate.target {
        AnnotationTaskTarget::ReferenceNote => {
            format!("#^{}", candidate.source_block_id)
        }
        AnnotationTaskTarget::RoutedNote(_) => format!(
            "{}#^{}",
            vault_relative_note_link(config, &candidate.source_ref_note_path),
            candidate.source_block_id
        ),
    };

    format!("[[{target}|{SOURCE_LINK_ALIAS}]] ")
}

pub(super) fn vault_relative_note_link(
    config: &Config,
    note_path: &Path,
) -> String {
    let mut link = vault_relative_path_value(config, note_path);
    if let Some(stripped) = link.strip_suffix(".md") {
        link = stripped.to_string();
    }
    link
}

pub(super) fn strip_optional_markdown_task_checkbox(item: &str) -> &str {
    strip_markdown_task_checkbox(item).unwrap_or_else(|| item.trim_start())
}

pub(super) fn strip_markdown_task_checkbox(item: &str) -> Option<&str> {
    let trimmed = item.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.len() < 3 || bytes[0] != b'[' || bytes[2] != b']' {
        return None;
    }
    if bytes.get(3).is_some_and(|byte| !byte.is_ascii_whitespace()) {
        return None;
    }
    Some(trimmed[3..].trim_start())
}

pub(super) fn annotation_task_identity(task_text: &str) -> Option<String> {
    let without_link = strip_source_block_link(task_text);
    let without_properties = strip_obsidian_task_properties(&without_link);
    let (without_route, _) =
        split_annotation_task_route_suffix(&without_properties);
    let identity = normalized_identity_text(&without_route);
    contains_markdown_token(&identity, PDF_TASK_TAG).then_some(identity)
}

/// Removes any `[[ ... ]]` wikilink whose target contains a block reference
/// (`#^`), covering annotation-level `h-...` links and task-specific `ht-...`
/// links in both same-file and full-note forms. PDF wikilinks
/// (`[[lib/example.pdf]]`) have no `#^` and are left untouched, so identity
/// stays stable across the link being injected into created task lines.
pub(super) fn strip_source_block_link(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut remaining = text;

    while let Some(start) = remaining.find("[[") {
        let after_start = &remaining[start + 2..];
        let Some(end) = after_start.find("]]") else {
            break;
        };
        let inside = &after_start[..end];
        if inside.contains("#^") {
            stripped.push_str(&remaining[..start]);
            remaining = &after_start[end + 2..];
            continue;
        }

        let keep_to = start + 2;
        stripped.push_str(&remaining[..keep_to]);
        remaining = &remaining[keep_to..];
    }

    stripped.push_str(remaining);
    stripped
}

#[cfg(test)]
pub(super) fn existing_annotation_task_identities(
    body: &str,
) -> BTreeSet<String> {
    body.lines()
        .filter_map(existing_annotation_task_identity)
        .collect()
}

#[cfg(test)]
pub(super) fn existing_annotation_task_identity(line: &str) -> Option<String> {
    if line.as_bytes().first().is_some_and(u8::is_ascii_whitespace) {
        return None;
    }

    let item = strip_unordered_list_marker(line)?;
    let task_body = strip_markdown_task_checkbox(item)?;
    if contains_markdown_token(task_body, PDF_TASK_BLOCK_ID) {
        return None;
    }
    annotation_task_identity(task_body)
}

pub(super) fn processed_task_index(
    config: &Config,
) -> Result<ProcessedTaskIndex> {
    let mut index = ProcessedTaskIndex::default();
    if !config.bob_dir.is_dir() {
        return Ok(index);
    }
    collect_processed_task_index_from_dir(&config.bob_dir, &mut index)?;
    Ok(index)
}

pub(super) fn collect_processed_task_index_from_dir(
    directory: &Path,
    index: &mut ProcessedTaskIndex,
) -> Result<()> {
    let entries = fs::read_dir(directory).map_err(|error| {
        CommandError::new(format!("scan {}: {error}", directory.display()))
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            CommandError::new(format!("scan {}: {error}", directory.display()))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CommandError::new(format!("stat {}: {error}", path.display()))
        })?;
        if file_type.is_dir() {
            if !is_hidden_path_component(&path) {
                collect_processed_task_index_from_dir(&path, index)?;
            }
        } else if file_type.is_file() && is_markdown_note_path(&path) {
            collect_processed_task_index_from_file(&path, index)?;
        }
    }

    Ok(())
}

pub(super) fn collect_processed_task_index_from_file(
    path: &Path,
    index: &mut ProcessedTaskIndex,
) -> Result<()> {
    let contents = fs::read_to_string(path).map_err(|error| {
        CommandError::new(format!(
            "read Markdown task index {}: {error}",
            path.display()
        ))
    })?;
    for line in contents.lines() {
        let Some(task_body) = markdown_task_body(line) else {
            continue;
        };
        for source_task_anchor in source_task_block_ids(task_body) {
            index.legacy_source_task_anchors.insert(source_task_anchor);
        }
        for processed_id in
            obsidian_task_property_values(task_body, HIGHLIGHT_TASK_FIELD)
                .into_iter()
                .chain(obsidian_task_property_values(
                    task_body,
                    LEGACY_HIGHLIGHT_TASK_FIELD,
                ))
        {
            index.processed_ids.insert(processed_id);
        }
        if let Some(identity) = annotation_task_identity(task_body) {
            index.legacy_identities.insert(identity);
        }
    }
    Ok(())
}

pub(super) fn is_hidden_path_component(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.starts_with('.'))
}

pub(super) fn is_markdown_note_path(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

pub(super) fn markdown_task_body(line: &str) -> Option<&str> {
    let item = strip_unordered_list_marker(line)?;
    strip_markdown_task_checkbox(item)
}

pub(super) fn source_task_block_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("[[") {
        let after_start = &remaining[start + 2..];
        let Some(end) = after_start.find("]]") else {
            break;
        };
        let inside = &after_start[..end];
        let target =
            inside.split_once('|').map_or(inside, |(target, _)| target);
        if let Some((_, block_id)) = target.rsplit_once("#^") {
            let block_id = block_id.trim();
            if block_id.starts_with(SOURCE_TASK_BLOCK_ID_PREFIX)
                && is_valid_block_id(block_id)
            {
                ids.push(block_id.to_string());
            }
        }

        remaining = &after_start[end + 2..];
    }

    ids
}

pub(super) fn obsidian_task_property_values(
    text: &str,
    key: &str,
) -> Vec<String> {
    let mut values = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find('[') {
        let after_start = &remaining[start + 1..];
        let Some(end) = after_start.find(']') else {
            break;
        };
        let inside = &after_start[..end];
        if let Some((property_key, value)) = obsidian_task_property(inside)
            && property_key.eq_ignore_ascii_case(key)
        {
            let value = value.trim();
            if !value.is_empty() {
                values.push(value.to_string());
            }
        }

        remaining = &after_start[end + 1..];
    }

    values
}

pub(super) fn obsidian_task_property(
    inside_brackets: &str,
) -> Option<(&str, &str)> {
    let (key, value) = inside_brackets.split_once("::")?;
    let key = key.trim();
    (!key.is_empty()
        && key.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
        }))
    .then_some((key, value))
}

pub(super) fn strip_created_task_properties(text: &str) -> String {
    strip_matching_obsidian_task_properties(text, |key| {
        key.eq_ignore_ascii_case("created")
    })
}

pub(super) fn strip_obsidian_task_properties(text: &str) -> String {
    strip_matching_obsidian_task_properties(text, |_| true)
}

pub(super) fn strip_matching_obsidian_task_properties(
    text: &str,
    should_strip: impl Fn(&str) -> bool,
) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut remaining = text;

    while let Some(start) = remaining.find('[') {
        let after_start = &remaining[start + 1..];
        let Some(end) = after_start.find(']') else {
            break;
        };
        let inside = &after_start[..end];
        if let Some(key) = obsidian_task_property_key(inside)
            && should_strip(key)
        {
            stripped.push_str(&remaining[..start]);
            remaining = &after_start[end + 1..];
            continue;
        }

        let bracket_end = start + 1;
        stripped.push_str(&remaining[..bracket_end]);
        remaining = &remaining[bracket_end..];
    }

    stripped.push_str(remaining);
    stripped
}

pub(super) fn obsidian_task_property_key(
    inside_brackets: &str,
) -> Option<&str> {
    obsidian_task_property(inside_brackets).map(|(key, _)| key)
}

pub(super) fn parse_pdf_task_line(body: &str) -> Result<PdfTaskLineState> {
    let mut found = None;
    let mut task_block_line_count = 0usize;

    for (line_index, line) in body.lines().enumerate() {
        if !contains_markdown_token(line, PDF_TASK_BLOCK_ID) {
            continue;
        }

        task_block_line_count += 1;
        if task_block_line_count > 1 {
            return Err(CommandError::new(
                "reference note has multiple generated PDF task lines with ^ref block ID",
            ));
        }

        let Some((checkbox_mark_index, checked, mark)) =
            parse_markdown_task_checkbox(line)
        else {
            return Err(malformed_pdf_task_line_error(line_index));
        };
        if !contains_markdown_token(line, PDF_TASK_TAG)
            || !contains_pdf_wikilink(line)
        {
            return Err(malformed_pdf_task_line_error(line_index));
        }

        found = Some(PdfTaskLine {
            line_index,
            checkbox_mark_index,
            checked,
            mark,
        });
    }

    Ok(found
        .map(PdfTaskLineState::Present)
        .unwrap_or(PdfTaskLineState::Missing))
}

pub(super) fn malformed_pdf_task_line_error(line_index: usize) -> CommandError {
    CommandError::new(format!(
        "generated PDF task line on line {} is malformed; expected a generated task with one of [ ], [*], [/], [x], [X], or [-], such as '- [ ] #task #ref [[...pdf]] #hide ^ref'; legacy generated lines without #ref, with [p::2], or without #hide are still accepted",
        line_index + 1
    ))
}

pub(super) fn contains_markdown_token(line: &str, token: &str) -> bool {
    line.split_whitespace().any(|word| word == token)
}

pub(super) fn parse_markdown_task_checkbox(
    line: &str,
) -> Option<(usize, bool, char)> {
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }

    if !bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b'-' | b'*' | b'+'))
    {
        return None;
    }
    index += 1;
    if !bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }

    if bytes.get(index) != Some(&b'[') || bytes.get(index + 2) != Some(&b']') {
        return None;
    }
    let mark_index = index + 1;
    let mark = *bytes.get(mark_index)? as char;
    match mark {
        ' ' => Some((mark_index, false, mark)),
        'x' | 'X' => Some((mark_index, true, mark)),
        '*' | '/' | '-' => Some((mark_index, false, mark)),
        _ => None,
    }
}

pub(super) fn contains_pdf_wikilink(line: &str) -> bool {
    let mut remaining = line;
    while let Some(start) = remaining.find("[[") {
        let after_start = &remaining[start + 2..];
        let Some(end) = after_start.find("]]") else {
            return false;
        };
        let target = after_start[..end]
            .split('|')
            .next()
            .unwrap_or("")
            .split('#')
            .next()
            .unwrap_or("")
            .trim();
        if target.to_ascii_lowercase().ends_with(".pdf") {
            return true;
        }
        remaining = &after_start[end + 2..];
    }
    false
}

/// Resolves the synced status targeted by the visible `^ref` task state.
pub(super) fn pdf_task_target_status(
    status: PdfTaskStatus,
) -> Option<&'static str> {
    status.target_status()
}

pub(super) fn apply_pdf_task_status_signal(
    resolution: &mut SyncResolution,
    task_line: &PdfTaskLineState,
    base_projection: Option<&Projection>,
    marker_projection: &Projection,
    frontmatter_projection: &Projection,
) -> Result<PdfTaskStatusSignal> {
    let status = task_line.status();
    let mut signal = PdfTaskStatusSignal {
        status,
        status_contributed: None,
    };
    let Some(target_status) = pdf_task_target_status(status) else {
        return Ok(signal);
    };
    if projection_status_is(&resolution.projection, target_status) {
        return Ok(signal);
    }

    let conflicts = pdf_task_status_conflicts(
        base_projection,
        marker_projection,
        frontmatter_projection,
        target_status,
    );
    if !conflicts.is_empty() {
        return Err(pdf_task_status_conflict_error(
            status,
            target_status,
            &conflicts,
        ));
    }

    resolution.projection.insert(
        FIELD_STATUS.to_string(),
        MarkerValue::String(target_status.to_string()),
    );
    resolution.decision.frontmatter_contributed = true;
    if !resolution.decision.reason.is_empty() {
        resolution.decision.reason.push_str("; ");
    }
    if let Some(reason) = status.contribution_reason() {
        resolution.decision.reason.push_str(reason);
    }
    signal.status_contributed = Some(target_status);
    Ok(signal)
}

#[derive(Debug, Clone)]
pub(super) struct PdfTaskStatusConflict {
    pub(super) source: &'static str,
    pub(super) base: Option<MarkerValue>,
    pub(super) value: Option<MarkerValue>,
}

pub(super) fn pdf_task_status_conflicts(
    base_projection: Option<&Projection>,
    marker_projection: &Projection,
    frontmatter_projection: &Projection,
    target_status: &str,
) -> Vec<PdfTaskStatusConflict> {
    let Some(base_projection) = base_projection else {
        return Vec::new();
    };

    let mut conflicts = Vec::new();
    for (source, projection) in [
        ("marker", marker_projection),
        ("frontmatter", frontmatter_projection),
    ] {
        let base = base_projection.get(FIELD_STATUS);
        let value = projection.get(FIELD_STATUS);
        if value != base && !status_value_is(value, target_status) {
            conflicts.push(PdfTaskStatusConflict {
                source,
                base: base.cloned(),
                value: value.cloned(),
            });
        }
    }
    conflicts
}

pub(super) fn pdf_task_status_conflict_error(
    status: PdfTaskStatus,
    target_status: &str,
    conflicts: &[PdfTaskStatusConflict],
) -> CommandError {
    let mut message = format!(
        "{} PDF task conflicts with marker/frontmatter status edit:",
        status.label()
    );
    for conflict in conflicts.iter().take(4) {
        message.push_str(&format!(
            "\n  {} status={}, base={}",
            conflict.source,
            diagnostic_projection_value(conflict.value.as_ref()),
            diagnostic_projection_value(conflict.base.as_ref()),
        ));
    }
    message.push_str(&format!(
        "\n{} the PDF task or set the marker/frontmatter status to {target_status}",
        status.conflict_action()
    ));
    CommandError::new(message)
}

pub(super) fn projection_status_is(
    projection: &Projection,
    status: &str,
) -> bool {
    status_value_is(projection.get(FIELD_STATUS), status)
}

pub(super) fn projection_pdf_task_mark(projection: &Projection) -> char {
    match projection
        .get(FIELD_STATUS)
        .and_then(MarkerValue::as_string)
    {
        Some(STATUS_READY) => ' ',
        Some(STATUS_NEXT) => '*',
        Some(STATUS_WIP) => '/',
        Some(STATUS_READ) => 'x',
        Some(STATUS_ABANDONED) => '-',
        _ => ' ',
    }
}

pub(super) fn status_value_is(
    value: Option<&MarkerValue>,
    status: &str,
) -> bool {
    value.and_then(MarkerValue::as_string) == Some(status)
}
