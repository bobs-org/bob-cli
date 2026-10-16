//! Sections, levels, numbering, the ownership token, and identity.

use std::sync::LazyLock;

use regex::Regex;

use super::findings::Finding;
use super::plan::{ArticleSection, ChangeCounts, ReplacedToken};
use super::scan::{setext_span, ClassifiedLine, Region};
use crate::native::markdown::{atx_heading, setext_underline};

static SECTION_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{1,3}\.|\d{1,3}(?:\.\d{1,3})+\.?)(?:[ \t]+|$)")
        .expect("section number token")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StrippedNumber<'a> {
    pub(crate) token: Option<&'a str>,
    pub(crate) rest: &'a str,
}

/// Remove exactly one leading house-style number token from a heading title.
pub(crate) fn strip_section_number(title: &str) -> StrippedNumber<'_> {
    let Some(caps) = SECTION_NUMBER.captures(title) else {
        return StrippedNumber {
            token: None,
            rest: title,
        };
    };
    let token = caps.get(1).map(|m| m.as_str());
    let rest = title[caps.get(0).map(|m| m.end()).unwrap_or(0)..].trim_start();
    StrippedNumber { token, rest }
}

pub(crate) fn heading_identity(title: &str) -> String {
    normalize_identity(strip_section_number(title).rest)
}

pub(crate) fn heading_plain_identity(title: &str) -> String {
    normalize_identity(&strip_inline_markup(strip_section_number(title).rest))
}

