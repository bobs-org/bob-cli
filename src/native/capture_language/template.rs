//! Pure framing, validation, and expansion for `@@template:` capture drafts.
//!
//! This module only reads the supplied text. It deliberately knows nothing
//! about the vault or capture planning so `bob capture -E` can use it as a
//! cheap, deterministic preview.

use super::draft::split_physical_lines;
use super::model::{tokenize_with_spans, RawLine};
use serde::Serialize;
use std::collections::BTreeSet;
use std::ops::Range;

const MAX_ROWS: usize = 1_000;
const MAX_EXPANDED_BYTES: usize = 1024 * 1024;
const HEADER: &str = "@@template:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateIssue {
    code: &'static str,
    message: String,
    range: Range<usize>,
}

impl TemplateIssue {
    pub(crate) fn code(&self) -> &'static str {
        self.code
    }

    pub(crate) fn message(&self) -> String {
        self.message.clone()
    }

    pub(crate) fn range(&self) -> Range<usize> {
        self.range.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TemplateSummary {
    pub(crate) line: usize,
    pub(crate) field_count: usize,
    pub(crate) row_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplatePlaceholder {
    pub(crate) field: usize,
    pub(crate) range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateField {
    pub(crate) field: usize,
    pub(crate) value: String,
    pub(crate) range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateRow {
    pub(crate) row: usize,
    pub(crate) line: usize,
    pub(crate) range: Range<usize>,
    pub(crate) fields: Vec<TemplateField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateBlock {
    pub(crate) range: Range<usize>,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    lines: Vec<(usize, Range<usize>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateAnalysis {
    pub(crate) header: Option<(Range<usize>, usize)>,
    pub(crate) block: Option<TemplateBlock>,
    pub(crate) placeholders: Vec<TemplatePlaceholder>,
    pub(crate) escapes: Vec<Range<usize>>,
    pub(crate) field_count: usize,
    pub(crate) rows: Vec<TemplateRow>,
    pub(crate) issues: Vec<TemplateIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateInstance {
    pub(crate) row: usize,
    pub(crate) line: usize,
    pub(crate) template_line_start: usize,
    pub(crate) row_range: Range<usize>,
    pub(crate) expanded_range: Range<usize>,
    pub(crate) text: String,
    pub(crate) source_map: Vec<TemplateSourceSegment>,
}

/// One expansion interval and the typed source interval it came from.
/// `field` is set for inserted row values; literal template text (including
/// escaped braces and line endings) maps directly to the template source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateSourceSegment {
    pub(crate) expanded: Range<usize>,
    pub(crate) source: Range<usize>,
    pub(crate) field: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateExpansion {
    pub(crate) text: String,
    pub(crate) instances: Vec<TemplateInstance>,
    pub(crate) summary: TemplateSummary,
}

/// Return a same-length copy suitable for the editor parser. Header and row
/// lines are hidden, while placeholders and brace escapes retain their byte
/// offsets as harmless ASCII text.
pub(crate) fn masked(raw: &str) -> String {
    let analysis = analyze(raw);
    let lines = split_physical_lines(raw);
    let mut bytes = raw.as_bytes().to_vec();

    if let Some((_, line_number)) = analysis.header.as_ref() {
        if let Some(line) = lines.get(line_number.saturating_sub(1)) {
            bytes[line.start..line.end].fill(b' ');
        }
    }
    for row in &analysis.rows {
        if let Some(line) = lines.get(row.line.saturating_sub(1)) {
            bytes[line.start..line.end].fill(b' ');
        }
    }

    let mut masked_ranges = analysis
        .placeholders
        .iter()
        .map(|placeholder| placeholder.range.clone())
        .chain(analysis.escapes.iter().cloned())
        .chain(
            analysis
                .issues
                .iter()
                .filter(|issue| issue.code == "template_invalid_placeholder")
                .map(|issue| issue.range.clone()),
        )
        .collect::<Vec<_>>();
    masked_ranges.sort_by_key(|range| range.start);
    for range in masked_ranges {
        bytes[range].fill(b'x');
    }

    String::from_utf8(bytes).expect("ASCII masking keeps UTF-8 valid")
}

/// Build instances independently for capture-parse. A malformed row does
/// not prevent other well-formed rows from being previewed, and each valid
/// instance carries the map needed to report diagnostics against the typed
/// template and row values.
pub(crate) fn expand_rows_for_editor(
    raw: &str,
    analysis: &TemplateAnalysis,
) -> Vec<Option<TemplateInstance>> {
    let Some(block) = analysis.block.as_ref() else {
        return Vec::new();
    };
    if analysis
        .issues
        .iter()
        .any(|issue| issue.code == "template_invalid_placeholder")
        || analysis.field_count == 0
    {
        return analysis.rows.iter().map(|_| None).collect();
    }

    let mut expanded_bytes = 0usize;
    analysis
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            if index >= MAX_ROWS
                || analysis.issues.iter().any(|issue| {
                    issue.code == "template_unterminated_quote"
                        && issue.range.start < row.range.end
                        && row.range.start < issue.range.end
                })
            {
                return None;
            }
            if row.fields.len() != analysis.field_count {
                return None;
            }
            let instance = build_instance(
                raw,
                block,
                &analysis.placeholders,
                &analysis.escapes,
                row,
            )
            .ok()?;
            let separator = usize::from(expanded_bytes > 0) * 2;
            if expanded_bytes
                .saturating_add(separator)
                .saturating_add(instance.text.len())
                .saturating_add(1)
                > MAX_EXPANDED_BYTES
            {
                return None;
            }
            expanded_bytes = expanded_bytes
                .saturating_add(separator)
                .saturating_add(instance.text.len());
            Some(instance)
        })
        .collect()
}

/// Analyze the framing and language of `raw`. A missing header with no
/// misplaced header is represented by an analysis with no `header` and no
/// issues; a misplaced `@@template:` still produces an issue.
pub(crate) fn analyze(raw: &str) -> TemplateAnalysis {
    let lines = split_physical_lines(raw);
    let first_nonblank =
        lines.iter().position(|line| !line.text.trim().is_empty());
    let header_index = first_nonblank.filter(|index| {
        trim_ascii_space_tab(lines[*index].text).eq_ignore_ascii_case(HEADER)
    });
    let mut issues = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        for token in tokenize_with_spans(line.text) {
            if token.text.to_ascii_lowercase().starts_with(HEADER)
                && Some(index) != header_index
            {
                let range =
                    (line.start + token.start)..(line.start + token.end);
                issues.push(issue(
                    "template_header_misplaced",
                    "write `@@template:` alone on the draft's first line",
                    range,
                ));
            }
        }
    }

    let Some(header_index) = header_index else {
        sort_issues(&mut issues);
        return TemplateAnalysis {
            header: None,
            block: None,
            placeholders: Vec::new(),
            escapes: Vec::new(),
            field_count: 0,
            rows: Vec::new(),
            issues,
        };
    };

    let header_line = lines[header_index];
    let header_text = trim_ascii_space_tab(header_line.text);
    let header_start = header_line.start
        + (header_text.as_ptr() as usize - header_line.text.as_ptr() as usize);
    let header_range = header_start..(header_start + header_text.len());
    let header = Some((header_range.clone(), header_index + 1));

    let Some(block_start_index) = (header_index + 1..lines.len())
        .find(|index| !lines[*index].text.trim().is_empty())
    else {
        issues.push(issue(
            "template_missing_rows",
            "write the template on the next line, then a blank line and one row per capture",
            header_range,
        ));
        sort_issues(&mut issues);
        return TemplateAnalysis {
            header,
            block: None,
            placeholders: Vec::new(),
            escapes: Vec::new(),
            field_count: 0,
            rows: Vec::new(),
            issues,
        };
    };

    let separator_index = (block_start_index..lines.len())
        .find(|index| lines[*index].text.trim().is_empty());
    let block_end_index = separator_index.unwrap_or(lines.len());
    let block_lines = (block_start_index..block_end_index)
        .map(|index| (index + 1, lines[index].start..lines[index].end))
        .collect::<Vec<_>>();
    let block = TemplateBlock {
        range: lines[block_start_index].start..lines[block_end_index - 1].end,
        line_start: block_start_index + 1,
        line_end: block_end_index,
        lines: block_lines,
    };

    for (line_number, range) in block.lines.iter().skip(1) {
        let line_text = &raw[range.clone()];
        if !looks_like_bullet(line_text) {
            issues.push(issue(
                "template_needs_separator",
                format!("template line {line_number} directly under the template is not a child bullet; add a blank line between the template and its rows"),
                range.clone(),
            ));
        }
    }

    let mut placeholders = Vec::new();
    let mut escaped_braces = Vec::new();
    for (line_number, range) in &block.lines {
        let text = &raw[range.clone()];
        scan_template_braces(
            text,
            range.start,
            *line_number,
            &mut placeholders,
            &mut escaped_braces,
            &mut issues,
        );
    }
    placeholders.sort_by_key(|placeholder| placeholder.range.start);
    let fields_used = placeholders
        .iter()
        .map(|placeholder| placeholder.field)
        .collect::<BTreeSet<_>>();
    let field_count = fields_used.iter().next_back().map_or(0, |last| last + 1);
    if !fields_used.is_empty() {
        if fields_used.iter().copied().ne(0..field_count) {
            let missing = (0..field_count)
                .find(|index| !fields_used.contains(index))
                .expect("non-dense set has a missing value");
            let description = if fields_used.first() == Some(&1)
                && fields_used.iter().copied().eq(1..field_count)
            {
                if field_count == 2 {
                    "{1}".to_string()
                } else {
                    format!("{{1}}–{{{}}}", field_count - 1)
                }
            } else {
                let shown = fields_used
                    .iter()
                    .map(|field| format!("{{{field}}}"))
                    .collect::<Vec<_>>()
                    .join(" and ");
                shown
            };
            let message = if fields_used.first() == Some(&1)
                && fields_used.iter().copied().eq(1..field_count)
            {
                format!("placeholders start at {{0}}; this template uses {description}")
            } else {
                format!("this template uses {description} but not {{{missing}}}; number placeholders {{0}}–{{{}}} without gaps", field_count - 1)
            };
            issues.push(issue(
                "template_placeholder_gap",
                format!("template line {}: {message}", block.line_start),
                block.range.clone(),
            ));
        }
    }

    let mut rows = Vec::new();
    if let Some(separator_index) = separator_index {
        for (index, line) in lines.iter().enumerate().skip(separator_index + 1)
        {
            let trimmed = trim_ascii_space_tab(line.text);
            if trimmed.is_empty() {
                continue;
            }
            let row_number = rows.len() + 1;
            let leading = line.text.len()
                - line.text.trim_start_matches([' ', '\t']).len();
            let trailing = trimmed.len();
            let row_range =
                (line.start + leading)..(line.start + leading + trailing);
            if row_number == MAX_ROWS + 1 {
                issues.push(issue(
                    "template_too_large",
                    format!("template row {row_number} (line {}) exceeds the limit of {MAX_ROWS} rows", index + 1),
                    row_range.clone(),
                ));
            }
            check_global_tokens_in_row(
                line,
                row_number,
                index + 1,
                &mut issues,
            );
            if is_row_bullet(trimmed) {
                issues.push(issue(
                    "template_row_bullet",
                    format!("template row {row_number} (line {}): rows can't have child bullets yet; put each capture's values on one row", index + 1),
                    row_range.clone(),
                ));
                rows.push(TemplateRow {
                    row: row_number,
                    line: index + 1,
                    range: row_range,
                    fields: Vec::new(),
                });
                continue;
            }
            let (fields, quote_issue) = lex_row(
                trimmed,
                row_range.start,
                row_number,
                index + 1,
                field_count,
            );
            let unterminated_quote = quote_issue.is_some();
            if let Some(issue) = quote_issue {
                issues.push(issue);
            }
            if !unterminated_quote
                && field_count > 0
                && fields.len() != field_count
            {
                issues.push(issue(
                    "template_row_arity",
                    format!("template row {row_number} (line {}): has {} field{} but the template uses {}; quote a field that contains spaces: \"Two Sigma\"", index + 1, fields.len(), if fields.len() == 1 { "" } else { "s" }, field_range(field_count)),
                    row_range.clone(),
                ));
            }
            rows.push(TemplateRow {
                row: row_number,
                line: index + 1,
                range: row_range,
                fields,
            });
        }
    }

    for (line_number, range) in &block.lines {
        check_global_tokens(
            &RawLine {
                text: &raw[range.clone()],
                start: range.start,
                end: range.end,
            },
            *line_number,
            &mut issues,
        );
    }

    if rows.is_empty() {
        issues.push(issue(
            "template_missing_rows",
            "add a blank line, then one row per capture",
            block.range.end..block.range.end,
        ));
    }
    if !rows.is_empty()
        && placeholders.is_empty()
        && !issues
            .iter()
            .any(|issue| issue.code() == "template_invalid_placeholder")
    {
        issues.push(issue(
            "template_missing_placeholder",
            format!(
                "template line {}: add at least one placeholder such as {{0}}",
                block.line_start
            ),
            block.range.end..block.range.end,
        ));
    }

    // Bounds on expanded bytes are checked by constructing only when the
    // language is otherwise valid. This keeps the issue in draft order and
    // avoids trying to interpret malformed placeholders.
    if issues.is_empty() {
        match build_expansion(
            raw,
            &block,
            &placeholders,
            &escaped_braces,
            &rows,
            header_index + 1,
            field_count,
        ) {
            Ok(expansion) => {
                for instance in &expansion.instances {
                    for token in tokenize_with_spans(&instance.text) {
                        if token.text.to_ascii_lowercase().starts_with(HEADER) {
                            issues.push(issue(
                                "template_header_misplaced",
                                "write `@@template:` alone on the draft's first line",
                                instance.row_range.clone(),
                            ));
                        } else if token.text.starts_with("@@") {
                            issues.push(issue(
                                "template_global_declaration",
                                format!("template row {} (line {}): a template draft can't declare `@@` destinations; put the destination in the template line", instance.row, instance.line),
                                instance.row_range.clone(),
                            ));
                        }
                    }
                }
            }
            Err(()) => {
                let range = rows
                    .last()
                    .map(|row| row.range.clone())
                    .unwrap_or_else(|| block.range.clone());
                issues.push(issue(
                    "template_too_large",
                    "expanded template exceeds the 1 MiB limit",
                    range,
                ));
            }
        }
    }

    sort_issues(&mut issues);
    TemplateAnalysis {
        header,
        block: Some(block),
        placeholders,
        escapes: escaped_braces,
        field_count,
        rows,
        issues,
    }
}

/// The 1-based row whose range overlaps `issue`, when the issue is
/// row-scoped. `template_too_large` is always template-scoped, even when its
/// range sits on a row.
pub(crate) fn row_number_for_issue(
    analysis: &TemplateAnalysis,
    issue: &TemplateIssue,
) -> Option<usize> {
    if issue.code() == "template_too_large" {
        return None;
    }
    analysis.rows.iter().find_map(|row| {
        let overlaps = row.range.start < issue.range().end
            && issue.range().start < row.range.end;
        overlaps.then_some(row.row)
    })
}

/// Build expansion instances for every analyzable row before `upto_row`
/// (exclusive). Rows that cannot be built are skipped; callers only use this
/// to surface an earlier execution error ahead of a later row-scoped
/// language issue, so leading rows are expected to be valid.
pub(crate) fn leading_instances(
    raw: &str,
    analysis: &TemplateAnalysis,
    upto_row: usize,
) -> Vec<TemplateInstance> {
    let Some(block) = analysis.block.as_ref() else {
        return Vec::new();
    };
    analysis
        .rows
        .iter()
        .filter(|row| row.row < upto_row)
        .filter_map(|row| {
            build_instance(
                raw,
                block,
                &analysis.placeholders,
                &analysis.escapes,
                row,
            )
            .ok()
        })
        .collect()
}

/// Expand a template draft, or return `None` unchanged for an ordinary
/// capture draft. The first issue in draft order is the stable CLI error.
pub(crate) fn expand(
    raw: &str,
) -> Result<Option<TemplateExpansion>, TemplateIssue> {
    let analysis = analyze(raw);
    if let Some(issue) = analysis.issues.first() {
        return Err(issue.clone());
    }
    let Some((_, header_line)) = analysis.header else {
        return Ok(None);
    };
    let Some(block) = analysis.block else {
        return Ok(None);
    };
    Ok(Some(
        build_expansion(
            raw,
            &block,
            &analysis.placeholders,
            &analysis.escapes,
            &analysis.rows,
            header_line,
            analysis.field_count,
        )
        .expect("analyzed template expansion must fit the size limit"),
    ))
}

fn build_expansion(
    raw: &str,
    block: &TemplateBlock,
    placeholders: &[TemplatePlaceholder],
    escapes: &[Range<usize>],
    rows: &[TemplateRow],
    header_line: usize,
    field_count: usize,
) -> Result<TemplateExpansion, ()> {
    let mut text = String::new();
    let mut instances = Vec::with_capacity(rows.len());
    for row in rows {
        let mut instance =
            build_instance(raw, block, placeholders, escapes, row)?;
        if !text.is_empty() {
            push_bounded_str(&mut text, "\n\n")?;
        }
        let start = text.len();
        push_bounded_str(&mut text, &instance.text)?;
        let end = text.len();
        instance.expanded_range = start..end;
        instances.push(instance);
    }
    if !instances.is_empty() {
        push_bounded_char(&mut text, '\n')?;
    }
    Ok(TemplateExpansion {
        text,
        instances,
        summary: TemplateSummary {
            line: header_line,
            field_count,
            row_count: rows.len(),
        },
    })
}

fn build_instance(
    raw: &str,
    block: &TemplateBlock,
    placeholders: &[TemplatePlaceholder],
    escapes: &[Range<usize>],
    row: &TemplateRow,
) -> Result<TemplateInstance, ()> {
    let mut text = String::new();
    let mut source_map = Vec::new();
    for (line_index, (_, range)) in block.lines.iter().enumerate() {
        if line_index > 0 {
            let previous = &block.lines[line_index - 1].1;
            let newline_range = previous.end..range.start;
            let start = text.len();
            push_bounded_char(&mut text, '\n')?;
            push_source_segment(
                &mut source_map,
                start..text.len(),
                newline_range,
                None,
            );
        }
        append_expanded_line(
            &mut text,
            &raw[range.clone()],
            range.start,
            placeholders,
            escapes,
            &row.fields,
            &mut source_map,
        )?;
    }
    Ok(TemplateInstance {
        row: row.row,
        line: row.line,
        template_line_start: block.line_start,
        row_range: row.range.clone(),
        expanded_range: 0..text.len(),
        text,
        source_map,
    })
}

fn append_expanded_line(
    output: &mut String,
    line: &str,
    base: usize,
    placeholders: &[TemplatePlaceholder],
    escapes: &[Range<usize>],
    fields: &[TemplateField],
    source_map: &mut Vec<TemplateSourceSegment>,
) -> Result<(), ()> {
    let mut cursor = 0;
    let mut placeholder_index = placeholders
        .partition_point(|placeholder| placeholder.range.start < base);
    let mut escape_index =
        escapes.partition_point(|escape| escape.start < base);
    while cursor < line.len() {
        let absolute = base + cursor;
        if let Some(placeholder) = placeholders.get(placeholder_index)
            && placeholder.range.start == absolute
        {
            let field = fields.get(placeholder.field).ok_or(())?;
            let start = output.len();
            push_bounded_str(output, &field.value)?;
            if start < output.len() {
                push_source_segment(
                    source_map,
                    start..output.len(),
                    field.range.clone(),
                    Some(field.field),
                );
            }
            cursor = placeholder.range.end - base;
            placeholder_index += 1;
        } else if let Some(escape) = escapes.get(escape_index)
            && escape.start == absolute
        {
            let start = output.len();
            push_bounded_char(
                output,
                line[cursor..].chars().next().expect("brace"),
            )?;
            push_source_segment(
                source_map,
                start..output.len(),
                escape.clone(),
                None,
            );
            cursor = escape.end - base;
            escape_index += 1;
        } else {
            let character = line[cursor..].chars().next().expect("valid UTF-8");
            let start = output.len();
            push_bounded_char(output, character)?;
            push_source_segment(
                source_map,
                start..output.len(),
                absolute..(absolute + character.len_utf8()),
                None,
            );
            cursor += character.len_utf8();
        }
    }
    Ok(())
}

fn push_source_segment(
    segments: &mut Vec<TemplateSourceSegment>,
    expanded: Range<usize>,
    source: Range<usize>,
    field: Option<usize>,
) {
    if expanded.is_empty() {
        return;
    }
    if let Some(previous) = segments.last_mut()
        && previous.expanded.end == expanded.start
        && previous.source.end == source.start
        && previous.field == field
    {
        previous.expanded.end = expanded.end;
        previous.source.end = source.end;
    } else {
        segments.push(TemplateSourceSegment {
            expanded,
            source,
            field,
        });
    }
}

fn push_bounded_str(output: &mut String, value: &str) -> Result<(), ()> {
    if output.len().saturating_add(value.len()) > MAX_EXPANDED_BYTES {
        return Err(());
    }
    output.push_str(value);
    Ok(())
}

fn push_bounded_char(output: &mut String, value: char) -> Result<(), ()> {
    if output.len().saturating_add(value.len_utf8()) > MAX_EXPANDED_BYTES {
        return Err(());
    }
    output.push(value);
    Ok(())
}

fn scan_template_braces(
    line: &str,
    base: usize,
    line_number: usize,
    placeholders: &mut Vec<TemplatePlaceholder>,
    escapes: &mut Vec<Range<usize>>,
    issues: &mut Vec<TemplateIssue>,
) {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' if bytes.get(index + 1) == Some(&b'{') => {
                escapes.push((base + index)..(base + index + 2));
                index += 2;
            }
            b'}' if bytes.get(index + 1) == Some(&b'}') => {
                escapes.push((base + index)..(base + index + 2));
                index += 2;
            }
            b'{' => {
                let close = bytes[index + 1..]
                    .iter()
                    .position(|byte| *byte == b'}')
                    .map(|offset| index + 1 + offset);
                let end = close.map_or(bytes.len(), |close| close + 1);
                let valid = close.is_some_and(|close| {
                    let digits = &line[index + 1..close];
                    !digits.is_empty()
                        && digits.bytes().all(|byte| byte.is_ascii_digit())
                        && (digits == "0" || !digits.starts_with('0'))
                        && digits
                            .parse::<usize>()
                            .is_ok_and(|number| number <= 999)
                });
                if valid {
                    let close = close.expect("valid placeholder has a close");
                    let field = line[index + 1..close]
                        .parse::<usize>()
                        .expect("validated decimal placeholder");
                    placeholders.push(TemplatePlaceholder {
                        field,
                        range: (base + index)..(base + close + 1),
                    });
                } else {
                    issues.push(issue(
                        "template_invalid_placeholder",
                        format!("template line {line_number}: invalid placeholder; write `{{N}}` with a canonical number, and write `{{{{` for a literal brace"),
                        (base + index)..(base + end),
                    ));
                }
                index = end;
            }
            b'}' => {
                issues.push(issue(
                    "template_invalid_placeholder",
                    format!("template line {line_number}: invalid placeholder; write `{{N}}` with a canonical number, and write `{{{{` for a literal brace"),
                    (base + index)..(base + index + 1),
                ));
                index += 1;
            }
            _ => index += 1,
        }
    }
}

fn lex_row(
    row: &str,
    base: usize,
    row_number: usize,
    line_number: usize,
    field_count: usize,
) -> (Vec<TemplateField>, Option<TemplateIssue>) {
    if field_count == 1 {
        return (
            vec![TemplateField {
                field: 0,
                value: row.to_string(),
                range: base..(base + row.len()),
            }],
            None,
        );
    }

    let mut fields = Vec::new();
    let mut cursor = 0;
    while cursor < row.len() {
        while cursor < row.len() && is_row_space(row.as_bytes()[cursor]) {
            cursor += 1;
        }
        if cursor >= row.len() {
            break;
        }
        let field_start = cursor;
        let first = row[cursor..].chars().next().expect("valid UTF-8");
        let (value, end, unterminated) = if first == '"' || first == '“' {
            scan_quoted_field(row, cursor)
        } else {
            let mut end = cursor;
            while end < row.len() && !is_row_space(row.as_bytes()[end]) {
                end += 1;
            }
            (row[cursor..end].to_string(), end, false)
        };
        fields.push(TemplateField {
            field: fields.len(),
            value,
            range: (base + field_start)..(base + end),
        });
        cursor = end;
        if unterminated {
            return (
                fields,
                Some(issue(
                    "template_unterminated_quote",
                    format!("template row {row_number} (line {line_number}): quoted field has no closing quote"),
                    (base + field_start)..(base + row.len()),
                )),
            );
        }
    }
    (fields, None)
}

fn scan_quoted_field(row: &str, start: usize) -> (String, usize, bool) {
    let mut value = String::new();
    let mut cursor =
        start + row[start..].chars().next().expect("quote").len_utf8();
    while cursor < row.len() {
        let character = row[cursor..].chars().next().expect("valid UTF-8");
        if character == '\\' {
            let next_start = cursor + 1;
            if next_start < row.len() {
                let next =
                    row[next_start..].chars().next().expect("valid UTF-8");
                if next == '"' || next == '\\' {
                    value.push(next);
                    cursor = next_start + next.len_utf8();
                    continue;
                }
            }
            value.push(character);
            cursor += character.len_utf8();
            continue;
        }
        if (character == '"' || character == '”')
            && (cursor + character.len_utf8() == row.len()
                || is_row_space(row.as_bytes()[cursor + character.len_utf8()]))
        {
            let end = cursor + character.len_utf8();
            return (value, end, false);
        }
        value.push(character);
        cursor += character.len_utf8();
    }
    (value, row.len(), true)
}

fn check_global_tokens(
    line: &RawLine<'_>,
    line_number: usize,
    issues: &mut Vec<TemplateIssue>,
) {
    for token in tokenize_with_spans(line.text) {
        if token.text.starts_with("@@")
            && !token.text.to_ascii_lowercase().starts_with(HEADER)
        {
            issues.push(issue(
                "template_global_declaration",
                format!("template line {line_number}: a template draft can't declare `@@` destinations; put the destination in the template line"),
                (line.start + token.start)..(line.start + token.end),
            ));
        }
    }
}

fn check_global_tokens_in_row(
    line: &RawLine<'_>,
    row_number: usize,
    line_number: usize,
    issues: &mut Vec<TemplateIssue>,
) {
    for token in tokenize_with_spans(line.text) {
        if token.text.starts_with("@@")
            && !token.text.to_ascii_lowercase().starts_with(HEADER)
        {
            issues.push(issue(
                "template_global_declaration",
                format!("template row {row_number} (line {line_number}): a template draft can't declare `@@` destinations; put the destination in the template line"),
                (line.start + token.start)..(line.start + token.end),
            ));
        }
    }
}

fn field_range(field_count: usize) -> String {
    match field_count {
        0 => "no placeholders".to_string(),
        1 => "{0}".to_string(),
        count => format!("{{0}}–{{{}}}", count - 1),
    }
}

fn looks_like_bullet(line: &str) -> bool {
    let body = line.trim_start_matches([' ', '\t']);
    body.as_bytes().get(1).is_some_and(|separator| {
        matches!(body.as_bytes()[0], b'-' | b'*' | b'+')
            && matches!(separator, b' ' | b'\t')
    })
}

fn is_row_bullet(line: &str) -> bool {
    looks_like_bullet(line)
}

fn is_row_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

fn trim_ascii_space_tab(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
}

fn issue(
    code: &'static str,
    message: impl Into<String>,
    range: Range<usize>,
) -> TemplateIssue {
    TemplateIssue {
        code,
        message: message.into(),
        range,
    }
}

fn sort_issues(issues: &mut [TemplateIssue]) {
    issues.sort_by_key(|issue| issue.range.start);
}
