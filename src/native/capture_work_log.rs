//! Pure writer for dated Pomodoro Work Log groups.

use super::capture::{
    leading_spaces_or_tabs_len, line_spans, list_marker_len,
    nearest_shallower_list_item_parent, parse_managed_task_log_marker,
    ManagedTaskLogKind,
};

const WORK_LOG_EMOJI: &str = "🛠️";
const INDENT_UNIT: &str = "\t";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkLogNode {
    pub(crate) marker: String,
    pub(crate) body_text: String,
    pub(crate) children: Vec<WorkLogNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkLogCursor {
    insert_line: usize,
    entry_indent: String,
    entry_marker: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkLogWrite {
    pub(crate) contents: String,
    /// Dated depth-one entry bodies, without their list prefix.
    pub(crate) entries: Vec<String>,
    pub(crate) next_cursor: WorkLogCursor,
}

pub(crate) fn write_work_log_group(
    contents: &str,
    task_line: usize,
    note_roots: &[WorkLogNode],
    date: &str,
    prior: Option<&WorkLogCursor>,
) -> Option<WorkLogWrite> {
    let spans = line_spans(contents);
    let lines = spans.iter().map(|line| line.text).collect::<Vec<_>>();
    let task_text = *lines.get(task_line)?;
    let task_indent = indentation(task_text);
    let task_end = child_block_end(&lines, task_line);

    let (insert_line, entry_indent, entry_marker, marker_line) =
        if let Some(prior) = prior {
            (
                prior.insert_line,
                prior.entry_indent.clone(),
                prior.entry_marker.clone(),
                None,
            )
        } else if let Some(marker_index) =
            find_work_marker(&spans, task_line, task_end)
        {
            let marker_indent = indentation(lines[marker_index]);
            let first_entry =
                first_direct_child(&spans, marker_index, task_end);
            let (entry_indent, entry_marker) = first_entry
                .map(|index| {
                    (indentation(lines[index]), item_marker(lines[index]))
                })
                .unwrap_or_else(|| {
                    (
                        format!(
                            "{marker_indent}{}",
                            child_indent_unit(&marker_indent)
                        ),
                        "-".to_string(),
                    )
                });
            (marker_index + 1, entry_indent, entry_marker, None)
        } else {
            let first_child = first_direct_child(&spans, task_line, task_end);
            let (parent_indent, parent_marker) = first_child
                .map(|index| {
                    (indentation(lines[index]), item_marker(lines[index]))
                })
                .unwrap_or_else(|| {
                    (
                        format!(
                            "{task_indent}{}",
                            child_indent_unit(&task_indent)
                        ),
                        "-".to_string(),
                    )
                });
            let unit = child_indent_unit(&parent_indent);
            (
                task_end + 1,
                format!("{parent_indent}{unit}"),
                "-".to_string(),
                Some(format!(
                "{parent_indent}{parent_marker} {WORK_LOG_EMOJI} **WORK LOG**"
            )),
            )
        };

    let mut entries = Vec::new();
    let mut inserted = Vec::new();
    if let Some(marker) = marker_line {
        inserted.push(marker);
    }
    for node in note_roots {
        append_entry_node(
            node,
            &entry_indent,
            &entry_marker,
            date,
            &mut entries,
            &mut inserted,
        );
    }
    if entries.is_empty() {
        return None;
    }

    let next_cursor = WorkLogCursor {
        insert_line: insert_line + inserted.len(),
        entry_indent,
        entry_marker,
    };
    Some(WorkLogWrite {
        contents: insert_lines(contents, insert_line, &inserted),
        entries,
        next_cursor,
    })
}

fn append_entry_node(
    node: &WorkLogNode,
    entry_indent: &str,
    entry_marker: &str,
    date: &str,
    entries: &mut Vec<String>,
    output: &mut Vec<String>,
) {
    let body = node.body_text.trim();
    if body.is_empty() {
        return;
    }
    let dated = format!("*{date}* — {body}");
    output.push(format!("{entry_indent}{entry_marker} {dated}"));
    entries.push(dated);
    append_descendants(&node.children, entry_indent, output);
}

fn append_descendants(
    nodes: &[WorkLogNode],
    parent_indent: &str,
    output: &mut Vec<String>,
) {
    if nodes.is_empty() {
        return;
    }
    let indent = format!("{parent_indent}{}", child_indent_unit(parent_indent));
    for node in nodes {
        let body = node.body_text.trim();
        if body.is_empty() {
            continue;
        }
        output.push(format!("{indent}{} {body}", node.marker));
        append_descendants(&node.children, &indent, output);
    }
}

fn find_work_marker(
    spans: &[super::capture::LineSpan<'_>],
    task_line: usize,
    task_end: usize,
) -> Option<usize> {
    (task_line + 1..=task_end).find(|&index| {
        parse_managed_task_log_marker(spans[index].text)
            == Some(ManagedTaskLogKind::Work)
            && nearest_shallower_list_item_parent(spans, index)
                == Some(task_line)
    })
}

fn first_direct_child(
    spans: &[super::capture::LineSpan<'_>],
    parent: usize,
    block_end: usize,
) -> Option<usize> {
    (parent + 1..=block_end).find(|&index| {
        !spans[index].text.trim().is_empty()
            && list_marker_len(
                &spans[index].text
                    [leading_spaces_or_tabs_len(spans[index].text)..],
            )
            .is_some()
            && nearest_shallower_list_item_parent(spans, index) == Some(parent)
    })
}

fn child_block_end(lines: &[&str], parent: usize) -> usize {
    let parent_indent = leading_spaces_or_tabs_len(lines[parent]);
    let mut end = parent;
    for index in parent + 1..lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            let next_child = lines[index + 1..]
                .iter()
                .find(|line| !line.trim().is_empty())
                .is_some_and(|line| {
                    leading_spaces_or_tabs_len(line) > parent_indent
                });
            if next_child {
                end = index;
                continue;
            }
            break;
        }
        if leading_spaces_or_tabs_len(line) <= parent_indent {
            break;
        }
        end = index;
    }
    end
}

fn indentation(line: &str) -> String {
    line[..leading_spaces_or_tabs_len(line)].to_string()
}

fn item_marker(line: &str) -> String {
    let start = leading_spaces_or_tabs_len(line);
    let rest = &line[start..];
    list_marker_len(rest)
        .map(|length| rest[..length].to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn child_indent_unit(parent_indent: &str) -> String {
    if !parent_indent.is_empty() && !parent_indent.contains('\t') {
        parent_indent.to_string()
    } else {
        INDENT_UNIT.to_string()
    }
}

fn insert_lines(contents: &str, index: usize, added: &[String]) -> String {
    let mut lines = split_lines(contents);
    let index = index.min(lines.len());
    let final_newline = contents.ends_with('\n');
    let ending = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };

    let inserted = added
        .iter()
        .enumerate()
        .map(|(offset, line)| {
            let is_last_at_eof =
                index == lines.len() && offset + 1 == added.len();
            let line_ending = if is_last_at_eof && !final_newline {
                String::new()
            } else {
                ending.to_string()
            };
            (line.clone(), line_ending)
        })
        .collect::<Vec<_>>();

    if index == lines.len()
        && !final_newline
        && let Some((_, previous_ending)) = lines.last_mut()
    {
        *previous_ending = ending.to_string();
    }
    lines.splice(index..index, inserted);
    lines
        .into_iter()
        .map(|(line, ending)| format!("{line}{ending}"))
        .collect()
}

fn split_lines(contents: &str) -> Vec<(String, String)> {
    let mut lines = Vec::new();
    for segment in contents.split_inclusive('\n') {
        if let Some(line) = segment.strip_suffix("\r\n") {
            lines.push((line.to_string(), "\r\n".to_string()));
        } else if let Some(line) = segment.strip_suffix('\n') {
            lines.push((line.to_string(), "\n".to_string()));
        } else {
            lines.push((segment.to_string(), String::new()));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(body: &str) -> WorkLogNode {
        WorkLogNode {
            marker: "-".to_string(),
            body_text: body.to_string(),
            children: Vec::new(),
        }
    }

    #[test]
    fn appends_work_log_after_schedule_log_and_uses_task_indent_style() {
        let source = concat!(
            "- [/] #task Work ^work\n",
            "  * 🗓️ **SCHEDULE LOG**\n",
            "    * scheduled note\n",
        );
        let result = write_work_log_group(
            source,
            0,
            &[node("Did work")],
            "2026-09-28",
            None,
        )
        .expect("log write");
        assert_eq!(
            result.contents,
            concat!(
                "- [/] #task Work ^work\n",
                "  * 🗓️ **SCHEDULE LOG**\n",
                "    * scheduled note\n",
                "  * 🛠️ **WORK LOG**\n",
                "    - *2026-09-28* — Did work\n",
            )
        );
    }

    #[test]
    fn prepends_under_existing_work_marker_and_inherits_entry_prefix() {
        let source = concat!(
            "- [/] #task Work ^work\n",
            "\t- 🛠️ **WORK LOG**\n",
            "\t\t* _2026-09-27_ — Previous\n",
        );
        let result = write_work_log_group(
            source,
            0,
            &[node("Current")],
            "2026-09-28",
            None,
        )
        .expect("log write");
        assert_eq!(
            result.contents,
            concat!(
                "- [/] #task Work ^work\n",
                "\t- 🛠️ **WORK LOG**\n",
                "\t\t* *2026-09-28* — Current\n",
                "\t\t* _2026-09-27_ — Previous\n",
            )
        );
    }

    #[test]
    fn empty_existing_marker_derives_child_indent_and_marker() {
        let source = "- [/] #task Work ^work\n\t- **WORK LOG:**\n";
        let result = write_work_log_group(
            source,
            0,
            &[node("Current")],
            "2026-09-28",
            None,
        )
        .expect("log write");
        assert!(result
            .contents
            .ends_with("\t- **WORK LOG:**\n\t\t- *2026-09-28* — Current\n"));
    }

    #[test]
    fn writes_same_target_groups_in_source_order_with_prior_cursor() {
        let source = "- [ ] #task Work ^work\n";
        let first = write_work_log_group(
            source,
            0,
            &[node("First")],
            "2026-09-28",
            None,
        )
        .expect("first group");
        let second = write_work_log_group(
            &first.contents,
            0,
            &[node("Second")],
            "2026-09-28",
            Some(&first.next_cursor),
        )
        .expect("second group");
        assert!(
            second.contents.find("First").unwrap()
                < second.contents.find("Second").unwrap()
        );
    }

    #[test]
    fn preserves_crlf_and_missing_final_newline() {
        let source = "- [ ] #task Work ^work\r\n";
        let result = write_work_log_group(
            source,
            0,
            &[node("Did work")],
            "2026-09-28",
            None,
        )
        .expect("log write");
        assert!(result.contents.contains("\r\n\t- 🛠️ **WORK LOG**\r\n"));
        assert!(result.contents.ends_with("*2026-09-28* — Did work\r\n"));

        let source = "- [ ] #task Work ^work";
        let result = write_work_log_group(
            source,
            0,
            &[node("Did work")],
            "2026-09-28",
            None,
        )
        .expect("log write");
        assert!(!result.contents.ends_with('\n'));
    }
}
