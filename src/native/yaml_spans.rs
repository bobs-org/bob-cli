//! Album-agnostic YAML span primitives.
//!
//! Line endings, BOM, comments, flow-map keys, and scalar quoting are
//! shared by the album schema editor and the bank config editor. Domain
//! patchers live beside their commands.

use std::collections::HashSet;

use serde_yaml::Value as YamlValue;

/// One source line plus the newline bytes that followed it.
///
/// An empty `ending` means the source ended without a newline. A leading
/// BOM stays on the first line's `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    pub(crate) text: String,
    pub(crate) ending: String,
}

/// Split `text` into lines, preserving `\n` / `\r\n` endings and a
/// missing final newline.
pub(crate) fn split_lines(text: &str) -> Vec<Line> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\n' {
            let end = index + 1;
            let raw = &text[start..end];
            let (content, ending) =
                if let Some(content) = raw.strip_suffix("\r\n") {
                    (content, "\r\n")
                } else {
                    (
                        raw.strip_suffix('\n').expect("line ends with newline"),
                        "\n",
                    )
                };
            lines.push(Line {
                text: content.to_owned(),
                ending: ending.to_owned(),
            });
            start = end;
        }
        index += 1;
    }
    if start < text.len() {
        lines.push(Line {
            text: text[start..].to_owned(),
            ending: String::new(),
        });
    }
    lines
}

/// Join lines back into source bytes, including each stored ending.
pub(crate) fn join_lines(lines: &[Line]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(&line.text);
        out.push_str(&line.ending);
    }
    out
}

/// Frontmatter YAML with Unix newlines for `serde_yaml`.
pub(crate) fn yaml_text(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split a value fragment into the value and a trailing `#` comment.
///
/// The comment includes the `#`. Quotes hide interior hashes.
pub(crate) fn split_comment(value: &str) -> (&str, &str) {
    let bytes = value.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if let Some(current) = quote {
            if current == b'"' && escaped {
                escaped = false;
            } else if current == b'"' && byte == b'\\' {
                escaped = true;
            } else if current == b'\''
                && byte == b'\''
                && bytes.get(index + 1) == Some(&b'\'')
            {
                continue;
            } else if byte == current {
                quote = None;
            }
        } else if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
        } else if byte == b'#'
            && (index == 0 || bytes[index - 1].is_ascii_whitespace())
        {
            return (&value[..index], &value[index..]);
        }
    }
    (value, "")
}

/// Whitespace between `:` and the inline value, defaulting to one space.
pub(crate) fn gap_after_colon(after: &str) -> &str {
    let (value, _) = split_comment(after);
    let gap = value.len() - value.trim_start().len();
    if gap == 0 {
        " "
    } else {
        &value[..gap]
    }
}

/// Whitespace sitting immediately before a trailing comment.
pub(crate) fn whitespace_before_comment(after: &str) -> &str {
    let (value, comment) = split_comment(after);
    if comment.is_empty() {
        return "";
    }
    &value[value.trim_end().len()..]
}

pub(crate) fn blank_or_comment(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with('#')
}

pub(crate) fn indentation(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

pub(crate) fn has_tab_indent(line: &str) -> bool {
    let indent = indentation(line);
    indent > 0 && line[..indent].contains('\t')
}

pub(crate) fn first_content_indent(
    lines: &[Line],
    start: usize,
    end: usize,
) -> Option<usize> {
    (start..end.min(lines.len())).find_map(|index| {
        let line = &lines[index].text;
        (!blank_or_comment(line)).then(|| indentation(line))
    })
}

/// True when an unquoted `&`, `*`, or `!` token looks like an
/// anchor, alias, or tag.
pub(crate) fn has_yaml_feature(lines: &[Line]) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for line in lines {
        let bytes = line.text.as_bytes();
        for (index, byte) in bytes.iter().copied().enumerate() {
            if let Some(current) = quote {
                if current == b'"' && escaped {
                    escaped = false;
                } else if current == b'"' && byte == b'\\' {
                    escaped = true;
                } else if byte == current {
                    quote = None;
                }
                continue;
            }
            if byte == b'"' || byte == b'\'' {
                quote = Some(byte);
                continue;
            }
            if byte == b'#'
                && (index == 0 || bytes[index - 1].is_ascii_whitespace())
            {
                break;
            }
            if matches!(byte, b'&' | b'*' | b'!')
                && (index == 0
                    || bytes[index - 1].is_ascii_whitespace()
                    || matches!(
                        bytes[index - 1],
                        b'[' | b'{' | b',' | b':' | b'-'
                    ))
                && bytes.get(index + 1).is_some_and(|next| {
                    !next.is_ascii_whitespace() && *next != b'#'
                })
            {
                return true;
            }
        }
    }
    false
}

/// Why a flow mapping cannot be edited faithfully.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlowMapError {
    UnresolvableKey,
    DuplicateKey,
    CommentInside,
}

/// Duplicate keys, unresolvable keys, and comments inside a flow value.
pub(crate) fn validate_flow_map_keys(flow: &str) -> Result<(), FlowMapError> {
    let mut stack = Vec::<FlowFrame>::new();
    let bytes = flow.as_bytes();
    let mut index = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut key_start = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(current) = quote {
            if current == b'"' && escaped {
                escaped = false;
            } else if current == b'"' && byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            index += 1;
            continue;
        }
        if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
            index += 1;
            continue;
        }
        match byte {
            b'{' => {
                stack.push(FlowFrame::Map(HashSet::new(), true));
                key_start = index + 1;
            }
            b'[' => stack.push(FlowFrame::Sequence),
            b'}' | b']' => {
                stack.pop();
            }
            b',' => {
                if let Some(FlowFrame::Map(_, expecting_key)) = stack.last_mut()
                {
                    *expecting_key = true;
                    key_start = index + 1;
                }
            }
            b':' => {
                if let Some(FlowFrame::Map(keys, expecting_key)) =
                    stack.last_mut()
                    && *expecting_key
                {
                    let raw = flow[key_start..index].trim();
                    let key: YamlValue = serde_yaml::from_str(raw)
                        .map_err(|_| FlowMapError::UnresolvableKey)?;
                    if !keys.insert(key) {
                        return Err(FlowMapError::DuplicateKey);
                    }
                    *expecting_key = false;
                }
            }
            b'#' if index == 0 || bytes[index - 1].is_ascii_whitespace() => {
                return Err(FlowMapError::CommentInside);
            }
            _ => {}
        }
        index += 1;
    }
    Ok(())
}

