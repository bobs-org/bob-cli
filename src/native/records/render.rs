use std::collections::BTreeMap;

use chrono::NaiveDate;

use super::super::capture_project_note::{
    format_project_section_title, is_project_section_title,
    normalize_project_section_title,
};
use super::schema::{
    validate_field_input, FieldInputState, RecordDiagnostic,
    RecordDiagnosticCode, RecordField, RecordScalar, RecordType, RecordValue,
    ValidatedFieldInput,
};

/// One first-level authored note plus its nested bullets.
///
/// `text` is the first-level bullet text without any `- ` prefix or dated
/// prefix. `children` are nested bullet texts without any indent or `- `
/// prefix. When `children` is non-empty and `text` is ALL-CAPS, the entry is
/// a named section and renders as a `## Title Case` section using the
/// project-note section helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordNoteInput {
    pub(crate) text: String,
    pub(crate) children: Vec<String>,
}

impl RecordNoteInput {
    pub(crate) fn bullet(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            children: Vec::new(),
        }
    }

    pub(crate) fn section(
        text: impl Into<String>,
        children: Vec<String>,
    ) -> Self {
        Self {
            text: text.into(),
            children,
        }
    }
}

/// Everything `render_record` needs. Dates arrive resolved: `created` is the
/// already-formatted frontmatter timestamp, `note_date` is the `YYYY-MM-DD`
/// string for dated notes, and `today` resolves dynamic `today`/`yesterday`
/// date values that survived validation as dynamic defaults.
#[derive(Debug, Clone)]
pub(crate) struct RecordRenderRequest<'a> {
    pub(crate) record_type: &'a RecordType,
    pub(crate) title: &'a str,
    pub(crate) parent: Option<&'a str>,
    pub(crate) created: &'a str,
    pub(crate) values: &'a BTreeMap<String, RecordValue>,
    pub(crate) notes: &'a [RecordNoteInput],
    pub(crate) note_date: Option<&'a str>,
    pub(crate) today: NaiveDate,
}

/// New note bytes plus the body after frontmatter for previews.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedRecord {
    pub(crate) markdown: String,
    pub(crate) body: String,
}

/// One field edit plus notes to append. Only listed fields are validated;
/// every other schema defect is reported as a warning.
#[derive(Debug, Clone)]
pub(crate) struct RecordUpdateRequest<'a> {
    pub(crate) record_type: &'a RecordType,
    pub(crate) original: &'a str,
    pub(crate) fields: &'a [super::schema::RecordFieldInput],
    pub(crate) notes: &'a [RecordNoteInput],
    pub(crate) note_date: Option<&'a str>,
    pub(crate) today: NaiveDate,
}

/// Edited bytes plus changed keys in schema order and unrelated warnings.
#[derive(Debug, Clone)]
pub(crate) struct AppliedRecordUpdate {
    pub(crate) markdown: String,
    pub(crate) changed: Vec<String>,
    pub(crate) warnings: Vec<RecordDiagnostic>,
}

/// Typed refusal that always carries the unchanged bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordUpdateError {
    pub(crate) code: RecordUpdateErrorCode,
    pub(crate) field: Option<String>,
    pub(crate) message: String,
    pub(crate) unchanged: String,
}

/// Machine-readable refusal reasons for targeted updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordUpdateErrorCode {
    MissingFrontmatter,
    MalformedFrontmatter,
    DuplicateKey,
    UnsupportedYaml,
    RequiredEmpty,
    InvalidValue,
}

impl std::fmt::Display for RecordUpdateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(field) = &self.field {
            write!(
                formatter,
                "record update refused ({} field {field}): {}",
                self.code.as_str(),
                self.message
            )
        } else {
            write!(
                formatter,
                "record update refused ({}): {}",
                self.code.as_str(),
                self.message
            )
        }
    }
}

impl std::error::Error for RecordUpdateError {}

impl RecordUpdateErrorCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::MissingFrontmatter => "missing-frontmatter",
            Self::MalformedFrontmatter => "malformed-frontmatter",
            Self::DuplicateKey => "duplicate-key",
            Self::UnsupportedYaml => "unsupported-yaml",
            Self::RequiredEmpty => "required-empty",
            Self::InvalidValue => "invalid-value",
        }
    }
}

fn refusal(
    code: RecordUpdateErrorCode,
    field: Option<&str>,
    message: impl Into<String>,
    unchanged: &str,
) -> RecordUpdateError {
    RecordUpdateError {
        code,
        field: field.map(str::to_owned),
        message: message.into(),
        unchanged: unchanged.to_owned(),
    }
}

/// Quote with JSON string syntax, which YAML double-quoted style accepts.
fn yaml_quote(text: &str) -> String {
    serde_json::to_string(text).expect("JSON string quoting never fails")
}

fn is_bare_choice_token(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    if !token
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return false;
    }
    !matches!(
        token.to_ascii_lowercase().as_str(),
        "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "y" | "n"
    )
}

fn format_number(value: f64) -> String {
    value.to_string()
}

fn format_scalar(scalar: &RecordScalar, today: NaiveDate) -> String {
    match scalar {
        RecordScalar::Text(text) => yaml_quote(text),
        RecordScalar::Choice(token) => {
            if is_bare_choice_token(token) {
                token.clone()
            } else {
                yaml_quote(token)
            }
        }
        RecordScalar::Date(date) => {
            let resolved = date.resolve(today).unwrap_or(today);
            resolved.format("%Y-%m-%d").to_string()
        }
        RecordScalar::Link(link) => yaml_quote(link),
        RecordScalar::Url(url) => url.to_string(),
        RecordScalar::Number(number) => format_number(*number),
    }
}

fn format_value(value: &RecordValue, today: NaiveDate) -> Vec<String> {
    match value {
        RecordValue::Scalar(scalar) => vec![format_scalar(scalar, today)],
        RecordValue::List(items) => items
            .iter()
            .map(|scalar| format_scalar(scalar, today))
            .collect(),
    }
}

fn is_section_note(note: &RecordNoteInput) -> bool {
    !note.children.is_empty() && is_project_section_title(&note.text)
}

fn render_note_bullet(
    dated_notes: bool,
    note_date: Option<&str>,
    note: &RecordNoteInput,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut dated: Option<&str> = None;
    if dated_notes && !has_date_prefix(&note.text) {
        dated = note_date;
    }
    let prefix = match dated {
        Some(date) => format!("- {date}: {}", note.text),
        None => format!("- {}", note.text),
    };
    lines.push(prefix);
    for child in &note.children {
        lines.push(format!("  - {child}"));
    }
    lines
}

/// Render a new record note. Creation order is parent (when resolved), type,
/// title, schema fields in schema order, then created. String scalars use
/// safe double quoting, choice tokens and dates/URLs/numbers stay bare when
/// YAML-safe, lists are block lists, empty optionals are omitted, and legacy
/// keys are never written. The body always holds H1 and `## Notes`.
pub(crate) fn render_record(
    request: &RecordRenderRequest<'_>,
) -> RenderedRecord {
    let record_type = request.record_type;
    let mut frontmatter = String::from("---\n");
    if let Some(parent) = request.parent {
        frontmatter.push_str(&format!("parent: {}\n", yaml_quote(parent)));
    }
    frontmatter.push_str(&format!(
        "type: {}\n",
        yaml_quote(&format!("[[{}]]", record_type.id))
    ));
    frontmatter.push_str(&format!("title: {}\n", yaml_quote(request.title)));
    for field in &record_type.fields {
        let Some(value) = request.values.get(&field.key) else {
            continue;
        };
        match value {
            RecordValue::Scalar(scalar) => {
                frontmatter.push_str(&format!(
                    "{}: {}\n",
                    field.key,
                    format_scalar(scalar, request.today)
                ));
            }
            RecordValue::List(items) => {
                if items.is_empty() {
                    continue;
                }
                frontmatter.push_str(&format!("{}:\n", field.key));
                for scalar in items {
                    frontmatter.push_str(&format!(
                        "  - {}\n",
                        format_scalar(scalar, request.today)
                    ));
                }
            }
        }
    }
    frontmatter.push_str(&format!("created: {}\n", request.created));
    frontmatter.push_str("---\n");

    let mut ordinary: Vec<&RecordNoteInput> = Vec::new();
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    let mut section_index: BTreeMap<String, usize> = BTreeMap::new();
    for note in request.notes {
        if is_section_note(note) {
            let title = format_project_section_title(&note.text);
            let normalized = normalize_project_section_title(&title);
            let rendered_children: Vec<String> = note
                .children
                .iter()
                .map(|child| format!("- {child}"))
                .collect();
            if let Some(&index) = section_index.get(&normalized) {
                sections[index].1.extend(rendered_children);
            } else {
                section_index.insert(normalized, sections.len());
                sections.push((title, rendered_children));
            }
        } else {
            ordinary.push(note);
        }
    }

    let mut body = format!("# {}\n\n## Notes\n", request.title);
    if !ordinary.is_empty() {
        body.push('\n');
        for note in ordinary {
            for line in render_note_bullet(
                record_type.dated_notes,
                request.note_date,
                note,
            ) {
                body.push_str(&line);
                body.push('\n');
            }
        }
    }
    for (title, note_lines) in &sections {
        body.push_str(&format!("\n## {title}\n\n"));
        for line in note_lines {
            body.push_str(line);
            body.push('\n');
        }
    }

    let markdown = format!("{frontmatter}\n{body}");
    RenderedRecord { markdown, body }
}

fn has_date_prefix(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 12 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        if index < 4 || (index >= 5 && index < 7) || (index >= 8 && index < 10)
        {
            if !byte.is_ascii_digit() {
                return false;
            }
        } else if index == 4 || index == 7 {
            if *byte != b'-' {
                return false;
            }
        } else if index == 10 {
            if *byte != b':' {
                return false;
            }
        } else if index == 11 {
            if *byte != b' ' {
                return false;
            }
            return true;
        } else {
            return false;
        }
    }
    false
}

