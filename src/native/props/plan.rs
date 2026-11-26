//! Pure property-span planner.
//!
//! `plan` takes current note bytes, a resolved role context, and a parsed
//! envelope. It returns candidate bytes, the post-image report, fingerprints,
//! inverse operations, or a typed refusal. There is no filesystem I/O.

use std::ops::Range;

use serde_yaml::Value as YamlValue;

use super::{
    contract::{expand, validate},
    document::{
        datetime_parts, is_datetime, FrontmatterState, PropertyEntry,
        PropertyKind, PropsDocument,
    },
    emit::{
        emit_key, emit_value, existing_is_short_flow_list, parse_yaml_fragment,
        typed_from_json, yaml_eq,
    },
    inspect,
    protocol::{Envelope, Framing, Op, Position},
    roles::Role,
    show::ShowReport,
    InspectContext, PropsError,
};
use crate::native::yaml_spans;
use serde_json::Value as JsonValue;

/// Successful in-memory plan for one envelope.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlanOutcome {
    pub(crate) candidate: String,
    pub(crate) report: ShowReport,
    pub(crate) fingerprint_before: String,
    pub(crate) fingerprint_after: String,
    pub(crate) changed: bool,
    pub(crate) inverse_ops: Vec<Op>,
    pub(crate) warnings: Vec<PropsError>,
    pub(crate) effects: Vec<super::contract::effects::Effect>,
}

/// Plan ordered operations against `contents` without writing files.
pub(crate) fn plan(
    contents: &str,
    context: &InspectContext,
    envelope: &Envelope,
) -> Result<PlanOutcome, PropsError> {
    let before = inspect(contents, context);
    refuse_unwritable(&before.document, None)?;
    let fingerprint_before =
        before
            .document
            .fingerprint
            .clone()
            .ok_or_else(|| PropsError {
                code: "prop_invalid_yaml".to_owned(),
                message: "fingerprint is unavailable for this note".to_owned(),
                hint: "Repair the frontmatter YAML before editing properties."
                    .to_owned(),
                key: None,
                line: before
                    .document
                    .error
                    .as_ref()
                    .and_then(|error| error.line),
                op_index: None,
            })?;
    if fingerprint_before != envelope.expect {
        return Err(PropsError {
            code: "prop_stale".to_owned(),
            message: "frontmatter fingerprint does not match expect".to_owned(),
            hint: "Re-read the note and retry with a fresh fingerprint."
                .to_owned(),
            key: None,
            line: None,
            op_index: None,
        });
    }
    if envelope.ops.is_empty() {
        return Ok(PlanOutcome {
            candidate: contents.to_owned(),
            report: before.report,
            fingerprint_before: fingerprint_before.clone(),
            fingerprint_after: fingerprint_before,
            changed: false,
            inverse_ops: Vec::new(),
            warnings: Vec::new(),
            effects: Vec::new(),
        });
    }
    if !context.roles.writes_are_safe() {
        return Err(PropsError {
            code: "prop_ownership_unresolved".to_owned(),
            message: context.roles.diagnostics.first().cloned().unwrap_or_else(
                || "property ownership could not be resolved".to_owned(),
            ),
            hint:
                "Resolve note and schema ownership before editing properties."
                    .to_owned(),
            key: None,
            line: None,
            op_index: Some(0),
        });
    }

    let mut current = contents.to_owned();
    let mut inverse_groups = Vec::new();
    let mut expanded_ops = Vec::new();
    let mut warnings = Vec::new();
    for (index, op) in envelope.ops.iter().enumerate() {
        let setting =
            matches!(op, Op::SettingSet { .. } | Op::SettingClear { .. });
        let document = PropsDocument::parse(&current);
        for primitive in expand::expand(context, &document, op, index)? {
            let before_op = PropsDocument::parse(&current);
            let (next, inverse) =
                apply_one(&current, context, &primitive, index, setting)?;
            let after_op = PropsDocument::parse(&next);
            verify_op(&before_op, &after_op, context, &primitive, index)?;
            warnings.extend(validate::operation(
                context, &after_op, &primitive, index,
            )?);
            current = next;
            inverse_groups.push(inverse);
            expanded_ops.push(primitive);
        }
    }
    inverse_groups.reverse();
    let inverses = inverse_groups.into_iter().flatten().collect();

    let after = inspect(&current, context);
    refuse_unwritable(&after.document, None)?;
    verify(&before.document, &after.document, &expanded_ops)?;
    let fingerprint_after =
        after.document.fingerprint.clone().ok_or_else(|| {
            verify_failed("candidate fingerprint is unavailable", None, None)
        })?;
    let effects = context
        .contract
        .as_ref()
        .map(|contract| {
            super::contract::effects::collect(
                contract,
                &context.normalized_path,
                contents,
                &current,
                context.roles.note_kind,
            )
        })
        .unwrap_or_default();
    Ok(PlanOutcome {
        changed: current != contents,
        candidate: current,
        report: after.report,
        fingerprint_before,
        fingerprint_after,
        inverse_ops: inverses,
        warnings,
        effects,
    })
}

fn apply_one(
    contents: &str,
    context: &InspectContext,
    op: &Op,
    index: usize,
    setting: bool,
) -> Result<(String, Vec<Op>), PropsError> {
    let document = PropsDocument::parse(contents);
    refuse_unwritable(&document, Some(index))?;
    match op {
        Op::SettingSet { setting, value }
            if setting == "title"
                && context.roles.note_kind
                    == super::roles::NoteKind::Reference =>
        {
            apply_reference_title(contents, &document, context, value, index)
        }
        Op::SettingSet { .. } | Op::SettingClear { .. } => {
            unreachable!("non-title settings expand before span planning")
        }
        Op::Set {
            key,
            value,
            kind,
            position,
        } => apply_set(
            contents,
            &document,
            context,
            key,
            value,
            *kind,
            position.as_ref(),
            index,
        ),
        Op::Unset { key } => {
            apply_unset(contents, &document, context, key, index)
        }
        Op::Rename { key, to } => {
            apply_rename(contents, &document, context, key, to, index)
        }
        Op::Move { key, position } => {
            apply_move(contents, &document, context, key, position, index)
        }
        Op::SetYaml {
            key,
            yaml,
            position,
        } => apply_set_yaml(
            contents,
            &document,
            context,
            key,
            yaml,
            position.as_ref(),
            index,
            setting && key == "offclock_types",
        ),
        Op::Restore {
            key,
            raw,
            position,
            framing,
            body,
        } => apply_restore(
            contents,
            &document,
            context,
            key,
            raw.as_deref(),
            position.as_ref(),
            framing.as_ref(),
            body.as_ref(),
            index,
        ),
    }
}

fn apply_set(
    contents: &str,
    document: &PropsDocument,
    context: &InspectContext,
    key: &str,
    value: &serde_json::Value,
    kind: Option<PropertyKind>,
    position: Option<&Position>,
    index: usize,
) -> Result<(String, Vec<Op>), PropsError> {
    validate_key(key, index)?;
    refuse_protected(context, key, index)?;
    refuse_merge_siblings(document, key, index)?;
    let typed = typed_from_json(key, value, kind, index)?;
    super::contract::validate::input(
        context,
        contents,
        key,
        &typed.yaml,
        index,
    )?;
    if let Some(entry) = find_entry(document, key) {
        refuse_opaque(entry, index)?;
        let inverse = restore_full(contents, document, entry);
        if entry
            .raw_value
            .as_ref()
            .is_some_and(|existing| yaml_eq(existing, &typed.yaml))
        {
            return Ok((contents.to_owned(), vec![inverse]));
        }
        let newline = document.preferred_newline();
        let retain_flow = existing_is_short_flow_list(
            contents,
            &entry.value_range,
            &entry.style,
        ) && matches!(
            typed.kind,
            PropertyKind::List | PropertyKind::Tags | PropertyKind::Aliases
        );
        let emitted = emit_value(
            &typed.yaml,
            typed.kind,
            Some(&entry.style),
            retain_flow,
            &newline,
        );
        let rebuilt =
            rebuild_entry_with_value(contents, entry, &emitted, &newline);
        let next =
            replace_range(contents, entry.source_range.clone(), &rebuilt);
        Ok((next, vec![inverse]))
    } else {
        let newline = document.preferred_newline();
        let entry_text = emit_new_entry(key, &typed.yaml, typed.kind, &newline);
        let created = document.frontmatter_range.is_none();
        let next = insert_entry(
            contents,
            document,
            &entry_text,
            position,
            index,
            created,
        )?;
        Ok((
            next.clone(),
            vec![restore_absence(
                contents, document, &next, key, created, &newline,
            )],
        ))
    }
}

