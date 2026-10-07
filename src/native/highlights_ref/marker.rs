//! PDF marker read, parse, validate, and render.
use super::*;

pub(super) fn read_pdf_marker(path: &Path) -> Result<PdfMarker> {
    // Read the PDF from disk exactly once and reuse the bytes for both the
    // SHA-256 (pipeline metadata) and the lopdf parse, instead of reading the
    // whole file twice (Document::load plus a separate hash pass).
    let bytes = fs::read(path).map_err(|error| {
        CommandError::new(format!("read PDF {}: {error}", path.display()))
    })?;
    let source_pdf_sha256 = hex::encode(Sha256::digest(&bytes));
    let document = Document::load_mem(&bytes).map_err(|error| {
        CommandError::new(format!("read PDF {}: {error}", path.display()))
    })?;
    let Some(first_page_id) = document.page_iter().next() else {
        return Err(CommandError::new(format!(
            "no first page found in {}; marker note must be on page 1",
            path.display()
        )));
    };
    let mut note_number = 0;

    for annotation_id in annotation_ids_for_page(&document, first_page_id)? {
        let annotation =
            document.get_dictionary(annotation_id).map_err(|error| {
                CommandError::new(format!(
                    "read annotation {annotation_id:?} in {}: {error}",
                    path.display()
                ))
            })?;
        if !is_standalone_note(annotation) {
            continue;
        }
        note_number += 1;
        let contents = annotation
            .get(b"Contents")
            .ok()
            .map(decode_marker_contents)
            .transpose()
            .map_err(|error| {
                CommandError::new(format!(
                    "decode marker contents in {}: {error}",
                    path.display()
                ))
            })?
            .unwrap_or_default();
        return Ok(PdfMarker {
            annotation_id,
            contents,
            page_number: 1,
            note_number,
            source_pdf_sha256,
        });
    }

    Err(CommandError::new(format!(
        "no standalone /Text note annotations found on page 1 in {}",
        path.display()
    )))
}

pub(super) fn decode_marker_contents(
    contents: &Object,
) -> std::result::Result<String, lopdf::Error> {
    let Object::String(bytes, _) = contents else {
        return decode_text_string(contents);
    };
    if has_text_string_bom(bytes) {
        return decode_text_string(contents);
    }

    let mut decoded = String::new();
    let mut segment_start = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                decoded.push_str(&decode_marker_text_segment(
                    &bytes[segment_start..index],
                )?);
                decoded.push('\n');
                index += 1;
                if bytes.get(index) == Some(&b'\n') {
                    index += 1;
                }
                segment_start = index;
            }
            b'\n' => {
                decoded.push_str(&decode_marker_text_segment(
                    &bytes[segment_start..index],
                )?);
                decoded.push('\n');
                index += 1;
                segment_start = index;
            }
            _ => index += 1,
        }
    }
    decoded.push_str(&decode_marker_text_segment(&bytes[segment_start..])?);
    Ok(decoded)
}

pub(super) fn has_text_string_bom(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\xFE\xFF") || bytes.starts_with(b"\xEF\xBB\xBF")
}

pub(super) fn decode_marker_text_segment(
    segment: &[u8],
) -> std::result::Result<String, lopdf::Error> {
    decode_text_string(&Object::String(segment.to_vec(), StringFormat::Literal))
}

pub(super) fn annotation_ids_for_page(
    document: &Document,
    page_id: ObjectId,
) -> Result<Vec<ObjectId>> {
    let page = document.get_dictionary(page_id).map_err(|error| {
        CommandError::new(format!("read page {page_id:?}: {error}"))
    })?;
    let Ok(annots) = page.get(b"Annots") else {
        return Ok(Vec::new());
    };
    let annot_array = match annots {
        Object::Reference(id) => document
            .get_object(*id)
            .and_then(Object::as_array)
            .map_err(|error| {
                CommandError::new(format!(
                    "read annotation array {id:?}: {error}"
                ))
            })?,
        Object::Array(array) => array,
        _ => return Ok(Vec::new()),
    };

    let mut ids = Vec::new();
    for annot in annot_array {
        if let Ok(id) = annot.as_reference() {
            ids.push(id);
        }
    }
    Ok(ids)
}

