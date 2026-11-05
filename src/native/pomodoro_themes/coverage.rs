//! Note-default and task-field coverage for `bob types rename` / `split`.
//!
//! Rename rewrites matching `task_type:` / `theme:` frontmatter on
//! area/project notes and `[type::]` / `(type::)` plus legacy `[theme::]` /
//! `(theme::)` fields on task lines. Split leaves both alone
//! and warns with leftover counts.

use std::{fs, path::Path};

use crate::native::{
    capture_dependency_tasks,
    markdown::split_line_ending,
    note_tasks::{self, NoteTaskSettings},
    plan_budget::normalize_component,
    pomodoro_homes::{NoteFacts, NoteKind, NoteTheme},
    projects::trim_yaml_scalar,
    task_fields::{canonical_theme_name, inline_fields},
};

fn raw_lines_preserving(contents: &str) -> Vec<String> {
    contents.split_inclusive('\n').map(str::to_string).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LineEdit {
    pub(super) line: usize,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) before: String,
    pub(super) after: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct LeftoverThemeUses {
    pub(super) defaults: usize,
    pub(super) default_notes: Vec<String>,
    pub(super) task_themes: usize,
    pub(super) offclock: usize,
    pub(super) offclock_notes: Vec<String>,
}

pub(super) fn leftover_theme_uses(
    vault: &Path,
    old_key: &str,
) -> LeftoverThemeUses {
    let settings = note_tasks::read_settings(vault);
    let (paths, _) = capture_dependency_tasks::vault_note_paths(vault);
    let mut uses = LeftoverThemeUses::default();
    for relative in paths {
        let absolute = vault.join(&relative);
        let Ok(contents) = fs::read_to_string(&absolute) else {
            continue;
        };
        let path = capture_dependency_tasks::display_path(&relative);
        if !default_edits(&contents, old_key, "").is_empty() {
            uses.defaults += 1;
            uses.default_notes.push(path.clone());
        }
        uses.task_themes +=
            task_theme_edits(&contents, old_key, "", &settings).len();
        if offclock_names_old(&contents, old_key) {
            uses.offclock += 1;
            uses.offclock_notes.push(path);
        }
    }
    uses
}

#[allow(dead_code)]
pub(super) fn default_edit(
    contents: &str,
    old_key: &str,
    new_display: &str,
) -> Option<LineEdit> {
    default_edits(contents, old_key, new_display)
        .into_iter()
        .next()
}

pub(super) fn default_edits(
    contents: &str,
    old_key: &str,
    new_display: &str,
) -> Vec<LineEdit> {
    let facts = NoteFacts::parse(contents);
    match (facts.kind, facts.theme) {
        (NoteKind::Area | NoteKind::Project, Some(NoteTheme::Name(name)))
            if normalize_component(&name) == old_key => {}
        _ => return Vec::new(),
    }
    let owned = raw_lines_preserving(contents);
    let mut edits = Vec::new();
    for (index, raw) in owned.iter().enumerate() {
        if index == 0 {
            continue;
        }
        let (body, _) = split_line_ending(raw);
        if body.trim() == "---" {
            break;
        }
        for key in ["task_type", "theme"] {
            let Some((start, end, before)) =
                frontmatter_default_value_span(body, key, old_key)
            else {
                continue;
            };
            edits.push(LineEdit {
                line: index + 1,
                start,
                end,
                before,
                after: new_display.to_string(),
            });
        }
    }
    edits
}

pub(super) fn leftover_warning(
    old_name: &str,
    uses: &LeftoverThemeUses,
) -> Option<String> {
    if uses.defaults == 0 && uses.task_themes == 0 && uses.offclock == 0 {
        return None;
    }
    let mut parts = Vec::new();
    if uses.defaults > 0 {
        parts.push(format!(
            "{} default{}",
            uses.defaults,
            if uses.defaults == 1 { "" } else { "s" }
        ));
    }
    if uses.task_themes > 0 {
        parts.push(format!(
            "{} task field{}",
            uses.task_themes,
            if uses.task_themes == 1 { "" } else { "s" }
        ));
    }
    if uses.offclock > 0 {
        parts.push(format!(
            "{} offclock list{}",
            uses.offclock,
            if uses.offclock == 1 { "" } else { "s" }
        ));
    }
    Some(format!(
        "split_leaves_task_themes: {} still name {old_name}",
        parts.join(", ")
    ))
}

pub(super) fn task_theme_edits(
    contents: &str,
    old_key: &str,
    new_display: &str,
    settings: &NoteTaskSettings,
) -> Vec<LineEdit> {
    let scan = note_tasks::scan(contents, settings);
    let owned = raw_lines_preserving(contents);
    let mut edits = Vec::new();
    for task in scan.tasks() {
        let Some(raw) = owned.get(task.line_index) else {
            continue;
        };
        let (body, _) = split_line_ending(raw);
        let mut fields = inline_fields(body, "type");
        fields.extend(inline_fields(body, "theme"));
        fields.sort_by_key(|field| field.value_start);
        for field in fields {
            let Some(name) = canonical_theme_name(&field.value) else {
                continue;
            };
            if normalize_component(&name) != old_key {
                continue;
            }
            edits.push(LineEdit {
                line: task.line_index + 1,
                start: field.value_start,
                end: field.value_end,
                before: field.value,
                after: new_display.to_string(),
            });
        }
    }
    edits
}

#[allow(dead_code)]
pub(super) fn apply_line_edits(contents: &str, edits: &[LineEdit]) -> String {
    apply_coverage_edits(contents, edits, &[])
}

/// Preview display for one `offclock_types` item change. `after` is the
/// new display name, or empty when the item is removed by a merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OffclockChange {
    pub(super) line: usize,
    pub(super) before: String,
    pub(super) after: String,
}