fn apply_reference_title(
    contents: &str,
    document: &PropsDocument,
    context: &InspectContext,
    value: &JsonValue,
    index: usize,
) -> Result<(String, Vec<Op>), PropsError> {
    let typed = typed_from_json("title", value, None, index)?;
    let old_title =
        reference_title(contents, document, &context.normalized_path);
    let (mut candidate, mut inverses) = apply_set(
        contents, document, context, "title", value, None, None, index,
    )?;
    let Some(old_title) = old_title else {
        return Ok((candidate, inverses));
    };
    let Some(new_title) = typed.yaml.as_str() else {
        return Ok((candidate, inverses));
    };
    let old_body = &contents[document.body_range.clone()];
    let candidate_document = PropsDocument::parse(&candidate);
    let candidate_body = &candidate[candidate_document.body_range.clone()];
    let new_body =
        retitle_matching_first_h1(candidate_body, &old_title, new_title);
    if new_body == candidate_body {
        return Ok((candidate, inverses));
    }
    candidate = replace_range(
        &candidate,
        candidate_document.body_range.clone(),
        &new_body,
    );
    let Some(Op::Restore { body, .. }) = inverses.first_mut() else {
        return Err(verify_failed(
            "Reference title edit did not produce an undo operation",
            Some("title".to_owned()),
            Some(index),
        ));
    };
    *body = Some(super::protocol::BodyRestore {
        expected: new_body,
        prior: old_body.to_owned(),
    });
    Ok((candidate, inverses))
}