#[derive(Debug, Clone)]
struct SplitLine {
    content: String,
    ending: String,
}

fn split_preserving_ends(text: &str) -> Vec<SplitLine> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\n' {
            let end = index + 1;
            let raw = &text[start..end];
            let (content, ending) = if raw.ends_with("\r\n") {
                (raw[..raw.len() - 2].to_owned(), "\r\n".to_owned())
            } else {
                (raw[..raw.len() - 1].to_owned(), "\n".to_owned())
            };
            lines.push(SplitLine { content, ending });
            start = end;
        }
        index += 1;
    }
    if start < text.len() {
        lines.push(SplitLine {
            content: text[start..].to_owned(),
            ending: String::new(),
        });
    } else if text.is_empty() {
        lines.push(SplitLine {
            content: String::new(),
            ending: String::new(),
        });
    }
    lines
}

fn detect_newline(original: &str) -> &'static str {
    if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

#[derive(Debug, Clone)]
struct KeySpan {
    name: String,
    key_line: usize,
    value_end: usize,
    prefix: String,
    comment: String,
    is_list: bool,
}

fn parse_key_line(content: &str) -> Option<(String, String, String)> {
    let indent_len = content.len() - content.trim_start().len();
    let indent = content[..indent_len].to_owned();
    let rest = &content[indent_len..];
    if rest.is_empty() {
        return None;
    }
    if let Some(quote) = rest.chars().next()
        && (quote == '"' || quote == '\'')
    {
        let mut closing: Option<usize> = None;
        let bytes = rest.as_bytes();
        let mut cursor = 1;
        while cursor < bytes.len() {
            if quote == '"' && bytes[cursor] == b'\\' {
                cursor += 2;
                continue;
            }
            if quote == '\'' {
                if bytes[cursor] == b'\''
                    && cursor + 1 < bytes.len()
                    && bytes[cursor + 1] == b'\''
                {
                    cursor += 2;
                    continue;
                }
                if bytes[cursor] == b'\'' {
                    closing = Some(cursor);
                    break;
                }
                cursor += 1;
                continue;
            }
            if bytes[cursor] == b'"' {
                closing = Some(cursor);
                break;
            }
            cursor += 1;
        }
        let close = closing?;
        let name = rest[1..close].to_owned();
        let after = &rest[close + 1..];
        let after_trimmed = after.trim_start();
        if !after_trimmed.starts_with(':') {
            return None;
        }
        let colon = after.find(':')?;
        let raw_key = rest[..close + 1].to_owned();
        let prefix = format!("{indent}{raw_key}{}", &after[..colon + 1]);
        let after_colon = after[colon + 1..].to_owned();
        return Some((name, prefix, after_colon));
    }
    let colon = rest.find(':')?;
    let name = rest[..colon].trim_end().to_owned();
    if name.is_empty()
        || name.contains(' ')
        || name.contains('\t')
        || name.contains('[')
        || name.contains(']')
        || name.contains('{')
        || name.contains('}')
    {
        return None;
    }
    let prefix = format!("{indent}{}", &rest[..colon + 1]);
    let after_colon = rest[colon + 1..].to_owned();
    Some((name, prefix, after_colon))
}

fn is_list_item(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with("- ") || trimmed == "-" || trimmed.starts_with("-\t")
}

fn is_blank_or_comment(content: &str) -> bool {
    let trimmed = content.trim();
    trimmed.is_empty() || trimmed.starts_with('#')
}

fn split_value_and_comment(after_colon: &str) -> (String, String) {
    let bytes = after_colon.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_double {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == b'"' {
                in_double = false;
            }
            index += 1;
            continue;
        }
        if in_single {
            if byte == b'\''
                && index + 1 < bytes.len()
                && bytes[index + 1] == b'\''
            {
                index += 2;
                continue;
            }
            if byte == b'\'' {
                in_single = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            in_double = true;
            index += 1;
            continue;
        }
        if byte == b'\'' {
            in_single = true;
            index += 1;
            continue;
        }
        if byte == b'#'
            && index > 0
            && (bytes[index - 1] == b' ' || bytes[index - 1] == b'\t')
        {
            let value = after_colon[..index].to_owned();
            let comment = after_colon[index - 1..].to_owned();
            return (value, comment);
        }
        index += 1;
    }
    (after_colon.to_owned(), String::new())
}

fn raw_value_has_unsafe_yaml(value_text: &str) -> bool {
    let trimmed = value_text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed == "[]" || trimmed == "{}" {
        return false;
    }
    let bytes = trimmed.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_double {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == b'"' {
                in_double = false;
            }
            index += 1;
            continue;
        }
        if in_single {
            if byte == b'\''
                && index + 1 < bytes.len()
                && bytes[index + 1] == b'\''
            {
                index += 2;
                continue;
            }
            if byte == b'\'' {
                in_single = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            in_double = true;
            index += 1;
            continue;
        }
        if byte == b'\'' {
            in_single = true;
            index += 1;
            continue;
        }
        if byte == b'{' || byte == b'}' {
            return true;
        }
        index += 1;
    }
    let first = trimmed.chars().next().unwrap_or(' ');
    matches!(first, '&' | '*' | '!' | '|' | '>' | '?')
}

fn collect_key_spans(lines: &[SplitLine]) -> (Vec<KeySpan>, Vec<String>) {
    let mut spans: Vec<KeySpan> = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut index = 0;
    while index < lines.len() {
        let content = &lines[index].content;
        if is_blank_or_comment(content) {
            index += 1;
            continue;
        }
        let Some((name, prefix, after_colon)) = parse_key_line(content) else {
            if is_list_item(content) {
                index += 1;
                continue;
            }
            if let Some(last) = spans.last_mut() {
                let trimmed = content.trim_start();
                let indent_len = content.len() - trimmed.len();
                if indent_len > 0 && !trimmed.is_empty() {
                    last.value_end = index;
                }
            }
            index += 1;
            continue;
        };
        *counts.entry(name.clone()).or_insert(0) += 1;
        let (value_part, comment) = split_value_and_comment(&after_colon);
        if value_part.trim().is_empty() {
            let mut end = index;
            let mut cursor = index + 1;
            let mut saw_list = false;
            while cursor < lines.len() {
                let next = &lines[cursor].content;
                if next.trim().is_empty() || next.trim_start().starts_with('#')
                {
                    cursor += 1;
                    continue;
                }
                if is_list_item(next) {
                    saw_list = true;
                    end = cursor;
                    cursor += 1;
                    continue;
                }
                if saw_list {
                    break;
                }
                let next_trimmed = next.trim_start();
                let next_indent = next.len() - next_trimmed.len();
                if next_indent > 0 && parse_key_line(next).is_none() {
                    end = cursor;
                    cursor += 1;
                    continue;
                }
                break;
            }
            spans.push(KeySpan {
                name,
                key_line: index,
                value_end: end,
                prefix,
                comment,
                is_list: saw_list,
            });
            index = end + 1;
            continue;
        }
        let trimmed_value = value_part.trim();
        let starts_block =
            trimmed_value.starts_with('|') || trimmed_value.starts_with('>');
        if starts_block {
            let mut end = index;
            let mut cursor = index + 1;
            while cursor < lines.len() {
                let next = &lines[cursor].content;
                if next.trim().is_empty() {
                    end = cursor;
                    cursor += 1;
                    continue;
                }
                let next_trimmed = next.trim_start();
                let next_indent = next.len() - next_trimmed.len();
                if next_indent > 0 {
                    end = cursor;
                    cursor += 1;
                    continue;
                }
                break;
            }
            spans.push(KeySpan {
                name,
                key_line: index,
                value_end: end,
                prefix,
                comment,
                is_list: false,
            });
            index = end + 1;
            continue;
        }
        let mut end = index;
        if is_quoted_multiline(&value_part) {
            let mut cursor = index + 1;
            let mut closed = false;
            while cursor < lines.len() {
                end = cursor;
                if closes_quote(&lines[cursor].content) {
                    closed = true;
                    break;
                }
                cursor += 1;
            }
            if !closed {
                end = index;
            }
            spans.push(KeySpan {
                name,
                key_line: index,
                value_end: end,
                prefix,
                comment,
                is_list: false,
            });
            index = end + 1;
            continue;
        }
        spans.push(KeySpan {
            name,
            key_line: index,
            value_end: end,
            prefix,
            comment,
            is_list: false,
        });
        index += 1;
    }
    let duplicates = counts
        .into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name))
        .collect();
    (spans, duplicates)
}

fn is_quoted_multiline(value_part: &str) -> bool {
    let trimmed = value_part.trim_start();
    if !(trimmed.starts_with('"') || trimmed.starts_with('\'')) {
        return false;
    }
    let quote = trimmed.chars().next().unwrap_or('"');
    let mut escaped = false;
    let mut seen_open = false;
    let mut closed = false;
    for character in trimmed.chars() {
        if !seen_open {
            seen_open = true;
            continue;
        }
        if quote == '"' {
            if escaped {
                escaped = false;
                continue;
            }
            if character == '\\' {
                escaped = true;
                continue;
            }
            if character == '"' {
                closed = true;
            }
        } else if character == '\'' {
            closed = true;
        }
    }
    seen_open && !closed
}

fn closes_quote(content: &str) -> bool {
    let trimmed = content.trim();
    if trimmed.contains('"') || trimmed.contains('\'') {
        return true;
    }
    false
}

