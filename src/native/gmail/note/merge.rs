//! Append-only mail-note refresh.

use std::collections::BTreeMap;
use std::ops::Range;

use super::{layout::RenderedNote, parse, NoteError};

/// Merge a fresh rendering into an existing note. `None` means the refresh is
/// a true byte-identical no-op. Rerender refreshes current cards while keeping
/// any cards for messages no longer returned by Gmail.
pub(crate) fn merge(
    existing: &str,
    fresh: &RenderedNote,
    rerender: bool,
) -> Result<Option<String>, NoteError> {
    let parsed = parse::parse(existing)?;
    if parsed.account != fresh.metadata.account
        || parsed.thread_id != fresh.metadata.gmail_thread_id
    {
        return Err(NoteError::Damaged {
            reason: "mail note identity does not match the fetched thread",
        });
    }
    validate_order(&parsed, fresh)?;

    let fresh_by_id: BTreeMap<_, _> = fresh
        .cards
        .iter()
        .map(|card| (card.id.as_str(), card))
        .collect();
    let old_ids: std::collections::BTreeSet<_> =
        parsed.cards.iter().map(|card| card.id.as_str()).collect();
    let appended: Vec<_> = fresh
        .cards
        .iter()
        .filter(|card| !old_ids.contains(card.id.as_str()))
        .collect();
    let heading = format!("# {}", fresh.metadata.title);
    let summary = format!("· {}", fresh.summary);
    let metadata_changed =
        !parse::metadata_matches(&parsed.frontmatter, &fresh.metadata);
    let top_changed = parsed.heading != heading || parsed.summary != summary;
    if !rerender && appended.is_empty() && !metadata_changed && !top_changed {
        return Ok(None);
    }

    let mut inner =
        existing[parsed.begin_inner_start..parsed.end_inner_start].to_string();
    if rerender {
        inner = rerender_inner(existing, &parsed, &fresh_by_id, &appended);
    } else if !appended.is_empty() {
        inner =
            flip_previous_open_header(existing, &parsed, &fresh_by_id, &inner);
        let cards = appended
            .iter()
            .map(|card| card.markdown.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let core = inner.trim_end_matches(char::is_whitespace);
        inner = if core.is_empty() {
            format!("{cards}\n\n")
        } else {
            format!("{core}\n\n{cards}\n\n")
        };
    }

    let mut frontmatter = parsed.frontmatter.clone();
    parse::set_owned_metadata(&mut frontmatter, &fresh.metadata)?;
    let serialized = serde_yaml::to_string(&frontmatter).map_err(|_| {
        NoteError::Damaged {
            reason: "frontmatter could not be serialized",
        }
    })?;
    let serialized = serialized.strip_prefix("---\n").unwrap_or(&serialized);
    let mut new_frontmatter = String::from("---\n");
    new_frontmatter.push_str(serialized);
    if !serialized.ends_with('\n') {
        new_frontmatter.push('\n');
    }
    new_frontmatter.push_str("---\n");

    let body = &existing[parsed.body_start..];
    let mut replacements: Vec<(Range<usize>, String)> = vec![
        (
            parsed.heading_range.start - parsed.body_start
                ..parsed.heading_range.end - parsed.body_start,
            heading,
        ),
        (
            parsed.summary_range.start - parsed.body_start
                ..parsed.summary_range.end - parsed.body_start,
            summary,
        ),
        (
            parsed.begin_inner_start - parsed.body_start
                ..parsed.end_inner_start - parsed.body_start,
            inner,
        ),
    ];
    replacements.sort_by(|left, right| right.0.start.cmp(&left.0.start));
    let mut new_body = body.to_string();
    for (range, replacement) in replacements {
        new_body.replace_range(range, &replacement);
    }
    Ok(Some(format!("{new_frontmatter}{new_body}")))
}

fn validate_order(
    parsed: &parse::ParsedNote,
    fresh: &RenderedNote,
) -> Result<(), NoteError> {
    let positions: BTreeMap<_, _> = fresh
        .cards
        .iter()
        .enumerate()
        .map(|(index, card)| (card.id.as_str(), index))
        .collect();
    let mut previous = None;
    for card in &parsed.cards {
        if let Some(position) = positions.get(card.id.as_str()).copied() {
            if previous.is_some_and(|previous| position <= previous) {
                return Err(NoteError::Damaged {
                    reason: "message markers no longer match Gmail order",
                });
            }
            previous = Some(position);
        }
    }
    Ok(())
}

fn flip_previous_open_header(
    existing: &str,
    parsed: &parse::ParsedNote,
    fresh_by_id: &BTreeMap<&str, &super::layout::RenderedCard>,
    inner: &str,
) -> String {
    let Some(old_card) = parsed
        .cards
        .iter()
        .rev()
        .find(|card| card.header.contains("[!mail+]"))
    else {
        return inner.to_string();
    };
    let Some(fresh_card) = fresh_by_id.get(old_card.id.as_str()) else {
        return inner.to_string();
    };
    let expected_previous_open =
        fresh_card.header.replace("[!mail-]", "[!mail+]");
    if existing[old_card.header_range.clone()] != expected_previous_open {
        return inner.to_string();
    }
    let range = old_card.header_range.start - parsed.begin_inner_start
        ..old_card.header_range.end - parsed.begin_inner_start;
    let Some(start) = inner.get(range.clone()) else {
        return inner.to_string();
    };
    let replacement = start.replace("[!mail+]", "[!mail-]");
    let mut output = inner.to_string();
    output.replace_range(range, &replacement);
    output
}

fn rerender_inner(
    existing: &str,
    parsed: &parse::ParsedNote,
    fresh_by_id: &BTreeMap<&str, &super::layout::RenderedCard>,
    appended: &[&super::layout::RenderedCard],
) -> String {
    let mut inner =
        existing[parsed.begin_inner_start..parsed.end_inner_start].to_string();
    let mut replacements = Vec::new();
    for old_card in &parsed.cards {
        if let Some(fresh_card) = fresh_by_id.get(old_card.id.as_str()) {
            replacements.push((
                old_card.card_range.start - parsed.begin_inner_start
                    ..old_card.card_range.end - parsed.begin_inner_start,
                format!("{}\n", fresh_card.markdown),
            ));
        }
    }
    replacements.sort_by(|left: &(Range<usize>, String), right| {
        right.0.start.cmp(&left.0.start)
    });
    for (range, replacement) in replacements {
        inner.replace_range(range, &replacement);
    }
    if !appended.is_empty() {
        let cards = appended
            .iter()
            .map(|card| card.markdown.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let core = inner.trim_end_matches(char::is_whitespace);
        inner = if core.is_empty() {
            format!("{cards}\n\n")
        } else {
            format!("{core}\n\n{cards}\n\n")
        };
    }
    inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::config::GmailBodyMode;
    use crate::native::gmail::{
        body::{render_message, FetchedBodies, RenderedBody},
        model::{GmailMessage, GmailThread, Header, MimeBody, MimePart},
        note::layout::{render, LayoutInput},
    };
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use chrono::{TimeZone, Utc};
    use std::collections::BTreeMap;

    fn message(id: &str, date: &str, body: &str) -> GmailMessage {
        let headers = vec![
            Header {
                name: "From".to_string(),
                value: "Dana <dana@example.com>".to_string(),
            },
            Header {
                name: "To".to_string(),
                value: "me@example.com".to_string(),
            },
            Header {
                name: "Subject".to_string(),
                value: "Lease".to_string(),
            },
        ];
        GmailMessage {
            id: id.to_string(),
            thread_id: "aabbccddeeff0011".to_string(),
            label_ids: Vec::new(),
            internal_date: Some(date.to_string()),
            snippet: Some(body.to_string()),
            headers,
            payload: Some(MimePart {
                part_id: Some("0".to_string()),
                mime_type: "text/plain".to_string(),
                filename: None,
                headers: Vec::new(),
                body: Some(MimeBody {
                    size: Some(body.len() as u64),
                    data: Some(URL_SAFE_NO_PAD.encode(body.as_bytes())),
                    attachment_id: None,
                }),
                parts: Vec::new(),
            }),
        }
    }

    fn note(thread: &GmailThread, body_mode: GmailBodyMode) -> RenderedNote {
        let fetched = FetchedBodies::default();
        let rendered: BTreeMap<String, RenderedBody> = thread
            .messages
            .iter()
            .map(|message| {
                (
                    message.id.clone(),
                    render_message(message, &fetched).unwrap(),
                )
            })
            .collect();
        render(&LayoutInput {
            account: "me@example.com",
            body_mode,
            current_year: 2026,
            rendered: &rendered,
            saved_at: Utc.timestamp_millis_opt(1799848500000).unwrap(),
            thread,
            timezone: chrono_tz::America::New_York,
        })
        .unwrap()
    }

    #[test]
    fn unchanged_refresh_is_byte_identical_and_noop() {
        let thread = GmailThread {
            id: "aabbccddeeff0011".to_string(),
            history_id: None,
            messages: vec![message("aabbccddeeff0011", "1799847992000", "One")],
        };
        let fresh = note(&thread, GmailBodyMode::Full);
        assert_eq!(merge(&fresh.content, &fresh, false).unwrap(), None);
    }

    #[test]
    fn appends_messages_and_flips_only_an_untouched_previous_header() {
        let first = message("aabbccddeeff0011", "1799847992000", "One");
        let mut thread = GmailThread {
            id: "aabbccddeeff0011".to_string(),
            history_id: None,
            messages: vec![first.clone()],
        };
        let original = note(&thread, GmailBodyMode::Full);
        let mut edited = original
            .content
            .replace("## Notes\n\n", "## Notes\n\nBryan's note.\n\n");
        thread.messages.push(message(
            "bbbbccddeeff0022",
            "1799848200000",
            "Two",
        ));
        let fresh = note(&thread, GmailBodyMode::Full);
        let merged = merge(&edited, &fresh, false).unwrap().unwrap();
        assert!(merged.contains("Bryan's note."));
        assert!(merged.contains("> [!mail-] **Dana**"));
        assert!(merged.contains("<!-- bob:gmail:msg bbbbccddeeff0022 - -->"));

        edited = original
            .content
            .replace("> [!mail+] **Dana**", "> [!mail+] **A changed name**");
        let merged = merge(&edited, &fresh, false).unwrap().unwrap();
        assert!(merged.contains("> [!mail+] **A changed name**"));
    }

    #[test]
    fn marker_order_changes_refuse_and_user_parent_survives() {
        let thread = GmailThread {
            id: "aabbccddeeff0011".to_string(),
            history_id: None,
            messages: vec![message("aabbccddeeff0011", "1799847992000", "One")],
        };
        let original = note(&thread, GmailBodyMode::Full);
        let edited = with_parent(&original.content, "[[custom]]");
        let mut changed = thread.clone();
        changed.messages.push(message(
            "bbbbccddeeff0022",
            "1799848200000",
            "Two",
        ));
        let fresh = note(&changed, GmailBodyMode::Full);
        let merged = merge(&edited, &fresh, false).unwrap().unwrap();
        let frontmatter = parse::parse_frontmatter_only(&merged).unwrap();
        assert_eq!(
            frontmatter
                .as_mapping()
                .unwrap()
                .get(serde_yaml::Value::String("parent".to_string()))
                .and_then(serde_yaml::Value::as_str),
            Some("[[custom]]")
        );
        assert!(merged.contains("<!-- bob:gmail:msg bbbbccddeeff0022 - -->"));

        let mut ordered = changed.clone();
        ordered.messages.push(message(
            "ccccccddeeff0033",
            "1799848300000",
            "Three",
        ));
        let original_two = note(&ordered, GmailBodyMode::Full);
        ordered.messages.reverse();
        let reordered = note(&ordered, GmailBodyMode::Full);
        assert!(merge(&original_two.content, &reordered, false).is_err());
    }

    fn with_parent(note: &str, parent: &str) -> String {
        let mut frontmatter = parse::parse_frontmatter_only(note).unwrap();
        frontmatter.as_mapping_mut().unwrap().insert(
            serde_yaml::Value::String("parent".to_string()),
            serde_yaml::Value::String(parent.to_string()),
        );
        let yaml = serde_yaml::to_string(&frontmatter).unwrap();
        let closing = note.find("\n---\n").unwrap();
        format!("---\n{yaml}---\n{}", &note[closing + 5..])
    }
}
