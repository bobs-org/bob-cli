//! Article outline fields and report-only word counts.

use std::path::Path;

use serde::Serialize;

use super::scan::{classify, ClassifiedLine, Document, Region};
use super::sections::PlannedHeading;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ArticleOutline {
    pub(crate) title: String,
    pub(crate) numbering: bool,
    pub(crate) words: usize,
    pub(crate) preamble: ArticlePreamble,
    pub(crate) sections: Vec<OutlineSection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ArticlePreamble {
    pub(crate) line: usize,
    pub(crate) words: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OutlineSection {
    pub(crate) number: Option<String>,
    pub(crate) written: Option<String>,
    pub(crate) stale: bool,
    pub(crate) depth: usize,
    pub(crate) level: usize,
    pub(crate) title: String,
    pub(crate) line: usize,
    pub(crate) end_line: usize,
    pub(crate) output_line: usize,
    pub(crate) words: usize,
    pub(crate) stub: bool,
}

pub(crate) fn build(
    input: &Document<'_>,
    headings: &[PlannedHeading],
    output: &str,
    title: Option<&str>,
    numbering: bool,
    path: &Path,
) -> ArticleOutline {
    let display_title = title
        .filter(|title| !title.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_owned()
        });
    let first_heading = headings
        .first()
        .map_or(input.lines.len(), |heading| heading.line);
    let words = count_range(&input.lines, 0, input.lines.len());
    let preamble_line = first_body_line(&input.lines);
    let preamble = ArticlePreamble {
        line: preamble_line,
        words: count_range(
            &input.lines,
            0,
            first_heading.min(input.lines.len()),
        ),
    };

    let output_doc = classify(output);
    let output_heading_lines = headings
        .iter()
        .scan(0_usize, |cursor, heading| {
            let found = output_doc
                .lines
                .iter()
                .enumerate()
                .skip(*cursor)
                .find(|(_, line)| {
                    line.region == Region::AtxHeading
                        && line.content == heading.rewritten
                })
                .map(|(index, _)| index);
            if let Some(index) = found {
                *cursor = index + 1;
            }
            Some(found.map_or(0, |index| index + 1))
        })
        .collect::<Vec<_>>();

    let sections = headings
        .iter()
        .enumerate()
        .map(|(index, heading)| {
            let end = headings
                .get(index + 1)
                .map_or(input.lines.len(), |next| next.line);
            let body_start = heading.line.saturating_add(1).min(end);
            let output_line = output_heading_lines
                .get(index)
                .copied()
                .unwrap_or(heading.line + 1);
            OutlineSection {
                number: heading.number.clone(),
                written: heading.written.clone(),
                stale: heading.written != heading.number,
                depth: heading.depth,
                level: heading.new_level,
                title: heading.title.clone(),
                line: heading.line + 1,
                end_line: end,
                output_line,
                words: count_range(&input.lines, body_start, end),
                stub: is_stub(&input.lines, body_start, end),
            }
        })
        .collect();

    ArticleOutline {
        title: display_title,
        numbering,
        words,
        preamble,
        sections,
    }
}

pub(crate) fn empty_outline(path: &Path, numbering: bool) -> ArticleOutline {
    ArticleOutline {
        title: path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_owned(),
        numbering,
        words: 0,
        preamble: ArticlePreamble { line: 1, words: 0 },
        sections: Vec::new(),
    }
}

fn first_body_line(lines: &[ClassifiedLine<'_>]) -> usize {
    lines
        .iter()
        .find(|line| {
            line.region != Region::Frontmatter
                && line.region != Region::Blank
                && !is_non_prose(line.region)
        })
        .map_or_else(
            || {
                lines
                    .iter()
                    .find(|line| line.region != Region::Frontmatter)
                    .map_or(1, |line| line.index + 1)
            },
            |line| line.index + 1,
        )
}

fn count_range(
    lines: &[ClassifiedLine<'_>],
    start: usize,
    end: usize,
) -> usize {
    lines
        .iter()
        .take(end)
        .skip(start)
        .filter(|line| !is_non_prose(line.region))
        .map(|line| super::marker::mask_inline_spans(line.content))
        .map(|line| line.split_whitespace().count())
        .sum()
}

fn is_non_prose(region: Region) -> bool {
    matches!(
        region,
        Region::Frontmatter
            | Region::Fence
            | Region::UnclosedFence
            | Region::Math
            | Region::PercentComment
            | Region::HtmlComment
            | Region::HtmlBlock
    )
}

fn is_stub(lines: &[ClassifiedLine<'_>], start: usize, end: usize) -> bool {
    for line in lines
        .iter()
        .take(end)
        .skip(start)
        .filter(|line| !is_non_prose(line.region))
    {
        let text = super::marker::mask_inline_spans(line.content);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if !todo_marker(text) {
            return false;
        }
    }
    true
}

fn todo_marker(text: &str) -> bool {
    let mut text = text.trim();
    loop {
        let stripped = text
            .strip_prefix("- [ ] ")
            .or_else(|| text.strip_prefix("- [x] "))
            .or_else(|| text.strip_prefix("* [ ] "))
            .or_else(|| text.strip_prefix("* [x] "))
            .or_else(|| text.strip_prefix("+ [ ] "))
            .or_else(|| text.strip_prefix("+ [x] "))
            .or_else(|| text.strip_prefix("- "))
            .or_else(|| text.strip_prefix("* "))
            .or_else(|| text.strip_prefix("+ "));
        let Some(stripped) = stripped else { break };
        text = stripped.trim_start();
    }
    let text = text
        .trim_matches('`')
        .trim_start_matches(['[', ' '])
        .trim_end_matches([']', ' ']);
    ["TODO", "TBD"].iter().any(|marker| {
        text.get(..marker.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(marker))
            && text.get(marker.len()..).is_none_or(|tail| {
                tail.is_empty()
                    || tail.starts_with([':', '.', '!', '?', ' ', '…'])
            })
    })
}
