//! Locate the journaled description-bullet block under one Event Task.
//!
//! Search is pure: a window of `line_count` consecutive child lines is
//! the block when it starts at structural depth 1, every line in it is
//! at depth 1 or 2, the line after it (if still inside the child block)
//! is at depth 1 or shallower, and its [`bullets_hash`] equals the
//! recorded hash. A window of `line_count == 0` is always the insertion
//! point after the leading run of managed direct-child subtrees.

use crate::native::{
    capture::{
        line_spans, list_item_body, nearest_shallower_list_item_parent,
        LineSpan,
    },
    capture_task_toggle::child_block_end_line,
    task_children,
};

use super::description::bullets::bullets_hash;

/// Inclusive-start, exclusive-end line range of a description block.
/// An empty window (`start == end`) is the insertion index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BulletWindow {
    pub start: usize,
    pub end: usize,
}

/// Find the first window under `row_index` whose bodies hash to `hash`
/// over `line_count` lines. `line_count == 0` always yields the
/// insertion point.
pub(crate) fn find_block(
    lines: &[String],
    row_index: usize,
    hash: &str,
    line_count: usize,
) -> Option<BulletWindow> {
    if line_count == 0 {
        let start = insertion_index(lines, row_index);
        return Some(BulletWindow { start, end: start });
    }
    let contents = join_lines(lines);
    let spans = line_spans(&contents);
    if row_index >= spans.len() {
        return None;
    }
    let child_end = child_end_index(&spans, row_index);
    if child_end < row_index {
        return None;
    }
    let last_start = (child_end + 1).saturating_sub(line_count);
    let mut start = row_index + 1;
    while start <= last_start {
        let end = start + line_count;
        if is_block_window(&spans, row_index, child_end, start, end)
            && window_hash(lines, &spans, row_index, start, end).as_deref()
                == Some(hash)
        {
            return Some(BulletWindow { start, end });
        }
        start += 1;
    }
    None
}

/// Line index at which a new empty block inserts: after the leading
/// run of managed direct-child subtrees (Depends-On, BLOCKS, LINKS /
/// LINKED FROM, CANCEL LOG, Schedule / Work / Rotten logs).
pub(crate) fn insertion_index(lines: &[String], row_index: usize) -> usize {
    let contents = join_lines(lines);
    let spans = line_spans(&contents);
    if row_index >= spans.len() {
        return row_index.saturating_add(1).min(lines.len());
    }
    let child_end = child_end_index(&spans, row_index);
    let mut cursor = row_index + 1;
    while cursor <= child_end && cursor < lines.len() {
        if lines[cursor].trim().is_empty() {
            break;
        }
        if structural_depth(&spans, row_index, cursor) == Some(1)
            && task_children::classify_task_child(&lines[cursor], None)
                .is_managed()
        {
            cursor = child_block_end_line(&spans, cursor) + 1;
            continue;
        }
        break;
    }
    cursor.min(lines.len())
}

/// Replace `window` with `new_lines` in place.
pub(crate) fn replace_window(
    lines: &mut Vec<String>,
    window: BulletWindow,
    new_lines: &[String],
) {
    let start = window.start.min(lines.len());
    let end = window.end.min(lines.len()).max(start);
    lines.splice(start..end, new_lines.iter().cloned());
}

fn join_lines(lines: &[String]) -> String {
    lines.join("\n")
}

