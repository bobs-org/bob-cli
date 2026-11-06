//! Strict parser for Bob-owned mail-note regions.

use std::ops::Range;

use serde_yaml::{Mapping, Value};

use super::{layout::NoteMetadata, NoteError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedCard {
    pub(crate) id: String,
    pub(crate) header: String,
    pub(crate) header_range: Range<usize>,
    pub(crate) card_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedNote {
    pub(crate) account: String,
    pub(crate) thread_id: String,
    pub(crate) frontmatter: Value,
    pub(crate) body_start: usize,
    pub(crate) begin_inner_start: usize,
    pub(crate) end_inner_start: usize,
    pub(crate) heading: String,
    pub(crate) heading_range: Range<usize>,
    pub(crate) summary: String,
    pub(crate) summary_range: Range<usize>,
    pub(crate) cards: Vec<ParsedCard>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    content: String,
    start: usize,
    content_end: usize,
    end: usize,
}

pub(crate) fn parse(contents: &str) -> Result<ParsedNote, NoteError> {
    let lines = lines(contents);
    let Some((frontmatter_end, _frontmatter_range, frontmatter)) =
        parse_frontmatter_lines(&lines)
    else {
        return Err(damaged("frontmatter is missing or invalid"));
    };
    let account = scalar(&frontmatter, "gmail_account")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| damaged("mail identity is missing"))?;
    let thread_id = scalar(&frontmatter, "gmail_thread_id")
        .filter(|value| is_hex(value))
        .ok_or_else(|| damaged("mail identity is invalid"))?
        .to_ascii_lowercase();
    if scalar(&frontmatter, "content").as_deref() != Some("untrusted-email") {
        return Err(damaged("mail content marker is missing"));
    }

    let begin_lines: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            line.content.trim() == "<!-- bob:gmail:begin v1 -->"
        })
        .map(|(index, _)| index)
        .collect();
    let end_lines: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.content.trim() == "<!-- bob:gmail:end -->")
        .map(|(index, _)| index)
        .collect();
    if begin_lines.len() != 1 || end_lines.len() != 1 {
        return Err(damaged("managed markers are missing or duplicated"));
    }
    let begin_index = begin_lines[0];
    let end_index = end_lines[0];
    if begin_index <= frontmatter_end || end_index <= begin_index {
        return Err(damaged("managed markers are reordered"));
    }

    let mut marker_lines = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.content.contains("<!-- bob:gmail:msg") {
            if index <= begin_index || index >= end_index {
                return Err(damaged(
                    "message marker is outside the managed block",
                ));
            }
            marker_lines.push(index);
        }
    }
    if marker_lines.is_empty() {
        return Err(damaged("managed block has no message cards"));
    }

    let mut cards = Vec::with_capacity(marker_lines.len());
    let mut ids = std::collections::BTreeSet::new();
    let mut anchors = std::collections::BTreeMap::<String, usize>::new();
    for (position, marker_index) in marker_lines.iter().copied().enumerate() {
        let line = &lines[marker_index];
        let Some(id) = parse_message_marker(&line.content) else {
            return Err(damaged("message marker is malformed"));
        };
        if !ids.insert(id.clone()) {
            return Err(damaged("message markers are duplicated"));
        }
        let next_marker =
            marker_lines.get(position + 1).copied().unwrap_or(end_index);
        let header_index = ((marker_index + 1)..next_marker)
            .find(|index| !lines[*index].content.trim().is_empty())
            .ok_or_else(|| damaged("message card has no header"))?;
        let header = lines[header_index].content.clone();
        let header_for_check = header.trim_start();
        if !(header_for_check.starts_with("> [!mail+] ")
            || header_for_check.starts_with("> [!mail-] "))
        {
            return Err(damaged("message card header is malformed"));
        }
        let anchor = format!("^m-{id}");
        let card_anchors: Vec<_> = ((header_index + 1)..next_marker)
            .filter(|index| lines[*index].content.trim() == anchor)
            .collect();
        if card_anchors.len() != 1 {
            return Err(damaged(
                "message card anchors are missing or duplicated",
            ));
        }
        let anchor_index = card_anchors[0];
        cards.push(ParsedCard {
            id,
            header,
            header_range: lines[header_index].start
                ..lines[header_index].content_end,
            card_range: line.start..lines[anchor_index].end,
        });
    }
    for (index, line) in lines.iter().enumerate() {
        if line.content.trim_start().starts_with("^m-")
            && index > begin_index
            && index < end_index
        {
            let Some(id) = line.content.trim().strip_prefix("^m-") else {
                continue;
            };
            let id = id.to_ascii_lowercase();
            if !ids.contains(&id) {
                return Err(damaged("orphan message anchor is present"));
            }
            *anchors.entry(id).or_default() += 1;
        }
    }
    if ids
        .iter()
        .any(|id| anchors.get(id).copied().unwrap_or_default() != 1)
    {
        return Err(damaged("message anchors are duplicated"));
    }

    let body_lines = &lines[(frontmatter_end + 1)..];
    let heading_line = body_lines
        .iter()
        .find(|line| line.content.starts_with("# "))
        .ok_or_else(|| damaged("mail title is missing"))?;
    let summary_line = body_lines
        .iter()
        .skip_while(|line| line.start < heading_line.end)
        .take_while(|line| !line.content.starts_with("## Notes"))
        .find(|line| line.content.starts_with("· "))
        .ok_or_else(|| damaged("mail summary is missing"))?;
    let begin_line = &lines[begin_index];
    let end_line = &lines[end_index];
    Ok(ParsedNote {
        account,
        thread_id,
        frontmatter,
        body_start: lines[frontmatter_end].end,
        begin_inner_start: begin_line.end,
        end_inner_start: end_line.start,
        heading: heading_line.content.clone(),
        heading_range: heading_line.start..heading_line.end,
        summary: summary_line.content.clone(),
        summary_range: summary_line.start..summary_line.end,
        cards,
    })
}