fn reference_title(
    contents: &str,
    document: &PropsDocument,
    path: &str,
) -> Option<String> {
    if let Some(title) = document
        .entries
        .iter()
        .find(|entry| entry.key == "title")
        .and_then(|entry| entry.value.as_str())
        .filter(|title| !title.trim().is_empty())
    {
        return Some(title.to_owned());
    }
    let body = &contents[document.body_range.clone()];
    first_body_h1(body)
        .map(|(title, _)| title)
        .filter(|title| !title.trim().is_empty())
        .or_else(|| {
            std::path::Path::new(path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
        })
}

fn retitle_matching_first_h1(
    body: &str,
    old_title: &str,
    new_title: &str,
) -> String {
    let Some((heading, range)) = first_body_h1(body) else {
        return body.to_owned();
    };
    if heading != old_title {
        return body.to_owned();
    }
    replace_range(body, range, new_title)
}

fn first_body_h1(body: &str) -> Option<(String, Range<usize>)> {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let logical: Vec<&str> = lines
        .iter()
        .map(|line| crate::native::markdown::split_line_ending(line).0)
        .collect();
    let fenced =
        crate::native::markdown::fenced_lines(&logical, 0..logical.len());
    let comments = crate::native::markdown::multiline_comment_lines(&logical);
    let mut offset = 0;
    for (index, line) in logical.iter().enumerate() {
        let full_line = lines[index];
        if !fenced.contains(&index)
            && !comments.contains(&index)
            && crate::native::markdown::atx_heading(line)
                .is_some_and(|(level, _)| level == 1)
        {
            let (_, title) = crate::native::markdown::atx_heading(line)?;
            let indentation =
                line.bytes().take_while(|byte| *byte == b' ').count();
            let hash_count = line[indentation..]
                .bytes()
                .take_while(|byte| *byte == b'#')
                .count();
            let tail_start = indentation + hash_count;
            let tail = &line[tail_start..];
            let title_start =
                tail_start + (tail.len() - tail.trim_start().len());
            let title_range = if title.is_empty() {
                title_start..title_start
            } else {
                let start = line[title_start..].find(title)? + title_start;
                start..start + title.len()
            };
            return Some((
                title.to_owned(),
                offset + title_range.start..offset + title_range.end,
            ));
        }
        offset += full_line.len();
    }
    None
}

fn is_title_only_body_change(current: &str, prior: &str) -> bool {
    let Some((prior_title, _)) = first_body_h1(prior) else {
        return false;
    };
    let Some((_, current_range)) = first_body_h1(current) else {
        return false;
    };
    replace_range(current, current_range, &prior_title) == prior
}

fn apply_set_yaml(
    contents: &str,
    document: &PropsDocument,
    context: &InspectContext,
    key: &str,
    yaml: &str,
    position: Option<&Position>,
    index: usize,
    canonical_block: bool,
) -> Result<(String, Vec<Op>), PropsError> {
    validate_key(key, index)?;
    refuse_protected(context, key, index)?;
    refuse_merge_siblings(document, key, index)?;
    let parsed = parse_yaml_fragment(yaml, key, index)?;
    super::contract::validate::input(context, contents, key, &parsed, index)?;
    if let Some(entry) = find_entry(document, key) {
        refuse_opaque(entry, index)?;
        let inverse = restore_full(contents, document, entry);
        if !canonical_block
            && entry
                .raw_value
                .as_ref()
                .is_some_and(|existing| yaml_eq(existing, &parsed))
        {
            return Ok((contents.to_owned(), vec![inverse]));
        }
        let newline = document.preferred_newline();
        let spliced = splice_yaml_fragment(yaml, &newline);
        let rebuilt =
            rebuild_entry_with_value(contents, entry, &spliced, &newline);
        let next =
            replace_range(contents, entry.source_range.clone(), &rebuilt);
        verify_single_key_after_set(&next, document, key, index)?;
        Ok((next, vec![inverse]))
    } else {
        let newline = document.preferred_newline();
        let spliced = splice_yaml_fragment(yaml, &newline);
        let entry_text = emit_new_entry_fragment(key, &spliced, &newline);
        let created = document.frontmatter_range.is_none();
        let next = insert_entry(
            contents,
            document,
            &entry_text,
            position,
            index,
            created,
        )?;
        verify_single_key_after_set(&next, document, key, index)?;
        Ok((
            next.clone(),
            vec![restore_absence(
                contents, document, &next, key, created, &newline,
            )],
        ))
    }
}

fn apply_unset(
    contents: &str,
    document: &PropsDocument,
    context: &InspectContext,
    key: &str,
    index: usize,
) -> Result<(String, Vec<Op>), PropsError> {
    validate_key(key, index)?;
    refuse_protected(context, key, index)?;
    refuse_merge_siblings(document, key, index)?;
    let (entry_index, entry) = require_entry(document, key, index)?;
    refuse_opaque(entry, index)?;
    let inverse = restore_after_remove(contents, document, entry);
    let _ = entry_index;
    let next = remove_entry(contents, document, entry);
    Ok((next, vec![inverse]))
}

fn apply_rename(
    contents: &str,
    document: &PropsDocument,
    context: &InspectContext,
    key: &str,
    to: &str,
    index: usize,
) -> Result<(String, Vec<Op>), PropsError> {
    validate_key(key, index)?;
    validate_key(to, index)?;
    refuse_protected(context, key, index)?;
    refuse_protected(context, to, index)?;
    refuse_merge_siblings(document, key, index)?;
    if key == to {
        return Ok((
            contents.to_owned(),
            vec![Op::Rename {
                key: to.to_owned(),
                to: key.to_owned(),
            }],
        ));
    }
    if find_entry(document, to).is_some() {
        return Err(PropsError {
            code: "prop_exists".to_owned(),
            message: format!("property {to:?} already exists"),
            hint: "Choose a different name or unset the destination first."
                .to_owned(),
            key: Some(to.to_owned()),
            line: find_entry(document, to).and_then(|entry| entry.line),
            op_index: Some(index),
        });
    }
    let (_, entry) = require_entry(document, key, index)?;
    refuse_opaque(entry, index)?;
    let original = restore_after_remove(contents, document, entry);
    let next = replace_range(contents, entry.key_range.clone(), &emit_key(to));
    Ok((
        next,
        vec![
            Op::Restore {
                key: to.to_owned(),
                raw: None,
                position: None,
                framing: None,
                body: None,
            },
            original,
        ],
    ))
}

fn apply_move(
    contents: &str,
    document: &PropsDocument,
    context: &InspectContext,
    key: &str,
    position: &Position,
    index: usize,
) -> Result<(String, Vec<Op>), PropsError> {
    validate_key(key, index)?;
    refuse_protected(context, key, index)?;
    refuse_merge_siblings(document, key, index)?;
    let (entry_index, entry) = require_entry(document, key, index)?;
    refuse_opaque(entry, index)?;
    let dest = insertion_offset(document, Some(position), index)?;
    let start = entry.source_range.start;
    let end = entry.source_range.end;
    let inverse = move_inverse(document, entry_index);
    if dest == start || dest == end || (dest > start && dest < end) {
        return Ok((contents.to_owned(), vec![inverse]));
    }
    let next = move_span(contents, start, end, dest);
    Ok((next, vec![inverse]))
}

fn apply_restore(
    contents: &str,
    document: &PropsDocument,
    context: &InspectContext,
    key: &str,
    raw: Option<&str>,
    position: Option<&Position>,
    framing: Option<&Framing>,
    body_restore: Option<&super::protocol::BodyRestore>,
    index: usize,
) -> Result<(String, Vec<Op>), PropsError> {
    validate_key(key, index)?;
    refuse_protected(context, key, index)?;
    refuse_merge_siblings(document, key, index)?;
    if let Some(raw) = raw {
        validate_raw(raw, key, index)?;
    }
    if let Some(framing) = framing {
        validate_framing(framing, index)?;
    }
    let (mut next, mut inverses) = match raw {
        Some(raw) => {
            if let Some(entry) = find_entry(document, key) {
                refuse_opaque(entry, index)?;
                let inverse = restore_full(contents, document, entry);
                let next =
                    replace_range(contents, entry.source_range.clone(), raw);
                Ok((next, vec![inverse]))
            } else {
                let created = document.frontmatter_range.is_none();
                let newline = document.preferred_newline();
                let entry_text = ensure_trailing_newline(raw, &newline);
                let next = insert_entry(
                    contents,
                    document,
                    &entry_text,
                    position,
                    index,
                    created,
                )?;
                Ok((
                    next.clone(),
                    vec![restore_absence(
                        contents, document, &next, key, created, &newline,
                    )],
                ))
            }
        }
        None => {
            if let Some(entry) = find_entry(document, key) {
                refuse_opaque(entry, index)?;
                let inverse = restore_after_remove(contents, document, entry);
                let mut next = remove_entry(contents, document, entry);
                if let Some(framing) = framing {
                    next = apply_framing(&next, framing, index)?;
                }
                Ok((next, vec![inverse]))
            } else if let Some(framing) = framing {
                let next = apply_framing(contents, framing, index)?;
                Ok((
                    next,
                    vec![Op::Restore {
                        key: key.to_owned(),
                        raw: None,
                        position: None,
                        framing: Some(Framing {
                            expected: framing.prior.clone(),
                            prior: framing.expected.clone(),
                        }),
                        body: None,
                    }],
                ))
            } else {
                Ok((
                    contents.to_owned(),
                    vec![Op::Restore {
                        key: key.to_owned(),
                        raw: None,
                        position: None,
                        framing: None,
                        body: None,
                    }],
                ))
            }
        }
    }?;
    if let Some(body_restore) = body_restore {
        if key != "title"
            || context.roles.note_kind != super::roles::NoteKind::Reference
        {
            return Err(PropsError {
                code: "prop_restore_body_disabled".to_owned(),
                message:
                    "body restoration is reserved for a Reference title undo"
                        .to_owned(),
                hint: "Restore the frontmatter property without a body field."
                    .to_owned(),
                key: Some(key.to_owned()),
                line: None,
                op_index: Some(index),
            });
        }
        let next_document = PropsDocument::parse(&next);
        let current_body = next[next_document.body_range.clone()].to_owned();
        if current_body != body_restore.expected
            || !is_title_only_body_change(&current_body, &body_restore.prior)
        {
            return Err(PropsError {
                code: "prop_stale".to_owned(),
                message: "title undo body no longer matches its expected H1"
                    .to_owned(),
                hint: "Re-read the note and retry with a fresh preview."
                    .to_owned(),
                key: Some(key.to_owned()),
                line: None,
                op_index: Some(index),
            });
        }
        let prior_body = body_restore.prior.clone();
        next =
            replace_range(&next, next_document.body_range.clone(), &prior_body);
        let inverse_body = super::protocol::BodyRestore {
            expected: prior_body,
            prior: current_body,
        };
        match inverses.first_mut() {
            Some(Op::Restore { body, .. }) => *body = Some(inverse_body),
            _ => {
                return Err(verify_failed(
                    "body restoration did not produce a reversible property operation",
                    Some(key.to_owned()),
                    Some(index),
                ));
            }
        }
    }
    Ok((next, inverses))
}

fn emit_new_entry(
    key: &str,
    value: &YamlValue,
    kind: PropertyKind,
    newline: &str,
) -> String {
    emit_new_entry_fragment(
        key,
        &emit_value(value, kind, None, false, newline),
        newline,
    )
}

fn emit_new_entry_fragment(key: &str, rendered: &str, newline: &str) -> String {
    let key_text = emit_key(key);
    let mut text = if rendered.starts_with(newline) {
        format!("{key_text}:{rendered}")
    } else {
        format!("{key_text}: {rendered}")
    };
    if !text.ends_with('\n') && !text.ends_with('\r') {
        text.push_str(newline);
    }
    text
}

fn rebuild_entry_with_value(
    source: &str,
    entry: &PropertyEntry,
    emitted: &str,
    newline: &str,
) -> String {
    let leading = &source[entry.leading_range.clone()];
    let key = &source[entry.key_range.clone()];
    let after_colon = &source[entry.colon_range.end..entry.value_range.start];
    let block_multiline = emitted.starts_with(newline);
    let gap = if block_multiline {
        ""
    } else if after_colon.is_empty() {
        " "
    } else {
        after_colon
    };
    let comment = header_comment_suffix(source, entry);
    let value_and_comment_end = match &entry.inline_comment_range {
        Some(comment) => comment.end.max(entry.value_range.end),
        None => entry.value_range.end,
    };
    let trailing = &source[value_and_comment_end..entry.source_range.end];
    if emitted.contains('\n') || emitted.contains('\r') {
        let (first, rest) = split_once_ending(emitted, newline);
        let (eol, extra) = split_once_ending(trailing, newline);
        let eol = if eol.is_empty() { newline } else { eol };
        // The entry's source range usually ends with its final line ending.
        // Splitting `trailing` keeps only the remainder after that ending, so
        // re-append it unless `extra` already carries one; otherwise the next
        // entry's first line glues onto the new value's last line.
        let end = if trailing.ends_with('\n') || trailing.ends_with('\r') {
            if extra.ends_with('\n') || extra.ends_with('\r') {
                ""
            } else {
                eol
            }
        } else {
            ""
        };
        format!("{leading}{key}:{gap}{first}{comment}{eol}{rest}{extra}{end}")
    } else {
        format!("{leading}{key}:{gap}{emitted}{comment}{trailing}")
    }
}

/// Reattach a header-line `#` comment after the first emitted line.
///
/// Block scalars keep `value_range` starting at `|-` and spanning children,
/// so the comment sits *inside* that range. Trailing after
/// `max(value_end, comment_end)` then drops the old children instead of
/// duplicating them.
fn header_comment_suffix(source: &str, entry: &PropertyEntry) -> String {
    if entry.inline_comment_range.is_none() {
        return String::new();
    }
    let after_colon_start = entry.colon_range.end;
    let header_end = source[after_colon_start..]
        .find(['\n', '\r'])
        .map(|index| after_colon_start + index)
        .unwrap_or(source.len());
    let after = &source[after_colon_start..header_end];
    let (_, comment) = yaml_spans::split_comment(after);
    if comment.is_empty() {
        return String::new();
    }
    format!("{}{comment}", yaml_spans::whitespace_before_comment(after))
}

fn split_once_ending<'a>(text: &'a str, newline: &str) -> (&'a str, &'a str) {
    if let Some(rest) = text.strip_prefix(newline) {
        return ("", rest);
    }
    if let Some(index) = text.find(newline) {
        (&text[..index], &text[index + newline.len()..])
    } else if let Some(index) = text.find('\n') {
        (&text[..index], &text[index + 1..])
    } else {
        (text, "")
    }
}