pub(super) fn is_standalone_note(annotation: &lopdf::Dictionary) -> bool {
    annotation
        .get(b"Subtype")
        .and_then(Object::as_name)
        .is_ok_and(|name| name == b"Text")
}

pub(super) fn write_pdf_marker(
    path: &Path,
    annotation_id: ObjectId,
    contents: &str,
) -> Result<()> {
    let mut document = Document::load(path).map_err(|error| {
        CommandError::new(format!("read PDF {}: {error}", path.display()))
    })?;
    let annotation = document
        .get_object_mut(annotation_id)
        .and_then(Object::as_dict_mut)
        .map_err(|error| {
            CommandError::new(format!(
                "read marker annotation {annotation_id:?} in {}: {error}",
                path.display()
            ))
        })?;
    annotation.set("Contents", pdf_text_string(contents));
    atomic_save_pdf(path, &mut document)
}

pub(super) fn pdf_text_string(contents: &str) -> Object {
    Object::String(encode_utf16_be(contents), StringFormat::Hexadecimal)
}

pub(super) fn atomic_save_pdf(
    path: &Path,
    document: &mut Document,
) -> Result<()> {
    let temp_path = temporary_write_path(path)?;
    let _ = fs::remove_file(&temp_path);
    document.save(&temp_path).map_err(|error| {
        CommandError::new(format!(
            "write temporary PDF {}: {error}",
            temp_path.display()
        ))
    })?;
    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        CommandError::new(format!("install PDF {}: {error}", path.display()))
    })
}

pub(super) fn parse_marker(contents: &str) -> Result<Projection> {
    Ok(parse_marker_with_normalization(contents)?.projection)
}

pub(super) fn parse_marker_with_normalization(
    contents: &str,
) -> Result<NormalizedProjection> {
    let mut projection = Projection::new();
    for (index, line) in contents.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        else {
            return Err(CommandError::new(format!(
                "invalid marker item on line {line_number}: expected '- key: value' or '* key: value'"
            )));
        };
        let Some((key, value)) = item.split_once(':') else {
            return Err(CommandError::new(format!(
                "invalid marker item on line {line_number}: missing ':'"
            )));
        };
        let key = normalize_key(key);
        if key.is_empty() {
            return Err(CommandError::new(format!(
                "invalid marker item on line {line_number}: empty key"
            )));
        }
        if is_pipeline_field(&key) {
            return Err(CommandError::new(format!(
                "invalid marker item on line {line_number}: '{key}' is pipeline-owned and cannot be synced from the marker"
            )));
        }
        if is_command_managed_field(&key) {
            return Err(CommandError::new(format!(
                "invalid marker item on line {line_number}: '{key}' is command-managed and cannot be synced from the marker"
            )));
        }
        if key == FIELD_CREATED {
            return Err(CommandError::new(format!(
                "invalid marker item on line {line_number}: 'created' records reference-note creation time and cannot be synced from the marker"
            )));
        }
        if projection.contains_key(&key) {
            return Err(CommandError::new(format!(
                "duplicate marker key on line {line_number}: {key}"
            )));
        }
        let parsed_value = parse_value(value);
        if key == FIELD_PARENT {
            validate_marker_parent_value(value, line_number, &parsed_value)?;
        }
        projection.insert(key, parsed_value);
    }

    canonicalize_parent(&mut projection, "marker")?;
    let status_normalized = normalize_deprecated_status(&mut projection);
    validate_required_marker_keys(&projection, "marker")?;
    Ok(NormalizedProjection {
        projection,
        status_normalized,
    })
}

pub(super) fn normalize_deprecated_status(
    projection: &mut Projection,
) -> Option<DeprecatedStatusNormalization> {
    let Some(MarkerValue::String(status)) = projection.get_mut(FIELD_STATUS)
    else {
        return None;
    };
    match status.as_str() {
        DEPRECATED_STATUS_UNREAD => {
            *status = STATUS_READY.to_string();
            Some(DeprecatedStatusNormalization::UnreadToReady)
        }
        DEPRECATED_STATUS_DONE => {
            *status = STATUS_READ.to_string();
            Some(DeprecatedStatusNormalization::DoneToRead)
        }
        _ => None,
    }
}

