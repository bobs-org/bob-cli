//! Canonical `[root::]` / `(root::)` field parse, placement, and write
//! helpers.
//!
//! Syntax only: resolution of the note name happens in the homes
//! resolver. Placement matches [`super::task_theme::set_task_theme`].

use super::{
    task_fields::{inline_fields, InlineField},
    task_theme::trailing_metadata_start,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RootRepairKind {
    Duplicate,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskRootState {
    None,
    Set { raw: String },
    Repair { kind: RootRepairKind, raw: String },
}

impl TaskRootState {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn raw(&self) -> &str {
        match self {
            Self::None => "",
            Self::Set { raw } | Self::Repair { raw, .. } => raw,
        }
    }
}

pub(crate) fn parse_task_root(line: &str) -> TaskRootState {
    let fields = inline_fields(line, "root");
    match fields.as_slice() {
        [] => TaskRootState::None,
        [field] => {
            if is_invalid_root_value(&field.value) {
                TaskRootState::Repair {
                    kind: RootRepairKind::Invalid,
                    raw: field.value.clone(),
                }
            } else {
                TaskRootState::Set {
                    raw: field.value.clone(),
                }
            }
        }
        [first, ..] => TaskRootState::Repair {
            kind: RootRepairKind::Duplicate,
            raw: first.value.clone(),
        },
    }
}

/// Insert or replace the canonical `[root:: NOTE]` field at the left
/// edge of the trailing metadata run.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn set_task_root(line: &str, name: &str) -> String {
    let (body, ending) = split_line_ending(line);
    let cleared = remove_root_fields(body);
    let insertion = trailing_metadata_start(&cleared);
    let mut insert = format!("[root:: {name}]");
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

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn clear_task_root(line: &str) -> String {
    let (body, ending) = split_line_ending(line);
    let mut output = remove_root_fields(body);
    output.push_str(ending);
    output
}

pub(crate) fn is_invalid_root_value(value: &str) -> bool {
    value.is_empty()
        || value
            .chars()
            .any(|character| matches!(character, '[' | ']' | '|' | '#' | '^'))
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

fn remove_root_fields(line: &str) -> String {
    let fields = inline_fields(line, "root");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::task_theme::{clear_task_theme, set_task_theme};

    #[test]
    fn parse_none_set_repair_and_forms() {
        assert_eq!(parse_task_root("- [ ] #task Work ^t"), TaskRootState::None);
        assert_eq!(
            parse_task_root("- [ ] #task Work [root:: gtd_daily] ^gtd"),
            TaskRootState::Set {
                raw: "gtd_daily".into(),
            }
        );
        assert_eq!(
            parse_task_root("- [ ] #task Work (root::  sase) ^t"),
            TaskRootState::Set { raw: "sase".into() }
        );
        assert_eq!(
            parse_task_root("- [ ] #task Work [root::gtd_daily.md] ^t"),
            TaskRootState::Set {
                raw: "gtd_daily.md".into(),
            }
        );
        assert_eq!(
            parse_task_root("- [ ] #task Work [root:: My Area] ^t"),
            TaskRootState::Set {
                raw: "My Area".into(),
            }
        );
        let wikilink = parse_task_root("- [ ] #task Work [root:: [[sase]]] ^t");
        assert_eq!(
            wikilink,
            TaskRootState::Repair {
                kind: RootRepairKind::Invalid,
                raw: "[[sase".into(),
            }
        );
        assert_eq!(wikilink.raw(), "[[sase");
        assert_eq!(
            parse_task_root("- [ ] #task Work [root::] ^t"),
            TaskRootState::Repair {
                kind: RootRepairKind::Invalid,
                raw: String::new(),
            }
        );
        assert_eq!(
            parse_task_root("- [ ] #task Work [root:: sase] [root:: gtd] ^t"),
            TaskRootState::Repair {
                kind: RootRepairKind::Duplicate,
                raw: "sase".into(),
            }
        );
    }

    #[test]
    fn set_and_clear_use_theme_placement() {
        let before = "- [ ] #task Work [created:: 2026-01-01] ^t";
        let set = set_task_root(before, "gtd_daily");
        assert_eq!(
            set,
            "- [ ] #task Work [root:: gtd_daily] [created:: 2026-01-01] ^t"
        );
        assert_eq!(clear_task_root(&set), before);
        let paren =
            "- [ ] #task Work (root::  sase) [created:: 2026-01-01] ^t\n";
        assert_eq!(
            set_task_root(paren, "gtd"),
            "- [ ] #task Work [root:: gtd] [created:: 2026-01-01] ^t\n"
        );
        let spaced = set_task_root(before, "My Area");
        assert_eq!(
            spaced,
            "- [ ] #task Work [root:: My Area] [created:: 2026-01-01] ^t"
        );
        assert_eq!(clear_task_root(&spaced), before);
    }

    #[test]
    fn theme_set_and_clear_preserve_root_bytes() {
        let line = "- [ ] #task Work (root::  sase) [created:: 2026-01-01] ^t";
        let themed = set_task_theme(line, "WRITE");
        assert_eq!(
            themed,
            "- [ ] #task Work [type:: WRITE] (root::  sase) [created:: 2026-01-01] ^t"
        );
        assert!(themed.contains("(root::  sase)"));
        assert_eq!(clear_task_theme(&themed), line);
    }
}