/// Span edits plus whole-line removals for `offclock_types` renames.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct OffclockResult {
    pub(super) edits: Vec<LineEdit>,
    pub(super) removed_lines: Vec<usize>,
    pub(super) changes: Vec<OffclockChange>,
}

pub(super) fn offclock_edits(
    contents: &str,
    old_key: &str,
    new_display: &str,
) -> OffclockResult {
    let mut result = OffclockResult::default();
    let facts = NoteFacts::parse(contents);
    if !matches!(facts.kind, NoteKind::Area | NoteKind::Project) {
        return result;
    }
    let new_key = normalize_component(new_display);
    if new_key.is_empty() || old_key == new_key {
        return result;
    }
    let owned = raw_lines_preserving(contents);
    let key_index = find_offclock_key_line(&owned);
    let Some(key_index) = key_index else {
        return result;
    };
    let (key_body, _) = split_line_ending(&owned[key_index]);
    let key_line_no = key_index + 1;
    let rest = key_body
        .strip_prefix("offclock_types:")
        .unwrap_or("")
        .to_string();
    let rest_trimmed = rest.trim().to_string();
    let rest_without_comment =
        strip_offclock_comment(&rest_trimmed).trim().to_string();
    if rest_without_comment.is_empty() {
        collect_block_offclock_edits(
            &owned,
            key_index,
            old_key,
            new_display,
            &new_key,
            &mut result,
        );
        return result;
    }
    if rest_trimmed.starts_with('[') {
        collect_flow_offclock_edits(
            key_body,
            key_line_no,
            old_key,
            new_display,
            &new_key,
            &mut result,
        );
        return result;
    }
    if rest_trimmed.starts_with('{') {
        return result;
    }
    if let Some((start, end, before)) = offclock_scalar_span(key_body, old_key)
    {
        result.edits.push(LineEdit {
            line: key_line_no,
            start,
            end,
            before: before.clone(),
            after: new_display.to_string(),
        });
        result.changes.push(OffclockChange {
            line: key_line_no,
            before,
            after: new_display.to_string(),
        });
    }
    result
}

/// True when an area/project note carries an `offclock_types` item naming
/// `old_key`, in any form. Non-area notes never count.
pub(super) fn offclock_names_old(contents: &str, old_key: &str) -> bool {
    let facts = NoteFacts::parse(contents);
    if !matches!(facts.kind, NoteKind::Area | NoteKind::Project) {
        return false;
    }
    let owned = raw_lines_preserving(contents);
    let Some(key_index) = find_offclock_key_line(&owned) else {
        return false;
    };
    let (key_body, _) = split_line_ending(&owned[key_index]);
    let rest = key_body.strip_prefix("offclock_types:").unwrap_or("");
    let rest_trimmed = rest.trim();
    let rest_without_comment =
        strip_offclock_comment(rest_trimmed).trim().to_string();
    if rest_without_comment.is_empty() {
        return block_offclock_has_old(&owned, key_index, old_key);
    }
    if rest_trimmed.starts_with('[') {
        return flow_offclock_has_old(key_body, old_key);
    }
    if rest_trimmed.starts_with('{') {
        return false;
    }
    offclock_scalar_span(key_body, old_key).is_some()
}