fn insert_entry(
    contents: &str,
    document: &PropsDocument,
    entry_text: &str,
    position: Option<&Position>,
    index: usize,
    created: bool,
) -> Result<String, PropsError> {
    if created {
        let bom = document
            .bom_range
            .as_ref()
            .map(|range| &contents[range.clone()])
            .unwrap_or("");
        let body = &contents[document.body_range.clone()];
        let newline = document.preferred_newline();
        return Ok(format!("{bom}---{newline}{entry_text}---{newline}{body}"));
    }
    let offset = insertion_offset(document, position, index)?;
    if document.root_is_flow() {
        return insert_flow_item(contents, document, entry_text, offset);
    }
    Ok(insert_at(contents, offset, entry_text))
}

fn insert_flow_item(
    contents: &str,
    document: &PropsDocument,
    entry_text: &str,
    offset: usize,
) -> Result<String, PropsError> {
    let item = entry_text.trim_end_matches(['\n', '\r']);
    if document.entries.is_empty() {
        return Ok(insert_at(contents, offset, item));
    }
    let first = document.entries[0].source_range.start;
    let insertion = if offset <= first {
        format!("{item}, ")
    } else {
        format!(", {item}")
    };
    Ok(insert_at(contents, offset, &insertion))
}

fn insertion_offset(
    document: &PropsDocument,
    position: Option<&Position>,
    index: usize,
) -> Result<usize, PropsError> {
    match position {
        None => {
            if let Some(last) = document.entries.last() {
                Ok(last.source_range.end)
            } else if let Some(close) = close_start(document) {
                Ok(close)
            } else {
                Ok(document.body_range.start)
            }
        }
        Some(Position::Before(key)) => {
            let (_, entry) = require_neighbor(document, key, index)?;
            Ok(entry.source_range.start)
        }
        Some(Position::After(key)) => {
            let (_, entry) = require_neighbor(document, key, index)?;
            Ok(entry.source_range.end)
        }
    }
}

fn close_start(document: &PropsDocument) -> Option<usize> {
    let close_line = document.end_line?;
    let lines = crate::native::yaml_spans::split_lines(&document.source);
    let mut offset = 0usize;
    for (index, line) in lines.iter().enumerate() {
        if index + 1 == close_line {
            return Some(offset);
        }
        offset += line.text.len() + line.ending.len();
    }
    None
}

fn remove_entry(
    contents: &str,
    document: &PropsDocument,
    entry: &PropertyEntry,
) -> String {
    if document.root_is_flow() {
        return remove_flow_item(contents, document, entry);
    }
    if document.entries.len() == 1 {
        let leading = &contents[entry.leading_range.clone()];
        return replace_range(contents, entry.source_range.clone(), leading);
    }
    replace_range(contents, entry.source_range.clone(), "")
}

fn remove_flow_item(
    contents: &str,
    document: &PropsDocument,
    entry: &PropertyEntry,
) -> String {
    if document.entries.len() == 1 {
        return replace_range(contents, entry.source_range.clone(), "");
    }
    let start = entry.source_range.start;
    let end = entry.source_range.end;
    let is_last = document
        .entries
        .last()
        .is_some_and(|last| last.source_range == entry.source_range);
    if is_last {
        let mut comma = start;
        while comma > 0 && contents.as_bytes()[comma - 1].is_ascii_whitespace()
        {
            comma -= 1;
        }
        if comma > 0 && contents.as_bytes()[comma - 1] == b',' {
            comma -= 1;
        }
        replace_range(contents, comma..end, "")
    } else {
        let mut comma = end;
        while comma < contents.len()
            && contents.as_bytes()[comma].is_ascii_whitespace()
        {
            comma += 1;
        }
        if comma < contents.len() && contents.as_bytes()[comma] == b',' {
            comma += 1;
            while comma < contents.len()
                && contents.as_bytes()[comma].is_ascii_whitespace()
                && contents.as_bytes()[comma] != b'\n'
            {
                comma += 1;
            }
        }
        replace_range(contents, start..comma, "")
    }
}

fn move_span(source: &str, start: usize, end: usize, dest: usize) -> String {
    let entry = &source[start..end];
    if dest <= start {
        format!(
            "{}{}{}{}",
            &source[..dest],
            entry,
            &source[dest..start],
            &source[end..]
        )
    } else {
        format!(
            "{}{}{}{}",
            &source[..start],
            &source[end..dest],
            entry,
            &source[dest..]
        )
    }
}

fn move_inverse(document: &PropsDocument, entry_index: usize) -> Op {
    let key = document.entries[entry_index].key.clone();
    let position = if entry_index > 0 {
        Position::After(document.entries[entry_index - 1].key.clone())
    } else if document.entries.len() > 1 {
        Position::Before(document.entries[1].key.clone())
    } else {
        return Op::Move {
            key,
            position: Position::Before(
                document.entries[entry_index].key.clone(),
            ),
        };
    };
    Op::Move { key, position }
}

fn restore_full(
    contents: &str,
    document: &PropsDocument,
    entry: &PropertyEntry,
) -> Op {
    restore_bytes(contents, document, entry, entry.raw(contents).to_owned())
}

fn restore_after_remove(
    contents: &str,
    document: &PropsDocument,
    entry: &PropertyEntry,
) -> Op {
    let raw = if document.entries.len() == 1 {
        contents[entry.key_range.start..entry.source_range.end].to_owned()
    } else {
        entry.raw(contents).to_owned()
    };
    restore_bytes(contents, document, entry, raw)
}

fn restore_bytes(
    contents: &str,
    document: &PropsDocument,
    entry: &PropertyEntry,
    raw: String,
) -> Op {
    let index = document
        .entries
        .iter()
        .position(|item| item.source_range == entry.source_range)
        .unwrap_or(0);
    let position = if document.entries.len() == 1 {
        None
    } else if index > 0 {
        Some(Position::After(document.entries[index - 1].key.clone()))
    } else {
        Some(Position::Before(document.entries[1].key.clone()))
    };
    let _ = contents;
    Op::Restore {
        key: entry.key.clone(),
        raw: Some(raw),
        position,
        framing: None,
        body: None,
    }
}

fn restore_absence(
    original: &str,
    before: &PropsDocument,
    after: &str,
    key: &str,
    created: bool,
    newline: &str,
) -> Op {
    let framing = if created {
        let after_doc = PropsDocument::parse(after);
        if let Some(entry) = find_entry(&after_doc, key) {
            let without = replace_range(after, entry.source_range.clone(), "");
            let without_doc = PropsDocument::parse(&without);
            let bom = without_doc.bom_range.clone().unwrap_or(0..0);
            Some(Framing {
                expected: without[bom.end..without_doc.body_range.start]
                    .to_owned(),
                prior: String::new(),
            })
        } else {
            Some(Framing {
                expected: format!("---{newline}---{newline}"),
                prior: String::new(),
            })
        }
    } else {
        None
    };
    let _ = original;
    let _ = before;
    Op::Restore {
        key: key.to_owned(),
        raw: None,
        position: None,
        framing,
        body: None,
    }
}

/// Wrap restore raw as a one-entry frontmatter document.
///
/// Keep-chomp empty block lines are significant, so a raw that already
/// ends with a newline must not gain a second blank before the closer.
fn wrap_restore_raw(raw: &str) -> String {
    if raw.ends_with('\n') || raw.ends_with('\r') {
        format!("---\n{raw}---\n")
    } else {
        format!("---\n{raw}\n---\n")
    }
}