fn span_raw_text(lines: &[SplitLine], span: &KeySpan) -> String {
    if span.key_line == span.value_end && !span.is_list {
        let (_, after) = split_value_and_comment_untracked(lines, span);
        return after.trim().to_owned();
    }
    let mut parts = Vec::new();
    let (_, first_after) = split_value_and_comment_untracked(lines, span);
    if !first_after.trim().is_empty() {
        parts.push(first_after.trim().to_owned());
    }
    let mut cursor = span.key_line + 1;
    while cursor <= span.value_end {
        parts.push(lines[cursor].content.trim().to_owned());
        cursor += 1;
    }
    parts.join("\n")
}

fn split_value_and_comment_untracked(
    lines: &[SplitLine],
    span: &KeySpan,
) -> (String, String) {
    let content = &lines[span.key_line].content;
    let indent_len = content.len() - content.trim_start().len();
    let rest = &content[indent_len..];
    let Some(colon) = rest.find(':') else {
        return (span.prefix.clone(), String::new());
    };
    let after_colon = &rest[colon + 1..];
    let (value, _) = split_value_and_comment(after_colon);
    (span.prefix.clone(), value)
}

fn schema_position(record_type: &RecordType, key: &str) -> Option<usize> {
    record_type.fields.iter().position(|field| field.key == key)
}

fn field_by_key<'a>(
    record_type: &'a RecordType,
    key: &str,
) -> Option<&'a RecordField> {
    record_type.fields.iter().find(|field| field.key == key)
}