pub(super) fn apply_coverage_edits(
    contents: &str,
    edits: &[LineEdit],
    removed_lines: &[usize],
) -> String {
    if edits.is_empty() && removed_lines.is_empty() {
        return contents.to_string();
    }
    let mut owned = raw_lines_preserving(contents);
    if !edits.is_empty() {
        let mut by_line: Vec<(usize, Vec<&LineEdit>)> = Vec::new();
        for edit in edits {
            match by_line.iter_mut().find(|(line, _)| *line == edit.line) {
                Some((_, group)) => group.push(edit),
                None => by_line.push((edit.line, vec![edit])),
            }
        }
        for (line, mut group) in by_line {
            let index = line.saturating_sub(1);
            let Some(raw) = owned.get(index).cloned() else {
                continue;
            };
            let (body, ending) = split_line_ending(&raw);
            group.sort_by_key(|edit| std::cmp::Reverse(edit.start));
            let mut rewritten = body.to_string();
            for edit in group {
                if edit.end > rewritten.len() || edit.start > edit.end {
                    continue;
                }
                rewritten.replace_range(edit.start..edit.end, &edit.after);
            }
            owned[index] = format!("{rewritten}{ending}");
        }
    }
    if !removed_lines.is_empty() {
        let mut sorted = removed_lines.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        for line in sorted.into_iter().rev() {
            let index = line.saturating_sub(1);
            if index < owned.len() {
                owned.remove(index);
            }
        }
    }
    owned.concat()
}

fn find_offclock_key_line(owned: &[String]) -> Option<usize> {
    for (index, raw) in owned.iter().enumerate() {
        if index == 0 {
            continue;
        }
        let (body, _) = split_line_ending(raw);
        if body.trim() == "---" {
            break;
        }
        if body.strip_prefix("offclock_types:").is_some() {
            return Some(index);
        }
    }
    None
}

fn strip_offclock_comment(value: &str) -> &str {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'#'
            && (index == 0 || matches!(bytes[index - 1], b' ' | b'\t'))
        {
            return value[..index].trim_end();
        }
        index += 1;
    }
    value
}