fn validate_raw(raw: &str, key: &str, index: usize) -> Result<(), PropsError> {
    if raw.contains('\u{feff}') {
        return Err(bad_restore(
            key,
            index,
            "restore raw cannot include a BOM",
        ));
    }
    if raw.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == "---" || trimmed == "..."
    }) {
        return Err(bad_restore(
            key,
            index,
            "restore raw cannot contain a frontmatter delimiter or body",
        ));
    }
    let wrapped = wrap_restore_raw(raw);
    let parsed = PropsDocument::parse(&wrapped);
    if parsed.entries.len() != 1 {
        return Err(bad_restore(
            key,
            index,
            "restore raw must describe exactly one property entry",
        ));
    }
    if parsed.entries[0].key != key {
        return Err(bad_restore(
            key,
            index,
            "restore raw must describe the addressed key",
        ));
    }
    if parsed.entries[0].opaque {
        return Err(bad_restore(
            key,
            index,
            "restore raw cannot contain anchors, aliases, tags, or merge keys",
        ));
    }
    if !parsed.body_range.is_empty()
        && !parsed.source[parsed.body_range.clone()].trim().is_empty()
    {
        return Err(bad_restore(
            key,
            index,
            "restore raw cannot include body text",
        ));
    }
    Ok(())
}

fn validate_framing(framing: &Framing, index: usize) -> Result<(), PropsError> {
    if !framing_is_trivia(&framing.expected)
        || !framing_is_trivia(&framing.prior)
    {
        return Err(PropsError {
            code: "prop_bad_value".to_owned(),
            message: "framing fragments may contain only delimiters and trivia"
                .to_owned(),
            hint: "Keep BOM and body outside restore framing metadata."
                .to_owned(),
            key: None,
            line: None,
            op_index: Some(index),
        });
    }
    Ok(())
}

fn framing_is_trivia(fragment: &str) -> bool {
    if fragment.is_empty() {
        return true;
    }
    if fragment.contains('\u{feff}') {
        return false;
    }
    let parsed = PropsDocument::parse(fragment);
    matches!(
        parsed.state,
        FrontmatterState::Ok | FrontmatterState::Absent
    ) && parsed.entries.is_empty()
        && parsed.bom_range.is_none()
        && parsed.source[parsed.body_range.clone()].is_empty()
}

fn apply_framing(
    contents: &str,
    framing: &Framing,
    index: usize,
) -> Result<String, PropsError> {
    let document = PropsDocument::parse(contents);
    if !document.entries.is_empty() {
        return Err(PropsError {
            code: "prop_bad_value".to_owned(),
            message: "framing restoration requires an empty property list"
                .to_owned(),
            hint: "Restore absence first, then apply framing.".to_owned(),
            key: None,
            line: None,
            op_index: Some(index),
        });
    }
    let bom_end = document
        .bom_range
        .clone()
        .map(|range| range.end)
        .unwrap_or(0);
    let prefix = &contents[bom_end..document.body_range.start];
    if prefix != framing.expected {
        return Err(PropsError {
            code: "prop_verify_failed".to_owned(),
            message:
                "current frontmatter prefix does not match framing.expected"
                    .to_owned(),
            hint: "Re-read the note; framing restoration is exact.".to_owned(),
            key: None,
            line: None,
            op_index: Some(index),
        });
    }
    Ok(format!(
        "{}{}{}",
        &contents[..bom_end],
        framing.prior,
        &contents[document.body_range.start..]
    ))
}

fn splice_yaml_fragment(yaml: &str, newline: &str) -> String {
    let trimmed = yaml.trim_end_matches(['\n', '\r']);
    if trimmed.contains('\n') || trimmed.contains('\r') {
        let mut lines = trimmed.lines();
        let Some(first) = lines.next() else {
            return String::new();
        };
        let mut out = first.to_owned();
        for line in lines {
            out.push_str(newline);
            if line.is_empty() {
                continue;
            }
            if !line.starts_with(' ') && !line.starts_with('\t') {
                out.push_str("  ");
            }
            out.push_str(line);
        }
        out
    } else {
        trimmed.to_owned()
    }
}

fn verify_single_key_after_set(
    next: &str,
    before: &PropsDocument,
    key: &str,
    index: usize,
) -> Result<(), PropsError> {
    let after = PropsDocument::parse(next);
    let before_keys: Vec<_> = before
        .entries
        .iter()
        .map(|entry| entry.key.as_str())
        .collect();
    let extra = after
        .entries
        .iter()
        .filter(|entry| {
            entry.key != key && !before_keys.contains(&entry.key.as_str())
        })
        .count();
    if extra > 0 {
        return Err(PropsError {
            code: "prop_bad_value".to_owned(),
            message: "yaml fragment would inject sibling keys".to_owned(),
            hint: "Provide a single value fragment for the addressed key."
                .to_owned(),
            key: Some(key.to_owned()),
            line: None,
            op_index: Some(index),
        });
    }
    Ok(())
}

fn verify(
    original: &PropsDocument,
    candidate: &PropsDocument,
    ops: &[Op],
) -> Result<(), PropsError> {
    let original_body = &original.source[original.body_range.clone()];
    let candidate_body = &candidate.source[candidate.body_range.clone()];
    let body_operation = ops.iter().any(|op| {
        matches!(op, Op::SettingSet { setting, .. } if setting == "title")
            || matches!(op, Op::Restore { body: Some(_), .. })
    });
    if original_body != candidate_body && !body_operation {
        return Err(verify_failed("operation changed body bytes", None, None));
    }
    let original_bom = original
        .bom_range
        .as_ref()
        .map(|range| &original.source[range.clone()])
        .unwrap_or("");
    let candidate_bom = candidate
        .bom_range
        .as_ref()
        .map(|range| &candidate.source[range.clone()])
        .unwrap_or("");
    if original_bom != candidate_bom {
        return Err(verify_failed("operation changed BOM bytes", None, None));
    }
    let touched = touched_keys(ops);
    for entry in &original.entries {
        if touched.contains(&entry.key) {
            continue;
        }
        let Some(after) =
            candidate.entries.iter().find(|item| item.key == entry.key)
        else {
            return Err(verify_failed(
                &format!("untouched key {:?} is missing", entry.key),
                Some(entry.key.clone()),
                None,
            ));
        };
        if entry.raw(&original.source) != after.raw(&candidate.source) {
            return Err(verify_failed(
                &format!("untouched key {:?} changed bytes", entry.key),
                Some(entry.key.clone()),
                None,
            ));
        }
        if entry.raw_value != after.raw_value {
            return Err(verify_failed(
                &format!("untouched key {:?} changed meaning", entry.key),
                Some(entry.key.clone()),
                None,
            ));
        }
        if entry.value != after.value {
            return Err(verify_failed(
                &format!("untouched key {:?} changed JSON value", entry.key),
                Some(entry.key.clone()),
                None,
            ));
        }
    }
    Ok(())
}

