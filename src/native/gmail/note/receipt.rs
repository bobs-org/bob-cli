//! Narrow receipt projection for saved mail notes.
//!
//! The folded archive journal is authoritative; this helper only mirrors the
//! newest still-owned receipt into the `gmail_archived` frontmatter field and
//! one Bob-owned banner span above the first card. It changes nothing else:
//! parent, user text, Reply, notes, cards, anchors, attachments, and every
//! other metadata key pass through untouched. Damaged or duplicated markers
//! are refused rather than overwritten, so an uncertain note stays visible
//! and retryable instead of converging to a guess.

use serde_yaml::Value;

use super::{parse, NoteError};

pub(crate) const RECEIPT_BEGIN: &str = "<!-- bob:gmail:receipt:begin v1 -->";
pub(crate) const RECEIPT_END: &str = "<!-- bob:gmail:receipt:end -->";

/// Project receipt state into a saved mail note.
///
/// `Some` timestamp sets `gmail_archived` and the banner; `None` clears both.
/// `Ok(None)` means the bytes already match, so callers write zero bytes.
/// The timestamp must be RFC3339; the banner shows only its calendar date,
/// never task prose or remote text.
pub(crate) fn project(
    contents: &str,
    archived_at: Option<&str>,
) -> Result<Option<String>, NoteError> {
    let parsed = parse::parse(contents)?;
    let stamp = match archived_at {
        Some(stamp) => Some(validated_stamp(stamp)?),
        None => None,
    };
    let front_ok = parse::scalar(&parsed.frontmatter, "gmail_archived")
        .as_deref()
        == archived_at;
    let action = span_action(contents, &parsed, stamp.as_ref())?;
    if front_ok && action == SpanAction::Keep {
        return Ok(None);
    }
    let mut frontmatter = parsed.frontmatter.clone();
    let expected = "frontmatter is not a mapping";
    let Value::Mapping(mapping) = &mut frontmatter else {
        return Err(NoteError::Damaged { reason: expected });
    };
    parse::set_optional_after(
        mapping,
        "gmail_archived",
        stamp
            .as_ref()
            .map(|(stamp, _)| Value::String(stamp.clone())),
        "saved",
    );
    let serialized = serde_yaml::to_string(&frontmatter).map_err(|_| {
        NoteError::Damaged {
            reason: "frontmatter could not be serialized",
        }
    })?;
    let serialized = serialized.strip_prefix("---\n").unwrap_or(&serialized);
    let mut updated = String::from("---\n");
    updated.push_str(serialized);
    if !serialized.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str("---\n");
    let body = &contents[parsed.body_start..];
    let inner_start = parsed.begin_inner_start - parsed.body_start;
    let inner_end = parsed.end_inner_start - parsed.body_start;
    let inner = &body[inner_start..inner_end];
    let replacement = match action {
        SpanAction::Keep => inner.to_string(),
        SpanAction::Insert => {
            let (_, date) = stamp.as_ref().expect("insert needs a stamp");
            format!(
                "{}\n{}\n{}\n{inner}",
                RECEIPT_BEGIN,
                banner_line(date),
                RECEIPT_END
            )
        }
        SpanAction::Replace => {
            let (_, date) = stamp.as_ref().expect("replace needs a stamp");
            let mut owned: Vec<String> =
                inner.split_inclusive('\n').map(str::to_string).collect();
            owned[1] = format!("{}\n", banner_line(date));
            owned.concat()
        }
        SpanAction::Remove => {
            let owned: Vec<String> =
                inner.split_inclusive('\n').map(str::to_string).collect();
            owned[3..].concat()
        }
    };
    updated.push_str(&body[..inner_start]);
    updated.push_str(&replacement);
    updated.push_str(&body[inner_end..]);
    if updated == contents {
        return Ok(None);
    }
    Ok(Some(updated))
}

/// Validate the journal timestamp and split off its calendar date.
fn validated_stamp(stamp: &str) -> Result<(String, String), NoteError> {
    let parsed = chrono::DateTime::parse_from_rfc3339(stamp).map_err(|_| {
        NoteError::Damaged {
            reason: "archive timestamp is invalid",
        }
    })?;
    Ok((stamp.to_string(), parsed.format("%Y-%m-%d").to_string()))
}