fn offclock_canonical_key(item: &str) -> Option<String> {
    let trimmed = item.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let inner = if trimmed.len() >= 2
        && ((bytes[0] == b'"' && bytes[trimmed.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[trimmed.len() - 1] == b'\''))
    {
        trimmed[1..trimmed.len() - 1].trim()
    } else {
        trimmed
    };
    if inner.is_empty() || inner.eq_ignore_ascii_case("null") || inner == "~" {
        return None;
    }
    let without_comment = strip_offclock_comment(inner).trim();
    if without_comment.is_empty()
        || without_comment.eq_ignore_ascii_case("null")
        || without_comment == "~"
    {
        return None;
    }
    let canonical = canonical_theme_name(without_comment)?;
    Some(normalize_component(&canonical))
}

fn offclock_inner_span(
    value_part: &str,
    value_start: usize,
) -> Option<(usize, usize, String)> {
    if value_part.is_empty() {
        return None;
    }
    let bytes = value_part.as_bytes();
    if value_part.len() >= 2
        && ((bytes[0] == b'"' && bytes[value_part.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value_part.len() - 1] == b'\''))
    {
        let inner_raw = &value_part[1..value_part.len() - 1];
        let leading = inner_raw.len() - inner_raw.trim_start().len();
        let trailing = inner_raw.len() - inner_raw.trim_end().len();
        let start = value_start + 1 + leading;
        let end = value_start + value_part.len() - 1 - trailing;
        if start > end {
            return None;
        }
        Some((start, end, inner_raw.trim().to_string()))
    } else {
        Some((
            value_start,
            value_start + value_part.len(),
            value_part.to_string(),
        ))
    }
}

fn offclock_scalar_span(
    body: &str,
    old_key: &str,
) -> Option<(usize, usize, String)> {
    let rest = body.strip_prefix("offclock_types:")?;
    let leading = rest.len() - rest.trim_start().len();
    let value_start = "offclock_types:".len() + leading;
    let tail = &body[value_start..];
    let without_comment = strip_offclock_comment(tail);
    let value_part = without_comment.trim_end().to_string();
    if value_part.is_empty() {
        return None;
    }
    // A scalar mapping remnant is never a match.
    if value_part.starts_with(['{', '[']) {
        return None;
    }
    let (inner_start, inner_end, inner) =
        offclock_inner_span(&value_part, value_start)?;
    if inner.is_empty() {
        return None;
    }
    let canonical = canonical_theme_name(&inner)?;
    if normalize_component(&canonical) != old_key {
        return None;
    }
    Some((inner_start, inner_end, inner))
}

#[derive(Debug, Clone)]
struct FlowItem {
    value_start: usize,
    value_end: usize,
    inner_start: usize,
    inner_end: usize,
    inner: String,
    key: Option<String>,
}

fn parse_flow_items(body: &str) -> Option<(usize, usize, Vec<FlowItem>)> {
    let open = body.find('[')?;
    let close = body.rfind(']')?;
    if close < open {
        return None;
    }
    let inner_start = open + 1;
    let inner_end = close;
    let inner = &body[inner_start..inner_end];
    if inner.trim().is_empty() {
        return Some((inner_start, inner_end, Vec::new()));
    }
    // Split on commas outside quotes; quotes are ASCII so byte-wise
    // tracking stays aligned with the delimiters.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut quote: Option<u8> = None;
    let mut start = 0usize;
    for (index, byte) in inner.bytes().enumerate() {
        if let Some(active) = quote {
            if byte == active {
                quote = None;
            }
        } else if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
        } else if byte == b',' {
            ranges.push((start, index));
            start = index + 1;
        }
    }
    ranges.push((start, inner.len()));
    let mut items = Vec::new();
    for (rel_start, rel_end) in ranges {
        let raw_start_body = inner_start + rel_start;
        let raw_end_body = inner_start + rel_end;
        let raw_slice = &body[raw_start_body..raw_end_body];
        let leading = raw_slice.len() - raw_slice.trim_start().len();
        let trailing = raw_slice.len() - raw_slice.trim_end().len();
        if leading + trailing >= raw_slice.len() {
            continue;
        }
        let content_start = raw_start_body + leading;
        let content_end = raw_end_body - trailing;
        let trimmed = &body[content_start..content_end];
        let without_comment = strip_offclock_comment(trimmed).trim_end();
        if without_comment.is_empty() {
            continue;
        }
        let value_start = content_start;
        let value_end = value_start + without_comment.len();
        let value_part = without_comment.to_string();
        let Some((inner_s, inner_e, inner_str)) =
            offclock_inner_span(&value_part, value_start)
        else {
            continue;
        };
        if inner_str.is_empty() {
            continue;
        }
        let key = offclock_canonical_key(&value_part);
        items.push(FlowItem {
            value_start,
            value_end,
            inner_start: inner_s,
            inner_end: inner_e,
            inner: inner_str,
            key,
        });
    }
    Some((inner_start, inner_end, items))
}

fn flow_offclock_has_old(body: &str, old_key: &str) -> bool {
    let Some((_, _, items)) = parse_flow_items(body) else {
        return false;
    };
    items.iter().any(|item| {
        item.key.as_deref().is_some_and(|key| key == old_key)
            && !item.inner.is_empty()
    })
}

