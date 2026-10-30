//! Task-to-task block-link mentions (`docs/task-relations.md` §19).
//!
//! This is an occurrence scanner only. It derives candidate source/target
//! pairs from task text; relation and dependency precedence, writable-target
//! filtering, and mirror writes belong to the reconciliation pass.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use super::super::{
    collect_done::block_ids_in_markdown,
    task_dependencies::{
        block_link_spans, blocks::has_task_tag, inline_code_spans,
        is_managed_dependency_line, parse::parse_dependency_line,
        DependencyLine,
    },
    task_status_hooks::{
        fenced_lines, leading_indentation_width, logical_lines,
        nearest_parent_list_item, FileScan, TaskLine,
    },
    vault_links::NoteIndex,
};
use super::parse::{is_subprojects_line, links_children_of};
use crate::native::task_relations::{is_relation_line, LinksLine};

/// One source task and target task pair with at least one qualifying link.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct MentionPair {
    pub(crate) source_path: PathBuf,
    pub(crate) source_line: usize,
    pub(crate) source_block_id: Option<String>,
    pub(crate) target_path: PathBuf,
    pub(crate) target_block_id: String,
    pub(crate) occurrence_count: usize,
    /// Keep existing mirrors and do not add this pair when source evidence is
    /// incomplete (duplicate source id, malformed edge, or unclosed fence).
    pub(crate) source_uncertain: bool,
}

/// Scan result sorted by source and target identity for deterministic callers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MentionScan {
    pub(crate) pairs: Vec<MentionPair>,
}

/// Find task-owned block-link occurrences across an already parsed note set.
/// `files` may include active notes, the previous daily note, and `done/`
/// archives. The caller applies the existing writability and precedence rules.
pub(crate) fn scan_mentions<'a>(
    files: impl IntoIterator<Item = &'a FileScan>,
    note_index: &NoteIndex,
) -> MentionScan {
    let files: Vec<&FileScan> = files.into_iter().collect();
    let mut tasks_by_path: BTreeMap<PathBuf, &FileScan> = BTreeMap::new();
    for file in &files {
        tasks_by_path.insert(file.relative_path.clone(), *file);
    }
    let mut pairs: BTreeMap<(PathBuf, usize, PathBuf, String), MentionPair> =
        BTreeMap::new();

    for file in files {
        if !file.contents.contains("#^") {
            continue;
        }
        let lines = logical_lines(&file.contents);
        let refs: Vec<&str> = lines.to_vec();
        let fenced = fenced_lines(&refs, 0..refs.len());
        let unclosed_fence = has_unclosed_fence(&refs);
        let all_ids = block_ids_in_markdown(&file.contents);
        let mut id_counts = BTreeMap::<String, usize>::new();
        for id in all_ids {
            *id_counts.entry(id).or_default() += 1;
        }
        let tasks_by_line: BTreeMap<usize, &TaskLine> = file
            .tasks
            .iter()
            .map(|task| (task.line_index, task))
            .collect();

        for task in &file.tasks {
            if task.line_index >= refs.len()
                || fenced.contains(&task.line_index)
            {
                continue;
            }
            let source_line = refs[task.line_index];
            if is_proxy_task(source_line, task) {
                continue;
            }
            let malformed_links =
                links_children_of(&refs, &fenced, task.line_index)
                    .iter()
                    .any(|child| matches!(child.parsed, LinksLine::Malformed));
            let malformed_dependencies =
                malformed_dependency_child(&refs, &fenced, task.line_index);
            let duplicate_source_id =
                task.block_id.as_ref().is_some_and(|id| {
                    id_counts.get(id).copied().unwrap_or_default() > 1
                });
            let source_uncertain = unclosed_fence
                || duplicate_source_id
                || malformed_links
                || malformed_dependencies;

            let source_indent = leading_indentation_width(source_line);
            for line_index in task.line_index..refs.len() {
                if line_index != task.line_index
                    && !refs[line_index].trim().is_empty()
                    && leading_indentation_width(refs[line_index])
                        <= source_indent
                {
                    break;
                }
                if fenced.contains(&line_index)
                    || is_blockquote_line(refs[line_index])
                {
                    continue;
                }
                let (owner, under_edge) =
                    owner_and_edge_ancestry(&refs, line_index, &tasks_by_line);
                if owner != Some(task.line_index) || under_edge {
                    continue;
                }

                let ignored = ignored_spans(
                    refs[line_index],
                    comment_spans(&refs, &fenced, line_index),
                    dataview_field_spans(refs[line_index]),
                );
                for span in block_link_spans(refs[line_index]) {
                    if ignored.iter().any(|range| {
                        span.token_start < range.end && span.end > range.start
                    }) {
                        continue;
                    }
                    let Some(target_path) = note_index
                        .resolve(Some(&file.relative_path), &span.target)
                    else {
                        continue;
                    };
                    let Some(target_file) = tasks_by_path.get(&target_path)
                    else {
                        continue;
                    };
                    if id_counts_for(target_file, &span.block_id) != 1
                        || task_id_count(target_file, &span.block_id) != 1
                    {
                        continue;
                    }
                    if file.relative_path == target_path
                        && task.block_id.as_deref()
                            == Some(span.block_id.as_str())
                    {
                        continue;
                    }
                    let key = (
                        file.relative_path.clone(),
                        task.line_index,
                        target_path.clone(),
                        span.block_id.clone(),
                    );
                    pairs
                        .entry(key)
                        .and_modify(|pair| {
                            pair.occurrence_count += 1;
                            pair.source_uncertain |= source_uncertain;
                        })
                        .or_insert_with(|| MentionPair {
                            source_path: file.relative_path.clone(),
                            source_line: task.line_index,
                            source_block_id: task.block_id.clone(),
                            target_path,
                            target_block_id: span.block_id,
                            occurrence_count: 1,
                            source_uncertain,
                        });
                }
            }
        }
    }

    MentionScan {
        pairs: pairs.into_values().collect(),
    }
}

