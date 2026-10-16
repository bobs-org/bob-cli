//! Word invariant and second-pass idempotency.

use super::plan::Refusal;
use super::scan::{ClassifiedLine, Region};
use super::sections::{strip_section_number, PlannedHeading};
use crate::native::markdown::atx_heading;

pub(crate) fn word_tokens(
    lines: &[&str],
    classified: &[ClassifiedLine<'_>],
    headings: &[PlannedHeading],
) -> Vec<String> {
    let heading_lines: Vec<usize> = headings.iter().map(|h| h.line).collect();
    let underlines: Vec<usize> =
        headings.iter().filter_map(|h| h.underline).collect();
    let mut tokens = Vec::new();
    for (index, content) in lines.iter().copied().enumerate() {
        if underlines.contains(&index) {
            continue;
        }
        if heading_lines.contains(&index)
            || classified
                .get(index)
                .is_some_and(|l| l.region == Region::AtxHeading)
        {
            let title = atx_heading(content)
                .map(|(_, title)| title)
                .unwrap_or_else(|| content.trim_start_matches('#').trim());
            let rest = strip_section_number(title).rest;
            push_tokens(rest, &mut tokens);
            continue;
        }
        if classified
            .get(index)
            .is_some_and(|l| l.region == Region::SetextUnderline)
        {
            continue;
        }
        push_tokens(content, &mut tokens);
    }
    tokens
}

fn push_tokens(text: &str, tokens: &mut Vec<String>) {
    tokens.extend(text.split_whitespace().map(ToOwned::to_owned));
}

pub(crate) fn words_changed_reason(
    before: &[String],
    after: &[String],
) -> Option<Refusal> {
    if before == after {
        return None;
    }
    let position = before
        .iter()
        .zip(after.iter())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| before.len().min(after.len()));
    let expected = before.get(position).map(String::as_str).unwrap_or("<end>");
    let found = after.get(position).map(String::as_str).unwrap_or("<end>");
    Some(Refusal {
        code: "words-changed".to_string(),
        message: format!(
            "word invariant failed at token {position}: expected {expected}, found {found}"
        ),
    })
}

pub(crate) fn not_idempotent() -> Refusal {
    Refusal {
        code: "not-idempotent".to_string(),
        message: "second format pass still edits the note".to_string(),
    }
}
