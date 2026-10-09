//! v1 tracker parser, moved verbatim from `ref_library::status` so
//! `ref_tasks` never depends on `ref_library`.

use crate::native::highlights_ref::{MANAGED_BODY_BEGIN, MANAGED_BODY_END};
use crate::native::markdown::fenced_lines;

/// One `^ref` tracker candidate found in the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrackerHit {
    pub mark: char,
    pub line: String,
}

/// All `^ref` tracker candidates: list task lines carrying the token, outside
/// fenced code and outside the managed region.
pub(crate) fn find_trackers(body: &str) -> Vec<TrackerHit> {
    let lines: Vec<&str> = body.lines().collect();
    let fenced = fenced_lines(&lines, 0..lines.len());
    let region = managed_region_line_range(&lines);
    lines
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            !fenced.contains(index)
                && region
                    .is_none_or(|(begin, end)| *index < begin || *index > end)
        })
        .filter_map(|(_, line)| parse_tracker_line(line))
        .collect()
}

/// The managed-region line range (inclusive) when exactly one begin marker is
/// followed by its end marker; anything broken reads as no region.
pub(crate) fn managed_region_line_range(
    lines: &[&str],
) -> Option<(usize, usize)> {
    let begin = lines
        .iter()
        .position(|line| line.contains(MANAGED_BODY_BEGIN))?;
    if lines
        .iter()
        .filter(|line| line.contains(MANAGED_BODY_BEGIN))
        .count()
        > 1
    {
        return None;
    }
    let end = lines[begin..]
        .iter()
        .position(|line| line.contains(MANAGED_BODY_END))
        .map(|offset| begin + offset)?;
    if lines
        .iter()
        .filter(|line| line.contains(MANAGED_BODY_END))
        .count()
        > 1
    {
        return None;
    }
    Some((begin, end))
}

/// A loose `^ref` tracker line: an unordered list task checkbox of any single
/// mark whose whitespace tokens include `^ref`.
pub(crate) fn parse_tracker_line(line: &str) -> Option<TrackerHit> {
    let trimmed = line.trim_start();
    let after_marker = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))?;
    let bracket = after_marker.strip_prefix('[')?;
    let mark = bracket.chars().next()?;
    let rest = bracket[mark.len_utf8()..].strip_prefix(']')?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    if !line.split_whitespace().any(|token| token == "^ref") {
        return None;
    }
    Some(TrackerHit {
        mark,
        line: line.trim().to_string(),
    })
}