fn id_counts_for(file: &FileScan, id: &str) -> usize {
    block_ids_in_markdown(&file.contents)
        .iter()
        .filter(|candidate| candidate.as_str() == id)
        .count()
}

fn task_id_count(file: &FileScan, id: &str) -> usize {
    file.tasks
        .iter()
        .filter(|task| task.block_id.as_deref() == Some(id))
        .count()
}

fn owner_and_edge_ancestry(
    lines: &[&str],
    line_index: usize,
    tasks_by_line: &BTreeMap<usize, &TaskLine>,
) -> (Option<usize>, bool) {
    let mut current = Some(line_index);
    let mut owner = None;
    let mut under_edge = false;
    while let Some(index) = current {
        let line = lines[index];
        under_edge |= is_edge_line(line);
        if tasks_by_line.contains_key(&index) {
            owner = Some(index);
            break;
        }
        current = nearest_parent_list_item(lines, index);
    }
    (owner, under_edge)
}

fn is_edge_line(line: &str) -> bool {
    is_managed_dependency_line(line)
        || is_relation_line(line)
        || is_subprojects_line(line)
}

fn malformed_dependency_child(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
) -> bool {
    let Some(owner) = lines.get(task_line) else {
        return false;
    };
    if !has_task_tag(owner) {
        return false;
    }
    let owner_indent = leading_indentation_width(owner);
    for index in task_line + 1..lines.len() {
        if lines[index].trim().is_empty()
            || fenced.contains(&index)
            || leading_indentation_width(lines[index]) <= owner_indent
        {
            continue;
        }
        if nearest_parent_list_item(lines, index) != Some(task_line) {
            continue;
        }
        if matches!(
            parse_dependency_line(lines[index]),
            DependencyLine::Malformed
        ) {
            return true;
        }
    }
    false
}

fn is_proxy_task(line: &str, task: &TaskLine) -> bool {
    let indent = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let Some(marker_end) =
        super::super::task_status_hooks::after_list_marker(line, indent)
    else {
        return false;
    };
    let Some(close) = line[marker_end..].find(']') else {
        return false;
    };
    let body = line[marker_end + close + 1..].trim();
    let body = task
        .block_id
        .as_ref()
        .and_then(|id| body.strip_suffix(&format!("^{id}")))
        .unwrap_or(body)
        .trim();
    let body = strip_task_tags_and_fields(body);
    let spans = block_link_spans(&body);
    spans.len() == 1
        && spans[0].embedded
        && body[..spans[0].token_start].trim().is_empty()
        && body[spans[0].end..].trim().is_empty()
}

