//! Prose marker, options, and skip refusals.

use std::path::{Component, Path};

use super::plan::Refusal;
use crate::native::is_always_excluded_note_directory_name;
use crate::native::projects::{
    contains_task_tag, frontmatter_is_prose, frontmatter_type_names,
    is_prose_type_name, parse_frontmatter, trim_yaml_scalar, Frontmatter,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProseOptions {
    pub(crate) title: Option<String>,
    pub(crate) numbering: bool,
    pub(crate) bad_option_line: Option<usize>,
}

pub(crate) fn skip_reason(
    text: &str,
    path: &Path,
    classified: &[super::scan::ClassifiedLine<'_>],
) -> Option<Refusal> {
    if path_is_excluded(path) {
        return Some(Refusal {
            code: "excluded-directory".to_string(),
            message: format!(
                "skip {}: path is under an excluded directory",
                path.display()
            ),
        });
    }
    let text = strip_bom(text);
    if frontmatter_unclosed(text) {
        return Some(Refusal {
            code: "frontmatter-unclosed".to_string(),
            message: "opening --- never closes".to_string(),
        });
    }
    let Some(frontmatter) = parse_frontmatter(text) else {
        return Some(not_prose());
    };
    if !frontmatter_is_prose(&frontmatter) {
        return Some(not_prose());
    }
    let types = frontmatter_type_names(&frontmatter);
    if types.iter().any(|name| !is_prose_type_name(name)) {
        return Some(mixed_type_refusal(&types));
    }
    if let Some(refusal) = body_refusals(classified) {
        return Some(refusal);
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdoptDecision {
    AlreadyProse,
    Insert,
    Refuse(Refusal),
}

/// Whether `bob prose adopt` should insert a type, no-op, or refuse.
pub(crate) fn adopt_decision(
    text: &str,
    path: &Path,
    classified: &[super::scan::ClassifiedLine<'_>],
) -> AdoptDecision {
    if path_is_excluded(path) {
        return AdoptDecision::Refuse(Refusal {
            code: "excluded-directory".to_string(),
            message: format!(
                "skip {}: path is under an excluded directory",
                path.display()
            ),
        });
    }
    let text = strip_bom(text);
    if frontmatter_unclosed(text) {
        return AdoptDecision::Refuse(Refusal {
            code: "frontmatter-unclosed".to_string(),
            message: "opening --- never closes".to_string(),
        });
    }
    if let Some(frontmatter) = parse_frontmatter(text) {
        let types = frontmatter_type_names(&frontmatter);
        if frontmatter_is_prose(&frontmatter) {
            if types.iter().any(|name| !is_prose_type_name(name)) {
                return AdoptDecision::Refuse(mixed_type_refusal(&types));
            }
            return AdoptDecision::AlreadyProse;
        }
        if !types.is_empty() {
            return AdoptDecision::Refuse(Refusal {
                code: "mixed-type".to_string(),
                message: format!(
                    "note already has type [[{}]]",
                    types.join("]], [[")
                ),
            });
        }
    }
    if let Some(refusal) = body_refusals(classified) {
        return AdoptDecision::Refuse(refusal);
    }
    AdoptDecision::Insert
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProseTypeInsert {
    pub(crate) text: String,
    pub(crate) at: usize,
    pub(crate) count: usize,
}

const TYPE_LINE: &str = "type: \"[[prose]]\"";

pub(crate) fn set_section_numbers(text: &str, enabled: bool) -> String {
    if parse_frontmatter(strip_bom(text)).is_none() {
        return text.to_string();
    }
    let value = format!("section_numbers: {}", enabled);
    let mut lines = text
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    let closing = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| physical_content(line).trim() == "---")
        .map(|(index, _)| index)
        .expect("parsed frontmatter has a closing delimiter");

    if let Some((_index, line)) = lines
        .iter_mut()
        .enumerate()
        .take(closing)
        .skip(1)
        .find(|(_, line)| {
            physical_content(line).starts_with("section_numbers:")
        })
    {
        let ending = physical_ending(line).to_string();
        *line = format!("{value}{ending}");
        return lines.concat();
    }

    let Some(type_line) = lines
        .iter()
        .take(closing)
        .position(|line| physical_content(line).starts_with("type:"))
    else {
        return text.to_string();
    };
    let mut insert_at = type_line + 1;
    if physical_content(&lines[type_line])
        .strip_prefix("type:")
        .is_some_and(|rest| rest.trim().is_empty())
    {
        while insert_at < closing {
            let content = physical_content(&lines[insert_at]);
            if content.trim().is_empty()
                || content.starts_with(' ')
                || content.starts_with('\t')
            {
                insert_at += 1;
            } else {
                break;
            }
        }
    }
    let ending = lines
        .get(type_line)
        .map(|line| physical_ending(line))
        .filter(|ending| !ending.is_empty())
        .or_else(|| lines.first().map(|line| physical_ending(line)))
        .filter(|ending| !ending.is_empty())
        .unwrap_or("\n");
    lines.insert(insert_at, format!("{value}{ending}"));
    lines.concat()
}

fn physical_content(line: &str) -> &str {
    let content = line
        .strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .or_else(|| line.strip_suffix('\r'))
        .unwrap_or(line);
    content.strip_prefix('\u{feff}').unwrap_or(content)
}

fn physical_ending(line: &str) -> &str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else if line.ends_with('\r') {
        "\r"
    } else {
        ""
    }
}

/// Insert `type: "[[prose]]"` after `parent:`, before the closing
/// `---` when there is no parent, or as a new frontmatter block.
pub(crate) fn with_prose_type(text: &str) -> ProseTypeInsert {
    let (bom, rest) = match text.strip_prefix('\u{feff}') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let mut insert = insert_type_line(rest);
    if bom {
        insert.text.insert(0, '\u{feff}');
    }
    insert
}

fn insert_type_line(text: &str) -> ProseTypeInsert {
    let physical = super::scan::split_physical_lines(text);
    let default_ending = physical
        .iter()
        .find_map(|(_, ending)| (!ending.is_empty()).then_some(*ending))
        .unwrap_or("\n");
    let opens = physical
        .first()
        .is_some_and(|(content, _)| content.trim_end() == "---");
    if !opens {
        let mut out = String::from("---");
        out.push_str(default_ending);
        out.push_str(TYPE_LINE);
        out.push_str(default_ending);
        out.push_str("---");
        out.push_str(default_ending);
        out.push_str(default_ending);
        out.push_str(text);
        return ProseTypeInsert {
            text: out,
            at: 0,
            count: 4,
        };
    }

    let Some(close) = physical.iter().enumerate().skip(1).find_map(
        |(index, (content, _))| (content.trim_end() == "---").then_some(index),
    ) else {
        return ProseTypeInsert {
            text: text.to_owned(),
            at: 0,
            count: 0,
        };
    };

    let parent = physical[1..close].iter().enumerate().find_map(
        |(offset, (content, _))| {
            content
                .trim_start()
                .starts_with("parent:")
                .then_some(offset + 1)
        },
    );
    let insert_at = parent.map(|index| index + 1).unwrap_or(close);
    let insert_ending = physical
        .get(insert_at.saturating_sub(1))
        .map(|(_, ending)| *ending)
        .filter(|ending| !ending.is_empty())
        .unwrap_or(default_ending);

    let mut out = String::new();
    for (index, (content, ending)) in physical.iter().enumerate() {
        if index == insert_at {
            out.push_str(TYPE_LINE);
            out.push_str(insert_ending);
        }
        out.push_str(content);
        out.push_str(ending);
    }
    ProseTypeInsert {
        text: out,
        at: insert_at,
        count: 1,
    }
}

fn mixed_type_refusal(types: &[String]) -> Refusal {
    let prose_spelling = types
        .iter()
        .find(|name| is_prose_type_name(name))
        .map(String::as_str)
        .unwrap_or("prose");
    Refusal {
        code: "mixed-type".to_string(),
        message: format!(
            "type names [[{}]] as well as [[{prose_spelling}]]",
            types
                .iter()
                .filter(|name| !is_prose_type_name(name))
                .cloned()
                .collect::<Vec<_>>()
                .join("]], [[")
        ),
    }
}

pub(crate) fn parse_options(text: &str) -> ProseOptions {
    let text = strip_bom(text);
    let Some(frontmatter) = parse_frontmatter(text) else {
        return ProseOptions {
            title: None,
            numbering: true,
            bad_option_line: None,
        };
    };
    let title =
        crate::native::projects::frontmatter_value(&frontmatter, "title")
            .map(trim_yaml_scalar)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
    let (numbering, bad_option_line) =
        parse_section_numbers(&frontmatter, text);
    ProseOptions {
        title,
        numbering,
        bad_option_line,
    }
}

fn parse_section_numbers(
    frontmatter: &Frontmatter<'_>,
    text: &str,
) -> (bool, Option<usize>) {
    let Some(raw) = crate::native::projects::frontmatter_value(
        frontmatter,
        "section_numbers",
    ) else {
        return (true, None);
    };
    let value = trim_yaml_scalar(raw.trim());
    match value {
        "true" => (true, None),
        "false" => (false, None),
        _ => (true, Some(section_numbers_line(text))),
    }
}

fn section_numbers_line(text: &str) -> usize {
    let mut lines = text.lines().enumerate();
    if lines
        .next()
        .is_some_and(|(_, line)| line.trim_end() == "---")
    {
        for (index, line) in lines {
            if line.trim_end() == "---" {
                break;
            }
            if line.trim_start().starts_with("section_numbers:") {
                return index + 1;
            }
        }
    }
    1
}

fn not_prose() -> Refusal {
    Refusal {
        code: "not-article".to_string(),
        message: "not a prose note; run `bob prose adopt`".to_string(),
    }
}

pub(crate) fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

fn frontmatter_unclosed(text: &str) -> bool {
    let Some(first) = text.lines().next() else {
        return false;
    };
    if first.trim_end() != "---" {
        return false;
    }
    parse_frontmatter(text).is_none()
}

fn path_is_excluded(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => is_always_excluded_note_directory_name(name),
        _ => false,
    })
}

fn body_refusals(
    classified: &[super::scan::ClassifiedLine<'_>],
) -> Option<Refusal> {
    let mut task_lines = Vec::new();
    let mut holds_anchor = false;
    let mut conflict = false;
    for line in classified {
        if line.region.protects_body() {
            continue;
        }
        let masked = mask_inline_spans(line.content);
        if line.region != super::scan::Region::IndentedCode
            && is_task_checkbox_line(line.content)
            && contains_task_tag(&masked)
        {
            task_lines.push(line.index + 1);
        }
        if contains_block_anchor(&masked) {
            holds_anchor = true;
        }
        if is_conflict_marker(line.content) {
            conflict = true;
        }
    }
    if conflict {
        return Some(Refusal {
            code: "conflict-markers".to_string(),
            message: "note contains Git conflict markers".to_string(),
        });
    }
    if holds_anchor {
        return Some(Refusal {
            code: "holds-anchor".to_string(),
            message: "prose notes cannot be projects or refs".to_string(),
        });
    }
    if !task_lines.is_empty() {
        let line_word = if task_lines.len() == 1 {
            "line"
        } else {
            "lines"
        };
        let listed = task_lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Some(Refusal {
            code: "holds-tasks".to_string(),
            message: format!(
                "move them to the parent note (Ctrl+Shift+M); #task on {line_word} {listed}"
            ),
        });
    }
    None
}

fn is_task_checkbox_line(line: &str) -> bool {
    let line = line.trim_start();
    let marker_end = if line
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b'-' | b'*' | b'+'))
    {
        line.as_bytes()
            .get(1)
            .is_some_and(u8::is_ascii_whitespace)
            .then_some(2)
    } else {
        let digits = line
            .as_bytes()
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        (digits > 0
            && line
                .as_bytes()
                .get(digits)
                .is_some_and(|byte| matches!(byte, b'.' | b')'))
            && line
                .as_bytes()
                .get(digits + 1)
                .is_some_and(u8::is_ascii_whitespace))
        .then_some(digits + 2)
    };
    let Some(marker_end) = marker_end else {
        return false;
    };
    let body = line[marker_end..].trim_start();
    let mut chars = body.chars();
    if chars.next() != Some('[') || chars.next().is_none() {
        return false;
    }
    if chars.next() != Some(']') {
        return false;
    }
    chars.next().is_none_or(char::is_whitespace)
}

fn contains_block_anchor(text: &str) -> bool {
    contains_anchor_token(text, "^prj") || contains_anchor_token(text, "^ref")
}

fn contains_anchor_token(text: &str, token: &str) -> bool {
    let mut offset = 0;
    while let Some(relative) = text[offset..].find(token) {
        let start = offset + relative;
        let end = start + token.len();
        let before_ok = start == 0
            || text[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let after_ok = end == text.len()
            || text[end..].chars().next().is_some_and(char::is_whitespace);
        if before_ok && after_ok {
            return true;
        }
        offset = start + 1;
    }
    false
}

fn is_conflict_marker(line: &str) -> bool {
    let trimmed = line.trim_end();
    trimmed.starts_with("<<<<<<<") || trimmed.starts_with(">>>>>>>")
}

pub(crate) fn mask_inline_spans(line: &str) -> String {
    let mask =
        super::scan::inline_mask(line, super::scan::InlineMaskMode::Marker);
    line.char_indices()
        .map(|(byte, character)| {
            if mask.get(byte).copied().unwrap_or(false) {
                ' '
            } else {
                character
            }
        })
        .collect()
}
