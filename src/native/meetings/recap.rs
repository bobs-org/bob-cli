use std::{error::Error as StdError, fmt, ops::Range};

use super::{ContentRef, MeetingNotes, RecapMeta};

const BEGIN_PREFIX: &str = "<!-- bob:meeting-recap:begin v1 ";
const END_MARKER: &str = "<!-- bob:meeting-recap:end -->";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecapState {
    Absent,
    Owned,
    Edited,
}

#[derive(Debug)]
pub(crate) enum ReplaceRecapError {
    Edited,
    InvalidReplacement,
}

impl fmt::Display for ReplaceRecapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Edited => {
                write!(f, "meeting recap was edited and will not be replaced")
            }
            Self::InvalidReplacement => {
                write!(f, "replacement is not a valid Bob-owned meeting recap")
            }
        }
    }
}

impl StdError for ReplaceRecapError {}

/// Warning when overwrite keeps a user-edited recap byte-for-byte.
pub(crate) const EDITED_RECAP_WARNING: &str =
    "meeting recap was edited and was not replaced";

/// Result of applying a replacement recap against the live note's ownership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecapOverwrite {
    pub body: String,
    pub warning: Option<String>,
}

pub(crate) fn render_recap(notes: &MeetingNotes, meta: &RecapMeta) -> String {
    let mut inner = String::new();
    inner.push_str("## Notes\n\n");
    let day = super::profile::format_meeting_day(&meta.date);
    let event_link =
        render_event_link(&meta.event_link, meta.event_title.as_deref());
    let mut meta_line = format!("_{day} · {} · {event_link}", meta.span);
    if let Some(producer) = meta
        .producer
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        meta_line.push_str(" · ");
        meta_line.push_str(&escape_recap_text(producer));
    }
    meta_line.push('_');
    inner.push_str(&meta_line);

    if let Some(title) = notes
        .own_title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        inner.push_str("\n\n**");
        inner.push_str(&escape_recap_text(title));
        inner.push_str("**");
    }
    if !notes.tldr.trim().is_empty() {
        inner.push_str("\n\n> [!summary] TL;DR");
        for line in notes.tldr.lines() {
            if line.trim().is_empty() {
                inner.push_str("\n>");
            } else {
                inner.push_str("\n> ");
                inner.push_str(&escape_recap_text(line));
            }
        }
    }

    for content in &notes.content_order {
        match *content {
            ContentRef::NextSteps => {
                let heading =
                    notes.next_steps_heading.as_deref().unwrap_or("Next steps");
                inner.push_str("\n\n### ");
                inner.push_str(&escape_recap_text(heading));
                for step in &notes.next_steps {
                    inner.push_str("\n- ");
                    if let Some(owner) = step.owner.as_deref() {
                        inner.push_str("**");
                        inner.push_str(&escape_recap_text(owner));
                        inner.push_str("** — ");
                    }
                    inner.push_str(&escape_recap_text(&step.action));
                }
            }
            ContentRef::Section(index) => {
                let Some(section) = notes.sections.get(index) else {
                    continue;
                };
                if let Some(heading) = section.heading.as_deref() {
                    inner.push_str("\n\n");
                    if section.level >= 3 {
                        inner.push_str("#### ");
                    } else {
                        inner.push_str("### ");
                    }
                    inner.push_str(&escape_recap_text(heading));
                    for item in &section.items {
                        inner.push_str("\n- ");
                        render_multiline_item(item, &mut inner);
                    }
                } else if !section.items.is_empty() {
                    for (position, item) in section.items.iter().enumerate() {
                        if position == 0 {
                            inner.push_str("\n\n- ");
                        } else {
                            inner.push_str("\n- ");
                        }
                        render_multiline_item(item, &mut inner);
                    }
                }
            }
        }
    }

    let digest = crate::native::intentions::digest(&inner);
    format!("{BEGIN_PREFIX}{digest} -->\n{inner}\n{END_MARKER}")
}

fn render_event_link(event_link: &str, event_title: Option<&str>) -> String {
    let Some(title) =
        event_title.map(str::trim).filter(|title| !title.is_empty())
    else {
        return event_link.to_string();
    };
    let mut alias = crate::native::ref_tasks::sanitize_title_alias(title);
    alias = alias.replace("<!--", "");
    alias = alias
        .chars()
        .filter(|ch| !matches!(ch, '|' | '[' | ']' | '#' | '^' | '\n' | '\r'))
        .collect::<String>()
        .trim()
        .to_string();
    if alias.is_empty() || alias.contains("<!--") {
        return event_link.to_string();
    }
    let Some(bare) = event_link.strip_suffix("]]") else {
        return event_link.to_string();
    };
    if bare.contains('|') {
        return event_link.to_string();
    }
    format!("{}|{}]]", bare, escape_recap_text(&alias))
}