fn verify_op(
    before: &PropsDocument,
    after: &PropsDocument,
    context: &InspectContext,
    op: &Op,
    index: usize,
) -> Result<(), PropsError> {
    match op {
        Op::SettingSet { setting, value } if setting == "title" => {
            let typed = typed_from_json("title", value, None, index)?;
            verify_expected_value(after, "title", &typed.yaml, index)?;
            let Some(old_title) = reference_title(
                &before.source,
                before,
                &context.normalized_path,
            ) else {
                return Ok(());
            };
            let Some(new_title) = typed.yaml.as_str() else {
                return Ok(());
            };
            let expected = retitle_matching_first_h1(
                &before.source[before.body_range.clone()],
                &old_title,
                new_title,
            );
            if expected != after.source[after.body_range.clone()] {
                return Err(verify_failed(
                    "Reference title changed body bytes outside its matching first H1",
                    Some("title".to_owned()),
                    Some(index),
                ));
            }
            Ok(())
        }
        Op::SettingSet { .. } | Op::SettingClear { .. } => {
            unreachable!("non-title settings expand before span planning")
        }
        Op::Set {
            key,
            value,
            kind,
            position,
        } => {
            let typed = typed_from_json(key, value, *kind, index)?;
            verify_expected_value(after, key, &typed.yaml, index)?;
            if find_entry(before, key).is_none()
                && let Some(position) = position
            {
                verify_position(after, key, position, index)?;
            }
            Ok(())
        }
        Op::SetYaml {
            key,
            yaml,
            position,
        } => {
            let parsed = parse_yaml_fragment(yaml, key, index)?;
            verify_expected_value(after, key, &parsed, index)?;
            if find_entry(before, key).is_none()
                && let Some(position) = position
            {
                verify_position(after, key, position, index)?;
            }
            Ok(())
        }
        Op::Unset { key } => verify_absent(after, key, index),
        Op::Rename { key, to } => {
            if key == to {
                return Ok(());
            }
            verify_absent(after, key, index)?;
            let Some(original) = find_entry(before, key) else {
                return Ok(());
            };
            let Some(expected) = original.raw_value.as_ref() else {
                return Ok(());
            };
            verify_expected_value(after, to, expected, index)
        }
        Op::Move { key, position } => {
            verify_position(after, key, position, index)
        }
        Op::Restore {
            key,
            raw,
            position,
            framing: _,
            body,
        } => {
            let result = match raw {
                Some(raw) => {
                    let wrapped = wrap_restore_raw(raw);
                    let parsed = PropsDocument::parse(&wrapped);
                    let Some(expected) = parsed
                        .entries
                        .first()
                        .and_then(|entry| entry.raw_value.as_ref())
                    else {
                        return Ok(());
                    };
                    verify_expected_value(after, key, expected, index)?;
                    if find_entry(before, key).is_none()
                        && let Some(position) = position
                    {
                        verify_position(after, key, position, index)?;
                    }
                    Ok(())
                }
                None => verify_absent(after, key, index),
            };
            result?;
            if let Some(body) = body {
                let before_body = &before.source[before.body_range.clone()];
                let after_body = &after.source[after.body_range.clone()];
                if before_body != body.expected
                    || after_body != body.prior
                    || !is_title_only_body_change(before_body, after_body)
                {
                    return Err(verify_failed(
                        "body restore changed bytes outside the matching first H1",
                        Some(key.clone()),
                        Some(index),
                    ));
                }
            }
            Ok(())
        }
    }
}

fn verify_expected_value(
    document: &PropsDocument,
    key: &str,
    expected: &serde_yaml::Value,
    index: usize,
) -> Result<(), PropsError> {
    let Some(entry) = find_entry(document, key) else {
        return Err(verify_failed(
            &format!("operation did not leave {key:?}"),
            Some(key.to_owned()),
            Some(index),
        ));
    };
    if entry.opaque {
        return Err(verify_failed(
            &format!("operation produced an opaque {key:?} value"),
            Some(key.to_owned()),
            Some(index),
        ));
    }
    let Some(actual) = entry.raw_value.as_ref() else {
        return Err(verify_failed(
            &format!("{key:?} has no parsed YAML after the operation"),
            Some(key.to_owned()),
            Some(index),
        ));
    };
    if !yaml_semantically_eq(actual, expected) {
        return Err(verify_failed(
            &format!("{key:?} YAML does not match the operation"),
            Some(key.to_owned()),
            Some(index),
        ));
    }
    let expected_json =
        crate::native::records::fingerprint::yaml_to_canonical_json(expected);
    if !json_semantically_eq(&entry.value, &expected_json) {
        return Err(verify_failed(
            &format!("{key:?} JSON does not match the operation"),
            Some(key.to_owned()),
            Some(index),
        ));
    }
    Ok(())
}

fn verify_absent(
    document: &PropsDocument,
    key: &str,
    index: usize,
) -> Result<(), PropsError> {
    if find_entry(document, key).is_some() {
        return Err(verify_failed(
            &format!("operation left {key:?} in place"),
            Some(key.to_owned()),
            Some(index),
        ));
    }
    Ok(())
}

fn verify_position(
    document: &PropsDocument,
    key: &str,
    position: &Position,
    index: usize,
) -> Result<(), PropsError> {
    let keys: Vec<&str> = document
        .entries
        .iter()
        .map(|entry| entry.key.as_str())
        .collect();
    let Some(key_at) = keys.iter().position(|item| *item == key) else {
        return Err(verify_failed(
            &format!("operation did not leave {key:?}"),
            Some(key.to_owned()),
            Some(index),
        ));
    };
    match position {
        Position::Before(neighbor) => {
            let Some(neighbor_at) =
                keys.iter().position(|item| *item == neighbor.as_str())
            else {
                return Err(verify_failed(
                    &format!("position neighbor {neighbor:?} is missing"),
                    Some(neighbor.clone()),
                    Some(index),
                ));
            };
            if key_at + 1 != neighbor_at {
                return Err(verify_failed(
                    &format!("{key:?} is not before {neighbor:?}"),
                    Some(key.to_owned()),
                    Some(index),
                ));
            }
        }
        Position::After(neighbor) => {
            let Some(neighbor_at) =
                keys.iter().position(|item| *item == neighbor.as_str())
            else {
                return Err(verify_failed(
                    &format!("position neighbor {neighbor:?} is missing"),
                    Some(neighbor.clone()),
                    Some(index),
                ));
            };
            if neighbor_at + 1 != key_at {
                return Err(verify_failed(
                    &format!("{key:?} is not after {neighbor:?}"),
                    Some(key.to_owned()),
                    Some(index),
                ));
            }
        }
    }
    Ok(())
}

fn yaml_semantically_eq(
    left: &serde_yaml::Value,
    right: &serde_yaml::Value,
) -> bool {
    match (left, right) {
        (serde_yaml::Value::String(left), serde_yaml::Value::String(right)) => {
            datetime_canon(left)
                .zip(datetime_canon(right))
                .is_some_and(|(left, right)| left == right)
                || left == right
        }
        (
            serde_yaml::Value::Sequence(left),
            serde_yaml::Value::Sequence(right),
        ) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| yaml_semantically_eq(left, right))
        }
        (
            serde_yaml::Value::Mapping(left),
            serde_yaml::Value::Mapping(right),
        ) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|other| yaml_semantically_eq(value, other))
                })
        }
        (serde_yaml::Value::Tagged(left), serde_yaml::Value::Tagged(right)) => {
            left.tag == right.tag
                && yaml_semantically_eq(&left.value, &right.value)
        }
        _ => left == right,
    }
}

fn json_semantically_eq(left: &JsonValue, right: &JsonValue) -> bool {
    match (left, right) {
        (JsonValue::String(left), JsonValue::String(right)) => {
            datetime_canon(left)
                .zip(datetime_canon(right))
                .is_some_and(|(left, right)| left == right)
                || left == right
        }
        (JsonValue::Array(left), JsonValue::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| json_semantically_eq(left, right))
        }
        (JsonValue::Object(left), JsonValue::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|other| json_semantically_eq(value, other))
                })
        }
        _ => left == right,
    }
}

fn datetime_canon(text: &str) -> Option<String> {
    if !is_datetime(text) {
        return None;
    }
    let (_, offset) = datetime_parts(text)?;
    let date = &text[..10];
    let rest = &text[11..];
    let time = if offset.is_empty() {
        rest
    } else {
        rest.strip_suffix(&offset).unwrap_or(rest)
    };
    Some(format!("{date}T{time}{}", canon_offset(&offset)))
}

fn canon_offset(offset: &str) -> String {
    if offset.is_empty() {
        return String::new();
    }
    if offset == "Z" || offset == "+0000" || offset == "+00:00" {
        return "Z".to_owned();
    }
    let Some(sign) = offset.chars().next() else {
        return offset.to_owned();
    };
    let digits: String = offset
        .chars()
        .skip(1)
        .filter(char::is_ascii_digit)
        .collect();
    if digits.len() < 4 {
        return offset.to_owned();
    }
    format!("{sign}{}:{}", &digits[..2], &digits[2..4])
}

fn touched_keys(ops: &[Op]) -> Vec<String> {
    let mut keys = Vec::new();
    for op in ops {
        keys.push(op.key().to_owned());
        if let Op::Rename { to, .. } = op {
            keys.push(to.clone());
        }
    }
    keys
}