enum FlowFrame {
    Map(HashSet<YamlValue>, bool),
    Sequence,
}

/// Colon that ends a block-mapping key at `indent`, ignoring colons
/// inside quotes or flow collections.
pub(crate) fn mapping_colon(line: &str, indent: usize) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    let mut flow = 0isize;
    let bytes = line.as_bytes();
    for index in indent..bytes.len() {
        let byte = bytes[index];
        if let Some(current) = quote {
            if current == b'"' && escaped {
                escaped = false;
            } else if current == b'"' && byte == b'\\' {
                escaped = true;
            } else if current == b'\''
                && byte == b'\''
                && bytes.get(index + 1) == Some(&b'\'')
            {
                continue;
            } else if byte == current {
                quote = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'{' | b'[' => flow += 1,
            b'}' | b']' => flow -= 1,
            b'#' if index == indent
                || bytes[index - 1].is_ascii_whitespace() =>
            {
                return None;
            }
            b':' if flow == 0
                && (index + 1 == bytes.len()
                    || bytes[index + 1].is_ascii_whitespace()) =>
            {
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

/// Parse a block-mapping key at `indent`. The colon index is into `line`.
pub(crate) fn parse_key(line: &str, indent: usize) -> Option<(String, usize)> {
    let colon = mapping_colon(line, indent)?;
    let key_text = line[indent..colon].trim();
    if key_text.is_empty() {
        return None;
    }
    let key: YamlValue = serde_yaml::from_str(key_text).ok()?;
    Some((key.as_str()?.to_owned(), colon))
}

/// True when a `{...}` or `[...]` flow value is closed on this fragment.
pub(crate) fn flow_is_closed(flow: &str) -> bool {
    let bytes = flow.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    let mut depth = 0isize;
    let mut started = false;
    for byte in bytes {
        let byte = *byte;
        if let Some(current) = quote {
            if current == b'"' && escaped {
                escaped = false;
            } else if current == b'"' && byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'{' | b'[' => {
                started = true;
                depth += 1;
            }
            b'}' | b']' => {
                depth -= 1;
            }
            _ => {}
        }
    }
    started && depth == 0
}

/// Render a YAML scalar the way album and bank emitters do.
pub(crate) fn render_scalar(value: &YamlValue) -> String {
    match value {
        YamlValue::Null => "null".to_owned(),
        YamlValue::Bool(value) => value.to_string(),
        YamlValue::Number(value) => value.to_string(),
        YamlValue::String(value) => {
            crate::native::records::yaml_string_scalar(value)
        }
        YamlValue::Sequence(_) => "[]".to_owned(),
        YamlValue::Mapping(_) => "{}".to_owned(),
        YamlValue::Tagged(tagged) => render_scalar(&tagged.value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_lines_keeps_bom_crlf_and_a_missing_final_newline() {
        let text = "\u{feff}---\r\nbank:\r\n  stale_after: 10";
        let lines = split_lines(text);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "\u{feff}---");
        assert_eq!(lines[0].ending, "\r\n");
        assert_eq!(lines[2].text, "  stale_after: 10");
        assert_eq!(lines[2].ending, "");
        assert_eq!(join_lines(&lines), text);
    }

    #[test]
    fn quoted_punctuation_is_not_an_anchor_token() {
        let lines = split_lines("label: \"Place &home *text !tag\"\n");
        assert!(!has_yaml_feature(&lines));
        let lines = split_lines("label: *summary\n");
        assert!(has_yaml_feature(&lines));
    }

    #[test]
    fn flow_map_validation_catches_duplicates_and_comments() {
        assert!(validate_flow_map_keys("{ a: 1, b: 2 }").is_ok());
        assert_eq!(
            validate_flow_map_keys("{ a: 1, a: 2 }"),
            Err(FlowMapError::DuplicateKey)
        );
        assert_eq!(
            validate_flow_map_keys("{ a: 1 # nope }"),
            Err(FlowMapError::CommentInside)
        );
    }

    #[test]
    fn split_comment_ignores_hashes_inside_quotes() {
        let (value, comment) = split_comment(" \"a # b\" # keep");
        assert_eq!(value, " \"a # b\" ");
        assert_eq!(comment, "# keep");
    }

    #[test]
    fn digit_scalars_are_quoted() {
        assert_eq!(render_scalar(&YamlValue::String("1".to_owned())), "\"1\"");
        assert_eq!(
            render_scalar(&YamlValue::String("coffee".to_owned())),
            "coffee"
        );
        assert_eq!(render_scalar(&YamlValue::Bool(true)), "true");
        assert_eq!(render_scalar(&YamlValue::Number(10.into())), "10");
    }

    #[test]
    fn flow_is_closed_tracks_braces_outside_quotes() {
        assert!(flow_is_closed("{ label: \"{x}\", hotkey: c }"));
        assert!(!flow_is_closed("{ label: Coffee,"));
        assert!(flow_is_closed("[1, 2]"));
    }
}
