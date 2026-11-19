//! Readers for the pre-frontmatter intention and reflection shapes.

use std::collections::BTreeMap;

use crate::native::{daily_headline, markdown, periods::calendar::PeriodId};

pub(crate) fn intentions(
    id: IntentionsKind,
    contents: &str,
) -> Option<Vec<String>> {
    match id {
        IntentionsKind::Day => {
            parse_daily_headline(contents).map(|headline| vec![headline.text])
        }
        IntentionsKind::Period(PeriodId::Week { .. }) => {
            top_level_bullets(&sections(contents).get("Intentions")?.clone())
        }
        IntentionsKind::Period(PeriodId::Quarter { .. }) => {
            top_level_bullets(&sections(contents).get("Outcomes")?.clone())
        }
        IntentionsKind::Period(PeriodId::Year(_)) => {
            let sections = sections(contents);
            let mut values = Vec::new();
            if let Some(theme) =
                sections.get("Theme").and_then(|body| paragraph(body))
            {
                values.push(theme);
            }
            if let Some(outcomes) = sections.get("Outcomes") {
                values.extend(top_level_bullets(outcomes).unwrap_or_default());
            }
            (!values.is_empty()).then_some(values)
        }
        IntentionsKind::Period(PeriodId::AllTime) => None,
    }
}

/// Legacy daily preamble parser. `daily_headline::parse` remains a thin
/// compatibility wrapper for existing internal callers.
pub(crate) fn parse_daily_headline(
    contents: &str,
) -> Option<daily_headline::DailyHeadline> {
    let lines: Vec<&str> = contents.lines().collect();
    let start = markdown::strictly_closed_frontmatter_end(&lines)
        .map(|end| end + 1)
        .unwrap_or(0);
    let fenced = markdown::fenced_lines(&lines, start..lines.len());
    let mut index = start;
    while index < lines.len() {
        if fenced.contains(&index) {
            index += 1;
            continue;
        }
        let line = lines[index];
        if markdown::atx_heading(line).is_some_and(|(level, _)| level >= 2) {
            break;
        }
        if let Some(rest) = label_rest(line, "Headline") {
            let mut words: Vec<&str> = rest.split_whitespace().collect();
            let mut next = index + 1;
            while next < lines.len()
                && !fenced.contains(&next)
                && !daily_headline::ends_headline_paragraph(lines[next])
            {
                words.extend(lines[next].split_whitespace());
                next += 1;
            }
            if !words.is_empty() {
                let text = words.join(" ");
                let display = daily_headline::display_text(&text);
                return Some(daily_headline::DailyHeadline { text, display });
            }
            index = next.max(index + 1);
        } else {
            index += 1;
        }
    }
    None
}

pub(crate) fn reflection(
    kind: IntentionsKind,
    contents: &str,
) -> Option<String> {
    match kind {
        IntentionsKind::Day => preamble_label(contents, "Reflection"),
        IntentionsKind::Period(PeriodId::AllTime) => None,
        IntentionsKind::Period(_) => {
            let body = sections(contents).remove("Reflection")?;
            nonempty(body)
        }
    }
}

pub(crate) fn summary(kind: IntentionsKind, contents: &str) -> Option<String> {
    match kind {
        IntentionsKind::Day | IntentionsKind::Period(PeriodId::AllTime) => None,
        IntentionsKind::Period(_) => {
            let body = sections(contents).remove("Review")?;
            opening_headline(&body)
        }
    }
}

