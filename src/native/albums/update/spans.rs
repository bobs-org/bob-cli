//! Conservative, reversible edits of an album's nested `record:` YAML.
//!
//! The editor works from parsed ordered mappings plus line spans. It moves
//! existing entries as their original byte slices, emits only new or changed
//! values, and refuses YAML features that make a faithful edit uncertain.
//! Restorations name schema key paths and exact expected/prior entry bytes;
//! offsets and filesystem paths never come from the request.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use serde_yaml::{Mapping, Value as YamlValue};

use super::{
    ops::{self, ApplyInputs, OpError, OpOutcome},
    protocol::{Op, Restoration, RestoreSpan},
};
use crate::native::yaml_spans::{
    self, blank_or_comment, first_content_indent, gap_after_colon,
    has_yaml_feature, indentation, join_lines, parse_key, render_scalar,
    split_comment, split_lines, whitespace_before_comment, yaml_text, Line,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpanError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) line: Option<usize>,
    pub(crate) key_path: Option<String>,
}

impl std::fmt::Display for SpanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "[{}]: {}", self.code, self.message)
    }
}

impl std::error::Error for SpanError {}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RestoredOp {
    pub(crate) op: Op,
    pub(crate) restore: Option<Restoration>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SchemaSpanEdit {
    pub(crate) contents: String,
    pub(crate) changed: bool,
    /// Reverse-ordered semantic operations with byte restoration metadata.
    pub(crate) inverse_ops: Vec<RestoredOp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    key: String,
    start: usize,
    key_line: usize,
    end: usize,
    indent: usize,
    colon: usize,
}

#[derive(Debug)]
pub(crate) struct Document {
    lines: Vec<Line>,
    close: usize,
    newline: String,
}

#[derive(Debug, Clone)]
struct Step {
    before_text: String,
    after_text: String,
    inverse: Vec<Op>,
}

/// Apply the already-validated ordered operations to schema spans. The
/// operation planner validates the batch atomically first; this function
/// repeats its pure per-op state transitions only to preserve source bytes.
/// Top-level `walk_by` edits never touch the nested `record:` mapping and
/// never rewrite unrelated YAML.
pub(crate) fn apply_schema_ops(
    markdown: &str,
    ops_to_apply: &[Op],
    restorations: &[Option<Restoration>],
    outcome: &OpOutcome,
    inputs: &ApplyInputs<'_>,
) -> Result<SchemaSpanEdit, SpanError> {
    if restorations.len() != ops_to_apply.len() {
        return Err(error(
            "span_protocol_mismatch",
            "operation restoration list does not match the operation list",
            None,
            None,
        ));
    }
    if ops_to_apply.is_empty() {
        return Ok(SchemaSpanEdit {
            contents: markdown.to_owned(),
            changed: false,
            inverse_ops: Vec::new(),
        });
    }

    let (_, initial_raw) = parse_record_mapping(markdown)?;
    if &initial_raw != inputs.before_raw {
        return Err(error(
            "schema_span_stale",
            "the record mapping no longer matches the planner's preimage",
            None,
            Some("record".to_owned()),
        ));
    }
    // Net semantic no-ops return original bytes before any canonical
    // emission or index repair, comparing ordered raw schema/presence
    // state rather than emitted bytes. Explicit absent-versus-false/unset
    // stays distinct because presence is part of the ordered mapping.
    // Plural lives inside the record mapping when authored; derived
    // display plurals do not affect byte identity. The top-level walk
    // selection and authored presence are part of semantic identity too.
    // Restorations still run their stale/whitelist checks, so a semantic
    // no-op with restore metadata falls through rather than skipping.
    if mappings_equal_ordered(&initial_raw, &outcome.after_raw)
        && inputs.index_refs.walk_by == outcome.walk_after
        && inputs.index_refs.walk_by_authored == outcome.walk_authored_after
        && restorations.iter().all(Option::is_none)
    {
        return Ok(SchemaSpanEdit {
            contents: markdown.to_owned(),
            changed: false,
            inverse_ops: Vec::new(),
        });
    }

    let mut current_text = markdown.to_owned();
    let mut current_raw = initial_raw;
    let mut walk_current = inputs.index_refs.walk_by.clone();
    let mut walk_authored = inputs.index_refs.walk_by_authored;
    let mut steps = Vec::with_capacity(ops_to_apply.len());
    for (index, op) in ops_to_apply.iter().enumerate() {
        let before_text = current_text.clone();
        let mut next_raw = current_raw.clone();
        let mut next_walk = walk_current.clone();
        let result = ops::apply_one(
            op,
            index,
            &mut next_raw,
            inputs,
            &mut next_walk,
            &mut walk_authored,
        )
        .map_err(op_error)?;
        if matches!(op, Op::WalkBySet { .. }) {
            current_text = patch_walk_by(
                &current_text,
                next_walk.as_deref(),
                op,
                restorations[index].as_ref(),
            )?;
            let emitted = emitted_walk_selection(&current_text, inputs.stem)?;
            if emitted != next_walk {
                return Err(error(
                    "span_semantic_mismatch",
                    "restored walk_by does not match the validated selection",
                    None,
                    Some("walk_by".to_owned()),
                ));
            }
        } else {
            current_text = patch_record_mapping(
                &current_text,
                &current_raw,
                &next_raw,
                op,
                restorations[index].as_ref(),
            )?;
        }
        let (_, parsed_raw) = parse_record_mapping(&current_text)?;
        current_raw = parsed_raw;
        walk_current = next_walk;
        steps.push(Step {
            before_text,
            after_text: current_text.clone(),
            inverse: result.inverse,
        });
    }

    if current_raw != outcome.after_raw {
        return Err(error(
            "span_semantic_mismatch",
            "edited YAML does not match the validated post-schema",
            None,
            Some("record".to_owned()),
        ));
    }
    // This is an independent parse through the shared album validator. It
    // catches emitter or restoration output that is syntactically YAML but
    // does not preserve the exact validated schema.
    let final_type = crate::native::records::schema::parse_album_record_schema(
        Some(&YamlValue::Mapping(current_raw.clone())),
        Path::new("album.md"),
        inputs.stem,
        None,
        &HashMap::new(),
        1,
    );
    if final_type.record_type.as_ref() != Some(&outcome.after_type) {
        return Err(error(
            "span_semantic_mismatch",
            format!(
                "edited YAML does not parse to the validated record type: expected fields {:?}, parsed fields {:?}",
                outcome.after_type.fields,
                final_type.record_type.as_ref().map(|record_type| &record_type.fields)
            ),
            None,
            Some("record".to_owned()),
        ));
    }
    let parsed_walk = emitted_walk_selection(&current_text, inputs.stem)?;
    if parsed_walk != outcome.walk_after {
        return Err(error(
            "span_semantic_mismatch",
            "edited YAML does not preserve the validated walk_by selection",
            None,
            Some("walk_by".to_owned()),
        ));
    }

    let inverse_ops = build_inverse_ops(&steps);
    Ok(SchemaSpanEdit {
        changed: current_text != markdown,
        contents: current_text,
        inverse_ops,
    })
}

/// Extract the exact ordered raw `record:` mapping from an album note.
pub(crate) fn parse_record_mapping(
    markdown: &str,
) -> Result<(Document, Mapping), SpanError> {
    let document = document(markdown)?;
    let yaml = yaml_text(&document.lines[1..document.close]);
    let root: YamlValue =
        serde_yaml::from_str(&yaml).map_err(|yaml_error| {
            error(
                "unsupported_yaml",
                format!("frontmatter is not valid YAML: {yaml_error}"),
                None,
                None,
            )
        })?;
    let root_map = root.as_mapping().ok_or_else(|| {
        error(
            "unsupported_yaml",
            "frontmatter must be a YAML mapping",
            None,
            None,
        )
    })?;
    let record = root_map
        .get(YamlValue::String("record".to_owned()))
        .and_then(YamlValue::as_mapping)
        .ok_or_else(|| {
            error(
                "unsupported_yaml",
                "album record must be a nested YAML mapping",
                None,
                Some("record".to_owned()),
            )
        })?
        .clone();
    validate_root_spans(&document, root_map)?;
    validate_record_spans(&document, &record)?;
    Ok((document, record))
}

fn document(markdown: &str) -> Result<Document, SpanError> {
    let lines = split_lines(markdown);
    let opening = lines
        .first()
        .is_some_and(|line| line.text.trim_start_matches('\u{feff}') == "---");
    if !opening {
        return Err(error(
            "unsupported_yaml",
            "album note has no opening frontmatter delimiter",
            Some(1),
            None,
        ));
    }
    let close = lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.text == "---").then_some(index))
        .ok_or_else(|| {
            error(
                "unsupported_yaml",
                "album note has no closing frontmatter delimiter",
                None,
                None,
            )
        })?;
    let newline = lines
        .iter()
        .find(|line| !line.ending.is_empty())
        .map(|line| line.ending.clone())
        .unwrap_or_else(|| "\n".to_owned());
    Ok(Document {
        lines,
        close,
        newline,
    })
}