fn strip_task_tags_and_fields(text: &str) -> String {
    let mut ranges = dataview_field_spans(text);
    let mut index = 0;
    while index < text.len() {
        if text.as_bytes()[index] == b'#'
            && (index == 0
                || text[..index]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace))
        {
            let start = index;
            index += 1;
            while index < text.len()
                && !text[index..]
                    .chars()
                    .next()
                    .is_some_and(char::is_whitespace)
            {
                index += text[index..].chars().next().unwrap().len_utf8();
            }
            ranges.push(start..index);
            continue;
        }
        index += text[index..].chars().next().unwrap().len_utf8();
    }
    remove_ranges(text, ranges)
}

fn dataview_field_spans(line: &str) -> Vec<std::ops::Range<usize>> {
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let open = bytes[index];
        if !matches!(open, b'[' | b'(')
            || (open == b'[' && index > 0 && bytes[index - 1] == b'[')
        {
            index += 1;
            continue;
        }
        let close = if open == b'[' { b']' } else { b')' };
        let Some(offset) =
            bytes[index + 1..].iter().position(|byte| *byte == close)
        else {
            index += 1;
            continue;
        };
        let end = index + 1 + offset + 1;
        if line[index + 1..end - 1].contains("::") {
            spans.push(index..end);
            index = end;
        } else {
            index += 1;
        }
    }
    spans
}

fn comment_spans(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    line_index: usize,
) -> Vec<std::ops::Range<usize>> {
    let mut open = false;
    let mut spans = Vec::new();
    for (index, line) in lines.iter().enumerate().take(line_index + 1) {
        if fenced.contains(&index) {
            if index == line_index && open {
                spans.push(0..line.len());
            }
            continue;
        }
        let code = inline_code_spans(line);
        let mut cursor = 0;
        let mut start = open.then_some(0);
        while let Some(offset) = line[cursor..].find("%%") {
            let delimiter = cursor + offset;
            if code.iter().any(|range| {
                delimiter < range.end && delimiter + 2 > range.start
            }) {
                cursor = delimiter + 2;
                continue;
            }
            if open {
                let begin = start.unwrap_or(0);
                if index == line_index {
                    spans.push(begin..delimiter + 2);
                }
                open = false;
                start = None;
            } else {
                open = true;
                start = Some(delimiter);
            }
            cursor = delimiter + 2;
        }
        if index == line_index && open {
            spans.push(start.unwrap_or(0)..line.len());
        }
    }
    spans
}

fn ignored_spans(
    _line: &str,
    comments: Vec<std::ops::Range<usize>>,
    fields: Vec<std::ops::Range<usize>>,
) -> Vec<std::ops::Range<usize>> {
    comments.into_iter().chain(fields).collect()
}

fn remove_ranges(
    text: &str,
    mut ranges: Vec<std::ops::Range<usize>>,
) -> String {
    ranges.sort_by_key(|range| range.start);
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    for range in ranges {
        if range.start < cursor {
            continue;
        }
        output.push_str(&text[cursor..range.start]);
        cursor = range.end;
    }
    output.push_str(&text[cursor..]);
    output
}

fn is_blockquote_line(line: &str) -> bool {
    let mut rest = line.trim_start();
    loop {
        if rest.starts_with('>') {
            return true;
        }
        if rest.len() >= 2
            && matches!(rest.as_bytes()[0], b'-' | b'+' | b'*')
            && rest.as_bytes()[1].is_ascii_whitespace()
        {
            rest = rest[2..].trim_start();
            continue;
        }
        if rest.starts_with('[')
            && let Some(close) = rest.find(']')
            && rest[close + 1..].starts_with(char::is_whitespace)
        {
            rest = rest[close + 1..].trim_start();
            continue;
        }
        return false;
    }
}

