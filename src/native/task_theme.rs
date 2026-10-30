//! Canonical `[type::]` / `(type::)` field parse, placement, and write rule.
//!
//! `type` is canonical and `theme` is a legacy alias still read. Every
//! writer emits `[type::]` and removes both spellings.
//!
//! Kept independent of suggestion types so later capture writers can call
//! these helpers without a module cycle.

use std::collections::BTreeSet;

use super::{
    plan_budget::{normalize_component, split_components},
    ref_tasks::strip_blockquote_prefix,
    task_fields::{
        all_inline_fields, canonical_theme_name, inline_fields, InlineField,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RepairKind {
    Duplicate,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskThemeState {
    None,
    Set { value: String, key: String },
    Repair { kind: RepairKind },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ThemeChange {
    #[allow(dead_code)] // write_rule vectors until closeout
    None,
    Set {
        name: String,
        key: String,
    },
    Clear,
}

pub(crate) fn parse_task_theme(line: &str) -> TaskThemeState {
    let mut fields = inline_fields(line, "type");
    fields.extend(inline_fields(line, "theme"));
    fields.sort_by_key(|field| field.start);
    match fields.as_slice() {
        [] => TaskThemeState::None,
        [field] => match canonical_theme_name(&field.value) {
            Some(value) => {
                let key = component_key(&value);
                TaskThemeState::Set { value, key }
            }
            None => TaskThemeState::Repair {
                kind: RepairKind::Invalid,
            },
        },
        _ => TaskThemeState::Repair {
            kind: RepairKind::Duplicate,
        },
    }
}

pub(crate) fn set_task_theme(line: &str, name: &str) -> String {
    let (body, ending) = split_line_ending(line);
    let cleared = remove_theme_fields(body);
    let insertion = trailing_metadata_start(&cleared);
    let mut insert = format!("[type:: {name}]");
    if insertion > 0 && !cleared[..insertion].ends_with([' ', '\t']) {
        insert.insert(0, ' ');
    }
    if insertion < cleared.len()
        && !cleared[insertion..].starts_with([' ', '\t'])
    {
        insert.push(' ');
    }
    let mut output =
        String::with_capacity(cleared.len() + insert.len() + ending.len());
    output.push_str(&cleared[..insertion]);
    output.push_str(&insert);
    output.push_str(&cleared[insertion..]);
    output.push_str(ending);
    output
}

pub(crate) fn clear_task_theme(line: &str) -> String {
    let (body, ending) = split_line_ending(line);
    let mut output = remove_theme_fields(body);
    output.push_str(ending);
    output
}

#[allow(dead_code)] // write_rule vectors until closeout
pub(crate) fn plan_theme_change(
    field: &TaskThemeState,
    shown_suggestion_key: Option<&str>,
    inherited_key: Option<&str>,
    destination_name: &str,
    exempt_keys: &BTreeSet<String>,
    deliberate: bool,
) -> ThemeChange {
    if !deliberate {
        return ThemeChange::None;
    }
    let Some(chosen) =
        single_non_exempt_component(destination_name, exempt_keys)
    else {
        return ThemeChange::None;
    };
    if shown_suggestion_key.is_none() {
        return ThemeChange::None;
    }
    let chosen_key = component_key(&chosen);
    if inherited_key == Some(chosen_key.as_str()) {
        return match field {
            TaskThemeState::None => ThemeChange::None,
            _ => ThemeChange::Clear,
        };
    }
    if let TaskThemeState::Set { key, .. } = field
        && key == &chosen_key
    {
        return ThemeChange::None;
    }
    ThemeChange::Set {
        name: chosen,
        key: chosen_key,
    }
}

#[allow(dead_code)] // write_rule vectors until closeout
fn single_non_exempt_component(
    destination_name: &str,
    exempt_keys: &BTreeSet<String>,
) -> Option<String> {
    let trimmed = destination_name.trim();
    if trimmed.is_empty() || trimmed == "(unnamed)" {
        return None;
    }
    let non_exempt: Vec<String> = split_components(trimmed)
        .into_iter()
        .filter(|component| !exempt_keys.contains(&component_key(component)))
        .collect();
    let [component] = non_exempt.as_slice() else {
        return None;
    };
    canonical_theme_name(component)
}

fn component_key(value: &str) -> String {
    normalize_component(value)
}

fn split_line_ending(line: &str) -> (&str, &str) {
    if let Some(body) = line.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = line.strip_suffix('\n') {
        (body, "\n")
    } else if let Some(body) = line.strip_suffix('\r') {
        (body, "\r")
    } else {
        (line, "")
    }
}

fn remove_theme_fields(line: &str) -> String {
    let mut fields = inline_fields(line, "type");
    fields.extend(inline_fields(line, "theme"));
    if fields.is_empty() {
        return line.to_string();
    }
    let mut ranges: Vec<(usize, usize)> = fields
        .iter()
        .map(|field| one_space_removal_range(line, field))
        .collect();
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.0 <= last.1
        {
            last.1 = last.1.max(range.1);
        } else {
            merged.push(range);
        }
    }
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0;
    for (start, end) in merged {
        output.push_str(&line[cursor..start]);
        cursor = end;
    }
    output.push_str(&line[cursor..]);
    output
}

fn one_space_removal_range(line: &str, field: &InlineField) -> (usize, usize) {
    let bytes = line.as_bytes();
    if field.end < bytes.len() && matches!(bytes[field.end], b' ' | b'\t') {
        return (field.start, field.end + 1);
    }
    if field.start > 0 && matches!(bytes[field.start - 1], b' ' | b'\t') {
        return (field.start - 1, field.end);
    }
    (field.start, field.end)
}

/// Byte offset of the trailing metadata run: inline fields of any key,
/// tags, and a trailing `^id`, scanned back from the trimmed end. The
/// run does not include the task checkbox or title.
pub(crate) fn trailing_metadata_start(line: &str) -> usize {
    let trimmed_len = line.trim_end().len();
    suffix_start_inner(line).unwrap_or(trimmed_len)
}

fn suffix_start_inner(line: &str) -> Option<usize> {
    let floor = scan_floor(line)?;
    let trimmed_len = line.trim_end().len();
    let mut cursor = trimmed_len;
    let mut leftmost: Option<usize> = None;

    if let Some(block_start) = trailing_block_start(&line[..cursor])
        && block_start >= floor
    {
        leftmost = Some(block_start);
        cursor = block_start;
        cursor = trim_end_to(&line[..cursor], cursor);
    }

    loop {
        if cursor <= floor {
            break;
        }
        let slice = &line[..cursor];
        let trimmed = slice.trim_end();
        let end = trimmed.len();
        if end <= floor {
            break;
        }
        if let Some(tag_start) = trailing_tag_start(trimmed, floor) {
            leftmost = Some(tag_start);
            cursor = tag_start;
            cursor = trim_end_to(&line[..cursor], cursor);
            continue;
        }
        let Some((field_start, _)) = trailing_field_key(trimmed) else {
            break;
        };
        if field_start < floor {
            break;
        }
        leftmost = Some(field_start);
        cursor = field_start;
        cursor = trim_end_to(&line[..cursor], cursor);
    }

    leftmost
}

fn scan_floor(line: &str) -> Option<usize> {
    let stripped = strip_blockquote_prefix(line);
    let offset = line.len() - stripped.len();
    let bytes = stripped.as_bytes();
    let mut index = bytes
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    index = after_list_marker(stripped, index)?;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    let close = stripped[index..].find(']')?;
    let mut body_start = index + close + 1;
    while stripped[body_start..].starts_with([' ', '\t']) {
        body_start += 1;
    }
    let body = &stripped[body_start..];
    if body == "#task"
        || body.starts_with("#task ")
        || body.starts_with("#task\t")
    {
        Some(offset + body_start + "#task".len())
    } else {
        Some(offset + body_start)
    }
}

fn after_list_marker(line: &str, index: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    if matches!(bytes.get(index), Some(b'-' | b'*' | b'+')) {
        return bytes
            .get(index + 1)
            .is_some_and(u8::is_ascii_whitespace)
            .then_some(index + 1);
    }
    let digits = bytes[index..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 || !matches!(bytes.get(index + digits), Some(b'.' | b')')) {
        return None;
    }
    bytes
        .get(index + digits + 1)
        .is_some_and(u8::is_ascii_whitespace)
        .then_some(index + digits + 1)
}

fn trim_end_to(text: &str, mut cursor: usize) -> usize {
    cursor = cursor.min(text.len());
    while cursor > 0 && text[..cursor].ends_with([' ', '\t']) {
        cursor -= 1;
    }
    cursor
}

fn trailing_block_start(text: &str) -> Option<usize> {
    let trimmed = text.trim_end();
    let token_start = trimmed
        .rfind([' ', '\t'])
        .map(|index| index + 1)
        .unwrap_or(0);
    let token = &trimmed[token_start..];
    let id = token.strip_prefix('^')?;
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    if token_start > 0 {
        let before = trimmed[..token_start].chars().next_back()?;
        if before != ' ' && before != '\t' {
            return None;
        }
    }
    let end = text.trim_end().len();
    Some(end - token.len())
}

fn trailing_tag_start(trimmed: &str, floor: usize) -> Option<usize> {
    let token_start = trimmed
        .rfind([' ', '\t'])
        .map(|index| index + 1)
        .unwrap_or(0);
    let start = token_start.max(floor);
    let token = trimmed.get(start..)?;
    if !token.starts_with('#') {
        return None;
    }
    if start > floor {
        let before = trimmed[..start].chars().next_back()?;
        if before != ' ' && before != '\t' {
            return None;
        }
    }
    let value = token.strip_prefix('#')?;
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || "!@#$%^&*(),.?\":{}|<>".contains(c))
    {
        return None;
    }
    Some(start)
}

fn trailing_field_key(trimmed: &str) -> Option<(usize, String)> {
    let mut end = trimmed.len();
    if trimmed.ends_with(',') {
        end -= 1;
        end = trimmed[..end].trim_end().len();
    }
    let slice = &trimmed[..end];
    let field = all_inline_fields(slice).into_iter().last()?;
    (field.end == slice.len()).then_some((field.start, field.key))
}