fn validate_root_spans(
    document: &Document,
    root: &Mapping,
) -> Result<(), SpanError> {
    let entries = block_entries(&document.lines, 1, document.close, 0)?;
    ensure_entries_match(entries, root, "")
}

fn validate_record_spans(
    document: &Document,
    record: &Mapping,
) -> Result<(), SpanError> {
    let root_entries = block_entries(&document.lines, 1, document.close, 0)?;
    let record_entry = root_entries
        .iter()
        .find(|entry| entry.key == "record")
        .ok_or_else(|| {
            error(
                "unsupported_yaml",
                "record mapping has no unambiguous source entry",
                None,
                Some("record".to_owned()),
            )
        })?;
    if has_yaml_feature(&document.lines[record_entry.start..=record_entry.end])
    {
        return Err(error(
            "unsupported_yaml",
            "anchors, aliases, and tags inside record schemas are not safe to edit",
            Some(record_entry.key_line + 1),
            Some("record".to_owned()),
        ));
    }
    validate_nested_mapping(
        document,
        record_entry,
        &YamlValue::Mapping(record.clone()),
        "record",
    )
}

fn validate_nested_mapping(
    document: &Document,
    entry: &Entry,
    value: &YamlValue,
    path: &str,
) -> Result<(), SpanError> {
    let Some(mapping) = value.as_mapping() else {
        return Ok(());
    };
    let lines = &document.lines;
    let (inline, _) = line_value(lines, entry)?;
    if !inline.is_empty() {
        validate_flow_map_keys(&inline, path, entry.key_line + 1)?;
        return Ok(());
    }
    let child_start = entry.key_line + 1;
    let child_indent = first_content_indent(lines, child_start, entry.end)
        .unwrap_or(entry.indent + 2);
    if child_indent <= entry.indent {
        if mapping.is_empty() {
            return Ok(());
        }
        return Err(error(
            "unsupported_yaml",
            "nested mapping has no source span",
            Some(entry.key_line + 1),
            Some(path.to_owned()),
        ));
    }
    let children =
        block_entries(lines, child_start, entry.end + 1, child_indent)?;
    ensure_entries_match(children.clone(), mapping, path)?;
    for child in children {
        let key = YamlValue::String(child.key.clone());
        if let Some(child_value) = mapping.get(&key) {
            validate_nested_mapping(
                document,
                &child,
                child_value,
                &format!("{path}.{}", child.key),
            )?;
        }
    }
    Ok(())
}

fn ensure_entries_match(
    entries: Vec<Entry>,
    mapping: &Mapping,
    parent: &str,
) -> Result<(), SpanError> {
    if entries.len() != mapping.len() {
        return Err(error(
            "unsupported_yaml",
            "YAML mapping entries do not have unique editable source spans",
            None,
            (!parent.is_empty()).then(|| parent.to_owned()),
        ));
    }
    for key in mapping.keys() {
        let Some(key) = key.as_str() else {
            return Err(error(
                "unsupported_yaml",
                "schema mapping keys must be strings to preserve their identity",
                None,
                (!parent.is_empty()).then(|| parent.to_owned()),
            ));
        };
        if !entries.iter().any(|entry| entry.key == key) {
            return Err(error(
                "unsupported_yaml",
                format!("mapping key {key:?} has no unique source span"),
                None,
                Some(if parent.is_empty() {
                    key.to_owned()
                } else {
                    format!("{parent}.{key}")
                }),
            ));
        }
    }
    Ok(())
}

fn block_entries(
    lines: &[Line],
    start: usize,
    end: usize,
    indent: usize,
) -> Result<Vec<Entry>, SpanError> {
    let mut found = Vec::<(usize, String, usize)>::new();
    for (index, line) in lines
        .iter()
        .enumerate()
        .take(end.min(lines.len()))
        .skip(start)
    {
        let line = &line.text;
        if blank_or_comment(line) {
            continue;
        }
        let actual_indent = indentation(line);
        if actual_indent < indent {
            continue;
        }
        if actual_indent != indent {
            continue;
        }
        let Some((key, colon)) = parse_key(line, indent) else {
            return Err(error(
                "unsupported_yaml",
                "a block mapping line has no unambiguous key span",
                Some(index + 1),
                None,
            ));
        };
        if found.iter().any(|(_, prior, _)| prior == &key) {
            return Err(error(
                "unsupported_yaml",
                format!("duplicate YAML key {key:?} cannot be edited safely"),
                Some(index + 1),
                Some(key),
            ));
        }
        found.push((index, key, colon));
    }
    let mut entries = Vec::with_capacity(found.len());
    for (at, (key_line, key, colon)) in found.iter().enumerate() {
        let end = found
            .get(at + 1)
            .map(|(next, _, _)| next.saturating_sub(1))
            .unwrap_or_else(|| end.min(lines.len()).saturating_sub(1));
        entries.push(Entry {
            key: key.clone(),
            start: *key_line,
            key_line: *key_line,
            end: end.max(*key_line),
            indent,
            colon: *colon,
        });
    }
    Ok(entries)
}

fn line_value(
    lines: &[Line],
    entry: &Entry,
) -> Result<(String, String), SpanError> {
    let line = &lines[entry.key_line].text;
    let rest = &line[entry.colon + 1..];
    let (value, comment) = split_comment(rest);
    Ok((value.trim().to_owned(), comment.to_owned()))
}

fn validate_flow_map_keys(
    flow: &str,
    path: &str,
    line: usize,
) -> Result<(), SpanError> {
    yaml_spans::validate_flow_map_keys(flow).map_err(|err| {
        let message = match err {
            yaml_spans::FlowMapError::UnresolvableKey => {
                "flow mapping has a key that cannot be resolved safely"
            }
            yaml_spans::FlowMapError::DuplicateKey => {
                "duplicate key in flow YAML cannot be edited safely"
            }
            yaml_spans::FlowMapError::CommentInside => {
                "comments inside a flow value cannot be preserved safely"
            }
        };
        error(
            "unsupported_yaml",
            message,
            Some(line),
            Some(path.to_owned()),
        )
    })
}