pub(super) fn validate_required_marker_keys(
    projection: &Projection,
    source: &str,
) -> Result<()> {
    for key in MARKER_REQUIRED_KEYS {
        let Some(value) = projection.get(*key) else {
            return Err(CommandError::new(format!(
                "missing required marker key: {key}"
            )));
        };
        if value.is_empty_required_value() {
            return Err(CommandError::new(format!(
                "{source} has an empty required marker key: {key}"
            )));
        }
    }
    validate_status_value(projection, source)?;
    Ok(())
}

pub(super) fn validate_status_value(
    projection: &Projection,
    source: &str,
) -> Result<()> {
    let Some(value) = projection.get(FIELD_STATUS) else {
        return Ok(());
    };
    let Some(status) = value.as_string() else {
        return Err(CommandError::new(format!(
            "{source} status must be a scalar string; supported statuses: {}",
            ALLOWED_STATUS_VALUES.join(", ")
        )));
    };
    if ALLOWED_STATUS_VALUES.contains(&status) {
        return Ok(());
    }
    Err(CommandError::new(format!(
        "{source} has unsupported status {}: supported statuses: {}",
        quote_string(status),
        ALLOWED_STATUS_VALUES.join(", ")
    )))
}

pub(super) fn render_marker(projection: &Projection) -> Result<String> {
    let mut rendered = String::new();
    for key in ordered_projection_keys(projection) {
        let Some(value) = projection.get(&key) else {
            continue;
        };
        rendered.push_str("- ");
        rendered.push_str(&key);
        rendered.push_str(": ");
        if key == FIELD_PARENT {
            rendered.push_str(&render_marker_parent_value(value)?);
        } else {
            rendered.push_str(&value.as_marker_value());
        }
        rendered.push('\n');
    }
    Ok(rendered)
}

pub(super) fn validate_marker_parent_value(
    raw_value: &str,
    line_number: usize,
    parsed_value: &MarkerValue,
) -> Result<()> {
    let trimmed = raw_value.trim();
    if trimmed.is_empty() {
        return Err(empty_parent_error("marker"));
    }
    if matches!(parsed_value, MarkerValue::Null) {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; null is not supported"
        )));
    }
    if matches!(parsed_value, MarkerValue::List(_)) {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; inline lists are not supported"
        )));
    }
    if trimmed.starts_with("![[") && trimmed.ends_with("]]") {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; embeds are not supported"
        )));
    }
    if is_wikilink(trimmed) {
        let detail = if trimmed[2..trimmed.len() - 2].contains('|') {
            "aliases are not supported"
        } else if trimmed[2..trimmed.len() - 2].contains("#^") {
            "block links are not supported"
        } else {
            "wikilinks are not supported"
        };
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; {detail}"
        )));
    }
    if trimmed.contains("[[") || trimmed.contains("]]") {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; wikilinks are not supported"
        )));
    }
    if trimmed.starts_with(['"', '\'']) || trimmed.ends_with(['"', '\'']) {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; quoted parent values are not supported"
        )));
    }
    if trimmed.contains("#^") {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; block links are not supported"
        )));
    }
    if trimmed.contains('|') {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; aliases are not supported"
        )));
    }
    if trimmed.starts_with('[') || trimmed.ends_with(']') {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; structured marker syntax is not supported"
        )));
    }
    Ok(())
}

pub(super) fn render_marker_parent_value(
    value: &MarkerValue,
) -> Result<String> {
    let Some(parent) = value.as_string() else {
        return Err(CommandError::new(
            "parent cannot be rendered as a PDF marker bare note target: expected a canonical wikilink string",
        ));
    };
    let Some(target) = simple_wikilink_target(parent) else {
        return Err(CommandError::new(format!(
            "parent cannot be rendered as a PDF marker bare note target: expected a simple wikilink like [[memory_ref]], got {}",
            quote_string(parent)
        )));
    };
    Ok(target.to_string())
}

pub(super) fn ordered_projection_keys(projection: &Projection) -> Vec<String> {
    let mut keys = Vec::new();
    for key in MARKER_REQUIRED_KEYS {
        if projection.contains_key(*key) {
            keys.push((*key).to_string());
        }
    }
    for field in COMMON_USER_FIELDS {
        if projection.contains_key(*field)
            && !MARKER_REQUIRED_KEYS.contains(field)
        {
            keys.push((*field).to_string());
        }
    }
    for key in projection.keys() {
        if !MARKER_REQUIRED_KEYS.contains(&key.as_str())
            && !COMMON_USER_FIELDS.contains(&key.as_str())
        {
            keys.push(key.clone());
        }
    }
    keys
}

