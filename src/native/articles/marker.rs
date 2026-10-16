//! Article marker, options, and skip refusals.

use std::path::{Component, Path};

use super::plan::Refusal;
use crate::native::is_always_excluded_note_directory_name;
use crate::native::projects::{
    contains_task_tag, frontmatter_is_article, frontmatter_type_names,
    parse_frontmatter, trim_yaml_scalar, Frontmatter,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArticleOptions {
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
        return Some(not_article());
    };
    if !frontmatter_is_article(&frontmatter) {
        return Some(not_article());
    }
    let types = frontmatter_type_names(&frontmatter);
    if types.iter().any(|name| name != "article") {
        return Some(Refusal {
            code: "mixed-type".to_string(),
            message: format!(
                "type names [[{}]] as well as [[article]]",
                types
                    .iter()
                    .filter(|name| *name != "article")
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("]], [[")
            ),
        });
    }
    if let Some(refusal) = body_refusals(classified) {
        return Some(refusal);
    }
    None
}

pub(crate) fn parse_options(text: &str) -> ArticleOptions {
    let text = strip_bom(text);
    let Some(frontmatter) = parse_frontmatter(text) else {
        return ArticleOptions {
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
    ArticleOptions {
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
    text.lines()
        .take_while(|line| line.trim_end() != "---")
        .enumerate()
        .find_map(|(index, line)| {
            line.trim_start()
                .starts_with("section_numbers:")
                .then_some(index + 1)
        })
        .unwrap_or(1)
}

fn not_article() -> Refusal {
    Refusal {
        code: "not-article".to_string(),
        message: "not an article; run `bob articles adopt`".to_string(),
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
        if contains_task_tag(&masked) {
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
            message: "articles cannot be projects or refs".to_string(),
        });
    }
    if !task_lines.is_empty() {
        let listed = task_lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Some(Refusal {
            code: "holds-tasks".to_string(),
            message: format!(
                "move them to the parent note (Ctrl+Shift+M); #task on lines {listed}"
            ),
        });
    }
    None
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
    let mut out = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '`' {
            let ticks = chars[i..].iter().take_while(|c| **c == '`').count();
            if let Some(rel) = chars[i + ticks..]
                .windows(ticks)
                .position(|window| window.iter().all(|c| *c == '`'))
            {
                let end = i + ticks + rel + ticks;
                out.extend(std::iter::repeat_n(' ', end - i));
                i = end;
                continue;
            }
        }
        if chars[i] == '$'
            && chars.get(i + 1) == Some(&'$')
            && let Some(rel) =
                chars[i + 2..].windows(2).position(|w| w == ['$', '$'])
        {
            let end = i + 2 + rel + 2;
            out.extend(std::iter::repeat_n(' ', end - i));
            i = end;
            continue;
        }
        if chars[i] == '%'
            && chars.get(i + 1) == Some(&'%')
            && let Some(rel) =
                chars[i + 2..].windows(2).position(|w| w == ['%', '%'])
        {
            let end = i + 2 + rel + 2;
            out.extend(std::iter::repeat_n(' ', end - i));
            i = end;
            continue;
        }
        if line[byte_index(&chars, i)..].starts_with("<!--")
            && let Some(rel) = line[byte_index(&chars, i) + 4..].find("-->")
        {
            let start_byte = byte_index(&chars, i);
            let end_byte = start_byte + 4 + rel + 3;
            let end_char = line[..end_byte].chars().count();
            out.extend(std::iter::repeat_n(' ', end_char - i));
            i = end_char;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn byte_index(chars: &[char], char_index: usize) -> usize {
    chars[..char_index].iter().map(|c| c.len_utf8()).sum()
}