/// Patch only the top-level `walk_by` key, never the nested
/// `record.walk_by`. Existing entry bytes move as their original slice;
/// only the scalar value is replaced. Absent keys insert at the original
/// slot when restore locators are present, otherwise at the frontmatter
/// close. Restoration may only name `walk_by`.
fn patch_walk_by(
    markdown: &str,
    desired: Option<&str>,
    op: &Op,
    restoration: Option<&Restoration>,
) -> Result<String, SpanError> {
    let mut overrides: HashMap<String, String> = HashMap::new();
    if let Some(restoration) = restoration {
        validate_restoration_for_op(op, restoration)?;
        for span in &restoration.spans {
            if (span.after.is_some() || span.before.is_some())
                && span.key_path != "walk_by"
            {
                return Err(error(
                    "schema_restore_invalid",
                    "restore locators are only valid for walk_by",
                    None,
                    Some(span.key_path.clone()),
                ));
            }
            let actual = exact_span(markdown, &span.key_path)?;
            if actual != span.expected {
                return Err(error(
                    "schema_restore_stale",
                    format!(
                        "restore span {} no longer matches its expected bytes",
                        span.key_path
                    ),
                    None,
                    Some(span.key_path.clone()),
                ));
            }
            if let Some(prior) = &span.prior {
                if has_yaml_feature(&split_lines(prior)) {
                    return Err(error(
                        "schema_restore_invalid",
                        "restore span contains an anchor, alias, or tag",
                        None,
                        Some(span.key_path.clone()),
                    ));
                }
                overrides.insert(span.key_path.clone(), prior.clone());
            }
        }
    }
    if let Some(span) = restoration.and_then(|restoration| {
        restoration
            .spans
            .iter()
            .find(|span| span.key_path == "walk_by" && span.prior.is_some())
    }) {
        return apply_walk_by_prior(markdown, span);
    }
    // When the restoration explicitly expects absence (`expected: null`)
    // with no prior, the caller already validated the current span is
    // absent; fall through to the desired-state patch which must also be
    // absent for a valid undo.
    let mut doc = document(markdown)?;
    let entries = block_entries(&doc.lines, 1, doc.close, 0)?;
    let existing = entries.iter().find(|entry| entry.key == "walk_by");
    match (existing, desired) {
        (None, None) => Ok(markdown.to_owned()),
        (Some(entry), None) => {
            // Remove the whole entry span; unrelated lines stay byte-identical.
            let mut lines = doc.lines.clone();
            lines.drain(entry.start..=entry.end);
            // Recompute the closing delimiter after removal.
            doc.close = lines
                .iter()
                .enumerate()
                .skip(1)
                .find_map(|(index, line)| (line.text == "---").then_some(index))
                .ok_or_else(|| {
                    error(
                        "span_internal",
                        "closing delimiter disappeared",
                        None,
                        None,
                    )
                })?;
            doc.lines = lines;
            Ok(join_lines(&doc.lines))
        }
        (None, Some(value)) => {
            let rendered = render_entry(
                "walk_by",
                &YamlValue::String(value.to_owned()),
                0,
                &doc.newline,
            );
            let close = doc.close;
            let rendered_len = rendered.len();
            let mut lines = doc.lines.clone();
            lines.splice(close..close, rendered);
            let _ = rendered_len;
            Ok(join_lines(&lines))
        }
        (Some(entry), Some(value)) => {
            let current_inline = line_value(&doc.lines, entry)?.0;
            // Compare decoded scalar text to avoid rewriting identical values
            // with different quoting; the caller treats semantic no-ops
            // before reaching here, but per-op idempotence still holds.
            let current_value = parse_top_level_scalar(&doc.lines, entry);
            if current_value.as_deref() == Some(value) && overrides.is_empty() {
                // Preserve original quoting/comments when the value is
                // already correct.
                return Ok(markdown.to_owned());
            }
            let _ = current_inline;
            let new_value = YamlValue::String(value.to_owned());
            let patched = replace_entry_value(
                &doc.lines,
                entry,
                &new_value,
                &doc.newline,
            );
            let mut lines = doc.lines.clone();
            lines.splice(entry.start..=entry.end, patched);
            Ok(join_lines(&lines))
        }
    }
}

fn parse_top_level_scalar(lines: &[Line], entry: &Entry) -> Option<String> {
    let (inline, _) = line_value(lines, entry).ok()?;
    if inline.is_empty() {
        return None;
    }
    match serde_yaml::from_str::<YamlValue>(&inline) {
        Ok(YamlValue::String(text)) => Some(text),
        Ok(YamlValue::Null) => None,
        _ => None,
    }
}

fn apply_walk_by_prior(
    markdown: &str,
    span: &RestoreSpan,
) -> Result<String, SpanError> {
    let prior = span.prior.as_deref().ok_or_else(|| {
        error(
            "schema_restore_invalid",
            "restore entry is empty",
            None,
            Some("walk_by".to_owned()),
        )
    })?;
    if prior.trim().is_empty() {
        return Err(error(
            "schema_restore_invalid",
            "restore entry is empty",
            None,
            Some("walk_by".to_owned()),
        ));
    }
    if has_yaml_feature(&split_lines(prior)) {
        return Err(error(
            "schema_restore_invalid",
            "restore span contains an anchor, alias, or tag",
            None,
            Some("walk_by".to_owned()),
        ));
    }
    let fragment = split_lines(prior);
    let first = fragment
        .iter()
        .position(|line| !blank_or_comment(&line.text))
        .ok_or_else(|| {
            error(
                "schema_restore_invalid",
                "restore entry is empty",
                None,
                Some("walk_by".to_owned()),
            )
        })?;
    let indent = indentation(&fragment[first].text);
    let (key, _) =
        parse_key(&fragment[first].text, indent).ok_or_else(|| {
            error(
                "schema_restore_invalid",
                "restore bytes do not begin with a YAML key",
                None,
                Some("walk_by".to_owned()),
            )
        })?;
    if key != "walk_by" || indent != 0 {
        return Err(error(
            "schema_restore_invalid",
            "restore bytes do not match the resolved schema key and indentation",
            None,
            Some("walk_by".to_owned()),
        ));
    }
    let top = block_entries(&fragment, first, fragment.len(), indent)?;
    if top.len() != 1 {
        return Err(error(
            "schema_restore_invalid",
            "restore bytes must contain exactly one schema entry",
            None,
            Some("walk_by".to_owned()),
        ));
    }
    // Validate through the generic fragment checker for consistent errors.
    let doc = document(markdown)?;
    let entries = block_entries(&doc.lines, 1, doc.close, 0)?;
    let existing = entries.iter().find(|entry| entry.key == "walk_by");
    match existing {
        Some(entry) => {
            let patched = validate_restore_fragment(prior, entry, "walk_by")?;
            let mut lines = doc.lines.clone();
            lines.splice(entry.start..=entry.end, patched);
            Ok(join_lines(&lines))
        }
        None => {
            let rendered =
                validate_absent_restore(prior, "walk_by", 0, "walk_by")?;
            let mut lines = doc.lines.clone();
            let insert_at = insertion_index(&entries, doc.close, span)?;
            lines.splice(insert_at..insert_at, rendered);
            Ok(join_lines(&lines))
        }
    }
}

/// Independent parse of the emitted album's effective `walk_by` selection.
/// BOM is stripped at this layer so discovery's exact-`---` rule stays
/// unchanged while span edits of BOM documents still bind to the
/// validated post-state.
fn emitted_walk_selection(
    markdown: &str,
    stem: &str,
) -> Result<Option<String>, SpanError> {
    let stripped = markdown.trim_start_matches('\u{feff}');
    let relative = format!("{stem}.md");
    Ok(crate::native::albums::parse_album_declaration(
        Path::new(&relative),
        stripped,
    )
    .and_then(|declaration| declaration.walk_by))
}

/// Line index at which to insert a restored absent `walk_by` entry.
/// Omitted locators keep the legacy insert-at-close behavior.
fn insertion_index(
    entries: &[Entry],
    close: usize,
    span: &RestoreSpan,
) -> Result<usize, SpanError> {
    if span.after.is_none() && span.before.is_none() {
        return Ok(close);
    }
    let after_idx: Option<isize> = match &span.after {
        None => None,
        Some(None) => Some(-1),
        Some(Some(key)) => Some(
            entries
                .iter()
                .position(|entry| &entry.key == key)
                .ok_or_else(|| {
                    error(
                        "schema_restore_stale",
                        format!(
                            "restore after neighbor {key:?} is not a current top-level key"
                        ),
                        None,
                        Some("walk_by".to_owned()),
                    )
                })? as isize,
        ),
    };
    let before_idx: Option<isize> = match &span.before {
        None => None,
        Some(None) => Some(entries.len() as isize),
        Some(Some(key)) => Some(
            entries
                .iter()
                .position(|entry| &entry.key == key)
                .ok_or_else(|| {
                    error(
                        "schema_restore_stale",
                        format!(
                            "restore before neighbor {key:?} is not a current top-level key"
                        ),
                        None,
                        Some("walk_by".to_owned()),
                    )
                })? as isize,
        ),
    };
    let slot = match (after_idx, before_idx) {
        (Some(after), Some(before)) => {
            if after + 1 != before {
                return Err(error(
                    "schema_restore_stale",
                    "restore locators are not adjacent top-level neighbors",
                    None,
                    Some("walk_by".to_owned()),
                ));
            }
            after + 1
        }
        (Some(after), None) => after + 1,
        (None, Some(before)) => before,
        (None, None) => return Ok(close),
    };
    if slot <= 0 {
        Ok(1)
    } else if slot as usize >= entries.len() {
        Ok(close)
    } else {
        Ok(entries[slot as usize].start)
    }
}