fn collect_flow_offclock_edits(
    body: &str,
    line_no: usize,
    old_key: &str,
    new_display: &str,
    new_key: &str,
    result: &mut OffclockResult,
) {
    let Some((_, _, items)) = parse_flow_items(body) else {
        return;
    };
    if items.is_empty() {
        return;
    }
    let matching: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.key.as_deref().is_some_and(|key| key == old_key)
                && !item.inner.is_empty()
        })
        .map(|(index, _)| index)
        .collect();
    if matching.is_empty() {
        return;
    }
    let new_listed = items
        .iter()
        .any(|item| item.key.as_deref().is_some_and(|key| key == new_key));
    if new_listed {
        // Remove every matching item, merging consecutive runs so no
        // stray comma survives. A leading run takes the following
        // separator; any other run takes the preceding one.
        let is_matching = |index: usize| matching.contains(&index);
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for index in matching.clone() {
            match runs.last_mut() {
                Some((_, last)) if *last + 1 == index => *last = index,
                _ => runs.push((index, index)),
            }
        }
        for (first, last) in runs {
            let prev_kept = (0..first).rev().find(|index| !is_matching(*index));
            let next_kept =
                ((last + 1)..items.len()).find(|index| !is_matching(*index));
            let (start, end) = match (prev_kept, next_kept) {
                (None, Some(next)) => {
                    (items[first].value_start, items[next].value_start)
                }
                (Some(prev), _) => {
                    (items[prev].value_end, items[last].value_end)
                }
                (None, None) => continue,
            };
            result.edits.push(LineEdit {
                line: line_no,
                start,
                end,
                before: body[start..end].to_string(),
                after: String::new(),
            });
        }
        for index in matching {
            result.changes.push(OffclockChange {
                line: line_no,
                before: items[index].inner.clone(),
                after: String::new(),
            });
        }
    } else {
        for index in matching {
            let item = &items[index];
            result.edits.push(LineEdit {
                line: line_no,
                start: item.inner_start,
                end: item.inner_end,
                before: item.inner.clone(),
                after: new_display.to_string(),
            });
            result.changes.push(OffclockChange {
                line: line_no,
                before: item.inner.clone(),
                after: new_display.to_string(),
            });
        }
    }
}

#[derive(Debug, Clone)]
struct BlockItem {
    line_no: usize,
    inner_start: usize,
    inner_end: usize,
    inner: String,
    key: Option<String>,
}

fn collect_block_items(owned: &[String], key_index: usize) -> Vec<BlockItem> {
    let mut items = Vec::new();
    for (offset, raw) in owned.iter().enumerate().skip(key_index + 1) {
        let (body, _) = split_line_ending(raw);
        let trimmed_start = body.trim_start();
        if trimmed_start.is_empty() || trimmed_start.starts_with('#') {
            continue;
        }
        let after_dash = if let Some(rest) = trimmed_start.strip_prefix("- ") {
            rest
        } else if let Some(rest) = trimmed_start.strip_prefix("-\t") {
            rest
        } else if trimmed_start == "-" {
            continue;
        } else {
            break;
        };
        let _ = after_dash;
        let leading_indent = body.len() - trimmed_start.len();
        // Dash plus one separator byte when present.
        let item_area_start = if trimmed_start == "-" {
            continue;
        } else {
            leading_indent + 2
        };
        if item_area_start > body.len() {
            continue;
        }
        let tail = &body[item_area_start..];
        let value_leading = tail.len() - tail.trim_start().len();
        let value_start = item_area_start + value_leading;
        let tail_trimmed = tail.trim_start();
        let without_comment =
            strip_offclock_comment(tail_trimmed).trim_end().to_string();
        if without_comment.is_empty() {
            continue;
        }
        let Some((inner_start, inner_end, inner)) =
            offclock_inner_span(&without_comment, value_start)
        else {
            continue;
        };
        if inner.is_empty() {
            continue;
        }
        let key = offclock_canonical_key(&without_comment);
        items.push(BlockItem {
            line_no: offset + 1,
            inner_start,
            inner_end,
            inner,
            key,
        });
    }
    items
}

fn block_offclock_has_old(
    owned: &[String],
    key_index: usize,
    old_key: &str,
) -> bool {
    collect_block_items(owned, key_index)
        .iter()
        .any(|item| item.key.as_deref().is_some_and(|key| key == old_key))
}