pub(crate) fn metadata_matches(
    frontmatter: &Value,
    metadata: &NoteMetadata,
) -> bool {
    scalar(frontmatter, "gmail_account").as_deref() == Some(&metadata.account)
        && scalar(frontmatter, "gmail_thread_id").as_deref()
            == Some(&metadata.gmail_thread_id)
        && scalar(frontmatter, "gmail_url").as_deref()
            == Some(&metadata.gmail_url)
        && scalar(frontmatter, "title").as_deref() == Some(&metadata.title)
        && scalar(frontmatter, "who").as_deref() == Some(&metadata.who)
        && scalar(frontmatter, "started").as_deref() == Some(&metadata.started)
        && scalar(frontmatter, "updated").as_deref() == Some(&metadata.updated)
        && scalar(frontmatter, "messages").as_deref()
            == Some(&metadata.messages.to_string())
        && scalar(frontmatter, "content").as_deref() == Some(&metadata.content)
}

pub(crate) fn set_owned_metadata(
    frontmatter: &mut Value,
    metadata: &NoteMetadata,
) -> Result<(), NoteError> {
    let Value::Mapping(mapping) = frontmatter else {
        return Err(damaged("frontmatter is not a mapping"));
    };
    set(mapping, "type", Value::String("[[mail]]".to_string()));
    set(
        mapping,
        "cssclasses",
        Value::Sequence(vec![Value::String("bob-mail".to_string())]),
    );
    set(mapping, "title", Value::String(metadata.title.clone()));
    set(mapping, "who", Value::String(metadata.who.clone()));
    set(mapping, "started", Value::String(metadata.started.clone()));
    set(mapping, "updated", Value::String(metadata.updated.clone()));
    set(mapping, "messages", Value::Number(metadata.messages.into()));
    set(
        mapping,
        "gmail_account",
        Value::String(metadata.account.clone()),
    );
    set(
        mapping,
        "gmail_thread_id",
        Value::String(metadata.gmail_thread_id.clone()),
    );
    set(
        mapping,
        "gmail_url",
        Value::String(metadata.gmail_url.clone()),
    );
    set(mapping, "content", Value::String(metadata.content.clone()));
    Ok(())
}

pub(crate) fn parse_frontmatter_only(contents: &str) -> Option<Value> {
    let lines = lines(contents);
    parse_frontmatter_lines(&lines).map(|(_, _, value)| value)
}

fn parse_frontmatter_lines(
    lines: &[Line],
) -> Option<(usize, Range<usize>, Value)> {
    if lines.first()?.content.trim() != "---" {
        return None;
    }
    let close = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.content.trim() == "---")
        .map(|(index, _)| index)?;
    let start = lines[0].end;
    let end = lines[close].start;
    let yaml = lines
        .get(1..close)?
        .iter()
        .map(|line| line.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let value = serde_yaml::from_str::<Value>(&yaml).ok()?;
    Some((close, start..end, value))
}