fn patch_record_mapping(
    markdown: &str,
    before: &Mapping,
    after: &Mapping,
    op: &Op,
    restoration: Option<&Restoration>,
) -> Result<String, SpanError> {
    let (mut doc, parsed_before) = parse_record_mapping(markdown)?;
    if &parsed_before != before {
        return Err(error(
            "schema_span_stale",
            "current record bytes do not match the operation's raw schema preimage",
            None,
            Some("record".to_owned()),
        ));
    }
    let mut overrides = HashMap::<String, String>::new();
    if let Some(restoration) = restoration {
        validate_restoration_for_op(op, restoration)?;
        for span in &restoration.spans {
            let actual = exact_span(markdown, &span.key_path)?;
            if actual != span.expected {
                return Err(error(
                    "schema_restore_stale",
                    format!(
                        "restore span {} no longer matches its expected bytes",
                        span.key_path
                    ),
                    None,
                    Some(span.key_path.clone()),
                ));
            }
            if let Some(prior) = &span.prior {
                if has_yaml_feature(&split_lines(prior)) {
                    return Err(error(
                        "schema_restore_invalid",
                        "restore span contains an anchor, alias, or tag",
                        None,
                        Some(span.key_path.clone()),
                    ));
                }
                overrides.insert(span.key_path.clone(), prior.clone());
            }
        }
    }
    let yaml_root = yaml_text(&doc.lines[1..doc.close]);
    let root: YamlValue =
        serde_yaml::from_str(&yaml_root).map_err(|yaml_error| {
            error(
                "unsupported_yaml",
                format!("frontmatter parse failed: {yaml_error}"),
                None,
                None,
            )
        })?;
    let old_root = root
        .as_mapping()
        .expect("validated frontmatter mapping")
        .clone();
    let mut new_root = old_root.clone();
    new_root.insert(
        YamlValue::String("record".to_owned()),
        YamlValue::Mapping(after.clone()),
    );
    let patched = patch_mapping(
        &doc.lines,
        1,
        doc.close,
        0,
        &old_root,
        &new_root,
        "",
        &doc.newline,
        &overrides,
    )?;
    doc.lines.splice(1..doc.close, patched);
    // Locate the closing delimiter again because schema lines may have been
    // inserted or removed. Its line identity is outside the edited span.
    doc.close = doc
        .lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.text == "---").then_some(index))
        .ok_or_else(|| {
            error("span_internal", "closing delimiter disappeared", None, None)
        })?;
    let postimage = join_lines(&doc.lines);
    let (_, actual_after) = parse_record_mapping(&postimage)?;
    if let Some(restoration) = restoration {
        for span in &restoration.spans {
            if exact_span(&postimage, &span.key_path)? != span.prior {
                return Err(error(
                    "schema_restore_invalid",
                    format!("restore bytes do not match the requested prior span {}", span.key_path),
                    None,
                    Some(span.key_path.clone()),
                ));
            }
        }
    }
    // A restoration can temporarily reintroduce attributes that a later
    // inverse op in the same envelope restores semantically. The caller
    // checks the complete envelope against `OpOutcome::after_raw`.
    let _ = actual_after;
    Ok(postimage)
}