fn refuse_unwritable(
    document: &PropsDocument,
    op_index: Option<usize>,
) -> Result<(), PropsError> {
    match document.state {
        FrontmatterState::Ok | FrontmatterState::Absent => Ok(()),
        FrontmatterState::Invalid | FrontmatterState::Unclosed => {
            let mut error =
                document.error.clone().unwrap_or_else(|| PropsError {
                    code: "prop_invalid_yaml".to_owned(),
                    message: "frontmatter YAML cannot be edited".to_owned(),
                    hint:
                        "Repair the frontmatter YAML before editing properties."
                            .to_owned(),
                    key: None,
                    line: document.start_line,
                    op_index,
                });
            error.op_index = op_index.or(error.op_index);
            Err(error)
        }
    }
}

fn refuse_protected(
    context: &InspectContext,
    key: &str,
    index: usize,
) -> Result<(), PropsError> {
    let classified = context.roles.classify(key);
    match classified.role {
        Some(Role::Loose) => Ok(()),
        Some(Role::Card) => Err(PropsError {
            code: "prop_owned".to_owned(),
            message: format!(
                "property {key:?} is owned by {}",
                classified.owner.as_deref().unwrap_or("a card")
            ),
            hint: classified.editor.clone().unwrap_or_else(|| "==".to_owned()),
            key: Some(key.to_owned()),
            line: None,
            op_index: Some(index),
        }),
        Some(Role::Derived) => Err(PropsError {
            code: "prop_derived".to_owned(),
            message: format!("property {key:?} is derived"),
            hint: classified.reason.clone().unwrap_or_else(|| {
                "Edit the source command that owns this field.".to_owned()
            }),
            key: Some(key.to_owned()),
            line: None,
            op_index: Some(index),
        }),
        Some(Role::Identity) => Err(PropsError {
            code: "prop_identity".to_owned(),
            message: format!("property {key:?} identifies this note"),
            hint: classified.reason.clone().unwrap_or_else(|| {
                "Identity keys cannot be changed here.".to_owned()
            }),
            key: Some(key.to_owned()),
            line: None,
            op_index: Some(index),
        }),
        None => Err(PropsError {
            code: "prop_ownership_unresolved".to_owned(),
            message: classified.diagnostic.unwrap_or_else(|| {
                format!("ownership of {key:?} could not be resolved")
            }),
            hint:
                "Resolve note and schema ownership before editing properties."
                    .to_owned(),
            key: Some(key.to_owned()),
            line: None,
            op_index: Some(index),
        }),
    }
}

fn refuse_opaque(
    entry: &PropertyEntry,
    index: usize,
) -> Result<(), PropsError> {
    if !entry.opaque {
        return Ok(());
    }
    Err(PropsError {
        code: "prop_opaque".to_owned(),
        message: entry
            .diagnostic
            .clone()
            .unwrap_or_else(|| format!("property {:?} is opaque", entry.key)),
        hint: "Repair or edit this YAML value outside the property writer."
            .to_owned(),
        key: Some(entry.key.clone()),
        line: entry.line,
        op_index: Some(index),
    })
}

fn refuse_merge_siblings(
    document: &PropsDocument,
    key: &str,
    index: usize,
) -> Result<(), PropsError> {
    let Some(merge) = document.entries.iter().find(|entry| entry.key == "<<")
    else {
        return Ok(());
    };
    if merge.key == key {
        return Ok(());
    }
    Err(PropsError {
        code: "prop_opaque".to_owned(),
        message: "a YAML merge key affects sibling property semantics"
            .to_owned(),
        hint: "Remove the merge key before editing other properties."
            .to_owned(),
        key: Some(key.to_owned()),
        line: merge.line,
        op_index: Some(index),
    })
}

fn validate_key(key: &str, index: usize) -> Result<(), PropsError> {
    if key.is_empty()
        || key.contains('\n')
        || key.contains('\r')
        || key.contains('\u{feff}')
    {
        return Err(PropsError {
            code: "prop_bad_key".to_owned(),
            message: format!("invalid property key {key:?}"),
            hint: "Keys must be non-empty single-line strings.".to_owned(),
            key: Some(key.to_owned()),
            line: None,
            op_index: Some(index),
        });
    }
    Ok(())
}

fn find_entry<'a>(
    document: &'a PropsDocument,
    key: &str,
) -> Option<&'a PropertyEntry> {
    document.entries.iter().find(|entry| entry.key == key)
}

fn require_entry<'a>(
    document: &'a PropsDocument,
    key: &str,
    index: usize,
) -> Result<(usize, &'a PropertyEntry), PropsError> {
    document
        .entries
        .iter()
        .enumerate()
        .find(|(_, entry)| entry.key == key)
        .ok_or_else(|| PropsError {
            code: "prop_missing".to_owned(),
            message: format!("property {key:?} does not exist"),
            hint: "Use prop.set to create a missing key.".to_owned(),
            key: Some(key.to_owned()),
            line: None,
            op_index: Some(index),
        })
}

fn require_neighbor<'a>(
    document: &'a PropsDocument,
    key: &str,
    index: usize,
) -> Result<(usize, &'a PropertyEntry), PropsError> {
    document
        .entries
        .iter()
        .enumerate()
        .find(|(_, entry)| entry.key == key)
        .ok_or_else(|| PropsError {
            code: "prop_missing".to_owned(),
            message: format!("position neighbor {key:?} does not exist"),
            hint: "Name an existing neighbor for before/after.".to_owned(),
            key: Some(key.to_owned()),
            line: None,
            op_index: Some(index),
        })
}

fn replace_range(
    source: &str,
    range: Range<usize>,
    replacement: &str,
) -> String {
    let mut out = String::with_capacity(source.len() + replacement.len());
    out.push_str(&source[..range.start]);
    out.push_str(replacement);
    out.push_str(&source[range.end..]);
    out
}

fn insert_at(source: &str, offset: usize, text: &str) -> String {
    let mut out = String::with_capacity(source.len() + text.len());
    out.push_str(&source[..offset]);
    out.push_str(text);
    out.push_str(&source[offset..]);
    out
}

fn ensure_trailing_newline(text: &str, newline: &str) -> String {
    if text.ends_with('\n') || text.ends_with('\r') {
        text.to_owned()
    } else {
        format!("{text}{newline}")
    }
}

fn verify_failed(
    message: &str,
    key: Option<String>,
    op_index: Option<usize>,
) -> PropsError {
    PropsError {
        code: "prop_verify_failed".to_owned(),
        message: message.to_owned(),
        hint:
            "The planned candidate did not reparse to the expected properties."
                .to_owned(),
        key,
        line: None,
        op_index,
    }
}