pub(super) fn parse_value(value: &str) -> MarkerValue {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return MarkerValue::String(String::new());
    }
    if trimmed.eq_ignore_ascii_case("null") || trimmed == "~" {
        return MarkerValue::Null;
    }
    if trimmed.eq_ignore_ascii_case("true") {
        return MarkerValue::Bool(true);
    }
    if trimmed.eq_ignore_ascii_case("false") {
        return MarkerValue::Bool(false);
    }
    if let Some(value) = parse_quoted_string(trimmed) {
        return MarkerValue::String(value);
    }
    if is_wikilink(trimmed) {
        return MarkerValue::String(trimmed.to_string());
    }
    if let Some(values) = parse_inline_list(trimmed) {
        return MarkerValue::List(values);
    }
    if is_number_literal(trimmed) {
        return MarkerValue::Number(trimmed.to_string());
    }
    MarkerValue::String(trimmed.to_string())
}

pub(super) fn parse_quoted_string(value: &str) -> Option<String> {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return serde_json::from_str::<String>(value).ok();
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        let inner = &value[1..value.len() - 1];
        return Some(inner.replace("''", "'"));
    }
    None
}

pub(super) fn parse_inline_list(value: &str) -> Option<Vec<MarkerValue>> {
    if !(value.starts_with('[') && value.ends_with(']')) {
        return None;
    }
    let inner = &value[1..value.len() - 1];
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }

    split_inline_list(inner)
        .map(|items| items.into_iter().map(|item| parse_value(&item)).collect())
}

pub(super) fn split_inline_list(value: &str) -> Option<Vec<String>> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars().peekable();
    let mut quote = None;
    let mut bracket_depth = 0usize;

    while let Some(character) = chars.next() {
        match (quote, character) {
            (Some('"'), '\\') => {
                current.push(character);
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (Some(active), c) if c == active => {
                quote = None;
                current.push(c);
            }
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => {
                quote = Some(character);
                current.push(character);
            }
            (None, '[') => {
                bracket_depth += 1;
                current.push(character);
            }
            (None, ']') if bracket_depth > 0 => {
                bracket_depth -= 1;
                current.push(character);
            }
            (None, ',') if bracket_depth == 0 => {
                items.push(current.trim().to_string());
                current.clear();
            }
            (None, c) => current.push(c),
        }
    }

    if quote.is_some() || bracket_depth != 0 {
        return None;
    }
    items.push(current.trim().to_string());
    Some(items)
}

pub(super) fn is_number_literal(value: &str) -> bool {
    if value.starts_with('+') {
        return false;
    }
    if value.parse::<i64>().is_ok() {
        return true;
    }
    value.contains('.')
        && value.parse::<f64>().is_ok()
        && value.chars().all(|character| {
            character.is_ascii_digit() || matches!(character, '-' | '.')
        })
}

pub(super) fn normalize_key(key: &str) -> String {
    key.trim()
        .chars()
        .map(|character| match character {
            '-' | ' ' => '_',
            other => other.to_ascii_lowercase(),
        })
        .collect::<String>()
}

/// Narrow seam for the `index` phase (`ref_library`): map a `^ref` checkbox
/// mark to its canonical status through the [`PdfTaskLine`] mapping, without
/// exposing task-line types. Returns `None` for an unknown mark.
pub(crate) fn ref_task_mark_status(mark: char) -> Option<&'static str> {
    match mark {
        ' ' | '*' | '/' | 'x' | 'X' | '-' => PdfTaskLine {
            line_index: 0,
            checkbox_mark_index: 0,
            checked: false,
            mark,
        }
        .status()
        .target_status(),
        _ => None,
    }
}

/// Narrow seam for the `index` phase (`ref_library`): the deprecated
/// frontmatter alias map from [`normalize_deprecated_status`], returning the
/// input unchanged when it carries no deprecated alias.
pub(crate) fn normalize_deprecated_status_str(status: &str) -> &str {
    match status {
        DEPRECATED_STATUS_UNREAD => STATUS_READY,
        DEPRECATED_STATUS_DONE => STATUS_READ,
        _ => status,
    }
}