fn lines(contents: &str) -> Vec<Line> {
    let mut result = Vec::new();
    let mut start = 0;
    for segment in contents.split_inclusive('\n') {
        let end = start + segment.len();
        let content_end = if segment.ends_with('\n') {
            end - 1
        } else {
            end
        };
        let content_end = if content_end > start
            && contents.as_bytes()[content_end - 1] == b'\r'
        {
            content_end - 1
        } else {
            content_end
        };
        result.push(Line {
            content: contents[start..content_end].to_string(),
            start,
            content_end,
            end,
        });
        start = end;
    }
    if start < contents.len() || contents.is_empty() {
        result.push(Line {
            content: contents[start..].to_string(),
            start,
            content_end: contents.len(),
            end: contents.len(),
        });
    }
    result
}

fn parse_message_marker(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let inner = trimmed
        .strip_prefix("<!-- bob:gmail:msg ")?
        .strip_suffix(" -->")?;
    let mut parts = inner.split_whitespace();
    let id = parts.next()?;
    let rfc = parts.next()?;
    if parts.next().is_some()
        || !is_hex(id)
        || rfc.is_empty()
        || rfc.contains("--")
        || rfc.contains('>')
    {
        return None;
    }
    Some(id.to_ascii_lowercase())
}

fn scalar(frontmatter: &Value, key: &str) -> Option<String> {
    let value = frontmatter
        .as_mapping()?
        .get(Value::String(key.to_string()))?;
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn set(mapping: &mut Mapping, key: &str, value: Value) {
    mapping.insert(Value::String(key.to_string()), value);
}

fn is_hex(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn damaged(reason: &'static str) -> NoteError {
    NoteError::Damaged { reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\nparent: \"[[mail]]\"\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: me@example.com\ngmail_thread_id: aabbccddeeff0011\n---\n\n# Subject\n\n[Open in Gmail ↗](https://mail.google.com/)\n· Dana ↔ you · 1 messages · Sep 19 → Sep 19\n\n## Notes\n\n## Conversation\n\n<!-- bob:gmail:begin v1 -->\n<!-- bob:gmail:msg aabbccddeeff0011 - -->\n> [!mail+] **Dana** · Fri, Sep 19 · 9:46 AM to you\n> body\n\n^m-aabbccddeeff0011\n\n<!-- bob:gmail:end -->\n";

    #[test]
    fn parses_card_markers_and_byte_ranges() {
        let parsed = parse(NOTE).unwrap();
        assert_eq!(parsed.account, "me@example.com");
        assert_eq!(parsed.thread_id, "aabbccddeeff0011");
        assert_eq!(parsed.cards.len(), 1);
        assert_eq!(parsed.cards[0].id, "aabbccddeeff0011");
        assert_eq!(
            &NOTE[parsed.cards[0].header_range.clone()],
            parsed.cards[0].header
        );
        assert_eq!(
            parsed.summary,
            "· Dana ↔ you · 1 messages · Sep 19 → Sep 19"
        );
    }

    #[test]
    fn rejects_missing_duplicate_reordered_or_orphaned_markers() {
        assert!(parse(&NOTE.replace("<!-- bob:gmail:end -->", "")).is_err());
        let duplicate = NOTE.replace(
            "<!-- bob:gmail:end -->",
            "<!-- bob:gmail:end -->\n<!-- bob:gmail:end -->",
        );
        assert!(parse(&duplicate).is_err());
        let reordered = NOTE.replace(
            "<!-- bob:gmail:begin v1 -->\n<!-- bob:gmail:msg",
            "<!-- bob:gmail:msg",
        );
        assert!(parse(&reordered).is_err());
        let orphan = NOTE.replace("^m-aabbccddeeff0011", "^m-ffffffffffffffff");
        assert!(parse(&orphan).is_err());
    }

    #[test]
    fn whitespace_only_linter_changes_still_parse() {
        let linted =
            NOTE.replace("\n\n<!-- bob:gmail:msg", "\n   \n<!-- bob:gmail:msg");
        assert!(parse(&linted).is_ok());
    }
}
