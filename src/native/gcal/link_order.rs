//! Keep same-note Event Task links in each Pomodoro entry in meeting-time order.

use std::collections::HashMap;

use chrono::NaiveDate;

use crate::native::{
    capture::line_spans,
    capture_pomodoro_start, capture_pomodoros,
    capture_task_toggle::child_block_end_line,
    event_task::{self, EventKind},
};

#[derive(Debug, Clone)]
struct LinkBlock {
    start: usize,
    end: usize,
    rank: usize,
}

/// Reorder each Pomodoro entry's same-note Event Task links by meeting span,
/// leaving every other child in its slot. Returns the new contents and how
/// many Event Task link positions changed.
pub(super) fn sort_event_task_links(
    contents: &str,
    day: NaiveDate,
) -> (String, usize) {
    let events = event_task::events_in_note(contents, day);
    let mut id_counts = HashMap::<&str, usize>::new();
    for event in &events {
        if event.kind == EventKind::Task
            && let Some(block_id) = event.block_id.as_deref()
            && !block_id.is_empty()
        {
            *id_counts.entry(block_id).or_default() += 1;
        }
    }

    let mut ranks = HashMap::<String, usize>::new();
    for (rank, event) in events.iter().enumerate() {
        if event.kind != EventKind::Task {
            continue;
        }
        let Some(block_id) = event.block_id.as_deref() else {
            continue;
        };
        if !block_id.is_empty() && id_counts.get(block_id) == Some(&1) {
            ranks.insert(block_id.to_string(), rank);
        }
    }
    if ranks.is_empty() {
        return (contents.to_string(), 0);
    }

    let scan = capture_pomodoros::scan(contents);
    if !scan.has_section {
        return (contents.to_string(), 0);
    }

    let spans = line_spans(contents);
    let mut lines: Vec<String> = contents.lines().map(str::to_string).collect();
    let note_stem = day.format("%Y%m%d").to_string();
    let mut links_sorted = 0;

    for entry in scan.entries {
        let entry_index = entry.line - 1;
        let mut blocks = Vec::new();
        for link in capture_pomodoro_start::session_links(contents, entry_index)
        {
            if !names_this_note(&link.path_part, &note_stem) {
                continue;
            }
            let Some(&rank) = ranks.get(&link.block_id) else {
                continue;
            };
            let start = link.ledger_line - 1;
            let end =
                (child_block_end_line(&spans, start) + 1).min(lines.len());
            if end > start {
                blocks.push(LinkBlock { start, end, rank });
            }
        }
        if blocks.len() < 2 {
            continue;
        }
        blocks.sort_by_key(|block| block.start);

        let mut permutation: Vec<usize> = (0..blocks.len()).collect();
        permutation.sort_by_key(|&index| blocks[index].rank);
        if permutation
            .iter()
            .enumerate()
            .all(|(slot, &index)| slot == index)
        {
            continue;
        }

        let region_start = blocks[0].start;
        let region_end = blocks.last().expect("at least two links").end;
        let mut replacement = Vec::with_capacity(region_end - region_start);
        let mut cursor = region_start;
        for (slot, block) in blocks.iter().enumerate() {
            replacement.extend(lines[cursor..block.start].iter().cloned());
            let source = &blocks[permutation[slot]];
            replacement.extend(lines[source.start..source.end].iter().cloned());
            cursor = block.end;
        }
        replacement.extend(lines[cursor..region_end].iter().cloned());
        debug_assert_eq!(replacement.len(), region_end - region_start);
        lines[region_start..region_end].clone_from_slice(&replacement);
        links_sorted += permutation
            .iter()
            .enumerate()
            .filter(|(slot, source)| slot != *source)
            .count();
    }

    if links_sorted == 0 {
        return (contents.to_string(), 0);
    }

    let mut sorted = lines.join("\n");
    if contents.ends_with('\n') && !lines.is_empty() {
        sorted.push('\n');
    }
    (sorted, links_sorted)
}