fn bad_restore(key: &str, index: usize, message: &str) -> PropsError {
    PropsError {
        code: "prop_bad_value".to_owned(),
        message: message.to_owned(),
        hint:
            "raw must be the complete entry for this key, or null for absence."
                .to_owned(),
        key: Some(key.to_owned()),
        line: None,
        op_index: Some(index),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{plan, PlanOutcome};
    use crate::native::props::{
        document::PropsDocument,
        protocol::{parse_envelope, Op},
        roles::{NoteKind, RoleContext},
        InspectContext, PropsError,
    };

    fn context(note_kind: NoteKind) -> InspectContext {
        InspectContext {
            contract: None,
            normalized_path: "notes/example.md".to_owned(),
            roles: RoleContext {
                note_kind,
                record_fields: Some(BTreeSet::new()),
                reference_has_task: false,
                diagnostics: Vec::new(),
            },
        }
    }

    fn plan_ops(
        note: &str,
        kind: NoteKind,
        ops: &str,
    ) -> Result<PlanOutcome, PropsError> {
        let document = PropsDocument::parse(note);
        let expect = document
            .fingerprint
            .clone()
            .unwrap_or_else(|| "sha256:invalid".to_owned());
        let envelope = parse_envelope(&format!(
            "{{\"schema_version\":1,\"expect\":{expect:?},\"ops\":[{ops}]}}"
        ))
        .expect("envelope");
        plan(note, &context(kind), &envelope)
    }

    fn assert_inverse_restores(
        note: &str,
        kind: NoteKind,
        outcome: &PlanOutcome,
    ) {
        if outcome.inverse_ops.is_empty() {
            assert_eq!(outcome.candidate, note);
            return;
        }
        let expect = PropsDocument::parse(&outcome.candidate)
            .fingerprint
            .expect("candidate fingerprint");
        let ops = outcome
            .inverse_ops
            .iter()
            .map(Op::to_json)
            .collect::<Vec<_>>();
        let envelope = parse_envelope(
            &serde_json::json!({
                "schema_version": 1,
                "expect": expect,
                "ops": ops,
            })
            .to_string(),
        )
        .expect("inverse envelope");
        let undone = plan(&outcome.candidate, &context(kind), &envelope)
            .expect("inverse plan");
        assert_eq!(
            undone.candidate, note,
            "inverse must restore original bytes"
        );
    }

    #[test]
    fn empty_ops_are_a_byte_noop() {
        let note = "---\ncreated: 2026-11-22\n---\nbody\n";
        let outcome = plan_ops(note, NoteKind::Note, "").unwrap();
        assert!(!outcome.changed);
        assert_eq!(outcome.candidate, note);
        assert!(outcome.inverse_ops.is_empty());
    }

    #[test]
    fn set_replace_keeps_comments_and_undoes() {
        let note =
            "---\n# keep\ncreated: 2026-11-22 # stamp\nname: old\n---\nbody\n";
        let outcome = plan_ops(
            note,
            NoteKind::Note,
            r#"{"op":"prop.set","key":"name","value":"new"}"#,
        )
        .unwrap();
        assert!(outcome.candidate.contains("name: new"));
        assert!(outcome.candidate.contains("# keep"));
        assert!(outcome.candidate.contains("# stamp"));
        assert!(outcome.candidate.contains("created: 2026-11-22"));
        assert_inverse_restores(note, NoteKind::Note, &outcome);
    }

    #[test]
    fn semantic_noop_set_keeps_original_bytes() {
        let note = "---\nname: old\n---\n";
        let outcome = plan_ops(
            note,
            NoteKind::Note,
            r#"{"op":"prop.set","key":"name","value":"old"}"#,
        )
        .unwrap();
        assert_eq!(outcome.candidate, note);
        assert!(!outcome.changed);
    }

    #[test]
    fn insert_and_undo_create_frontmatter() {
        let note = "\u{feff}# Body\n";
        let outcome = plan_ops(
            note,
            NoteKind::Note,
            r#"{"op":"prop.set","key":"created","value":"2026-11-22","kind":"date"}"#,
        )
        .unwrap();
        assert!(outcome.candidate.starts_with('\u{feff}'));
        assert!(outcome.candidate.contains("created: 2026-11-22"));
        assert!(outcome.candidate.contains("# Body"));
        assert_inverse_restores(note, NoteKind::Note, &outcome);
    }

    #[test]
    fn unset_preserves_empty_authored_frontmatter() {
        let note = "---\n# authored\ncreated: 2026-11-22\n---\nbody\n";
        let outcome = plan_ops(
            note,
            NoteKind::Note,
            r#"{"op":"prop.unset","key":"created"}"#,
        )
        .unwrap();
        assert!(outcome.candidate.starts_with("---\n# authored\n---\n"));
        assert_inverse_restores(note, NoteKind::Note, &outcome);
    }

    #[test]
    fn rename_preserves_value_bytes() {
        let note = "---\n'10': ten # keep\n---\n";
        let outcome = plan_ops(
            note,
            NoteKind::Note,
            r#"{"op":"prop.rename","key":"10","to":"2"}"#,
        )
        .unwrap();
        assert!(
            outcome.candidate.contains("\"2\": ten # keep")
                || outcome.candidate.contains("'2': ten # keep")
                || outcome.candidate.contains("2: ten # keep")
        );
        assert!(outcome.candidate.contains("# keep"));
        assert_inverse_restores(note, NoteKind::Note, &outcome);
    }

    #[test]
    fn move_keeps_leading_comments() {
        let note =
            "---\ncreated: 2026-11-22\n# title comment\nname: old\n---\n";
        let outcome = plan_ops(
            note,
            NoteKind::Note,
            r#"{"op":"prop.move","key":"name","before":"created"}"#,
        )
        .unwrap();
        let name_at = outcome.candidate.find("name: old").unwrap();
        let created_at = outcome.candidate.find("created:").unwrap();
        assert!(name_at < created_at);
        assert!(outcome.candidate.contains("# title comment"));
        assert_inverse_restores(note, NoteKind::Note, &outcome);
    }

    #[test]
    fn later_failure_discards_the_candidate() {
        let note = "---\ncreated: 2026-11-22\n---\n";
        let error = plan_ops(
            note,
            NoteKind::Note,
            r#"{"op":"prop.set","key":"name","value":"x"},{"op":"prop.unset","key":"missing"}"#,
        )
        .expect_err("second op missing");
        assert_eq!(error.code, "prop_missing");
        assert_eq!(error.op_index, Some(1));
    }

    #[test]
    fn identity_and_card_keys_are_refused_for_every_op() {
        let note =
            "---\ntype: [[day]]\nsummary: draft\ncreated: 2026-11-22\n---\n";
        for ops in [
            r#"{"op":"prop.set","key":"type","value":"other"}"#,
            r#"{"op":"prop.unset","key":"type"}"#,
            r#"{"op":"prop.rename","key":"type","to":"kind"}"#,
            r#"{"op":"prop.move","key":"type","after":"created"}"#,
            r#"{"op":"prop.set_yaml","key":"type","yaml":"other"}"#,
            r#"{"op":"prop.restore","key":"type","raw":"type: other\n"}"#,
        ] {
            let error = plan_ops(note, NoteKind::Day, ops).expect_err(ops);
            assert_eq!(error.code, "prop_identity", "{ops}");
        }
        let error = plan_ops(
            note,
            NoteKind::Day,
            r#"{"op":"prop.set","key":"summary","value":"nope"}"#,
        )
        .expect_err("card");
        assert_eq!(error.code, "prop_owned");
        assert_eq!(error.hint, "==");
    }

    #[test]
    fn invalid_yaml_and_duplicates_refuse_update() {
        let invalid = plan_ops("---\nname: [\n---\n", NoteKind::Note, "")
            .expect_err("invalid");
        assert_eq!(invalid.code, "prop_invalid_yaml");
        let duplicate =
            plan_ops("---\nname: a\nname: b\n---\n", NoteKind::Note, "")
                .expect_err("duplicate");
        assert_eq!(duplicate.code, "prop_duplicate_key");
    }

    #[test]
    fn header_comment_on_block_scalar_survives_replace() {
        let note =
            "---\nx: |- # inline\n  hello\n  world\ncreated: 2026-11-22\n---\n";
        let scalar = plan_ops(
            note,
            NoteKind::Note,
            r#"{"op":"prop.set","key":"x","value":"new"}"#,
        )
        .unwrap();
        assert_eq!(
            scalar.candidate,
            "---\nx: new # inline\ncreated: 2026-11-22\n---\n"
        );
        assert_inverse_restores(note, NoteKind::Note, &scalar);

        let multiline = plan_ops(
            note,
            NoteKind::Note,
            r#"{"op":"prop.set","key":"x","value":"hello\nworld","kind":"text"}"#,
        )
        .unwrap();
        assert_eq!(
            multiline.candidate,
            "---\nx: |- # inline\n  hello\n  world\ncreated: 2026-11-22\n---\n"
        );
        assert_inverse_restores(note, NoteKind::Note, &multiline);
    }

    #[test]
    fn unquoted_wikilink_keeps_parsed_meaning_when_untouched() {
        let note = "---\nparent: [[area]]\ncreated: 2026-11-22\n---\n";
        let outcome = plan_ops(
            note,
            NoteKind::Note,
            r#"{"op":"prop.set","key":"created","value":"2026-11-23","kind":"date"}"#,
        )
        .unwrap();
        assert!(outcome.candidate.contains("parent: [[area]]"));
        let original = PropsDocument::parse(note);
        let candidate = PropsDocument::parse(&outcome.candidate);
        assert_eq!(
            original.entries[0].raw_value,
            candidate.entries[0].raw_value
        );
        assert_inverse_restores(note, NoteKind::Note, &outcome);
    }
}