pub(crate) fn recap_state(note_body: &str) -> RecapState {
    match recap_region(note_body) {
        None if note_body.contains(BEGIN_PREFIX)
            || note_body.contains(END_MARKER) =>
        {
            RecapState::Edited
        }
        None => RecapState::Absent,
        Some((range, digest, inner)) => {
            if crate::native::intentions::digest(inner) == digest
                && !note_body[range.end..].contains(BEGIN_PREFIX)
                && !note_body[..range.start].contains(BEGIN_PREFIX)
            {
                RecapState::Owned
            } else {
                RecapState::Edited
            }
        }
    }
}

pub(crate) fn replace_recap(
    note_body: &str,
    new_recap: &str,
) -> Result<String, ReplaceRecapError> {
    if recap_state(new_recap) != RecapState::Owned {
        return Err(ReplaceRecapError::InvalidReplacement);
    }
    match recap_state(note_body) {
        RecapState::Edited => Err(ReplaceRecapError::Edited),
        RecapState::Owned => {
            let (range, _, _) = recap_region(note_body)
                .ok_or(ReplaceRecapError::InvalidReplacement)?;
            let mut result = String::with_capacity(
                note_body.len() - (range.end - range.start) + new_recap.len(),
            );
            result.push_str(&note_body[..range.start]);
            result.push_str(new_recap);
            result.push_str(&note_body[range.end..]);
            Ok(result)
        }
        RecapState::Absent => Ok(insert_recap(note_body, new_recap)),
    }
}

/// Apply a newly rendered owned recap to a fresh overwrite skeleton using
/// the live note body's recap ownership at the transaction preimage.
///
/// Owned and absent regions are replaced. An edited or malformed region
/// stays on the live note; missing or malformed markers never authorize
/// deleting user text.
pub(crate) fn apply_overwrite_recap(
    live_body: &str,
    fresh_body: &str,
    replacement: &str,
) -> Result<RecapOverwrite, ReplaceRecapError> {
    match recap_state(live_body) {
        RecapState::Owned | RecapState::Absent => Ok(RecapOverwrite {
            body: replace_recap(fresh_body, replacement)?,
            warning: None,
        }),
        RecapState::Edited => {
            let body = if let Some(old) = extract_recap(live_body) {
                place_recap(fresh_body, old)
            } else {
                place_unparsed_recap(live_body, fresh_body)
            };
            Ok(RecapOverwrite {
                body,
                warning: Some(EDITED_RECAP_WARNING.to_string()),
            })
        }
    }
}

/// Byte span of a recap region that overwrite may preserve or strip, even
/// when the digest no longer matches. Malformed begin/end pairs still
/// identify a span so user text is not deleted.
pub(crate) fn recap_preservation_span(note_body: &str) -> Option<Range<usize>> {
    if let Some((range, _, _)) = recap_region(note_body) {
        return Some(range);
    }
    let start = match note_body.find(BEGIN_PREFIX) {
        Some(start) => start,
        None => {
            let end = note_body.find(END_MARKER)?;
            let start = note_body[..end]
                .rfind("\n\n")
                .map(|index| index + 2)
                .unwrap_or(0);
            return Some(start..end + END_MARKER.len());
        }
    };
    if let Some(rel) = note_body[start..].find(END_MARKER) {
        return Some(start..start + rel + END_MARKER.len());
    }
    let end = note_body[start..]
        .find("## Highlights")
        .map(|rel| start + rel)
        .unwrap_or(note_body.len());
    Some(start..end)
}

pub(crate) fn extract_recap(note_body: &str) -> Option<&str> {
    recap_preservation_span(note_body).map(|range| &note_body[range])
}

/// Remove a recap span and collapse the blank lines around the cut so a
/// managed skeleton comparison does not treat an owned recap as authored
/// prose.
pub(crate) fn strip_recap(note_body: &str) -> String {
    let Some(range) = recap_preservation_span(note_body) else {
        return note_body.to_string();
    };
    collapse_cut(note_body, range)
}