#[allow(clippy::too_many_arguments)]
fn patch_mapping(
    lines: &[Line],
    start: usize,
    end: usize,
    indent: usize,
    old: &Mapping,
    new: &Mapping,
    parent_path: &str,
    newline: &str,
    overrides: &HashMap<String, String>,
) -> Result<Vec<Line>, SpanError> {
    let entries = block_entries(lines, start, end, indent)?;
    ensure_entries_match(entries.clone(), old, parent_path)?;
    let first = entries.first().map_or(end, |entry| entry.key_line);
    let mut output = lines[start..first].to_vec();
    let mut by_key = HashMap::<String, Entry>::new();
    for entry in entries {
        by_key.insert(entry.key.clone(), entry);
    }
    for (key_value, new_value) in new {
        let key = key_value.as_str().ok_or_else(|| {
            error(
                "unsupported_yaml",
                "schema mapping keys must be strings",
                None,
                None,
            )
        })?;
        let path = if parent_path.is_empty() {
            key.to_owned()
        } else {
            format!("{parent_path}.{key}")
        };
        if let Some(entry) = by_key.get(key) {
            let old_value = old
                .get(YamlValue::String(key.to_owned()))
                .expect("old mapping keys were checked");
            let raw = patch_entry(
                lines, entry, old_value, new_value, &path, newline, overrides,
            )?;
            output.extend(raw);
        } else if let Some(prior) = overrides.get(&path) {
            // Ordinary inverse insertion cannot restore prior bytes through
            // canonical emission; emit the server-resolved prior entry
            // directly after validating its key and indentation.
            output.extend(validate_absent_restore(prior, key, indent, &path)?);
        } else {
            output.extend(render_entry(key, new_value, indent, newline));
        }
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn patch_entry(
    lines: &[Line],
    entry: &Entry,
    old: &YamlValue,
    new: &YamlValue,
    path: &str,
    newline: &str,
    overrides: &HashMap<String, String>,
) -> Result<Vec<Line>, SpanError> {
    if let Some(prior) = overrides.get(path) {
        return validate_restore_fragment(prior, entry, path);
    }
    // serde_yaml::Mapping equality is key/value based and ignores insertion
    // order. Schema field and choice order is user-visible, so a pure reorder
    // must still patch the original entry slices into the new order.
    if ordered_yaml_eq(old, new) {
        return Ok(lines[entry.start..=entry.end].to_vec());
    }
    let (inline, _) = line_value(lines, entry)?;
    // Flow empty mappings (`fields: {}`) cannot be patched through the block
    // path above, yet inverse restores need their nested absent entries.
    // When overrides name nested paths under this entry, rebuild the block
    // from server-resolved priors instead of canonical emission.
    if let (Some(old_map), Some(new_map)) = (old.as_mapping(), new.as_mapping())
    {
        let has_nested_override = overrides
            .keys()
            .any(|key| key.starts_with(&format!("{path}.")));
        if has_nested_override && !new_map.is_empty() && inline.trim() == "{}" {
            let child_indent = entry.indent + 2;
            let mut result = lines[entry.key_line..entry.key_line + 1].to_vec();
            // Strip the flow `{}` so the block children have a clean parent.
            // Block parents carry no trailing gap; comments keep their
            // leading whitespace via `trailing`.
            {
                let key_line = &mut result[0];
                let prefix = &key_line.text[..entry.colon + 1];
                let (_, comment) =
                    split_comment(&key_line.text[entry.colon + 1..]);
                let trailing = whitespace_before_comment(
                    &key_line.text[entry.colon + 1..],
                );
                key_line.text = format!("{prefix}{trailing}{comment}");
                if key_line.ending.is_empty() {
                    key_line.ending = newline.to_owned();
                }
            }
            for (key_value, new_value) in new_map {
                let key = key_value.as_str().ok_or_else(|| {
                    error(
                        "unsupported_yaml",
                        "schema mapping keys must be strings",
                        None,
                        None,
                    )
                })?;
                let child_path = format!("{path}.{key}");
                if let Some(prior) = overrides.get(&child_path) {
                    // Nested absent entries restore whole subtrees; deeper
                    // overrides under the same path are contained in `prior`.
                    result.extend(validate_absent_restore(
                        prior,
                        key,
                        child_indent,
                        &child_path,
                    )?);
                } else {
                    result.extend(render_entry(
                        key,
                        new_value,
                        child_indent,
                        newline,
                    ));
                }
            }
            // Ensure the old flow value does not leak into the new block.
            let _ = old_map;
            return Ok(result);
        }
    }
    if let (Some(old_map), Some(new_map)) = (old.as_mapping(), new.as_mapping())
        && inline.is_empty()
    {
        let child_start = entry.key_line + 1;
        let child_indent =
            first_content_indent(lines, child_start, entry.end + 1)
                .unwrap_or(entry.indent + 2);
        let children = patch_mapping(
            lines,
            child_start,
            entry.end + 1,
            child_indent,
            old_map,
            new_map,
            path,
            newline,
            overrides,
        )?;
        if !new_map.is_empty() {
            let mut result = lines[entry.key_line..entry.key_line + 1].to_vec();
            if result[0].ending.is_empty() {
                result[0].ending = newline.to_owned();
            }
            result.extend(children);
            return Ok(result);
        }
        let mut key_line = lines[entry.key_line].clone();
        let prefix = &key_line.text[..entry.colon + 1];
        let (_, comment) = split_comment(&key_line.text[entry.colon + 1..]);
        let gap = gap_after_colon(&key_line.text[entry.colon + 1..]);
        let trailing =
            whitespace_before_comment(&key_line.text[entry.colon + 1..]);
        key_line.text = format!("{prefix}{gap}{{}}{trailing}{comment}");
        let mut result = vec![key_line];
        // Preserve standalone comments/blank lines that were not part of a
        // removed entry. They remain inert after the explicit empty map.
        result.extend(
            children
                .into_iter()
                .filter(|line| blank_or_comment(&line.text)),
        );
        return Ok(result);
    }
    if !inline.is_empty()
        && (inline.starts_with('{') || inline.starts_with('['))
    {
        validate_flow_map_keys(&inline, path, entry.key_line + 1)?;
    }
    Ok(replace_entry_value(lines, entry, new, newline))
}

fn ordered_yaml_eq(left: &YamlValue, right: &YamlValue) -> bool {
    match (left, right) {
        (YamlValue::Mapping(left), YamlValue::Mapping(right)) => {
            left.len() == right.len()
                && left.iter().zip(right.iter()).all(
                    |((left_key, left_value), (right_key, right_value))| {
                        left_key == right_key
                            && ordered_yaml_eq(left_value, right_value)
                    },
                )
        }
        (YamlValue::Sequence(left), YamlValue::Sequence(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| ordered_yaml_eq(left, right))
        }
        _ => left == right,
    }
}

fn mappings_equal_ordered(left: &Mapping, right: &Mapping) -> bool {
    ordered_yaml_eq(
        &YamlValue::Mapping(left.clone()),
        &YamlValue::Mapping(right.clone()),
    )
}

fn validate_restore_fragment(
    prior: &str,
    entry: &Entry,
    path: &str,
) -> Result<Vec<Line>, SpanError> {
    let fragment = split_lines(prior);
    let first = fragment
        .iter()
        .position(|line| !blank_or_comment(&line.text))
        .ok_or_else(|| {
            error(
                "schema_restore_invalid",
                "restore entry is empty",
                None,
                Some(path.to_owned()),
            )
        })?;
    let indent = indentation(&fragment[first].text);
    let (key, _) =
        parse_key(&fragment[first].text, indent).ok_or_else(|| {
            error(
                "schema_restore_invalid",
                "restore bytes do not begin with a YAML key",
                None,
                Some(path.to_owned()),
            )
        })?;
    if key != entry.key || indent != entry.indent {
        return Err(error(
            "schema_restore_invalid",
            "restore bytes do not match the resolved schema key and indentation",
            None,
            Some(path.to_owned()),
        ));
    }
    let top = block_entries(&fragment, first, fragment.len(), indent)?;
    if top.len() != 1 {
        return Err(error(
            "schema_restore_invalid",
            "restore bytes must contain exactly one schema entry",
            None,
            Some(path.to_owned()),
        ));
    }
    Ok(fragment)
}

fn validate_absent_restore(
    prior: &str,
    key: &str,
    indent: usize,
    path: &str,
) -> Result<Vec<Line>, SpanError> {
    // Restoration of an absent entry: `expected` was `None` (already checked
    // against the current note) and `prior` carries the exact server-resolved
    // bytes. Check the resolved path, key, and indentation; never trust
    // client offsets.
    if has_yaml_feature(&split_lines(prior)) {
        return Err(error(
            "schema_restore_invalid",
            "restore span contains an anchor, alias, or tag",
            None,
            Some(path.to_owned()),
        ));
    }
    let fragment = split_lines(prior);
    let first = fragment
        .iter()
        .position(|line| !blank_or_comment(&line.text))
        .ok_or_else(|| {
            error(
                "schema_restore_invalid",
                "restore entry is empty",
                None,
                Some(path.to_owned()),
            )
        })?;
    let actual_indent = indentation(&fragment[first].text);
    let (parsed_key, _) = parse_key(&fragment[first].text, actual_indent)
        .ok_or_else(|| {
            error(
                "schema_restore_invalid",
                "restore bytes do not begin with a YAML key",
                None,
                Some(path.to_owned()),
            )
        })?;
    if parsed_key != key || actual_indent != indent {
        return Err(error(
            "schema_restore_invalid",
            "restore bytes do not match the resolved schema key and indentation",
            None,
            Some(path.to_owned()),
        ));
    }
    let top = block_entries(&fragment, first, fragment.len(), indent)?;
    if top.len() != 1 {
        return Err(error(
            "schema_restore_invalid",
            "restore bytes must contain exactly one schema entry",
            None,
            Some(path.to_owned()),
        ));
    }
    Ok(fragment)
}

fn replace_entry_value(
    lines: &[Line],
    entry: &Entry,
    value: &YamlValue,
    newline: &str,
) -> Vec<Line> {
    let mut key_line = lines[entry.key_line].clone();
    let prefix = &key_line.text[..entry.colon + 1];
    let (_, comment) = split_comment(&key_line.text[entry.colon + 1..]);
    if is_nonempty_container(value) {
        // Block parents carry no trailing gap; comments keep their leading
        // whitespace so `values:` never gains a trailing space.
        let trailing =
            whitespace_before_comment(&key_line.text[entry.colon + 1..]);
        key_line.text = format!("{prefix}{trailing}{comment}");
        if key_line.ending.is_empty() {
            key_line.ending = newline.to_owned();
        }
        let mut output = vec![key_line];
        output.extend(render_container(value, entry.indent + 2, newline));
        output
    } else {
        let gap = gap_after_colon(&key_line.text[entry.colon + 1..]);
        let trailing =
            whitespace_before_comment(&key_line.text[entry.colon + 1..]);
        key_line.text =
            format!("{prefix}{gap}{}{trailing}{comment}", render_scalar(value));
        key_line.ending = lines[entry.key_line].ending.clone();
        vec![key_line]
    }
}

fn render_entry(
    key: &str,
    value: &YamlValue,
    indent: usize,
    newline: &str,
) -> Vec<Line> {
    let key = crate::native::records::yaml_string_scalar(key);
    let spaces = " ".repeat(indent);
    if is_nonempty_container(value) {
        let mut result = vec![Line {
            text: format!("{spaces}{key}:"),
            ending: newline.to_owned(),
        }];
        result.extend(render_container(value, indent + 2, newline));
        result
    } else {
        vec![Line {
            text: format!("{spaces}{key}: {}", render_scalar(value)),
            ending: newline.to_owned(),
        }]
    }
}

fn render_container(
    value: &YamlValue,
    indent: usize,
    newline: &str,
) -> Vec<Line> {
    let mut result = Vec::new();
    match value {
        YamlValue::Mapping(mapping) => {
            for (key, value) in mapping {
                let Some(key) = key.as_str() else { continue };
                result.extend(render_entry(key, value, indent, newline));
            }
        }
        YamlValue::Sequence(sequence) => {
            for item in sequence {
                if is_nonempty_container(item) {
                    result.push(Line {
                        text: format!("{}-", " ".repeat(indent)),
                        ending: newline.to_owned(),
                    });
                    result.extend(render_container(item, indent + 2, newline));
                } else {
                    result.push(Line {
                        text: format!(
                            "{}- {}",
                            " ".repeat(indent),
                            render_scalar(item)
                        ),
                        ending: newline.to_owned(),
                    });
                }
            }
        }
        _ => {}
    }
    result
}

fn is_nonempty_container(value: &YamlValue) -> bool {
    match value {
        YamlValue::Mapping(mapping) => !mapping.is_empty(),
        YamlValue::Sequence(sequence) => !sequence.is_empty(),
        _ => false,
    }
}

fn exact_span(
    markdown: &str,
    key_path: &str,
) -> Result<Option<String>, SpanError> {
    let doc = document(markdown)?;
    let parts = key_path.split('.').collect::<Vec<_>>();
    let root_entries = block_entries(&doc.lines, 1, doc.close, 0)?;
    let mut entry = root_entries
        .iter()
        .find(|entry| entry.key == parts[0])
        .cloned();
    for part in parts.iter().skip(1) {
        let Some(parent) = entry.as_ref() else {
            return Ok(None);
        };
        let (inline, _) = line_value(&doc.lines, parent)?;
        if !inline.is_empty() {
            return Ok(None);
        }
        let indent = first_content_indent(
            &doc.lines,
            parent.key_line + 1,
            parent.end + 1,
        )
        .unwrap_or(parent.indent + 2);
        let children = block_entries(
            &doc.lines,
            parent.key_line + 1,
            parent.end + 1,
            indent,
        )?;
        entry = children.into_iter().find(|child| child.key == *part);
    }
    Ok(entry.map(|entry| join_lines(&doc.lines[entry.start..=entry.end])))
}

fn validate_restoration_for_op(
    op: &Op,
    restoration: &Restoration,
) -> Result<(), SpanError> {
    let allowed = match op {
        Op::WalkBySet { .. } => restoration
            .spans
            .iter()
            .all(|span| span.key_path == "walk_by"),
        Op::AlbumSet { set, unset } => {
            let keys = set
                .keys()
                .chain(unset.iter())
                .map(|key| format!("record.{key}"))
                .collect::<HashSet<_>>();
            restoration
                .spans
                .iter()
                .all(|span| keys.contains(&span.key_path))
        }
        Op::FieldAdd {
            restore_key: Some(field),
            ..
        } => restoration
            .spans
            .iter()
            .all(|span| span.key_path == format!("record.fields.{field}")),
        Op::FieldSet { field, set, unset } => {
            let keys = set
                .keys()
                .chain(unset.iter())
                .map(|key| format!("record.fields.{field}.{key}"))
                .collect::<HashSet<_>>();
            restoration.spans.iter().all(|span| {
                keys.contains(&span.key_path)
                    || span.key_path == format!("record.fields.{field}")
            })
        }
        Op::ChoiceAdd {
            field,
            restore_value: Some(value),
            ..
        } => restoration.spans.iter().all(|span| {
            span.key_path == format!("record.fields.{field}.values.{value}")
        }),
        Op::ChoiceSet { field, value, .. } => {
            restoration.spans.iter().all(|span| {
                span.key_path == format!("record.fields.{field}.values.{value}")
            })
        }
        _ => false,
    };
    if !allowed {
        return Err(error(
            "schema_restore_invalid",
            "restoration metadata does not belong to this operation",
            None,
            restoration.spans.first().map(|span| span.key_path.clone()),
        ));
    }
    for (index, left) in restoration.spans.iter().enumerate() {
        for right in restoration.spans.iter().skip(index + 1) {
            if left.key_path.starts_with(&format!("{}.", right.key_path))
                || right.key_path.starts_with(&format!("{}.", left.key_path))
            {
                return Err(error(
                    "schema_restore_invalid",
                    "restoration paths may not overlap",
                    None,
                    Some(left.key_path.clone()),
                ));
            }
        }
    }
    Ok(())
}

fn build_inverse_ops(steps: &[Step]) -> Vec<RestoredOp> {
    let mut result = Vec::new();
    for step in steps.iter().rev() {
        for (index, op) in step.inverse.iter().enumerate() {
            let paths = inverse_paths(op, &step.inverse[..index]);
            let spans = paths
                .into_iter()
                .filter_map(|key_path| {
                    let expected =
                        exact_span(&step.after_text, &key_path).ok().flatten();
                    let prior =
                        exact_span(&step.before_text, &key_path).ok().flatten();
                    (expected != prior).then_some({
                        let (after, before) = if key_path == "walk_by"
                            && prior.is_some()
                            && expected.is_none()
                        {
                            walk_by_neighbors(&step.before_text)
                        } else {
                            (None, None)
                        };
                        RestoreSpan {
                            key_path,
                            expected,
                            prior,
                            after,
                            before,
                        }
                    })
                })
                .collect::<Vec<_>>();
            result.push(RestoredOp {
                op: op.clone(),
                restore: (!spans.is_empty()).then_some(Restoration { spans }),
            });
        }
    }
    result
}

fn walk_by_neighbors(
    markdown: &str,
) -> (Option<Option<String>>, Option<Option<String>>) {
    let Ok(doc) = document(markdown) else {
        return (None, None);
    };
    let Ok(entries) = block_entries(&doc.lines, 1, doc.close, 0) else {
        return (None, None);
    };
    let Some(index) = entries.iter().position(|entry| entry.key == "walk_by")
    else {
        return (None, None);
    };
    let after = if index == 0 {
        Some(None)
    } else {
        Some(Some(entries[index - 1].key.clone()))
    };
    let before = if index + 1 == entries.len() {
        Some(None)
    } else {
        Some(Some(entries[index + 1].key.clone()))
    };
    (after, before)
}

fn inverse_paths(op: &Op, earlier: &[Op]) -> Vec<String> {
    match op {
        Op::WalkBySet { .. } => vec!["walk_by".to_owned()],
        Op::AlbumSet { set, unset } => {
            let mut keys =
                set.keys().chain(unset.iter()).cloned().collect::<Vec<_>>();
            keys.sort();
            keys.dedup();
            keys.into_iter()
                .map(|key| format!("record.{key}"))
                .collect()
        }
        Op::FieldAdd {
            restore_key: Some(field),
            ..
        } => vec![format!("record.fields.{field}")],
        Op::FieldSet { field, set, unset } => {
            if earlier.iter().any(|item| matches!(item, Op::FieldAdd { restore_key: Some(key), .. } if key == field)) {
                return Vec::new();
            }
            let mut keys =
                set.keys().chain(unset.iter()).cloned().collect::<Vec<_>>();
            keys.sort();
            keys.dedup();
            keys.into_iter()
                .map(|key| format!("record.fields.{field}.{key}"))
                .collect()
        }
        Op::ChoiceAdd {
            field,
            restore_value: Some(value),
            ..
        } => vec![format!("record.fields.{field}.values.{value}")],
        Op::ChoiceSet { field, value, .. } => {
            vec![format!("record.fields.{field}.values.{value}")]
        }
        _ => Vec::new(),
    }
}

fn op_error(error: OpError) -> SpanError {
    SpanError {
        code: error.code,
        message: error.message,
        line: None,
        key_path: error.key_path,
    }
}

fn error(
    code: &'static str,
    message: impl Into<String>,
    line: Option<usize>,
    key_path: Option<String>,
) -> SpanError {
    SpanError {
        code,
        message: message.into(),
        line,
        key_path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{
        albums::update::{
            ops::{ApplyInputs, IndexRefs, OtherAlbumField},
            protocol::{Op, Restoration, RestoreSpan},
        },
        records::{catalog::RecordCatalog, schema::parse_album_record_schema},
    };
    use chrono::NaiveDate;
    use std::collections::HashMap;

    fn apply(
        markdown: &str,
        operations: &[Op],
    ) -> (
        SchemaSpanEdit,
        Mapping,
        crate::native::records::schema::RecordType,
    ) {
        let (_, raw) =
            parse_record_mapping(markdown).expect("source schema parses");
        let parsed = parse_album_record_schema(
            Some(&YamlValue::Mapping(raw.clone())),
            Path::new("eat.md"),
            "eat",
            None,
            &HashMap::new(),
            1,
        );
        let record_type = parsed.record_type.expect("fixture schema is valid");
        let temp = tempfile::tempdir().expect("folder root");
        let mut catalog = RecordCatalog::new(temp.path());
        let folder = catalog.folder("eat");
        let index_refs = refs_from(markdown);
        let other_fields: Vec<OtherAlbumField> = Vec::new();
        let inputs = ApplyInputs {
            before_raw: &raw,
            before_type: &record_type,
            before_plural: None,
            stem: "eat",
            folder,
            index_refs: &index_refs,
            other_fields: &other_fields,
            today: NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
        };
        let outcome =
            ops::apply_ops(operations, &inputs).expect("ops validate");
        let restorations = vec![None; operations.len()];
        let edit = apply_schema_ops(
            markdown,
            operations,
            &restorations,
            &outcome,
            &inputs,
        )
        .expect("schema spans apply");
        (edit, raw, record_type)
    }

    fn inverse_roundtrip(
        original: &str,
        edit: &SchemaSpanEdit,
        before_raw: &Mapping,
        before_type: &crate::native::records::schema::RecordType,
    ) {
        let inverse = edit
            .inverse_ops
            .iter()
            .map(|item| item.op.clone())
            .collect::<Vec<_>>();
        let restorations = edit
            .inverse_ops
            .iter()
            .map(|item| item.restore.clone())
            .collect::<Vec<_>>();
        let parsed = parse_record_mapping(&edit.contents).unwrap().1;
        let parsed_type = parse_album_record_schema(
            Some(&YamlValue::Mapping(parsed.clone())),
            Path::new("eat.md"),
            "eat",
            None,
            &HashMap::new(),
            1,
        )
        .record_type
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut catalog = RecordCatalog::new(temp.path());
        let folder = catalog.folder("eat");
        let refs = refs_from(&edit.contents);
        let others: Vec<OtherAlbumField> = Vec::new();
        let inputs = ApplyInputs {
            before_raw: &parsed,
            before_type: &parsed_type,
            before_plural: None,
            stem: "eat",
            folder,
            index_refs: &refs,
            other_fields: &others,
            today: NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
        };
        let inverse_outcome =
            ops::apply_ops(&inverse, &inputs).expect("inverse validates");
        let restored = apply_schema_ops(
            &edit.contents,
            &inverse,
            &restorations,
            &inverse_outcome,
            &inputs,
        )
        .expect("inverse spans apply");
        assert_eq!(restored.contents, original);
        let (_, restored_raw) =
            parse_record_mapping(&restored.contents).unwrap();
        assert_eq!(&restored_raw, before_raw);
        let restored_type = parse_album_record_schema(
            Some(&YamlValue::Mapping(restored_raw)),
            Path::new("eat.md"),
            "eat",
            None,
            &HashMap::new(),
            1,
        )
        .record_type
        .unwrap();
        assert_eq!(&restored_type, before_type);
    }

    fn refs_from(markdown: &str) -> IndexRefs {
        crate::native::albums::parse_album_declaration(
            Path::new("eat.md"),
            markdown.trim_start_matches('\u{feff}'),
        )
        .map(|declaration| IndexRefs {
            index_by: declaration.index_by,
            index_group_by: declaration.index_group_by,
            index_facets: declaration.index_facets,
            walk_by: declaration.walk_by,
            walk_by_authored: declaration.walk_by_authored,
        })
        .unwrap_or_default()
    }

    fn apply_restored(
        markdown: &str,
        operations: &[Op],
        restorations: &[Option<Restoration>],
    ) -> Result<SchemaSpanEdit, SpanError> {
        let (_, raw) =
            parse_record_mapping(markdown).expect("source schema parses");
        let parsed = parse_album_record_schema(
            Some(&YamlValue::Mapping(raw.clone())),
            Path::new("eat.md"),
            "eat",
            None,
            &HashMap::new(),
            1,
        );
        let record_type = parsed.record_type.expect("fixture schema is valid");
        let temp = tempfile::tempdir().expect("folder root");
        let mut catalog = RecordCatalog::new(temp.path());
        let folder = catalog.folder("eat");
        let index_refs = refs_from(markdown);
        let other_fields: Vec<OtherAlbumField> = Vec::new();
        let inputs = ApplyInputs {
            before_raw: &raw,
            before_type: &record_type,
            before_plural: None,
            stem: "eat",
            folder,
            index_refs: &index_refs,
            other_fields: &other_fields,
            today: NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
        };
        let outcome =
            ops::apply_ops(operations, &inputs).expect("ops validate");
        apply_schema_ops(markdown, operations, restorations, &outcome, &inputs)
    }

    fn album_with_walk(walk_line: &str) -> String {
        format!(
            concat!(
                "---\n",
                "type: [[album]]\n",
                "parent: \"[[body]]\"\n",
                "summary: Places.\n",
                "{walk}",
                "record:\n",
                "  noun: restaurant\n",
                "  label: Restaurant\n",
                "  fields:\n",
                "    status:\n",
                "      values:\n",
                "        to_try: To try\n",
                "        liked: Liked\n",
                "---\n",
            ),
            walk = walk_line,
        )
    }

    #[test]
    fn no_op_keeps_original_bytes_and_index_placement() {
        let markdown = "\u{feff}---\r\ntype: [[album]]\r\nsummary: Keep me\r\nrecord:\r\n  noun: place\r\n  label: Place\r\n  fields:\r\n    status:\r\n      values:\r\n        ready: Ready\r\n---\r\n\r\n## Records\r\n> [!records]\r\n";
        let (doc, raw) =
            parse_record_mapping(markdown).expect("fixture parses");
        assert_eq!(raw.len(), 3);
        assert_eq!(join_lines(&doc.lines), markdown);
    }

    #[test]
    fn quoted_punctuation_is_not_an_anchor_token() {
        let markdown = "---\nrecord:\n  noun: place\n  label: \"Place &home *text !tag\"\n  fields: {}\n---\n";
        assert!(parse_record_mapping(markdown).is_ok());
    }

    #[test]
    fn duplicate_record_keys_refuse() {
        let markdown = "---\nrecord:\n  noun: place\n  label: Place\n  fields:\n    status: {}\n    status: {}\n---\n";
        let error = parse_record_mapping(markdown).expect_err("duplicate key");
        assert_eq!(error.code, "unsupported_yaml");
    }

    #[test]
    fn aliases_inside_record_refuse_but_external_plain_comment_is_safe() {
        let markdown = "---\nsummary: &summary Before\nrecord:\n  noun: place\n  label: *summary\n  fields: {}\n---\n";
        let error =
            parse_record_mapping(markdown).expect_err("cross-boundary alias");
        assert_eq!(error.code, "unsupported_yaml");
    }

    #[test]
    fn field_value_edit_preserves_comments_and_inverse_restores_exact_quote_bytes(
    ) {
        let original = concat!(
            "\u{feff}---\r\n",
            "type: [[album]]\r\n",
            "summary: Outside schema # keep this\r\n",
            "record:\r\n",
            "  noun: place\r\n",
            "  label: \"Place\" # keep record comment\r\n",
            "  fields:\r\n",
            "    status:\r\n",
            "      label: \"Status\" # keep field comment\r\n",
            "      values:\r\n",
            "        ready: \"Ready\" # keep choice comment\r\n",
            "---\r\n\r\n",
            "## Records\r\n",
        );
        let operations = [Op::FieldSet {
            field: "status".to_owned(),
            set: HashMap::from([(
                "label".to_owned(),
                serde_json::json!("State"),
            )]),
            unset: Vec::new(),
        }];
        let (edit, raw, ty) = apply(original, &operations);
        assert!(edit
            .contents
            .contains("label: State # keep field comment\r\n"));
        assert!(edit
            .contents
            .contains("summary: Outside schema # keep this\r\n"));
        assert!(edit.contents.contains("## Records\r\n"));
        assert_eq!(edit.inverse_ops.len(), 1);
        assert!(edit.inverse_ops[0].restore.is_some());
        inverse_roundtrip(original, &edit, &raw, &ty);
    }

    #[test]
    fn field_and_choice_moves_carry_original_entry_slices() {
        let original = concat!(
            "---\nrecord:\n  noun: place\n  label: Place\n  fields:\n",
            "    status:\n      values:\n        ready: Ready # ready\n        done: Done # done\n",
            "    location:\n      label: Location # location\n---\n",
        );
        let operations = [
            Op::FieldMove {
                field: "location".to_owned(),
                position: 0,
            },
            Op::ChoiceMove {
                field: "status".to_owned(),
                value: "done".to_owned(),
                position: 0,
            },
        ];
        let (edit, raw, ty) = apply(original, &operations);
        assert!(
            edit.contents.find("location:").unwrap()
                < edit.contents.find("status:").unwrap()
        );
        assert!(
            edit.contents.find("done: Done # done").unwrap()
                < edit.contents.find("ready: Ready # ready").unwrap()
        );
        inverse_roundtrip(original, &edit, &raw, &ty);
    }

    #[test]
    fn inserted_defaults_use_indented_obsidian_sequence_items() {
        let original =
            "---\nrecord:\n  noun: place\n  label: Place\n  fields: {}\n---\n";
        let operations = [
            Op::FieldAdd {
                label: "Keywords".to_owned(),
                kind: "text".to_owned(),
                list: true,
                suggest: false,
                position: None,
                choices: Vec::new(),
                restore_key: None,
                restore_values: None,
            },
            Op::FieldSet {
                field: "keywords".to_owned(),
                set: HashMap::from([(
                    "default".to_owned(),
                    serde_json::json!(["alpha", "beta"]),
                )]),
                unset: Vec::new(),
            },
        ];
        let (edit, raw, ty) = apply(original, &operations);
        assert!(edit
            .contents
            .contains("      default:\n        - alpha\n        - beta\n"));
        let (_, actual) = parse_record_mapping(&edit.contents).unwrap();
        assert!(actual.get(YamlValue::String("fields".to_owned())).is_some());
        inverse_roundtrip(original, &edit, &raw, &ty);
    }

    #[test]
    fn unlabeled_field_and_diverged_choices_round_trip_byte_identical() {
        // Unlabeled text field with unusual quoting and comments.
        let original = concat!(
            "---\nrecord:\n  noun: place\n  label: Place\n  fields:\n",
            "    title_note: # keep field comment\n",
            "      suggest: true # keep suggest\n",
            "---\n",
        );
        let (edit, raw, ty) = apply(
            original,
            &[Op::FieldRemove {
                field: "title_note".to_owned(),
            }],
        );
        inverse_roundtrip(original, &edit, &raw, &ty);

        // Choices whose stored keys differ from their labels.
        let original = concat!(
            "---\nrecord:\n  noun: place\n  label: Place\n  fields:\n",
            "    category:\n",
            "      label: Category\n",
            "      values:\n",
            "        old_key: Display label\n",
            "        other_key: Another label\n",
            "---\n",
        );
        let (edit, raw, ty) = apply(
            original,
            &[Op::FieldRemove {
                field: "category".to_owned(),
            }],
        );
        assert_eq!(edit.inverse_ops.len(), 1);
        inverse_roundtrip(original, &edit, &raw, &ty);
    }

    #[test]
    fn net_semantic_noop_returns_original_bytes() {
        let original = concat!(
            "---\nrecord:\n  noun: place\n  label: Place\n  fields:\n",
            "    title_note:\n",
            "      label: \"Original\"\n",
            "---\n",
        );
        let operations = [
            Op::FieldSet {
                field: "title_note".to_owned(),
                set: HashMap::from([(
                    "label".to_owned(),
                    serde_json::json!("Temporary"),
                )]),
                unset: Vec::new(),
            },
            Op::FieldSet {
                field: "title_note".to_owned(),
                set: HashMap::from([(
                    "label".to_owned(),
                    serde_json::json!("Original"),
                )]),
                unset: Vec::new(),
            },
        ];
        let (edit, _, _) = apply(original, &operations);
        assert!(!edit.changed, "cancelling batch is a net no-op");
        assert_eq!(edit.contents, original);
        assert!(edit.inverse_ops.is_empty());
    }

    #[test]
    fn tampered_restore_cannot_change_unrelated_fields() {
        let original = concat!(
            "---\nrecord:\n  noun: place\n  label: Place\n  fields:\n",
            "    status:\n",
            "      values:\n",
            "        ready: Ready\n",
            "---\n",
        );
        let (edit, _, _) = apply(
            original,
            &[Op::FieldRemove {
                field: "status".to_owned(),
            }],
        );
        // Tamper the restoration to target an unrelated field.
        let mut tampered = edit.inverse_ops.clone();
        if let Some(first) = tampered.first_mut()
            && let Some(restoration) = first.restore.as_mut()
            && let Some(span) = restoration.spans.first_mut()
        {
            span.key_path = "record.fields.other".to_owned();
        }
        let inverse: Vec<Op> =
            tampered.iter().map(|item| item.op.clone()).collect();
        let restorations: Vec<Option<Restoration>> =
            tampered.iter().map(|item| item.restore.clone()).collect();
        let parsed = parse_record_mapping(&edit.contents).unwrap().1;
        let parsed_type = parse_album_record_schema(
            Some(&YamlValue::Mapping(parsed.clone())),
            Path::new("eat.md"),
            "eat",
            None,
            &HashMap::new(),
            1,
        )
        .record_type
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut catalog = RecordCatalog::new(temp.path());
        let folder = catalog.folder("eat");
        let inputs = ApplyInputs {
            before_raw: &parsed,
            before_type: &parsed_type,
            before_plural: None,
            stem: "eat",
            folder,
            index_refs: &IndexRefs::default(),
            other_fields: &[],
            today: NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
        };
        let outcome =
            ops::apply_ops(&inverse, &inputs).expect("inverse validates");
        let tampered_result = apply_schema_ops(
            &edit.contents,
            &inverse,
            &restorations,
            &outcome,
            &inputs,
        );
        assert!(tampered_result.is_err(), "tampered restoration must refuse");
        assert_eq!(tampered_result.unwrap_err().code, "schema_restore_invalid");
    }

    #[test]
    fn quoted_commented_walk_by_roundtrips_in_place() {
        let original = album_with_walk("walk_by: \"status\" # authored\n");
        let (edit, raw, ty) = apply(
            &original,
            &[Op::WalkBySet {
                field: Some("status".to_owned()),
            }],
        );
        assert!(!edit.changed, "quoted equivalent is a semantic no-op");
        assert_eq!(edit.contents, original);
        inverse_roundtrip(&original, &edit, &raw, &ty);
    }

    #[test]
    fn explicit_unset_removes_an_invalid_authored_key() {
        let original = album_with_walk("walk_by: missing\n");
        let (edit, raw, ty) =
            apply(&original, &[Op::WalkBySet { field: None }]);
        assert!(edit.changed);
        assert!(
            !edit.contents.contains("walk_by"),
            "invalid authored key is removed: {}",
            edit.contents
        );
        inverse_roundtrip(&original, &edit, &raw, &ty);
        let locator = edit.inverse_ops[0]
            .restore
            .as_ref()
            .expect("unset inverse restores bytes")
            .spans[0]
            .clone();
        assert_eq!(locator.after, Some(Some("summary".to_owned())));
        assert_eq!(locator.before, Some(Some("record".to_owned())));
    }

    #[test]
    fn unset_inverse_restores_walk_by_between_neighbors() {
        let original = album_with_walk("walk_by: status\n");
        let (edit, raw, ty) =
            apply(&original, &[Op::WalkBySet { field: None }]);
        assert!(!edit.contents.contains("walk_by"), "{}", edit.contents);
        inverse_roundtrip(&original, &edit, &raw, &ty);
        assert!(
            original.contains("summary: Places.\nwalk_by: status\nrecord:"),
            "{original}"
        );
    }

    #[test]
    fn walk_by_set_then_unset_is_byte_identical() {
        let original = album_with_walk("");
        let (set, raw, ty) = apply(
            &original,
            &[Op::WalkBySet {
                field: Some("status".to_owned()),
            }],
        );
        assert!(set.contents.contains("walk_by: status"));
        inverse_roundtrip(&original, &set, &raw, &ty);
        let (unset, _, _) =
            apply(&set.contents, &[Op::WalkBySet { field: None }]);
        assert_eq!(unset.contents, original);
    }

    #[test]
    fn mismatching_walk_restore_refuses_without_writing() {
        let original = album_with_walk("");
        let restorations = vec![Some(Restoration {
            spans: vec![RestoreSpan {
                key_path: "walk_by".to_owned(),
                expected: None,
                prior: Some("walk_by: bogus\n".to_owned()),
                after: None,
                before: None,
            }],
        })];
        let error = apply_restored(
            &original,
            &[Op::WalkBySet {
                field: Some("status".to_owned()),
            }],
            &restorations,
        )
        .expect_err("malformed restore must refuse");
        assert_eq!(error.code, "span_semantic_mismatch");
    }

    #[test]
    fn stale_walk_restore_on_semantic_noop_still_validates() {
        let original = album_with_walk("");
        let restorations = vec![Some(Restoration {
            spans: vec![RestoreSpan {
                key_path: "walk_by".to_owned(),
                expected: Some("walk_by: bogus\n".to_owned()),
                prior: None,
                after: None,
                before: None,
            }],
        })];
        let error = apply_restored(
            &original,
            &[Op::WalkBySet { field: None }],
            &restorations,
        )
        .expect_err("stale expected on no-op must refuse");
        assert_eq!(error.code, "schema_restore_stale");
    }

    #[test]
    fn invalid_walk_locator_refuses() {
        let original = album_with_walk("");
        let restorations = vec![Some(Restoration {
            spans: vec![RestoreSpan {
                key_path: "walk_by".to_owned(),
                expected: None,
                prior: Some("walk_by: status\n".to_owned()),
                after: Some(Some("missing_neighbor".to_owned())),
                before: Some(Some("record".to_owned())),
            }],
        })];
        let error = apply_restored(
            &original,
            &[Op::WalkBySet {
                field: Some("status".to_owned()),
            }],
            &restorations,
        )
        .expect_err("unknown locator neighbor must refuse");
        assert_eq!(error.code, "schema_restore_stale");
    }

    #[test]
    fn crlf_commented_walk_by_unset_inverse_is_byte_identical() {
        let original = concat!(
            "\u{feff}---\r\n",
            "type: [[album]]\r\n",
            "summary: Places.\r\n",
            "walk_by: \"status\" # authored\r\n",
            "record:\r\n",
            "  noun: restaurant\r\n",
            "  fields:\r\n",
            "    status:\r\n",
            "      values:\r\n",
            "        to_try: To try\r\n",
            "---\r\n",
        );
        let (edit, raw, ty) = apply(original, &[Op::WalkBySet { field: None }]);
        assert!(!edit.contents.contains("walk_by"), "{}", edit.contents);
        inverse_roundtrip(original, &edit, &raw, &ty);
    }
}