fn collect_block_offclock_edits(
    owned: &[String],
    key_index: usize,
    old_key: &str,
    new_display: &str,
    new_key: &str,
    result: &mut OffclockResult,
) {
    let items = collect_block_items(owned, key_index);
    if items.is_empty() {
        return;
    }
    let matching: Vec<&BlockItem> = items
        .iter()
        .filter(|item| item.key.as_deref().is_some_and(|key| key == old_key))
        .collect();
    if matching.is_empty() {
        return;
    }
    let new_listed = items
        .iter()
        .any(|item| item.key.as_deref().is_some_and(|key| key == new_key));
    if new_listed {
        for item in matching {
            result.removed_lines.push(item.line_no);
            result.changes.push(OffclockChange {
                line: item.line_no,
                before: item.inner.clone(),
                after: String::new(),
            });
        }
    } else {
        for item in matching {
            result.edits.push(LineEdit {
                line: item.line_no,
                start: item.inner_start,
                end: item.inner_end,
                before: item.inner.clone(),
                after: new_display.to_string(),
            });
            result.changes.push(OffclockChange {
                line: item.line_no,
                before: item.inner.clone(),
                after: new_display.to_string(),
            });
        }
    }
}

fn frontmatter_default_value_span(
    line: &str,
    key: &str,
    old_key: &str,
) -> Option<(usize, usize, String)> {
    let rest = line.strip_prefix(&format!("{key}:"))?;
    let leading = rest.len().saturating_sub(rest.trim_start().len());
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (inner, quote_prefix) = quoted_scalar(trimmed);
    let name = canonical_theme_name(inner)?;
    if normalize_component(&name) != old_key {
        return None;
    }
    let start = key.len() + 1 + leading + quote_prefix;
    let end = start + inner.len();
    Some((start, end, inner.to_string()))
}