fn join_lines(lines: &[SplitLine]) -> String {
    let mut text = String::new();
    for line in lines {
        text.push_str(&line.content);
        text.push_str(&line.ending);
    }
    text
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BodyHeading {
    line: usize,
    level: usize,
    title: String,
}

fn parse_body_headings(body_lines: &[SplitLine]) -> Vec<BodyHeading> {
    let mut headings = Vec::new();
    let mut in_fence = false;
    for (index, line) in body_lines.iter().enumerate() {
        let trimmed = line.content.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let mut level = 0;
        for byte in trimmed.bytes() {
            if byte == b'#' {
                level += 1;
            } else {
                break;
            }
        }
        if level == 0 || level > 6 {
            continue;
        }
        let rest = trimmed[level..].trim_start();
        if rest.is_empty() {
            continue;
        }
        headings.push(BodyHeading {
            line: index,
            level,
            title: rest.to_owned(),
        });
    }
    headings
}

fn last_nonblank_before(
    body_lines: &[SplitLine],
    start: usize,
    end: usize,
) -> Option<usize> {
    if start >= end {
        return None;
    }
    let mut cursor = end;
    while cursor > start {
        cursor -= 1;
        if !body_lines[cursor].content.trim().is_empty() {
            return Some(cursor);
        }
        if cursor == start {
            break;
        }
    }
    None
}

fn frontmatter_text(lines: &[SplitLine]) -> String {
    lines
        .iter()
        .map(|line| line.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn collect_warnings(
    record_type: &RecordType,
    front_lines: &[SplitLine],
    edited_keys: &[String],
    today: NaiveDate,
) -> Vec<RecordDiagnostic> {
    let mut warnings = Vec::new();
    let joined = frontmatter_text(front_lines);
    let parsed: serde_yaml::Value =
        serde_yaml::from_str(&joined).unwrap_or(serde_yaml::Value::Null);
    let mapping = match parsed.as_mapping() {
        Some(mapping) => mapping,
        None => return warnings,
    };
    for field in &record_type.fields {
        if edited_keys.iter().any(|key| key == &field.key) {
            continue;
        }
        let key = serde_yaml::Value::String(field.key.clone());
        let Some(raw) = mapping.get(&key) else {
            if field.required {
                warnings.push(RecordDiagnostic {
                    code: RecordDiagnosticCode::InvalidValue,
                    config_path: None,
                    record_type: None,
                    field: Some(field.key.clone()),
                    message: "required field is missing".to_owned(),
                });
            }
            continue;
        };
        if raw.is_null() {
            warnings.push(RecordDiagnostic {
                code: RecordDiagnosticCode::InvalidValue,
                config_path: None,
                record_type: None,
                field: Some(field.key.clone()),
                message: "null values are not written; remove the key or supply a value"
                    .to_owned(),
            });
            continue;
        }
        let inputs: Vec<String> = match raw {
            serde_yaml::Value::String(text) => vec![text.clone()],
            serde_yaml::Value::Bool(flag) => vec![flag.to_string()],
            serde_yaml::Value::Number(number) => vec![number.to_string()],
            serde_yaml::Value::Sequence(items) => {
                let mut members = Vec::new();
                for item in items {
                    match item {
                        serde_yaml::Value::String(text) => {
                            members.push(text.clone());
                        }
                        serde_yaml::Value::Bool(flag) => {
                            members.push(flag.to_string());
                        }
                        serde_yaml::Value::Number(number) => {
                            members.push(number.to_string());
                        }
                        serde_yaml::Value::Null => {
                            members.push(String::new());
                        }
                        _ => {}
                    }
                }
                members
            }
            _ => continue,
        };
        if inputs.iter().any(|input| input.is_empty()) {
            warnings.push(RecordDiagnostic {
                code: RecordDiagnosticCode::InvalidValue,
                config_path: None,
                record_type: None,
                field: Some(field.key.clone()),
                message:
                    "empty list members must use the explicit empty-field state"
                        .to_owned(),
            });
            continue;
        }
        if inputs.is_empty() {
            continue;
        }
        let state = FieldInputState::SuppliedValues(inputs);
        if super::schema::validate_field_values(
            field,
            match &state {
                FieldInputState::SuppliedValues(values) => values,
                _ => &[],
            },
            today,
        )
        .is_err()
        {
            warnings.push(RecordDiagnostic {
                code: RecordDiagnosticCode::InvalidValue,
                config_path: None,
                record_type: None,
                field: Some(field.key.clone()),
                message: "existing value fails current schema validation"
                    .to_owned(),
            });
        }
    }
    warnings
}

fn gap_after_colon(after_colon: &str) -> String {
    let mut gap = String::new();
    for character in after_colon.chars() {
        if character == ' ' || character == '\t' {
            gap.push(character);
        } else {
            break;
        }
    }
    if gap.is_empty() {
        gap.push(' ');
    }
    gap
}

fn apply_frontmatter_edits(
    front_lines: &mut Vec<SplitLine>,
    record_type: &RecordType,
    edits: &BTreeMap<String, ValidatedFieldInput>,
    order: &[String],
    newline: &str,
    today: NaiveDate,
    original: &str,
) -> Result<Vec<String>, RecordUpdateError> {
    let mut changed: Vec<String> = Vec::new();
    for key in order {
        let Some(validated) = edits.get(key) else {
            continue;
        };
        if matches!(validated, ValidatedFieldInput::Absent) {
            continue;
        }
        let (spans, duplicates) = collect_key_spans(front_lines);
        if duplicates.iter().any(|name| name == key) {
            return Err(refusal(
                RecordUpdateErrorCode::DuplicateKey,
                Some(key),
                format!(
                    "duplicate frontmatter key {key:?} cannot be edited safely"
                ),
                original,
            ));
        }
        let position = spans.iter().position(|span| span.name == *key);
        match validated {
            ValidatedFieldInput::Empty => {
                let Some(at) = position else {
                    continue;
                };
                let span = spans[at].clone();
                if raw_value_has_unsafe_yaml(&span_raw_text(front_lines, &span))
                {
                    return Err(refusal(
                        RecordUpdateErrorCode::UnsupportedYaml,
                        Some(key),
                        format!(
                            "existing {key:?} value uses flow or anchor syntax that cannot be edited safely"
                        ),
                        original,
                    ));
                }
                let field =
                    field_by_key(record_type, key).ok_or_else(|| {
                        refusal(
                            RecordUpdateErrorCode::InvalidValue,
                            Some(key),
                            format!("unknown record field {key:?}"),
                            original,
                        )
                    })?;
                if field.required {
                    return Err(refusal(
                        RecordUpdateErrorCode::RequiredEmpty,
                        Some(key),
                        format!("required field {key:?} cannot be cleared"),
                        original,
                    ));
                }
                let end = span.value_end;
                front_lines.drain(span.key_line..=end);
                changed.push(key.clone());
            }
            ValidatedFieldInput::Value(value) => {
                if field_by_key(record_type, key).is_none() {
                    return Err(refusal(
                        RecordUpdateErrorCode::InvalidValue,
                        Some(key),
                        format!("unknown record field {key:?}"),
                        original,
                    ));
                }
                let rendered = format_value(value, today);
                let is_list = matches!(value, RecordValue::List(_));
                if let Some(at) = position {
                    let span = spans[at].clone();
                    let raw = span_raw_text(front_lines, &span);
                    if raw_value_has_unsafe_yaml(&raw) {
                        return Err(refusal(
                            RecordUpdateErrorCode::UnsupportedYaml,
                            Some(key),
                            format!(
                                "existing {key:?} value uses flow or anchor syntax that cannot be edited safely"
                            ),
                            original,
                        ));
                    }
                    if is_list {
                        let key_content =
                            front_lines[span.key_line].content.clone();
                        let key_ending =
                            front_lines[span.key_line].ending.clone();
                        let indent_len =
                            key_content.len() - key_content.trim_start().len();
                        let rest = &key_content[indent_len..];
                        let Some(colon) = rest.find(':') else {
                            return Err(refusal(
                                RecordUpdateErrorCode::MalformedFrontmatter,
                                Some(key),
                                format!("key {key:?} line has no colon"),
                                original,
                            ));
                        };
                        let after_colon = &rest[colon + 1..];
                        let (_, comment) = split_value_and_comment(after_colon);
                        let new_key_content =
                            format!("{}{comment}", span.prefix);
                        let before = join_lines(
                            &front_lines[span.key_line..=span.value_end],
                        );
                        front_lines[span.key_line].content = new_key_content;
                        front_lines[span.key_line].ending =
                            if key_ending.is_empty() {
                                newline.to_owned()
                            } else {
                                key_ending
                            };
                        let remove = span.value_end - span.key_line;
                        for _ in 0..remove {
                            front_lines.remove(span.key_line + 1);
                        }
                        for (offset, item) in rendered.iter().enumerate() {
                            front_lines.insert(
                                span.key_line + 1 + offset,
                                SplitLine {
                                    content: format!("  - {item}"),
                                    ending: newline.to_owned(),
                                },
                            );
                        }
                        let after = join_lines(
                            &front_lines[span.key_line
                                ..=span.key_line + rendered.len()],
                        );
                        if before != after
                            && !changed.iter().any(|name| name == key)
                        {
                            changed.push(key.clone());
                        }
                    } else {
                        let scalar =
                            rendered.first().cloned().unwrap_or_default();
                        let key_content =
                            front_lines[span.key_line].content.clone();
                        let indent_len =
                            key_content.len() - key_content.trim_start().len();
                        let rest = &key_content[indent_len..];
                        let Some(colon) = rest.find(':') else {
                            return Err(refusal(
                                RecordUpdateErrorCode::MalformedFrontmatter,
                                Some(key),
                                format!("key {key:?} line has no colon"),
                                original,
                            ));
                        };
                        let after_colon = &rest[colon + 1..];
                        let (_, comment) = split_value_and_comment(after_colon);
                        let gap = gap_after_colon(after_colon);
                        let new_content =
                            format!("{}{gap}{scalar}{comment}", span.prefix);
                        if front_lines[span.key_line].content != new_content {
                            front_lines[span.key_line].content = new_content;
                            if front_lines[span.key_line].ending.is_empty() {
                                front_lines[span.key_line].ending =
                                    newline.to_owned();
                            }
                            changed.push(key.clone());
                        }
                        if span.value_end > span.key_line {
                            front_lines
                                .drain(span.key_line + 1..=span.value_end);
                            if !changed.iter().any(|name| name == key) {
                                changed.push(key.clone());
                            }
                        }
                    }
                } else {
                    let insert_at =
                        find_insert_position(front_lines, record_type, key);
                    if is_list {
                        front_lines.insert(
                            insert_at,
                            SplitLine {
                                content: format!("{key}:"),
                                ending: newline.to_owned(),
                            },
                        );
                        for (offset, item) in rendered.iter().enumerate() {
                            front_lines.insert(
                                insert_at + 1 + offset,
                                SplitLine {
                                    content: format!("  - {item}"),
                                    ending: newline.to_owned(),
                                },
                            );
                        }
                    } else {
                        let scalar =
                            rendered.first().cloned().unwrap_or_default();
                        front_lines.insert(
                            insert_at,
                            SplitLine {
                                content: format!("{key}: {scalar}"),
                                ending: newline.to_owned(),
                            },
                        );
                    }
                    changed.push(key.clone());
                }
            }
            ValidatedFieldInput::Absent => continue,
        }
    }
    Ok(changed)
}

fn find_insert_position(
    front_lines: &[SplitLine],
    record_type: &RecordType,
    key: &str,
) -> usize {
    let Some(target) = schema_position(record_type, key) else {
        return front_lines.len();
    };
    let (spans, _) = collect_key_spans(front_lines);
    let mut predecessor_end: Option<usize> = None;
    for (index, field) in record_type.fields.iter().enumerate() {
        if index >= target {
            break;
        }
        if let Some(span) = spans.iter().find(|span| span.name == field.key) {
            predecessor_end = Some(span.value_end);
        }
    }
    if let Some(end) = predecessor_end {
        return end + 1;
    }
    if let Some(created) = spans.iter().find(|span| span.name == "created") {
        return created.key_line;
    }
    front_lines.len()
}

fn append_notes_to_body(
    body_lines: &mut Vec<SplitLine>,
    record_type: &RecordType,
    notes: &[RecordNoteInput],
    note_date: Option<&str>,
    newline: &str,
) {
    if notes.is_empty() {
        return;
    }
    let mut ordinary: Vec<&RecordNoteInput> = Vec::new();
    let mut sections: Vec<&RecordNoteInput> = Vec::new();
    for note in notes {
        if is_section_note(note) {
            sections.push(note);
        } else {
            ordinary.push(note);
        }
    }
    if !ordinary.is_empty() {
        let rendered: Vec<String> = ordinary
            .iter()
            .flat_map(|note| {
                render_note_bullet(record_type.dated_notes, note_date, note)
            })
            .collect();
        append_bullets_under_notes(body_lines, &rendered, newline);
    }
    for section_note in sections {
        append_section_bullets(body_lines, section_note, newline);
    }
}

fn append_bullets_under_notes(
    body_lines: &mut Vec<SplitLine>,
    rendered: &[String],
    newline: &str,
) {
    if rendered.is_empty() {
        return;
    }
    let headings = parse_body_headings(body_lines);
    let notes_at = headings
        .iter()
        .find(|heading| heading.level == 2 && heading.title.trim() == "Notes");
    if let Some(notes_heading) = notes_at {
        let notes_line = notes_heading.line;
        let next_boundary = headings
            .iter()
            .filter(|heading| heading.line > notes_line && heading.level <= 2)
            .map(|heading| heading.line)
            .min()
            .unwrap_or(body_lines.len());
        if let Some(last) =
            last_nonblank_before(body_lines, notes_line + 1, next_boundary)
        {
            let insert_at = last + 1;
            for (offset, line) in rendered.iter().enumerate() {
                body_lines.insert(
                    insert_at + offset,
                    SplitLine {
                        content: line.clone(),
                        ending: newline.to_owned(),
                    },
                );
            }
        } else {
            let mut insert_at = notes_line + 1;
            let mut skipped = 0;
            while insert_at + skipped < next_boundary
                && body_lines
                    .get(insert_at + skipped)
                    .is_some_and(|line| line.content.trim().is_empty())
            {
                skipped += 1;
            }
            if skipped == 0 {
                body_lines.insert(
                    insert_at,
                    SplitLine {
                        content: String::new(),
                        ending: newline.to_owned(),
                    },
                );
                insert_at += 1;
            } else {
                insert_at += 1;
            }
            let insert_at = insert_at.min(next_boundary + skipped.min(1));
            let clamped = insert_at.min(body_lines.len());
            for (offset, line) in rendered.iter().enumerate() {
                body_lines.insert(
                    clamped + offset,
                    SplitLine {
                        content: line.clone(),
                        ending: newline.to_owned(),
                    },
                );
            }
            if next_boundary < body_lines.len()
                && !rendered.is_empty()
                && body_lines
                    .get(clamped + rendered.len())
                    .is_some_and(|line| !line.content.trim().is_empty())
            {
                body_lines.insert(
                    clamped + rendered.len(),
                    SplitLine {
                        content: String::new(),
                        ending: newline.to_owned(),
                    },
                );
            }
        }
    } else {
        if !body_lines.is_empty()
            && !body_lines
                .last()
                .is_some_and(|line| line.content.trim().is_empty())
            && !body_lines.last().is_some_and(|line| {
                line.ending.is_empty() && line.content.is_empty()
            })
        {
            if body_lines.last().is_some_and(|line| line.ending.is_empty())
                && let Some(last) = body_lines.last_mut()
            {
                last.ending = newline.to_owned();
            }
            body_lines.push(SplitLine {
                content: String::new(),
                ending: newline.to_owned(),
            });
        }
        body_lines.push(SplitLine {
            content: "## Notes".to_owned(),
            ending: newline.to_owned(),
        });
        body_lines.push(SplitLine {
            content: String::new(),
            ending: newline.to_owned(),
        });
        for line in rendered {
            body_lines.push(SplitLine {
                content: line.clone(),
                ending: newline.to_owned(),
            });
        }
    }
}

fn append_section_bullets(
    body_lines: &mut Vec<SplitLine>,
    section_note: &RecordNoteInput,
    newline: &str,
) {
    let title = format_project_section_title(&section_note.text);
    let normalized = normalize_project_section_title(&title);
    let rendered: Vec<String> = section_note
        .children
        .iter()
        .map(|child| format!("- {child}"))
        .collect();
    if rendered.is_empty() {
        return;
    }
    let headings = parse_body_headings(body_lines);
    let existing = headings.iter().find(|heading| {
        heading.level == 2
            && normalize_project_section_title(&heading.title) == normalized
    });
    if let Some(matched) = existing {
        let start = matched.line;
        let next_boundary = headings
            .iter()
            .filter(|heading| heading.line > start && heading.level <= 2)
            .map(|heading| heading.line)
            .min()
            .unwrap_or(body_lines.len());
        if let Some(last) =
            last_nonblank_before(body_lines, start + 1, next_boundary)
        {
            let insert_at = last + 1;
            for (offset, line) in rendered.iter().enumerate() {
                body_lines.insert(
                    insert_at + offset,
                    SplitLine {
                        content: line.clone(),
                        ending: newline.to_owned(),
                    },
                );
            }
        } else {
            let mut insert_at = start + 1;
            if insert_at < next_boundary
                && body_lines
                    .get(insert_at)
                    .is_some_and(|line| line.content.trim().is_empty())
            {
                insert_at += 1;
            }
            let clamped = insert_at.min(body_lines.len());
            for (offset, line) in rendered.iter().enumerate() {
                body_lines.insert(
                    clamped + offset,
                    SplitLine {
                        content: line.clone(),
                        ending: newline.to_owned(),
                    },
                );
            }
        }
    } else {
        if !body_lines.is_empty()
            && body_lines
                .last()
                .is_some_and(|line| !line.content.trim().is_empty())
            && body_lines.last().is_some_and(|line| line.ending.is_empty())
            && let Some(last) = body_lines.last_mut()
        {
            last.ending = newline.to_owned();
            body_lines.push(SplitLine {
                content: String::new(),
                ending: newline.to_owned(),
            });
        }
        body_lines.push(SplitLine {
            content: format!("## {title}"),
            ending: newline.to_owned(),
        });
        body_lines.push(SplitLine {
            content: String::new(),
            ending: newline.to_owned(),
        });
        for line in rendered {
            body_lines.push(SplitLine {
                content: line,
                ending: newline.to_owned(),
            });
        }
    }
}

/// Apply conservative field and note edits, preserving every untouched byte.
/// Only explicitly listed fields are validated; defaults never apply.
/// Returns edited bytes plus changed keys in schema order.
pub(crate) fn apply_record_update(
    request: &RecordUpdateRequest<'_>,
) -> Result<AppliedRecordUpdate, RecordUpdateError> {
    let original = request.original;
    let newline = detect_newline(original);
    let all_lines = split_preserving_ends(original);
    if all_lines.is_empty() || all_lines[0].content != "---" {
        return Err(refusal(
            RecordUpdateErrorCode::MissingFrontmatter,
            None,
            "note has no opening frontmatter delimiter",
            original,
        ));
    }
    let mut closing: Option<usize> = None;
    for (index, line) in all_lines.iter().enumerate().skip(1) {
        if line.content == "---" {
            closing = Some(index);
            break;
        }
    }
    let Some(closing_line) = closing else {
        return Err(refusal(
            RecordUpdateErrorCode::MalformedFrontmatter,
            None,
            "frontmatter has no closing delimiter",
            original,
        ));
    };
    let mut front_lines: Vec<SplitLine> = all_lines[1..closing_line].to_vec();
    let mut body_lines: Vec<SplitLine> = all_lines[closing_line + 1..].to_vec();
    let closing_ending = all_lines[closing_line].ending.clone();

    let (early_spans, early_duplicates) = collect_key_spans(&front_lines);
    let _ = early_spans;
    for input in request.fields {
        if matches!(input.state, FieldInputState::Absent) {
            continue;
        }
        if early_duplicates.iter().any(|name| name == &input.key) {
            return Err(refusal(
                RecordUpdateErrorCode::DuplicateKey,
                Some(&input.key),
                format!(
                    "duplicate frontmatter key {:?} cannot be edited safely",
                    input.key
                ),
                original,
            ));
        }
    }

    let joined = frontmatter_text(&front_lines);
    if serde_yaml::from_str::<serde_yaml::Value>(&joined).is_err()
        && !front_lines
            .iter()
            .all(|line| line.content.trim().is_empty())
    {
        return Err(refusal(
            RecordUpdateErrorCode::MalformedFrontmatter,
            None,
            "frontmatter is not valid YAML",
            original,
        ));
    }

    let mut grouped: BTreeMap<String, Vec<FieldInputState>> = BTreeMap::new();
    for input in request.fields {
        grouped
            .entry(input.key.clone())
            .or_default()
            .push(input.state.clone());
    }
    let mut edits: BTreeMap<String, ValidatedFieldInput> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for field in &request.record_type.fields {
        let Some(states) = grouped.get(&field.key) else {
            continue;
        };
        let has_empty = states
            .iter()
            .any(|state| matches!(state, FieldInputState::SuppliedEmpty));
        let mut collected: Vec<String> = Vec::new();
        for state in states {
            if let FieldInputState::SuppliedValues(values) = state {
                collected.extend(values.iter().cloned());
            }
        }
        if has_empty && !collected.is_empty() {
            return Err(refusal(
                RecordUpdateErrorCode::InvalidValue,
                Some(&field.key),
                format!(
                    "field {:?} has both empty and valued edits in one request",
                    field.key
                ),
                original,
            ));
        }
        if has_empty {
            edits.insert(field.key.clone(), ValidatedFieldInput::Empty);
            order.push(field.key.clone());
            continue;
        }
        if collected.is_empty() {
            continue;
        }
        let state = FieldInputState::SuppliedValues(collected);
        match validate_field_input(field, &state, request.today) {
            Ok(validated) => {
                edits.insert(field.key.clone(), validated);
                order.push(field.key.clone());
            }
            Err(diagnostic) => {
                return Err(refusal(
                    RecordUpdateErrorCode::InvalidValue,
                    Some(&field.key),
                    diagnostic.message.clone(),
                    original,
                ));
            }
        }
    }
    for (key, states) in &grouped {
        if field_by_key(request.record_type, key).is_some() {
            continue;
        }
        let mut saw_value = false;
        for state in states {
            match state {
                FieldInputState::Absent => {}
                FieldInputState::SuppliedEmpty => {
                    return Err(refusal(
                        RecordUpdateErrorCode::InvalidValue,
                        Some(key),
                        format!("unknown record field {key:?}"),
                        original,
                    ));
                }
                FieldInputState::SuppliedValues(_) => saw_value = true,
            }
        }
        if saw_value {
            return Err(refusal(
                RecordUpdateErrorCode::InvalidValue,
                Some(key),
                format!("unknown record field {key:?}"),
                original,
            ));
        }
    }

    let mut changed = apply_frontmatter_edits(
        &mut front_lines,
        request.record_type,
        &edits,
        &order,
        newline,
        request.today,
        original,
    )?;
    let edited_keys: Vec<String> = order.clone();

    let body_before = join_lines(&body_lines);
    append_notes_to_body(
        &mut body_lines,
        request.record_type,
        request.notes,
        request.note_date,
        newline,
    );
    let body_after = join_lines(&body_lines);
    let notes_changed = body_before != body_after;

    let warnings = collect_warnings(
        request.record_type,
        &front_lines,
        &edited_keys,
        request.today,
    );

    changed.sort_by_key(|key| {
        schema_position(request.record_type, key).unwrap_or(usize::MAX)
    });

    let mut markdown = String::new();
    markdown.push_str(&all_lines[0].content);
    markdown.push_str(&all_lines[0].ending);
    markdown.push_str(&join_lines(&front_lines));
    markdown.push_str("---");
    markdown.push_str(&closing_ending);
    if closing_ending.is_empty() {
        markdown.push_str(newline);
    }
    markdown.push_str(&join_lines(&body_lines));
    if markdown.is_empty() || !markdown.ends_with('\n') {
        if markdown.ends_with("\r") {
            markdown.push('\n');
        } else if !markdown.is_empty() && !markdown.ends_with('\n') {
            markdown.push_str(newline);
        }
    }
    let _ = notes_changed;

    Ok(AppliedRecordUpdate {
        markdown,
        changed,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::NaiveDate;
    use url::Url;

    use super::super::schema::{
        FieldInputState, FieldKind, RecordChoice, RecordField,
        RecordFieldInput, RecordScalar, RecordType, RecordValue,
    };
    use super::{
        apply_record_update, has_date_prefix, is_bare_choice_token,
        render_record, split_preserving_ends, AppliedRecordUpdate,
        RecordNoteInput, RecordRenderRequest, RecordUpdateErrorCode,
        RecordUpdateRequest,
    };

    const TODAY: NaiveDate = match NaiveDate::from_ymd_opt(2026, 11, 8) {
        Some(date) => date,
        None => panic!("test date is valid"),
    };
    const CREATED: &str = "2026-11-08T12:41:07-0500";
    const NOTE_DATE: &str = "2026-11-08";

    fn restaurant_type() -> RecordType {
        RecordType {
            id: "restaurant".to_owned(),
            dir: "eat".to_owned(),
            label: "Restaurant".to_owned(),
            emoji: Some("🍽️".to_owned()),
            parent: None,
            dated_notes: true,
            fields: vec![
                RecordField {
                    key: "status".to_owned(),
                    label: "Status".to_owned(),
                    kind: FieldKind::Choices(vec![
                        RecordChoice {
                            value: "to_try".to_owned(),
                            label: "🔖 To try".to_owned(),
                        },
                        RecordChoice {
                            value: "liked".to_owned(),
                            label: "💚 Liked".to_owned(),
                        },
                        RecordChoice {
                            value: "not_liked".to_owned(),
                            label: "🚫 Not liked".to_owned(),
                        },
                    ]),
                    required: true,
                    list: false,
                    default: None,
                    suggest: false,
                    min: None,
                    max: None,
                },
                RecordField {
                    key: "category".to_owned(),
                    label: "Category".to_owned(),
                    kind: FieldKind::Text,
                    required: true,
                    list: false,
                    default: None,
                    suggest: true,
                    min: None,
                    max: None,
                },
                RecordField {
                    key: "location".to_owned(),
                    label: "Location".to_owned(),
                    kind: FieldKind::Text,
                    required: true,
                    list: false,
                    default: None,
                    suggest: true,
                    min: None,
                    max: None,
                },
                RecordField {
                    key: "companions".to_owned(),
                    label: "Companions".to_owned(),
                    kind: FieldKind::Link,
                    required: false,
                    list: true,
                    default: None,
                    suggest: false,
                    min: None,
                    max: None,
                },
                RecordField {
                    key: "visited".to_owned(),
                    label: "Visited".to_owned(),
                    kind: FieldKind::Date,
                    required: false,
                    list: false,
                    default: None,
                    suggest: false,
                    min: None,
                    max: None,
                },
                RecordField {
                    key: "url".to_owned(),
                    label: "Url".to_owned(),
                    kind: FieldKind::Url,
                    required: false,
                    list: false,
                    default: None,
                    suggest: false,
                    min: None,
                    max: None,
                },
            ],
            legacy: vec!["lid".to_owned(), "source_*".to_owned()],
        }
    }

    fn podcast_type() -> RecordType {
        RecordType {
            id: "podcast".to_owned(),
            dir: "podcasts".to_owned(),
            label: "Podcast".to_owned(),
            emoji: Some("🎙️".to_owned()),
            parent: None,
            dated_notes: false,
            fields: vec![
                RecordField {
                    key: "status".to_owned(),
                    label: "Status".to_owned(),
                    kind: FieldKind::Choices(vec![
                        RecordChoice {
                            value: "to_try".to_owned(),
                            label: "🔖 To try".to_owned(),
                        },
                        RecordChoice {
                            value: "subscribed".to_owned(),
                            label: "🎧 Subscribed".to_owned(),
                        },
                        RecordChoice {
                            value: "unsubscribed".to_owned(),
                            label: "🚫 Unsubscribed".to_owned(),
                        },
                    ]),
                    required: true,
                    list: false,
                    default: None,
                    suggest: false,
                    min: None,
                    max: None,
                },
                RecordField {
                    key: "category".to_owned(),
                    label: "Category".to_owned(),
                    kind: FieldKind::Text,
                    required: true,
                    list: false,
                    default: None,
                    suggest: true,
                    min: None,
                    max: None,
                },
                RecordField {
                    key: "url".to_owned(),
                    label: "Url".to_owned(),
                    kind: FieldKind::Url,
                    required: false,
                    list: false,
                    default: None,
                    suggest: false,
                    min: None,
                    max: None,
                },
            ],
            legacy: vec!["zorg_id".to_owned(), "source_*".to_owned()],
        }
    }

    fn values(
        pairs: Vec<(&str, RecordValue)>,
    ) -> BTreeMap<String, RecordValue> {
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect()
    }

    fn text_value(text: &str) -> RecordValue {
        RecordValue::Scalar(RecordScalar::Text(text.to_owned()))
    }

    fn choice_value(choice: &str) -> RecordValue {
        RecordValue::Scalar(RecordScalar::Choice(choice.to_owned()))
    }

    fn link_value(link: &str) -> RecordValue {
        RecordValue::Scalar(RecordScalar::Link(link.to_owned()))
    }

    fn link_list_value(links: &[&str]) -> RecordValue {
        RecordValue::List(
            links
                .iter()
                .map(|link| RecordScalar::Link((*link).to_owned()))
                .collect(),
        )
    }

    fn date_value(year: i32, month: u32, day: u32) -> RecordValue {
        RecordValue::Scalar(RecordScalar::Date(
            super::super::schema::RecordDate::Resolved(
                NaiveDate::from_ymd_opt(year, month, day).expect("valid date"),
            ),
        ))
    }

    fn number_value(number: f64) -> RecordValue {
        RecordValue::Scalar(RecordScalar::Number(number))
    }

    fn url_value(url: &str) -> RecordValue {
        RecordValue::Scalar(RecordScalar::Url(
            Url::parse(url).expect("valid test URL"),
        ))
    }

    fn field_input(key: &str, values: &[&str]) -> RecordFieldInput {
        RecordFieldInput {
            key: key.to_owned(),
            state: FieldInputState::SuppliedValues(
                values.iter().map(|value| (*value).to_owned()).collect(),
            ),
        }
    }

    fn empty_input(key: &str) -> RecordFieldInput {
        RecordFieldInput {
            key: key.to_owned(),
            state: FieldInputState::SuppliedEmpty,
        }
    }

    fn roundtrip_frontmatter(markdown: &str) -> BTreeMap<String, String> {
        let (front, _) =
            super::super::super::highlights_ref::split_frontmatter(markdown)
                .expect("new note has frontmatter");
        let parsed =
            crate::native::ref_library::ParsedFrontmatter::parse(&front);
        assert!(
            parsed.invalid_yaml.is_none(),
            "new frontmatter parses: {:?}",
            parsed.invalid_yaml
        );
        let mut flattened = BTreeMap::new();
        for key in [
            "parent",
            "type",
            "title",
            "status",
            "category",
            "location",
            "companions",
            "visited",
            "url",
            "created",
        ] {
            for value in parsed.get_all(key) {
                flattened.insert(format!("{key}#{value}"), value);
            }
        }
        flattened
    }

    #[test]
    fn bare_choice_tokens_preserve_string_identity() {
        assert!(is_bare_choice_token("liked"));
        assert!(is_bare_choice_token("to_try"));
        assert!(!is_bare_choice_token("false"));
        assert!(!is_bare_choice_token("true"));
        assert!(!is_bare_choice_token("null"));
        assert!(!is_bare_choice_token("yes"));
        assert!(!is_bare_choice_token("0"));
        assert!(!is_bare_choice_token("2026-11-08"));
        assert!(!is_bare_choice_token(""));
        assert!(!has_date_prefix("plain note"));
        assert!(has_date_prefix("2026-11-08: hello"));
    }

    #[test]
    fn canonical_create_omits_legacy_null_and_empty() {
        let record_type = restaurant_type();
        let request = RecordRenderRequest {
            record_type: &record_type,
            title: "Ramen Nagi",
            parent: Some("[[eat]]"),
            created: CREATED,
            values: &values(vec![
                ("status", choice_value("liked")),
                ("category", text_value("Ramen")),
                ("location", text_value("Clark")),
                ("companions", link_list_value(&["[[kelly]]"])),
                (
                    "visited",
                    RecordValue::Scalar(RecordScalar::Date(
                        super::super::schema::RecordDate::Resolved(TODAY),
                    )),
                ),
            ]),
            notes: &[RecordNoteInput::bullet(
                "Spicy tuna was great; Kelly loved the salmon.",
            )],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        };
        let rendered = render_record(&request);
        let expected_frontmatter = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"Ramen Nagi\"\nstatus: liked\ncategory: \"Ramen\"\nlocation: \"Clark\"\ncompanions:\n  - \"[[kelly]]\"\nvisited: 2026-11-08\ncreated: 2026-11-08T12:41:07-0500\n---\n";
        assert!(
            rendered.markdown.starts_with(expected_frontmatter),
            "canonical order and quoting:\n{}",
            rendered.markdown
        );
        assert!(
            !rendered.markdown.contains("lid"),
            "legacy never written:\n{}",
            rendered.markdown
        );
        assert!(
            !rendered.markdown.contains("null"),
            "null never written:\n{}",
            rendered.markdown
        );
        assert!(
            !rendered.markdown.contains("url:"),
            "empty optional omitted:\n{}",
            rendered.markdown
        );
        assert!(
            rendered.markdown.contains("# Ramen Nagi\n\n## Notes\n"),
            "H1 and Notes always present:\n{}",
            rendered.markdown
        );
        assert!(
            rendered
                .markdown
                .contains("- 2026-11-08: Spicy tuna was great;"),
            "dated note:\n{}",
            rendered.markdown
        );
        assert!(
            rendered.markdown.ends_with(&rendered.body),
            "markdown ends with body:\n{}",
            rendered.markdown
        );
        let flattened = roundtrip_frontmatter(&rendered.markdown);
        assert!(flattened.contains_key("status#liked"));
        assert!(flattened.contains_key("category#Ramen"));
        let yaml: serde_yaml::Value = serde_yaml::from_str(
            rendered.markdown.split("---").nth(1).unwrap_or(""),
        )
        .expect("frontmatter is valid YAML");
        assert_eq!(
            yaml.get(serde_yaml::Value::String("status".to_owned())),
            Some(&serde_yaml::Value::String("liked".to_owned()))
        );
    }

    #[test]
    fn quoting_covers_colon_hash_brackets_unicode_and_numbers() {
        let record_type = restaurant_type();
        let request = RecordRenderRequest {
            record_type: &record_type,
            title: "Café \"Quotes\": #1 [Best] 0",
            parent: Some("[[eat]]"),
            created: CREATED,
            values: &values(vec![
                ("status", choice_value("liked")),
                ("category", text_value("Coffee: \"dark\" #bold [x]")),
                ("location", text_value("München")),
                (
                    "companions",
                    link_list_value(&["[[nj_towns#^z-241117-0b|cranford]]"]),
                ),
            ]),
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        };
        let rendered = render_record(&request);
        assert!(
            rendered.markdown.contains("title: \"Café"),
            "unicode title quoted:\n{}",
            rendered.markdown
        );
        assert!(
            rendered.markdown.contains("category: \"Coffee:"),
            "colon and hash quoted:\n{}",
            rendered.markdown
        );
        assert!(
            rendered
                .markdown
                .contains("\"[[nj_towns#^z-241117-0b|cranford]]\""),
            "block link quoted:\n{}",
            rendered.markdown
        );
        let yaml: serde_yaml::Value = serde_yaml::from_str(
            rendered.markdown.split("---").nth(1).unwrap_or(""),
        )
        .expect("quoted YAML parses");
        let mapping = yaml.as_mapping().expect("mapping");
        assert_eq!(
            mapping.get(serde_yaml::Value::String("category".to_owned())),
            Some(&serde_yaml::Value::String(
                "Coffee: \"dark\" #bold [x]".to_owned()
            ))
        );
    }

    #[test]
    fn false_zero_and_numeric_titles_stay_strings() {
        let record_type = restaurant_type();
        let request = RecordRenderRequest {
            record_type: &record_type,
            title: "0",
            parent: Some("[[eat]]"),
            created: CREATED,
            values: &values(vec![
                ("status", choice_value("liked")),
                ("category", text_value("false")),
                ("location", text_value("0")),
            ]),
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        };
        let rendered = render_record(&request);
        assert!(
            rendered.markdown.contains("title: \"0\""),
            "numeric title quoted:\n{}",
            rendered.markdown
        );
        assert!(
            rendered.markdown.contains("category: \"false\""),
            "false text stays string:\n{}",
            rendered.markdown
        );
        assert!(
            rendered.markdown.contains("location: \"0\""),
            "zero text stays string:\n{}",
            rendered.markdown
        );
        let parsed = crate::native::ref_library::ParsedFrontmatter::parse(
            &super::super::super::highlights_ref::split_frontmatter(
                &rendered.markdown,
            )
            .expect("frontmatter")
            .0,
        );
        assert_eq!(parsed.get_str("category"), Some("false"));
        assert_eq!(parsed.get_str("location"), Some("0"));

        let mut number_type = restaurant_type();
        if let Some(field) = number_type
            .fields
            .iter_mut()
            .find(|field| field.key == "location")
        {
            field.kind = FieldKind::Number;
            field.required = false;
        }
        let number_request = RecordRenderRequest {
            record_type: &number_type,
            title: "Numbers",
            parent: Some("[[eat]]"),
            created: CREATED,
            values: &values(vec![
                ("status", choice_value("liked")),
                ("category", text_value("x")),
                ("location", number_value(0.0)),
            ]),
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        };
        let number_rendered = render_record(&number_request);
        assert!(
            number_rendered.markdown.contains("location: 0\n"),
            "numeric zero stays bare:\n{}",
            number_rendered.markdown
        );
    }

    #[test]
    fn choice_tokens_that_look_like_yaml_scalars_stay_quoted() {
        assert!(!is_bare_choice_token("false"));
        let mut record_type = restaurant_type();
        if let Some(field) = record_type
            .fields
            .iter_mut()
            .find(|field| field.key == "status")
        {
            field.kind = FieldKind::Choices(vec![
                RecordChoice {
                    value: "false".to_owned(),
                    label: "False".to_owned(),
                },
                RecordChoice {
                    value: "liked".to_owned(),
                    label: "Liked".to_owned(),
                },
            ]);
        }
        let request = RecordRenderRequest {
            record_type: &record_type,
            title: "Quoted choice",
            parent: Some("[[eat]]"),
            created: CREATED,
            values: &values(vec![
                ("status", choice_value("false")),
                ("category", text_value("Coffee")),
                ("location", text_value("Clark")),
            ]),
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        };
        let rendered = render_record(&request);
        assert!(
            rendered.markdown.contains("status: \"false\"\n"),
            "YAML-looking choice quoted:\n{}",
            rendered.markdown
        );
    }

    #[test]
    fn dated_notes_preserve_nesting_and_sections_use_helpers() {
        let record_type = restaurant_type();
        let request = RecordRenderRequest {
            record_type: &record_type,
            title: "Nested",
            parent: Some("[[eat]]"),
            created: CREATED,
            values: &values(vec![
                ("status", choice_value("liked")),
                ("category", text_value("Coffee")),
                ("location", text_value("Clark")),
            ]),
            notes: &[
                RecordNoteInput {
                    text: "First visit".to_owned(),
                    children: vec![
                        "oat latte".to_owned(),
                        "croissant".to_owned(),
                    ],
                },
                RecordNoteInput::section(
                    "FUTURE WORK",
                    vec!["try decaf".to_owned()],
                ),
            ],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        };
        let rendered = render_record(&request);
        assert!(
            rendered.markdown.contains(
                "- 2026-11-08: First visit\n  - oat latte\n  - croissant\n"
            ),
            "nested bullets preserved:\n{}",
            rendered.markdown
        );
        assert!(
            rendered
                .markdown
                .contains("## Future Work\n\n- try decaf\n"),
            "ALL-CAPS section title-cased:\n{}",
            rendered.markdown
        );
    }

    #[test]
    fn update_edits_only_target_span_and_preserves_rest() {
        let original = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"Ahrres\"\nstatus: liked # house favorite\ncategory: \"Coffee\"\nlocation: \"Westfield\" # county seat\nlid: \"ahrres\"\ncreated: 2026-07-04T06:03:31-04:00\n---\n\n# Ahrres\n\n## Notes\n";
        let record_type = restaurant_type();
        let applied: AppliedRecordUpdate =
            apply_record_update(&RecordUpdateRequest {
                record_type: &record_type,
                original,
                fields: &[field_input("status", &["not_liked"])],
                notes: &[],
                note_date: Some(NOTE_DATE),
                today: TODAY,
            })
            .expect("scalar edit succeeds");
        assert_eq!(applied.changed, vec!["status".to_owned()]);
        assert!(
            applied
                .markdown
                .contains("status: not_liked # house favorite\n"),
            "comment preserved:\n{}",
            applied.markdown
        );
        assert!(
            applied
                .markdown
                .contains("location: \"Westfield\" # county seat\n"),
            "untouched line preserved:\n{}",
            applied.markdown
        );
        assert!(
            applied.markdown.contains("lid: \"ahrres\"\n"),
            "legacy preserved:\n{}",
            applied.markdown
        );
        assert!(
            applied
                .markdown
                .contains("created: 2026-07-04T06:03:31-04:00\n"),
            "created preserved:\n{}",
            applied.markdown
        );
    }

    #[test]
    fn update_inserts_missing_key_in_schema_order() {
        let original = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"New\"\nstatus: liked\ncreated: 2026-11-08T12:41:07-0500\n---\n\n# New\n\n## Notes\n";
        let record_type = restaurant_type();
        let applied = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original,
            fields: &[
                field_input("location", &["Clark"]),
                field_input("category", &["Ramen"]),
            ],
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect("inserts succeed");
        assert_eq!(
            applied.changed,
            vec!["category".to_owned(), "location".to_owned()]
        );
        let status_at =
            applied.markdown.find("status: liked\n").expect("status");
        let category_at = applied
            .markdown
            .find("category: \"Ramen\"\n")
            .expect("category");
        let location_at = applied
            .markdown
            .find("location: \"Clark\"\n")
            .expect("location");
        let created_at = applied.markdown.find("created: ").expect("created");
        assert!(
            status_at < category_at
                && category_at < location_at
                && location_at < created_at,
            "schema order:\n{}",
            applied.markdown
        );
    }

    #[test]
    fn update_replaces_list_and_removes_optional_empty() {
        let original = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"T\"\nstatus: liked\ncategory: \"Coffee\"\nlocation: \"Clark\"\ncompanions:\n  - \"[[kelly]]\"\ncreated: 2026-11-08T12:41:07-0500\n---\n\n# T\n\n## Notes\n";
        let record_type = restaurant_type();
        let replaced = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original,
            fields: &[RecordFieldInput {
                key: "companions".to_owned(),
                state: FieldInputState::SuppliedValues(vec![
                    "[[kelly]], [[bob]]".to_owned(),
                ]),
            }],
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect("list replace succeeds");
        assert!(
            replaced
                .markdown
                .contains("companions:\n  - \"[[kelly]]\"\n  - \"[[bob]]\"\n"),
            "list replaced:\n{}",
            replaced.markdown
        );
        let removed = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original: &replaced.markdown,
            fields: &[empty_input("companions")],
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect("optional clear removes key");
        assert!(
            !removed.markdown.contains("companions"),
            "optional empty removes:\n{}",
            removed.markdown
        );
        assert_eq!(removed.changed, vec!["companions".to_owned()]);
    }

    #[test]
    fn update_required_empty_is_refused_with_unchanged_bytes() {
        let original = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"T\"\nstatus: liked\ncategory: \"Coffee\"\nlocation: \"Clark\"\ncreated: 2026-11-08T12:41:07-0500\n---\n\n# T\n\n## Notes\n";
        let record_type = restaurant_type();
        let error = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original,
            fields: &[empty_input("status")],
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect_err("required empty refuses");
        assert_eq!(error.code, RecordUpdateErrorCode::RequiredEmpty);
        assert_eq!(error.unchanged, original);
    }

    #[test]
    fn update_refuses_duplicate_flow_anchor_and_malformed() {
        let record_type = restaurant_type();
        let duplicate = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"T\"\nstatus: liked\nstatus: not_liked\ncategory: \"Coffee\"\nlocation: \"Clark\"\ncreated: 2026-11-08T12:41:07-0500\n---\n\n# T\n\n## Notes\n";
        let error = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original: duplicate,
            fields: &[field_input("status", &["liked"])],
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect_err("duplicate refuses");
        assert_eq!(error.code, RecordUpdateErrorCode::DuplicateKey);
        assert_eq!(error.unchanged, duplicate);

        let flow = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"T\"\nstatus: {liked: true}\ncategory: \"Coffee\"\nlocation: \"Clark\"\ncreated: 2026-11-08T12:41:07-0500\n---\n\n# T\n\n## Notes\n";
        let error = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original: flow,
            fields: &[field_input("status", &["liked"])],
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect_err("flow refuses");
        assert_eq!(error.code, RecordUpdateErrorCode::UnsupportedYaml);
        assert_eq!(error.unchanged, flow);

        let anchor = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"T\"\nstatus: &anchor liked\ncategory: \"Coffee\"\nlocation: \"Clark\"\ncreated: 2026-11-08T12:41:07-0500\n---\n\n# T\n\n## Notes\n";
        let error = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original: anchor,
            fields: &[field_input("status", &["liked"])],
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect_err("anchor refuses");
        assert_eq!(error.code, RecordUpdateErrorCode::UnsupportedYaml);
        assert_eq!(error.unchanged, anchor);

        let malformed = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"T\"\nstatus: \"unterminated\ncategory: \"Coffee\"\n";
        let error = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original: malformed,
            fields: &[field_input("status", &["liked"])],
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect_err("malformed refuses");
        assert_eq!(error.code, RecordUpdateErrorCode::MalformedFrontmatter);
        assert_eq!(error.unchanged, malformed);
    }

    #[test]
    fn update_preserves_quoted_keys_and_multiline_spans() {
        let original = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"T\"\n\"status\": liked\ncategory: \"long\n  continued\"\nlocation: \"Clark\"\ncreated: 2026-11-08T12:41:07-0500\n---\n\n# T\n\n## Notes\n";
        let record_type = restaurant_type();
        let applied = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original,
            fields: &[field_input("location", &["Westfield"])],
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect("quoted and multiline preserved");
        assert!(
            applied.markdown.contains("\"status\": liked\n"),
            "quoted key preserved:\n{}",
            applied.markdown
        );
        assert!(
            applied
                .markdown
                .contains("category: \"long\n  continued\"\n"),
            "multiline span preserved:\n{}",
            applied.markdown
        );
        assert!(
            applied.markdown.contains("location: \"Westfield\"\n"),
            "target replaced:\n{}",
            applied.markdown
        );
    }

    #[test]
    fn real_note_goldens_keep_source_fences_and_legacy() {
        let ahrres = include_str!(
            "../../../tests/fixtures/records/source/eat/ahrres.md"
        );
        let talk_python = include_str!(
            "../../../tests/fixtures/records/source/podcasts/talk_python.md"
        );
        let restaurant = restaurant_type();
        let podcast = podcast_type();

        let ahrres_applied = apply_record_update(&RecordUpdateRequest {
            record_type: &restaurant,
            original: ahrres,
            fields: &[field_input("location", &["Westfield"])],
            notes: &[RecordNoteInput::bullet("Revisit was great")],
            note_date: Some("2026-11-08"),
            today: TODAY,
        })
        .expect("ahrres update succeeds");
        assert!(
            ahrres_applied.markdown.contains("lid: \"ahrres\"\n"),
            "ahrres legacy lid survives:\n{}",
            ahrres_applied.markdown
        );
        assert!(
            ahrres_applied
                .markdown
                .contains("source_group: union_county\n"),
            "ahrres legacy source_group survives:\n{}",
            ahrres_applied.markdown
        );
        assert!(
            ahrres_applied.markdown.contains("## Source\n\n```text\n"),
            "ahrres Source fence survives:\n{}",
            ahrres_applied.markdown
        );
        assert!(
            ahrres_applied
                .markdown
                .contains("- 2026-11-08: Revisit was great\n"),
            "ahrres note appended:\n{}",
            ahrres_applied.markdown
        );
        assert!(
            ahrres_applied
                .markdown
                .contains("- 250828 240410#83 Ahrres"),
            "ahrres fenced source line untouched:\n{}",
            ahrres_applied.markdown
        );

        let talk_applied = apply_record_update(&RecordUpdateRequest {
            record_type: &podcast,
            original: talk_python,
            fields: &[field_input("category", &["python"])],
            notes: &[RecordNoteInput::bullet("Listened to the async episode")],
            note_date: None,
            today: TODAY,
        })
        .expect("talk_python update succeeds");
        assert!(
            talk_applied.markdown.contains("zorg_id: talk_python\n"),
            "talk_python legacy survives:\n{}",
            talk_applied.markdown
        );
        assert!(
            talk_applied.markdown.contains("## Source\n\n```text\n"),
            "talk_python Source fence survives:\n{}",
            talk_applied.markdown
        );
        assert!(
            talk_applied
                .markdown
                .contains("- Listened to the async episode\n"),
            "undated podcast note has no date:\n{}",
            talk_applied.markdown
        );
    }

    #[test]
    fn canonical_create_roundtrips_through_existing_reader() {
        let record_type = restaurant_type();
        let request = RecordRenderRequest {
            record_type: &record_type,
            title: "Round Trip",
            parent: Some("[[eat]]"),
            created: CREATED,
            values: &values(vec![
                ("status", choice_value("liked")),
                ("category", text_value("Coffee")),
                ("location", text_value("Westfield")),
                ("companions", link_list_value(&["[[kelly]]"])),
                ("visited", date_value(2026, 11, 8)),
                ("url", url_value("https://example.com/menu")),
            ]),
            notes: &[RecordNoteInput::bullet("hello")],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        };
        let rendered = render_record(&request);
        let (front, body) =
            super::super::super::highlights_ref::split_frontmatter(
                &rendered.markdown,
            )
            .expect("splits");
        assert!(
            body.contains("# Round Trip"),
            "body survives split:\n{body}"
        );
        let parsed =
            crate::native::ref_library::ParsedFrontmatter::parse(&front);
        assert_eq!(parsed.get_str("status"), Some("liked"));
        assert_eq!(parsed.get_str("category"), Some("Coffee"));
        assert_eq!(parsed.get_all("companions"), vec!["[[kelly]]".to_owned()]);
        assert_eq!(parsed.get_str("visited"), Some("2026-11-08"));
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&front.join("\n")).expect("valid YAML");
        assert_eq!(
            yaml.get(serde_yaml::Value::String("status".to_owned())),
            Some(&serde_yaml::Value::String("liked".to_owned()))
        );
        assert_eq!(
            yaml.get(serde_yaml::Value::String("visited".to_owned())),
            Some(&serde_yaml::Value::String("2026-11-08".to_owned()))
        );
    }

    #[test]
    fn body_appends_ignore_fenced_headings_and_handle_crlf() {
        let record_type = restaurant_type();
        let original = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"T\"\nstatus: liked\ncategory: \"Coffee\"\nlocation: \"Clark\"\ncreated: 2026-11-08T12:41:07-0500\n---\n\n# T\n\n## Notes\n\n- old\n\n```text\n## Notes\n```\n\n## Source\n";
        let applied = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original,
            fields: &[],
            notes: &[RecordNoteInput::bullet("new")],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect("fenced headings ignored");
        let notes_pos = applied
            .markdown
            .find("- 2026-11-08: new\n")
            .expect("new note");
        let source_pos = applied.markdown.find("## Source\n").expect("source");
        let fenced_pos =
            applied.markdown.find("```text\n## Notes").expect("fence");
        assert!(
            notes_pos > fenced_pos && notes_pos < source_pos,
            "appended before real Source, after fence:\n{}",
            applied.markdown
        );

        let crlf_original = "---\r\nparent: \"[[eat]]\"\r\ntype: \"[[restaurant]]\"\r\ntitle: \"T\"\r\nstatus: liked\r\ncategory: \"Coffee\"\r\nlocation: \"Clark\"\r\ncreated: 2026-11-08T12:41:07-0500\r\n---\r\n\r\n# T\r\n\r\n## Notes\r\n";
        let crlf_applied = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original: crlf_original,
            fields: &[field_input("location", &["Westfield"])],
            notes: &[RecordNoteInput::bullet("crlf note")],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect("CRLF preserved");
        assert!(
            crlf_applied.markdown.contains("\r\n"),
            "CRLF preserved:\n{}",
            crlf_applied.markdown.replace('\r', "\\r")
        );
        assert!(
            !crlf_applied.markdown.contains("location: \"Clark\""),
            "CRLF target replaced"
        );
        assert!(
            crlf_applied
                .markdown
                .contains("location: \"Westfield\"\r\n"),
            "CRLF insertion uses CRLF:\n{}",
            crlf_applied.markdown.replace('\r', "\\r")
        );

        let no_newline = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"T\"\nstatus: liked\ncategory: \"Coffee\"\nlocation: \"Clark\"\ncreated: 2026-11-08T12:41:07-0500\n---\n\n# T\n\n## Notes";
        let no_newline_applied = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original: no_newline,
            fields: &[],
            notes: &[RecordNoteInput::bullet("tail")],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect("no-final-newline appends");
        assert!(
            no_newline_applied.markdown.ends_with('\n'),
            "appended file ends with newline"
        );
        assert!(
            no_newline_applied.markdown.contains("- 2026-11-08: tail\n"),
            "tail note:\n{}",
            no_newline_applied.markdown
        );
    }

    #[test]
    fn named_sections_merge_instead_of_duplicating() {
        let record_type = restaurant_type();
        let original = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"T\"\nstatus: liked\ncategory: \"Coffee\"\nlocation: \"Clark\"\ncreated: 2026-11-08T12:41:07-0500\n---\n\n# T\n\n## Notes\n\n- old\n\n## Future Work\n\n- existing\n";
        let applied = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original,
            fields: &[],
            notes: &[RecordNoteInput::section(
                "FUTURE WORK",
                vec!["new item".to_owned()],
            )],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect("section merges");
        assert_eq!(
            applied.markdown.matches("## Future Work").count(),
            1,
            "no duplicate section:\n{}",
            applied.markdown
        );
        assert!(
            applied.markdown.contains("- existing\n- new item\n"),
            "section children appended:\n{}",
            applied.markdown
        );
    }

    #[test]
    fn unrelated_defects_are_warnings_not_refusals() {
        let record_type = restaurant_type();
        let original = "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: \"T\"\nstatus: bogus\ncategory: \"Coffee\"\nlocation: \"Clark\"\ncreated: 2026-11-08T12:41:07-0500\n---\n\n# T\n\n## Notes\n";
        let applied = apply_record_update(&RecordUpdateRequest {
            record_type: &record_type,
            original,
            fields: &[field_input("location", &["Westfield"])],
            notes: &[],
            note_date: Some(NOTE_DATE),
            today: TODAY,
        })
        .expect("unrelated defect warns");
        assert_eq!(applied.changed, vec!["location".to_owned()]);
        assert!(
            applied
                .warnings
                .iter()
                .any(|warning| warning.field.as_deref() == Some("status")),
            "status warning:\n{applied:?}"
        );
        assert!(
            applied.markdown.contains("status: bogus\n"),
            "untouched defect preserved:\n{}",
            applied.markdown
        );
    }

    #[test]
    fn split_preserving_ends_handles_crlf_and_missing_newline() {
        let lines = split_preserving_ends("a\r\nb\r\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].ending, "\r\n");
        let lines = split_preserving_ends("a\nb");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].ending, "");
    }
}
