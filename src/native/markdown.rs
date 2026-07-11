use std::collections::BTreeSet;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MarkdownFence {
    character: u8,
    length: usize,
}

pub(crate) fn fence_marker(line: &str) -> Option<MarkdownFence> {
    let indentation = line.bytes().take_while(|byte| *byte == b' ').count();
    if indentation > 3 {
        return None;
    }
    let line = &line[indentation..];
    let character = *line.as_bytes().first()?;
    if !matches!(character, b'`' | b'~') {
        return None;
    }
    let length = line.bytes().take_while(|byte| *byte == character).count();
    (length >= 3).then_some(MarkdownFence { character, length })
}

pub(crate) fn closes_fence(line: &str, open: MarkdownFence) -> bool {
    let Some(marker) = fence_marker(line) else {
        return false;
    };
    let trimmed = line.trim_start();
    marker.character == open.character
        && marker.length >= open.length
        && trimmed[marker.length..].trim().is_empty()
}

pub(crate) fn fenced_lines(
    lines: &[&str],
    range: Range<usize>,
) -> BTreeSet<usize> {
    let mut fenced = BTreeSet::new();
    let mut open = None;
    for index in range {
        let line = lines[index];
        if let Some(marker) = open {
            fenced.insert(index);
            if closes_fence(line, marker) {
                open = None;
            }
        } else if let Some(marker) = fence_marker(line) {
            fenced.insert(index);
            open = Some(marker);
        }
    }
    fenced
}

pub(crate) fn strictly_closed_frontmatter_end(lines: &[&str]) -> Option<usize> {
    if lines.first().copied().map(str::trim_end) != Some("---") {
        return None;
    }
    lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.trim_end() == "---").then_some(index))
}