pub(crate) fn has_unclosed_fence(lines: &[&str]) -> bool {
    let mut open = None;
    for line in lines {
        if let Some(marker) = open {
            if crate::native::markdown::closes_fence(line, marker) {
                open = None;
            }
        } else if let Some(marker) = crate::native::markdown::fence_marker(line)
        {
            open = Some(marker);
        }
    }
    open.is_some()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        path::PathBuf,
    };

    use serde_json::Value;

    use super::*;
    use crate::native::task_status_hooks::{
        parse_tasks, TaskStatusType, TasksSettings,
    };

    fn settings() -> TasksSettings {
        TasksSettings {
            global_filter: "#task".to_string(),
            done_statuses: BTreeSet::from(['x', 'X']),
            status_types: BTreeMap::from([
                (' ', TaskStatusType::Todo),
                ('x', TaskStatusType::Done),
            ]),
            status_definitions: Vec::new(),
            status_settings_error: None,
        }
    }

    fn fixture_files(notes: &Value) -> Vec<FileScan> {
        let settings = settings();
        notes
            .as_object()
            .expect("notes object")
            .iter()
            .map(|(path, contents)| {
                let contents = contents.as_str().expect("note contents");
                let relative_path = PathBuf::from(path);
                FileScan {
                    path: PathBuf::from("/vault").join(&relative_path),
                    relative_path,
                    contents: contents.to_string(),
                    tasks: parse_tasks(contents, &settings),
                }
            })
            .collect()
    }

    fn expected_source(source: &str, files: &[FileScan]) -> (PathBuf, usize) {
        if let Some((path, id)) = source.split_once("#^") {
            let path = PathBuf::from(path);
            let file = files
                .iter()
                .find(|file| file.relative_path == path)
                .expect("source note exists");
            let task = file
                .tasks
                .iter()
                .find(|task| task.block_id.as_deref() == Some(id))
                .expect("source task exists");
            (path, task.line_index)
        } else {
            let (path, line) = source.rsplit_once(':').expect("line source");
            (PathBuf::from(path), line.parse::<usize>().unwrap() - 1)
        }
    }

    static TASK_MENTION_VECTORS: std::sync::LazyLock<Value> =
        std::sync::LazyLock::new(|| {
            serde_json::from_str(include_str!(
                "../../../tests/fixtures/task_mentions/vectors.json"
            ))
            .expect("mention vectors parse")
        });

    #[test]
    fn mn_vectors_match_rust_occurrence_pairs() {
        for vector in TASK_MENTION_VECTORS["vectors"]
            .as_array()
            .expect("vectors array")
        {
            let files = fixture_files(&vector["notes"]);
            let index = NoteIndex::from_paths(
                files.iter().map(|file| file.relative_path.clone()),
            );
            let found = scan_mentions(&files, &index);
            let actual = found
                .pairs
                .iter()
                .map(|pair| {
                    (
                        pair.source_path.clone(),
                        pair.source_line,
                        pair.target_path.clone(),
                        pair.target_block_id.clone(),
                        pair.occurrence_count,
                    )
                })
                .collect::<BTreeSet<_>>();
            let expected = vector["occurrences"]
                .as_array()
                .expect("occurrences array")
                .iter()
                .map(|occurrence| {
                    let (source_path, source_line) = expected_source(
                        occurrence["source"].as_str().expect("source"),
                        &files,
                    );
                    let (target_path, target_id) = occurrence["target"]
                        .as_str()
                        .expect("target")
                        .split_once("#^")
                        .expect("target task identity");
                    (
                        source_path,
                        source_line,
                        PathBuf::from(target_path),
                        target_id.to_string(),
                        occurrence["count"].as_u64().unwrap() as usize,
                    )
                })
                .collect::<BTreeSet<_>>();
            assert_eq!(actual, expected, "{}", vector["id"]);
            for pair in &found.pairs {
                if vector["id"] == "MN23" {
                    assert!(pair.source_uncertain);
                }
            }
        }
    }

    #[test]
    fn mention_label_is_not_an_authored_relation_label() {
        use crate::native::task_relations::{
            label::{normalize_relation_label, LabelError},
            parse::{parse_linked_from_line, parse_links_line},
            LinkedFromLine, LinksLine, RelationEntryKind, MENTION_LABEL,
        };

        let mention = "- 🪢 **LINKED FROM:** [[#^s]] (💬 mentions)";
        let LinkedFromLine::Accepted { entries, canonical } =
            parse_linked_from_line(mention)
        else {
            panic!("mention row should parse");
        };
        assert!(canonical);
        assert_eq!(entries[0].kind, RelationEntryKind::Mention);
        assert_eq!(entries[0].label, None);
        assert_eq!(entries[0].storage_label().as_deref(), Some(MENTION_LABEL));
        assert!(matches!(
            parse_linked_from_line(
                "- 🪢 **LINKED FROM:** [[#^s]] (💬 something else)"
            ),
            LinkedFromLine::Malformed
        ));
        assert!(matches!(
            parse_links_line("- 🪢 **LINKS:** 💬 mentions [[#^s]]"),
            LinksLine::Malformed
        ));
        assert_eq!(
            normalize_relation_label(" 💬   mentions:"),
            Err(LabelError::ReservedMention)
        );
    }
}
