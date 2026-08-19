//! Task-section scanner for `@route+block-id#section` capture.
//!
//! This module is the single owner of the title predicate, checkbox detection,
//! slug, direct-child enumeration, selector matching, and insertion geometry.
//! Later phases add CLI surfaces; this file currently has none.

#![allow(dead_code)]

use super::{
    capture::{
        dominant_indent_unit, first_child_indentation,
        first_direct_managed_log_start, leading_spaces_or_tabs_len,
        leading_whitespace, line_spans, list_item_body,
        nearest_shallower_list_item_parent, parse_managed_task_log_marker,
        LineSpan,
    },
    note_tasks::NoteTask,
};

/// A direct-child ALL-CAPS section bullet under a resolved task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskSection {
    pub(crate) title: String,
    pub(crate) slug: String,
    /// One-based line number of the section bullet.
    pub(crate) line: usize,
    pub(crate) indentation: String,
    pub(crate) block_end: usize,
    pub(crate) child_count: usize,
}

impl TaskSection {
    pub(crate) fn line_index(&self) -> usize {
        self.line - 1
    }
}

/// Where a capture block lands under a selected section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskSectionInsertion {
    pub(crate) offset: usize,
    pub(crate) indentation: String,
}

/// Canonical whitespace-free slug: trim, collapse internal whitespace to one
/// space, ASCII-lowercase, then replace each remaining space with `-`.
pub(crate) fn slug(text: &str) -> String {
    let mut result = String::new();
    for word in text.split_whitespace() {
        if !result.is_empty() {
            result.push('-');
        }
        result.push_str(&word.to_ascii_lowercase());
    }
    result
}

/// Enumerate qualifying section bullets that are direct children of `task`.
pub(crate) fn task_sections(
    contents: &str,
    task: &NoteTask,
) -> Vec<TaskSection> {
    let lines = line_spans(contents);
    let children =
        direct_child_list_indexes(&lines, task.line_index, task.block_end);
    let mut sections = Vec::new();
    for (index, &line_index) in children.iter().enumerate() {
        let line = lines[line_index];
        if parse_managed_task_log_marker(line.text).is_some() {
            continue;
        }
        let Some(body) = list_item_body(line.text) else {
            continue;
        };
        let Some(title) = parse_task_section_title(body) else {
            continue;
        };
        let block_end = children
            .get(index + 1)
            .map(|&next| line_start(&lines, next))
            .unwrap_or(task.block_end);
        let child_count =
            direct_child_list_indexes(&lines, line_index, block_end).len();
        sections.push(TaskSection {
            title: title.to_string(),
            slug: slug(title),
            line: line_index + 1,
            indentation: leading_whitespace(line.text).to_string(),
            block_end,
            child_count,
        });
    }
    sections
}

/// Whole-slug match in document order, else the first slug-prefix match.
pub(crate) fn select_section<'a>(
    sections: &'a [TaskSection],
    selector: &str,
) -> Option<&'a TaskSection> {
    let needle = slug(selector);
    if needle.is_empty() {
        return None;
    }
    sections
        .iter()
        .find(|section| section.slug == needle)
        .or_else(|| {
            sections
                .iter()
                .find(|section| section.slug.starts_with(&needle))
        })
}

/// Insertion offset and indent for a capture nested under `section`.
pub(crate) fn section_insertion(
    contents: &str,
    section: &TaskSection,
) -> TaskSectionInsertion {
    let lines = line_spans(contents);
    let indentation = first_child_indentation(
        &lines,
        section.line_index(),
        section.block_end,
        &section.indentation,
    )
    .or_else(|| {
        dominant_indent_unit(&lines)
            .map(|unit| format!("{}{}", section.indentation, unit))
    })
    .unwrap_or_else(|| format!("{}\t", section.indentation));
    let offset = first_direct_managed_log_start(
        &lines,
        section.line_index(),
        section.block_end,
    )
    .unwrap_or(section.block_end);
    TaskSectionInsertion {
        offset,
        indentation,
    }
}

/// Title of a list-item body when it is a task section, else `None`.
///
/// Capture does **not** require a nested list item under the bullet. The
/// plugin's Ctrl+Shift+Alt+N conversion does, because a childless bullet would
/// seed an empty `##` heading. Requiring that lookahead would reject a freshly
/// authored, still-empty `REQUIREMENTS` bullet — the moment this feature is
/// most useful. `child_count` lets a picker badge emptiness instead.
fn parse_task_section_title(body: &str) -> Option<&str> {
    if has_task_checkbox(body) {
        return None;
    }
    let title = body.trim();
    is_section_title(title).then_some(title)
}