fn child_end_index(spans: &[LineSpan<'_>], row_index: usize) -> usize {
    child_block_end_line(spans, row_index).min(spans.len().saturating_sub(1))
}

fn is_block_window(
    spans: &[LineSpan<'_>],
    row_index: usize,
    child_end: usize,
    start: usize,
    end: usize,
) -> bool {
    if start >= end || end.saturating_sub(1) > child_end {
        return false;
    }
    if structural_depth(spans, row_index, start) != Some(1) {
        return false;
    }
    for index in start..end {
        match structural_depth(spans, row_index, index) {
            Some(1 | 2) => {}
            _ => return false,
        }
    }
    after_window_is_closed(spans, row_index, child_end, end)
}

/// The line after the window, when it is still inside the child block,
/// must not be nested under the window (depth 2+).
fn after_window_is_closed(
    spans: &[LineSpan<'_>],
    row_index: usize,
    child_end: usize,
    end: usize,
) -> bool {
    if end > child_end || end >= spans.len() {
        return true;
    }
    match structural_depth(spans, row_index, end) {
        Some(depth) if depth >= 2 => false,
        _ => true,
    }
}

fn window_hash(
    lines: &[String],
    spans: &[LineSpan<'_>],
    row_index: usize,
    start: usize,
    end: usize,
) -> Option<String> {
    let mut pairs = Vec::with_capacity(end.saturating_sub(start));
    for index in start..end {
        let depth = structural_depth(spans, row_index, index)?;
        let body = bullet_body(&lines[index]);
        pairs.push((depth, body));
    }
    Some(bullets_hash(
        pairs.iter().map(|(depth, body)| (*depth, body.as_str())),
    ))
}

fn bullet_body(line: &str) -> String {
    line.trim_start_matches([' ', '\t']).trim_end().to_string()
}

/// Structural depth relative to `row_index`: 1 when the nearest
/// shallower list-item parent is the row, 2 when that parent is itself
/// a depth-1 child of the row.
fn structural_depth(
    spans: &[LineSpan<'_>],
    row_index: usize,
    line_index: usize,
) -> Option<u8> {
    if line_index == row_index || line_index >= spans.len() {
        return None;
    }
    if spans[line_index].text.trim().is_empty() {
        return None;
    }
    list_item_body(spans[line_index].text)?;
    let parent = nearest_shallower_list_item_parent(spans, line_index)?;
    if parent == row_index {
        return Some(1);
    }
    let grandparent = nearest_shallower_list_item_parent(spans, parent)?;
    (grandparent == row_index).then_some(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gcal::description::bullets::bullets_hash;

    fn note(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    fn hash(pairs: &[(u8, &str)]) -> String {
        bullets_hash(pairs.iter().copied())
    }

    #[test]
    fn finds_a_one_line_block() {
        let lines = note(concat!(
            "- [*] #task **0800-0815** Swings! ^swings\n",
            "\t- And retainers!\n",
            "- [*] #task **0900-0930** Next ^next\n",
        ));
        let expected = hash(&[(1, "- And retainers!")]);
        assert_eq!(
            find_block(&lines, 0, &expected, 1),
            Some(BulletWindow { start: 1, end: 2 })
        );
    }

    #[test]
    fn hash_ignores_indent_characters() {
        let tabs = note(concat!(
            "- [*] #task **0800-0815** Swings! ^swings\n",
            "\t- And retainers!\n",
        ));
        let spaces = note(concat!(
            "- [*] #task **0800-0815** Swings! ^swings\n",
            "  - And retainers!\n",
        ));
        let expected = hash(&[(1, "- And retainers!")]);
        assert_eq!(
            find_block(&tabs, 0, &expected, 1),
            Some(BulletWindow { start: 1, end: 2 })
        );
        assert_eq!(
            find_block(&spaces, 0, &expected, 1),
            Some(BulletWindow { start: 1, end: 2 })
        );
    }

    #[test]
    fn nested_depth_two_stays_in_the_window() {
        let lines = note(concat!(
            "- [*] #task **1400-1500** Onsite ^onsite\n",
            "\t- **Agenda:**\n",
            "\t\t- Intros\n",
            "\t\t- System design\n",
            "\t- Bring laptop & ID\n",
        ));
        let expected = hash(&[
            (1, "- **Agenda:**"),
            (2, "- Intros"),
            (2, "- System design"),
            (1, "- Bring laptop & ID"),
        ]);
        assert_eq!(
            find_block(&lines, 0, &expected, 4),
            Some(BulletWindow { start: 1, end: 5 })
        );
    }

    #[test]
    fn nested_note_hides_the_block() {
        let lines = note(concat!(
            "- [*] #task **0800-0815** Swings! ^swings\n",
            "\t- And retainers!\n",
            "\t\t- asked about the case\n",
        ));
        let expected = hash(&[(1, "- And retainers!")]);
        assert_eq!(find_block(&lines, 0, &expected, 1), None);
    }

    #[test]
    fn sibling_notes_above_or_below_do_not_hide() {
        let lines = note(concat!(
            "- [*] #task **0800-0815** Swings! ^swings\n",
            "\t- my note above\n",
            "\t- And retainers!\n",
            "\t- my note below\n",
        ));
        let expected = hash(&[(1, "- And retainers!")]);
        assert_eq!(
            find_block(&lines, 0, &expected, 1),
            Some(BulletWindow { start: 2, end: 3 })
        );
    }

    #[test]
    fn work_log_above_or_below_does_not_hide() {
        let lines = note(concat!(
            "- [*] #task **0800-0815** Swings! ^swings\n",
            "\t- 🛠️ **WORK LOG**\n",
            "\t\t- *12:00* — started\n",
            "\t- And retainers!\n",
            "\t- 🛠️ **WORK LOG**\n",
            "\t\t- *12:05* — wrapped\n",
        ));
        let expected = hash(&[(1, "- And retainers!")]);
        assert_eq!(
            find_block(&lines, 0, &expected, 1),
            Some(BulletWindow { start: 3, end: 4 })
        );
    }

    #[test]
    fn depends_on_above_does_not_hide() {
        let lines = note(concat!(
            "- [*] #task **0800-0815** Swings! ^swings\n",
            "\t- ⛓️ **DEPENDS ON:** [[#^prep]]\n",
            "\t- And retainers!\n",
        ));
        let expected = hash(&[(1, "- And retainers!")]);
        assert_eq!(
            find_block(&lines, 0, &expected, 1),
            Some(BulletWindow { start: 2, end: 3 })
        );
    }

    #[test]
    fn empty_window_is_the_insertion_point_after_managed() {
        let lines = note(concat!(
            "- [-] #task **1400-1500** Onsite ^onsite\n",
            "\t- ❌ **CANCEL LOG**\n",
            "\t\t- *2026-11-03* — Declined in Google Calendar\n",
            "\t- ⛓️ **DEPENDS ON:** [[#^prep]]\n",
            "\t- And retainers!\n",
        ));
        // CANCEL LOG then Depends-On: both managed, insertion after both.
        // Wait: Depends-On after CANCEL LOG is still a leading managed
        // run. Insertion is after Depends-On, before the Google bullet.
        assert_eq!(insertion_index(&lines, 0), 4);
        let empty = bullets_hash(std::iter::empty());
        assert_eq!(
            find_block(&lines, 0, &empty, 0),
            Some(BulletWindow { start: 4, end: 4 })
        );
    }

    #[test]
    fn empty_window_on_a_childless_row_is_directly_under_it() {
        let lines = note(concat!(
            "- [*] #task **0800-0815** Swings! ^swings\n",
            "- [*] #task **0900-0930** Next ^next\n",
        ));
        assert_eq!(insertion_index(&lines, 0), 1);
        let empty = bullets_hash(std::iter::empty());
        assert_eq!(
            find_block(&lines, 0, &empty, 0),
            Some(BulletWindow { start: 1, end: 1 })
        );
    }

    #[test]
    fn first_matching_window_wins() {
        let lines = note(concat!(
            "- [*] #task **0800-0815** Swings! ^swings\n",
            "\t- And retainers!\n",
            "\t- And retainers!\n",
        ));
        let expected = hash(&[(1, "- And retainers!")]);
        assert_eq!(
            find_block(&lines, 0, &expected, 1),
            Some(BulletWindow { start: 1, end: 2 })
        );
    }

    #[test]
    fn replace_window_swaps_or_removes() {
        let mut lines = note(concat!(
            "- [*] #task **0800-0815** Swings! ^swings\n",
            "\t- And retainers!\n",
            "\t- my note\n",
        ));
        replace_window(
            &mut lines,
            BulletWindow { start: 1, end: 2 },
            &["\t- Bring the case".to_string()],
        );
        assert_eq!(lines[1], "\t- Bring the case");
        assert_eq!(lines[2], "\t- my note");
        replace_window(&mut lines, BulletWindow { start: 1, end: 2 }, &[]);
        assert_eq!(lines[1], "\t- my note");
    }

    #[test]
    fn insertion_skips_a_cancel_log_subtree() {
        let lines = note(concat!(
            "- [-] #task **1400-1500** Onsite ^onsite\n",
            "\t- ❌ **CANCEL LOG**\n",
            "\t\t- *2026-11-03* — Declined in Google Calendar\n",
            "\t- And retainers!\n",
        ));
        assert_eq!(insertion_index(&lines, 0), 3);
    }

    #[test]
    fn a_wrong_hash_is_not_the_block() {
        let lines = note(concat!(
            "- [*] #task **0800-0815** Swings! ^swings\n",
            "\t- And retainers!\n",
        ));
        let other = hash(&[(1, "- Bring the case")]);
        assert_eq!(find_block(&lines, 0, &other, 1), None);
    }
}