pub(crate) fn period_sections(contents: &str) -> BTreeMap<String, String> {
    sections(contents)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntentionsKind {
    Day,
    Period(PeriodId),
}

fn sections(contents: &str) -> BTreeMap<String, String> {
    let lines: Vec<&str> = contents.lines().collect();
    let mut index = 0;
    if lines.first().is_some_and(|line| {
        line.trim_start_matches('\u{feff}').trim_end() == "---"
    }) {
        index = 1;
        while index < lines.len() && lines[index].trim_end() != "---" {
            index += 1;
        }
        if index < lines.len() {
            index += 1;
        }
    }
    let fenced = markdown::fenced_lines(&lines, index..lines.len());
    let mut result = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut body = Vec::new();
    for (line_index, line) in lines.iter().enumerate().skip(index) {
        if fenced.contains(&line_index) {
            continue;
        }
        if line.starts_with("> [!agent]")
            || line.trim() == "## Archive"
            || line.starts_with("## Archive ")
        {
            break;
        }
        if let Some(name) = line.strip_prefix("## ") {
            if let Some(previous) = current.take() {
                result.insert(previous, trim_body(&body));
            }
            current = Some(name.trim().to_owned());
            body.clear();
        } else if current.is_some() {
            body.push((*line).to_owned());
        }
    }
    if let Some(previous) = current {
        result.insert(previous, trim_body(&body));
    }
    result
}

fn trim_body(lines: &[String]) -> String {
    lines.join("\n").trim().to_owned()
}

fn top_level_bullets(body: &str) -> Option<Vec<String>> {
    let lines: Vec<_> = body.lines().collect();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    let mut values: Vec<String> = Vec::new();
    let mut active: Option<usize> = None;
    for (index, line) in lines.iter().enumerate() {
        if fenced.contains(&index) {
            continue;
        }
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        let is_bullet = indent == 0
            && (trimmed.starts_with("- ")
                || trimmed.starts_with("* ")
                || trimmed.starts_with("+ "));
        if is_bullet {
            values.push(trimmed[2..].trim().to_owned());
            active = Some(values.len() - 1);
        } else if indent > 0
            && let Some(which) = active
            && !trimmed.is_empty()
            && !trimmed.starts_with(['-', '*', '+'])
        {
            values[which].push(' ');
            values[which].push_str(trimmed);
        } else if !trimmed.is_empty() {
            active = None;
        }
    }
    values.retain(|value| !value.trim().is_empty());
    (!values.is_empty()).then_some(values)
}

fn paragraph(body: &str) -> Option<String> {
    let mut lines = Vec::new();
    for line in body.lines() {
        if lines.is_empty() && line.trim().is_empty() {
            continue;
        }
        if line.trim().is_empty() || markdown::atx_heading(line).is_some() {
            break;
        }
        lines.push(line.trim());
    }
    let text = lines
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    nonempty(text)
}

fn preamble_label(contents: &str, wanted: &str) -> Option<String> {
    let lines: Vec<&str> = contents.lines().collect();
    let start = markdown::strictly_closed_frontmatter_end(&lines)
        .map(|end| end + 1)
        .unwrap_or(0);
    let fenced = markdown::fenced_lines(&lines, start..lines.len());
    let mut index = start;
    while index < lines.len() {
        let line = lines[index];
        if fenced.contains(&index) {
            index += 1;
            continue;
        }
        if markdown::atx_heading(line).is_some_and(|(level, _)| level >= 2) {
            break;
        }
        if let Some(rest) = label_rest(line, wanted) {
            let mut words: Vec<&str> = rest.split_whitespace().collect();
            let mut next = index + 1;
            while next < lines.len()
                && !fenced.contains(&next)
                && !daily_headline::ends_headline_paragraph(lines[next])
            {
                words.extend(lines[next].split_whitespace());
                next += 1;
            }
            let text = words.join(" ");
            if !text.trim().is_empty() {
                return Some(text);
            }
            index = next.max(index + 1);
        } else {
            index += 1;
        }
    }
    None
}

fn label_rest<'a>(line: &'a str, wanted: &str) -> Option<&'a str> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("**")?;
    let close = rest.find("**")?;
    let label = &rest[..close];
    let after = &rest[close + 2..];
    if label.eq_ignore_ascii_case(&format!("{wanted}:")) {
        Some(after)
    } else if label.eq_ignore_ascii_case(wanted) {
        after.strip_prefix(':')
    } else {
        None
    }
}

pub(crate) fn opening_headline(body: &str) -> Option<String> {
    let mut lines = body.lines();
    let first = lines.next()?;
    let rest = label_rest(first, "Headline")?;
    let mut words: Vec<&str> = rest.split_whitespace().collect();
    for line in lines {
        if daily_headline::ends_headline_paragraph(line) {
            break;
        }
        words.extend(line.split_whitespace());
    }
    nonempty(words.join(" "))
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}