fn normalize_identity(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn strip_inline_markup(text: &str) -> String {
    let without_links = replace_wikilinks(text);
    let mut out = String::with_capacity(without_links.len());
    let chars: Vec<char> = without_links.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '`' {
            let ticks = chars[i..].iter().take_while(|c| **c == '`').count();
            let rest = &chars[i + ticks..];
            if let Some(rel) = rest
                .windows(ticks)
                .position(|window| window.iter().all(|c| *c == '`'))
            {
                let inner = &rest[..rel];
                out.extend(inner.iter().copied());
                i += ticks + rel + ticks;
                continue;
            }
        }
        if (chars[i] == '*' || chars[i] == '_')
            && chars.get(i + 1) == Some(&chars[i])
        {
            i += 2;
            continue;
        }
        if chars[i] == '*' || chars[i] == '_' {
            i += 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn replace_wikilinks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        out.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        let Some(end) = rest.find("]]") else {
            out.push_str("[[");
            out.push_str(rest);
            return out;
        };
        let inner = &rest[..end];
        let display = inner.rsplit_once('|').map_or(inner, |(_, alias)| alias);
        out.push_str(display);
        rest = &rest[end + 2..];
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedHeading {
    pub(crate) line: usize,
    pub(crate) original_level: usize,
    pub(crate) new_level: usize,
    pub(crate) depth: usize,
    pub(crate) title: String,
    pub(crate) written: Option<String>,
    pub(crate) number: Option<String>,
    pub(crate) rewritten: String,
    pub(crate) setext: bool,
    pub(crate) setext_multiline: bool,
    pub(crate) underline: Option<usize>,
}

pub(crate) struct Numbering {
    pub(crate) headings: Vec<PlannedHeading>,
    pub(crate) findings: Vec<Finding>,
    pub(crate) replaced: Vec<ReplacedToken>,
    pub(crate) changes: ChangeCounts,
}

pub(crate) fn plan_headings(
    lines: &[ClassifiedLine<'_>],
    numbering: bool,
) -> Numbering {
    let mut findings = Vec::new();
    let mut raw = collect_headings(lines);
    raw.retain(|heading| {
        if heading.setext_multiline {
            findings.push(Finding::warning(
                "setext-multiline",
                heading.underline.map_or(heading.line + 1, |u| u + 1),
                "multi-line Setext heading left unchanged",
                "Convert it by hand",
            ));
            false
        } else {
            true
        }
    });
    if raw.is_empty() {
        return Numbering {
            headings: Vec::new(),
            findings,
            replaced: Vec::new(),
            changes: ChangeCounts::default(),
        };
    }
    let base = raw.iter().map(|h| h.original_level).min().unwrap_or(1);
    for index in 0..raw.len() {
        let mut new_level = base;
        for earlier in (0..index).rev() {
            if raw[earlier].original_level < raw[index].original_level {
                new_level = raw[earlier].new_level.saturating_add(1);
                break;
            }
        }
        if new_level > raw[index].original_level {
            new_level = raw[index].original_level;
        }
        raw[index].new_level = new_level;
    }

    let mut headings = Vec::new();
    let mut replaced = Vec::new();
    let mut changes = ChangeCounts::default();
    let mut counters = [0_usize; 8];

    for raw_heading in &raw {
        let new_level = raw_heading.new_level;
        if new_level < raw_heading.original_level {
            changes.promoted += 1;
        }
        let depth = new_level.saturating_sub(base).saturating_add(1);
        if depth > 4 {
            findings.push(Finding::info(
                "deep-section",
                raw_heading.line + 1,
                format!("section is depth {depth} (beyond 4)"),
                "Consider flattening",
            ));
        }
        if depth < counters.len() {
            counters[depth] += 1;
            for counter in counters.iter_mut().skip(depth + 1) {
                *counter = 0;
            }
        }
        let number = numbering.then(|| format_number(&counters, depth));
        if let Some(token) = raw_heading.written.as_deref()
            && let Some(number) = number.as_deref()
            && token_mismatch_depth(token, depth)
        {
            findings.push(Finding::warning(
                "number-replaced",
                raw_heading.line + 1,
                format!("replaced {token} with {number}"),
                "Escape it, as in `1\\.5`, if it was text",
            ));
        }
        if let Some(token) = raw_heading.written.as_deref() {
            let written_number = number.clone().unwrap_or_default();
            if !numbering || Some(token) != number.as_deref() {
                replaced.push(ReplacedToken {
                    line: raw_heading.line + 1,
                    token: token.to_string(),
                    number: written_number,
                });
            }
        }
        if numbering {
            changes.numbered += 1;
            if raw_heading.written.as_deref() != number.as_deref()
                && raw_heading.written.is_some()
            {
                changes.renumbered += 1;
            }
        } else if raw_heading.written.is_some() {
            changes.unnumbered += 1;
        }
        if raw_heading.setext && !raw_heading.setext_multiline {
            changes.setext += 1;
        }
        let rewritten = if raw_heading.setext_multiline {
            String::new()
        } else {
            rewrite_heading(new_level, number.as_deref(), &raw_heading.title)
        };
        headings.push(PlannedHeading {
            line: raw_heading.line,
            original_level: raw_heading.original_level,
            new_level,
            depth,
            title: raw_heading.title.clone(),
            written: raw_heading.written.clone(),
            number,
            rewritten,
            setext: raw_heading.setext,
            setext_multiline: raw_heading.setext_multiline,
            underline: raw_heading.underline,
        });
    }

    Numbering {
        headings,
        findings,
        replaced,
        changes,
    }
}

struct RawHeading {
    line: usize,
    original_level: usize,
    new_level: usize,
    title: String,
    written: Option<String>,
    setext: bool,
    setext_multiline: bool,
    underline: Option<usize>,
}

fn collect_headings(lines: &[ClassifiedLine<'_>]) -> Vec<RawHeading> {
    let mut headings = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        match line.region {
            Region::AtxHeading => {
                let Some((level, title)) = atx_heading(line.content) else {
                    continue;
                };
                let stripped = strip_section_number(title);
                headings.push(RawHeading {
                    line: index,
                    original_level: level,
                    new_level: level,
                    title: stripped.rest.to_string(),
                    written: stripped.token.map(ToOwned::to_owned),
                    setext: false,
                    setext_multiline: false,
                    underline: None,
                });
            }
            Region::SetextUnderline => {
                let Some((start, count)) = setext_span(lines, index) else {
                    continue;
                };
                let level = setext_underline(line.content).unwrap_or(1);
                let multiline = count > 1;
                let title_line = lines[start].content.trim_end();
                let stripped = strip_section_number(title_line.trim_start());
                headings.push(RawHeading {
                    line: start,
                    original_level: level,
                    new_level: level,
                    title: stripped.rest.to_string(),
                    written: stripped.token.map(ToOwned::to_owned),
                    setext: true,
                    setext_multiline: multiline,
                    underline: Some(index),
                });
            }
            _ => {}
        }
    }
    headings
}

fn format_number(counters: &[usize], depth: usize) -> String {
    let parts: Vec<String> = (1..=depth)
        .map(|d| counters.get(d).copied().unwrap_or(0).to_string())
        .collect();
    if depth <= 1 {
        format!("{}.", parts.first().map_or("1", String::as_str))
    } else {
        parts.join(".")
    }
}

fn token_mismatch_depth(token: &str, depth: usize) -> bool {
    token_part_count(token) != depth
}

fn token_part_count(token: &str) -> usize {
    token
        .trim_end_matches('.')
        .split('.')
        .filter(|part| !part.is_empty())
        .count()
}

fn rewrite_heading(level: usize, number: Option<&str>, title: &str) -> String {
    let hashes = "#".repeat(level.clamp(1, 6));
    match (number, title.is_empty()) {
        (Some(number), false) => format!("{hashes} {number} {title}"),
        (Some(number), true) => format!("{hashes} {number}"),
        (None, false) => format!("{hashes} {title}"),
        (None, true) => hashes,
    }
}

pub(crate) fn article_sections(
    headings: &[PlannedHeading],
    line_count: usize,
) -> Vec<ArticleSection> {
    headings
        .iter()
        .enumerate()
        .map(|(index, heading)| {
            let end_line = headings
                .get(index + 1)
                .map(|next| next.line)
                .unwrap_or(line_count);
            ArticleSection {
                number: heading.number.clone(),
                written: heading.written.clone(),
                depth: heading.depth,
                level: heading.new_level,
                original_level: heading.original_level,
                title: heading.title.clone(),
                line: heading.line + 1,
                end_line,
            }
        })
        .collect()
}
