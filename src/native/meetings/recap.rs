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

pub(crate) fn render_recap(notes: &MeetingNotes, meta: &RecapMeta) -> String {
    let mut inner = String::new();
    let mut meta_line =
        format!("_{} · {} · {}", meta.date, meta.span, meta.event_link);
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

    if let Some(title) = notes.own_title.as_deref() {
        inner.push_str("\n\n### ");
        inner.push_str(&escape_recap_text(title));
    }
    if !notes.tldr.trim().is_empty() {
        inner.push_str("\n\n");
        render_text_block(&notes.tldr, &mut inner);
    }

    for content in &notes.content_order {
        match *content {
            ContentRef::NextSteps => {
                let heading =
                    notes.next_steps_heading.as_deref().unwrap_or("Next steps");
                inner.push_str("\n\n");
                inner.push_str(
                    &"#".repeat(notes.next_steps_level.clamp(2, 3) as usize),
                );
                inner.push(' ');
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
                    inner.push_str(
                        &"#".repeat(section.level.clamp(2, 3) as usize),
                    );
                    inner.push(' ');
                    inner.push_str(&escape_recap_text(heading));
                }
                for item in &section.items {
                    inner.push_str("\n- ");
                    render_multiline_item(item, &mut inner);
                }
            }
        }
    }

    let digest = crate::native::intentions::digest(&inner);
    format!("{BEGIN_PREFIX}{digest} -->\n{inner}\n{END_MARKER}")
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

fn render_text_block(text: &str, output: &mut String) {
    let lines: Vec<&str> = text.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        output.push_str(&escape_recap_text(line));
    }
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