fn names_this_note(path_part: &str, note_stem: &str) -> bool {
    if path_part.is_empty() {
        return true;
    }
    let last = path_part.rsplit('/').next().unwrap_or(path_part);
    last.strip_suffix(".md").unwrap_or(last) == note_stem
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 30).expect("date")
    }

    fn sort(input: &str) -> (String, usize) {
        sort_event_task_links(input, day())
    }

    #[test]
    fn sorts_two_links_and_leaves_sorted_input_byte_identical() {
        let input = "## Events\n\n- [ ] #task **1900-1930** Late ^late\n- [ ] #task **0800-0830** Early ^early\n\n## Pomodoros\n- [ ] () — MEET\n  - [[#^late]]\n  - [[#^early]]\n";
        let expected = input.replace(
            "  - [[#^late]]\n  - [[#^early]]",
            "  - [[#^early]]\n  - [[#^late]]",
        );
        assert_eq!(sort(input), (expected.clone(), 2));
        assert_eq!(sort(&expected), (expected, 0));
    }

    #[test]
    fn non_event_children_keep_their_slots() {
        let input = "## Events\n\n- [ ] #task **1900-1930** Late ^late\n- [ ] #task **0800-0830** Early ^early\n\n## Pomodoros\n- [ ] () — MEET\n  - [[#^late]]\n  - [[sase#^task]]\n  - plain note\n  - [[#^early]]\n";
        let expected = input.replace(
            "  - [[#^late]]\n  - [[sase#^task]]\n  - plain note\n  - [[#^early]]",
            "  - [[#^early]]\n  - [[sase#^task]]\n  - plain note\n  - [[#^late]]",
        );
        assert_eq!(sort(input), (expected, 2));
    }

    #[test]
    fn link_subtrees_move_around_other_child_blocks() {
        let input = "## Events\n\n- [ ] #task **1900-1930** Late ^late\n- [ ] #task **0800-0830** Early ^early\n\n## Pomodoros\n- [ ] () — MEET\n  - [[#^late]]\n    - late child\n      - late grandchild\n\n  - plain note\n\n  - [[#^early]]\n    - early child\n";
        let input = input.replace(
            "    - late grandchild\n\n  - plain note\n\n  - [[#^early]]",
            "    - late grandchild\n  - plain note\n  - [[#^early]]",
        );
        let expected = "## Events\n\n- [ ] #task **1900-1930** Late ^late\n- [ ] #task **0800-0830** Early ^early\n\n## Pomodoros\n- [ ] () — MEET\n  - [[#^early]]\n    - early child\n  - plain note\n  - [[#^late]]\n    - late child\n      - late grandchild\n";
        assert_eq!(sort(&input), (expected.to_string(), 2));
    }

    #[test]
    fn sorts_closed_tomato_and_struck_links_but_not_mixed_or_deeper_links() {
        let input = "## Events\n\n- [ ] #task **1900-1930** Late ^late\n- [ ] #task **0800-0830** Early ^early\n\n## Pomodoros\n- [x] (**0800-1930** [t:: 30m]) — MEET\n  - 🍅 [[#^late]]\n    - [[#^early]]\n  - mixed text [[#^late]]\n  - 🍅 ~~[[#^early]]~~\n";
        let expected = "## Events\n\n- [ ] #task **1900-1930** Late ^late\n- [ ] #task **0800-0830** Early ^early\n\n## Pomodoros\n- [x] (**0800-1930** [t:: 30m]) — MEET\n  - 🍅 ~~[[#^early]]~~\n  - mixed text [[#^late]]\n  - 🍅 [[#^late]]\n    - [[#^early]]\n";
        let input = input.replace(
            "  - mixed text [[#^late]]\n  - 🍅 ~~[[#^early]]~~",
            "  - plain note\n    - [[#^early]]\n  - mixed text [[#^late]]\n  - 🍅 ~~[[#^early]]~~",
        );
        let expected = expected.replace(
            "  - mixed text [[#^late]]\n  - 🍅 [[#^late]]",
            "  - plain note\n    - [[#^early]]\n  - mixed text [[#^late]]\n  - 🍅 [[#^late]]",
        );
        assert_eq!(sort(&input), (expected, 2));
    }

    #[test]
    fn ties_use_end_then_event_row_order_and_duplicate_links_stay_stable() {
        let input = "## Events\n\n- [ ] #task **0900-1000** Longer ^long\n- [ ] #task **0900-0930** Shorter ^short\n- [ ] #task **0900-0930** Same span ^same\n\n## Pomodoros\n- [ ] () — MEET\n  - [[#^long]]\n  - 🍅 [[#^same]]\n  - [[#^short]]\n  - [[#^same]]\n";
        let expected = input.replace(
            "  - [[#^long]]\n  - 🍅 [[#^same]]\n  - [[#^short]]\n  - [[#^same]]",
            "  - [[#^short]]\n  - 🍅 [[#^same]]\n  - [[#^same]]\n  - [[#^long]]",
        );
        assert_eq!(sort(input), (expected, 3));
    }

    #[test]
    fn ignores_other_notes_unknown_ids_and_ambiguous_event_ids() {
        let input = "## Events\n\n- [ ] #task **1900-1930** Late ^late\n- [ ] #task **0800-0830** Early ^early\n- [ ] #task **0700-0730** Duplicate one ^duplicate\n- [ ] #task **0600-0630** Duplicate two ^duplicate\n\n## Pomodoros\n- [ ] () — MEET\n  - [[#^late]]\n  - [[2026/20261030#^early]]\n  - [[2026/20261029.md#^early]]\n  - [[#^unknown]]\n  - [[#^duplicate]]\n";
        let expected = input.replace(
            "  - [[#^late]]\n  - [[2026/20261030#^early]]",
            "  - [[2026/20261030#^early]]\n  - [[#^late]]",
        );
        assert_eq!(sort(input), (expected, 2));
    }

    #[test]
    fn each_entry_sorts_independently_and_cross_midnight_uses_start() {
        let input = "## Events\n\n- [ ] #task **2330-0030** Overnight ^overnight\n- [ ] #task **2200-2230** Earlier ^earlier\n- [ ] #task **0100-0130** Early ^early\n\n## Pomodoros\n- [ ] () — MEET\n  - [[#^overnight]]\n  - [[#^earlier]]\n- [ ] () — MEET\n  - [[#^early]]\n  - [[#^overnight]]\n";
        let expected = input.replace(
            "  - [[#^overnight]]\n  - [[#^earlier]]",
            "  - [[#^earlier]]\n  - [[#^overnight]]",
        );
        assert_eq!(sort(input), (expected, 2));
    }

    #[test]
    fn no_event_rows_or_no_pomodoros_returns_input_unchanged() {
        let no_events =
            "## Events\n\n- [ ] #task no time ^task\n\n## Pomodoros\n";
        let no_pomodoros =
            "## Events\n\n- [ ] #task **0800-0830** Early ^early\n";
        assert_eq!(sort(no_events), (no_events.to_string(), 0));
        assert_eq!(sort(no_pomodoros), (no_pomodoros.to_string(), 0));
    }
}