fn banner_line(date: &str) -> String {
    format!(
        "> \u{1f5c4}\u{fe0f} Archived by Bob on {date} \u{2014} to move it back, use Move to Inbox in Gmail."
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpanAction {
    Keep,
    Insert,
    Replace,
    Remove,
}

fn span_action(
    contents: &str,
    parsed: &parse::ParsedNote,
    stamp: Option<&(String, String)>,
) -> Result<SpanAction, NoteError> {
    let inner_start = parsed.begin_inner_start;
    let inner_end = parsed.end_inner_start;
    let mut begins = Vec::new();
    let mut ends = Vec::new();
    let mut offset = 0;
    for segment in contents.split_inclusive('\n') {
        let end = offset + segment.len();
        let trimmed = segment.trim();
        let is_marker = trimmed == RECEIPT_BEGIN || trimmed == RECEIPT_END;
        if is_marker && !(offset >= inner_start && end <= inner_end) {
            return Err(NoteError::Damaged {
                reason: "receipt markers are outside the managed block",
            });
        }
        if offset >= inner_start && end <= inner_end {
            if trimmed == RECEIPT_BEGIN {
                begins.push(offset);
            } else if trimmed == RECEIPT_END {
                ends.push(offset);
            }
        }
        offset = end;
    }
    if begins.is_empty() && ends.is_empty() {
        return Ok(if stamp.is_some() {
            SpanAction::Insert
        } else {
            SpanAction::Keep
        });
    }
    if begins.len() != 1 || ends.len() != 1 {
        return Err(NoteError::Damaged {
            reason: "receipt markers are duplicated",
        });
    }
    let inner = &contents[inner_start..inner_end];
    let lines: Vec<&str> = inner.split_inclusive('\n').collect();
    if lines.len() < 3
        || lines[0].trim() != RECEIPT_BEGIN
        || lines[2].trim() != RECEIPT_END
        || begins[0] != inner_start
        || ends[0] != inner_start + lines[0].len() + lines[1].len()
    {
        return Err(NoteError::Damaged {
            reason: "receipt markers are misplaced",
        });
    }
    if !lines[1].trim_start().starts_with("> ") {
        return Err(NoteError::Damaged {
            reason: "receipt banner is damaged",
        });
    }
    let first_card = lines
        .iter()
        .position(|line| line.contains("<!-- bob:gmail:msg"));
    if first_card.is_none_or(|index| index <= 2) {
        return Err(NoteError::Damaged {
            reason: "receipt markers are misplaced",
        });
    }
    match stamp {
        Some((_, date)) => {
            if lines[1].trim_end() == banner_line(date) {
                Ok(SpanAction::Keep)
            } else {
                Ok(SpanAction::Replace)
            }
        }
        None => Ok(SpanAction::Remove),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        layout::{NoteMetadata, RenderedCard, RenderedNote},
        merge, parse,
    };
    use super::*;

    const STAMP: &str = "2026-11-13T08:15:00-05:00";

    const NOTE: &str = "---\nparent: \"[[mail]]\"\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: me@example.com\ngmail_thread_id: aabbccddeeff0011\nsaved: 2026-11-12T09:00:00-05:00\n---\n\n# Subject\n\n[Open in Gmail \u{2197}](https://mail.google.com/)\n\u{b7} Dana \u{2194} you \u{b7} 1 messages\n\n## Notes\n\nUser keeps this.\n\nReply draft stays.\n\n## Conversation\n\n<!-- bob:gmail:begin v1 -->\n<!-- bob:gmail:msg aabbccddeeff0011 - -->\n> [!mail+] **Dana** \u{b7} Fri, Sep 19 \u{b7} 9:46 AM to you\n> body\n\n^m-aabbccddeeff0011\n\n<!-- bob:gmail:end -->\n";

    fn bannered() -> String {
        project(NOTE, Some(STAMP)).unwrap().expect("set receipt")
    }

    #[test]
    fn set_projects_field_and_banner_then_repeats_as_noop() {
        let updated = bannered();
        assert!(updated.contains("gmail_archived: 2026-11-13T08:15:00-05:00"));
        assert!(updated.contains(RECEIPT_BEGIN));
        assert!(updated.contains(
            "> \u{1f5c4}\u{fe0f} Archived by Bob on 2026-11-13 \u{2014} to move it back, use Move to Inbox in Gmail."
        ));
        assert!(updated.contains(RECEIPT_END));
        assert_eq!(project(&updated, Some(STAMP)).unwrap(), None);
    }

    #[test]
    fn clear_is_noop_when_absent_and_removes_both_when_present() {
        assert_eq!(project(NOTE, None).unwrap(), None);
        let updated = bannered();
        let cleared = project(&updated, None).unwrap().expect("clear");
        assert!(!cleared.contains("gmail_archived"));
        assert!(!cleared.contains(RECEIPT_BEGIN));
        assert!(!cleared.contains(RECEIPT_END));
        assert!(cleared.contains("## Notes"));
        assert!(cleared.contains("^m-aabbccddeeff0011"));
        assert_eq!(project(&cleared, None).unwrap(), None);
    }

    #[test]
    fn projection_preserves_everything_besides_the_receipt() {
        let updated = bannered();
        for kept in [
            "parent:",
            "[[mail]]",
            "User keeps this.",
            "Reply draft stays.",
            "## Notes",
            "## Conversation",
            "> body",
            "^m-aabbccddeeff0011",
            "saved: 2026-11-12T09:00:00-05:00",
        ] {
            assert!(updated.contains(kept), "keeps {kept}");
        }
        let parsed = parse::parse(&updated).unwrap();
        assert_eq!(parsed.cards.len(), 1);
        assert_eq!(parsed.cards[0].id, "aabbccddeeff0011");
    }

    #[test]
    fn stale_banner_date_refreshes_without_touching_cards() {
        let updated = bannered();
        let newer = project(&updated, Some("2026-11-14T09:00:00-05:00"))
            .unwrap()
            .expect("refresh date");
        assert!(newer.contains("Archived by Bob on 2026-11-14"));
        assert!(!newer.contains("Archived by Bob on 2026-11-13"));
        assert!(newer.contains("^m-aabbccddeeff0011"));
        assert_eq!(newer.matches(RECEIPT_BEGIN).count(), 1, "still one banner");
    }

    #[test]
    fn rejects_invalid_timestamps_and_damaged_markers() {
        assert!(project(NOTE, Some("not-a-date")).is_err());
        assert!(project(NOTE, Some("2026-11-13")).is_err());
        let duplicate = bannered().replacen(
            RECEIPT_END,
            &format!("{RECEIPT_END}\n{RECEIPT_BEGIN}\n> x\n{RECEIPT_END}"),
            1,
        );
        assert!(project(&duplicate, Some(STAMP)).is_err());
        assert!(project(&duplicate, None).is_err());
        let misplaced = NOTE.replacen(
            "^m-aabbccddeeff0011\n",
            &format!(
                "^m-aabbccddeeff0011\n{RECEIPT_BEGIN}\n> x\n{RECEIPT_END}\n"
            ),
            1,
        );
        assert!(project(&misplaced, Some(STAMP)).is_err());
        assert!(project(&misplaced, None).is_err());
        let outside = bannered().replacen(
            "## Notes",
            &format!("## Notes\n\n{RECEIPT_BEGIN}\n> x\n{RECEIPT_END}"),
            1,
        );
        assert!(project(&outside, Some(STAMP)).is_err());
        let torn = bannered().replacen(RECEIPT_END, "", 1);
        assert!(project(&torn, Some(STAMP)).is_err());
    }

    #[test]
    fn banner_carries_only_the_date_on_hostile_notes() {
        let hostile = NOTE
            .replace("# Subject", "# Evil \"><script>alert(1)</script>")
            .replace("User keeps this.", "[Move back](http://evil.example)");
        let updated = project(&hostile, Some(STAMP))
            .unwrap()
            .expect("hostile set");
        let banner = updated
            .lines()
            .find(|line| line.contains("Archived by Bob"))
            .expect("banner line");
        assert_eq!(
            banner,
            "> \u{1f5c4}\u{fe0f} Archived by Bob on 2026-11-13 \u{2014} to move it back, use Move to Inbox in Gmail."
        );
    }

    #[test]
    fn refresh_paths_preserve_the_receipt() {
        let updated = bannered();
        let turned = parse::update_turn_fields(
            &updated,
            Some("theirs"),
            Some("2026-11-13T08:20:00-05:00"),
        )
        .unwrap()
        .expect("turn refresh writes");
        assert!(turned.contains("gmail_archived: 2026-11-13T08:15:00-05:00"));
        assert!(turned.contains(RECEIPT_BEGIN));
        let fresh = RenderedNote {
            content: String::new(),
            metadata: NoteMetadata {
                account: "me@example.com".to_string(),
                content: "untrusted-email".to_string(),
                gmail_thread_id: "aabbccddeeff0011".to_string(),
                gmail_url: "https://mail.google.com/".to_string(),
                messages: 2,
                saved: "2026-11-13T08:15:00-05:00".to_string(),
                started: "2026-11-12T09:00:00-05:00".to_string(),
                title: "Subject".to_string(),
                turn: None,
                turn_since: None,
                updated: "2026-11-13T08:15:00-05:00".to_string(),
                who: "Dana".to_string(),
            },
            open_gmail: String::new(),
            summary: "\u{b7} Dana \u{2194} you \u{b7} 2 messages".to_string(),
            cards: vec![RenderedCard {
                id: "bbccddeeff001122".to_string(),
                header: "> [!mail-] **Dana** \u{b7} later".to_string(),
                markdown: "<!-- bob:gmail:msg bbccddeeff001122 - -->\n> [!mail-] **Dana** \u{b7} later\n> second\n\n^m-bbccddeeff001122"
                    .to_string(),
            }],
        };
        for rerender in [false, true] {
            let merged = merge::merge(&updated, &fresh, rerender)
                .unwrap()
                .expect("append writes");
            assert_eq!(
                merged.matches(RECEIPT_BEGIN).count(),
                1,
                "one banner after merge rerender={rerender}"
            );
            assert!(
                merged.contains("gmail_archived: 2026-11-13T08:15:00-05:00")
            );
            assert!(merged.contains("^m-bbccddeeff001122"));
            assert!(merged.contains("User keeps this."));
        }
    }
}