fn quoted_scalar(trimmed: &str) -> (&str, usize) {
    let inner = trim_yaml_scalar(trimmed);
    if inner.len() + 2 == trimmed.len()
        && (trimmed.starts_with('"') && trimmed.ends_with('"')
            || trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        (inner, 1)
    } else {
        (inner, 0)
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    use std::path::Path;

    fn settings() -> NoteTaskSettings {
        note_tasks::read_settings(Path::new("/nonexistent"))
    }

    #[test]
    fn default_edit_preserves_quote_styles() {
        let unquoted = "---\ntype: [[area]]\ntheme: BUILD\n---\n";
        let double = "---\ntype: [[project]]\ntheme: \"BUILD\"\n---\n";
        let single = "---\ntype: [[area]]\ntheme: 'BUILD'\n---\n";
        let padded = "---\ntype: [[area]]\ntheme:  BUILD  \n---\n";
        let edit = default_edit(unquoted, "build", "FIX").expect("unquoted");
        assert_eq!(edit.line, 3);
        assert_eq!(edit.before, "BUILD");
        assert_eq!(
            apply_line_edits(unquoted, &[edit]),
            "---\ntype: [[area]]\ntheme: FIX\n---\n"
        );
        let edit = default_edit(double, "build", "FIX").expect("double");
        assert_eq!(
            apply_line_edits(double, &[edit]),
            "---\ntype: [[project]]\ntheme: \"FIX\"\n---\n"
        );
        let edit = default_edit(single, "build", "FIX").expect("single");
        assert_eq!(
            apply_line_edits(single, &[edit]),
            "---\ntype: [[area]]\ntheme: 'FIX'\n---\n"
        );
        let edit = default_edit(padded, "build", "FIX").expect("padded");
        assert_eq!(
            apply_line_edits(padded, &[edit]),
            "---\ntype: [[area]]\ntheme:  FIX  \n---\n"
        );
    }

    #[test]
    fn default_edits_cover_canonical_and_legacy_spellings() {
        let canonical = "---\ntype: [[area]]\ntask_type: BUILD\n---\n";
        let edits = default_edits(canonical, "build", "FIX");
        assert_eq!(edits.len(), 1);
        assert_eq!(
            apply_line_edits(canonical, &edits),
            "---\ntype: [[area]]\ntask_type: FIX\n---\n"
        );
        let both = "---\ntype: [[area]]\ntask_type: BUILD\ntheme: BUILD\n---\n";
        let edits = default_edits(both, "build", "FIX");
        assert_eq!(edits.len(), 2);
        assert_eq!(
            apply_line_edits(both, &edits),
            "---\ntype: [[area]]\ntask_type: FIX\ntheme: FIX\n---\n"
        );
        let quoted = "---\ntype: [[project]]\ntask_type: \"BUILD\"\n---\n";
        let edits = default_edits(quoted, "build", "FIX");
        assert_eq!(
            apply_line_edits(quoted, &edits),
            "---\ntype: [[project]]\ntask_type: \"FIX\"\n---\n"
        );
    }

    #[test]
    fn default_edit_skips_other_kinds_and_off() {
        let other = "---\ntype: [[ref]]\ntheme: BUILD\n---\n";
        let off = "---\ntype: [[area]]\ntheme: off\n---\n";
        let invalid = "---\ntype: [[area]]\ntheme: BUILD + FIX\n---\n";
        assert!(default_edit(other, "build", "FIX").is_none());
        assert!(default_edit(off, "build", "FIX").is_none());
        assert!(default_edit(invalid, "build", "FIX").is_none());
    }

    #[test]
    fn task_theme_edits_cover_paren_and_skip_fenced_and_prose() {
        let contents = "\
---\ntype: [[area]]\n---\n\
- [ ] #task Open [theme:: BUILD] ^open\n\
- [x] #task Done (theme:: BUILD) ^done\n\
Prose [theme:: BUILD] stays.\n\
```md\n\
- [ ] #task Fenced [theme:: BUILD]\n\
```\n\
- [ ] not a task [theme:: BUILD]\n";
        let edits = task_theme_edits(contents, "build", "FIX", &settings());
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].before, "BUILD");
        assert_eq!(edits[1].before, "BUILD");
        let after = apply_line_edits(contents, &edits);
        assert!(after.contains("[theme:: FIX]"), "{after}");
        assert!(after.contains("(theme:: FIX)"), "{after}");
        assert!(after.contains("Prose [theme:: BUILD] stays."), "{after}");
        assert!(
            after.contains("- [ ] #task Fenced [theme:: BUILD]"),
            "{after}"
        );
        assert!(
            after.contains("- [ ] not a task [theme:: BUILD]"),
            "{after}"
        );
    }

    #[test]
    fn task_theme_edits_cover_canonical_and_mixed_spellings() {
        let contents = "\
---\ntype: [[area]]\n---\n\
- [ ] #task Open [type:: BUILD] ^open\n\
- [ ] #task Legacy (theme:: BUILD) ^legacy\n\
- [ ] #task Mixed [type:: BUILD] (theme:: BUILD) ^mixed\n";
        let edits = task_theme_edits(contents, "build", "FIX", &settings());
        assert_eq!(edits.len(), 4);
        let after = apply_line_edits(contents, &edits);
        assert!(
            after.contains("- [ ] #task Open [type:: FIX] ^open"),
            "{after}"
        );
        assert!(
            after.contains("- [ ] #task Legacy (theme:: FIX) ^legacy"),
            "{after}"
        );
        assert!(
            after.contains(
                "- [ ] #task Mixed [type:: FIX] (theme:: FIX) ^mixed"
            ),
            "{after}"
        );
    }

    #[test]
    fn leftover_warning_names_counts() {
        let both = LeftoverThemeUses {
            defaults: 2,
            default_notes: vec!["dev.md".into(), "job.md".into()],
            task_themes: 3,
            ..LeftoverThemeUses::default()
        };
        assert_eq!(
            leftover_warning("JOB", &both).as_deref(),
            Some("split_leaves_task_themes: 2 defaults, 3 task fields still name JOB")
        );
        let one = LeftoverThemeUses {
            defaults: 1,
            default_notes: vec!["dev.md".into()],
            task_themes: 0,
            ..LeftoverThemeUses::default()
        };
        assert_eq!(
            leftover_warning("JOB", &one).as_deref(),
            Some("split_leaves_task_themes: 1 default still name JOB")
        );
        assert!(
            leftover_warning("JOB", &LeftoverThemeUses::default()).is_none()
        );
    }

    #[test]
    fn leftover_warning_names_offclock_lists() {
        let uses = LeftoverThemeUses {
            defaults: 0,
            default_notes: Vec::new(),
            task_themes: 1,
            offclock: 2,
            offclock_notes: vec!["love.md".into(), "body.md".into()],
        };
        assert_eq!(
            leftover_warning("JOB", &uses).as_deref(),
            Some(
                "split_leaves_task_themes: 1 task field, 2 offclock lists still name JOB"
            )
        );
    }

    fn area_note(frontmatter_body: &str) -> String {
        format!("---\ntype: [[area]]\n{frontmatter_body}---\n")
    }

    fn apply_offclock(contents: &str, old: &str, new: &str) -> String {
        let result = offclock_edits(contents, old, new);
        apply_coverage_edits(contents, &result.edits, &result.removed_lines)
    }

    #[test]
    fn offclock_scalar_rewrites_and_keeps_comment() {
        let before = area_note("offclock_types: MEET # evenings\n");
        let result = offclock_edits(&before, "meet", "CALL");
        assert_eq!(result.removed_lines, Vec::<usize>::new());
        assert_eq!(result.changes.len(), 1);
        assert_eq!(
            apply_coverage_edits(&before, &result.edits, &result.removed_lines),
            area_note("offclock_types: CALL # evenings\n")
        );
    }

    #[test]
    fn offclock_scalar_preserves_quotes() {
        let before = area_note("offclock_types: \"MEET\"\n");
        assert_eq!(
            apply_offclock(&before, "meet", "CALL"),
            area_note("offclock_types: \"CALL\"\n")
        );
        let single = area_note("offclock_types: 'MEET'\n");
        assert_eq!(
            apply_offclock(&single, "meet", "CALL"),
            area_note("offclock_types: 'CALL'\n")
        );
    }

    #[test]
    fn offclock_flow_rewrites_matching_item() {
        let before = area_note("offclock_types: [MEET, CALL]\n");
        assert_eq!(
            apply_offclock(&before, "call", "FIX"),
            area_note("offclock_types: [MEET, FIX]\n")
        );
    }

    #[test]
    fn offclock_flow_merge_removes_duplicate() {
        let before = area_note("offclock_types: [MEET, CALL]\n");
        let result = offclock_edits(&before, "call", "MEET");
        assert!(result.edits.len() == 1);
        assert_eq!(result.removed_lines, Vec::<usize>::new());
        assert_eq!(
            apply_coverage_edits(&before, &result.edits, &result.removed_lines),
            area_note("offclock_types: [MEET]\n")
        );
        let leading = area_note("offclock_types: [CALL, MEET]\n");
        assert_eq!(
            apply_offclock(&leading, "call", "MEET"),
            area_note("offclock_types: [MEET]\n")
        );
    }

    #[test]
    fn offclock_block_rewrites_items() {
        let before = area_note("offclock_types:\n  - MEET\n  - CALL\n");
        assert_eq!(
            apply_offclock(&before, "call", "FIX"),
            area_note("offclock_types:\n  - MEET\n  - FIX\n")
        );
    }

    #[test]
    fn offclock_block_merge_drops_item_line() {
        let before = area_note("offclock_types:\n  - MEET\n  - CALL\n");
        let result = offclock_edits(&before, "call", "MEET");
        assert_eq!(result.edits, Vec::new());
        assert_eq!(result.removed_lines, vec![5]);
        assert_eq!(
            apply_coverage_edits(&before, &result.edits, &result.removed_lines),
            area_note("offclock_types:\n  - MEET\n")
        );
    }

    #[test]
    fn offclock_rewrites_match_despite_invalid_sibling() {
        let before = area_note("offclock_types: [MEET, BAD + NAME]\n");
        assert_eq!(
            apply_offclock(&before, "meet", "CALL"),
            area_note("offclock_types: [CALL, BAD + NAME]\n")
        );
        assert!(offclock_names_old(&before, "meet"));
        assert!(!offclock_names_old(&before, "fix"));
    }

    #[test]
    fn offclock_ignores_non_area_notes() {
        let ref_note =
            "---\ntype: [[ref]]\noffclock_types: [MEET]\n---\n".to_string();
        assert!(offclock_edits(&ref_note, "meet", "CALL").changes.is_empty());
        assert!(!offclock_names_old(&ref_note, "meet"));
    }

    #[test]
    fn offclock_leaves_non_matching_lists_alone() {
        let before = area_note("offclock_types: [MEET]\n");
        assert!(offclock_edits(&before, "call", "FIX").changes.is_empty());
        assert_eq!(apply_offclock(&before, "call", "FIX"), before);
    }
}