fn recap_region(note_body: &str) -> Option<(Range<usize>, String, &str)> {
    let start = note_body.find(BEGIN_PREFIX)?;
    let digest_start = start + BEGIN_PREFIX.len();
    let digest_end = note_body[digest_start..].find(" -->")? + digest_start;
    let digest = note_body[digest_start..digest_end].to_string();
    if digest.len() != 23
        || !digest.is_ascii()
        || !digest.starts_with("sha256:")
    {
        return None;
    }
    if !digest[7..]
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let marker_end = digest_end + 4;
    let body_start = if note_body[marker_end..].starts_with('\n') {
        marker_end + 1
    } else {
        return None;
    };
    let end_marker_start =
        note_body[body_start..].find(END_MARKER)? + body_start;
    let inner_end = end_marker_start.checked_sub(1)?;
    if note_body.as_bytes().get(inner_end) != Some(&b'\n') {
        return None;
    }
    let inner = &note_body[body_start..inner_end];
    let range = start..end_marker_start + END_MARKER.len();
    Some((range, digest, inner))
}

fn place_recap(note_body: &str, recap: &str) -> String {
    if let Some(range) = recap_preservation_span(note_body) {
        let mut result = String::with_capacity(
            note_body.len() - (range.end - range.start) + recap.len(),
        );
        result.push_str(&note_body[..range.start]);
        result.push_str(recap);
        result.push_str(&note_body[range.end..]);
        result
    } else {
        insert_recap(note_body, recap)
    }
}

fn place_unparsed_recap(live_body: &str, fresh_body: &str) -> String {
    let Some(old_span) = embed_to_highlights(live_body) else {
        return insert_recap(fresh_body, live_body.trim());
    };
    let kept = live_body[old_span.clone()].trim();
    if kept.is_empty() {
        return fresh_body.to_string();
    }
    insert_recap(fresh_body, kept)
}

fn embed_to_highlights(body: &str) -> Option<Range<usize>> {
    let embed = body.find("![[")?;
    let embed_end = body[embed..].find('\n').map(|rel| embed + rel)?;
    let start = if body[embed_end..].starts_with('\n') {
        embed_end + 1
    } else {
        embed_end
    };
    let end = body[start..]
        .find("## Highlights")
        .map(|rel| start + rel)
        .unwrap_or(body.len());
    Some(start..end)
}

fn collapse_cut(text: &str, range: Range<usize>) -> String {
    let mut before = text[..range.start].to_string();
    let mut after = text[range.end..].to_string();
    while before.ends_with('\n') {
        before.pop();
    }
    while after.starts_with('\n') {
        after.remove(0);
    }
    if after.is_empty() {
        if before.is_empty() {
            String::new()
        } else {
            before.push('\n');
            before
        }
    } else if before.is_empty() {
        after
    } else {
        format!("{before}\n\n{after}")
    }
}

fn insert_recap(note_body: &str, recap: &str) -> String {
    let insertion = note_body
        .find("## Highlights")
        .map(|position| {
            note_body[..position]
                .rfind('\n')
                .map_or(0, |index| index + 1)
        })
        .unwrap_or(note_body.len());
    let before = &note_body[..insertion];
    let after = &note_body[insertion..];
    let mut result = String::with_capacity(note_body.len() + recap.len() + 4);
    result.push_str(before);
    if !before.is_empty() && !before.ends_with("\n\n") {
        if before.ends_with('\n') {
            result.push('\n');
        } else {
            result.push_str("\n\n");
        }
    }
    result.push_str(recap);
    if !after.is_empty() {
        result.push_str("\n\n");
    } else {
        result.push('\n');
    }
    result.push_str(after);
    result
}

fn render_multiline_item(item: &str, output: &mut String) {
    let mut lines = item.lines();
    if let Some(first) = lines.next() {
        output.push_str(&escape_recap_text(first));
    }
    for line in lines {
        output.push_str("\n  ");
        output.push_str(&escape_recap_text(line));
    }
}

/// Encode Markdown and Obsidian control characters as displayed text. This keeps
/// notes-derived content inert, including HTML comments that could close the region.
fn escape_recap_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        escaped.push_str(match ch {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '[' => "&#91;",
            ']' => "&#93;",
            '#' => "&#35;",
            ':' => "&#58;",
            '^' => "&#94;",
            '%' => "&#37;",
            '$' => "&#36;",
            '*' => "&#42;",
            '_' => "&#95;",
            '`' => "&#96;",
            '\\' => "&#92;",
            '~' => "&#126;",
            '!' => "&#33;",
            '-' => "&#45;",
            '+' => "&#43;",
            '|' => "&#124;",
            '=' => "&#61;",
            '{' => "&#123;",
            '}' => "&#125;",
            _ => {
                escaped.push(ch);
                continue;
            }
        });
    }
    escaped
}