fn is_section_title(title: &str) -> bool {
    let mut chars = title.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !is_section_title_first(first) {
        return false;
    }
    let mut has_letter = first.is_ascii_uppercase();
    for character in chars {
        if !is_section_title_rest(character) {
            return false;
        }
        has_letter |= character.is_ascii_uppercase();
    }
    has_letter
}

fn is_section_title_first(character: char) -> bool {
    character.is_ascii_uppercase() || character.is_ascii_digit()
}

fn is_section_title_rest(character: char) -> bool {
    is_section_title_first(character)
        || matches!(
            character,
            ' ' | '\t' | '&' | '\'' | '(' | ')' | ',' | '.' | '/' | '-'
        )
}

/// Plugin `PROJECT_CHILD_LIST_ITEM_RE` checkbox: `[x]` plus following
/// whitespace, where `x` is a single character other than `]` or newline.
fn has_task_checkbox(body: &str) -> bool {
    let mut chars = body.chars();
    if chars.next() != Some('[') {
        return false;
    }
    let Some(status) = chars.next() else {
        return false;
    };
    if status == ']' || status == '\n' {
        return false;
    }
    if chars.next() != Some(']') {
        return false;
    }
    leading_spaces_or_tabs_len(chars.as_str()) > 0
}

fn direct_child_list_indexes(
    lines: &[LineSpan<'_>],
    parent_index: usize,
    block_end: usize,
) -> Vec<usize> {
    let parent_indent = leading_spaces_or_tabs_len(lines[parent_index].text);
    let mut indexes = Vec::new();
    for (offset, line) in lines[parent_index + 1..].iter().enumerate() {
        if line.end > block_end {
            break;
        }
        let line_index = parent_index + 1 + offset;
        if leading_spaces_or_tabs_len(line.text) <= parent_indent
            || list_item_body(line.text).is_none()
            || nearest_shallower_list_item_parent(lines, line_index)
                != Some(parent_index)
        {
            continue;
        }
        indexes.push(line_index);
    }
    indexes
}

fn line_start(lines: &[LineSpan<'_>], index: usize) -> usize {
    if index == 0 {
        0
    } else {
        lines[index - 1].end
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::native::note_tasks;

    fn parent_task(contents: &str) -> NoteTask {
        let settings = note_tasks::read_settings(Path::new("/nonexistent"));
        let scan = note_tasks::scan(contents, &settings);
        scan.task_named("Parent")
            .cloned()
            .unwrap_or_else(|| panic!("missing Parent task in {contents:?}"))
    }

    fn sections_of(contents: &str) -> Vec<TaskSection> {
        task_sections(contents, &parent_task(contents))
    }

    fn titles_of(contents: &str) -> Vec<String> {
        sections_of(contents)
            .into_iter()
            .map(|section| section.title)
            .collect()
    }

    fn section_named<'a>(
        sections: &'a [TaskSection],
        title: &str,
    ) -> &'a TaskSection {
        sections
            .iter()
            .find(|section| section.title == title)
            .unwrap_or_else(|| panic!("missing section {title:?}"))
    }

    #[test]
    fn slug_trims_collapses_whitespace_and_lowercases() {
        assert_eq!(slug("REQUIREMENTS"), "requirements");
        assert_eq!(slug("FUTURE WORK"), "future-work");
        assert_eq!(slug("FUTURE  WORK"), "future-work");
        assert_eq!(slug("  Q&A  "), "q&a");
        assert_eq!(slug("NON-GOALS"), "non-goals");
        assert_eq!(slug("WHAT'S NEXT"), "what's-next");
        assert_eq!(slug(""), "");
    }

    #[test]
    fn title_whitelist_edges() {
        assert_eq!(
            parse_task_section_title("1 REQUIREMENTS"),
            Some("1 REQUIREMENTS")
        );
        assert_eq!(
            parse_task_section_title("Q&A (DRAFT), V2.0/OK-GO"),
            Some("Q&A (DRAFT), V2.0/OK-GO")
        );
        assert_eq!(
            parse_task_section_title("WHAT'S NEXT"),
            Some("WHAT'S NEXT")
        );
        assert_eq!(
            parse_task_section_title("FUTURE  WORK"),
            Some("FUTURE  WORK")
        );
        assert_eq!(
            parse_task_section_title("  REQUIREMENTS  "),
            Some("REQUIREMENTS")
        );

        for body in [
            "SNAKE_CASE",
            "requirements",
            "Requirements",
            "123",
            "",
            "[[SOME_NOTE]]",
            "#TODO",
            "REQUIREMENTS ^abc",
            "`CODE`",
            "FOO_BAR",
            "A_B",
        ] {
            assert_eq!(parse_task_section_title(body), None, "{body:?}");
        }

        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- 1 REQUIREMENTS\n",
            "\t- Q&A (DRAFT), V2.0/OK-GO\n",
            "\t- FUTURE  WORK\n",
            "\t- SNAKE_CASE\n",
            "\t- requirements\n",
            "\t- 123\n",
            "\t- [[SOME_NOTE]]\n",
            "\t- #TODO\n",
            "\t- REQUIREMENTS ^abc\n",
            "\t- `CODE`\n",
            "\t- NOTES\n",
        );
        assert_eq!(
            titles_of(contents),
            [
                "1 REQUIREMENTS",
                "Q&A (DRAFT), V2.0/OK-GO",
                "FUTURE  WORK",
                "NOTES"
            ]
        );
        assert_eq!(
            sections_of(contents)
                .into_iter()
                .map(|section| section.slug)
                .collect::<Vec<_>>(),
            [
                "1-requirements",
                "q&a-(draft),-v2.0/ok-go",
                "future-work",
                "notes"
            ]
        );
    }

    #[test]
    fn checkboxed_all_caps_children_are_not_sections() {
        assert_eq!(parse_task_section_title("[ ] REQUIREMENTS"), None);
        assert_eq!(parse_task_section_title("[x] REQUIREMENTS"), None);
        assert_eq!(parse_task_section_title("[/] REQUIREMENTS"), None);
        assert_eq!(parse_task_section_title("[x]REQUIREMENTS"), None);

        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- [ ] REQUIREMENTS\n",
            "\t- [x] REQUIREMENTS\n",
            "\t- FUTURE WORK\n",
        );
        assert_eq!(titles_of(contents), ["FUTURE WORK"]);
    }

    #[test]
    fn grandchild_is_not_a_direct_child_section() {
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- GRANDCHILD\n",
            "\t- FUTURE WORK\n",
        );
        let sections = sections_of(contents);
        assert_eq!(
            sections
                .iter()
                .map(|section| section.title.as_str())
                .collect::<Vec<_>>(),
            ["REQUIREMENTS", "FUTURE WORK"]
        );
        assert_eq!(section_named(&sections, "REQUIREMENTS").child_count, 1);
        assert_eq!(section_named(&sections, "FUTURE WORK").child_count, 0);
    }

    #[test]
    fn empty_section_bullet_still_qualifies() {
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t- FUTURE WORK\n",
        );
        let sections = sections_of(contents);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "REQUIREMENTS");
        assert_eq!(sections[0].child_count, 0);
        assert_eq!(sections[1].child_count, 0);
    }

    #[test]
    fn ordered_and_star_plus_markers_qualify() {
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t1. REQUIREMENTS\n",
            "\t* FUTURE WORK\n",
            "\t+ NOTES\n",
            "\t2) Q&A\n",
        );
        assert_eq!(
            titles_of(contents),
            ["REQUIREMENTS", "FUTURE WORK", "NOTES", "Q&A"]
        );
    }

    #[test]
    fn tab_two_space_four_space_and_mixed_indentation() {
        let tab = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t- FUTURE WORK\n",
        );
        let tab_sections = sections_of(tab);
        assert_eq!(
            tab_sections
                .iter()
                .map(|section| section.title.as_str())
                .collect::<Vec<_>>(),
            ["REQUIREMENTS", "FUTURE WORK"]
        );
        assert_eq!(tab_sections[0].indentation, "\t");
        assert_eq!(
            section_insertion(tab, &tab_sections[0]).indentation,
            "\t\t"
        );

        let two_space = concat!(
            "- [ ] #task Parent\n",
            "  - REQUIREMENTS\n",
            "    - existing\n",
            "  - FUTURE WORK\n",
        );
        let two_space_sections = sections_of(two_space);
        assert_eq!(two_space_sections[0].indentation, "  ");
        assert_eq!(
            section_insertion(two_space, &two_space_sections[0]).indentation,
            "    "
        );

        let four_space = concat!(
            "- [ ] #task Parent\n",
            "    - REQUIREMENTS\n",
            "        - existing\n",
            "    - FUTURE WORK\n",
        );
        let four_space_sections = sections_of(four_space);
        assert_eq!(four_space_sections[0].indentation, "    ");
        assert_eq!(
            section_insertion(four_space, &four_space_sections[0]).indentation,
            "        "
        );

        // Mixed-indent siblings are both direct children via nearest-shallower
        // ancestry; do not collapse to the plugin's shallowest-only rule.
        let mixed = concat!(
            "- [ ] #task Parent\n",
            "    - REQUIREMENTS\n",
            "  - FUTURE WORK\n",
        );
        assert_eq!(titles_of(mixed), ["REQUIREMENTS", "FUTURE WORK"]);
    }

    #[test]
    fn managed_logs_are_never_sections_plain_titles_are() {
        let recognized = [
            "\t- 🗓️ **SCHEDULE LOG**",
            "\t* **SCHEDULE LOG**",
            "\t+ **SCHEDULE LOG:**",
            "\t1. **Schedule log:**",
            "\t2) 🗓️ **Schedule log**",
            "\t- 🛠️ **WORK LOG**",
            "\t* **WORK LOG**",
            "\t+ **Work log:**",
            "\t10. **Work log**",
            "\t3) 🛠️ **WORK LOG:**",
        ];
        for marker in recognized {
            let contents = format!("- [ ] #task Parent\n{marker}\n\t- NOTES\n");
            assert_eq!(titles_of(&contents), ["NOTES"], "{marker}");
        }

        let plain = concat!(
            "- [ ] #task Parent\n",
            "\t- SCHEDULE LOG\n",
            "\t- WORK LOG\n",
        );
        assert_eq!(titles_of(plain), ["SCHEDULE LOG", "WORK LOG"]);
    }

    #[test]
    fn whole_slug_beats_earlier_prefix_and_first_duplicate_wins() {
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- FUTURE WORKFLOW\n",
            "\t- FUTURE WORK\n",
            "\t- FUTURE  WORK\n",
        );
        let sections = sections_of(contents);
        assert_eq!(
            select_section(&sections, "future-work")
                .map(|section| section.title.as_str()),
            Some("FUTURE WORK")
        );
        assert_eq!(
            select_section(&sections, "FUTURE WORK")
                .map(|section| section.line),
            Some(3)
        );
        assert_eq!(
            select_section(&sections, "future")
                .map(|section| section.title.as_str()),
            Some("FUTURE WORKFLOW")
        );
        assert_eq!(
            select_section(&sections, "future-workflow")
                .map(|section| section.title.as_str()),
            Some("FUTURE WORKFLOW")
        );
        assert_eq!(select_section(&sections, ""), None);
        assert_eq!(select_section(&sections, "absent"), None);
    }

    #[test]
    fn insertion_geometry_for_middle_last_blank_and_managed_log() {
        let middle = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t- FUTURE WORK\n",
            "\t\t- later\n",
            "\t- NOTES\n",
        );
        let sections = sections_of(middle);
        let requirements = section_named(&sections, "REQUIREMENTS");
        let future = section_named(&sections, "FUTURE WORK");
        let notes = section_named(&sections, "NOTES");
        assert_eq!(
            section_insertion(middle, requirements).offset,
            middle.find("\t- FUTURE WORK").expect("middle sibling")
        );
        assert_eq!(
            section_insertion(middle, future).offset,
            middle.find("\t- NOTES").expect("last-but-one sibling")
        );
        assert_eq!(section_insertion(middle, notes).offset, middle.len());
        assert_eq!(notes.child_count, 0);

        let blank = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\n",
            "\t- FUTURE WORK\n",
        );
        let blank_sections = sections_of(blank);
        assert_eq!(
            section_insertion(blank, &blank_sections[0]).offset,
            blank.find("\t- FUTURE WORK").expect("after blank")
        );

        let managed = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t\t- *2026-08-01* — scheduled\n",
            "\t- FUTURE WORK\n",
        );
        let managed_sections = sections_of(managed);
        let managed_requirements =
            section_named(&managed_sections, "REQUIREMENTS");
        assert_eq!(managed_requirements.child_count, 1);
        assert_eq!(
            section_insertion(managed, managed_requirements).offset,
            managed
                .find("\t\t- 🗓️ **SCHEDULE LOG**")
                .expect("managed log")
        );
        assert_eq!(
            section_insertion(managed, managed_requirements).indentation,
            "\t\t"
        );
    }

    #[test]
    fn insertion_geometry_preserves_crlf_offsets() {
        let contents = concat!(
            "- [ ] #task Parent\r\n",
            "\t- REQUIREMENTS\r\n",
            "\t\t- existing\r\n",
            "\t- FUTURE WORK\r\n",
        );
        let sections = sections_of(contents);
        let requirements = section_named(&sections, "REQUIREMENTS");
        let offset = section_insertion(contents, requirements).offset;
        assert_eq!(
            offset,
            contents.find("\t- FUTURE WORK").expect("crlf sibling")
        );
        assert!(contents[..offset].ends_with("\r\n"));
        assert_eq!(
            section_insertion(
                contents,
                section_named(&sections, "FUTURE WORK")
            )
            .offset,
            contents.len()
        );
    }
}
