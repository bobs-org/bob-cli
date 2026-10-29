use std::collections::HashMap;

use super::super::{
    collect_done::trailing_block_id_in_line,
    markdown,
    projects::{
        frontmatter_is_area, frontmatter_is_project, frontmatter_scalar,
        frontmatter_value, parse_frontmatter, trim_yaml_scalar,
        wikilink_path_target, FrontmatterScalar,
    },
    ref_tasks::strip_blockquote_prefix,
    task_fields::canonical_theme_name,
    task_root::{parse_task_root, TaskRootState},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoteKind {
    Area,
    Project,
    Other,
}

/// A `theme:` frontmatter value on a note. Absent/blank inherit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoteTheme {
    Name(String),
    Off,
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskRootFact {
    pub(crate) line_number: usize,
    pub(crate) state: TaskRootState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteFacts {
    pub(crate) kind: NoteKind,
    pub(crate) parent: Option<String>,
    pub(crate) color: Option<String>,
    pub(crate) root: bool,
    pub(crate) theme: Option<NoteTheme>,
    /// Unique trailing block id → root field on that task line.
    /// Duplicate block ids are omitted, matching `note_tasks::by_block_id`.
    pub(crate) task_roots: HashMap<String, TaskRootFact>,
}

impl NoteFacts {
    pub(crate) fn parse(contents: &str) -> Self {
        let frontmatter = parse_frontmatter(contents);
        let kind =
            frontmatter.as_ref().map_or(NoteKind::Other, |frontmatter| {
                if frontmatter_is_area(frontmatter) {
                    NoteKind::Area
                } else if frontmatter_is_project(frontmatter) {
                    NoteKind::Project
                } else {
                    NoteKind::Other
                }
            });
        let parent = frontmatter
            .as_ref()
            .and_then(|frontmatter| frontmatter_value(frontmatter, "parent"))
            .and_then(wikilink_path_target);
        let color = frontmatter
            .as_ref()
            .and_then(|frontmatter| frontmatter_value(frontmatter, "color"))
            .map(|value| trim_yaml_scalar(value).to_string());
        let root = frontmatter
            .as_ref()
            .and_then(|frontmatter| frontmatter_value(frontmatter, "root"))
            .is_some_and(|value| {
                trim_yaml_scalar(value).eq_ignore_ascii_case("true")
            });
        let theme = frontmatter.as_ref().and_then(parse_note_theme);
        Self {
            kind,
            parent,
            color,
            root,
            theme,
            task_roots: collect_task_roots(contents),
        }
    }
}

fn contains_root_field_key(text: &str) -> bool {
    let mut remainder = text;
    while let Some(index) = remainder.find("root") {
        let after = remainder[index + 4..].trim_start_matches([' ', '\t']);
        if after.starts_with("::") {
            return true;
        }
        remainder = &remainder[index + 1..];
    }
    false
}

fn collect_task_roots(contents: &str) -> HashMap<String, TaskRootFact> {
    if !contains_root_field_key(contents) {
        return HashMap::new();
    }
    let lines: Vec<&str> = contents.lines().collect();
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&lines);
    let scan_start = frontmatter_end.map_or(0, |end| end + 1);
    let fenced = markdown::fenced_lines(&lines, scan_start..lines.len());
    let mut occurrences: HashMap<String, Vec<(usize, TaskRootState)>> =
        HashMap::new();
    for (index, line) in lines.iter().enumerate() {
        if frontmatter_end.is_some_and(|end| index <= end)
            || fenced.contains(&index)
            || !contains_root_field_key(line)
            || !is_checkbox_line(line)
        {
            continue;
        }
        let Some(block_id) = trailing_block_id_in_line(line) else {
            continue;
        };
        let state = parse_task_root(line);
        if matches!(state, TaskRootState::None) {
            continue;
        }
        occurrences
            .entry(block_id)
            .or_default()
            .push((index + 1, state));
    }
    occurrences
        .into_iter()
        .filter_map(|(block_id, found)| {
            let [(line_number, state)] = found.as_slice() else {
                return None;
            };
            Some((
                block_id,
                TaskRootFact {
                    line_number: *line_number,
                    state: state.clone(),
                },
            ))
        })
        .collect()
}

fn is_checkbox_line(line: &str) -> bool {
    let stripped = strip_blockquote_prefix(line).trim_start();
    let after_marker = stripped
        .strip_prefix('-')
        .or_else(|| stripped.strip_prefix('*'))
        .or_else(|| stripped.strip_prefix('+'))
        .or_else(|| {
            let digits =
                stripped.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            stripped[digits..]
                .strip_prefix('.')
                .or_else(|| stripped[digits..].strip_prefix(')'))
        });
    let Some(rest) =
        after_marker.and_then(|rest| rest.strip_prefix([' ', '\t']))
    else {
        return false;
    };
    rest.len() >= 3 && rest.as_bytes()[0] == b'[' && rest.as_bytes()[2] == b']'
}

fn parse_note_theme(
    frontmatter: &super::super::projects::Frontmatter<'_>,
) -> Option<NoteTheme> {
    match frontmatter_scalar(frontmatter, "theme") {
        FrontmatterScalar::Absent | FrontmatterScalar::Blank => None,
        FrontmatterScalar::NonScalar(raw) => Some(NoteTheme::Invalid(raw)),
        FrontmatterScalar::Value(raw) => {
            if raw.eq_ignore_ascii_case("off") {
                Some(NoteTheme::Off)
            } else {
                match canonical_theme_name(&raw) {
                    Some(name) => Some(NoteTheme::Name(name)),
                    None => Some(NoteTheme::Invalid(raw)),
                }
            }
        }
    }
}
